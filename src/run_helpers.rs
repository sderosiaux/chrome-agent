use std::collections::HashMap;

use serde_json::json;

use crate::cdp::client::CdpClient;
use crate::commands;
use crate::element_ref::ElementRef;
use crate::session::{self, BrowserSession, SessionStore};

// One kill path for `run.rs`, `main.rs` and `orphans.rs`; a second bypasses the pid-reuse guard.
pub use crate::kill::{KillOutcome, close_message, kill_pid};

/// Connect to a page-level CDP endpoint, 8 attempts. Enables Page, injects the console
/// interceptor, and applies stealth patches or enables Runtime.
pub async fn connect_page(
    http_endpoint: &str,
    target_id: &str,
    stealth: bool,
) -> Result<CdpClient, crate::BoxError> {
    let mut last_err = String::new();
    for attempt in 0..8u32 {
        match crate::browser::get_page_ws_url(http_endpoint, target_id).await {
            Ok(page_ws) => match CdpClient::connect(&page_ws).await {
                Ok(client) => {
                    // Cheap liveness check: a connected socket is not a working session.
                    if let Err(e) = client
                        .call::<_, serde_json::Value>(
                            "Runtime.evaluate",
                            json!({"expression": "1", "returnByValue": true}),
                        )
                        .await
                    {
                        last_err = format!("Connection verify failed: {e}");
                        drop(client);
                        if attempt < 7 {
                            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        }
                        continue;
                    }
                    if let Err(e) = client.enable("Page").await {
                        last_err = format!("Page.enable failed: {e}");
                        drop(client);
                        if attempt < 7 {
                            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                        }
                        continue;
                    }
                    commands::console::inject(&client).await;
                    if stealth {
                        crate::setup::apply_stealth(&client).await;
                    } else {
                        let _ = client.enable("Runtime").await;
                    }
                    return Ok(client);
                }
                Err(e) => last_err = e.to_string(),
            },
            Err(e) => last_err = e.to_string(),
        }
        if attempt < 7 {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        }
    }
    Err(format!("Failed to connect to page after 8 attempts: {last_err}").into())
}

/// What an action reports about the page once it has run.
pub struct ActionReport {
    /// `--inspect`: the whole tree.
    pub inspect: bool,
    /// `--verdict auto`: what changed since the last snapshot of this page.
    pub changes: bool,
    /// Character cap on the change report. 0 removes it.
    pub budget: usize,
    pub max_depth: Option<usize>,
}

/// Reporting policy taken from the global flags, before `cli.command` is consumed.
/// `on_intercept` rides here rather than in a parallel parameter: this struct already
/// threads through the CLI, pipe and batch paths, so it costs no dispatcher signature.
#[derive(Clone, Copy)]
pub struct ReportPolicy {
    pub changes: bool,
    pub budget: usize,
    pub on_intercept: crate::hit_test::OnIntercept,
}

impl ReportPolicy {
    /// Build the per-action report from the policy plus that command's own flags.
    pub const fn for_action(self, inspect: bool, max_depth: Option<usize>) -> ActionReport {
        ActionReport {
            inspect,
            changes: self.changes,
            budget: self.budget,
            max_depth,
        }
    }
}

/// What the four read-back verbs put on their responses. Re-exported so a caller writes
/// `run_helpers::fill_value_report` next to the `output_action_with` that ships it.
pub use crate::read_back::{bulk_fill_report, check_report, fill_value_report, select_report};

/// The node an action is about to touch: `uid`, plus best-effort `role`/`name` off the
/// `DOM.describeNode` the uid already needs. Returns fields to merge into the response.
///
/// Must be called BEFORE the action, or a detached element makes it describe another page.
pub async fn target_details(
    client: &CdpClient,
    selector: Option<&str>,
    uid: Option<&str>,
) -> Option<serde_json::Value> {
    match (selector, uid) {
        (Some(sel), _) => {
            let handle = crate::hit_test::resolve_selector(client, sel).await.ok()?;
            let mut out = json!({"uid": handle.uid?});
            if let Some(role) = handle.role {
                out["role"] = json!(role);
            }
            if let Some(name) = handle.name {
                out["name"] = json!(name);
            }
            Some(out)
        }
        // Echoed so `uid` means the same thing whichever way the caller aimed.
        (None, Some(uid)) => Some(json!({"uid": uid})),
        (None, None) => None,
    }
}

