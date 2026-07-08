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
        // Support both plain substrings and regex patterns (e.g. "Foo|Bar", "^Loading").
        // new RegExp(pattern) is backward-compatible with plain strings since every
        // literal string is a valid regex that matches itself.
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
    // Already quiet? Start the idle clock immediately.
    let mut idle_since = Some(Instant::now());

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
                match tracker.observe(&event.method, request_id) {
                    // Became busy: stop the idle clock.
                    Transition::BecameBusy => idle_since = None,
                    // Became idle: (re)start the idle clock.
                    Transition::BecameIdle => idle_since = Some(Instant::now()),
                    Transition::NoChange => {}
                }
            }
            // Missed events under load — conservatively treat as activity.
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {
                idle_since = None;
            }
            Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                return Err("Connection closed while waiting for network idle".into());
            }
            // No event within the poll window — loop to re-check timers.
            Err(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{InFlightTracker, Transition};

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
