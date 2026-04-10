mod browser;
mod cdp;
mod cli;
mod commands;
#[cfg(unix)]
mod daemon;
mod element;
mod element_ref;
mod pipe;
mod pipe_dispatch;
mod run;
mod run_helpers;
mod session;
mod setup;
mod snapshot;
mod truncate;

/// Shared error type alias used across the crate.
pub(crate) type BoxError = Box<dyn std::error::Error>;

use clap::Parser;
use serde_json::json;

use crate::cli::Cli;
use crate::run_helpers::error_hint;

#[tokio::main]
async fn main() {
    // Install signal handler so managed Chrome is cleaned up on Ctrl+C
    tokio::spawn(async {
        if matches!(tokio::signal::ctrl_c().await, Ok(())) {
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
            std::process::exit(130);
        }
    });

    let cli = Cli::parse();
    let json_mode = cli.json;

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
        if !json_mode {
            std::process::exit(1);
        }
    }
}
