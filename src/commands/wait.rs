use std::collections::HashSet;
use std::time::{Duration, Instant};

use serde_json::json;

use crate::cdp::client::CdpClient;
use crate::cdp::types::EvaluateResult;

/// Whether an event flipped the page's idle state.
#[derive(Debug, PartialEq, Eq)]
pub enum Transition {
    /// Went from idle to at least one request in flight.
    BecameBusy,
    /// Went from busy back to zero requests in flight.
    BecameIdle,
    /// Idle state unchanged (e.g. an extra concurrent request, or an ignored event).
    NoChange,
}

/// Tracks in-flight network requests by requestId so we can tell when the page
/// has gone quiet. Keying on a set (not a bare counter) makes duplicate `start`
/// events and repeated `finish`/`fail` events harmless — removing an id that is
/// not present is a no-op.
#[derive(Default)]
pub struct InFlightTracker {
    in_flight: HashSet<String>,
}

impl InFlightTracker {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Update the set from a CDP event. Unrelated events are ignored.
    pub fn on_event(&mut self, method: &str, request_id: Option<&str>) {
        let Some(id) = request_id else { return };
        match method {
            "Network.requestWillBeSent" => {
                self.in_flight.insert(id.to_string());
            }
            "Network.loadingFinished" | "Network.loadingFailed" => {
                self.in_flight.remove(id);
            }
            _ => {}
        }
    }

    /// Apply an event and report whether it flipped the idle state. This is the
    /// exact decision the wait loop uses to start/stop its idle clock.
    pub fn observe(&mut self, method: &str, request_id: Option<&str>) -> Transition {
        let was_idle = self.is_idle();
        self.on_event(method, request_id);
        match (was_idle, self.is_idle()) {
            (true, false) => Transition::BecameBusy,
            (false, true) => Transition::BecameIdle,
            _ => Transition::NoChange,
        }
    }

    #[must_use]
    pub fn count(&self) -> usize {
        self.in_flight.len()
    }

    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.in_flight.is_empty()
    }
}

/// Turn an in-page evaluation exception into a clear, actionable error.
///
/// `wait text` compiles the pattern with `new RegExp(pattern)`, so an invalid
/// pattern throws a `SyntaxError` in the page. Without this the probe would just
/// return no match and time out with a misleading "waiting for text" message.
#[must_use]
fn eval_exception_message(what: &str, pattern: &str, detail: &str) -> String {
    if what == "text" {
        format!(
            "Invalid regex for `wait text` pattern \"{pattern}\": {detail}. \
             The pattern is compiled as a JavaScript RegExp — escape metacharacters \
             (e.g. \\( \\[ \\\\ ) to match them literally."
        )
    } else {
        format!("Error evaluating wait condition ({what} \"{pattern}\"): {detail}")
    }
}

