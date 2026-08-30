//! Where a mouse event will actually land, measured before it is dispatched.
//!
//! A click used to be reported against the element the caller named. Nothing checked that the
//! named element was the one at the coordinates: on `tests/fixtures/click_overlay.html` a
//! click on `#target` reported `changed / focus_only` while `window.receiver === "scrim"` —
//! the scrim's handler had run, the button's never had, and the response looked exactly like
//! a click a person could have made. The same silence covered a second failure: under
//! `scroll-behavior: smooth` the box model was read mid-animation and the event was dispatched
//! hundreds of pixels away from anything, which also reported `focus_only`.
//!
//! One `Runtime.callFunctionOn` bound to the target's own `objectId` answers both. It must be
//! bound to the node and not be a bare `Runtime.evaluate`: `this.getRootNode()` on a handle to
//! a node inside a CLOSED shadow root returns that root and `.host` works, which is what makes
//! the containment climb below correct without `DOM.getNodeForLocation`.
//!
//! What this is NOT: proof that the receiver did anything, and not the browser's own input
//! hit test. `document.elementFromPoint` agreed with a real coordinate click on every fixture
//! it was checked against, which is evidence and not equivalence — top-layer content,
//! compositor-side scroll offsets and in-flight transforms can diverge. That is why the
//! default policy still dispatches (`--on-intercept dispatch`): a wrong interception call
//! costs a warning, refusing by default would cost the action.

use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::cdp::client::CdpClient;
use crate::element::ElementError;
use crate::verdict::Delivery;

/// What to do when the aim point turns out to belong to another element.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OnIntercept {
    /// Dispatch anyway and say who received it. What a pointer does, and what every release
    /// before the hit test did — so the flag changes the report, not the behaviour.
    #[default]
    Dispatch,
    /// Refuse the action without dispatching. For a caller that would rather re-plan than
    /// hand an event to an element it did not name.
    Refuse,
}

impl OnIntercept {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "dispatch" => Ok(Self::Dispatch),
            "refuse" => Ok(Self::Refuse),
            other => Err(format!(
                "Unknown --on-intercept value '{other}'. Use \"dispatch\" (default) or \"refuse\"."
            )),
        }
    }

    /// The per-command override, falling back to the session's policy.
    #[must_use]
    pub fn from_cmd(cmd: &Value, fallback: Self) -> Self {
        cmd.get("on_intercept")
            .and_then(Value::as_str)
            .and_then(|v| Self::parse(v).ok())
            .unwrap_or(fallback)
    }
}

/// How long the settle loop waits between readings, and how many it takes.
///
/// 5 × 30 ms is enough for a layout that settles (a sticky header reflowing, an
/// `IntersectionObserver` inserting content above the target) and deliberately NOT enough to
/// sit out a long animation: a caller who needs that has `wait`, and silently absorbing
/// seconds inside every click is worse than saying the aim never settled.
const SETTLE_ATTEMPTS: u32 = 5;
const SETTLE_GAP_MS: u64 = 30;

/// A point counts as the same point across two readings within half a CSS pixel — a scroll
/// still animating moves much further than that between readings.
const SAME_POINT_EPSILON: f64 = 0.5;

/// What the probe found at the point it is about to be dispatched at.
///
/// Deserialized straight from the JS return value so [`classify`] can be exercised on frozen
/// JSON with no Chrome in the loop.
// One flag per independent question the probe answers; collapsing them into an enum would
// force an order on facts the classifier is what orders.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Deserialize)]
pub struct Probe {
    /// The node has at least one client rect. `false` is a zero-size or `display:none`
    /// element: there is no point to aim at, and no hit test to run.
    #[serde(default)]
    pub rendered: bool,
    /// The aim point lies inside the hit-testing document's viewport.
    #[serde(default, rename = "inViewport")]
    pub in_viewport: bool,
    /// The aim point lies inside one of the target's own client rects. False on shapes where
    /// the centre of the bounding box falls in a gap — an inline link wrapped across two
    /// lines is the canonical one, which is why the largest rect is aimed at rather than the
    /// bounding box.
    #[serde(default, rename = "aimIn")]
    pub aim_in: bool,
    /// The element at the aim point is the target, inside it, its label's control, or its
    /// shadow host.
    #[serde(default)]
    pub landed: bool,
    /// How many frames up from the target to the top document.
    #[serde(default)]
    pub depth: u32,
    /// Whether the frame chain could be walked to convert the local aim point into the
    /// top-level coordinates `Input.dispatchMouseEvent` takes.
    #[serde(default, rename = "offsetKnown")]
    pub offset_known: bool,
    /// The aim point in the target document's own coordinates.
    #[serde(default)]
    pub aim: Option<[f64; 2]>,
    /// The same point in top-level viewport coordinates.
    #[serde(default)]
    pub top: Option<[f64; 2]>,
    /// The element at the aim point, whoever it is.
    #[serde(default)]
    pub hit: Option<Hit>,
}

