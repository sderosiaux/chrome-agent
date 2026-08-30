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

/// Parse `"Name: Value"` into its pair, splitting on the FIRST colon so values may contain
/// colons. Both sides are trimmed. Errors on no colon or an empty name.
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
    // Auto-prefix https:// when no scheme is given.
    let url = if url.contains("://") {
        url.to_string()
    } else {
        format!("https://{url}")
    };
    let url = url.as_str();

    // For loadEventFired.
    client.enable("Page").await?;

    // Extra headers must be set before navigating, and need the Network domain.
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

    // Subscribe BEFORE navigating: a cached load fires `Page.loadEventFired` immediately, and
    // missing it stalls until the full timeout.
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
        // The URL is in the message because `hints::error_hint` gets a string and nothing
        // else, and must name the host and scheme in the command it suggests.
        return Err(format!("Navigation failed for {url}: {error_text}").into());
    }

    let _ = CdpClient::wait_for_event_on(
        &mut events,
        "Page.loadEventFired",
        Duration::from_secs(timeout_secs),
    )
    .await;

    // Let the DOM settle, since SPAs render after loadEventFired: resolve after 200 ms quiet,
    // never later than 3000 ms. The quiet window starts immediately, so a static page is not
    // charged the whole budget; the ceiling is never cleared by a mutation, so a page that
    // never goes quiet still returns. `awaitPromise` has no deadline of its own.
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

    // One read for three things, since `Page.navigate` only echoes the requested URL:
    // `location.href` after redirects, the status from Navigation Timing (no `Network.enable`,
    // so `--stealth` holds; absent on older Chrome and without an HTTP response), and the
    // document shape `serving.rs` judges.
    //
    // Every count over-counts rather than under-counts (hidden text, off-screen buttons),
    // because over-counting produces `serving: "page"`, which is silence. Text is walked with
    // a TreeWalker capped at 4096 chars rather than read from `innerText`, which forces layout.
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
                    let shape = null;
                    try {
                        const root = document.body || document.documentElement;
                        // Frames AND scripts: DataDome puts the vendor URL on the frame, while
                        // Cloudflare's interstitial uses an empty-src about:blank frame and
                        // only the Turnstile script names the vendor. Query strings are
                        // dropped; they carry a per-visit id the vendor table ignores.
                        const resources = [];
                        const collect = (selector, cap) => {
                            let taken = 0;
                            for (const el of document.querySelectorAll(selector)) {
                                if (taken >= cap) break;
                                if (!el.src) continue;
                                resources.push(el.src.split('?')[0]);
                                taken++;
                            }
                        };
                        collect('iframe,frame', 20);
                        collect('script[src]', 60);
                        const controls = root.querySelectorAll(
                            'button,select,textarea,input:not([type=hidden]),[role=button],[contenteditable]'
                        ).length;
                        let links = 0;
                        for (const a of root.querySelectorAll('a[href]')) {
                            // A `javascript:` anchor is not a destination; the F5 refusal
                            // notice's only link is one.
                            if (a.protocol !== 'http:' && a.protocol !== 'https:') continue;
                            if (++links >= 64) break;
                        }
                        const scripts = document.querySelectorAll('script[src]').length;
                        let text = 0;
                        const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
                        while (text < 4096) {
                            const node = walker.nextNode();
                            if (!node) break;
                            const tag = node.parentNode && node.parentNode.nodeName;
                            if (tag === 'SCRIPT' || tag === 'STYLE' || tag === 'NOSCRIPT' || tag === 'TEMPLATE') continue;
                            text += node.data.trim().length;
                        }
                        shape = { resources, controls, links, scripts, text };
                    } catch (e) { shape = null; }
                    return { url: location.href, title: document.title, status, shape };
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

    // Absent rather than defaulted: a zero-valued shape would read as "an empty document",
    // the strongest thing `serving` can say, from having measured nothing.
    let shape = crate::serving::PageShape::from_probe(
        page_state.and_then(|state| state.get("shape")),
    );

    // `url`, not the caller's raw argument: comparing against the pre-normalised form would
    // report a redirect on every `goto example.com`.
    let landed = crate::landing::Landing::new(url, &settled_url, status, shape.as_ref());

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