/// Copy an optional field set into a response object at the top level.
///
/// The ONE merge loop. It existed three times — inside a `merge_details` the CLI used, again
/// inside `output_action_with`, and a third time as `pipe_dispatch::merge_into` — for one
/// four-line body that decides what an action's response says about the node it touched.
pub fn merge_into(obj: &mut serde_json::Value, details: Option<&serde_json::Value>) {
    if let (Some(target), Some(fields)) = (
        obj.as_object_mut(),
        details.and_then(serde_json::Value::as_object),
    ) {
        for (key, value) in fields {
            target.insert(key.clone(), value.clone());
        }
    }
}

/// Report what a command did to the page and persist the new baseline, plus whatever the
/// command itself observed (the value a fill left behind, the window a check looked through).
///
/// By default an action answers "what changed", saving a second call; `--verdict off` trades
/// that for latency. `details` is merged at the top level, so a response built here and the same
/// response built by a `pipe_dispatch::dispatch_*` have one shape — which is what lets `run.rs`
/// call the dispatcher and hand its object straight to this.
pub async fn output_action_with(
    ctx: &mut crate::page_ctx::PageCtx<'_>,
    msg: String,
    report: &ActionReport,
    json_mode: bool,
    details: Option<serde_json::Value>,
) -> Result<(), crate::BoxError> {
    let client = ctx.client;
    let mut obj = json!({"ok": true, "message": msg});
    merge_into(&mut obj, details.as_ref());
    let mut trailer = String::new();
    // The response always names WHY it has nothing to report.
    let mut observation = if report.changes {
        crate::verdict::Observation::NoBaseline
    } else {
        crate::verdict::Observation::ReportingDisabled
    };

    if report.inspect || report.changes {
        // 100 ms quiet window, 1 s ceiling — not a fixed guess.
        crate::snapshot::settle(client, 100, 1000).await;
        // One reading, two renderings (`snapshot::Views`): the baseline is always full depth,
        // `--inspect --max-depth` only narrows what is shown.
        //
        // A read that fails is NOT an action that failed — `ok:false` after a delivered click
        // invites a second click — hence the early return with a `read_failed` verdict.
        let display_depth = if report.inspect {
            report.max_depth
        } else {
            None
        };
        let Ok(views) = commands::inspect::views(client, false, display_depth, None, None).await
        else {
            let assessment = crate::pipe_report::attach_verdict_for(
                client,
                &mut obj,
                crate::verdict::Observation::ReadFailed,
            );
            if json_mode {
                json_output(&obj);
            } else {
                print_action(&msg, "", &obj, assessment);
            }
            return Ok(());
        };
        let shown = report.inspect.then(|| views.shown().to_string());
        let snapshot = views.full;

        if report.changes {
            let previous = ctx
                .store
                .browsers
                .get(ctx.browser)
                .and_then(|b| b.pages.get(ctx.page))
                .map(|p| {
                    (
                        p.last_snapshot.clone(),
                        p.last_snapshot_frame
                            .clone()
                            .zip(p.last_snapshot_loader.clone()),
                    )
                });
            if let Some((Some(old_text), stored)) = previous {
                let identity = commands::diff::Identity::from_loader(
                    stored.as_ref().map(|(f, l)| (f.as_str(), l.as_str())),
                    snapshot
                        .identity
                        .as_ref()
                        .map(|(f, l)| (f.as_str(), l.as_str())),
                );
                let cmp = commands::diff::compare(identity, &old_text, &snapshot.text);
                let body = if report.budget == 0 {
                    cmp.text.clone()
                } else {
                    crate::truncate::truncate_str(
                        cmp.text.trim_end(),
                        report.budget,
                        "\n… truncated, run `inspect` for the rest",
                    )
                    .into_owned()
                };
                obj["changed"] = json!({
                    "added": cmp.added,
                    "removed": cmp.removed,
                    "changed": cmp.changed,
                    "unchanged": cmp.unchanged,
                    "moved": cmp.moved,
                    "anonymous": cmp.anonymous,
                    "document_changed": cmp.document_changed,
                    "identity_known": cmp.identity_known,
                });
                obj["delta"] = json!(body);
                // Must run before the verdict is settled; it feeds it. `Box::pin` keeps the
                // future off `run`'s single stack frame (clippy `large_stack_frames`).
                let values_lost = Box::pin(crate::pipe_report::attach_values_lost(
                    client,
                    &snapshot.uid_map,
                    &cmp.values_lost,
                    &mut obj,
                ))
                .await;
                observation = crate::verdict::Observation::Compared {
                    document_changed: cmp.document_changed,
                    identity_known: cmp.identity_known,
                    edits: cmp.added + cmp.removed + cmp.changed,
                    moved: cmp.moved,
                    focus_moved: cmp.focus_from.is_some() || cmp.focus_to.is_some(),
                    values_lost,
                };
                if cmp.focus_from.is_some() || cmp.focus_to.is_some() {
                    obj["focus"] = json!({"from": cmp.focus_from, "to": cmp.focus_to});
                }
                if let Some(hint) = cmp.hint {
                    obj["hint"] = json!(hint);
                }
                trailer = body;
            }
        }

        if let Some(shown) = shown {
            // The caller's depth, for display only; the baseline above stays full.
            obj["snapshot"] = json!(&shown);
            trailer = shown;
        }

        ctx.store_snapshot(snapshot);
    }

    let assessment = crate::pipe_report::attach_verdict_for(client, &mut obj, observation);

    if json_mode {
        json_output(&obj);
    } else {
        print_action(&msg, &trailer, &obj, assessment);
    }
    Ok(())
}

