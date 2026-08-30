//! What a snapshot may not print.
//!
//! `inspect` prints the a11y tree and every action report quotes it inside `delta`, so a
//! field's value reaches stdout through here. Chrome masks `type=password` itself; the rest
//! of `element::SECRET_FIELD` (a card number in a `type=text` field) does not.
//!
//! Secret-ness is a property of the ELEMENT and the a11y tree carries no `type` and no
//! `autocomplete`, so the page is asked: one scan with `element::SECRET_FIELD` as the
//! predicate, then one round trip per secret field FOUND. Nothing is asked when the tree
//! holds no value to hide.

use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};

use crate::cdp::client::CdpClient;
use crate::cdp::types::AXNode;

/// What stands in for a value the tree may not print. Fixed, never derived from the value:
/// two snapshots of an unchanged secret field must compare equal, or every action report
/// would claim the field changed.
pub const MARKER: &str = "<redacted>";

/// Shortest secret searched for OUTSIDE the field holding it. The field's own node is
/// redacted by identity at any length. Below four characters an echo search is meaningless:
/// a security code of `123` also appears in a price, a date and a street number.
const MIN_SEARCHABLE: usize = 4;

/// Above this many secret fields, redact every value instead of locating each one — locating
/// one costs a CDP round trip and the probe runs on every snapshot.
const MAX_SECRET_FIELDS: usize = 32;

/// The object group the probe's handles live in, released before the probe returns.
const OBJECT_GROUP: &str = "chrome-agent-secret";

/// Which rendered strings the snapshot must replace with [`MARKER`].
#[derive(Default)]
pub struct Redaction {
    /// Nodes whose `value=` is a secret. Their accessible name is a label, and stays.
    values: HashSet<i64>,
    /// Nodes whose accessible NAME is the secret itself: Chrome exposes an input's editable
    /// content as a `generic` child whose name is the value.
    texts: HashSet<i64>,
    /// The secret strings, to catch a page echoing one elsewhere (a checkout showing the card
    /// it will charge). Fails safe: an unrelated node with the same string is redacted too.
    strings: Vec<String>,
}

impl Redaction {
    pub fn none() -> Self {
        Self::default()
    }

    pub fn value<'a>(&'a self, backend: Option<i64>, value: &'a str) -> &'a str {
        match backend {
            // A value with no DOM node behind it cannot be classified, so it is a secret.
            None => MARKER,
            Some(id) if self.values.contains(&id) => MARKER,
            _ => self.scrub(value),
        }
    }

    pub fn name<'a>(&'a self, backend: Option<i64>, name: &'a str) -> &'a str {
        if backend.is_some_and(|id| self.texts.contains(&id)) {
            return MARKER;
        }
        self.scrub(name)
    }

    /// Replace the whole string when it carries a secret, not just the matching slice: a
    /// partially masked card number is still a card number minus a substring.
    fn scrub<'a>(&'a self, s: &'a str) -> &'a str {
        if self
            .strings
            .iter()
            .any(|secret| s.contains(secret.as_str()))
        {
            MARKER
        } else {
            s
        }
    }

    fn hide_string(&mut self, s: &str) {
        if s.chars().count() >= MIN_SEARCHABLE && !self.strings.iter().any(|k| k == s) {
            self.strings.push(s.to_string());
        }
    }

    /// Build a redaction directly, for the rendering tests.
    #[cfg(test)]
    pub fn for_tests(values: &[i64], texts: &[i64], strings: &[&str]) -> Self {
        Self {
            values: values.iter().copied().collect(),
            texts: texts.iter().copied().collect(),
            strings: strings.iter().map(|s| (*s).to_string()).collect(),
        }
    }
}

/// A node that will render a `value=`, and therefore has to be classified.
struct Candidate {
    node_id: String,
    backend: i64,
    value: String,
}

/// Decide what the tree may print, before a line of it is rendered.
/// Costs nothing on a page with no filled field.
pub async fn probe(client: &CdpClient, nodes: &[AXNode]) -> Redaction {
    let candidates = candidates(nodes);
    if candidates.is_empty() {
        return Redaction::none();
    }
    build(nodes, &candidates, secret_nodes(client).await.as_ref())
}

/// Turn the page's answer into a redaction. `secret == None` means the page could not be
/// asked. Pure, so the `None` branch is testable without a browser.
fn build(nodes: &[AXNode], candidates: &[Candidate], secret: Option<&HashSet<i64>>) -> Redaction {
    let by_id: HashMap<&str, &AXNode> = nodes.iter().map(|n| (n.node_id.as_str(), n)).collect();
    let mut redaction = Redaction::default();
    for candidate in candidates {
        // Fails closed: when the page could not be asked, every value it holds is a secret.
        if secret.is_some_and(|ids| !ids.contains(&candidate.backend)) {
            continue;
        }
        redaction.values.insert(candidate.backend);
        redaction.hide_string(&candidate.value);
        hide_subtree(&by_id, &candidate.node_id, &mut redaction);
    }
    redaction
}

