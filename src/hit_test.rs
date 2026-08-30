//! Where a mouse event will land, measured before it is dispatched.
//!
//! One `Runtime.callFunctionOn` bound to the target's `objectId`, never a bare
//! `Runtime.evaluate`: on a handle inside a CLOSED shadow root, `this.getRootNode().host` works,
//! which is what makes the containment climb correct without `DOM.getNodeForLocation`.
//!
//! `document.elementFromPoint` is evidence, not the browser's own input hit test — top layer,
//! compositor scroll offsets and in-flight transforms can diverge. Hence the default dispatches.

use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::cdp::client::CdpClient;
use crate::element::{ElementError, check_js_exception};
use crate::verdict::Delivery;

/// What to do when the aim point turns out to belong to another element.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OnIntercept {
    /// Dispatch anyway and say who received it, as a pointer would.
    #[default]
    Dispatch,
    /// Refuse the action without dispatching.
    Refuse,
    /// Dispatch through a receiver that looks like static content; refuse one that looks like a
    /// control (`Hit::looks_inert`).
    Guard,
}

impl OnIntercept {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "dispatch" => Ok(Self::Dispatch),
            "refuse" => Ok(Self::Refuse),
            "guard" => Ok(Self::Guard),
            other => Err(format!(
                "Unknown --on-intercept value '{other}'. Use \"dispatch\" (default), \"refuse\", or \"guard\"."
            )),
        }
    }
}

/// `Refuse` always refuses, `Dispatch` never does, `Guard` refuses unless the receiver is
/// positively known and inert ([`Hit::looks_inert`]) — so `None` and an `<iframe>` both refuse.
#[must_use]
pub fn should_refuse_intercept(on_intercept: OnIntercept, receiver: Option<&Hit>) -> bool {
    match on_intercept {
        OnIntercept::Refuse => true,
        OnIntercept::Dispatch => false,
        OnIntercept::Guard => !receiver.is_some_and(Hit::looks_inert),
    }
}

/// Settle budget: 5 × 30 ms. Enough for a layout that settles (a reflowing sticky header, an
/// `IntersectionObserver`), deliberately not enough to sit out an animation — that is `wait`.
pub const SETTLE_ATTEMPTS: u32 = 5;
pub const SETTLE_GAP_MS: u64 = 30;

/// Two readings are the same point within half a CSS pixel; an animating scroll moves further.
const SAME_POINT_EPSILON: f64 = 0.5;

/// What the probe found at the point about to be dispatched at. Deserialized from the JS return
/// value, so [`classify`] can run on frozen JSON with no Chrome.
// One flag per independent question; an enum would force an order the classifier owns.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Deserialize)]
pub struct Probe {
    /// The node has at least one client rect. `false` on a zero-size or `display:none` element.
    #[serde(default)]
    pub rendered: bool,
    /// The aim point lies inside the hit-testing document's viewport.
    #[serde(default, rename = "inViewport")]
    pub in_viewport: bool,
    /// The aim point lies inside one of the target's own client rects. False when the bounding
    /// box centre falls in a gap (an inline link wrapped across two lines), which is why the
    /// LARGEST rect is aimed at rather than the bounding box.
    #[serde(default, rename = "aimIn")]
    pub aim_in: bool,
    /// The element at the aim point is the target, inside it, its label's control, or its host.
    #[serde(default)]
    pub landed: bool,
    /// How many frames up from the target to the top document.
    #[serde(default)]
    pub depth: u32,
    /// Whether the frame chain could be walked into the top-level coordinates
    /// `Input.dispatchMouseEvent` takes.
    #[serde(default, rename = "offsetKnown")]
    pub offset_known: bool,
    /// The aim point in the target document's own coordinates.
    #[serde(default)]
    pub aim: Option<[f64; 2]>,
    /// The same point in top-level viewport coordinates.
    #[serde(default)]
    pub top: Option<[f64; 2]>,
    /// The element at the aim point.
    #[serde(default)]
    pub hit: Option<Hit>,
}

