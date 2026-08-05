use std::collections::{BTreeMap, HashMap, HashSet};

use axum::extract::ws::{Message as WsMessage, WebSocket};
use futures::{SinkExt, StreamExt};
use protocol::subscription::{
    EventSubscriptionSelector, SubscriptionEvent, SubscriptionEventPayload, SubscriptionFrame,
    SubscriptionFramePayload, SubscriptionId, SubscriptionRejectionCode, SubscriptionRequest,
    SubscriptionResponse, SubscriptionSnapshot, SubscriptionTerminationCode, SubscriptionWorker,
};
use tokio::sync::mpsc;
use worker_runtime::identity::RuntimeWorkerRef;

use crate::runtime_subscription::{BrokerSubscriptionEvent, RuntimeSubscriptionBroker};
use crate::server::{WorkspaceApi, connect_workspace_worker_protocol};

const OUTBOUND_CAPACITY: usize = 256;

struct ActiveSubscription {
    task: tokio::task::JoinHandle<()>,
    methods: Option<mpsc::Sender<protocol::Method>>,
}

pub(crate) async fn serve_workspace_subscription(api: WorkspaceApi, socket: WebSocket) {
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
                                match connect_workspace_worker_protocol(&api, &worker).await {
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
                        if methods.send(message.method).await.is_err() {
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
    broker: RuntimeSubscriptionBroker,
    request_id: protocol::subscription::SubscriptionRequestId,
    subscription_id: SubscriptionId,
    outbound: mpsc::Sender<WsMessage>,
) {
    let runtime_ids = broker.runtime_ids();
    let mut pending = runtime_ids.iter().cloned().collect::<HashSet<_>>();
    let (events, mut event_receiver) = mpsc::channel(OUTBOUND_CAPACITY);
    let mut upstreams = tokio::task::JoinSet::new();
    for runtime_id in runtime_ids {
        let Ok(mut subscription) =
            broker.subscribe(&runtime_id, EventSubscriptionSelector::RuntimeWorkers)
        else {
            pending.remove(&runtime_id);
            continue;
        };
        let sender = events.clone();
        upstreams.spawn(async move {
            while let Some(event) = subscription.recv().await {
                if sender.send((runtime_id.clone(), event)).await.is_err() {
                    break;
                }
            }
        });
    }
    drop(events);

    let mut workers = HashMap::<String, BTreeMap<String, SubscriptionWorker>>::new();
    while !pending.is_empty() {
        let Some((runtime_id, event)) = event_receiver.recv().await else {
            return;
        };
        match event {
            BrokerSubscriptionEvent::Snapshot { snapshot, .. } => {
                install_snapshot(&mut workers, &runtime_id, snapshot);
                pending.remove(&runtime_id);
            }
            BrokerSubscriptionEvent::Disconnected { .. }
            | BrokerSubscriptionEvent::Rejected { .. }
            | BrokerSubscriptionEvent::Closed { .. } => {
                pending.remove(&runtime_id);
            }
            BrokerSubscriptionEvent::Event { .. } => {}
        }
    }

    let mut revisions = HashMap::<RuntimeWorkerRef, u64>::new();
    let mut initial_workers = Vec::new();
    for (runtime_id, runtime) in &mut workers {
        for worker in runtime.values_mut() {
            let worker_ref = RuntimeWorkerRef::new(runtime_id, worker.worker_id.as_str());
            worker.subject_revision = next_revision(&mut revisions, &worker_ref);
            initial_workers.push(worker.clone());
        }
    }
    sort_workers(&mut initial_workers);
    if send_frame(
        &outbound,
        SubscriptionFrame::new(SubscriptionFramePayload::Response(
            SubscriptionResponse::Subscribed {
                request_id,
                subscription_id: subscription_id.clone(),
                selector: EventSubscriptionSelector::WorkspaceWorkers,
                snapshot_revision: 1,
                snapshot: SubscriptionSnapshot::Workers {
                    workers: initial_workers,
                },
            },
        )),
    )
    .await
    .is_err()
    {
        return;
    }

    while let Some((runtime_id, event)) = event_receiver.recv().await {
        match event {
            BrokerSubscriptionEvent::Snapshot { snapshot, .. } => {
                let removed = workers.remove(&runtime_id).unwrap_or_default();
                for worker in removed.values() {
                    let worker_ref = RuntimeWorkerRef::new(&runtime_id, worker.worker_id.as_str());
                    let revision = next_revision(&mut revisions, &worker_ref);
                    if send_event(
                        &outbound,
                        &subscription_id,
                        revision,
                        SubscriptionEventPayload::WorkerRemoved {
                            worker_id: worker.worker_id.clone(),
                            runtime_id: Some(runtime_id.clone()),
                        },
                    )
                    .await
                    .is_err()
                    {
                        return;
                    }
                }
                install_snapshot(&mut workers, &runtime_id, snapshot);
                if let Some(current) = workers.get_mut(&runtime_id) {
                    for worker in current.values_mut() {
                        let worker_ref =
                            RuntimeWorkerRef::new(&runtime_id, worker.worker_id.as_str());
                        let revision = next_revision(&mut revisions, &worker_ref);
                        worker.subject_revision = revision;
                        if send_event(
                            &outbound,
                            &subscription_id,
                            revision,
                            SubscriptionEventPayload::WorkerUpserted {
                                worker: worker.clone(),
                            },
                        )
                        .await
                        .is_err()
                        {
                            return;
                        }
                    }
                }
            }
            BrokerSubscriptionEvent::Event { payload, .. } => match payload {
                SubscriptionEventPayload::WorkerUpserted { mut worker } => {
                    worker.runtime_id = Some(runtime_id.clone());
                    let worker_ref = RuntimeWorkerRef::new(&runtime_id, worker.worker_id.as_str());
                    let revision = next_revision(&mut revisions, &worker_ref);
                    worker.subject_revision = revision;
                    workers
                        .entry(runtime_id)
                        .or_default()
                        .insert(worker.worker_id.to_string(), worker.clone());
                    if send_event(
                        &outbound,
                        &subscription_id,
                        revision,
                        SubscriptionEventPayload::WorkerUpserted { worker },
                    )
                    .await
                    .is_err()
                    {
                        return;
                    }
                }
                SubscriptionEventPayload::WorkerRemoved { worker_id, .. } => {
                    workers
                        .entry(runtime_id.clone())
                        .or_default()
                        .remove(worker_id.as_str());
                    let worker_ref = RuntimeWorkerRef::new(&runtime_id, worker_id.as_str());
                    let revision = next_revision(&mut revisions, &worker_ref);
                    if send_event(
                        &outbound,
                        &subscription_id,
                        revision,
                        SubscriptionEventPayload::WorkerRemoved {
                            worker_id,
                            runtime_id: Some(runtime_id),
                        },
                    )
                    .await
                    .is_err()
                    {
                        return;
                    }
                }
                _ => {}
            },
            BrokerSubscriptionEvent::Disconnected { .. } => {}
            BrokerSubscriptionEvent::Rejected { code, message, .. } => {
                let _ = send_frame(
                    &outbound,
                    SubscriptionFrame::new(SubscriptionFramePayload::Event(
                        SubscriptionEvent::SubscriptionClosed {
                            subscription_id: subscription_id.clone(),
                            code: rejection_termination(code),
                            message,
                        },
                    )),
                )
                .await;
                return;
            }
            BrokerSubscriptionEvent::Closed { code, message, .. } => {
                let _ = send_frame(
                    &outbound,
                    SubscriptionFrame::new(SubscriptionFramePayload::Event(
                        SubscriptionEvent::SubscriptionClosed {
                            subscription_id: subscription_id.clone(),
                            code,
                            message,
                        },
                    )),
                )
                .await;
                return;
            }
        }
    }
}

fn install_snapshot(
    workers: &mut HashMap<String, BTreeMap<String, SubscriptionWorker>>,
    runtime_id: &str,
    snapshot: SubscriptionSnapshot,
) {
    let SubscriptionSnapshot::Workers {
        workers: snapshot_workers,
    } = snapshot
    else {
        return;
    };
    let mut projected = BTreeMap::new();
    for mut worker in snapshot_workers {
        worker.runtime_id = Some(runtime_id.to_string());
        projected.insert(worker.worker_id.to_string(), worker);
    }
    workers.insert(runtime_id.to_string(), projected);
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

fn next_revision(revisions: &mut HashMap<RuntimeWorkerRef, u64>, worker: &RuntimeWorkerRef) -> u64 {
    let revision = revisions.entry(worker.clone()).or_insert(0);
    *revision = revision.saturating_add(1);
    *revision
}
fn sort_workers(workers: &mut [SubscriptionWorker]) {
    workers.sort_by(|left, right| {
        left.runtime_id
            .cmp(&right.runtime_id)
            .then_with(|| left.worker_id.cmp(&right.worker_id))
    });
}
fn rejection_termination(code: SubscriptionRejectionCode) -> SubscriptionTerminationCode {
    match code {
        SubscriptionRejectionCode::Unauthorized => SubscriptionTerminationCode::Unauthorized,
        SubscriptionRejectionCode::ResourceNotFound => SubscriptionTerminationCode::ResourceGone,
        _ => SubscriptionTerminationCode::ServerShutdown,
    }
}
