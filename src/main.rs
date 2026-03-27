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
Workflow: snap → read uids → act (click/fill) → snap again.\n\
Use --snap on action commands to combine action + observation in one call.";

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

    /// Launch browser in headless mode
    #[arg(long)]
    headless: bool,

    /// Global timeout in seconds for page loads
    #[arg(long, default_value = "30")]
    timeout: u64,

    /// Ignore HTTPS certificate errors
    #[arg(long)]
    ignore_https_errors: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Navigate to a URL
    Goto {
        /// Target URL
        url: String,
        /// Take a snapshot after navigation
        #[arg(long)]
        snap: bool,
    },

    /// Click an element by uid
    Click {
        /// Element uid (e.g. "e5")
        uid: String,
        /// Take a snapshot after clicking
        #[arg(long)]
        snap: bool,
    },

    /// Fill an input element by uid
    Fill {
        /// Element uid (e.g. "e5")
        uid: String,
        /// Value to fill
        value: String,
        /// Take a snapshot after filling
        #[arg(long)]
        snap: bool,
    },

    /// Fill multiple form fields at once
    #[command(name = "fill-form")]
    FillForm {
        /// uid=value pairs (e.g. "e5=hello" "e7=world")
        pairs: Vec<String>,
        /// Take a snapshot after filling
        #[arg(long)]
        snap: bool,
    },

    /// Take an accessibility tree snapshot
    Snap {
        /// Include ignored/generic nodes
        #[arg(long)]
        verbose: bool,
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
    let cli = Cli::parse();

    if let Err(e) = run(cli).await {
        eprintln!("error: {e}");
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
            return cmd_status().await;
        }

        Command::Stop => {
            return cmd_stop().await;
        }

        Command::Close => {
            return cmd_close(&cli.browser).await;
        }

        _ => {}
    }

    // All other commands need a browser connection + CDP client
    let mut store = session::load_session()?;
    session::cleanup_stale(&mut store);

    // Resolve browser
    let opts = BrowserOptions {
        name: cli.browser.clone(),
        headless: cli.headless,
        ignore_https_errors: cli.ignore_https_errors,
        connect: cli.connect.clone(),
    };

    let conn = browser::resolve_browser(&opts).await?;

    // Connect CDP
    let client = CdpClient::connect(&conn.ws_endpoint).await?;
    client.enable("Page").await?;
    client.enable("Runtime").await?;

    // Ensure browser session
    let browser_session = session::ensure_browser(
        &mut store,
        &cli.browser,
        &conn.ws_endpoint,
        conn.pid,
        cli.headless,
    );

    // Resolve page target — use first existing page or create one
    let target_id = resolve_page_target(&client, browser_session).await?;

    // Attach to target to get a session-scoped client
    // For now we operate on the browser-level endpoint which works for single-tab usage

    // Execute command
    match cli.command {
        Command::Goto { url, snap } => {
            let result = commands::goto::run(&client, &url).await?;
            println!("{} — {}", result.url, result.title);

            // Update session
            let page = session::ensure_page(
                store.browsers.get_mut(&cli.browser).unwrap(),
                "default",
                &target_id,
            );

            if snap {
                let snapshot = commands::snap::run(&client, false).await?;
                page.uid_map = snapshot.uid_map;
                println!("{}", snapshot.text);
            }
        }

        Command::Click { uid, snap } => {
            let uid_map = get_uid_map(&store, &cli.browser);
            let msg = commands::click::run(&client, &uid_map, &uid).await?;
            println!("{msg}");

            if snap {
                let snapshot = commands::snap::run(&client, false).await?;
                if let Some(browser_s) = store.browsers.get_mut(&cli.browser) {
                    let page = session::ensure_page(browser_s, "default", &target_id);
                    page.uid_map = snapshot.uid_map;
                }
                println!("{}", snapshot.text);
            }
        }

        Command::Fill { uid, value, snap } => {
            let uid_map = get_uid_map(&store, &cli.browser);
            let msg = commands::fill::run(&client, &uid_map, &uid, &value).await?;
            println!("{msg}");

            if snap {
                let snapshot = commands::snap::run(&client, false).await?;
                if let Some(browser_s) = store.browsers.get_mut(&cli.browser) {
                    let page = session::ensure_page(browser_s, "default", &target_id);
                    page.uid_map = snapshot.uid_map;
                }
                println!("{}", snapshot.text);
            }
        }

        Command::FillForm { pairs, snap } => {
            let uid_map = get_uid_map(&store, &cli.browser);
            let parsed: Result<Vec<(&str, &str)>, _> = pairs
                .iter()
                .map(|p| {
                    p.split_once('=')
                        .ok_or_else(|| format!("Invalid pair (expected uid=value): {p}"))
                })
                .collect();
            let parsed = parsed?;

            let msg = commands::fill::run_form(&client, &uid_map, &parsed).await?;
            println!("{msg}");

            if snap {
                let snapshot = commands::snap::run(&client, false).await?;
                if let Some(browser_s) = store.browsers.get_mut(&cli.browser) {
                    let page = session::ensure_page(browser_s, "default", &target_id);
                    page.uid_map = snapshot.uid_map;
                }
                println!("{}", snapshot.text);
            }
        }

        Command::Snap { verbose } => {
            let snapshot = commands::snap::run(&client, verbose).await?;
            if let Some(browser_s) = store.browsers.get_mut(&cli.browser) {
                let page = session::ensure_page(browser_s, "default", &target_id);
                page.uid_map = snapshot.uid_map;
            }
            println!("{}", snapshot.text);
        }

        Command::Screenshot { filename } => {
            let path = commands::screenshot::run(
                &client,
                filename.as_deref(),
            )
            .await?;
            println!("{path}");
        }

        Command::Eval { expression } => {
            let result = commands::eval::run(&client, &expression).await?;
            println!("{result}");
        }

        Command::Tabs => {
            let output = commands::tabs::run(&client).await?;
            print!("{output}");
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

/// Get the uid_map from the current session, or empty if none.
fn get_uid_map(store: &SessionStore, browser_name: &str) -> HashMap<String, ElementRef> {
    store
        .browsers
        .get(browser_name)
        .and_then(|b| b.pages.get("default"))
        .map(|p| p.uid_map.clone())
        .unwrap_or_default()
}

/// Resolve the page target id: use existing from session, or pick first page, or create one.
async fn resolve_page_target(
    client: &CdpClient,
    browser_session: &mut BrowserSession,
) -> Result<String, Box<dyn std::error::Error>> {
    // Check if we have a stored page
    if let Some(page) = browser_session.pages.get("default") {
        return Ok(page.target_id.clone());
    }

    // List targets and pick the first page
    let result: crate::cdp::types::GetTargetsResult = client
        .call("Target.getTargets", serde_json::json!({}))
        .await?;

    let page_target = result
        .target_infos
        .iter()
        .find(|t| t.target_type == "page");

    if let Some(target) = page_target {
        let target_id = target.target_id.clone();
        session::ensure_page(browser_session, "default", &target_id);
        return Ok(target_id);
    }

    // No pages — create one
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
    session::ensure_page(browser_session, "default", &target_id);
    Ok(target_id)
}

async fn cmd_status() -> Result<(), Box<dyn std::error::Error>> {
    let store = session::load_session()?;

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

    let daemon_alive = session::daemon_socket_exists();
    println!(
        "daemon: {}",
        if daemon_alive { "running" } else { "stopped" }
    );

    Ok(())
}

async fn cmd_stop() -> Result<(), Box<dyn std::error::Error>> {
    let socket_path = session::daemon_socket_path()?;
    if !socket_path.exists() {
        println!("Daemon is not running.");
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

    println!("Daemon stopped.");
    Ok(())
}

async fn cmd_close(browser_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut store = session::load_session()?;

    let browser = store.browsers.remove(browser_name);

    match browser {
        Some(b) => {
            // Kill the browser process if we manage it
            if let Some(pid) = b.pid {
                #[cfg(unix)]
                unsafe {
                    libc::kill(pid as libc::pid_t, libc::SIGTERM);
                }
                #[cfg(not(unix))]
                {
                    let _ = pid;
                }
                println!("Closed browser={browser_name} (pid={pid})");
            } else {
                println!("Removed external browser session: {browser_name}");
            }
        }
        None => {
            println!("No browser session named '{browser_name}'.");
        }
    }

    session::save_session(&store)?;
    Ok(())
}
