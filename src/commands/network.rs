use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};

use crate::cdp::client::CdpClient;
use crate::cdp::types::EvaluateResult;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkEntry {
    pub url: String,
    /// Resource classification, not an HTTP verb — neither source exposes the method. Live
    /// mode stores the CDP `ResourceType`, retroactive mode the Resource Timing
    /// `initiatorType`.
    pub resource_type: String,
    pub status: u16,
    pub content_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Why `body` is absent although capture was requested: binary, unavailable from CDP, or no
    /// `loadingFinished` observed before capture stopped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_omitted: Option<String>,
    pub size: u64,
    pub duration_ms: u64,
}

// Content types eligible for body capture when the caller named nothing. `--filter` is
// itself a selection and overrides this list: the allowlist keeps an unfiltered `--body`
// from pulling every image and font into the response, not to veto an explicit ask.
const CAPTURABLE_TYPES: &[&str] = &["json", "text", "javascript", "xml"];
/// Once `--limit` is reached, observe completion events for bodies already selected, but never
/// wait out the caller's whole live window for an SSE or long poll.
const BODY_EVENT_GRACE: Duration = Duration::from_secs(1);
/// Body reads happen after event collection so a CDP round trip cannot make the receiver lag.
/// One deadline covers the whole set, rather than charging it once per entry.
const BODY_READ_GRACE: Duration = Duration::from_secs(1);

fn is_capturable_type(ct: &str) -> bool {
    let lower = ct.to_ascii_lowercase();
    CAPTURABLE_TYPES.iter().any(|t| lower.contains(t))
}

/// Requested, and either selected by a URL filter or admitted by the allowlist.
const fn should_capture_body(requested: bool, filtered: bool, capturable_type: bool) -> bool {
    requested && (filtered || capturable_type)
}

/// What `Network.getResponseBody` handed back. `base64Encoded` does NOT mean binary —
/// Chrome sets it for every MIME its own list does not call text (e.g. `application/yaml`).
/// The content decides: decoded bytes that are valid UTF-8 are printed, the rest counted.
enum FetchedBody {
    Text(String),
    Binary { bytes: usize },
}

fn classify_body(body: &str, base64_encoded: bool) -> FetchedBody {
    if base64_encoded {
        let Ok(bytes) = crate::base64::decode(body) else {
            // Undecodable: all that is known is the size.
            return FetchedBody::Binary { bytes: body.len() };
        };
        match String::from_utf8(bytes) {
            Ok(text) => FetchedBody::Text(
                crate::truncate::truncate_str(&text, 2000, "...(truncated)").into_owned(),
            ),
            Err(error) => FetchedBody::Binary {
                bytes: error.as_bytes().len(),
            },
        }
    } else {
        FetchedBody::Text(crate::truncate::truncate_str(body, 2000, "...(truncated)").into_owned())
    }
}

/// List resources already loaded, via the Resource Timing API. No `Network.enable`, so
/// stealth-safe; no status codes either.
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
                && !url.to_ascii_lowercase().contains(f.as_str())
            {
                return None;
            }
            let initiator = e.get("type").and_then(Value::as_str).unwrap_or("other");
            let duration = e.get("duration").and_then(Value::as_u64).unwrap_or(0);
            let size = e.get("size").and_then(Value::as_u64).unwrap_or(0);

            // Initiator type → readable content-type hint.
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
                resource_type: initiator.to_string(), // initiatorType, not the HTTP method
                status: 0,                            // not exposed by Resource Timing
                content_type,
                body: None,
                body_omitted: None,
                size,
                duration_ms: duration,
            })
        })
        .take(limit)
        .collect();

    Ok(results)
}

struct LiveCapture {
    entries: Vec<NetworkEntry>,
    lost_events: u64,
}

struct BodyRead {
    request_id: String,
    index: usize,
    loading_finished_observed: bool,
}