/// The element the aim point resolved to.
// Independent facts, like `Probe` above; an enum would force an order nothing here has.
#[allow(clippy::struct_excessive_bools)]
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
    /// A native interactive tag, an ARIA interactive role, `tabIndex >= 0`, or a
    /// `cursor: pointer` computed style, measured inside `PROBE_JS` at no extra round trip.
    /// Structural on purpose, never a keyword match. Read through [`Hit::looks_inert`].
    #[serde(default)]
    pub actionable: bool,
    /// Resolved afterwards, best effort: an overlay usually has no accessibility node.
    #[serde(skip)]
    pub uid: Option<String>,
}

impl Hit {
    /// `tag#id.class`, the shape a person can find in the page source.
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

    /// Whether `--on-intercept guard` may dispatch through this receiver. An `<iframe>` is
    /// always `false`, whatever [`Hit::actionable`] says: its content is opaque from outside, so
    /// "inert" cannot be measured there, only assumed.
    #[must_use]
    pub const fn looks_inert(&self) -> bool {
        !self.iframe && !self.actionable
    }

    pub(crate) fn report(&self) -> Value {
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
            "actionable": self.actionable,
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
    // 'instant', never the page's scroll-behavior: `smooth` makes this an animation and every
    // rect read below would report the pre-scroll position.
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
  // Local -> top-level coordinates: Input.dispatchMouseEvent takes the top-level viewport,
  // getClientRects inside a frame does not.
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
    // A cross-origin ancestor: the offset is unknowable from here.
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
  // Structural, not a wordlist. Imperfect: a hover wrapper reads actionable, a bare div with a
  // click listener reads inert.
  const INTERACTIVE_TAGS = ['BUTTON', 'A', 'INPUT', 'SELECT', 'TEXTAREA', 'LABEL', 'OPTION', 'SUMMARY'];
  const INTERACTIVE_ROLES = ['button', 'link', 'checkbox', 'radio', 'menuitem', 'menuitemcheckbox',
    'menuitemradio', 'option', 'tab', 'switch', 'combobox'];
  const actionableOf = (el, style) => {
    if (INTERACTIVE_TAGS.includes(el.tagName)) return true;
    const role = (el.getAttribute('role') || '').toLowerCase();
    if (INTERACTIVE_ROLES.includes(role)) return true;
    if (typeof el.tabIndex === 'number' && el.tabIndex >= 0) return true;
    return style.cursor === 'pointer';
  };
  return {
    rendered: true, inViewport, aimIn, landed, depth, offsetKnown,
    aim: [cx, cy], top: [cx + ox, cy + oy],
    hit: h ? (() => {
      const style = window.getComputedStyle(h);
      return {
        tag: h.tagName,
        id: h.id || null,
        cls: (typeof h.className === 'string' && h.className) ? h.className : null,
        z: style.zIndex,
        text: (h.textContent || '').replace(/\s+/g, ' ').trim().slice(0, 40),
        modal: modalOf(h),
        iframe: h.tagName === 'IFRAME',
        sameDoc: h.ownerDocument === this.ownerDocument,
        actionable: actionableOf(h, style)
      };
    })() : null
  };
}";

/// Whether the aim point stopped moving — the one thing a single reading cannot say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settle {
    /// Two consecutive readings agreed within [`SAME_POINT_EPSILON`].
    Converged,
    /// Still disagreeing when the settle budget ran out.
    Moving,
}

/// Turn one probe reading into a delivery. Pure; the temporal half comes in as [`Settle`].
/// Readings that DISAGREE about a point off screen are `not_settled` (retry); readings that
/// AGREE are `off_target`, whose recovery is `inspect`, not another attempt.
#[must_use]
pub const fn classify(probe: &Probe, settle: Settle) -> Delivery {
    if !probe.rendered {
        // Nothing to aim at; the caller falls back to a JS click.
        return Delivery::NotProbed;
    }
    if !probe.in_viewport {
        return match settle {
            // Stable and out of reach: no scroll this tool can perform will change it.
            Settle::Converged => Delivery::OffTarget,
            Settle::Moving => Delivery::NotSettled,
        };
    }
    // On screen but still moving: the smooth-scroll case with a smaller offset.
    if matches!(settle, Settle::Moving) {
        return Delivery::NotSettled;
    }
    if !probe.aim_in {
        return Delivery::OffTarget;
    }
    // Inside a frame the hit test sees the frame's own document, never its parent's: an overlay
    // covering the <iframe> is invisible from here. The aim point stays, the claim does not.
    if probe.depth > 0 || !probe.offset_known {
        return Delivery::NotProbed;
    }
    if probe.landed {
        Delivery::TargetHit
    } else {
        Delivery::Intercepted
    }
}