/// Poll the page until a condition is met, or timeout.
pub async fn run(
    client: &CdpClient,
    what: &str,
    pattern: &str,
    timeout_secs: u64,
    idle_ms: u64,
) -> Result<String, crate::BoxError> {
    if what == "network-idle" {
        return wait_network_idle(client, timeout_secs, idle_ms).await;
    }

    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let poll_interval = Duration::from_millis(200);

    let expression = match what {
        // Regex semantics: the pattern is compiled in-page with `new RegExp(pattern)`
        // and tested against `document.body.innerText`. This supports both plain
        // substrings and full regex (e.g. "Foo|Bar", "^Loading") because every literal
        // string is a valid regex that matches itself. The flip side: regex
        // metacharacters are significant — a pattern like "cost($5)" is an *invalid*
        // regex (unbalanced group) and throws a SyntaxError in the page rather than
        // matching literally. We surface that exception below instead of silently
        // timing out (see the `exception_details` check).
        "text" => format!(
            "new RegExp({}).test(document.body.innerText)",
            serde_json::to_string(pattern)?
        ),
        "url" => format!(
            "location.href.includes({})",
            serde_json::to_string(pattern)?
        ),
        "selector" => format!(
            "!!document.querySelector({})",
            serde_json::to_string(pattern)?
        ),
        other => return Err(format!(
            "Unknown wait type: {other}. Use \"text\", \"url\", \"selector\", or \"network-idle\"."
        ).into()),
    };

    loop {
        let result: EvaluateResult = client
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "returnByValue": true,
                }),
            )
            .await?;

        // A thrown in-page exception (most commonly an invalid `text` regex) leaves
        // `result.value` absent, which would otherwise look like "no match" and loop
        // until the misleading timeout. Detect it and surface a clear error instead.
        if let Some(exc) = result.exception_details.as_ref() {
            let detail = exc
                .exception
                .as_ref()
                .and_then(|o| o.description.as_ref())
                .map_or(exc.text.as_str(), String::as_str);
            // Errors' `description` carries a multi-line stack — keep just the message.
            let detail = detail.lines().next().unwrap_or(detail);
            return Err(eval_exception_message(what, pattern, detail).into());
        }

        let matched = result
            .result
            .value
            .as_ref()
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

        if matched {
            return Ok(format!("Found: {what} matching \"{pattern}\""));
        }

        if Instant::now() >= deadline {
            return Err(format!(
                "Timeout after {timeout_secs}s waiting for {what} matching \"{pattern}\""
            )
            .into());
        }

        tokio::time::sleep(poll_interval).await;
    }
}

/// Wait until there are zero in-flight network requests for `idle_ms` continuously,
/// bounded by `timeout_secs`. Opt-in (enables the Network domain) so it stays off
/// the stealth hot path.
async fn wait_network_idle(
    client: &CdpClient,
    timeout_secs: u64,
    idle_ms: u64,
) -> Result<String, crate::BoxError> {
    let mut rx = client.events();
    client.enable("Network").await?;

    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let idle = Duration::from_millis(idle_ms);
    // Poll cap so we re-check the idle timer even when no events arrive.
    let poll = idle.min(Duration::from_millis(100)).max(Duration::from_millis(10));

    let mut tracker = InFlightTracker::new();

    // CDP does NOT replay `Network.requestWillBeSent` for requests that were
    // already in flight when we called `Network.enable`. So an empty tracker does
    // NOT mean the page is quiet — starting the idle clock right away produces a
    // false "idle" while the initial load is still fetching subresources. Gate the
    // clock on the document's load state instead: only treat the page as
    // idle-eligible once `document.readyState === 'complete'`. While it's still
    // loading we hold the clock off and re-probe, which lets the pre-existing
    // in-flight requests finish (via `Page`/`load`) before we ever declare idle.
    //
    // Limitation: a `fetch`/XHR kicked off *before* subscribe that keeps running
    // *after* `readyState` reaches "complete" still isn't individually tracked
    // (its `requestWillBeSent` was missed). readyState covers the common "page is
    // still loading" case; post-load background requests remain best-effort.
    let mut page_loaded = document_complete(client).await.unwrap_or(false);
    let mut idle_since = refresh_idle_clock(page_loaded, &tracker, None);

    loop {
        if let Some(since) = idle_since
            && since.elapsed() >= idle
        {
            return Ok(format!("Network idle for {idle_ms}ms"));
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "Timeout after {timeout_secs}s waiting for network idle (in-flight: {})",
                tracker.count()
            )
            .into());
        }

        match tokio::time::timeout(poll, rx.recv()).await {
            Ok(Ok(event)) => {
                let request_id = event
                    .params
                    .get("requestId")
                    .and_then(serde_json::Value::as_str);
                tracker.observe(&event.method, request_id);
                idle_since = refresh_idle_clock(page_loaded, &tracker, idle_since);
            }
            // Missed events under load — conservatively restart the idle clock.
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {
                idle_since = None;
            }
            Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                return Err("Connection closed while waiting for network idle".into());
            }
            // No event within the poll window. Until the initial load completes,
            // re-probe readyState so pre-existing in-flight requests keep us busy;
            // then re-check the idle timer.
            Err(_) => {
                if !page_loaded {
                    page_loaded = document_complete(client).await.unwrap_or(page_loaded);
                }
                idle_since = refresh_idle_clock(page_loaded, &tracker, idle_since);
            }
        }
    }
}

