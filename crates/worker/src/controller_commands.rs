//! Command issuance belongs to the Controller dequeue boundary, not its observers.
//! Replies share the admission ledger and are private to the dispatched endpoint;
//! broadcast events remain available to protocol clients but cannot satisfy an
//! unrelated Host request.

use super::*;
use std::sync::atomic::AtomicU64;

/// A failure to obtain admission evidence, not a command disposition.
/// `NotDelivered` means no lifecycle Method was dispatched: an exhausted Host
/// issuance request can reach the inbox without creating a command.
/// `AdmissionUnknown` follows successful enqueue without a usable reply; it is
/// not rejection and must not authorize a new ID or replacement endpoint.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WorkerCommandSendError {
    #[error("method is not a lifecycle command")]
    NotACommand,
    #[error("command {command:?} ({command_id:?}) was not delivered: {reason:?}")]
    NotDelivered {
        command_id: Option<u64>,
        command: WorkerCommandKind,
        reason: WorkerCommandNotDeliveredReason,
    },
    #[error("admission of command {command:?} ({command_id:?}) is unknown")]
    AdmissionUnknown {
        command_id: Option<u64>,
        command: WorkerCommandKind,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerCommandNotDeliveredReason {
    ControllerClosed,
    SubjectReleaseReserved,
    CommandIdExhausted,
}

pub(crate) type CommandReply =
    oneshot::Sender<Result<WorkerCommandAcknowledgement, WorkerCommandSendError>>;

pub(crate) enum ControllerMessage {
    Protocol(Method),
    Command {
        method: Method,
        reply: CommandReply,
    },
    HostCommand {
        kind: WorkerCommandKind,
        issued_id: Arc<AtomicU64>,
        reply: CommandReply,
    },
}

pub(crate) struct ControllerReceiver {
    rx: mpsc::Receiver<ControllerMessage>,
    shared_state: WorkerStateOwner,
    events: broadcast::Sender<Event>,
}

impl ControllerReceiver {
    pub(crate) fn new(
        rx: mpsc::Receiver<ControllerMessage>,
        shared_state: WorkerStateOwner,
        events: broadcast::Sender<Event>,
    ) -> Self {
        Self {
            rx,
            shared_state,
            events,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.rx.is_empty()
    }
    pub(crate) fn is_closed(&self) -> bool {
        self.rx.is_closed()
    }

    // There is no await between dequeue, ID allocation and ledger admission.
    // All controller phases use this receiver, including Run and compaction.
    pub(crate) async fn recv(&mut self) -> Option<Method> {
        loop {
            let (method, reply, host_admitted) = match self.rx.recv().await? {
                ControllerMessage::Protocol(method) => (method, None, false),
                ControllerMessage::Command { method, reply } => (method, Some(reply), false),
                ControllerMessage::HostCommand {
                    kind,
                    issued_id,
                    reply,
                } => {
                    let Some(envelope) = self.shared_state.admit_host_command(kind) else {
                        let _ = reply.send(Err(WorkerCommandSendError::NotDelivered {
                            command_id: None,
                            command: kind,
                            reason: WorkerCommandNotDeliveredReason::CommandIdExhausted,
                        }));
                        continue;
                    };
                    issued_id.store(envelope.command_id, Ordering::Release);
                    (command_method(kind, envelope), Some(reply), true)
                }
            };
            let Some((envelope, kind)) = command_identity(&method) else {
                return Some(method);
            };
            let admission = if host_admitted {
                WorkerCommandAdmission::Accepted
            } else {
                self.shared_state.admit_command(envelope, kind)
            };
            match admission {
                WorkerCommandAdmission::Accepted | WorkerCommandAdmission::Retry => {
                    if admission == WorkerCommandAdmission::Accepted {
                        // Command identity is snapshot content even when the command
                        // does not change Run state. Publish the same source sequence that
                        // a reconnect can already read before dispatching effects.
                        let _ = self.events.send(Event::WorkerState {
                            snapshot: self.shared_state.snapshot(),
                        });
                    }
                    if let Some(reply) = reply {
                        self.shared_state
                            .register_command_reply(envelope.command_id, kind, reply);
                    }
                    if admission == WorkerCommandAdmission::Retry {
                        if let Some(acknowledgement) = self
                            .shared_state
                            .command_acknowledgement(envelope.command_id, kind)
                        {
                            let _ = self
                                .events
                                .send(Event::CommandAcknowledged { acknowledgement });
                        }
                        // Shutdown retries must still reach the cleanup barrier.
                        // Other exact retries only replay evidence, never effects.
                        if kind != WorkerCommandKind::Shutdown {
                            continue;
                        }
                    }
                    return Some(method);
                }
                WorkerCommandAdmission::Conflict | WorkerCommandAdmission::StaleCommandId => {
                    let acknowledgement = WorkerCommandAcknowledgement {
                        command_id: envelope.command_id,
                        command: kind,
                        disposition: if admission == WorkerCommandAdmission::Conflict {
                            WorkerCommandDisposition::Conflict
                        } else {
                            WorkerCommandDisposition::StaleCommandId
                        },
                        state: self.shared_state.snapshot(),
                    };
                    if let Some(reply) = reply {
                        let _ = reply.send(Ok(acknowledgement.clone()));
                    }
                    let _ = self
                        .events
                        .send(Event::CommandAcknowledged { acknowledgement });
                }
            }
        }
    }
}

impl Drop for ControllerReceiver {
    fn drop(&mut self) {
        // Handles can retain shared state after this endpoint exits. Do not let
        // their ledger retain senders and turn endpoint loss into an endless wait.
        self.shared_state.close_command_replies();
    }
}

pub(super) fn command_identity(
    method: &Method,
) -> Option<(WorkerCommandEnvelope, WorkerCommandKind)> {
    match method {
        Method::Resume { command } => Some((*command, WorkerCommandKind::Resume)),
        Method::Cancel { command } => Some((*command, WorkerCommandKind::Cancel)),
        Method::Pause { command } => Some((*command, WorkerCommandKind::Pause)),
        Method::Compact { command } => Some((*command, WorkerCommandKind::Compact)),
        Method::Shutdown { command } => Some((*command, WorkerCommandKind::Shutdown)),
        _ => None,
    }
}

fn command_method(kind: WorkerCommandKind, command: WorkerCommandEnvelope) -> Method {
    match kind {
        WorkerCommandKind::Resume => Method::Resume { command },
        WorkerCommandKind::Cancel => Method::Cancel { command },
        WorkerCommandKind::Pause => Method::Pause { command },
        WorkerCommandKind::Compact => Method::Compact { command },
        WorkerCommandKind::Shutdown => Method::Shutdown { command },
    }
}

impl WorkerHandle {
    /// Allocate an ID at this Controller's dequeue/admission boundary and wait
    /// for its correlated disposition. Accepted is not Run/maintenance completion.
    ///
    /// This has no internal timeout. A caller may bound the wait, but cancellation
    /// after enqueue leaves admission unknown and must not trigger a fresh command.
    /// IDs and retry evidence are endpoint-local and retained only for a bounded
    /// window; neither endpoint replacement nor an evicted ID permits fresh replay.
    /// Replayed acknowledgements retain their original admission snapshot; they
    /// must not be used as a second writer of current Worker state.
    pub async fn send_host_command(
        &self,
        kind: WorkerCommandKind,
    ) -> Result<WorkerCommandAcknowledgement, WorkerCommandSendError> {
        let issued_id = Arc::new(AtomicU64::new(0));
        let (reply, response) = oneshot::channel();
        self.dispatch_command(
            ControllerMessage::HostCommand {
                kind,
                issued_id: issued_id.clone(),
                reply,
            },
            kind,
            None,
            response,
            Some(issued_id),
        )
        .await
    }

    /// Send an externally identified lifecycle command to this exact endpoint
    /// and await admission evidence without depending on broadcast delivery.
    /// An exact retry replays the recorded disposition without repeating effects
    /// (Shutdown retains its cleanup-barrier behavior). Non-command methods are
    /// not dispatched. There is no automatic retry or completion wait.
    pub async fn send_command(
        &self,
        method: Method,
    ) -> Result<WorkerCommandAcknowledgement, WorkerCommandSendError> {
        let (envelope, kind) =
            command_identity(&method).ok_or(WorkerCommandSendError::NotACommand)?;
        let (reply, response) = oneshot::channel();
        self.dispatch_command(
            ControllerMessage::Command { method, reply },
            kind,
            Some(envelope.command_id),
            response,
            None,
        )
        .await
    }

    async fn dispatch_command(
        &self,
        message: ControllerMessage,
        command: WorkerCommandKind,
        command_id: Option<u64>,
        response: oneshot::Receiver<Result<WorkerCommandAcknowledgement, WorkerCommandSendError>>,
        issued_id: Option<Arc<AtomicU64>>,
    ) -> Result<WorkerCommandAcknowledgement, WorkerCommandSendError> {
        // Keep the same enqueue lease as send(Method). Shutdown is deliberately
        // exempt so a release reservation cannot obstruct lifecycle cleanup.
        let admission = if command != WorkerCommandKind::Shutdown {
            self.shared_state
                .subject_release
                .get()
                .map(|control| control.authority.enter())
                .transpose()
                .map_err(|_| WorkerCommandSendError::NotDelivered {
                    command_id,
                    command,
                    reason: WorkerCommandNotDeliveredReason::SubjectReleaseReserved,
                })?
        } else {
            None
        };
        self.method_tx
            .send(message)
            .await
            .map_err(|_| WorkerCommandSendError::NotDelivered {
                command_id,
                command,
                reason: WorkerCommandNotDeliveredReason::ControllerClosed,
            })?;
        drop(admission);
        response.await.unwrap_or_else(|_| {
            let command_id = command_id.or_else(|| {
                issued_id
                    .as_ref()
                    .and_then(|id| match id.load(Ordering::Acquire) {
                        0 => None,
                        id => Some(id),
                    })
            });
            Err(WorkerCommandSendError::AdmissionUnknown {
                command_id,
                command,
            })
        })
    }
}
