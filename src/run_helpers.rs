use std::collections::HashMap;

use serde_json::json;

use crate::cdp::client::CdpClient;
use crate::commands;
use crate::element_ref::ElementRef;
use crate::session::{self, BrowserSession, SessionStore};

// Split for the 1000-line cap. Re-exported so `kill_pid` keeps one import path across
// `run.rs`, `main.rs` and `orphans.rs` — three call sites that must not drift onto two
// different kill paths, which is how the pid-reuse guard came to be bypassed once already.
pub use crate::kill::{KillOutcome, close_message, kill_pid};

/// Connect to a page-level CDP endpoint with retry. Sets up Page domain,
/// console interceptor, and optionally Runtime domain + stealth patches.
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
                    // Verify connection is alive with a lightweight call
                    if let Err(e) = client.call::<_, serde_json::Value>(
                        "Runtime.evaluate",
                        json!({"expression": "1", "returnByValue": true}),
                    ).await {
                        last_err = format!("Connection verify failed: {e}");
                        drop(client);
                        if attempt < 7 {
                            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        }
                        continue;
                    }
                    // Setup: enable Page domain
                    if let Err(e) = client.enable("Page").await {
                        last_err = format!("Page.enable failed: {e}");
                        drop(client);
                        if attempt < 7 {
                            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                        }
                        continue;
                    }
                    // Console interceptor
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
///
/// `on_intercept` rides here rather than in a parallel parameter: it is a global flag like the
/// other two, and this struct is already threaded through the CLI, pipe and batch paths, so
/// carrying it costs no dispatcher signature.
#[derive(Clone, Copy)]
pub struct ReportPolicy {
    pub changes: bool,
    pub budget: usize,
    pub on_intercept: crate::hit_test::OnIntercept,
}

impl ReportPolicy {
    /// Build the per-action report from the policy plus that command's own flags.
    pub const fn for_action(self, inspect: bool, max_depth: Option<usize>) -> ActionReport {
        ActionReport { inspect, changes: self.changes, budget: self.budget, max_depth }
    }
}

/// What the four read-back verbs put on their responses: moved to `read_back` for the
/// 1000-line file cap and re-exported here, so a caller still writes
/// `run_helpers::fill_value_report` next to the `output_action` that ships it.
pub use crate::read_back::{bulk_fill_report, check_report, fill_value_report, select_report};

/// The node an action is about to touch, whichever way it was named: `uid`, plus `role` and
/// `name` when they come free.
///
/// Resolved before the action runs: afterwards the element may be detached, and the answer
/// would describe a different page. Returns the fields to merge into the response, so a
/// caller that has none of its own can pass this straight through.
///
/// `role`/`name` are best effort and come out of the `DOM.describeNode` the uid already needs
/// — the explicit ARIA role or the tag name, and an accessible-name attribute if the element
/// carries one. Not the computed accessibility name: that would cost another read, and it is
/// what `inspect` is for. The uid path stays a plain echo and pays no round trip at all.
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
        // A uid-targeted action already names its node; echoing it keeps the field's
        // meaning the same whichever way the caller aimed.
        (None, Some(uid)) => Some(json!({"uid": uid})),
        (None, None) => None,
    }
}

/// Merge two optional field sets into one response object.
#[must_use]
pub fn merge_details(
    first: Option<serde_json::Value>,
    second: Option<serde_json::Value>,
) -> Option<serde_json::Value> {
    match (first, second) {
        (Some(mut a), Some(b)) => {
            if let (Some(target), Some(extra)) = (a.as_object_mut(), b.as_object()) {
                for (key, value) in extra {
                    target.insert(key.clone(), value.clone());
                }
            }
            Some(a)
        }
        (Some(only), None) | (None, Some(only)) => Some(only),
        (None, None) => None,
    }
}

/// Execute a command, report what it did to the page, and persist the new baseline.
///
/// By default an action now answers "what changed", not just "what I was asked to do".
/// Without it the agent has to spend a second call to find out whether the click landed,
/// and that extra turn is the cost this is meant to remove. `--verdict off` restores the
/// older behaviour for callers that would rather have the latency back.
pub async fn output_action(
    client: &CdpClient,
    store: &mut SessionStore,
    browser_name: &str,
    page_name: &str,
    target_id: &str,
    msg: String,
    report: &ActionReport,
    json_mode: bool,
) -> Result<(), crate::BoxError> {
    output_action_with(client, store, browser_name, page_name, target_id, msg, report, json_mode, None).await
}