/// The result of aiming at a node.
pub enum Aim {
    /// A point to dispatch at, in top-level coordinates, and what sits there.
    At {
        point: (f64, f64),
        delivery: Delivery,
        receiver: Option<Hit>,
        /// Which shape of `Delivery::OffTarget` was measured, when that is the reading.
        unaimable: Option<Unaimable>,
    },
    /// The node has no layout box at all.
    NoBox,
    /// The probe could not run, or could not map its own coordinates. The caller claims nothing.
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
    // Discarded on purpose: `None` here is `Aim::Unprobed`, which already has a defined recovery
    // (the box model's centre, or the JS fallback). The probe's script is ours, so a throw names
    // no page problem the caller could act on.
    if crate::element::js_exception(&result).is_some() {
        return None;
    }
    serde_json::from_value(result.get("result")?.get("value")?.clone()).ok()
}

/// Scroll the target into view, work out where a click would go, and say what would receive it,
/// without dispatching anything. Two readings must agree AND be in the viewport before the point
/// is trusted — the failure guarded against is a scroll still running when the rect is read.
///
/// Being in the viewport is not evidence of having stopped: a smooth scroll passes THROUGH the
/// viewport, and `scrollIntoView({behavior:'instant'})` throwing drops the probe onto a fallback
/// that honours the page's own `scroll-behavior`. So the second reading is always taken, and the
/// common path pays one extra round trip plus [`SETTLE_GAP_MS`].
pub async fn aim(client: &CdpClient, object_id: &str) -> Aim {
    let Some(mut probe) = probe_once(client, object_id).await else {
        return Aim::Unprobed;
    };
    if !probe.rendered {
        return Aim::NoBox;
    }
    // Two independent facts: convergence is about the readings, the viewport is about the page.
    // Collapsing them makes a permanent miss read as a temporary one — and one reading can only
    // ever speak to the second, so it starts as `Moving` whatever the viewport says.
    let mut settle = Settle::Moving;
    let mut attempts = 0;
    while !(settle == Settle::Converged && probe.in_viewport) && attempts < SETTLE_ATTEMPTS {
        let previous = probe.aim;
        tokio::time::sleep(Duration::from_millis(SETTLE_GAP_MS)).await;
        let Some(next) = probe_once(client, object_id).await else {
            break;
        };
        if !next.rendered {
            return Aim::NoBox;
        }
        settle = if same_point(previous, next.aim) {
            Settle::Converged
        } else {
            Settle::Moving
        };
        probe = next;
        attempts += 1;
    }

    // Unmappable: fall back to the box model rather than click at an arbitrary coordinate.
    let Some(top) = probe.top.filter(|_| probe.offset_known) else {
        return Aim::Unprobed;
    };
    let delivery = classify(&probe, settle);
    // On screen and unaimable is a layout to aim around; off screen is the page holding it there.
    let unaimable = match delivery {
        Delivery::OffTarget if probe.in_viewport => Some(Unaimable::NoBoxToAimAt),
        Delivery::OffTarget => Some(Unaimable::StableOffViewport),
        _ => None,
    };
    let mut receiver = if delivery == Delivery::Intercepted {
        probe.hit.clone()
    } else {
        None
    };
    if let Some(hit) = receiver.as_mut() {
        hit.uid = receiver_uid(client, object_id).await;
    }
    Aim::At {
        point: (top[0], top[1]),
        delivery,
        receiver,
        unaimable,
    }
}

