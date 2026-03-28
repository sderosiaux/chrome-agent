mod browser;
mod cdp;
mod commands;
#[cfg(unix)]
mod daemon;
mod element;
mod element_ref;
mod pipe;
mod run_helpers;
mod session;
mod setup;
mod snapshot;
mod truncate;

/// Shared error type alias used across the crate.
pub(crate) type BoxError = Box<dyn std::error::Error>;

use clap::{Parser, Subcommand};
use serde_json::json;

use crate::browser::BrowserOptions;
use crate::cdp::client::CdpClient;
use crate::run_helpers::{cmd_close, cmd_status, cmd_stop, connect_page, error_hint, get_uid_map, json_output, output_action, output_goto, resolve_page_target};

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

const CLI_LONG_ABOUT: &str = "\
aibrowsr — browser automation for AI agents. Controls Chrome via CDP.\n\
Single binary, zero runtime dependencies. Named pages persist between invocations.\n\
Use --stealth to bypass bot detection (Cloudflare, Turnstile).\n\
Use --copy-cookies to access sites where you're already logged in (X.com, Gmail).\n\
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
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct Cli {
    /// Named browser profile (default: "default")
    #[arg(long, default_value = "default")]
    pub(crate) browser: String,

    /// Connect to existing browser: ws:// URL, http:// URL, or "auto"
    #[arg(long)]
    pub(crate) connect: Option<String>,

    /// Launch browser with a visible window (default is headless)
    #[arg(long)]
    pub(crate) headed: bool,

    /// Global timeout in seconds for page loads
    #[arg(long, default_value = "30")]
    pub(crate) timeout: u64,

    /// Ignore HTTPS certificate errors
    #[arg(long)]
    pub(crate) ignore_https_errors: bool,

    /// Output structured JSON instead of text
    #[arg(long)]
    pub(crate) json: bool,

    /// Stealth mode: 7 anti-detection patches (webdriver, UA, WebGL, input leak, Runtime.enable skipped)
    #[arg(long)]
    pub(crate) stealth: bool,

    /// Max depth for --inspect output (used by goto, click, fill, etc.)
    #[arg(long)]
    pub(crate) max_depth: Option<usize>,

    /// Copy cookies from your real Chrome profile (uses your logged-in sessions)
    #[arg(long)]
    pub(crate) copy_cookies: bool,

    /// Named page/tab within the browser (default: "default")
    #[arg(long, default_value = "default")]
    pub(crate) page: String,

    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Navigate to a URL
    #[command(alias = "navigate", alias = "open", alias = "go")]
    Goto {
        /// Target URL
        url: String,
        /// Inspect page after navigation
        #[arg(long)]
        inspect: bool,
        /// Max depth for inspect output (also accepted as global flag)
        #[arg(long)]
        max_depth: Option<usize>,
        /// Wait for a CSS selector to appear after navigation
        #[arg(long)]
        wait_for: Option<String>,
    },

    /// Click an element by uid, CSS selector, or coordinates
    #[command(alias = "tap")]
    Click {
        /// Element uid (e.g. "n47") — omit if using --selector or --xy
        uid: Option<String>,
        /// CSS selector to click
        #[arg(long)]
        selector: Option<String>,
        /// Click at x,y coordinates (e.g. --xy 100,200)
        #[arg(long, value_delimiter = ',')]
        xy: Option<Vec<f64>>,
        /// Inspect page after clicking
        #[arg(long)]
        inspect: bool,
        /// Max depth for inspect output (also accepted as global flag)
        #[arg(long)]
        max_depth: Option<usize>,
    },

    /// Fill an input element by uid or CSS selector
    Fill {
        /// Value to fill
        value: String,
        /// Element uid (e.g. "n47") — omit if using --selector
        #[arg(long)]
        uid: Option<String>,
        /// CSS selector to fill
        #[arg(long)]
        selector: Option<String>,
        /// Inspect page after filling
        #[arg(long)]
        inspect: bool,
        /// Max depth for inspect output (also accepted as global flag)
        #[arg(long)]
        max_depth: Option<usize>,
    },

    /// Fill multiple form fields at once
    #[command(name = "fill-form")]
    FillForm {
        /// uid=value pairs (e.g. "e5=hello" "e7=world")
        pairs: Vec<String>,
        /// Inspect page after filling
        #[arg(long)]
        inspect: bool,
        /// Max depth for inspect output (also accepted as global flag)
        #[arg(long)]
        max_depth: Option<usize>,
    },

    /// Extract visible text from the page or an element
    Text {
        /// Element uid to extract text from (default: entire page)
        uid: Option<String>,
        /// CSS selector to extract text from (e.g. "article", ".content")
        #[arg(long)]
        selector: Option<String>,
        /// Truncate output to N characters (appends "..." if truncated)
        #[arg(long)]
        truncate: Option<usize>,
    },

    /// Extract main content using Readability (Mozilla's reader mode)
    Read {
        /// Return cleaned HTML instead of plain text
        #[arg(long)]
        html: bool,
        /// Truncate output to N characters
        #[arg(long)]
        truncate: Option<usize>,
    },

    /// Navigate back in browser history
    Back,

    /// Take an accessibility tree inspection
    #[command(alias = "snap", alias = "snapshot", alias = "tree")]
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
        /// Only show nodes matching these roles (comma-separated, e.g. "button,link,textbox")
        #[arg(long)]
        filter: Option<String>,
    },

    /// Show what changed since the last inspect
    Diff,

    /// Capture a screenshot
    #[command(alias = "capture")]
    Screenshot {
        /// Output filename (default: timestamped)
        #[arg(long)]
        filename: Option<String>,
    },

    /// Auto-extract structured data from repeating page elements (lists, tables, cards)
    Extract {
        /// CSS selector to scope extraction (e.g. "main", ".results")
        #[arg(long)]
        selector: Option<String>,
        /// Max items to extract
        #[arg(long, default_value = "10")]
        limit: usize,
        /// Scroll to load lazy content before extracting (useful for infinite-scroll pages)
        #[arg(long)]
        scroll: bool,
    },

    /// Evaluate JavaScript in the page
    #[command(alias = "js", alias = "execute")]
    Eval {
        /// JS expression to evaluate (if --selector, `el` is the matched element)
        expression: String,
        /// CSS selector — the matched element is available as `el` in the expression
        #[arg(long)]
        selector: Option<String>,
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

    /// Type text into the focused element (or focus a selector first)
    Type {
        /// Text to type
        text: String,
        /// CSS selector to focus before typing
        #[arg(long)]
        selector: Option<String>,
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
        /// Element uid (e.g. "n47")
        uid: String,
    },

    /// Capture network requests (API responses, XHR, fetch)
    Network {
        /// URL pattern to filter (case-insensitive contains match)
        #[arg(long)]
        filter: Option<String>,
        /// Include response bodies (JSON/text only, truncated to 2000 chars)
        #[arg(long)]
        body: bool,
        /// Capture live traffic for N seconds (default: show already-loaded resources via Performance API)
        #[arg(long)]
        live: Option<u64>,
        /// Max entries to show
        #[arg(long, default_value = "50")]
        limit: usize,
    },

    /// Show captured console messages and JS errors
    Console {
        /// Filter by level: log, warn, error, info, exception
        #[arg(long)]
        level: Option<String>,
        /// Clear captured messages after reading
        #[arg(long)]
        clear: bool,
        /// Max entries to show
        #[arg(long, default_value = "50")]
        limit: usize,
    },

    /// Replay a recorded session file
    Replay {
        /// Path to the recording file
        file: String,
        /// Variable substitutions (key=value, comma-separated)
        #[arg(long, value_delimiter = ',')]
        vars: Option<Vec<String>>,
    },

    /// Show browsing history
    History {
        /// Filter entries by URL pattern (case-insensitive)
        #[arg(long)]
        filter: Option<String>,
        /// Max entries to show
        #[arg(long, default_value = "20")]
        limit: usize,
    },

    /// Persistent connection mode — read JSON commands from stdin (one per line)
    Pipe,

    /// List open browser tabs
    Tabs,

    /// Close the managed browser
    Close {
        /// Also delete the browser profile (cookies, cache, data)
        #[arg(long)]
        purge: bool,
    },

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
        if matches!(tokio::signal::ctrl_c().await, Ok(())) {
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
        let msg = e.to_string();
        if json_mode {
            let hint = error_hint(&msg);
            let mut obj = json!({"ok": false, "error": msg});
            if let Some(h) = hint {
                obj["hint"] = json!(h);
            }
            println!("{}", serde_json::to_string(&obj).unwrap_or_default());
        } else {
            eprintln!("error: {msg}");
            if let Some(hint) = error_hint(&msg) {
                eprintln!("hint: {hint}");
            }
        }
        if !json_mode {
            std::process::exit(1);
        }
        // JSON mode: exit 0 so agents can parse {"ok":false} without exit code checks
    }
}

