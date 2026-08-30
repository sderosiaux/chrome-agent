use serde::Serialize;
use serde_json::{Value, json};

use crate::cdp::client::CdpClient;

/// The shipped repeating-record extraction algorithm (MDR/DEPTA-inspired), exposing
/// `extract(scope, limit)`. Embedded rather than duplicated inline, so the jsdom suite in
/// `tests/js/*.test.js` covers the code that actually ships.
const EXTRACT_JS: &str = include_str!("../../vendor/extract.js");

/// In-page routine for `extract --scroll`: scroll to the bottom up to `MAX_SCROLLS` times,
/// with a `MutationObserver` debounce to detect settling. The loop is raced against a hard
/// 8s deadline, because a continuously-mutating page keeps the debounce re-armed forever and
/// `CdpClient::call` has no timeout of its own.
const SCROLL_JS: &str = r"(async () => {
        const MAX_SCROLLS = 10;
        const SETTLE_MS = 1000;
        const DEADLINE_MS = 8000;
        // Some sites (YouTube) scroll on documentElement, not body.
        const getHeight = () => Math.max(document.body.scrollHeight, document.documentElement.scrollHeight);
        const root = document.body.scrollHeight > 0 ? document.body : document.documentElement;
        const scrollLoop = (async () => {
            let prevHeight = 0;
            for (let i = 0; i < MAX_SCROLLS; i++) {
                const height = getHeight();
                if (height === prevHeight && i > 0) break;
                prevHeight = height;
                window.scrollTo(0, height);
                // Wait for the DOM to settle.
                await new Promise(resolve => {
                    let timer = setTimeout(resolve, SETTLE_MS);
                    const observer = new MutationObserver(() => {
                        clearTimeout(timer);
                        timer = setTimeout(() => {
                            observer.disconnect();
                            resolve();
                        }, 300);
                    });
                    observer.observe(root, { childList: true, subtree: true });
                });
            }
        })();
        // Hard deadline: resolve with partial results rather than hanging the CDP call.
        const deadline = new Promise(resolve => setTimeout(resolve, DEADLINE_MS));
        await Promise.race([scrollLoop, deadline]);
        window.scrollTo(0, 0);
        return getHeight();
    })()";

#[derive(Debug, Serialize)]
pub struct ExtractResult {
    pub items: Vec<Value>,
    pub count: usize,
    pub pattern: String,
}

/// Scroll to the bottom until no new content loads: max 10 iterations, bounded overall by
/// the in-page deadline in [`SCROLL_JS`].
pub async fn scroll_to_load(client: &CdpClient) -> Result<(), crate::BoxError> {
    let _: Value = client
        .call(
            "Runtime.evaluate",
            json!({
                "expression": SCROLL_JS,
                "returnByValue": true,
                "awaitPromise": true,
            }),
        )
        .await?;

    Ok(())
}

/// The records `extract` reports, whichever tree they came from. The one entry point for CLI,
/// pipe and batch, so the DOM/a11y choice and the scroll rule cannot drift into two versions.
///
/// `run_a11y` scrolls internally, so only the DOM path takes an explicit `scroll_to_load` —
/// otherwise `--a11y --scroll` scrolls twice.
pub async fn collect(
    client: &CdpClient,
    selector: Option<&str>,
    limit: usize,
    scroll: bool,
    a11y: bool,
) -> Result<ExtractResult, crate::BoxError> {
    if a11y {
        return run_a11y(client, limit, scroll).await;
    }
    if scroll {
        scroll_to_load(client).await?;
    }
    run(client, selector, limit).await
}

