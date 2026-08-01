use std::collections::{BTreeMap, HashMap, HashSet};

use axum::extract::ws::{Message as WsMessage, WebSocket};
use futures::{SinkExt, StreamExt};
use protocol::subscription::{
    EventSubscriptionSelector, SubscriptionEvent, SubscriptionEventPayload, SubscriptionFrame,
    SubscriptionFramePayload, SubscriptionId, SubscriptionRejectionCode, SubscriptionRequest,
    SubscriptionResponse, SubscriptionSnapshot, SubscriptionTerminationCode, SubscriptionWorker,
};
use tokio::sync::mpsc;

use crate::runtime_subscription::{BrokerSubscriptionEvent, RuntimeSubscriptionBroker};

const OUTBOUND_CAPACITY: usize = 256;

pub(crate) async fn serve_workspace_subscription(
    broker: RuntimeSubscriptionBroker,
    socket: WebSocket,
) {
    let (mut socket_sender, mut socket_receiver) = socket.split();
    let (outbound, mut outbound_receiver) = mpsc::channel::<WsMessage>(OUTBOUND_CAPACITY);
    let writer = tokio::spawn(async move {
        while let Some(message) = outbound_receiver.recv().await {
            if socket_sender.send(message).await.is_err() {
                break;
            }
        }
    });
    let mut next_subscription_id = 1_u64;
    let mut subscriptions = HashMap::<SubscriptionId, tokio::task::JoinHandle<()>>::new();

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
                let SubscriptionFramePayload::Request(request) = frame.payload else {
                    break;
                };
                subscriptions.retain(|_, task| !task.is_finished());
                match request {
                    SubscriptionRequest::SubscribeEvents {
                        request_id,
                        selector,
                    } => {
                        if selector != EventSubscriptionSelector::WorkspaceWorkers {
                            let _ = send_frame(&outbound, SubscriptionFrame::new(
                                SubscriptionFramePayload::Response(
                                    SubscriptionResponse::SubscriptionRejected {
                                        request_id,
                                        subscription_id: None,
                                        code: SubscriptionRejectionCode::UnsupportedSelector,
                                        message: "Workspace clients may subscribe only to workspace_workers on this endpoint".to_string(),
                                    },
                                ),
                            )).await;
                            continue;
                        }
                        let subscription_id = SubscriptionId::new(format!(
                            "workspace-subscription-{next_subscription_id}"
                        ))
                        .expect("generated Workspace subscription id is valid");
                        next_subscription_id = next_subscription_id.saturating_add(1);
                        let task = tokio::spawn(run_workspace_workers(
                            broker.clone(),
                            request_id,
                            subscription_id.clone(),
                            outbound.clone(),
                        ));
                        subscriptions.insert(subscription_id, task);
                    }
                    SubscriptionRequest::UnsubscribeEvents {
                        request_id,
                        subscription_id,
                    } => {
                        if let Some(task) = subscriptions.remove(&subscription_id) {
                            task.abort();
                        }
                        if send_frame(
                            &outbound,
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
                }
            }
            WsMessage::Ping(value) => {
                if outbound.send(WsMessage::Pong(value)).await.is_err() {
                    break;
                }
            }
            WsMessage::Pong(_) => {}
            WsMessage::Close(_) | WsMessage::Binary(_) => break,
        }
    }

    for (_, task) in subscriptions {
        task.abort();
    }
    drop(outbound);
    let _ = writer.await;
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

    let mut revisions = HashMap::<String, u64>::new();
    let mut initial_workers = workers
        .values_mut()
        .flat_map(|runtime| runtime.values_mut())
        .map(|worker| {
            let key = worker_key(worker.runtime_id.as_deref(), worker.worker_id.as_str());
            worker.subject_revision = next_revision(&mut revisions, &key);
            worker.clone()
        })
        .collect::<Vec<_>>();
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
                    let key = worker_key(Some(&runtime_id), worker.worker_id.as_str());
                    let revision = next_revision(&mut revisions, &key);
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
                        let key = worker_key(Some(&runtime_id), worker.worker_id.as_str());
                        let revision = next_revision(&mut revisions, &key);
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
                    let key = worker_key(Some(&runtime_id), worker.worker_id.as_str());
                    let revision = next_revision(&mut revisions, &key);
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
                    let key = worker_key(Some(&runtime_id), worker_id.as_str());
                    let revision = next_revision(&mut revisions, &key);
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

fn next_revision(revisions: &mut HashMap<String, u64>, key: &str) -> u64 {
    let revision = revisions.entry(key.to_string()).or_insert(0);
    *revision = revision.saturating_add(1);
    *revision
}
fn worker_key(runtime_id: Option<&str>, worker_id: &str) -> String {
    format!("{}:{worker_id}", runtime_id.unwrap_or_default())
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
