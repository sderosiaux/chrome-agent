use std::time::Duration;

use serde_json::json;

use crate::cdp::client::CdpClient;
use crate::cdp::types::{EvaluateResult, NavigateParams, NavigateResult};

pub struct GotoResult {
    pub url: String,
    pub title: String,
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

    // Wait for DOM to stabilize (SPAs often render after loadEventFired).
    // Uses MutationObserver: resolves once no DOM changes for 200ms, max 3s.
    let _ = client
        .call::<_, serde_json::Value>(
            "Runtime.evaluate",
            json!({
                "expression": r"new Promise(resolve => {
                    let timer = setTimeout(resolve, 3000);
                    const obs = new MutationObserver(() => {
                        clearTimeout(timer);
                        timer = setTimeout(() => { obs.disconnect(); resolve(); }, 200);
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
    let eval_result: EvaluateResult = client
        .call(
            "Runtime.evaluate",
            json!({
                "expression": "({ url: location.href, title: document.title })",
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

    Ok(GotoResult {
        url: settled_url,
        title,
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
