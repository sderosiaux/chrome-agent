//! What an action reports about the page once it ran, for pipe and batch.
//!
//! Split out of `pipe_dispatch.rs` for the 1000-line cap, and re-exported from it so the
//! dispatchers keep their existing call sites. This is the central hook the CLAUDE.md
//! design note describes: adding a mutating command means adding it to `mutates_page` and
//! nothing else.

use serde_json::{json, Value};

use crate::cdp::client::CdpClient;
use crate::commands;
use crate::session::{self, SessionStore};

/// What the action said about its own delivery, read back off the response it built.
///
/// The hit test runs inside the action, in `element`; the verdict is decided afterwards, in
/// three different places (CLI, pipe, batch). Passing the delivery through the response rather
/// than through every dispatcher signature is what keeps those three in agreement — and keeps
/// `mutates_page` the only thing a new command has to be added to.
///
/// A response with no `delivery` field is `NotProbed`: every non-mouse command, and any action
/// that predates this wiring. Absence of evidence, never a claim.
pub fn delivery_from_response(client: &CdpClient, obj: &Value) -> crate::verdict::Delivered {
    let Some(token) = obj.get("delivery").and_then(Value::as_str) else {
        return crate::verdict::Delivered::NOT_PROBED;
    };
    crate::verdict::Delivered {
        how: crate::verdict::Delivery::parse(token),
        modal_receiver: obj
            .get("intercepted_by")
            .and_then(|r| r.get("modal"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        // Measured from the dispatch, not from here: the window `no_effect` names has to be
        // the one the page actually had.
        observed_after_ms: client.ms_since_dispatch(),
    }
}

/// What the action's own read-back said, off the response it built.
///
/// Same reason as `delivery_from_response`: the read-back happens inside the action, the
/// verdict is settled afterwards in three different places, and carrying the answer on the
/// response is what keeps those three in agreement without a signature per command.
///
/// One key, `value`, for all four verbs that read a state back. `fill` reports
/// `value.verbatim`; `fill-form` and `fill_and_submit` report one per field under `values`, and
/// one field the page did not keep is enough — a form half-filled is not filled, and for
/// `fill_and_submit` those per-field reports are the only witness there is. `select` and
/// `check`/`uncheck` write the same object (`read_back::select_report`, `check_report`): they
/// perform the same measurement on a different kind of control, and while each REFUSES when the
/// read-back disagrees — so `Discarded` and `Rewritten` are unreachable from them — a
/// CONFIRMED state is evidence in exactly the way a confirmed fill is. They used to report the
/// window and nothing else, so the classifier saw no postcondition at all and a fresh session
/// answered `unknown / no_baseline` for an action whose own target had been measured. That is
/// the asymmetry this module exists to remove, in the same shape it removed it for `fill`.
///
/// A `check` that dispatched nothing (the element already held the state) deliberately carries
/// no `value`: there is no write of ours to have been kept, and `value_kept` there would be a
/// claim about a click that never happened.
///
/// A `verbatim` that is not a boolean is `NotRead`, not a failure: an unreadable field is an
/// absence of evidence, and this rung outranks the page read.
pub fn postcondition_from_response(out: &Value) -> crate::verdict::Postcondition {
    let Some(fields) = out.get("values").and_then(Value::as_array) else {
        return field_postcondition(out.get("value"));
    };
    // The worst of the fields decides, and `Discarded` is the worst: a form where one field
    // took nothing is not filled, whatever the others kept.
    let mut seen = crate::verdict::Postcondition::NotRead;
    for field in fields {
        match field_postcondition(field.get("value")) {
            crate::verdict::Postcondition::Discarded => return crate::verdict::Postcondition::Discarded,
            crate::verdict::Postcondition::Rewritten => seen = crate::verdict::Postcondition::Rewritten,
            crate::verdict::Postcondition::Kept
                if seen == crate::verdict::Postcondition::NotRead =>
            {
                seen = crate::verdict::Postcondition::Kept;
            }
            crate::verdict::Postcondition::Kept | crate::verdict::Postcondition::NotRead => {}
        }
    }
    seen
}

/// One field's `value` report, read as a postcondition.
///
/// Emptiness is what separates the two failures, and it is readable without the value: a
/// redacted secret reports `actual_length` in place of `actual`, so a password the page threw
/// away is classified the same way as any other field and nothing secret is read to do it.
fn field_postcondition(value: Option<&Value>) -> crate::verdict::Postcondition {
    use crate::verdict::Postcondition;

    let Some(value) = value else { return Postcondition::NotRead };
    match value.get("verbatim").and_then(Value::as_bool) {
        Some(true) => Postcondition::Kept,
        None => Postcondition::NotRead,
        Some(false) => {
            let empty = match value.get("actual_length").and_then(Value::as_u64) {
                Some(len) => len == 0,
                // No length means a plain field: `actual` is the string itself, and `null`
                // (an element with no `value` at all) counts as holding nothing.
                None => value
                    .get("actual")
                    .and_then(Value::as_str)
                    .is_none_or(str::is_empty),
            };
            if empty { Postcondition::Discarded } else { Postcondition::Rewritten }
        }
    }
}

/// How many lost values are reported before the list is cut off.
///
/// A page that clears fifty fields at once has said what it needed to say in the first few, and
/// each entry costs a pair of CDP calls to classify. The count is reported whatever the cap.
const LOST_VALUE_LIMIT: usize = 10;

/// Attach `values_lost` to the response and return how many there were.
///
/// The diff already knew a field had gone from holding something to holding nothing — the
/// `value=` token stops appearing after the `->` on its line. What it could not do is make that
/// contractual: an agent reading JSON saw `ok:true` and `verdict:"changed"`, both true, and
/// never learnt the field it had just filled was empty again.
///
/// Every entry is classified against `element::SECRET_FIELD`, the same predicate `fill` redacts
/// on, by resolving the node and asking the page. It FAILS CLOSED: a field whose kind could not
/// be read is redacted, because the alternative is printing a password. A redacted entry
/// carries no length either — the only length available is the one the accessibility tree
/// reported, and for a `type=password` that is the length of Chrome's mask, not of the value.
pub async fn attach_values_lost(
    client: &CdpClient,
    uid_map: &std::collections::HashMap<String, crate::element_ref::ElementRef>,
    lost: &[commands::diff::LostValue],
    out: &mut Value,
) -> usize {
    if lost.is_empty() {
        return 0;
    }
    let mut reported = Vec::new();
    for entry in lost.iter().take(LOST_VALUE_LIMIT) {
        let mut item = json!({"uid": entry.uid, "role": entry.role});
        if let Some(name) = &entry.name {
            item["name"] = json!(name);
        }
        if is_secret_field(client, uid_map, &entry.uid).await {
            item["redacted"] = json!(true);
        } else {
            item["was"] = json!(entry.was);
        }
        reported.push(item);
    }
    if let Some(obj) = out.as_object_mut() {
        obj.insert("values_lost".into(), Value::Array(reported));
        if lost.len() > LOST_VALUE_LIMIT {
            obj.insert("values_lost_total".into(), json!(lost.len()));
        }
    }
    lost.len()
}

/// Whether this uid names a field whose value must never be printed.
///
/// `true` on any failure: an unclassified field is treated as a secret.
async fn is_secret_field(
    client: &CdpClient,
    uid_map: &std::collections::HashMap<String, crate::element_ref::ElementRef>,
    uid: &str,
) -> bool {
    let Ok(resolved) = crate::element::resolve_uid(client, uid_map, uid).await else {
        return true;
    };
    let js = format!(
        "function() {{ const el = this; return !!{}; }}",
        crate::element::SECRET_FIELD
    );
    let Ok(result) = client
        .call::<_, Value>(
            "Runtime.callFunctionOn",
            json!({
                "objectId": resolved.object_id,
                "functionDeclaration": js,
                "returnByValue": true,
            }),
        )
        .await
    else {
        return true;
    };
    if result.get("exceptionDetails").is_some() {
        return true;
    }
    result
        .get("result")
        .and_then(|r| r.get("value"))
        .and_then(Value::as_bool)
        // A reply we cannot read is not a licence to print the value.
        .unwrap_or(true)
}

/// Decide and attach the verdict for one observation, reading the delivery and the
/// postcondition off the response.
///
/// The single place all three modes settle a verdict, so `no_effect` cannot appear in one of
/// them without the window it was measured over, and `not_kept` cannot be missed in another.
pub fn attach_verdict_for(
    client: &CdpClient,
    out: &mut Value,
    observation: crate::verdict::Observation,
) -> crate::verdict::Assessment {
    let delivered = delivery_from_response(client, out);
    let assessment =
        crate::verdict::classify(observation, delivered, postcondition_from_response(out));
    if assessment.verdict == crate::verdict::Verdict::NoEffect
        && let Some(ms) = delivered.observed_after_ms
        && let Some(map) = out.as_object_mut()
    {
        // `or_insert`: a command with its own read-back window (check, select) already
        // reported a narrower, more specific one, and that claim is the stronger of the two.
        map.entry("observed_after_ms").or_insert_with(|| json!(ms));
    }
    // What the action spent waiting, when it waited. Attached here because this is the one
    // place all three modes pass through, and absent otherwise: a field that reads `waited_ms: 0`
    // on every fast action is a field nobody reads on the one action that took ten seconds.
    if let Some(ms) = client.take_settle_wait_ms()
        && let Some(map) = out.as_object_mut()
    {
        map.insert("waited_ms".into(), json!(ms));
    }
    crate::run_helpers::attach_verdict(out, assessment);
    assessment
}

/// Commands that can move the page, and therefore owe the caller a change report.
///
/// `webmcp_call` is here and `webmcp_list` is not: calling a tool is the one `WebMCP` action that
/// can move the page, and its declared result carries no schema to check against (no
/// `outputSchema` exists in the protocol) — the accessibility-tree delta this hook attaches is
/// the only corroboration available, exactly the reason this predicate exists. Listing tools is
/// a read, like `assert`.
pub fn mutates_page(cmd: &str) -> bool {
    matches!(
        cmd,
        "click" | "tap" | "dblclick" | "double_click" | "double-click"
            | "fill" | "type" | "press" | "select" | "check" | "uncheck"
            | "upload" | "drag" | "hover" | "scroll"
            | "fill-form" | "fill_form" | "fillform"
            | "fill_and_submit" | "fill-and-submit"
            | "webmcp_call" | "webmcp-call"
    )
}

/// Re-read the page after an action and say what moved, mirroring the CLI default.
///
/// Failures here are swallowed on purpose: the action itself already succeeded, and losing
/// the report is a smaller problem than turning a successful action into an error.
pub async fn attach_change_report(
    client: &CdpClient,
    store: &mut SessionStore,
    browser_name: &str,
    page_name: &str,
    target_id: &str,
    report: crate::run_helpers::ReportPolicy,
    old_text: Option<&str>,
    stored: Option<(String, String)>,
    out: &mut Value,
) {
    crate::snapshot::settle(client, 100, 1000).await;
    let Ok(snapshot) = commands::inspect::run(client, false, None, None, None).await else {
        // The action landed and the read did not. Saying nothing here is what made this
        // indistinguishable from a page that did not move.
        attach_verdict_for(client, out, crate::verdict::Observation::ReadFailed);
        return;
    };
    // Store the fresh snapshot whatever happens: without this the very first action of a
    // session had no baseline, so it wrote none, so the session never acquired one and the
    // change report stayed silently off for its whole life.
    let Some(old_text) = old_text else {
        if let Some(browser_s) = store.browsers.get_mut(browser_name) {
            let page = session::ensure_page(browser_s, page_name, target_id);
            page.uid_map = snapshot.uid_map;
            page.last_snapshot = Some(snapshot.text);
            let (f, l) = snapshot.identity.map_or((None, None), |(f, l)| (Some(f), Some(l)));
            page.last_snapshot_frame = f;
            page.last_snapshot_loader = l;
        }
        attach_verdict_for(client, out, crate::verdict::Observation::NoBaseline);
        return;
    };
    let identity = commands::diff::Identity::from_loader(
        stored.as_ref().map(|(f, l)| (f.as_str(), l.as_str())),
        snapshot.identity.as_ref().map(|(f, l)| (f.as_str(), l.as_str())),
    );
    let cmp = commands::diff::compare(identity, old_text, &snapshot.text);
    let body = if report.budget == 0 {
        cmp.text.clone()
    } else {
        crate::truncate::truncate_str(
            cmp.text.trim_end(),
            report.budget,
            "\n… truncated, send {\"cmd\":\"inspect\"} for the rest",
        )
        .into_owned()
    };
    if let Some(obj) = out.as_object_mut() {
        obj.insert(
            "changed".into(),
            json!({
                "added": cmp.added,
                "removed": cmp.removed,
                "changed": cmp.changed,
                "unchanged": cmp.unchanged,
                    "moved": cmp.moved,
                    "anonymous": cmp.anonymous,
                "document_changed": cmp.document_changed,
                    "identity_known": cmp.identity_known,
            }),
        );
        obj.insert("delta".into(), json!(body));
        if cmp.focus_from.is_some() || cmp.focus_to.is_some() {
            obj.insert("focus".into(), json!({"from": cmp.focus_from, "to": cmp.focus_to}));
        }
        if let Some(hint) = cmp.hint {
            obj.entry("hint").or_insert_with(|| json!(hint));
        }
    }
    // Before the verdict: it is one of the classifier's inputs, and it is read off the fresh
    // uid_map, which the store does not own yet.
    let values_lost = attach_values_lost(client, &snapshot.uid_map, &cmp.values_lost, out).await;
    attach_verdict_for(
        client,
        out,
        crate::verdict::Observation::Compared {
            document_changed: cmp.document_changed,
            identity_known: cmp.identity_known,
            edits: cmp.added + cmp.removed + cmp.changed,
            moved: cmp.moved,
            // The document taking focus is not evidence an element received the click.
            // `focus_only` claims the action ARRIVED somewhere, and it is the only such
            // claim available on a path with no hit test (`--xy`, a JS click, a target in a
            // frame). Chrome marks the RootWebArea `focused` whenever `<body>` holds focus,
            // which is what a click on nothing focusable produces — and what the FIRST click
            // anywhere in a fresh page produces, including one that hit nothing at all. So a
            // move whose only content is "the document gained focus" cannot separate the two
            // and must not license the word. A blur is still counted: if a real element LOST
            // focus, something of ours reached the page. The `focus` field itself is
            // untouched, because the reading is true — see `diff::focus_to_document`.
            focus_moved: cmp.focus_from.is_some()
                || (cmp.focus_to.is_some() && !cmp.focus_to_document),
            values_lost,
        },
    );
    if let Some(browser_s) = store.browsers.get_mut(browser_name) {
        let page = session::ensure_page(browser_s, page_name, target_id);
        page.uid_map = snapshot.uid_map;
        page.last_snapshot = Some(snapshot.text);
            let (f, l) = snapshot.identity.map_or((None, None), |(f, l)| (Some(f), Some(l)));
            page.last_snapshot_frame = f;
            page.last_snapshot_loader = l;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verdict::Postcondition;

    /// The shape `run_helpers::fill_value_report` writes for a plain field.
    fn value(requested: &str, actual: Option<&str>) -> Value {
        json!({
            "requested": requested,
            "actual": actual,
            "verbatim": actual == Some(requested),
            "observed_after_ms": 60,
        })
    }

    #[test]
    fn a_fill_the_page_kept_reads_as_kept() {
        let out = json!({"ok": true, "value": value("ada@example.com", Some("ada@example.com"))});
        assert_eq!(postcondition_from_response(&out), Postcondition::Kept);
    }

    /// `form_value_microtask_revert.html`: the page emptied the field.
    #[test]
    fn an_emptied_field_reads_as_discarded() {
        let out = json!({"ok": true, "value": value("hello@example.com", Some(""))});
        assert_eq!(postcondition_from_response(&out), Postcondition::Discarded);
        // An element with no value at all holds nothing either.
        let out = json!({"ok": true, "value": value("x", None)});
        assert_eq!(postcondition_from_response(&out), Postcondition::Discarded);
    }

    /// `form_value_phone_mask.html`: the write landed, in the page's own shape.
    #[test]
    fn a_reformatted_field_reads_as_rewritten() {
        let out = json!({"ok": true, "value": value("5551234567", Some("(555) 123-4567"))});
        assert_eq!(postcondition_from_response(&out), Postcondition::Rewritten);
    }

    /// A secret is redacted down to `verbatim` and two lengths. That is enough to classify it,
    /// which is the point: a password the page threw away must not be the one silent case.
    #[test]
    fn a_redacted_secret_is_classified_from_its_lengths_alone() {
        let kept = json!({"ok": true, "value": {
            "redacted": true, "requested_length": 12, "actual_length": 12, "verbatim": true,
        }});
        assert_eq!(postcondition_from_response(&kept), Postcondition::Kept);
        let emptied = json!({"ok": true, "value": {
            "redacted": true, "requested_length": 12, "actual_length": 0, "verbatim": false,
        }});
        assert_eq!(postcondition_from_response(&emptied), Postcondition::Discarded);
        let rewritten = json!({"ok": true, "value": {
            "redacted": true, "requested_length": 12, "actual_length": 8, "verbatim": false,
        }});
        assert_eq!(postcondition_from_response(&rewritten), Postcondition::Rewritten);
    }

    /// A bulk fill is judged on its worst field: a form with one empty field is not filled,
    /// whatever the others kept.
    #[test]
    fn a_bulk_fill_is_judged_on_its_worst_field() {
        let all_kept = json!({"ok": true, "values": [
            {"uid": "n1", "value": value("a", Some("a"))},
            {"uid": "n2", "value": value("b", Some("b"))},
        ]});
        assert_eq!(postcondition_from_response(&all_kept), Postcondition::Kept);

        let one_masked = json!({"ok": true, "values": [
            {"uid": "n1", "value": value("a", Some("a"))},
            {"uid": "n2", "value": value("5551234567", Some("(555) 123-4567"))},
        ]});
        assert_eq!(postcondition_from_response(&one_masked), Postcondition::Rewritten);

        let one_emptied = json!({"ok": true, "values": [
            {"uid": "n1", "value": value("5551234567", Some("(555) 123-4567"))},
            {"uid": "n2", "value": value("b", Some(""))},
        ]});
        assert_eq!(postcondition_from_response(&one_emptied), Postcondition::Discarded);
    }

    /// Every command with nothing to read back, and any response that lost the field. This is
    /// the rung that outranks the page read, so an absence here must never read as a failure.
    #[test]
    fn a_response_with_no_read_back_claims_nothing() {
        for out in [
            json!({"ok": true, "message": "Clicked uid=n12"}),
            json!({"ok": true, "value": {"requested": "x", "actual": "x"}}),
            json!({"ok": true, "value": "not an object"}),
            json!({"ok": true, "values": []}),
            json!({"ok": true, "values": [{"uid": "n1"}]}),
        ] {
            assert_eq!(postcondition_from_response(&out), Postcondition::NotRead, "for {out}");
        }
    }
}
