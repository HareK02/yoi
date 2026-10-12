//! Workspace multiplexer over the shared identity-bound Worker method sender.
//!
//! Separate bounded control/protocol lanes preserve control priority. Writer deadline/exit
//! cancels receive/admission even while waiting for session ownership or control capacity;
//! receive exit aborts/joins writer and subscriptions rather than draining a congested wire.
//! Caller cancellation owns abort-on-drop tasks. Bridge overflow reports Lagged when a terminal
//! control frame can be delivered, never normal resource disappearance. Reconnect/resubscribe
//! for a fresh snapshot; termination proves neither method non-execution nor Worker shutdown.
use std::collections::HashMap;

use axum::extract::ws::Message as WsMessage;
use futures::{SinkExt, StreamExt};
use protocol::subscription::{
    EventSubscriptionSelector, SubscriptionEvent, SubscriptionEventPayload, SubscriptionFrame,
    SubscriptionFramePayload, SubscriptionId, SubscriptionRejectionCode, SubscriptionRequest,
    SubscriptionResponse, SubscriptionSnapshot, SubscriptionTerminationCode, SubscriptionWorker,
};
use tokio::sync::mpsc;
use worker_runtime::identity::RuntimeWorkerRef;

use crate::runtime_subscription::RuntimeSubscriptionBroker;
use crate::server::{
    WorkerOperationContext, WorkerProtocolEnd, WorkspaceApi, WorkspaceWorker,
    WorkspaceWorkerMethodSender,
};

const OUTBOUND_CAPACITY: usize = 256;
const WORKER_PROTOCOL_SNAPSHOT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

// Every spawned writer/subscription is connection-owned, including caller cancellation.
struct SubscriptionTask(tokio::task::JoinHandle<()>);
impl Drop for SubscriptionTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}
struct ActiveSubscription {
    task: SubscriptionTask,
    methods: Option<WorkspaceWorkerMethodSender>,
}