/// Extract records from the accessibility tree instead of the DOM. Works on SPAs whose DOM
/// structure is opaque but whose roles are clean.
///
/// ONE reading, filtered four times in memory. A read per candidate role saw four different page
/// states, so which pattern "won" depended on when the page settled — and with `--scroll` it also
/// ran four scroll loops. The read asks for all four roles at once so `--scroll`'s `limit` still
/// counts records rather than every node on the page; `apply_role_filter` then partitions it at
/// no CDP cost.
pub async fn run_a11y(
    client: &CdpClient,
    limit: usize,
    scroll: bool,
) -> Result<ExtractResult, crate::BoxError> {
    let roles = ["article", "listitem", "row", "treeitem"];

    // Read-only: no baseline is stored, so the filtered rendering is enough.
    let text = if scroll {
        super::inspect::scroll_collect(client, false, None, Some(&roles), limit)
            .await?
            .shown()
            .to_string()
    } else {
        super::inspect::run(client, false, None, None, Some(&roles))
            .await?
            .text
    };

    for role in &roles {
        let one_role = [*role];
        let filtered =
            crate::snapshot_render::apply_role_filter(text.clone(), Some(&one_role), None);
        let lines: Vec<&str> = filtered
            .lines()
            .filter(|l| l.trim().starts_with("uid="))
            .collect();

        if lines.is_empty() {
            continue;
        }
        if lines.len() < 3 && !scroll {
            continue;
        }

        let items: Vec<Value> = lines
            .iter()
            .take(limit)
            .map(|line| {
                // `uid=n123 article "the text"` → the text, decoded.
                let text = crate::snapshot_render::name_in(line)
                    .unwrap_or_else(|| line.trim().to_string());
                json!({"text": text})
            })
            .collect();

        let count = lines.len();
        return Ok(ExtractResult {
            items,
            count,
            pattern: format!("a11y:{role}"),
        });
    }

    Err(
        "No repeating a11y pattern found. Try: extract (DOM mode) or inspect --filter \"article\""
            .into(),
    )
}

/// Bind `_scope`/`_limit`, embed [`EXTRACT_JS`], and call `extract(_scope, _limit)`. Wrapped
/// in an arrow IIFE so the selector-not-found `return` short-circuits cleanly.
fn build_extract_js(selector: Option<&str>, limit: usize) -> String {
    let scope_js = if let Some(sel) = selector {
        let escaped = serde_json::to_string(sel).unwrap_or_default();
        format!(
            "const _scope = document.querySelector({escaped}); if (!_scope) return JSON.stringify({{ items: [], hint: 'Selector ' + {escaped} + ' not found' }});"
        )
    } else {
        "const _scope = document;".to_string()
    };

    format!(
        "(() => {{\n{scope_js}\nconst _limit = {limit};\n{EXTRACT_JS}\nreturn extract(_scope, _limit);\n}})()"
    )
}

pub async fn run(
    client: &CdpClient,
    selector: Option<&str>,
    limit: usize,
) -> Result<ExtractResult, crate::BoxError> {
    let js = build_extract_js(selector, limit);

    let raw = crate::commands::eval::run_raw(client, &js).await?;

    // The JS returns a JSON string; parse it.
    let parsed: Value = match raw {
        Value::String(s) => serde_json::from_str(&s)?,
        other => other,
    };

    let items = parsed
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let count = parsed
        .get("count")
        .and_then(Value::as_u64)
        .unwrap_or(items.len() as u64) as usize;
    let pattern = parsed
        .get("pattern")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    // A hint with no items means no pattern was found; propagate it as an error.
    if let Some(hint) = parsed.get("hint").and_then(Value::as_str)
        && items.is_empty()
    {
        return Err(hint.into());
    }

    Ok(ExtractResult {
        items,
        count,
        pattern,
    })
}

pub fn format_text(result: &ExtractResult) -> String {
    let mut out = if result.count > result.items.len() {
        format!(
            "Found {} items, showing {} (pattern: {}) — raise --limit for the rest\n",
            result.count,
            result.items.len(),
            result.pattern
        )
    } else {
        format!(
            "Found {} items (pattern: {})\n",
            result.count, result.pattern
        )
    };
    for (i, item) in result.items.iter().enumerate() {
        let mut parts: Vec<String> = Vec::new();
        if let Some(title) = item.get("title").and_then(Value::as_str) {
            parts.push(format!("Title: \"{title}\""));
        }
        if let Some(price) = item.get("price").and_then(Value::as_str) {
            parts.push(format!("Price: \"{price}\""));
        }
        if let Some(date) = item.get("date").and_then(Value::as_str) {
            parts.push(format!("Date: \"{date}\""));
        }
        if let Some(url) = item.get("url").and_then(Value::as_str) {
            parts.push(format!("URL: {url}"));
        }
        if let Some(image) = item.get("image").and_then(Value::as_str) {
            parts.push(format!("Image: {image}"));
        }
        if let Some(text) = item.get("text").and_then(Value::as_str) {
            parts.push(format!("Text: \"{text}\""));
        }
        if let Some(fields) = item.get("fields").and_then(Value::as_array) {
            let texts: Vec<&str> = fields.iter().filter_map(Value::as_str).collect();
            if !texts.is_empty() {
                parts.push(format!("Fields: [{}]", texts.join(", ")));
            }
        }
        out.push_str(&format!("{}. {}\n", i + 1, parts.join(" | ")));
    }
    out
}

