use std::time::Duration;

use serde_json::json;

use crate::cdp::client::CdpClient;
use crate::cdp::types::{EvaluateResult, NavigateParams, NavigateResult};

pub struct GotoResult {
    pub url: String,
    pub title: String,
}

pub async fn run(client: &CdpClient, url: &str) -> Result<GotoResult, Box<dyn std::error::Error>> {
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

    // Wait for Page.loadEventFired with 5s timeout
    let _ = client
        .wait_for_event("Page.loadEventFired", Duration::from_secs(5))
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
