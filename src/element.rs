use std::collections::HashMap;
use std::time::Duration;

use serde_json::json;

use crate::cdp::client::CdpClient;
use crate::cdp::types::{
    DispatchMouseEventParams, GetBoxModelResult, MouseButton, MouseEventType, ResolveNodeParams, ResolveNodeResult,
};
use crate::element_ref::ElementRef;

/// Resolve a uid to a CDP objectId via the `ElementRef` in the uid map.
async fn resolve_uid(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    uid: &str,
) -> Result<ResolvedElement, ElementError> {
    let element_ref = uid_map.get(uid).ok_or_else(|| {
        ElementError::NotFound(format!(
            "Element uid={uid} not found. Run 'chrome-agent inspect' to get fresh uids."
        ))
    })?;

    let backend_node_id = element_ref.backend_node_id().ok_or_else(|| {
        ElementError::NotFound(format!("Element uid={uid} has no resolvable backend node."))
    })?;

    // Resolve to a JS object
    let result: ResolveNodeResult = client
        .call(
            "DOM.resolveNode",
            ResolveNodeParams {
                node_id: None,
                backend_node_id: Some(backend_node_id),
                object_group: Some("dev-browser".into()),
                execution_context_id: None,
            },
        )
        .await
        .map_err(|e| {
            ElementError::Detached(format!(
                "Element uid={uid} no longer exists. The page may have changed. \
                 Run 'chrome-agent inspect' to get fresh uids. ({e})"
            ))
        })?;

    let object_id = result.object.object_id.ok_or_else(|| {
        ElementError::Detached(format!(
            "Element uid={uid} could not be resolved to a JS object."
        ))
    })?;

    // Get box model for coordinates
    let box_result: Result<GetBoxModelResult, _> = client
        .call(
            "DOM.getBoxModel",
            json!({ "backendNodeId": backend_node_id }),
        )
        .await;

    let center = box_result.ok().map(|r| r.model.content_center());

    Ok(ResolvedElement {
        object_id,
        center,
        backend_node_id,
    })
}

struct ResolvedElement {
    object_id: String,
    center: Option<(f64, f64)>,
    backend_node_id: i64,
}

/// Click an element by uid.
///
/// Strategy: try mouse event at element center coordinates first.
/// If no box model available (hidden, custom component, a11y reports "disabled"
/// but DOM isn't), falls back to JS `.click()` on the element directly.
pub async fn click(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    uid: &str,
) -> Result<(), ElementError> {
    let resolved = resolve_uid(client, uid_map, uid).await?;

    // If no box model, fallback to JS click immediately
    if resolved.center.is_none() {
        return js_click(client, &resolved.object_id).await;
    }

    // Scroll element into view first
    let _ = client
        .call::<_, serde_json::Value>(
            "Runtime.callFunctionOn",
            json!({
                "objectId": resolved.object_id,
                "functionDeclaration": "function() { this.scrollIntoViewIfNeeded(); }",
                "returnByValue": true,
            }),
        )
        .await;

    // Re-fetch box model after scroll
    let box_result: Result<GetBoxModelResult, _> = client
        .call(
            "DOM.getBoxModel",
            json!({ "backendNodeId": resolved.backend_node_id }),
        )
        .await;

    let Some((cx, cy)) = box_result.ok().map(|r| r.model.content_center()) else {
        // Box model disappeared after scroll — fallback to JS click
        return js_click(client, &resolved.object_id).await;
    };

    // mousePressed
    client
        .send(
            "Input.dispatchMouseEvent",
            DispatchMouseEventParams {
                event_type: MouseEventType::MousePressed,
                x: cx,
                y: cy,
                button: Some(MouseButton::Left),
                buttons: Some(1),
                click_count: Some(1),
                modifiers: None,
                timestamp: None,
                delta_x: None,
                delta_y: None,
                pointer_type: Some("mouse".into()),
            },
        )
        .await
        .map_err(|e| ElementError::Action(format!("mousePressed failed: {e}")))?;

    // mouseReleased
    client
        .send(
            "Input.dispatchMouseEvent",
            DispatchMouseEventParams {
                event_type: MouseEventType::MouseReleased,
                x: cx,
                y: cy,
                button: Some(MouseButton::Left),
                buttons: Some(0),
                click_count: Some(1),
                modifiers: None,
                timestamp: None,
                delta_x: None,
                delta_y: None,
                pointer_type: Some("mouse".into()),
            },
        )
        .await
        .map_err(|e| ElementError::Action(format!("mouseReleased failed: {e}")))?;

    // Wait for action stabilization
    wait_for_stabilization(client).await;

    Ok(())
}

