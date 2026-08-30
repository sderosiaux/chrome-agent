//! Render an accessibility tree into the compact text a snapshot is.
//!
//! Pure: `&[AXNode]` in, text plus uid maps out, no CDP call and no I/O. That is what lets
//! `snapshot::take_views` render ONE reading twice — full depth for the diff baseline, and
//! again through the caller's `--filter`/`--max-depth`/`--uid`.

use std::collections::HashMap;

use crate::cdp::types::AXNode;
use crate::element_ref::ElementRef;
use crate::snapshot::Redaction;

/// Format a flat `AXNode` list (CDP links them by `parentId`/`childIds`) into indented text
/// plus the uid map.
///
/// `focus_uid` triggers a full pass to assign uids, then a re-render of that node's subtree
/// from depth 0, so numbering matches a normal inspect.
///
/// `preassigned` maps an `AXNode` id to the `e{n}` uid a previous rendering of the SAME tree
/// gave it; anonymous uids are counted in traversal order, so a rendering that skips nodes
/// would renumber the rest. The third return value is this rendering's own assignment.
pub fn format_ax_tree(
    nodes: &[AXNode],
    verbose: bool,
    max_depth: Option<usize>,
    focus_uid: Option<&str>,
    role_filter: Option<&[&str]>,
    redaction: &Redaction,
    preassigned: Option<&HashMap<String, String>>,
) -> (String, HashMap<String, ElementRef>, HashMap<String, String>) {
    let node_by_id: HashMap<&str, &AXNode> = nodes
        .iter()
        .map(|n| (n.node_id.as_str(), n))
        .collect();

    let root_id = nodes
        .iter()
        .find(|n| n.parent_id.is_none())
        .map(|n| n.node_id.as_str());

    let Some(root_id) = root_id else {
        return (String::new(), HashMap::new(), HashMap::new());
    };

    if let Some(focus) = focus_uid {
        // First pass: assign uids with no depth limit, and map uid → nodeId so the subtree
        // root can be found.
        let mut uid_map_full = HashMap::new();
        let mut uid_counter: u32 = 0;
        let mut discard = String::new();
        let mut uid_to_node_id: HashMap<String, String> = HashMap::new();
        let mut anon_full: HashMap<String, String> = HashMap::new();
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
            redaction,
            preassigned,
            &mut anon_full,
        );

        let focus_node_id = uid_to_node_id.get(focus);
        if let Some(focus_node_id) = focus_node_id {
            let mut uid_map = HashMap::new();
            let mut output = String::new();
            let mut uid_counter2: u32 = 0;
            let mut tracking2: HashMap<String, String> = HashMap::new();
            let mut anon2: HashMap<String, String> = HashMap::new();
            // The subtree inherits the caller's assignment, or the full pass above; otherwise
            // it restarts the counter and hands `e1` to an already-named node.
            let inherited = preassigned.unwrap_or(&anon_full);
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
                redaction,
                Some(inherited),
                &mut anon2,
            );
            return (apply_role_filter(output, role_filter, max_depth), uid_map, anon2);
        }

        // uid not found. Do NOT route this through the role filter: the message begins with
        // "uid=" and would be stripped as a non-matching node, leaving empty output.
        return (
            format!("uid={focus} not found in accessibility tree\n"),
            uid_map_full,
            anon_full,
        );
    }

    let mut uid_map = HashMap::new();
    let mut output = String::new();
    let mut uid_counter: u32 = 0;
    let mut anon: HashMap<String, String> = HashMap::new();
    format_node(
        root_id,
        &node_by_id,
        0,
        verbose,
        max_depth,
        &mut uid_counter,
        &mut uid_map,
        &mut output,
        redaction,
        preassigned,
        &mut anon,
    );

    let output = apply_role_filter(output, role_filter, max_depth);

    (output, uid_map, anon)
}

