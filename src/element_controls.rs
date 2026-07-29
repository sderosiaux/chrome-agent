//! Form controls, file inputs and drag, split out of `element.rs` to stay under the repo's
//! 1000-line file cap. Re-exported from `element` so callers keep one path.

use std::collections::HashMap;

use serde_json::json;

use crate::cdp::client::CdpClient;
use crate::cdp::types::{DispatchMouseEventParams, MouseButton, MouseEventType};
use crate::element_ref::ElementRef;

use std::time::Duration;

use super::element::{check_js_exception, click, resolve_uid, wait_for_stabilization, ElementError};

/// What a select did, and when it looked.
///
/// The read-back happens through the same observation window as fill and check
/// (`READ_BACK_MS`): a controlled component that snaps the selection back on a microtask
/// or `setTimeout(0)` is caught; a validator firing later is not, which is why the window
/// is reported rather than persistence asserted.
pub struct SelectOutcome {
    /// Text of the option the page still held when we looked.
    pub text: String,
    pub observed_after_ms: u64,
}

/// Shared body: set the selection, dispatch `change`, then read it back after the
/// window — all bound to the same `el`, so a document change between round trips
/// cannot swap the node under us.
const SELECT_APPLY: &str = r"function (el, target, windowMs) {
    if (el.tagName !== 'SELECT') throw new Error('Element is not a <select>');
    const opts = Array.from(el.options);
    let idx = opts.findIndex(o => o.value === target);
    if (idx === -1) idx = opts.findIndex(o => o.text.trim() === target);
    if (idx === -1) throw new Error('No option matching: ' + target);
    el.selectedIndex = idx;
    el.dispatchEvent(new Event('change', {bubbles: true}));
    return new Promise(resolve => setTimeout(() => {
        const now = el.selectedIndex;
        resolve({
            requested: opts[idx].text,
            kept: now === idx,
            actual: (now >= 0 && el.options[now]) ? el.options[now].text : null,
        });
    }, windowMs));
}";

