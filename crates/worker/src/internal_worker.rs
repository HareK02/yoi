//! Reusable execution substrate for Worker-backed internal jobs.
//!
//! Internal jobs are intentionally not Runtime-catalogued Workers. Each run still owns a
//! distinct Worker identity and executes through [`Worker`], including feature installation,
//! Workspace authority, session history, lifecycle records, usage accounting, cancellation,
//! and error handling. The session store is in-memory and dropped with the run; callers that
//! need durable domain audit must keep using their domain authority (for example Memory audit).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use agen::timeline::event::UsageEvent;
use agen::{Engine, EngineError, llm_client::LlmClient};
use manifest::{Scope, WorkerManifest};
use protocol::{Event, InFlightSnapshot, WorkerStatus};
use session_store::{LogEntry, SegmentId, SessionId, Store, StoreError, TraceEntry};
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::controller::{wire_event_bridges_on_engine, wire_workdir_command_events};
use crate::feature::FeatureRegistryBuilder;
use crate::in_flight::{InFlightEvents, snapshot_from_guard};
use crate::ipc::alerter::Alerter;
use crate::ipc::protocol_session::live_log_entry_event;
use crate::segment_log_sink::SegmentLogSink;
use crate::spawn::registry::SpawnedWorkerRegistry;
use crate::worker::{
    Worker, WorkerError, WorkerFilesystemAuthority, WorkerRunResult, WorkerWorkspaceContext,
};

/// Per-run identity for an internal Worker that is not registered in the public Runtime catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InternalWorkerIdentity {
    pub kind: &'static str,
    pub run_id: Uuid,
}

/// Explicit authority granted to one internal Worker run.
///
/// Extraction currently receives Workspace authority but no filesystem authority. Workdir
/// capabilities for Flow verifiers are deliberately left to the downstream Flow ticket.
pub(crate) struct InternalWorkerAuthority {
    pub workspace: WorkerWorkspaceContext,
    pub filesystem: WorkerFilesystemAuthority,
    pub scope: Scope,
    /// Provider-bound session inherited in an attenuated form from the owner.
    pub workdir_session: Option<workdir::WorkdirSessionHandle>,
}

pub(crate) struct InternalWorkerSpec {
    pub identity: InternalWorkerIdentity,
    pub manifest: WorkerManifest,
    pub client: Box<dyn LlmClient>,
    pub system_prompt: String,
    pub input: String,
    pub cache_key: Option<String>,
    pub max_turns: Option<u32>,
    pub engine_configurator: Option<
        Box<
            dyn FnOnce(
                    &mut Engine<
                        Box<dyn LlmClient>,
                        agen::state::Mutable,
                        crate::SessionHistoryMetadata,
                    >,
                ) + Send,
        >,
    >,
    pub features: FeatureRegistryBuilder,
    pub required_tools: &'static [&'static str],
    pub authority: InternalWorkerAuthority,
}

pub(crate) struct InternalWorkerResult {
    pub usage: Option<UsageEvent>,
    pub identity: InternalWorkerIdentity,
    pub lifecycle: WorkerRunResult,
    pub history_entries: usize,
}

pub(crate) struct InternalWorkerError {
    pub source: WorkerError,
    pub usage: Option<UsageEvent>,
    pub identity: InternalWorkerIdentity,
    pub history_entries: usize,
}

/// Execute an internal job through the normal Worker substrate.
///
/// The caller supplies the effective model client, an explicitly restricted feature set, and
/// explicit authority. No tools are registered directly on `Engine`, and no ambient filesystem
/// authority is inferred.
pub(crate) async fn run_internal_worker(
    spec: InternalWorkerSpec,
) -> Result<InternalWorkerResult, InternalWorkerError> {
    run_internal_worker_with_cancel_sender(spec, |_| {}).await
}

