use std::collections::HashMap;
use std::time::Duration;

use serde_json::json;
use tokio::sync::broadcast;

use crate::cdp::client::CdpClient;
use crate::cdp::types::{
    CdpEvent, DispatchMouseEventParams, GetBoxModelResult, MouseButton, MouseEventType, ResolveNodeParams,
    ResolveNodeResult,
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
        .call("DOM.resolveNode", ResolveNodeParams {
            node_id: None,
            backend_node_id: Some(backend_node_id),
            object_group: Some("dev-browser".into()),
            execution_context_id: None,
        })
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

    // Subscribe BEFORE dispatching so a fast navigation isn't missed.
    let nav_events = client.events();
    // mousePressed
    client
        .send("Input.dispatchMouseEvent", DispatchMouseEventParams {
            event_type: MouseEventType::MousePressed,
            x: cx, y: cy,
            button: Some(MouseButton::Left), buttons: Some(1), click_count: Some(1),
            modifiers: None, timestamp: None, delta_x: None, delta_y: None,
            pointer_type: Some("mouse".into()),
        })
        .await
        .map_err(|e| ElementError::Action(format!("mousePressed failed: {e}")))?;

    // mouseReleased
    client
        .send("Input.dispatchMouseEvent", DispatchMouseEventParams {
            event_type: MouseEventType::MouseReleased,
            x: cx, y: cy,
            button: Some(MouseButton::Left), buttons: Some(0), click_count: Some(1),
            modifiers: None, timestamp: None, delta_x: None, delta_y: None,
            pointer_type: Some("mouse".into()),
        })
        .await
        .map_err(|e| ElementError::Action(format!("mouseReleased failed: {e}")))?;

    wait_for_stabilization(nav_events).await;
    Ok(())
}

/// Fallback: click an element via JS `.click()` when mouse events can't be dispatched.
async fn js_click(client: &CdpClient, object_id: &str) -> Result<(), ElementError> {
    let nav_events = client.events();
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

    wait_for_stabilization(nav_events).await;
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

    let nav_events = client.events();
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

    wait_for_stabilization(nav_events).await;
    Ok(())
}

/// Type text character by character using Input.insertText.
pub async fn type_text(
    client: &CdpClient,
    text: &str,
) -> Result<(), ElementError> {
    let nav_events = client.events();
    client
        .send("Input.insertText", json!({ "text": text }))
        .await
        .map_err(|e| ElementError::Action(format!("insertText failed: {e}")))?;

    wait_for_stabilization(nav_events).await;
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
    let nav_events = client.events();
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

    wait_for_stabilization(nav_events).await;
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
        .send("Input.dispatchMouseEvent", DispatchMouseEventParams {
            event_type: MouseEventType::MouseMoved,
            x, y,
            button: None, buttons: None, click_count: None,
            modifiers: None, timestamp: None, delta_x: None, delta_y: None,
            pointer_type: Some("mouse".into()),
        })
        .await
        .map_err(|e| ElementError::Action(format!("hover failed: {e}")))?;

    Ok(())
}

/// Wait (≤`timeout`) for one event matching `method` on an already-open
/// subscription. `true` if it arrived. Lagged: keep going, the event may follow.
async fn recv_event(rx: &mut broadcast::Receiver<CdpEvent>, method: &str, timeout: Duration) -> bool {
    tokio::time::timeout(timeout, async {
        loop {
            match rx.recv().await {
                Ok(event) if event.method == method => return true,
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return false,
            }
        }
    })
    .await
    .unwrap_or(false)
}

/// Wait for the page to stabilize after an action. `nav_events` MUST be
/// subscribed (`client.events()`) BEFORE dispatching the action — `broadcast`
/// only delivers post-subscribe messages, so a fast `frameNavigated`/
/// `loadEventFired` firing before we wait would be missed (the `goto` race).
/// 50ms probe for navigation; only then wait (≤10s) for load.
pub async fn wait_for_stabilization(mut nav_events: broadcast::Receiver<CdpEvent>) {
    if recv_event(&mut nav_events, "Page.frameNavigated", Duration::from_millis(50)).await {
        let _ = recv_event(&mut nav_events, "Page.loadEventFired", Duration::from_secs(10)).await;
    }
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
    // Subscribe BEFORE dispatching so a fast navigation isn't missed.
    let nav_events = client.events();
    // mousePressed
    client
        .send("Input.dispatchMouseEvent", DispatchMouseEventParams {
            event_type: MouseEventType::MousePressed,
            x, y,
            button: Some(MouseButton::Left), buttons: Some(1), click_count: Some(1),
            modifiers: None, timestamp: None, delta_x: None, delta_y: None,
            pointer_type: Some("mouse".into()),
        })
        .await
        .map_err(|e| ElementError::Action(format!("mousePressed failed: {e}")))?;

    // mouseReleased
    client
        .send("Input.dispatchMouseEvent", DispatchMouseEventParams {
            event_type: MouseEventType::MouseReleased,
            x, y,
            button: Some(MouseButton::Left), buttons: Some(0), click_count: Some(1),
            modifiers: None, timestamp: None, delta_x: None, delta_y: None,
            pointer_type: Some("mouse".into()),
        })
        .await
        .map_err(|e| ElementError::Action(format!("mouseReleased failed: {e}")))?;

    wait_for_stabilization(nav_events).await;
    Ok(())
}

