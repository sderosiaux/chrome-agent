use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

/// Channel buffer size for outbound and inbound WebSocket messages.
const CHANNEL_BUFFER: usize = 256;

/// Sender half of the CDP transport. Clone-safe — can be shared across tasks.
#[derive(Debug, Clone)]
pub struct CdpSender {
    outbound_tx: mpsc::Sender<String>,
}

/// Receiver half of the CDP transport. Owned by a single consumer (the dispatcher).
#[derive(Debug)]
pub struct CdpReceiver {
    inbound_rx: mpsc::Receiver<String>,
    _shutdown: ShutdownHandle,
}

/// Holds the sender half of the outbound channel plus the join handle.
/// When this struct is dropped the outbound channel closes, which signals
/// the background writer loop to terminate. The reader loop terminates
/// when the WebSocket itself closes or errors.
#[derive(Debug)]
struct ShutdownHandle {
    task: tokio::task::JoinHandle<()>,
}

impl Drop for ShutdownHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Connect to a Chrome `DevTools` Protocol WebSocket endpoint.
///
/// Returns a split `(CdpSender, CdpReceiver)` pair. The sender is clone-safe
/// and can be shared across tasks. The receiver is owned by a single consumer
/// (typically the dispatcher loop in `CdpClient`).
///
/// Spawns a background tokio task that bridges the WebSocket to mpsc channels.
/// The task exits when either side is dropped or the WebSocket closes.
pub async fn connect(url: &str) -> Result<(CdpSender, CdpReceiver), CdpTransportError> {
    let (ws_stream, _response) = tokio_tungstenite::connect_async(url)
        .await
        .map_err(|e| CdpTransportError::Connect(e.to_string()))?;

    let (ws_write, ws_read) = ws_stream.split();

    let (outbound_tx, outbound_rx) = mpsc::channel::<String>(CHANNEL_BUFFER);
    let (inbound_tx, inbound_rx) = mpsc::channel::<String>(CHANNEL_BUFFER);

    let task = tokio::spawn(io_loop(ws_write, ws_read, outbound_rx, inbound_tx));

    Ok((
        CdpSender { outbound_tx },
        CdpReceiver {
            inbound_rx,
            _shutdown: ShutdownHandle { task },
        },
    ))
}

impl CdpSender {
    /// Send a JSON text message to Chrome.
    pub async fn send(&self, message: String) -> Result<(), CdpTransportError> {
        self.outbound_tx
            .send(message)
            .await
            .map_err(|_| CdpTransportError::Closed)
    }
}

impl CdpReceiver {
    /// Receive the next JSON text message from Chrome.
    ///
    /// Returns `Ok(None)` when the WebSocket has closed cleanly.
    pub async fn recv(&mut self) -> Result<Option<String>, CdpTransportError> {
        Ok(self.inbound_rx.recv().await)
    }
}

/// Background I/O loop that bridges the WebSocket to mpsc channels.
///
/// Runs two concurrent paths via `tokio::select!`:
/// - **Writer**: pulls from `outbound_rx`, serialises to WS text frames.
/// - **Reader**: pulls from the WS stream, deserialises text frames into
///   `inbound_tx`.
///
/// Terminates when:
/// - The outbound channel is closed (caller dropped or called `close`).
/// - The WebSocket stream ends or errors.
/// - The inbound channel is closed (caller dropped `inbound_rx`).
async fn io_loop<S, R>(
    mut ws_write: S,
    mut ws_read: R,
    mut outbound_rx: mpsc::Receiver<String>,
    inbound_tx: mpsc::Sender<String>,
) where
    S: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
    R: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        tokio::select! {
            // --- Writer path: caller → WebSocket ---
            msg = outbound_rx.recv() => {
                if let Some(text) = msg {
                    if ws_write.send(Message::Text(text)).await.is_err() {
                        // WebSocket write failed — connection dead.
                        break;
                    }
                } else {
                    // Outbound channel closed — caller is done sending.
                    // Send a clean WebSocket close frame.
                    let _ = ws_write.send(Message::Close(None)).await;
                    break;
                }
            }

            // --- Reader path: WebSocket → caller ---
            frame = ws_read.next() => {
                match frame {
                    Some(Ok(Message::Text(text))) => {
                        if inbound_tx.send(text).await.is_err() {
                            // Receiver dropped — nobody is reading.
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        // WebSocket closed by remote or stream exhausted.
                        break;
                    }
                    Some(Ok(Message::Ping(data))) => {
                        // Respond to pings to keep the connection alive.
                        if ws_write.send(Message::Pong(data)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(_)) => {
                        // Binary, Pong, Frame — ignore.
                    }
                    Some(Err(_)) => {
                        // WebSocket read error — connection dead.
                        break;
                    }
                }
            }
        }
    }

    // Best-effort close — ignore errors since we're shutting down anyway.
    let _ = ws_write.close().await;
}

/// Errors produced by `CdpTransport`.
#[derive(Debug)]
pub enum CdpTransportError {
    /// Failed to establish the WebSocket connection.
    Connect(String),
    /// The transport channel is closed (background task exited).
    Closed,
}

impl std::fmt::Display for CdpTransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connect(reason) => write!(f, "WebSocket connect failed: {reason}"),
            Self::Closed => write!(f, "transport closed"),
        }
    }
}

impl std::error::Error for CdpTransportError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn connect_to_invalid_url_returns_error() {
        let result = super::connect("ws://127.0.0.1:1/invalid").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, CdpTransportError::Connect(_)));
        assert!(err.to_string().contains("WebSocket connect failed"));
    }
}
