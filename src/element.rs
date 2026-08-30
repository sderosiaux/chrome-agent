use std::collections::HashMap;
use std::time::Duration;

use serde_json::json;
use tokio::sync::broadcast;

use crate::cdp::client::CdpClient;
use crate::cdp::types::{CdpEvent, GetBoxModelResult, ResolveNodeParams, ResolveNodeResult};
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

/// How long every read-back waits before looking at what the page kept.
/// 60 ms catches a revert on the microtask queue, in `setTimeout(0)` or in an animation frame,
/// and NOT a validator firing at 400 ms — no fixed window could. Hence read-backs report
/// `observed_after_ms` rather than asserting persistence. Raising it costs every fill and check.
pub const READ_BACK_MS: u64 = 60;

// The predicate lives with the reports it redacts (`read_back`), re-exported so every call site
// keeps saying `element::SECRET_FIELD`. Split out for the 1000-line cap.
pub use crate::read_back::SECRET_FIELD;

/// Fill by uid, reading secrecy off the element. See [`fill_with`] for the caller that asserts it.
pub async fn fill(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    uid: &str,
    value: &str,
) -> Result<FillOutcome, ElementError> {
    fill_with(client, uid_map, uid, value, false).await
}

/// Fill by uid. `asserted_secret` is the CALLER's claim about the value, on top of what
/// [`SECRET_FIELD`] read off the element: it only ever ADDS redaction, because an override that
/// turned redaction off would be a way to print a password.
///
/// `false` — infer from the DOM — is what every front end passes today. The flag that would set
/// it is a follow-up in `src/cli.rs`.
pub async fn fill_with(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    uid: &str,
    value: &str,
    asserted_secret: bool,
) -> Result<FillOutcome, ElementError> {
    let resolved = resolve_uid(client, uid_map, uid).await?;
    fill_object_with(client, &resolved.object_id, value, asserted_secret).await
}

/// Fill one already-resolved object. Selector callers use this so identity, write and read-back
/// cannot resolve different nodes between CDP round trips.
pub async fn fill_object_with(
    client: &CdpClient,
    object_id: &str,
    value: &str,
    asserted_secret: bool,
) -> Result<FillOutcome, ElementError> {
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
                "objectId": object_id,
                "functionDeclaration": js,
                "arguments": [{"value": value}],
                "returnByValue": true,
                "awaitPromise": true,
            }),
        )
        .await
        .map_err(|e| ElementError::Action(format!("fill failed: {e}")))?;

    check_js_exception(&result)?;

    let payload = result
        .get("result")
        .and_then(|r| r.get("value"))
        .cloned()
        .unwrap_or_default();
    let actual = payload
        .get("value")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let max_length = payload.get("maxLength").and_then(serde_json::Value::as_i64);
    let sensitive = payload
        .get("sensitive")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    wait_for_stabilization(client, nav_events).await;
    Ok(FillOutcome::new(value, actual)
        .with_max_length(max_length)
        .secret(asserted_secret || sensitive))
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
        .call(
            "Runtime.evaluate",
            json!({"expression": probe, "returnByValue": true}),
        )
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

