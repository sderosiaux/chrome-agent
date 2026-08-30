//! Form controls, file inputs and drag. Re-exported from `element` so callers keep one path.

use std::collections::HashMap;

use serde_json::json;

use crate::cdp::client::CdpClient;
use crate::cdp::types::{DispatchMouseEventParams, MouseButton, MouseEventType};
use crate::element_ref::ElementRef;

use std::time::Duration;

use super::element::{
    ElementError, check_js_exception, click, resolve_uid, wait_for_stabilization,
};

/// What a select did, and when it looked. The read-back uses the same window as fill and check
/// (`READ_BACK_MS`), so a component snapping the selection back on a microtask is caught and a
/// later validator is not. `actual`/`kept` are carried, never recomputed from control flow.
pub struct SelectOutcome {
    /// Text of the option the element was asked to hold.
    pub text: String,
    pub actual: Option<String>,
    /// Whether that was the same option — compared by INDEX, so two options sharing a label
    /// cannot pass for each other.
    pub kept: bool,
    /// The `<select>` names a secret through `autocomplete` (`element::SECRET_FIELD`); the
    /// option text is then withheld from the report and the message.
    pub secret: bool,
    pub observed_after_ms: u64,
}

impl SelectOutcome {
    /// The option text for a message, or a marker when the element names a secret. The message
    /// reaches stdout, the transcript and any `--record` file, same as the report.
    #[must_use]
    pub fn label(&self) -> &str {
        if self.secret {
            "(redacted)"
        } else {
            &self.text
        }
    }
}

/// How a `<select>`'s current selection is read. One reader for `select`'s read-back and
/// `assert state --selected`, so the two cannot disagree about the same element.
pub const SELECT_READ: &str = r"function (el) {
    if (el.tagName !== 'SELECT') throw new Error('Element is not a <select>');
    const i = el.selectedIndex;
    const o = (i >= 0 && el.options[i]) ? el.options[i] : null;
    return { index: i, text: o ? o.text : null, value: o ? o.value : null };
}";

/// Set the selection, dispatch `change`, read it back after the window — all bound to the same
/// `el`. Composed at run time so the read-back goes through `SELECT_READ` and not a copy.
fn select_apply() -> String {
    format!(
        r"function (el, target, windowMs) {{
    if (el.tagName !== 'SELECT') throw new Error('Element is not a <select>');
    const opts = Array.from(el.options);
    let idx = opts.findIndex(o => o.value === target);
    if (idx === -1) idx = opts.findIndex(o => o.text.trim() === target);
    if (idx === -1) throw new Error('No option matching: ' + target);
    el.selectedIndex = idx;
    el.dispatchEvent(new Event('change', {{bubbles: true}}));
    return new Promise(resolve => setTimeout(() => {{
        const now = ({SELECT_READ})(el);
        resolve({{
            requested: opts[idx].text,
            kept: now.index === idx,
            actual: now.text,
            secret: !!{SECRET},
        }});
    }}, windowMs));
}}",
        SECRET = crate::element::SECRET_FIELD
    )
}