/// `output_action` plus whatever the command itself observed — the value a fill left
/// behind, the window a check looked through. Merged at the top level of the response so
/// the CLI and the pipe dispatchers, which build their JSON separately, agree on shape.
#[allow(clippy::too_many_arguments)]
pub async fn output_action_with(
    client: &CdpClient,
    store: &mut SessionStore,
    browser_name: &str,
    page_name: &str,
    target_id: &str,
    msg: String,
    report: &ActionReport,
    json_mode: bool,
    details: Option<serde_json::Value>,
) -> Result<(), crate::BoxError> {
    let mut obj = json!({"ok": true, "message": msg});
    if let Some(fields) = details.as_ref().and_then(serde_json::Value::as_object) {
        for (key, value) in fields {
            obj[key.as_str()] = value.clone();
        }
    }
    let mut trailer = String::new();
    // Silence used to mean four different things here. Whatever happens below, the response
    // carries the one that applies.
    let mut observation = if report.changes {
        crate::verdict::Observation::NoBaseline
    } else {
        crate::verdict::Observation::ReportingDisabled
    };

    if report.inspect || report.changes {
        // Wait for the page to stop reacting rather than for a fixed guess: a page that
        // does nothing costs a quiet window, one that renders late is still caught.
        crate::snapshot::settle(client, 100, 1000).await;
        // The baseline is always full depth. Storing a `--max-depth` view would make the
        // next comparison read every node the limit cut off as newly added: verified, an
        // action with `--max-depth 1` then a plain `diff` invented additions.
        //
        // A read that fails is not an action that failed. This used to propagate with `?`,
        // so a click that had already been delivered came back as `ok:false` — and the
        // natural response to that is to click again, which is real. `pipe_dispatch` stated
        // the opposite policy in a comment and followed it; this is the CLI adopting it.
        let Ok(snapshot) = commands::inspect::run(client, false, None, None, None).await else {
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

        if report.changes {
            let previous = store
                .browsers
                .get(browser_name)
                .and_then(|b| b.pages.get(page_name))
                .map(|p| {
                    (
                        p.last_snapshot.clone(),
                        p.last_snapshot_frame.clone().zip(p.last_snapshot_loader.clone()),
                    )
                });
            if let Some((Some(old_text), stored)) = previous {
                let identity = commands::diff::Identity::from_loader(
                    stored.as_ref().map(|(f, l)| (f.as_str(), l.as_str())),
                    snapshot.identity.as_ref().map(|(f, l)| (f.as_str(), l.as_str())),
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
                // Read off the fresh uid_map, which is still ours until the store takes it
                // below. Feeds the verdict, so it has to run before it is settled.
                //
                // `Box::pin`: this future lives inside `output_action_with`, which the CLI's
                // one big `match` on `Command` embeds in a single stack frame. Inlining its
                // state machine there pushed that frame past clippy's `large_stack_frames`
                // ceiling; boxing it keeps the frame flat for the cost of one allocation on a
                // path that already did a full page read.
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

        if report.inspect {
            // The caller asked to see the tree at their depth; the baseline above stays
            // full so the two never get confused.
            let shown = if report.max_depth.is_some() {
                commands::inspect::run(client, false, report.max_depth, None, None)
                    .await
                    .map_or_else(|_| snapshot.text.clone(), |s| s.text)
            } else {
                snapshot.text.clone()
            };
            obj["snapshot"] = json!(shown);
            trailer.clone_from(&shown);
        }

        if let Some(browser_s) = store.browsers.get_mut(browser_name) {
            let page = session::ensure_page(browser_s, page_name, target_id);
            page.last_snapshot = Some(snapshot.text);
            let (f, l) = snapshot.identity.map_or((None, None), |(f, l)| (Some(f), Some(l)));
            page.last_snapshot_frame = f;
            page.last_snapshot_loader = l;
            page.uid_map = snapshot.uid_map;
        }
    }

    let assessment = crate::pipe_report::attach_verdict_for(client, &mut obj, observation);

    if json_mode {
        json_output(&obj);
    } else {
        print_action(&msg, &trailer, &obj, assessment);
    }
    Ok(())
}

/// The text-mode report for one action: its own message, the delta, and everything the
/// response measured that the text branch used to drop (`src/render.rs`).
///
/// One function for both exits — the normal one and the failed-read early return — so the two
/// cannot drift into printing different shapes for the same response.
fn print_action(
    msg: &str,
    trailer: &str,
    obj: &serde_json::Value,
    assessment: crate::verdict::Assessment,
) {
    println!("{msg}");
    if !trailer.is_empty() {
        println!("{}", trailer.trim_end());
    }
    for line in crate::render::action_lines(obj, assessment, crate::render::Paint::for_stdout()) {
        println!("{line}");
    }
}

/// Write the verdict, its reason, and — when the verdict is an admission of ignorance —
/// what to do about it.
///
/// `hint` may already hold the diff's own advice (a navigation tells the caller its uids
/// are dead). The verdict's hint goes in its own field rather than overwriting it: two
/// different pieces of advice, one slot, and the more specific one loses.
pub fn attach_verdict(obj: &mut serde_json::Value, assessment: crate::verdict::Assessment) {
    obj["verdict"] = json!(assessment.verdict.as_str());
    obj["verdict_reason"] = json!(assessment.reason);
    // One token from a closed set of six, so an agent can branch on the next step without
    // parsing the hint prose. Written here because this is the one place all three modes
    // settle a verdict — the same reason `verdict_reason` lives here.
    obj["next"] = json!(crate::verdict::next_for(assessment).as_str());
    if let Some(hint) = crate::verdict::hint_for(assessment) {
        // An action that already wrote a hint knows more than the verdict does — an
        // intercepted click can name the element that took it, where the generic hint can
        // only say that one exists. Never overwrite the specific one with the generic one.
        if let Some(map) = obj.as_object_mut() {
            map.entry("verdict_hint").or_insert_with(|| json!(hint));
        }
    }
}

/// Output goto result with optional post-inspect.
pub async fn output_goto(
    client: &CdpClient,
    store: &mut SessionStore,
    browser_name: &str,
    page_name: &str,
    target_id: &str,
    url: &str,
    title: &str,
    landed: Option<&crate::landing::Landing>,
    inspect: bool,
    max_depth: Option<usize>,
    json_mode: bool,
) -> Result<(), crate::BoxError> {
    let browser_session = store.browsers.get_mut(browser_name)
        .ok_or_else(|| format!("Browser session '{browser_name}' not found in session store"))?;
    let page = session::ensure_page(
        browser_session,
        page_name,
        target_id,
    );
    // The old document is gone, so every uid in the stored map now points at a node that
    // no longer exists — and `backendNodeId` counters overlap between documents, so a
    // stale uid can silently resolve to an unrelated element on the new page. Drop the
    // map here; the `if inspect` branches below refill it when the caller asked to see
    // the page. Without a fresh inspect the agent gets "uid not found" and a hint, which
    // is the correct answer.
    page.uid_map.clear();
    if json_mode {
        let mut obj = json!({"ok": true, "url": url, "title": title});
        if let Some(landing) = landed {
            landing.attach(&mut obj);
        }
        if inspect {
            let snapshot = commands::inspect::run(client, false, max_depth, None, None).await?;
            obj["snapshot"] = json!(snapshot.text);
            page.last_snapshot = Some(snapshot.text);
            let (f, l) = snapshot.identity.map_or((None, None), |(f, l)| (Some(f), Some(l)));
            page.last_snapshot_frame = f;
            page.last_snapshot_loader = l;
            page.uid_map = snapshot.uid_map;
        }
        json_output(&obj);
    } else {
        if title.is_empty() {
            println!("{url}");
        } else {
            println!("{url} — {title}");
        }
        if let Some(line) = landed.and_then(crate::landing::Landing::text_line) {
            println!("{line}");
        }
        if inspect {
            let snapshot = commands::inspect::run(client, false, max_depth, None, None).await?;
            println!("{}", snapshot.text);
            page.last_snapshot = Some(snapshot.text);
            let (f, l) = snapshot.identity.map_or((None, None), |(f, l)| (Some(f), Some(l)));
            page.last_snapshot_frame = f;
            page.last_snapshot_loader = l;
            page.uid_map = snapshot.uid_map;
        }
    }
    Ok(())
}

/// Print a `serde_json::Value` as a single compact JSON line to stdout.
pub fn json_output(value: &serde_json::Value) {
    println!("{}", serde_json::to_string(value).unwrap_or_default());
}

/// The error-recovery hints, moved to `hints` for the 1000-line file cap and re-exported
/// here so `main`, `pipe` and `pipe_dispatch` keep their existing call sites.
pub use crate::hints::error_hint;

/// Get the `uid_map` from the current session, or empty if none.
pub fn get_uid_map(store: &SessionStore, browser_name: &str, page_name: &str) -> HashMap<String, ElementRef> {
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
    // Reported next to the sessions rather than in a command of its own: a browser the
    // registry lost is invisible exactly where a user goes to look for one, and the two
    // 19-day-old Chromes that motivated this were found with `ps`, not with this tool.
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
        // `null` where the process table could not be read, which is not the same claim
        // as an empty list and would be a false all-clear if flattened into one.
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
            println!("No active browser sessions.");
        } else {
            for (name, browser) in &store.browsers {
                let status = if let Some(pid) = browser.pid {
                    format!("pid={pid}")
                } else {
                    "external".into()
                };
                let mode = if browser.headless { "headless" } else { "headed" };
                println!(
                    "browser={name}  {status}  {mode}  pages={}  ws={}",
                    browser.pages.len(),
                    browser.ws_endpoint
                );
            }
        }

        for orphan in orphans.iter().flatten() {
            println!(
                "orphan={}  pid={}  no session entry — close with `chrome-agent close --orphans`",
                orphan.name, orphan.pid
            );
        }

        println!(
            "daemon: {}",
            if daemon_alive { "running" } else { "stopped" }
        );
    }

    Ok(())
}


/// Message for `cmd_stop`, given whether we actually reached a live daemon.
/// Pure so the stop decision can be unit-tested without a socket.
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
        if json_mode { json_output(&json!({"ok": true, "message": msg})); }
        else { println!("{msg}"); }
        return Ok(());
    }

    #[cfg(unix)]
    {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixStream;

    let socket_path = session::daemon_socket_path()?;

    // Try to reach the daemon. A missing socket — or a stale one left by a
    // crashed daemon (connect yields ECONNREFUSED) — both mean "not running".
    // Don't let the raw connect error escape via `?`; clean the stale socket
    // and report the friendly path instead.
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
        if json_mode { json_output(&json!({"ok": true, "message": msg})); }
        else { println!("{msg}"); }
        return Ok(());
    };

    stream
        .write_all(b"{\"command\":\"stop\"}\n")
        .await?;
    stream.shutdown().await?;

    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf).await;

    let msg = stop_message(true);
    if json_mode { json_output(&json!({"ok": true, "message": msg})); }
    else { println!("{msg}"); }
    Ok(())
    } // #[cfg(unix)]
}

