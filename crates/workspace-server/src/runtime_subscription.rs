use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use protocol::subscription::{
    EventSubscriptionSelector, SubscriptionEvent, SubscriptionEventPayload, SubscriptionFrame,
    SubscriptionFramePayload, SubscriptionId, SubscriptionRejectionCode, SubscriptionRequest,
    SubscriptionRequestId, SubscriptionResponse, SubscriptionSnapshot, SubscriptionTerminationCode,
};
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use worker_runtime::auth::{CapabilityTokenSigner, capability_claims};

use crate::hosts::RemoteRuntimeConfig;

const DOWNSTREAM_QUEUE_CAPACITY: usize = 256;
const RECONNECT_DELAY: Duration = Duration::from_millis(100);

type RuntimeSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

#[derive(Debug, thiserror::Error)]
pub enum RuntimeSubscriptionBrokerError {
    #[error("unknown Runtime {0:?}")]
    UnknownRuntime(String),
    #[error("Runtime subscription broker command channel closed")]
    Closed,
}

#[derive(Clone, Debug)]
pub enum BrokerSubscriptionEvent {
    Snapshot {
        connection_generation: u64,
        snapshot_revision: u64,
        snapshot: SubscriptionSnapshot,
    },
    Event {
        connection_generation: u64,
        subject_revision: u64,
        payload: SubscriptionEventPayload,
    },
    Disconnected {
        connection_generation: u64,
        message: String,
    },
    Rejected {
        connection_generation: u64,
        code: SubscriptionRejectionCode,
        message: String,
    },
    Closed {
        connection_generation: u64,
        code: SubscriptionTerminationCode,
        message: String,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RuntimeSubscriptionBrokerStatus {
    pub connection_generation: u64,
    pub connected: bool,
    pub desired_selectors: usize,
    pub upstream_subscriptions: usize,
}

pub struct BrokerSubscription {
    downstream_id: u64,
    runtime_id: String,
    selector: EventSubscriptionSelector,
    receiver: mpsc::Receiver<BrokerSubscriptionEvent>,
    commands: mpsc::UnboundedSender<Command>,
}

impl BrokerSubscription {
    pub fn runtime_id(&self) -> &str {
        &self.runtime_id
    }
    pub fn selector(&self) -> &EventSubscriptionSelector {
        &self.selector
    }
    pub async fn recv(&mut self) -> Option<BrokerSubscriptionEvent> {
        self.receiver.recv().await
    }
}

impl Drop for BrokerSubscription {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Unsubscribe(self.downstream_id));
    }
}

#[derive(Clone)]
struct Registration {
    generation: u64,
    commands: mpsc::UnboundedSender<Command>,
    status: Arc<RwLock<RuntimeSubscriptionBrokerStatus>>,
}

#[derive(Clone)]
pub struct RuntimeSubscriptionBroker {
    workspace_id: Arc<str>,
    next_generation: Arc<AtomicU64>,
    next_downstream: Arc<AtomicU64>,
    registrations: Arc<RwLock<HashMap<String, Registration>>>,
}