/// Fallback: click an element via JS `.click()` when mouse events can't be dispatched.
async fn js_click(client: &CdpClient, object_id: &str) -> Result<(), ElementError> {
    let result: serde_json::Value = client
        .call(
            "Runtime.callFunctionOn",
            json!({
                "objectId": object_id,
                "functionDeclaration": "function() { this.click(); }",
                "returnByValue": true,
            }),
        )
        .await
        .map_err(|e| ElementError::Action(format!("JS click fallback failed: {e}")))?;

    if let Some(exception) = result.get("exceptionDetails") {
        return Err(ElementError::Action(format!(
            "JS click threw: {}",
            exception.get("text").and_then(|t| t.as_str()).unwrap_or("unknown")
        )));
    }

    wait_for_stabilization(client).await;
    Ok(())
}

/// Fill an element (input/textarea) by uid.
pub async fn fill(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    uid: &str,
    value: &str,
) -> Result<(), ElementError> {
    let resolved = resolve_uid(client, uid_map, uid).await?;

    // Focus, clear, set value, dispatch events.
    // Use the native HTMLInputElement/HTMLTextAreaElement value setter so React's
    // synthetic onChange fires (React wraps the descriptor; direct assignment is
    // intercepted by React but the setter via Object.getOwnPropertyDescriptor is not).
    let js = r"function(v) {
            this.focus();
            var proto = this instanceof HTMLTextAreaElement
                ? window.HTMLTextAreaElement.prototype
                : window.HTMLInputElement.prototype;
            var setter = Object.getOwnPropertyDescriptor(proto, 'value');
            if (setter && setter.set) {
                setter.set.call(this, v);
            } else {
                this.value = v;
            }
            this.dispatchEvent(new Event('input', {bubbles: true}));
            this.dispatchEvent(new Event('change', {bubbles: true}));
        }".to_string();

    let result: serde_json::Value = client
        .call(
            "Runtime.callFunctionOn",
            json!({
                "objectId": resolved.object_id,
                "functionDeclaration": js,
                "arguments": [{"value": value}],
                "returnByValue": true,
            }),
        )
        .await
        .map_err(|e| ElementError::Action(format!("fill failed: {e}")))?;

    // Check for exception
    if let Some(exception) = result.get("exceptionDetails") {
        return Err(ElementError::Action(format!(
            "fill threw: {}",
            exception
                .get("text")
                .and_then(|t| t.as_str())
                .unwrap_or("unknown error")
        )));
    }

    wait_for_stabilization(client).await;

    Ok(())
}

/// Type text character by character using Input.insertText.
pub async fn type_text(
    client: &CdpClient,
    text: &str,
) -> Result<(), ElementError> {
    client
        .send("Input.insertText", json!({ "text": text }))
        .await
        .map_err(|e| ElementError::Action(format!("insertText failed: {e}")))?;

    wait_for_stabilization(client).await;
    Ok(())
}