// Selector-based actions (click/dblclick/fill/focus) live in `element_selector`
// to keep this file under the 1000-line module cap; re-exported here so callers
// keep using `crate::element::*`.
pub use crate::element_selector::{click_selector, dblclick_selector, fill_selector, focus_selector};

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

    let nav_events = client.events();
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

    wait_for_stabilization(nav_events).await;
    Ok(())
}

async fn js_dblclick(client: &CdpClient, object_id: &str) -> Result<(), ElementError> {
    let nav_events = client.events();
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

    wait_for_stabilization(nav_events).await;
    Ok(())
}

/// Double-click at coordinates.
pub async fn dblclick_at_coords(client: &CdpClient, x: f64, y: f64) -> Result<(), ElementError> {
    let nav_events = client.events();
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
    wait_for_stabilization(nav_events).await;
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

/// Idempotent check/uncheck by CSS selector.
pub async fn set_checked_selector(
    client: &CdpClient,
    selector: &str,
    desired: bool,
) -> Result<String, ElementError> {
    let sel_json = serde_json::to_string(selector).unwrap_or_default();
    let desired_js = if desired { "true" } else { "false" };
    let js = format!(
        r"(() => {{
            const el = document.querySelector({sel_json});
            if (!el) throw new Error('No element matches selector: ' + {sel_json});
            const current = !!el.checked;
            if (current === {desired_js}) return 'already';
            el.click();
            return 'toggled';
        }})()"
    );
    let result: serde_json::Value = client
        .call("Runtime.evaluate", json!({"expression": js, "returnByValue": true}))
        .await
        .map_err(|e| ElementError::Action(format!("set_checked_selector failed: {e}")))?;

    check_js_exception(&result)?;
    let action = result.get("result")
        .and_then(|r| r.get("value"))
        .and_then(|v| v.as_str())
        .unwrap_or("toggled");
    let state_word = if desired { "checked" } else { "unchecked" };
    if action == "already" {
        Ok(format!("Already {state_word} selector '{selector}'"))
    } else {
        Ok(format!("{} selector '{selector}'", if desired { "Checked" } else { "Unchecked" }))
    }
}

// ---------------------------------------------------------------------------
// File upload
// ---------------------------------------------------------------------------

/// Validate every upload path exists before invoking CDP; returns the first
/// missing path as `ElementError::Action`. Shared by both upload entry points.
fn validate_upload_paths(files: &[String]) -> Result<(), ElementError> {
    for f in files {
        if !std::path::Path::new(f).exists() {
            return Err(ElementError::Action(format!("File not found: {f}")));
        }
    }
    Ok(())
}

/// Set files on a file input using `DOM.setFileInputFiles`.
pub async fn set_file_input(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    uid: &str,
    files: &[String],
) -> Result<(), ElementError> {
    validate_upload_paths(files)?;
    let resolved = resolve_uid(client, uid_map, uid).await?;
    let nav_events = client.events();
    client
        .send("DOM.setFileInputFiles", json!({
            "files": files,
            "backendNodeId": resolved.backend_node_id,
        }))
        .await
        .map_err(|e| ElementError::Action(format!("setFileInputFiles failed: {e}")))?;
    wait_for_stabilization(nav_events).await;
    Ok(())
}

