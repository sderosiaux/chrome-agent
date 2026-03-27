mod browser;
mod cdp;
mod commands;
mod daemon;
mod element;
mod element_ref;
mod session;
mod snapshot;

use std::collections::HashMap;

use clap::{Parser, Subcommand};
use serde_json::json;

use crate::browser::BrowserOptions;
use crate::cdp::client::CdpClient;
use crate::element_ref::ElementRef;
use crate::session::{BrowserSession, SessionStore};

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

const CLI_LONG_ABOUT: &str = "\
aibrowsr — browser automation for AI agents. Controls Chrome via CDP.\n\
Single binary, zero runtime dependencies. Named pages persist between invocations.\n\
\n\
Workflow: inspect → read uids → act (click/fill) → inspect again.\n\
Use --inspect on action commands to combine action + observation in one call.";

const CLI_AFTER_LONG_HELP: &str = include_str!("../llm-guide.txt");

#[derive(Parser)]
#[command(
    name = "aibrowsr",
    version,
    about = "aibrowsr — browser automation for AI agents",
    long_about = CLI_LONG_ABOUT,
    after_long_help = CLI_AFTER_LONG_HELP,
)]
struct Cli {
    /// Named browser profile (default: "default")
    #[arg(long, default_value = "default")]
    browser: String,

    /// Connect to existing browser: ws:// URL, http:// URL, or "auto"
    #[arg(long)]
    connect: Option<String>,

    /// Launch browser with a visible window (default is headless)
    #[arg(long)]
    headed: bool,

    /// Global timeout in seconds for page loads
    #[arg(long, default_value = "30")]
    timeout: u64,

    /// Ignore HTTPS certificate errors
    #[arg(long)]
    ignore_https_errors: bool,

    /// Output structured JSON instead of text
    #[arg(long)]
    json: bool,

    /// Named page/tab within the browser (default: "default")
    #[arg(long, default_value = "default")]
    page: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Navigate to a URL
    Goto {
        /// Target URL
        url: String,
        /// Inspect page after navigation
        #[arg(long)]
        inspect: bool,
    },

    /// Click an element by uid
    Click {
        /// Element uid (e.g. "e5")
        uid: String,
        /// Inspect page after clicking
        #[arg(long)]
        inspect: bool,
    },

    /// Fill an input element by uid
    Fill {
        /// Element uid (e.g. "e5")
        uid: String,
        /// Value to fill
        value: String,
        /// Inspect page after filling
        #[arg(long)]
        inspect: bool,
    },

    /// Fill multiple form fields at once
    #[command(name = "fill-form")]
    FillForm {
        /// uid=value pairs (e.g. "e5=hello" "e7=world")
        pairs: Vec<String>,
        /// Inspect page after filling
        #[arg(long)]
        inspect: bool,
    },

    /// Extract visible text from the page or an element
    Text {
        /// Element uid to extract text from (default: entire page)
        uid: Option<String>,
    },

    /// Navigate back in browser history
    Back,

    /// Take an accessibility tree inspection
    Inspect {
        /// Include ignored/generic nodes
        #[arg(long)]
        verbose: bool,
        /// Maximum tree depth (0 = root only)
        #[arg(long)]
        max_depth: Option<usize>,
        /// Only inspect children of this uid
        #[arg(long)]
        uid: Option<String>,
    },

    /// Capture a screenshot
    Screenshot {
        /// Output filename (default: timestamped)
        #[arg(long)]
        filename: Option<String>,
    },

    /// Evaluate JavaScript in the page
    Eval {
        /// JS expression to evaluate
        expression: String,
    },

    /// Wait for a condition (text, url, or selector)
    Wait {
        /// What to wait for: "text", "url", or "selector"
        what: String,
        /// Pattern to match
        pattern: String,
        /// Timeout in seconds
        #[arg(long, default_value = "10")]
        timeout: u64,
    },

    /// Type text into the focused element
    Type {
        /// Text to type
        text: String,
    },

    /// Press a key (Enter, Tab, Escape, etc.)
    Press {
        /// Key name
        key: String,
    },

    /// Scroll the page or an element into view
    Scroll {
        /// "up", "down", or a uid to scroll into view
        target: String,
    },

    /// Hover over an element by uid
    Hover {
        /// Element uid (e.g. "e5")
        uid: String,
    },

    /// List open browser tabs
    Tabs,

    /// Close the managed browser
    Close,

    /// Show session status
    Status,

