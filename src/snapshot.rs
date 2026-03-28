use std::collections::HashMap;

use crate::cdp::client::{CdpClient, CdpClientError};
use crate::cdp::types::{AXNode, GetFullAXTreeResult};
use crate::element_ref::ElementRef;

/// Result of taking an a11y tree snapshot.
pub struct Snapshot {
    /// Formatted text output for the agent.
    pub text: String,
    /// uid → `ElementRef` mapping for subsequent actions.
    pub uid_map: HashMap<String, ElementRef>,
}

/// Take an accessibility tree snapshot of the current page.
///
/// Calls `Accessibility.getFullAXTree` via CDP, formats the tree into
/// a compact text representation with uid identifiers, and builds the
/// uid → `ElementRef` mapping.
///
/// If `focus_uid` is provided (e.g. "e5"), the output is scoped to the
/// subtree rooted at that element. `max_depth` limits how deep the tree
/// is rendered (0 = root only).
pub async fn take_snapshot(
    client: &CdpClient,
    verbose: bool,
    max_depth: Option<usize>,
    focus_uid: Option<&str>,
) -> Result<Snapshot, CdpClientError> {
    // Enable accessibility domain
    client
        .send("Accessibility.enable", serde_json::json!({}))
        .await?;

    let result: GetFullAXTreeResult = client
        .call("Accessibility.getFullAXTree", serde_json::json!({}))
        .await?;

    let (text, uid_map) = format_ax_tree(&result.nodes, verbose, max_depth, focus_uid);

    Ok(Snapshot { text, uid_map })
}

/// Format `AXNode` list into indented text + uid map.
///
/// CDP returns a flat list of `AXNodes` with parent/child relationships
/// via `parentId` and `childIds`. We reconstruct the tree and format it.
///
/// When `focus_uid` is set, we first do a full pass to assign uids (so
/// the numbering matches a normal inspect), then find the node whose uid
/// matches and re-render only that subtree from depth 0.
fn format_ax_tree(
    nodes: &[AXNode],
    verbose: bool,
    max_depth: Option<usize>,
    focus_uid: Option<&str>,
) -> (String, HashMap<String, ElementRef>) {
    // Build lookup: nodeId → AXNode
    let node_by_id: HashMap<&str, &AXNode> = nodes
        .iter()
        .map(|n| (n.node_id.as_str(), n))
        .collect();

    // Find root (node with no parentId, or first node)
    let root_id = nodes
        .iter()
        .find(|n| n.parent_id.is_none())
        .map(|n| n.node_id.as_str());

    let Some(root_id) = root_id else {
        return (String::new(), HashMap::new());
    };

    if let Some(focus) = focus_uid {
        // First pass: assign uids without max_depth to find the target node
        let mut uid_map_full = HashMap::new();
        let mut uid_counter: u32 = 0;
        let mut discard = String::new();
        // Map uid → AXNode nodeId so we can find the subtree root
        let mut uid_to_node_id: HashMap<String, String> = HashMap::new();
        format_node_with_tracking(
            root_id,
            &node_by_id,
            0,
            verbose,
            None, // no depth limit for uid assignment
            &mut uid_counter,
            &mut uid_map_full,
            &mut discard,
            &mut uid_to_node_id,
        );

        // Find the AXNode nodeId for the focus uid
        let focus_node_id = uid_to_node_id.get(focus);
        if let Some(focus_node_id) = focus_node_id {
            // Second pass: render only the subtree
            let mut uid_map = HashMap::new();
            let mut output = String::new();
            let mut uid_counter2: u32 = 0;
            let mut tracking2: HashMap<String, String> = HashMap::new();
            format_node_with_tracking(
                focus_node_id,
                &node_by_id,
                0, // reset depth to 0
                verbose,
                max_depth,
                &mut uid_counter2,
                &mut uid_map,
                &mut output,
                &mut tracking2,
            );
            return (output, uid_map);
        }

        // uid not found — fall through to full tree
        return (
            format!("uid={focus} not found in accessibility tree\n"),
            uid_map_full,
        );
    }

    // Normal (no focus_uid) path
    let mut uid_map = HashMap::new();
    let mut output = String::new();
    let mut uid_counter: u32 = 0;
    format_node(
        root_id,
        &node_by_id,
        0,
        verbose,
        max_depth,
        &mut uid_counter,
        &mut uid_map,
        &mut output,
    );

    (output, uid_map)
}

