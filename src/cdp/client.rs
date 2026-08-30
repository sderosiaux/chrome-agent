use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::{broadcast, oneshot, Mutex};

use super::transport::{self, CdpSender, CdpTransportError};
use super::types::{CdpEvent, CdpMessage, CdpRequest, CdpResponse};

type PendingMap = Arc<Mutex<HashMap<u64, oneshot::Sender<PendingReply>>>>;

/// What the dispatcher hands back to the call waiting on an `id`.
enum PendingReply {
    /// Chrome answered and the answer parsed.
    Read(CdpResponse),
    /// A message carrying this `id` could not be read; carries serde's complaint (type
    /// mismatch and position) plus the message's key names, so the call fails instead of
    /// waiting out its deadline.
    Unreadable(String),
}

/// Deadline applied to a CDP response when the caller sets none. Matches the CLI's
/// `--timeout` default.
const DEFAULT_CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Deadline for an input event's acknowledgement, whatever `--timeout` says. 8 s clears the
/// fixed 5.00 s stall a background tab produces and undercuts the 30 s default. A deadline on
/// the ANSWER, never on the event, so its failure forbids a retry (`element::input_timeout`).
const INPUT_ACK_DEADLINE: std::time::Duration = std::time::Duration::from_secs(8);
const DIALOG_REQUEST_ID_START: u64 = 1_000_000_000;
const DIALOG_REQUEST_ID_MAX: u64 = i32::MAX as u64;

/// What a call that ran out of time says about itself. Only a slow computation may be
/// repeated (raise `--timeout`); an input event whose acknowledgement expired may already have
/// reached the page. `hints::error_hint` keys the recovery off this wording.
fn timeout_message(method: &str, deadline: std::time::Duration) -> String {
    if method.starts_with("Input.") {
        format!(
            "{method} was dispatched and Chrome did not acknowledge it within {}s, so what the \
             page did with it is unknown. The event may already have reached the page.",
            deadline.as_secs()
        )
    } else {
        format!(
            "{method} did not answer within {}s. An in-page promise that never settles \
             (awaitPromise) is the usual cause; raise --timeout if the page is merely slow.",
            deadline.as_secs()
        )
    }
}

/// What a call says when Chrome answered and chrome-agent could not read the answer. The
/// `response parse` prefix is load-bearing: `hints::error_hint` keys its recovery off it. Split
/// on `Input.` because an unreadable acknowledgement still means the event was dispatched.
fn unreadable_message(method: &str, detail: &str) -> String {
    if method.starts_with("Input.") {
        format!(
            "response parse: {method} was dispatched and Chrome answered, but chrome-agent \
             could not read the answer ({detail}), so what the page did with it is unknown. \
             The event may already have reached the page."
        )
    } else {
        format!(
            "response parse: {method} was answered and chrome-agent could not read the answer \
             ({detail}), so the command produced nothing. Chrome replied; the page is not slow."
        )
    }
}

/// Execution context bound to a frame by the `frame` command: `eval` runs in `context_id`,
/// `inspect`/snapshot scope to `frame_id`. Navigating the top document kills the isolated
/// world, so callers must clear this on navigation rather than send a dead `contextId`.
#[derive(Clone, Debug)]
pub struct FrameContext {
    /// `Page.FrameId` of the target frame.
    pub frame_id: String,
    /// `Runtime.ExecutionContextId` of the frame's isolated world.
    pub context_id: i64,
}