impl RuntimeSubscriptionBroker {
    pub fn new(workspace_id: impl Into<String>) -> Self {
        Self {
            workspace_id: Arc::from(workspace_id.into()),
            next_generation: Arc::new(AtomicU64::new(1)),
            next_downstream: Arc::new(AtomicU64::new(1)),
            registrations: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn register_remote_runtime(&self, config: RemoteRuntimeConfig) -> u64 {
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        let (commands, receiver) = mpsc::unbounded_channel();
        let status = Arc::new(RwLock::new(RuntimeSubscriptionBrokerStatus {
            connection_generation: generation,
            ..Default::default()
        }));
        let registration = Registration {
            generation,
            commands: commands.clone(),
            status: status.clone(),
        };
        let previous = self
            .registrations
            .write()
            .expect("broker registry poisoned")
            .insert(config.runtime_id.clone(), registration);
        if let Some(previous) = previous {
            let _ = previous.commands.send(Command::Shutdown(generation));
        }
        tokio::spawn(run_connection(
            config,
            self.workspace_id.to_string(),
            generation,
            receiver,
            status,
        ));
        generation
    }

    pub fn register_embedded_runtime(
        &self,
        runtime_id: impl Into<String>,
        runtime: worker_runtime::Runtime,
    ) -> u64 {
        let runtime_id = runtime_id.into();
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        let (commands, receiver) = mpsc::unbounded_channel();
        let status = Arc::new(RwLock::new(RuntimeSubscriptionBrokerStatus {
            connection_generation: generation,
            connected: true,
            ..Default::default()
        }));
        let previous = self
            .registrations
            .write()
            .expect("broker registry poisoned")
            .insert(
                runtime_id.clone(),
                Registration {
                    generation,
                    commands: commands.clone(),
                    status: status.clone(),
                },
            );
        if let Some(previous) = previous {
            let _ = previous.commands.send(Command::Shutdown(generation));
        }
        tokio::spawn(run_embedded_connection(
            runtime_id,
            runtime,
            self.workspace_id.to_string(),
            generation,
            receiver,
            status,
        ));
        generation
    }

    pub fn unregister_runtime(&self, runtime_id: &str) {
        if let Some(registration) = self
            .registrations
            .write()
            .expect("broker registry poisoned")
            .remove(runtime_id)
        {
            let _ = registration
                .commands
                .send(Command::Shutdown(registration.generation.saturating_add(1)));
        }
    }

    pub fn runtime_ids(&self) -> Vec<String> {
        let mut runtime_ids = self
            .registrations
            .read()
            .expect("broker registry poisoned")
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        runtime_ids.sort();
        runtime_ids
    }

    pub fn status(&self, runtime_id: &str) -> Option<RuntimeSubscriptionBrokerStatus> {
        let status = self
            .registrations
            .read()
            .expect("broker registry poisoned")
            .get(runtime_id)?
            .status
            .clone();
        Some(status.read().expect("broker status poisoned").clone())
    }

    pub fn subscribe(
        &self,
        runtime_id: &str,
        selector: EventSubscriptionSelector,
    ) -> Result<BrokerSubscription, RuntimeSubscriptionBrokerError> {
        selector
            .validate()
            .map_err(|_| RuntimeSubscriptionBrokerError::Closed)?;
        let registration = self
            .registrations
            .read()
            .expect("broker registry poisoned")
            .get(runtime_id)
            .cloned()
            .ok_or_else(|| {
                RuntimeSubscriptionBrokerError::UnknownRuntime(runtime_id.to_string())
            })?;
        let downstream_id = self.next_downstream.fetch_add(1, Ordering::Relaxed);
        let (events, receiver) = mpsc::channel(DOWNSTREAM_QUEUE_CAPACITY);
        let initial_events = events.clone();
        registration
            .commands
            .send(Command::Subscribe {
                downstream_id,
                selector: selector.clone(),
                events,
            })
            .map_err(|_| RuntimeSubscriptionBrokerError::Closed)?;
        let initial_status = registration
            .status
            .read()
            .expect("broker status poisoned")
            .clone();
        if !initial_status.connected {
            let _ = initial_events.try_send(BrokerSubscriptionEvent::Disconnected {
                connection_generation: initial_status.connection_generation,
                message: "Runtime subscription connection is not currently available".to_string(),
            });
        }
        Ok(BrokerSubscription {
            downstream_id,
            runtime_id: runtime_id.to_string(),
            selector,
            receiver,
            commands: registration.commands,
        })
    }
}

#[derive(Debug)]
enum Command {
    Subscribe {
        downstream_id: u64,
        selector: EventSubscriptionSelector,
        events: mpsc::Sender<BrokerSubscriptionEvent>,
    },
    Unsubscribe(u64),
    Shutdown(u64),
}

struct SelectorState {
    downstreams: HashMap<u64, mpsc::Sender<BrokerSubscriptionEvent>>,
    upstream_id: Option<SubscriptionId>,
    pending: bool,
    snapshot: Option<(u64, SubscriptionSnapshot)>,
    revisions: HashMap<String, u64>,
}
impl SelectorState {
    fn new() -> Self {
        Self {
            downstreams: HashMap::new(),
            upstream_id: None,
            pending: false,
            snapshot: None,
            revisions: HashMap::new(),
        }
    }
}

struct State {
    runtime_id: String,
    generation: u64,
    next_request: u64,
    selectors: HashMap<EventSubscriptionSelector, SelectorState>,
    downstream_index: HashMap<u64, EventSubscriptionSelector>,
    pending: HashMap<SubscriptionRequestId, EventSubscriptionSelector>,
    upstream_index: HashMap<SubscriptionId, EventSubscriptionSelector>,
}
impl State {
    fn new(runtime_id: String, generation: u64) -> Self {
        Self {
            runtime_id,
            generation,
            next_request: 1,
            selectors: HashMap::new(),
            downstream_index: HashMap::new(),
            pending: HashMap::new(),
            upstream_index: HashMap::new(),
        }
    }
    fn request_id(&mut self) -> SubscriptionRequestId {
        let id = self.next_request;
        self.next_request = self.next_request.saturating_add(1);
        SubscriptionRequestId::new(format!("server-{}-{id}", self.generation)).unwrap()
    }
    fn disconnected(&mut self, message: String) {
        self.pending.clear();
        self.upstream_index.clear();
        for selector in self.selectors.values_mut() {
            selector.upstream_id = None;
            selector.pending = false;
            selector.snapshot = None;
            selector.revisions.clear();
            broadcast(
                &mut selector.downstreams,
                BrokerSubscriptionEvent::Disconnected {
                    connection_generation: self.generation,
                    message: message.clone(),
                },
            );
        }
    }
}

struct EmbeddedEntry {
    downstreams: HashMap<u64, mpsc::Sender<BrokerSubscriptionEvent>>,
    snapshot_revision: u64,
    snapshot: SubscriptionSnapshot,
    task: tokio::task::JoinHandle<()>,
}

async fn run_embedded_connection(
    runtime_id: String,
    runtime: worker_runtime::Runtime,
    _workspace_id: String,
    generation: u64,
    mut commands: mpsc::UnboundedReceiver<Command>,
    status: Arc<RwLock<RuntimeSubscriptionBrokerStatus>>,
) {
    let (updates, mut update_receiver) = mpsc::unbounded_channel();
    let mut entries = HashMap::<EventSubscriptionSelector, EmbeddedEntry>::new();
    let mut downstream_index = HashMap::<u64, EventSubscriptionSelector>::new();
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                Some(Command::Subscribe { downstream_id, selector, events }) => {
                    downstream_index.insert(downstream_id, selector.clone());
                    if let Some(entry) = entries.get_mut(&selector) {
                        let _ = events.try_send(BrokerSubscriptionEvent::Snapshot {
                            connection_generation: generation,
                            snapshot_revision: entry.snapshot_revision,
                            snapshot: entry.snapshot.clone(),
                        });
                        entry.downstreams.insert(downstream_id, events);
                    } else {
                        match runtime.subscribe_event_selector(selector.clone()) {
                            Ok(mut subscription) => {
                                let snapshot_revision = subscription.snapshot_revision();
                                let snapshot = project_snapshot_runtime(subscription.snapshot().clone(), &runtime_id);
                                let _ = events.try_send(BrokerSubscriptionEvent::Snapshot { connection_generation: generation, snapshot_revision, snapshot: snapshot.clone() });
                                let sender = updates.clone();
                                let task_selector = selector.clone();
                                let task_runtime_id = runtime_id.clone();
                                let task = tokio::spawn(async move {
                                    while let Ok(update) = subscription.recv().await {
                                        let payload = project_payload_runtime(update.payload, &task_runtime_id);
                                        if sender.send((task_selector.clone(), update.subject_revision, payload)).is_err() { break; }
                                    }
                                });
                                entries.insert(selector, EmbeddedEntry { downstreams: HashMap::from([(downstream_id, events)]), snapshot_revision, snapshot, task });
                            }
                            Err(error) => {
                                let _ = events.try_send(BrokerSubscriptionEvent::Rejected { connection_generation: generation, code: SubscriptionRejectionCode::UnsupportedSelector, message: error.to_string() });
                            }
                        }
                    }
                }
                Some(Command::Unsubscribe(id)) => {
                    if let Some(selector) = downstream_index.remove(&id) {
                        let empty = entries.get_mut(&selector).is_some_and(|entry| { entry.downstreams.remove(&id); entry.downstreams.is_empty() });
                        if empty { if let Some(entry) = entries.remove(&selector) { entry.task.abort(); } }
                    }
                }
                Some(Command::Shutdown(replacement)) => {
                    for entry in entries.values_mut() {
                        broadcast(&mut entry.downstreams, BrokerSubscriptionEvent::Closed { connection_generation: generation, code: SubscriptionTerminationCode::ServerShutdown, message: format!("embedded Runtime generation {generation} was fenced by {replacement}") });
                        entry.task.abort();
                    }
                    return;
                }
                None => return,
            },
            update = update_receiver.recv() => {
                let Some((selector, subject_revision, payload)) = update else { return; };
                if let Some(entry) = entries.get_mut(&selector) {
                    entry.snapshot_revision = entry.snapshot_revision.saturating_add(1);
                    apply_event_to_cached_snapshot(&mut entry.snapshot, &payload);
                    broadcast(&mut entry.downstreams, BrokerSubscriptionEvent::Event { connection_generation: generation, subject_revision, payload });
                }
            }
        }
        *status.write().expect("broker status poisoned") = RuntimeSubscriptionBrokerStatus {
            connection_generation: generation,
            connected: true,
            desired_selectors: entries.len(),
            upstream_subscriptions: entries.len(),
        };
    }
}