/// Press a key (Enter, Tab, Escape, etc.).
pub async fn press_key(
    client: &CdpClient,
    key: &str,
) -> Result<(), ElementError> {
    // Map common key names to their virtual key codes and text values
    let (vk_code, text) = match key {
        "Enter" | "Return" => (13, Some("\r")),
        "Tab" => (9, None),
        "Escape" => (27, None),
        "Backspace" => (8, None),
        "Delete" => (46, None),
        "ArrowUp" => (38, None),
        "ArrowDown" => (40, None),
        "ArrowLeft" => (37, None),
        "ArrowRight" => (39, None),
        "Space" | " " => (32, Some(" ")),
        _ => (0, None),
    };

    // keyDown (with virtual key code for proper event dispatch)
    let mut key_down = json!({
        "type": "keyDown",
        "key": key,
    });
    if vk_code > 0 {
        key_down["windowsVirtualKeyCode"] = json!(vk_code);
        key_down["nativeVirtualKeyCode"] = json!(vk_code);
    }
    if let Some(t) = text {
        key_down["text"] = json!(t);
    }
    client
        .send("Input.dispatchKeyEvent", key_down)
        .await
        .map_err(|e| ElementError::Action(format!("keyDown failed: {e}")))?;

    // keyUp
    client
        .send(
            "Input.dispatchKeyEvent",
            json!({
                "type": "keyUp",
                "key": key,
            }),
        )
        .await
        .map_err(|e| ElementError::Action(format!("keyUp failed: {e}")))?;

    wait_for_stabilization(client).await;
    Ok(())
}

/// Hover over an element by uid.
pub async fn hover(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    uid: &str,
) -> Result<(), ElementError> {
    let resolved = resolve_uid(client, uid_map, uid).await?;

    let (x, y) = resolved.center.ok_or_else(|| {
        ElementError::NotInteractable(format!(
            "Element uid={uid} has no visible box model."
        ))
    })?;

    client
        .send(
            "Input.dispatchMouseEvent",
            DispatchMouseEventParams {
                event_type: MouseEventType::MouseMoved,
                x,
                y,
                button: None,
                buttons: None,
                click_count: None,
                modifiers: None,
                timestamp: None,
                delta_x: None,
                delta_y: None,
                pointer_type: Some("mouse".into()),
            },
        )
        .await
        .map_err(|e| ElementError::Action(format!("hover failed: {e}")))?;

    Ok(())
}

/// Wait for the page to stabilize after an action.
///
/// Uses a short probe (50ms) to detect if navigation started.
/// Only waits for full page load if navigation was actually triggered.
/// Non-navigating actions (menu click, toggle, dropdown) return instantly.
async fn wait_for_stabilization(client: &CdpClient) {
    // Short probe: did this action trigger a navigation?
    let nav = client
        .wait_for_event("Page.frameNavigated", Duration::from_millis(50))
        .await;

    if nav.is_ok() {
        // Navigation detected — wait for load to complete (up to 10s)
        let _ = client
            .wait_for_event("Page.loadEventFired", Duration::from_secs(10))
            .await;
    }
    // No navigation detected — return immediately, no 500ms penalty
}

#[derive(Debug, thiserror::Error)]
pub enum ElementError {
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Detached(String),
    #[error("{0}")]
    NotInteractable(String),
    #[error("{0}")]
    Action(String),
}

/// Click at explicit (x, y) coordinates using Input.dispatchMouseEvent.
pub async fn click_at_coords(
    client: &CdpClient,
    x: f64,
    y: f64,
) -> Result<(), ElementError> {
    // mousePressed
    client
        .send(
            "Input.dispatchMouseEvent",
            DispatchMouseEventParams {
                event_type: MouseEventType::MousePressed,
                x,
                y,
                button: Some(MouseButton::Left),
                buttons: Some(1),
                click_count: Some(1),
                modifiers: None,
                timestamp: None,
                delta_x: None,
                delta_y: None,
                pointer_type: Some("mouse".into()),
            },
        )
        .await
        .map_err(|e| ElementError::Action(format!("mousePressed failed: {e}")))?;

    // mouseReleased
    client
        .send(
            "Input.dispatchMouseEvent",
            DispatchMouseEventParams {
                event_type: MouseEventType::MouseReleased,
                x,
                y,
                button: Some(MouseButton::Left),
                buttons: Some(0),
                click_count: Some(1),
                modifiers: None,
                timestamp: None,
                delta_x: None,
                delta_y: None,
                pointer_type: Some("mouse".into()),
            },
        )
        .await
        .map_err(|e| ElementError::Action(format!("mouseReleased failed: {e}")))?;

    wait_for_stabilization(client).await;
    Ok(())
}

