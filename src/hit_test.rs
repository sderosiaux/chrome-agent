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
    /// Dispatch through a receiver that looks like static content in the way; refuse a receiver
    /// that looks like it does something (`Hit::looks_inert`). Neither extreme is right for
    /// every caller: `dispatch` sent a click through lequipe.fr's cookie-consent "accept" button
    /// while aiming at unrelated navigation and accepted the wall on the caller's behalf, and
    /// `refuse` would also stop on the five of eight measured interceptions that were a
    /// `HEADER`, plain text, an image, or an inert iframe — none of which needed re-planning.
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

    /// The per-command override, falling back to the session's policy.
    #[must_use]
    pub fn from_cmd(cmd: &Value, fallback: Self) -> Self {
        cmd.get("on_intercept")
            .and_then(Value::as_str)
            .and_then(|v| Self::parse(v).ok())
            .unwrap_or(fallback)
    }
}

/// Whether an interception should be refused under `on_intercept`, given what the probe found
/// occupying the aim point.
///
/// `Refuse` always refuses and `Dispatch` never does — this is the one place `Guard` differs:
/// it refuses unless the receiver is POSITIVELY known and looks inert (`Hit::looks_inert`).
/// `None` (no receiver was identified, which `aim` treats as possible though rare) and an
/// `<iframe>` receiver (opaque from here, see [`Hit::looks_inert`]) both fail that test and so
/// both refuse — the same "unknown leans toward caution" rule applied twice, once for a
/// specific structural case and once for the absence of any reading at all.
#[must_use]
pub fn should_refuse_intercept(on_intercept: OnIntercept, receiver: Option<&Hit>) -> bool {
    match on_intercept {
        OnIntercept::Refuse => true,
        OnIntercept::Dispatch => false,
        OnIntercept::Guard => !receiver.is_some_and(Hit::looks_inert),
    }
}

/// How long the settle loop waits between readings, and how many it takes.
///
/// 5 × 30 ms is enough for a layout that settles (a sticky header reflowing, an
/// `IntersectionObserver` inserting content above the target) and deliberately NOT enough to
/// sit out a long animation: a caller who needs that has `wait`, and silently absorbing
/// seconds inside every click is worse than saying the aim never settled.
pub const SETTLE_ATTEMPTS: u32 = 5;
pub const SETTLE_GAP_MS: u64 = 30;

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
// Each flag answers an independent question the probe measured (in the top layer, an iframe,
// same document, structurally interactive); collapsing them into an enum would force an order
// on facts nothing here orders — `Probe` above takes the same allowance for the same reason.
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
    /// A native interactive tag, an ARIA interactive role, explicit keyboard focusability
    /// (`tabIndex >= 0`), or a `cursor: pointer` computed style — computed inside the same probe
    /// call that already runs (`PROBE_JS`), so this costs no extra CDP round trip. Read through
    /// [`Hit::looks_inert`], never alone: an `<iframe>` ignores this field entirely (see there).
    ///
    /// Deliberately NOT a keyword match against `text`/`class` ("accept", "agree", "j'accepte",
    /// the CMP vendor's own class fragments): measured against eight real interceptions, `class`
    /// and `text` were what let a PERSON recognise a consent button on sight, but a keyword list
    /// is never complete, never covers every language, and is exactly the kind of site-specific
    /// pattern-matching this project avoids elsewhere. A structural signal generalises; a
    /// wordlist accumulates exceptions forever.
    #[serde(default)]
    pub actionable: bool,
    /// Resolved afterwards, best effort — an overlay container usually has no accessibility
    /// node, so it usually has no uid either.
    #[serde(skip)]
    pub uid: Option<String>,
}

impl Hit {
    /// `tag#id.class`, the shape a person can find in the page source.
    ///
    /// `pub(crate)` for two readers, both of which would otherwise send the caller to `inspect`
    /// for a fact the response already holds: the refusal that names this element, written in
    /// `hit_test_report`, and `hints` — a click-triggered download that produced nothing has one
    /// explanation the hit test already measured, which is that another element took the click.
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