fn apply_event_to_cached_snapshot(
    snapshot: &mut SubscriptionSnapshot,
    payload: &SubscriptionEventPayload,
) {
    let SubscriptionSnapshot::Workers { workers } = snapshot else {
        return;
    };
    match payload {
        SubscriptionEventPayload::WorkerUpserted { worker } => {
            if let Some(existing) = workers
                .iter_mut()
                .find(|existing| existing.worker_id == worker.worker_id)
            {
                *existing = worker.clone();
            } else {
                workers.push(worker.clone());
            }
        }
        SubscriptionEventPayload::WorkerRemoved { worker_id, .. } => {
            workers.retain(|worker| worker.worker_id != *worker_id);
        }
        _ => {}
    }
}

fn project_snapshot_runtime(
    mut snapshot: SubscriptionSnapshot,
    runtime_id: &str,
) -> SubscriptionSnapshot {
    if let SubscriptionSnapshot::Workers { workers } = &mut snapshot {
        for worker in workers {
            worker.runtime_id = Some(runtime_id.to_string());
        }
    }
    snapshot
}

fn project_payload_runtime(
    mut payload: SubscriptionEventPayload,
    runtime_id: &str,
) -> SubscriptionEventPayload {
    match &mut payload {
        SubscriptionEventPayload::WorkerUpserted { worker } => {
            worker.runtime_id = Some(runtime_id.to_string());
        }
        SubscriptionEventPayload::WorkerRemoved {
            runtime_id: projected_runtime_id,
            ..
        } => {
            *projected_runtime_id = Some(runtime_id.to_string());
        }
        _ => {}
    }
    payload
}

