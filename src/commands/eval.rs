use serde_json::{json, Value};

use crate::cdp::client::CdpClient;
use crate::cdp::types::EvaluateResult;

/// Evaluate JS and return the raw `serde_json::Value` (for JSON mode).
pub async fn run_raw(
    client: &CdpClient,
    expression: &str,
) -> Result<Value, crate::BoxError> {
    let result: EvaluateResult = client
        .call(
            "Runtime.evaluate",
            json!({
                "expression": expression,
                "returnByValue": true,
                "awaitPromise": true,
            }),
        )
        .await?;

    if let Some(exception) = &result.exception_details {
        return Err(format!(
            "Evaluation error: {}",
            exception
                .exception
                .as_ref()
                .and_then(|e| e.description.as_deref())
                .unwrap_or(&exception.text)
        )
        .into());
    }

    Ok(result.result.value.unwrap_or_default())
}

/// Evaluate JS and return a display string (for text mode).
pub async fn run(
    client: &CdpClient,
    expression: &str,
) -> Result<String, crate::BoxError> {
    let result: EvaluateResult = client
        .call(
            "Runtime.evaluate",
            json!({
                "expression": expression,
                "returnByValue": true,
                "awaitPromise": true,
            }),
        )
        .await?;

    // Check for exception
    if let Some(exception) = &result.exception_details {
        return Err(format!(
            "Evaluation error: {}",
            exception
                .exception
                .as_ref()
                .and_then(|e| e.description.as_deref())
                .unwrap_or(&exception.text)
        )
        .into());
    }

    // Stringify the result value
    let output = match &result.result.value {
        Some(val) => serde_json::to_string(val)?,
        None => {
            // No value returned — use description or type
            result
                .result
                .description
                .clone()
                .unwrap_or_else(|| result.result.remote_type.clone())
        }
    };

    Ok(output)
}
