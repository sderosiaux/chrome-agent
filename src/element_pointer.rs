//! The pointer path: aiming a click/double-click/hover at a node and dispatching it — mouse
//! natively, touch under emulation, JS as the fallback when there is no box to aim at. It moves
//! when `hit_test` moves; the rest of `element` does not. Re-exported from `element` so callers
//! keep one path.

use std::collections::HashMap;

use serde_json::json;

use crate::cdp::client::CdpClient;
use crate::cdp::types::{DispatchMouseEventParams, MouseButton, MouseEventType};
use crate::element::{ElementError, js_exception, resolve_uid, wait_for_stabilization};
use crate::element_ref::ElementRef;

/// Which pointer action is being aimed. The aiming, the refusals and the fallbacks are identical
/// for both verbs — they differ in the three things this enum carries: the JS fallback used when
/// there is no box to aim at, the native dispatch at the point the probe agreed on, and the word a
/// refusal is written in. Held as one type so a rule added to the aim path cannot land on one verb
/// and miss the other, which is how the two copies of it drifted before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerVerb {
    Click,
    DblClick,
}

impl PointerVerb {
    /// The word a refusal message is written in.
    const fn word(self) -> &'static str {
        match self {
            Self::Click => "click",
            Self::DblClick => "double-click",
        }
    }

    /// The fallback when the element has no layout box: no point to aim at, so no hit test.
    async fn js_fallback(self, client: &CdpClient, object_id: &str) -> Result<(), ElementError> {
        match self {
            Self::Click => js_click(client, object_id).await,
            Self::DblClick => js_dblclick(client, object_id).await,
        }
    }

    /// The native dispatch at a point somebody else decided on.
    async fn dispatch_at(self, client: &CdpClient, point: (f64, f64)) -> Result<(), ElementError> {
        match self {
            Self::Click => dispatch_click_at(client, point.0, point.1).await,
            Self::DblClick => dblclick_at_coords(client, point.0, point.1).await,
        }
    }
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
    pointer(client, uid_map, uid, PointerVerb::Click, on_intercept).await
}

/// Double-click an element by uid. Aimed by the same probe as `click`.
pub async fn dblclick(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    uid: &str,
    on_intercept: crate::hit_test::OnIntercept,
) -> Result<crate::hit_test::Dispatched, ElementError> {
    pointer(client, uid_map, uid, PointerVerb::DblClick, on_intercept).await
}

/// Aim a pointer verb at a uid and dispatch it, naming the node on whatever comes back — the
/// success and the refusal both carry it.
async fn pointer(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    uid: &str,
    verb: PointerVerb,
    on_intercept: crate::hit_test::OnIntercept,
) -> Result<crate::hit_test::Dispatched, ElementError> {
    let resolved = resolve_uid(client, uid_map, uid).await?;
    if resolved.center.is_none() {
        verb.js_fallback(client, &resolved.object_id).await?;
        return Ok(crate::hit_test::Dispatched::js().named(Some(uid.to_string()), None, None));
    }
    let outcome = aim_and_dispatch(
        client,
        verb,
        &resolved.object_id,
        resolved.center,
        on_intercept,
        &format!("uid={uid}"),
    )
    .await
    .map_err(|e| e.naming(Some(uid.to_string()), None, None))?;
    Ok(outcome.named(Some(uid.to_string()), None, None))
}

