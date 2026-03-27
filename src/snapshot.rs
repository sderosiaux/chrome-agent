use std::collections::HashMap;

use crate::cdp::client::{CdpClient, CdpClientError};
use crate::cdp::types::{AXNode, GetFullAXTreeResult};
use crate::element_ref::ElementRef;

/// Result of taking an a11y tree snapshot.
pub struct Snapshot {
    /// Formatted text output for the agent.
    pub text: String,
    /// uid → ElementRef mapping for subsequent actions.
    pub uid_map: HashMap<String, ElementRef>,
}

/// Take an accessibility tree snapshot of the current page.
///
/// Calls `Accessibility.getFullAXTree` via CDP, formats the tree into
/// a compact text representation with uid identifiers, and builds the
/// uid → ElementRef mapping.
pub async fn take_snapshot(
    client: &CdpClient,
    verbose: bool,
) -> Result<Snapshot, CdpClientError> {
    // Enable accessibility domain
    client
        .send("Accessibility.enable", serde_json::json!({}))
        .await?;

    let result: GetFullAXTreeResult = client
        .call("Accessibility.getFullAXTree", serde_json::json!({}))
        .await?;

    let (text, uid_map) = format_ax_tree(&result.nodes, verbose);

    Ok(Snapshot { text, uid_map })
}

/// Format AXNode list into indented text + uid map.
///
/// CDP returns a flat list of AXNodes with parent/child relationships
/// via `parentId` and `childIds`. We reconstruct the tree and format it.
fn format_ax_tree(nodes: &[AXNode], verbose: bool) -> (String, HashMap<String, ElementRef>) {
    let mut uid_map = HashMap::new();
    let mut output = String::new();
    let mut uid_counter: u32 = 0;

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

    if let Some(root_id) = root_id {
        format_node(
            root_id,
            &node_by_id,
            0,
            verbose,
            &mut uid_counter,
            &mut uid_map,
            &mut output,
        );
    }

    (output, uid_map)
}

fn format_node(
    node_id: &str,
    nodes: &HashMap<&str, &AXNode>,
    depth: usize,
    verbose: bool,
    uid_counter: &mut u32,
    uid_map: &mut HashMap<String, ElementRef>,
    output: &mut String,
) {
    let Some(node) = nodes.get(node_id) else {
        return;
    };

    // Skip ignored nodes unless verbose
    if node.ignored && !verbose {
        // Still recurse into children — some ignored nodes have visible children
        if let Some(child_ids) = &node.child_ids {
            for child_id in child_ids {
                format_node(child_id, nodes, depth, verbose, uid_counter, uid_map, output);
            }
        }
        return;
    }

    let role = node.role_name().unwrap_or("");
    let name = node.name_value().unwrap_or("");

    // Skip "none" role nodes unless verbose
    if role == "none" && !verbose {
        if let Some(child_ids) = &node.child_ids {
            for child_id in child_ids {
                format_node(child_id, nodes, depth, verbose, uid_counter, uid_map, output);
            }
        }
        return;
    }

    // Skip generic containers with no name unless verbose
    if !verbose && role == "generic" && name.is_empty() {
        if let Some(child_ids) = &node.child_ids {
            for child_id in child_ids {
                format_node(child_id, nodes, depth, verbose, uid_counter, uid_map, output);
            }
        }
        return;
    }

    // Assign uid
    *uid_counter += 1;
    let uid = format!("e{uid_counter}");

    // Register in uid_map if we have a backendDOMNodeId
    if let Some(backend_id) = node.backend_dom_node_id {
        uid_map.insert(uid.clone(), ElementRef::backend_node(backend_id));
    }

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
        output.push_str(name);
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
                    if prop_val.and_then(|v| v.as_bool()).unwrap_or(false) {
                        output.push_str(" focused");
                    }
                }
                "disabled" => {
                    if prop_val.and_then(|v| v.as_bool()).unwrap_or(false) {
                        output.push_str(" disabled");
                    }
                }
                "expanded" => {
                    if prop_val.and_then(|v| v.as_bool()).unwrap_or(false) {
                        output.push_str(" expanded");
                    }
                }
                "selected" => {
                    if prop_val.and_then(|v| v.as_bool()).unwrap_or(false) {
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
                    if let Some(level) = prop_val.and_then(|v| v.as_u64()) {
                        output.push_str(&format!(" level={level}"));
                    }
                }
                "required" => {
                    if prop_val.and_then(|v| v.as_bool()).unwrap_or(false) {
                        output.push_str(" required");
                    }
                }
                "readonly" => {
                    if prop_val.and_then(|v| v.as_bool()).unwrap_or(false) {
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

    // Recurse children
    if let Some(child_ids) = &node.child_ids {
        for child_id in child_ids {
            format_node(
                child_id,
                nodes,
                depth + 1,
                verbose,
                uid_counter,
                uid_map,
                output,
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

        let (text, uid_map) = format_ax_tree(&nodes, false);
        assert!(text.contains("uid=e1 heading \"Welcome\" level=1"));
        assert!(uid_map.contains_key("e1"));
        assert_eq!(uid_map["e1"].backend_node_id(), Some(10));
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

        let (text, uid_map) = format_ax_tree(&nodes, false);
        assert!(!text.contains("ignored"));
        assert!(text.contains("uid=e1 button \"Click me\" focused"));
        assert_eq!(uid_map.len(), 1);
    }
}
