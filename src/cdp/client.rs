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
}

#[derive(Debug)]
pub enum CdpClientError {
    Transport(CdpTransportError),
    Serialization(serde_json::Error),
    Protocol { code: i64, message: String },
    ResponseParse(serde_json::Error),
    DispatcherGone,
}

impl std::fmt::Display for CdpClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(e) => write!(f, "transport: {e}"),
            Self::Serialization(e) => write!(f, "serialization: {e}"),
            Self::Protocol { code, message } => write!(f, "CDP error {code}: {message}"),
            Self::ResponseParse(e) => write!(f, "response parse: {e}"),
            Self::DispatcherGone => write!(f, "dispatcher task exited"),
        }
    }
}

impl std::error::Error for CdpClientError {}

impl From<CdpTransportError> for CdpClientError {
    fn from(e: CdpTransportError) -> Self {
        Self::Transport(e)
    }
}

impl CdpClient {
    /// Connect to a Chrome DevTools Protocol endpoint.
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
        })
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

        let result_value = response.result.unwrap_or(Value::Null);
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
                    Ok(_) => continue,
                    Err(_) => return Err(CdpClientError::DispatcherGone),
                }
            }
        })
        .await;

        match result {
            Ok(inner) => inner,
            Err(_) => Err(CdpClientError::Protocol {
                code: -1,
                message: format!("Timeout waiting for event {method}"),
            }),
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
        let message = match receiver.recv().await {
            Ok(Some(text)) => text,
            Ok(None) | Err(_) => break,
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
