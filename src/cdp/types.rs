//! Rust shapes for the slice of the Chrome `DevTools` Protocol this tool speaks.
//!
//! **Why the `dead_code` allows here are per field and never per module.** This module carried
//! one `#[allow(dead_code)]` on `pub mod types;` justified as "CDP fields kept for serde
//! deserialization completeness", and that justification was wrong on every count. Serde ignores
//! unknown fields by default, so a field this tool does not read costs nothing to omit and buys
//! nothing to keep — completeness is not a deserialization requirement, it is a documentation
//! choice. Worse, `BoxModel`'s four unread fields are neither `Option` nor `#[serde(default)]`,
//! so keeping them TIGHTENS what Chrome must send rather than loosening it. And `MouseButton`
//! is `Serialize` only; nothing deserializes it, so no reading of "completeness" reaches it.
//!
//! The cost of the module-wide allow was that anything in this file could die unnoticed. It had:
//! 28 unread fields and 5 never-constructed enum variants, and the only place in the repository
//! that said anything about a right mouse button said something untrue. The allows below are
//! therefore attached to the individual item, each with the reason that item is still here — so
//! a field added tomorrow and never read warns on the next `cargo check`, and a reason that stops
//! being true is a line to delete rather than a blanket to hide under.
//!
//! Three reasons recur, named rather than repeated in full at each site:
//! - **envelope** — part of the CDP message frame itself, read by nothing today because this tool
//!   drives one session per connection; a reader would appear the day it drives several.
//! - **shape** — the response really carries this and a caller could want it; keeping it costs one
//!   `Option` and documents the protocol at the point of use.
//! - **pinned** — removing it would change behaviour or break a test outside this module.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Generic CDP wire protocol
// ---------------------------------------------------------------------------

/// Outgoing CDP request envelope.
#[derive(Debug, Serialize)]
pub struct CdpRequest {
    pub id: u64,
    pub method: &'static str,
    pub params: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// Incoming CDP message — either a response to a request or an async event.
///
/// `untagged` makes every field below load-bearing in one direction nobody expects: when a
/// message fails BOTH variants it is not a message with a missing field, it is a message with
/// no home, and `dispatch_loop` used to drop it silently. A response carries `id` and no
/// `method`, an event carries `method` and no `id`, so the discriminating fields never
/// mis-assign; the only way to fall out of the enum entirely is an optional field arriving with
/// a JSON type the struct did not declare. Every optional field here is therefore either
/// `Value` (which accepts anything) or a type CDP genuinely pins. See
/// `client::resolve_unreadable`, which now answers the waiting caller instead of leaving it to
/// time out.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum CdpMessage {
    Response(CdpResponse),
    Event(CdpEvent),
}

/// Response to a request we sent (matched by `id`).
#[derive(Debug, Deserialize)]
pub struct CdpResponse {
    pub id: u64,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<CdpError>,
    /// envelope — which `Target.attachToTarget` session answered. This tool sends `sessionId`
    /// on the way out (`CdpRequest::session_id`) and correlates the way back by `id` alone,
    /// which is unambiguous because ids are unique across sessions on one connection.
    #[allow(dead_code)]
    #[serde(default, rename = "sessionId")]
    pub session_id: Option<String>,
}

/// Protocol-level error attached to a response.
#[derive(Debug, Deserialize)]
pub struct CdpError {
    pub code: i64,
    pub message: String,
    /// shape — CDP's free-form detail beside `code`/`message`, read by nothing here:
    /// `client::call_within` reports `code` and `message`, and `Display` prints the same two.
    ///
    /// **Typed `Value`, and that is the point.** It was `Option<String>`. Every Chrome this tool
    /// has been run against sends a string there, so this is a robustness and diagnosis defect
    /// and NOT an observed bug — nothing in the field has been measured producing it. What makes
    /// it worth the change is the size of the consequence against the size of the cost: `data`
    /// is the only optional field on an incoming message whose JSON type the protocol does not
    /// pin, and had one arrived as an object, `CdpResponse` would have failed to deserialize,
    /// `CdpMessage` being `untagged` would then have failed `CdpEvent` too, and the message would
    /// have been dropped — an error Chrome answered in milliseconds reaching the caller half a
    /// minute later as "did not answer within 30s … raise --timeout if the page is merely slow".
    /// A `Value` cannot fail that way. This is the one instance we can name;
    /// `client::resolve_unreadable` covers the class, including whatever we have not thought of.
    #[allow(dead_code)]
    #[serde(default)]
    pub data: Option<Value>,
}

