//! Remove a pending request when an enclosing operation cancels its CDP read.

use super::PendingMap;

pub(super) struct PendingCall<'a> {
    pending: &'a PendingMap,
    id: u64,
}

impl<'a> PendingCall<'a> {
    pub const fn new(pending: &'a PendingMap, id: u64) -> Self {
        Self { pending, id }
    }
}

impl Drop for PendingCall<'_> {
    fn drop(&mut self) {
        if let Ok(mut pending) = self.pending.try_lock() {
            pending.remove(&self.id);
        } else {
            // The dispatcher holds the map only for synchronous insert/remove operations.
            // A contended drop cannot await its lock; finish cleanup on the same runtime.
            let pending = self.pending.clone();
            let id = self.id;
            tokio::spawn(async move { pending.lock().await.remove(&id) });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use futures_util::StreamExt as _;
    use serde_json::{Value, json};
    use tokio::sync::{Mutex, oneshot};

    use super::*;

    #[tokio::test]
    async fn cancelling_a_call_removes_its_pending_response_slot() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (seen, received) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            socket.next().await.unwrap().unwrap();
            seen.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        let client = super::super::CdpClient::connect(&format!("ws://{address}"))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            tokio::select! {
                result = client.call::<_, Value>("Runtime.evaluate", json!({"expression":"1"})) => panic!("unexpected reply: {result:?}"),
                result = received => result.unwrap(),
            }
        }).await.unwrap();
        assert!(client.pending.lock().await.is_empty());
        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn cleanup_also_runs_when_the_dispatcher_holds_the_map_lock() {
        let pending: PendingMap = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let (tx, _rx) = oneshot::channel();
        let mut map = pending.lock().await;
        map.insert(1, tx);
        drop(PendingCall::new(&pending, 1));
        drop(map);
        tokio::time::timeout(Duration::from_secs(1), async {
            while !pending.lock().await.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
}
