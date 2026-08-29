use std::collections::{HashMap, HashSet};

use serde_json::json;

use crate::cdp::client::CdpClient;
use crate::element_ref::ElementRef;
use crate::snapshot::{Snapshot, Views};

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

/// One reading of the page, rendered twice: the full baseline to persist and the reduced
/// view the caller asked for.
///
/// Every caller that both *shows* a tree and *stores* one goes through here, so the rule
/// that a display flag never reaches the baseline is stated once — see `snapshot::Views`.
pub async fn views(
    client: &CdpClient,
    verbose: bool,
    max_depth: Option<usize>,
    focus_uid: Option<&str>,
    role_filter: Option<&[&str]>,
) -> Result<Views, crate::BoxError> {
    Ok(crate::snapshot::take_views(client, verbose, max_depth, focus_uid, role_filter).await?)
}

/// Scroll and collect unique filtered items from virtualized lists (X.com, etc.).
/// Takes repeated snapshots while scrolling, deduplicates by text content,
/// stops when `limit` unique items are collected or no new items appear.
///
/// The collected text is a UNION over scroll positions: it never described the page at any
/// one moment, so it cannot be a diff baseline. `Views::full` is therefore a fresh, full
/// reading taken once the scrolling has stopped — the page as it now stands — while the
/// union stays what the caller is shown. Its `uid_map` keeps the union too: an item that
/// scrolled out of a virtualized list is gone from the final tree, and dropping its uid
/// would take away the only handle the caller was given for it.
pub async fn scroll_collect(
    client: &CdpClient,
    verbose: bool,
    focus_uid: Option<&str>,
    role_filter: Option<&[&str]>,
    limit: usize,
) -> Result<Views, crate::BoxError> {
    let mut collected: Vec<String> = Vec::new();
    let mut seen = HashSet::new();
    let mut uid_map: HashMap<String, ElementRef> = HashMap::new();
    let max_scrolls = limit * 3;
    let mut stale_count = 0;
    // `limit * 3` bounds the iterations, not the time: each one costs a settle window, so a
    // page that keeps producing items turns `--limit 500` into 1500 rounds of up to two
    // seconds. The caller's `--timeout` is the answer to "how long am I willing to wait",
    // so it bounds the whole collection too.
    let deadline = std::time::Instant::now() + client.call_timeout();
    let mut ran_out_of_time = false;

    for _ in 0..max_scrolls {
        if std::time::Instant::now() >= deadline {
            ran_out_of_time = true;
            break;
        }
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

        // Scroll down one viewport, then wait for DOM mutations to settle.
        //
        // The wait used to be an inline promise whose 400ms debounce re-armed on every
        // mutation with no ceiling: on a page that mutates forever — a ticker, a live feed,
        // a rotating ad slot — it never resolved, and `awaitPromise` then held the whole
        // command open. `snapshot::settle` is the same debounce with a hard timer that
        // nothing clears, so a live page costs the ceiling instead of the session.
        let _ = client
            .call::<_, serde_json::Value>(
                "Runtime.evaluate",
                json!({
                    "expression": "window.scrollBy(0, window.innerHeight)",
                    "returnByValue": true,
                }),
            )
            .await;
        crate::snapshot::settle(client, 400, 2000).await;
    }

    collected.truncate(limit);
    // Say when the list is short because time ran out rather than because the page ended:
    // the two look identical in the output otherwise.
    let note = if ran_out_of_time {
        format!(
            "\n({} items collected; stopped at the {}s --timeout while the page was still producing items)",
            collected.len(),
            client.call_timeout().as_secs()
        )
    } else {
        format!("\n({} items collected)", collected.len())
    };
    let text = format!("{}{note}", collected.join("\n"));

    // One more full reading, after the scrolling: the baseline has to be a page state, and
    // the union above is not one. Costs a single `getFullAXTree` on a path that already
    // spent up to `limit * 3` of them.
    let mut full = crate::snapshot::take_snapshot(client, verbose, None, None, None).await?;
    // The final tree wins on any uid it also holds; the union keeps the ones it alone saw.
    for (uid, element) in full.uid_map {
        uid_map.insert(uid, element);
    }
    full.uid_map = uid_map;
    Ok(Views::from_parts(full, Some(text)))
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

/// Result of paging/capping a rendered snapshot for display.
pub struct Paged {
    /// The (possibly windowed) text, with a truncation note appended when truncated.
    pub text: String,
    /// Total number of characters in the full snapshot.
    pub total_chars: usize,
    /// Whether characters were dropped after the returned window.
    pub truncated: bool,
    /// Offset to pass on the next call to continue paging (only when truncated).
    pub next_offset: Option<usize>,
}

/// Apply char-based paging to a rendered snapshot.
///
/// `offset` skips the first N characters (stable because uids are stable across
/// inspects); `max_chars` caps the returned window. UTF-8 safe — never slices a
/// multi-byte char. When the window is capped short of the end, a machine-readable
/// tail is appended so an agent knows how to continue or narrow.
///
/// When `offset == 0` and `max_chars` is `None`, the text is returned unchanged
/// (matches the no-cap default of `text`/`read`).
#[must_use]
pub fn paginate(text: &str, offset: usize, max_chars: Option<usize>) -> Paged {
    let total_chars = text.chars().count();

    if offset == 0 && max_chars.is_none() {
        return Paged { text: text.to_string(), total_chars, truncated: false, next_offset: None };
    }

    // Byte index of the offset-th char (clamped to end).
    let start_byte = text.char_indices().nth(offset).map_or(text.len(), |(i, _)| i);
    let window = &text[start_byte..];
    let window_chars = total_chars.saturating_sub(offset);

    let (shown, kept) = match max_chars {
        Some(max) if window_chars > max => {
            let end_byte = window.char_indices().nth(max).map_or(window.len(), |(i, _)| i);
            (&window[..end_byte], max)
        }
        _ => (window, window_chars),
    };

    let end_offset = offset + kept;
    let truncated = end_offset < total_chars;
    let mut out = shown.to_string();
    if truncated {
        let remaining = total_chars - end_offset;
        out.push_str(&format!(
            "\n... {remaining} chars truncated (total {total_chars}), re-run with --offset {end_offset} or narrow with --filter/--uid"
        ));
    }

    Paged {
        text: out,
        total_chars,
        truncated,
        next_offset: truncated.then_some(end_offset),
    }
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
    use super::paginate;

    #[test]
    fn paginate_no_cap_is_passthrough() {
        let text = "uid=n1 button \"A\"\nuid=n2 link \"B\"";
        let p = paginate(text, 0, None);
        assert_eq!(p.text, text);
        assert!(!p.truncated);
        assert_eq!(p.next_offset, None);
        assert_eq!(p.total_chars, text.chars().count());
    }

    #[test]
    fn paginate_caps_and_appends_tail() {
        let text = "abcdefghij"; // 10 chars
        let p = paginate(text, 0, Some(4));
        assert!(p.text.starts_with("abcd"));
        assert!(p.truncated);
        assert_eq!(p.next_offset, Some(4));
        assert!(p.text.contains("6 chars truncated"));
        assert!(p.text.contains("--offset 4"));
        assert!(p.text.contains("total 10"));
    }

    #[test]
    fn paginate_offset_windows_middle() {
        let text = "abcdefghij"; // 10 chars
        let p = paginate(text, 4, Some(3)); // chars 4..7 = "efg"
        assert!(p.text.starts_with("efg"));
        assert!(p.truncated);
        assert_eq!(p.next_offset, Some(7));
        assert!(p.text.contains("3 chars truncated"));
    }

    #[test]
    fn paginate_last_page_not_truncated() {
        let text = "abcdefghij"; // 10 chars
        let p = paginate(text, 8, Some(5)); // chars 8..10 = "ij", window shorter than cap
        assert_eq!(p.text, "ij");
        assert!(!p.truncated);
        assert_eq!(p.next_offset, None);
    }

    #[test]
    fn paginate_offset_past_end_is_empty() {
        let text = "abc";
        let p = paginate(text, 99, Some(10));
        assert_eq!(p.text, "");
        assert!(!p.truncated);
    }

    #[test]
    fn paginate_utf8_safe_no_panic() {
        // 3-byte chars: naive byte slicing would panic mid-char.
        let text = "日本語テストデータ"; // 9 chars
        let p = paginate(text, 2, Some(3)); // chars 2..5 = "語テス"
        assert_eq!(p.text.chars().take(3).collect::<String>(), "語テス");
        assert!(p.truncated);
        assert_eq!(p.next_offset, Some(5));
    }

    #[test]
    fn paginate_exact_fit_not_truncated() {
        // Guards the `window_chars > max` comparison against an off-by-one (> vs >=):
        // when the window exactly fills the cap, nothing is dropped and no tail is added.
        let text = "abcdefghij"; // 10 chars
        let p = paginate(text, 0, Some(10));
        assert_eq!(p.text, text);
        assert!(!p.truncated);
        assert_eq!(p.next_offset, None);

        // Same, offset into the middle: chars 4..10 = "efghij", cap == remaining.
        let p2 = paginate(text, 4, Some(6));
        assert_eq!(p2.text, "efghij");
        assert!(!p2.truncated);
        assert_eq!(p2.next_offset, None);
    }

    #[test]
    fn paginate_offset_only_no_cap() {
        let text = "abcdefghij";
        let p = paginate(text, 3, None); // chars 3.. = "defghij", no further truncation
        assert_eq!(p.text, "defghij");
        assert!(!p.truncated);
    }

    #[test]
    fn url_append_only_on_links() {
        let text = "uid=n1 heading \"Title\"\nuid=n2 link \"Click me\"\nuid=n3 button \"OK\"\n";
        // Without CDP, we just verify the parsing logic identifies link lines
        for line in text.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("uid=")
                && let Some((_uid, after_uid)) = rest.split_once(' ')
            {
                let role = after_uid.split([' ', '"']).next().unwrap_or("");
                if role == "link" {
                    assert!(line.contains("Click me"));
                }
            }
        }
    }
}
