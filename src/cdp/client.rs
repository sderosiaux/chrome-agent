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
///
/// It used to hand back a `CdpResponse` and nothing else, which left the dispatcher with exactly
/// one way to report a message it could not read: say nothing, and let the caller's deadline
/// expire. Two outcomes, so two variants.
enum PendingReply {
    /// Chrome answered and the answer parsed.
    Read(CdpResponse),
    /// A message carrying this `id` arrived and could not be read. Carries serde's own
    /// complaint — the type mismatch and its position — plus the message's key names.
    Unreadable(String),
}

/// Deadline applied to a CDP response when the caller sets none. Matches the CLI's
/// `--timeout` default, which is the number a caller reaches for when asked how long they
/// are willing to wait.
const DEFAULT_CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Deadline for an input event's acknowledgement, whatever `--timeout` says.
///
/// An input event is not a computation the page might legitimately take half a minute over:
/// Chrome acknowledges one in single-digit milliseconds when the pipeline is healthy. The one
/// measured exception is a page that is not the active tab, where `Input.dispatchMouseEvent`
/// answers after a fixed 5.00 s — 5007, 5004, 5023 ms across runs — so a deadline at or below
/// five seconds would turn a slow-but-delivered click into an error. Eight seconds sits above
/// that and far below the 30 s default, which is the difference between an agent that learns
/// something is wrong and one that stares at a silent terminal for half a minute.
///
/// This is a deadline on the ANSWER, never on the event: see `element::input_timeout`, which
/// is why the failure it produces forbids the retry instead of inviting it.
const INPUT_ACK_DEADLINE: std::time::Duration = std::time::Duration::from_secs(8);
const DIALOG_REQUEST_ID_START: u64 = 1_000_000_000;
const DIALOG_REQUEST_ID_MAX: u64 = i32::MAX as u64;

/// What a call that ran out of time says about itself.
///
/// Two failures, two sentences, and the difference is not cosmetic: only one of them may be
/// repeated safely. A call that computes something may legitimately outlast the caller's
/// patience, and raising `--timeout` is the answer. An input event is not a computation — what
/// expired is the ACKNOWLEDGEMENT, and the event itself may already be in the page, so the
/// sentence says "dispatched" first and never invites a second attempt. `hints::error_hint`
/// keys the recovery off this wording.
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

/// What a call says when Chrome answered and chrome-agent could not read the answer.
///
/// This is the sentence that used to be [`timeout_message`]'s. A message that failed to
/// deserialize was dropped by the dispatcher, the caller's oneshot was never resolved, and
/// thirty seconds later the command reported "did not answer within 30s … raise --timeout if
/// the page is merely slow" — about a reply that had arrived immediately. Two wrong claims in
/// one line: that nothing answered, and that the page is the thing to be patient with. On the
/// binary whose `hints.rs` and [`INPUT_ACK_DEADLINE`] exist so that a slow-page diagnosis can be
/// trusted, that is the failure those two were built to prevent, arriving by a route neither
/// could see.
///
/// The prefix `response parse` is load-bearing exactly as the `dispatched` wording is above:
/// `hints::error_hint` keys its recovery off it, and the recovery is the right one here — a
/// Chrome much newer or older than the bundled Chromium is what a shape this tool cannot read
/// looks like.
///
/// Split on `Input.` for the same reason [`timeout_message`] is, and it is not symmetry for its
/// own sake: an input event whose acknowledgement could not be READ was still dispatched and
/// still acknowledged, so the page may have acted on it, and the sentence must not invite a
/// second click. Every other method produced no result this tool could use, so the caller can
/// safely act as if the command had not run.
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

/// Execution context bound to a specific frame by the `frame` command.
///
/// Once set, subsequent `eval` calls run in `context_id` (the frame's
/// isolated world) and `inspect`/snapshot scope to `frame_id`. Navigating
/// the top document invalidates the isolated world, so callers clear this
/// on navigation to avoid sending a dead `contextId`.
#[derive(Clone, Debug)]
pub struct FrameContext {
    /// `Page.FrameId` of the target frame.
    pub frame_id: String,
    /// `Runtime.ExecutionContextId` of the frame's isolated world.
    pub context_id: i64,
}

