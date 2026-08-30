//! CSS-selector-based element actions (click, double-click, fill, focus). Re-exported from
//! `element`.

use serde_json::json;

use crate::cdp::client::CdpClient;

/// Thrown JS arrives as `Error: message\n    at <anonymous>:3:19`; the stack is noise.
fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or(text).trim_start_matches("Error: ").to_string()
}
use crate::element::{wait_for_stabilization, ElementError};

/// Single-click an element matched by a CSS selector.
///
/// Resolves ONE handle and does everything through it — aim probe, dispatch, uid on the response
/// — so a re-render between round trips cannot bind three different nodes. Dispatching real input
/// rather than `el.click()` means a covered element hands the click to whatever covers it, and
/// the response names it. Falls back to a JS `click()` only when there is no layout box.
pub async fn click_selector(
    client: &CdpClient,
    selector: &str,
    on_intercept: crate::hit_test::OnIntercept,
) -> Result<crate::hit_test::Dispatched, ElementError> {
    let handle = crate::hit_test::resolve_selector(client, selector).await?;
    let outcome = crate::element::click_handle(
        client,
        &handle.object_id,
        None,
        on_intercept,
        &format!("selector '{selector}'"),
    )
    .await
    // `named` below runs only on the `Ok` path, and a refusal needs the node too.
    .map_err(|e| e.naming(handle.uid.clone(), handle.role.clone(), handle.name.clone()))?;
    Ok(outcome.named(handle.uid, handle.role, handle.name))
}

/// Double-click an element matched by a CSS selector.
///
/// Same single-handle path as `click_selector`, dispatching a genuine native double-click.
pub async fn dblclick_selector(
    client: &CdpClient,
    selector: &str,
    on_intercept: crate::hit_test::OnIntercept,
) -> Result<crate::hit_test::Dispatched, ElementError> {
    let handle = crate::hit_test::resolve_selector(client, selector).await?;
    let outcome = crate::element::dblclick_handle(
        client,
        &handle.object_id,
        None,
        on_intercept,
        &format!("selector '{selector}'"),
    )
    .await
    // `named` below runs only on the `Ok` path, and a refusal needs the node too.
    .map_err(|e| e.naming(handle.uid.clone(), handle.role.clone(), handle.name.clone()))?;
    Ok(outcome.named(handle.uid, handle.role, handle.name))
}

/// Fill an element matched by a CSS selector, and report what the page holds afterwards. Probe,
/// write and read-back happen in one evaluation, so all three bind the same node.
pub async fn fill_selector(
    client: &CdpClient,
    selector: &str,
    value: &str,
) -> Result<crate::element::FillOutcome, ElementError> {
    let js = format!(
        r"(() => {{
            const el = document.querySelector({sel});
            if (!el) throw new Error('No element matches selector: ' + {sel});
            if (el.matches(':disabled')) throw new Error('Element is disabled and cannot be filled: ' + {sel});
            if (el.readOnly) throw new Error('Element is readonly and cannot be filled: ' + {sel});
            el.focus();
            const proto = el instanceof HTMLTextAreaElement
                ? window.HTMLTextAreaElement.prototype
                : window.HTMLInputElement.prototype;
            const setter = Object.getOwnPropertyDescriptor(proto, 'value');
            if (setter && setter.set) {{
                setter.set.call(el, {val});
            }} else {{
                el.value = {val};
            }}
            el.dispatchEvent(new Event('input', {{bubbles: true}}));
            el.dispatchEvent(new Event('change', {{bubbles: true}}));
            // Read after the window, not on the next line: a controlled component reverting in
            // a promise callback has not run yet when the write returns.
            return new Promise(resolve => setTimeout(() => resolve({{
                value: el.value === undefined ? null : String(el.value),
                maxLength: typeof el.maxLength === 'number' ? el.maxLength : null,
                sensitive: {secret}
            }}), {window}));
        }})()",
        sel = serde_json::to_string(selector).unwrap_or_default(),
        val = serde_json::to_string(value).unwrap_or_default(),
        window = crate::element::READ_BACK_MS,
        secret = crate::element::SECRET_FIELD
    );
    let nav_events = client.events();
    let result: serde_json::Value = client
        .call(
            "Runtime.evaluate",
            json!({ "expression": js, "returnByValue": true, "awaitPromise": true }),
        )
        .await
        .map_err(|e| ElementError::Action(format!("fill_selector failed: {e}")))?;

    if let Some(exception) = result.get("exceptionDetails") {
        let text = exception
            .get("exception")
            .and_then(|ex| ex.get("description"))
            .and_then(|d| d.as_str())
            .or_else(|| exception.get("text").and_then(|t| t.as_str()))
            .unwrap_or("unknown error");
        return Err(ElementError::Action(first_line(text)));
    }

    let payload = result.get("result").and_then(|r| r.get("value")).cloned().unwrap_or_default();
    let actual = payload.get("value").and_then(serde_json::Value::as_str).map(str::to_string);
    let max_length = payload.get("maxLength").and_then(serde_json::Value::as_i64);
    let sensitive = payload.get("sensitive").and_then(serde_json::Value::as_bool).unwrap_or(false);
    wait_for_stabilization(client, nav_events).await;
    Ok(crate::element::FillOutcome::new(value, actual)
        .with_max_length(max_length)
        .secret(sensitive))
}

/// Focus an element matched by a CSS selector via `Runtime.evaluate`.
pub async fn focus_selector(client: &CdpClient, selector: &str) -> Result<(), ElementError> {
    let js = format!(
        r"(() => {{
            const el = document.querySelector({sel});
            if (!el) throw new Error('No element matches selector: ' + {sel});
            el.focus();
        }})()",
        sel = serde_json::to_string(selector).unwrap_or_default()
    );
    let result: serde_json::Value = client
        .call("Runtime.evaluate", json!({ "expression": js, "returnByValue": true }))
        .await
        .map_err(|e| ElementError::Action(format!("focus_selector failed: {e}")))?;

    if let Some(exception) = result.get("exceptionDetails") {
        let text = exception
            .get("exception")
            .and_then(|ex| ex.get("description"))
            .and_then(|d| d.as_str())
            .or_else(|| exception.get("text").and_then(|t| t.as_str()))
            .unwrap_or("unknown error");
        return Err(ElementError::NotFound(text.to_string()));
    }

    Ok(())
}
