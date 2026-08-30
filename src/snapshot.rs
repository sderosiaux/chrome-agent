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
    /// `(frameId, loaderId)` of the document. The loader id changes on every document load
    /// and only then: stable across a reload, unlike the URL, and moved by a fragment jump.
    pub identity: Option<(String, String)>,
}

/// Read `(frameId, loaderId)` for the frame we are acting in.
///
/// Polls `Page.getFrameTree` rather than subscribing: `CdpClient::events()` only delivers
/// messages received after subscribing, and several commands never subscribe.
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

/// Wait for the DOM to stop changing: returns after `quiet_ms` without a mutation, or at
/// `hard_ms` regardless. Bounded at both ends so a static page pays only the quiet window
/// and a continuously-mutating page still returns.
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
/// A display flag (`--filter`, `--max-depth`, `--uid`, `--urls`) must never narrow the
/// persisted baseline, or the next `diff` reports every dropped node as an addition. Both
/// renderings come from ONE `getFullAXTree`: a second read lets the page move between them.
pub struct Views {
    /// Full depth, no focus, no role filter. This is what gets persisted.
    pub full: Snapshot,
    /// The reduced rendering, or `None` when the caller asked for no reduction.
    shown: Option<String>,
}

impl Views {
    /// The text to print or return.
    pub fn shown(&self) -> &str {
        self.shown.as_deref().unwrap_or(&self.full.text)
    }

    /// Build from an already-taken full snapshot and a rendering of the same tree.
    /// Used by `scroll_collect`, whose text is a union over scroll positions and so is
    /// never a page state.
    pub const fn from_parts(full: Snapshot, shown: Option<String>) -> Self {
        Self { full, shown }
    }
}

/// Take an accessibility tree snapshot: compact text plus the uid → `ElementRef` map.
///
/// `focus_uid` scopes output to that element's subtree; `max_depth` limits depth
/// (0 = root only). For callers that also STORE a baseline, use `take_views`.
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
/// The persisted `uid_map` is deliberately the FULL one: a display flag must not decide
/// which nodes the next command may act on. Anonymous `e{n}` uids are numbered in traversal
/// order, so the reduced view inherits the full view's numbering (`anon`) rather than
/// restarting its counter — otherwise the `e1` printed and the `e1` stored differ.
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
async fn fetch_tree(client: &CdpClient) -> Result<(Vec<AXNode>, Redaction), CdpClientError> {
    client
        .send("Accessibility.enable", serde_json::json!({}))
        .await?;

    // Scope to the frame bound by `frame`, if any. Omitting `frameId` yields the root frame.
    let mut params = serde_json::json!({});
    if let Some(ctx) = client.frame_context() {
        params["frameId"] = serde_json::json!(ctx.frame_id);
    }
    let result: GetFullAXTreeResult = client
        .call("Accessibility.getFullAXTree", params)
        .await?;

    // Must happen before any line is rendered: the tree alone cannot say whether a field
    // holds a secret, and a value printed once is on stdout and in any `--record` file.
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
            border: vec![],
        };
        let (x, y) = model.content_center();
        assert!(x.abs() < f64::EPSILON);
        assert!(y.abs() < f64::EPSILON);
    }
}