/// The text-mode report for one action: its message, the delta, and what the response
/// measured (`src/render.rs`). One function for both exits, normal and failed-read, so they
/// cannot print different shapes for the same response.
fn print_action(
    msg: &str,
    trailer: &str,
    obj: &serde_json::Value,
    assessment: crate::verdict::Assessment,
) {
    out_line!("{msg}");
    if !trailer.is_empty() {
        out_line!("{}", trailer.trim_end());
    }
    for line in crate::render::action_lines(obj, assessment, crate::render::Paint::for_stdout()) {
        out_line!("{line}");
    }
}

/// Write the verdict, its reason, and `next`. The verdict's hint gets its own field so it
/// never overwrites the more specific `hint` the diff or the action may already have written.
pub fn attach_verdict(obj: &mut serde_json::Value, assessment: crate::verdict::Assessment) {
    obj["verdict"] = json!(assessment.verdict.as_str());
    obj["verdict_reason"] = json!(assessment.reason);
    // One token from a closed set of six, so an agent branches without parsing prose.
    obj["next"] = json!(crate::verdict::next_for(assessment).as_str());
    if let Some(hint) = crate::verdict::hint_for(assessment) {
        // `or_insert`: an action's own hint is more specific and must not be replaced.
        if let Some(map) = obj.as_object_mut() {
            map.entry("verdict_hint").or_insert_with(|| json!(hint));
        }
    }
}

pub async fn output_goto(
    ctx: &mut crate::page_ctx::PageCtx<'_>,
    url: &str,
    title: &str,
    landed: Option<&crate::landing::Landing>,
    inspect: bool,
    max_depth: Option<usize>,
    json_mode: bool,
) -> Result<(), crate::BoxError> {
    let (client, browser_name) = (ctx.client, ctx.browser);
    if !ctx.store.browsers.contains_key(browser_name) {
        return Err(format!("Browser session '{browser_name}' not found in session store").into());
    }
    // `backendNodeId` counters overlap between documents, so a stale uid can resolve to an
    // unrelated element. The `if inspect` branch below refills the map.
    ctx.clear_uid_map();
    // One reading for both output modes. `--max-depth` decides what is PRINTED, never what is
    // stored: a truncated baseline makes the next `diff` report every node past the limit added.
    let views = if inspect {
        Some(commands::inspect::views(client, false, max_depth, None, None).await?)
    } else {
        None
    };
    let shown = views.as_ref().map(|v| v.shown().to_string());
    if let Some(views) = views {
        ctx.store_snapshot(views.full);
    }
    if json_mode {
        let mut obj = json!({"ok": true, "url": url, "title": title});
        if let Some(landing) = landed {
            landing.attach(&mut obj, browser_name);
        }
        if let Some(shown) = shown {
            obj["snapshot"] = json!(shown);
        }
        json_output(&obj);
    } else {
        if title.is_empty() {
            out_line!("{url}");
        } else {
            out_line!("{url} — {title}");
        }
        if let Some(line) = landed.and_then(|landing| landing.text_line(browser_name)) {
            out_line!("{line}");
        }
        if let Some(shown) = shown {
            out_line!("{shown}");
        }
    }
    Ok(())
}

