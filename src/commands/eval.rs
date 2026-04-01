use serde_json::{json, Value};

use crate::cdp::client::CdpClient;
use crate::cdp::types::EvaluateResult;

/// Wrap the expression in a block scope `{ ... }` when it contains top-level
/// `const` or `let` declarations so that repeated `eval` calls don't fail with
/// "Identifier already declared".  V8's completion-value semantics mean the
/// block still returns the value of its last expression statement.
fn maybe_block_scope(expression: &str) -> std::borrow::Cow<'_, str> {
    let t = expression.trim();
    let has_declaration = t.starts_with("const ")
        || t.starts_with("let ")
        || t.contains("\nconst ")
        || t.contains("\nlet ")
        || t.contains(";const ")
        || t.contains("; const ")
        || t.contains(";let ")
        || t.contains("; let ");
    if has_declaration {
        std::borrow::Cow::Owned(format!("{{\n{t}\n}}"))
    } else {
        std::borrow::Cow::Borrowed(expression)
    }
}

/// Evaluate JS and return the raw `serde_json::Value` (for JSON mode).
pub async fn run_raw(
    client: &CdpClient,
    expression: &str,
) -> Result<Value, crate::BoxError> {
    let expression = maybe_block_scope(expression);
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
    let expression = maybe_block_scope(expression);
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
