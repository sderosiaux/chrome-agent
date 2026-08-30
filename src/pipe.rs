use std::io::Write as _;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::browser::{self, BrowserOptions};
use crate::cdp::client::CdpClient;
use crate::commands;
use crate::pipe_dispatch::EmulationRecovery;
use crate::session::{self, SessionStore};
use crate::cli::Cli;

/// Run pipe mode: persistent CDP connection, reading JSON commands from stdin.
pub async fn run_pipe(cli: &Cli) -> Result<(), crate::BoxError> {
    let mut session = open_session(cli).await?;
    // A stored device configuration that no longer applies must not fail the session before
    // stdin is read: the recovery state reports it per command, while still admitting the
    // `emulate device`/`emulate reset` that repair it.
    let mut emulation_recovery =
        EmulationRecovery::new(&session.client, &session.store, &cli.browser, &cli.page).await;

    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    // What `macro record` distils at the end of the session. Slim entries, so it stays small.
    let mut history: Vec<crate::macros_record::Observed> = Vec::new();

    loop {
        let Ok(Some(line)) = lines.next_line().await else {
            break;
        };
        let line = line.trim().to_string();
        if line.is_empty() { continue; }

        let cmd: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => { emit(&json!({"ok": false, "error": format!("Invalid JSON: {e}")})); continue; }
        };

        // A recording that cannot be opened refuses the command: running it unrecorded is
        // not what the caller asked for, and the gap would only surface at `replay` time.
        let record_path = cmd.get("_record").and_then(Value::as_str).map(String::from);
        if let Some(ref path) = record_path
            && let Err(e) = commands::record::start_recording(path) {
                emit(&json!({"ok": false, "error": format!("{e}"), "hint": "Check the --record path's directory exists and is writable."}));
                continue;
            }

        // Answered before `dispatch`: `macro` acts on the session's history, not the page,
        // so it can be asked for after the fact.
        if cmd.get("cmd").and_then(Value::as_str) == Some("macro") {
            let answer = crate::macros_cmd::dispatch_pipe(&cmd, &history)
                .unwrap_or_else(|e| json!({"ok": false, "error": e.to_string()}));
            emit(&answer);
            continue;
        }

        let mut response = dispatch_on(&mut session, cli, &cmd, &mut emulation_recovery).await;

        if let Some(ref path) = record_path
            && let Err(e) = commands::record::log_entry(path, &cmd, &response) {
                // The command ran; only the record of it was lost. Failing here would
                // invite a retry of real work.
                response["recording_error"] = json!(format!("{e}"));
            }

        // Slim on purpose (`macros_record::Observed`): kept for the session's whole life, and
        // retains only what the whitelist reads, so it cannot leak the page's text.
        let snapshot = session
            .store
            .browsers
            .get(&cli.browser)
            .and_then(|b| b.pages.get(&cli.page))
            .and_then(|p| p.last_snapshot.clone());
        history.push(crate::macros_record::Observed::read_with_snapshot(
            &cmd,
            &response,
            snapshot.as_deref(),
        ));

        emit(&response);
    }

    let _ = session::save_session(&mut session.store);
    Ok(())
}

/// Everything a session needs to dispatch commands: the two clients, the store, the page.
///
/// One copy of the sixty lines `run_pipe`, `run_replay` and `macros_run` each need. The claim
/// used to be false — the first two carried their own copy and had drifted (a different "no
/// HTTP endpoint" message) — which is the shape of comment that survives precisely because
/// nothing reads it.
pub struct Session {
    pub store: SessionStore,
    pub browser_client: CdpClient,
    pub client: CdpClient,
    pub target_id: String,
    pub policy: crate::run_helpers::ReportPolicy,
}

pub async fn open_session(cli: &Cli) -> Result<Session, crate::BoxError> {
    let mut store = session::load_session()?;
    let want_headless = !cli.headed;
    let requested_proxy =
        browser::normalized_proxy_option(cli.connect.as_deref(), cli.proxy_server.as_deref())?;
    let requested_chrome_args =
        browser::normalized_chrome_args_option(cli.connect.as_deref(), &cli.chrome_args)?;
    let effective_proxy = requested_proxy
        .or_else(|| store.browsers.get(&cli.browser).and_then(|b| b.proxy_server.clone()));
    let effective_chrome_args =
        crate::chrome_args::effective_chrome_args(&store, &cli.browser, &requested_chrome_args);

    let (conn, browser_client) = connect_browser(
        &mut store,
        cli,
        want_headless,
        effective_proxy.clone(),
        effective_chrome_args.clone(),
    )
    .await?;

    let http_endpoint = conn
        .http_endpoint
        .as_deref()
        .ok_or("No HTTP endpoint available. Cannot resolve page WebSocket URL.")?;
    let target_id = {
        let browser_session = session::ensure_browser(
            &mut store,
            &cli.browser,
            &conn.ws_endpoint,
            conn.pid,
            want_headless,
            effective_proxy,
            effective_chrome_args,
        );
        crate::run_helpers::resolve_page_target(&browser_client, browser_session, &cli.page).await?
    };
    let _ = session::save_session(&mut store);

    let page_ws = browser::get_page_ws_url(http_endpoint, &target_id).await?;
    let client = CdpClient::connect(&page_ws).await?;
    client.set_call_timeout(std::time::Duration::from_secs(cli.timeout));
    client.enable("Page").await?;
    commands::console::inject(&client).await;
    if cli.stealth {
        crate::setup::apply_stealth(&client).await;
    } else {
        client.enable("Runtime").await?;
    }
    let dialog_policy = crate::setup::DialogPolicy::parse(&cli.dialog)?;
    client.spawn_dialog_handler(dialog_policy, cli.dialog_text.clone());
    let policy = report_policy(cli)?;
    Ok(Session { store, browser_client, client, target_id, policy })
}