pub(crate) async fn serve_workspace_subscription<S>(
    api: WorkspaceApi,
    socket: S,
    context: WorkerOperationContext,
) where
    S: futures::Stream<Item = Result<WsMessage, axum::Error>>
        + futures::Sink<WsMessage>
        + Unpin
        + Send
        + 'static,
    S::Error: Send,
{
    let broker = api.runtime_subscription_broker().clone();
    let (mut socket_sender, mut socket_receiver) = socket.split();
    let (control_outbound, mut control_receiver) = mpsc::channel::<WsMessage>(OUTBOUND_CAPACITY);
    let (protocol_outbound, mut protocol_receiver) = mpsc::channel::<WsMessage>(OUTBOUND_CAPACITY);
    let (writer_done, finished) = tokio::sync::oneshot::channel();
    let mut writer = SubscriptionTask(tokio::spawn(async move {
        loop {
            let message = tokio::select! {
                biased;
                message = control_receiver.recv() => message,
                message = protocol_receiver.recv() => message,
            };
            let Some(message) = message else { break };
            if !matches!(
                tokio::time::timeout(
                    WORKER_PROTOCOL_SNAPSHOT_TIMEOUT,
                    socket_sender.send(message)
                )
                .await,
                Ok(Ok(()))
            ) {
                break;
            }
        }
        let _ = writer_done.send(());
    }));
    let mut next_subscription_id = 1_u64;
    let mut subscriptions = HashMap::<SubscriptionId, ActiveSubscription>::new();

    let receiving = async {
        while let Some(message) = socket_receiver.next().await {
            let Ok(message) = message else { break };
            match message {
                WsMessage::Text(text) => {
                    let Ok(frame) = serde_json::from_str::<SubscriptionFrame>(text.as_str()) else {
                        break;
                    };
                    if frame.validate().is_err() {
                        break;
                    }
                    subscriptions.retain(|_, subscription| !subscription.task.0.is_finished());
                    match frame.payload {
                        SubscriptionFramePayload::Request(
                            SubscriptionRequest::SubscribeEvents {
                                request_id,
                                selector,
                            },
                        ) => {
                            let subscription_id = SubscriptionId::new(format!(
                                "workspace-subscription-{next_subscription_id}"
                            ))
                            .expect("generated Workspace subscription id is valid");
                            next_subscription_id = next_subscription_id.saturating_add(1);
                            match selector {
                                EventSubscriptionSelector::WorkspaceWorkers => {
                                    let task = tokio::spawn(run_workspace_workers(
                                        api.clone(),
                                        broker.clone(),
                                        request_id,
                                        subscription_id.clone(),
                                        control_outbound.clone(),
                                    ));
                                    subscriptions.insert(
                                        subscription_id,
                                        ActiveSubscription {
                                            task: SubscriptionTask(task),
                                            methods: None,
                                        },
                                    );
                                }
                                EventSubscriptionSelector::WorkerProtocol {
                                    worker_id,
                                    runtime_id: Some(runtime_id),
                                } => {
                                    let connection = match WorkspaceWorker::resolve_async(
                                        &api,
                                        &runtime_id,
                                        worker_id.as_str(),
                                    )
                                    .await
                                    {
                                        Ok(worker) => worker
                                            .connect_protocol(&context)
                                            .await
                                            .map_err(|error| error.to_string()),
                                        Err(error) => Err(error.to_string()),
                                    };
                                    match connection {
                                        Ok(connection) => {
                                            let methods = connection.sender.clone();
                                            let task = tokio::spawn(run_worker_protocol(
                                                request_id,
                                                subscription_id.clone(),
                                                runtime_id,
                                                worker_id,
                                                connection.events,
                                                methods.end_status(),
                                                control_outbound.clone(),
                                                protocol_outbound.clone(),
                                            ));
                                            subscriptions.insert(
                                                subscription_id,
                                                ActiveSubscription {
                                                    task: SubscriptionTask(task),
                                                    methods: Some(methods),
                                                },
                                            );
                                        }
                                        Err(error) => {
                                            let _ = send_rejected(
                                                &control_outbound,
                                                request_id,
                                                SubscriptionRejectionCode::ResourceNotFound,
                                                error.to_string(),
                                            )
                                            .await;
                                        }
                                    }
                                }
                                _ => {
                                    let _ = send_rejected(
                                    &control_outbound, request_id, SubscriptionRejectionCode::UnsupportedSelector,
                                    "Workspace clients may subscribe only to workspace_workers or a runtime-scoped worker_protocol selector".to_string(),
                                ).await;
                                }
                            }
                        }
                        SubscriptionFramePayload::Request(
                            SubscriptionRequest::UnsubscribeEvents {
                                request_id,
                                subscription_id,
                            },
                        ) => {
                            if let Some(mut subscription) = subscriptions.remove(&subscription_id) {
                                subscription.task.0.abort();
                                let _ = (&mut subscription.task.0).await;
                            }
                            if send_frame(
                                &control_outbound,
                                SubscriptionFrame::new(SubscriptionFramePayload::Response(
                                    SubscriptionResponse::Unsubscribed {
                                        request_id,
                                        subscription_id,
                                    },
                                )),
                            )
                            .await
                            .is_err()
                            {
                                break;
                            }
                        }
                        SubscriptionFramePayload::WorkerProtocol(message) => {
                            let Some(methods) = subscriptions
                                .get(&message.subscription_id)
                                .and_then(|value| value.methods.clone())
                            else {
                                break;
                            };
                            if methods.send(&context, message.method).await.is_err() {
                                break;
                            }
                        }
                        SubscriptionFramePayload::Response(_)
                        | SubscriptionFramePayload::Event(_) => {
                            break;
                        }
                    }
                }
                WsMessage::Ping(value) => {
                    if control_outbound.send(WsMessage::Pong(value)).await.is_err() {
                        break;
                    }
                }
                WsMessage::Pong(_) => {}
                WsMessage::Close(_) | WsMessage::Binary(_) => break,
            }
        }
    };
    // Writer termination also preempts method authorization/control queue waits.
    tokio::select! { _ = receiving => {}, _ = finished => {} }
    for (_, mut subscription) in subscriptions {
        subscription.task.0.abort();
        let _ = (&mut subscription.task.0).await;
    }
    // Never wait for a congested writer to drain after disconnect or admission failure.
    writer.0.abort();
    let _ = (&mut writer.0).await;
}

