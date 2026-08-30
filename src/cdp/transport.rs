use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

/// Channel buffer size for outbound and inbound WebSocket messages.
const CHANNEL_BUFFER: usize = 256;

/// The largest single CDP reply this connection will read, and therefore the largest amount of
/// memory one answer from Chrome can make this process allocate.
///
/// Set explicitly rather than left to tungstenite's defaults, which are 64 MiB per message and
/// **16 MiB per frame** — and Chrome sends a CDP reply unfragmented, so the frame limit was the
/// real ceiling and it sat four times below the one the code reasoned about. Both are set to
/// this value so there is one number.
///
/// 96 MiB is chosen so `download`'s advertised 64 MiB fits: base64 costs +33 %, so 64 MiB of
/// file is 85.3 MiB on the wire. `commands::download` derives its own ceiling back out of this
/// (`MAX_FETCH_BYTES`) and a `const` assertion there fails the build if the two ever disagree.
pub const MAX_MESSAGE_BYTES: usize = 96 << 20;

/// Sender half of the CDP transport. Clone-safe — can be shared across tasks.
#[derive(Debug, Clone)]
pub struct CdpSender {
    outbound_tx: mpsc::Sender<String>,
}

/// Receiver half of the CDP transport. Owned by a single consumer (the dispatcher).
#[derive(Debug)]
pub struct CdpReceiver {
    inbound_rx: mpsc::Receiver<String>,
    /// Set by [`io_loop`] when the read half died on the size limit rather than on a close.
    /// Without it the two are indistinguishable downstream and a reply that was too big to read
    /// is reported as a connection that ended — which says nothing about why.
    oversize: Arc<AtomicBool>,
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
/// What this connection will read off the wire. One value for both limits: a frame ceiling
/// below the message ceiling is the lower of the two and nothing reads it that way.
fn read_limits() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_MESSAGE_BYTES))
}

pub async fn connect(url: &str) -> Result<(CdpSender, CdpReceiver), CdpTransportError> {
    let (ws_stream, _response) =
        tokio_tungstenite::connect_async_with_config(url, Some(read_limits()), false)
            .await
            .map_err(|e| CdpTransportError::Connect(e.to_string()))?;

    let (ws_write, ws_read) = ws_stream.split();

    let (outbound_tx, outbound_rx) = mpsc::channel::<String>(CHANNEL_BUFFER);
    let (inbound_tx, inbound_rx) = mpsc::channel::<String>(CHANNEL_BUFFER);

    let oversize = Arc::new(AtomicBool::new(false));
    let task = tokio::spawn(io_loop(
        ws_write,
        ws_read,
        outbound_rx,
        inbound_tx,
        Arc::clone(&oversize),
    ));

    Ok((
        CdpSender { outbound_tx },
        CdpReceiver {
            inbound_rx,
            oversize,
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
    /// Receive the next JSON text message from Chrome. `Ok(None)` once the WebSocket closed,
    /// and [`CdpTransportError::MessageTooLarge`] when what closed it was a reply past
    /// [`MAX_MESSAGE_BYTES`] — a bounded refusal, not an unexplained end of connection.
    pub async fn recv(&mut self) -> Result<Option<String>, CdpTransportError> {
        match self.inbound_rx.recv().await {
            Some(message) => Ok(Some(message)),
            None if self.oversize.load(Ordering::Relaxed) => {
                Err(CdpTransportError::MessageTooLarge {
                    limit: MAX_MESSAGE_BYTES,
                })
            }
            None => Ok(None),
        }
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
    oversize: Arc<AtomicBool>,
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
                    // A reply past the size limit: record WHY before ending the loop, or the
                    // dispatcher can only report that the connection stopped.
                    Some(Err(tokio_tungstenite::tungstenite::Error::Capacity(_))) => {
                        oversize.store(true, Ordering::Relaxed);
                        break;
                    }
                    // Clean close, stream end and every other transport error end the loop.
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
///
/// `Clone`, because one dead connection has to be reported to every call still waiting on it.
#[derive(Clone, Debug, thiserror::Error)]
pub enum CdpTransportError {
    /// Failed to establish the WebSocket connection.
    #[error("WebSocket connect failed: {0}")]
    Connect(String),
    /// The transport channel is closed (background task exited).
    #[error("transport closed")]
    Closed,
    /// Chrome answered with a message past [`MAX_MESSAGE_BYTES`]. The connection is over —
    /// tungstenite cannot resynchronise a stream it stopped reading mid-message — but the size
    /// is the fact, and it is a bound this tool set rather than anything the page did wrong.
    #[error(
        "a CDP message exceeded the {limit}-byte ceiling this connection reads, so it was not \
         read and the connection ended"
    )]
    MessageTooLarge { limit: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reply too big to read is a different fact from a connection that ended, and the two
    /// used to be one `Ok(None)`. Driven through the same channel `io_loop` writes.
    #[tokio::test]
    async fn a_message_past_the_ceiling_is_reported_as_a_size_and_not_as_a_close() {
        let (tx, inbound_rx) = mpsc::channel::<String>(1);
        let oversize = Arc::new(AtomicBool::new(false));
        let idle = tokio::spawn(async {});
        let mut receiver = CdpReceiver {
            inbound_rx,
            oversize: Arc::clone(&oversize),
            _shutdown: ShutdownHandle { task: idle },
        };

        // A plain close still reads as one.
        drop(tx);
        assert!(matches!(receiver.recv().await, Ok(None)));

        // The same closed channel, with the size flag set by the read half.
        oversize.store(true, Ordering::Relaxed);
        let error = receiver.recv().await.expect_err("a size, not a silence");
        assert!(matches!(error, CdpTransportError::MessageTooLarge { .. }));
        assert!(
            error.to_string().contains(&MAX_MESSAGE_BYTES.to_string()),
            "{error}"
        );
    }

    /// The frame limit used to sit at tungstenite's 16 MiB default while the code reasoned in
    /// terms of the 64 MiB message limit — and Chrome does not fragment a CDP reply, so the
    /// frame limit was the one that applied.
    #[test]
    fn the_frame_ceiling_is_not_lower_than_the_message_ceiling() {
        let config = read_limits();
        assert_eq!(config.max_frame_size, Some(MAX_MESSAGE_BYTES));
        assert_eq!(config.max_message_size, Some(MAX_MESSAGE_BYTES));
        assert!(
            WebSocketConfig::default().max_frame_size < Some(MAX_MESSAGE_BYTES),
            "the default frame limit was already the effective ceiling; this test proves nothing"
        );
    }

    #[tokio::test]
    async fn connect_to_invalid_url_returns_error() {
        let result = super::connect("ws://127.0.0.1:1/invalid").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, CdpTransportError::Connect(_)));
        assert!(err.to_string().contains("WebSocket connect failed"));
    }
}
