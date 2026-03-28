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

        // Scroll down one viewport
        let _ = client.call::<_, serde_json::Value>(
            "Runtime.evaluate",
            json!({"expression": "window.scrollBy(0, window.innerHeight)", "returnByValue": true}),
        ).await;
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    }

    collected.truncate(limit);
    let text = format!("{}\n({} items collected)", collected.join("\n"), collected.len());
    Ok(Snapshot { text, uid_map })
}
