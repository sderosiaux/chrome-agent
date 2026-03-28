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
    #[serde(default, rename = "sessionId")]
    pub session_id: Option<String>,
}

/// Protocol-level error attached to a response.
#[derive(Debug, Deserialize)]
pub struct CdpError {
    pub code: i64,
    pub message: String,
    #[serde(default)]
    pub data: Option<String>,
}

/// Async event pushed by Chrome.
#[derive(Debug, Clone, Deserialize)]
pub struct CdpEvent {
    pub method: String,
    #[serde(default)]
    pub params: Value,
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
    #[serde(default)]
    pub attached: bool,
    #[serde(default)]
    pub opener_id: Option<String>,
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
    pub frame_id: String,
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
    #[serde(default)]
    pub subtype: Option<String>,
    #[serde(default)]
    pub class_name: Option<String>,
    #[serde(default)]
    pub value: Option<Value>,
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
    pub exception_id: u64,
    pub text: String,
    pub line_number: i64,
    pub column_number: i64,
    #[serde(default)]
    pub script_id: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub exception: Option<RemoteObject>,
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
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoxModel {
    pub content: Quad,
    pub padding: Quad,
    pub border: Quad,
    pub margin: Quad,
    pub width: u32,
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MouseButton {
    None,
    Left,
    Middle,
    Right,
    Back,
    Forward,
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
    #[serde(default)]
    pub frame_id: Option<String>,
    #[serde(default)]
    pub parent_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AXValue {
    #[serde(rename = "type")]
    pub value_type: String,
    #[serde(default)]
    pub value: Option<Value>,
    #[serde(default)]
    pub related_nodes: Option<Vec<AXRelatedNode>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AXRelatedNode {
    #[serde(default, rename = "backendDOMNodeId")]
    pub backend_dom_node_id: Option<i64>,
    #[serde(default)]
    pub idref: Option<String>,
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
        self.role.as_ref().and_then(|v| {
            v.value
                .as_ref()
                .and_then(|val| val.as_str())
        })
    }

    /// Extract the human-readable name string, if present.
    pub fn name_value(&self) -> Option<&str> {
        self.name.as_ref().and_then(|v| {
            v.value
                .as_ref()
                .and_then(|val| val.as_str())
        })
    }
}

impl std::fmt::Display for CdpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CDP error {}: {}", self.code, self.message)
    }
}

impl std::error::Error for CdpError {}