/// The one writer of a `--json` response. Every `--json` line the CLI prints goes through here.
///
/// Serialization of a `Value` can still fail (a non-finite float reaches `serde_json` as an
/// error, not as a panic), and `unwrap_or_default()` answered that with an EMPTY line — which
/// breaks the `--json` contract in the one direction an agent cannot recover from: it reads as
/// "no response" rather than as a failure. The fallback names what happened instead.
pub fn json_output(value: &serde_json::Value) {
    out_line!("{}", json_line(value));
}

/// The line `json_output` prints, split out so the failure branch can be proven by a test
/// instead of merely asserted — nothing about `out_line!` is observable from one.
///
/// Generic over `Serialize` rather than taking a `Value`: a `Value` is very nearly
/// infallible to serialize, so a test could not reach the branch through one.
pub fn json_line<T: serde::Serialize + ?Sized>(value: &T) -> String {
    match serde_json::to_string(value) {
        Ok(text) => text,
        // Built by hand rather than through `json!` + `to_string`: the fallback for a failed
        // serialization may not itself depend on serialization succeeding. `Value::String`'s
        // `Display` is infallible and escapes the message, so the line is always valid JSON.
        Err(e) => {
            let escaped =
                serde_json::Value::String(format!("response could not be serialized: {e}"));
            format!("{{\"ok\":false,\"error\":{escaped}}}")
        }
    }
}

/// A CLI `batch` that `--stop-on-error` cut short — the carrier for exit code 1.
///
/// Like [`crate::commands::assert::NotHeld`] it travels the error channel without being an
/// error, so no caller threads a second return type through `run`. Unlike `NotHeld` it prints
/// nothing: the arm already wrote the batch's one response (JSON or text), and a second line
/// here would put two responses on stdout for one invocation. `main` recognises it before its
/// generic handler and exits 1 silently.
#[derive(Debug)]
pub struct BatchStopped;

impl std::fmt::Display for BatchStopped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("batch stopped at the first failed command")
    }
}

impl std::error::Error for BatchStopped {}

/// The text rendering of a batch response, for the same reason every other arm has one:
/// `--json` is what asks for JSON, and text mode was printing the raw object.
///
/// One line per entry. The entry's own sentence when it has one (`message`, or `error` for a
/// failure); otherwise the whole object, because a `text`/`snapshot`/`result` payload has no
/// one-line form and dropping it would lose what the caller asked for.
pub fn print_batch_text(out: &serde_json::Value, stopped_at: Option<u64>) {
    for line in batch_text_lines(out, stopped_at) {
        out_line!("{line}");
    }
}

/// The pure half of [`print_batch_text`], so a test can assert on the lines without a terminal
/// — the same split [`crate::render::action_lines`] uses.
#[must_use]
pub fn batch_text_lines(out: &serde_json::Value, stopped_at: Option<u64>) -> Vec<String> {
    let results = out.get("results").and_then(serde_json::Value::as_array);
    let mut lines: Vec<String> = results
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(index, entry)| {
            let ok = entry
                .get("ok")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let status = if ok { "ok" } else { "error" };
            let summary = entry
                .get("message")
                .or_else(|| entry.get("error"))
                .and_then(serde_json::Value::as_str)
                .map_or_else(|| entry.to_string(), str::to_string);
            format!("[{index}] {status} — {summary}")
        })
        .collect();
    if let Some(index) = stopped_at {
        let skipped = out
            .get("skipped")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        lines.push(format!("stopped at command {index} — {skipped} skipped"));
    }
    lines
}