/// The element the aim point resolved to.
#[derive(Debug, Clone, Deserialize)]
pub struct Hit {
    pub tag: String,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub cls: Option<String>,
    #[serde(default)]
    pub z: Option<String>,
    #[serde(default)]
    pub text: String,
    /// In the top layer (`:modal`). Its own recovery: close the dialog.
    #[serde(default)]
    pub modal: bool,
    #[serde(default)]
    pub iframe: bool,
    #[serde(default, rename = "sameDoc")]
    pub same_doc: bool,
    /// Resolved afterwards, best effort — an overlay container usually has no accessibility
    /// node, so it usually has no uid either.
    #[serde(skip)]
    pub uid: Option<String>,
}

impl Hit {
    /// `tag#id.class`, the shape a person can find in the page source.
    ///
    /// `pub(crate)` for `hints`: a click-triggered download that produced nothing has one
    /// explanation the hit test already measured — another element took the click — and a hint
    /// that cannot name it sends the caller to `inspect` for a fact this response holds.
    pub(crate) fn describe(&self) -> String {
        let mut out = self.tag.to_lowercase();
        if let Some(id) = self.id.as_deref().filter(|s| !s.is_empty()) {
            out.push('#');
            out.push_str(id);
        }
        if let Some(class) = self.cls.as_deref().filter(|s| !s.is_empty()) {
            for part in class.split_whitespace().take(2) {
                out.push('.');
                out.push_str(part);
            }
        }
        out
    }

    fn report(&self) -> Value {
        json!({
            "uid": self.uid,
            "tag": self.tag,
            "id": self.id,
            "class": self.cls,
            "z_index": self.z,
            "text": self.text,
            "modal": self.modal,
            "iframe": self.iframe,
            "same_document": self.same_doc,
        })
    }
}