/// Whether this command can own the browser named by `--browser`, and may therefore
/// take it down when interrupted.
///
/// `--browser` is a global flag, so every invocation carries a name — defaulted to
/// `"default"`, the one most single-agent users get — including the commands that never
/// open a browser at all. `run::run` returns before the connection block for each of
/// these; arming the handler for them meant Ctrl+C during a read-only `status` killed
/// whichever agent happened to hold that name. `close` is excluded for the opposite
/// reason: it kills its own pid deliberately, and does not need a second, racier path.
#[must_use]
pub const fn interrupt_owns_browser(command: &crate::cli::Command) -> bool {
    use crate::cli::Command as C;
    !matches!(
        command,
        C::Daemon { .. } | C::Status | C::Stop | C::Close { .. } | C::History { .. }
    )
}

/// The pid this invocation may kill on interrupt: its own browser's, and no other.
///
/// The Ctrl+C handler used to walk every entry in `sessions.json` — a file shared by
/// every agent on the machine — so interrupting one agent killed the Chrome of every
/// other agent running under a different `--browser` name, which is exactly the
/// isolation the flag exists to provide.
#[must_use]
pub fn interrupt_kill_target(store: &SessionStore, browser_name: &str) -> Option<u32> {
    store.browsers.get(browser_name).and_then(|b| b.pid)
}