/// High-level CDP client: request/response correlation and event dispatch. Spawns a dispatcher
/// task routing incoming messages to pending request futures or to event subscribers.
pub struct CdpClient {
    sender: CdpSender,
    next_id: AtomicU64,
    pending: PendingMap,
    events_tx: broadcast::Sender<CdpEvent>,
    _dispatcher: tokio::task::JoinHandle<()>,
    /// Frame the `frame` command switched into. Interior-mutable so `eval`/`inspect`, which
    /// take `&self`, can read it.
    frame_ctx: std::sync::Mutex<Option<FrameContext>>,
    /// How long to wait for a response. Without a deadline, an `awaitPromise` evaluation hangs
    /// forever and silently on a promise that never settles.
    call_timeout: std::sync::Mutex<std::time::Duration>,
    /// When the last input event went out. Set in `element`, read where the verdict is
    /// assembled, so `no_effect`'s observation window is measured rather than assumed.
    last_dispatch: std::sync::Mutex<Option<std::time::Instant>>,
    /// Whether this connection already brought its page forward. `Page.bringToFront` is a
    /// browser state change, so only pointer paths call it — see `ensure_foreground`.
    foregrounded: AtomicBool,
    /// How long this action waited for a page load. Set in `element`, read where the response
    /// is assembled.
    settle_wait: std::sync::Mutex<Option<std::time::Duration>>,
    /// Whether to synthesize taps instead of mouse clicks. Connection-local, so the target's
    /// persisted `--touch` setting does not leak into sibling pages.
    touch_emulation: AtomicBool,
}

#[derive(Debug, thiserror::Error)]
pub enum CdpClientError {
    #[error("transport: {0}")]
    Transport(#[from] CdpTransportError),
    #[error("serialization: {0}")]
    Serialization(serde_json::Error),
    #[error("CDP error {code}: {message}")]
    Protocol { code: i64, message: String },
    #[error("response parse: {0}")]
    ResponseParse(serde_json::Error),
    /// A message carrying this call's `id` did not fit `CdpMessage`: the ENVELOPE not fitting,
    /// where [`Self::ResponseParse`] is `result` not fitting one call site's `R`. Both carry a
    /// `response parse` prefix so `hints::error_hint` gives them the same recovery. Built by
    /// [`unreadable_message`].
    #[error("{0}")]
    Unreadable(String),
    #[error("timeout: {0}")]
    Timeout(String),
    #[error("dispatcher task exited")]
    DispatcherGone,
}

impl CdpClient {
    /// Connect to a Chrome `DevTools` Protocol endpoint.
    pub async fn connect(url: &str) -> Result<Self, CdpClientError> {
        let (sender, receiver) = transport::connect(url).await?;
        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let (events_tx, _) = broadcast::channel::<CdpEvent>(256);

        let dispatcher = tokio::spawn(dispatch_loop(
            receiver,
            Arc::clone(&pending),
            events_tx.clone(),
        ));

        Ok(Self {
            sender,
            next_id: AtomicU64::new(1),
            pending,
            events_tx,
            _dispatcher: dispatcher,
            frame_ctx: std::sync::Mutex::new(None),
            call_timeout: std::sync::Mutex::new(DEFAULT_CALL_TIMEOUT),
            last_dispatch: std::sync::Mutex::new(None),
            foregrounded: AtomicBool::new(false),
            settle_wait: std::sync::Mutex::new(None),
            touch_emulation: AtomicBool::new(false),
        })
    }

    pub fn mark_dispatch(&self) {
        if let Ok(mut slot) = self.last_dispatch.lock() {
            *slot = Some(std::time::Instant::now());
        }
        // Pipe reuses one connection, so clear the settle wait where the action starts: it
        // belongs to the action that paid it, not to the next command.
        if let Ok(mut slot) = self.settle_wait.lock() {
            *slot = None;
        }
    }

    /// Bring this connection's page to the foreground, once.
    ///
    /// On a hidden page `Input.dispatchMouseEvent` answers on a fixed 5 s timer; after
    /// `Page.bringToFront` (3 ms) the same events take 0–6 ms. Pointer paths only:
    /// `Input.dispatchKeyEvent` answers in 1 ms hidden. So with several pages open, clicking
    /// on one foregrounds it. Best effort — a refusal only costs the latency this removes.
    pub async fn ensure_foreground(&self) {
        if self.foregrounded.swap(true, Ordering::Relaxed) {
            return;
        }
        // Boxed: inlining a fourth `call` state machine into `run::run`'s match arm pushes
        // that frame past clippy's ceiling.
        let call = Box::pin(self.call::<_, Value>("Page.bringToFront", serde_json::json!({})));
        let _: Result<Value, _> = call.await;
    }