/// The probe, bound to the target's `objectId`.
const PROBE_JS: &str = r"function () {
  const rectsOf = (el) => Array.from(el.getClientRects()).filter(r => r.width > 0 || r.height > 0);
  const largest = (rs) => rs.reduce((a, b) => (b.width * b.height > a.width * a.height ? b : a));
  const vw = window.innerWidth, vh = window.innerHeight;
  const fully = (r) => r.top >= 0 && r.left >= 0 && r.bottom <= vh && r.right <= vw;
  let rects = rectsOf(this);
  if (!rects.length) return { rendered: false };
  let box = largest(rects);
  if (!fully(box)) {
    // 'instant', never the page's own scroll-behavior: under `smooth` the scroll becomes an
    // animation and every rect read below would still report the pre-scroll position.
    try {
      this.scrollIntoView({ block: 'center', inline: 'center', behavior: 'instant' });
    } catch (e) {
      this.scrollIntoView();
    }
    rects = rectsOf(this);
    if (!rects.length) return { rendered: false };
    box = largest(rects);
  }
  const cx = box.left + box.width / 2, cy = box.top + box.height / 2;
  // Local -> top-level coordinates. Input.dispatchMouseEvent takes the top-level viewport;
  // getClientRects inside a frame does not, so a target in an iframe would be clicked at the
  // frame's own offset without this walk.
  let ox = 0, oy = 0, depth = 0, offsetKnown = true;
  try {
    let w = window;
    while (w.frameElement) {
      const fe = w.frameElement, fr = fe.getBoundingClientRect();
      const cs = fe.ownerDocument.defaultView.getComputedStyle(fe);
      ox += fr.left + parseFloat(cs.borderLeftWidth || '0') + parseFloat(cs.paddingLeft || '0');
      oy += fr.top + parseFloat(cs.borderTopWidth || '0') + parseFloat(cs.paddingTop || '0');
      depth += 1;
      w = w.parent;
      if (depth > 8) { offsetKnown = false; break; }
    }
  } catch (e) {
    // A cross-origin ancestor: the offset is unknowable from here, so say so rather than
    // dispatching at a coordinate that means nothing.
    offsetKnown = false;
  }
  const inViewport = cx >= 0 && cy >= 0 && cx < vw && cy < vh;
  const aimIn = rects.some(r => cx >= r.left && cx <= r.right && cy >= r.top && cy <= r.bottom);
  let h = document.elementFromPoint(cx, cy);
  while (h && h.shadowRoot) {
    const inner = h.shadowRoot.elementFromPoint(cx, cy);
    if (!inner || inner === h) break;
    h = inner;
  }
  const root = this.getRootNode();
  const host = (root && root.host) ? root.host : null;
  let landed = false;
  if (h) {
    landed = h === this ||
      this.contains(h) ||
      (!!h.closest && !!h.closest('label') && h.closest('label').control === this) ||
      (host !== null && (host === h || host.contains(h)));
  }
  const modalOf = (el) => {
    try { return el.matches(':modal'); }
    catch (e) { return el.tagName === 'DIALOG' && el.hasAttribute('open'); }
  };
  return {
    rendered: true, inViewport, aimIn, landed, depth, offsetKnown,
    aim: [cx, cy], top: [cx + ox, cy + oy],
    hit: h ? {
      tag: h.tagName,
      id: h.id || null,
      cls: (typeof h.className === 'string' && h.className) ? h.className : null,
      z: window.getComputedStyle(h).zIndex,
      text: (h.textContent || '').replace(/\s+/g, ' ').trim().slice(0, 40),
      modal: modalOf(h),
      iframe: h.tagName === 'IFRAME',
      sameDoc: h.ownerDocument === this.ownerDocument
    } : null
  };
}";

/// Turn one probe reading into a delivery. Pure; the temporal half (whether the aim point has
/// stopped moving) belongs to [`aim`], which owns the loop.
#[must_use]
pub const fn classify(probe: &Probe) -> Delivery {
    if !probe.rendered {
        // Nothing to aim at. The caller falls back to a JS click and records that instead.
        return Delivery::NotProbed;
    }
    if !probe.in_viewport {
        return Delivery::NotSettled;
    }
    if !probe.aim_in {
        return Delivery::OffTarget;
    }
    // Inside a frame we hit-test the frame's own document, never its parent's: an overlay in
    // the parent covering the <iframe> is invisible from here. The aim point is still right
    // (it was mapped through the frame chain), the claim would not be — so no claim is made.
    // This is the paired half of naming an <iframe> as a receiver: without it, every target
    // inside a frame would report a clean hit that nothing checked.
    if probe.depth > 0 || !probe.offset_known {
        return Delivery::NotProbed;
    }
    if probe.landed { Delivery::TargetHit } else { Delivery::Intercepted }
}

/// The result of aiming at a node.
pub enum Aim {
    /// A point to dispatch at, in top-level coordinates, and what sits there.
    At { point: (f64, f64), delivery: Delivery, receiver: Option<Hit> },
    /// The node has no layout box at all. There is nothing to aim at.
    NoBox,
    /// The probe could not be run, or could not map its own coordinates. The caller keeps
    /// whatever it had and claims nothing.
    Unprobed,
}

fn same_point(a: Option<[f64; 2]>, b: Option<[f64; 2]>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => {
            (x[0] - y[0]).abs() < SAME_POINT_EPSILON && (x[1] - y[1]).abs() < SAME_POINT_EPSILON
        }
        _ => false,
    }
}