/// Remove a profile directory and confirm it stayed removed.
///
/// `remove_dir_all` returning `Ok` is not the same as the profile being gone. A Chrome that
/// has been signalled but has not exited yet writes its state back on the way down, and the
/// old loop broke on that first `Ok` and then claimed "(profile purged)" over a directory
/// that reappeared a third of a second later. Measured on one close: 235 files before, none
/// immediately after, 22 once the shutdown flush landed (`Local State`,
/// `Default/Preferences`, `TransportSecurity`, three cache stubs) — and 946 of the 1204
/// profile directories that had accumulated on a developer machine were exactly that
/// residue. The loop now ends when the directory is absent rather than when one call
/// succeeded, and a purge that never converges says so instead of being reported as done.
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

/// Sweep the profile directories the automatic prune would reach one command at a time.
///
/// The save-path sweep is capped at one removal per invocation so a read-only command never
/// pays for housekeeping, which means a store that accumulated before any of this existed
/// needs as many commands as it has orphans. This is that sweep, uncapped, on request.
pub fn cmd_purge_orphans(json_mode: bool) -> Result<(), crate::BoxError> {
    // Loaded, not saved: saving would run the capped sweep under the lock as well, and the
    // grace window is what makes reading the store outside the lock safe here.
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
        println!("{message}");
        for failure in &failed {
            eprintln!("warning: {failure}");
        }
    }
    Ok(())
}