/// Whether the element holding focus is one whose value must never be printed. `false` on any
/// failure to read: nothing was typed yet, so this is a question about the page, not a refusal.
async fn focused_is_secret(client: &CdpClient) -> bool {
    let probe = format!(
        "(() => {{ const el = document.activeElement; if (!el) return false; return !!{SECRET_FIELD}; }})()"
    );
    let Ok(result) = client
        .call::<_, serde_json::Value>(
            "Runtime.evaluate",
            json!({"expression": probe, "returnByValue": true}),
        )
        .await
    else {
        return false;
    };
    result
        .get("result")
        .and_then(|r| r.get("value"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// Type into whatever holds focus, and return the message the response should carry.
///
/// `type` has no read-back — `Input.insertText` goes wherever focus is and the tool never learns
/// what the page kept — so the message is the whole report, and it is built HERE rather than at
/// the call site so the predicate and the wording cannot drift apart. A secret field names
/// neither the text (which was never echoed) nor its length: unlike `fill` there is no
/// `verbatim` to classify, so the length buys the caller nothing and only narrows the value.
///
/// The caller appends its own `into selector '…'` clause, as it already did.
pub async fn type_text_with(
    client: &CdpClient,
    text: &str,
    asserted_secret: bool,
) -> Result<String, ElementError> {
    // Before the insert, and skipped when the caller already asserted secrecy: one round trip,
    // and after typing `document.activeElement` may be somewhere else entirely.
    let sensitive = asserted_secret || focused_is_secret(client).await;
    let nav_events = client.events();
    // An input event, so: the input-event deadline rather than `--timeout`, and the dispatch mark
    // that both starts the observation window and clears the previous action's settle wait.
    client.mark_dispatch();
    client
        .send_input("Input.insertText", json!({ "text": text }))
        .await
        .map_err(|e| ElementError::Action(format!("insertText failed: {e}")))?;

    wait_for_stabilization(client, nav_events).await;
    Ok(if sensitive {
        "Typed into a secret field (length withheld)".to_string()
    } else {
        format!("Typed {} chars", text.chars().count())
    })
}

pub async fn press_key(client: &CdpClient, key: &str) -> Result<(), ElementError> {
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
    // Same as `type_text_with`: a keyboard event is an input event, deadline and mark included.
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

/// Wait (≤`timeout`) for one event satisfying `matches` on an already-open subscription. `true`
/// if it arrived. `lost_means_match` is for the short navigation probe: a dropped event cannot
/// prove navigation absent, so it conservatively arms the load wait. A predicate rather than a
/// method name, because the name alone cannot tell two navigations apart ([`main_frame_navigated`]).
async fn recv_event_where(
    rx: &mut broadcast::Receiver<CdpEvent>,
    matches: impl Fn(&CdpEvent) -> bool + Send + Sync,
    timeout: Duration,
    lost_means_match: bool,
) -> bool {
    tokio::time::timeout(timeout, async {
        loop {
            match rx.recv().await {
                Ok(event) if matches(&event) => return true,
                Err(broadcast::error::RecvError::Lagged(_)) if lost_means_match => return true,
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return false,
            }
        }
    })
    .await
    .unwrap_or(false)
}

/// Wait (≤`timeout`) for one event of this exact method.
async fn recv_event(
    rx: &mut broadcast::Receiver<CdpEvent>,
    method: &str,
    timeout: Duration,
) -> bool {
    recv_event_where(rx, |event| event.method == method, timeout, false).await
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
pub async fn wait_for_stabilization(client: &CdpClient, nav_events: broadcast::Receiver<CdpEvent>) {
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
    if !recv_event_where(
        &mut nav_events,
        main_frame_navigated,
        Duration::from_millis(50),
        true,
    )
    .await
    {
        return None;
    }
    let started = std::time::Instant::now();
    let _ = recv_event(
        &mut nav_events,
        "Page.loadEventFired",
        Duration::from_secs(10),
    )
    .await;
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
    pub fn naming(self, uid: Option<String>, role: Option<String>, name: Option<String>) -> Self {
        match self {
            Self::Refused(refused) => Self::Refused(Box::new(refused.naming(uid, role, name))),
            other => other,
        }
    }
}

// Form controls, the pointer path and the selector-based actions live in their own modules,
// re-exported so callers keep using `crate::element::*`.
pub use crate::element_controls::{
    CheckOutcome, SelectOutcome, drag, select_option, select_option_handle, set_checked,
    set_checked_handle, set_file_input, set_file_input_handle,
};
pub use crate::element_pointer::{
    PointerVerb, aim_and_dispatch, click, click_at_coords, dblclick, dblclick_at_coords, hover,
};
pub use crate::element_selector::{
    click_selector, dblclick_selector, fill_selector_handle, fill_selector_with, focus_selector,
};

/// What a `Runtime` reply says was thrown, if anything. A throw is not a transport failure: CDP
/// answers `Ok` and puts the exception in `exceptionDetails`, so every caller has to look. This is
/// the one place that knows where to look — `exception.description` first, `text` only as the
/// fallback, since the latter is Chrome's wrapper ("Uncaught Error: …") rather than the message —
/// and what to keep: the thrown text arrives as `Error: message\n    at <anonymous>:3:19`, and the
/// stack is noise.
///
/// Returns the message rather than an error, because the variant is the caller's call: the same
/// throw is an `Action` failure to `fill` and a `NotFound` to `focus`.
#[must_use]
pub fn js_exception(result: &serde_json::Value) -> Option<String> {
    let exception = result.get("exceptionDetails")?;
    let text = exception
        .get("exception")
        .and_then(|ex| ex.get("description"))
        .and_then(|d| d.as_str())
        .or_else(|| exception.get("text").and_then(|t| t.as_str()))
        .unwrap_or("unknown error");
    Some(
        text.lines()
            .next()
            .unwrap_or(text)
            .trim_start_matches("Error: ")
            .to_string(),
    )
}

/// [`js_exception`] for the majority of callers, to whom a throw is a failed action.
pub fn check_js_exception(result: &serde_json::Value) -> Result<(), ElementError> {
    js_exception(result).map_or(Ok(()), |thrown| Err(ElementError::Action(thrown)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(method: &str) -> CdpEvent {
        CdpEvent {
            method: method.to_string(),
            params: serde_json::Value::Null,
        }
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

    /// The thrown message, not Chrome's wrapper around it, and not the stack under it. Five
    /// call sites each re-derived this; the ones reading `text` first answered
    /// `Uncaught Error: boom` where the description says `boom`.
    #[test]
    fn the_description_outranks_the_wrapper_and_the_stack_is_dropped() {
        let val = serde_json::json!({"exceptionDetails": {
            "text": "Uncaught Error: boom",
            "exception": {"description": "Error: boom\n    at <anonymous>:3:19"},
        }});
        assert_eq!(js_exception(&val).as_deref(), Some("boom"));
    }

    /// A `DOMException`'s description carries the error's own class, which must survive: a
    /// `SyntaxError` is what tells a caller the SELECTOR was the problem.
    #[test]
    fn a_non_error_class_keeps_its_name() {
        let val = serde_json::json!({"exceptionDetails": {
            "exception": {"description": "SyntaxError: '[' is not a valid selector.\n    at x"},
        }});
        assert_eq!(
            js_exception(&val).as_deref(),
            Some("SyntaxError: '[' is not a valid selector.")
        );
    }

    /// No throw, no message — the ordinary path costs one `get`.
    #[test]
    fn a_clean_reply_carries_no_exception() {
        assert!(js_exception(&serde_json::json!({"result": {"value": 1}})).is_none());
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
        assert_eq!(
            waited, None,
            "nothing was waited for, and nothing is reported"
        );
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
        assert!(
            waited.is_some(),
            "the top-frame navigation still arms the wait"
        );
    }

    #[tokio::test]
    async fn a_lost_probe_event_conservatively_arms_the_load_wait() {
        let (tx, _) = broadcast::channel::<CdpEvent>(1);
        let rx = tx.subscribe();
        tx.send(ev("Runtime.consoleAPICalled")).unwrap();
        tx.send(ev("Runtime.consoleAPICalled")).unwrap();
        let task = tokio::spawn(settle_after_navigation(rx));
        tokio::time::sleep(Duration::from_millis(10)).await;
        tx.send(ev("Page.loadEventFired")).unwrap();
        let waited = tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("the conservative wait should see the later load")
            .expect("task");
        assert!(
            waited.is_some(),
            "a lost navigation cannot be reported absent"
        );
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
