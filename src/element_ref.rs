use serde::{Deserialize, Serialize};

/// Abstract element reference that decouples uid resolution from CDP internals.
///
/// Today this wraps a `backendNodeId`. Tomorrow it could wrap an objectId,
/// a WebDriver BiDi reference, or a selector fallback — without changing
/// the session format or CLI surface.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ElementRef {
    /// Resolved via `DOM.resolveNode({backendNodeId})`.
    #[serde(rename = "backendNode")]
    BackendNode {
        #[serde(rename = "id")]
        backend_node_id: i64,
    },
}

impl ElementRef {
    pub fn backend_node(id: i64) -> Self {
        Self::BackendNode {
            backend_node_id: id,
        }
    }

    /// Extract the backendNodeId if this is a BackendNode ref.
    pub fn backend_node_id(&self) -> Option<i64> {
        match self {
            Self::BackendNode { backend_node_id } => Some(*backend_node_id),
        }
    }
}