/// Turn the read-back into the outcome, refusing when the page took the selection away.
///
/// Same policy as check: reporting "Selected" for a select the page has already snapped
/// back is the one answer an agent cannot recover from — it submits the form believing a
/// different option is chosen than what the page holds.
fn select_outcome(result: &serde_json::Value) -> Result<SelectOutcome, ElementError> {
    check_js_exception(result)?;
    let value = result.get("result").and_then(|r| r.get("value"));
    let kept = value.and_then(|v| v.get("kept")).and_then(serde_json::Value::as_bool).unwrap_or(false);
    let requested = value.and_then(|v| v.get("requested")).and_then(serde_json::Value::as_str).unwrap_or("");
    if !kept {
        let actual = value
            .and_then(|v| v.get("actual"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("nothing");
        return Err(ElementError::Action(format!(
            "The page reverted the selection to \"{actual}\" within {}ms; \"{requested}\" did not stick.",
            crate::element::READ_BACK_MS
        )));
    }
    Ok(SelectOutcome {
        text: requested.to_string(),
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
    let js = format!(
        "function(target, windowMs) {{ return ({SELECT_APPLY})(this, target, windowMs); }}"
    );
    let result: serde_json::Value = client
        .call("Runtime.callFunctionOn", json!({
            "objectId": resolved.object_id,
            "functionDeclaration": js,
            "arguments": [{"value": value}, {"value": crate::element::READ_BACK_MS}],
            "returnByValue": true,
            "awaitPromise": true,
        }))
        .await
        .map_err(|e| ElementError::Action(format!("select_option failed: {e}")))?;

    select_outcome(&result)
}

/// Select a dropdown option by CSS selector.
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
            return ({SELECT_APPLY})(el, {val_json}, {window});
        }})()",
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

// ---------------------------------------------------------------------------
// Check / Uncheck
// ---------------------------------------------------------------------------

/// Classify a checkable and read its current state, as a JS expression taking `el`.
///
/// `el.checked` is the wrong reading twice over. Every `HTMLInputElement` exposes it, so a
/// text input answers `false` and a click on it reports success while meaning nothing. And a
/// `<div role="checkbox" aria-checked="true">` has no such property at all, so a truthiness
/// read calls a checked box unchecked and the click turns it OFF while reporting success.
const CHECKABLE_PROBE: &str = r"function (el) {
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

/// What the probe found: either the element can't be checked, or here is its state.
struct Checkable {
    kind: String,
    radio: bool,
    state: String,
    tag: String,
    ty: String,
    role: String,
}

fn parse_probe(v: &serde_json::Value) -> Checkable {
    let r = v.get("result").and_then(|r| r.get("value")).cloned().unwrap_or_default();
    let s = |k: &str| r.get(k).and_then(serde_json::Value::as_str).unwrap_or_default().to_string();
    Checkable {
        kind: s("kind"),
        radio: r.get("radio").and_then(serde_json::Value::as_bool).unwrap_or(false),
        state: s("state"),
        tag: s("tag"),
        ty: s("type"),
        role: s("role"),
    }
}

/// Reject an element that cannot hold a checked state, or a radio asked to become unchecked.
fn refuse_uncheckable(probe: &Checkable, desired: bool) -> Result<(), ElementError> {
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

/// Idempotent check/uncheck: query current state, click only if different.
/// What a check/uncheck did, and when it looked.
///
/// The message alone could not say whether a read-back had happened: "Already checked" is a
/// pre-action observation, "Checked" is a post-action one, and only the second has a window.
pub struct CheckOutcome {
    pub message: String,
    /// `None` when the element already held the desired state, so nothing was dispatched
    /// and there was nothing to observe afterwards.
    pub observed_after_ms: Option<u64>,
}

impl CheckOutcome {
    const fn already(message: String) -> Self {
        Self { message, observed_after_ms: None }
    }
    const fn acted(message: String) -> Self {
        Self { message, observed_after_ms: Some(crate::element::READ_BACK_MS) }
    }
}

pub async fn set_checked(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    uid: &str,
    desired: bool,
) -> Result<CheckOutcome, ElementError> {
    let resolved = resolve_uid(client, uid_map, uid).await?;
    let probe_fn = format!("function() {{ return ({CHECKABLE_PROBE})(this); }}");

    let read_state = |object_id: String, decl: String| async move {
        client
            .call::<_, serde_json::Value>("Runtime.callFunctionOn", json!({
                "objectId": object_id,
                "functionDeclaration": decl,
                "returnByValue": true,
            }))
            .await
            .map_err(|e| ElementError::Action(format!("read checked state failed: {e}")))
            .and_then(|v| {
                // A throwing probe used to yield an empty kind, sail past the refusal and
                // click the element anyway — blaming the page for our own read failure
                // while mutating it unasked.
                check_js_exception(&v)?;
                Ok(v)
            })
    };

    let before = parse_probe(&read_state(resolved.object_id.clone(), probe_fn.clone()).await?);
    refuse_uncheckable(&before, desired)?;

    let want = if desired { "true" } else { "false" };
    let state_word = if desired { "checked" } else { "unchecked" };
    if before.state == want {
        return Ok(CheckOutcome::already(format!("Already {state_word} uid={uid}")));
    }

    click(client, uid_map, uid).await?;

    // Read it back: a click is a request, not a result. A handler can reject or revert it,
    // and reporting "Checked" for a box that is still off is the one answer an agent cannot
    // recover from. Waited explicitly, not by accident: this path used to observe whatever
    // a CDP round trip happened to cost, which is neither the selector path's window nor
    // any window at all.
    tokio::time::sleep(std::time::Duration::from_millis(crate::element::READ_BACK_MS)).await;
    let after = parse_probe(&read_state(resolved.object_id, probe_fn).await?);
    if after.state != want {
        return Err(ElementError::Action(format!(
            "uid={uid} is still {} after the click; the page did not accept the change.",
            if after.state == "mixed" { "indeterminate" } else { &after.state }
        )));
    }
    Ok(CheckOutcome::acted(format!(
        "{} uid={uid}",
        if desired { "Checked" } else { "Unchecked" }
    )))
}

/// Idempotent check/uncheck by CSS selector.
pub async fn set_checked_selector(
    client: &CdpClient,
    selector: &str,
    desired: bool,
) -> Result<CheckOutcome, ElementError> {
    let sel_json = serde_json::to_string(selector).unwrap_or_default();
    let want = if desired { "true" } else { "false" };
    // One evaluation does probe, click and read-back, so all three bind the same node even
    // if the document changes under us between round trips.
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
            // Read back after the window: a handler that reverts the change in a promise
            // or a timeout is invisible to a synchronous read.
            return new Promise(resolve => setTimeout(() => {{
                const after = probe(el);
                resolve({{ kind: before.kind, state: after.state === '{want}' ? 'ok' : after.state }});
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

    let state_word = if desired { "checked" } else { "unchecked" };
    match probe.state.as_str() {
        "already" => Ok(CheckOutcome::already(format!(
            "Already {state_word} selector '{selector}'"
        ))),
        "ok" => Ok(CheckOutcome::acted(format!(
            "{} selector '{selector}'",
            if desired { "Checked" } else { "Unchecked" }
        ))),
        other => Err(ElementError::Action(format!(
            "selector '{selector}' is still {} after the click; the page did not accept the change.",
            if other == "mixed" { "indeterminate" } else { other }
        ))),
    }
}

// ---------------------------------------------------------------------------
// File upload
// ---------------------------------------------------------------------------

/// Validate every upload path exists before invoking CDP; returns the first
/// missing path as `ElementError::Action`. Shared by both upload entry points.
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
pub fn drag_interpolation_points(x1: f64, y1: f64, x2: f64, y2: f64, steps: u32) -> Vec<(f64, f64)> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
