use std::time::{Duration, Instant};

use serde_json::json;

use crate::cdp::client::CdpClient;
use crate::cdp::types::EvaluateResult;

/// Poll the page until a condition is met, or timeout.
pub async fn run(
    client: &CdpClient,
    what: &str,
    pattern: &str,
    timeout_secs: u64,
) -> Result<String, Box<dyn std::error::Error>> {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let poll_interval = Duration::from_millis(200);

    let expression = match what {
        "text" => format!(
            "document.body.innerText.includes({})",
            serde_json::to_string(pattern)?
        ),
        "url" => format!(
            "location.href.includes({})",
            serde_json::to_string(pattern)?
        ),
        "selector" => format!(
            "!!document.querySelector({})",
            serde_json::to_string(pattern)?
        ),
        other => return Err(format!("Unknown wait type: {other}. Use \"text\", \"url\", or \"selector\".").into()),
    };

    loop {
        let result: EvaluateResult = client
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "returnByValue": true,
                }),
            )
            .await?;

        let matched = result
            .result
            .value
            .as_ref()
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

        if matched {
            return Ok(format!("Found: {what} matching \"{pattern}\""));
        }

        if Instant::now() >= deadline {
            return Err(format!(
                "Timeout after {timeout_secs}s waiting for {what} matching \"{pattern}\""
            )
            .into());
        }

        tokio::time::sleep(poll_interval).await;
    }
}