    /// Whether `--on-intercept guard` may dispatch through this receiver.
    ///
    /// An `<iframe>` is always `false` here, whatever [`Hit::actionable`] says: its content is
    /// opaque from outside — cross-origin content refuses to answer at all, and same-origin
    /// would need a second execution context this probe does not open, which is a CDP call this
    /// project does not spend on every intercepted click to resolve a case measured at one in
    /// eight. Between a false refusal on an inert search-box iframe and a false dispatch into an
    /// unseen consent wall, this project accepts the former: the receiver is genuinely unknown,
    /// and `Guard`'s whole premise is to lean on "unknown" rather than guess through it.
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
  // Structural interactivity, not a wordlist: a native interactive tag, an ARIA interactive
  // role, explicit keyboard focusability, or a pointer cursor — none perfect alone (a
  // decorative hover-effect wrapper reads as actionable; a bare div with a click listener and
  // none of these reads as inert), computed here so it costs no extra round trip.
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

/// Whether the aim point stopped moving, which is the one thing a single reading cannot say.
///
/// [`aim`] owns the loop and hands the answer here, because the two failures it separates are
/// identical in a single probe and opposite in what the caller should do: a point still moving
/// will be somewhere else in a moment (repeat), a point that has stopped and is still off
/// screen will be in exactly the same place next time (look at the page).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settle {
    /// Two consecutive readings agreed to within [`SAME_POINT_EPSILON`], or the first reading
    /// was already inside the viewport and nothing had to be waited for.
    Converged,
    /// The readings still disagreed when the settle budget ran out.
    Moving,
}

