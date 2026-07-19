use serde::Deserialize;
use serde_json::json;

use crate::cdp::client::CdpClient;
use crate::cdp::types::EvaluateResult;

const READABILITY_JS: &str = include_str!("../../vendor/Readability.js");

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadResult {
    pub title: String,
    pub text_content: String,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub excerpt: Option<String>,
    #[serde(default)]
    pub byline: Option<String>,
}

pub async fn run(
    client: &CdpClient,
    html: bool,
    truncate: Option<usize>,
) -> Result<ReadResult, crate::BoxError> {
    // Inject Readability.js into the page and parse
    #[allow(clippy::needless_raw_string_hashes)]
    let js = format!(
        r#"(() => {{
            try {{
                {READABILITY_JS}
                const doc = document.cloneNode(true);
                const reader = new Readability(doc);
                const article = reader.parse();
                if (!article) return JSON.stringify({{__error: "Readability returned null — page may not have article structure. Try: chrome-agent text --selector main"}});
                return JSON.stringify({{
                    title: article.title || '',
                    textContent: article.textContent || '',
                    content: article.content || '',
                    excerpt: article.excerpt || '',
                    byline: article.byline || '',
                }});
            }} catch(e) {{
                return JSON.stringify({{__error: "Readability failed: " + e.message + ". Try: chrome-agent text --selector main"}});
            }}
        }})()"#
    );

    let result: EvaluateResult = client
        .call(
            "Runtime.evaluate",
            json!({
                "expression": js,
                "returnByValue": true,
                "awaitPromise": false,
            }),
        )
        .await?;

    if let Some(exception) = &result.exception_details {
        return Err(format!(
            "Readability failed: {}",
            exception.text
        )
        .into());
    }

    let raw = result
        .result
        .value
        .as_ref()
        .and_then(|v| v.as_str())
        .ok_or("Readability returned null — page may not have an article structure. Try: chrome-agent text --selector main")?;

    // Check for in-JS error return
    let raw_value: serde_json::Value = serde_json::from_str(raw)?;
    if let Some(err) = raw_value.get("__error").and_then(|v| v.as_str()) {
        return Err(err.into());
    }

    let mut parsed: ReadResult = serde_json::from_value(raw_value)?;

    // Clean up: collapse whitespace runs in textContent
    parsed.text_content = collapse_whitespace(&parsed.text_content);

    if !html {
        // Clear HTML content to save memory/tokens when not requested
        parsed.content = None;
    }

    parsed.text_content = finalize_text(parsed.text_content, truncate)?;

    Ok(parsed)
}

const MIN_READABLE_CHARS: usize = 200;

/// Apply the min-content guard, then truncate.
///
/// Ordering matters: the guard must run BEFORE truncation, otherwise truncating a
/// valid article down to `< MIN_READABLE_CHARS` would trip the guard and error out.
/// Both the guard and the truncation are char-based (not byte-based) so multi-byte
/// UTF-8 content is measured consistently and never split mid-codepoint.
fn finalize_text(text: String, truncate: Option<usize>) -> Result<String, crate::BoxError> {
    if text.chars().count() < MIN_READABLE_CHARS {
        return Err("Page has minimal readable content — likely not an article. Try: chrome-agent text --selector main".into());
    }

    if let Some(max) = truncate
        && text.chars().count() > max
    {
        return Ok(crate::truncate::truncate_str(&text, max, "...").into_owned());
    }

    Ok(text)
}

fn collapse_whitespace(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut blank_count = 0;

    for line in s.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            blank_count += 1;
            if blank_count <= 1 {
                result.push('\n');
            }
        } else {
            blank_count = 0;
            if !result.is_empty() && !result.ends_with('\n') {
                result.push('\n');
            }
            result.push_str(trimmed);
            result.push('\n');
        }
    }

    result.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapse_cleans_whitespace() {
        let input = "  Title  \n\n\n\n  Body text  \n  More text  \n\n\n  End  ";
        let result = collapse_whitespace(input);
        assert_eq!(result, "Title\n\nBody text\nMore text\n\nEnd");
    }

    #[test]
    fn guard_runs_before_truncation_valid_article_survives() {
        // A valid article (>= 200 chars) truncated below 200 must NOT trip the
        // min-content guard. Regression for A8: guard used to run after truncation.
        let article = "x".repeat(500);
        let out = finalize_text(article, Some(50)).expect("valid article must not error");
        // Truncated to 50 chars + "..." suffix.
        assert_eq!(out.chars().count(), 53);
        assert!(out.ends_with("..."));
    }

    #[test]
    fn guard_rejects_short_content_regardless_of_truncate() {
        let short = "x".repeat(MIN_READABLE_CHARS - 1);
        assert!(finalize_text(short.clone(), None).is_err());
        assert!(finalize_text(short, Some(10)).is_err());
    }

    #[test]
    fn guard_is_char_based_not_byte_based() {
        // 150 multi-byte chars = 450 bytes. Byte-based guard (< 200) would wrongly
        // accept this short article; char-based (< 200) correctly rejects it.
        let multibyte = "日".repeat(150);
        assert!(multibyte.len() > MIN_READABLE_CHARS); // 450 bytes
        assert!(multibyte.chars().count() < MIN_READABLE_CHARS); // 150 chars
        assert!(finalize_text(multibyte, None).is_err());
    }

    #[test]
    fn no_truncate_returns_full_valid_article() {
        let article = "y".repeat(300);
        let out = finalize_text(article.clone(), None).expect("valid article");
        assert_eq!(out, article);
    }

    #[test]
    fn truncate_larger_than_content_is_noop() {
        let article = "z".repeat(300);
        let out = finalize_text(article.clone(), Some(10_000)).expect("valid article");
        assert_eq!(out, article);
    }
}
