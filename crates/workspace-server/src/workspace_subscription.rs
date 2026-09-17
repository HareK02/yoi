use std::collections::HashMap;

use axum::extract::ws::{Message as WsMessage, WebSocket};
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
    WorkspaceApi, authorize_browser_worker_method, connect_workspace_worker_protocol,
};

const OUTBOUND_CAPACITY: usize = 256;

struct ActiveSubscription {
    task: tokio::task::JoinHandle<()>,
    methods: Option<mpsc::Sender<protocol::Method>>,
}

pub(crate) async fn serve_workspace_subscription(
    api: WorkspaceApi,
    socket: WebSocket,
    input_source: protocol::AuthenticatedInputSource,
) {
    let broker = api.runtime_subscription_broker().clone();
    let (mut socket_sender, mut socket_receiver) = socket.split();
    let (control_outbound, mut control_receiver) = mpsc::channel::<WsMessage>(OUTBOUND_CAPACITY);
    let (protocol_outbound, mut protocol_receiver) = mpsc::channel::<WsMessage>(OUTBOUND_CAPACITY);
    let writer = tokio::spawn(async move {
        loop {
            let message = tokio::select! {
                biased;
                message = control_receiver.recv() => message,
                message = protocol_receiver.recv() => message,
            };
            let Some(message) = message else { break };
            if socket_sender.send(message).await.is_err() {
                break;
            }
        }
    });
    let mut next_subscription_id = 1_u64;
    let mut subscriptions = HashMap::<SubscriptionId, ActiveSubscription>::new();

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
                subscriptions.retain(|_, subscription| !subscription.task.is_finished());
                match frame.payload {
                    SubscriptionFramePayload::Request(SubscriptionRequest::SubscribeEvents {
                        request_id,
                        selector,
                    }) => {
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
                                        task,
                                        methods: None,
                                    },
                                );
                            }
                            EventSubscriptionSelector::WorkerProtocol {
                                worker_id,
                                runtime_id: Some(runtime_id),
                            } => {
                                let worker = RuntimeWorkerRef::new(&runtime_id, worker_id.as_str());
                                match connect_workspace_worker_protocol(
                                    &api,
                                    &worker,
                                    Some(&input_source),
                                )
                                .await
                                {
                                    Ok(connection) => {
                                        let methods = connection.methods.clone();
                                        let task = tokio::spawn(run_worker_protocol(
                                            request_id,
                                            subscription_id.clone(),
                                            runtime_id,
                                            worker_id,
                                            connection.events,
                                            control_outbound.clone(),
                                            protocol_outbound.clone(),
                                        ));
                                        subscriptions.insert(
                                            subscription_id,
                                            ActiveSubscription {
                                                task,
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
                    SubscriptionFramePayload::Request(SubscriptionRequest::UnsubscribeEvents {
                        request_id,
                        subscription_id,
                    }) => {
                        if let Some(subscription) = subscriptions.remove(&subscription_id) {
                            subscription.task.abort();
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
                        let Ok(method) =
                            authorize_browser_worker_method(message.method, &input_source)
                        else {
                            break;
                        };
                        if methods.send(method).await.is_err() {
                            break;
                        }
                    }
                    SubscriptionFramePayload::Response(_) | SubscriptionFramePayload::Event(_) => {
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

    for (_, subscription) in subscriptions {
        subscription.task.abort();
    }
    drop(control_outbound);
    drop(protocol_outbound);
    let _ = writer.await;
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
    control_outbound: mpsc::Sender<WsMessage>,
    protocol_outbound: mpsc::Sender<WsMessage>,
) {
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
                snapshot_revision: 0,
                snapshot: SubscriptionSnapshot::WorkerProtocol {
                    worker_id: worker_id.clone(),
                    events: Vec::new(),
                },
            },
        )),
    )
    .await
    .is_err()
    {
        return;
    }
    let mut subject_revision = 0_u64;
    while let Some(event) = events.recv().await {
        subject_revision = subject_revision.saturating_add(1);
        let frame =
            SubscriptionFrame::new(SubscriptionFramePayload::Event(SubscriptionEvent::Event {
                subscription_id: subscription_id.clone(),
                subject_revision,
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
                code: SubscriptionTerminationCode::ResourceGone,
                message: "Worker protocol stream closed".to_string(),
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
    // Subscribe before reading so commits racing with the snapshot are replayed.
    let mut events = api.worker_projection.subscribe();
    let Ok((snapshot_revision, records)) = api.store.worker_registry_projection_snapshot(
        &api.config.workspace_id,
        api.config.max_records.max(1),
    ) else {
        let _ = send_rejected(
            &outbound,
            request_id,
            SubscriptionRejectionCode::Internal,
            "failed to read durable Worker projection".to_string(),
        )
        .await;
        return;
    };
    let mut workers = records
        .into_iter()
        .filter_map(project_registry_worker)
        .collect::<Vec<_>>();
    sort_workers(&mut workers);
    if send_frame(
        &outbound,
        SubscriptionFrame::new(SubscriptionFramePayload::Response(
            SubscriptionResponse::Subscribed {
                request_id,
                subscription_id: subscription_id.clone(),
                selector: EventSubscriptionSelector::WorkspaceWorkers,
                snapshot_revision,
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
        match events.recv().await {
            Ok(event) if event.revision <= snapshot_revision => continue,
            Ok(event) => {
                for change in event.changes {
                    let payload = match change {
                        crate::worker_projection::WorkerProjectionChange::Upsert(record) => {
                            let Some(mut worker) = project_registry_worker(record) else {
                                continue;
                            };
                            worker.subject_revision = event.revision;
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
                    if send_event(&outbound, &subscription_id, event.revision, payload)
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

fn project_registry_worker(
    record: crate::store::WorkerRegistryProjectionRecord,
) -> Option<SubscriptionWorker> {
    let runtime_id = record.registry.worker.runtime_id.clone();
    let mut worker = if let Some(observation) = record.observation {
        let mut worker = observation.worker;
        worker.availability = observation.availability;
        worker.subject_revision = observation.projection_revision;
        worker
    } else {
        SubscriptionWorker {
            worker_id: protocol::subscription::SubscriptionWorkerId::new(
                record.registry.worker.worker_id.clone(),
            )
            .ok()?,
            runtime_id: Some(runtime_id.clone()),
            resource_key: record.resource_key.clone(),
            availability: protocol::subscription::SubscriptionWorkerAvailability::Unavailable,
            subject_revision: 0,
            worker_state: None,
            state: protocol::subscription::SubscriptionWorkerState::Stopped,
            has_running_internal_workers: false,
            workspace_id: Some(record.registry.workspace_id.clone()),
            display_name: Some(record.registry.display_name.clone()),
            profile: record.registry.profile.clone(),
            repository_id: None,
            repository_key: None,
            working_directory_id: None,
        }
    };
    worker.runtime_id = Some(runtime_id);
    worker.resource_key = record.resource_key;
    worker.workspace_id = Some(record.registry.workspace_id);
    worker.display_name = Some(record.registry.display_name);
    worker.profile = record.registry.profile;
    Some(worker)
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
    subject_revision: u64,
    payload: SubscriptionEventPayload,
) -> Result<(), ()> {
    send_frame(
        outbound,
        SubscriptionFrame::new(SubscriptionFramePayload::Event(SubscriptionEvent::Event {
            subscription_id: subscription_id.clone(),
            subject_revision,
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