/// Live capture: enable `Network`, collect `responseReceived` for `timeout_secs`.
async fn run_live(
    client: &CdpClient,
    filter: Option<&str>,
    capture_body: bool,
    limit: usize,
    timeout_secs: u64,
) -> Result<LiveCapture, crate::BoxError> {
    let mut rx = client.network_events();
    client.enable("Network").await?;
    let deadline = tokio::time::Instant::now()
        .checked_add(Duration::from_secs(timeout_secs))
        .ok_or("Network live timeout is too large")?;
    let filter_lower = filter.map(str::to_ascii_lowercase);

    let mut entries: Vec<NetworkEntry> = Vec::new();
    // request id -> index into `entries`, for bodies still in flight.
    let mut pending_bodies: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut body_reads = Vec::new();
    let mut limit_deadline = None;
    let mut lost_events = 0_u64;

    loop {
        if entries.len() >= limit && pending_bodies.is_empty() {
            break;
        }
        let receive_deadline = limit_deadline.map_or(deadline, |grace| deadline.min(grace));
        let remaining = receive_deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }

        let event = tokio::time::timeout(remaining, rx.recv()).await;

        let event = match event {
            Ok(Ok(event)) => event,
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(missed))) => {
                lost_events = lost_events.saturating_add(missed);
                break;
            }
            Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                return Err("Event channel closed".into());
            }
            Err(_) => break, // timeout
        };

        // The entry is pushed at `responseReceived` (where the metadata is), then queued for a
        // body read once `loadingFinished` is observed. The CDP calls themselves happen after
        // event collection, because awaiting one here can overflow the receiver.
        if event.method == "Network.loadingFinished" {
            let request_id = event
                .params
                .get("requestId")
                .and_then(Value::as_str)
                .unwrap_or("");
            if let Some(index) = pending_bodies.remove(request_id) {
                body_reads.push(BodyRead {
                    request_id: request_id.to_string(),
                    index,
                    loading_finished_observed: true,
                });
            }
            continue;
        }

        if event.method == "Network.loadingFailed" {
            let request_id = event
                .params
                .get("requestId")
                .and_then(Value::as_str)
                .unwrap_or("");
            if let Some(index) = pending_bodies.remove(request_id) {
                entries[index].body_omitted = Some(
                    "Network.loadingFailed was observed before the body completed".to_string(),
                );
            }
            continue;
        }

        if event.method != "Network.responseReceived" {
            continue;
        }

        // The limit bounds returned entries, not the bounded observation grace for bodies already
        // selected. Keep draining their loadingFinished events without admitting more responses.
        if entries.len() >= limit {
            continue;
        }

        let Some(response) = event.params.get("response") else {
            continue;
        };

        let url = response
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        if let Some(ref f) = filter_lower
            && !url.to_ascii_lowercase().contains(f.as_str())
        {
            continue;
        }

        let status = match response.get("status").and_then(Value::as_u64) {
            Some(raw) => u16::try_from(raw)
                .map_err(|_| format!("Network.responseReceived status {raw} is out of range"))?,
            None => 0,
        };
        let content_type = response
            .get("mimeType")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let resource_type = event
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

        // A URL filter is an explicit selection, so it overrides the MIME allowlist.
        let wants_body = should_capture_body(
            capture_body,
            filter_lower.is_some(),
            is_capturable_type(&content_type),
        ) && !request_id.is_empty();
        if wants_body {
            pending_bodies.insert(request_id.to_string(), entries.len());
        }

        entries.push(NetworkEntry {
            url,
            resource_type,
            status,
            content_type,
            body: None,
            body_omitted: None,
            size: encoded_length,
            duration_ms: 0, // not available from responseReceived
        });

        if entries.len() >= limit && !pending_bodies.is_empty() {
            limit_deadline = Some(
                tokio::time::Instant::now()
                    .checked_add(BODY_EVENT_GRACE)
                    .ok_or("Network body grace deadline overflow")?,
            );
        }
    }

    body_reads.extend(
        pending_bodies
            .into_iter()
            .map(|(request_id, index)| BodyRead {
                request_id,
                index,
                loading_finished_observed: false,
            }),
    );

    let mut completed_reads = 0_usize;
    let read_result = tokio::time::timeout(BODY_READ_GRACE, async {
        for read in &body_reads {
            fill_body(
                client,
                &read.request_id,
                &mut entries[read.index],
                read.loading_finished_observed,
            )
            .await;
            completed_reads += 1;
        }
    })
    .await;
    if read_result.is_err() {
        for read in &body_reads[completed_reads..] {
            let observation = if read.loading_finished_observed {
                "Network.loadingFinished was observed"
            } else {
                "Network.loadingFinished was not observed before capture stopped"
            };
            entries[read.index].body_omitted = Some(format!(
                "{observation}; body read did not answer within the {}s post-capture window",
                BODY_READ_GRACE.as_secs()
            ));
        }
    }

    Ok(LiveCapture {
        entries,
        lost_events,
    })
}