async fn probe_once(client: &CdpClient, object_id: &str) -> Option<Probe> {
    let result: Value = client
        .call(
            "Runtime.callFunctionOn",
            json!({
                "objectId": object_id,
                "functionDeclaration": PROBE_JS,
                "returnByValue": true,
            }),
        )
        .await
        .ok()?;
    if result.get("exceptionDetails").is_some() {
        return None;
    }
    serde_json::from_value(result.get("result")?.get("value")?.clone()).ok()
}

/// Scroll the target into view, work out where a click on it would go, and say what would
/// receive it — without dispatching anything.
///
/// The loop exists for one measured failure: a scroll that is still running when the rect is
/// read. Two consecutive readings must agree AND be in the viewport before the point is
/// trusted; a point that is merely in the viewport while still moving is the smooth-scroll bug
/// with a smaller offset. A single reading is accepted when it is already settled, so the
/// common case pays one round trip and no wait.
pub async fn aim(client: &CdpClient, object_id: &str) -> Aim {
    let Some(mut probe) = probe_once(client, object_id).await else {
        return Aim::Unprobed;
    };
    if !probe.rendered {
        return Aim::NoBox;
    }
    let mut settled = probe.in_viewport;
    let mut attempts = 0;
    while !settled && attempts < SETTLE_ATTEMPTS {
        let previous = probe.aim;
        tokio::time::sleep(Duration::from_millis(SETTLE_GAP_MS)).await;
        let Some(next) = probe_once(client, object_id).await else {
            break;
        };
        if !next.rendered {
            return Aim::NoBox;
        }
        settled = next.in_viewport && same_point(previous, next.aim);
        probe = next;
        attempts += 1;
    }

    // The mapping failed, so the point we measured cannot be expressed in the coordinates a
    // dispatch takes. Better to fall back to the box model than to click somewhere arbitrary.
    let Some(top) = probe.top.filter(|_| probe.offset_known) else {
        return Aim::Unprobed;
    };
    let delivery = if settled { classify(&probe) } else { Delivery::NotSettled };
    let mut receiver = if delivery == Delivery::Intercepted { probe.hit.clone() } else { None };
    if let Some(hit) = receiver.as_mut() {
        hit.uid = receiver_uid(client, object_id).await;
    }
    Aim::At { point: (top[0], top[1]), delivery, receiver }
}

/// The uid of the element that received the event, when it has one.
///
/// Two extra round trips, only on the intercepted path, and best effort throughout: the
/// containers that swallow clicks — scrims, cookie veils, hotspot overlays — carry no
/// accessibility node, so they are absent from every snapshot and have no uid to find. Naming
/// them structurally is the fallback, which is why `intercepted_by` also carries tag/id/class.
async fn receiver_uid(client: &CdpClient, object_id: &str) -> Option<String> {
    // Recomputed from the target's handle rather than passed a coordinate: the point is
    // derived the same way as in the probe, so the two cannot drift apart.
    let js = r"function () {
        const r = Array.from(this.getClientRects()).filter(x => x.width > 0 || x.height > 0);
        if (!r.length) return null;
        const b = r.reduce((a, c) => (c.width * c.height > a.width * a.height ? c : a));
        let h = document.elementFromPoint(b.left + b.width / 2, b.top + b.height / 2);
        while (h && h.shadowRoot) {
            const inner = h.shadowRoot.elementFromPoint(b.left + b.width / 2, b.top + b.height / 2);
            if (!inner || inner === h) break;
            h = inner;
        }
        return h;
    }";
    let handle: Value = client
        .call(
            "Runtime.callFunctionOn",
            json!({"objectId": object_id, "functionDeclaration": js}),
        )
        .await
        .ok()?;
    let receiver_object = handle.get("result")?.get("objectId")?.as_str()?.to_string();
    let described: Value = client
        .call("DOM.describeNode", json!({"objectId": receiver_object}))
        .await
        .ok()?;
    let backend_id = described.get("node")?.get("backendNodeId")?.as_i64()?;
    Some(format!("n{backend_id}"))
}

/// What a pointer-targeted action did, and to whom.
pub struct Dispatched {
    pub delivery: Delivery,
    /// False when the aim never settled, or the caller asked to refuse an interception.
    pub sent: bool,
    pub aim: Option<(f64, f64)>,
    pub receiver: Option<Hit>,
    /// The node that was acted on, resolved before the action from the same handle that was
    /// probed and clicked — so the uid in the response and the uid in the delta are the same
    /// node by construction, whichever way the caller aimed.
    pub uid: Option<String>,
    pub role: Option<String>,
    pub name: Option<String>,
}