    pub fn note_settle_wait(&self, waited: std::time::Duration) {
        if let Ok(mut slot) = self.settle_wait.lock() {
            *slot = Some(waited);
        }
    }

    /// How long this action waited for a load, when it waited at all. Takes rather than reads:
    /// a connection that outlives the action (pipe, batch) must not hand it to the next one.
    #[must_use]
    pub fn take_settle_wait_ms(&self) -> Option<u64> {
        let waited = self.settle_wait.lock().ok()?.take()?;
        u64::try_from(waited.as_millis()).ok()
    }

    pub(crate) fn set_touch_emulation(&self, enabled: bool) {
        self.touch_emulation.store(enabled, Ordering::Relaxed);
    }

    pub(crate) fn touch_emulation_enabled(&self) -> bool {
        self.touch_emulation.load(Ordering::Relaxed)
    }

    /// How long ago the last input event went out, or `None` if none has.
    #[must_use]
    pub fn ms_since_dispatch(&self) -> Option<u64> {
        let at = (*self.last_dispatch.lock().ok()?)?;
        u64::try_from(at.elapsed().as_millis()).ok()
    }

    pub fn frame_context(&self) -> Option<FrameContext> {
        self.frame_ctx.lock().unwrap().clone()
    }

    /// Bind (`Some`) or clear (`None`) the current frame context: setting it scopes `eval`
    /// and `inspect` to that frame, clearing restores the top document. Navigation clears it.
    pub fn set_frame_context(&self, ctx: Option<FrameContext>) {
        *self.frame_ctx.lock().unwrap() = ctx;
    }

    /// Send a CDP command and wait for the typed response.
    pub async fn call<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &'static str,
        params: P,
    ) -> Result<R, CdpClientError> {
        self.call_with_session(method, params, None).await
    }

