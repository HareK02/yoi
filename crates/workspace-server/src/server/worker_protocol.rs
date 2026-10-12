//! Bounded operational protocol connections, separate from Workspace request admission.
//!
//! Each bridge captures one execution/transport, never a restored endpoint under the same
//! Worker ID. Snapshot precedes live events; direct replies and events keep bridge ordering,
//! without another pump. Method admission never waits for queue capacity under a session
//! guard. Event enqueue never waits; overflow ends the stream rather than skipping deltas.
//! The first terminal cause wins and preempts pending authorization and transport waits.
//!
//! Full-queue rejection means that method was not queued. Earlier queued methods, interrupted
//! sends and begun execution can have unknown side effects: no automatic retry or synthesized
//! success/non-execution. Cancel uses the same bounded queue, not an unbounded control lane.
//! The last client capability owns abort-on-drop; the pump does not retain that owner.
//! Event-consumer closure cancels the pump too. Reclamation drops channels and transport,
//! not the Worker, and is not rollback of a delivered method.
//!
//! Handshake and wire writes, including browser Pong/error, have bounded deadlines. The
//! dedicated session makes one separately bounded 1011 Close attempt; a blocked wire may
//! receive no terminal frame. Reconnect/resubscribe explicitly and replace local state with
//! a fresh snapshot; never replay old queues/deltas or implicitly Restore/retarget execution.
//! A begun synchronous embedded call settles its cancellation-and-join adapter wait before
//! endpoint exclusion is released; disconnect cannot immediately preempt that call.
use super::*;
use futures::{Sink, Stream};
use std::future::Future;
use tokio::sync::{mpsc, watch};

pub(super) const WORKER_PROTOCOL_METHOD_CAPACITY: usize = 256;
pub(super) const WORKER_PROTOCOL_EVENT_CAPACITY: usize = 512;
pub(super) const WORKER_PROTOCOL_TRANSPORT_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkerProtocolEnd {
    MethodOverflow,
    EventOverflow,
    TransportTimeout,
    Disconnected,
}
impl WorkerProtocolEnd {
    fn message(self) -> &'static str {
        match self {
            Self::MethodOverflow => "Worker protocol method queue overflow",
            Self::EventOverflow => "Worker protocol event queue overflow/lag",
            Self::TransportTimeout => "Worker protocol transport timed out",
            Self::Disconnected => "Worker protocol disconnected",
        }
    }
}