impl Dispatched {
    const fn bare(delivery: Delivery, sent: bool) -> Self {
        Self {
            delivery,
            sent,
            aim: None,
            receiver: None,
            uid: None,
            role: None,
            name: None,
        }
    }

    /// A JS `click()`/`MouseEvent`: no hit test happened and none could have.
    #[must_use]
    pub const fn js() -> Self {
        Self::bare(Delivery::JsDispatch, true)
    }

    #[must_use]
    pub fn landed(delivery: Delivery, aim: (f64, f64), receiver: Option<Hit>) -> Self {
        Self { aim: Some(aim), receiver, ..Self::bare(delivery, true) }
    }

    /// Aimed, refused, nothing sent. Keeps the receiver so the refusal can name it.
    #[must_use]
    pub fn skipped(delivery: Delivery, aim: (f64, f64), receiver: Option<Hit>) -> Self {
        Self { aim: Some(aim), receiver, ..Self::bare(delivery, false) }
    }

    /// Carry over the identity of the node an action resolved for itself.
    #[must_use]
    pub fn named(mut self, uid: Option<String>, role: Option<String>, name: Option<String>) -> Self {
        self.uid = uid;
        self.role = role;
        self.name = name;
        self
    }

    /// The fields this outcome contributes to the response.
    #[must_use]
    pub fn report(&self) -> Value {
        let mut out = json!({"delivery": self.delivery.as_str()});
        if let Some(uid) = &self.uid {
            out["uid"] = json!(uid);
        }
        if let Some(role) = &self.role {
            out["role"] = json!(role);
        }
        if let Some(name) = &self.name {
            out["name"] = json!(name);
        }
        if let Some((x, y)) = self.aim {
            out["aim"] = json!([x, y]);
        }
        if let Some(receiver) = &self.receiver {
            out["intercepted_by"] = receiver.report();
            // Written here rather than left to the verdict's generic hint: this one can name
            // the element, and an agent that has to guess which overlay to deal with is back
            // to spending a turn finding out.
            out["verdict_hint"] = json!(format!(
                "The event was aimed at the target's centre and {} occupies that point, so it \
                 received the event instead. Deal with it first (dismiss the banner or scrim, \
                 close the dialog), then repeat this action. Nothing is known about what the \
                 target itself would have done.",
                receiver.describe()
            ));
        }
        out
    }

    /// The message for an action that never dispatched, or `None` when it did.
    ///
    /// "Clicked" would be false for both refusals, and a false message is what the change
    /// report cannot undo.
    #[must_use]
    pub fn refusal_message(&self, verb: &str, target: &str) -> Option<String> {
        if self.sent {
            return None;
        }
        Some(match self.delivery {
            Delivery::NotSettled => format!(
                "Did not {verb} {target}: the aim point was still moving, or outside the \
                 viewport, after {}ms of settling, so nothing was dispatched.",
                u64::from(SETTLE_ATTEMPTS) * SETTLE_GAP_MS
            ),
            Delivery::OffTarget => format!(
                "Did not {verb} {target}: no point inside the element's own boxes could be \
                 aimed at, so nothing was dispatched."
            ),
            _ => format!(
                "Did not {verb} {target}: {} occupies the point it would have been aimed at, \
                 and --on-intercept refuse was set.",
                self.receiver.as_ref().map_or_else(
                    || "another element".to_string(),
                    Hit::describe
                )
            ),
        })
    }
}

/// A handle to the single node a selector resolved to, plus how it will be reported.
///
/// One `querySelector`, one `objectId`, used for the probe, the dispatch and the response's
/// uid. The three used to be independent evaluations, so a re-render between them could bind
/// three different nodes while the response described one.
pub struct SelectorHandle {
    pub object_id: String,
    pub uid: Option<String>,
    pub role: Option<String>,
    pub name: Option<String>,
}