async fn run_connection(
    config: RemoteRuntimeConfig,
    workspace_id: String,
    generation: u64,
    mut commands: mpsc::UnboundedReceiver<Command>,
    status: Arc<RwLock<RuntimeSubscriptionBrokerStatus>>,
) {
    let mut state = State::new(config.runtime_id.clone(), generation);
    let mut disconnect_notified = false;
    loop {
        update_status(&status, &state, false);
        let connecting = connect_runtime(&config, &workspace_id);
        tokio::pin!(connecting);
        let connection = loop {
            tokio::select! {
                command = commands.recv() => match command {
                    Some(Command::Shutdown(replacement)) => { close_all(&mut state, replacement); return; }
                    Some(command) => { apply_offline(&mut state, command); update_status(&status, &state, false); }
                    None => return,
                },
                connected = &mut connecting => break connected,
            }
        };
        let mut socket = match connection {
            Ok(socket) => socket,
            Err(error) => {
                if !disconnect_notified {
                    state.disconnected(error);
                    disconnect_notified = true;
                }
                tokio::time::sleep(RECONNECT_DELAY).await;
                continue;
            }
        };
        if resubscribe_all(&mut socket, &mut state).await.is_err() {
            state.disconnected("failed to restore Runtime subscriptions".into());
            disconnect_notified = true;
            tokio::time::sleep(RECONNECT_DELAY).await;
            continue;
        }
        update_status(&status, &state, true);
        let reason = loop {
            tokio::select! {
                command = commands.recv() => match command {
                    Some(Command::Shutdown(replacement)) => { let _ = socket.close(None).await; close_all(&mut state, replacement); return; }
                    Some(command) => if apply_online(&mut socket, &mut state, command).await.is_err() { break "failed to apply Runtime subscription command".into(); },
                    None => return,
                },
                message = socket.next() => match message {
                    Some(Ok(Message::Text(text))) => match serde_json::from_str::<SubscriptionFrame>(text.as_str()) {
                        Ok(frame) if frame.validate().is_ok() => if handle_frame(&mut socket, &mut state, frame).await.is_err() { break "invalid Runtime subscription transition".into(); },
                        _ => break "Runtime returned an invalid subscription frame".into(),
                    },
                    Some(Ok(Message::Ping(value))) => if socket.send(Message::Pong(value)).await.is_err() { break "Runtime pong failed".into(); },
                    Some(Ok(Message::Pong(_))) => {},
                    Some(Ok(Message::Close(_))) | None => break "Runtime subscription connection closed".into(),
                    Some(Ok(Message::Binary(_) | Message::Frame(_))) => break "Runtime returned a non-text subscription frame".into(),
                    Some(Err(error)) => break format!("Runtime subscription connection failed: {error}"),
                }
            }
            update_status(&status, &state, true);
        };
        state.disconnected(reason);
        disconnect_notified = true;
        update_status(&status, &state, false);
        tokio::time::sleep(RECONNECT_DELAY).await;
    }
}