/// Dispatch one command on an open session. `pub` for `macros_run`: a macro step IS a pipe
/// command and must execute identically.
pub async fn dispatch_on(
    session: &mut Session,
    cli: &Cli,
    cmd: &Value,
    emulation_recovery: &mut EmulationRecovery,
) -> Value {
    if let Some(response) = emulation_recovery.refusal_for(cmd) {
        return response;
    }
    let response = crate::pipe_dispatch::dispatch_single(
        &session.client,
        &session.browser_client,
        &mut session.store,
        &cli.browser,
        &cli.page,
        &session.target_id,
        cli.timeout,
        cli.max_depth,
        session.policy,
        cmd,
        emulation_recovery,
    )
    .await;
    emulation_recovery.update_after(cmd, &response);
    response
}

pub async fn run_replay(
    cli: &Cli, file: &str, vars: Option<&[String]>,
) -> Result<(), crate::BoxError> {
    let content = std::fs::read_to_string(file)
        .map_err(|e| format!("Cannot read replay file '{file}': {e}"))?;

    let replacements: Vec<(&str, &str)> = vars
        .unwrap_or(&[]).iter().filter_map(|pair| pair.split_once('=')).collect();

    let mut session = open_session(cli).await?;
    // Same recovery state as a live pipe: a recording may begin with the `emulate`
    // device/reset command that repairs its stored configuration.
    let mut emulation_recovery =
        EmulationRecovery::new(&session.client, &session.store, &cli.browser, &cli.page).await;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        let mut resolved = line.to_string();
        for (key, val) in &replacements {
            resolved = resolved.replace(&format!("{{{{{key}}}}}"), val);
        }

        let parsed: Value = serde_json::from_str(&resolved)
            .map_err(|e| format!("Invalid JSON in replay: {e}"))?;

        let cmd = if parsed.get("cmd").is_some_and(Value::is_object) && parsed.get("response").is_some() {
            parsed.get("cmd").cloned().unwrap_or_default()
        } else { parsed };

        let response = dispatch_on(&mut session, cli, &cmd, &mut emulation_recovery).await;
        emit(&response);
    }

    let _ = session::save_session(&mut session.store);
    Ok(())
}

// --- Helpers ---

/// The global reporting flags, parsed once for the session rather than per command.
fn report_policy(cli: &Cli) -> Result<crate::run_helpers::ReportPolicy, crate::BoxError> {
    Ok(crate::run_helpers::ReportPolicy {
        changes: cli.verdict == "auto",
        budget: cli.budget,
        on_intercept: crate::hit_test::OnIntercept::parse(&cli.on_intercept)?,
    })
}

fn emit(value: &Value) {
    let line = serde_json::to_string(value).unwrap_or_default();
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    let _ = writeln!(handle, "{line}");
    let _ = handle.flush();
}

async fn connect_browser(
    store: &mut SessionStore,
    cli: &Cli,
    want_headless: bool,
    effective_proxy: Option<String>,
    effective_chrome_args: Vec<String>,
) -> Result<(browser::BrowserConnection, CdpClient), crate::BoxError> {
    if let Some(existing) = store.browsers.get(&cli.browser) {
        let mode_matches = existing.headless == want_headless;
        let ws = &existing.ws_endpoint;
        let http = browser::extract_http_from_ws(ws);

        if mode_matches {
            if let Ok(client) = CdpClient::connect(ws).await {
                session::ensure_proxy_compatible(existing, effective_proxy.as_deref())?;
                session::ensure_chrome_args_compatible(existing, &effective_chrome_args)?;
                let conn = browser::BrowserConnection {
                    ws_endpoint: ws.clone(), http_endpoint: Some(http), pid: existing.pid,
                };
                client.set_call_timeout(std::time::Duration::from_secs(cli.timeout));
                return Ok((conn, client));
            }
        } else if let Some(pid) = existing.pid {
            // The mode changed, so this browser is replaced. Through the same guarded helper
            // every other kill site uses: `kill_pid` refuses a pid that is no longer a
            // browser (a recycled one belongs to something else), and the wait is what stops
            // the relaunch reconnecting to the Chrome it just signalled.
            browser::kill_and_await_exit(&cli.browser, pid)?;
        }
        store.browsers.remove(&cli.browser);
    }

    let opts = BrowserOptions {
        name: cli.browser.clone(), headless: want_headless,
        ignore_https_errors: cli.ignore_https_errors, stealth: cli.stealth,
        connect: cli.connect.clone(), proxy_server: effective_proxy,
        copy_cookies: cli.copy_cookies,
        chrome_args: effective_chrome_args,
    };
    let conn = browser::resolve_browser(&opts).await?;
    let client = CdpClient::connect(&conn.ws_endpoint).await?;
    // Browser-level Target.* calls obey the caller's --timeout like page calls do.
    client.set_call_timeout(std::time::Duration::from_secs(cli.timeout));
    Ok((conn, client))
}