/// The error-recovery hints, re-exported for `main`, `pipe` and `pipe_dispatch`.
pub use crate::hints::error_hint;

pub fn get_uid_map(
    store: &SessionStore,
    browser_name: &str,
    page_name: &str,
) -> HashMap<String, ElementRef> {
    store
        .browsers
        .get(browser_name)
        .and_then(|b| b.pages.get(page_name))
        .map(|p| p.uid_map.clone())
        .unwrap_or_default()
}

/// Resolve the page target id: use existing from session, or pick first page, or create one.
pub async fn resolve_page_target(
    client: &CdpClient,
    browser_session: &mut BrowserSession,
    page_name: &str,
) -> Result<String, crate::BoxError> {
    if let Some(page) = browser_session.pages.get(page_name) {
        return Ok(page.target_id.clone());
    }

    if page_name == "default" {
        let result: crate::cdp::types::GetTargetsResult = client
            .call("Target.getTargets", serde_json::json!({}))
            .await?;

        let claimed_targets: std::collections::HashSet<&str> = browser_session
            .pages
            .values()
            .map(|p| p.target_id.as_str())
            .collect();

        let available = result
            .target_infos
            .iter()
            .find(|t| t.target_type == "page" && !claimed_targets.contains(t.target_id.as_str()));

        if let Some(target) = available {
            let target_id = target.target_id.clone();
            session::ensure_page(browser_session, page_name, &target_id);
            return Ok(target_id);
        }
    }

    let create_result: crate::cdp::types::CreateTargetResult = client
        .call(
            "Target.createTarget",
            crate::cdp::types::CreateTargetParams {
                url: "about:blank".into(),
                width: None,
                height: None,
                new_window: None,
                background: None,
            },
        )
        .await?;

    let target_id = create_result.target_id;
    session::ensure_page(browser_session, page_name, &target_id);
    Ok(target_id)
}

pub fn cmd_status(json_mode: bool) -> Result<(), crate::BoxError> {
    let store = session::load_session()?;
    let daemon_alive = session::daemon_socket_exists();
    // Reported beside the sessions: a browser the registry lost is invisible exactly where
    // someone goes to look for one.
    let orphans = crate::orphans::scan(&store);

    if json_mode {
        let browsers: Vec<serde_json::Value> = store
            .browsers
            .iter()
            .map(|(name, b)| {
                json!({
                    "name": name,
                    "pid": b.pid,
                    "headless": b.headless,
                    "pages": b.pages.len(),
                    "ws": b.ws_endpoint,
                })
            })
            .collect();
        // `null` where the process table could not be read; an empty list is a false all-clear.
        let orphan_json = orphans.as_ref().map(|found| {
            found
                .iter()
                .map(|o| json!({"name": o.name, "pid": o.pid}))
                .collect::<Vec<_>>()
        });
        json_output(&json!({
            "ok": true,
            "browsers": browsers,
            "orphans": orphan_json,
            "daemon": if daemon_alive { "running" } else { "stopped" },
        }));
    } else {
        if store.browsers.is_empty() {
            out_line!("No active browser sessions.");
        } else {
            for (name, browser) in &store.browsers {
                let status = if let Some(pid) = browser.pid {
                    format!("pid={pid}")
                } else {
                    "external".into()
                };
                let mode = if browser.headless {
                    "headless"
                } else {
                    "headed"
                };
                out_line!(
                    "browser={name}  {status}  {mode}  pages={}  ws={}",
                    browser.pages.len(),
                    browser.ws_endpoint
                );
            }
        }

        for orphan in orphans.iter().flatten() {
            out_line!(
                "orphan={}  pid={}  no session entry — close with `chrome-agent close --orphans`",
                orphan.name,
                orphan.pid
            );
        }

        out_line!(
            "daemon: {}",
            if daemon_alive { "running" } else { "stopped" }
        );
    }

    Ok(())
}

