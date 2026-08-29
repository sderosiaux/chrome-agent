use std::collections::HashMap;

use crate::cdp::client::{CdpClient, CdpClientError};
use crate::cdp::types::{AXNode, GetFullAXTreeResult};
use crate::element_ref::ElementRef;
use crate::snapshot_render::format_ax_tree;
pub use crate::snapshot_secret::Redaction;

/// Result of taking an a11y tree snapshot.
pub struct Snapshot {
    /// Formatted text output for the agent.
    pub text: String,
    /// uid → `ElementRef` mapping for subsequent actions.
    pub uid_map: HashMap<String, ElementRef>,
    /// `(frameId, loaderId)` of the document. The loader id changes on every document
    /// load and only then, which is what the URL could never express: it stays put across
    /// a reload and moves on a fragment jump.
    pub identity: Option<(String, String)>,
}

/// Read `(frameId, loaderId)` for the frame we are acting in.
///
/// `Page.getFrameTree` rather than an event subscription: `CdpClient::events()` is a
/// broadcast that only delivers messages received after subscribing, and several commands
/// never subscribe at all, so an event-derived identity would be blind for them.
pub async fn document_identity(client: &CdpClient) -> Option<(String, String)> {
    let tree: serde_json::Value = client.call("Page.getFrameTree", serde_json::json!({})).await.ok()?;
    let root = tree.get("frameTree")?;
    let wanted = client.frame_context().map(|c| c.frame_id);
    find_frame(root, wanted.as_deref())
}

/// Walk the frame tree for the bound frame, falling back to the root.
fn find_frame(node: &serde_json::Value, wanted: Option<&str>) -> Option<(String, String)> {
    let frame = node.get("frame")?;
    let id = frame.get("id")?.as_str()?.to_string();
    let loader = frame.get("loaderId")?.as_str()?.to_string();
    if wanted.is_none_or(|w| w == id) {
        return Some((id, loader));
    }
    node.get("childFrames")?
        .as_array()?
        .iter()
        .find_map(|child| find_frame(child, wanted))
}

/// Wait for the DOM to stop changing, bounded at both ends.
///
/// Replaces a blind `sleep`: a page that does not react returns in about a quiet window
/// instead of paying the whole budget, and a page that never settles still returns.
/// Measured on the Hacker News front page, this took the default action report from
/// +181ms down to the time the page actually needs.
pub async fn settle(client: &CdpClient, quiet_ms: u32, hard_ms: u32) {
    let expression = format!(
        r"new Promise(resolve => {{
            let settled = false, quiet = null, obs = null;
            const finish = () => {{
                if (settled) return;
                settled = true;
                clearTimeout(quiet);
                clearTimeout(hard);
                if (obs) obs.disconnect();
                resolve();
            }};
            quiet = setTimeout(finish, {quiet_ms});
            const hard = setTimeout(finish, {hard_ms});
            obs = new MutationObserver(() => {{
                clearTimeout(quiet);
                quiet = setTimeout(finish, {quiet_ms});
            }});
            obs.observe(document.body || document.documentElement, {{
                childList: true, subtree: true, attributes: true, characterData: true
            }});
        }})"
    );
    let _ = client
        .call::<_, serde_json::Value>(
            "Runtime.evaluate",
            serde_json::json!({
                "expression": expression,
                "awaitPromise": true,
                "returnByValue": true,
            }),
        )
        .await;
}

/// One accessibility tree, rendered twice: the baseline and what the caller asked to see.
///
/// Every display flag on `inspect` — `--filter`, `--max-depth`, `--uid`, and downstream
/// `--urls` and `--max-chars`/`--offset` — narrows the page. Persisting the narrowed
/// rendering as `last_snapshot` makes the next `diff` compare the whole page against an
/// amputated one and report every node the flag dropped as an addition. Measured on
/// `tests/fixtures/snapshot_filter_baseline.html`: `inspect --filter button` then one
/// injected button then `diff` answered `added=13` where the truth is 1.
///
/// So the two readings are separated here rather than at each call site, and they come from
/// ONE `getFullAXTree`: taking a second tree for the display would let the page move between
/// them, and then the snapshot printed is not the baseline stored.
pub struct Views {
    /// Full depth, no focus, no role filter. This is what gets persisted.
    pub full: Snapshot,
    /// The reduced rendering, or `None` when the caller asked for no reduction — the full
    /// text is then what they asked to see, and rendering it twice would be waste.
    shown: Option<String>,
}

