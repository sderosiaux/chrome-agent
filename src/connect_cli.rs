//! Resolving a single CLI invocation's browser + page connection.
//!
//! Pipe and replay mode have their own equivalent (`pipe::connect_browser`): they read
//! commands from stdin instead of dispatching on `cli.command`.

use crate::BoxError;
use crate::browser::{self, BrowserOptions};
use crate::cdp::client::CdpClient;
use crate::cli::{Cli, Command};
use crate::run_helpers::{connect_page, resolve_page_target};
use crate::session::{self, SessionStore};

/// Which mode this invocation needs the named browser to run in.
///
/// `--headed` is a `SetTrue` flag with no negation and no default of its own, so `true` says
/// the caller wrote it on THIS command line — the one signal strong enough to overrule a
/// stored mode, and the relaunch it implies is what pipe mode already does. `false` is the
/// absence of a preference, not a request for headless: a stored mode wins, or every flagless
/// follow-up would kill the headed browser the previous command launched (the regression
/// cf1e8a8 fixed by reading the stored mode and, in doing so, made `--headed` unreachable
/// from the CLI — the flag parsed, changed nothing, and said nothing).
///
/// Consequence, stated: with no `--headless` flag to write, a named browser cannot be moved
/// back from headed to headless in place. `close` then relaunch does it.
const fn want_headless(headed_flag: bool, stored_headless: Option<bool>) -> bool {
    if headed_flag {
        return false;
    }
    match stored_headless {
        Some(headless) => headless,
        None => true,
    }
}

/// Load the session store, connect to (or launch) `cli.browser`, resolve `cli.page`'s
/// target, and connect its page-level CDP client.
///
/// Returns the store (the caller keeps mutating and saving it), the browser-level client
/// (`Target.*`), the page-level client, and the resolved target id.
pub async fn resolve_cli_connection(
    cli: &Cli,
) -> Result<(SessionStore, CdpClient, CdpClient, String), BoxError> {
    let mut store = session::load_session()?;
    let requested_proxy =
        browser::normalized_proxy_option(cli.connect.as_deref(), cli.proxy_server.as_deref())?;
    let requested_chrome_args =
        browser::normalized_chrome_args_option(cli.connect.as_deref(), &cli.chrome_args)?;

    let existing_mode = store.browsers.get(&cli.browser).map(|b| b.headless);
    let want_headless = want_headless(cli.headed, existing_mode);

    // A proxy is fixed at launch, so a relaunch inherits the stored one unless the caller
    // asked for a proxy explicitly; otherwise omitting the flag routes traffic directly.
    // An explicit mismatch is caught on the live path by `ensure_proxy_compatible`.
    let stored_proxy = store
        .browsers
        .get(&cli.browser)
        .and_then(|b| b.proxy_server.clone());
    let effective_proxy = requested_proxy.clone().or(stored_proxy);

    let effective_chrome_args =
        crate::chrome_args::effective_chrome_args(&store, &cli.browser, &requested_chrome_args);

    let (conn, browser_client) = if let Some(existing) = store.browsers.get(&cli.browser) {
        let mode_matches = existing.headless == want_headless;
        let ws = &existing.ws_endpoint;
        let http = browser::extract_http_from_ws(ws);

        if mode_matches {
            if let Ok(client) = CdpClient::connect(ws).await {
                session::ensure_proxy_compatible(existing, requested_proxy.as_deref())?;
                session::ensure_chrome_args_compatible(existing, &requested_chrome_args)?;
                let conn = browser::BrowserConnection {
                    ws_endpoint: ws.clone(),
                    http_endpoint: Some(http),
                    pid: existing.pid,
                };
                (conn, client)
            } else {
                // Unreachable: kill the pid before relaunching, or a live-but-unresponsive
                // Chrome is orphaned when its session entry is replaced. Waited out, or the
                // relaunch reconnects to the instance still tearing down.
                if let Some(pid) = existing.pid {
                    browser::kill_and_await_exit(&cli.browser, pid)?;
                }
                store.browsers.remove(&cli.browser);
                let opts = BrowserOptions {
                    name: cli.browser.clone(),
                    headless: want_headless,
                    ignore_https_errors: cli.ignore_https_errors,
                    stealth: cli.stealth,
                    connect: cli.connect.clone(),
                    proxy_server: effective_proxy.clone(),
                    copy_cookies: cli.copy_cookies,
                    chrome_args: effective_chrome_args.clone(),
                };
                let conn = Box::pin(browser::resolve_browser(&opts)).await?;
                let client = CdpClient::connect(&conn.ws_endpoint).await?;
                (conn, client)
            }
        } else {
            // The caller asked for the other mode: replace the browser, and wait for the one
            // being replaced to actually exit before resolving its successor.
            if let Some(pid) = existing.pid {
                browser::kill_and_await_exit(&cli.browser, pid)?;
            }
            store.browsers.remove(&cli.browser);
            let opts = BrowserOptions {
                name: cli.browser.clone(),
                headless: want_headless,
                ignore_https_errors: cli.ignore_https_errors,
                stealth: cli.stealth,
                connect: cli.connect.clone(),
                proxy_server: effective_proxy.clone(),
                copy_cookies: cli.copy_cookies,
                chrome_args: effective_chrome_args.clone(),
            };
            let conn = Box::pin(browser::resolve_browser(&opts)).await?;
            let client = CdpClient::connect(&conn.ws_endpoint).await?;
            (conn, client)
        }
    } else {
        let needs_existing = !matches!(cli.command, Command::Goto { .. } | Command::Pipe);
        if needs_existing {
            return Err(format!(
                "No browser session '{}'. Run `chrome-agent --browser {} goto <url>` first.",
                cli.browser, cli.browser
            )
            .into());
        }
        let opts = BrowserOptions {
            name: cli.browser.clone(),
            headless: want_headless,
            ignore_https_errors: cli.ignore_https_errors,
            stealth: cli.stealth,
            connect: cli.connect.clone(),
            proxy_server: effective_proxy.clone(),
            copy_cookies: cli.copy_cookies,
            chrome_args: effective_chrome_args.clone(),
        };
        let conn = Box::pin(browser::resolve_browser(&opts)).await?;
        let client = CdpClient::connect(&conn.ws_endpoint).await?;
        (conn, client)
    };

    // The Target.* client is bound by --timeout like the page client: its error message
    // says "raise --timeout", which must actually work.
    browser_client.set_call_timeout(std::time::Duration::from_secs(cli.timeout));

    let http_endpoint = conn
        .http_endpoint
        .as_deref()
        .ok_or("No HTTP endpoint available. Cannot resolve page WebSocket URL.")?;

    let target_id = {
        let browser_session = session::ensure_browser(
            &mut store,
            &cli.browser,
            &conn.ws_endpoint,
            conn.pid,
            want_headless,
            effective_proxy,
            effective_chrome_args,
        );
        resolve_page_target(&browser_client, browser_session, &cli.page).await?
    };
    let _ = session::save_session(&mut store);

    let client = Box::pin(connect_page(http_endpoint, &target_id, cli.stealth)).await?;

    Ok((store, browser_client, client, target_id))
}