pub fn cmd_close(browser_name: &str, purge: bool, json_mode: bool) -> Result<(), crate::BoxError> {
    let mut store = session::load_session()?;

    let browser = store.browsers.remove(browser_name);

    let outcome = browser.as_ref().and_then(|b| b.pid).map(|pid| (pid, kill_pid(pid)));

    // A signal is not an exit. Chrome keeps its DevTools HTTP endpoint answering for a
    // moment while it tears down, so a relaunch inside that window reconnects to the
    // dying instance and fails its WebSocket handshake. Waiting until the pid is gone —
    // and removing the port file it leaves — is what lets `close` immediately followed
    // by a fresh command on the same name work instead of racing.
    let exited = outcome.map(|(pid, o)| match o {
        crate::kill::KillOutcome::Signalled => {
            let gone = crate::kill::wait_until_gone(pid, std::time::Duration::from_secs(5));
            if gone && let Ok(dir) = session::browsers_dir() {
                let _ = std::fs::remove_file(dir.join(browser_name).join("DevToolsActivePort"));
            }
            gone
        }
        crate::kill::KillOutcome::Gone => true,
        crate::kill::KillOutcome::NotABrowser => false,
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

    // Purge browser profile if requested
    let purge_outcome = if purge {
        session::browsers_dir().ok().map(|dir| purge_profile(&dir.join(browser_name)))
    } else {
        None
    };

    let message = match purge_outcome {
        None => message,
        Some(Ok(())) => format!("{message} (profile purged)"),
        Some(Err(e)) => format!("{message} (profile NOT purged: {e})"),
    };

    if json_mode {
        // `ok` has always meant "the command ran", so a caller cannot read a kill out of
        // it. `signalled` is the act itself: false where the pid was gone or reused, and
        // the only field that separates a browser this closed from one it merely forgot.
        let mut response = json!({
            "ok": true,
            "message": message,
            "signalled": outcome.is_some_and(|(_, o)| o == KillOutcome::Signalled),
        });
        // Only meaningful when something was signalled: whether the process was observed
        // gone before `close` returned. Absent otherwise — a claim about a wait that
        // never happened would be noise.
        if outcome.is_some_and(|(_, o)| o == KillOutcome::Signalled) {
            response["exited"] = json!(exited == Some(true));
        }
        json_output(&response);
    } else {
        println!("{message}");
    }

    session::save_session(&mut store)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_that_never_opens_a_browser_has_none_to_interrupt() {
        // `--browser` is global, so these carry the default name and would otherwise
        // kill whichever agent happens to be using it — while never having touched it.
        // Each of these returns from `run::run` before the connection block.
        use crate::cli::Command as C;
        for command in [
            C::Daemon { action: crate::cli::DaemonAction::Start },
            C::Status,
            C::Stop,
            C::Close { purge: false, purge_orphans: false, orphans: false },
            C::History { filter: None, limit: 20 },
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
        session::ensure_browser(&mut store, "agent-1", "ws://a", Some(111), true, None, Vec::new());
        session::ensure_browser(&mut store, "agent-2", "ws://b", Some(222), true, None, Vec::new());

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
        // Regression for A3c: a stale socket (connect refused) must map to the
        // friendly "not running" path, not a raw propagated error. The reached=false
        // branch is exactly what cmd_stop selects when connect fails.
        assert_eq!(stop_message(true), "Daemon stopped.");
        assert_eq!(stop_message(false), "Daemon is not running.");
    }
}
