//! CSS-selector-based element actions (click, double-click, fill, focus).
//!
//! Split out of `element.rs` to keep that file under the 1000-line module cap.
//! Re-exported from `element` (`pub use`) so callers keep using
//! `crate::element::click_selector` etc.

use serde_json::json;

use crate::cdp::client::CdpClient;

/// Thrown JS arrives as "Error: message\n    at <anonymous>:3:19". The stack is noise in a
/// field an agent reads to decide what to do next.
fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or(text).trim_start_matches("Error: ").to_string()
}
use crate::element::{click_at_coords, dblclick_at_coords, wait_for_stabilization, ElementError};

/// The uid a snapshot would give the element a selector resolves to, or `None` when it
/// matches nothing (or the node has no backend id).
///
/// A selector-targeted action used to report only the selector it was handed, while the
/// change report named uids: nothing tied the two together, so an agent could not check
/// that the node the delta describes is the node it aimed at, and a selector matching
/// several elements gave no clue which one was used. Resolved BEFORE the action — after it,
/// the element may be gone, and the answer would describe a different page.
pub async fn selector_uid(client: &CdpClient, selector: &str) -> Option<String> {
    let expression = format!(
        "document.querySelector({})",
        serde_json::to_string(selector).unwrap_or_default()
    );
    let handle: serde_json::Value = client
        .call("Runtime.evaluate", json!({ "expression": expression }))
        .await
        .ok()?;
    let object_id = handle.get("result")?.get("objectId")?.as_str()?.to_string();
    let described: serde_json::Value = client
        .call("DOM.describeNode", json!({ "objectId": object_id }))
        .await
        .ok()?;
    let backend_id = described.get("node")?.get("backendNodeId")?.as_i64()?;
    Some(format!("n{backend_id}"))
}

/// Single-click an element matched by a CSS selector.
///
/// Resolves the element's viewport-center coordinates, then dispatches native CDP mouse
/// events there — the same thing `click <uid>` does, and what `dblclick_selector` already
/// did. It used to call `el.click()`, which fires the handler on the node whatever is
/// stacked on top of it: a click on a button under a modal scrim reported success with the
/// same shape as a click a user could have made. Two spellings of one verb, doing different
/// things, told apart by nothing in the response.
///
/// The consequence is deliberate and worth stating: a covered element now hands the click to
/// whatever covers it, so `--selector` on a button behind a cookie banner clicks the banner.
/// That is what a pointer does. Falls back to a JS `click()` only when the element has no
/// layout box (zero-size), where there is no point to aim at.
pub async fn click_selector(client: &CdpClient, selector: &str) -> Result<(), ElementError> {
    let sel = serde_json::to_string(selector).unwrap_or_default();
    let js = format!(
        r"(() => {{
            const el = document.querySelector({sel});
            if (!el) throw new Error('No element matches selector: ' + {sel});
            el.scrollIntoView({{block: 'center', inline: 'center'}});
            const r = el.getBoundingClientRect();
            if (r.width === 0 && r.height === 0) return null;
            return [r.left + r.width / 2, r.top + r.height / 2];
        }})()"
    );
    let result: serde_json::Value = client
        .call("Runtime.evaluate", json!({ "expression": js, "returnByValue": true }))
        .await
        .map_err(|e| ElementError::Action(format!("click_selector failed: {e}")))?;

    if let Some(exception) = result.get("exceptionDetails") {
        let text = exception
            .get("exception")
            .and_then(|ex| ex.get("description"))
            .and_then(|d| d.as_str())
            .or_else(|| exception.get("text").and_then(|t| t.as_str()))
            .unwrap_or("unknown error");
        return Err(ElementError::NotFound(text.to_string()));
    }

    let center = result
        .get("result")
        .and_then(|r| r.get("value"))
        .and_then(serde_json::Value::as_array)
        .filter(|a| a.len() == 2)
        .and_then(|a| Some((a[0].as_f64()?, a[1].as_f64()?)));

    if let Some((cx, cy)) = center {
        return click_at_coords(client, cx, cy).await;
    }

    // Zero-size / non-laid-out element: there is nowhere to aim, so dispatch the DOM click.
    let fallback = format!(
        r"(() => {{
            const el = document.querySelector({sel});
            if (!el) throw new Error('No element matches selector: ' + {sel});
            el.click();
        }})()"
    );
    let nav_events = client.events();
    client
        .call::<_, serde_json::Value>(
            "Runtime.evaluate",
            json!({ "expression": fallback, "returnByValue": true }),
        )
        .await
        .map_err(|e| ElementError::Action(format!("click_selector fallback failed: {e}")))?;
    wait_for_stabilization(nav_events).await;
    Ok(())
}