#[cfg(test)]
mod tests {
    use super::want_headless;
    use clap::Parser as _;

    /// Both directions of the same decision, and they are not symmetric: the flag overrules a
    /// stored mode, its absence does not.
    #[test]
    fn an_explicit_headed_overrules_a_stored_mode_and_its_absence_does_not() {
        // What the CLI silently dropped: --headed against a stored headless browser must be
        // a mismatch, or the relaunch branch is unreachable and the flag changes nothing.
        assert!(
            !want_headless(true, Some(true)),
            "--headed against a headless browser must ask for headed, i.e. relaunch"
        );
        assert!(
            !want_headless(true, None),
            "--headed on a first launch is headed"
        );
        assert!(
            !want_headless(true, Some(false)),
            "--headed on a headed browser: no change"
        );

        // cf1e8a8, which must stay fixed: a flagless command inherits the stored mode rather
        // than reading its own default as a request for headless.
        assert!(
            !want_headless(false, Some(false)),
            "a plain command against a HEADED browser must not ask for headless (it would kill it)"
        );
        assert!(
            want_headless(false, Some(true)),
            "a headless browser stays headless"
        );
        assert!(
            want_headless(false, None),
            "headless is the default for a first launch"
        );
    }

    /// The relaunch kills through `kill::kill_pid`, whose guard refuses a pid that is not a
    /// browser — a stored pid may have been recycled. A refusal is not a teardown: nothing
    /// was signalled, so there is nothing to wait out and the caller proceeds at once.
    /// (Lives here rather than in `browser.rs`, which is at the 1000-line cap.)
    #[cfg(unix)]
    #[test]
    fn a_pid_that_is_not_a_browser_is_neither_killed_nor_waited_for() {
        let mut bystander = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("stand-in for a recycled pid");

        let started = std::time::Instant::now();
        let outcome = crate::browser::kill_and_await_exit("test-not-a-browser", bystander.id());
        let elapsed = started.elapsed();

        assert!(outcome.is_ok(), "a refused kill is not a failed relaunch");
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "nothing was signalled, so nothing may be waited out: took {elapsed:?}"
        );
        assert!(
            bystander.try_wait().expect("poll the stand-in").is_none(),
            "a pid the guard refused must still be running"
        );
        let _ = bystander.kill();
        let _ = bystander.wait();
    }

    /// A pid the OS calls gone: nothing to signal, nothing to wait for.
    #[cfg(unix)]
    #[test]
    fn an_already_dead_browser_does_not_block_the_relaunch() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn a pid we can then free");
        let pid = child.id();
        child.kill().expect("signal it");
        child.wait().expect("reap it");

        let started = std::time::Instant::now();
        assert!(crate::browser::kill_and_await_exit("test-already-gone", pid).is_ok());
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "a dead pid is answered without a wait"
        );
    }

    /// The predicate above reads `cli.headed` as "written on this command line". That holds
    /// only while the flag stays a bare `SetTrue` boolean with no default and no negation —
    /// a `--headed=false` spelling, or a default of `true`, would make the two diverge
    /// silently.
    #[test]
    fn the_headed_flag_is_true_only_when_it_was_written() {
        let plain = crate::cli::Cli::try_parse_from(["chrome-agent", "goto", "https://e.com"])
            .expect("a plain goto parses");
        assert!(!plain.headed, "omitted --headed must not read as passed");

        for argv in [
            vec!["chrome-agent", "--headed", "goto", "https://e.com"],
            vec!["chrome-agent", "goto", "--headed", "https://e.com"],
        ] {
            let cli = crate::cli::Cli::try_parse_from(argv.clone()).expect("global flag parses");
            assert!(cli.headed, "--headed must read as passed in {argv:?}");
        }

        // No value-taking spelling: if this ever parses, `cli.headed == false` stops meaning
        // "not written" and `want_headless` needs `ArgMatches::value_source` instead.
        assert!(
            crate::cli::Cli::try_parse_from(["chrome-agent", "--headed=false", "goto", "u"])
                .is_err(),
            "--headed must take no value for `headed == true` to mean `explicitly passed`"
        );
    }
}