async fn send_rejected(
    outbound: &mpsc::Sender<WsMessage>,
    request_id: protocol::subscription::SubscriptionRequestId,
    code: SubscriptionRejectionCode,
    message: String,
) -> Result<(), ()> {
    send_frame(
        outbound,
        SubscriptionFrame::new(SubscriptionFramePayload::Response(
            SubscriptionResponse::SubscriptionRejected {
                request_id,
                subscription_id: None,
                code,
                message,
            },
        )),
    )
    .await
}

async fn run_worker_protocol(
    request_id: protocol::subscription::SubscriptionRequestId,
    subscription_id: SubscriptionId,
    runtime_id: String,
    worker_id: protocol::subscription::SubscriptionWorkerId,
    mut events: mpsc::Receiver<protocol::Event>,
    end_status: tokio::sync::watch::Receiver<Option<WorkerProtocolEnd>>,
    control_outbound: mpsc::Sender<WsMessage>,
    protocol_outbound: mpsc::Sender<WsMessage>,
) {
    let initial_event_result =
        tokio::time::timeout(WORKER_PROTOCOL_SNAPSHOT_TIMEOUT, events.recv()).await;
    let initial_event = match initial_event_result {
        Ok(Some(event @ protocol::Event::Snapshot { .. })) => event,
        Ok(Some(_)) => {
            let _ = send_rejected(
                &control_outbound,
                request_id,
                SubscriptionRejectionCode::Internal,
                "Worker protocol did not begin with a session snapshot".to_string(),
            )
            .await;
            return;
        }
        Ok(None) => {
            let _ = send_rejected(
                &control_outbound,
                request_id,
                SubscriptionRejectionCode::ResourceNotFound,
                "Worker protocol closed before the session snapshot".to_string(),
            )
            .await;
            return;
        }
        Err(_) => {
            let _ = send_rejected(
                &control_outbound,
                request_id,
                SubscriptionRejectionCode::Internal,
                "Timed out waiting for the Worker protocol session snapshot".to_string(),
            )
            .await;
            return;
        }
    };
    if send_frame(
        &control_outbound,
        SubscriptionFrame::new(SubscriptionFramePayload::Response(
            SubscriptionResponse::Subscribed {
                request_id,
                subscription_id: subscription_id.clone(),
                selector: EventSubscriptionSelector::WorkerProtocol {
                    worker_id: worker_id.clone(),
                    runtime_id: Some(runtime_id),
                },
                snapshot: SubscriptionSnapshot::WorkerProtocol {
                    worker_id: worker_id.clone(),
                    events: vec![initial_event],
                },
            },
        )),
    )
    .await
    .is_err()
    {
        return;
    }
    while let Some(event) = events.recv().await {
        let frame =
            SubscriptionFrame::new(SubscriptionFramePayload::Event(SubscriptionEvent::Event {
                subscription_id: subscription_id.clone(),
                payload: SubscriptionEventPayload::WorkerProtocol {
                    worker_id: worker_id.clone(),
                    event,
                },
            }));
        if try_send_frame(&protocol_outbound, frame).is_err() {
            let _ = send_frame(
                &control_outbound,
                SubscriptionFrame::new(SubscriptionFramePayload::Event(
                    SubscriptionEvent::SubscriptionClosed {
                        subscription_id: subscription_id.clone(),
                        code: SubscriptionTerminationCode::Lagged,
                        message:
                            "Worker protocol subscriber lagged; resubscribe for a fresh snapshot"
                                .to_string(),
                    },
                )),
            )
            .await;
            return;
        }
    }
    let _ = send_frame(
        &control_outbound,
        SubscriptionFrame::new(SubscriptionFramePayload::Event(
            SubscriptionEvent::SubscriptionClosed {
                subscription_id,
                code: if matches!(*end_status.borrow(), Some(WorkerProtocolEnd::EventOverflow | WorkerProtocolEnd::MethodOverflow)) { SubscriptionTerminationCode::Lagged } else { SubscriptionTerminationCode::ResourceGone },
                message: "Worker protocol stream ended; pending method delivery may be unknown; resubscribe with a fresh snapshot, no automatic retry".to_string(),
            },
        )),
    )
    .await;
}

