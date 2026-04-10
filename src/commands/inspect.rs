use std::collections::{HashMap, HashSet};

use serde_json::json;

use crate::cdp::client::CdpClient;
use crate::element_ref::ElementRef;
use crate::snapshot::Snapshot;

pub async fn run(
    client: &CdpClient,
    verbose: bool,
    max_depth: Option<usize>,
    focus_uid: Option<&str>,
    role_filter: Option<&[&str]>,
) -> Result<Snapshot, crate::BoxError> {
    let snapshot = crate::snapshot::take_snapshot(client, verbose, max_depth, focus_uid, role_filter).await?;
    Ok(snapshot)
}

/// Scroll and collect unique filtered items from virtualized lists (X.com, etc.).
/// Takes repeated snapshots while scrolling, deduplicates by text content,
/// stops when `limit` unique items are collected or no new items appear.
pub async fn scroll_collect(
    client: &CdpClient,
    verbose: bool,
    focus_uid: Option<&str>,
    role_filter: Option<&[&str]>,
    limit: usize,
) -> Result<Snapshot, crate::BoxError> {
    let mut collected: Vec<String> = Vec::new();
    let mut seen = HashSet::new();
    let mut uid_map: HashMap<String, ElementRef> = HashMap::new();
    let max_scrolls = limit * 3;
    let mut stale_count = 0;

    for _ in 0..max_scrolls {
        let snapshot = crate::snapshot::take_snapshot(client, verbose, None, focus_uid, role_filter).await?;
        let prev_len = collected.len();
        for line in snapshot.text.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() && seen.insert(trimmed.to_string()) {
                collected.push(trimmed.to_string());
            }
        }
        uid_map.extend(snapshot.uid_map);

        if collected.len() >= limit { break; }

        // If no new items found after scroll, stop (end of list)
        if collected.len() == prev_len {
            stale_count += 1;
            if stale_count >= 3 { break; }
        } else {
            stale_count = 0;
        }

        // Scroll down one viewport, then wait for DOM mutations to settle
        let _ = client.call::<_, serde_json::Value>(
            "Runtime.evaluate",
            json!({
                "expression": r"(async () => {
                    window.scrollBy(0, window.innerHeight);
                    await new Promise(resolve => {
                        let timer = setTimeout(resolve, 2000);
                        const obs = new MutationObserver(() => {
                            clearTimeout(timer);
                            timer = setTimeout(() => { obs.disconnect(); resolve(); }, 400);
                        });
                        obs.observe(document.body || document.documentElement, { childList: true, subtree: true });
                    });
                })()",
                "awaitPromise": true,
                "returnByValue": true,
            }),
        ).await;
    }

    collected.truncate(limit);
    let text = format!("{}\n({} items collected)", collected.join("\n"), collected.len());
    Ok(Snapshot { text, uid_map })
}

/// Post-process snapshot text to resolve and append href URLs on link nodes.
pub async fn resolve_urls(
    client: &CdpClient,
    text: &str,
    uid_map: &HashMap<String, ElementRef>,
) -> String {
    let mut result = String::with_capacity(text.len());
    for line in text.lines() {
        result.push_str(line);
        // Match lines like "uid=n42 link "Some text""
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("uid=")
            && let Some((uid, after_uid)) = rest.split_once(' ') {
                let role = after_uid.split([' ', '"']).next().unwrap_or("");
                if role == "link"
                    && let Some(element_ref) = uid_map.get(uid)
                        && let Some(backend_id) = element_ref.backend_node_id()
                            && let Ok(href) = resolve_href(client, backend_id).await
                                && !href.is_empty() {
                                    result.push_str(&format!(" url=\"{href}\""));
                                }
            }
        result.push('\n');
    }
    result
}

async fn resolve_href(client: &CdpClient, backend_node_id: i64) -> Result<String, crate::BoxError> {
    let resolved: crate::cdp::types::ResolveNodeResult = client
        .call("DOM.resolveNode", crate::cdp::types::ResolveNodeParams {
            node_id: None,
            backend_node_id: Some(backend_node_id),
            object_group: Some("chrome-agent-urls".into()),
            execution_context_id: None,
        })
        .await?;
    let object_id = resolved.object.object_id.ok_or("no objectId")?;
    let result: serde_json::Value = client
        .call("Runtime.callFunctionOn", json!({
            "objectId": object_id,
            "functionDeclaration": "function() { return this.href || this.closest('a')?.href || ''; }",
            "returnByValue": true,
        }))
        .await?;
    let href = result.get("result")
        .and_then(|r| r.get("value"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    Ok(href.to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn url_append_only_on_links() {
        let text = "uid=n1 heading \"Title\"\nuid=n2 link \"Click me\"\nuid=n3 button \"OK\"\n";
        // Without CDP, we just verify the parsing logic identifies link lines
        for line in text.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("uid=") {
                if let Some((_uid, after_uid)) = rest.split_once(' ') {
                    let role = after_uid.split([' ', '"']).next().unwrap_or("");
                    if role == "link" {
                        assert!(line.contains("Click me"));
                    }
                }
            }
        }
    }
}
