use serde_json::json;

use crate::cdp::client::CdpClient;
use crate::cdp::types::EvaluateResult;

pub async fn run(
    client: &CdpClient,
    expression: &str,
) -> Result<String, Box<dyn std::error::Error>> {
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