/// Click an element matched by a CSS selector via Runtime.evaluate.
pub async fn click_selector(
    client: &CdpClient,
    selector: &str,
) -> Result<(), ElementError> {
    let js = format!(
        r"(() => {{
            const el = document.querySelector({sel});
            if (!el) throw new Error('No element matches selector: ' + {sel});
            el.click();
        }})()",
        sel = serde_json::to_string(selector).unwrap_or_default()
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

    wait_for_stabilization(client).await;
    Ok(())
}

/// Fill an element matched by a CSS selector via Runtime.evaluate.
pub async fn fill_selector(
    client: &CdpClient,
    selector: &str,
    value: &str,
) -> Result<(), ElementError> {
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

    wait_for_stabilization(client).await;
    Ok(())
}

/// Focus an element matched by a CSS selector via Runtime.evaluate.
pub async fn focus_selector(
    client: &CdpClient,
    selector: &str,
) -> Result<(), ElementError> {
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

// ---------------------------------------------------------------------------
// Double-click
// ---------------------------------------------------------------------------

/// Double-click an element by uid.
pub async fn dblclick(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    uid: &str,
) -> Result<(), ElementError> {
    let resolved = resolve_uid(client, uid_map, uid).await?;

    if resolved.center.is_none() {
        return js_dblclick(client, &resolved.object_id).await;
    }

    let _ = client
        .call::<_, serde_json::Value>(
            "Runtime.callFunctionOn",
            json!({
                "objectId": resolved.object_id,
                "functionDeclaration": "function() { this.scrollIntoViewIfNeeded(); }",
                "returnByValue": true,
            }),
        )
        .await;

    let box_result: Result<GetBoxModelResult, _> = client
        .call("DOM.getBoxModel", json!({ "backendNodeId": resolved.backend_node_id }))
        .await;

    let Some((cx, cy)) = box_result.ok().map(|r| r.model.content_center()) else {
        return js_dblclick(client, &resolved.object_id).await;
    };

    for click_count in [1, 2] {
        client
            .send("Input.dispatchMouseEvent", DispatchMouseEventParams {
                event_type: MouseEventType::MousePressed,
                x: cx, y: cy,
                button: Some(MouseButton::Left), buttons: Some(1),
                click_count: Some(click_count),
                modifiers: None, timestamp: None, delta_x: None, delta_y: None,
                pointer_type: Some("mouse".into()),
            })
            .await
            .map_err(|e| ElementError::Action(format!("mousePressed failed: {e}")))?;

        client
            .send("Input.dispatchMouseEvent", DispatchMouseEventParams {
                event_type: MouseEventType::MouseReleased,
                x: cx, y: cy,
                button: Some(MouseButton::Left), buttons: Some(0),
                click_count: Some(click_count),
                modifiers: None, timestamp: None, delta_x: None, delta_y: None,
                pointer_type: Some("mouse".into()),
            })
            .await
            .map_err(|e| ElementError::Action(format!("mouseReleased failed: {e}")))?;
    }

    wait_for_stabilization(client).await;
    Ok(())
}

async fn js_dblclick(client: &CdpClient, object_id: &str) -> Result<(), ElementError> {
    client
        .call::<_, serde_json::Value>(
            "Runtime.callFunctionOn",
            json!({
                "objectId": object_id,
                "functionDeclaration": "function() { this.dispatchEvent(new MouseEvent('dblclick', {bubbles:true, cancelable:true})); }",
                "returnByValue": true,
            }),
        )
        .await
        .map_err(|e| ElementError::Action(format!("JS dblclick failed: {e}")))?;

    wait_for_stabilization(client).await;
    Ok(())
}

/// Double-click at coordinates.
pub async fn dblclick_at_coords(client: &CdpClient, x: f64, y: f64) -> Result<(), ElementError> {
    for click_count in [1, 2] {
        client
            .send("Input.dispatchMouseEvent", DispatchMouseEventParams {
                event_type: MouseEventType::MousePressed, x, y,
                button: Some(MouseButton::Left), buttons: Some(1),
                click_count: Some(click_count),
                modifiers: None, timestamp: None, delta_x: None, delta_y: None,
                pointer_type: Some("mouse".into()),
            })
            .await
            .map_err(|e| ElementError::Action(format!("mousePressed failed: {e}")))?;

        client
            .send("Input.dispatchMouseEvent", DispatchMouseEventParams {
                event_type: MouseEventType::MouseReleased, x, y,
                button: Some(MouseButton::Left), buttons: Some(0),
                click_count: Some(click_count),
                modifiers: None, timestamp: None, delta_x: None, delta_y: None,
                pointer_type: Some("mouse".into()),
            })
            .await
            .map_err(|e| ElementError::Action(format!("mouseReleased failed: {e}")))?;
    }
    wait_for_stabilization(client).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Select
// ---------------------------------------------------------------------------

/// Select a dropdown option by uid and value/text.
pub async fn select_option(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    uid: &str,
    value: &str,
) -> Result<String, ElementError> {
    let resolved = resolve_uid(client, uid_map, uid).await?;
    let js = r"function(target) {
        if (this.tagName !== 'SELECT') throw new Error('Element is not a <select>');
        const opts = Array.from(this.options);
        let idx = opts.findIndex(o => o.value === target);
        if (idx === -1) idx = opts.findIndex(o => o.text.trim() === target);
        if (idx === -1) throw new Error('No option matching: ' + target);
        this.selectedIndex = idx;
        this.dispatchEvent(new Event('change', {bubbles: true}));
        return opts[idx].text;
    }";
    let result: serde_json::Value = client
        .call("Runtime.callFunctionOn", json!({
            "objectId": resolved.object_id,
            "functionDeclaration": js,
            "arguments": [{"value": value}],
            "returnByValue": true,
        }))
        .await
        .map_err(|e| ElementError::Action(format!("select_option failed: {e}")))?;

    check_js_exception(&result)?;
    let text = result.get("result")
        .and_then(|r| r.get("value"))
        .and_then(|v| v.as_str())
        .unwrap_or(value);
    Ok(text.to_string())
}

/// Select a dropdown option by CSS selector.
pub async fn select_option_selector(
    client: &CdpClient,
    selector: &str,
    value: &str,
) -> Result<String, ElementError> {
    let sel_json = serde_json::to_string(selector).unwrap_or_default();
    let val_json = serde_json::to_string(value).unwrap_or_default();
    let js = format!(
        r"(() => {{
            const el = document.querySelector({sel_json});
            if (!el) throw new Error('No element matches selector: ' + {sel_json});
            if (el.tagName !== 'SELECT') throw new Error('Element is not a <select>');
            const opts = Array.from(el.options);
            let idx = opts.findIndex(o => o.value === {val_json});
            if (idx === -1) idx = opts.findIndex(o => o.text.trim() === {val_json});
            if (idx === -1) throw new Error('No option matching: ' + {val_json});
            el.selectedIndex = idx;
            el.dispatchEvent(new Event('change', {{bubbles: true}}));
            return opts[idx].text;
        }})()"
    );
    let result: serde_json::Value = client
        .call("Runtime.evaluate", json!({"expression": js, "returnByValue": true}))
        .await
        .map_err(|e| ElementError::Action(format!("select_option_selector failed: {e}")))?;

    check_js_exception(&result)?;
    let text = result.get("result")
        .and_then(|r| r.get("value"))
        .and_then(|v| v.as_str())
        .unwrap_or(value);
    Ok(text.to_string())
}

