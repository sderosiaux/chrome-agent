//! CSS-selector-based element actions (click, double-click, fill, focus).
//!
//! Split out of `element.rs` to keep that file under the 1000-line module cap.
//! Re-exported from `element` (`pub use`) so callers keep using
//! `crate::element::click_selector` etc.

use serde_json::json;

use crate::cdp::client::CdpClient;
use crate::element::{dblclick_at_coords, wait_for_stabilization, ElementError};

/// Single-click an element matched by a CSS selector via `Runtime.evaluate`.
pub async fn click_selector(client: &CdpClient, selector: &str) -> Result<(), ElementError> {
    let js = format!(
        r"(() => {{
            const el = document.querySelector({sel});
            if (!el) throw new Error('No element matches selector: ' + {sel});
            el.click();
        }})()",
        sel = serde_json::to_string(selector).unwrap_or_default()
    );
    let nav_events = client.events();
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

/// Fill an element matched by a CSS selector via `Runtime.evaluate`.
pub async fn fill_selector(client: &CdpClient, selector: &str, value: &str) -> Result<(), ElementError> {
    let js = format!(
        r"(() => {{
            const el = document.querySelector({sel});
            if (!el) throw new Error('No element matches selector: ' + {sel});
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
        }})()",
        sel = serde_json::to_string(selector).unwrap_or_default(),
        val = serde_json::to_string(value).unwrap_or_default()
    );
    let nav_events = client.events();
    let result: serde_json::Value = client
        .call("Runtime.evaluate", json!({ "expression": js, "returnByValue": true }))
        .await
        .map_err(|e| ElementError::Action(format!("fill_selector failed: {e}")))?;

    if let Some(exception) = result.get("exceptionDetails") {
        let text = exception
            .get("exception")
            .and_then(|ex| ex.get("description"))
            .and_then(|d| d.as_str())
            .or_else(|| exception.get("text").and_then(|t| t.as_str()))
            .unwrap_or("unknown error");
        return Err(ElementError::Action(text.to_string()));
    }

    wait_for_stabilization(nav_events).await;
    Ok(())
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