fn apply_offline(state: &mut State, command: Command) {
    match command {
        Command::Subscribe {
            downstream_id,
            selector,
            events,
        } => {
            state
                .downstream_index
                .insert(downstream_id, selector.clone());
            state
                .selectors
                .entry(selector)
                .or_insert_with(SelectorState::new)
                .downstreams
                .insert(downstream_id, events);
        }
        Command::Unsubscribe(id) => remove_downstream(state, id),
        Command::Shutdown(_) => unreachable!(),
    }
}

async fn apply_online(
    socket: &mut RuntimeSocket,
    state: &mut State,
    command: Command,
) -> Result<(), ()> {
    match command {
        Command::Subscribe {
            downstream_id,
            selector,
            events,
        } => {
            state
                .downstream_index
                .insert(downstream_id, selector.clone());
            let entry = state
                .selectors
                .entry(selector.clone())
                .or_insert_with(SelectorState::new);
            if let Some((revision, snapshot)) = &entry.snapshot {
                let _ = events.try_send(BrokerSubscriptionEvent::Snapshot {
                    connection_generation: state.generation,
                    snapshot_revision: *revision,
                    snapshot: snapshot.clone(),
                });
            }
            entry.downstreams.insert(downstream_id, events);
            if entry.upstream_id.is_none() && !entry.pending {
                send_subscribe(socket, state, selector).await?;
            }
        }
        Command::Unsubscribe(id) => {
            let selector = state.downstream_index.get(&id).cloned();
            remove_downstream(state, id);
            if let Some(selector) = selector {
                maybe_unsubscribe(socket, state, selector).await?;
            }
        }
        Command::Shutdown(_) => unreachable!(),
    }
    Ok(())
}

