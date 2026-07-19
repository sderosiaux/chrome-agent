use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::{broadcast, oneshot, Mutex};

use super::transport::{self, CdpSender, CdpTransportError};
use super::types::{CdpEvent, CdpMessage, CdpRequest, CdpResponse};

type PendingMap = Arc<Mutex<HashMap<u64, oneshot::Sender<CdpResponse>>>>;

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
        })
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

        let response = rx.await.map_err(|_| CdpClientError::DispatcherGone)?;

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
            // High offset so our fire-and-forget ids never collide with the
            // sequential request ids (unmatched responses are dropped harmlessly).
            let mut local_id: u64 = 1 << 40;
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
                        local_id = local_id.wrapping_add(1);
                        let _ = sender.send(request.to_string()).await;
                        eprintln!(
                            "dialog auto-{}: {dtype} {message:?}",
                            if decision.accept { "accepted" } else { "dismissed" }
                        );
                    }
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
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

        let parsed: CdpMessage = match serde_json::from_str(&message) {
            Ok(m) => m,
            Err(_) => continue,
        };

        match parsed {
            CdpMessage::Response(response) => {
                if let Some(tx) = pending.lock().await.remove(&response.id) {
                    let _ = tx.send(response);
                }
            }
            CdpMessage::Event(event) => {
                let _ = events_tx.send(event);
            }
        }
    }

    // Transport closed — clear pending so callers get RecvError.
    pending.lock().await.clear();
}
