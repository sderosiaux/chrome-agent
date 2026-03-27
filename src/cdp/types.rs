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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivateTargetParams {
    pub target_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloseTargetParams {
    pub target_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloseTargetResult {
    pub success: bool,
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

// Target events

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetCreatedEvent {
    pub target_info: TargetInfo,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetDestroyedEvent {
    pub target_id: String,
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
    pub clip: Option<Viewport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_surface: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capture_beyond_viewport: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub optimize_for_speed: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Viewport {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub scale: f64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureScreenshotResult {
    pub data: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetFrameTreeResult {
    pub frame_tree: FrameTree,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameTree {
    pub frame: Frame,
    #[serde(default)]
    pub child_frames: Option<Vec<FrameTree>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Frame {
    pub id: String,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub loader_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    pub url: String,
    #[serde(default)]
    pub security_origin: Option<String>,
    #[serde(default)]
    pub mime_type: Option<String>,
}

// Page events

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameNavigatedEvent {
    pub frame: Frame,
    #[serde(rename = "type", default)]
    pub navigation_type: Option<String>,
}

/// `Page.loadEventFired` carries only a timestamp.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadEventFiredEvent {
    pub timestamp: f64,
}

// ---------------------------------------------------------------------------
// Runtime domain
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvaluateParams {
    pub expression: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object_group: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_command_line_api: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub silent: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub return_by_value: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generate_preview: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_gesture: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub await_promise: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub throw_on_side_effect: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvaluateResult {
    pub result: RemoteObject,
    #[serde(default)]
    pub exception_details: Option<ExceptionDetails>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallFunctionOnParams {
    pub function_declaration: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Vec<CallArgument>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub silent: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub return_by_value: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generate_preview: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_gesture: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub await_promise: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_context_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object_group: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallArgument {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unserializable_value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallFunctionOnResult {
    pub result: RemoteObject,
    #[serde(default)]
    pub exception_details: Option<ExceptionDetails>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddBindingParams {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_context_name: Option<String>,
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

// Runtime events

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BindingCalledEvent {
    pub name: String,
    pub payload: String,
    pub execution_context_id: u64,
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetBoxModelParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend_node_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object_id: Option<String>,
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DescribeNodeParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend_node_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depth: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pierce: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DescribeNodeResult {
    pub node: DomNode,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DomNode {
    pub node_id: i64,
    #[serde(default)]
    pub parent_id: Option<i64>,
    pub backend_node_id: i64,
    pub node_type: i32,
    pub node_name: String,
    pub local_name: String,
    pub node_value: String,
    #[serde(default)]
    pub child_node_count: Option<u32>,
    #[serde(default)]
    pub children: Option<Vec<DomNode>>,
    #[serde(default)]
    pub attributes: Option<Vec<String>>,
    #[serde(default)]
    pub document_url: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub frame_id: Option<String>,
    #[serde(default)]
    pub content_document: Option<Box<DomNode>>,
}

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
    MouseWheel,
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatchKeyEventParams {
    #[serde(rename = "type")]
    pub event_type: KeyEventType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modifiers: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unmodified_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_identifier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub windows_virtual_key_code: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_virtual_key_code: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_repeat: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_keypad: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_system_key: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<u32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum KeyEventType {
    KeyDown,
    KeyUp,
    RawKeyDown,
    Char,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InsertTextParams {
    pub text: String,
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
    #[serde(default)]
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
    pub backend_dom_node_id: i64,
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