async fn resubscribe_all(socket: &mut RuntimeSocket, state: &mut State) -> Result<(), ()> {
    let selectors = state
        .selectors
        .iter()
        .filter(|(_, value)| !value.downstreams.is_empty())
        .map(|(key, _)| key.clone())
        .collect::<Vec<_>>();
    for selector in selectors {
        send_subscribe(socket, state, selector).await?;
    }
    Ok(())
}

async fn send_subscribe(
    socket: &mut RuntimeSocket,
    state: &mut State,
    selector: EventSubscriptionSelector,
) -> Result<(), ()> {
    let request_id = state.request_id();
    send_frame(
        socket,
        SubscriptionFrame::new(SubscriptionFramePayload::Request(
            SubscriptionRequest::SubscribeEvents {
                request_id: request_id.clone(),
                selector: selector.clone(),
            },
        )),
    )
    .await?;
    state.pending.insert(request_id, selector.clone());
    state.selectors.get_mut(&selector).unwrap().pending = true;
    Ok(())
}

async fn maybe_unsubscribe(
    socket: &mut RuntimeSocket,
    state: &mut State,
    selector: EventSubscriptionSelector,
) -> Result<(), ()> {
    let Some(entry) = state.selectors.get(&selector) else {
        return Ok(());
    };
    if !entry.downstreams.is_empty() {
        return Ok(());
    }
    if let Some(subscription_id) = entry.upstream_id.clone() {
        let request_id = state.request_id();
        send_frame(
            socket,
            SubscriptionFrame::new(SubscriptionFramePayload::Request(
                SubscriptionRequest::UnsubscribeEvents {
                    request_id,
                    subscription_id: subscription_id.clone(),
                },
            )),
        )
        .await?;
        state.upstream_index.remove(&subscription_id);
        state.selectors.remove(&selector);
    } else if !entry.pending {
        state.selectors.remove(&selector);
    }
    Ok(())
}