impl Views {
    /// The text to print or return.
    pub fn shown(&self) -> &str {
        self.shown.as_deref().unwrap_or(&self.full.text)
    }

    /// Build from an already-taken full snapshot and a rendering of the same tree.
    ///
    /// `scroll_collect` uses this: its collected text is a union over scroll positions,
    /// which is not a page state and can never be a baseline.
    pub const fn from_parts(full: Snapshot, shown: Option<String>) -> Self {
        Self { full, shown }
    }
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
    role_filter: Option<&[&str]>,
) -> Result<Snapshot, CdpClientError> {
    let (nodes, redaction) = fetch_tree(client).await?;
    let (text, uid_map, _) =
        format_ax_tree(&nodes, verbose, max_depth, focus_uid, role_filter, &redaction, None);
    let identity = document_identity(client).await;

    Ok(Snapshot { text, uid_map, identity })
}

/// Take one tree and render both the baseline and the caller's view of it.
///
/// The `uid_map` persisted is the FULL one, deliberately: a display flag should not decide
/// which nodes the next command may act on, and `inspect --max-depth 1` used to leave a map
/// holding four uids on a page with thirteen. Anonymous `e{n}` uids are the one thing that
/// does not survive a re-render — they are numbered in traversal order and a truncated
/// traversal renumbers them — so the reduced view inherits the full view's numbering
/// (`anon` below) instead of restarting its counter. Without that, the `e1` printed and the
/// `e1` stored would be different nodes.
pub async fn take_views(
    client: &CdpClient,
    verbose: bool,
    max_depth: Option<usize>,
    focus_uid: Option<&str>,
    role_filter: Option<&[&str]>,
) -> Result<Views, CdpClientError> {
    let (nodes, redaction) = fetch_tree(client).await?;
    let (text, uid_map, anon) =
        format_ax_tree(&nodes, verbose, None, None, None, &redaction, None);
    let identity = document_identity(client).await;
    let full = Snapshot { text, uid_map, identity };

    let reduced = max_depth.is_some() || focus_uid.is_some() || role_filter.is_some();
    let shown = reduced.then(|| {
        format_ax_tree(&nodes, verbose, max_depth, focus_uid, role_filter, &redaction, Some(&anon)).0
    });
    Ok(Views { full, shown })
}

/// Read the accessibility tree and the secret-field redaction that applies to it.
///
/// Split out so the baseline and the caller's view are two renderings of ONE reading.
async fn fetch_tree(client: &CdpClient) -> Result<(Vec<AXNode>, Redaction), CdpClientError> {
    // Enable accessibility domain
    client
        .send("Accessibility.enable", serde_json::json!({}))
        .await?;

    // Scope to the frame bound by the `frame` command, if any (issue #8).
    // Omitting `frameId` yields the root frame's tree, preserving prior behavior.
    let mut params = serde_json::json!({});
    if let Some(ctx) = client.frame_context() {
        params["frameId"] = serde_json::json!(ctx.frame_id);
    }
    let result: GetFullAXTreeResult = client
        .call("Accessibility.getFullAXTree", params)
        .await?;

    // Before a line is rendered: what the tree holds cannot say whether a field is a secret,
    // and a value printed once is on stdout, in the transcript and in any `--record` file.
    let redaction = crate::snapshot_secret::probe(client, &result.nodes).await;
    Ok((result.nodes, redaction))
}
#[cfg(test)]
mod tests {
    #[test]
    fn bug_content_center_empty_quad() {
        use crate::cdp::types::BoxModel;
        let model = BoxModel {
            content: vec![],  // empty quad
            padding: vec![],
            border: vec![],
            margin: vec![],
            width: 0,
            height: 0,
        };
        let (x, y) = model.content_center();
        assert!(x.abs() < f64::EPSILON);
        assert!(y.abs() < f64::EPSILON);
    }
}