/// Pure, so the stop decision is unit-testable without a socket.
#[cfg(any(unix, test))]
const fn stop_message(reached_daemon: bool) -> &'static str {
    if reached_daemon {
        "Daemon stopped."
    } else {
        "Daemon is not running."
    }
}

pub async fn cmd_stop(json_mode: bool) -> Result<(), crate::BoxError> {
    #[cfg(not(unix))]
    {
        let msg = "Daemon is not supported on this platform.";
        if json_mode {
            json_output(&json!({"ok": true, "message": msg}));
        } else {
            out_line!("{msg}");
        }
        return Ok(());
    }

    #[cfg(unix)]
    {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::UnixStream;

        let socket_path = session::daemon_socket_path()?;

        // A missing socket and a stale one (ECONNREFUSED) both mean "not running": remove the
        // stale socket rather than let the connect error escape via `?`.
        let stream = if socket_path.exists() {
            match UnixStream::connect(&socket_path).await {
                Ok(stream) => Some(stream),
                Err(_) => {
                    let _ = std::fs::remove_file(&socket_path);
                    None
                }
            }
        } else {
            None
        };

        let Some(mut stream) = stream else {
            let msg = stop_message(false);
            if json_mode {
                json_output(&json!({"ok": true, "message": msg}));
            } else {
                out_line!("{msg}");
            }
            return Ok(());
        };

        stream.write_all(b"{\"command\":\"stop\"}\n").await?;
        stream.shutdown().await?;

        let mut buf = Vec::new();
        let _ = stream.read_to_end(&mut buf).await;

        let msg = stop_message(true);
        if json_mode {
            json_output(&json!({"ok": true, "message": msg}));
        } else {
            out_line!("{msg}");
        }
        Ok(())
    } // #[cfg(unix)]
}

/// Whether this command owns the browser named by `--browser`, and may take it down when
/// interrupted. `--browser` defaults to `"default"` even for commands that open no browser,
/// so arming the handler for those lets a read-only `status` kill another agent's Chrome.
/// `close` is excluded for the opposite reason: it already kills its own pid.
#[must_use]
pub const fn interrupt_owns_browser(command: &crate::cli::Command) -> bool {
    use crate::cli::Command as C;
    !matches!(
        command,
        C::Daemon { .. } | C::Status | C::Stop | C::Close { .. } | C::History { .. }
    )
}

/// The pid this invocation may kill on interrupt: its own browser's, and no other.
/// `sessions.json` is shared by every agent on the machine, so it must never be walked
/// whole here.
#[must_use]
pub fn interrupt_kill_target(store: &SessionStore, browser_name: &str) -> Option<u32> {
    store.browsers.get(browser_name).and_then(|b| b.pid)
}

/// Remove a profile directory and confirm it stayed removed: 8 attempts, 250 ms apart.
/// `remove_dir_all` returning `Ok` is not enough — a signalled Chrome writes its state back
/// on the way down. The loop ends when the directory is ABSENT, and a purge that never
/// converges says so instead of claiming success.
fn purge_profile(profile_dir: &std::path::Path) -> Result<(), String> {
    let mut last_error = None;
    for attempt in 0..8u32 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        if !profile_dir.exists() {
            return Ok(());
        }
        if let Err(e) = std::fs::remove_dir_all(profile_dir) {
            last_error = Some(e.to_string());
        }
    }
    Err(last_error.unwrap_or_else(|| {
        "profile was recreated after every removal; the browser may still be shutting down"
            .to_string()
    }))
}