async fn handle_frame(
    socket: &mut RuntimeSocket,
    state: &mut State,
    frame: SubscriptionFrame,
) -> Result<(), ()> {
    match frame.payload {
        SubscriptionFramePayload::Response(SubscriptionResponse::Subscribed {
            request_id,
            subscription_id,
            selector,
            snapshot_revision,
            snapshot,
        }) => {
            if state.pending.remove(&request_id) != Some(selector.clone()) {
                return Err(());
            }
            let snapshot = project_snapshot_runtime(snapshot, &state.runtime_id);
            let entry = state.selectors.get_mut(&selector).ok_or(())?;
            entry.pending = false;
            entry.upstream_id = Some(subscription_id.clone());
            entry.snapshot = Some((snapshot_revision, snapshot.clone()));
            entry.revisions = snapshot_revisions(&snapshot);
            state
                .upstream_index
                .insert(subscription_id, selector.clone());
            broadcast(
                &mut entry.downstreams,
                BrokerSubscriptionEvent::Snapshot {
                    connection_generation: state.generation,
                    snapshot_revision,
                    snapshot,
                },
            );
            if entry.downstreams.is_empty() {
                maybe_unsubscribe(socket, state, selector).await?;
            }
        }
        SubscriptionFramePayload::Response(SubscriptionResponse::Unsubscribed { .. }) => {}
        SubscriptionFramePayload::Response(SubscriptionResponse::SubscriptionRejected {
            request_id,
            code,
            message,
            ..
        }) => {
            if let Some(selector) = state.pending.remove(&request_id) {
                if let Some(mut entry) = state.selectors.remove(&selector) {
                    broadcast(
                        &mut entry.downstreams,
                        BrokerSubscriptionEvent::Rejected {
                            connection_generation: state.generation,
                            code,
                            message,
                        },
                    );
                }
            }
        }
        SubscriptionFramePayload::Event(SubscriptionEvent::Event {
            subscription_id,
            subject_revision,
            payload,
        }) => {
            let selector = state
                .upstream_index
                .get(&subscription_id)
                .cloned()
                .ok_or(())?;
            payload.validate_for_selector(&selector).map_err(|_| ())?;
            let payload = project_payload_runtime(payload, &state.runtime_id);
            let entry = state.selectors.get_mut(&selector).ok_or(())?;
            if let Some(subject) = event_subject(&payload) {
                let revision = entry.revisions.entry(subject).or_insert(0);
                if subject_revision <= *revision {
                    return Ok(());
                }
                *revision = subject_revision;
            }
            if let Some((snapshot_revision, snapshot)) = entry.snapshot.as_mut() {
                *snapshot_revision = snapshot_revision.saturating_add(1);
                apply_event_to_cached_snapshot(snapshot, &payload);
            }
            broadcast(
                &mut entry.downstreams,
                BrokerSubscriptionEvent::Event {
                    connection_generation: state.generation,
                    subject_revision,
                    payload,
                },
            );
        }
        SubscriptionFramePayload::Event(SubscriptionEvent::SubscriptionClosed {
            subscription_id,
            code,
            message,
        }) => {
            let selector = state.upstream_index.remove(&subscription_id).ok_or(())?;
            let should_resubscribe = if let Some(entry) = state.selectors.get_mut(&selector) {
                entry.upstream_id = None;
                entry.snapshot = None;
                entry.revisions.clear();
                broadcast(
                    &mut entry.downstreams,
                    BrokerSubscriptionEvent::Closed {
                        connection_generation: state.generation,
                        code,
                        message,
                    },
                );
                !entry.downstreams.is_empty()
            } else {
                false
            };
            if should_resubscribe {
                send_subscribe(socket, state, selector).await?;
            }
        }
        SubscriptionFramePayload::Request(_) | SubscriptionFramePayload::WorkerProtocol(_) => {
            return Err(());
        }
    }
    Ok(())
}

fn remove_downstream(state: &mut State, id: u64) {
    if let Some(selector) = state.downstream_index.remove(&id) {
        if let Some(entry) = state.selectors.get_mut(&selector) {
            entry.downstreams.remove(&id);
        }
    }
}

fn close_all(state: &mut State, replacement: u64) {
    for entry in state.selectors.values_mut() {
        broadcast(
            &mut entry.downstreams,
            BrokerSubscriptionEvent::Closed {
                connection_generation: state.generation,
                code: SubscriptionTerminationCode::ServerShutdown,
                message: format!(
                    "Runtime connection generation {} was fenced by generation {replacement}",
                    state.generation
                ),
            },
        );
    }
}

fn broadcast(
    downstreams: &mut HashMap<u64, mpsc::Sender<BrokerSubscriptionEvent>>,
    event: BrokerSubscriptionEvent,
) {
    let mut closed = HashSet::new();
    for (id, sender) in downstreams.iter() {
        if sender.try_send(event.clone()).is_err() {
            closed.insert(*id);
        }
    }
    downstreams.retain(|id, _| !closed.contains(id));
}

