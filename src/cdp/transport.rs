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

/// Aborts the background I/O task on drop.
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
/// Spawns a background task bridging the WebSocket to mpsc channels; it exits when either
/// side is dropped or the WebSocket closes.
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
    /// Receive the next JSON text message from Chrome. `Ok(None)` once the WebSocket closed.
    pub async fn recv(&mut self) -> Result<Option<String>, CdpTransportError> {
        Ok(self.inbound_rx.recv().await)
    }
}

/// Background I/O loop bridging the WebSocket to mpsc channels: writer pulls from
/// `outbound_rx`, reader pushes WS text frames into `inbound_tx`.
///
/// Terminates when either channel closes or the WebSocket ends or errors.
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
            // Writer: caller → WebSocket.
            msg = outbound_rx.recv() => {
                if let Some(text) = msg {
                    if ws_write.send(Message::Text(text.into())).await.is_err() {
                        break;
                    }
                } else {
                    // Caller is done sending: close cleanly.
                    let _ = ws_write.send(Message::Close(None)).await;
                    break;
                }
            }

            // Reader: WebSocket → caller.
            frame = ws_read.next() => {
                match frame {
                    Some(Ok(Message::Text(text))) => {
                        if inbound_tx.send(text.to_string()).await.is_err() {
                            break;
                        }
                    }
                    // Clean close, stream end and transport error all end the loop.
                    Some(Ok(Message::Close(_)) | Err(_)) | None => {
                        break;
                    }
                    Some(Ok(Message::Ping(data))) => {
                        // Pong back to keep the connection alive.
                        if ws_write.send(Message::Pong(data)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(_)) => {
                        // Binary, Pong, Frame — ignore.
                    }
                }
            }
        }
    }

    // Best effort: we are shutting down either way.
    let _ = ws_write.close().await;
}

/// Errors produced by `CdpTransport`.
#[derive(Debug, thiserror::Error)]
pub enum CdpTransportError {
    /// Failed to establish the WebSocket connection.
    #[error("WebSocket connect failed: {0}")]
    Connect(String),
    /// The transport channel is closed (background task exited).
    #[error("transport closed")]
    Closed,
}

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
