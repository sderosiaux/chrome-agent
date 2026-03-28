use std::collections::HashMap;
use std::time::Duration;

use serde_json::json;

use crate::cdp::client::CdpClient;
use crate::cdp::types::{
    BoxModel, DispatchMouseEventParams, GetBoxModelResult, MouseButton, MouseEventType,
    RemoteObject, ResolveNodeParams, ResolveNodeResult,
};
use crate::element_ref::ElementRef;

/// Resolve a uid to a CDP objectId via the ElementRef in the uid map.
async fn resolve_uid(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    uid: &str,
) -> Result<ResolvedElement, ElementError> {
    let element_ref = uid_map.get(uid).ok_or_else(|| {
        ElementError::NotFound(format!(
            "Element uid={uid} not found. Run 'aibrowsr inspect' to get fresh uids."
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
                 Run 'aibrowsr inspect' to get fresh uids. ({e})"
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
pub async fn click(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    uid: &str,
) -> Result<(), ElementError> {
    let resolved = resolve_uid(client, uid_map, uid).await?;

    let (x, y) = resolved.center.ok_or_else(|| {
        ElementError::NotInteractable(format!(
            "Element uid={uid} has no visible box model. It may be hidden or zero-sized."
        ))
    })?;

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

    let (cx, cy) = box_result
        .map(|r| r.model.content_center())
        .unwrap_or((x, y));

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

/// Fill an element (input/textarea) by uid.
pub async fn fill(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    uid: &str,
    value: &str,
) -> Result<(), ElementError> {
    let resolved = resolve_uid(client, uid_map, uid).await?;

    // Focus, clear, set value, dispatch events
    let js = format!(
        r#"function(v) {{
            this.focus();
            this.value = '';
            this.value = v;
            this.dispatchEvent(new Event('input', {{bubbles: true}}));
            this.dispatchEvent(new Event('change', {{bubbles: true}}));
        }}"#
    );

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
    // keyDown
    client
        .send(
            "Input.dispatchKeyEvent",
            json!({
                "type": "keyDown",
                "key": key,
            }),
        )
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

#[derive(Debug)]
pub enum ElementError {
    NotFound(String),
    Detached(String),
    NotInteractable(String),
    Action(String),
}

impl std::fmt::Display for ElementError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(msg) | Self::Detached(msg) | Self::NotInteractable(msg) | Self::Action(msg) => {
                write!(f, "{msg}")
            }
        }
    }
}

impl std::error::Error for ElementError {}

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
        r#"(() => {{
            const el = document.querySelector({sel});
            if (!el) throw new Error('No element matches selector: ' + {sel});
            el.click();
        }})()"#,
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
        r#"(() => {{
            const el = document.querySelector({sel});
            if (!el) throw new Error('No element matches selector: ' + {sel});
            el.focus();
            el.value = '';
            el.value = {val};
            el.dispatchEvent(new Event('input', {{bubbles: true}}));
            el.dispatchEvent(new Event('change', {{bubbles: true}}));
        }})()"#,
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
        r#"(() => {{
            const el = document.querySelector({sel});
            if (!el) throw new Error('No element matches selector: ' + {sel});
            el.focus();
        }})()"#,
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
