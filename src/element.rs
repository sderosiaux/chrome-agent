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
    .await
    .map_err(|e| e.naming(Some(uid.to_string()), None, None))?;
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
        // `unaimable` is what separates a box a pointer cannot reach from a box the page is
        // holding off screen. Same verdict, same absence of a dispatch, different recovery.
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

/// The error a refused pointer action returns, carrying what the probe measured.
///
/// One place for both verbs: a refusal that names the receiver on `click` and not on
/// `dblclick` is the same class of asymmetry the verdict module exists to remove.
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
    // A pointer event on a page that is not the foreground tab is answered on a fixed
    // five-second timer; on the foreground tab, in single-digit milliseconds. Once per
    // connection, 3 ms — see `CdpClient::ensure_foreground` for the measurements.
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
    wait_for_stabilization(client, nav_events).await;
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

    wait_for_stabilization(client, nav_events).await;
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

    wait_for_stabilization(client, nav_events).await;
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

    // A pointer event on a page that is not the foreground tab is answered on a fixed
    // five-second timer; on the foreground tab, in single-digit milliseconds. Once per
    // connection, 3 ms — see `CdpClient::ensure_foreground` for the measurements.
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

/// Wait (≤`timeout`) for one event satisfying `matches` on an already-open subscription.
/// `true` if it arrived. Lagged: keep going, the event may follow.
///
/// A predicate rather than a method name, because the name alone was not enough to tell the
/// two navigations apart — see [`main_frame_navigated`].
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

/// A `Page.frameNavigated` for the TOP frame, which is the only one whose load we can wait for.
///
/// `Page.loadEventFired` is a main-frame event: it carries a timestamp and nothing else, and it
/// fires once per top-level document. A subframe navigating produces `Page.frameNavigated` with
/// a `parentId` and, later, `Page.frameStoppedLoading` — never a load event. So a wait armed by
/// a subframe's navigation is a wait for something that cannot arrive, and it ran to the full
/// ceiling every time.
///
/// That is not a corner case. Measured on shop.app: clicking a product tile appends a tracking
/// iframe, which navigates to `about:blank` and then to `chrome-error://chromewebdata/` — two
/// `frameNavigated` events for a SUBFRAME, 4 ms apart, no load event ever. The click took
/// 10.10 s, of which 10.05 s was this wait; a click at an inert coordinate on the same page in
/// the same session took 0.14 s. `tests/fixtures/click_spawns_subframe.html` is that shape,
/// offline.
fn main_frame_navigated(event: &CdpEvent) -> bool {
    event.method == "Page.frameNavigated"
        && event
            .params
            .get("frame")
            .is_some_and(|frame| frame.get("parentId").is_none())
}

/// Wait for the page to stabilize after an action. `nav_events` MUST be
/// subscribed (`client.events()`) BEFORE dispatching the action — `broadcast`
/// only delivers post-subscribe messages, so a fast `frameNavigated`/
/// `loadEventFired` firing before we wait would be missed (the `goto` race).
/// 50ms probe for a TOP-frame navigation; only then wait (≤10s) for its load.
///
/// The probe keeps reading for its whole window rather than returning on the first
/// `frameNavigated`: a click that spawns a tracking iframe AND navigates the page produces the
/// subframe's event first, and stopping there would now skip a wait that is owed.
///
/// The wait is measured and handed to the connection, so the response can say how long it took
/// (`waited_ms`). A ten-second action that explains itself is a page being slow; a ten-second
/// action that says nothing is the tool being broken, and the caller cannot tell them apart.
pub async fn wait_for_stabilization(
    client: &CdpClient,
    nav_events: broadcast::Receiver<CdpEvent>,
) {
    // Boxed: this future holds a subscription and two nested timeouts, and it is awaited from
    // thirteen action paths that `run::run` holds live in one match arm. Inline, it is counted
    // thirteen times in that frame.
    if let Some(waited) = Box::pin(settle_after_navigation(nav_events)).await {
        client.note_settle_wait(waited);
    }
}

/// The wait itself: `Some(duration)` when a top-frame navigation armed it, `None` when none
/// was seen. Split from the wrapper so the rule can be tested with a broadcast channel and no
/// Chrome — which is how the subframe case is pinned.
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
    /// `--on-intercept refuse` stopped a pointer action before it dispatched.
    ///
    /// A variant of its own, and not a `NotInteractable(String)`, because everything the
    /// caller needs to act — the receiver, its uid, the aim point, the branch — had been
    /// measured and was being thrown away at the point where the message was formatted. It
    /// rides through the error channel the way `commands::assert::NotHeld` carries exit 2, and
    /// the three error boundaries unpack it with `hit_test::refusal_in`.
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
    /// Carry the identity of the node an action resolved onto the refusal it produced.
    ///
    /// A no-op for every other variant: only a refusal has a structured response to put it on.
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
        // `unaimable` is what separates a box a pointer cannot reach from a box the page is
        // holding off screen. Same verdict, same absence of a dispatch, different recovery.
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

/// Double-click at coordinates.
pub async fn dblclick_at_coords(client: &CdpClient, x: f64, y: f64) -> Result<(), ElementError> {
    // A pointer event on a page that is not the foreground tab is answered on a fixed
    // five-second timer; on the foreground tab, in single-digit milliseconds. Once per
    // connection, 3 ms — see `CdpClient::ensure_foreground` for the measurements.
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
        CdpEvent { method: method.to_string(), params: serde_json::Value::Null }
    }

    /// A `Page.frameNavigated` as Chrome sends it: the top frame carries no `parentId`, a
    /// subframe does.
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
        // Only an unrelated event → probe returns false quickly (no navigation).
        assert!(!recv_event(&mut rx, "Page.frameNavigated", Duration::from_millis(20)).await);
    }
    #[tokio::test]
    async fn stabilization_sees_navigation_buffered_before_wait() {
        let (tx, _) = broadcast::channel::<CdpEvent>(16);
        let rx = tx.subscribe(); // subscribe first (pre-action)
        tx.send(navigated(None)).unwrap();
        tx.send(ev("Page.loadEventFired")).unwrap();
        // Both events already buffered → completes promptly, does not hang.
        let waited = tokio::time::timeout(Duration::from_secs(1), settle_after_navigation(rx))
            .await
            .expect("should not hang when nav events are already buffered");
        assert!(waited.is_some(), "a top-frame navigation arms the wait");
    }

    /// The ten seconds. A subframe navigating is not something `Page.loadEventFired` will ever
    /// answer for, so arming the wait on it meant waiting the full ceiling on every page that
    /// spawns a tracking iframe when clicked — 10.10 s on shop.app, measured.
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

    /// And the case that makes the probe read its whole window instead of returning on the
    /// first event: a click that spawns a tracker AND navigates. The subframe's event comes
    /// first; the wait is still owed to the top frame's.
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
        // A frameNavigated whose shape we cannot read is not a top-frame claim either way;
        // `params` absent means no `frame`, and the predicate must not panic on it.
        assert!(!main_frame_navigated(&ev("Page.frameNavigated")));
        assert!(!main_frame_navigated(&ev("Page.loadEventFired")));
    }
}
