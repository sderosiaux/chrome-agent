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

/// One reading of the page rendered twice: the full baseline to persist, and the reduced view
/// the caller asked for. Every caller that both shows and stores a tree goes through here, so
/// a display flag can never reach the baseline.
pub async fn views(
    client: &CdpClient,
    verbose: bool,
    max_depth: Option<usize>,
    focus_uid: Option<&str>,
    role_filter: Option<&[&str]>,
) -> Result<Views, crate::BoxError> {
    Ok(crate::snapshot::take_views(client, verbose, max_depth, focus_uid, role_filter).await?)
}

/// Collect unique items from a virtualized list by snapshotting while scrolling, deduplicated
/// by text, until `limit` items or three barren rounds.
///
/// The collected text is a UNION over scroll positions and never described the page at one
/// moment, so it cannot be a diff baseline: `Views::full` is a fresh reading taken after the
/// scrolling stops. The `uid_map` keeps the union, since an item that scrolled out of the
/// final tree still needs a handle.
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
    // `limit * 3` bounds iterations, not time: each costs a settle window of up to 2s. The
    // caller's `--timeout` bounds the whole collection.
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

        // Three scrolls with no new item means the end of the list.
        if collected.len() == prev_len {
            stale_count += 1;
            if stale_count >= 3 { break; }
        } else {
            stale_count = 0;
        }

        // Scroll one viewport, then settle: a 400 ms debounce under a 2000 ms ceiling that no
        // mutation clears, so a continuously-mutating page costs the ceiling, not the session.
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
    // A list cut short by the timeout and one cut short by the page ending look identical
    // otherwise.
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

    // One more full reading: the baseline has to be a page state, and the union is not one.
    let mut full = crate::snapshot::take_snapshot(client, verbose, None, None, None).await?;
    // The final tree wins on any uid it also holds; the union keeps the ones it alone saw.
    for (uid, element) in full.uid_map {
        uid_map.insert(uid, element);
    }
    full.uid_map = uid_map;
    Ok(Views::from_parts(full, Some(text)))
}

/// Append resolved `href` URLs to the link nodes of a rendered snapshot.
pub async fn resolve_urls(
    client: &CdpClient,
    text: &str,
    uid_map: &HashMap<String, ElementRef>,
) -> String {
    let mut result = String::with_capacity(text.len());
    for line in text.lines() {
        result.push_str(line);
        // Matches `uid=n42 link "Some text"`.
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

/// A windowed snapshot rendering.
pub struct Paged {
    /// The window, with a truncation note appended when there is one.
    pub text: String,
    /// Characters in the FULL snapshot, not in the window.
    pub total_chars: usize,
    pub truncated: bool,
    /// Where to resume paging; `None` unless truncated.
    pub next_offset: Option<usize>,
}

/// Char-based paging over a rendered snapshot: `offset` skips N characters, `max_chars` caps
/// the window, and a truncated window gains a tail naming the next `--offset`. UTF-8 safe.
/// `offset == 0` with no `max_chars` returns the text unchanged.
#[must_use]
pub fn paginate(text: &str, offset: usize, max_chars: Option<usize>) -> Paged {
    let total_chars = text.chars().count();

    if offset == 0 && max_chars.is_none() {
        return Paged { text: text.to_string(), total_chars, truncated: false, next_offset: None };
    }

    // Byte index of the offset-th char, clamped to the end.
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
        // Naive byte slicing would panic mid-char.
        let text = "日本語テストデータ"; // 9 chars
        let p = paginate(text, 2, Some(3)); // chars 2..5 = "語テス"
        assert_eq!(p.text.chars().take(3).collect::<String>(), "語テス");
        assert!(p.truncated);
        assert_eq!(p.next_offset, Some(5));
    }

    #[test]
    fn paginate_exact_fit_not_truncated() {
        // Guards `window_chars > max` against an off-by-one.
        let text = "abcdefghij"; // 10 chars
        let p = paginate(text, 0, Some(10));
        assert_eq!(p.text, text);
        assert!(!p.truncated);
        assert_eq!(p.next_offset, None);

        // Offset into the middle, cap == remaining.
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
        // No CDP here: only the line-parsing that picks out link rows.
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