/// Turn the read-back into the outcome, refusing when the page took the selection away: an
/// agent told "Selected" about a reverted selection submits the form and cannot recover.
fn select_outcome(result: &serde_json::Value) -> Result<SelectOutcome, ElementError> {
    check_js_exception(result)?;
    let value = result.get("result").and_then(|r| r.get("value"));
    let kept = value
        .and_then(|v| v.get("kept"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let requested = value
        .and_then(|v| v.get("requested"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let secret = value
        .and_then(|v| v.get("secret"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let actual = value
        .and_then(|v| v.get("actual"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    if !kept {
        // A secret element names neither option: "reverted to X" leaks X onto stdout and into
        // the transcript as surely as the success would.
        let (held, asked) = if secret {
            (
                "another option".to_string(),
                "the option asked for".to_string(),
            )
        } else {
            (
                actual
                    .as_ref()
                    .map_or_else(|| "nothing".to_string(), |a| format!("\"{a}\"")),
                format!("\"{requested}\""),
            )
        };
        return Err(ElementError::Action(format!(
            "The page reverted the selection to {held} within {}ms; {asked} did not stick.",
            crate::element::READ_BACK_MS
        )));
    }
    Ok(SelectOutcome {
        text: requested.to_string(),
        actual,
        kept,
        secret,
        observed_after_ms: crate::element::READ_BACK_MS,
    })
}

pub async fn select_option(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    uid: &str,
    value: &str,
) -> Result<SelectOutcome, ElementError> {
    let resolved = resolve_uid(client, uid_map, uid).await?;
    let apply = select_apply();
    let js = format!("function(target, windowMs) {{ return ({apply})(this, target, windowMs); }}");
    let result: serde_json::Value = client
        .call(
            "Runtime.callFunctionOn",
            json!({
                "objectId": resolved.object_id,
                "functionDeclaration": js,
                "arguments": [{"value": value}, {"value": crate::element::READ_BACK_MS}],
                "returnByValue": true,
                "awaitPromise": true,
            }),
        )
        .await
        .map_err(|e| ElementError::Action(format!("select_option failed: {e}")))?;

    select_outcome(&result)
}

pub async fn select_option_selector(
    client: &CdpClient,
    selector: &str,
    value: &str,
) -> Result<SelectOutcome, ElementError> {
    let sel_json = serde_json::to_string(selector).unwrap_or_default();
    let val_json = serde_json::to_string(value).unwrap_or_default();
    let js = format!(
        r"(() => {{
            const el = document.querySelector({sel_json});
            if (!el) throw new Error('No element matches selector: ' + {sel_json});
            return ({apply})(el, {val_json}, {window});
        }})()",
        apply = select_apply(),
        window = crate::element::READ_BACK_MS
    );
    let result: serde_json::Value = client
        .call(
            "Runtime.evaluate",
            json!({"expression": js, "returnByValue": true, "awaitPromise": true}),
        )
        .await
        .map_err(|e| ElementError::Action(format!("select_option_selector failed: {e}")))?;

    select_outcome(&result)
}

/// Classify a checkable and read its current state, as a JS expression taking `el`.
///
/// `el.checked` is wrong both ways: every `HTMLInputElement` exposes it, so a text input answers
/// `false`; a `<div role="checkbox" aria-checked="true">` has no such property at all.
pub const CHECKABLE_PROBE: &str = r"function (el) {
  const tag = el.tagName;
  const type = (el.type || '').toLowerCase();
  const role = (el.getAttribute('role') || '').toLowerCase();
  const ariaAttr = el.getAttribute('aria-checked');
  const native = tag === 'INPUT' && (type === 'checkbox' || type === 'radio');
  const aria = ariaAttr !== null ||
    ['checkbox', 'radio', 'switch', 'menuitemcheckbox', 'menuitemradio'].indexOf(role) >= 0;
  if (!native && !aria) {
    return { kind: 'none', tag: tag, type: type, role: role };
  }
  const state = native
    ? (el.indeterminate ? 'mixed' : (el.checked ? 'true' : 'false'))
    : (ariaAttr === null ? 'false' : ariaAttr.toLowerCase());
  return {
    kind: native ? 'native' : 'aria',
    radio: (native && type === 'radio') || role === 'radio' || role === 'menuitemradio',
    state: state
  };
}";

/// What the probe found: either the element cannot be checked, or its state. Crate-visible so
/// `assert state --checked` reads through the same classification the action does.
pub struct Checkable {
    pub kind: String,
    pub radio: bool,
    pub state: String,
    pub tag: String,
    pub ty: String,
    pub role: String,
}

fn parse_probe(v: &serde_json::Value) -> Checkable {
    parse_probe_value(
        &v.get("result")
            .and_then(|r| r.get("value"))
            .cloned()
            .unwrap_or_default(),
    )
}

/// `parse_probe` for a caller that already unwrapped the CDP envelope.
pub fn parse_probe_value(r: &serde_json::Value) -> Checkable {
    let s = |k: &str| {
        r.get(k)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    Checkable {
        kind: s("kind"),
        radio: r
            .get("radio")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        state: s("state"),
        tag: s("tag"),
        ty: s("type"),
        role: s("role"),
    }
}

/// Reject an element that cannot hold a checked state, or a radio asked to become unchecked.
/// `assert state --checked` passes `desired: true`, so only the first refusal applies.
pub fn refuse_uncheckable(probe: &Checkable, desired: bool) -> Result<(), ElementError> {
    if probe.kind == "none" {
        let mut what = probe.tag.to_lowercase();
        if !probe.ty.is_empty() {
            what.push_str(&format!(" type={}", probe.ty));
        }
        if !probe.role.is_empty() {
            what.push_str(&format!(" role={}", probe.role));
        }
        return Err(ElementError::Action(format!(
            "<{what}> has no checked state. check/uncheck need an <input type=checkbox|radio> \
             or an element with role=checkbox|radio|switch and aria-checked."
        )));
    }
    if !desired && probe.radio {
        return Err(ElementError::Action(
            "A radio cannot be unchecked by clicking it. Select another radio in the group instead."
                .into(),
        ));
    }
    Ok(())
}

/// What a check/uncheck asked the element to hold, and what it held when read back. `actual` is
/// the probe's reading, carried rather than assumed equal to `requested`.
pub struct CheckReadBack {
    /// The state asked for, in the words the message uses: `checked` or `unchecked`.
    pub requested: &'static str,
    pub actual: String,
    pub observed_after_ms: u64,
}

/// What a check/uncheck did, and when it looked. "Already checked" is a pre-action observation
/// and "Checked" a post-action one; only the second has a window, which no message conveys.
pub struct CheckOutcome {
    pub message: String,
    /// `None` when the element already held the state: nothing was dispatched, so there is no
    /// post-action moment and no write of ours to have been kept.
    pub read_back: Option<CheckReadBack>,
    /// How the click that changed the state was delivered. `NotProbed` when none was needed;
    /// `JsDispatch` on the selector path, which uses `el.click()` and runs no hit test.
    pub delivery: crate::verdict::Delivery,
}

/// The probe's state token in the words the messages use. An unrecognised `aria-checked` passes
/// through unchanged.
fn state_word(state: &str) -> String {
    match state {
        "true" => "checked".to_string(),
        "false" => "unchecked".to_string(),
        "mixed" => "indeterminate".to_string(),
        other => other.to_string(),
    }
}

impl CheckOutcome {
    const fn already(message: String) -> Self {
        Self {
            message,
            read_back: None,
            delivery: crate::verdict::Delivery::NotProbed,
        }
    }
    fn acted(
        message: String,
        delivery: crate::verdict::Delivery,
        desired: bool,
        held: &str,
    ) -> Self {
        Self {
            message,
            read_back: Some(CheckReadBack {
                requested: if desired { "checked" } else { "unchecked" },
                actual: state_word(held),
                observed_after_ms: crate::element::READ_BACK_MS,
            }),
            delivery,
        }
    }
}

pub async fn set_checked(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    uid: &str,
    desired: bool,
    on_intercept: crate::hit_test::OnIntercept,
) -> Result<CheckOutcome, ElementError> {
    let resolved = resolve_uid(client, uid_map, uid).await?;
    let probe_fn = format!("function() {{ return ({CHECKABLE_PROBE})(this); }}");

    let read_state = |object_id: String, decl: String| async move {
        client
            .call::<_, serde_json::Value>(
                "Runtime.callFunctionOn",
                json!({
                    "objectId": object_id,
                    "functionDeclaration": decl,
                    "returnByValue": true,
                }),
            )
            .await
            .map_err(|e| ElementError::Action(format!("read checked state failed: {e}")))
            .and_then(|v| {
                // A throwing probe yields an empty kind, which sails past the refusal below.
                check_js_exception(&v)?;
                Ok(v)
            })
    };

    let before = parse_probe(&read_state(resolved.object_id.clone(), probe_fn.clone()).await?);
    refuse_uncheckable(&before, desired)?;

    let want = if desired { "true" } else { "false" };
    let asked_for = if desired { "checked" } else { "unchecked" };
    if before.state == want {
        return Ok(CheckOutcome::already(format!(
            "Already {asked_for} uid={uid}"
        )));
    }

    let dispatched = click(client, uid_map, uid, on_intercept).await?;
    // Nothing was dispatched, so the read-back below would blame the page for a click never
    // made and send an agent hunting an overlay that does not exist.
    if let Some(refused) = dispatched.refusal_message("check", &format!("uid={uid}")) {
        return Err(ElementError::NotInteractable(refused));
    }

    // A click is a request, not a result. Waited through the shared window, not for however
    // long a CDP round trip happens to cost.
    tokio::time::sleep(std::time::Duration::from_millis(
        crate::element::READ_BACK_MS,
    ))
    .await;
    let after = parse_probe(&read_state(resolved.object_id, probe_fn).await?);
    if after.state != want {
        let received = dispatched.receiver.as_ref().map_or_else(
            || "the page did not accept the change".to_string(),
            |hit| {
                format!(
                    "the click was received by {}, which covers it",
                    hit.id
                        .as_deref()
                        .map_or_else(|| hit.tag.to_lowercase(), |id| format!("#{id}"))
                )
            },
        );
        return Err(ElementError::Action(format!(
            "uid={uid} is still {} after the click; {received}.",
            state_word(&after.state)
        )));
    }
    Ok(CheckOutcome::acted(
        format!(
            "{} uid={uid}",
            if desired { "Checked" } else { "Unchecked" }
        ),
        dispatched.delivery,
        desired,
        &after.state,
    ))
}

/// Idempotent check/uncheck by CSS selector.
pub async fn set_checked_selector(
    client: &CdpClient,
    selector: &str,
    desired: bool,
) -> Result<CheckOutcome, ElementError> {
    let sel_json = serde_json::to_string(selector).unwrap_or_default();
    let want = if desired { "true" } else { "false" };
    // One evaluation does probe, click and read-back, so all three bind the same node even if
    // the document changes between round trips.
    let js = format!(
        r"(() => {{
            const el = document.querySelector({sel_json});
            if (!el) throw new Error('No element matches selector: ' + {sel_json});
            const probe = ({CHECKABLE_PROBE});
            const before = probe(el);
            if (before.kind === 'none') return before;
            if ('{want}' === 'false' && before.radio) return {{ kind: 'radio_locked' }};
            if (before.state === '{want}') return {{ kind: before.kind, state: 'already' }};
            el.click();
            // Read back after the window: a handler reverting in a promise or a timeout is
            // invisible to a synchronous read.
            return new Promise(resolve => setTimeout(() => {{
                const after = probe(el);
                resolve({{
                    kind: before.kind,
                    state: after.state === '{want}' ? 'ok' : after.state,
                    // The reading itself, beside the verdict on it, so the response reports what
                    // the element held rather than which branch control flow took.
                    held: after.state,
                }});
            }}, {window}));
        }})()",
        window = crate::element::READ_BACK_MS
    );
    let result: serde_json::Value = client
        .call(
            "Runtime.evaluate",
            json!({"expression": js, "returnByValue": true, "awaitPromise": true}),
        )
        .await
        .map_err(|e| ElementError::Action(format!("set_checked_selector failed: {e}")))?;

    check_js_exception(&result)?;
    let probe = parse_probe(&result);
    if probe.kind == "radio_locked" {
        return Err(ElementError::Action(
            "A radio cannot be unchecked by clicking it. Select another radio in the group instead."
                .into(),
        ));
    }
    refuse_uncheckable(&probe, desired)?;

    let held = result
        .get("result")
        .and_then(|r| r.get("value"))
        .and_then(|v| v.get("held"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let asked_for = if desired { "checked" } else { "unchecked" };
    match probe.state.as_str() {
        "already" => Ok(CheckOutcome::already(format!(
            "Already {asked_for} selector '{selector}'"
        ))),
        "ok" => Ok(CheckOutcome::acted(
            format!(
                "{} selector '{selector}'",
                if desired { "Checked" } else { "Unchecked" }
            ),
            // `el.click()` inside the same evaluation as the probe and read-back: no hit test
            // happened, so interception is inapplicable here rather than undetected.
            crate::verdict::Delivery::JsDispatch,
            desired,
            held,
        )),
        // `state_word`, not the probe's token: one state must not have two spellings.
        other => Err(ElementError::Action(format!(
            "selector '{selector}' is still {} after the click; the page did not accept the change.",
            state_word(other)
        ))),
    }
}

/// Validate every upload path exists before invoking CDP, returning the first missing one.
pub fn validate_upload_paths(files: &[String]) -> Result<(), ElementError> {
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
        .send(
            "DOM.setFileInputFiles",
            json!({
                "files": files,
                "backendNodeId": resolved.backend_node_id,
            }),
        )
        .await
        .map_err(|e| ElementError::Action(format!("setFileInputFiles failed: {e}")))?;
    wait_for_stabilization(client, nav_events).await;
    Ok(())
}

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
    let root_node_id = doc
        .get("root")
        .and_then(|r| r.get("nodeId"))
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| ElementError::Action("Could not get root nodeId".into()))?;

    let qs_result: serde_json::Value = client
        .call(
            "DOM.querySelector",
            json!({"nodeId": root_node_id, "selector": selector}),
        )
        .await
        .map_err(|e| ElementError::Action(format!("DOM.querySelector failed: {e}")))?;
    let node_id = qs_result
        .get("nodeId")
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| ElementError::Action(format!("No element matches selector: {selector}")))?;

    let nav_events = client.events();
    client
        .send(
            "DOM.setFileInputFiles",
            json!({
                "files": files,
                "nodeId": node_id,
            }),
        )
        .await
        .map_err(|e| ElementError::Action(format!("setFileInputFiles failed: {e}")))?;
    wait_for_stabilization(client, nav_events).await;
    Ok(())
}

/// Mouse-move points for a drag from `(x1,y1)` to `(x2,y2)` over `steps` segments; the last one
/// lands on the destination.
pub fn drag_interpolation_points(
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    steps: u32,
) -> Vec<(f64, f64)> {
    (1..=steps)
        .map(|i| {
            let t = f64::from(i) / f64::from(steps);
            ((x2 - x1).mul_add(t, x1), (y2 - y1).mul_add(t, y1))
        })
        .collect()
}

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

    let mouse = |et, x, y, btn: Option<MouseButton>, btns, cc| DispatchMouseEventParams {
        event_type: et,
        x,
        y,
        button: btn,
        buttons: btns,
        click_count: cc,
        modifiers: None,
        timestamp: None,
        delta_x: None,
        delta_y: None,
        pointer_type: Some("mouse".into()),
    };

    // Five pointer events, each of which would pay the background-tab timer without this.
    client.ensure_foreground().await;
    let nav_events = client.events();
    client
        .send_input(
            "Input.dispatchMouseEvent",
            mouse(MouseEventType::MouseMoved, x1, y1, None, None, None),
        )
        .await
        .map_err(|e| ElementError::Action(format!("drag move failed: {e}")))?;

    client
        .send_input(
            "Input.dispatchMouseEvent",
            mouse(
                MouseEventType::MousePressed,
                x1,
                y1,
                Some(MouseButton::Left),
                Some(1),
                Some(1),
            ),
        )
        .await
        .map_err(|e| ElementError::Action(format!("drag press failed: {e}")))?;

    for (x, y) in drag_interpolation_points(x1, y1, x2, y2, 5) {
        client
            .send_input(
                "Input.dispatchMouseEvent",
                mouse(
                    MouseEventType::MouseMoved,
                    x,
                    y,
                    Some(MouseButton::Left),
                    Some(1),
                    None,
                ),
            )
            .await
            .map_err(|e| ElementError::Action(format!("drag step failed: {e}")))?;
        tokio::time::sleep(Duration::from_millis(16)).await;
    }

    client
        .send_input(
            "Input.dispatchMouseEvent",
            mouse(
                MouseEventType::MouseReleased,
                x2,
                y2,
                Some(MouseButton::Left),
                Some(0),
                Some(1),
            ),
        )
        .await
        .map_err(|e| ElementError::Action(format!("drag release failed: {e}")))?;

    wait_for_stabilization(client, nav_events).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_interpolation_5_steps() {
        let points = drag_interpolation_points(100.0, 100.0, 200.0, 300.0, 5);
        assert_eq!(points.len(), 5);
        assert!((points[0].0 - 120.0).abs() < 0.01);
        assert!((points[0].1 - 140.0).abs() < 0.01);
        // Last point lands exactly on the destination.
        assert!((points[4].0 - 200.0).abs() < 0.01);
        assert!((points[4].1 - 300.0).abs() < 0.01);
    }

    #[test]
    fn upload_validation_rejects_missing_and_accepts_existing() {
        let missing = vec!["/nonexistent/file.txt".to_string()];
        let err = validate_upload_paths(&missing).unwrap_err();
        assert!(matches!(err, ElementError::Action(_)));
        assert!(
            err.to_string()
                .contains("File not found: /nonexistent/file.txt")
        );
        // An existing path: this test binary.
        let exe = std::env::current_exe()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(validate_upload_paths(&[exe]).is_ok());
    }
}
