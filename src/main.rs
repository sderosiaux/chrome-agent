mod base64;
mod browser;
mod cdp;
mod chrome_args;
mod cli;
mod cli_actions;
mod commands;
mod connect_cli;
#[cfg(unix)]
mod daemon;
mod element;
mod element_controls;
mod element_pointer;
mod element_ref;
mod element_selector;
mod emulation;
mod geometry;
mod hints;
mod hit_test;
mod hit_test_report;
mod kill;
mod landing;
mod macros;
mod macros_cmd;
mod macros_record;
mod macros_run;
mod orphans;
mod page_ctx;
mod pipe;
mod pipe_command;
mod pipe_dispatch;
mod pipe_dispatch_actions;
mod pipe_emulation;
mod pipe_report;
mod profiles;
mod read_back;
mod render;
mod run;
mod run_helpers;
mod serving;
mod session;
mod session_load;
mod session_save;
mod setup;
mod snapshot;
mod snapshot_render;
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
    // Not `Cli::parse()`: clap exits 2 on a usage error, and 2 is reserved for "a claim this
    // tool made did not hold" — an assertion (`commands::assert`) or a macro guard
    // (`macros_run`). Usage errors exit 1; `--help`/`--version` exit 0.
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
                // `hints` rewrites the flag-position error, whose clap tip misleads;
                // every other error passes through unchanged.
                let argv: Vec<String> = std::env::args().collect();
                eprint!("{}", hints::usage_error(&e.to_string(), &argv));
            } else {
                let _ = e.print();
            }
            std::process::exit(i32::from(usage));
        }
    };
    let json_mode = cli.json;
    // Captured before `run` consumes `cli`: error hints name a command, and it must target
    // the browser this invocation drove, not the default one.
    let browser = cli.browser.clone();

    // Parse and stop, without launching a browser: lets the suite check the embedded guide's
    // examples against clap. An env var, not a flag — it is a test affordance.
    if std::env::var_os("CHROME_AGENT_PARSE_ONLY").is_some() {
        return;
    }

    // On Ctrl+C, clean up this invocation's managed Chrome and only this one; other agents
    // share sessions.json. Installed after parsing, because `--browser` is global and a
    // command like `daemon start` carries a name for a browser it never launched.
    let interrupted_browser =
        run_helpers::interrupt_owns_browser(&cli.command).then(|| cli.browser.clone());
    tokio::spawn(async move {
        if matches!(tokio::signal::ctrl_c().await, Ok(())) {
            if let Some(name) = interrupted_browser
                && let Ok(store) = session::load_session()
                && let Some(pid) = run_helpers::interrupt_kill_target(&store, &name)
            {
                run_helpers::kill_pid(pid);
            }
            // The store only knows browsers that reached it: interrupting a cold start
            // before its first save leaves a Chrome no later command could name.
            kill::reap_unpersisted();
            std::process::exit(130);
        }
    });

    if let Err(e) = run::run(cli).await {
        // Same window, reached by returning rather than by signal: the launch succeeded and
        // the connect failed. A browser that DID reach the store is left alone.
        kill::reap_unpersisted();
        // Exit 2 for "a claim this tool made did not hold", distinct from 1 "the browser never
        // started". Checked before the generic handler, which would exit 1.
        if let Some(not_held) = e.downcast_ref::<commands::assert::NotHeld>() {
            std::process::exit(not_held.report());
        }
        // A stopped macro has its own report (step, guard, observation, `next`). Printed
        // here so the handler below does not flatten it to its first sentence. A macro guard
        // is the same claim class as an assertion, so a guard that ran and did not hold exits
        // 2 as well; a step that never ran exits 1. `Stopped` reads that off its report.
        if let Some(stopped) = e.downcast_ref::<macros_run::Stopped>() {
            std::process::exit(stopped.report());
        }
        // A CLI batch stopped by `--stop-on-error` already printed its one response; only the
        // exit code is left to say. 1, not 2: 2 is reserved for "the page is not in that
        // state", and a stopped batch is an error like any other.
        if e.downcast_ref::<run_helpers::BatchStopped>().is_some() {
            std::process::exit(1);
        }
        // `--on-intercept refuse` carries who was in the way, so it prints structured rather
        // than as a bare sentence. Still `ok:false` and exit 1: nothing was dispatched.
        if let Some(refused) = hit_test::refusal_in(&e) {
            if json_mode {
                println!("{}", refused.to_json(&browser));
            } else {
                eprintln!("error: {refused}");
                for line in refused.text_lines(&browser) {
                    eprintln!("{line}");
                }
            }
            std::process::exit(1);
        }
        let msg = e.to_string();
        if json_mode {
            let hint = error_hint(&msg, &browser);
            let mut obj = json!({"ok": false, "error": msg});
            if let Some(h) = hint {
                obj["hint"] = json!(h);
            }
            // Through the one writer, so a serialization failure still answers `ok:false`
            // instead of an empty line.
            run_helpers::json_output(&obj);
        } else {
            eprintln!("error: {msg}");
            if let Some(hint) = error_hint(&msg, &browser) {
                eprintln!("hint: {hint}");
            }
        }
        std::process::exit(1);
    }
}