struct WorkerProtocolPump(tokio::task::AbortHandle);
impl Drop for WorkerProtocolPump {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// The last client capability owns the pump. The pump holds only the watch sender,
/// not this owner, so dropping a cancelled connection cannot leave an orphan task.
#[derive(Clone)]
pub(crate) struct WorkerProtocolTermination {
    state: watch::Sender<Option<WorkerProtocolEnd>>,
    _pump: std::sync::Arc<WorkerProtocolPump>,
}
impl WorkerProtocolTermination {
    pub(crate) fn subscribe(&self) -> watch::Receiver<Option<WorkerProtocolEnd>> {
        self.state.subscribe()
    }
    pub(crate) fn reason(&self) -> Option<WorkerProtocolEnd> {
        *self.state.borrow()
    }
    pub(crate) fn terminate(&self, reason: WorkerProtocolEnd) {
        finish(&self.state, reason);
    }
    pub(crate) async fn ended(&self) -> WorkerProtocolEnd {
        ended(&mut self.state.subscribe()).await
    }
}
fn finish(state: &watch::Sender<Option<WorkerProtocolEnd>>, reason: WorkerProtocolEnd) {
    state.send_if_modified(|current| {
        if current.is_some() {
            return false;
        }
        *current = Some(reason);
        true
    });
}
async fn ended(state: &mut watch::Receiver<Option<WorkerProtocolEnd>>) -> WorkerProtocolEnd {
    loop {
        if let Some(reason) = *state.borrow_and_update() {
            return reason;
        }
        if state.changed().await.is_err() {
            return WorkerProtocolEnd::Disconnected;
        }
    }
}

pub(crate) struct WorkspaceWorkerProtocolConnection {
    pub(crate) methods: mpsc::Sender<protocol::Method>,
    pub(crate) events: mpsc::Receiver<protocol::Event>,
    pub(crate) termination: WorkerProtocolTermination,
}

pub(super) fn spawn_worker_protocol_pump<F, Fut>(pump: F) -> WorkspaceWorkerProtocolConnection
where
    F: FnOnce(mpsc::Receiver<protocol::Method>, mpsc::Sender<protocol::Event>) -> Fut,
    Fut: Future<Output = WorkerProtocolEnd> + Send + 'static,
{
    let (methods, receiver) = mpsc::channel(WORKER_PROTOCOL_METHOD_CAPACITY);
    let (sender, events) = mpsc::channel(WORKER_PROTOCOL_EVENT_CAPACITY);
    let (state, mut shutdown) = watch::channel(None);
    let task_state = state.clone();
    let closed = sender.clone();
    let future = pump(receiver, sender);
    let task = tokio::spawn(async move {
        let reason = tokio::select! {
            biased;
            reason = ended(&mut shutdown) => reason,
            _ = closed.closed() => WorkerProtocolEnd::Disconnected,
            reason = future => reason,
        };
        finish(&task_state, reason);
        // Dropping the future releases both queues and the captured execution/WS endpoint.
    });
    WorkspaceWorkerProtocolConnection {
        methods,
        events,
        termination: WorkerProtocolTermination {
            state,
            _pump: std::sync::Arc::new(WorkerProtocolPump(task.abort_handle())),
        },
    }
}

pub(super) fn enqueue_worker_protocol_event(
    sender: &mpsc::Sender<protocol::Event>,
    event: protocol::Event,
) -> std::result::Result<(), WorkerProtocolEnd> {
    sender.try_send(event).map_err(|error| match error {
        mpsc::error::TrySendError::Full(_) => WorkerProtocolEnd::EventOverflow,
        mpsc::error::TrySendError::Closed(_) => WorkerProtocolEnd::Disconnected,
    })
}

/// Actual remote bridge pump, also usable with a bounded in-process wire fixture.
/// A transport send may have partially succeeded when interrupted: no retry or success
/// acknowledgement is synthesized. All transport waits have a deadline; cancellation
/// of the common owner drops the pending send and both halves of this one connection.
pub(super) fn remote_worker_protocol_bridge<S, R>(
    mut sink: S,
    mut stream: R,
) -> WorkspaceWorkerProtocolConnection
where
    S: Sink<TungsteniteMessage> + Unpin + Send + 'static,
    R: Stream<
            Item = std::result::Result<TungsteniteMessage, tokio_tungstenite::tungstenite::Error>,
        > + Unpin
        + Send
        + 'static,
    S::Error: Send,
{
    spawn_worker_protocol_pump(move |mut methods, events| async move {
        loop {
            tokio::select! {
                method = methods.recv() => {
                    let Some(method) = method else { return WorkerProtocolEnd::Disconnected; };
                    let Ok(text) = protocol::stream::encode_method(&method) else { return WorkerProtocolEnd::Disconnected; };
                    match tokio::time::timeout(WORKER_PROTOCOL_TRANSPORT_TIMEOUT, sink.send(TungsteniteMessage::Text(text.into()))).await {
                        Ok(Ok(())) => {},
                        Ok(Err(_)) => return WorkerProtocolEnd::Disconnected,
                        Err(_) => return WorkerProtocolEnd::TransportTimeout,
                    }
                },
                message = stream.next() => match message {
                    Some(Ok(TungsteniteMessage::Text(text))) => {
                        let Ok(event) = protocol::stream::decode_event(text.as_ref()) else { return WorkerProtocolEnd::Disconnected; };
                        if let Err(reason) = enqueue_worker_protocol_event(&events, event) { return reason; }
                    }
                    Some(Ok(TungsteniteMessage::Ping(value))) => {
                        match tokio::time::timeout(WORKER_PROTOCOL_TRANSPORT_TIMEOUT, sink.send(TungsteniteMessage::Pong(value))).await {
                            Ok(Ok(())) => {},
                            Ok(Err(_)) => return WorkerProtocolEnd::Disconnected,
                            Err(_) => return WorkerProtocolEnd::TransportTimeout,
                        }
                    }
                    Some(Ok(TungsteniteMessage::Pong(_))) => {},
                    _ => return WorkerProtocolEnd::Disconnected,
                }
            }
        }
    })
}

/// Browser admission can await authorization/session ownership, but never method queue
/// capacity. Common termination preempts that wait. Browser writes are bounded too.
pub(super) async fn workspace_worker_protocol_session<S>(
    mut connection: worker_operations::WorkspaceWorkerConnection,
    mut socket: S,
    context: WorkerOperationContext,
) where
    S: Stream<Item = std::result::Result<WsMessage, axum::Error>> + Sink<WsMessage> + Unpin,
{
    let termination = connection.sender.termination.clone();
    let session = async {
        loop {
            tokio::select! {
                inbound = socket.next() => match inbound {
                    Some(Ok(WsMessage::Text(text))) => {
                        let result = match decode_method(&text) {
                            Ok(method) => connection.sender.send(&context, method).await.map_err(|error| error.to_string()),
                            Err(error) => Err(error.to_string()),
                        };
                        if let Err(error) = result {
                            let event = protocol_error_event(error);
                            let Ok(text) = protocol::stream::encode_event(&event) else { return WorkerProtocolEnd::Disconnected; };
                            if let Err(reason) = browser_send(&mut socket, WsMessage::Text(text.into())).await { return reason; }
                        }
                    }
                    Some(Ok(WsMessage::Ping(value))) => {
                        if let Err(reason) = browser_send(&mut socket, WsMessage::Pong(value)).await { return reason; }
                    }
                    Some(Ok(WsMessage::Pong(_))) => {},
                    _ => return WorkerProtocolEnd::Disconnected,
                },
                event = connection.events.recv() => {
                    let Some(event) = event else { return WorkerProtocolEnd::Disconnected; };
                    let shutdown = matches!(event, protocol::Event::Shutdown);
                    let Ok(text) = protocol::stream::encode_event(&event) else { return WorkerProtocolEnd::Disconnected; };
                    if let Err(reason) = browser_send(&mut socket, WsMessage::Text(text.into())).await { return reason; }
                    if shutdown { return WorkerProtocolEnd::Disconnected; }
                }
            }
        }
    };
    let reason = tokio::select! {
        biased;
        reason = termination.ended() => reason,
        reason = session => reason,
    };
    termination.terminate(reason);
    // Best-effort explicit close even if the downstream transport is also blocked.
    // The close reason deliberately makes no claim that admitted methods did not run.
    let _ = tokio::time::timeout(
        WORKER_PROTOCOL_TRANSPORT_TIMEOUT,
        socket.send(WsMessage::Close(Some(axum::extract::ws::CloseFrame {
            code: 1011,
            reason: format!(
                "{}; pending delivery unknown; resync snapshot; no retry",
                reason.message()
            )
            .into(),
        }))),
    )
    .await;
}
async fn browser_send<S: Sink<WsMessage> + Unpin>(
    socket: &mut S,
    message: WsMessage,
) -> std::result::Result<(), WorkerProtocolEnd> {
    match tokio::time::timeout(WORKER_PROTOCOL_TRANSPORT_TIMEOUT, socket.send(message)).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(_)) => Err(WorkerProtocolEnd::Disconnected),
        Err(_) => Err(WorkerProtocolEnd::TransportTimeout),
    }
}
