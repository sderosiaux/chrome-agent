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

/// Click an element by uid. `hit_test::aim` scrolls the element into view, measures where the click would go and says what
/// sits there, in one round trip. A click delivered elsewhere reports `intercepted`; an aim point
/// still moving under a smooth scroll is refused. No layout box falls back to a JS `.click()`.
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
    .await
    .map_err(|e| e.naming(Some(uid.to_string()), None, None))?;
    Ok(outcome.named(Some(uid.to_string()), None, None))
}

/// Aim at a resolved handle and single-click it. Shared by the uid and selector paths.
/// `fallback_center` is the box model's centre, used only when the probe could not run.
pub async fn click_handle(
    client: &CdpClient,
    object_id: &str,
    fallback_center: Option<(f64, f64)>,
    on_intercept: crate::hit_test::OnIntercept,
    target: &str,
) -> Result<crate::hit_test::Dispatched, ElementError> {
    use crate::hit_test::{Aim, Dispatched};
    use crate::verdict::Delivery;

    let (point, delivery, receiver, unaimable) = match crate::hit_test::aim(client, object_id).await {
        Aim::NoBox => {
            js_click(client, object_id).await?;
            return Ok(Dispatched::js());
        }
        Aim::Unprobed => {
            let Some(center) = fallback_center else {
                js_click(client, object_id).await?;
                return Ok(Dispatched::js());
            };
            (center, Delivery::NotProbed, None, None)
        }
        Aim::At { point, delivery, receiver, unaimable } => (point, delivery, receiver, unaimable),
    };

    if matches!(delivery, Delivery::NotSettled | Delivery::OffTarget) {
        // `unaimable` separates a box a pointer cannot reach from a box the page holds off
        // screen: same verdict, same absence of a dispatch, different recovery.
        return Ok(Dispatched::skipped(delivery, point, None).unaimed(unaimable));
    }
    if delivery == Delivery::Intercepted
        && crate::hit_test::should_refuse_intercept(on_intercept, receiver.as_ref())
    {
        return Err(refusal(
            Dispatched::skipped(delivery, point, receiver).under(on_intercept),
            "click",
            target,
        ));
    }

    dispatch_click_at(client, point.0, point.1).await?;
    Ok(Dispatched::landed(delivery, point, receiver))
}

/// The error a refused pointer action returns, carrying what the probe measured. One place for
/// both verbs, so `click` and `dblclick` refuse in the same words.
fn refusal(
    dispatched: crate::hit_test::Dispatched,
    verb: &str,
    target: &str,
) -> ElementError {
    let message = dispatched
        .refusal_message(verb, target)
        .unwrap_or_else(|| format!("Refused to {verb} {target}"));
    ElementError::Refused(Box::new(crate::hit_test::Refused::new(message, dispatched)))
}

/// A click at coordinates somebody else decided on: mouse normally, touch under emulation.
async fn dispatch_click_at(client: &CdpClient, cx: f64, cy: f64) -> Result<(), ElementError> {
    // A background tab answers a pointer event on a fixed five-second timer, the foreground tab
    // in single-digit ms. Costs 3 ms, once per connection (`CdpClient::ensure_foreground`).
    client.ensure_foreground().await;
    // Subscribe BEFORE dispatching so a fast navigation isn't missed.
    let nav_events = client.events();
    client.mark_dispatch();
    if client.touch_emulation_enabled() {
        client
            .send_input(
                "Input.dispatchTouchEvent",
                json!({
                    "type": "touchStart",
                    "touchPoints": [{"x": cx, "y": cy, "id": 0}],
                }),
            )
            .await
            .map_err(|e| ElementError::Action(format!("touchStart failed: {e}")))?;
        client
            .send_input(
                "Input.dispatchTouchEvent",
                json!({"type": "touchEnd", "touchPoints": []}),
            )
            .await
            .map_err(|e| ElementError::Action(format!("touchEnd failed: {e}")))?;
    } else {
        client
            .send_input("Input.dispatchMouseEvent", DispatchMouseEventParams {
                event_type: MouseEventType::MousePressed,
                x: cx, y: cy,
                button: Some(MouseButton::Left), buttons: Some(1), click_count: Some(1),
                modifiers: None, timestamp: None, delta_x: None, delta_y: None,
                pointer_type: Some("mouse".into()),
            })
            .await
            .map_err(|e| ElementError::Action(format!("mousePressed failed: {e}")))?;

        client
            .send_input("Input.dispatchMouseEvent", DispatchMouseEventParams {
                event_type: MouseEventType::MouseReleased,
                x: cx, y: cy,
                button: Some(MouseButton::Left), buttons: Some(0), click_count: Some(1),
                modifiers: None, timestamp: None, delta_x: None, delta_y: None,
                pointer_type: Some("mouse".into()),
            })
            .await
            .map_err(|e| ElementError::Action(format!("mouseReleased failed: {e}")))?;
    }

    wait_for_stabilization(client, nav_events).await;
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

    wait_for_stabilization(client, nav_events).await;
    Ok(())
}

