use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};

use crate::cdp::client::CdpClient;
use crate::cdp::types::EvaluateResult;

/// A captured network entry.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkEntry {
    pub url: String,
    pub method: String,
    pub status: u16,
    pub content_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    pub size: u64,
    pub duration_ms: u64,
}

// Content types eligible for body capture.
const CAPTURABLE_TYPES: &[&str] = &["json", "text", "javascript", "xml"];

fn is_capturable_type(ct: &str) -> bool {
    let lower = ct.to_ascii_lowercase();
    CAPTURABLE_TYPES.iter().any(|t| lower.contains(t))
}

/// Retroactive mode: uses the Performance/Resource Timing API to list resources
/// already loaded on the current page. Works without `Network.enable` (stealth-safe).
pub async fn run_retroactive(
    client: &CdpClient,
    filter: Option<&str>,
    limit: usize,
) -> Result<Vec<NetworkEntry>, crate::BoxError> {
    let js = r"
        JSON.stringify(
            performance.getEntriesByType('resource').map(e => ({
                url: e.name,
                type: e.initiatorType,
                duration: Math.round(e.duration),
                size: e.transferSize || 0,
            }))
        )
    ";

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

    if let Some(exc) = &result.exception_details {
        return Err(format!(
            "Performance API error: {}",
            exc.exception
                .as_ref()
                .and_then(|e| e.description.as_deref())
                .unwrap_or(&exc.text)
        )
        .into());
    }

    let raw = result
        .result
        .value
        .as_ref()
        .and_then(|v| v.as_str())
        .unwrap_or("[]");

    let entries: Vec<Value> = serde_json::from_str(raw)?;

    let filter_lower = filter.map(str::to_ascii_lowercase);

    let results: Vec<NetworkEntry> = entries
        .into_iter()
        .filter_map(|e| {
            let url = e.get("url")?.as_str()?.to_string();
            if let Some(ref f) = filter_lower
                && !url.to_ascii_lowercase().contains(f.as_str()) {
                    return None;
                }
            let initiator = e.get("type").and_then(Value::as_str).unwrap_or("other");
            let duration = e.get("duration").and_then(Value::as_u64).unwrap_or(0);
            let size = e.get("size").and_then(Value::as_u64).unwrap_or(0);

            // Map initiator type to a readable content type hint
            let content_type = match initiator {
                "xmlhttprequest" | "fetch" => "xhr/fetch".to_string(),
                "script" => "script".to_string(),
                "css" | "link" => "stylesheet".to_string(),
                "img" => "image".to_string(),
                "font" => "font".to_string(),
                other => other.to_string(),
            };

            Some(NetworkEntry {
                url,
                method: "GET".to_string(), // Resource Timing API doesn't expose method
                status: 0,                 // Resource Timing API doesn't expose status
                content_type,
                body: None,
                size,
                duration_ms: duration,
            })
        })
        .take(limit)
        .collect();

    Ok(results)
}

/// Live capture mode: enables `Network` domain, subscribes to `responseReceived`
/// events, and collects responses for the specified duration.
pub async fn run_live(
    client: &CdpClient,
    filter: Option<&str>,
    capture_body: bool,
    limit: usize,
    timeout_secs: u64,
) -> Result<Vec<NetworkEntry>, crate::BoxError> {
    // Enable Network domain (required for live capture)
    client.enable("Network").await?;

    let mut rx = client.events();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    let filter_lower = filter.map(str::to_ascii_lowercase);

    let mut entries: Vec<NetworkEntry> = Vec::new();

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() || entries.len() >= limit {
            break;
        }

        let event = tokio::time::timeout(remaining, async {
            loop {
                match rx.recv().await {
                    Ok(ev) => return Ok(ev),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        return Err("Event channel closed".to_string());
                    }
                }
            }
        })
        .await;

        let event = match event {
            Ok(Ok(ev)) => ev,
            Ok(Err(e)) => return Err(e.into()),
            Err(_) => break, // timeout
        };

        if event.method != "Network.responseReceived" {
            continue;
        }

        let Some(response) = event.params.get("response") else { continue };

        let url = response
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        // Apply filter
        if let Some(ref f) = filter_lower
            && !url.to_ascii_lowercase().contains(f.as_str()) {
                continue;
            }

        let status = response
            .get("status")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u16;
        let content_type = response
            .get("mimeType")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let method = event
            .params
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("Other")
            .to_string();
        let request_id = event
            .params
            .get("requestId")
            .and_then(Value::as_str)
            .unwrap_or("");
        let encoded_length = response
            .get("encodedDataLength")
            .and_then(Value::as_u64)
            .unwrap_or(0);

        // Optionally fetch body for text-like content types
        let body = if capture_body && is_capturable_type(&content_type) && !request_id.is_empty() {
            fetch_response_body(client, request_id).await
        } else {
            None
        };

        entries.push(NetworkEntry {
            url,
            method,
            status,
            content_type,
            body,
            size: encoded_length,
            duration_ms: 0, // timing not directly available from responseReceived
        });

        if entries.len() >= limit {
            break;
        }
    }

    Ok(entries)
}