/// Fetch and classify one entry's body in place. CDP can discard a completed body; that refusal
/// is part of this entry instead of becoming an unexplained absent field or erasing other entries.
async fn fill_body(
    client: &CdpClient,
    request_id: &str,
    entry: &mut NetworkEntry,
    loading_finished_observed: bool,
) {
    match fetch_response_body(client, request_id).await {
        Ok(FetchedBody::Text(text)) => entry.body = Some(text),
        Ok(FetchedBody::Binary { bytes }) => {
            entry.body_omitted = Some(format!(
                "binary body ({bytes} bytes) not shown; `chrome-agent download {}` fetches the bytes",
                entry.url
            ));
        }
        Err(error) => {
            let observation = if loading_finished_observed {
                "after Network.loadingFinished"
            } else {
                "Network.loadingFinished was not observed before capture stopped;"
            };
            entry.body_omitted = Some(format!("{observation} body unavailable from CDP: {error}"));
        }
    }
}

/// What one `network` invocation observed. `--abort` blocks for a window and reports what it
/// stopped; every other shape reports what it saw.
pub enum Capture {
    Blocked {
        pattern: String,
        seconds: u64,
        urls: Vec<String>,
    },
    Requests {
        entries: Vec<NetworkEntry>,
        lost_events: u64,
    },
}

impl Capture {
    /// The response every mode carries.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Self::Blocked { urls, .. } => json!({"ok": true, "blocked": urls.len(), "urls": urls}),
            Self::Requests {
                entries,
                lost_events: 0,
            } => json!({"ok": true, "requests": entries}),
            Self::Requests {
                entries,
                lost_events,
            } => json!({
                "ok": false,
                "error": event_loss("Network capture", "Network", *lost_events),
                "complete": false,
                "lostEvents": lost_events,
                "requests": entries,
            }),
        }
    }

    /// The CLI's rendering. `--abort` names its window before the list, because the command
    /// blocked for it.
    #[must_use]
    pub fn text(&self) -> String {
        match self {
            Self::Blocked {
                pattern,
                seconds,
                urls,
            } => {
                let mut out =
                    format!("Blocking requests matching \"{pattern}\" for {seconds}s...\n");
                for url in urls {
                    out += &format!("  blocked: {url}\n");
                }
                out + &format!("Blocked {} request(s)", urls.len())
            }
            Self::Requests {
                entries,
                lost_events: 0,
            } => format_text(entries),
            Self::Requests {
                entries,
                lost_events,
            } => format!(
                "{}\n\nIncomplete: lost {lost_events} Network event(s); entries are partial.",
                format_text(entries)
            ),
        }
    }

    #[must_use]
    pub const fn is_complete(&self) -> bool {
        !matches!(
            self,
            Self::Requests {
                lost_events: 1..,
                ..
            }
        )
    }
}

#[derive(Debug, thiserror::Error)]
#[error("network capture was incomplete and has already been reported")]
pub struct IncompleteCapture;

/// One `network` invocation, whichever of its three shapes was asked for. The single entry point
/// for CLI, pipe and batch — the branch used to be written out in both.
pub async fn collect(
    client: &CdpClient,
    filter: Option<&str>,
    body: bool,
    live: Option<u64>,
    limit: usize,
    abort: Option<&str>,
) -> Result<Capture, crate::BoxError> {
    if let Some(pattern) = abort {
        // `--live` doubles as the abort window (default 30s).
        let seconds = live.unwrap_or(30);
        let urls = run_route_abort(client, pattern, seconds).await?;
        return Ok(Capture::Blocked {
            pattern: pattern.to_string(),
            seconds,
            urls,
        });
    }
    let (entries, lost_events) = match live {
        Some(secs) => {
            let capture = run_live(client, filter, body, limit, secs).await?;
            (capture.entries, capture.lost_events)
        }
        None => (run_retroactive(client, filter, limit).await?, 0),
    };
    Ok(Capture::Requests {
        entries,
        lost_events,
    })
}

/// Render the entries as a table.
pub fn format_text(entries: &[NetworkEntry]) -> String {
    if entries.is_empty() {
        return "No network entries captured.".to_string();
    }
    let mut out = format!(
        "{:<70} {:>6} {:<14} {:>8} {:>6}\n{}\n",
        "URL",
        "STATUS",
        "TYPE",
        "SIZE",
        "MS",
        "-".repeat(110)
    );
    for e in entries {
        let url_display = crate::truncate::truncate_str(&e.url, 67, "...");
        let status_str = if e.status == 0 {
            "-".to_string()
        } else {
            e.status.to_string()
        };
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
        if let Some(ref omitted) = e.body_omitted {
            out += &format!("  body: <{omitted}>\n");
        }
    }
    out += &format!("\n{} entries", entries.len());
    out
}

