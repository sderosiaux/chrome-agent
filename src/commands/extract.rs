use serde::Serialize;
use serde_json::{json, Value};

use crate::cdp::client::CdpClient;

/// The shipped repeating-record extraction algorithm (MDR/DEPTA-inspired).
/// Single source of truth: the same file the jsdom test-suite exercises
/// (`tests/js/*.test.js`). Exposes `extract(scope, limit)` returning a JSON
/// string. Embedding it here (instead of an inline `format!` duplicate) means
/// the 100+ jsdom tests actually cover the code that ships in the binary.
const EXTRACT_JS: &str = include_str!("../../vendor/extract.js");

/// In-page routine for `extract --scroll`. Scrolls to the bottom repeatedly,
/// using a `MutationObserver` debounce to detect settling, bounded by
/// `MAX_SCROLLS`. The whole loop is raced against a hard `DEADLINE_MS`
/// (`Promise.race`) so a continuously-mutating page (feeds, ad rotators,
/// clocks) can never leave the `MutationObserver` debounce permanently
/// re-armed — the awaited promise always resolves (with partial results)
/// within the deadline, so `CdpClient::call` (which has no timeout) can't hang.
const SCROLL_JS: &str = r"(async () => {
        const MAX_SCROLLS = 10;
        const SETTLE_MS = 1000;
        const DEADLINE_MS = 8000;
        // Some sites (YouTube) scroll on documentElement, not body
        const getHeight = () => Math.max(document.body.scrollHeight, document.documentElement.scrollHeight);
        const root = document.body.scrollHeight > 0 ? document.body : document.documentElement;
        const scrollLoop = (async () => {
            let prevHeight = 0;
            for (let i = 0; i < MAX_SCROLLS; i++) {
                const height = getHeight();
                if (height === prevHeight && i > 0) break;
                prevHeight = height;
                window.scrollTo(0, height);
                // Wait for DOM to settle using MutationObserver
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
        // Hard deadline: even if the page never stops mutating, resolve with
        // whatever loaded so far rather than hanging the CDP call forever.
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

/// Scroll to bottom repeatedly until no new content loads.
/// Uses `MutationObserver` to detect DOM changes instead of blind sleep.
/// Max 10 scroll iterations to avoid infinite scroll traps, bounded overall by
/// an in-page `Promise.race` deadline (see [`SCROLL_JS`]).
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


/// Extract structured data using the accessibility tree instead of DOM.
/// Delegates to inspect with a role filter. Works on React SPAs (X.com)
/// where DOM structure is opaque but a11y roles are clean.
pub async fn run_a11y(
    client: &CdpClient,
    limit: usize,
    scroll: bool,
) -> Result<ExtractResult, crate::BoxError> {
    let roles = ["article", "listitem", "row", "treeitem"];

    for role in &roles {
        let filter = vec![*role];
        let snapshot = if scroll {
            super::inspect::scroll_collect(client, false, None, Some(&filter), limit).await?
        } else {
            super::inspect::run(client, false, None, None, Some(&filter)).await?
        };

        let lines: Vec<&str> = snapshot.text.lines()
            .filter(|l| l.trim().starts_with("uid="))
            .collect();

        if lines.is_empty() { continue; }
        if lines.len() < 3 && !scroll { continue; }

        let items: Vec<Value> = lines.iter()
            .take(limit)
            .map(|line| {
                // Strip "uid=nXXX role " prefix to get the content text
                let text = line.trim();
                let text = if let Some(rest) = text.strip_prefix("uid=") {
                    // Format: "uid=n123 article \"actual text here\""
                    // Skip the "nXXX role " part
                    if let Some((_uid_role, content)) = rest.split_once('"') {
                        content.trim_end_matches('"')
                    } else {
                        rest.splitn(3, ' ').last().unwrap_or(rest)
                    }
                } else {
                    text
                };
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

    Err("No repeating a11y pattern found. Try: extract (DOM mode) or inspect --filter \"article\"".into())
}

/// Build the in-page extraction expression: bind `_scope`/`_limit`, embed the
/// vendored [`EXTRACT_JS`] algorithm, then invoke its `extract(_scope, _limit)`
/// entrypoint. Wrapped in an arrow IIFE so the selector-not-found `return`
/// short-circuits cleanly. The vendored source's trailing
/// `if (typeof module !== 'undefined') module.exports = extract;` is a no-op in
/// the browser (`module` is undefined there).
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

    // If there's a hint (no pattern found), propagate as error.
    if let Some(hint) = parsed.get("hint").and_then(Value::as_str)
        && items.is_empty() {
            return Err(hint.into());
        }

    Ok(ExtractResult {
        items,
        count,
        pattern,
    })
}

/// Format the extract result as human-readable text.
pub fn format_text(result: &ExtractResult) -> String {
    let mut out = format!(
        "Found {} items (pattern: {})\n",
        result.count, result.pattern
    );
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

/// Build the JSON output for the extract command.
pub fn to_json(result: &ExtractResult) -> Value {
    json!({
        "ok": true,
        "items": result.items,
        "count": result.count,
        "pattern": result.pattern,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- FIX A9: --scroll must not hang on continuously-mutating pages ---

    #[test]
    fn scroll_js_has_hard_deadline_race() {
        // Before the fix the awaited promise was resolved only by the 300ms
        // MutationObserver debounce; a page that never stops mutating kept the
        // debounce re-armed forever and `CdpClient::call` (no timeout) hung.
        // The routine must now race the scroll loop against a fixed deadline so
        // it always resolves with partial results.
        assert!(
            SCROLL_JS.contains("Promise.race"),
            "scroll routine must bound itself with Promise.race"
        );
        assert!(
            SCROLL_JS.contains("DEADLINE_MS"),
            "scroll routine must define a hard deadline"
        );
        // The deadline promise must actually resolve (setTimeout(resolve, ...)),
        // otherwise racing it would be pointless.
        assert!(
            SCROLL_JS.contains("const deadline = new Promise(resolve => setTimeout(resolve, DEADLINE_MS))"),
            "deadline must be a self-resolving timeout"
        );
        // MAX_SCROLLS must be preserved per the fix requirements.
        assert!(SCROLL_JS.contains("MAX_SCROLLS = 10"));
    }

    // --- dedup fix: shipped algorithm must be the vendored, jsdom-tested one ---

    #[test]
    fn extract_js_is_the_vendored_source() {
        // Guard against the inline `format!` duplicate creeping back in: the
        // embedded algorithm must be the exact file the jsdom suite exercises,
        // and it must expose the `extract(scope, limit)` entrypoint the Rust
        // side calls.
        assert_eq!(EXTRACT_JS, include_str!("../../vendor/extract.js"));
        assert!(
            EXTRACT_JS.contains("function extract(_scope, _limit)"),
            "vendored source must expose extract(_scope, _limit)"
        );
    }

    #[test]
    fn build_extract_js_embeds_vendor_and_calls_entrypoint() {
        let js = build_extract_js(None, 20);
        // Whole-document scope, limit injected, vendored algorithm embedded,
        // and the entrypoint invoked with the bound args.
        assert!(js.contains("const _scope = document;"));
        assert!(js.contains("const _limit = 20;"));
        assert!(js.contains("function extract(_scope, _limit)"));
        assert!(js.contains("return extract(_scope, _limit);"));
        // Wrapped in an arrow IIFE so the selector short-circuit `return` is legal.
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
        // A selector containing quotes/backslashes must be JSON-escaped, not
        // concatenated raw, so it can't break out of the string literal.
        let js = build_extract_js(Some("a[href=\"x\"]"), 1);
        assert!(js.contains(r#"document.querySelector("a[href=\"x\"]")"#));
        // No unescaped raw selector delimiter leaked verbatim.
        assert!(!js.contains("querySelector(a[href=\"x\"])"));
    }
}
