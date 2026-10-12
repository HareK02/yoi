use super::*;
use protocol::subscription::{
    EventSubscriptionSelector, SubscriptionFrame, SubscriptionFramePayload, SubscriptionRequest,
    SubscriptionRequestId,
};
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::sync::{mpsc, oneshot};

// A controlled browser wire. Receiving remains independent of a blocked writer,
// reproducing the real multiplexer split without filling an OS socket by timing.
struct CongestedBrowser {
    inbound: mpsc::Receiver<WsMessage>,
    entered: Option<oneshot::Sender<()>>,
    dropped: Option<oneshot::Sender<()>>,
}
impl futures::Stream for CongestedBrowser {
    type Item = std::result::Result<WsMessage, axum::Error>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inbound.poll_recv(cx).map(|message| message.map(Ok))
    }
}
impl futures::Sink<WsMessage> for CongestedBrowser {
    type Error = std::convert::Infallible;
    fn poll_ready(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
    ) -> Poll<std::result::Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
    fn start_send(mut self: Pin<&mut Self>, _: WsMessage) -> std::result::Result<(), Self::Error> {
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
impl Drop for CongestedBrowser {
    fn drop(&mut self) {
        if let Some(dropped) = self.dropped.take() {
            let _ = dropped.send(());
        }
    }
}

#[tokio::test(start_paused = true)]
async fn actual_workspace_multiplexer_stalled_writer_timeout_or_disconnect_reclaims_connection_tasks()
 {
    for exit in ["timeout", "disconnect", "caller-cancel"] {
        let workspace = tempfile::tempdir().unwrap();
        let (api, _) = test_api_with_recording_backend(workspace.path()).await;
        let identity = spawn_ticket_check_source(&api, "multiplexer-stalled-writer");
        let (inbound, receiver) = mpsc::channel(300);
        let (entered_tx, entered) = oneshot::channel();
        let (dropped_tx, dropped) = oneshot::channel();
        let socket = CongestedBrowser {
            inbound: receiver,
            entered: Some(entered_tx),
            dropped: Some(dropped_tx),
        };
        let frame = SubscriptionFrame::new(SubscriptionFramePayload::Request(
            SubscriptionRequest::SubscribeEvents {
                request_id: SubscriptionRequestId::new("saturated-browser").unwrap(),
                selector: EventSubscriptionSelector::WorkspaceWorkers,
            },
        ));
        inbound
            .try_send(WsMessage::Text(
                serde_json::to_string(&frame).unwrap().into(),
            ))
            .unwrap();
        let task = tokio::spawn(crate::workspace_subscription::serve_workspace_subscription(
            api.clone(),
            socket,
            WorkerOperationContext::Backend,
        ));
        entered.await.unwrap(); // Actual subscription snapshot reached the blocked wire.
        match exit {
            "timeout" => {
                // Fill the control queue as well; the receiving side may now be
                // waiting for Pong capacity. Writer failure must preempt that wait.
                for _ in 0..258 {
                    inbound
                        .try_send(WsMessage::Ping(Vec::new().into()))
                        .unwrap();
                }
                tokio::time::advance(std::time::Duration::from_secs(10)).await;
                task.await.unwrap();
            }
            "disconnect" => {
                drop(inbound);
                task.await.unwrap();
            }
            "caller-cancel" => {
                task.abort();
                assert!(task.await.unwrap_err().is_cancelled());
            }
            _ => unreachable!(),
        }
        dropped.await.unwrap();
        let worker =
            WorkspaceWorker::resolve(&api, &identity.runtime_id, &identity.worker_id).unwrap();
        let mut fresh = worker
            .connect_protocol(&WorkerOperationContext::Backend)
            .await
            .unwrap();
        assert!(matches!(
            fresh.events.recv().await,
            Some(protocol::Event::Snapshot { .. })
        ));
        let stopped = worker
            .stop(
                &WorkerOperationContext::Backend,
                WorkerLifecycleRequest {
                    reason: Some("cleanup-after-multiplexer-exit".into()),
                    ticket_assignment: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(stopped.state, InternalWorkerOperationState::Accepted);
        assert!(
            current_worker_session_lock(&api, &identity)
                .try_lock()
                .is_ok()
        );
    }
}
