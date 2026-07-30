use std::time::Duration;

use serde_json::json;

use crate::cdp::client::CdpClient;
use crate::cdp::types::{EvaluateResult, NavigateParams, NavigateResult};

pub struct GotoResult {
    pub url: String,
    pub title: String,
    /// Where the navigation ended up relative to where it was aimed.
    pub landed: crate::landing::Landing,
}

/// Parse a `"Name: Value"` header string into its (name, value) pair.
///
/// Splits on the FIRST colon so values may themselves contain colons
/// (e.g. `"X-Trace: a:b:c"`). Both sides are trimmed. Errors when there is no
/// colon or the name is empty.
pub fn parse_header(raw: &str) -> Result<(String, String), crate::BoxError> {
    let (name, value) = raw
        .split_once(':')
        .ok_or_else(|| format!("Invalid --header {raw:?}: expected \"Name: Value\""))?;
    let name = name.trim();
    if name.is_empty() {
        return Err(format!("Invalid --header {raw:?}: header name is empty").into());
    }
    Ok((name.to_string(), value.trim().to_string()))
}

pub async fn run(
    client: &CdpClient,
    url: &str,
    timeout_secs: u64,
    headers: &[(String, String)],
) -> Result<GotoResult, crate::BoxError> {
    // Auto-prefix https:// if no scheme is provided
    let url = if url.contains("://") {
        url.to_string()
    } else {
        format!("https://{url}")
    };
    let url = url.as_str();

    // Ensure Page domain is enabled so we receive loadEventFired
    client.enable("Page").await?;

    // Apply extra HTTP headers (auth tokens, multi-tenant routing, etc.) before
    // navigating. Requires the Network domain.
    if !headers.is_empty() {
        client.enable("Network").await?;
        let map: serde_json::Map<String, serde_json::Value> = headers
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
            .collect();
        client
            .send("Network.setExtraHTTPHeaders", json!({ "headers": map }))
            .await?;
    }

    // Subscribe to events BEFORE navigating so a fast/cached load that fires
    // Page.loadEventFired before we start waiting is not missed (which would
    // otherwise stall until the full timeout).
    let mut events = client.events();

    let nav_result: NavigateResult = client
        .call(
            "Page.navigate",
            NavigateParams {
                url: url.to_string(),
                referrer: None,
                transition_type: None,
                frame_id: None,
            },
        )
        .await?;

    if let Some(error_text) = &nav_result.error_text {
        return Err(format!("Navigation failed: {error_text}").into());
    }

    // Wait for Page.loadEventFired on the pre-navigate subscription.
    let _ = CdpClient::wait_for_event_on(
        &mut events,
        "Page.loadEventFired",
        Duration::from_secs(timeout_secs),
    )
    .await;

    // Wait for the DOM to stabilize (SPAs often render after loadEventFired): resolve once
    // nothing has changed for QUIET, and never later than HARD.
    //
    // Both bounds matter. The quiet window starts immediately, so a page where nothing ever
    // mutates resolves in QUIET rather than being charged the whole budget to discover that.
    // And the ceiling is never cleared by a mutation, so a page that never goes quiet — a
    // chat window, a live dashboard, a rotating ad slot — still returns. `awaitPromise` has
    // no deadline of its own, so a probe that can fail to resolve holds the command open
    // for as long as the page keeps moving.
    let _ = client
        .call::<_, serde_json::Value>(
            "Runtime.evaluate",
            json!({
                "expression": r"new Promise(resolve => {
                    const QUIET = 200, HARD = 3000;
                    let settled = false, quiet = null, obs = null;
                    const finish = () => {
                        if (settled) return;
                        settled = true;
                        clearTimeout(quiet);
                        clearTimeout(hard);
                        if (obs) obs.disconnect();
                        resolve();
                    };
                    quiet = setTimeout(finish, QUIET);
                    const hard = setTimeout(finish, HARD);
                    obs = new MutationObserver(() => {
                        clearTimeout(quiet);
                        quiet = setTimeout(finish, QUIET);
                    });
                    obs.observe(document.body || document.documentElement, { childList: true, subtree: true });
                })",
                "awaitPromise": true,
                "returnByValue": true,
            }),
        )
        .await;

    // Read the settled page state from the renderer. Page.navigate only echoes
    // the requested URL; after an HTTP/client-side redirect the authoritative
    // URL is location.href.
    //
    // The status rides on the same read. Navigation Timing is the stealth-safe path the
    // retroactive network capture already uses: no `Network.enable`, so `--stealth` keeps
    // its promise, and no extra round trip. `responseStatus` is missing on older Chrome and
    // 0 on a document with no HTTP response, both of which `Landing` reports as absence.
    let eval_result: EvaluateResult = client
        .call(
            "Runtime.evaluate",
            json!({
                "expression": r"(() => {
                    let status = null;
                    try {
                        const nav = performance.getEntriesByType('navigation')[0];
                        if (nav && typeof nav.responseStatus === 'number') status = nav.responseStatus;
                    } catch (e) {}
                    return { url: location.href, title: document.title, status };
                })()",
                "returnByValue": true,
            }),
        )
        .await?;

    let page_state = eval_result
        .result
        .value
        .as_ref()
        .and_then(serde_json::Value::as_object);
    let settled_url = page_state
        .and_then(|state| state.get("url"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or(url)
        .to_string();
    let title = page_state
        .and_then(|state| state.get("title"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    let status = page_state
        .and_then(|state| state.get("status"))
        .and_then(serde_json::Value::as_u64)
        .and_then(|code| u16::try_from(code).ok());

    // `url`, not the caller's raw argument: the https:// prefixing above is the tool's own
    // normalisation, and comparing against the pre-normalised form would report a redirect
    // on every `goto example.com`.
    let landed = crate::landing::Landing::new(url, &settled_url, status);

    Ok(GotoResult {
        url: settled_url,
        title,
        landed,
    })
}

#[cfg(test)]
mod tests {
    use super::parse_header;

    #[test]
    fn parses_and_trims() {
        let (n, v) = parse_header("Authorization: Bearer xyz").unwrap();
        assert_eq!(n, "Authorization");
        assert_eq!(v, "Bearer xyz");
    }

    #[test]
    fn keeps_colons_in_value() {
        let (n, v) = parse_header("X-Trace:  a:b:c ").unwrap();
        assert_eq!(n, "X-Trace");
        assert_eq!(v, "a:b:c");
    }

    #[test]
    fn empty_value_is_allowed() {
        let (n, v) = parse_header("X-Empty:").unwrap();
        assert_eq!(n, "X-Empty");
        assert_eq!(v, "");
    }

    #[test]
    fn trims_nonempty_name_and_whitespace_value() {
        // Guards against a partial-trim regression that the empty-name test can't catch.
        let (n, v) = parse_header("  X-Foo :   ").unwrap();
        assert_eq!(n, "X-Foo");
        assert_eq!(v, "");
    }

    #[test]
    fn rejects_missing_colon() {
        assert!(parse_header("NoColonHere").is_err());
    }

    #[test]
    fn rejects_empty_name() {
        assert!(parse_header("   : value").is_err());
    }
}