/// The uid of the element that received the event, when it has one.
/// Two extra round trips, on the intercepted path only, best effort: scrims usually carry no
/// accessibility node, hence `intercepted_by` also carrying tag/id/class.
async fn receiver_uid(client: &CdpClient, object_id: &str) -> Option<String> {
    // Recomputed from the target's handle, so this point and the probe's cannot drift apart.
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

/// What the response says about all this. Defined in `hit_test_report`, re-exported here.
pub use crate::hit_test_report::{Dispatched, Refused, Unaimable};

/// The structured refusal inside an error, when it is one. The three error boundaries call this
/// before flattening a `BoxError` into `{"ok":false,"error":…}`.
#[must_use]
pub fn refusal_in(error: &crate::BoxError) -> Option<&Refused> {
    match error.downcast_ref::<ElementError>() {
        Some(ElementError::Refused(refused)) => Some(refused),
        _ => None,
    }
}

/// The single node a selector resolved to. One `querySelector`, one `objectId`, used for the
/// probe, the dispatch and the response's uid, so a re-render cannot bind three different nodes.
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
    // A throw still comes back with an `objectId` — for the THROWN object. Reading it here binds
    // the handle to a DOMException, and every later step (probe, dispatch, JS fallback) then
    // reports a type error about the exception instead of the malformed selector that caused it.
    check_js_exception(&handle).map_err(|thrown| {
        ElementError::Action(format!("Selector '{selector}' could not be used: {thrown}"))
    })?;
    let object_id = handle
        .get("result")
        .and_then(|r| r.get("objectId"))
        .and_then(Value::as_str)
        .ok_or_else(|| ElementError::NotFound(format!("No element matches selector: {selector}")))?
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

/// A resolved handle's uid, plus role/name from the attributes the same call already returns.
/// `role` is the explicit ARIA role or the tag name, `name` an accessible-name attribute. Neither
/// is the computed accessibility name; that lives in the snapshot.
pub async fn describe(client: &CdpClient, object_id: &str) -> Option<Described> {
    let described: Value = client
        .call("DOM.describeNode", json!({"objectId": object_id}))
        .await
        .ok()?;
    let node = described.get("node")?;
    let uid = node
        .get("backendNodeId")
        .and_then(Value::as_i64)
        .map(|id| format!("n{id}"));
    let attributes = attribute_pairs(node);
    let role = attributes
        .iter()
        .find(|(k, _)| k == "role")
        .map(|(_, v)| v.clone())
        .or_else(|| {
            node.get("localName")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .filter(|s| !s.is_empty());
    let name = ["aria-label", "title", "alt", "placeholder", "name"]
        .iter()
        .find_map(|key| {
            attributes
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
        })
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

    fn probe(json: &str) -> Probe {
        serde_json::from_str(json).expect("probe JSON")
    }

    const CLEAN: &str = r#"{"rendered":true,"inViewport":true,"aimIn":true,"landed":true,
        "depth":0,"offsetKnown":true,"aim":[200,130],"top":[200,130],
        "hit":{"tag":"BUTTON","id":"target","cls":null,"z":"auto","text":"Underneath",
        "modal":false,"iframe":false,"sameDoc":true}}"#;

    #[test]
    fn a_clean_hit_is_the_only_reading_that_licenses_no_effect() {
        assert_eq!(
            classify(&probe(CLEAN), Settle::Converged),
            Delivery::TargetHit
        );
    }

    #[test]
    fn an_element_over_the_aim_point_is_the_receiver() {
        let covered = probe(
            r#"{"rendered":true,"inViewport":true,"aimIn":true,"landed":false,
            "depth":0,"offsetKnown":true,"aim":[200,130],"top":[200,130],
            "hit":{"tag":"DIV","id":"scrim","cls":null,"z":"auto","text":"",
            "modal":false,"iframe":false,"sameDoc":true}}"#,
        );
        assert_eq!(classify(&covered, Settle::Converged), Delivery::Intercepted);
        assert_eq!(covered.hit.as_ref().unwrap().describe(), "div#scrim");
    }

    /// Only the reason token reads `modal` (`verdict::classify`); the classifier ignores it.
    #[test]
    fn a_modal_receiver_is_carried_on_the_hit_not_the_delivery() {
        let backdrop = probe(
            r#"{"rendered":true,"inViewport":true,"aimIn":true,"landed":false,
            "depth":0,"offsetKnown":true,"aim":[200,130],"top":[200,130],
            "hit":{"tag":"DIALOG","id":"terms","cls":null,"z":"auto","text":"Terms",
            "modal":true,"iframe":false,"sameDoc":true}}"#,
        );
        assert_eq!(
            classify(&backdrop, Settle::Converged),
            Delivery::Intercepted
        );
        assert!(backdrop.hit.expect("receiver").modal);
    }

    /// The smooth-scroll reading: the target has already left that coordinate.
    #[test]
    fn an_aim_point_outside_the_viewport_is_never_a_hit() {
        let mid_scroll = probe(
            r#"{"rendered":true,"inViewport":false,"aimIn":true,"landed":false,
            "depth":0,"offsetKnown":true,"aim":[200,3028],"top":[200,3028],"hit":null}"#,
        );
        assert_eq!(classify(&mid_scroll, Settle::Moving), Delivery::NotSettled);
        assert_ne!(
            classify(&mid_scroll, Settle::Converged),
            Delivery::TargetHit
        );
    }

    /// One probe, two `Settle` values, opposite `next` steps. Measured on a consent wall:
    /// `(378, -14)`, identical to the pixel on seven attempts.
    #[test]
    fn a_point_that_stopped_moving_off_screen_is_off_target_not_unsettled() {
        let pinned = probe(
            r#"{"rendered":true,"inViewport":false,"aimIn":true,"landed":false,
            "depth":0,"offsetKnown":true,"aim":[378,-14],"top":[378,-14],"hit":null}"#,
        );
        assert_eq!(classify(&pinned, Settle::Converged), Delivery::OffTarget);
        assert_eq!(classify(&pinned, Settle::Moving), Delivery::NotSettled);
    }

    /// On screen is not enough: a point still moving is the same case with a smaller offset.
    #[test]
    fn a_point_still_moving_is_refused_even_inside_the_viewport() {
        assert_eq!(
            classify(&probe(CLEAN), Settle::Moving),
            Delivery::NotSettled
        );
    }

    /// An off-viewport point outranks both the interception branch and the frame branch.
    #[test]
    fn a_refusal_to_aim_outranks_every_other_reading() {
        let mid_scroll = probe(
            r#"{"rendered":true,"inViewport":false,"aimIn":false,"landed":false,
            "depth":3,"offsetKnown":false,"aim":[200,3028],"top":[200,3028],
            "hit":{"tag":"DIV","id":"scrim","cls":null,"z":"9","text":"","modal":false,
            "iframe":false,"sameDoc":true}}"#,
        );
        assert_eq!(classify(&mid_scroll, Settle::Moving), Delivery::NotSettled);
        assert_eq!(
            classify(&mid_scroll, Settle::Converged),
            Delivery::OffTarget
        );
    }

    #[test]
    fn a_point_in_the_gap_between_line_boxes_is_off_target_not_intercepted() {
        let wrapped = probe(
            r#"{"rendered":true,"inViewport":true,"aimIn":false,"landed":false,
            "depth":0,"offsetKnown":true,"aim":[300,90],"top":[300,90],
            "hit":{"tag":"P","id":"prose","cls":null,"z":"auto","text":"a link that wraps",
            "modal":false,"iframe":false,"sameDoc":true}}"#,
        );
        assert_eq!(classify(&wrapped, Settle::Converged), Delivery::OffTarget);
    }

    /// The parent's overlays are invisible from the frame's own document.
    #[test]
    fn a_target_inside_a_frame_is_aimed_at_but_never_judged() {
        let inside = probe(
            r#"{"rendered":true,"inViewport":true,"aimIn":true,"landed":true,
            "depth":1,"offsetKnown":true,"aim":[40,20],"top":[52,132],
            "hit":{"tag":"BUTTON","id":"buy","cls":null,"z":"auto","text":"Buy",
            "modal":false,"iframe":false,"sameDoc":true}}"#,
        );
        assert_eq!(classify(&inside, Settle::Converged), Delivery::NotProbed);
        assert_eq!(
            inside.top,
            Some([52.0, 132.0]),
            "the dispatch point is still mapped"
        );
    }

    #[test]
    fn an_unknown_frame_offset_makes_no_claim() {
        let cross_origin = probe(
            r#"{"rendered":true,"inViewport":true,"aimIn":true,"landed":false,
            "depth":1,"offsetKnown":false,"aim":[40,20],"top":[40,20],
            "hit":{"tag":"DIV","id":"veil","cls":null,"z":"5","text":"","modal":false,
            "iframe":false,"sameDoc":true}}"#,
        );
        assert_eq!(
            classify(&cross_origin, Settle::Converged),
            Delivery::NotProbed
        );
    }

    /// A zero-size element has no point to aim at; the caller falls back to a JS click.
    #[test]
    fn an_element_with_no_box_yields_no_claim() {
        assert_eq!(
            classify(&probe(r#"{"rendered":false}"#), Settle::Converged),
            Delivery::NotProbed
        );
    }

    /// A page still scrolling moves much further than the epsilon between two readings.
    #[test]
    fn point_equality_tolerates_subpixel_layout_but_not_a_moving_scroll() {
        assert!(same_point(Some([100.0, 200.0]), Some([100.2, 199.9])));
        assert!(!same_point(Some([100.0, 200.0]), Some([100.0, 260.0])));
        assert!(
            !same_point(None, Some([100.0, 200.0])),
            "an absent reading agrees with nothing"
        );
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
            actionable: false,
            uid: None,
        };
        // Two classes at most: findable, not a reproduction of the attribute.
        assert_eq!(hit.describe(), "div#cookie-banner.veil.is-open");
    }

    #[test]
    fn on_intercept_refuses_an_unknown_policy() {
        assert_eq!(OnIntercept::parse("dispatch"), Ok(OnIntercept::Dispatch));
        assert_eq!(OnIntercept::parse("refuse"), Ok(OnIntercept::Refuse));
        assert_eq!(OnIntercept::parse("guard"), Ok(OnIntercept::Guard));
        assert!(OnIntercept::parse("maybe").is_err());
    }

    fn hit(tag: &str, iframe: bool, actionable: bool) -> Hit {
        Hit {
            tag: tag.into(),
            id: None,
            cls: None,
            z: None,
            text: String::new(),
            modal: false,
            iframe,
            same_doc: true,
            actionable,
            uid: None,
        }
    }

    /// The two measured families: inert (HEADER, text DIV, IMG) and actionable (consent BUTTON,
    /// CMP iframe, selector cell).
    #[test]
    fn looks_inert_separates_the_two_measured_families() {
        assert!(hit("HEADER", false, false).looks_inert());
        assert!(hit("DIV", false, false).looks_inert(), "plain banner text");
        assert!(hit("IMG", false, false).looks_inert());
        assert!(
            !hit("BUTTON", false, true).looks_inert(),
            "consent accept button"
        );
        assert!(
            !hit("DIV", false, true).looks_inert(),
            "a div acting as a selector option"
        );
        // An iframe may hold inert content, but that cannot be measured from outside.
        assert!(
            !hit("IFRAME", true, false).looks_inert(),
            "opaque content refuses regardless"
        );
        assert!(
            !hit("IFRAME", true, true).looks_inert(),
            "a CMP iframe, doubly so"
        );
    }

    #[test]
    fn should_refuse_intercept_only_guards_what_looks_actionable() {
        let inert = hit("DIV", false, false);
        let actionable = hit("BUTTON", false, true);
        let unknown_iframe = hit("IFRAME", true, false);

        assert!(!should_refuse_intercept(
            OnIntercept::Dispatch,
            Some(&inert)
        ));
        assert!(!should_refuse_intercept(
            OnIntercept::Dispatch,
            Some(&actionable)
        ));
        assert!(should_refuse_intercept(OnIntercept::Refuse, Some(&inert)));
        assert!(should_refuse_intercept(
            OnIntercept::Refuse,
            Some(&actionable)
        ));

        assert!(!should_refuse_intercept(OnIntercept::Guard, Some(&inert)));
        assert!(should_refuse_intercept(
            OnIntercept::Guard,
            Some(&actionable)
        ));
        assert!(should_refuse_intercept(
            OnIntercept::Guard,
            Some(&unknown_iframe)
        ));
        // No receiver identified at all: still refuse under Guard rather than assume inert.
        assert!(should_refuse_intercept(OnIntercept::Guard, None));
    }

    /// `DOM.describeNode` hands attributes back flat; only a pair-wise read tells name from value.
    #[test]
    fn attributes_are_read_as_pairs() {
        let node = json!({"attributes": ["id", "go", "role", "button", "aria-label", "Go now"]});
        let pairs = attribute_pairs(&node);
        assert_eq!(pairs.len(), 3);
        assert_eq!(pairs[1], ("role".to_string(), "button".to_string()));
        assert!(attribute_pairs(&json!({})).is_empty());
    }
}