/// `Network.getResponseBody`, preserving a refusal instead of turning it into an absent body.
async fn fetch_response_body(
    client: &CdpClient,
    request_id: &str,
) -> Result<FetchedBody, crate::BoxError> {
    let result: Value = client
        .call(
            "Network.getResponseBody",
            json!({ "requestId": request_id }),
        )
        .await
        .map_err(|error| format!("Could not read network response body {request_id}: {error}"))?;

    let base64_encoded = result
        .get("base64Encoded")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let body = result
        .get("body")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("Network response body {request_id} was not a string"))?;
    Ok(classify_body(body, base64_encoded))
}

/// `Fetch.enable` params pausing every request matching `pattern`. Extracted so tests can
/// assert the wire shape.
fn fetch_enable_params(pattern: &str) -> Value {
    json!({
        "patterns": [{"urlPattern": pattern, "requestStage": "Request"}]
    })
}

/// `Fetch.failRequest` params aborting a paused request. Extracted for the same reason.
fn fail_request_params(request_id: &str) -> Value {
    json!({
        "requestId": request_id,
        "reason": "BlockedByClient",
    })
}

/// Block requests matching a URL pattern using the Fetch domain.
pub async fn run_route_abort(
    client: &CdpClient,
    pattern: &str,
    timeout_secs: u64,
) -> Result<Vec<String>, crate::BoxError> {
    let deadline = std::time::Instant::now()
        .checked_add(std::time::Duration::from_secs(timeout_secs))
        .ok_or("Network abort timeout is too large")?;
    // Subscribe before enabling Fetch: a matching request can pause as soon as enable lands.
    let mut rx = client.fetch_events();
    client
        .send("Fetch.enable", fetch_enable_params(pattern))
        .await?;

    let capture: Result<Vec<String>, String> = async {
        let mut blocked = Vec::new();
        while std::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            let event = tokio::time::timeout(remaining, async {
                loop {
                    match rx.recv().await {
                        Ok(ev) if ev.method == "Fetch.requestPaused" => return Ok(ev),
                        Ok(_) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(missed)) => {
                            return Err(event_loss("Network abort", "Fetch", missed));
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            return Err("Event channel closed during network abort".to_string());
                        }
                    }
                }
            })
            .await;

            match event {
                Err(_) => break,
                Ok(Err(error)) => return Err(error),
                Ok(Ok(ev)) => {
                    let request_id = ev
                        .params
                        .get("requestId")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let url = ev
                        .params
                        .get("request")
                        .and_then(|r| r.get("url"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    client
                        .send("Fetch.failRequest", fail_request_params(request_id))
                        .await
                        .map_err(|e| format!("Failed to abort request {request_id}: {e}"))?;
                    if !url.is_empty() {
                        blocked.push(url);
                    }
                }
            }
        }
        Ok(blocked)
    }
    .await;

    let disabled = client.send("Fetch.disable", serde_json::json!({})).await;
    match (capture, disabled) {
        (Ok(blocked), Ok(())) => Ok(blocked),
        (Err(error), Ok(())) => Err(error.into()),
        (Ok(_), Err(error)) => {
            Err(format!("Failed to disable network abort routing: {error}").into())
        }
        (Err(capture_error), Err(disable_error)) => Err(format!(
            "{capture_error}; also failed to disable network abort routing: {disable_error}"
        )
        .into()),
    }
}