/// The save path removes one orphaned profile per invocation, so a read-only command never
/// pays for housekeeping. This is the same sweep, uncapped, on request.
pub fn cmd_purge_orphans(json_mode: bool) -> Result<(), crate::BoxError> {
    // Loaded, not saved: saving would run the capped sweep under the lock too. The grace
    // window is what makes this unlocked read safe.
    let store = session::load_session()?;
    let referenced = store.browsers.keys().cloned().collect();
    let browsers_dir = session::browsers_dir()?;
    let grace = crate::profiles::Limits::default().grace;

    let mut removed = 0usize;
    let mut failed = Vec::new();
    for path in crate::profiles::all_removable(&browsers_dir, &referenced, grace) {
        match std::fs::remove_dir_all(&path) {
            Ok(()) => removed += 1,
            Err(e) => failed.push(format!("{}: {e}", path.display())),
        }
    }

    let message = format!("Purged {removed} orphaned profile(s)");
    if json_mode {
        json_output(&json!({"ok": true, "message": message, "purged": removed, "failed": failed}));
    } else {
        out_line!("{message}");
        for failure in &failed {
            eprintln!("warning: {failure}");
        }
    }
    Ok(())
}

pub fn cmd_close(browser_name: &str, purge: bool, json_mode: bool) -> Result<(), crate::BoxError> {
    let mut store = session::load_session()?;

    let browser = store.browsers.remove(browser_name);

    // A signal is not an exit: Chrome answers on its DevTools endpoint while tearing down, so
    // a relaunch inside that window handshakes with the dying instance. Waiting for the pid
    // (5s) and removing its port file is what makes `close` then reuse the name work — through
    // `browser::kill_and_await_exit`, which owns that sequence AND the port file's real path.
    // The copy that lived here removed `browsers_dir()/<name>/DevToolsActivePort`, one
    // directory above `browser_profile_dir`, so it had never removed anything.
    let killed = browser
        .as_ref()
        .and_then(|b| b.pid)
        .map(|pid| (pid, crate::browser::kill_and_await_exit(browser_name, pid)));

    let outcome = killed.as_ref().map(|(pid, result)| {
        // `Err` is only ever "signalled, still running after the wait"; its message is the
        // relaunch caller's, and `close` writes its own.
        (
            *pid,
            result.as_ref().copied().unwrap_or(KillOutcome::Signalled),
        )
    });
    let exited = killed.as_ref().map(|(_, result)| match result {
        // Signalled AND observed gone, or never signalled because it already was.
        Ok(KillOutcome::Signalled | KillOutcome::Gone) => true,
        // Nothing was signalled; `false` means "not observed gone".
        Ok(KillOutcome::NotABrowser | KillOutcome::Unverified) | Err(_) => false,
    });

    let message = match (&browser, outcome) {
        (Some(_), Some((pid, outcome))) => {
            let base = close_message(browser_name, pid, outcome);
            if exited == Some(false) && outcome == KillOutcome::Signalled {
                format!("{base} — signalled but still shutting down after 5s")
            } else {
                base
            }
        }
        (Some(_), None) => format!("Removed external browser session: {browser_name}"),
        (None, _) => format!("No browser session named '{browser_name}'."),
    };

    // Removed above before anything was known; an outcome that established nothing puts it
    // back, or this command manufactures the orphans `close --orphans` cleans up.
    let kept = outcome.is_some_and(|(_, o)| !o.entry_may_be_dropped());
    if kept && let Some(entry) = browser {
        store.browsers.insert(browser_name.to_string(), entry);
    }

    // Never purge on the outcome that KEPT the session: nothing was closed, so a live Chrome
    // may still hold the profile. Refusing costs a repeat; the alternative is irreversible.
    let purge_outcome = if purge && !kept {
        session::browsers_dir()
            .ok()
            .map(|dir| purge_profile(&dir.join(browser_name)))
    } else {
        None
    };

    let message = match purge_outcome {
        None if purge && kept => {
            format!(
                "{message} (profile NOT purged: nothing was closed, so its profile may be in use)"
            )
        }
        None => message,
        Some(Ok(())) => format!("{message} (profile purged)"),
        Some(Err(e)) => format!("{message} (profile NOT purged: {e})"),
    };

    if json_mode {
        // `ok` means "the command ran", so `signalled` is the only field separating a browser
        // this closed from one it merely forgot.
        let mut response = json!({
            "ok": true,
            "message": message,
            "signalled": outcome.is_some_and(|(_, o)| o == KillOutcome::Signalled),
        });
        // Absent unless something was signalled: otherwise no wait ever happened.
        if outcome.is_some_and(|(_, o)| o == KillOutcome::Signalled) {
            response["exited"] = json!(exited == Some(true));
        }
        json_output(&response);
    } else {
        out_line!("{message}");
    }

    session::save_session(&mut store)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_that_never_opens_a_browser_has_none_to_interrupt() {
        // These carry the default `--browser` name without touching a browser.
        use crate::cli::Command as C;
        for command in [
            C::Daemon {
                action: crate::cli::DaemonAction::Start,
            },
            C::Status,
            C::Stop,
            C::Close {
                purge: false,
                purge_orphans: false,
                orphans: false,
            },
            C::History {
                filter: None,
                limit: 20,
            },
        ] {
            assert!(
                !interrupt_owns_browser(&command),
                "this command never opens a browser, so it has none to kill"
            );
        }
        // Anything that does connect keeps the cleanup.
        assert!(interrupt_owns_browser(&C::Tabs));
        assert!(interrupt_owns_browser(&C::Pipe));
    }

    #[test]
    fn an_interrupt_only_targets_this_invocation_s_browser() {
        let mut store = SessionStore::default();
        session::ensure_browser(
            &mut store,
            "agent-1",
            "ws://a",
            Some(111),
            true,
            None,
            Vec::new(),
        );
        session::ensure_browser(
            &mut store,
            "agent-2",
            "ws://b",
            Some(222),
            true,
            None,
            Vec::new(),
        );

        assert_eq!(interrupt_kill_target(&store, "agent-1"), Some(111));
        assert_eq!(
            interrupt_kill_target(&store, "agent-2"),
            Some(222),
            "a sibling agent's browser is never this invocation's to kill"
        );
        assert_eq!(interrupt_kill_target(&store, "never-launched"), None);
    }

    #[test]
    fn stop_message_reflects_daemon_reachability() {
        // A stale socket (connect refused) is the reached=false branch, not an error.
        assert_eq!(stop_message(true), "Daemon stopped.");
        assert_eq!(stop_message(false), "Daemon is not running.");
    }

    /// A value that always fails to serialize, so the fallback branch is reachable from a test.
    struct Unserializable;

    impl serde::Serialize for Unserializable {
        fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("a map key was not a string"))
        }
    }

    #[test]
    fn a_response_that_cannot_be_serialized_still_answers_ok_false() {
        let line = json_line(&Unserializable);
        let parsed: serde_json::Value =
            serde_json::from_str(&line).expect("the fallback must itself be valid JSON");
        assert_eq!(
            parsed["ok"], false,
            "the --json contract promises {{\"ok\":false}} on stdout for every failure, and an \
             empty line reads as no response at all: {line}"
        );
        let error = parsed["error"].as_str().unwrap_or_default();
        assert!(
            error.contains("could not be serialized"),
            "the fallback names what happened: {line}"
        );
        assert!(
            error.contains("a map key was not a string"),
            "and carries serde's own reason: {line}"
        );
    }

    #[test]
    fn a_response_that_serializes_is_untouched() {
        assert_eq!(json_line(&json!({"ok": true})), r#"{"ok":true}"#);
    }

    #[test]
    fn batch_text_names_each_entry_and_what_was_skipped() {
        // Captured through the pure builder rather than through `out_line!`, which a test
        // cannot observe. Same two fields the JSON carries.
        let out = json!({
            "ok": false,
            "results": [
                {"ok": true, "message": "Clicked uid=n12"},
                {"ok": false, "error": "No element with uid n99"},
            ],
            "stopped_at": 1,
            "skipped": 2,
        });
        let lines = batch_text_lines(&out, Some(1));
        assert_eq!(
            lines,
            vec![
                "[0] ok — Clicked uid=n12".to_string(),
                "[1] error — No element with uid n99".to_string(),
                "stopped at command 1 — 2 skipped".to_string(),
            ]
        );
    }

    #[test]
    fn a_batch_entry_with_no_sentence_keeps_its_payload() {
        let out = json!({"ok": true, "results": [{"ok": true, "text": "hello"}]});
        let lines = batch_text_lines(&out, None);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].contains("hello"),
            "dropping a payload with no one-line form loses what the caller asked for: {lines:?}"
        );
    }
}