/// Set files on a file input identified by CSS selector.
pub async fn set_file_input_selector(
    client: &CdpClient,
    selector: &str,
    files: &[String],
) -> Result<(), ElementError> {
    validate_upload_paths(files)?;
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

    let nav_events = client.events();
    client
        .send("DOM.setFileInputFiles", json!({
            "files": files,
            "nodeId": node_id,
        }))
        .await
        .map_err(|e| ElementError::Action(format!("setFileInputFiles failed: {e}")))?;
    wait_for_stabilization(nav_events).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Drag
// ---------------------------------------------------------------------------

/// Linear-interpolate the mouse-move points for a drag from `(x1,y1)` to
/// `(x2,y2)` over `steps` segments (last point lands on the destination).
/// Extracted so `drag` and its regression test exercise the *same* math.
fn drag_interpolation_points(x1: f64, y1: f64, x2: f64, y2: f64, steps: u32) -> Vec<(f64, f64)> {
    (1..=steps)
        .map(|i| {
            let t = f64::from(i) / f64::from(steps);
            ((x2 - x1).mul_add(t, x1), (y2 - y1).mul_add(t, y1))
        })
        .collect()
}

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

    let nav_events = client.events();
    client.send("Input.dispatchMouseEvent",
        mouse(MouseEventType::MouseMoved, x1, y1, None, None, None))
        .await.map_err(|e| ElementError::Action(format!("drag move failed: {e}")))?;

    client.send("Input.dispatchMouseEvent",
        mouse(MouseEventType::MousePressed, x1, y1, Some(MouseButton::Left), Some(1), Some(1)))
        .await.map_err(|e| ElementError::Action(format!("drag press failed: {e}")))?;

    for (x, y) in drag_interpolation_points(x1, y1, x2, y2, 5) {
        client.send("Input.dispatchMouseEvent",
            mouse(MouseEventType::MouseMoved, x, y, Some(MouseButton::Left), Some(1), None))
            .await.map_err(|e| ElementError::Action(format!("drag step failed: {e}")))?;
        tokio::time::sleep(Duration::from_millis(16)).await;
    }

    client.send("Input.dispatchMouseEvent",
        mouse(MouseEventType::MouseReleased, x2, y2, Some(MouseButton::Left), Some(0), Some(1)))
        .await.map_err(|e| ElementError::Action(format!("drag release failed: {e}")))?;

    wait_for_stabilization(nav_events).await;
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

    fn ev(method: &str) -> CdpEvent {
        CdpEvent { method: method.to_string(), params: serde_json::Value::Null, session_id: None }
    }

    #[test]
    fn drag_interpolation_5_steps() {
        // Exercises the real fn `drag` calls (not a re-implementation of the math).
        let points = drag_interpolation_points(100.0, 100.0, 200.0, 300.0, 5);
        assert_eq!(points.len(), 5);
        assert!((points[0].0 - 120.0).abs() < 0.01);
        assert!((points[0].1 - 140.0).abs() < 0.01);
        // Last point lands exactly on the destination.
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
    fn upload_validation_rejects_missing_and_accepts_existing() {
        // Missing path → error through the real upload validation code path.
        let missing = vec!["/nonexistent/file.txt".to_string()];
        let err = validate_upload_paths(&missing).unwrap_err();
        assert!(matches!(err, ElementError::Action(_)));
        assert!(err.to_string().contains("File not found: /nonexistent/file.txt"));
        // An existing path (this test binary) passes validation.
        let exe = std::env::current_exe().unwrap().to_string_lossy().into_owned();
        assert!(validate_upload_paths(&[exe]).is_ok());
    }

    // A10f: a receiver subscribed BEFORE the action still observes navigation
    // events that fired before we start waiting — the fast-load race the fix closes.
    #[tokio::test]
    async fn stabilization_sees_navigation_buffered_before_wait() {
        let (tx, _) = broadcast::channel::<CdpEvent>(16);
        let rx = tx.subscribe(); // subscribe first (pre-action)
        tx.send(ev("Page.frameNavigated")).unwrap();
        tx.send(ev("Page.loadEventFired")).unwrap();
        // Both events already buffered → completes promptly, does not hang.
        tokio::time::timeout(Duration::from_secs(1), wait_for_stabilization(rx))
            .await
            .expect("should not hang when nav events are already buffered");
    }

    #[tokio::test]
    async fn recv_event_times_out_without_match() {
        let (tx, _) = broadcast::channel::<CdpEvent>(16);
        let mut rx = tx.subscribe();
        tx.send(ev("Runtime.consoleAPICalled")).unwrap();
        // Only an unrelated event → probe returns false quickly (no navigation).
        assert!(!recv_event(&mut rx, "Page.frameNavigated", Duration::from_millis(20)).await);
    }
}