/// Double-click an element matched by a CSS selector.
///
/// Resolves the element's viewport-center coordinates, then dispatches a native
/// CDP double-click there (mirroring the uid path). Falls back to a JS `dblclick`
/// `MouseEvent` when the element has no layout box (e.g. zero-size). This is a
/// genuine double-click — not `click_selector`, which only single-clicks.
pub async fn dblclick_selector(client: &CdpClient, selector: &str) -> Result<(), ElementError> {
    let sel = serde_json::to_string(selector).unwrap_or_default();
    let js = format!(
        r"(() => {{
            const el = document.querySelector({sel});
            if (!el) throw new Error('No element matches selector: ' + {sel});
            el.scrollIntoView({{block: 'center', inline: 'center'}});
            const r = el.getBoundingClientRect();
            if (r.width === 0 && r.height === 0) return null;
            return [r.left + r.width / 2, r.top + r.height / 2];
        }})()"
    );
    let result: serde_json::Value = client
        .call("Runtime.evaluate", json!({ "expression": js, "returnByValue": true }))
        .await
        .map_err(|e| ElementError::Action(format!("dblclick_selector failed: {e}")))?;

    if let Some(exception) = result.get("exceptionDetails") {
        let text = exception
            .get("exception")
            .and_then(|ex| ex.get("description"))
            .and_then(|d| d.as_str())
            .or_else(|| exception.get("text").and_then(|t| t.as_str()))
            .unwrap_or("unknown error");
        return Err(ElementError::NotFound(text.to_string()));
    }

    let center = result
        .get("result")
        .and_then(|r| r.get("value"))
        .and_then(serde_json::Value::as_array)
        .filter(|a| a.len() == 2)
        .and_then(|a| Some((a[0].as_f64()?, a[1].as_f64()?)));

    if let Some((cx, cy)) = center {
        return dblclick_at_coords(client, cx, cy).await;
    }

    // Zero-size / non-laid-out element: dispatch a JS dblclick event instead.
    let fallback = format!(
        r"(() => {{
            const el = document.querySelector({sel});
            if (!el) throw new Error('No element matches selector: ' + {sel});
            el.dispatchEvent(new MouseEvent('dblclick', {{bubbles: true, cancelable: true}}));
        }})()"
    );
    let nav_events = client.events();
    client
        .call::<_, serde_json::Value>(
            "Runtime.evaluate",
            json!({ "expression": fallback, "returnByValue": true }),
        )
        .await
        .map_err(|e| ElementError::Action(format!("dblclick_selector fallback failed: {e}")))?;
    wait_for_stabilization(nav_events).await;
    Ok(())
}

/// Fill an element matched by a CSS selector, and report what the page holds afterwards.
///
/// Probe, write and read-back happen in one evaluation so all three bind the same node: a
/// re-render between separate `querySelector` calls would otherwise let them act on
/// different elements while reporting one result.
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
            // Read after the window rather than on the next line: a controlled component
            // that reverts in a promise callback has not run yet when the write returns.
            return new Promise(resolve => setTimeout(() => resolve({{
                value: el.value === undefined ? null : String(el.value),
                maxLength: typeof el.maxLength === 'number' ? el.maxLength : null,
                sensitive: el.type === 'password' ||
                    /password|cc-number|cc-csc|one-time-code/i.test(el.autocomplete || '')
            }}), {window}));
        }})()",
        sel = serde_json::to_string(selector).unwrap_or_default(),
        val = serde_json::to_string(value).unwrap_or_default(),
        window = crate::element::READ_BACK_MS
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
    wait_for_stabilization(nav_events).await;
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