async fn run(cli: Cli) -> Result<(), BoxError> {
    match cli.command {
        Command::Daemon { action } => {
            match action {
                DaemonAction::Start => {
                    #[cfg(unix)]
                    {
                        let socket_path = session::daemon_socket_path()?;
                        daemon::run_daemon(&socket_path).await?;
                    }
                    #[cfg(not(unix))]
                    {
                        return Err("Daemon is not supported on Windows. Commands work without a daemon.".into());
                    }
                }
            }
            return Ok(());
        }

        Command::Status => {
            return cmd_status(cli.json);
        }

        Command::Stop => {
            return cmd_stop(cli.json).await;
        }

        Command::Close { purge } => {
            return cmd_close(&cli.browser, purge, cli.json);
        }

        Command::Pipe => {
            return pipe::run_pipe(&cli).await;
        }

        Command::Replay { ref file, ref vars } => {
            return pipe::run_replay(&cli, file, vars.as_deref()).await;
        }

        Command::History { ref filter, limit } => {
            let entries = commands::history::run(filter.as_deref(), limit)?;
            if cli.json {
                let entries_json: Vec<serde_json::Value> = entries
                    .iter()
                    .map(|e| json!({"ts": e.ts, "url": e.url, "title": e.title, "page": e.page}))
                    .collect();
                json_output(&json!({"ok": true, "entries": entries_json}));
            } else {
                let text = commands::history::format_text(&entries);
                if text.is_empty() {
                    println!("No history entries found.");
                } else {
                    println!("{text}");
                }
            }
            return Ok(());
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
            if let Ok(client) = CdpClient::connect(ws).await {
                let conn = browser::BrowserConnection {
                    ws_endpoint: ws.clone(),
                    http_endpoint: Some(http),
                    pid: existing.pid,
                };
                (conn, client)
            } else {
                store.browsers.remove(&cli.browser);
                let opts = BrowserOptions {
                    name: cli.browser.clone(),
                    headless: want_headless,
                    ignore_https_errors: cli.ignore_https_errors,
                    stealth: cli.stealth,
                    connect: cli.connect.clone(),
                    copy_cookies: cli.copy_cookies,
                };
                let conn = browser::resolve_browser(&opts).await?;
                let client = CdpClient::connect(&conn.ws_endpoint).await?;
                (conn, client)
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
                stealth: cli.stealth,
                connect: cli.connect.clone(),
                    copy_cookies: cli.copy_cookies,
            };
            let conn = browser::resolve_browser(&opts).await?;
            let client = CdpClient::connect(&conn.ws_endpoint).await?;
            (conn, client)
        }
    } else {
        // No existing session. Only auto-launch Chrome for commands that navigate
        // (goto, pipe). For action commands (click, extract, inspect, etc.),
        // a missing session means the user forgot to `goto` first.
        let needs_existing = !matches!(
            cli.command,
            Command::Goto { .. } | Command::Pipe
        );
        if needs_existing {
            return Err(format!(
                "No browser session '{}'. Run `aibrowsr --browser {} goto <url>` first.",
                cli.browser, cli.browser
            ).into());
        }
        let opts = BrowserOptions {
            name: cli.browser.clone(),
            headless: want_headless,
            ignore_https_errors: cli.ignore_https_errors,
            stealth: cli.stealth,
            connect: cli.connect.clone(),
                    copy_cookies: cli.copy_cookies,
        };
        let conn = browser::resolve_browser(&opts).await?;
        let client = CdpClient::connect(&conn.ws_endpoint).await?;
        (conn, client)
    };

    let http_endpoint = conn.http_endpoint.as_deref().ok_or({
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
    let _ = session::save_session(&mut store);

    // Connect page-level CDP with retry + full setup (Page.enable, console, stealth)
    let client = connect_page(http_endpoint, &target_id, cli.stealth).await?;

    // Execute command
    let json_mode = cli.json;
    match cli.command {
        Command::Goto { url, inspect, max_depth, wait_for } => {
            let depth = max_depth.or(cli.max_depth);
            let result = commands::goto::run(&client, &url, cli.timeout).await?;
            // Wait for a selector to appear if requested
            if let Some(ref selector) = wait_for {
                commands::wait::run(&client, "selector", selector, cli.timeout).await?;
            }
            // Log to browsing history
            let _ = commands::history::append(&result.url, &result.title, &cli.page);
            output_goto(&client, &mut store, &cli.browser, &cli.page, &target_id, &result.url, &result.title, inspect, depth, json_mode).await?;
        }

        Command::Click { uid, selector, xy, inspect, max_depth } => {
            let depth = max_depth.or(cli.max_depth);
            let provided = u8::from(uid.is_some()) + u8::from(selector.is_some()) + u8::from(xy.is_some());
            if provided == 0 {
                return Err("Provide a uid, --selector, or --xy to identify the click target.".into());
            }
            if provided > 1 {
                return Err("Only one of uid, --selector, or --xy can be provided.".into());
            }

            let msg = if let Some(ref sel) = selector {
                crate::element::click_selector(&client, sel).await?;
                format!("Clicked selector '{sel}'")
            } else if let Some(ref coords) = xy {
                if coords.len() != 2 {
                    return Err("--xy requires exactly 2 values: x,y".into());
                }
                crate::element::click_at_coords(&client, coords[0], coords[1]).await?;
                format!("Clicked at ({}, {})", coords[0], coords[1])
            } else {
                let uid = uid.as_ref().unwrap();
                let uid_map = get_uid_map(&store, &cli.browser, &cli.page);
                commands::click::run(&client, &uid_map, uid).await?
            };

            output_action(&client, &mut store, &cli.browser, &cli.page, &target_id, msg, inspect, depth, json_mode).await?;
        }

        Command::Fill { uid, selector, value, inspect, max_depth } => {
            let depth = max_depth.or(cli.max_depth);
            let provided = u8::from(uid.is_some()) + u8::from(selector.is_some());
            if provided == 0 {
                return Err("Provide --uid or --selector to identify the element.".into());
            }
            if provided > 1 {
                return Err("Only one of --uid or --selector can be provided.".into());
            }

            let msg = if let Some(ref sel) = selector {
                crate::element::fill_selector(&client, sel, &value).await?;
                format!("Filled selector '{sel}'")
            } else {
                let uid = uid.as_ref().unwrap();
                let uid_map = get_uid_map(&store, &cli.browser, &cli.page);
                commands::fill::run(&client, &uid_map, uid, &value).await?
            };

            output_action(&client, &mut store, &cli.browser, &cli.page, &target_id, msg, inspect, depth, json_mode).await?;
        }

        Command::FillForm { pairs, inspect, max_depth } => {
            let depth = max_depth.or(cli.max_depth);
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

            output_action(&client, &mut store, &cli.browser, &cli.page, &target_id, msg, inspect, depth, json_mode).await?;
        }

        Command::Text { uid, selector, truncate } => {
            if uid.is_some() && selector.is_some() {
                return Err("Only one of uid or --selector can be provided.".into());
            }
            let uid_map = get_uid_map(&store, &cli.browser, &cli.page);
            let text = commands::text::run(&client, uid.as_deref(), selector.as_deref(), &uid_map).await?;
            let full_length = text.chars().count();
            let (text, truncated) = if let Some(n) = truncate
                && full_length > n {
                    (crate::truncate::truncate_str(&text, n, "...").into_owned(), true)
                } else {
                    (text, false)
                };
            if json_mode {
                let mut obj = json!({"ok": true, "text": text});
                if truncated {
                    obj["truncated"] = json!(true);
                    obj["fullLength"] = json!(full_length);
                }
                json_output(&obj);
            } else {
                println!("{text}");
            }
        }

        Command::Read { html, truncate } => {
            let result = commands::read::run(&client, html, truncate).await?;
            if json_mode {
                let mut obj = json!({"ok": true, "title": result.title, "text": result.text_content});
                if let Some(excerpt) = &result.excerpt {
                    obj["excerpt"] = json!(excerpt);
                }
                if let Some(byline) = &result.byline {
                    obj["byline"] = json!(byline);
                }
                json_output(&obj);
            } else {
                if !result.title.is_empty() {
                    println!("# {}", result.title);
                    println!();
                }
                if html {
                    if let Some(content) = &result.content {
                        println!("{content}");
                    }
                } else {
                    println!("{}", result.text_content);
                }
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

        Command::Inspect { verbose, max_depth, uid, filter } => {
            let role_filter: Option<Vec<&str>> = filter.as_deref().map(|f| f.split(',').map(str::trim).collect());
            let snapshot = commands::inspect::run(&client, verbose, max_depth, uid.as_deref(), role_filter.as_deref()).await?;
            if let Some(browser_s) = store.browsers.get_mut(&cli.browser) {
                let page = session::ensure_page(browser_s, &cli.page, &target_id);
                page.uid_map = snapshot.uid_map;
                page.last_snapshot = Some(snapshot.text.clone());
            }
            if json_mode {
                json_output(&json!({"ok": true, "snapshot": snapshot.text}));
            } else {
                println!("{}", snapshot.text);
            }
        }

        Command::Diff => {
            let old_snapshot = store
                .browsers
                .get(&cli.browser)
                .and_then(|b| b.pages.get(&cli.page))
                .and_then(|p| p.last_snapshot.clone());
            let old_text = old_snapshot.ok_or("No previous snapshot. Run 'aibrowsr inspect' first.")?;
            let snapshot = commands::inspect::run(&client, false, None, None, None).await?;
            let diff = commands::diff::diff_snapshots(&old_text, &snapshot.text);
            let stats = commands::diff::diff_stats(&diff);
            // Update session with new snapshot
            if let Some(browser_s) = store.browsers.get_mut(&cli.browser) {
                let page = session::ensure_page(browser_s, &cli.page, &target_id);
                page.last_snapshot = Some(snapshot.text);
                page.uid_map = snapshot.uid_map;
            }
            if json_mode {
                json_output(&json!({
                    "ok": true,
                    "added": stats.added,
                    "removed": stats.removed,
                    "changed": stats.changed,
                    "diff": diff.trim_end(),
                }));
            } else {
                print!("{diff}");
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

        Command::Extract { selector, limit, scroll } => {
            if scroll {
                commands::extract::scroll_to_load(&client).await?;
            }
            let result = commands::extract::run(&client, selector.as_deref(), limit).await?;
            if json_mode {
                json_output(&commands::extract::to_json(&result));
            } else {
                print!("{}", commands::extract::format_text(&result));
            }
        }

        Command::Eval { expression, selector } => {
            // If --selector provided, wrap expression so `el` is the matched element
            let expr = if let Some(ref sel) = selector {
                let escaped = serde_json::to_string(sel).unwrap_or_default();
                format!("((el) => {{ if (!el) throw new Error('No element matches selector ' + {escaped}); return {expression} }})(document.querySelector({escaped}))")
            } else {
                expression
            };
            if json_mode {
                let raw = commands::eval::run_raw(&client, &expr).await?;
                json_output(&json!({"ok": true, "result": raw}));
            } else {
                let result = commands::eval::run(&client, &expr).await?;
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

        Command::Type { text, selector } => {
            if let Some(ref sel) = selector {
                crate::element::focus_selector(&client, sel).await?;
            }
            crate::element::type_text(&client, &text).await?;
            let msg = if selector.is_some() {
                format!("Typed {} chars into selector '{}'", text.len(), selector.as_ref().unwrap())
            } else {
                format!("Typed {} chars", text.len())
            };
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

        Command::Network { filter, body, live, limit } => {
            let entries = if let Some(secs) = live {
                if cli.stealth { eprintln!("warning: --live enables Network domain (detectable)"); }
                commands::network::run_live(&client, filter.as_deref(), body, limit, secs).await?
            } else {
                commands::network::run_retroactive(&client, filter.as_deref(), limit).await?
            };
            if json_mode {
                json_output(&json!({"ok": true, "requests": entries}));
            } else {
                println!("{}", commands::network::format_text(&entries));
            }
        }

        Command::Console { level, clear, limit } => {
            let entries = commands::console::run(&client, level.as_deref(), clear, limit).await?;
            if json_mode {
                let messages: Vec<serde_json::Value> = entries
                    .iter()
                    .map(|e| json!({"level": e.level, "message": e.message, "timestamp": e.timestamp}))
                    .collect();
                json_output(&json!({"ok": true, "messages": messages}));
            } else {
                println!("{}", commands::console::format_text(&entries));
            }
        }

        Command::Tabs => {
            if json_mode {
                let tabs = commands::tabs::run_structured(&browser_client, &store).await?;
                json_output(&json!({"ok": true, "tabs": tabs}));
            } else {
                let output = commands::tabs::run(&browser_client, &store).await?;
                print!("{output}");
            }
        }

        // Already handled above
        Command::Daemon { .. } | Command::Status | Command::Stop | Command::Close { .. }
        | Command::Pipe | Command::Replay { .. } | Command::History { .. } => {
            unreachable!()
        }
    }

    // Save session
    session::save_session(&mut store)?;

    Ok(())
}