    /// Stop the background daemon
    Stop,

    /// Daemon management
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },
}

#[derive(Subcommand)]
enum DaemonAction {
    /// Start the daemon (foreground, used internally)
    Start,
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    // Install signal handler so managed Chrome is cleaned up on Ctrl+C
    tokio::spawn(async {
        if let Ok(()) = tokio::signal::ctrl_c().await {
            // Best-effort: kill all managed Chrome processes from session
            if let Ok(store) = session::load_session() {
                for browser in store.browsers.values() {
                    if let Some(pid) = browser.pid {
                        #[cfg(unix)]
                        {
                            let _ = std::process::Command::new("kill")
                                .arg(pid.to_string())
                                .stdout(std::process::Stdio::null())
                                .stderr(std::process::Stdio::null())
                                .status();
                        }
                    }
                }
            }
            std::process::exit(130); // 128 + SIGINT
        }
    });

    let cli = Cli::parse();
    let json_mode = cli.json;

    if let Err(e) = run(cli).await {
        if json_mode {
            let msg = e.to_string();
            let hint = error_hint(&msg);
            let mut obj = json!({"ok": false, "error": msg});
            if let Some(h) = hint {
                obj["hint"] = json!(h);
            }
            println!("{}", serde_json::to_string(&obj).unwrap_or_default());
        } else {
            let msg = e.to_string();
            eprintln!("error: {msg}");
            if let Some(hint) = error_hint(&msg) {
                eprintln!("hint: {hint}");
            }
        }
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    match cli.command {
        Command::Daemon { action } => {
            match action {
                DaemonAction::Start => {
                    let socket_path = session::daemon_socket_path()?;
                    daemon::run_daemon(&socket_path).await?;
                }
            }
            return Ok(());
        }

        Command::Status => {
            return cmd_status(cli.json).await;
        }

        Command::Stop => {
            return cmd_stop(cli.json).await;
        }

        Command::Close => {
            return cmd_close(&cli.browser, cli.json).await;
        }

        _ => {}
    }

    // All other commands need a browser connection + CDP client
    let mut store = session::load_session()?;

    let want_headless = !cli.headed;

    // Try to reuse existing session, or launch a new browser
    let (conn, browser_client) = if let Some(existing) = store.browsers.get(&cli.browser) {
        let mode_matches = existing.headless == want_headless;
        let ws = &existing.ws_endpoint;
        let http = browser::extract_http_from_ws(ws);

        if mode_matches {
            match CdpClient::connect(ws).await {
                Ok(client) => {
                    let conn = browser::BrowserConnection {
                        ws_endpoint: ws.clone(),
                        http_endpoint: Some(http),
                        pid: existing.pid,
                    };
                    (conn, client)
                }
                Err(_) => {
                    store.browsers.remove(&cli.browser);
                    let opts = BrowserOptions {
                        name: cli.browser.clone(),
                        headless: want_headless,
                        ignore_https_errors: cli.ignore_https_errors,
                        connect: cli.connect.clone(),
                    };
                    let conn = browser::resolve_browser(&opts).await?;
                    let client = CdpClient::connect(&conn.ws_endpoint).await?;
                    (conn, client)
                }
            }
        } else {
            // Mode mismatch (e.g. old session is headed, agent wants headless).
            // Kill old browser and launch fresh with correct mode.
            if let Some(pid) = existing.pid {
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
            let opts = BrowserOptions {
                name: cli.browser.clone(),
                headless: want_headless,
                ignore_https_errors: cli.ignore_https_errors,
                connect: cli.connect.clone(),
            };
            let conn = browser::resolve_browser(&opts).await?;
            let client = CdpClient::connect(&conn.ws_endpoint).await?;
            (conn, client)
        }
    } else {
        let opts = BrowserOptions {
            name: cli.browser.clone(),
            headless: want_headless,
            ignore_https_errors: cli.ignore_https_errors,
            connect: cli.connect.clone(),
        };
        let conn = browser::resolve_browser(&opts).await?;
        let client = CdpClient::connect(&conn.ws_endpoint).await?;
        (conn, client)
    };

    let http_endpoint = conn.http_endpoint.as_deref().ok_or_else(|| {
        "No HTTP endpoint available. Cannot resolve page WebSocket URL."
    })?;

    // Ensure browser session and resolve page target
    let target_id = {
        let browser_session = session::ensure_browser(
            &mut store,
            &cli.browser,
            &conn.ws_endpoint,
            conn.pid,
            !cli.headed,
        );
        resolve_page_target(&browser_client, browser_session, &cli.page).await?
    };
    // Save session immediately so Chrome PID is persisted even if CLI crashes later
    let _ = session::save_session(&store);

    // Connect page-level CDP (for Page.*, Runtime.*, DOM.*, etc.)
    let page_ws = browser::get_page_ws_url(http_endpoint, &target_id).await?;
    let client = CdpClient::connect(&page_ws).await?;
    client.enable("Page").await?;
    client.enable("Runtime").await?;

    // Execute command
    let json_mode = cli.json;
    match cli.command {
        Command::Goto { url, inspect } => {
            let result = commands::goto::run(&client, &url, cli.timeout).await?;

            let page = session::ensure_page(
                store.browsers.get_mut(&cli.browser).unwrap(),
                &cli.page,
                &target_id,
            );

            if json_mode {
                let mut obj = json!({"ok": true, "url": result.url, "title": result.title});
                if inspect {
                    let snapshot = commands::inspect::run(&client, false, None, None).await?;
                    obj["snapshot"] = json!(snapshot.text);
                    page.uid_map = snapshot.uid_map;
                }
                json_output(&obj);
            } else {
                println!("{} — {}", result.url, result.title);
                if inspect {
                    let snapshot = commands::inspect::run(&client, false, None, None).await?;
                    page.uid_map = snapshot.uid_map;
                    println!("{}", snapshot.text);
                }
            }
        }

        Command::Click { uid, inspect } => {
            let uid_map = get_uid_map(&store, &cli.browser, &cli.page);
            let msg = commands::click::run(&client, &uid_map, &uid).await?;

            if json_mode {
                let mut obj = json!({"ok": true, "message": msg});
                if inspect {
                    let snapshot = commands::inspect::run(&client, false, None, None).await?;
                    obj["snapshot"] = json!(snapshot.text);
                    if let Some(browser_s) = store.browsers.get_mut(&cli.browser) {
                        let page = session::ensure_page(browser_s, &cli.page, &target_id);
                        page.uid_map = snapshot.uid_map;
                    }
                }
                json_output(&obj);
            } else {
                println!("{msg}");
                if inspect {
                    let snapshot = commands::inspect::run(&client, false, None, None).await?;
                    if let Some(browser_s) = store.browsers.get_mut(&cli.browser) {
                        let page = session::ensure_page(browser_s, &cli.page, &target_id);
                        page.uid_map = snapshot.uid_map;
                    }
                    println!("{}", snapshot.text);
                }
            }
        }

        Command::Fill { uid, value, inspect } => {
            let uid_map = get_uid_map(&store, &cli.browser, &cli.page);
            let msg = commands::fill::run(&client, &uid_map, &uid, &value).await?;

            if json_mode {
                let mut obj = json!({"ok": true, "message": msg});
                if inspect {
                    let snapshot = commands::inspect::run(&client, false, None, None).await?;
                    obj["snapshot"] = json!(snapshot.text);
                    if let Some(browser_s) = store.browsers.get_mut(&cli.browser) {
                        let page = session::ensure_page(browser_s, &cli.page, &target_id);
                        page.uid_map = snapshot.uid_map;
                    }
                }
                json_output(&obj);
            } else {
                println!("{msg}");
                if inspect {
                    let snapshot = commands::inspect::run(&client, false, None, None).await?;
                    if let Some(browser_s) = store.browsers.get_mut(&cli.browser) {
                        let page = session::ensure_page(browser_s, &cli.page, &target_id);
                        page.uid_map = snapshot.uid_map;
                    }
                    println!("{}", snapshot.text);
                }
            }
        }

        Command::FillForm { pairs, inspect } => {
            let uid_map = get_uid_map(&store, &cli.browser, &cli.page);
            let parsed: Result<Vec<(&str, &str)>, _> = pairs
                .iter()
                .map(|p| {
                    p.split_once('=')
                        .ok_or_else(|| format!("Invalid pair (expected uid=value): {p}"))
                })
                .collect();
            let parsed = parsed?;

            let msg = commands::fill::run_form(&client, &uid_map, &parsed).await?;

            if json_mode {
                let mut obj = json!({"ok": true, "message": msg});
                if inspect {
                    let snapshot = commands::inspect::run(&client, false, None, None).await?;
                    obj["snapshot"] = json!(snapshot.text);
                    if let Some(browser_s) = store.browsers.get_mut(&cli.browser) {
                        let page = session::ensure_page(browser_s, &cli.page, &target_id);
                        page.uid_map = snapshot.uid_map;
                    }
                }
                json_output(&obj);
            } else {
                println!("{msg}");
                if inspect {
                    let snapshot = commands::inspect::run(&client, false, None, None).await?;
                    if let Some(browser_s) = store.browsers.get_mut(&cli.browser) {
                        let page = session::ensure_page(browser_s, &cli.page, &target_id);
                        page.uid_map = snapshot.uid_map;
                    }
                    println!("{}", snapshot.text);
                }
            }
        }

        Command::Text { uid } => {
            let uid_map = get_uid_map(&store, &cli.browser, &cli.page);
            let text = commands::text::run(&client, uid.as_deref(), &uid_map).await?;
            if json_mode {
                json_output(&json!({"ok": true, "text": text}));
            } else {
                println!("{text}");
            }
        }

        Command::Back => {
            client.send("Runtime.evaluate", json!({"expression": "history.back()"})).await?;
            // Wait briefly for navigation
            let _ = client.wait_for_event("Page.loadEventFired", std::time::Duration::from_secs(5)).await;
            let title: crate::cdp::types::EvaluateResult = client
                .call("Runtime.evaluate", json!({"expression": "document.title", "returnByValue": true}))
                .await?;
            let title_str = title.result.value.as_ref().and_then(|v| v.as_str()).unwrap_or("");
            if json_mode {
                json_output(&json!({"ok": true, "title": title_str}));
            } else {
                println!("Navigated back — {title_str}");
            }
        }

        Command::Inspect { verbose, max_depth, uid } => {
            let snapshot = commands::inspect::run(&client, verbose, max_depth, uid.as_deref()).await?;
            if let Some(browser_s) = store.browsers.get_mut(&cli.browser) {
                let page = session::ensure_page(browser_s, &cli.page, &target_id);
                page.uid_map = snapshot.uid_map;
            }
            if json_mode {
                json_output(&json!({"ok": true, "snapshot": snapshot.text}));
            } else {
                println!("{}", snapshot.text);
            }
        }

        Command::Screenshot { filename } => {
            let path = commands::screenshot::run(&client, filename.as_deref()).await?;
            if json_mode {
                json_output(&json!({"ok": true, "path": path}));
            } else {
                println!("{path}");
            }
        }

        Command::Eval { expression } => {
            if json_mode {
                let raw = commands::eval::run_raw(&client, &expression).await?;
                json_output(&json!({"ok": true, "result": raw}));
            } else {
                let result = commands::eval::run(&client, &expression).await?;
                println!("{result}");
            }
        }

        Command::Wait { what, pattern, timeout } => {
            let msg = commands::wait::run(&client, &what, &pattern, timeout).await?;
            if json_mode {
                json_output(&json!({"ok": true, "message": msg}));
            } else {
                println!("{msg}");
            }
        }

        Command::Type { text } => {
            crate::element::type_text(&client, &text).await?;
            let msg = format!("Typed {} chars", text.len());
            if json_mode {
                json_output(&json!({"ok": true, "message": msg}));
            } else {
                println!("{msg}");
            }
        }

        Command::Press { key } => {
            crate::element::press_key(&client, &key).await?;
            let msg = format!("Pressed {key}");
            if json_mode {
                json_output(&json!({"ok": true, "message": msg}));
            } else {
                println!("{msg}");
            }
        }

        Command::Scroll { target } => {
            let msg = match target.as_str() {
                "down" => {
                    let _: serde_json::Value = client
                        .call("Runtime.evaluate", json!({
                            "expression": "window.scrollBy(0, 500)",
                            "returnByValue": true,
                        }))
                        .await?;
                    "Scrolled down".to_string()
                }
                "up" => {
                    let _: serde_json::Value = client
                        .call("Runtime.evaluate", json!({
                            "expression": "window.scrollBy(0, -500)",
                            "returnByValue": true,
                        }))
                        .await?;
                    "Scrolled up".to_string()
                }
                uid => {
                    let uid_map = get_uid_map(&store, &cli.browser, &cli.page);
                    let element_ref = uid_map.get(uid).ok_or_else(|| {
                        format!("Element uid={uid} not found. Run 'aibrowsr inspect' to get fresh uids.")
                    })?;
                    let backend_node_id = element_ref.backend_node_id().ok_or_else(|| {
                        format!("Element uid={uid} has no resolvable backend node.")
                    })?;
                    let resolve_result: crate::cdp::types::ResolveNodeResult = client
                        .call(
                            "DOM.resolveNode",
                            crate::cdp::types::ResolveNodeParams {
                                node_id: None,
                                backend_node_id: Some(backend_node_id),
                                object_group: Some("aibrowsr".into()),
                                execution_context_id: None,
                            },
                        )
                        .await?;
                    let object_id = resolve_result.object.object_id.ok_or_else(|| {
                        format!("Element uid={uid} could not be resolved to a JS object.")
                    })?;
                    let _: serde_json::Value = client
                        .call(
                            "Runtime.callFunctionOn",
                            json!({
                                "objectId": object_id,
                                "functionDeclaration": "function() { this.scrollIntoView({block: 'center'}); }",
                                "returnByValue": true,
                            }),
                        )
                        .await?;
                    format!("Scrolled uid={uid} into view")
                }
            };
            if json_mode {
                json_output(&json!({"ok": true, "message": msg}));
            } else {
                println!("{msg}");
            }
        }

        Command::Hover { uid } => {
            let uid_map = get_uid_map(&store, &cli.browser, &cli.page);
            crate::element::hover(&client, &uid_map, &uid).await?;
            let msg = format!("Hovered uid={uid}");
            if json_mode {
                json_output(&json!({"ok": true, "message": msg}));
            } else {
                println!("{msg}");
            }
        }

        Command::Tabs => {
            if json_mode {
                let tabs = commands::tabs::run_structured(&browser_client).await?;
                json_output(&json!({"ok": true, "tabs": tabs}));
            } else {
                let output = commands::tabs::run(&browser_client).await?;
                print!("{output}");
            }
        }

        // Already handled above
        Command::Daemon { .. } | Command::Status | Command::Stop | Command::Close => {
            unreachable!()
        }
    }

    // Save session
    session::save_session(&store)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Print a `serde_json::Value` as a single compact JSON line to stdout.
fn json_output(value: &serde_json::Value) {
    println!("{}", serde_json::to_string(value).unwrap_or_default());
}

/// Provide a contextual hint for common errors.
fn error_hint(msg: &str) -> Option<&'static str> {
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
        Some("Element may be hidden. Try scrolling.")
    } else {
        None
    }
}

/// Get the uid_map from the current session, or empty if none.
fn get_uid_map(store: &SessionStore, browser_name: &str, page_name: &str) -> HashMap<String, ElementRef> {
    store
        .browsers
        .get(browser_name)
        .and_then(|b| b.pages.get(page_name))
        .map(|p| p.uid_map.clone())
        .unwrap_or_default()
}

/// Resolve the page target id: use existing from session, or pick first page, or create one.
///
/// For the "default" page: reuse the first existing Chrome tab.
/// For named pages: always create a new tab (proper multi-tab support).
async fn resolve_page_target(
    client: &CdpClient,
    browser_session: &mut BrowserSession,
    page_name: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    // Check if we have a stored page with this name
    if let Some(page) = browser_session.pages.get(page_name) {
        return Ok(page.target_id.clone());
    }

    // For "default" page: try to reuse the first existing Chrome tab
    if page_name == "default" {
        let result: crate::cdp::types::GetTargetsResult = client
            .call("Target.getTargets", serde_json::json!({}))
            .await?;

        // Only reuse tabs that aren't already claimed by another named page
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

    // Create a new tab for this named page
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

async fn cmd_status(json_mode: bool) -> Result<(), Box<dyn std::error::Error>> {
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

async fn cmd_stop(json_mode: bool) -> Result<(), Box<dyn std::error::Error>> {
    let socket_path = session::daemon_socket_path()?;
    if !socket_path.exists() {
        if json_mode {
            json_output(&json!({"ok": true, "message": "Daemon is not running."}));
        } else {
            println!("Daemon is not running.");
        }
        return Ok(());
    }

    // Send stop command
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
}

async fn cmd_close(browser_name: &str, json_mode: bool) -> Result<(), Box<dyn std::error::Error>> {
    let mut store = session::load_session()?;

    let browser = store.browsers.remove(browser_name);

    let message = match browser {
        Some(b) => {
            // Kill the browser process if we manage it
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

    if json_mode {
        json_output(&json!({"ok": true, "message": message}));
    } else {
        println!("{message}");
    }

    session::save_session(&store)?;
    Ok(())
}