    pub async fn call_with_session<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &'static str,
        params: P,
        session_id: Option<String>,
    ) -> Result<R, CdpClientError> {
        self.call_within(method, params, session_id, self.call_timeout()).await
    }

    /// Dispatch an input event and wait for Chrome to acknowledge it, under
    /// [`INPUT_ACK_DEADLINE`] rather than `--timeout`, which is the caller's patience for the
    /// page's own work and not for the browser's input pipeline.
    pub async fn send_input<P: Serialize>(
        &self,
        method: &'static str,
        params: P,
    ) -> Result<(), CdpClientError> {
        let _: Value = self.call_within(method, params, None, INPUT_ACK_DEADLINE).await?;
        Ok(())
    }

    async fn call_within<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &'static str,
        params: P,
        session_id: Option<String>,
        deadline: std::time::Duration,
    ) -> Result<R, CdpClientError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let params_value =
            serde_json::to_value(params).map_err(CdpClientError::Serialization)?;

        let request = CdpRequest {
            id,
            method,
            params: params_value,
            session_id,
        };

        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let json = serde_json::to_string(&request).map_err(CdpClientError::Serialization)?;
        if let Err(e) = self.sender.send(json).await {
            self.pending.lock().await.remove(&id);
            return Err(e.into());
        }

        let response = match tokio::time::timeout(deadline, rx).await {
            Ok(received) => match received.map_err(|_| CdpClientError::DispatcherGone)? {
                PendingReply::Read(response) => response,
                // Arrives as fast as the message did, instead of as a deadline nobody set.
                PendingReply::Unreadable(detail) => {
                    return Err(CdpClientError::Unreadable(unreadable_message(
                        method, &detail,
                    )))
                }
            },
            Err(_) => {
                // Drop the slot: keeping it leaks an entry per timed-out call and would
                // deliver a late answer to a receiver nobody awaits.
                self.pending.lock().await.remove(&id);
                return Err(CdpClientError::Timeout(timeout_message(method, deadline)));
            }
        };

        if let Some(error) = response.error {
            return Err(CdpClientError::Protocol {
                code: error.code,
                message: error.message,
            });
        }

        let result_value = response.result.unwrap_or_default();
        serde_json::from_value(result_value).map_err(CdpClientError::ResponseParse)
    }

    /// Send a CDP command that returns no meaningful result (e.g. `Page.enable`).
    pub async fn send<P: Serialize>(
        &self,
        method: &'static str,
        params: P,
    ) -> Result<(), CdpClientError> {
        let _: Value = self.call(method, params).await?;
        Ok(())
    }

    #[must_use]
    pub fn call_timeout(&self) -> std::time::Duration {
        self.call_timeout.lock().map_or(DEFAULT_CALL_TIMEOUT, |d| *d)
    }

    /// Set the deadline for every subsequent call, from the caller's `--timeout`.
    pub fn set_call_timeout(&self, timeout: std::time::Duration) {
        if let Ok(mut slot) = self.call_timeout.lock() {
            *slot = timeout;
        }
    }

    pub fn events(&self) -> broadcast::Receiver<CdpEvent> {
        self.events_tx.subscribe()
    }

    /// Install a background task that auto-answers JS dialogs per `policy`, so an unanswered
    /// one cannot hang the next command. No-op for `DialogPolicy::Manual`. Needs the Page
    /// domain enabled for `Page.javascriptDialogOpening` to fire; lives as long as the
    /// connection.
    pub fn spawn_dialog_handler(
        &self,
        policy: crate::setup::DialogPolicy,
        prompt_text: Option<String>,
    ) {
        if !policy.auto_handles() {
            return;
        }
        let mut rx = self.events();
        let sender = self.sender.clone();
        tokio::spawn(async move {
            // Fire-and-forget ids stay inside Chromium's accepted signed 32-bit range and
            // far above the sequential request ids. Chromium silently ignores anything
            // larger (2^40 left the dialog open and its command blocked).
            let mut local_id = DIALOG_REQUEST_ID_START;
            loop {
                match rx.recv().await {
                    Ok(event) if event.method == "Page.javascriptDialogOpening" => {
                        let dtype = event
                            .params
                            .get("type")
                            .and_then(Value::as_str)
                            .unwrap_or("alert");
                        let message = event
                            .params
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        let decision =
                            crate::setup::dialog_decision(policy, dtype, prompt_text.as_deref());
                        let mut params = serde_json::json!({ "accept": decision.accept });
                        if let Some(pt) = &decision.prompt_text {
                            params["promptText"] = Value::String(pt.clone());
                        }
                        let request = serde_json::json!({
                            "id": local_id,
                            "method": "Page.handleJavaScriptDialog",
                            "params": params,
                        });
                        local_id = if local_id >= DIALOG_REQUEST_ID_MAX {
                            DIALOG_REQUEST_ID_START
                        } else {
                            local_id + 1
                        };
                        let _ = sender.send(request.to_string()).await;
                        eprintln!(
                            "dialog auto-{}: {dtype} {message:?}",
                            if decision.accept { "accepted" } else { "dismissed" }
                        );
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        // A dropped Page.javascriptDialogOpening leaves the page blocked with
                        // no DOM signal. Report on stderr (never stdout, so --json stays
                        // clean) and keep handling.
                        eprintln!(
                            "dialog handler lagged: {n} event(s) dropped; a dialog may be unanswered"
                        );
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    pub async fn wait_for_event(
        &self,
        method: &str,
        timeout: std::time::Duration,
    ) -> Result<CdpEvent, CdpClientError> {
        let mut rx = self.events();
        Self::wait_for_event_on(&mut rx, method, timeout).await
    }

    /// Wait for a specific CDP event on an already-subscribed receiver.
    ///
    /// Subscribe with [`Self::events`] *before* issuing the triggering command: a fast (e.g.
    /// cached) response fires the event before a late subscription exists, and the wait then
    /// stalls until the timeout.
    pub async fn wait_for_event_on(
        rx: &mut broadcast::Receiver<CdpEvent>,
        method: &str,
        timeout: std::time::Duration,
    ) -> Result<CdpEvent, CdpClientError> {
        let result = tokio::time::timeout(timeout, async {
            loop {
                match rx.recv().await {
                    Ok(event) if event.method == method => return Ok(event),
                    Ok(_)
                    | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        return Err(CdpClientError::DispatcherGone)
                    }
                }
            }
        })
        .await;

        match result {
            Ok(inner) => inner,
            Err(_) => Err(CdpClientError::Timeout(format!(
                "Timeout waiting for event {method}"
            ))),
        }
    }

    pub async fn enable(&self, domain: &'static str) -> Result<(), CdpClientError> {
        let method = match domain {
            "Page" => "Page.enable",
            "Runtime" => "Runtime.enable",
            "DOM" => "DOM.enable",
            "Network" => "Network.enable",
            "Target" => "Target.setDiscoverTargets",
            _ => {
                return Err(CdpClientError::Protocol {
                    code: -1,
                    message: format!("Unknown domain: {domain}"),
                })
            }
        };

        if domain == "Target" {
            self.send(method, serde_json::json!({"discover": true}))
                .await
        } else {
            self.send(method, serde_json::json!({})).await
        }
    }
}

impl Drop for CdpClient {
    fn drop(&mut self) {
        self._dispatcher.abort();
    }
}

/// Reads from the transport, routes responses to pending calls and events to subscribers.
async fn dispatch_loop(
    mut receiver: transport::CdpReceiver,
    pending: PendingMap,
    events_tx: broadcast::Sender<CdpEvent>,
) {
    loop {
        let Ok(Some(message)) = receiver.recv().await else {
            break;
        };
        route_message(&message, &pending, &events_tx).await;
    }

    // Transport closed — clear pending so callers get RecvError.
    pending.lock().await.clear();
}

/// Route one raw message from Chrome: to the call waiting on its `id`, to the event
/// subscribers, or — when it fits neither shape — to [`resolve_unreadable`].
///
/// Separate from [`dispatch_loop`] so the routing decision is testable without a WebSocket.
async fn route_message(
    message: &str,
    pending: &PendingMap,
    events_tx: &broadcast::Sender<CdpEvent>,
) {
    let parsed: CdpMessage = match serde_json::from_str(message) {
        Ok(m) => m,
        // Never dropped: if it carried an `id`, a call is waiting on it and would otherwise
        // wait out its whole deadline for an answer that already arrived.
        Err(_) => return resolve_unreadable(message, pending).await,
    };

    match parsed {
        CdpMessage::Response(response) => {
            if let Some(tx) = pending.lock().await.remove(&response.id) {
                let _ = tx.send(PendingReply::Read(response));
            }
        }
        CdpMessage::Event(event) => {
            let _ = events_tx.send(event);
        }
    }
}

/// Fail the call waiting on an unreadable message's `id`, rather than let it time out.
///
/// Reading the `id` out of a plain `Value` needs no field declarations, so the answer reaches
/// its caller even when the shape around it did not. No shape today's Chrome sends reaches
/// here; what this covers is a protocol change, which then costs one error naming the cause
/// instead of a timeout blaming the page.
async fn resolve_unreadable(message: &str, pending: &PendingMap) {
    if let Some((id, detail)) = unreadable_reply(message) {
        // Bound so the guard is released before the `eprintln!` below, which is I/O.
        let waiting = pending.lock().await.remove(&id);
        if let Some(tx) = waiting {
            let _ = tx.send(PendingReply::Unreadable(detail));
            return;
        }
    }
    // No `id` to route by, or no call waiting on it: nobody is owed an error, so this is the
    // one place a message really is dropped, and it says so on stderr (never stdout, so --json
    // stays clean). Keys only, never content — a CDP message routinely carries a page value
    // such as the card number a `fill` just wrote.
    eprintln!(
        "cdp: dropped a message chrome-agent could not read and no call was waiting for (keys: {})",
        top_level_keys(message).unwrap_or_else(|| "not an object".to_string())
    );
}

/// The `id` an unreadable message carries, plus what can be said about its shape.
///
/// The second parse exists because `untagged` only ever says `data did not match any variant
/// of untagged enum CdpMessage`, while re-parsing as a response gives serde's real complaint:
/// type mismatch and byte offset, though not the field name. The key list closes that gap.
/// Keys, never values — see `resolve_unreadable`.
fn unreadable_reply(message: &str) -> Option<(u64, String)> {
    let id = serde_json::from_str::<Value>(message)
        .ok()?
        .get("id")?
        .as_u64()?;
    // `from_str`, not `from_value`: only the string parse keeps the position.
    let detail = match serde_json::from_str::<CdpResponse>(message) {
        Err(e) => e.to_string(),
        // Reachable only if the two parses disagree, which they should not.
        Ok(_) => "the message fitted neither CdpMessage variant".to_string(),
    };
    let keys = top_level_keys(message).unwrap_or_else(|| "unknown".to_string());
    Some((id, format!("{detail}; message keys: {keys}")))
}

/// The top-level key names of a JSON object, or `None` when it is not one. Names only — see
/// `resolve_unreadable` for why no value ever reaches a diagnostic.
fn top_level_keys(message: &str) -> Option<String> {
    let value: Value = serde_json::from_str(message).ok()?;
    let keys: Vec<&str> = value.as_object()?.keys().map(String::as_str).collect();
    Some(keys.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// The two timeouts do not share a sentence: raise the budget, versus the action may
    /// already have happened.
    #[test]
    fn an_input_that_went_unacknowledged_never_reads_as_a_slow_page() {
        let input = timeout_message("Input.dispatchMouseEvent", INPUT_ACK_DEADLINE);
        assert!(input.starts_with("Input.dispatchMouseEvent was dispatched"), "{input}");
        assert!(input.contains("may already have reached the page"), "{input}");
        assert!(!input.contains("--timeout"), "raising the budget is not the recovery: {input}");
        assert!(input.contains("8s"), "the deadline it actually waited: {input}");

        let evaluate = timeout_message("Runtime.evaluate", DEFAULT_CALL_TIMEOUT);
        assert!(evaluate.contains("--timeout"), "{evaluate}");
        assert!(!evaluate.contains("dispatched"), "{evaluate}");
    }

    /// The deadline must sit above the one stall Chrome is known to produce, or a slow but
    /// delivered click becomes an error.
    #[test]
    fn the_input_deadline_clears_the_background_tab_stall_and_undercuts_the_default() {
        assert!(INPUT_ACK_DEADLINE > Duration::from_secs(5), "the measured stall is 5.00 s");
        assert!(INPUT_ACK_DEADLINE < DEFAULT_CALL_TIMEOUT);
    }

    /// A message with a known `id` and an optional field of an unexpected type resolves the
    /// pending call at once, with an error naming what could not be read. The vector is
    /// `error.code`, the only remaining non-`Value` optional field; today's Chrome cannot send
    /// it, so the test is of the class, any field typed more narrowly than CDP guarantees.
    #[tokio::test]
    async fn an_unreadable_answer_fails_its_call_instead_of_timing_out() {
        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = oneshot::channel();
        pending.lock().await.insert(7, tx);
        let (events_tx, _events_rx) = broadcast::channel::<CdpEvent>(16);

        route_message(r#"{"id":7,"error":{"code":"-32000","message":"x"}}"#, &pending, &events_tx)
            .await;

        // Under a second: nothing here may wait on a deadline.
        let reply = tokio::time::timeout(Duration::from_secs(1), rx)
            .await
            .expect("the pending call must be resolved, not left to expire")
            .expect("the dispatcher must answer rather than drop the sender");
        let PendingReply::Unreadable(detail) = reply else {
            panic!("a message that fits neither variant is not a readable response");
        };
        assert!(
            detail.contains("invalid type: string \"-32000\", expected i64"),
            "the error must say what could not be read: {detail}"
        );
        assert!(
            detail.contains("message keys: error, id"),
            "and where to look for it, since serde does not name the field: {detail}"
        );
        assert!(
            pending.lock().await.is_empty(),
            "the slot must be dropped, or a late answer is delivered to nobody"
        );

        // The sentence the caller sees says Chrome answered, not that the page was slow.
        let message = unreadable_message("Target.getTargets", &detail);
        assert!(message.contains("Target.getTargets"), "{message}");
        assert!(message.contains("Chrome replied; the page is not slow"), "{message}");
        assert!(
            !message.contains("timeout") && !message.contains("--timeout"),
            "the old failure blamed the caller's patience: {message}"
        );
        assert!(
            message.starts_with("response parse"),
            "hints::error_hint keys the recovery off this prefix: {message}"
        );
    }

    /// An unreadable acknowledgement still means the event was dispatched: the sentence must
    /// not read as "nothing occurred".
    #[test]
    fn an_unreadable_input_ack_never_reads_as_an_event_that_did_not_land() {
        let input = unreadable_message("Input.dispatchMouseEvent", "invalid type: map");
        assert!(input.contains("was dispatched"), "{input}");
        assert!(input.contains("may already have reached the page"), "{input}");
        assert!(!input.contains("produced nothing"), "{input}");

        let other = unreadable_message("Runtime.evaluate", "invalid type: map");
        assert!(other.contains("produced nothing"), "{other}");
        assert!(!other.contains("was dispatched"), "{other}");
    }

    /// The `response parse` prefix is a claim about `hints::error_hint`'s branch order, so it
    /// is checked here: a reply that arrived at once must never reach the generic timeout
    /// branch ("Use --timeout N for slow pages").
    #[test]
    fn an_unreadable_answer_gets_the_parse_hint_and_never_the_slow_page_one() {
        for method in ["Runtime.evaluate", "Input.dispatchMouseEvent"] {
            let message = unreadable_message(method, "invalid type: map, expected a string");
            let hint = crate::hints::error_hint(&message, "b1")
                .unwrap_or_else(|| panic!("every error carries a hint: {message}"));
            assert!(
                hint.contains("could not read the answer"),
                "{method} must reach the parse branch, got: {hint}"
            );
            assert!(
                !hint.contains("Use --timeout N"),
                "{method} must never be reported as a slow page: {hint}"
            );
        }
    }

    /// The diagnostic quotes Chrome's key names and never a page value such as one a `fill`
    /// just wrote.
    #[test]
    fn the_diagnostic_carries_key_names_and_no_value() {
        let raw = r#"{"id":9,"sessionId":42,"result":{"value":"4111111111111111"}}"#;
        let (id, detail) = unreadable_reply(raw).expect("an id is there to route by");
        assert_eq!(id, 9);
        assert!(
            !detail.contains("4111111111111111"),
            "a page value must not reach a diagnostic: {detail}"
        );
        // Sorted, not in wire order: serde_json's map is a BTreeMap here.
        assert_eq!(top_level_keys(raw).unwrap(), "id, result, sessionId");
        assert_eq!(top_level_keys("[1,2]"), None);
    }

    /// With no `id` there is no call to fail: routing must not invent one, nor panic on a
    /// message that is not JSON.
    #[tokio::test]
    async fn an_unreadable_message_with_no_id_fails_nothing() {
        assert_eq!(unreadable_reply(r#"{"method":"Page.loadEventFired"}"#), None);
        assert_eq!(unreadable_reply("not json at all"), None);

        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = oneshot::channel();
        pending.lock().await.insert(7, tx);
        let (events_tx, _events_rx) = broadcast::channel::<CdpEvent>(16);

        route_message("not json at all", &pending, &events_tx).await;

        assert_eq!(pending.lock().await.len(), 1, "an unrelated call must be untouched");
        drop(rx);
    }

    /// A readable response still reaches its caller and a readable event its subscribers.
    #[tokio::test]
    async fn a_readable_message_still_routes_where_it_did() {
        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = oneshot::channel();
        pending.lock().await.insert(7, tx);
        let (events_tx, mut events_rx) = broadcast::channel::<CdpEvent>(16);

        route_message(r#"{"id":7,"result":{"ok":true}}"#, &pending, &events_tx).await;
        let PendingReply::Read(response) = rx.await.expect("the response must be delivered") else {
            panic!("a well-formed response is readable");
        };
        assert_eq!(response.id, 7);

        route_message(r#"{"method":"Page.loadEventFired","params":{}}"#, &pending, &events_tx)
            .await;
        assert_eq!(events_rx.recv().await.unwrap().method, "Page.loadEventFired");
    }

    /// A response whose `error.data` is an object is an ordinary protocol error: typed
    /// `Option<String>` it fell out of both variants, and it is now undeclared.
    #[tokio::test]
    async fn a_structured_error_detail_is_a_protocol_error_and_not_a_lost_message() {
        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = oneshot::channel();
        pending.lock().await.insert(3, tx);
        let (events_tx, _events_rx) = broadcast::channel::<CdpEvent>(16);

        route_message(
            r#"{"id":3,"error":{"code":-32000,"message":"Cannot find context","data":{"context":9}}}"#,
            &pending,
            &events_tx,
        )
        .await;

        let PendingReply::Read(response) = rx.await.expect("delivered") else {
            panic!("a structured `data` must not take the message out of CdpResponse");
        };
        let error = response.error.expect("the error is still there");
        assert_eq!(error.code, -32000);
        assert_eq!(error.message, "Cannot find context");
    }

    fn event(method: &str) -> CdpEvent {
        CdpEvent {
            method: method.to_string(),
            params: Value::Null,
        }
    }

    // An event fired after subscribing but before the wait starts must still be observed.
    #[tokio::test]
    async fn wait_for_event_on_sees_event_buffered_before_wait() {
        let (tx, _) = broadcast::channel::<CdpEvent>(16);
        let mut rx = tx.subscribe();
        // Event arrives before we begin waiting; the receiver buffers it.
        tx.send(event("Page.loadEventFired")).unwrap();

        let got = CdpClient::wait_for_event_on(
            &mut rx,
            "Page.loadEventFired",
            Duration::from_secs(5),
        )
        .await
        .expect("buffered event should be returned without timing out");
        assert_eq!(got.method, "Page.loadEventFired");
    }

    // Contrast: subscribing after the event was sent misses it and hits the timeout.
    #[tokio::test]
    async fn wait_for_event_on_misses_event_sent_before_subscribe() {
        // Keep the initial receiver alive so `send` has a subscriber; `rx` subscribes after.
        let (tx, _keep_alive) = broadcast::channel::<CdpEvent>(16);
        tx.send(event("Page.loadEventFired")).unwrap();
        let mut rx = tx.subscribe(); // too late — event is gone for this receiver

        let err = CdpClient::wait_for_event_on(
            &mut rx,
            "Page.loadEventFired",
            Duration::from_millis(50),
        )
        .await
        .expect_err("late subscriber must miss the event and time out");
        assert!(matches!(err, CdpClientError::Timeout(_)));
    }

    // Unrelated events are skipped; the target still resolves.
    #[tokio::test]
    async fn wait_for_event_on_skips_other_events() {
        let (tx, _) = broadcast::channel::<CdpEvent>(16);
        let mut rx = tx.subscribe();
        tx.send(event("Page.frameNavigated")).unwrap();
        tx.send(event("Page.loadEventFired")).unwrap();

        let got = CdpClient::wait_for_event_on(
            &mut rx,
            "Page.loadEventFired",
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!(got.method, "Page.loadEventFired");
    }
}
