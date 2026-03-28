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
                if (!article) return JSON.stringify({{__error: "Readability returned null — page may not have article structure. Try: aibrowsr text --selector main"}});
                return JSON.stringify({{
                    title: article.title || '',
                    textContent: article.textContent || '',
                    content: article.content || '',
                    excerpt: article.excerpt || '',
                    byline: article.byline || '',
                }});
            }} catch(e) {{
                return JSON.stringify({{__error: "Readability failed: " + e.message + ". Try: aibrowsr text --selector main"}});
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
        .ok_or("Readability returned null — page may not have an article structure. Try: aibrowsr text --selector main")?;

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

    if let Some(max) = truncate
        && parsed.text_content.chars().count() > max {
            parsed.text_content = crate::truncate::truncate_str(&parsed.text_content, max, "...").into_owned();
        }

    Ok(parsed)
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
}