/// Every node that renders a non-empty `value=`.
fn candidates(nodes: &[AXNode]) -> Vec<Candidate> {
    let mut seen = HashSet::new();
    nodes
        .iter()
        .filter_map(|node| {
            let value = node
                .value
                .as_ref()
                .and_then(|v| v.value.as_ref())
                .and_then(|v| v.as_str())
                .filter(|v| !v.is_empty())?;
            let backend = node.backend_dom_node_id?;
            seen.insert(backend).then(|| Candidate {
                node_id: node.node_id.clone(),
                backend,
                value: value.to_string(),
            })
        })
        .collect()
}

/// Mark the descendants of a secret field: their text IS its value.
fn hide_subtree(by_id: &HashMap<&str, &AXNode>, node_id: &str, redaction: &mut Redaction) {
    let Some(node) = by_id.get(node_id) else {
        return;
    };
    let Some(child_ids) = &node.child_ids else {
        return;
    };
    for child_id in child_ids {
        if let Some(child) = by_id.get(child_id.as_str()) {
            if let Some(backend) = child.backend_dom_node_id {
                redaction.texts.insert(backend);
            }
            let child_value = child
                .value
                .as_ref()
                .and_then(|v| v.value.as_ref())
                .and_then(|v| v.as_str());
            for text in [child.name_value(), child_value].into_iter().flatten() {
                redaction.hide_string(text);
            }
        }
        hide_subtree(by_id, child_id, redaction);
    }
}

/// Which nodes on the page are secret fields, as backend node ids. `None` means the question
/// could not be answered, and the caller then redacts every value in the tree.
///
/// One scan for the whole page, then one round trip per SECRET field found — not per
/// value-carrying node, which measured +171ms on a form of 60 filled inputs, on every action.
async fn secret_nodes(client: &CdpClient) -> Option<HashSet<i64>> {
    let handles = scan(client).await?;
    let mut ids = HashSet::new();
    let mut complete = handles.len() <= MAX_SECRET_FIELDS;
    if complete {
        for handle in &handles {
            match backend_id(client, handle).await {
                Some(id) => {
                    ids.insert(id);
                }
                // A secret field we cannot name is one we cannot mask by identity.
                None => complete = false,
            }
        }
    }
    let _ = client
        .call::<_, Value>(
            "Runtime.releaseObjectGroup",
            json!({"objectGroup": OBJECT_GROUP}),
        )
        .await;
    complete.then_some(ids)
}

/// One in-page pass with `element::SECRET_FIELD` as the predicate, returning a JS handle per
/// match. Descends into same-origin iframes, which is exactly the set
/// `Accessibility.getFullAXTree` reports on — cross-origin frames are out-of-process.
async fn scan(client: &CdpClient) -> Option<Vec<String>> {
    let expression = format!(
        r"(() => {{
            const found = [];
            const walk = (doc) => {{
                for (const el of doc.querySelectorAll('input,textarea,select,[autocomplete]')) {{
                    if ({predicate}) found.push(el);
                }}
                for (const frame of doc.querySelectorAll('iframe')) {{
                    try {{ if (frame.contentDocument) walk(frame.contentDocument); }} catch (e) {{}}
                }}
            }};
            walk(document);
            return found;
        }})()",
        predicate = crate::element::SECRET_FIELD
    );
    let mut params = json!({
        "expression": expression,
        "objectGroup": OBJECT_GROUP,
        "returnByValue": false,
    });
    // Scope to the bound frame, like `eval` and `inspect`: the tree being rendered is that
    // frame's, so the scan must run there too.
    if let Some(ctx) = client.frame_context() {
        params["contextId"] = json!(ctx.context_id);
    }
    let result: Value = client.call("Runtime.evaluate", params).await.ok()?;
    // Discarded on purpose: `None` makes `build` redact every value on the page, and there is no
    // channel here to say why — the caller renders a tree, it does not report errors. A message
    // would have to be printed beside the redaction, which is louder than the fact it explains.
    if crate::element::js_exception(&result).is_some() {
        return None;
    }
    let array = result.get("result")?.get("objectId")?.as_str()?;
    let properties: Value = client
        .call(
            "Runtime.getProperties",
            json!({"objectId": array, "ownProperties": true}),
        )
        .await
        .ok()?;
    let entries = properties.get("result")?.as_array()?;
    let mut handles = Vec::new();
    for entry in entries {
        // Skip `length` and anything else that is not an element handle.
        if entry
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|n| n.parse::<usize>().is_ok())
            && let Some(id) = entry
                .get("value")
                .and_then(|v| v.get("objectId"))
                .and_then(Value::as_str)
        {
            handles.push(id.to_string());
        }
    }
    Some(handles)
}

