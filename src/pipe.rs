use std::io::Write as _;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::browser::{self, BrowserOptions};
use crate::cdp::client::CdpClient;
use crate::commands;
use crate::pipe_dispatch::{
    dispatch_back, dispatch_batch, dispatch_check, dispatch_click,
    dispatch_console, dispatch_dblclick, dispatch_diff, dispatch_drag,
    dispatch_eval, dispatch_extract, dispatch_fill, dispatch_fill_and_submit,
    dispatch_fill_form, dispatch_forward, dispatch_frame, dispatch_goto,
    dispatch_history, dispatch_hover, dispatch_inspect,
    dispatch_navigate_and_read, dispatch_network, dispatch_press,
    dispatch_read, dispatch_screenshot, dispatch_scroll, dispatch_select,
    dispatch_tabs, dispatch_text, dispatch_type, dispatch_upload,
    dispatch_wait,
};
use crate::run_helpers::error_hint;
use crate::session::{self, SessionStore};
use crate::cli::Cli;

/// Run pipe mode: persistent CDP connection, reading JSON commands from stdin.
pub async fn run_pipe(cli: &Cli) -> Result<(), crate::BoxError> {
    let mut store = session::load_session()?;
    let want_headless = !cli.headed;

    let (conn, browser_client) = connect_browser(&mut store, cli, want_headless).await?;

    let http_endpoint = conn.http_endpoint.as_deref().ok_or(
        "No HTTP endpoint available. Cannot resolve page WebSocket URL.",
    )?;

    let target_id = {
        let browser_session = session::ensure_browser(
            &mut store, &cli.browser, &conn.ws_endpoint, conn.pid, want_headless,
        );
        crate::run_helpers::resolve_page_target(&browser_client, browser_session, &cli.page).await?
    };
    let _ = session::save_session(&mut store);

    let page_ws = browser::get_page_ws_url(http_endpoint, &target_id).await?;
    let client = CdpClient::connect(&page_ws).await?;
    client.enable("Page").await?;

    // Console interceptor (stealth-safe)
    commands::console::inject(&client).await;

    if cli.stealth {
        crate::setup::apply_stealth(&client).await;
    } else {
        client.enable("Runtime").await?;
    }

    // Main loop: read JSON commands from stdin
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();

    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim().to_string();
        if line.is_empty() { continue; }

        let cmd: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => { emit(&json!({"ok": false, "error": format!("Invalid JSON: {e}")})); continue; }
        };

        let record_path = cmd.get("_record").and_then(Value::as_str).map(String::from);
        if let Some(ref path) = record_path {
            let _ = commands::record::start_recording(path);
        }

        let response = dispatch(
            &client, &browser_client, &mut store,
            &cli.browser, &cli.page, &target_id, cli.timeout, cli.max_depth, &cmd,
        ).await;

        if let Some(ref path) = record_path {
            let _ = commands::record::log_entry(path, &cmd, &response);
        }

        emit(&response);
    }

    let _ = session::save_session(&mut store);
    Ok(())
}

