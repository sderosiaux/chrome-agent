//! Resolving a single CLI invocation's browser + page connection.
//!
//! Split out of `run::run` for the repo's 1000-line file cap: this block — load the session
//! store, connect to (or launch) the named browser, resolve its target page, connect the
//! page-level CDP client — was already at the file's edge before `--chrome-arg` needed one
//! more line per call site it mirrors `--proxy-server` on. Pipe and replay mode have their
//! own version of this (`pipe::connect_browser`) because they read from stdin rather than
//! returning to a caller that dispatches on `cli.command`; unifying the two was out of scope
//! for a flag addition and would be a larger, riskier change than this cap requires.

use crate::BoxError;
use crate::browser::{self, BrowserOptions};
use crate::cdp::client::CdpClient;
use crate::cli::{Cli, Command};
use crate::run_helpers::{connect_page, kill_pid, resolve_page_target};
use crate::session::{self, SessionStore};

/// Load the session store, connect to (or launch) `cli.browser`, resolve `cli.page`'s
/// target, and connect its page-level CDP client. Returns the store (so the caller can keep
/// mutating and eventually save it), the browser-level client (`Target.*` calls), the
/// page-level client, and the resolved target id.
pub async fn resolve_cli_connection(
    cli: &Cli,
) -> Result<(SessionStore, CdpClient, CdpClient, String), BoxError> {
    let mut store = session::load_session()?;
    let requested_proxy = browser::normalized_proxy_option(
        cli.connect.as_deref(),
        cli.proxy_server.as_deref(),
    )?;
    let requested_chrome_args = browser::normalized_chrome_args_option(
        cli.connect.as_deref(),
        &cli.chrome_args,
    )?;

    let existing_mode = store.browsers.get(&cli.browser).map(|b| b.headless);
    let want_headless = existing_mode.unwrap_or(!cli.headed);

    // A managed browser's proxy is fixed at launch and persisted per named
    // browser. When relaunching an existing named browser (dead reconnect or
    // mode switch), inherit its stored proxy unless the caller explicitly asked
    // for one — otherwise omitting the flag would silently drop the proxy and
    // route traffic directly. An explicit mismatch is caught on the live path
    // by `ensure_proxy_compatible`.
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
                // Reconnect failed: the recorded browser is unreachable. Kill its
                // pid before relaunching so a still-alive-but-unresponsive Chrome
                // isn't orphaned (its session entry is about to be replaced).
                if let Some(pid) = existing.pid {
                    kill_pid(pid);
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
            if let Some(pid) = existing.pid {
                kill_pid(pid);
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

    // The browser-level client answers Target.* calls (page resolution, tabs).
    // It is bound by the caller's --timeout like the page client below — its
    // error message says "raise --timeout", which must actually work.
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
