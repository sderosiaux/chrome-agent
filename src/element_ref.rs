use serde::{Deserialize, Serialize};

/// Abstract element reference that decouples uid resolution from CDP internals.
///
/// Today this wraps a `backendNodeId`. Tomorrow it could wrap an objectId,
/// a `WebDriver` `BiDi` reference, or a selector fallback — without changing
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
    pub const fn backend_node(id: i64) -> Self {
        Self::BackendNode {
            backend_node_id: id,
        }
    }

    /// Extract the backendNodeId if this is a `BackendNode` ref.
    pub const fn backend_node_id(&self) -> Option<i64> {
        match self {
            Self::BackendNode { backend_node_id } => Some(*backend_node_id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn element_ref_serialization_roundtrip() {
        let er = ElementRef::backend_node(42);
        let json = serde_json::to_string(&er).unwrap();
        assert!(json.contains("backendNode"));
        assert!(json.contains("42"));
        let back: ElementRef = serde_json::from_str(&json).unwrap();
        assert_eq!(back.backend_node_id(), Some(42));
    }
}