/// Keep only lines whose role matches `role_filter`, expanding aliases. Returns `output`
/// unchanged when no filter is requested, and a hint rather than empty output when the
/// filter matches nothing under a `max_depth`.
///
/// Applied on every rendering path including the `focus_uid` subtree, so
/// `inspect --uid nN --filter button` scopes to both the subtree and the role.
fn apply_role_filter(output: String, role_filter: Option<&[&str]>, max_depth: Option<usize>) -> String {
    let Some(roles) = role_filter else {
        return output;
    };
    // Aliases, so agents need not know exact ARIA role names.
    let expanded: Vec<String> = roles.iter().flat_map(|&r| {
        let mut v = vec![(*r).to_string()];
        match r.to_lowercase().as_str() {
            "textbox" => { v.push("searchbox".into()); v.push("combobox".into()); }
            "input" => {
                for r in ["textbox", "searchbox", "combobox", "checkbox", "radio", "slider", "spinbutton", "switch"] {
                    v.push(r.into());
                }
            }
            "button" => { v.push("menuitem".into()); }
            _ => {}
        }
        v
    }).collect();
    let filtered: String = output
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            if let Some(after_uid) = trimmed.strip_prefix("uid=")
                && let Some(rest) = after_uid.split_once(' ') {
                    let role = rest.1.split([' ', '"']).next().unwrap_or("");
                    return expanded.iter().any(|r| r.eq_ignore_ascii_case(role));
                }
            false
        })
        .fold(String::new(), |mut acc, line| {
            acc.push_str(line.trim_start());
            acc.push('\n');
            acc
        });
    // No match under a depth limit usually means the elements are deeper; say so rather
    // than print nothing.
    if filtered.is_empty() && max_depth.is_some() {
        format!("No elements matching filter {:?} found within --max-depth {}. Try increasing depth or removing --max-depth.\n",
            roles, max_depth.unwrap_or(0))
    } else {
        filtered
    }
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
    redaction: &Redaction,
    preassigned: Option<&HashMap<String, String>>,
    anon: &mut HashMap<String, String>,
) {
    let mut discard: HashMap<String, String> = HashMap::new();
    format_node_with_tracking(
        node_id, nodes, depth, verbose, max_depth, uid_counter, uid_map, output, &mut discard,
        redaction, preassigned, anon,
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
    redaction: &Redaction,
    preassigned: Option<&HashMap<String, String>>,
    anon: &mut HashMap<String, String>,
) {
    let Some(node) = nodes.get(node_id) else {
        return;
    };

    // Skip ignored nodes unless verbose, but still recurse: they can have visible children.
    if node.ignored && !verbose {
        if let Some(child_ids) = &node.child_ids {
            for child_id in child_ids {
                format_node_with_tracking(child_id, nodes, depth, verbose, max_depth, uid_counter, uid_map, output, uid_to_node_id, redaction, preassigned, anon);
            }
        }
        return;
    }

    let role = node.role_name().unwrap_or("");
    let mut name = node.name_value().unwrap_or("").to_string();

    // Noise roles repeat parent content; skipping them cuts tokens ~66%.
    const NOISE_ROLES: &[&str] = &["none", "StaticText", "InlineTextBox"];
    if !verbose && NOISE_ROLES.contains(&role) {
        if let Some(child_ids) = &node.child_ids {
            for child_id in child_ids {
                format_node_with_tracking(child_id, nodes, depth, verbose, max_depth, uid_counter, uid_map, output, uid_to_node_id, redaction, preassigned, anon);
            }
        }
        return;
    }

    // Unnamed node with noise filtered: recover its text from StaticText children.
    if !verbose && name.is_empty()
        && let Some(child_ids) = &node.child_ids {
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

    if !verbose && role == "generic" && name.is_empty() {
        if let Some(child_ids) = &node.child_ids {
            for child_id in child_ids {
                format_node_with_tracking(child_id, nodes, depth, verbose, max_depth, uid_counter, uid_map, output, uid_to_node_id, redaction, preassigned, anon);
            }
        }
        return;
    }

    // Stable `n{backendNodeId}` when available, else a positional `e{n}` that a truncated
    // traversal would renumber — so `preassigned` wins whenever it knows this node.
    let uid = if let Some(backend_id) = node.backend_dom_node_id {
        let uid = format!("n{backend_id}");
        uid_map.insert(uid.clone(), ElementRef::backend_node(backend_id));
        uid
    } else if let Some(existing) = preassigned.and_then(|m| m.get(node_id)) {
        existing.clone()
    } else {
        *uid_counter += 1;
        let uid = format!("e{uid_counter}");
        anon.insert(node_id.to_string(), uid.clone());
        uid
    };

    uid_to_node_id.insert(uid.clone(), node_id.to_string());

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
        output.push_str(redaction.name(node.backend_dom_node_id, &name));
        output.push('"');
    }

    // Input values. The redaction marker stays INSIDE the quotes: `diff` reads a value via
    // the `value="` prefix, and an unquoted marker would read as an empty field.
    if let Some(value_ax) = &node.value
        && let Some(val) = value_ax.value.as_ref().and_then(|v| v.as_str())
            && !val.is_empty() {
                output.push_str(" value=\"");
                output.push_str(redaction.value(node.backend_dom_node_id, val));
                output.push('"');
            }

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
                    if let Some(val) = prop_val.and_then(|v| v.as_str())
                        && val != "false" {
                            output.push_str(" checked=");
                            output.push_str(val);
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
                    if verbose
                        && let Some(val) = prop_val {
                            output.push(' ');
                            output.push_str(&prop.name);
                            output.push('=');
                            match val {
                                serde_json::Value::Bool(b) => output.push_str(&b.to_string()),
                                serde_json::Value::Number(n) => output.push_str(&n.to_string()),
                                serde_json::Value::String(s) => {
                                    output.push('"');
                                    output.push_str(redaction.name(node.backend_dom_node_id, s));
                                    output.push('"');
                                }
                                _ => output.push_str(&val.to_string()),
                            }
                        }
                }
            }
        }
    }

    output.push('\n');

    if let Some(max) = max_depth
        && depth >= max {
            return;
        }

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
                redaction,
                preassigned,
                anon,
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
            value: Some(serde_json::Value::String(s.into())),
        }
    }

    fn default_ax_node() -> AXNode {
        AXNode {
            node_id: String::new(),
            ignored: false,
            role: None,
            name: None,
            value: None,
            properties: None,
            child_ids: None,
            backend_dom_node_id: None,
            parent_id: None,
        }
    }

    fn make_bool_prop(name: &str, val: bool) -> AXProperty {
        AXProperty {
            name: name.into(),
            value: AXValue {
                value: Some(serde_json::Value::Bool(val)),
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
                value: None,
                properties: Some(vec![AXProperty {
                    name: "level".into(),
                    value: AXValue {
                        value: Some(serde_json::json!(1)),
                    },
                }]),
                child_ids: Some(vec![]),
                backend_dom_node_id: Some(10),
                parent_id: None,
            },
        ];

        let (text, uid_map, _) = format_ax_tree(&nodes, false, None, None, None, &Redaction::none(), None);
        assert!(text.contains("uid=n10 heading \"Welcome\" level=1"));
        assert!(uid_map.contains_key("n10"));
        assert_eq!(uid_map["n10"].backend_node_id(), Some(10));
    }

    #[test]
    fn input_alias_covers_all_input_roles() {
        // Contract: "input→all input roles". A checkbox dropped by --filter input reads
        // as a page with no such control.
        let output = "uid=n1 textbox \"Name\"\n\
                      uid=n2 checkbox \"Agree\"\n\
                      uid=n3 radio \"Choice A\"\n\
                      uid=n4 slider \"Volume\"\n\
                      uid=n5 spinbutton \"Qty\"\n\
                      uid=n6 switch \"Dark mode\"\n\
                      uid=n7 searchbox \"Search\"\n\
                      uid=n8 combobox \"Country\"\n\
                      uid=n9 button \"Go\"\n"
            .to_string();
        let filtered = apply_role_filter(output, Some(&["input"]), None);
        for role in ["textbox", "checkbox", "radio", "slider", "spinbutton", "switch", "searchbox", "combobox"] {
            assert!(filtered.contains(role), "input alias should keep role {role}, got:\n{filtered}");
        }
        assert!(!filtered.contains("uid=n9"), "input alias must not keep non-input roles, got:\n{filtered}");
    }

    #[test]
    fn skips_ignored_nodes() {
        let nodes = vec![
            AXNode {
                node_id: "1".into(),
                ignored: true,
                role: None,
                name: None,
                value: None,
                properties: None,
                child_ids: Some(vec!["2".into()]),
                backend_dom_node_id: None,
                parent_id: None,
            },
            AXNode {
                node_id: "2".into(),
                ignored: false,
                role: Some(make_ax_value("button")),
                name: Some(make_ax_value("Click me")),
                value: None,
                properties: Some(vec![make_bool_prop("focused", true)]),
                child_ids: Some(vec![]),
                backend_dom_node_id: Some(20),
                parent_id: Some("1".into()),
            },
        ];

        let (text, uid_map, _) = format_ax_tree(&nodes, false, None, None, None, &Redaction::none(), None);
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
        let (text, _, _) = format_ax_tree(&nodes, false, Some(1), None, None, &Redaction::none(), None);
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
        let (text, _, _) = format_ax_tree(&nodes, false, None, Some("n3"), None, &Redaction::none(), None);
        assert!(text.contains("Submit"));
        assert!(!text.contains("Title"));
    }

    #[test]
    fn focus_uid_applies_role_filter() {
        // `inspect --uid n1 --filter button` must return only button-role descendants,
        // not the whole subtree.
        let nodes = vec![
            AXNode {
                node_id: "1".into(),
                role: Some(make_ax_value("form")),
                name: Some(make_ax_value("Signup")),
                child_ids: Some(vec!["2".into(), "3".into()]),
                parent_id: None,
                backend_dom_node_id: Some(1),
                ..default_ax_node()
            },
            AXNode {
                node_id: "2".into(),
                role: Some(make_ax_value("heading")),
                name: Some(make_ax_value("Please sign up")),
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
        let (text, _, _) = format_ax_tree(&nodes, false, None, Some("n1"), Some(&["button"]), &Redaction::none(), None);
        assert!(text.contains("uid=n3 button \"Submit\""));
        assert!(!text.contains("Please sign up")); // heading filtered out
        assert!(!text.contains("form")); // focus root also filtered (not a button)
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
        let (text, _, _) = format_ax_tree(&nodes, false, None, Some("e99"), None, &Redaction::none(), None);
        assert!(text.contains("not found"));
    }

    #[test]
    fn bug_empty_tree() {
        let nodes: Vec<AXNode> = vec![];
        let (text, uid_map, _) = format_ax_tree(&nodes, false, None, None, None, &Redaction::none(), None);
        assert!(text.is_empty());
        assert!(uid_map.is_empty());
    }

    #[test]
    fn bug_all_ignored_nodes() {
        let nodes = vec![
            AXNode {
                node_id: "1".into(),
                ignored: true,
                child_ids: Some(vec!["2".into()]),
                parent_id: None,
                ..default_ax_node()
            },
            AXNode {
                node_id: "2".into(),
                ignored: true,
                child_ids: Some(vec![]),
                parent_id: Some("1".into()),
                ..default_ax_node()
            },
        ];
        let (text, uid_map, _) = format_ax_tree(&nodes, false, None, None, None, &Redaction::none(), None);
        assert!(text.is_empty());
        assert!(uid_map.is_empty());
    }

    #[test]
    fn bug_filter_no_match() {
        let nodes = vec![AXNode {
            node_id: "1".into(),
            role: Some(make_ax_value("heading")),
            name: Some(make_ax_value("Title")),
            child_ids: Some(vec![]),
            parent_id: None,
            backend_dom_node_id: Some(1),
            ..default_ax_node()
        }];
        let (text, _, _) = format_ax_tree(&nodes, false, None, None, Some(&["button"]), &Redaction::none(), None);
        assert!(text.is_empty() || !text.contains("heading"));
    }

    /// A tree whose anonymous nodes sit at different depths, so a truncated traversal
    /// reaches them in a different order than a full one.
    fn tree_with_anonymous_nodes() -> Vec<AXNode> {
        vec![
            AXNode {
                node_id: "1".into(),
                role: Some(make_ax_value("WebArea")),
                name: Some(make_ax_value("Page")),
                child_ids: Some(vec!["2".into(), "4".into()]),
                parent_id: None,
                backend_dom_node_id: Some(1),
                ..default_ax_node()
            },
            AXNode {
                node_id: "2".into(),
                role: Some(make_ax_value("region")),
                name: Some(make_ax_value("Deep branch")),
                child_ids: Some(vec!["3".into()]),
                parent_id: Some("1".into()),
                backend_dom_node_id: Some(2),
                ..default_ax_node()
            },
            // No backendDOMNodeId, and only reachable at depth 2.
            AXNode {
                node_id: "3".into(),
                role: Some(make_ax_value("banner")),
                name: Some(make_ax_value("Deep anonymous")),
                child_ids: Some(vec![]),
                parent_id: Some("2".into()),
                backend_dom_node_id: None,
                ..default_ax_node()
            },
            // No backendDOMNodeId, reachable at depth 1.
            AXNode {
                node_id: "4".into(),
                role: Some(make_ax_value("contentinfo")),
                name: Some(make_ax_value("Shallow anonymous")),
                child_ids: Some(vec![]),
                parent_id: Some("1".into()),
                backend_dom_node_id: None,
                ..default_ax_node()
            },
        ]
    }

    #[test]
    fn anonymous_uids_are_renumbered_by_a_truncated_traversal() {
        // The behaviour preassignment exists to correct: the deep node takes `e1` on a full
        // walk and disappears at depth 1, moving the shallow node from `e2` to `e1`.
        let nodes = tree_with_anonymous_nodes();
        let (full, _, _) = format_ax_tree(&nodes, false, None, None, None, &Redaction::none(), None);
        assert!(full.contains("uid=e1 banner \"Deep anonymous\""), "got {full}");
        assert!(full.contains("uid=e2 contentinfo \"Shallow anonymous\""), "got {full}");

        let (shallow, _, _) =
            format_ax_tree(&nodes, false, Some(1), None, None, &Redaction::none(), None);
        assert!(shallow.contains("uid=e1 contentinfo \"Shallow anonymous\""), "got {shallow}");
    }

    #[test]
    fn a_preassigned_anonymous_uid_survives_a_truncated_traversal() {
        // What `take_views` relies on: the reduced view inherits the baseline's numbering,
        // so the uid printed and the uid stored name the same node.
        let nodes = tree_with_anonymous_nodes();
        let (_, _, anon) = format_ax_tree(&nodes, false, None, None, None, &Redaction::none(), None);
        let (shallow, _, _) =
            format_ax_tree(&nodes, false, Some(1), None, None, &Redaction::none(), Some(&anon));
        assert!(
            shallow.contains("uid=e2 contentinfo \"Shallow anonymous\""),
            "the reduced view must keep the number the full view gave this node, got {shallow}"
        );
        assert!(!shallow.contains("uid=e1"), "e1 belongs to the deep node, got {shallow}");
    }

}
