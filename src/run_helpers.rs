use std::collections::HashMap;

use serde_json::json;

use crate::cdp::client::CdpClient;
use crate::commands;
use crate::element_ref::ElementRef;
use crate::session::{self, BrowserSession, SessionStore};

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
) -> Result<(), Box<dyn std::error::Error>> {
    if json_mode {
        let mut obj = json!({"ok": true, "message": msg});
        if inspect {
            let snapshot = commands::inspect::run(client, false, max_depth, None, None).await?;
            obj["snapshot"] = json!(snapshot.text);
            if let Some(browser_s) = store.browsers.get_mut(browser_name) {
                let page = session::ensure_page(browser_s, page_name, target_id);
                page.uid_map = snapshot.uid_map;
            }
        }
        json_output(&obj);
    } else {
        println!("{msg}");
        if inspect {
            let snapshot = commands::inspect::run(client, false, max_depth, None, None).await?;
            if let Some(browser_s) = store.browsers.get_mut(browser_name) {
                let page = session::ensure_page(browser_s, page_name, target_id);
                page.uid_map = snapshot.uid_map;
            }
            println!("{}", snapshot.text);
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
) -> Result<(), Box<dyn std::error::Error>> {
    let page = session::ensure_page(
        store.browsers.get_mut(browser_name).unwrap(),
        page_name,
        target_id,
    );
    if json_mode {
        let mut obj = json!({"ok": true, "url": url, "title": title});
        if inspect {
            let snapshot = commands::inspect::run(client, false, max_depth, None, None).await?;
            obj["snapshot"] = json!(snapshot.text);
            page.uid_map = snapshot.uid_map;
        }
        json_output(&obj);
    } else {
        println!("{url} — {title}");
        if inspect {
            let snapshot = commands::inspect::run(client, false, max_depth, None, None).await?;
            page.uid_map = snapshot.uid_map;
            println!("{}", snapshot.text);
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
    if msg.contains("Connection refused") || msg.contains("No such file") {
        Some("Is Chrome running? Try: aibrowsr goto <url>")
    } else if msg.contains("uid=") && msg.contains("not found") {
        Some("Run `aibrowsr inspect` to refresh element uids")
    } else if msg.contains("Navigation failed") {
        Some("Check the URL is valid and the page is reachable")
    } else if msg.contains("No snapshot") || msg.contains("No inspect") || msg.contains("uid_map is empty") {
        Some("Run 'aibrowsr inspect' first")
    } else if msg.contains("Timeout") || msg.contains("timeout") {
        Some("Use --timeout N for slow pages")
    } else if msg.contains("not interactable") || msg.contains("no visible box model") {
        Some("Element may be hidden. Try: aibrowsr scroll <uid>")
    } else if msg.contains("No element matches selector") {
        Some("CSS selector didn't match. Check with: aibrowsr eval \"document.querySelector('...')\"")
    } else if msg.contains("backendDomNodeId") || msg.contains("response parse") {
        Some("Page structure issue. Try: aibrowsr click --selector or aibrowsr eval")
    } else if msg.contains("may not have an article") || msg.contains("Readability") {
        Some("Page has no article structure. Try: aibrowsr text or aibrowsr text --selector \"main\"")
    } else if msg.contains("Provide a uid") || msg.contains("Provide --uid") {
        Some("Specify what to target: uid (e.g. n47), --selector \"css\", or --xy x,y")
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
) -> Result<String, Box<dyn std::error::Error>> {
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

pub fn cmd_status(json_mode: bool) -> Result<(), Box<dyn std::error::Error>> {
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

pub async fn cmd_stop(json_mode: bool) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(not(unix))]
    {
        let msg = "Daemon is not supported on this platform.";
        if json_mode { json_output(&json!({"ok": true, "message": msg})); }
        else { println!("{msg}"); }
        return Ok(());
    }

    #[cfg(unix)]
    {
    let socket_path = session::daemon_socket_path()?;
    if !socket_path.exists() {
        if json_mode {
            json_output(&json!({"ok": true, "message": "Daemon is not running."}));
        } else {
            println!("Daemon is not running.");
        }
        return Ok(());
    }

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixStream;

    let mut stream = UnixStream::connect(&socket_path).await?;
    stream
        .write_all(b"{\"command\":\"stop\"}\n")
        .await?;
    stream.shutdown().await?;

    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf).await;

    if json_mode {
        json_output(&json!({"ok": true, "message": "Daemon stopped."}));
    } else {
        println!("Daemon stopped.");
    }
    Ok(())
    } // #[cfg(unix)]
}

pub fn cmd_close(browser_name: &str, purge: bool, json_mode: bool) -> Result<(), Box<dyn std::error::Error>> {
    let mut store = session::load_session()?;

    let browser = store.browsers.remove(browser_name);

    let message = match browser {
        Some(b) => {
            if let Some(pid) = b.pid {
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
    if purge {
        if let Some(home) = dirs::home_dir() {
            let profile_dir = home.join(".aibrowsr").join("browsers").join(browser_name);
            if profile_dir.exists() {
                let _ = std::fs::remove_dir_all(&profile_dir);
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

    session::save_session(&store)?;
    Ok(())
}
