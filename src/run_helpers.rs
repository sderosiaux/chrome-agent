use std::collections::HashMap;

use serde_json::json;

use crate::cdp::client::CdpClient;
use crate::commands;
use crate::element_ref::ElementRef;
use crate::session::{self, BrowserSession, SessionStore};

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


/// Execute a command, optionally inspect after, and output result.
pub async fn output_action(
    client: &CdpClient,
    store: &mut SessionStore,
    browser_name: &str,
    page_name: &str,
    target_id: &str,
    msg: String,
    inspect: bool,
    max_depth: Option<usize>,
    json_mode: bool,
) -> Result<(), crate::BoxError> {
    if json_mode {
        let mut obj = json!({"ok": true, "message": msg});
        if inspect {
            // Brief pause for navigation/re-render after click/fill before inspecting
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            let snapshot = commands::inspect::run(client, false, max_depth, None, None).await?;
            obj["snapshot"] = json!(snapshot.text);
            if let Some(browser_s) = store.browsers.get_mut(browser_name) {
                let page = session::ensure_page(browser_s, page_name, target_id);
                page.last_snapshot = Some(snapshot.text);
                page.uid_map = snapshot.uid_map;
            }
        }
        json_output(&obj);
    } else {
        println!("{msg}");
        if inspect {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            let snapshot = commands::inspect::run(client, false, max_depth, None, None).await?;
            println!("{}", snapshot.text);
            if let Some(browser_s) = store.browsers.get_mut(browser_name) {
                let page = session::ensure_page(browser_s, page_name, target_id);
                page.last_snapshot = Some(snapshot.text);
                page.uid_map = snapshot.uid_map;
            }
        }
    }
    Ok(())
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
    if json_mode {
        let mut obj = json!({"ok": true, "url": url, "title": title});
        if inspect {
            let snapshot = commands::inspect::run(client, false, max_depth, None, None).await?;
            obj["snapshot"] = json!(snapshot.text);
            page.last_snapshot = Some(snapshot.text);
            page.uid_map = snapshot.uid_map;
        }
        json_output(&obj);
    } else {
        if title.is_empty() {
            println!("{url}");
        } else {
            println!("{url} — {title}");
        }
        if inspect {
            let snapshot = commands::inspect::run(client, false, max_depth, None, None).await?;
            println!("{}", snapshot.text);
            page.last_snapshot = Some(snapshot.text);
            page.uid_map = snapshot.uid_map;
        }
    }
    Ok(())
}

/// Print a `serde_json::Value` as a single compact JSON line to stdout.
pub fn json_output(value: &serde_json::Value) {
    println!("{}", serde_json::to_string(value).unwrap_or_default());
}