/// What the page holds after a fill, next to what was asked for. A write is a request: masks reformat, `maxlength` truncates, controlled components rewrite,
/// number inputs discard what they cannot parse. The value landed, just not verbatim.
pub struct FillOutcome {
    pub requested: String,
    pub actual: Option<String>,
    /// The field holds a secret, so neither value may be reported; verbatim-ness and length are.
    pub sensitive: bool,
    /// Set when the landed value could not have been typed: `maxlength` constrains the editing
    /// pipeline, not the value setter, so the form will reject it on submit.
    pub caveat: Option<String>,
    /// How long after the write the value was read; "the field holds X" is only true of a moment.
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
/// Shared by `fill` (uid and selector), `assert value` and the `values_lost` report, because it
/// gates what reaches stdout, the transcript and any `--record` file.
/// Chrome masks `type=password` in the accessibility tree; the `autocomplete` half it does not,
/// so a one-time code or card number in a `type=text` field is redacted only by this.
pub const SECRET_FIELD: &str =
    r"(el.type === 'password' || /password|cc-number|cc-csc|one-time-code/i.test(el.autocomplete || ''))";

/// How long every read-back waits before looking at what the page kept.
/// 60 ms catches a revert on the microtask queue, in `setTimeout(0)` or in an animation frame,
/// and NOT a validator firing at 400 ms — no fixed window could. Hence read-backs report
/// `observed_after_ms` rather than asserting persistence. Raising it costs every fill and check.
pub const READ_BACK_MS: u64 = 60;

pub async fn fill(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    uid: &str,
    value: &str,
) -> Result<FillOutcome, ElementError> {
    let resolved = resolve_uid(client, uid_map, uid).await?;

    // The native value setter, so React's synthetic onChange fires: React intercepts direct
    // assignment but not the setter reached through Object.getOwnPropertyDescriptor.
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
            // Read after the window, not on the next line: a controlled component reverting in
            // a promise callback has not run yet when the write returns.
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
    wait_for_stabilization(client, nav_events).await;
    Ok(FillOutcome::new(value, actual).with_max_length(max_length).secret(sensitive))
}

/// Refuse to type when nothing editable holds focus. `Input.insertText` goes to whatever is focused; with focus on BODY it goes nowhere, and a
/// character count is a claim about the request, never about the page.
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

pub async fn type_text(
    client: &CdpClient,
    text: &str,
) -> Result<(), ElementError> {
    let nav_events = client.events();
    // An input event, so: the input-event deadline rather than `--timeout`, and the dispatch mark
    // that both starts the observation window and clears the previous action's settle wait.
    client.mark_dispatch();
    client
        .send_input("Input.insertText", json!({ "text": text }))
        .await
        .map_err(|e| ElementError::Action(format!("insertText failed: {e}")))?;

    wait_for_stabilization(client, nav_events).await;
    Ok(())
}

pub async fn press_key(
    client: &CdpClient,
    key: &str,
) -> Result<(), ElementError> {
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
        // A single printable character needs `text`, or nothing is inserted.
        _ if key.chars().count() == 1 => {
            let ch = key.chars().next().unwrap_or(' ');
            // Only alphanumerics have a virtual key code equal to their uppercase ASCII byte;
            // deriving one for punctuation lands on an editing key ('.' is 46, VK_DELETE).
            let vk = if ch.is_ascii_alphanumeric() {
                u32::from(ch.to_ascii_uppercase() as u8)
            } else {
                0
            };
            (vk, Some(key))
        }
        // Anything else would go out with virtual key code 0, which no handler reads as a key.
        other => {
            return Err(ElementError::Action(format!(
                "Unknown key '{other}'. Use a single character, or one of: Enter, Tab, Escape, \
                 Backspace, Delete, Space, Home, End, PageUp, PageDown, Insert, \
                 ArrowUp/Down/Left/Right, F1-F12."
            )));
        }
    };

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
    // Same as `type_text`: a keyboard event is an input event, deadline and mark included.
    client.mark_dispatch();
    client
        .send_input("Input.dispatchKeyEvent", key_down)
        .await
        .map_err(|e| ElementError::Action(format!("keyDown failed: {e}")))?;