fn snapshot_revisions(snapshot: &SubscriptionSnapshot) -> HashMap<String, u64> {
    match snapshot {
        SubscriptionSnapshot::Workers { workers } => workers
            .iter()
            .map(|worker| {
                (
                    format!(
                        "{}:{}",
                        worker.runtime_id.as_deref().unwrap_or_default(),
                        worker.worker_id
                    ),
                    worker.subject_revision,
                )
            })
            .collect(),
        SubscriptionSnapshot::WorkerProtocol { worker_id, .. } => {
            HashMap::from([(worker_id.to_string(), 0)])
        }
        SubscriptionSnapshot::WorkspaceWorkdirs { .. } => HashMap::new(),
    }
}
fn event_subject(payload: &SubscriptionEventPayload) -> Option<String> {
    Some(match payload {
        SubscriptionEventPayload::WorkerUpserted { worker } => format!(
            "{}:{}",
            worker.runtime_id.as_deref().unwrap_or_default(),
            worker.worker_id
        ),
        SubscriptionEventPayload::WorkerRemoved {
            worker_id,
            runtime_id,
        } => format!(
            "{}:{}",
            runtime_id.as_deref().unwrap_or_default(),
            worker_id
        ),
        SubscriptionEventPayload::WorkerProtocol { worker_id, .. } => worker_id.to_string(),
        SubscriptionEventPayload::WorkdirUpserted { workdir } => {
            workdir.working_directory_id.to_string()
        }
        SubscriptionEventPayload::WorkdirRemoved {
            working_directory_id,
        } => working_directory_id.to_string(),
    })
}
async fn send_frame(socket: &mut RuntimeSocket, frame: SubscriptionFrame) -> Result<(), ()> {
    frame.validate().map_err(|_| ())?;
    socket
        .send(Message::Text(
            serde_json::to_string(&frame).map_err(|_| ())?.into(),
        ))
        .await
        .map_err(|_| ())
}

async fn connect_runtime(
    config: &RemoteRuntimeConfig,
    workspace_id: &str,
) -> Result<RuntimeSocket, String> {
    let endpoint = runtime_endpoint(&config.base_url);
    let mut request = endpoint
        .into_client_request()
        .map_err(|error| format!("invalid Runtime subscription endpoint: {error}"))?;
    if let Some(token) = runtime_token(config, workspace_id)? {
        request.headers_mut().insert(
            "authorization",
            format!("Bearer {token}")
                .parse()
                .map_err(|error| format!("invalid Runtime authorization header: {error}"))?,
        );
    }
    connect_async(request)
        .await
        .map(|(socket, _)| socket)
        .map_err(|error| format!("failed to connect Runtime subscription endpoint: {error}"))
}
fn runtime_endpoint(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}/v1/protocol/ws")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}/v1/protocol/ws")
    } else {
        format!("{base}/v1/protocol/ws")
    }
}
fn runtime_token(
    config: &RemoteRuntimeConfig,
    workspace_id: &str,
) -> Result<Option<String>, String> {
    let Some(auth) = config.auth.as_ref() else {
        return Ok(config.bearer_token.clone());
    };
    let signer = CapabilityTokenSigner::new(&auth.server_id, &auth.server_private_key);
    let claims = capability_claims(
        &auth.server_id,
        &config.runtime_id,
        workspace_id,
        vec!["workers:list".into()],
        300,
    )
    .map_err(|error| error.to_string())?;
    signer
        .sign(&claims)
        .map(Some)
        .map_err(|error| error.to_string())
}
fn update_status(status: &RwLock<RuntimeSubscriptionBrokerStatus>, state: &State, connected: bool) {
    *status.write().expect("broker status poisoned") = RuntimeSubscriptionBrokerStatus {
        connection_generation: state.generation,
        connected,
        desired_selectors: state
            .selectors
            .values()
            .filter(|value| !value.downstreams.is_empty())
            .count(),
        upstream_subscriptions: state.upstream_index.len(),
    };
}

#[cfg(test)]
#[path = "runtime_subscription_tests.rs"]
mod tests;
