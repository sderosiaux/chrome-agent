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
mod emulation;
mod geometry;
mod hit_test;
mod hints;
mod kill;
mod landing;
mod orphans;
mod pipe;
mod pipe_dispatch;
mod pipe_dispatch_actions;
mod pipe_emulation;
mod pipe_report;
mod profiles;
mod read_back;
mod render;
mod run;
mod run_helpers;
mod session;
mod setup;
mod snapshot;
mod snapshot_secret;
mod truncate;
mod verdict;
mod verdict_evidence;
mod verdict_words;

/// Shared error type alias used across the crate.
pub(crate) type BoxError = Box<dyn std::error::Error>;

use clap::Parser;
use serde_json::json;

use crate::cli::Cli;
use crate::run_helpers::error_hint;

#[tokio::main]
async fn main() {
    // Not `Cli::parse()`: clap exits 2 on a usage error, and 2 now means "the assertion did
    // not hold" (`commands::assert`). A wrong flag is the caller's mistake, not a fact about
    // the page, so it joins every other operational failure at 1 and leaves 2 to mean one
    // thing. `--help`/`--version` still print to stdout and exit 0.
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            let usage = !matches!(
                e.kind(),
                clap::error::ErrorKind::DisplayHelp
                    | clap::error::ErrorKind::DisplayVersion
                    | clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
            );
            if usage {
                // One usage error is really about flag position, and clap's own tip for it sends
                // the reader off to escape an argument they never meant as a string. `hints`
                // rewrites that one and returns every other error unchanged. Help and version
                // still go through clap, on stdout.
                let argv: Vec<String> = std::env::args().collect();
                eprint!("{}", hints::usage_error(&e.to_string(), &argv));
            } else {
                let _ = e.print();
            }
            std::process::exit(i32::from(usage));
        }
    };
    let json_mode = cli.json;
    // Captured before `cli` is consumed by `run`: an error hint names a command to run, and
    // that command has to reach the browser THIS invocation drove, not the default one.
    let browser = cli.browser.clone();

    // Parse and stop. The embedded guide (`llm-guide.txt`, printed by `--help`) is what
    // an agent copies its invocations from, and it once documented a flag that did not
    // exist; checking it against the parser needs a way to reach clap's verdict —
    // including missing required arguments, which `--help` short-circuits past — without
    // launching a browser. Env var rather than a flag: this is a test affordance, not
    // part of the command surface.
    if std::env::var_os("CHROME_AGENT_PARSE_ONLY").is_some() {
        return;
    }

    // Clean up this invocation's managed Chrome on Ctrl+C — and only this one. The
    // handler used to walk every entry in the shared sessions.json and kill each pid
    // raw, so interrupting one agent killed every other agent's browser mid-task and
    // bypassed the PID-reuse guard every other kill path goes through. Installed after
    // parsing because it needs to know which browser is ours — and whether we own one
    // at all: `--browser` is global, so `daemon start` carries the default name for a
    // browser it never launched.
    let interrupted_browser = run_helpers::interrupt_owns_browser(&cli.command).then(|| cli.browser.clone());
    tokio::spawn(async move {
        if matches!(tokio::signal::ctrl_c().await, Ok(())) {
            if let Some(name) = interrupted_browser
                && let Ok(store) = session::load_session()
                && let Some(pid) = run_helpers::interrupt_kill_target(&store, &name) {
                    run_helpers::kill_pid(pid);
                }
            // The store answers only for browsers that reached it. Interrupting a cold
            // start before its first save — reproducible inside ~0.3 s — left a Chrome
            // running that no later command could name, which is how a headless browser
            // survives for 19 days. This reaps that one, and is a no-op once saved.
            kill::reap_unpersisted();
            std::process::exit(130);
        }
    });

    if let Err(e) = run::run(cli).await {
        // Same window, reached by returning rather than by signal: the launch succeeds and
        // then `CdpClient::connect` or `resolve_page_target` fails, so the browser is up and
        // the store never learned its pid. A browser that DID reach the store is left alone
        // — a failed command leaving a usable browser behind is the existing contract.
        kill::reap_unpersisted();
        // An assertion that did not hold is not a broken tool: it gets its own exit code so
        // a caller can tell "the page is not in that state" (2) from "the browser never
        // started" (1). Checked before the generic handler below, which would print it as a
        // failure and exit 1 — the very conflation the code exists to remove.
        if let Some(not_held) = e.downcast_ref::<commands::assert::NotHeld>() {
            std::process::exit(not_held.report());
        }
        let msg = e.to_string();
        if json_mode {
            let hint = error_hint(&msg, &browser);
            let mut obj = json!({"ok": false, "error": msg});
            if let Some(h) = hint {
                obj["hint"] = json!(h);
            }
            println!("{}", serde_json::to_string(&obj).unwrap_or_default());
        } else {
            eprintln!("error: {msg}");
            if let Some(hint) = error_hint(&msg, &browser) {
                eprintln!("hint: {hint}");
            }
        }
        std::process::exit(1);
    }
}
