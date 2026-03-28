use std::time::Duration;

use serde_json::json;

use crate::cdp::client::CdpClient;
use crate::cdp::types::{EvaluateResult, NavigateParams, NavigateResult};

pub struct GotoResult {
    pub url: String,
    pub title: String,
}

pub async fn run(client: &CdpClient, url: &str, timeout_secs: u64) -> Result<GotoResult, crate::BoxError> {
    // Auto-prefix https:// if no scheme is provided
    let url = if url.contains("://") {
        url.to_string()
    } else {
        format!("https://{url}")
    };
    let url = url.as_str();

    // Ensure Page domain is enabled so we receive loadEventFired
    client.enable("Page").await?;

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

    // Wait for Page.loadEventFired
    let _ = client
        .wait_for_event("Page.loadEventFired", Duration::from_secs(timeout_secs))
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

    // Get page title via Runtime.evaluate
    let eval_result: EvaluateResult = client
        .call(
            "Runtime.evaluate",
            json!({
                "expression": "document.title",
                "returnByValue": true,
            }),
        )
        .await?;

    let title = eval_result
        .result
        .value
        .as_ref()
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Ok(GotoResult {
        url: url.to_string(),
        title,
    })
}