/// Async event pushed by Chrome.
#[derive(Debug, Clone, Deserialize)]
pub struct CdpEvent {
    pub method: String,
    #[serde(default)]
    pub params: Value,
    /// envelope — see `CdpResponse::session_id`. Events are broadcast to every subscriber and
    /// filtered on `method`; a filter on the session would need this.
    #[allow(dead_code)]
    #[serde(default, rename = "sessionId")]
    pub session_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Target domain
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTargetParams {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_window: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTargetResult {
    pub target_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetTargetsResult {
    pub target_infos: Vec<TargetInfo>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetInfo {
    pub target_id: String,
    #[serde(rename = "type")]
    pub target_type: String,
    pub title: String,
    pub url: String,
    /// shape — whether a debugger is already attached. `tabs`/`resolve_page_target` pick a
    /// target by type and url; nothing here refuses an already-attached one.
    #[allow(dead_code)]
    #[serde(default)]
    pub attached: bool,
    /// shape — the target that opened this one. A `window.open` popup is the case it names,
    /// and this tool has no verb that follows one.
    #[allow(dead_code)]
    #[serde(default)]
    pub opener_id: Option<String>,
    /// shape — the browser context (incognito or otherwise) the target lives in. Isolation
    /// here is done with one `--user-data-dir` per named browser, not with contexts.
    #[allow(dead_code)]
    #[serde(default)]
    pub browser_context_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Page domain
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NavigateParams {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub referrer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transition_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NavigateResult {
    /// shape — the frame that navigated. Deliberately not the source of document identity:
    /// `diff::Identity` reads `(frameId, loaderId)` from `Page.getFrameTree` afterwards, which
    /// answers for a document arrived at by any route, not only by a `goto` this tool sent.
    #[allow(dead_code)]
    pub frame_id: String,
    /// shape — see `frame_id`. Same pair, same reason it is read elsewhere.
    #[allow(dead_code)]
    #[serde(default)]
    pub loader_id: Option<String>,
    #[serde(default)]
    pub error_text: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureScreenshotParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clip: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_surface: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capture_beyond_viewport: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub optimize_for_speed: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureScreenshotResult {
    pub data: String,
}

// ---------------------------------------------------------------------------
// Runtime domain
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvaluateResult {
    pub result: RemoteObject,
    #[serde(default)]
    pub exception_details: Option<ExceptionDetails>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteObject {
    #[serde(rename = "type")]
    pub remote_type: String,
    /// shape — `"array"`, `"node"`, `"promise"`… beside `remote_type`'s `"object"`. Callers
    /// here ask for `returnByValue` and read `value`, or ask for a handle and read `object_id`.
    #[allow(dead_code)]
    #[serde(default)]
    pub subtype: Option<String>,
    /// shape — the constructor name of an object handle. `DOM.describeNode` is what this tool
    /// uses to name a node, and it answers about the DOM rather than about the JS wrapper.
    #[allow(dead_code)]
    #[serde(default)]
    pub class_name: Option<String>,
    #[serde(default)]
    pub value: Option<Value>,
    /// shape — how `NaN`, `Infinity` and `-0` come back, none of which JSON can carry. No
    /// evaluation in this tool returns one; a numeric probe that did would read this.
    #[allow(dead_code)]
    #[serde(default)]
    pub unserializable_value: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub object_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExceptionDetails {
    /// shape — Chrome's own counter for the throw. Nothing correlates exceptions across calls.
    #[allow(dead_code)]
    pub exception_id: u64,
    pub text: String,
    /// shape — position inside the script that threw. Every script this tool evaluates is one
    /// it wrote itself and injected as a single expression, so a line number here points into
    /// a string in this repository, not into anything the caller can open. The eight readers of
    /// `exception_details` all report `exception.description` and fall back to `text`, which is
    /// the message a caller can act on.
    #[allow(dead_code)]
    pub line_number: i64,
    /// shape — see `line_number`.
    #[allow(dead_code)]
    pub column_number: i64,
    /// shape — see `line_number`; identifies the injected script, which has no name.
    #[allow(dead_code)]
    #[serde(default)]
    pub script_id: Option<String>,
    /// shape — the script's url, absent for everything this tool evaluates.
    #[allow(dead_code)]
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub exception: Option<RemoteObject>,
    /// shape — which execution context threw. `frame` already knows the context it bound, and
    /// an exception arrives on the call that caused it.
    #[allow(dead_code)]
    #[serde(default)]
    pub execution_context_id: Option<u64>,
}

// ---------------------------------------------------------------------------
// DOM domain
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveNodeParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend_node_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object_group: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_context_id: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveNodeResult {
    pub object: RemoteObject,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetBoxModelResult {
    pub model: BoxModel,
}

/// CDP box model. Each quad is an array of 8 floats: [x1,y1, x2,y2, x3,y3, x4,y4].
///
/// `content` is what `content_center` aims at and `border` is what `geometry` clips a screenshot
/// to; the other four are read by nothing. They are kept, and unlike everywhere else in this file
/// the reason is not that they document the protocol: none of the six is `Option` or
/// `#[serde(default)]`, so each one is a field Chrome MUST send for `DOM.getBoxModel` to parse at
/// all. Keeping an unread required field is the opposite of leniency — it is a constraint this
/// tool imposes for nothing. CDP declares all six required, so the constraint costs nothing
/// today; it is written down because "kept for deserialization completeness" was the old
/// justification for this whole module and it had this exactly backwards.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoxModel {
    pub content: Quad,
    /// pinned — `snapshot.rs`'s `bug_content_center_empty_quad` constructs a `BoxModel` and so
    /// names every field. See the type doc for why these four are not merely decorative.
    #[allow(dead_code)]
    pub padding: Quad,
    pub border: Quad,
    /// pinned — see `padding`.
    #[allow(dead_code)]
    pub margin: Quad,
    /// pinned — see `padding`.
    #[allow(dead_code)]
    pub width: u32,
    /// pinned — see `padding`.
    #[allow(dead_code)]
    pub height: u32,
}

/// A quad is 4 (x, y) points = 8 floats.
pub type Quad = Vec<f64>;

// ---------------------------------------------------------------------------
// Input domain
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatchMouseEventParams {
    #[serde(rename = "type")]
    pub event_type: MouseEventType,
    pub x: f64,
    pub y: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub button: Option<MouseButton>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buttons: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modifiers: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub click_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta_x: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta_y: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pointer_type: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MouseEventType {
    MousePressed,
    MouseReleased,
    MouseMoved,
}

/// The mouse button this tool sends. CDP defines six; one is wired.
///
/// It used to declare all six — `None`, `Left`, `Middle`, `Right`, `Back`, `Forward` — and
/// construct exactly one of them, at the eight sites that dispatch pointer input (`element.rs`
/// ×4 for click and dblclick, `element_controls.rs` ×4 for drag). Nothing else in the repository
/// mentions a right click: no command, no flag, no test, no line of documentation. So this enum
/// was the only place that said anything at all about the subject, and what it said was untrue —
/// a reader of the type concluded a right click existed somewhere, and there is nowhere for it
/// to exist. The five went, and the absence became a fact of the type rather than a discovery
/// made by grepping.
///
/// Two things make the removal cheap. Nothing deserializes `MouseButton` (`Serialize` only), so
/// no message Chrome sends can stop fitting; and the value only ever leaves in
/// `DispatchMouseEventParams::button`, so removing a variant narrows what this tool can emit and
/// changes nothing it accepts. `None` went with the rest and was redundant besides: a move with
/// no button held is already spelled `button: None` in Rust, which omits the field entirely
/// (`skip_serializing_if`) and lets CDP apply its own `"none"` default — `element_controls`'s
/// drag does exactly that between press and release.
///
/// **What this costs, stated:** adding a right click later means adding a variant back. That is
/// one line, and it will be the smallest part of the work — there is no verb, no CLI surface and
/// no `hit_test` story for a context menu today, and whoever writes them will not be slowed by
/// this. `types.rs` is a hand-picked subset of CDP, not a mirror of it (there are no `Network.*`
/// types here either), so carrying a complete enum inside an incomplete module bought fidelity
/// nowhere and asserted a capability in the one spot a reader would look for it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MouseButton {
    Left,
}

// ---------------------------------------------------------------------------
// Accessibility domain
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetFullAXTreeResult {
    pub nodes: Vec<AXNode>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AXNode {
    pub node_id: String,
    #[serde(default)]
    pub ignored: bool,
    #[serde(default)]
    pub role: Option<AXValue>,
    #[serde(default)]
    pub name: Option<AXValue>,
    /// shape — the node's accessible description (`aria-describedby`, `title`). Deliberately
    /// not rendered: `snapshot_render` strips to role, name and value precisely to keep a tree
    /// an agent can read, and a description is the longest field on the node.
    #[allow(dead_code)]
    #[serde(default)]
    pub description: Option<AXValue>,
    #[serde(default)]
    pub value: Option<AXValue>,
    #[serde(default)]
    pub properties: Option<Vec<AXProperty>>,
    #[serde(default)]
    pub child_ids: Option<Vec<String>>,
    #[serde(default, rename = "backendDOMNodeId")]
    pub backend_dom_node_id: Option<i64>,
    /// shape — set on the root node of each frame in a cross-frame `getFullAXTree`. Scoping is
    /// done on the way out instead: `snapshot` passes `frameId` as a REQUEST parameter, so the
    /// tree that comes back is already the frame's.
    #[allow(dead_code)]
    #[serde(default)]
    pub frame_id: Option<String>,
    #[serde(default)]
    pub parent_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AXValue {
    /// shape — how to read `value`: `"string"`, `"boolean"`, `"idrefList"`, `"computedString"`…
    /// Every reader here goes through `AXNode::role_name`/`name_value` or `AXProperty`, all of
    /// which ask `value.as_str()`/`as_bool()` and get `None` when the type is not the one they
    /// wanted — so the tag is checked by the read rather than before it.
    #[allow(dead_code)]
    #[serde(rename = "type")]
    pub value_type: String,
    #[serde(default)]
    pub value: Option<Value>,
    /// shape — the nodes an `aria-labelledby`/`aria-controls`/`aria-owns` points at. This is the
    /// only field carrying `AXRelatedNode`, so the struct below is unread through it and not on
    /// its own account.
    #[allow(dead_code)]
    #[serde(default)]
    pub related_nodes: Option<Vec<AXRelatedNode>>,
}

/// A node referenced by another node's ARIA relation. Reached only through
/// `AXValue::related_nodes`, which nothing reads — so all three fields are unread for that one
/// reason, and they are kept as a set because a reader of relations would want the three
/// together: what the relation points at, how it was written, and what it says.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AXRelatedNode {
    /// shape — the DOM node the relation names; this is what a uid would be built from.
    #[allow(dead_code)]
    #[serde(default, rename = "backendDOMNodeId")]
    pub backend_dom_node_id: Option<i64>,
    /// shape — the `id` attribute as the author wrote it in the relation.
    #[allow(dead_code)]
    #[serde(default)]
    pub idref: Option<String>,
    /// shape — the related node's text, which is how a label reaches an input's accessible name.
    #[allow(dead_code)]
    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AXProperty {
    pub name: String,
    pub value: AXValue,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

impl BoxModel {
    /// Return the center (x, y) of the content quad.
    pub fn content_center(&self) -> (f64, f64) {
        // content quad is [x1,y1, x2,y2, x3,y3, x4,y4]
        if self.content.len() < 8 {
            return (0.0, 0.0);
        }
        let cx = (self.content[0] + self.content[2] + self.content[4] + self.content[6]) / 4.0;
        let cy = (self.content[1] + self.content[3] + self.content[5] + self.content[7]) / 4.0;
        (cx, cy)
    }
}

impl AXNode {
    /// Extract the human-readable role string, if present.
    pub fn role_name(&self) -> Option<&str> {
        self.role.as_ref()?.value.as_ref()?.as_str()
    }

    /// Extract the human-readable name string, if present.
    pub fn name_value(&self) -> Option<&str> {
        self.name.as_ref()?.value.as_ref()?.as_str()
    }
}

impl std::fmt::Display for CdpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CDP error {}: {}", self.code, self.message)
    }
}

impl std::error::Error for CdpError {}