// ---------------------------------------------------------------------------
// Check / Uncheck
// ---------------------------------------------------------------------------

/// Idempotent check/uncheck: query current state, click only if different.
pub async fn set_checked(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    uid: &str,
    desired: bool,
) -> Result<String, ElementError> {
    let resolved = resolve_uid(client, uid_map, uid).await?;

    let result: serde_json::Value = client
        .call("Runtime.callFunctionOn", json!({
            "objectId": resolved.object_id,
            "functionDeclaration": "function() { return !!this.checked; }",
            "returnByValue": true,
        }))
        .await
        .map_err(|e| ElementError::Action(format!("get checked state failed: {e}")))?;

    let current = result.get("result")
        .and_then(|r| r.get("value"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    let state_word = if desired { "checked" } else { "unchecked" };
    if current == desired {
        return Ok(format!("Already {state_word} uid={uid}"));
    }

    click(client, uid_map, uid).await?;
    Ok(format!("{} uid={uid}", if desired { "Checked" } else { "Unchecked" }))
}

// ---------------------------------------------------------------------------
// File upload
// ---------------------------------------------------------------------------

/// Set files on a file input using `DOM.setFileInputFiles`.
pub async fn set_file_input(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    uid: &str,
    files: &[String],
) -> Result<(), ElementError> {
    for f in files {
        if !std::path::Path::new(f).exists() {
            return Err(ElementError::Action(format!("File not found: {f}")));
        }
    }
    let resolved = resolve_uid(client, uid_map, uid).await?;
    client
        .send("DOM.setFileInputFiles", json!({
            "files": files,
            "backendNodeId": resolved.backend_node_id,
        }))
        .await
        .map_err(|e| ElementError::Action(format!("setFileInputFiles failed: {e}")))?;
    wait_for_stabilization(client).await;
    Ok(())
}

/// Set files on a file input identified by CSS selector.
pub async fn set_file_input_selector(
    client: &CdpClient,
    selector: &str,
    files: &[String],
) -> Result<(), ElementError> {
    for f in files {
        if !std::path::Path::new(f).exists() {
            return Err(ElementError::Action(format!("File not found: {f}")));
        }
    }
    let sel_json = serde_json::to_string(selector).unwrap_or_default();
    let node: serde_json::Value = client
        .call("Runtime.evaluate", json!({
            "expression": format!("(() => {{ const el = document.querySelector({sel_json}); if (!el) throw new Error('No element matches selector: ' + {sel_json}); return true; }})()"),
            "returnByValue": true,
        }))
        .await
        .map_err(|e| ElementError::Action(format!("set_file_input_selector resolve failed: {e}")))?;
    check_js_exception(&node)?;

    let doc: serde_json::Value = client
        .call("DOM.getDocument", json!({"depth": 0}))
        .await
        .map_err(|e| ElementError::Action(format!("DOM.getDocument failed: {e}")))?;
    let root_node_id = doc.get("root")
        .and_then(|r| r.get("nodeId"))
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| ElementError::Action("Could not get root nodeId".into()))?;

    let qs_result: serde_json::Value = client
        .call("DOM.querySelector", json!({"nodeId": root_node_id, "selector": selector}))
        .await
        .map_err(|e| ElementError::Action(format!("DOM.querySelector failed: {e}")))?;
    let node_id = qs_result.get("nodeId")
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| ElementError::Action(format!("No element matches selector: {selector}")))?;

    client
        .send("DOM.setFileInputFiles", json!({
            "files": files,
            "nodeId": node_id,
        }))
        .await
        .map_err(|e| ElementError::Action(format!("setFileInputFiles failed: {e}")))?;
    wait_for_stabilization(client).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Drag
// ---------------------------------------------------------------------------

/// Drag from one element to another.
pub async fn drag(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    from_uid: &str,
    to_uid: &str,
) -> Result<(), ElementError> {
    let from = resolve_uid(client, uid_map, from_uid).await?;
    let to = resolve_uid(client, uid_map, to_uid).await?;

    let (x1, y1) = from.center.ok_or_else(|| {
        ElementError::NotInteractable(format!("Element uid={from_uid} has no visible box model."))
    })?;
    let (x2, y2) = to.center.ok_or_else(|| {
        ElementError::NotInteractable(format!("Element uid={to_uid} has no visible box model."))
    })?;

    let mouse = |et, x, y, btn: Option<MouseButton>, btns, cc| {
        DispatchMouseEventParams {
            event_type: et, x, y,
            button: btn, buttons: btns, click_count: cc,
            modifiers: None, timestamp: None, delta_x: None, delta_y: None,
            pointer_type: Some("mouse".into()),
        }
    };

    client.send("Input.dispatchMouseEvent",
        mouse(MouseEventType::MouseMoved, x1, y1, None, None, None))
        .await.map_err(|e| ElementError::Action(format!("drag move failed: {e}")))?;

    client.send("Input.dispatchMouseEvent",
        mouse(MouseEventType::MousePressed, x1, y1, Some(MouseButton::Left), Some(1), Some(1)))
        .await.map_err(|e| ElementError::Action(format!("drag press failed: {e}")))?;

    let steps = 5u32;
    for i in 1..=steps {
        let t = f64::from(i) / f64::from(steps);
        let x = (x2 - x1).mul_add(t, x1);
        let y = (y2 - y1).mul_add(t, y1);
        client.send("Input.dispatchMouseEvent",
            mouse(MouseEventType::MouseMoved, x, y, Some(MouseButton::Left), Some(1), None))
            .await.map_err(|e| ElementError::Action(format!("drag step failed: {e}")))?;
        tokio::time::sleep(Duration::from_millis(16)).await;
    }

    client.send("Input.dispatchMouseEvent",
        mouse(MouseEventType::MouseReleased, x2, y2, Some(MouseButton::Left), Some(0), Some(1)))
        .await.map_err(|e| ElementError::Action(format!("drag release failed: {e}")))?;

    wait_for_stabilization(client).await;
    Ok(())
}

fn check_js_exception(result: &serde_json::Value) -> Result<(), ElementError> {
    if let Some(exception) = result.get("exceptionDetails") {
        let text = exception
            .get("exception")
            .and_then(|ex| ex.get("description"))
            .and_then(|d| d.as_str())
            .or_else(|| exception.get("text").and_then(|t| t.as_str()))
            .unwrap_or("unknown error");
        return Err(ElementError::Action(text.to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_interpolation_5_steps() {
        let (x1, y1) = (100.0, 100.0);
        let (x2, y2) = (200.0, 300.0);
        let steps = 5u32;
        let points: Vec<(f64, f64)> = (1..=steps)
            .map(|i| {
                let t = f64::from(i) / f64::from(steps);
                (x1 + (x2 - x1) * t, y1 + (y2 - y1) * t)
            })
            .collect();
        assert_eq!(points.len(), 5);
        assert!((points[0].0 - 120.0).abs() < 0.01);
        assert!((points[0].1 - 140.0).abs() < 0.01);
        assert!((points[4].0 - 200.0).abs() < 0.01);
        assert!((points[4].1 - 300.0).abs() < 0.01);
    }

    #[test]
    fn check_js_exception_none() {
        let val = serde_json::json!({"result": {"value": true}});
        assert!(check_js_exception(&val).is_ok());
    }

    #[test]
    fn check_js_exception_present() {
        let val = serde_json::json!({"exceptionDetails": {"text": "boom"}});
        let err = check_js_exception(&val).unwrap_err();
        assert!(err.to_string().contains("boom"));
    }

    #[test]
    fn file_not_found_validation() {
        assert!(!std::path::Path::new("/nonexistent/file.txt").exists());
    }
}