/// The page counts as quiet only when its initial load has completed AND no
/// tracked requests are in flight. Gating on the load state is what prevents a
/// false "idle" for requests already in flight before we subscribed (CDP does not
/// replay their `requestWillBeSent`).
#[must_use]
fn is_quiet(page_loaded: bool, tracker: &InFlightTracker) -> bool {
    page_loaded && tracker.is_idle()
}

/// Recompute the idle clock from the combined quiet state. The clock starts only
/// on a rising edge (busy → quiet) so a continuously-quiet stretch is measured
/// from its true start, and is cleared the moment the page goes busy again.
fn refresh_idle_clock(
    page_loaded: bool,
    tracker: &InFlightTracker,
    current: Option<Instant>,
) -> Option<Instant> {
    match (current.is_some(), is_quiet(page_loaded, tracker)) {
        (false, true) => Some(Instant::now()),
        (true, false) => None,
        _ => current,
    }
}

/// Probe whether the document has finished its initial load. Used to seed and
/// re-check the network-idle gate; see `wait_network_idle` for why an empty
/// in-flight tracker is not sufficient to declare the page quiet.
async fn document_complete(client: &CdpClient) -> Result<bool, crate::BoxError> {
    let result: EvaluateResult = client
        .call(
            "Runtime.evaluate",
            json!({
                "expression": "document.readyState === 'complete'",
                "returnByValue": true,
            }),
        )
        .await?;
    Ok(result
        .result
        .value
        .as_ref()
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::{eval_exception_message, is_quiet, refresh_idle_clock, InFlightTracker, Transition};

    // --- A10c: invalid-regex exception surfacing ---------------------------

    #[test]
    fn text_exception_reports_invalid_regex() {
        let msg = eval_exception_message("text", "cost($5)", "SyntaxError: Invalid regular expression");
        assert!(msg.contains("Invalid regex"), "should name the regex problem: {msg}");
        assert!(msg.contains("cost($5)"), "should echo the offending pattern: {msg}");
        assert!(msg.contains("SyntaxError"), "should include the engine detail: {msg}");
        assert!(msg.contains("RegExp"), "should explain regex semantics: {msg}");
    }

    #[test]
    fn non_text_exception_is_generic() {
        let msg = eval_exception_message("selector", "div", "TypeError: boom");
        assert!(!msg.contains("Invalid regex"), "non-text wait is not a regex: {msg}");
        assert!(msg.contains("selector"));
        assert!(msg.contains("div"));
        assert!(msg.contains("TypeError: boom"));
    }

    // --- A10d: network-idle gating on load state ---------------------------

    #[test]
    fn not_quiet_until_page_loaded_even_with_empty_tracker() {
        // The core of the bug: an empty tracker (no observed requests) must NOT
        // count as idle while the initial load is still in progress.
        let t = InFlightTracker::new();
        assert!(t.is_idle(), "no observed requests");
        assert!(!is_quiet(false, &t), "page still loading -> not quiet");
        assert!(is_quiet(true, &t), "load complete + empty tracker -> quiet");
    }

    #[test]
    fn not_quiet_when_requests_in_flight_even_if_loaded() {
        let mut t = InFlightTracker::new();
        t.on_event("Network.requestWillBeSent", Some("r1"));
        assert!(!is_quiet(true, &t), "in-flight request keeps it busy");
    }

    #[test]
    fn idle_clock_holds_off_until_page_loaded() {
        let t = InFlightTracker::new();
        // While loading, the clock never starts despite the empty tracker.
        assert!(refresh_idle_clock(false, &t, None).is_none());
        // Load completes -> clock starts (rising edge).
        let started = refresh_idle_clock(true, &t, None);
        assert!(started.is_some(), "load complete should start the idle clock");
        // Staying quiet must preserve the original start (no reset).
        let kept = refresh_idle_clock(true, &t, started);
        assert_eq!(kept, started, "continuously-quiet must not reset the clock");
    }

    #[test]
    fn idle_clock_clears_when_page_goes_busy() {
        let mut t = InFlightTracker::new();
        let running = Some(Instant::now());
        t.on_event("Network.requestWillBeSent", Some("r1"));
        assert!(
            refresh_idle_clock(true, &t, running).is_none(),
            "a new request must stop the idle clock"
        );
    }

    #[test]
    fn starts_idle() {
        let t = InFlightTracker::new();
        assert!(t.is_idle());
        assert_eq!(t.count(), 0);
    }

    #[test]
    fn request_then_finish_returns_to_idle() {
        let mut t = InFlightTracker::new();
        t.on_event("Network.requestWillBeSent", Some("r1"));
        assert!(!t.is_idle());
        assert_eq!(t.count(), 1);
        t.on_event("Network.loadingFinished", Some("r1"));
        assert!(t.is_idle());
    }

    #[test]
    fn failed_request_also_clears() {
        let mut t = InFlightTracker::new();
        t.on_event("Network.requestWillBeSent", Some("r1"));
        t.on_event("Network.loadingFailed", Some("r1"));
        assert!(t.is_idle());
    }

    #[test]
    fn concurrent_requests_need_all_to_finish() {
        let mut t = InFlightTracker::new();
        t.on_event("Network.requestWillBeSent", Some("a"));
        t.on_event("Network.requestWillBeSent", Some("b"));
        assert_eq!(t.count(), 2);
        t.on_event("Network.loadingFinished", Some("a"));
        assert!(!t.is_idle(), "still one request in flight");
        t.on_event("Network.loadingFinished", Some("b"));
        assert!(t.is_idle());
    }

    #[test]
    fn duplicate_request_id_not_double_counted() {
        let mut t = InFlightTracker::new();
        t.on_event("Network.requestWillBeSent", Some("dup"));
        t.on_event("Network.requestWillBeSent", Some("dup"));
        assert_eq!(t.count(), 1);
        t.on_event("Network.loadingFinished", Some("dup"));
        assert!(t.is_idle());
    }

    #[test]
    fn finish_for_unknown_id_is_noop() {
        let mut t = InFlightTracker::new();
        t.on_event("Network.loadingFinished", Some("ghost"));
        assert!(t.is_idle());
        assert_eq!(t.count(), 0);
    }

    #[test]
    fn unrelated_event_and_missing_id_ignored() {
        let mut t = InFlightTracker::new();
        t.on_event("Network.responseReceived", Some("r1")); // not a start/finish
        t.on_event("Network.requestWillBeSent", None); // no id
        assert!(t.is_idle());
    }

    #[test]
    fn repeated_finish_is_harmless() {
        // A late/duplicate finish for the same id must not underflow or flip state.
        let mut t = InFlightTracker::new();
        t.on_event("Network.requestWillBeSent", Some("r1"));
        t.on_event("Network.loadingFinished", Some("r1"));
        t.on_event("Network.loadingFinished", Some("r1"));
        assert!(t.is_idle());
        assert_eq!(t.count(), 0);
    }

    #[test]
    fn observe_reports_idle_transitions() {
        // This is the exact decision the wait loop drives its idle clock from.
        let mut t = InFlightTracker::new();
        assert_eq!(t.observe("Network.requestWillBeSent", Some("a")), Transition::BecameBusy);
        // A second concurrent request does not re-trigger "busy".
        assert_eq!(t.observe("Network.requestWillBeSent", Some("b")), Transition::NoChange);
        // First of two finishing keeps us busy.
        assert_eq!(t.observe("Network.loadingFinished", Some("a")), Transition::NoChange);
        // Last one finishing flips to idle exactly once.
        assert_eq!(t.observe("Network.loadingFinished", Some("b")), Transition::BecameIdle);
    }

    #[test]
    fn observe_ignores_noise_without_transition() {
        let mut t = InFlightTracker::new();
        assert_eq!(t.observe("Network.responseReceived", Some("x")), Transition::NoChange);
        assert_eq!(t.observe("Network.requestWillBeSent", None), Transition::NoChange);
        assert!(t.is_idle());
    }
}