pub async fn resolve_selector(
    client: &CdpClient,
    selector: &str,
) -> Result<SelectorHandle, ElementError> {
    let expression = format!(
        "document.querySelector({})",
        serde_json::to_string(selector).unwrap_or_default()
    );
    let handle: Value = client
        .call("Runtime.evaluate", json!({"expression": expression}))
        .await
        .map_err(|e| ElementError::Action(format!("selector resolution failed: {e}")))?;
    let object_id = handle
        .get("result")
        .and_then(|r| r.get("objectId"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ElementError::NotFound(format!("No element matches selector: {selector}"))
        })?
        .to_string();
    let described = describe(client, &object_id).await;
    Ok(SelectorHandle {
        object_id,
        uid: described.as_ref().and_then(|d| d.uid.clone()),
        role: described.as_ref().and_then(|d| d.role.clone()),
        name: described.and_then(|d| d.name),
    })
}

/// The uid, and the cheapest honest (role, name) available for it.
pub struct Described {
    pub uid: Option<String>,
    pub role: Option<String>,
    pub name: Option<String>,
}

/// Describe a resolved handle: its uid, and role/name from the attributes the same call
/// already returns.
///
/// Best effort by construction, and deliberately not a second probe: `role` is the explicit
/// ARIA role or the tag name, `name` an accessible-name attribute if the element carries one.
/// Neither is the computed accessibility name — that lives in the snapshot, which is where a
/// caller that needs it should look.
pub async fn describe(client: &CdpClient, object_id: &str) -> Option<Described> {
    let described: Value = client
        .call("DOM.describeNode", json!({"objectId": object_id}))
        .await
        .ok()?;
    let node = described.get("node")?;
    let uid = node.get("backendNodeId").and_then(Value::as_i64).map(|id| format!("n{id}"));
    let attributes = attribute_pairs(node);
    let role = attributes
        .iter()
        .find(|(k, _)| k == "role")
        .map(|(_, v)| v.clone())
        .or_else(|| node.get("localName").and_then(Value::as_str).map(str::to_string))
        .filter(|s| !s.is_empty());
    let name = ["aria-label", "title", "alt", "placeholder", "name"]
        .iter()
        .find_map(|key| attributes.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()))
        .filter(|s| !s.is_empty());
    Some(Described { uid, role, name })
}

