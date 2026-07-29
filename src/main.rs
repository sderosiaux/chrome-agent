mod base64;
mod browser;
mod cdp;
mod cli;
mod commands;
#[cfg(unix)]
mod daemon;
mod element;
mod element_ref;
mod element_selector;
mod element_controls;
mod geometry;
mod pipe;
mod pipe_dispatch;
mod pipe_dispatch_actions;
mod pipe_report;
mod run;
mod run_helpers;
mod session;
mod setup;
mod snapshot;
mod truncate;
mod verdict;

/// Shared error type alias used across the crate.
pub(crate) type BoxError = Box<dyn std::error::Error>;

use clap::Parser;
use serde_json::json;

use crate::cli::Cli;
use crate::run_helpers::error_hint;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let json_mode = cli.json;

    // Clean up this invocation's managed Chrome on Ctrl+C — and only this one. The
    // handler used to walk every entry in the shared sessions.json and kill each pid
    // raw, so interrupting one agent killed every other agent's browser mid-task and
    // bypassed the PID-reuse guard every other kill path goes through. Installed after
    // parsing because it needs to know which browser is ours.
    let interrupted_browser = cli.browser.clone();
    tokio::spawn(async move {
        if matches!(tokio::signal::ctrl_c().await, Ok(())) {
            if let Ok(store) = session::load_session()
                && let Some(pid) = run_helpers::interrupt_kill_target(&store, &interrupted_browser) {
                    run_helpers::kill_pid(pid);
                }
            std::process::exit(130);
        }
    });

    if let Err(e) = run::run(cli).await {
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
        std::process::exit(1);
    }
}