/// High-level CDP client. Handles request/response correlation and event dispatch.
///
/// Built on top of the split transport (`CdpSender` + `CdpReceiver`).
/// Spawns a dispatcher task that routes incoming messages to either
/// pending request futures or broadcast event subscribers.
pub struct CdpClient {
    sender: CdpSender,
    next_id: AtomicU64,
    pending: PendingMap,
    events_tx: broadcast::Sender<CdpEvent>,
    _dispatcher: tokio::task::JoinHandle<()>,
    /// Frame the `frame` command switched into, if any. Interior-mutable so
    /// `eval`/`inspect` (which take `&self`) can read it without threading
    /// state through every call site.
    frame_ctx: std::sync::Mutex<Option<FrameContext>>,
    /// How long to wait for a response before giving up on it.
    ///
    /// Every `call` used to await its response channel with no deadline. Chrome answers
    /// promptly, but an evaluation sent with `awaitPromise` only answers when the page's
    /// promise settles — and a promise that never settles left the command hanging with no
    /// error, no output and no recovery, in pipe mode for the rest of the session. Nothing
    /// was broken enough to notice: the socket stayed open and the dispatcher kept running.
    call_timeout: std::sync::Mutex<std::time::Duration>,
    /// When the last input event went out on this connection.
    ///
    /// `no_effect` is only ever a claim about a window — "the page did not move for N ms
    /// after the event" — and the two ends of that window live in different modules: the
    /// dispatch is in `element`, the observation in the verdict wiring. Recording it here
    /// keeps the number measured rather than assumed, without threading an `Instant` through
    /// every dispatcher signature in all three modes.
    last_dispatch: std::sync::Mutex<Option<std::time::Instant>>,
    /// Whether this connection has already asked its page to come to the foreground.
    ///
    /// `Page.bringToFront` costs 3 ms and is idempotent, but it is a state change on the
    /// browser, so it is made once per connection and only by a path that dispatches pointer
    /// input — see `ensure_foreground`.
    foregrounded: AtomicBool,
    /// How long this action spent waiting for a page load it had reason to expect.
    ///
    /// Recorded here for the same reason `last_dispatch` is: the wait happens in `element`,
    /// the response is assembled in the verdict wiring, and threading a `Duration` through
    /// every dispatcher signature in all three modes to carry one number is worse than one
    /// interior-mutable slot on the connection they all share.
    settle_wait: std::sync::Mutex<Option<std::time::Duration>>,
    /// Whether chrome-agent should synthesize taps instead of mouse clicks for this target.
    ///
    /// Device emulation is reapplied when each connection opens, so this connection-local flag
    /// follows the target's persisted `--touch` setting without leaking it into sibling pages.
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
    /// A message carrying this call's `id` arrived and did not fit `CdpMessage`.
    ///
    /// Distinct from [`Self::ResponseParse`], which is the `result` field not fitting the `R` a
    /// call site asked for — a mismatch between this tool and one command. This one is the
    /// ENVELOPE not fitting, which is a mismatch between this tool and the protocol, and the two
    /// have different fixes. They share a `response parse` prefix so `hints::error_hint` gives
    /// them the same recovery, which is genuinely the same: find out what Chrome this is.
    /// Built by [`unreadable_message`], never formatted at a call site.
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

    /// Record that an input event has just gone out.
    pub fn mark_dispatch(&self) {
        if let Ok(mut slot) = self.last_dispatch.lock() {
            *slot = Some(std::time::Instant::now());
        }
        // A pipe session reuses one connection for every command, so a wait recorded by the
        // click that navigated would otherwise still be on the response of the next command,
        // which waited for nothing. Cleared where the action starts, taken where it is read.
        if let Ok(mut slot) = self.settle_wait.lock() {
            *slot = None;
        }
    }