/// Provide a contextual hint for common errors.
pub fn error_hint(msg: &str) -> Option<&'static str> {
    // Chrome 136+ refuses CDP on the *default* user profile. chrome-agent launches
    // its own dedicated profile so this only bites when --connect points at a Chrome
    // started on the normal profile. Matched before the generic "Connection refused"
    // branch so the actionable hint wins.
    if msg.contains("Failed to connect to page") || msg.contains("DevToolsActivePort") {
        Some("Could not attach over CDP. Chrome 136+ disables remote debugging on the default profile: drop --connect to let chrome-agent launch its own dedicated profile, or relaunch your Chrome with a separate --user-data-dir.")
    } else if msg.contains("Connection refused") || msg.contains("No such file") {
        Some("Is Chrome running? Try: chrome-agent goto <url>")
    } else if msg.contains("uid=") && msg.contains("not found") {
        Some("Run `chrome-agent inspect` to refresh element uids")
    } else if msg.contains("Navigation failed") {
        Some("Check the URL is valid and the page is reachable")
    } else if msg.contains("No snapshot") || msg.contains("No inspect") || msg.contains("uid_map is empty") {
        Some("Run 'chrome-agent inspect' first")
    } else if msg.contains("Timeout") || msg.contains("timeout") {
        Some("Use --timeout N for slow pages")
    } else if msg.contains("not interactable") || msg.contains("no visible box model") {
        Some("Element may be hidden. Try: chrome-agent scroll <uid>")
    } else if msg.contains("No element matches selector") {
        Some("CSS selector didn't match. Check with: chrome-agent eval \"document.querySelector('...')\"")
    } else if msg.contains("backendDomNodeId") || msg.contains("response parse") {
        Some("Page structure issue. Try: chrome-agent click --selector or chrome-agent eval")
    } else if msg.contains("may not have an article") || msg.contains("Readability") {
        Some("Page has no article structure. Try: chrome-agent text or chrome-agent text --selector \"main\"")
    } else if msg.contains("Provide a uid") || msg.contains("Provide --uid") {
        Some("Specify what to target: uid (e.g. n47), --selector \"css\", or --xy x,y")
    } else if msg.contains("Evaluation error") || msg.contains("TypeError") || msg.contains("ReferenceError") || msg.contains("SyntaxError") {
        Some("JS error in page context. Check expression syntax. Use --selector to scope to an element.")
    } else if msg.contains("dispatcher task exited") || msg.contains("transport closed") {
        Some("Browser connection lost. Try running the command again.")
    } else if msg.contains("not an <iframe>") || msg.contains("not an <IFRAME>") {
        Some("Only <iframe> is supported. For <frame>/<frameset>, use eval to access frame content.")
    } else if msg.contains("No child frame found") {
        Some("Iframe not found. Check the selector matches an <iframe> element.")
    } else if msg.contains("not a <select>") {
        Some("Element is not a <select>. For custom dropdowns, click to open then click the option.")
    } else if msg.contains("No option matching") {
        Some("No dropdown option matched. Use inspect --uid to check available options, or try the visible text.")
    } else if msg.contains("File not found") {
        Some("Check the file path exists on disk.")
    } else if msg.contains("expected a JSON array") {
        Some("Batch expects a JSON array of commands on stdin: [{\"cmd\":\"inspect\"}, ...]")
    } else {
        None
    }
}

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
        json_output(&json!({
            "ok": true,
            "browsers": browsers,
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

/// SIGKILL a managed-browser process (best-effort, unix only). Killing the
/// main Chrome process is enough — its helper processes exit with it.
pub fn kill_pid(pid: u32) {
    #[cfg(unix)]
    {
        let _ = std::process::Command::new("kill")
            .arg(pid.to_string())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
    }
}

pub fn cmd_close(browser_name: &str, purge: bool, json_mode: bool) -> Result<(), crate::BoxError> {
    let mut store = session::load_session()?;

    let browser = store.browsers.remove(browser_name);

    let message = match browser {
        Some(b) => {
            if let Some(pid) = b.pid {
                kill_pid(pid);
                format!("Closed browser={browser_name} (pid={pid})")
            } else {
                format!("Removed external browser session: {browser_name}")
            }
        }
        None => {
            format!("No browser session named '{browser_name}'.")
        }
    };

    // Purge browser profile if requested
    if purge
        && let Some(home) = dirs::home_dir() {
            let profile_dir = home.join(".chrome-agent").join("browsers").join(browser_name);
            if profile_dir.exists() {
                // Wait briefly for Chrome to exit after kill, then retry purge
                for _ in 0..5 {
                    if std::fs::remove_dir_all(&profile_dir).is_ok() {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
            }
        }

    let message = if purge {
        format!("{message} (profile purged)")
    } else {
        message
    };

    if json_mode {
        json_output(&json!({"ok": true, "message": message}));
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
    fn bug_error_hint_covers_all_cases() {
        // Verify all error patterns have hints
        assert!(error_hint("Connection refused").is_some());
        assert!(error_hint("uid=n5 not found").is_some());
        assert!(error_hint("Navigation failed").is_some());
        assert!(error_hint("No snapshot").is_some());
        assert!(error_hint("Timeout waiting").is_some());
        assert!(error_hint("not interactable").is_some());
        assert!(error_hint("No element matches selector").is_some());
        assert!(error_hint("response parse error").is_some());
        assert!(error_hint("Readability failed").is_some());
        assert!(error_hint("Provide a uid").is_some());
        assert!(error_hint("Evaluation error: TypeError: foo").is_some());
        assert!(error_hint("dispatcher task exited").is_some());
        // v0.4.0 new command hints
        assert!(error_hint("Element is not an <iframe>").is_some());
        assert!(error_hint("No child frame found for selector").is_some());
        assert!(error_hint("Element is not a <select>").is_some());
        assert!(error_hint("No option matching: foo").is_some());
        assert!(error_hint("File not found: /tmp/nope").is_some());
        assert!(error_hint("batch: expected a JSON array").is_some());
        // Unknown errors should return None
        assert!(error_hint("something random").is_none());
    }

    #[test]
    fn connect_failure_hints_at_chrome_136() {
        // The page-attach failure and the missing-port marker both point the user
        // at the Chrome 136+ default-profile restriction and the --connect workaround.
        for msg in [
            "Failed to connect to page after 8 attempts: Connection refused",
            "DevToolsActivePort file doesn't exist",
        ] {
            let hint = error_hint(msg).expect("connect failure should have a hint");
            assert!(hint.contains("136"), "hint should mention Chrome 136: {hint}");
            assert!(hint.contains("--connect"), "hint should mention --connect: {hint}");
        }
    }

    #[test]
    fn plain_connection_refused_keeps_generic_hint() {
        // A bare "Connection refused" (no page-attach context) must NOT be hijacked
        // by the 136 branch — it keeps the generic "is Chrome running?" hint.
        let hint = error_hint("Connection refused").unwrap();
        assert!(hint.contains("Chrome running"));
        assert!(!hint.contains("136"));
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