/// Execute an internal job while exposing only its cancellation capability to the caller.
///
/// This keeps the Internal Worker instance and its ephemeral session private while allowing an
/// owning caller to route a real cancellation through the normal Engine lifecycle.
pub(crate) async fn run_internal_worker_with_cancel_sender<F>(
    spec: InternalWorkerSpec,
    on_cancel_sender: F,
) -> Result<InternalWorkerResult, InternalWorkerError>
where
    F: FnOnce(tokio::sync::mpsc::Sender<()>),
{
    let InternalWorkerSpec {
        identity,
        mut manifest,
        client,
        system_prompt,
        input,
        cache_key,
        max_turns,
        engine_configurator,
        features,
        required_tools,
        authority,
    } = spec;

    // Internal identities are run-scoped and never enter the public Runtime Worker catalog.
    manifest.worker.name = format!("internal-{}-{}", identity.kind, identity.run_id);
    // Internal jobs only receive features supplied below. A parent manifest must not accidentally
    // grant its normal public tool surface or recursively schedule memory work.
    manifest.feature = Default::default();
    manifest.plugins = Default::default();
    manifest.mcp = Default::default();
    manifest.skills = None;
    manifest.compaction = None;
    manifest.memory = None;

    let last_usage = Arc::new(Mutex::new(None::<UsageEvent>));
    let usage_slot = last_usage.clone();
    let mut engine =
        Engine::<_, agen::state::Mutable, crate::SessionHistoryMetadata>::new_annotated(client)
            .system_prompt(system_prompt);
    engine.on_usage(move |usage| {
        if let Ok(mut slot) = usage_slot.lock() {
            *slot = Some(usage.clone());
        }
    });
    engine.set_cache_key(cache_key);
    engine.set_max_turns(max_turns);
    if let Some(configure) = engine_configurator {
        configure(&mut engine);
    }
    let store = EphemeralSessionStore::default();
    let inherited_workdir_session = authority.workdir_session.clone();
    let mut worker = Worker::new(
        manifest,
        engine,
        store.clone(),
        authority.workspace,
        authority.filesystem,
        authority.scope,
    )
    .await
    .map_err(|source| InternalWorkerError {
        source,
        usage: last_usage.lock().ok().and_then(|slot| slot.clone()),
        identity: identity.clone(),
        history_entries: 0,
    })?;
    if let Some(session) = inherited_workdir_session {
        worker.bind_workdir_session(Some(session));
    }

    let install_report = worker.install_features(features);
    let installed_tools = install_report.installed_tool_names();
    let required_tools_missing = required_tools.iter().any(|required| {
        !installed_tools
            .iter()
            .any(|installed| installed == required)
    });
    let install_failed = install_report
        .reports
        .iter()
        .any(|report| !report.installed);
    if install_failed || required_tools_missing {
        let diagnostics = install_report
            .reports
            .iter()
            .flat_map(|report| report.diagnostics.iter())
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        let missing = required_tools
            .iter()
            .filter(|required| {
                !installed_tools
                    .iter()
                    .any(|installed| installed == **required)
            })
            .copied()
            .collect::<Vec<_>>()
            .join(", ");
        return Err(InternalWorkerError {
            source: WorkerError::FeatureInstall(format!(
                "internal Worker feature installation failed: {diagnostics}; missing tools: {missing}"
            )),
            usage: last_usage.lock().ok().and_then(|slot| slot.clone()),
            identity,
            history_entries: 0,
        });
    }
    let session_id = worker.session_id();
    let segment_id = worker.segment_id();
    on_cancel_sender(worker.engine_mut().cancel_sender());

    match worker.run_text(&input).await {
        Ok(lifecycle @ WorkerRunResult::Finished)
        | Ok(lifecycle @ WorkerRunResult::Paused)
        | Ok(lifecycle @ WorkerRunResult::RolledBack) => Ok(InternalWorkerResult {
            usage: last_usage.lock().ok().and_then(|slot| slot.clone()),
            identity,
            lifecycle,
            history_entries: store.entries_count(session_id, segment_id),
        }),
        Ok(WorkerRunResult::LimitReached) => Err(InternalWorkerError {
            source: WorkerError::Engine(EngineError::Aborted(
                "internal Worker reached its turn limit".to_string(),
            )),
            usage: last_usage.lock().ok().and_then(|slot| slot.clone()),
            identity,
            history_entries: store.entries_count(session_id, segment_id),
        }),
        Ok(WorkerRunResult::Interrupted { message, .. }) => Err(InternalWorkerError {
            source: WorkerError::Engine(EngineError::Aborted(message)),
            usage: last_usage.lock().ok().and_then(|slot| slot.clone()),
            identity,
            history_entries: store.entries_count(session_id, segment_id),
        }),
        Err(source) => Err(InternalWorkerError {
            source,
            usage: last_usage.lock().ok().and_then(|slot| slot.clone()),
            identity,
            history_entries: store.entries_count(session_id, segment_id),
        }),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InternalWorkerVisibility {
    /// Output may be projected only through the owning parent's protocol stream.
    ParentClient,
    /// Backend-owned helper output remains private to the service authority.
    ServicePrivate,
}

impl Default for InternalWorkerVisibility {
    fn default() -> Self {
        Self::ServicePrivate
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InternalWorkerSessionStatus {
    Idle,
    Running,
    Paused,
    Stopping,
    Stopped,
    Failed,
}

impl InternalWorkerSessionStatus {
    fn encode(self) -> u8 {
        match self {
            Self::Idle => 0,
            Self::Running => 1,
            Self::Paused => 2,
            Self::Stopping => 3,
            Self::Stopped => 4,
            Self::Failed => 5,
        }
    }

    fn decode(value: u8) -> Self {
        match value {
            0 => Self::Idle,
            1 => Self::Running,
            2 => Self::Paused,
            3 => Self::Stopping,
            4 => Self::Stopped,
            _ => Self::Failed,
        }
    }
}

fn classify_internal_turn_result(
    result: Result<WorkerRunResult, WorkerError>,
) -> (InternalWorkerSessionStatus, Option<String>) {
    match result {
        Ok(WorkerRunResult::Finished) => (InternalWorkerSessionStatus::Idle, None),
        Ok(WorkerRunResult::Paused) => (InternalWorkerSessionStatus::Paused, None),
        Ok(WorkerRunResult::LimitReached) => (
            InternalWorkerSessionStatus::Stopped,
            Some("internal Worker reached its turn limit".to_string()),
        ),
        Ok(WorkerRunResult::Interrupted { message, .. }) => {
            (InternalWorkerSessionStatus::Stopped, Some(message))
        }
        Ok(WorkerRunResult::RolledBack) => (
            InternalWorkerSessionStatus::Stopped,
            Some("internal Worker run was cancelled before AI output".to_string()),
        ),
        Err(error) => (InternalWorkerSessionStatus::Failed, Some(error.to_string())),
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum InternalWorkerSessionError {
    #[error("failed to build internal Worker session: {message}")]
    Build { message: String },
    #[error("internal Worker session is busy")]
    Busy,
    #[error("internal Worker session is stopped")]
    Stopped,
    #[error("internal Worker session actor is unavailable")]
    Unavailable,
}

enum InternalWorkerSessionCommand {
    Run(String),
    Stop(tokio::sync::oneshot::Sender<()>),
}

/// Parent-owned handle for a long-lived Internal Worker session.
///
/// The handle exposes typed turn, history, status, presentation snapshot, event subscription, and
/// stop operations. The underlying Worker, Engine, and cancellation sender remain inside the actor
/// task; protocol access is consumed only by the owning parent registry.
#[derive(Debug, Clone)]
pub(crate) struct InternalWorkerSessionSnapshot {
    pub entries: Vec<LogEntry>,
    pub status: WorkerStatus,
    pub error: Option<String>,
    pub in_flight: InFlightSnapshot,
    pub internal_workers: Vec<protocol::InternalWorkerSnapshot>,
}

#[derive(Clone)]
pub(crate) struct InternalWorkerSessionHandle {
    command_tx: tokio::sync::mpsc::Sender<InternalWorkerSessionCommand>,
    status: Arc<std::sync::atomic::AtomicU8>,
    store: EphemeralSessionStore,
    session_id: SessionId,
    segment_id: SegmentId,
    state_changed: Arc<tokio::sync::Notify>,
    in_flight: InFlightEvents,
    event_tx: broadcast::Sender<Event>,
    visibility: InternalWorkerVisibility,
    last_error: Arc<Mutex<Option<String>>>,
    child_registry: Option<Arc<SpawnedWorkerRegistry>>,
    sink: SegmentLogSink,
    #[cfg(test)]
    fail_stop: Arc<std::sync::atomic::AtomicBool>,
}

impl InternalWorkerSessionHandle {
    pub(crate) fn session_id_string(&self) -> String {
        self.session_id.to_string()
    }

    pub(crate) fn status(&self) -> InternalWorkerSessionStatus {
        InternalWorkerSessionStatus::decode(self.status.load(std::sync::atomic::Ordering::Acquire))
    }

    pub(crate) fn visibility(&self) -> InternalWorkerVisibility {
        self.visibility
    }

    pub(crate) fn subscribe_events(&self) -> broadcast::Receiver<Event> {
        self.event_tx.subscribe()
    }

    pub(crate) fn protocol_sender(&self) -> broadcast::Sender<Event> {
        self.event_tx.clone()
    }

    #[cfg(test)]
    pub(crate) fn publish_test_entry(&self, entry: LogEntry) {
        self.store
            .append(self.session_id, self.segment_id, &entry)
            .expect("append test Internal Worker entry");
        self.sink.publish(entry);
    }

    #[cfg(test)]
    pub(crate) fn emit_test_text_delta(&self, text: &str) {
        let block_id = self.in_flight.start_text_block();
        self.in_flight.text_delta(block_id, text.to_owned());
    }

    pub(crate) fn protocol_snapshot(&self) -> InternalWorkerSessionSnapshot {
        let (entries, in_flight) = {
            let guard = self.in_flight.snapshot_guard();
            let (entries, _) = self.sink.subscribe_with_snapshot();
            (entries, snapshot_from_guard(&guard))
        };
        InternalWorkerSessionSnapshot {
            entries,
            status: match self.status() {
                InternalWorkerSessionStatus::Running => WorkerStatus::Running,
                InternalWorkerSessionStatus::Paused => WorkerStatus::Paused,
                InternalWorkerSessionStatus::Idle => WorkerStatus::Idle,
                InternalWorkerSessionStatus::Stopping
                | InternalWorkerSessionStatus::Stopped
                | InternalWorkerSessionStatus::Failed => WorkerStatus::Stopped,
            },
            error: self.last_error.lock().unwrap().clone(),
            in_flight,
            internal_workers: self
                .child_registry
                .as_ref()
                .map(|registry| registry.internal_worker_snapshots())
                .unwrap_or_default(),
        }
    }

    pub(crate) fn entries(&self) -> Vec<LogEntry> {
        self.store
            .read_all(self.session_id, self.segment_id)
            .unwrap_or_default()
    }

    pub(crate) async fn send(
        &self,
        input: impl Into<String>,
    ) -> Result<(), InternalWorkerSessionError> {
        self.status
            .compare_exchange(
                InternalWorkerSessionStatus::Idle.encode(),
                InternalWorkerSessionStatus::Running.encode(),
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .map_err(
                |current| match InternalWorkerSessionStatus::decode(current) {
                    InternalWorkerSessionStatus::Running
                    | InternalWorkerSessionStatus::Paused
                    | InternalWorkerSessionStatus::Stopping => InternalWorkerSessionError::Busy,
                    InternalWorkerSessionStatus::Stopped | InternalWorkerSessionStatus::Failed => {
                        InternalWorkerSessionError::Stopped
                    }
                    InternalWorkerSessionStatus::Idle => InternalWorkerSessionError::Unavailable,
                },
            )?;
        if self
            .command_tx
            .send(InternalWorkerSessionCommand::Run(input.into()))
            .await
            .is_err()
        {
            self.status.store(
                InternalWorkerSessionStatus::Failed.encode(),
                std::sync::atomic::Ordering::Release,
            );
            self.state_changed.notify_waiters();
            let message = "internal Worker session actor is unavailable".to_owned();
            *self.last_error.lock().unwrap() = Some(message.clone());
            let _ = self.event_tx.send(Event::Error {
                code: protocol::ErrorCode::Internal,
                message,
            });
            return Err(InternalWorkerSessionError::Unavailable);
        }
        let _ = self.event_tx.send(Event::Status {
            status: WorkerStatus::Running,
        });
        Ok(())
    }

    pub(crate) async fn wait_until_idle(&self) -> InternalWorkerSessionStatus {
        loop {
            let notified = self.state_changed.notified();
            let status = self.status();
            if status != InternalWorkerSessionStatus::Running
                && status != InternalWorkerSessionStatus::Stopping
            {
                return status;
            }
            notified.await;
        }
    }

    #[cfg(test)]
    pub(crate) fn force_status(&self, status: InternalWorkerSessionStatus) {
        self.status
            .store(status.encode(), std::sync::atomic::Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn force_stop_failure(&self) {
        self.fail_stop
            .store(true, std::sync::atomic::Ordering::Release);
    }

    pub(crate) async fn stop(&self) -> Result<(), InternalWorkerSessionError> {
        #[cfg(test)]
        if self.fail_stop.load(std::sync::atomic::Ordering::Acquire) {
            return Err(InternalWorkerSessionError::Unavailable);
        }
        let prior = self.status.swap(
            InternalWorkerSessionStatus::Stopping.encode(),
            std::sync::atomic::Ordering::AcqRel,
        );
        if matches!(
            InternalWorkerSessionStatus::decode(prior),
            InternalWorkerSessionStatus::Stopped | InternalWorkerSessionStatus::Failed
        ) {
            return Ok(());
        }
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        self.command_tx
            .send(InternalWorkerSessionCommand::Stop(done_tx))
            .await
            .map_err(|_| InternalWorkerSessionError::Unavailable)?;
        done_rx
            .await
            .map_err(|_| InternalWorkerSessionError::Unavailable)
    }
}

/// Start a reusable Internal Worker session and accept its first turn.
#[cfg(test)]
pub(crate) async fn spawn_internal_worker_session(
    spec: InternalWorkerSpec,
) -> Result<InternalWorkerSessionHandle, InternalWorkerSessionError> {
    let InternalWorkerSpec {
        identity,
        mut manifest,
        client,
        system_prompt,
        input,
        cache_key,
        max_turns,
        engine_configurator,
        features,
        required_tools,
        authority,
    } = spec;
    manifest.worker.name = format!("internal-{}-{}", identity.kind, identity.run_id);
    manifest.memory = None;

    let last_usage = Arc::new(Mutex::new(None::<UsageEvent>));
    let usage_slot = last_usage.clone();
    let mut engine =
        Engine::<_, agen::state::Mutable, crate::SessionHistoryMetadata>::new_annotated(client)
            .system_prompt(system_prompt);
    engine.on_usage(move |usage| {
        if let Ok(mut slot) = usage_slot.lock() {
            *slot = Some(usage.clone());
        }
    });
    engine.set_cache_key(cache_key);
    engine.set_max_turns(max_turns);
    if let Some(configure) = engine_configurator {
        configure(&mut engine);
    }
    let store = EphemeralSessionStore::default();
    let inherited_workdir_session = authority.workdir_session.clone();
    let mut worker = Worker::new(
        manifest,
        engine,
        store.clone(),
        authority.workspace,
        authority.filesystem,
        authority.scope,
    )
    .await
    .map_err(|source| InternalWorkerSessionError::Build {
        message: source.to_string(),
    })?;
    if let Some(session) = inherited_workdir_session {
        worker.bind_workdir_session(Some(session));
    }
    let install_report = worker.install_features(features);
    let installed_tools = install_report.installed_tool_names();
    let install_failed = install_report
        .reports
        .iter()
        .any(|report| !report.installed);
    let missing = required_tools
        .iter()
        .filter(|required| {
            !installed_tools
                .iter()
                .any(|installed| installed == **required)
        })
        .copied()
        .collect::<Vec<_>>();
    if install_failed || !missing.is_empty() {
        let diagnostics = install_report
            .reports
            .iter()
            .flat_map(|report| report.diagnostics.iter())
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        return Err(InternalWorkerSessionError::Build {
            message: format!(
                "internal Worker feature installation failed: {diagnostics}; missing tools: {}",
                missing.join(", ")
            ),
        });
    }

    spawn_prepared_internal_worker_session(worker, store, input, None).await
}

/// Prepare an observable Internal Worker from the same bounded spec as one-shot
/// helpers, but do not start its first turn. The owner must register the returned
/// handle before calling `send`, preserving the snapshot/live boundary.
pub(crate) fn prepare_internal_worker_from_spec(
    spec: InternalWorkerSpec,
    visibility: InternalWorkerVisibility,
) -> std::pin::Pin<
    Box<
        dyn std::future::Future<
                Output = Result<InternalWorkerSessionHandle, InternalWorkerSessionError>,
            > + Send,
    >,
> {
    Box::pin(async move {
        let InternalWorkerSpec {
            identity,
            mut manifest,
            client,
            system_prompt,
            input: _,
            cache_key,
            max_turns,
            engine_configurator,
            features,
            required_tools,
            authority,
        } = spec;
        manifest.worker.name = format!("internal-{}-{}", identity.kind, identity.run_id);
        manifest.feature = Default::default();
        manifest.plugins = Default::default();
        manifest.mcp = Default::default();
        manifest.skills = None;
        manifest.compaction = None;
        manifest.memory = None;

        let mut engine =
            Engine::<_, agen::state::Mutable, crate::SessionHistoryMetadata>::new_annotated(client)
                .system_prompt(system_prompt);
        engine.set_cache_key(cache_key);
        engine.set_max_turns(max_turns);
        if let Some(configure) = engine_configurator {
            configure(&mut engine);
        }
        let store = EphemeralSessionStore::default();
        let inherited_workdir_session = authority.workdir_session.clone();
        let mut worker = Worker::new(
            manifest,
            engine,
            store.clone(),
            authority.workspace,
            authority.filesystem,
            authority.scope,
        )
        .await
        .map_err(|source| InternalWorkerSessionError::Build {
            message: source.to_string(),
        })?;
        if let Some(session) = inherited_workdir_session {
            worker.bind_workdir_session(Some(session));
        }
        let install_report = worker.install_features(features);
        let installed_tools = install_report.installed_tool_names();
        let install_failed = install_report
            .reports
            .iter()
            .any(|report| !report.installed);
        let missing = required_tools
            .iter()
            .filter(|required| {
                !installed_tools
                    .iter()
                    .any(|installed| installed == **required)
            })
            .copied()
            .collect::<Vec<_>>();
        if install_failed || !missing.is_empty() {
            let diagnostics = install_report
                .reports
                .iter()
                .flat_map(|report| report.diagnostics.iter())
                .map(|diagnostic| diagnostic.message.as_str())
                .collect::<Vec<_>>()
                .join("; ");
            return Err(InternalWorkerSessionError::Build {
                message: format!(
                    "internal Worker feature installation failed: {diagnostics}; missing tools: {}",
                    missing.join(", ")
                ),
            });
        }

        Box::pin(prepare_internal_worker_session(
            worker, store, visibility, None, None,
        ))
        .await
    })
}

fn spawn_internal_log_event_bridge(sink: SegmentLogSink, event_tx: broadcast::Sender<Event>) {
    let (_, mut log_rx) = sink.subscribe_with_snapshot();
    tokio::spawn(async move {
        loop {
            match log_rx.recv().await {
                Ok(entry) => {
                    if let Some(event) = live_log_entry_event(entry) {
                        let _ = event_tx.send(event);
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    let _ = event_tx.send(Event::Error {
                        code: protocol::ErrorCode::Internal,
                        message: format!(
                            "internal Worker session-log output lagged by {skipped} entries; reconnect to resynchronize"
                        ),
                    });
                    break;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

pub(crate) async fn prepare_internal_worker_session(
    mut worker: Worker<Box<dyn LlmClient>, EphemeralSessionStore>,
    store: EphemeralSessionStore,
    visibility: InternalWorkerVisibility,
    child_registry: Option<Arc<SpawnedWorkerRegistry>>,
    on_turn_end: Option<Arc<dyn Fn(InternalWorkerSessionStatus) + Send + Sync>>,
) -> Result<InternalWorkerSessionHandle, InternalWorkerSessionError> {
    let (event_tx, _event_rx) = broadcast::channel(256);
    let sink = worker.sink();
    spawn_internal_log_event_bridge(sink.clone(), event_tx.clone());
    let alerter = Alerter::new(event_tx.clone());
    let in_flight = InFlightEvents::new(event_tx.clone());
    if let Some(session) = worker.workdir_session() {
        wire_workdir_command_events(session, &in_flight);
    }
    let actor_in_flight = in_flight.clone();
    worker.attach_alerter(alerter.clone());
    worker.attach_event_tx(event_tx.clone());
    worker.attach_in_flight_events(in_flight.clone());
    wire_event_bridges_on_engine(&mut worker, &event_tx, &alerter, &in_flight);

    let session_id = worker.session_id();
    let segment_id = worker.segment_id();
    let (command_tx, mut command_rx) = tokio::sync::mpsc::channel(8);
    let status = Arc::new(std::sync::atomic::AtomicU8::new(
        InternalWorkerSessionStatus::Idle.encode(),
    ));
    let state_changed = Arc::new(tokio::sync::Notify::new());
    let last_error = Arc::new(Mutex::new(None));
    let handle = InternalWorkerSessionHandle {
        command_tx,
        status: status.clone(),
        store,
        session_id,
        segment_id,
        state_changed: state_changed.clone(),
        in_flight,
        event_tx: event_tx.clone(),
        visibility,
        last_error: last_error.clone(),
        child_registry,
        sink,
        #[cfg(test)]
        fail_stop: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };

    tokio::spawn(async move {
        while let Some(command) = command_rx.recv().await {
            match command {
                InternalWorkerSessionCommand::Run(input) => {
                    actor_in_flight.clear();
                    let cancel_sender = worker.engine_mut().cancel_sender();
                    let mut run = std::pin::pin!(worker.run_text(&input));
                    loop {
                        tokio::select! {
                            result = &mut run => {
                                let (turn_status, error) = classify_internal_turn_result(result);
                                actor_in_flight.clear();
                                status.store(turn_status.encode(), std::sync::atomic::Ordering::Release);
                                if let Some(message) = error {
                                    *last_error.lock().unwrap() = Some(message.clone());
                                    let _ = event_tx.send(Event::Error {
                                        code: protocol::ErrorCode::Internal,
                                        message,
                                    });
                                }
                                let protocol_status = match turn_status {
                                    InternalWorkerSessionStatus::Idle => WorkerStatus::Idle,
                                    InternalWorkerSessionStatus::Paused => WorkerStatus::Paused,
                                    InternalWorkerSessionStatus::Stopped
                                    | InternalWorkerSessionStatus::Failed => WorkerStatus::Stopped,
                                    InternalWorkerSessionStatus::Running
                                    | InternalWorkerSessionStatus::Stopping => {
                                        unreachable!("run completion cannot remain active")
                                    }
                                };
                                let _ = event_tx.send(Event::Status {
                                    status: protocol_status,
                                });
                                if let Some(callback) = &on_turn_end {
                                    callback(turn_status);
                                }
                                state_changed.notify_waiters();
                                break;
                            }
                            command = command_rx.recv() => {
                                match command {
                                    Some(InternalWorkerSessionCommand::Stop(done)) => {
                                        let _ = cancel_sender.send(()).await;
                                        let _ = (&mut run).await;
                                        actor_in_flight.clear();
                                        status.store(InternalWorkerSessionStatus::Stopped.encode(), std::sync::atomic::Ordering::Release);
                                        let _ = event_tx.send(Event::Status { status: WorkerStatus::Stopped });
                                        let _ = event_tx.send(Event::Shutdown);
                                        state_changed.notify_waiters();
                                        let _ = done.send(());
                                        return;
                                    }
                                    Some(InternalWorkerSessionCommand::Run(_)) => {
                                        // `send` reserves Running atomically, so a second Run cannot be enqueued.
                                    }
                                    None => {
                                        let _ = cancel_sender.send(()).await;
                                        actor_in_flight.clear();
                                        return;
                                    }
                                }
                            }
                        }
                    }
                }
                InternalWorkerSessionCommand::Stop(done) => {
                    actor_in_flight.clear();
                    status.store(
                        InternalWorkerSessionStatus::Stopped.encode(),
                        std::sync::atomic::Ordering::Release,
                    );
                    let _ = event_tx.send(Event::Status {
                        status: WorkerStatus::Stopped,
                    });
                    let _ = event_tx.send(Event::Shutdown);
                    state_changed.notify_waiters();
                    let _ = done.send(());
                    return;
                }
            }
        }
        actor_in_flight.clear();
    });

    Ok(handle)
}

#[cfg(test)]
pub(crate) async fn spawn_prepared_internal_worker_session(
    worker: Worker<Box<dyn LlmClient>, EphemeralSessionStore>,
    store: EphemeralSessionStore,
    input: String,
    on_turn_end: Option<Arc<dyn Fn(InternalWorkerSessionStatus) + Send + Sync>>,
) -> Result<InternalWorkerSessionHandle, InternalWorkerSessionError> {
    let handle = prepare_internal_worker_session(
        worker,
        store,
        InternalWorkerVisibility::ServicePrivate,
        None,
        on_turn_end,
    )
    .await?;
    handle.send(input).await?;
    Ok(handle)
}

/// Session history for an ephemeral internal Worker.
///
/// Keeping the normal Store contract makes history/lifecycle/error records identical to a normal
/// Worker while avoiding a second public persistence/catalog policy for helper executions.
#[derive(Clone, Default)]
pub(crate) struct EphemeralSessionStore {
    entries: Arc<Mutex<HashMap<(SessionId, SegmentId), Vec<LogEntry>>>>,
    traces: Arc<Mutex<HashMap<(SessionId, SegmentId), Vec<TraceEntry>>>>,
    worker_metadata: Arc<Mutex<HashMap<String, session_store::WorkerMetadata>>>,
}

impl EphemeralSessionStore {
    fn entries_count(&self, session_id: SessionId, segment_id: SegmentId) -> usize {
        self.entries
            .lock()
            .ok()
            .and_then(|entries| entries.get(&(session_id, segment_id)).map(Vec::len))
            .unwrap_or_default()
    }
}

impl Store for EphemeralSessionStore {
    fn append(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
        entry: &LogEntry,
    ) -> Result<(), StoreError> {
        self.entries
            .lock()
            .expect("ephemeral session store mutex poisoned")
            .entry((session_id, segment_id))
            .or_default()
            .push(entry.clone());
        Ok(())
    }

    fn read_all(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
    ) -> Result<Vec<LogEntry>, StoreError> {
        Ok(self
            .entries
            .lock()
            .expect("ephemeral session store mutex poisoned")
            .get(&(session_id, segment_id))
            .cloned()
            .unwrap_or_default())
    }

    fn list_sessions(&self) -> Result<Vec<SessionId>, StoreError> {
        let mut sessions = self
            .entries
            .lock()
            .expect("ephemeral session store mutex poisoned")
            .keys()
            .map(|(session_id, _)| *session_id)
            .collect::<Vec<_>>();
        sessions.sort_unstable();
        sessions.dedup();
        sessions.reverse();
        Ok(sessions)
    }

    fn list_segments(&self, session_id: SessionId) -> Result<Vec<SegmentId>, StoreError> {
        let mut segments = self
            .entries
            .lock()
            .expect("ephemeral session store mutex poisoned")
            .keys()
            .filter_map(|(entry_session_id, segment_id)| {
                (*entry_session_id == session_id).then_some(*segment_id)
            })
            .collect::<Vec<_>>();
        segments.sort_unstable();
        segments.reverse();
        Ok(segments)
    }

    fn lookup_session_of(&self, segment_id: SegmentId) -> Result<Option<SessionId>, StoreError> {
        Ok(self
            .entries
            .lock()
            .expect("ephemeral session store mutex poisoned")
            .keys()
            .find_map(|(session_id, entry_segment_id)| {
                (*entry_segment_id == segment_id).then_some(*session_id)
            }))
    }

    fn create_segment(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
        entries: &[LogEntry],
    ) -> Result<(), StoreError> {
        self.entries
            .lock()
            .expect("ephemeral session store mutex poisoned")
            .insert((session_id, segment_id), entries.to_vec());
        Ok(())
    }

    fn exists(&self, session_id: SessionId, segment_id: SegmentId) -> Result<bool, StoreError> {
        Ok(self
            .entries
            .lock()
            .expect("ephemeral session store mutex poisoned")
            .contains_key(&(session_id, segment_id)))
    }

    fn read_entry_count(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
    ) -> Result<usize, StoreError> {
        Ok(self.entries_count(session_id, segment_id))
    }

    fn append_trace(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
        entry: &TraceEntry,
    ) -> Result<(), StoreError> {
        self.traces
            .lock()
            .expect("ephemeral session trace store mutex poisoned")
            .entry((session_id, segment_id))
            .or_default()
            .push(entry.clone());
        Ok(())
    }
}

impl session_store::WorkerMetadataStore for EphemeralSessionStore {
    fn write(
        &self,
        metadata: &session_store::WorkerMetadata,
    ) -> Result<(), session_store::WorkerStoreError> {
        self.worker_metadata
            .lock()
            .map_err(|_| {
                session_store::WorkerStoreError::Io(std::io::Error::other(
                    "ephemeral metadata lock poisoned",
                ))
            })?
            .insert(metadata.worker_name.clone(), metadata.clone());
        Ok(())
    }

    fn read_by_name(
        &self,
        worker_name: &str,
    ) -> Result<Option<session_store::WorkerMetadata>, session_store::WorkerStoreError> {
        Ok(self
            .worker_metadata
            .lock()
            .map_err(|_| {
                session_store::WorkerStoreError::Io(std::io::Error::other(
                    "ephemeral metadata lock poisoned",
                ))
            })?
            .get(worker_name)
            .cloned())
    }

    fn list_names(&self) -> Result<Vec<String>, session_store::WorkerStoreError> {
        Ok(self
            .worker_metadata
            .lock()
            .map_err(|_| {
                session_store::WorkerStoreError::Io(std::io::Error::other(
                    "ephemeral metadata lock poisoned",
                ))
            })?
            .keys()
            .cloned()
            .collect())
    }

    fn delete_by_name(&self, worker_name: &str) -> Result<(), session_store::WorkerStoreError> {
        self.worker_metadata
            .lock()
            .map_err(|_| {
                session_store::WorkerStoreError::Io(std::io::Error::other(
                    "ephemeral metadata lock poisoned",
                ))
            })?
            .remove(worker_name);
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn test_internal_worker_session(
    visibility: InternalWorkerVisibility,
) -> (InternalWorkerSessionHandle, broadcast::Sender<Event>) {
    let store = EphemeralSessionStore::default();
    let session_id = session_store::new_session_id();
    let segment_id = session_store::new_segment_id();
    let (command_tx, mut command_rx) = tokio::sync::mpsc::channel(1);
    let (event_tx, _) = broadcast::channel(256);
    let command_event_tx = event_tx.clone();
    tokio::spawn(async move {
        while let Some(command) = command_rx.recv().await {
            if let InternalWorkerSessionCommand::Stop(done_tx) = command {
                let _ = command_event_tx.send(Event::Shutdown);
                let _ = done_tx.send(());
                break;
            }
        }
    });
    let sink = SegmentLogSink::new();
    spawn_internal_log_event_bridge(sink.clone(), event_tx.clone());
    let handle = InternalWorkerSessionHandle {
        command_tx,
        status: Arc::new(std::sync::atomic::AtomicU8::new(
            InternalWorkerSessionStatus::Idle.encode(),
        )),
        store,
        session_id,
        segment_id,
        state_changed: Arc::new(tokio::sync::Notify::new()),
        in_flight: InFlightEvents::new(event_tx.clone()),
        event_tx: event_tx.clone(),
        visibility,
        last_error: Arc::new(Mutex::new(None)),
        child_registry: None,
        sink,
        fail_stop: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    (handle, event_tx)
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use agen::llm_client::event::{Event as LlmEvent, ResponseStatus, StatusEvent};
    use agen::llm_client::{ClientError, Request};
    use async_trait::async_trait;
    use futures::Stream;

    use super::*;

    #[derive(Clone)]
    struct OneTurnClient {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl LlmClient for OneTurnClient {
        fn clone_boxed(&self) -> Box<dyn LlmClient> {
            Box::new(self.clone())
        }

        async fn stream(
            &self,
            _request: Request,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent, ClientError>> + Send>>, ClientError>
        {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(Box::pin(futures::stream::iter(vec![
                Ok(LlmEvent::text_block_start(0)),
                Ok(LlmEvent::text_delta(0, "done")),
                Ok(LlmEvent::text_block_stop(0, None)),
                Ok(LlmEvent::Status(StatusEvent {
                    status: ResponseStatus::Completed,
                })),
            ])))
        }
    }

    #[derive(Clone)]
    struct FailingClient;

    #[async_trait]
    impl LlmClient for FailingClient {
        fn clone_boxed(&self) -> Box<dyn LlmClient> {
            Box::new(self.clone())
        }

        async fn stream(
            &self,
            _request: Request,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent, ClientError>> + Send>>, ClientError>
        {
            Err(ClientError::Config(
                "intentional internal failure".to_string(),
            ))
        }
    }

    #[derive(Clone)]
    struct CancelBeforeAiClient {
        calls: Arc<AtomicUsize>,
        cancel_sender: Arc<Mutex<Option<tokio::sync::mpsc::Sender<()>>>>,
    }

    #[async_trait]
    impl LlmClient for CancelBeforeAiClient {
        fn clone_boxed(&self) -> Box<dyn LlmClient> {
            Box::new(self.clone())
        }

        async fn stream(
            &self,
            _request: Request,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent, ClientError>> + Send>>, ClientError>
        {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let sender = self
                .cancel_sender
                .lock()
                .expect("cancel sender lock")
                .clone()
                .expect("cancel sender installed before run");
            sender.send(()).await.expect("internal engine is live");
            Ok(Box::pin(futures::stream::pending()))
        }
    }

    #[derive(Clone)]
    struct PendingClient {
        calls: Arc<AtomicUsize>,
        entered: Arc<tokio::sync::Notify>,
    }

    #[async_trait]
    impl LlmClient for PendingClient {
        fn clone_boxed(&self) -> Box<dyn LlmClient> {
            Box::new(self.clone())
        }

        async fn stream(
            &self,
            _request: Request,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent, ClientError>> + Send>>, ClientError>
        {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.entered.notify_one();
            Ok(Box::pin(futures::stream::pending()))
        }
    }

    fn manifest() -> WorkerManifest {
        WorkerManifest::from_toml(
            r#"
[worker]
name = "parent"

[model]
scheme = "anthropic"
model_id = "test-model"

[engine]

[[scope.allow]]
target = "/abs/scope"
permission = "write"
"#,
        )
        .unwrap()
    }

    fn spec(
        calls: Arc<AtomicUsize>,
        required_tools: &'static [&'static str],
    ) -> InternalWorkerSpec {
        InternalWorkerSpec {
            identity: InternalWorkerIdentity {
                kind: "test",
                run_id: Uuid::from_u128(1),
            },
            manifest: manifest(),
            client: Box::new(OneTurnClient { calls }),
            system_prompt: "system".to_string(),
            input: "input".to_string(),
            cache_key: Some("internal-test".to_string()),
            max_turns: Some(1),
            engine_configurator: None,
            features: FeatureRegistryBuilder::new(),
            required_tools,
            authority: InternalWorkerAuthority {
                workspace: WorkerWorkspaceContext::no_workspace(),
                filesystem: WorkerFilesystemAuthority::None,
                scope: Scope::empty(),
                workdir_session: None,
            },
        }
    }

    #[tokio::test]
    async fn executes_through_worker_and_records_ephemeral_history() {
        let calls = Arc::new(AtomicUsize::new(0));
        let result = match run_internal_worker(spec(calls.clone(), &[])).await {
            Ok(result) => result,
            Err(error) => panic!("internal Worker should complete: {}", error.source),
        };

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(matches!(result.lifecycle, WorkerRunResult::Finished));
        assert!(result.history_entries >= 4);
        assert_eq!(result.identity.kind, "test");
    }

    #[test]
    fn internal_turn_result_mapping_is_exhaustive() {
        let cases = [
            (
                WorkerRunResult::Finished,
                InternalWorkerSessionStatus::Idle,
                false,
            ),
            (
                WorkerRunResult::Paused,
                InternalWorkerSessionStatus::Paused,
                false,
            ),
            (
                WorkerRunResult::LimitReached,
                InternalWorkerSessionStatus::Stopped,
                true,
            ),
            (
                WorkerRunResult::Interrupted {
                    code: protocol::ErrorCode::Internal,
                    message: "cancelled".to_string(),
                },
                InternalWorkerSessionStatus::Stopped,
                true,
            ),
            (
                WorkerRunResult::RolledBack,
                InternalWorkerSessionStatus::Stopped,
                true,
            ),
        ];

        for (result, expected_status, expects_error) in cases {
            let (status, error) = classify_internal_turn_result(Ok(result));
            assert_eq!(status, expected_status);
            assert_eq!(error.is_some(), expects_error);
        }

        let (status, error) = classify_internal_turn_result(Err(WorkerError::Engine(
            EngineError::Aborted("fatal".to_string()),
        )));
        assert_eq!(status, InternalWorkerSessionStatus::Failed);
        assert!(error.is_some_and(|message| message.contains("fatal")));
    }

    #[tokio::test]
    async fn fatal_internal_run_transitions_to_stopped_protocol_status() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut internal_spec = spec(calls, &[]);
        internal_spec.client = Box::new(FailingClient);

        let handle = spawn_internal_worker_session(internal_spec)
            .await
            .expect("spawn failing Internal Worker session");
        assert_eq!(
            handle.wait_until_idle().await,
            InternalWorkerSessionStatus::Stopped
        );
        assert_eq!(handle.status(), InternalWorkerSessionStatus::Stopped);
        assert_eq!(handle.protocol_snapshot().status, WorkerStatus::Stopped);
        assert!(
            handle
                .last_error
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|message| message.contains("intentional internal failure"))
        );
    }

    #[tokio::test]
    async fn session_accepts_follow_up_turns_and_stops_without_runtime_registration() {
        let calls = Arc::new(AtomicUsize::new(0));
        let handle = spawn_internal_worker_session(spec(calls.clone(), &[]))
            .await
            .expect("spawn Internal Worker session");

        assert_eq!(
            handle.wait_until_idle().await,
            InternalWorkerSessionStatus::Idle
        );
        handle
            .in_flight
            .tool_call_start("stale-call".to_string(), "Read".to_string());
        assert_eq!(handle.protocol_snapshot().in_flight.blocks.len(), 1);
        let entries_after_first = handle.entries().len();
        assert!(entries_after_first >= 4);
        handle.send("follow-up").await.expect("send follow-up turn");
        assert_eq!(
            handle.wait_until_idle().await,
            InternalWorkerSessionStatus::Idle
        );
        assert!(handle.protocol_snapshot().in_flight.blocks.is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(handle.entries().len() > entries_after_first);

        handle.stop().await.expect("stop Internal Worker session");
        assert_eq!(handle.status(), InternalWorkerSessionStatus::Stopped);
        assert!(matches!(
            handle.send("too late").await,
            Err(InternalWorkerSessionError::Stopped)
        ));
    }

    #[tokio::test]
    async fn session_rejects_parallel_turns_and_cancels_running_turn_on_stop() {
        let calls = Arc::new(AtomicUsize::new(0));
        let entered = Arc::new(tokio::sync::Notify::new());
        let mut internal_spec = spec(calls.clone(), &[]);
        internal_spec.client = Box::new(PendingClient {
            calls: calls.clone(),
            entered: entered.clone(),
        });
        let handle = spawn_internal_worker_session(internal_spec)
            .await
            .expect("spawn running Internal Worker session");

        assert_eq!(handle.status(), InternalWorkerSessionStatus::Running);
        assert!(matches!(
            handle.send("parallel").await,
            Err(InternalWorkerSessionError::Busy)
        ));
        entered.notified().await;
        handle.stop().await.expect("cancel and stop session");
        assert_eq!(handle.status(), InternalWorkerSessionStatus::Stopped);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cancellation_before_ai_item_returns_rolled_back_lifecycle() {
        let calls = Arc::new(AtomicUsize::new(0));
        let cancel_sender = Arc::new(Mutex::new(None));
        let mut internal_spec = spec(calls.clone(), &[]);
        internal_spec.client = Box::new(CancelBeforeAiClient {
            calls: calls.clone(),
            cancel_sender: cancel_sender.clone(),
        });
        let prepare_sender = cancel_sender.clone();

        let result =
            match run_internal_worker_with_cancel_sender(internal_spec, move |cancel_sender| {
                *prepare_sender.lock().expect("cancel sender lock") = Some(cancel_sender);
            })
            .await
            {
                Ok(result) => result,
                Err(error) => panic!(
                    "Worker rollback should remain a lifecycle result: {:?}",
                    error.source
                ),
            };

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(matches!(result.lifecycle, WorkerRunResult::RolledBack));
    }

    #[tokio::test]
    async fn rejects_missing_explicit_tools_before_model_execution() {
        let calls = Arc::new(AtomicUsize::new(0));
        let error = match run_internal_worker(spec(calls.clone(), &["missing_tool"])).await {
            Err(error) => error,
            Ok(_) => panic!("required tool must be installed through the Worker registry"),
        };

        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(matches!(error.source, WorkerError::FeatureInstall(_)));
    }
}