    /// Bring this connection's page to the foreground, once.
    ///
    /// Measured, on a page that is not the active tab (`document.visibilityState === "hidden"`):
    /// `Input.dispatchMouseEvent` answers after 5007, 5004 and 5023 ms, while `Runtime.evaluate`
    /// on the same connection answers in 0–1 ms — so the renderer's main thread is not busy, the
    /// input pipeline is waiting for something a backgrounded page never produces, and Chrome
    /// gives up on a fixed five-second timer. `Page.bringToFront` costs 3 ms and takes the same
    /// events to 0–6 ms. A page becomes hidden without anyone asking: opening a second page
    /// backgrounds the first, and Chrome's own `chrome://settings/help` update check did it to a
    /// browser this tool had launched.
    ///
    /// Only the pointer paths call this, and the restraint is measured too: `Input.dispatchKeyEvent`
    /// answers in 1 ms on the same hidden page, so `press` and `type` have nothing to gain and
    /// would be paying a state change for it.
    ///
    /// Consequence, stated rather than hidden: with several pages open in one browser, clicking
    /// on one foregrounds it — which is what clicking means, and what `emulation` already does
    /// for the same class of reason. Best effort: a target that refuses to come forward is not a
    /// reason to refuse the click, it only costs the latency this exists to remove.
    pub async fn ensure_foreground(&self) {
        if self.foregrounded.swap(true, Ordering::Relaxed) {
            return;
        }
        // Boxed: this runs inside every pointer path, and those futures are held alive inside
        // `run::run`'s match arm. Inlining another `call` state machine four times over pushed
        // that frame past clippy's ceiling — a pin costs one allocation on a path that already
        // makes a round trip.
        let call = Box::pin(self.call::<_, Value>("Page.bringToFront", serde_json::json!({})));
        let _: Result<Value, _> = call.await;
    }

    /// Record how long an action waited for a page load after dispatching.
    pub fn note_settle_wait(&self, waited: std::time::Duration) {
        if let Ok(mut slot) = self.settle_wait.lock() {
            *slot = Some(waited);
        }
    }

    /// How long this action waited for a load, when it waited at all.
    ///
    /// Takes rather than reads: the number belongs to one action's response, and a connection
    /// that outlives the action (pipe, batch) must not hand it to the next one.
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

    /// Return the frame context set by the `frame` command, if any.
    pub fn frame_context(&self) -> Option<FrameContext> {
        self.frame_ctx.lock().unwrap().clone()
    }

    /// Bind (`Some`) or clear (`None`) the current frame context. Setting it
    /// scopes subsequent `eval`/`inspect` to that frame; clearing restores the
    /// top document. Navigation clears it (the isolated world dies with it).
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