/// JSON output. `count` is what the page matched, `returned` what `--limit` let through;
/// when they differ, `truncated` and a hint say so.
pub fn to_json(result: &ExtractResult) -> Value {
    let returned = result.items.len();
    let truncated = result.count > returned;
    let mut out = json!({
        "ok": true,
        "items": result.items,
        "count": result.count,
        "returned": returned,
        "truncated": truncated,
        "pattern": result.pattern,
    });
    if truncated {
        out["hint"] = json!(format!(
            "Showing {returned} of {} matched records. Raise --limit to see more.",
            result.count
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// When `count` and `items` differ, both outputs must say so.
    #[test]
    fn a_truncated_extract_says_how_many_it_is_holding_back() {
        let result = ExtractResult {
            items: (0..10)
                .map(|i| json!({"title": format!("item {i}")}))
                .collect(),
            count: 30,
            pattern: "div.card".into(),
        };

        let text = format_text(&result);
        assert!(
            text.contains("10") && text.contains("30"),
            "the text output must show both what was matched and what was returned: {text}"
        );
        assert!(
            text.contains("--limit"),
            "and name the flag that decides it: {text}"
        );

        let v = to_json(&result);
        assert_eq!(
            v["count"], 30,
            "count stays the number of records on the page"
        );
        assert_eq!(
            v["returned"], 10,
            "returned is what the caller actually holds"
        );
        assert_eq!(v["truncated"], true, "and the divergence is flagged: {v}");
        assert!(
            v["hint"].as_str().unwrap_or_default().contains("--limit"),
            "the hint names the flag to raise: {v}"
        );

        // An untruncated result carries no warning.
        let whole = ExtractResult {
            items: vec![json!({"title": "only one"})],
            count: 1,
            pattern: "div.card".into(),
        };
        let v = to_json(&whole);
        assert_eq!(v["truncated"], false, "{v}");
        assert!(v["hint"].is_null(), "{v}");
    }

    #[test]
    fn scroll_js_has_hard_deadline_race() {
        // Without the race, a page that never stops mutating keeps the debounce re-armed
        // and hangs the timeout-less `CdpClient::call`.
        assert!(
            SCROLL_JS.contains("Promise.race"),
            "scroll routine must bound itself with Promise.race"
        );
        assert!(
            SCROLL_JS.contains("DEADLINE_MS"),
            "scroll routine must define a hard deadline"
        );
        // The deadline promise must actually resolve, or racing it is pointless.
        assert!(
            SCROLL_JS.contains(
                "const deadline = new Promise(resolve => setTimeout(resolve, DEADLINE_MS))"
            ),
            "deadline must be a self-resolving timeout"
        );
        assert!(SCROLL_JS.contains("MAX_SCROLLS = 10"));
    }

    #[test]
    fn extract_js_is_the_vendored_source() {
        // The embedded algorithm must be the exact file the jsdom suite exercises.
        assert_eq!(EXTRACT_JS, include_str!("../../vendor/extract.js"));
        assert!(
            EXTRACT_JS.contains("function extract(_scope, _limit)"),
            "vendored source must expose extract(_scope, _limit)"
        );
    }

    #[test]
    fn build_extract_js_embeds_vendor_and_calls_entrypoint() {
        let js = build_extract_js(None, 20);
        assert!(js.contains("const _scope = document;"));
        assert!(js.contains("const _limit = 20;"));
        assert!(js.contains("function extract(_scope, _limit)"));
        assert!(js.contains("return extract(_scope, _limit);"));
        // Arrow IIFE, so the selector short-circuit `return` is legal.
        assert!(js.trim_start().starts_with("(() => {"));
        assert!(js.trim_end().ends_with("})()"));
    }

    #[test]
    fn build_extract_js_scopes_and_escapes_selector() {
        let js = build_extract_js(Some("div.card"), 5);
        assert!(js.contains("document.querySelector(\"div.card\")"));
        assert!(js.contains("const _limit = 5;"));
        assert!(js.contains("if (!_scope) return JSON.stringify"));
    }

    #[test]
    fn build_extract_js_selector_escaping_is_injection_safe() {
        // Quotes and backslashes are JSON-escaped, so a selector cannot break out of the
        // string literal.
        let js = build_extract_js(Some("a[href=\"x\"]"), 1);
        assert!(js.contains(r#"document.querySelector("a[href=\"x\"]")"#));
        assert!(!js.contains("querySelector(a[href=\"x\"])"));
    }
}