fn event_loss(operation: &str, domain: &str, missed: u64) -> String {
    format!("{operation} lost {missed} {domain} event(s); result is incomplete")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bug_url_truncation_utf8_safe() {
        // `&e.url[..67]` panics on multi-byte characters.
        let entry = NetworkEntry {
            url: "https://example.com/café/résumé/über/naïve/длинный".to_string(),
            resource_type: "Document".to_string(),
            status: 200,
            content_type: "text/html".to_string(),
            size: 1000,
            duration_ms: 50,
            body: None,
            body_omitted: None,
        };
        let text = format_text(&[entry]);
        assert!(!text.is_empty());
    }

    #[test]
    fn route_abort_params_structure() {
        let params = fetch_enable_params("*tracking*");
        let patterns = params.get("patterns").unwrap().as_array().unwrap();
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0]["urlPattern"], "*tracking*");
        assert_eq!(patterns[0]["requestStage"], "Request");

        let fail = fail_request_params("req-42");
        assert_eq!(fail["requestId"], "req-42");
        assert_eq!(fail["reason"], "BlockedByClient");
    }

    #[test]
    fn event_loss_is_an_explicit_refusal() {
        let message = event_loss("Network capture", "Network", 42);
        assert!(message.contains("lost 42 Network event(s)"));
        assert!(message.contains("result is incomplete"));
    }

    #[test]
    fn event_loss_keeps_partial_entries_but_never_answers_ok() {
        let capture = Capture::Requests {
            entries: vec![NetworkEntry {
                url: "https://example.test/observed".to_string(),
                resource_type: "XHR".to_string(),
                status: 200,
                content_type: "application/json".to_string(),
                body: None,
                body_omitted: None,
                size: 12,
                duration_ms: 0,
            }],
            lost_events: 7,
        };
        let answer = capture.to_json();
        assert_eq!(answer["ok"], false);
        assert_eq!(answer["complete"], false);
        assert_eq!(answer["lostEvents"], 7);
        assert_eq!(answer["requests"].as_array().map(Vec::len), Some(1));
        assert!(!capture.is_complete());
    }

    #[test]
    fn a_url_filter_overrides_the_mime_allowlist() {
        // The allowlist protects an unfiltered --body from images and fonts; a filter is
        // the caller naming what they want.
        assert!(should_capture_body(true, true, false));
        assert!(should_capture_body(true, false, true));
        assert!(!should_capture_body(true, false, false));
        // Without --body nothing is fetched, filter or not.
        assert!(!should_capture_body(false, true, true));
    }

    #[test]
    fn base64_from_chrome_means_undecided_not_binary() {
        // "cmV0cmllczogMw==" is "retries: 3": textual content stays text whatever the
        // transport encoding said.
        match classify_body("cmV0cmllczogMw==", true) {
            FetchedBody::Text(text) => assert_eq!(text, "retries: 3"),
            FetchedBody::Binary { .. } => panic!("valid UTF-8 counted as binary"),
        }
        // "//79/A==" decodes to [0xFF, 0xFE, 0xFD, 0xFC]: not UTF-8, counted not printed.
        match classify_body("//79/A==", true) {
            FetchedBody::Binary { bytes } => assert_eq!(bytes, 4),
            FetchedBody::Text(_) => panic!("raw bytes classified as text"),
        }
    }

    #[test]
    fn a_text_body_keeps_the_existing_truncation() {
        match classify_body(&"x".repeat(3000), false) {
            FetchedBody::Text(text) => {
                assert!(text.len() < 3000);
                assert!(text.ends_with("...(truncated)"));
            }
            FetchedBody::Binary { .. } => panic!("text answer classified as binary"),
        }
    }

    #[test]
    fn bug_body_truncation_utf8_safe() {
        // `&body[..2000]` panics on multi-byte characters.
        let entry = NetworkEntry {
            url: "https://example.com".to_string(),
            resource_type: "XHR".to_string(),
            status: 200,
            content_type: "application/json".to_string(),
            size: 5000,
            duration_ms: 50,
            body: Some("é".repeat(3000)),
            body_omitted: None,
        };
        let text = format_text(&[entry]);
        assert!(!text.is_empty());
    }

    #[test]
    fn entry_serializes_resource_type_not_method() {
        // The field must serialize as `resourceType`; it never held an HTTP method.
        let entry = NetworkEntry {
            url: "https://example.com/api".to_string(),
            resource_type: "XHR".to_string(),
            status: 200,
            content_type: "application/json".to_string(),
            size: 10,
            duration_ms: 5,
            body: None,
            body_omitted: None,
        };
        let v = serde_json::to_value(&entry).unwrap();
        assert_eq!(v["resourceType"], "XHR");
        assert!(v.get("method").is_none(), "must not emit a `method` key");
    }

    #[test]
    fn bug_body_preview_utf8_safe() {
        // `&b[..200]` panics on multi-byte characters.
        let entry = NetworkEntry {
            url: "https://example.com".to_string(),
            resource_type: "XHR".to_string(),
            status: 200,
            content_type: "application/json".to_string(),
            size: 500,
            duration_ms: 50,
            body: Some("日本語テスト".repeat(100)),
            body_omitted: None,
        };
        let text = format_text(&[entry]);
        assert!(!text.is_empty());
    }
}
