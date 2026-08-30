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
    let snapshot =
        crate::snapshot::take_snapshot(client, verbose, max_depth, focus_uid, role_filter).await?;
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
    let max_scrolls = limit.saturating_mul(3);
    let mut stale_count = 0;
    // `limit * 3` bounds iterations, not time: each costs a settle window of up to 2s. The
    // caller's `--timeout` bounds the whole collection.
    let deadline = std::time::Instant::now()
        .checked_add(client.call_timeout())
        .ok_or("Inspect timeout is too large")?;
    let mut ran_out_of_time = false;

    for _ in 0..max_scrolls {
        if std::time::Instant::now() >= deadline {
            ran_out_of_time = true;
            break;
        }
        let snapshot =
            crate::snapshot::take_snapshot(client, verbose, None, focus_uid, role_filter).await?;
        let prev_len = collected.len();
        for line in snapshot.text.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() && seen.insert(trimmed.to_string()) {
                collected.push(trimmed.to_string());
            }
        }
        uid_map.extend(snapshot.uid_map);

        if collected.len() >= limit {
            break;
        }

        // Three scrolls with no new item means the end of the list.
        if collected.len() == prev_len {
            stale_count += 1;
            if stale_count >= 3 {
                break;
            }
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
///
/// Two CDP calls whatever the link count, and one when every href is already absolute. It used to
/// be a `DOM.resolveNode` + `Runtime.callFunctionOn` pair per link node — 240 serial round trips
/// on a 120-link page, unbounded and the highest count in this codebase. `DOM.getDocument` carries
/// every `backendNodeId` with its `href` attribute in one reading; a single `Runtime.evaluate`
/// then absolutises the relative ones against the base URL of the document each came from.
pub async fn resolve_urls(
    client: &CdpClient,
    text: &str,
    uid_map: &HashMap<String, ElementRef>,
) -> String {
    let link_backend = |line: &str| -> Option<i64> {
        let (uid, role) = crate::snapshot_render::uid_and_role(line)?;
        if role != "link" {
            return None;
        }
        uid_map.get(uid).and_then(ElementRef::backend_node_id)
    };

    let wanted: HashSet<i64> = text.lines().filter_map(link_backend).collect();
    let hrefs = if wanted.is_empty() {
        HashMap::new()
    } else {
        link_hrefs(client, &wanted).await.unwrap_or_default()
    };

    let mut result = String::with_capacity(text.len());
    for line in text.lines() {
        result.push_str(line);
        if let Some(backend_id) = link_backend(line)
            && let Some(href) = hrefs.get(&backend_id)
            && !href.is_empty()
        {
            result.push_str(" url=");
            result.push_str(&crate::snapshot_render::quote(href));
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
        return Paged {
            text: text.to_string(),
            total_chars,
            truncated: false,
            next_offset: None,
        };
    }

    // Byte index of the offset-th char, clamped to the end.
    let start_byte = text
        .char_indices()
        .nth(offset)
        .map_or(text.len(), |(i, _)| i);
    let window = &text[start_byte..];
    let window_chars = total_chars.saturating_sub(offset);

    let (shown, kept) = match max_chars {
        Some(max) if window_chars > max => {
            let end_byte = window
                .char_indices()
                .nth(max)
                .map_or(window.len(), |(i, _)| i);
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

/// `backendNodeId → absolute href` for the wanted nodes.
async fn link_hrefs(
    client: &CdpClient,
    wanted: &HashSet<i64>,
) -> Result<HashMap<i64, String>, crate::BoxError> {
    let doc: serde_json::Value = client
        .call("DOM.getDocument", json!({"depth": -1, "pierce": true}))
        .await?;
    let root = doc.get("root").ok_or("DOM.getDocument returned no root")?;
    let mut found: Vec<(i64, String, String)> = Vec::new();
    collect_links(root, "", None, wanted, &mut found);

    // Only relative hrefs need the page; a site writing absolute ones costs one call in total.
    let relative: Vec<usize> = (0..found.len())
        .filter(|&i| !is_absolute(&found[i].2))
        .collect();
    if !relative.is_empty() {
        let pairs: Vec<[&str; 2]> = relative
            .iter()
            .map(|&i| [found[i].1.as_str(), found[i].2.as_str()])
            .collect();
        if let Some(absolute) = absolutise(client, &pairs).await {
            for (&slot, url) in relative.iter().zip(absolute) {
                if !url.is_empty() {
                    found[slot].2 = url;
                }
            }
        }
    }

    Ok(found.into_iter().map(|(id, _, href)| (id, href)).collect())
}

/// Walk a `DOM.getDocument` tree collecting `(backendNodeId, base URL, raw href)` for the wanted
/// nodes. `anchor` carries the enclosing `<a>`'s href down, which is what the old
/// `this.closest('a')` did for a link node pointing at an element inside the anchor.
fn collect_links(
    node: &serde_json::Value,
    base: &str,
    anchor: Option<&str>,
    wanted: &HashSet<i64>,
    out: &mut Vec<(i64, String, String)>,
) {
    // A document node — the page's, or an iframe's — carries the base its hrefs resolve against.
    let base = node
        .get("baseURL")
        .or_else(|| node.get("documentURL"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or(base);
    let anchor = href_attribute(node).or(anchor);

    if let Some(href) = anchor
        && let Some(id) = node
            .get("backendNodeId")
            .and_then(serde_json::Value::as_i64)
        && wanted.contains(&id)
    {
        out.push((id, base.to_string(), href.to_string()));
    }

    for key in ["children", "shadowRoots", "pseudoElements"] {
        if let Some(list) = node.get(key).and_then(serde_json::Value::as_array) {
            for child in list {
                collect_links(child, base, anchor, wanted, out);
            }
        }
    }
    if let Some(content) = node.get("contentDocument") {
        collect_links(content, base, None, wanted, out);
    }
}

/// The `href` of an `<a>`/`<area>`. `DOM.getDocument` hands attributes back flat.
fn href_attribute(node: &serde_json::Value) -> Option<&str> {
    let name = node.get("nodeName").and_then(serde_json::Value::as_str)?;
    if !name.eq_ignore_ascii_case("a") && !name.eq_ignore_ascii_case("area") {
        return None;
    }
    node.get("attributes")?
        .as_array()?
        .chunks(2)
        .find_map(|pair| {
            let key = pair.first()?.as_str()?;
            key.eq_ignore_ascii_case("href")
                .then(|| pair.get(1)?.as_str())
                .flatten()
        })
}

/// Whether an href already carries a scheme (RFC 3986 `scheme:`), so no base is needed.
fn is_absolute(href: &str) -> bool {
    let mut chars = href.chars();
    if !chars.next().is_some_and(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    for c in chars {
        if c == ':' {
            return true;
        }
        if !c.is_ascii_alphanumeric() && !matches!(c, '+' | '-' | '.') {
            return false;
        }
    }
    false
}

/// Resolve `[base, href]` pairs against each other in one call. `None` on any failure, which
/// leaves the raw attribute in place rather than dropping the link.
async fn absolutise(client: &CdpClient, pairs: &[[&str; 2]]) -> Option<Vec<String>> {
    // U+2028/U+2029 are legal inside a JSON string and were illegal inside a JS one before ES2019.
    let payload = serde_json::to_string(pairs)
        .ok()?
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029");
    let expression = format!(
        "JSON.stringify({payload}.map(p => {{ try {{ return new URL(p[1], p[0]).href }} catch (e) {{ return \"\"; }} }}))"
    );
    let result: serde_json::Value = client
        .call(
            "Runtime.evaluate",
            json!({"expression": expression, "returnByValue": true}),
        )
        .await
        .ok()?;
    let text = result.get("result")?.get("value")?.as_str()?;
    serde_json::from_str(text).ok()
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

    use super::{collect_links, href_attribute, is_absolute};
    use serde_json::json;
    use std::collections::HashSet;

    #[test]
    fn a_scheme_is_what_makes_an_href_absolute() {
        for absolute in [
            "https://e.com/a",
            "mailto:a@b.c",
            "javascript:void(0)",
            "data:,x",
        ] {
            assert!(is_absolute(absolute), "{absolute}");
        }
        for relative in ["/a", "a/b", "//host/p", "#frag", "?q=1", "", "1x:y"] {
            assert!(!is_absolute(relative), "{relative}");
        }
    }

    #[test]
    fn only_anchors_carry_an_href() {
        let anchor = json!({"nodeName": "A", "attributes": ["class", "c", "href", "/x"]});
        assert_eq!(href_attribute(&anchor), Some("/x"));
        let link_tag = json!({"nodeName": "LINK", "attributes": ["href", "/style.css"]});
        assert_eq!(href_attribute(&link_tag), None);
        let bare = json!({"nodeName": "A", "attributes": ["class", "c"]});
        assert_eq!(href_attribute(&bare), None);
    }

    /// One walk replaces the `DOM.resolveNode` + `Runtime.callFunctionOn` pair per link: the base
    /// comes from the enclosing document, and a node INSIDE an anchor inherits its href — what
    /// `this.closest('a')` used to do.
    #[test]
    fn one_walk_finds_every_wanted_href_with_its_base() {
        let doc = json!({
            "nodeName": "#document",
            "baseURL": "https://e.com/dir/page",
            "children": [{
                "nodeName": "A",
                "backendNodeId": 10,
                "attributes": ["href", "../other"],
                "children": [
                    {"nodeName": "SPAN", "backendNodeId": 11, "attributes": []},
                    {"nodeName": "#text", "backendNodeId": 12}
                ]
            }, {
                "nodeName": "IFRAME",
                "backendNodeId": 20,
                "contentDocument": {
                    "nodeName": "#document",
                    "baseURL": "https://inner.example/sub/",
                    "children": [{"nodeName": "A", "backendNodeId": 21, "attributes": ["href", "deep"]}]
                }
            }]
        });
        let wanted: HashSet<i64> = [10, 11, 21, 99].into_iter().collect();
        let mut found = Vec::new();
        collect_links(&doc, "", None, &wanted, &mut found);

        assert_eq!(found.len(), 3, "{found:?}");
        assert!(found.contains(&(10, "https://e.com/dir/page".into(), "../other".into())));
        assert!(
            found.contains(&(11, "https://e.com/dir/page".into(), "../other".into())),
            "a node inside the anchor inherits it: {found:?}"
        );
        assert!(
            found.contains(&(21, "https://inner.example/sub/".into(), "deep".into())),
            "an iframe's links resolve against the iframe's base: {found:?}"
        );
    }
}