fn format_node(
    node_id: &str,
    nodes: &HashMap<&str, &AXNode>,
    depth: usize,
    verbose: bool,
    max_depth: Option<usize>,
    uid_counter: &mut u32,
    uid_map: &mut HashMap<String, ElementRef>,
    output: &mut String,
) {
    let mut discard: HashMap<String, String> = HashMap::new();
    format_node_with_tracking(
        node_id, nodes, depth, verbose, max_depth, uid_counter, uid_map, output, &mut discard,
    );
}

fn format_node_with_tracking(
    node_id: &str,
    nodes: &HashMap<&str, &AXNode>,
    depth: usize,
    verbose: bool,
    max_depth: Option<usize>,
    uid_counter: &mut u32,
    uid_map: &mut HashMap<String, ElementRef>,
    output: &mut String,
    uid_to_node_id: &mut HashMap<String, String>,
) {
    let Some(node) = nodes.get(node_id) else {
        return;
    };

    // Skip ignored nodes unless verbose
    if node.ignored && !verbose {
        // Still recurse into children — some ignored nodes have visible children
        if let Some(child_ids) = &node.child_ids {
            for child_id in child_ids {
                format_node_with_tracking(child_id, nodes, depth, verbose, max_depth, uid_counter, uid_map, output, uid_to_node_id);
            }
        }
        return;
    }

    let role = node.role_name().unwrap_or("");
    let mut name = node.name_value().unwrap_or("").to_string();

    // Skip noise roles unless verbose — these repeat parent content and waste tokens
    const NOISE_ROLES: &[&str] = &["none", "StaticText", "InlineTextBox"];
    if !verbose && NOISE_ROLES.contains(&role) {
        if let Some(child_ids) = &node.child_ids {
            for child_id in child_ids {
                format_node_with_tracking(child_id, nodes, depth, verbose, max_depth, uid_counter, uid_map, output, uid_to_node_id);
            }
        }
        return;
    }

    // If name is empty and we're filtering noise, pull text from StaticText children
    if !verbose && name.is_empty() {
        if let Some(child_ids) = &node.child_ids {
            let texts: Vec<&str> = child_ids
                .iter()
                .filter_map(|cid| nodes.get(cid.as_str()))
                .filter(|n| n.role_name() == Some("StaticText"))
                .filter_map(|n| n.name_value())
                .collect();
            if !texts.is_empty() {
                name = texts.join(" ");
            }
        }
    }

    // Skip generic containers with no name unless verbose
    if !verbose && role == "generic" && name.is_empty() {
        if let Some(child_ids) = &node.child_ids {
            for child_id in child_ids {
                format_node_with_tracking(child_id, nodes, depth, verbose, max_depth, uid_counter, uid_map, output, uid_to_node_id);
            }
        }
        return;
    }

    // Assign uid — stable (based on backendNodeId) when available, sequential fallback
    let uid = if let Some(backend_id) = node.backend_dom_node_id {
        let uid = format!("n{backend_id}");
        uid_map.insert(uid.clone(), ElementRef::backend_node(backend_id));
        uid
    } else {
        *uid_counter += 1;
        format!("e{uid_counter}")
    };

    // Track uid → AXNode nodeId for focus_uid lookup
    uid_to_node_id.insert(uid.clone(), node_id.to_string());

    // Build attribute string
    let indent = "  ".repeat(depth);
    output.push_str(&indent);
    output.push_str("uid=");
    output.push_str(&uid);

    if !role.is_empty() {
        output.push(' ');
        if role == "none" {
            output.push_str("ignored");
        } else {
            output.push_str(role);
        }
    }

    if !name.is_empty() {
        output.push_str(" \"");
        output.push_str(&name);
        output.push('"');
    }

    // Value (for inputs)
    if let Some(value_ax) = &node.value {
        if let Some(val) = value_ax.value.as_ref().and_then(|v| v.as_str()) {
            if !val.is_empty() {
                output.push_str(" value=\"");
                output.push_str(val);
                output.push('"');
            }
        }
    }

    // Properties: focused, disabled, expanded, selected, level, checked
    if let Some(props) = &node.properties {
        for prop in props {
            let prop_val = prop.value.value.as_ref();
            match prop.name.as_str() {
                "focused" => {
                    if prop_val.and_then(serde_json::Value::as_bool).unwrap_or(false) {
                        output.push_str(" focused");
                    }
                }
                "disabled" => {
                    if prop_val.and_then(serde_json::Value::as_bool).unwrap_or(false) {
                        output.push_str(" disabled");
                    }
                }
                "expanded" => {
                    if prop_val.and_then(serde_json::Value::as_bool).unwrap_or(false) {
                        output.push_str(" expanded");
                    }
                }
                "selected" => {
                    if prop_val.and_then(serde_json::Value::as_bool).unwrap_or(false) {
                        output.push_str(" selected");
                    }
                }
                "checked" => {
                    if let Some(val) = prop_val.and_then(|v| v.as_str()) {
                        if val != "false" {
                            output.push_str(" checked=");
                            output.push_str(val);
                        }
                    }
                }
                "level" => {
                    if let Some(level) = prop_val.and_then(serde_json::Value::as_u64) {
                        output.push_str(&format!(" level={level}"));
                    }
                }
                "required" => {
                    if prop_val.and_then(serde_json::Value::as_bool).unwrap_or(false) {
                        output.push_str(" required");
                    }
                }
                "readonly" => {
                    if prop_val.and_then(serde_json::Value::as_bool).unwrap_or(false) {
                        output.push_str(" readonly");
                    }
                }
                _ => {
                    // Include all properties in verbose mode
                    if verbose {
                        if let Some(val) = prop_val {
                            output.push(' ');
                            output.push_str(&prop.name);
                            output.push('=');
                            match val {
                                serde_json::Value::Bool(b) => output.push_str(&b.to_string()),
                                serde_json::Value::Number(n) => output.push_str(&n.to_string()),
                                serde_json::Value::String(s) => {
                                    output.push('"');
                                    output.push_str(s);
                                    output.push('"');
                                }
                                _ => output.push_str(&val.to_string()),
                            }
                        }
                    }
                }
            }
        }
    }

    output.push('\n');

    // Depth limit: skip children if we've reached max_depth
    if let Some(max) = max_depth {
        if depth >= max {
            return;
        }
    }

    // Recurse children
    if let Some(child_ids) = &node.child_ids {
        for child_id in child_ids {
            format_node_with_tracking(
                child_id,
                nodes,
                depth + 1,
                verbose,
                max_depth,
                uid_counter,
                uid_map,
                output,
                uid_to_node_id,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdp::types::{AXValue, AXProperty};

    fn make_ax_value(s: &str) -> AXValue {
        AXValue {
            value_type: "string".into(),
            value: Some(serde_json::Value::String(s.into())),
            related_nodes: None,
        }
    }

    fn default_ax_node() -> AXNode {
        AXNode {
            node_id: String::new(),
            ignored: false,
            role: None,
            name: None,
            description: None,
            value: None,
            properties: None,
            child_ids: None,
            backend_dom_node_id: None,
            frame_id: None,
            parent_id: None,
        }
    }

    fn make_bool_prop(name: &str, val: bool) -> AXProperty {
        AXProperty {
            name: name.into(),
            value: AXValue {
                value_type: "boolean".into(),
                value: Some(serde_json::Value::Bool(val)),
                related_nodes: None,
            },
        }
    }

    #[test]
    fn formats_simple_tree() {
        let nodes = vec![
            AXNode {
                node_id: "1".into(),
                ignored: false,
                role: Some(make_ax_value("heading")),
                name: Some(make_ax_value("Welcome")),
                description: None,
                value: None,
                properties: Some(vec![AXProperty {
                    name: "level".into(),
                    value: AXValue {
                        value_type: "integer".into(),
                        value: Some(serde_json::json!(1)),
                        related_nodes: None,
                    },
                }]),
                child_ids: Some(vec![]),
                backend_dom_node_id: Some(10),
                frame_id: None,
                parent_id: None,
            },
        ];

        let (text, uid_map) = format_ax_tree(&nodes, false, None, None);
        assert!(text.contains("uid=n10 heading \"Welcome\" level=1"));
        assert!(uid_map.contains_key("n10"));
        assert_eq!(uid_map["n10"].backend_node_id(), Some(10));
    }

    #[test]
    fn skips_ignored_nodes() {
        let nodes = vec![
            AXNode {
                node_id: "1".into(),
                ignored: true,
                role: None,
                name: None,
                description: None,
                value: None,
                properties: None,
                child_ids: Some(vec!["2".into()]),
                backend_dom_node_id: None,
                frame_id: None,
                parent_id: None,
            },
            AXNode {
                node_id: "2".into(),
                ignored: false,
                role: Some(make_ax_value("button")),
                name: Some(make_ax_value("Click me")),
                description: None,
                value: None,
                properties: Some(vec![make_bool_prop("focused", true)]),
                child_ids: Some(vec![]),
                backend_dom_node_id: Some(20),
                frame_id: None,
                parent_id: Some("1".into()),
            },
        ];

        let (text, uid_map) = format_ax_tree(&nodes, false, None, None);
        assert!(!text.contains("ignored"));
        assert!(text.contains("uid=n20 button \"Click me\" focused"));
        assert_eq!(uid_map.len(), 1);
    }

    #[test]
    fn max_depth_limits_output() {
        let nodes = vec![
            AXNode {
                node_id: "1".into(),
                role: Some(make_ax_value("heading")),
                name: Some(make_ax_value("Root")),
                child_ids: Some(vec!["2".into()]),
                parent_id: None,
                backend_dom_node_id: Some(1),
                ..default_ax_node()
            },
            AXNode {
                node_id: "2".into(),
                role: Some(make_ax_value("button")),
                name: Some(make_ax_value("Child")),
                child_ids: Some(vec!["3".into()]),
                parent_id: Some("1".into()),
                backend_dom_node_id: Some(2),
                ..default_ax_node()
            },
            AXNode {
                node_id: "3".into(),
                role: Some(make_ax_value("link")),
                name: Some(make_ax_value("Grand")),
                child_ids: Some(vec![]),
                parent_id: Some("2".into()),
                backend_dom_node_id: Some(3),
                ..default_ax_node()
            },
        ];
        let (text, _) = format_ax_tree(&nodes, false, Some(1), None);
        assert!(text.contains("Root"));
        assert!(text.contains("Child"));
        assert!(!text.contains("Grand")); // depth 2 filtered
    }

    #[test]
    fn focus_uid_scopes_subtree() {
        let nodes = vec![
            AXNode {
                node_id: "1".into(),
                role: Some(make_ax_value("WebArea")),
                name: Some(make_ax_value("Page")),
                child_ids: Some(vec!["2".into(), "3".into()]),
                parent_id: None,
                backend_dom_node_id: Some(1),
                ..default_ax_node()
            },
            AXNode {
                node_id: "2".into(),
                role: Some(make_ax_value("heading")),
                name: Some(make_ax_value("Title")),
                child_ids: Some(vec![]),
                parent_id: Some("1".into()),
                backend_dom_node_id: Some(2),
                ..default_ax_node()
            },
            AXNode {
                node_id: "3".into(),
                role: Some(make_ax_value("button")),
                name: Some(make_ax_value("Submit")),
                child_ids: Some(vec![]),
                parent_id: Some("1".into()),
                backend_dom_node_id: Some(3),
                ..default_ax_node()
            },
        ];
        // n1=WebArea, n2=heading, n3=button — focus on n3
        let (text, _) = format_ax_tree(&nodes, false, None, Some("n3"));
        assert!(text.contains("Submit"));
        assert!(!text.contains("Title"));
    }

    #[test]
    fn focus_uid_not_found() {
        let nodes = vec![AXNode {
            node_id: "1".into(),
            role: Some(make_ax_value("heading")),
            name: Some(make_ax_value("Root")),
            child_ids: Some(vec![]),
            parent_id: None,
            backend_dom_node_id: Some(1),
            ..default_ax_node()
        }];
        let (text, _) = format_ax_tree(&nodes, false, None, Some("e99"));
        assert!(text.contains("not found"));
    }
}