/// Aim at a resolved handle and dispatch `verb` at it. The one aim path: shared by the uid and
/// selector callers of both verbs. `fallback_center` is the box model's centre, used only when the
/// probe could not run.
pub async fn aim_and_dispatch(
    client: &CdpClient,
    verb: PointerVerb,
    object_id: &str,
    fallback_center: Option<(f64, f64)>,
    on_intercept: crate::hit_test::OnIntercept,
    target: &str,
) -> Result<crate::hit_test::Dispatched, ElementError> {
    use crate::hit_test::{Aim, Dispatched};
    use crate::verdict::Delivery;

    let (point, delivery, receiver, unaimable) = match crate::hit_test::aim(client, object_id).await
    {
        Aim::NoBox => {
            verb.js_fallback(client, object_id).await?;
            return Ok(Dispatched::js());
        }
        Aim::Unprobed => {
            let Some(center) = fallback_center else {
                verb.js_fallback(client, object_id).await?;
                return Ok(Dispatched::js());
            };
            (center, Delivery::NotProbed, None, None)
        }
        Aim::At {
            point,
            delivery,
            receiver,
            unaimable,
        } => (point, delivery, receiver, unaimable),
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
            verb.word(),
            target,
        ));
    }

    verb.dispatch_at(client, point).await?;
    Ok(Dispatched::landed(delivery, point, receiver))
}

/// The error a refused pointer action returns, carrying what the probe measured. One place for
/// both verbs, so `click` and `dblclick` refuse in the same words.
fn refusal(dispatched: crate::hit_test::Dispatched, verb: &str, target: &str) -> ElementError {
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
    let nav_events = client.page_events();
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
            .send_input(
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

        client
            .send_input(
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
    }

    wait_for_stabilization(client, nav_events).await;
    Ok(())
}

/// Click at explicit (x, y) coordinates. No hit test: `--xy` names no element, so there is
/// nothing for a receiver to differ from.
pub async fn click_at_coords(client: &CdpClient, x: f64, y: f64) -> Result<(), ElementError> {
    dispatch_click_at(client, x, y).await
}

/// Fallback: click an element via JS `.click()` when mouse events can't be dispatched.
pub async fn js_click(client: &CdpClient, object_id: &str) -> Result<(), ElementError> {
    let nav_events = client.page_events();
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

    if let Some(thrown) = js_exception(&result) {
        return Err(ElementError::Action(format!("JS click threw: {thrown}")));
    }

    wait_for_stabilization(client, nav_events).await;
    Ok(())
}

pub async fn js_dblclick(client: &CdpClient, object_id: &str) -> Result<(), ElementError> {
    let nav_events = client.page_events();
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
    let nav_events = client.page_events();
    client.mark_dispatch();
    for click_count in [1, 2] {
        client
            .send_input(
                "Input.dispatchMouseEvent",
                DispatchMouseEventParams {
                    event_type: MouseEventType::MousePressed,
                    x,
                    y,
                    button: Some(MouseButton::Left),
                    buttons: Some(1),
                    click_count: Some(click_count),
                    modifiers: None,
                    timestamp: None,
                    delta_x: None,
                    delta_y: None,
                    pointer_type: Some("mouse".into()),
                },
            )
            .await
            .map_err(|e| ElementError::Action(format!("mousePressed failed: {e}")))?;

        client
            .send_input(
                "Input.dispatchMouseEvent",
                DispatchMouseEventParams {
                    event_type: MouseEventType::MouseReleased,
                    x,
                    y,
                    button: Some(MouseButton::Left),
                    buttons: Some(0),
                    click_count: Some(click_count),
                    modifiers: None,
                    timestamp: None,
                    delta_x: None,
                    delta_y: None,
                    pointer_type: Some("mouse".into()),
                },
            )
            .await
            .map_err(|e| ElementError::Action(format!("mouseReleased failed: {e}")))?;
    }
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
        ElementError::NotInteractable(format!("Element uid={uid} has no visible box model."))
    })?;

    // A background tab answers a pointer event on a fixed five-second timer, the foreground tab
    // in single-digit ms. Costs 3 ms, once per connection (`CdpClient::ensure_foreground`).
    client.ensure_foreground().await;
    client
        .send_input(
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

#[cfg(test)]
mod tests {
    use super::PointerVerb;

    /// The refusal is written in the verb that was refused, not in a word one of the two
    /// duplicated paths happened to hardcode.
    #[test]
    fn each_pointer_verb_refuses_in_its_own_word() {
        assert_eq!(PointerVerb::Click.word(), "click");
        assert_eq!(PointerVerb::DblClick.word(), "double-click");
    }
}
