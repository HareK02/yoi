use super::*;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::sync::oneshot;

// The real bridge owns this sink exactly as it owns a TCP WebSocket sink. It
// accepts bytes but never finishes flushing: delivery is deliberately unknown.
struct StalledWire {
    entered: Option<oneshot::Sender<()>>,
    dropped: Option<oneshot::Sender<()>>,
    sent: std::sync::Arc<std::sync::Mutex<Vec<TungsteniteMessage>>>,
}
impl futures::Sink<TungsteniteMessage> for StalledWire {
    type Error = tokio_tungstenite::tungstenite::Error;
    fn poll_ready(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
    ) -> Poll<std::result::Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
    fn start_send(
        mut self: Pin<&mut Self>,
        message: TungsteniteMessage,
    ) -> std::result::Result<(), Self::Error> {
        self.sent.lock().unwrap().push(message);
        if let Some(entered) = self.entered.take() {
            let _ = entered.send(());
        }
        Ok(())
    }
    fn poll_flush(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
    ) -> Poll<std::result::Result<(), Self::Error>> {
        Poll::Pending
    }
    fn poll_close(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
    ) -> Poll<std::result::Result<(), Self::Error>> {
        Poll::Pending
    }
}
impl Drop for StalledWire {
    fn drop(&mut self) {
        if let Some(dropped) = self.dropped.take() {
            let _ = dropped.send(());
        }
    }
}
pub(super) fn stalled_remote_bridge() -> (
    WorkspaceWorkerProtocolConnection,
    oneshot::Receiver<()>,
    oneshot::Receiver<()>,
    std::sync::Arc<std::sync::Mutex<Vec<TungsteniteMessage>>>,
) {
    let (entered_tx, entered) = oneshot::channel();
    let (dropped_tx, dropped) = oneshot::channel();
    let sent = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = StalledWire {
        entered: Some(entered_tx),
        dropped: Some(dropped_tx),
        sent: sent.clone(),
    };
    let connection = remote_worker_protocol_bridge(sink, futures::stream::pending());
    (connection, entered, dropped, sent)
}

#[tokio::test(start_paused = true)]
async fn remote_pending_method_timeout_closes_both_queues_and_never_retries_unknown_delivery() {
    let (mut connection, entered, dropped, sent) = stalled_remote_bridge();
    connection
        .methods
        .try_send(protocol_test_completion("delivery-unknown"))
        .unwrap();
    entered.await.unwrap();
    for _ in 0..WORKER_PROTOCOL_METHOD_CAPACITY {
        connection
            .methods
            .try_send(protocol::Method::ListPendingSubmissions)
            .unwrap();
    }
    // Queues are saturated while the actual bridge is stalled in a wire send.
    assert!(
        connection
            .methods
            .try_send(protocol::Method::ListWorkers)
            .is_err()
    );
    tokio::time::advance(std::time::Duration::from_secs(10)).await;
    assert_eq!(
        connection.termination.ended().await,
        WorkerProtocolEnd::TransportTimeout
    );
    dropped.await.unwrap();
    assert!(connection.methods.is_closed());
    assert!(connection.events.recv().await.is_none());
    assert_eq!(
        sent.lock().unwrap().len(),
        1,
        "must not retry or drain uncertain pending methods"
    );
}

#[tokio::test]
async fn remote_disconnect_and_last_capability_drop_reclaim_a_pending_wire_send() {
    for explicit in [false, true] {
        let (connection, entered, dropped, sent) = stalled_remote_bridge();
        connection
            .methods
            .try_send(protocol_test_completion("cancelled-wire"))
            .unwrap();
        entered.await.unwrap();
        if explicit {
            connection
                .termination
                .terminate(WorkerProtocolEnd::Disconnected);
        }
        drop(connection);
        tokio::time::timeout(std::time::Duration::from_secs(2), dropped)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(sent.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn remote_actual_connector_overflow_ends_connection_and_preserves_delivered_event_order() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (release_tx, release) = oneshot::channel();
    let (wire_closed_tx, wire_closed) = oneshot::channel();
    let peer = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_hdr_async(
            stream,
            |request: &tokio_tungstenite::tungstenite::handshake::server::Request, response| {
                assert_eq!(
                    request.headers()["authorization"],
                    "Bearer test-worker-protocol"
                );
                assert_eq!(
                    request.headers()[protocol::AUTHENTICATED_ACCOUNT_ID_HEADER],
                    "test-account"
                );
                Ok(response)
            },
        )
        .await
        .unwrap();
        release.await.unwrap();
        for i in 0..=WORKER_PROTOCOL_EVENT_CAPACITY {
            let event = protocol::Event::TextDelta {
                text: i.to_string(),
            };
            let text = protocol::stream::encode_event(&event).unwrap();
            // TCP may observe closure before all writes flush; either way the
            // consumer must reach the same explicit bounded-overflow end.
            if socket
                .send(TungsteniteMessage::Text(text.into()))
                .await
                .is_err()
            {
                break;
            }
        }
        while let Some(Ok(message)) = socket.next().await {
            if matches!(message, TungsteniteMessage::Close(_)) {
                break;
            }
        }
        let _ = wire_closed_tx.send(());
    });
    let config = RuntimeObservationSourceConfig {
        worker: RuntimeWorkerRef::new("test-runtime", "test-worker"),
        endpoint: format!("ws://{addr}/protocol/ws"),
        bearer_token: Some("test-worker-protocol".into()),
    };
    let mut connection = connect_remote_worker_protocol(
        config,
        Some(&protocol::AuthenticatedInputSource::Account {
            account_id: "test-account".into(),
        }),
    )
    .await
    .unwrap();
    release_tx.send(()).unwrap();
    assert_eq!(
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            connection.termination.ended()
        )
        .await
        .unwrap(),
        WorkerProtocolEnd::EventOverflow
    );
    for i in 0..WORKER_PROTOCOL_EVENT_CAPACITY {
        assert!(
            matches!(connection.events.recv().await, Some(protocol::Event::TextDelta { text }) if text == i.to_string())
        );
    }
    assert!(connection.events.recv().await.is_none());
    assert!(connection.methods.is_closed());
    tokio::time::timeout(std::time::Duration::from_secs(2), wire_closed)
        .await
        .unwrap()
        .unwrap();
    peer.await.unwrap();
}
