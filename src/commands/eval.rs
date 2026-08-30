use serde_json::{Value, json};

use crate::cdp::client::CdpClient;
use crate::cdp::types::EvaluateResult;

/// Wrap top-level `const`/`let` declarations in a block, so repeated `eval` calls do not fail
/// with "Identifier already declared". V8 completion values still return the last expression.
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

/// Build `Runtime.evaluate` params, scoped to a bound frame's isolated world when there is
/// one. Without a binding the `contextId` is omitted and the top document is targeted.
fn evaluate_params(client: &CdpClient, expression: &str) -> Value {
    let mut params = json!({
        "expression": expression,
        "returnByValue": true,
        "awaitPromise": true,
    });
    if let Some(ctx) = client.frame_context() {
        params["contextId"] = json!(ctx.context_id);
    }
    params
}

/// What `--selector` actually evaluates: the caller's expression with `el` bound to the matched
/// element, and a throw naming the selector when nothing matches.
///
/// One definition. `run::run` and `pipe_dispatch::dispatch_eval` each carried this format string,
/// character for character.
#[must_use]
pub fn scoped_expression(expression: &str, selector: Option<&str>) -> String {
    let Some(selector) = selector else {
        return expression.to_string();
    };
    let escaped = serde_json::to_string(selector).unwrap_or_default();
    format!(
        "((el) => {{ if (!el) throw new Error('No element matches selector ' + {escaped}); \
         return {expression} }})(document.querySelector({escaped}))"
    )
}

/// Evaluate JS and return the raw `serde_json::Value` (for JSON mode).
pub async fn run_raw(client: &CdpClient, expression: &str) -> Result<Value, crate::BoxError> {
    let expression = maybe_block_scope(expression);
    let result: EvaluateResult = client
        .call("Runtime.evaluate", evaluate_params(client, &expression))
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
pub async fn run(client: &CdpClient, expression: &str) -> Result<String, crate::BoxError> {
    let expression = maybe_block_scope(expression);
    let result: EvaluateResult = client
        .call("Runtime.evaluate", evaluate_params(client, &expression))
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

    let output = match &result.result.value {
        Some(val) => serde_json::to_string(val)?,
        None => {
            // No value: fall back to the description, then the type.
            result
                .result
                .description
                .clone()
                .unwrap_or_else(|| result.result.remote_type.clone())
        }
    };

    Ok(output)
}