/// Turn one probe reading into a delivery. Pure; the temporal half comes in as [`Settle`],
/// measured by [`aim`], which owns the loop.
///
/// The off-viewport rung is why `settle` is a parameter rather than an inference. A point
/// outside the viewport used to be `not_settled` unconditionally, which reads as "wait and it
/// will be fine" — and on a consent wall in `position: fixed` over a document whose scroll is
/// locked, it never is: the same coordinate came back seven times, the verdict said `retry`
/// each time, and the retry is the loop. Two readings that AGREE about a point off screen are
/// a measurement, not an unfinished one, so they report `off_target` — which is the reading
/// whose `next` is `inspect`.
#[must_use]
pub const fn classify(probe: &Probe, settle: Settle) -> Delivery {
    if !probe.rendered {
        // Nothing to aim at. The caller falls back to a JS click and records that instead.
        return Delivery::NotProbed;
    }
    if !probe.in_viewport {
        return match settle {
            // Stable and out of reach: no scroll this tool can perform will change it.
            Settle::Converged => Delivery::OffTarget,
            Settle::Moving => Delivery::NotSettled,
        };
    }
    // In the viewport but still moving: the smooth-scroll case with a smaller offset, and the
    // reason a point being on screen is not on its own enough to dispatch at.
    if matches!(settle, Settle::Moving) {
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
    At {
        point: (f64, f64),
        delivery: Delivery,
        receiver: Option<Hit>,
        /// Why nothing can be aimed at, when that is the reading. `Delivery::OffTarget` covers
        /// two shapes with one token; this is which of them was measured.
        unaimable: Option<Unaimable>,
    },
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
    // Two independent facts, and collapsing them into one flag is what made a permanent miss
    // read as a temporary one: the loop only ever exited on `in_viewport && agreed`, so a
    // point that agreed with itself five times over while sitting off screen came out as
    // "never settled". Convergence is about the readings, the viewport is about the page.
    let mut settle = if probe.in_viewport { Settle::Converged } else { Settle::Moving };
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
        settle = if same_point(previous, next.aim) { Settle::Converged } else { Settle::Moving };
        probe = next;
        attempts += 1;
    }

    // The mapping failed, so the point we measured cannot be expressed in the coordinates a
    // dispatch takes. Better to fall back to the box model than to click somewhere arbitrary.
    let Some(top) = probe.top.filter(|_| probe.offset_known) else {
        return Aim::Unprobed;
    };
    let delivery = classify(&probe, settle);
    // Which shape of `off_target` this is. On screen and unaimable is a layout the caller can
    // aim around; off screen and unaimable is the page holding the element there.
    let unaimable = match delivery {
        Delivery::OffTarget if probe.in_viewport => Some(Unaimable::NoBoxToAimAt),
        Delivery::OffTarget => Some(Unaimable::StableOffViewport),
        _ => None,
    };
    let mut receiver = if delivery == Delivery::Intercepted { probe.hit.clone() } else { None };
    if let Some(hit) = receiver.as_mut() {
        hit.uid = receiver_uid(client, object_id).await;
    }
    Aim::At { point: (top[0], top[1]), delivery, receiver, unaimable }
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

/// What the response says about all this: moved to `hit_test_report` for the 1000-line file
/// cap and re-exported here, so a caller still writes `crate::hit_test::Dispatched` beside the
/// probe that produced it.
pub use crate::hit_test_report::{Dispatched, Refused, Unaimable};

/// The structured refusal inside an error, when it is one.
///
/// The three error boundaries (`main`, `pipe::dispatch`, `pipe_dispatch::dispatch_single`) each
/// hold a `BoxError` and print `{"ok":false,"error":…}` from its `Display`. This is how they
/// ask whether the thing they are about to flatten was measured rather than merely worded.
#[must_use]
pub fn refusal_in(error: &crate::BoxError) -> Option<&Refused> {
    match error.downcast_ref::<ElementError>() {
        Some(ElementError::Refused(refused)) => Some(refused),
        _ => None,
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
        assert_eq!(classify(&probe(CLEAN), Settle::Converged), Delivery::TargetHit);
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
        assert_eq!(classify(&covered, Settle::Converged), Delivery::Intercepted);
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
        assert_eq!(classify(&backdrop, Settle::Converged), Delivery::Intercepted);
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
        assert_eq!(classify(&mid_scroll, Settle::Moving), Delivery::NotSettled);
        assert_ne!(classify(&mid_scroll, Settle::Converged), Delivery::TargetHit);
    }

    /// The reading this fix is about. The same probe, twice, differing only in whether the
    /// point was still moving — and the two answers have opposite `next` steps.
    ///
    /// Measured on a real consent wall: `(378, -14)` on seven attempts, three of them on a
    /// fresh profile, identical to the pixel. Reported as `not_settled` → `unknown /
    /// scroll_not_settled` → `next: retry`, which for a `position: fixed` container over a
    /// document whose scroll is locked is an instruction to loop forever.
    #[test]
    fn a_point_that_stopped_moving_off_screen_is_off_target_not_unsettled() {
        let pinned = probe(
            r#"{"rendered":true,"inViewport":false,"aimIn":true,"landed":false,
            "depth":0,"offsetKnown":true,"aim":[378,-14],"top":[378,-14],"hit":null}"#,
        );
        assert_eq!(classify(&pinned, Settle::Converged), Delivery::OffTarget);
        assert_eq!(classify(&pinned, Settle::Moving), Delivery::NotSettled);
    }

    /// In the viewport is not enough on its own: a point that is on screen and still moving is
    /// the smooth-scroll bug with a smaller offset, and dispatching at it is the same mistake.
    #[test]
    fn a_point_still_moving_is_refused_even_inside_the_viewport() {
        assert_eq!(classify(&probe(CLEAN), Settle::Moving), Delivery::NotSettled);
    }

    /// Ordering matters here: an off-viewport point must not be reported as an interception
    /// just because `elementFromPoint` returned nothing, nor as a frame's unprobed target.
    #[test]
    fn a_refusal_to_aim_outranks_every_other_reading() {
        let mid_scroll = probe(
            r#"{"rendered":true,"inViewport":false,"aimIn":false,"landed":false,
            "depth":3,"offsetKnown":false,"aim":[200,3028],"top":[200,3028],
            "hit":{"tag":"DIV","id":"scrim","cls":null,"z":"9","text":"","modal":false,
            "iframe":false,"sameDoc":true}}"#,
        );
        assert_eq!(classify(&mid_scroll, Settle::Moving), Delivery::NotSettled);
        assert_eq!(classify(&mid_scroll, Settle::Converged), Delivery::OffTarget);
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
        assert_eq!(classify(&inside, Settle::Converged), Delivery::NotProbed);
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
        assert_eq!(classify(&cross_origin, Settle::Converged), Delivery::NotProbed);
    }

    /// A zero-size element has no point to aim at. The absence is encoded as an absence, and
    /// the caller falls back to a JS click.
    #[test]
    fn an_element_with_no_box_yields_no_claim() {
        assert_eq!(classify(&probe(r#"{"rendered":false}"#), Settle::Converged), Delivery::NotProbed);
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
            actionable: false,
            uid: None,
        };
        // Two classes at most: the point is to be findable, not to reproduce the attribute.
        assert_eq!(hit.describe(), "div#cookie-banner.veil.is-open");
    }

    #[test]
    fn on_intercept_refuses_an_unknown_policy() {
        assert_eq!(OnIntercept::parse("dispatch"), Ok(OnIntercept::Dispatch));
        assert_eq!(OnIntercept::parse("refuse"), Ok(OnIntercept::Refuse));
        assert_eq!(OnIntercept::parse("guard"), Ok(OnIntercept::Guard));
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

    /// The eight receivers measured on lequipe.fr/lefigaro.fr/vinted.fr, reduced to their two
    /// families: five inert (a HEADER, two text DIVs, an IMG, a search iframe) and three
    /// actionable (a consent BUTTON, a CMP iframe, a country-selector cell) — `z_index` and
    /// `modal` carried nothing on any of the eight, so neither appears here.
    #[test]
    fn looks_inert_separates_the_two_measured_families() {
        assert!(hit("HEADER", false, false).looks_inert());
        assert!(hit("DIV", false, false).looks_inert(), "plain banner text");
        assert!(hit("IMG", false, false).looks_inert());
        assert!(!hit("BUTTON", false, true).looks_inert(), "consent accept button");
        assert!(!hit("DIV", false, true).looks_inert(), "a div acting as a selector option");
        // An iframe is inert here (a search box) but `looks_inert` still refuses it: content is
        // opaque from outside, so "inert" cannot be measured, only assumed.
        assert!(!hit("IFRAME", true, false).looks_inert(), "opaque content refuses regardless");
        assert!(!hit("IFRAME", true, true).looks_inert(), "a CMP iframe, doubly so");
    }

    #[test]
    fn should_refuse_intercept_only_guards_what_looks_actionable() {
        let inert = hit("DIV", false, false);
        let actionable = hit("BUTTON", false, true);
        let unknown_iframe = hit("IFRAME", true, false);

        assert!(!should_refuse_intercept(OnIntercept::Dispatch, Some(&inert)));
        assert!(!should_refuse_intercept(OnIntercept::Dispatch, Some(&actionable)));
        assert!(should_refuse_intercept(OnIntercept::Refuse, Some(&inert)));
        assert!(should_refuse_intercept(OnIntercept::Refuse, Some(&actionable)));

        assert!(!should_refuse_intercept(OnIntercept::Guard, Some(&inert)));
        assert!(should_refuse_intercept(OnIntercept::Guard, Some(&actionable)));
        assert!(should_refuse_intercept(OnIntercept::Guard, Some(&unknown_iframe)));
        // No receiver identified at all: still refuse under Guard rather than assume inert.
        assert!(should_refuse_intercept(OnIntercept::Guard, None));
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
}