async fn backend_id(client: &CdpClient, object_id: &str) -> Option<i64> {
    let described: Value = client
        .call("DOM.describeNode", json!({"objectId": object_id}))
        .await
        .ok()?;
    described.get("node")?.get("backendNodeId")?.as_i64()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_value_renders_as_the_marker_and_keeps_its_label() {
        let r = Redaction::for_tests(&[2], &[16], &["4111111111111111"]);
        assert_eq!(r.value(Some(2), "4111111111111111"), MARKER);
        // The label is what the agent aims by, and it is not the secret.
        assert_eq!(r.name(Some(2), "Card number"), "Card number");
        // Chrome's editable-content child, whose NAME is the value.
        assert_eq!(r.name(Some(16), "4111111111111111"), MARKER);
    }

    #[test]
    fn an_ordinary_value_is_untouched() {
        let r = Redaction::for_tests(&[2], &[16], &["4111111111111111"]);
        assert_eq!(r.value(Some(6), "leave at the door"), "leave at the door");
        assert_eq!(
            r.name(Some(6), "Note for the courier"),
            "Note for the courier"
        );
    }

    #[test]
    fn an_echo_of_the_secret_is_redacted_wherever_it_appears() {
        // A checkout echoing the card it will charge: same digits, never a value=.
        let r = Redaction::for_tests(&[2], &[], &["4111111111111111"]);
        assert_eq!(r.name(Some(26), "4111111111111111"), MARKER);
        assert_eq!(r.name(Some(26), "Charging card 4111111111111111"), MARKER);
    }

    #[test]
    fn a_value_with_no_dom_node_behind_it_is_redacted() {
        // `e{n}` uids have no backendDOMNodeId, so nothing can be asked about them.
        let r = Redaction::none();
        assert_eq!(r.value(None, "whatever"), MARKER);
    }

    #[test]
    fn nothing_is_hidden_when_nothing_was_classified() {
        let r = Redaction::none();
        assert_eq!(r.value(Some(2), "hello@example.com"), "hello@example.com");
        assert_eq!(r.name(Some(2), "Email"), "Email");
    }

    #[test]
    fn the_marker_does_not_depend_on_the_value_it_hides() {
        // A marker that varied would turn every action report into a report of change.
        let r = Redaction::for_tests(&[2], &[], &[]);
        assert_eq!(
            r.value(Some(2), "4111111111111111"),
            r.value(Some(2), "4242424242424242")
        );
    }

    #[test]
    fn a_short_secret_is_not_searched_for_across_the_page() {
        let mut r = Redaction::default();
        r.hide_string("123");
        r.hide_string("7391");
        assert_eq!(r.name(Some(9), "Order 123 of 4"), "Order 123 of 4");
        assert_eq!(r.name(Some(9), "7391"), MARKER);
    }

    #[test]
    fn candidates_are_the_value_carrying_nodes_only() {
        let nodes = vec![
            ax(1, "1", None, Some("4111111111111111")),
            ax(2, "2", Some("Submit"), None),
            ax(3, "3", None, Some("")),
        ];
        let found = candidates(&nodes);
        assert_eq!(found.len(), 1, "only the filled field costs a round trip");
        assert_eq!(found[0].backend, 1);
    }

    #[test]
    fn an_unanswered_question_redacts_every_value() {
        // The scan threw, or a secret field could not be named: nothing is known, so
        // nothing is printed.
        let nodes = [
            ax(2, "a", Some("Card number"), Some("4111111111111111")),
            ax(6, "b", Some("Note"), Some("leave at the door")),
        ];
        let found = candidates(&nodes);
        let r = build(&nodes, &found, None);
        assert_eq!(r.value(Some(2), "4111111111111111"), MARKER);
        assert_eq!(r.value(Some(6), "leave at the door"), MARKER);
        // Names survive, so an agent can still aim at the fields.
        assert_eq!(r.name(Some(6), "Note"), "Note");
    }

    #[test]
    fn an_answered_question_redacts_only_what_it_named() {
        let nodes = [
            ax(2, "a", Some("Card number"), Some("4111111111111111")),
            ax(6, "b", Some("Note"), Some("leave at the door")),
        ];
        let found = candidates(&nodes);
        let r = build(&nodes, &found, Some(&HashSet::from([2])));
        assert_eq!(r.value(Some(2), "4111111111111111"), MARKER);
        assert_eq!(r.value(Some(6), "leave at the door"), "leave at the door");
    }

    #[test]
    fn a_subtree_of_a_secret_field_is_hidden_with_it() {
        let mut parent = ax(2, "p", Some("Card number"), Some("4111111111111111"));
        parent.child_ids = Some(vec!["c".into()]);
        let child = ax(16, "c", Some("4111111111111111"), None);
        let nodes = [parent, child];
        let by_id: HashMap<&str, &AXNode> = nodes.iter().map(|n| (n.node_id.as_str(), n)).collect();
        let mut r = Redaction::default();
        hide_subtree(&by_id, "p", &mut r);
        assert!(r.texts.contains(&16));
        assert_eq!(r.name(Some(16), "4111111111111111"), MARKER);
    }

    fn ax(backend: i64, node_id: &str, name: Option<&str>, value: Option<&str>) -> AXNode {
        use crate::cdp::types::AXValue;
        let wrap = |s: &str| AXValue {
            value: Some(Value::String(s.into())),
        };
        AXNode {
            node_id: node_id.into(),
            ignored: false,
            role: None,
            name: name.map(wrap),
            value: value.map(wrap),
            properties: None,
            child_ids: None,
            backend_dom_node_id: Some(backend),
            parent_id: None,
        }
    }
}