/// Format network entries as a human-readable table.
pub fn format_text(entries: &[NetworkEntry]) -> String {
    if entries.is_empty() {
        return "No network entries captured.".to_string();
    }
    let mut out = format!(
        "{:<70} {:>6} {:<14} {:>8} {:>6}\n{}\n",
        "URL", "STATUS", "TYPE", "SIZE", "MS",
        "-".repeat(110)
    );
    for e in entries {
        let url_display = crate::truncate::truncate_str(&e.url, 67, "...");
        let status_str = if e.status == 0 { "-".to_string() } else { e.status.to_string() };
        let size_str = if e.size == 0 {
            "-".to_string()
        } else if e.size >= 1024 {
            format!("{}K", e.size / 1024)
        } else {
            format!("{}B", e.size)
        };
        out += &format!(
            "{:<70} {:>6} {:<14} {:>8} {:>6}\n",
            url_display, status_str, e.content_type, size_str, e.duration_ms
        );
        if let Some(ref b) = e.body {
            let preview = crate::truncate::truncate_str(b, 200, "...");
            out += &format!("  body: {preview}\n");
        }
    }
    out += &format!("\n{} entries", entries.len());
    out
}

/// Fetch a response body via `Network.getResponseBody`. Returns `None` on failure
/// (body may not be available if request was evicted from memory).
async fn fetch_response_body(client: &CdpClient, request_id: &str) -> Option<String> {
    let result: Value = client
        .call(
            "Network.getResponseBody",
            json!({ "requestId": request_id }),
        )
        .await
        .ok()?;

    let body = result.get("body")?.as_str()?;
    Some(crate::truncate::truncate_str(body, 2000, "...(truncated)").into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bug_url_truncation_utf8_safe() {
        // Bug: &e.url[..67] panics on URLs with multi-byte chars
        let entry = NetworkEntry {
            url: "https://example.com/café/résumé/über/naïve/длинный".to_string(),
            method: "GET".to_string(),
            status: 200,
            content_type: "text/html".to_string(),
            size: 1000,
            duration_ms: 50,
            body: None,
        };
        // This should not panic
        let text = format_text(&[entry]);
        assert!(!text.is_empty());
    }

    #[test]
    fn bug_body_truncation_utf8_safe() {
        // Bug: &body[..2000] panics on bodies with multi-byte chars
        let entry = NetworkEntry {
            url: "https://example.com".to_string(),
            method: "GET".to_string(),
            status: 200,
            content_type: "application/json".to_string(),
            size: 5000,
            duration_ms: 50,
            body: Some("é".repeat(3000)),  // each é is 2 bytes
        };
        let text = format_text(&[entry]);
        assert!(!text.is_empty());
    }

    #[test]
    fn bug_body_preview_utf8_safe() {
        // Bug: &b[..200] panics on bodies with multi-byte chars
        let entry = NetworkEntry {
            url: "https://example.com".to_string(),
            method: "GET".to_string(),
            status: 200,
            content_type: "application/json".to_string(),
            size: 500,
            duration_ms: 50,
            body: Some("日本語テスト".repeat(100)),  // multi-byte Japanese
        };
        let text = format_text(&[entry]);
        assert!(!text.is_empty());
    }
}
