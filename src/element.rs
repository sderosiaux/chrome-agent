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
pub async fn resolve_uid(
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

pub struct ResolvedElement {
    pub object_id: String,
    pub center: Option<(f64, f64)>,
    pub backend_node_id: i64,
}

/// Click an element by uid.
///
/// The aim point comes from `hit_test::aim`, which scrolls the element into view, measures
/// where a click on it would go, and says what sits there — one round trip, replacing the
/// separate `scrollIntoViewIfNeeded` call and second `DOM.getBoxModel` this used to do. What
/// the probe reports is what the response reports: a click delivered to something else is
/// `intercepted` rather than a success indistinguishable from a real one, and an aim point
/// still moving under a smooth scroll is refused rather than dispatched into empty space.
///
/// An element with no layout box still falls back to a JS `.click()`, where there is no point
/// to aim at and no hit test to run.
pub async fn click(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    uid: &str,
    on_intercept: crate::hit_test::OnIntercept,
) -> Result<crate::hit_test::Dispatched, ElementError> {
    let resolved = resolve_uid(client, uid_map, uid).await?;
    if resolved.center.is_none() {
        js_click(client, &resolved.object_id).await?;
        return Ok(crate::hit_test::Dispatched::js().named(Some(uid.to_string()), None, None));
    }
    let outcome = click_handle(
        client,
        &resolved.object_id,
        resolved.center,
        on_intercept,
        &format!("uid={uid}"),
    )
    .await?;
    Ok(outcome.named(Some(uid.to_string()), None, None))
}

/// Aim at a resolved handle and single-click it. Shared by the uid and the selector paths, so
/// the two spellings of `click` cannot drift apart again.
///
/// `fallback_center` is the box model's centre, used only when the probe itself could not run.
pub async fn click_handle(
    client: &CdpClient,
    object_id: &str,
    fallback_center: Option<(f64, f64)>,
    on_intercept: crate::hit_test::OnIntercept,
    target: &str,
) -> Result<crate::hit_test::Dispatched, ElementError> {
    use crate::hit_test::{Aim, Dispatched};
    use crate::verdict::Delivery;

    let (point, delivery, receiver) = match crate::hit_test::aim(client, object_id).await {
        Aim::NoBox => {
            js_click(client, object_id).await?;
            return Ok(Dispatched::js());
        }
        Aim::Unprobed => {
            let Some(center) = fallback_center else {
                js_click(client, object_id).await?;
                return Ok(Dispatched::js());
            };
            (center, Delivery::NotProbed, None)
        }
        Aim::At { point, delivery, receiver } => (point, delivery, receiver),
    };

    if matches!(delivery, Delivery::NotSettled | Delivery::OffTarget) {
        return Ok(Dispatched::skipped(delivery, point, None));
    }
    if delivery == Delivery::Intercepted && on_intercept == crate::hit_test::OnIntercept::Refuse {
        let refused = Dispatched::skipped(delivery, point, receiver);
        return Err(ElementError::NotInteractable(
            refused
                .refusal_message("click", target)
                .unwrap_or_else(|| format!("Refused to click {target}")),
        ));
    }

    dispatch_click_at(client, point.0, point.1).await?;
    Ok(Dispatched::landed(delivery, point, receiver))
}

/// A click at coordinates somebody else decided on: mouse normally, touch under emulation.
async fn dispatch_click_at(client: &CdpClient, cx: f64, cy: f64) -> Result<(), ElementError> {
    // Subscribe BEFORE dispatching so a fast navigation isn't missed.
    let nav_events = client.events();
    client.mark_dispatch();
    if client.touch_emulation_enabled() {
        client
            .send(
                "Input.dispatchTouchEvent",
                json!({
                    "type": "touchStart",
                    "touchPoints": [{"x": cx, "y": cy, "id": 0}],
                }),
            )
            .await
            .map_err(|e| ElementError::Action(format!("touchStart failed: {e}")))?;
        client
            .send(
                "Input.dispatchTouchEvent",
                json!({"type": "touchEnd", "touchPoints": []}),
            )
            .await
            .map_err(|e| ElementError::Action(format!("touchEnd failed: {e}")))?;
    } else {
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
    }

    wait_for_stabilization(nav_events).await;
    Ok(())
}

/// Fallback: click an element via JS `.click()` when mouse events can't be dispatched.
pub async fn js_click(client: &CdpClient, object_id: &str) -> Result<(), ElementError> {
    let nav_events = client.events();
    client.mark_dispatch();
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

/// What the page holds after a fill, next to what was asked for.
///
/// A write is a request. Masks reformat, `maxlength` truncates, controlled components
/// rewrite, and number inputs discard what they cannot parse. Reporting only "filled"
/// hides all four, and reporting failure would be wrong for all four too — the value did
/// land, just not verbatim.
pub struct FillOutcome {
    pub requested: String,
    pub actual: Option<String>,
    /// The field holds a secret, so neither value may be reported. The response still says
    /// whether the write landed verbatim and how long it is, which is what the caller
    /// needs, without putting a password on stdout, into an agent transcript and into any
    /// `--record` file.
    pub sensitive: bool,
    /// Set when the value that landed could not have been typed by a person: `maxlength`
    /// constrains the editing pipeline, not the value setter, so a programmatic fill walks
    /// straight past it and the form will reject the field on submit.
    pub caveat: Option<String>,
    /// How long after the write the value was read. "The field holds X" is only ever true
    /// as of a moment, and this is the moment.
    pub observed_after_ms: u64,
}

impl FillOutcome {
    pub fn new(requested: &str, actual: Option<String>) -> Self {
        Self {
            requested: requested.to_string(),
            actual,
            caveat: None,
            sensitive: false,
            observed_after_ms: READ_BACK_MS,
        }
    }

    /// Mark the outcome as holding a secret.
    pub const fn secret(mut self, sensitive: bool) -> Self {
        self.sensitive = sensitive;
        self
    }

    /// Attach the over-the-cap caveat when a `maxlength` was bypassed.
    pub fn with_max_length(mut self, max_length: Option<i64>) -> Self {
        if let (Some(max), Some(actual)) = (max_length, self.actual.as_deref())
            && let Ok(cap) = usize::try_from(max)
            && actual.chars().count() > cap
        {
            {
                self.caveat = Some(format!(
                    "exceeds maxlength={max}; a person typing could not have produced this, \
                     and the form is likely to reject it"
                ));
            }
        }
        self
    }

    /// True when the page holds exactly what was asked for.
    pub fn verbatim(&self) -> bool {
        self.actual.as_deref() == Some(self.requested.as_str())
    }
}

/// Whether a field holds something that must never be printed, as a JS expression over `el`.
///
/// One reader, four callers: `fill` by uid and by selector, `assert value`, and the
/// `values_lost` report. The predicate decides whether a value reaches stdout, an agent
/// transcript and any `--record` file, so four copies of it that agree today is four chances
/// for one of them to be widened alone — the same reason `CHECKABLE_PROBE` and `SELECT_READ`
/// are shared between their action and their assertion.
///
/// `type=password` is masked by Chrome in the accessibility tree as well; the `autocomplete`
/// half is not, so a one-time code or a card number in a `type=text` field is only redacted
/// because this predicate names it.
pub const SECRET_FIELD: &str =
    r"(el.type === 'password' || /password|cc-number|cc-csc|one-time-code/i.test(el.autocomplete || ''))";

/// How long a read-back waits before looking at what the page kept.
///
/// The three read-back paths used to disagree: `fill` read synchronously (0ms), so a value
/// reverted one microtask later was reported as kept — verbatim:true on a field the page
/// had already emptied. `check --selector` waited 60ms, `check <uid>` waited for however
/// long a CDP round trip happened to take.
///
/// 60ms catches a revert on the microtask queue, in a `setTimeout(0)`, or in an animation
/// frame — the shapes a controlled component uses. It does NOT catch a validator that
/// fires at 400ms (`tests/fixtures/form_value_late_revert.html`), and no fixed window
/// could: a page may revert at any time. That is why every read-back reports
/// `observed_after_ms` alongside the value rather than asserting persistence. Raising it
/// would buy a few more shapes at the cost of that much latency on every fill and check.
pub const READ_BACK_MS: u64 = 60;

/// Fill an element (input/textarea) by uid.
pub async fn fill(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    uid: &str,
    value: &str,
) -> Result<FillOutcome, ElementError> {
    let resolved = resolve_uid(client, uid_map, uid).await?;

    // Focus, clear, set value, dispatch events.
    // Use the native HTMLInputElement/HTMLTextAreaElement value setter so React's
    // synthetic onChange fires (React wraps the descriptor; direct assignment is
    // intercepted by React but the setter via Object.getOwnPropertyDescriptor is not).
    let js = r"function(v) {
            if (this.matches(':disabled')) throw new Error('Element is disabled and cannot be filled');
            if (this.readOnly) throw new Error('Element is readonly and cannot be filled');
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
            var el = this;
            // Read after the window, not on the next line: a controlled component that
            // reverts in a promise callback has not run yet when the write returns.
            return new Promise(function (resolve) {
                setTimeout(function () {
                    resolve({
                        value: el.value === undefined ? null : String(el.value),
                        maxLength: typeof el.maxLength === 'number' ? el.maxLength : null,
                        sensitive: SECRET_EXPR
                    });
                }, WINDOW_MS);
            });
        }".replace("WINDOW_MS", &READ_BACK_MS.to_string())
        .replace("SECRET_EXPR", SECRET_FIELD);

    let nav_events = client.events();
    let result: serde_json::Value = client
        .call(
            "Runtime.callFunctionOn",
            json!({
                "objectId": resolved.object_id,
                "functionDeclaration": js,
                "arguments": [{"value": value}],
                "returnByValue": true,
                "awaitPromise": true,
            }),
        )
        .await
        .map_err(|e| ElementError::Action(format!("fill failed: {e}")))?;

    // Check for exception
    if let Some(exception) = result.get("exceptionDetails") {
        let text = exception
            .get("exception")
            .and_then(|ex| ex.get("description"))
            .and_then(|d| d.as_str())
            .or_else(|| exception.get("text").and_then(|t| t.as_str()))
            .unwrap_or("unknown error");
        return Err(ElementError::Action(
            text.lines().next().unwrap_or(text).trim_start_matches("Error: ").to_string(),
        ));
    }

    let payload = result.get("result").and_then(|r| r.get("value")).cloned().unwrap_or_default();
    let actual = payload.get("value").and_then(serde_json::Value::as_str).map(str::to_string);
    let max_length = payload.get("maxLength").and_then(serde_json::Value::as_i64);
    let sensitive = payload.get("sensitive").and_then(serde_json::Value::as_bool).unwrap_or(false);
    wait_for_stabilization(nav_events).await;
    Ok(FillOutcome::new(value, actual).with_max_length(max_length).secret(sensitive))
}

/// Refuse to type when nothing editable holds focus.
///
/// `Input.insertText` goes to whatever is focused. With focus on BODY it goes nowhere, and
/// the old message was built from `text.len()` — a claim about the request, never about the
/// page. Verified: `type "hello"` with nothing focused reported "Typed 5 chars" and left
/// the page untouched.
pub async fn require_editable_focus(client: &CdpClient) -> Result<(), ElementError> {
    let probe = r"(() => {
        const a = document.activeElement;
        if (!a || a === document.body || a === document.documentElement) return 'none';
        const tag = a.tagName;
        if (tag === 'INPUT' || tag === 'TEXTAREA' || a.isContentEditable) return 'ok';
        return tag.toLowerCase();
    })()";
    let result: serde_json::Value = client
        .call("Runtime.evaluate", json!({"expression": probe, "returnByValue": true}))
        .await
        .map_err(|e| ElementError::Action(format!("focus check failed: {e}")))?;
    let state = result
        .get("result")
        .and_then(|r| r.get("value"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("none");
    match state {
        "ok" => Ok(()),
        "none" => Err(ElementError::Action(
            "Nothing editable has focus, so there is nowhere to type. Focus a field first: \
             click its uid, or use `fill --selector` to set a value directly."
                .into(),
        )),
        other => Err(ElementError::Action(format!(
            "Focus is on a <{other}>, which does not accept typing. Focus an input, a \
             textarea or a contenteditable element first."
        ))),
    }
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
        "Home" => (36, None),
        "End" => (35, None),
        "PageUp" => (33, None),
        "PageDown" => (34, None),
        "Insert" => (45, None),
        "F1" => (112, None),
        "F2" => (113, None),
        "F3" => (114, None),
        "F4" => (115, None),
        "F5" => (116, None),
        "F6" => (117, None),
        "F7" => (118, None),
        "F8" => (119, None),
        "F9" => (120, None),
        "F10" => (121, None),
        "F11" => (122, None),
        "F12" => (123, None),
        // A single printable character types itself. Without `text` the page sees a keydown
        // and nothing is inserted, so `press a` reported success and typed nothing.
        _ if key.chars().count() == 1 => {
            let ch = key.chars().next().unwrap_or(' ');
            // Only alphanumerics have a virtual key code equal to their uppercase ASCII
            // byte. Deriving one for punctuation lands on an editing or navigation key:
            // '.' is 46, which is VK_DELETE, so `press .` deleted a character and reported
            // success. Send 0 and let Chrome insert from `text` alone.
            let vk = if ch.is_ascii_alphanumeric() {
                u32::from(ch.to_ascii_uppercase() as u8)
            } else {
                0
            };
            (vk, Some(key))
        }
        // Anything else would go out with virtual key code 0, which no handler reads as a
        // key. Saying so beats reporting success for an event that means nothing.
        other => {
            return Err(ElementError::Action(format!(
                "Unknown key '{other}'. Use a single character, or one of: Enter, Tab, Escape, \
                 Backspace, Delete, Space, Home, End, PageUp, PageDown, Insert, \
                 ArrowUp/Down/Left/Right, F1-F12."
            )));
        }
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
///
/// No hit test: `--xy` names no element, so there is nothing for a receiver to differ from.
/// Only "received by X" could be reported, never an interception.
pub async fn click_at_coords(
    client: &CdpClient,
    x: f64,
    y: f64,
) -> Result<(), ElementError> {
    dispatch_click_at(client, x, y).await
}

// Selector-based actions (click/dblclick/fill/focus) live in `element_selector`
// to keep this file under the 1000-line module cap; re-exported here so callers
// keep using `crate::element::*`.
pub use crate::element_selector::{
    click_selector, dblclick_selector, fill_selector, focus_selector,
};
// Split out for the 1000-line file cap; callers keep using `element::*`.
pub use crate::element_controls::{
    drag, select_option, select_option_selector, set_checked, set_checked_selector, CheckOutcome,
    SelectOutcome, set_file_input, set_file_input_selector,
};

// ---------------------------------------------------------------------------
// Double-click
// ---------------------------------------------------------------------------

/// Double-click an element by uid. Aimed by the same probe as `click` — a double-click that
/// lands on a scrim is the same false success twice over.
pub async fn dblclick(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    uid: &str,
    on_intercept: crate::hit_test::OnIntercept,
) -> Result<crate::hit_test::Dispatched, ElementError> {
    let resolved = resolve_uid(client, uid_map, uid).await?;
    if resolved.center.is_none() {
        js_dblclick(client, &resolved.object_id).await?;
        return Ok(crate::hit_test::Dispatched::js().named(Some(uid.to_string()), None, None));
    }
    let outcome = dblclick_handle(
        client,
        &resolved.object_id,
        resolved.center,
        on_intercept,
        &format!("uid={uid}"),
    )
    .await?;
    Ok(outcome.named(Some(uid.to_string()), None, None))
}

/// Aim at a resolved handle and double-click it. Mirrors `click_handle`.
pub async fn dblclick_handle(
    client: &CdpClient,
    object_id: &str,
    fallback_center: Option<(f64, f64)>,
    on_intercept: crate::hit_test::OnIntercept,
    target: &str,
) -> Result<crate::hit_test::Dispatched, ElementError> {
    use crate::hit_test::{Aim, Dispatched};
    use crate::verdict::Delivery;

    let (point, delivery, receiver) = match crate::hit_test::aim(client, object_id).await {
        Aim::NoBox => {
            js_dblclick(client, object_id).await?;
            return Ok(Dispatched::js());
        }
        Aim::Unprobed => {
            let Some(center) = fallback_center else {
                js_dblclick(client, object_id).await?;
                return Ok(Dispatched::js());
            };
            (center, Delivery::NotProbed, None)
        }
        Aim::At { point, delivery, receiver } => (point, delivery, receiver),
    };

    if matches!(delivery, Delivery::NotSettled | Delivery::OffTarget) {
        return Ok(Dispatched::skipped(delivery, point, None));
    }
    if delivery == Delivery::Intercepted && on_intercept == crate::hit_test::OnIntercept::Refuse {
        let refused = Dispatched::skipped(delivery, point, receiver);
        return Err(ElementError::NotInteractable(
            refused
                .refusal_message("double-click", target)
                .unwrap_or_else(|| format!("Refused to double-click {target}")),
        ));
    }

    dblclick_at_coords(client, point.0, point.1).await?;
    Ok(Dispatched::landed(delivery, point, receiver))
}

pub async fn js_dblclick(client: &CdpClient, object_id: &str) -> Result<(), ElementError> {
    let nav_events = client.events();
    client.mark_dispatch();
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
    client.mark_dispatch();
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
pub fn check_js_exception(result: &serde_json::Value) -> Result<(), ElementError> {
    if let Some(exception) = result.get("exceptionDetails") {
        let text = exception
            .get("exception")
            .and_then(|ex| ex.get("description"))
            .and_then(|d| d.as_str())
            .or_else(|| exception.get("text").and_then(|t| t.as_str()))
            .unwrap_or("unknown error");
        return Err(ElementError::Action(
            text.lines().next().unwrap_or(text).trim_start_matches("Error: ").to_string(),
        ));
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

    #[tokio::test]
    async fn recv_event_times_out_without_match() {
        let (tx, _) = broadcast::channel::<CdpEvent>(16);
        let mut rx = tx.subscribe();
        tx.send(ev("Runtime.consoleAPICalled")).unwrap();
        // Only an unrelated event → probe returns false quickly (no navigation).
        assert!(!recv_event(&mut rx, "Page.frameNavigated", Duration::from_millis(20)).await);
    }
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
}