async fn run_workspace_workers(
    api: WorkspaceApi,
    _broker: RuntimeSubscriptionBroker,
    request_id: protocol::subscription::SubscriptionRequestId,
    subscription_id: SubscriptionId,
    outbound: mpsc::Sender<WsMessage>,
) {
    let Ok(mut projection) = api
        .worker_projection
        .subscribe_ordered_async(api.config.max_records.max(1))
        .await
    else {
        let _ = send_rejected(
            &outbound,
            request_id,
            SubscriptionRejectionCode::Internal,
            "failed to read durable Worker projection".to_string(),
        )
        .await;
        return;
    };
    let Ok(mut workers) = project_registry_workers_async(
        api.store.clone(),
        api.config.workspace_id.clone(),
        projection.take_snapshot(),
    )
    .await
    else {
        let _ = send_rejected(
            &outbound,
            request_id,
            SubscriptionRejectionCode::Internal,
            "failed to enrich durable Worker projection".to_string(),
        )
        .await;
        return;
    };
    sort_workers(&mut workers);
    if send_frame(
        &outbound,
        SubscriptionFrame::new(SubscriptionFramePayload::Response(
            SubscriptionResponse::Subscribed {
                request_id,
                subscription_id: subscription_id.clone(),
                selector: EventSubscriptionSelector::WorkspaceWorkers,
                snapshot: SubscriptionSnapshot::Workers { workers },
            },
        )),
    )
    .await
    .is_err()
    {
        return;
    }

    loop {
        match projection.recv().await {
            Ok(event) => {
                for change in event.changes {
                    let payload = match change {
                        crate::worker_projection::WorkerProjectionChange::Upsert(record) => {
                            let workers = project_registry_workers_async(
                                api.store.clone(),
                                api.config.workspace_id.clone(),
                                vec![record],
                            )
                            .await;
                            let Ok(mut workers) = workers else {
                                // Admission failure cannot silently lose a live
                                // update: terminate and require a fresh snapshot.
                                let _ = send_frame(
                                    &outbound,
                                    SubscriptionFrame::new(SubscriptionFramePayload::Event(
                                        SubscriptionEvent::SubscriptionClosed {
                                            subscription_id: subscription_id.clone(),
                                            code: SubscriptionTerminationCode::Lagged,
                                            message: "Worker projection enrichment unavailable; reconnect for a fresh snapshot".to_string(),
                                        },
                                    )),
                                )
                                .await;
                                return;
                            };
                            let Some(worker) = workers.pop() else {
                                continue;
                            };
                            SubscriptionEventPayload::WorkerUpserted { worker }
                        }
                        crate::worker_projection::WorkerProjectionChange::Removed(worker) => {
                            let Ok(worker_id) =
                                protocol::subscription::SubscriptionWorkerId::new(worker.worker_id)
                            else {
                                continue;
                            };
                            SubscriptionEventPayload::WorkerRemoved {
                                worker_id,
                                runtime_id: Some(worker.runtime_id),
                            }
                        }
                    };
                    if send_event(&outbound, &subscription_id, payload)
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                let _ = send_frame(
                    &outbound,
                    SubscriptionFrame::new(SubscriptionFramePayload::Event(
                        SubscriptionEvent::SubscriptionClosed {
                            subscription_id: subscription_id.clone(),
                            code: protocol::subscription::SubscriptionTerminationCode::Lagged,
                            message: "durable Worker projection subscriber lagged; reconnect for a fresh snapshot".to_string(),
                        },
                    )),
                )
                .await;
                return;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
        }
    }
}

async fn project_registry_workers_async(
    store: std::sync::Arc<dyn crate::store::ControlPlaneStore>,
    workspace_id: String,
    records: Vec<crate::store::WorkerRegistryProjectionRecord>,
) -> crate::Result<Vec<SubscriptionWorker>> {
    store
        .db_executor()
        .run(move || {
            // Keep the committed Worker payload; only the existing Workspace-scoped
            // workdir/history/repository enrichment reads occur inside this job.
            Ok(records
                .into_iter()
                .filter_map(|record| project_registry_worker(store.as_ref(), &workspace_id, record))
                .collect())
        })
        .await
}

// Synchronous helpers stay synchronous; they run only inside the job above.
fn project_registry_worker(
    store: &dyn crate::store::ControlPlaneStore,
    workspace_id: &str,
    record: crate::store::WorkerRegistryProjectionRecord,
) -> Option<SubscriptionWorker> {
    let runtime_id = record.registry.worker.runtime_id.clone();
    let mut worker = if let Some(observation) = record.observation {
        let mut worker = observation.worker;
        worker.availability = observation.availability;
        worker
    } else {
        SubscriptionWorker {
            restore_observation_token: None,
            worker_id: protocol::subscription::SubscriptionWorkerId::new(
                record.registry.worker.worker_id.clone(),
            )
            .ok()?,
            runtime_id: Some(runtime_id.clone()),
            resource_key: record.resource_key.clone(),
            availability: protocol::subscription::SubscriptionWorkerAvailability::Unavailable,
            worker_state: None,
            state: protocol::subscription::SubscriptionWorkerState::Stopped,
            has_running_internal_workers: false,
            workspace_id: Some(record.registry.workspace_id.clone()),
            display_name: Some(record.registry.display_name.clone()),
            profile: record.registry.profile.clone(),
            job: None,
            workdir_attachments: Vec::new(),
        }
    };
    worker.runtime_id = Some(runtime_id);
    worker.resource_key = record.resource_key;
    worker.workspace_id = Some(record.registry.workspace_id);
    worker.display_name = Some(record.registry.display_name);
    worker.profile = record.registry.profile;
    worker.job = record
        .job
        .map(|job| protocol::subscription::SubscriptionWorkerJob {
            job_id: job.job_id,
            attempt_id: job.attempt_id,
            purpose: job.purpose,
        });
    if !project_working_directory(store, workspace_id, &record.registry.worker, &mut worker) {
        return None;
    }
    Some(worker)
}

fn project_working_directory(
    store: &dyn crate::store::ControlPlaneStore,
    workspace_id: &str,
    worker_ref: &RuntimeWorkerRef,
    worker: &mut SubscriptionWorker,
) -> bool {
    let Ok(links) = store.list_worker_workdir_links(workspace_id, worker_ref) else {
        return false;
    };
    if !links.is_empty() {
        let mut attachments = Vec::with_capacity(links.len());
        for link in links {
            let Ok(Some(workdir)) = store.get_workdir_registry(workspace_id, &link.workdir_id)
            else {
                return false;
            };
            let Ok(working_directory_id) =
                protocol::subscription::SubscriptionWorkdirId::new(workdir.workdir_id)
            else {
                return false;
            };
            attachments.push(
                protocol::subscription::SubscriptionWorkerWorkdirAttachment {
                    alias: link.alias,
                    repository_id: workdir.source.repository_id().map(str::to_string),
                    repository_key: None,
                    working_directory_id,
                },
            );
        }
        worker.workdir_attachments = attachments;
    } else {
        let Ok(has_link_history) =
            store.worker_workdir_link_history_exists(workspace_id, worker_ref)
        else {
            return false;
        };
        if has_link_history {
            worker.workdir_attachments.clear();
        }
    }
    project_repository_keys(store, workspace_id, worker)
}

fn project_repository_keys(
    store: &dyn crate::store::ControlPlaneStore,
    workspace_id: &str,
    worker: &mut SubscriptionWorker,
) -> bool {
    project_repository_keys_with(worker, |repository_id| {
        store
            .get_repository(workspace_id, repository_id)
            .ok()
            .flatten()
            .map(|repository| repository.repository_key)
    })
}

fn project_repository_keys_with(
    worker: &mut SubscriptionWorker,
    mut resolve_repository_key: impl FnMut(&str) -> Option<String>,
) -> bool {
    for attachment in &mut worker.workdir_attachments {
        let Some(repository_id) = attachment.repository_id.take() else {
            // External Workdirs have no Repository identity. Their stable
            // working_directory_id joins against the Workspace Workdir projection.
            continue;
        };
        let Some(repository_key) = resolve_repository_key(&repository_id) else {
            return false;
        };
        attachment.repository_key = Some(repository_key);
    }
    true
}

fn sort_workers(workers: &mut [SubscriptionWorker]) {
    workers.sort_by(|left, right| {
        left.runtime_id
            .cmp(&right.runtime_id)
            .then_with(|| left.worker_id.cmp(&right.worker_id))
    });
}

async fn send_event(
    outbound: &mpsc::Sender<WsMessage>,
    subscription_id: &SubscriptionId,
    payload: SubscriptionEventPayload,
) -> Result<(), ()> {
    send_frame(
        outbound,
        SubscriptionFrame::new(SubscriptionFramePayload::Event(SubscriptionEvent::Event {
            subscription_id: subscription_id.clone(),
            payload,
        })),
    )
    .await
}

fn try_send_frame(outbound: &mpsc::Sender<WsMessage>, frame: SubscriptionFrame) -> Result<(), ()> {
    frame.validate().map_err(|_| ())?;
    let text = serde_json::to_string(&frame).map_err(|_| ())?;
    outbound
        .try_send(WsMessage::Text(text.into()))
        .map_err(|_| ())
}

async fn send_frame(
    outbound: &mpsc::Sender<WsMessage>,
    frame: SubscriptionFrame,
) -> Result<(), ()> {
    frame.validate().map_err(|_| ())?;
    outbound
        .send(WsMessage::Text(
            serde_json::to_string(&frame).map_err(|_| ())?.into(),
        ))
        .await
        .map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request_id() -> protocol::subscription::SubscriptionRequestId {
        protocol::subscription::SubscriptionRequestId::new("request-1").unwrap()
    }

    fn subscription_id() -> SubscriptionId {
        SubscriptionId::new("subscription-1").unwrap()
    }

    fn worker_id() -> protocol::subscription::SubscriptionWorkerId {
        protocol::subscription::SubscriptionWorkerId::new("worker-1").unwrap()
    }

    fn external_worker() -> SubscriptionWorker {
        SubscriptionWorker {
            restore_observation_token: None,
            worker_id: worker_id(),
            runtime_id: Some("runtime-1".to_string()),
            resource_key: Some("W-1".to_string()),
            availability: protocol::subscription::SubscriptionWorkerAvailability::Observed,
            worker_state: None,
            state: protocol::subscription::SubscriptionWorkerState::Idle,
            has_running_internal_workers: false,
            workspace_id: Some("workspace-1".to_string()),
            display_name: Some("Companion".to_string()),
            profile: Some("builtin:companion".to_string()),
            job: None,
            workdir_attachments: vec![
                protocol::subscription::SubscriptionWorkerWorkdirAttachment {
                    alias: "external".to_string(),
                    repository_id: None,
                    repository_key: None,
                    working_directory_id: protocol::subscription::SubscriptionWorkdirId::new(
                        "external-workdir-1",
                    )
                    .unwrap(),
                },
            ],
        }
    }

    #[test]
    fn external_workdir_attachment_does_not_remove_workspace_worker() {
        let mut worker = external_worker();
        assert!(project_repository_keys_with(&mut worker, |_| {
            panic!("External Workdir must not require Repository projection")
        }));
        worker.validate().unwrap();
        assert_eq!(worker.workdir_attachments.len(), 1);
        assert_eq!(
            worker.workdir_attachments[0].working_directory_id.as_str(),
            "external-workdir-1"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn async_enrichment_preserves_committed_worker_and_external_attachment_but_hides_unresolved_repository()
     {
        use crate::store::{
            AccountRecord, ControlPlaneStore, SqliteWorkspaceStore, WorkerRegistryRecord,
            WorkspaceRecord,
        };
        let store = std::sync::Arc::new(
            SqliteWorkspaceStore::in_memory_for_test()
                .unwrap()
                .with_db_executor(crate::db_executor::DbExecutor::new(Default::default()).unwrap()),
        );
        store
            .upsert_account(&AccountRecord {
                account_id: "owner".into(),
                kind: "user".into(),
                handle: "owner".into(),
                display_name: "Owner".into(),
                created_at: "1".into(),
                updated_at: "1".into(),
            })
            .unwrap();
        store
            .upsert_workspace(&WorkspaceRecord {
                workspace_id: "workspace-1".into(),
                owner_account_id: "owner".into(),
                display_name: "Enrichment".into(),
                state: "active".into(),
                created_at: "1".into(),
                updated_at: "1".into(),
            })
            .await
            .unwrap();
        let worker_ref = RuntimeWorkerRef::new("runtime-1", "worker-1");
        store
            .upsert_worker_registry(&WorkerRegistryRecord {
                workspace_id: "workspace-1".into(),
                worker: worker_ref.clone(),
                display_name: "Durable name".into(),
                profile: Some("builtin:coder".into()),
                retention_state: "normal".into(),
                transcript_ref: None,
                session_ref: None,
                summary_ref: None,
                diagnostics_ref: None,
                created_at: "1".into(),
                updated_at: "1".into(),
            })
            .unwrap();
        store
            .apply_worker_registry_observation("workspace-1", "runtime-1", &external_worker(), "2")
            .unwrap();
        let committed = store
            .worker_registry_projection("workspace-1", &worker_ref)
            .unwrap()
            .unwrap();
        // Change authority after capturing the payload, as can happen while async
        // enrichment is queued. It must not re-query and replace the committed Worker.
        let mut later = external_worker();
        later.state = protocol::subscription::SubscriptionWorkerState::Running;
        store
            .apply_worker_registry_observation("workspace-1", "runtime-1", &later, "3")
            .unwrap();
        let projected = project_registry_workers_async(
            store.clone(),
            "workspace-1".into(),
            vec![committed.clone()],
        )
        .await
        .unwrap();
        assert_eq!(projected.len(), 1);
        let worker = &projected[0];
        assert_eq!(
            worker.state,
            protocol::subscription::SubscriptionWorkerState::Idle
        );
        assert_eq!(
            worker.availability,
            protocol::subscription::SubscriptionWorkerAvailability::Observed
        );
        assert_eq!(worker.display_name.as_deref(), Some("Durable name"));
        assert_eq!(worker.profile.as_deref(), Some("builtin:coder"));
        assert_eq!(worker.runtime_id.as_deref(), Some("runtime-1"));
        assert_eq!(worker.workspace_id.as_deref(), Some("workspace-1"));
        assert_eq!(
            worker.workdir_attachments,
            external_worker().workdir_attachments
        );
        let mut unresolved = committed;
        unresolved
            .observation
            .as_mut()
            .unwrap()
            .worker
            .workdir_attachments[0]
            .repository_id = Some("missing-repository".into());
        assert!(
            project_registry_workers_async(store, "workspace-1".into(), vec![unresolved])
                .await
                .unwrap()
                .is_empty()
        );
    }

    async fn run_with_events(events: mpsc::Receiver<protocol::Event>) -> SubscriptionResponse {
        let (control_outbound, mut control_receiver) = mpsc::channel(1);
        let (protocol_outbound, _protocol_receiver) = mpsc::channel(1);
        run_worker_protocol(
            request_id(),
            subscription_id(),
            "runtime-1".to_string(),
            worker_id(),
            events,
            tokio::sync::watch::channel(None).1,
            control_outbound,
            protocol_outbound,
        )
        .await;
        let WsMessage::Text(text) = control_receiver.recv().await.unwrap() else {
            panic!("expected text response");
        };
        let frame: SubscriptionFrame = serde_json::from_str(text.as_str()).unwrap();
        let SubscriptionFramePayload::Response(response) = frame.payload else {
            panic!("expected subscription response");
        };
        response
    }

    #[tokio::test]
    async fn worker_bridge_overflow_is_serialized_as_lagged_not_resource_disappearance() {
        for (reason, expected) in [
            (
                WorkerProtocolEnd::EventOverflow,
                SubscriptionTerminationCode::Lagged,
            ),
            (
                WorkerProtocolEnd::MethodOverflow,
                SubscriptionTerminationCode::Lagged,
            ),
            (
                WorkerProtocolEnd::Disconnected,
                SubscriptionTerminationCode::ResourceGone,
            ),
        ] {
            let (event_sender, events) = mpsc::channel(1);
            event_sender
                .try_send(protocol::Event::Snapshot {
                    session: protocol::SessionSnapshot {
                        entries: vec![],
                        pending_submissions: Default::default(),
                    },
                    greeting: protocol::Greeting {
                        worker_name: "worker-1".into(),
                        cwd: String::new(),
                        provider: "fixture".into(),
                        model: "fixture".into(),
                        reasoning: None,
                        scope_summary: String::new(),
                        tools: vec![],
                        context_window: 0,
                        context_tokens: 0,
                        context_usage: None,
                    },
                    state: protocol::WorkerStateSnapshot::initial(),
                    in_flight: Default::default(),
                    internal_workers: vec![],
                })
                .unwrap();
            drop(event_sender);
            let (control, mut receiver) = mpsc::channel(2);
            let (protocol, _events) = mpsc::channel(1);
            run_worker_protocol(
                request_id(),
                subscription_id(),
                "runtime-1".into(),
                worker_id(),
                events,
                tokio::sync::watch::channel(Some(reason)).1,
                control,
                protocol,
            )
            .await;
            let WsMessage::Text(snapshot) = receiver.recv().await.unwrap() else {
                panic!("expected snapshot response");
            };
            let snapshot: SubscriptionFrame = serde_json::from_str(snapshot.as_str()).unwrap();
            assert!(matches!(
                snapshot.payload,
                SubscriptionFramePayload::Response(SubscriptionResponse::Subscribed { .. })
            ));
            let WsMessage::Text(closed) = receiver.recv().await.unwrap() else {
                panic!("expected terminal control frame");
            };
            let frame: SubscriptionFrame = serde_json::from_str(closed.as_str()).unwrap();
            frame.validate().unwrap();
            assert!(
                matches!(frame.payload, SubscriptionFramePayload::Event(SubscriptionEvent::SubscriptionClosed { code, message, .. })
                if code == expected && message.contains("unknown") && message.contains("fresh snapshot")),
                "{closed}"
            );
        }
    }

    #[tokio::test]
    async fn worker_protocol_rejects_a_non_snapshot_initial_event() {
        let (event_sender, events) = mpsc::channel(1);
        event_sender
            .send(protocol::Event::Error {
                code: protocol::ErrorCode::Internal,
                message: "not a snapshot".to_string(),
            })
            .await
            .unwrap();

        assert!(matches!(
            run_with_events(events).await,
            SubscriptionResponse::SubscriptionRejected {
                code: SubscriptionRejectionCode::Internal,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn worker_protocol_rejects_a_stream_closed_before_snapshot() {
        let (event_sender, events) = mpsc::channel(1);
        drop(event_sender);

        assert!(matches!(
            run_with_events(events).await,
            SubscriptionResponse::SubscriptionRejected {
                code: SubscriptionRejectionCode::ResourceNotFound,
                ..
            }
        ));
    }
}