    client
        .send_input(
            "Input.dispatchKeyEvent",
            json!({
                "type": "keyUp",
                "key": key,
            }),
        )
        .await
        .map_err(|e| ElementError::Action(format!("keyUp failed: {e}")))?;

    wait_for_stabilization(client, nav_events).await;
    Ok(())
}

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

    // A background tab answers a pointer event on a fixed five-second timer, the foreground tab
    // in single-digit ms. Costs 3 ms, once per connection (`CdpClient::ensure_foreground`).
    client.ensure_foreground().await;
    client
        .send_input("Input.dispatchMouseEvent", DispatchMouseEventParams {
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

/// Wait (≤`timeout`) for one event satisfying `matches` on an already-open subscription. `true`
/// if it arrived; on Lagged, keep going — the event may follow. A predicate rather than a method
/// name, because the name alone cannot tell the two navigations apart ([`main_frame_navigated`]).
async fn recv_event_where(
    rx: &mut broadcast::Receiver<CdpEvent>,
    matches: impl Fn(&CdpEvent) -> bool + Send + Sync,
    timeout: Duration,
) -> bool {
    tokio::time::timeout(timeout, async {
        loop {
            match rx.recv().await {
                Ok(event) if matches(&event) => return true,
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return false,
            }
        }
    })
    .await
    .unwrap_or(false)
}

/// Wait (≤`timeout`) for one event of this exact method.
async fn recv_event(rx: &mut broadcast::Receiver<CdpEvent>, method: &str, timeout: Duration) -> bool {
    recv_event_where(rx, |event| event.method == method, timeout).await
}

/// A `Page.frameNavigated` for the TOP frame, the only one whose load can be waited for.
/// `Page.loadEventFired` fires once per top-level document; a subframe gets `frameNavigated` and
/// `frameStoppedLoading` but never a load event, so a wait armed by one runs the full 10 s
/// ceiling (measured 10.10 s on a click appending a tracking iframe, 0.14 s for an inert one).
fn main_frame_navigated(event: &CdpEvent) -> bool {
    event.method == "Page.frameNavigated"
        && event
            .params
            .get("frame")
            .is_some_and(|frame| frame.get("parentId").is_none())
}

/// Wait for the page to stabilize: a 50 ms probe for a TOP-frame navigation, then ≤10 s for its
/// load. The wait is handed to the connection so the response can report `waited_ms`.
/// `nav_events` MUST be subscribed (`client.events()`) BEFORE dispatching — `broadcast` only
/// delivers post-subscribe messages. The probe reads its whole window rather than stopping at
/// the first event: a click that spawns a tracker AND navigates emits the subframe's first.
pub async fn wait_for_stabilization(
    client: &CdpClient,
    nav_events: broadcast::Receiver<CdpEvent>,
) {
    // Boxed: thirteen action paths await this inside one `run::run` match arm, where inline its
    // subscription and two nested timeouts would be counted thirteen times over.
    if let Some(waited) = Box::pin(settle_after_navigation(nav_events)).await {
        client.note_settle_wait(waited);
    }
}

/// The wait itself: `Some(duration)` when a top-frame navigation armed it, `None` otherwise.
/// Split from the wrapper so it can be tested with a broadcast channel and no Chrome.
async fn settle_after_navigation(
    mut nav_events: broadcast::Receiver<CdpEvent>,
) -> Option<Duration> {
    if !recv_event_where(&mut nav_events, main_frame_navigated, Duration::from_millis(50)).await {
        return None;
    }
    let started = std::time::Instant::now();
    let _ = recv_event(&mut nav_events, "Page.loadEventFired", Duration::from_secs(10)).await;
    Some(started.elapsed())
}

#[derive(Debug, thiserror::Error)]
pub enum ElementError {
    /// `--on-intercept` stopped a pointer action before it dispatched. A variant of its own, so
    /// the receiver, its uid, the aim point and the branch survive instead of being flattened
    /// into a message; the error boundaries unpack it with `hit_test::refusal_in`.
    #[error("{0}")]
    Refused(Box<crate::hit_test::Refused>),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Detached(String),
    #[error("{0}")]
    NotInteractable(String),
    #[error("{0}")]
    Action(String),
}

impl ElementError {
    /// Carry the identity of the node an action resolved onto the refusal it produced. A no-op
    /// for every other variant: only a refusal has a structured response to put it on.
    #[must_use]
    pub fn naming(
        self,
        uid: Option<String>,
        role: Option<String>,
        name: Option<String>,
    ) -> Self {
        match self {
            Self::Refused(refused) => Self::Refused(Box::new(refused.naming(uid, role, name))),
            other => other,
        }
    }
}

/// Click at explicit (x, y) coordinates. No hit test: `--xy` names no element, so there is
/// nothing for a receiver to differ from.
pub async fn click_at_coords(
    client: &CdpClient,
    x: f64,
    y: f64,
) -> Result<(), ElementError> {
    dispatch_click_at(client, x, y).await
}

// Selector-based actions live in `element_selector`, re-exported so callers keep using
// `crate::element::*`.
pub use crate::element_selector::{
    click_selector, dblclick_selector, fill_selector, focus_selector,
};
pub use crate::element_controls::{
    drag, select_option, select_option_selector, set_checked, set_checked_selector, CheckOutcome,
    SelectOutcome, set_file_input, set_file_input_selector,
};

/// Double-click an element by uid. Aimed by the same probe as `click`.
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
    .await
    .map_err(|e| e.naming(Some(uid.to_string()), None, None))?;
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

    let (point, delivery, receiver, unaimable) = match crate::hit_test::aim(client, object_id).await {
        Aim::NoBox => {
            js_dblclick(client, object_id).await?;
            return Ok(Dispatched::js());
        }
        Aim::Unprobed => {
            let Some(center) = fallback_center else {
                js_dblclick(client, object_id).await?;
                return Ok(Dispatched::js());
            };
            (center, Delivery::NotProbed, None, None)
        }
        Aim::At { point, delivery, receiver, unaimable } => (point, delivery, receiver, unaimable),
    };

    if matches!(delivery, Delivery::NotSettled | Delivery::OffTarget) {
        // `unaimable` separates a box a pointer cannot reach from a box the page holds off
        // screen: same verdict, same absence of a dispatch, different recovery.
        return Ok(Dispatched::skipped(delivery, point, None).unaimed(unaimable));
    }
    if delivery == Delivery::Intercepted
        && crate::hit_test::should_refuse_intercept(on_intercept, receiver.as_ref())
    {
        return Err(refusal(
            Dispatched::skipped(delivery, point, receiver).under(on_intercept),
            "double-click",
            target,
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

    wait_for_stabilization(client, nav_events).await;
    Ok(())
}

pub async fn dblclick_at_coords(client: &CdpClient, x: f64, y: f64) -> Result<(), ElementError> {
    // A background tab answers a pointer event on a fixed five-second timer, the foreground tab
    // in single-digit ms. Costs 3 ms, once per connection (`CdpClient::ensure_foreground`).
    client.ensure_foreground().await;
    let nav_events = client.events();
    client.mark_dispatch();
    for click_count in [1, 2] {
        client
            .send_input("Input.dispatchMouseEvent", DispatchMouseEventParams {
                event_type: MouseEventType::MousePressed, x, y,
                button: Some(MouseButton::Left), buttons: Some(1),
                click_count: Some(click_count),
                modifiers: None, timestamp: None, delta_x: None, delta_y: None,
                pointer_type: Some("mouse".into()),
            })
            .await
            .map_err(|e| ElementError::Action(format!("mousePressed failed: {e}")))?;

        client
            .send_input("Input.dispatchMouseEvent", DispatchMouseEventParams {
                event_type: MouseEventType::MouseReleased, x, y,
                button: Some(MouseButton::Left), buttons: Some(0),
                click_count: Some(click_count),
                modifiers: None, timestamp: None, delta_x: None, delta_y: None,
                pointer_type: Some("mouse".into()),
            })
            .await
            .map_err(|e| ElementError::Action(format!("mouseReleased failed: {e}")))?;
    }
    wait_for_stabilization(client, nav_events).await;
    Ok(())
}

/// Turn an `exceptionDetails` on a CDP result into an `ElementError`, keeping the first line of
/// the thrown text and dropping the stack.
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
        CdpEvent { method: method.to_string(), params: serde_json::Value::Null }
    }

    /// As Chrome sends it: the top frame carries no `parentId`, a subframe does.
    fn navigated(parent: Option<&str>) -> CdpEvent {
        let frame = match parent {
            Some(id) => serde_json::json!({"id": "F1", "parentId": id, "url": "about:blank"}),
            None => serde_json::json!({"id": "F0", "url": "https://example.com/"}),
        };
        CdpEvent {
            method: "Page.frameNavigated".to_string(),
            params: serde_json::json!({"frame": frame}),
        }
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
        // An unrelated event only → returns false quickly.
        assert!(!recv_event(&mut rx, "Page.frameNavigated", Duration::from_millis(20)).await);
    }
    #[tokio::test]
    async fn stabilization_sees_navigation_buffered_before_wait() {
        let (tx, _) = broadcast::channel::<CdpEvent>(16);
        let rx = tx.subscribe(); // pre-action subscription
        tx.send(navigated(None)).unwrap();
        tx.send(ev("Page.loadEventFired")).unwrap();
        let waited = tokio::time::timeout(Duration::from_secs(1), settle_after_navigation(rx))
            .await
            .expect("should not hang when nav events are already buffered");
        assert!(waited.is_some(), "a top-frame navigation arms the wait");
    }

    /// `Page.loadEventFired` never answers for a subframe, so arming the wait on one costs the
    /// full 10 s ceiling.
    #[tokio::test]
    async fn a_subframe_navigation_arms_no_wait() {
        let (tx, _) = broadcast::channel::<CdpEvent>(16);
        let rx = tx.subscribe();
        tx.send(navigated(Some("F0"))).unwrap();
        tx.send(navigated(Some("F0"))).unwrap();
        let waited = tokio::time::timeout(Duration::from_secs(2), settle_after_navigation(rx))
            .await
            .expect("must not wait for a load event that cannot come");
        assert_eq!(waited, None, "nothing was waited for, and nothing is reported");
    }

    /// Why the probe reads its whole window: a click that spawns a tracker AND navigates emits
    /// the subframe's event first, and the top frame's wait is still owed.
    #[tokio::test]
    async fn a_subframe_event_does_not_hide_the_navigation_behind_it() {
        let (tx, _) = broadcast::channel::<CdpEvent>(16);
        let rx = tx.subscribe();
        tx.send(navigated(Some("F0"))).unwrap();
        tx.send(navigated(None)).unwrap();
        tx.send(ev("Page.loadEventFired")).unwrap();
        let waited = tokio::time::timeout(Duration::from_secs(1), settle_after_navigation(rx))
            .await
            .expect("should not hang");
        assert!(waited.is_some(), "the top-frame navigation still arms the wait");
    }

    #[test]
    fn only_the_top_frame_is_a_navigation_we_can_wait_for() {
        assert!(main_frame_navigated(&navigated(None)));
        assert!(!main_frame_navigated(&navigated(Some("F0"))));
        // An unreadable frameNavigated is not a top-frame claim, and must not panic.
        assert!(!main_frame_navigated(&ev("Page.frameNavigated")));
        assert!(!main_frame_navigated(&ev("Page.loadEventFired")));
    }
}
