//! Rust shapes for the slice of the Chrome `DevTools` Protocol this tool speaks.
//!
//! A struct declares only what something reads. There is no `dead_code` allow here, so a field
//! added and never read is a build failure. Serde ignores undeclared fields, so omitting one is
//! free; a declared field that is neither `Option` nor `#[serde(default)]` is a field Chrome
//! MUST send. Unread protocol fields are noted in prose on the type that would carry them.
//! `serde_proof` below pins both halves.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// --- Generic CDP wire protocol ---

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
/// `untagged`, and a response carries `id` while an event carries `method`, so the two never
/// mis-assign. Falling out of the enum takes an optional field of an undeclared JSON type, so
/// every optional field here is `Value` or a type CDP pins; see `client::resolve_unreadable`.
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
}

/// Protocol-level error attached to a response. CDP's free-form `data` stays undeclared: typed
/// `Option<String>`, an object-valued `data` fell out of both `CdpMessage` variants and turned
/// an immediate error into a timeout. Only `code` and `message` are read.
#[derive(Debug, Deserialize)]
pub struct CdpError {
    pub code: i64,
    pub message: String,
}

/// Async event pushed by Chrome.
#[derive(Debug, Clone, Deserialize)]
pub struct CdpEvent {
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

// --- Target domain ---

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
}

// --- Page domain ---

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

/// `Page.navigate`'s answer, reduced to `errorText`. Document identity comes from
/// `Page.getFrameTree` afterwards (`diff::Identity` reads `(frameId, loaderId)` there), which
/// answers for a document arrived at by any route, not only by a `goto` this tool sent.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NavigateResult {
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

// --- Runtime domain ---

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
    #[serde(default)]
    pub value: Option<Value>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub object_id: Option<String>,
}

/// A throw, as its readers use it: `exception.description`, falling back to `text`. CDP's
/// `exceptionId`, `lineNumber`, `columnNumber`, `scriptId`, `url` and `executionContextId` stay
/// undeclared — a line and column point into an expression this repository injected.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExceptionDetails {
    pub text: String,
    #[serde(default)]
    pub exception: Option<RemoteObject>,
}

// --- DOM domain ---

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
/// `content_center` aims at `content`, `geometry` clips a screenshot to `border`. `padding`,
/// `margin`, `width` and `height` stay undeclared: unread, and none was `Option`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoxModel {
    pub content: Quad,
    pub border: Quad,
}

/// A quad is 4 (x, y) points = 8 floats.
pub type Quad = Vec<f64>;

// --- Input domain ---

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

/// The mouse button this tool sends. CDP defines six; only `Left` is wired, since no verb,
/// flag or test here produces another. `Serialize` only, so narrowing it changes nothing this
/// tool accepts. A move with no button held is `button: None`, which omits the field
/// (`skip_serializing_if`) and lets CDP apply its own `"none"` default.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MouseButton {
    Left,
}

// --- Accessibility domain ---

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetFullAXTreeResult {
    pub nodes: Vec<AXNode>,
}

/// One accessibility node, reduced to what `snapshot_render` renders and what the traversal
/// walks. `description` and `frameId` are undeclared: the render keeps role, name and value
/// only, and `snapshot` scopes frames by passing `frameId` as a request parameter.
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
    #[serde(default)]
    pub value: Option<AXValue>,
    #[serde(default)]
    pub properties: Option<Vec<AXProperty>>,
    #[serde(default)]
    pub child_ids: Option<Vec<String>>,
    #[serde(default, rename = "backendDOMNodeId")]
    pub backend_dom_node_id: Option<i64>,
    #[serde(default)]
    pub parent_id: Option<String>,
}

/// A tagged accessibility value. CDP's `type` and `relatedNodes` stay undeclared: readers ask
/// `value.as_str()`/`as_bool()` and get `None` on a mismatch, so the tag is checked by the
/// read, whereas a declared `type` was required and an untagged value broke the whole tree.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AXValue {
    #[serde(default)]
    pub value: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AXProperty {
    pub name: String,
    pub value: AXValue,
}

// --- Helpers ---

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

/// The two facts the module doc rests on: an undeclared field cannot break a parse, and a
/// declared non-`Option` field is a demand made of Chrome.
#[cfg(test)]
mod serde_proof {
    use super::*;

    /// An undeclared field is ignored whatever it holds.
    #[test]
    fn an_undeclared_field_cannot_break_a_parse() {
        // One arrives as an object: the JSON type that dropped the whole message when `data`
        // was typed `Option<String>`.
        let json = r#"{"id":7,"error":{"code":-32000,"message":"boom","data":{"detail":"x"}},
                       "sessionId":"S1","whatever":42}"#;
        let r: CdpResponse = serde_json::from_str(json).expect("undeclared fields are ignored");
        assert_eq!(r.id, 7);
        assert_eq!(r.error.expect("error parsed").code, -32000);
    }

    /// A declared field that is neither `Option` nor `#[serde(default)]` is REQUIRED.
    #[test]
    fn only_what_is_read_is_demanded_of_chrome() {
        let two_quads_only = r#"{"content":[0,0,1,0,1,1,0,1],"border":[0,0,1,0,1,1,0,1]}"#;
        assert!(serde_json::from_str::<BoxModel>(two_quads_only).is_ok());

        // `content` IS read, so it stays required.
        let err = serde_json::from_str::<BoxModel>(r#"{"border":[0,0,1,0,1,1,0,1]}"#)
            .expect_err("content is read, so it is still demanded")
            .to_string();
        assert!(
            err.contains("content"),
            "expected a missing-field error naming content: {err}"
        );

        // Same on the other three, whose unread fields were required too.
        assert!(serde_json::from_str::<ExceptionDetails>(r#"{"text":"ReferenceError"}"#).is_ok());
        assert!(serde_json::from_str::<AXValue>(r#"{"value":"button"}"#).is_ok());
        assert!(serde_json::from_str::<NavigateResult>("{}").is_ok());
    }
}
