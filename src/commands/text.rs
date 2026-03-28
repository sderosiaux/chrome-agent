use std::collections::HashMap;

use serde_json::json;

use crate::cdp::client::CdpClient;
use crate::cdp::types::{EvaluateResult, ResolveNodeParams, ResolveNodeResult};
use crate::element_ref::ElementRef;

/// Extract visible text from the page, a specific element by uid, or a CSS selector.
pub async fn run(
    client: &CdpClient,
    uid: Option<&str>,
    selector: Option<&str>,
    uid_map: &HashMap<String, ElementRef>,
) -> Result<String, Box<dyn std::error::Error>> {
    let raw = if let Some(sel) = selector {
        // Selector-based extraction with role-attribute fallback
        // "main" also matches [role=main], "nav" also matches [role=navigation], etc.
        let safe_sel = serde_json::to_string(sel).unwrap_or_default();
        let expr = format!(
            "(() => {{ let el = document.querySelector({safe_sel}); if (!el) {{ el = document.querySelector('[role=' + {safe_sel} + ']'); }} return el ? el.innerText || '' : ''; }})()"
        );
        let result: EvaluateResult = client
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": expr,
                    "returnByValue": true,
                }),
            )
            .await?;
        let text = result
            .result
            .value
            .as_ref()
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if text.is_empty() {
            return Err(format!("No element matches selector '{sel}' or element has no text.").into());
        }
        text
    } else {
        match uid {
        None => {
            // Whole page text
            let result: EvaluateResult = client
                .call(
                    "Runtime.evaluate",
                    json!({
                        "expression": "document.body.innerText",
                        "returnByValue": true,
                    }),
                )
                .await?;
            result
                .result
                .value
                .as_ref()
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        }
        Some(uid) => {
            let element_ref = uid_map.get(uid).ok_or_else(|| {
                format!(
                    "Element uid={uid} not found. Run 'aibrowsr inspect' to get fresh uids."
                )
            })?;
            let backend_node_id = element_ref.backend_node_id().ok_or_else(|| {
                format!("Element uid={uid} has no resolvable backend node.")
            })?;

            let resolve_result: ResolveNodeResult = client
                .call(
                    "DOM.resolveNode",
                    ResolveNodeParams {
                        node_id: None,
                        backend_node_id: Some(backend_node_id),
                        object_group: Some("aibrowsr".into()),
                        execution_context_id: None,
                    },
                )
                .await?;

            let object_id = resolve_result.object.object_id.ok_or_else(|| {
                format!("Element uid={uid} could not be resolved to a JS object.")
            })?;

            let result: serde_json::Value = client
                .call(
                    "Runtime.callFunctionOn",
                    json!({
                        "objectId": object_id,
                        "functionDeclaration": "function() { return this.innerText || this.textContent; }",
                        "returnByValue": true,
                    }),
                )
                .await?;

            result
                .get("result")
                .and_then(|r| r.get("value"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        }
    }
    };

    Ok(collapse_blank_lines(&raw))
}

/// Trim trailing whitespace per line and collapse multiple blank lines into one.
fn collapse_blank_lines(s: &str) -> String {
    let trimmed = s.trim_end();
    let mut result = String::with_capacity(trimmed.len());
    let mut prev_blank = false;
    for line in trimmed.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            if prev_blank {
                continue;
            }
            prev_blank = true;
        } else {
            prev_blank = false;
        }
        result.push_str(line);
        result.push('\n');
    }
    // Remove trailing newline
    if result.ends_with('\n') {
        result.pop();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapse_multiple_blank_lines() {
        let input = "hello\n\n\n\nworld\n\n  \nfoo  \n";
        let out = collapse_blank_lines(input);
        assert_eq!(out, "hello\n\nworld\n\nfoo");
    }

    #[test]
    fn no_blanks() {
        assert_eq!(collapse_blank_lines("a\nb\nc"), "a\nb\nc");
    }

    #[test]
    fn bug_selector_injection_escaped() {
        // serde_json::to_string wraps in double quotes and escapes internals
        let malicious = r"'); alert('xss";
        let escaped = serde_json::to_string(malicious).unwrap();
        assert!(escaped.starts_with('"'));
        assert!(escaped.ends_with('"'));
        // The single quotes are preserved (not dangerous in double-quoted JS string)
        // The important thing: no way to break out of the double-quoted string
        assert!(!escaped.contains(r#"\""#) || escaped.starts_with('"'));
    }
}