/// Replay a recorded session file, optionally substituting variables.
pub async fn run_replay(
    cli: &Cli, file: &str, vars: Option<&[String]>,
) -> Result<(), crate::BoxError> {
    let content = std::fs::read_to_string(file)
        .map_err(|e| format!("Cannot read replay file '{file}': {e}"))?;

    let replacements: Vec<(&str, &str)> = vars
        .unwrap_or(&[]).iter().filter_map(|pair| pair.split_once('=')).collect();

    let mut store = session::load_session()?;
    let want_headless = !cli.headed;
    let (conn, browser_client) = connect_browser(&mut store, cli, want_headless).await?;

    let http_endpoint = conn.http_endpoint.as_deref().ok_or("No HTTP endpoint available.")?;
    let target_id = {
        let browser_session = session::ensure_browser(
            &mut store, &cli.browser, &conn.ws_endpoint, conn.pid, want_headless,
        );
        crate::run_helpers::resolve_page_target(&browser_client, browser_session, &cli.page).await?
    };
    let _ = session::save_session(&mut store);

    let page_ws = browser::get_page_ws_url(http_endpoint, &target_id).await?;
    let client = CdpClient::connect(&page_ws).await?;
    client.enable("Page").await?;
    commands::console::inject(&client).await;
    if cli.stealth { crate::setup::apply_stealth(&client).await; }
    else { client.enable("Runtime").await?; }

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

        let response = dispatch(
            &client, &browser_client, &mut store,
            &cli.browser, &cli.page, &target_id, cli.timeout, cli.max_depth, &cmd,
        ).await;

        emit(&response);
    }

    let _ = session::save_session(&mut store);
    Ok(())
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
async fn dispatch(
    client: &CdpClient, browser_client: &CdpClient, store: &mut SessionStore,
    browser_name: &str, page_name: &str, target_id: &str,
    timeout: u64, global_max_depth: Option<usize>, cmd: &Value,
) -> Value {
    let cmd_name = cmd.get("cmd").and_then(Value::as_str).unwrap_or("");

    let result: Result<Value, crate::BoxError> = match cmd_name {
        "goto" => dispatch_goto(client, store, browser_name, page_name, target_id, timeout, global_max_depth, cmd).await,
        "click" => dispatch_click(client, store, browser_name, page_name, target_id, global_max_depth, cmd).await,
        "fill" => dispatch_fill(client, store, browser_name, page_name, target_id, global_max_depth, cmd).await,
        "inspect" => dispatch_inspect(client, store, browser_name, page_name, target_id, cmd).await,
        "eval" => dispatch_eval(client, cmd).await,
        "read" => dispatch_read(client, cmd).await,
        "text" => dispatch_text(client, store, browser_name, page_name, cmd).await,
        "screenshot" => dispatch_screenshot(client).await,
        "wait" => dispatch_wait(client, timeout, cmd).await,
        "back" => dispatch_back(client).await,
        "forward" => dispatch_forward(client).await,
        "scroll" => dispatch_scroll(client, store, browser_name, page_name, cmd).await,
        "type" => dispatch_type(client, cmd).await,
        "press" => dispatch_press(client, cmd).await,
        "fill-form" | "fill_form" | "fillform" => dispatch_fill_form(client, store, browser_name, page_name, target_id, global_max_depth, cmd).await,
        "dblclick" => dispatch_dblclick(client, store, browser_name, page_name, target_id, global_max_depth, cmd).await,
        "select" => dispatch_select(client, store, browser_name, page_name, target_id, global_max_depth, cmd).await,
        "check" => dispatch_check(client, store, browser_name, page_name, cmd).await,
        "uncheck" => {
            let mut cmd_with_desired = cmd.clone();
            if let Some(m) = cmd_with_desired.as_object_mut() {
                m.insert("desired".into(), Value::Bool(false));
            }
            dispatch_check(client, store, browser_name, page_name, &cmd_with_desired).await
        }
        "upload" => dispatch_upload(client, store, browser_name, page_name, cmd).await,
        "drag" => dispatch_drag(client, store, browser_name, page_name, cmd).await,
        "hover" => dispatch_hover(client, store, browser_name, page_name, cmd).await,
        "tabs" => dispatch_tabs(browser_client, store).await,
        "network" => dispatch_network(client, cmd).await,
        "console" => dispatch_console(client, cmd).await,
        "diff" => dispatch_diff(client, store, browser_name, page_name, target_id).await,
        "extract" => dispatch_extract(client, cmd).await,
        "navigate_and_read" | "navigate-and-read" => dispatch_navigate_and_read(client, store, browser_name, page_name, target_id, timeout, cmd).await,
        "fill_and_submit" | "fill-and-submit" => dispatch_fill_and_submit(client, timeout, cmd).await,
        "history" => dispatch_history(cmd),
        "frame" => dispatch_frame(client, cmd).await,
        "batch" => dispatch_batch(client, browser_client, store, browser_name, page_name, target_id, timeout, global_max_depth, cmd).await,
        "" => Err("Missing \"cmd\" field".into()),
        other => Err(format!("Unknown command: {other}").into()),
    };

    match result {
        Ok(v) => v,
        Err(e) => {
            let msg = e.to_string();
            let mut obj = json!({"ok": false, "error": msg});
            if let Some(h) = error_hint(&msg) { obj["hint"] = json!(h); }
            obj
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn emit(value: &Value) {
    let line = serde_json::to_string(value).unwrap_or_default();
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    let _ = writeln!(handle, "{line}");
    let _ = handle.flush();
}

async fn connect_browser(
    store: &mut SessionStore, cli: &Cli, want_headless: bool,
) -> Result<(browser::BrowserConnection, CdpClient), crate::BoxError> {
    if let Some(existing) = store.browsers.get(&cli.browser) {
        let mode_matches = existing.headless == want_headless;
        let ws = &existing.ws_endpoint;
        let http = browser::extract_http_from_ws(ws);

        if mode_matches {
            if let Ok(client) = CdpClient::connect(ws).await {
                let conn = browser::BrowserConnection {
                    ws_endpoint: ws.clone(), http_endpoint: Some(http), pid: existing.pid,
                };
                return Ok((conn, client));
            }
        } else if let Some(pid) = existing.pid {
            #[cfg(unix)]
            {
                let _ = std::process::Command::new("kill")
                    .arg(pid.to_string())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            }
        }
        store.browsers.remove(&cli.browser);
    }

    let opts = BrowserOptions {
        name: cli.browser.clone(), headless: want_headless,
        ignore_https_errors: cli.ignore_https_errors, stealth: cli.stealth,
        connect: cli.connect.clone(), copy_cookies: cli.copy_cookies,
    };
    let conn = browser::resolve_browser(&opts).await?;
    let client = CdpClient::connect(&conn.ws_endpoint).await?;
    Ok((conn, client))
}