    /// Send a CDP command on a specific session.
    pub async fn call_with_session<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &'static str,
        params: P,
        session_id: Option<String>,
    ) -> Result<R, CdpClientError> {
        self.call_within(method, params, session_id, self.call_timeout()).await
    }

    /// Dispatch an input event and wait for Chrome to acknowledge it, under
    /// [`INPUT_ACK_DEADLINE`] rather than `--timeout`.
    ///
    /// The distinction is not tidiness. `--timeout` is the caller's patience for the page's
    /// own work — a slow load, an evaluation that awaits a promise — and an input event is
    /// none of that: the acknowledgement comes from the browser's input pipeline, and when it
    /// does not come, waiting thirty seconds tells the caller nothing that eight does not.
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
                // The whole point of the second variant: this arrives as fast as the message
                // did, instead of as a deadline nobody set.
                PendingReply::Unreadable(detail) => {
                    return Err(CdpClientError::Unreadable(unreadable_message(
                        method, &detail,
                    )))
                }
            },
            Err(_) => {
                // Drop the slot: leaving it behind leaks one entry per timed-out call, and
                // a late answer would then be delivered to a receiver nobody awaits.
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

    /// How long a call waits for its response.
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

    /// Subscribe to CDP events. Returns a broadcast receiver.
    pub fn events(&self) -> broadcast::Receiver<CdpEvent> {
        self.events_tx.subscribe()
    }

    /// Install a background task that auto-answers JS dialogs
    /// (`alert`/`confirm`/`prompt`/`beforeunload`) per `policy`.
    ///
    /// A native dialog blocks the page with no DOM signal; without this the next
    /// command silently hangs. No-op for `DialogPolicy::Manual`. The Page domain
    /// must be enabled for `Page.javascriptDialogOpening` to fire. The task lives
    /// as long as the connection (it ends when the event channel closes).
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
            // Keep fire-and-forget ids inside Chromium's accepted signed
            // 32-bit range. Values such as 2^40 are silently ignored, leaving
            // the dialog open and the command that triggered it blocked.
            // This offset remains far above normal sequential request ids.
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
                        // A dropped Page.javascriptDialogOpening leaves the page
                        // blocked with no DOM signal. Surface it on stderr (never
                        // stdout, so --json stays clean) and keep handling.
                        eprintln!(
                            "dialog handler lagged: {n} event(s) dropped; a dialog may be unanswered"
                        );
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    /// Wait for a specific CDP event matching the given method name.
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
    /// Subscribe with [`Self::events`] *before* issuing the command that
    /// triggers the event, then wait here — this avoids the race where a fast
    /// (e.g. cached) response fires the event before a late subscription exists,
    /// which would otherwise stall until the timeout.
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

    /// Enable a CDP domain.
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

/// Dispatcher loop: reads from transport receiver, routes responses to pending
/// request futures, broadcasts events to subscribers.
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
/// Split out of [`dispatch_loop`] so the routing decision can be exercised without a WebSocket:
/// `CdpReceiver`'s fields are private to `transport`, so the loop itself cannot be fed a
/// message, and the third branch is precisely the one that had never been tested.
async fn route_message(
    message: &str,
    pending: &PendingMap,
    events_tx: &broadcast::Sender<CdpEvent>,
) {
    let parsed: CdpMessage = match serde_json::from_str(message) {
        Ok(m) => m,
        // Was `Err(_) => continue`, with no comment where the two branches around it had one.
        // A dropped message is not a message that never came: if it carried an `id`, a call is
        // waiting on it and will now wait out its whole deadline for an answer that already
        // arrived.
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
/// `CdpResponse` requires `id` and `CdpEvent` requires `method`, and every other field of both
/// is `#[serde(default)]`, so a message that fits neither is not a message missing a field — it
/// is one whose optional field arrived with a JSON type the struct did not declare. Reading the
/// `id` out of a plain `Value` needs none of those declarations, so the answer reaches its caller
/// even when the shape around it did not.
///
/// **Status, honestly: this has not been observed in the field.** The trigger was never measured
/// — it is reasoned from the struct definitions — and with `CdpError::data` now typed `Value`,
/// every remaining narrowly-typed field of the envelope (`id`, `code`, `message`, `method`,
/// `sessionId`) is one CDP does pin, so there is no shape today's Chrome is expected to send that
/// would reach this function at all. What is fixed is the CONSEQUENCE: whatever produces an unreadable message next —
/// a protocol change, a Chrome much newer than the bundled Chromium, a field we type too
/// narrowly — costs one error naming the cause instead of thirty seconds and a sentence blaming
/// the page. The `eprintln!` below is what would confirm the residual half on a real machine.
async fn resolve_unreadable(message: &str, pending: &PendingMap) {
    if let Some((id, detail)) = unreadable_reply(message) {
        // Bound so the guard is released before the `eprintln!` below, which is I/O.
        let waiting = pending.lock().await.remove(&id);
        if let Some(tx) = waiting {
            let _ = tx.send(PendingReply::Unreadable(detail));
            return;
        }
    }
    // Residual path: no `id` to route by, or no call waiting on it — an event we could not read,
    // or a late answer to a call that already gave up. Nobody is owed an error, so this is the
    // one place the message really is dropped, and it says so on stderr (never stdout, so --json
    // stays clean) instead of vanishing.
    //
    // Its KEYS and not its content, for the reason `snapshot_secret` exists: a CDP message is
    // routinely a page value, and the one thing that must never be traded for a diagnostic is
    // the card number a `fill` just wrote. Key names are protocol identifiers and belong to
    // Chrome, not to the page.
    eprintln!(
        "cdp: dropped a message chrome-agent could not read and no call was waiting for (keys: {})",
        top_level_keys(message).unwrap_or_else(|| "not an object".to_string())
    );
}

/// The `id` an unreadable message carries, and what can honestly be said about the shape.
///
/// Two parses on a path that only runs when one has already failed. The first reads the `id` out
/// of a `Value`, which no field declaration can block. The second exists because `untagged`
/// answers `data did not match any variant of untagged enum CdpMessage` — true, and naming
/// nothing at all; retrying the response shape ALONE produces serde's real complaint, which is
/// the whole diagnostic value of this fix and worth a second parse on a path taken approximately
/// never.
///
/// What that complaint gives, precisely, is the type mismatch and a byte offset — `invalid type:
/// integer 42, expected a string at line 1 column 22` — and NOT the field's name, which serde
/// does not carry without a dependency this crate will not take (the musl graph is guarded, and
/// a field name is not worth a crate). The key list is added beside it because it closes most of
/// the gap for free: the mismatch says what was wrong and the keys say where to look for it,
/// and both are Chrome's vocabulary rather than the page's — see `resolve_unreadable` on why
/// the message body itself is never quoted.
fn unreadable_reply(message: &str) -> Option<(u64, String)> {
    let id = serde_json::from_str::<Value>(message)
        .ok()?
        .get("id")?
        .as_u64()?;
    // Deliberately `from_str` and not `from_value`: only the string parse keeps the position,
    // and the position is the half of the diagnostic serde does give us.
    let detail = match serde_json::from_str::<CdpResponse>(message) {
        Err(e) => e.to_string(),
        // Reachable only if the two parses disagree, which they should not. Saying so beats
        // asserting a reason we do not have.
        Ok(_) => "the message fitted neither CdpMessage variant".to_string(),
    };
    let keys = top_level_keys(message).unwrap_or_else(|| "unknown".to_string());
    Some((id, format!("{detail}; message keys: {keys}")))
}

/// The top-level key names of a JSON object, or `None` when it is not one.
///
/// Names only. See `resolve_unreadable` for why no value ever appears in a diagnostic here.
fn top_level_keys(message: &str) -> Option<String> {
    let value: Value = serde_json::from_str(message).ok()?;
    let keys: Vec<&str> = value.as_object()?.keys().map(String::as_str).collect();
    Some(keys.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// The two timeouts do not share a sentence, because they do not share a recovery: one
    /// says raise the budget, the other says the action may already have happened.
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

    /// The deadline has to sit above the one stall Chrome is known to produce, or a click that
    /// works — slowly — becomes an error.
    #[test]
    fn the_input_deadline_clears_the_background_tab_stall_and_undercuts_the_default() {
        assert!(INPUT_ACK_DEADLINE > Duration::from_secs(5), "the measured stall is 5.00 s");
        assert!(INPUT_ACK_DEADLINE < DEFAULT_CALL_TIMEOUT);
    }

    /// The fix, end to end on the routing decision: a message carrying a known `id` and an
    /// optional field of an unexpected type resolves the pending call at once, with an error
    /// naming what could not be read — where it used to resolve nothing at all and leave the
    /// caller to its deadline.
    ///
    /// `sessionId` is the vector because it is now the only optional field on an incoming
    /// message that is not a `Value` (`CdpError::data` was the other, and typing it `Value` is
    /// half of this fix). Today's Chrome cannot send this: the test is of the CLASS, which is
    /// any field we type more narrowly than the protocol guarantees, and the class is the part
    /// that survives a protocol change.
    #[tokio::test]
    async fn an_unreadable_answer_fails_its_call_instead_of_timing_out() {
        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = oneshot::channel();
        pending.lock().await.insert(7, tx);
        let (events_tx, _events_rx) = broadcast::channel::<CdpEvent>(16);

        route_message(r#"{"id":7,"sessionId":42}"#, &pending, &events_tx).await;

        // Under a second, and by a wide margin: the point is that nothing waits on a deadline.
        let reply = tokio::time::timeout(Duration::from_secs(1), rx)
            .await
            .expect("the pending call must be resolved, not left to expire")
            .expect("the dispatcher must answer rather than drop the sender");
        let PendingReply::Unreadable(detail) = reply else {
            panic!("a message that fits neither variant is not a readable response");
        };
        assert!(
            detail.contains("invalid type: integer `42`, expected a string"),
            "the error must say what could not be read: {detail}"
        );
        assert!(
            detail.contains("message keys: id, sessionId"),
            "and where to look for it, since serde does not name the field: {detail}"
        );
        assert!(
            pending.lock().await.is_empty(),
            "the slot must be dropped, or a late answer is delivered to nobody"
        );

        // And the sentence the caller sees says Chrome answered — not that the page was slow.
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

    /// An unreadable ACKNOWLEDGEMENT of an input event is not an input event that did not
    /// happen: it was dispatched and Chrome answered, so the sentence must not read as "nothing
    /// occurred" and must not invite a second click. Same split as `timeout_message`'s, for the
    /// same reason.
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

    /// The `response parse` prefix is a claim about `hints::error_hint`'s branch order, so it is
    /// checked rather than asserted in prose. What must NOT happen is the generic timeout branch
    /// ("Use --timeout N for slow pages"), which is the advice this whole fix exists to stop
    /// giving about a reply that arrived at once.
    ///
    /// The positive marker is a FRAGMENT of another module's prose, which is the drift this repo
    /// spent a whole pass removing elsewhere — so it is deliberately the shortest phrase that
    /// identifies the branch and nothing about its advice. It moved once already: the branch used
    /// to say "could not parse" and to claim the command never ran, which is false for an
    /// `Input.*` whose ack was unreadable — the event was dispatched and Chrome answered, and only
    /// our reading of the receipt failed. That is now fixed in `hints`, and this marker follows it.
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

    /// The diagnostic quotes Chrome's vocabulary and never the page's. A CDP message routinely
    /// carries a value a `fill` just wrote, and this project redacts those from stdout, the
    /// transcript and any `--record` file; an error path is not an exemption.
    #[test]
    fn the_diagnostic_carries_key_names_and_no_value() {
        let raw = r#"{"id":9,"sessionId":42,"result":{"value":"4111111111111111"}}"#;
        let (id, detail) = unreadable_reply(raw).expect("an id is there to route by");
        assert_eq!(id, 9);
        assert!(
            !detail.contains("4111111111111111"),
            "a page value must not reach a diagnostic: {detail}"
        );
        // Sorted, not in wire order: serde_json's map is a BTreeMap here, which makes the list
        // the same for the same shape however Chrome laid it out.
        assert_eq!(top_level_keys(raw).unwrap(), "id, result, sessionId");
        assert_eq!(top_level_keys("[1,2]"), None);
    }

    /// A message with no `id` has no call waiting on it, so there is nothing to fail — and the
    /// routing must not invent one, nor panic on a message that is not JSON at all.
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

    /// A readable response still reaches its caller, and a readable event still reaches the
    /// subscribers — the two branches the third one sits beside.
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

    /// `CdpError::data` is the instance that motivated the class. A response whose `data` is an
    /// object used to take the whole message out of both variants; it is now an ordinary
    /// protocol error, delivered as one.
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
            session_id: None,
        }
    }

    // A10b: an event fired AFTER we subscribe but BEFORE we start waiting must
    // still be observed. This is the exact race goto.rs hits on cached loads —
    // subscribing before navigate keeps the (buffered) event, so the wait
    // returns immediately instead of stalling until timeout.
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

    // Contrast: subscribing AFTER the event was sent (the pre-fix ordering)
    // misses it and hits the timeout — proving why the subscription must
    // happen before the triggering command.
    #[tokio::test]
    async fn wait_for_event_on_misses_event_sent_before_subscribe() {
        // Keep the initial receiver alive so `send` has a subscriber and succeeds;
        // our `rx` subscribes afterwards and therefore never sees this event.
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