/// `DOM.describeNode` returns attributes as a flat `[name, value, name, value, …]` list.
fn attribute_pairs(node: &Value) -> Vec<(String, String)> {
    node.get("attributes")
        .and_then(Value::as_array)
        .map(|flat| {
            flat.chunks_exact(2)
                .filter_map(|pair| {
                    Some((pair[0].as_str()?.to_string(), pair[1].as_str()?.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Frozen probe readings, exactly as the JS returns them. The classifier is the half of
    /// the hit test that has to be right on shapes that are painful to reproduce live.
    fn probe(json: &str) -> Probe {
        serde_json::from_str(json).expect("probe JSON")
    }

    const CLEAN: &str = r#"{"rendered":true,"inViewport":true,"aimIn":true,"landed":true,
        "depth":0,"offsetKnown":true,"aim":[200,130],"top":[200,130],
        "hit":{"tag":"BUTTON","id":"target","cls":null,"z":"auto","text":"Underneath",
        "modal":false,"iframe":false,"sameDoc":true}}"#;

    #[test]
    fn a_clean_hit_is_the_only_reading_that_licenses_no_effect() {
        assert_eq!(classify(&probe(CLEAN)), Delivery::TargetHit);
    }

    /// The canonical false success: the aim point is on the target's own box, and another
    /// element is what sits there.
    #[test]
    fn an_element_over_the_aim_point_is_the_receiver() {
        let covered = probe(
            r#"{"rendered":true,"inViewport":true,"aimIn":true,"landed":false,
            "depth":0,"offsetKnown":true,"aim":[200,130],"top":[200,130],
            "hit":{"tag":"DIV","id":"scrim","cls":null,"z":"auto","text":"",
            "modal":false,"iframe":false,"sameDoc":true}}"#,
        );
        assert_eq!(classify(&covered), Delivery::Intercepted);
        assert_eq!(covered.hit.as_ref().unwrap().describe(), "div#scrim");
    }

    /// A `<dialog>` in the top layer is still an interception; the classifier does not care,
    /// the reason token does (`verdict::classify` reads `modal`).
    #[test]
    fn a_modal_receiver_is_carried_on_the_hit_not_the_delivery() {
        let backdrop = probe(
            r#"{"rendered":true,"inViewport":true,"aimIn":true,"landed":false,
            "depth":0,"offsetKnown":true,"aim":[200,130],"top":[200,130],
            "hit":{"tag":"DIALOG","id":"terms","cls":null,"z":"auto","text":"Terms",
            "modal":true,"iframe":false,"sameDoc":true}}"#,
        );
        assert_eq!(classify(&backdrop), Delivery::Intercepted);
        assert!(backdrop.hit.expect("receiver").modal);
    }

    /// The smooth-scroll reading. Refusing to dispatch is the point: the alternative is an
    /// event sent to a coordinate the target has already left.
    #[test]
    fn an_aim_point_outside_the_viewport_is_never_a_hit() {
        let mid_scroll = probe(
            r#"{"rendered":true,"inViewport":false,"aimIn":true,"landed":false,
            "depth":0,"offsetKnown":true,"aim":[200,3028],"top":[200,3028],"hit":null}"#,
        );
        assert_eq!(classify(&mid_scroll), Delivery::NotSettled);
    }

    /// Ordering matters here: an off-viewport point must not be reported as an interception
    /// just because `elementFromPoint` returned nothing.
    #[test]
    fn not_settled_outranks_every_other_reading() {
        let mid_scroll = probe(
            r#"{"rendered":true,"inViewport":false,"aimIn":false,"landed":false,
            "depth":3,"offsetKnown":false,"aim":[200,3028],"top":[200,3028],
            "hit":{"tag":"DIV","id":"scrim","cls":null,"z":"9","text":"","modal":false,
            "iframe":false,"sameDoc":true}}"#,
        );
        assert_eq!(classify(&mid_scroll), Delivery::NotSettled);
    }

    #[test]
    fn a_point_in_the_gap_between_line_boxes_is_off_target_not_intercepted() {
        let wrapped = probe(
            r#"{"rendered":true,"inViewport":true,"aimIn":false,"landed":false,
            "depth":0,"offsetKnown":true,"aim":[300,90],"top":[300,90],
            "hit":{"tag":"P","id":"prose","cls":null,"z":"auto","text":"a link that wraps",
            "modal":false,"iframe":false,"sameDoc":true}}"#,
        );
        assert_eq!(classify(&wrapped), Delivery::OffTarget);
    }

    /// A target inside a frame gets a correct aim point and no claim: the parent's overlays
    /// are invisible from the frame's own document, so a clean reading there proves nothing.
    #[test]
    fn a_target_inside_a_frame_is_aimed_at_but_never_judged() {
        let inside = probe(
            r#"{"rendered":true,"inViewport":true,"aimIn":true,"landed":true,
            "depth":1,"offsetKnown":true,"aim":[40,20],"top":[52,132],
            "hit":{"tag":"BUTTON","id":"buy","cls":null,"z":"auto","text":"Buy",
            "modal":false,"iframe":false,"sameDoc":true}}"#,
        );
        assert_eq!(classify(&inside), Delivery::NotProbed);
        assert_eq!(inside.top, Some([52.0, 132.0]), "the dispatch point is still mapped");
    }

    /// An unmappable coordinate must not be turned into a claim either way.
    #[test]
    fn an_unknown_frame_offset_makes_no_claim() {
        let cross_origin = probe(
            r#"{"rendered":true,"inViewport":true,"aimIn":true,"landed":false,
            "depth":1,"offsetKnown":false,"aim":[40,20],"top":[40,20],
            "hit":{"tag":"DIV","id":"veil","cls":null,"z":"5","text":"","modal":false,
            "iframe":false,"sameDoc":true}}"#,
        );
        assert_eq!(classify(&cross_origin), Delivery::NotProbed);
    }

    /// A zero-size element has no point to aim at. The absence is encoded as an absence, and
    /// the caller falls back to a JS click.
    #[test]
    fn an_element_with_no_box_yields_no_claim() {
        assert_eq!(classify(&probe(r#"{"rendered":false}"#)), Delivery::NotProbed);
    }

    /// The settle loop compares points, and a page still scrolling moves much further than
    /// the epsilon between two readings.
    #[test]
    fn point_equality_tolerates_subpixel_layout_but_not_a_moving_scroll() {
        assert!(same_point(Some([100.0, 200.0]), Some([100.2, 199.9])));
        assert!(!same_point(Some([100.0, 200.0]), Some([100.0, 260.0])));
        assert!(!same_point(None, Some([100.0, 200.0])), "an absent reading agrees with nothing");
    }

    #[test]
    fn a_receiver_is_named_the_way_it_appears_in_the_page() {
        let hit = Hit {
            tag: "DIV".into(),
            id: Some("cookie-banner".into()),
            cls: Some("veil is-open extra".into()),
            z: Some("9999".into()),
            text: "We use cookies".into(),
            modal: false,
            iframe: false,
            same_doc: true,
            uid: None,
        };
        // Two classes at most: the point is to be findable, not to reproduce the attribute.
        assert_eq!(hit.describe(), "div#cookie-banner.veil.is-open");
    }

    #[test]
    fn on_intercept_refuses_an_unknown_policy() {
        assert_eq!(OnIntercept::parse("dispatch"), Ok(OnIntercept::Dispatch));
        assert_eq!(OnIntercept::parse("refuse"), Ok(OnIntercept::Refuse));
        assert!(OnIntercept::parse("maybe").is_err());
        // A per-command override, and the session policy when there is none.
        assert_eq!(
            OnIntercept::from_cmd(&json!({"on_intercept": "refuse"}), OnIntercept::Dispatch),
            OnIntercept::Refuse
        );
        assert_eq!(
            OnIntercept::from_cmd(&json!({"cmd": "click"}), OnIntercept::Refuse),
            OnIntercept::Refuse
        );
    }

    /// An action that did not dispatch must not answer "Clicked".
    #[test]
    fn a_refusal_says_what_it_did_not_do() {
        let not_settled = Dispatched::skipped(Delivery::NotSettled, (10.0, 20.0), None);
        let msg = not_settled.refusal_message("click", "uid=n9").expect("a refusal message");
        assert!(msg.starts_with("Did not click uid=n9"), "{msg}");
        assert!(msg.contains("150ms"), "the settle budget is stated: {msg}");
        assert!(
            Dispatched::landed(Delivery::TargetHit, (10.0, 20.0), None)
                .refusal_message("click", "uid=n9")
                .is_none()
        );
    }

    /// `DOM.describeNode` hands attributes back as one flat list; a pair-wise read of it is
    /// the difference between a role and a value.
    #[test]
    fn attributes_are_read_as_pairs() {
        let node = json!({"attributes": ["id", "go", "role", "button", "aria-label", "Go now"]});
        let pairs = attribute_pairs(&node);
        assert_eq!(pairs.len(), 3);
        assert_eq!(pairs[1], ("role".to_string(), "button".to_string()));
        assert!(attribute_pairs(&json!({})).is_empty());
    }

    /// The response is what the classifier reads back, so the delivery token and the receiver
    /// must both survive the trip.
    #[test]
    fn the_report_carries_the_receiver_and_a_hint_that_names_it() {
        let hit = Hit {
            tag: "DIV".into(),
            id: Some("scrim".into()),
            cls: None,
            z: Some("auto".into()),
            text: String::new(),
            modal: false,
            iframe: false,
            same_doc: true,
            uid: Some("n11".into()),
        };
        let report = Dispatched::landed(Delivery::Intercepted, (200.0, 130.0), Some(hit)).report();
        assert_eq!(report["delivery"], "intercepted");
        assert_eq!(report["intercepted_by"]["id"], "scrim");
        assert_eq!(report["intercepted_by"]["uid"], "n11");
        assert_eq!(report["aim"], json!([200.0, 130.0]));
        assert!(
            report["verdict_hint"].as_str().unwrap().contains("div#scrim"),
            "the hint has to name the receiver: {report}"
        );
    }
}
