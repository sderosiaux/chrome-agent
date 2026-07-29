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

/// Commands that can move the page, and therefore owe the caller a change report.
pub fn mutates_page(cmd: &str) -> bool {
    matches!(
        cmd,
        "click" | "tap" | "dblclick" | "double_click" | "double-click"
            | "fill" | "type" | "press" | "select" | "check" | "uncheck"
            | "upload" | "drag" | "hover" | "scroll"
            | "fill-form" | "fill_form" | "fillform"
            | "fill_and_submit" | "fill-and-submit"
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
        crate::run_helpers::attach_verdict(
            out,
            crate::verdict::classify(crate::verdict::Observation::ReadFailed),
        );
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
        crate::run_helpers::attach_verdict(
            out,
            crate::verdict::classify(crate::verdict::Observation::NoBaseline),
        );
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
    crate::run_helpers::attach_verdict(
        out,
        crate::verdict::classify(crate::verdict::Observation::Compared {
            document_changed: cmp.document_changed,
            identity_known: cmp.identity_known,
            edits: cmp.added + cmp.removed + cmp.changed,
            moved: cmp.moved,
            focus_moved: cmp.focus_from.is_some() || cmp.focus_to.is_some(),
        }),
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
