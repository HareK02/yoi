use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use agen::EngineError;
use agen::llm_client::client::LlmClient;
use session_store::WorkerMetadataStore;
use session_store::{LogEntry, Store};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::discovery::WorkerDiscovery;
use crate::feature::FeatureRegistryBuilder;
use crate::in_flight::{InFlightEvents, snapshot_from_guard};
use crate::ipc::alerter::Alerter;
use crate::ipc::notify_buffer::NotifyBuffer;
use crate::ipc::server::SocketServer;
use crate::runtime::dir::RuntimeDir;
use crate::segment_log_sink::SegmentLogSink;
use crate::shared_state::{WorkerCommandAdmission, WorkerSharedState};
use crate::shutdown_after_idle::{
    ShutdownAfterIdleRequest, TicketIntakeReadyShutdownHook, is_ticket_intake_role,
    take_shutdown_request_after_status,
};
use crate::spawn::registry::SpawnedWorkerRegistry;
use crate::spawn::tool::sub_worker_spawn_tool;
use crate::worker::{SystemItemCommitter, Worker, WorkerError, WorkerRunResult};
use protocol::{
    AlertLevel, AlertSource, CommandEvent as ProtocolCommandEvent,
    CommandSnapshot as ProtocolCommandSnapshot, CommandStatus as ProtocolCommandStatus,
    CommandStream as ProtocolCommandStream, CommandStreamSlice as ProtocolCommandStreamSlice,
    ErrorCode, Event, Method, RewindTargetId, RunResult, TurnResult, UploadedFileRef,
    WorkerBusyState, WorkerCommandAcknowledgement, WorkerCommandDisposition, WorkerCommandEnvelope,
    WorkerCommandKind, WorkerMaintenanceState, WorkerRunState, WorkerState, WorkerStatus,
};
use workdir::{
    CommandEvent as WorkdirCommandEvent, CommandSnapshot as WorkdirCommandSnapshot,
    CommandStatus as WorkdirCommandStatus, CommandStream as WorkdirCommandStream, WorkdirSession,
};

// ---------------------------------------------------------------------------
// WorkerHandle — client-facing, Clone-able
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct WorkerHandle {
    method_tx: mpsc::Sender<Method>,
    working_event_tx: broadcast::Sender<Event>,
    pub shared_state: Arc<WorkerSharedState>,
    pub runtime_dir: Arc<RuntimeDir>,
    pub alerter: Alerter,
    pub in_flight: InFlightEvents,
    /// Segment-log mirror + session-entry channel. The IPC server snapshots
    /// it on every new connection (Event::Snapshot) and forwards
    /// subsequent commits (Event::Entry) on the receiver.
    pub sink: SegmentLogSink,
    spawned_registry: Arc<SpawnedWorkerRegistry>,
    artifact_store: Arc<dyn Store>,
    session_id: session_store::SessionId,
    pending_activations: Arc<std::sync::Mutex<crate::worker::PendingActivationState>>,
}

impl WorkerHandle {
    pub async fn send(&self, method: Method) -> Result<(), mpsc::error::SendError<Method>> {
        self.method_tx.send(method).await
    }

    pub fn upload_file(
        &self,
        file_name: &str,
        media_type: &str,
        content: &[u8],
    ) -> Result<UploadedFileRef, session_store::StoreError> {
        self.artifact_store.write_uploaded_file(
            self.session_id,
            file_name,
            media_type,
            content,
            session_store::UploadedFileLimits::default(),
        )
    }

    pub fn upload_file_with_context(
        &self,
        file_name: &str,
        media_type: &str,
        content: &[u8],
        context: &session_store::UploadedFileUploadContext,
    ) -> Result<UploadedFileRef, session_store::StoreError> {
        self.artifact_store.write_uploaded_file_with_context(
            self.session_id,
            file_name,
            media_type,
            content,
            context,
            session_store::UploadedFileLimits::default(),
        )
    }

    pub fn delete_uploaded_file(
        &self,
        artifact_id: &str,
    ) -> Result<bool, session_store::StoreError> {
        self.artifact_store
            .delete_uploaded_file(self.session_id, artifact_id)
    }

    pub fn delete_uncommitted_uploaded_files(&self) -> Result<u64, session_store::StoreError> {
        self.artifact_store
            .delete_uncommitted_uploaded_files(self.session_id)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.working_event_tx.subscribe()
    }

    pub fn committed_entries(&self) -> Vec<LogEntry> {
        self.sink.subscribe_with_snapshot().0
    }

    pub fn snapshot_event(&self) -> Event {
        self.snapshot_event_with_entry_subscription().0
    }

    pub(crate) fn snapshot_event_with_entry_subscription(
        &self,
    ) -> (Event, broadcast::Receiver<LogEntry>) {
        let (entries, entry_rx, in_flight) = {
            let in_flight_guard = self.in_flight.snapshot_guard();
            let (entries, entry_rx) = self.sink.subscribe_with_snapshot();
            let in_flight = snapshot_from_guard(&in_flight_guard);
            (entries, entry_rx, in_flight)
        };
        let mut session =
            session_store::public_snapshot::project_current_session_snapshot(&entries);
        session.pending_submissions = self
            .pending_activations
            .lock()
            .expect("pending activation state poisoned")
            .snapshot();
        let event = Event::Snapshot {
            session,
            greeting: self.shared_state.greeting.clone(),
            state: self.shared_state.snapshot(),
            in_flight,
            internal_workers: self.spawned_registry.internal_worker_snapshots(),
        };
        (event, entry_rx)
    }

    pub async fn completion_entries(
        &self,
        kind: protocol::CompletionKind,
        prefix: &str,
    ) -> Vec<protocol::CompletionEntry> {
        match kind {
            protocol::CompletionKind::File => {
                let Some(view) = self.shared_state.fs_view() else {
                    return Vec::new();
                };
                view.list_file_completions(prefix)
                    .await
                    .into_iter()
                    .map(|candidate| protocol::CompletionEntry {
                        value: candidate.path,
                        is_dir: candidate.is_dir,
                    })
                    .collect()
            }
        }
    }

    /// Broadcast an event to all listeners (including socket clients).
    pub fn send_event(&self, event: Event) -> Result<usize, broadcast::error::SendError<Event>> {
        self.working_event_tx.send(event)
    }

    /// Emit a user-facing alert. Thin wrapper over `Alerter::alert`.
    pub fn alert(&self, level: AlertLevel, source: AlertSource, message: String) {
        self.alerter.alert(level, source, message);
    }
}

fn command_admission_disposition(
    admission: WorkerCommandAdmission,
) -> Result<(), WorkerCommandDisposition> {
    match admission {
        WorkerCommandAdmission::Accepted => Ok(()),
        WorkerCommandAdmission::Retry | WorkerCommandAdmission::StaleCommandId => {
            Err(WorkerCommandDisposition::StaleCommandId)
        }
        WorkerCommandAdmission::Conflict => Err(WorkerCommandDisposition::Conflict),
        WorkerCommandAdmission::ExecutionGenerationMismatch => {
            Err(WorkerCommandDisposition::StaleExecutionGeneration)
        }
        WorkerCommandAdmission::StateRevisionMismatch => {
            Err(WorkerCommandDisposition::StaleWorkerStateRevision)
        }
    }
}

fn validate_command(
    envelope: WorkerCommandEnvelope,
    kind: WorkerCommandKind,
    shared_state: &WorkerSharedState,
) -> Result<(), WorkerCommandDisposition> {
    command_admission_disposition(shared_state.admit_command(envelope, kind, true))
}

fn validate_shutdown_command(
    envelope: WorkerCommandEnvelope,
    shared_state: &WorkerSharedState,
) -> Result<(), WorkerCommandDisposition> {
    match shared_state.admit_command(envelope, WorkerCommandKind::Shutdown, false) {
        WorkerCommandAdmission::Accepted | WorkerCommandAdmission::Retry => Ok(()),
        admission => command_admission_disposition(admission),
    }
}

fn acknowledge_command(
    working_event_tx: &broadcast::Sender<Event>,
    shared_state: &WorkerSharedState,
    command_id: u64,
    command: WorkerCommandKind,
    disposition: WorkerCommandDisposition,
) {
    shared_state.complete_command(command_id, command, disposition);
    let _ = working_event_tx.send(Event::CommandAcknowledged {
        acknowledgement: WorkerCommandAcknowledgement {
            command_id,
            command,
            disposition,
            state: shared_state.snapshot(),
        },
    });
}

fn reject_invalid_command_state(
    working_event_tx: &broadcast::Sender<Event>,
    shared_state: &WorkerSharedState,
    envelope: WorkerCommandEnvelope,
    command: WorkerCommandKind,
) {
    acknowledge_command(
        working_event_tx,
        shared_state,
        envelope.command_id,
        command,
        WorkerCommandDisposition::InvalidState,
    );
}

async fn set_controller_state(
    shared_state: &Arc<WorkerSharedState>,
    runtime_dir: &RuntimeDir,
    working_event_tx: &broadcast::Sender<Event>,
    state: WorkerState,
) -> protocol::WorkerStateSnapshot {
    let snapshot = shared_state.transition(state);
    let _ = runtime_dir.write_status(shared_state).await;
    let _ = working_event_tx.send(Event::WorkerState {
        snapshot: snapshot.clone(),
    });
    snapshot
}

async fn set_controller_status(
    shared_state: &Arc<WorkerSharedState>,
    runtime_dir: &RuntimeDir,
    working_event_tx: &broadcast::Sender<Event>,
    status: WorkerStatus,
) {
    let state = match status {
        WorkerStatus::Idle | WorkerStatus::Stopped => WorkerState::Idle,
        WorkerStatus::Running => WorkerState::Busy(WorkerBusyState::Run(WorkerRunState::Running)),
        WorkerStatus::Paused => WorkerState::Busy(WorkerBusyState::Run(WorkerRunState::Paused)),
    };
    set_controller_state(shared_state, runtime_dir, working_event_tx, state).await;
}

async fn finish_controller_run<C, St>(
    worker: &mut Worker<C, St>,
    shared_state: &Arc<WorkerSharedState>,
    runtime_dir: &RuntimeDir,
    working_event_tx: &broadcast::Sender<Event>,
    new_status: WorkerStatus,
) where
    C: LlmClient + Clone + 'static,
    St: Store + WorkerMetadataStore + Clone + 'static,
{
    // history / user_segments are no longer mirrored on WorkerSharedState —
    // clients reconstruct them from `Event::Snapshot` + live
    // `Event::Entry` deliveries driven by the session-log sink. The
    // lifecycle hook/task registry observes the terminal commit separately.
    //
    // In-flight blocks are run-local streaming state, not durable transcript.
    // Any block not cleared by a committed AssistantItem must be discarded at
    // the terminal run boundary so reconnect snapshots cannot append stale
    // partial text/tool arguments after newer entries.
    worker.clear_in_flight_events();
    set_controller_status(shared_state, runtime_dir, working_event_tx, new_status).await;
}

/// Pending turn launch staged by an event handler for the next outer-loop
/// iteration. Each variant carries the input needed by the corresponding
/// `Worker::*` entry point — `RunForNotification` carries none because
/// `worker.run_for_notification()` drains the NotifyBuffer on its own.
enum PendingRun {
    Submit(crate::worker::PendingSubmission),
    /// Self-initiated turn kicked from the notify buffer. The carried
    /// `InvokeKind` is the trigger that flipped the Worker from IDLE
    /// (Notify or WorkerEvent) and is recorded by the Invoke marker
    /// committed at the start of `worker.run_for_notification`.
    RunForNotification {
        invoke_kind: protocol::InvokeKind,
        notification_request_id: Option<String>,
    },
    Resume,
}

fn resolved_input_source<St: Store + Clone>(
    pending_submissions: &crate::worker::PendingSubmissionHandle<St>,
    source: &protocol::AuthenticatedInputSource,
) -> (String, session_store::LoggedSessionHistoryOrigin) {
    if matches!(source, protocol::AuthenticatedInputSource::UntrustedWire) {
        return (
            pending_submissions.direct_client_namespace(),
            session_store::LoggedSessionHistoryOrigin::LegacyUnknown,
        );
    }
    (
        source.namespace(),
        crate::worker::authenticated_input_provenance(source),
    )
}

fn durable_parent_notification_target<St: Store + Clone + Send + Sync + 'static>(
    pending_submissions: crate::worker::PendingSubmissionHandle<St>,
    notify_buffer: NotifyBuffer,
) -> crate::spawn::tool::ParentNotificationTarget {
    crate::spawn::tool::ParentNotificationTarget::Durable(Arc::new(move |method| {
        let Method::NotifyTracked {
            notification_request_id,
            message,
            auto_run,
            source,
        } = method
        else {
            return;
        };
        let (source_namespace, provenance) = resolved_input_source(&pending_submissions, &source);
        match pending_submissions.accept_notification_from_source(
            notification_request_id.clone(),
            message,
            source_namespace.clone(),
            provenance,
            auto_run,
        ) {
            Ok(_) if !auto_run => {
                stage_pending_notification(
                    &pending_submissions,
                    &notify_buffer,
                    &source_namespace,
                    &notification_request_id,
                );
            }
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(%error, "failed to durably accept SubWorker notification");
            }
        }
    }))
}

fn stage_pending_notification<St: Store + Clone>(
    pending_submissions: &crate::worker::PendingSubmissionHandle<St>,
    notify_buffer: &NotifyBuffer,
    source_namespace: &str,
    notification_request_id: &str,
) -> bool {
    let Some(notification) =
        pending_submissions.prepare_notification(source_namespace, notification_request_id)
    else {
        return false;
    };
    let extension = pending_submissions.notification_activation_extension();
    notify_buffer.push_durable_notify(
        notification.message,
        notification.auto_run,
        notification.provenance,
        extension,
    );
    true
}

fn stage_oldest_passive_notification<St: Store + Clone>(
    pending_submissions: &crate::worker::PendingSubmissionHandle<St>,
    notify_buffer: &NotifyBuffer,
) -> bool {
    pending_submissions
        .next_passive_notification_identity()
        .is_some_and(|(source_namespace, request_id)| {
            stage_pending_notification(
                pending_submissions,
                notify_buffer,
                &source_namespace,
                &request_id,
            )
        })
}

fn prepare_pending_run<St: Store + Clone>(
    pending_submissions: &crate::worker::PendingSubmissionHandle<St>,
    notify_buffer: &NotifyBuffer,
    fence: Option<(u64, &str)>,
) -> Result<Option<PendingRun>, crate::worker::PendingSubmissionError> {
    let staged_passive_notification = pending_submissions.activating_passive_notification_id();
    Ok(match pending_submissions.prepare_next_activation(fence)? {
        Some(crate::worker::PendingActivation::Submission(submission)) => {
            if staged_passive_notification.is_some() {
                let extension = pending_submissions.notification_activation_extension();
                debug_assert!(notify_buffer.replace_durable_notification_extension(extension));
            }
            Some(PendingRun::Submit(submission))
        }
        Some(crate::worker::PendingActivation::Notification(notification)) => {
            let extension = pending_submissions.notification_activation_extension();
            let notification_request_id = notification.notification_request_id.clone();
            notify_buffer.push_durable_notify(
                notification.message,
                notification.auto_run,
                notification.provenance,
                extension,
            );
            Some(PendingRun::RunForNotification {
                invoke_kind: protocol::InvokeKind::Notify,
                notification_request_id: Some(notification_request_id),
            })
        }
        None => None,
    })
}

impl PendingRun {
    /// Whether this turn was kicked off by the parent (via `Method::Submit`
    /// or `Method::Resume`). Used by [`drive_turn`] to gate upward
    /// `WorkerEvent::TurnEnded` / `WorkerEvent::Errored` reports so the parent
    /// only sees completion signals for work it actually delegated.
    /// `RunForNotification` covers self-initiated turns kicked from the
    /// notify buffer (Notify / inbound WorkerEvent) and stays silent.
    fn is_parent_originated(&self) -> bool {
        match self {
            PendingRun::Submit(_) | PendingRun::Resume => true,
            PendingRun::RunForNotification { .. } => false,
        }
    }
}

// ---------------------------------------------------------------------------
// WorkerController — actor that owns a Worker
// ---------------------------------------------------------------------------

pub type ShutdownReceiver = oneshot::Receiver<()>;

/// Client transport exposed by a Worker controller.
///
/// Process-hosted Workers use a Unix socket for external attach clients. Runtimes
/// that retain the returned [`WorkerHandle`] in the same process can disable that
/// redundant listener and drive the controller directly through its channels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WorkerControllerTransport {
    #[default]
    UnixSocket,
    InProcess,
}

pub struct WorkerController;

impl WorkerController {
    pub async fn spawn<C, St>(
        worker: Worker<C, St>,
        runtime_base: &Path,
        bash_output_dir: &Path,
    ) -> Result<(WorkerHandle, ShutdownReceiver), std::io::Error>
    where
        C: LlmClient + Clone + 'static,
        St: Store + WorkerMetadataStore + Clone + Send + Sync + 'static,
    {
        Self::spawn_inner(
            worker,
            runtime_base,
            bash_output_dir,
            false,
            None,
            WorkerControllerTransport::UnixSocket,
        )
        .await
    }

    /// Spawn a direct Worker while letting an in-process host select the
    /// controller transport explicitly.
    pub async fn spawn_with_transport<C, St>(
        worker: Worker<C, St>,
        runtime_base: &Path,
        bash_output_dir: &Path,
        transport: WorkerControllerTransport,
    ) -> Result<(WorkerHandle, ShutdownReceiver), std::io::Error>
    where
        C: LlmClient + Clone + 'static,
        St: Store + WorkerMetadataStore + Clone + Send + Sync + 'static,
    {
        Self::spawn_inner(
            worker,
            runtime_base,
            bash_output_dir,
            false,
            None,
            transport,
        )
        .await
    }

    /// Spawn a Worker owned by `worker-runtime`.
    ///
    /// The controller uses an ephemeral directory for Unix sockets while tool
    /// spill artifacts use the separately supplied Worker-owned temporary path.
    /// Runtime-managed Workers do not write legacy pid/status/manifest liveness
    /// projections.
    pub async fn spawn_runtime_managed<C, St>(
        worker: Worker<C, St>,
        runtime_base: &Path,
        bash_output_dir: &Path,
    ) -> Result<(WorkerHandle, ShutdownReceiver), std::io::Error>
    where
        C: LlmClient + Clone + 'static,
        St: Store + WorkerMetadataStore + Clone + Send + Sync + 'static,
    {
        Self::spawn_inner(
            worker,
            runtime_base,
            bash_output_dir,
            true,
            None,
            WorkerControllerTransport::UnixSocket,
        )
        .await
    }

    /// Spawn into an exact persistent `runs/<generation>` directory.
    pub async fn spawn_runtime_managed_run<C, St>(
        worker: Worker<C, St>,
        run_dir: &Path,
        bash_output_dir: &Path,
    ) -> Result<(WorkerHandle, ShutdownReceiver), std::io::Error>
    where
        C: LlmClient + Clone + 'static,
        St: Store + WorkerMetadataStore + Clone + Send + Sync + 'static,
    {
        Self::spawn_runtime_managed_run_with_transport(
            worker,
            run_dir,
            bash_output_dir,
            WorkerControllerTransport::UnixSocket,
        )
        .await
    }

    /// Spawn into an exact persistent `runs/<generation>` directory using the
    /// requested client transport.
    pub async fn spawn_runtime_managed_run_with_transport<C, St>(
        worker: Worker<C, St>,
        run_dir: &Path,
        bash_output_dir: &Path,
        transport: WorkerControllerTransport,
    ) -> Result<(WorkerHandle, ShutdownReceiver), std::io::Error>
    where
        C: LlmClient + Clone + 'static,
        St: Store + WorkerMetadataStore + Clone + Send + Sync + 'static,
    {
        let parent = run_dir
            .parent()
            .ok_or_else(|| std::io::Error::other("run path has no parent"))?;
        Self::spawn_inner(
            worker,
            parent,
            bash_output_dir,
            true,
            Some(run_dir),
            transport,
        )
        .await
    }

    async fn spawn_inner<C, St>(
        worker: Worker<C, St>,
        runtime_base: &Path,
        bash_output_dir: &Path,
        runtime_managed: bool,
        runtime_run: Option<&Path>,
        transport: WorkerControllerTransport,
    ) -> Result<(WorkerHandle, ShutdownReceiver), std::io::Error>
    where
        C: LlmClient + Clone + 'static,
        St: Store + WorkerMetadataStore + Clone + Send + Sync + 'static,
    {
        let session = worker.workdir_session().cloned();
        let result = Self::spawn_initialized(
            worker,
            runtime_base,
            bash_output_dir,
            runtime_managed,
            runtime_run,
            transport,
        )
        .await;
        if result.is_err()
            && let Some(session) = session
            && let Err(error) = session.close().await
        {
            tracing::warn!(%error, "Workdir session close after controller startup failure failed");
        }
        result
    }

    async fn spawn_initialized<C, St>(
        mut worker: Worker<C, St>,
        runtime_base: &Path,
        bash_output_dir: &Path,
        runtime_managed: bool,
        runtime_run: Option<&Path>,
        transport: WorkerControllerTransport,
    ) -> Result<(WorkerHandle, ShutdownReceiver), std::io::Error>
    where
        C: LlmClient + Clone + 'static,
        St: Store + WorkerMetadataStore + Clone + Send + Sync + 'static,
    {
        // === 1. Initialization (channels / RuntimeDir / worker-immutable
        //         snapshots / SpawnedWorkerRegistry / alerter attach /
        //         bash-output scope) ===
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let (method_tx, method_rx) = mpsc::channel::<Method>(32);
        let (working_event_tx, _) = broadcast::channel::<Event>(256);
        let alerter = Alerter::new(working_event_tx.clone());
        let in_flight = InFlightEvents::new(working_event_tx.clone());
        worker.attach_in_flight_events(in_flight.clone());

        // Runtime directory is created before tool registration because it owns
        // bounded tool artifacts, and before initial status/history writes consume
        // the greeting we build after registration is complete.
        let runtime_dir = Arc::new(if let Some(run_dir) = runtime_run {
            RuntimeDir::create_worker_run(run_dir).await?
        } else if runtime_managed {
            RuntimeDir::create_transient(runtime_base, &worker.manifest().worker.name).await?
        } else {
            RuntimeDir::create(runtime_base, &worker.manifest().worker.name).await?
        });

        let spawner_name = worker.manifest().worker.name.clone();
        let self_parent_socket = worker.callback_socket().cloned();
        let loaded_registry = SpawnedWorkerRegistry::load_from_worker_state_with_reclaim(
            runtime_dir.clone(),
            worker.store().clone(),
            spawner_name.clone(),
            Some(worker.scope().clone()),
        )
        .await?;
        let reclaimed_unreachable = loaded_registry.reclaimed_unreachable;
        let spawned_registry = loaded_registry.registry;
        if reclaimed_unreachable {
            worker.push_notify(
                "Restored Worker state contained unreachable delegated child Workers; their delegated write scopes were reclaimed before resume."
                    .to_string(),
                false,
            );
        }

        // Hand the alerter to the Worker so internal operations (compaction,
        // AGENTS.md ingestion during the first turn) can emit user-facing
        // notifications on the same channel.
        worker.attach_alerter(alerter.clone());
        // Also hand the raw broadcast sender so Worker-internal operations
        // can emit typed lifecycle `Event`s (currently: compact progress).
        worker.attach_internal_worker_registry(spawned_registry.clone());
        worker.attach_working_event_tx(working_event_tx.clone());

        // Bash spill artifacts are owned by the stable Worker identity rather
        // than a controller session/run generation. Push a recursive
        // `allow(Read)` for the exact tool output path into the Worker's shared
        // runtime scope so the Workdir session and system prompt stay aligned.
        let bash_output_dir = bash_output_dir.to_path_buf();
        std::fs::create_dir_all(&bash_output_dir).map_err(|e| {
            std::io::Error::other(format!(
                "create bash output dir {}: {e}",
                bash_output_dir.display()
            ))
        })?;
        worker
            .add_scope_rules([manifest::ScopeRule {
                target: bash_output_dir.clone(),
                permission: manifest::Permission::Read,
                recursive: true,
            }])
            .map_err(std::io::Error::other)?;

        // === 1.5. Direct writer wiring ===
        //
        // Engine callbacks fire `on_history_append` for each assistant
        // item / tool result that lands in history. With the sync
        // writer in place, the callback commits each item directly
        // through a `LogWriterHandle` (no mpsc ferry, no drain task).
        // The same handle is type-erased into a `SystemItemCommitter`
        // and handed to the interceptor for `SystemItem` commits, so
        // assistant / tool / system items all share one commit path.
        let writer_for_system: Arc<dyn SystemItemCommitter> = Arc::new(worker.log_writer_handle());
        worker.attach_log_writer(writer_for_system);
        worker.wire_history_persistence();

        // === 2. Engine event bridge wiring ===
        wire_event_bridges_on_engine(&mut worker, &working_event_tx, &alerter, &in_flight);

        // === 3. Tool registration (builtin / memory / spawn-orchestration) ===
        let fs_for_view = register_worker_tools(
            &mut worker,
            bash_output_dir,
            runtime_base.to_path_buf(),
            spawned_registry.clone(),
            Some(method_tx.downgrade()),
            None,
        )
        .await?;
        if let Some(session) = fs_for_view.as_ref() {
            wire_workdir_command_events(session, &in_flight);
        }

        // Intake role Workers self-terminate only after a successful
        // TicketIntakeReady turn has fully settled back to Idle. The request
        // is transient controller state, not model-visible context or ticket
        // claim metadata.
        let shutdown_after_idle = ShutdownAfterIdleRequest::default();
        worker.add_post_tool_call_hook(TicketIntakeReadyShutdownHook::new(
            shutdown_after_idle.clone(),
            is_ticket_intake_role(worker.runtime_ticket_role()),
        ));

        // Materialise pending tool factories so the greeting reflects
        // the actual registered set instead of a hand-maintained mirror.
        worker.engine().tool_server_handle().flush_pending();

        // === 4. Initial runtime files + WorkerSharedState + WorkerHandle +
        //         SocketServer ===
        let manifest_toml = toml::to_string_pretty(worker.manifest()).unwrap_or_default();
        worker
            .recover_unfinished_compaction()
            .await
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        let greeting = build_greeting(&worker);
        let execution_generation = runtime_dir
            .path()
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.parse::<u64>().ok())
            .filter(|generation| *generation > 0)
            .unwrap_or(1);
        let shared_state = Arc::new(WorkerSharedState::new_with_generation(
            worker.manifest().worker.name.clone(),
            worker.segment_id(),
            manifest_toml.clone(),
            greeting,
            execution_generation,
        ));
        if let Some(fs_for_view) = fs_for_view {
            shared_state.set_fs_view(crate::fs_view::WorkerFsView::new(fs_for_view));
        }
        runtime_dir.write_manifest(&manifest_toml).await?;
        runtime_dir.write_status(&shared_state).await?;

        let artifact_store: Arc<dyn Store> = Arc::new(worker.store().clone());
        let session_id = worker.session_id();
        let pending_activations = worker.pending_activation_state();
        let handle = WorkerHandle {
            method_tx,
            working_event_tx: working_event_tx.clone(),
            shared_state: shared_state.clone(),
            runtime_dir: runtime_dir.clone(),
            alerter: alerter.clone(),
            in_flight: in_flight.clone(),
            sink: worker.sink(),
            spawned_registry: spawned_registry.clone(),
            artifact_store,
            session_id,
            pending_activations,
        };

        let socket_server = match transport {
            WorkerControllerTransport::UnixSocket => Some(SocketServer::start(&handle).await?),
            WorkerControllerTransport::InProcess => None,
        };

        // === 5. controller_loop ===
        // Clone cancel sender and notification buffer before moving worker
        // into the controller task so the in-flight turn can be reached
        // via these handles while worker itself is borrowed by drive_turn.
        let cancel_tx = worker.engine_mut().cancel_sender();
        let pause_tx = worker.engine_mut().pause_sender();
        let notify_buffer = worker.notify_buffer_handle();

        tokio::spawn(controller_loop(
            worker,
            method_rx,
            working_event_tx,
            shared_state,
            runtime_dir,
            cancel_tx,
            pause_tx,
            notify_buffer,
            self_parent_socket,
            spawner_name,
            spawned_registry,
            shutdown_tx,
            socket_server,
            shutdown_after_idle,
        ));

        Ok((handle, shutdown_rx))
    }
}

pub(crate) fn wire_workdir_command_events(
    session: &Arc<dyn WorkdirSession>,
    in_flight: &InFlightEvents,
) {
    in_flight.replace_command_snapshot(protocol_command_snapshots(session.as_ref()));
    let Some(mut events) = session.subscribe_command_events() else {
        return;
    };
    // Keep only a weak reference in the observer task. Holding the session
    // strongly here would keep its broadcast sender alive forever and prevent
    // the receiver from observing closure during Worker teardown.
    let session = Arc::downgrade(session);
    let in_flight = in_flight.clone();
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(event) => in_flight.publish_command_event(protocol_command_event(event)),
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let Some(session) = session.upgrade() else {
                        break;
                    };
                    in_flight
                        .replace_command_snapshot(protocol_command_snapshots(session.as_ref()));
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

fn protocol_command_snapshots(session: &dyn WorkdirSession) -> Vec<ProtocolCommandSnapshot> {
    session
        .command_snapshot()
        .into_iter()
        .map(protocol_command_snapshot)
        .collect()
}

fn protocol_command_snapshot(snapshot: WorkdirCommandSnapshot) -> ProtocolCommandSnapshot {
    ProtocolCommandSnapshot {
        command_id: snapshot.command_id,
        tool_call_id: snapshot.tool_call_id,
        status: protocol_command_status(snapshot.status),
        started_at_ms: snapshot.started_at_ms,
        observed_at_ms: snapshot.observed_at_ms,
        last_output_at_ms: snapshot.last_output_at_ms,
        stdout: ProtocolCommandStreamSlice {
            start_offset: snapshot.stdout.start_offset,
            end_offset: snapshot.stdout.end_offset,
            content: snapshot.stdout.content,
            truncated: snapshot.stdout.truncated,
        },
        stderr: ProtocolCommandStreamSlice {
            start_offset: snapshot.stderr.start_offset,
            end_offset: snapshot.stderr.end_offset,
            content: snapshot.stderr.content,
            truncated: snapshot.stderr.truncated,
        },
        exit_code: snapshot.exit_code,
    }
}

fn protocol_command_event(event: WorkdirCommandEvent) -> ProtocolCommandEvent {
    match event {
        WorkdirCommandEvent::Started {
            command_id,
            tool_call_id,
            observed_at_ms,
        } => ProtocolCommandEvent::Started {
            command_id,
            tool_call_id,
            observed_at_ms,
        },
        WorkdirCommandEvent::Output {
            command_id,
            stream,
            start_offset,
            end_offset,
            content,
            observed_at_ms,
        } => ProtocolCommandEvent::Output {
            command_id,
            stream: match stream {
                WorkdirCommandStream::Stdout => ProtocolCommandStream::Stdout,
                WorkdirCommandStream::Stderr => ProtocolCommandStream::Stderr,
            },
            start_offset,
            end_offset,
            content,
            observed_at_ms,
        },
        WorkdirCommandEvent::Terminal {
            command_id,
            status,
            exit_code,
            stdout_end_offset,
            stderr_end_offset,
            observed_at_ms,
        } => ProtocolCommandEvent::Terminal {
            command_id,
            status: protocol_command_status(status),
            exit_code,
            stdout_end_offset,
            stderr_end_offset,
            observed_at_ms,
        },
    }
}

fn protocol_command_status(status: WorkdirCommandStatus) -> ProtocolCommandStatus {
    match status {
        WorkdirCommandStatus::Running => ProtocolCommandStatus::Running,
        WorkdirCommandStatus::Completed => ProtocolCommandStatus::Completed,
        WorkdirCommandStatus::Failed => ProtocolCommandStatus::Failed,
        WorkdirCommandStatus::TimedOut => ProtocolCommandStatus::TimedOut,
        WorkdirCommandStatus::Cancelled => ProtocolCommandStatus::Cancelled,
    }
}

/// Wire the per-event broadcast bridges on the Worker's Engine. Each callback
/// re-publishes a worker-level signal as a `protocol::Event` on `working_event_tx`
/// so subscribers (TUI, socket clients) get a single typed stream.
///
/// `Worker::wire_history_persistence` is called separately to wire the
/// per-item history commit callback so every assistant / tool item
/// landing in `worker.history` becomes a singular `LogEntry::AnnotatedAssistantItem`
/// / `AnnotatedToolResult` commit through the sync writer.
pub(crate) fn wire_event_bridges_on_engine<C, St>(
    worker: &mut Worker<C, St>,
    working_event_tx: &broadcast::Sender<Event>,
    alerter: &Alerter,
    in_flight: &InFlightEvents,
) where
    C: LlmClient + Clone + 'static,
    St: Store + WorkerMetadataStore + Clone + 'static,
{
    let ai_activity = worker.ai_activity_counter();
    let worker = worker.engine_mut();

    let tx = working_event_tx.clone();
    worker.on_turn_start(move |turn| {
        let _ = tx.send(Event::TurnStart { turn });
    });

    let tx = working_event_tx.clone();
    worker.on_turn_end(move |turn| {
        let _ = tx.send(Event::TurnEnd {
            turn,
            result: TurnResult::Finished,
        });
    });

    let tx = working_event_tx.clone();
    worker.on_llm_call_start(move |llm_call| {
        let _ = tx.send(Event::LlmCallStart { llm_call });
    });

    let tx = working_event_tx.clone();
    worker.on_llm_call_end(move |llm_call| {
        let _ = tx.send(Event::LlmCallEnd { llm_call });
    });

    let tx = working_event_tx.clone();
    worker.on_llm_retry(move |llm_call, notice| {
        let _ = tx.send(Event::LlmRetry {
            llm_call,
            failed_attempt: notice.failed_attempt,
            max_attempts: notice.max_attempts,
            wait_ms: notice.wait.as_millis() as u64,
            elapsed_ms: notice.elapsed.as_millis() as u64,
            status: notice.status,
            error: notice.error.clone(),
        });
    });

    let tx = working_event_tx.clone();
    worker.on_llm_continuation(move |llm_call, attempt, max_attempts, reason| {
        let _ = tx.send(Event::LlmContinuation {
            llm_call,
            attempt,
            max_attempts,
            reason: reason.to_owned(),
        });
    });

    let in_flight_text = in_flight.clone();
    let activity = ai_activity.clone();
    worker.on_text_block(move |block| {
        let block_id = in_flight_text.start_text_block();
        let in_flight_d = in_flight_text.clone();
        let activity_d = activity.clone();
        block.on_delta(move |text| {
            activity_d.fetch_add(1, Ordering::SeqCst);
            in_flight_d.text_delta(block_id, text.to_owned());
        });
        let in_flight_s = in_flight_text.clone();
        let activity_s = activity.clone();
        block.on_stop(move |text| {
            if !text.is_empty() {
                activity_s.fetch_add(1, Ordering::SeqCst);
            }
            in_flight_s.text_done(block_id, text.to_owned());
        });
    });

    let in_flight_thinking = in_flight.clone();
    let activity = ai_activity.clone();
    worker.on_thinking_block(move |block| {
        // Start fires unconditionally so the TUI can show "Thinking..."
        // even when the provider doesn't emit plaintext deltas.
        activity.fetch_add(1, Ordering::SeqCst);
        let block_id = in_flight_thinking.thinking_start();
        let in_flight_d = in_flight_thinking.clone();
        let activity_d = activity.clone();
        block.on_delta(move |text| {
            activity_d.fetch_add(1, Ordering::SeqCst);
            in_flight_d.thinking_delta(block_id, text.to_owned());
        });
        let in_flight_s = in_flight_thinking.clone();
        let activity_s = activity.clone();
        block.on_stop(move |text| {
            if !text.is_empty() {
                activity_s.fetch_add(1, Ordering::SeqCst);
            }
            in_flight_s.thinking_done(block_id, text.to_owned());
        });
    });

    let in_flight_tool = in_flight.clone();
    let activity = ai_activity.clone();
    worker.on_tool_use_block(move |start, block| {
        activity.fetch_add(1, Ordering::SeqCst);
        let block_id = in_flight_tool.tool_call_start(start.id.clone(), start.name.clone());
        let id_for_delta = start.id.clone();
        let in_flight_d = in_flight_tool.clone();
        let activity_d = activity.clone();
        block.on_delta(move |json| {
            activity_d.fetch_add(1, Ordering::SeqCst);
            in_flight_d.tool_call_args_delta(block_id, id_for_delta.clone(), json.to_owned());
        });
        let in_flight_s = in_flight_tool.clone();
        let activity_s = activity.clone();
        block.on_stop(move |call| {
            activity_s.fetch_add(1, Ordering::SeqCst);
            in_flight_s.tool_call_done(block_id, call.id.clone(), call.input.to_string());
        });
    });

    let tx = working_event_tx.clone();
    let activity = ai_activity.clone();
    worker.on_tool_result(move |result| {
        activity.fetch_add(1, Ordering::SeqCst);
        let _ = tx.send(Event::ToolResult {
            id: result.tool_use_id.clone(),
            summary: result.summary.clone(),
            output: result.content.clone(),
            disposition: Some(match result.disposition {
                agen::ToolResultDisposition::Success => protocol::ToolResultDisposition::Success,
                agen::ToolResultDisposition::Error => protocol::ToolResultDisposition::Error,
                agen::ToolResultDisposition::Interrupted => {
                    protocol::ToolResultDisposition::Interrupted
                }
                agen::ToolResultDisposition::Cancelled => {
                    protocol::ToolResultDisposition::Cancelled
                }
                agen::ToolResultDisposition::OutcomeUnknown => {
                    protocol::ToolResultDisposition::OutcomeUnknown
                }
            }),
            is_error: result.is_error,
        });
    });

    let tx = working_event_tx.clone();
    worker.on_usage(move |event| {
        let _ = tx.send(Event::Usage {
            input_tokens: event.input_tokens,
            output_tokens: event.output_tokens,
            cache_read_input_tokens: event.cache_read_input_tokens,
        });
    });

    let tx = working_event_tx.clone();
    worker.on_error(move |event| {
        let _ = tx.send(Event::Error {
            code: ErrorCode::ProviderError,
            message: event.message.clone(),
        });
    });

    let alerter_for_worker = alerter.clone();
    worker.on_warning(move |message| {
        alerter_for_worker.alert(AlertLevel::Warn, AlertSource::Engine, message.to_owned());
    });

    // History-append broadcasts (previously `Event::SystemMessage`)
    // have been removed: every persistent history item is now committed
    // through the session-log sink as a typed `LogEntry`, and clients
    // see it via `Event::Snapshot` + live `Event::Entry`. The
    // per-item commit channel is wired at the top of this function.
}

/// Register the builtin file-manipulation tools, optional memory tools,
/// and the Worker-orchestration tools (SubWorkerSpawn + comm) on the Worker's
/// Engine. Returns the WorkdirSession handle used to attach a `WorkerFsView` to
/// the shared state.
pub(crate) async fn register_worker_tools<C, St>(
    worker: &mut Worker<C, St>,
    bash_output_dir: PathBuf,
    runtime_base: PathBuf,
    spawned_registry: Arc<SpawnedWorkerRegistry>,
    parent_method_tx: Option<mpsc::WeakSender<Method>>,
    inherited_workdir_tool_broker: Option<workdir::WorkdirToolBroker>,
) -> std::io::Result<Option<workdir::WorkdirSessionHandle>>
where
    C: LlmClient + Clone + 'static,
    St: Store + WorkerMetadataStore + Clone + 'static,
{
    // Worker-immutable snapshots taken before the mutable worker borrow
    // below so the worker borrow doesn't conflict with reads on `worker`.
    let feature_config = worker.manifest().feature.clone();
    let mut workdir_tool_broker = inherited_workdir_tool_broker;
    if feature_config.manage_workdir.enabled && worker.workdir_session().is_none() {
        let workspace_client = worker.workspace_client_handle();
        let broker = workdir::WorkdirToolBroker::new(
            crate::feature::builtin::manage_workdir::WorkspaceAttachedWorkdirSession::handle(
                workspace_client,
            ),
        );
        worker.bind_workdir_session(Some(broker.tool_session()));
        workdir_tool_broker = Some(broker);
    } else if workdir_tool_broker.is_none()
        && let Some(existing) = worker.workdir_session().cloned()
    {
        let broker = workdir::WorkdirToolBroker::new(existing);
        worker.bind_workdir_session(Some(broker.tool_session()));
        workdir_tool_broker = Some(broker);
    }
    let worker_workdir = workdir_tool_broker
        .as_ref()
        .map(workdir::WorkdirToolBroker::tool_session);
    let local_filesystem = worker.local_working_directory().cloned();
    let local_workspace_root = local_filesystem.as_ref().map(|local| local.root.clone());
    let task_feature = worker.task_feature();
    let web_config = worker.manifest().web.clone();
    let mcp_config = worker.manifest().mcp.clone();
    let spawner_name = worker.manifest().worker.name.clone();
    let spawner_manifest = worker.manifest().clone();
    let spawner_workspace_context = worker.workspace_context_handle();
    let pending_submissions = worker.pending_submission_handle();
    let notify_buffer = worker.notify_buffer_handle();
    let durable_parent_notifications =
        durable_parent_notification_target(pending_submissions.clone(), notify_buffer.clone());
    let parent_notifications = match parent_method_tx {
        Some(sender) => crate::spawn::tool::ParentNotificationTarget::with_controller_fallback(
            sender,
            durable_parent_notifications,
        ),
        None => durable_parent_notifications,
    };
    let prompts = worker.prompts().clone();
    let paste_store = worker.store().clone();
    let paste_session_id = worker.session_id();
    worker
        .engine_mut()
        .register_tool(crate::paste_artifact_tool::search_input_artifact_tool(
            paste_store.clone(),
            paste_session_id,
        ));
    worker
        .engine_mut()
        .register_tool(crate::paste_artifact_tool::read_input_artifact_tool(
            paste_store,
            paste_session_id,
        ));
    // Resolve the existing Worker–Workdir binding into the domain provider.
    // Tools only consume the provider handle; they do not own its root, cwd,
    // scope, or lifecycle. No-workdir Workers expose no local tools.
    let (workdir_for_view, tracker) = if let Some(workdir) = worker_workdir {
        let tracker = tools::Tracker::new();
        worker
            .engine_mut()
            .register_tools(tools::core_builtin_tools(
                workdir.clone(),
                tracker.clone(),
                bash_output_dir.clone(),
            ));
        if feature_config.image.enabled && model_supports_image_attachments(&spawner_manifest.model)
        {
            worker
                .engine_mut()
                .register_tool(tools::view_image_tool(workdir.clone()));
        }
        (Some(workdir), Some(tracker))
    } else {
        (None, None)
    };
    if feature_config.web.enabled {
        worker
            .engine_mut()
            .register_tools(tools::web_builtin_tools(web_config));
    }

    let worker_enabled = feature_config.worker.enabled;
    let sub_worker_enabled = feature_config.sub_worker.enabled;
    let mut feature_registry = FeatureRegistryBuilder::new();
    let memory_install_plan = crate::feature::builtin::memory::MemoryFeatureInstallPlan::prepare(
        worker.manifest(),
        worker.workspace_client_handle(),
        worker.prompts().load_full(),
    )?;
    let memory_prompt_contribution = memory_install_plan.as_ref().map(|plan| {
        (
            plan.resident_summary_source.clone(),
            plan.system_prompt_override.clone(),
        )
    });
    let memory_lifecycle_config = memory_install_plan
        .as_ref()
        .map(|plan| plan.resolved_config.clone());
    if let Some(plan) = memory_install_plan {
        feature_registry.add_module(plan.module);
    }
    if let Some(memory_config) = memory_lifecycle_config
        && let Some(memory_lifecycle) =
            crate::feature::builtin::memory_lifecycle::MemoryLifecycleFeature::from_resolved_config(
                worker.manifest_lifecycle_features_enabled(),
                memory_config,
                worker.committed_session_capture_handle(),
                worker.session_extension_handle(),
                worker.workspace_client_handle(),
                spawner_manifest.clone(),
                worker.llm_client_handle(),
                prompts.clone(),
                spawner_workspace_context.clone(),
                worker.working_event_sender(),
            )?
    {
        feature_registry.add_module(memory_lifecycle);
    }
    if sub_worker_enabled && !worker_enabled {
        feature_registry.add_module(
            crate::feature::builtin::manage_worker::sub_worker_control_feature(
                worker.workspace_client_handle(),
                spawned_registry.clone(),
            ),
        );
    }
    if feature_config.task.enabled {
        feature_registry.add_module(task_feature);
    }
    if feature_config.ticket.enabled {
        let ticket_access = crate::feature::builtin::ticket::TicketFeatureAccess {
            authoring: feature_config.ticket.authoring,
            thread: feature_config.ticket.thread,
            intake: feature_config.ticket.intake,
            workflow: feature_config.ticket.workflow,
        };
        // Ticket tools are typed operations over the current workspace Ticket backend.
        // Workspace access must be authority-bound to the Backend Workspace API; the
        // Worker must not fall back to a local `.yoi/tickets` store.
        let workspace_client = worker.workspace_client_handle();
        if !workspace_client.is_available() || workspace_client.workspace_id().is_none() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "ticket tools require Backend Workspace API authority",
            ));
        }
        let ticket_backend = crate::feature::builtin::ticket::TicketFeatureBackend::WorkspaceClient(
            workspace_client,
        );
        feature_registry.add_module(
            crate::feature::builtin::ticket::ticket_tools_feature_with_backend(
                ticket_backend,
                ticket_access,
            ),
        );
    }
    if feature_config.merge_request.any() {
        let workspace_client = worker.workspace_client_handle();
        if !workspace_client.is_available() || workspace_client.workspace_id().is_none() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Merge Request tools require Backend Workspace API authority",
            ));
        }
        feature_registry.add_module(
            crate::feature::builtin::merge_request::MergeRequestFeature::new(
                workspace_client,
                feature_config.merge_request,
            ),
        );
    }
    if feature_config.manage_workdir.enabled {
        // Workdir lifecycle is Workspace control-plane authority. The Worker
        // receives only the injected WorkspaceClient and never Runtime URLs,
        // repository paths, materializer handles, or cleanup sessions.
        let workspace_client = worker.workspace_client_handle();
        let has_workspace_identity = workspace_client.workspace_id().is_some_and(|workspace_id| {
            !workspace_id.is_empty() && !workspace_id.chars().any(char::is_control)
        });
        if !workspace_client.is_available() || !has_workspace_identity {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "manage Workdir tools require Backend Workspace API authority",
            ));
        }
        let shutdown_registry = spawned_registry.clone();
        let reopen_registry = spawned_registry.clone();
        feature_registry.add_module(
            crate::feature::builtin::manage_workdir::ManageWorkdirFeature::with_child_lifecycle(
                workspace_client,
                Arc::new(move || {
                    let child_registry = shutdown_registry.clone();
                    Box::pin(async move { child_registry.shutdown_internal().await })
                }),
                Arc::new(move || reopen_registry.reopen_internal()),
            ),
        );
    }
    if feature_config.workspace_worker_discovery.enabled {
        let workspace_client = worker.workspace_client_handle();
        let has_workspace_identity = workspace_client.workspace_id().is_some_and(|workspace_id| {
            !workspace_id.is_empty() && !workspace_id.chars().any(char::is_control)
        });
        if !workspace_client.is_available() || !has_workspace_identity {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Workspace Worker discovery requires Backend Workspace API authority",
            ));
        }
        feature_registry.add_module(
            crate::feature::builtin::workspace_worker_discovery::workspace_worker_discovery_feature(
                workspace_client,
            ),
        );
    }
    if feature_config.worker.enabled {
        let workspace_client = worker.workspace_client_handle();
        let has_workspace_identity = workspace_client.workspace_id().is_some_and(|workspace_id| {
            !workspace_id.is_empty() && !workspace_id.chars().any(char::is_control)
        });
        if !workspace_client.is_available() || !has_workspace_identity {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Worker tools require Backend Workspace API authority",
            ));
        }
        feature_registry.add_module(
            crate::feature::builtin::manage_worker::manage_worker_feature(
                workspace_client,
                sub_worker_enabled.then(|| spawned_registry.clone()),
                feature_config.worker.direct_spawn,
            ),
        );
    }
    if feature_config.orchestration.enabled {
        feature_registry
            .add_module(crate::feature::builtin::orchestration::orchestration_feature());
    }
    for module in crate::feature::plugin::plugin_tool_features_if_enabled(
        feature_config.plugins.enabled,
        &worker.manifest().plugins,
    ) {
        feature_registry = feature_registry.with_module(module);
    }
    if let Some(workspace_root) = local_workspace_root.as_ref() {
        if let Some(module) =
            crate::feature::mcp::discover_stdio_tool_feature(&mcp_config, workspace_root).await
        {
            feature_registry = feature_registry.with_module(module);
        }
    }

    if feature_config.sub_worker.enabled {
        worker.register_worker_orchestration_instruction();
    }

    let host_worker_observation_provider = worker.worker_observation_provider();
    {
        let workspace_client = worker.workspace_client_handle();
        let engine = worker.engine_mut();

        // Objective tools expose read-only project Objective context through the
        // Backend Workspace API. Workers must not guess local `.yoi/objectives`
        // paths or read Objective files directly.
        if feature_config.objective.enabled {
            if workspace_client.is_available() && workspace_client.workspace_id().is_some() {
                for definition in crate::feature::builtin::objective::workspace_http_objective_tools(
                    workspace_client.clone(),
                ) {
                    engine.register_tool(definition);
                }
            } else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "objective tools require Backend Workspace API authority",
                ));
            }
        }

        let mut observation_providers: Vec<
            Arc<dyn crate::feature::builtin::worker_observation::WorkerObservationProvider>,
        > = Vec::new();

        // Worker-orchestration tools derive child filesystem authority from the
        // active provider-backed Workdir session. The tool remains registered
        // without one so invocation fails deterministically until the parent
        // attaches a Workdir.
        if feature_config.sub_worker.enabled {
            let spawner_workspace_root = local_workspace_root
                .clone()
                .unwrap_or_else(|| PathBuf::from("/"));
            engine.register_tool(sub_worker_spawn_tool(
                spawner_name.clone(),
                spawner_workspace_context,
                parent_notifications,
                runtime_base.clone(),
                bash_output_dir.clone(),
                spawner_workspace_root,
                workdir_tool_broker,
                spawned_registry.clone(),
                spawner_manifest,
                prompts,
            ));
            observation_providers.push(Arc::new(
                crate::feature::builtin::worker_observation::SpawnedSubWorkerObservationProvider::new(
                    spawned_registry,
                ),
            ));
        }
        if let Some(provider) = host_worker_observation_provider {
            observation_providers.push(provider);
        }
        if !observation_providers.is_empty() {
            feature_registry = feature_registry.with_module(
                crate::feature::builtin::worker_observation::WorkerObservationFeature::new(
                    Arc::new(
                        crate::feature::builtin::worker_observation::CompositeWorkerObservationProvider::new(
                            observation_providers,
                        ),
                    ),
                ),
            );
        }
    }
    let feature_install_report = worker.install_features(feature_registry);
    if feature_install_report.has_errors() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "Worker feature installation failed: {}",
                feature_install_report.error_message()
            ),
        ));
    }
    if let Some((resident_summary, system_prompt_override)) = memory_prompt_contribution {
        worker.install_system_prompt_contribution(resident_summary, system_prompt_override);
    }
    if let Some(tracker) = tracker {
        worker.attach_tracker(tracker);
    }
    Ok(workdir_for_view)
}

/// Idle/Paused event loop. Each iteration either fires a staged
/// `PendingRun` (delegating to [`drive_turn`] for the Running phase) or
/// waits for the next `Method`. Method handlers stop at "update state +
/// stage `pending`"; the loop's top-of-iteration block owns the
/// status-flip → run → finish sequence so it lives in exactly one
/// place.
#[allow(clippy::too_many_arguments)]
async fn controller_loop<C, St>(
    mut worker: Worker<C, St>,
    mut method_rx: mpsc::Receiver<Method>,
    working_event_tx: broadcast::Sender<Event>,
    shared_state: Arc<WorkerSharedState>,
    runtime_dir: Arc<RuntimeDir>,
    cancel_tx: mpsc::Sender<()>,
    pause_tx: mpsc::Sender<()>,
    notify_buffer: NotifyBuffer,
    self_parent_socket: Option<PathBuf>,
    spawner_name: String,
    spawned_registry: Arc<SpawnedWorkerRegistry>,
    shutdown_tx: oneshot::Sender<()>,
    socket_server: Option<SocketServer>,
    shutdown_after_idle: ShutdownAfterIdleRequest,
) where
    C: LlmClient + Clone + 'static,
    St: Store + WorkerMetadataStore + Clone + 'static,
{
    // Hold an optional external attach server alive for the controller lifetime.
    // In-process runtimes retain and drive the WorkerHandle directly.
    let _socket_server = socket_server;

    let discovery_runtime_base = runtime_dir
        .path()
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| runtime_dir.path().to_path_buf());
    let discovery_cwd = worker
        .local_working_directory()
        .map(|local| local.cwd.clone());
    let discovery = WorkerDiscovery::new(
        worker.store().clone(),
        spawner_name.clone(),
        discovery_runtime_base,
        discovery_cwd,
        spawned_registry.clone(),
    );
    let pending_submissions = worker.pending_submission_handle();
    stage_oldest_passive_notification(&pending_submissions, &notify_buffer);
    let mut pending = match prepare_pending_run(&pending_submissions, &notify_buffer, None) {
        Ok(pending) => pending,
        Err(error) => {
            let _ = working_event_tx.send(Event::Error {
                code: ErrorCode::Internal,
                message: error.to_string(),
            });
            None
        }
    };

    let mut deferred_methods = VecDeque::new();

    'controller: loop {
        // here so the status flip → drive_turn → finish sequence lives
        // in one place, regardless of which Method caused it.
        if let Some(run) = pending.take() {
            // Cancellation is meaningful only for an accepted running turn. Clear
            // idle/stale signals before the status flip; any Cancel/Pause received
            // after this point is delivered to the turn and must not be discarded by
            // the Engine at run start.
            worker.engine_mut().clear_pending_cancel();
            // In-flight display state belongs to the active run only. Defensive
            // clear at run start prevents stale partial output left by an older
            // interrupted/error turn from being carried into the next snapshot.
            worker.clear_in_flight_events();
            let parent_originated = run.is_parent_originated();
            let user_input_submit = matches!(&run, PendingRun::Submit(_));
            if !user_input_submit {
                set_controller_status(
                    &shared_state,
                    &runtime_dir,
                    &working_event_tx,
                    WorkerStatus::Running,
                )
                .await;
            }
            let notification_request_id = match &run {
                PendingRun::RunForNotification {
                    notification_request_id,
                    ..
                } => notification_request_id.clone(),
                _ => None,
            };
            let passive_notification_request_id =
                pending_submissions.activating_passive_notification_id();
            let (mut new_status, shutdown, may_drain_pending) = match run {
                PendingRun::Submit(submission) => {
                    let (input_commit_tx, input_commit_rx) = oneshot::channel();
                    let committed_submission = submission.clone();
                    let extension = pending_submissions.activation_extension();
                    drive_turn(
                        worker.run_with_input_extensions_and_commit_hook(
                            submission.input,
                            vec![extension],
                            submission.provenance,
                            move || {
                                let _ = input_commit_tx.send(());
                            },
                        ),
                        &mut method_rx,
                        &working_event_tx,
                        &cancel_tx,
                        &pause_tx,
                        &shared_state,
                        &runtime_dir,
                        Some((input_commit_rx, committed_submission)),
                        &notify_buffer,
                        &pending_submissions,
                        self_parent_socket.as_ref(),
                        &spawner_name,
                        &spawned_registry,
                        parent_originated,
                    )
                    .await
                }
                PendingRun::RunForNotification { invoke_kind, .. } => {
                    drive_turn(
                        worker.run_for_notification(invoke_kind),
                        &mut method_rx,
                        &working_event_tx,
                        &cancel_tx,
                        &pause_tx,
                        &shared_state,
                        &runtime_dir,
                        None,
                        &notify_buffer,
                        &pending_submissions,
                        self_parent_socket.as_ref(),
                        &spawner_name,
                        &spawned_registry,
                        parent_originated,
                    )
                    .await
                }
                PendingRun::Resume => {
                    drive_turn(
                        worker.resume(),
                        &mut method_rx,
                        &working_event_tx,
                        &cancel_tx,
                        &pause_tx,
                        &shared_state,
                        &runtime_dir,
                        None,
                        &notify_buffer,
                        &pending_submissions,
                        self_parent_socket.as_ref(),
                        &spawner_name,
                        &spawned_registry,
                        parent_originated,
                    )
                    .await
                }
            };
            if let Some(notification_request_id) =
                notification_request_id.or(passive_notification_request_id)
            {
                pending_submissions.finish_notification_activation(&notification_request_id);
                stage_oldest_passive_notification(&pending_submissions, &notify_buffer);
            }

            if !shutdown && may_drain_pending && new_status == WorkerStatus::Idle {
                match prepare_pending_run(&pending_submissions, &notify_buffer, None) {
                    Ok(Some(next)) => {
                        pending = Some(next);
                        new_status = WorkerStatus::Running;
                    }
                    Ok(None) => {
                        if notify_buffer.has_auto_run_pending() {
                            pending = Some(PendingRun::RunForNotification {
                                invoke_kind: protocol::InvokeKind::Notify,
                                notification_request_id: None,
                            });
                            new_status = WorkerStatus::Running;
                        }
                    }
                    Err(error) => {
                        let _ = working_event_tx.send(Event::Error {
                            code: ErrorCode::Internal,
                            message: error.to_string(),
                        });
                    }
                }
            }
            finish_controller_run(
                &mut worker,
                &shared_state,
                &runtime_dir,
                &working_event_tx,
                new_status,
            )
            .await;
            if shutdown {
                let _ = working_event_tx.send(Event::Shutdown);
                break;
            }
            if take_shutdown_request_after_status(&shutdown_after_idle, new_status) {
                let _ = working_event_tx.send(Event::Shutdown);
                break;
            }
            continue;
        }

        let method = if let Some(method) = deferred_methods.pop_front() {
            method
        } else {
            match method_rx.recv().await {
                Some(method) => method,
                None => break,
            }
        };

        match method {
            Method::Submit {
                submission_request_id,
                input,
            } => {
                let request_id = submission_request_id.clone();
                match pending_submissions.accept_from_source(
                    submission_request_id,
                    input,
                    pending_submissions.direct_client_namespace(),
                    session_store::LoggedSessionHistoryOrigin::LegacyUnknown,
                    true,
                ) {
                    Ok(acceptance) => {
                        if let Some(activation) = acceptance.activation {
                            pending = Some(PendingRun::Submit(activation));
                        } else {
                            let _ = working_event_tx.send(Event::SubmissionAccepted {
                                submission_request_id: acceptance.submission_request_id,
                                submission_id: acceptance.submission_id,
                                disposition: acceptance.disposition,
                            });
                        }
                    }
                    Err(error) => {
                        let _ = working_event_tx.send(Event::SubmissionRejected {
                            submission_request_id: request_id,
                            message: error.to_string(),
                        });
                    }
                }
            }
            Method::SubmitTracked {
                submission_request_id,
                input,
                source,
            } => {
                let request_id = submission_request_id.clone();
                let (source_namespace, provenance) =
                    resolved_input_source(&pending_submissions, &source);
                match pending_submissions.accept_from_source(
                    submission_request_id,
                    input,
                    source_namespace,
                    provenance,
                    true,
                ) {
                    Ok(acceptance) => {
                        if let Some(activation) = acceptance.activation {
                            pending = Some(PendingRun::Submit(activation));
                        } else {
                            let _ = working_event_tx.send(Event::SubmissionAccepted {
                                submission_request_id: acceptance.submission_request_id,
                                submission_id: acceptance.submission_id,
                                disposition: acceptance.disposition,
                            });
                        }
                    }
                    Err(error) => {
                        let _ = working_event_tx.send(Event::SubmissionRejected {
                            submission_request_id: request_id,
                            message: error.to_string(),
                        });
                    }
                }
            }

            Method::Notify {
                notification_request_id,
                message,
                auto_run,
            } => {
                let request_id = notification_request_id.clone();
                let source_namespace = pending_submissions.direct_client_namespace();
                match pending_submissions.accept_notification_from_source(
                    notification_request_id,
                    message,
                    source_namespace.clone(),
                    session_store::LoggedSessionHistoryOrigin::LegacyUnknown,
                    auto_run,
                ) {
                    Ok(_) if auto_run => {
                        match prepare_pending_run(&pending_submissions, &notify_buffer, None) {
                            Ok(Some(next)) => pending = Some(next),
                            Ok(None) => {}
                            Err(error) => {
                                let _ = working_event_tx.send(Event::Error {
                                    code: ErrorCode::Internal,
                                    message: error.to_string(),
                                });
                            }
                        }
                    }
                    Ok(_) => {
                        stage_pending_notification(
                            &pending_submissions,
                            &notify_buffer,
                            &source_namespace,
                            &request_id,
                        );
                    }
                    Err(error) => {
                        let _ = working_event_tx.send(Event::Error {
                            code: ErrorCode::InvalidRequest,
                            message: error.to_string(),
                        });
                    }
                }
            }

            Method::NotifyTracked {
                notification_request_id,
                message,
                auto_run,
                source,
            } => {
                let request_id = notification_request_id.clone();
                let (source_namespace, provenance) =
                    resolved_input_source(&pending_submissions, &source);
                match pending_submissions.accept_notification_from_source(
                    notification_request_id,
                    message,
                    source_namespace.clone(),
                    provenance,
                    auto_run,
                ) {
                    Ok(_) if auto_run => {
                        match prepare_pending_run(&pending_submissions, &notify_buffer, None) {
                            Ok(Some(next)) => pending = Some(next),
                            Ok(None) => {}
                            Err(error) => {
                                let _ = working_event_tx.send(Event::Error {
                                    code: ErrorCode::Internal,
                                    message: error.to_string(),
                                });
                            }
                        }
                    }
                    Ok(_) => {
                        stage_pending_notification(
                            &pending_submissions,
                            &notify_buffer,
                            &source_namespace,
                            &request_id,
                        );
                    }
                    Err(error) => {
                        let _ = working_event_tx.send(Event::Error {
                            code: ErrorCode::InvalidRequest,
                            message: error.to_string(),
                        });
                    }
                }
            }

            Method::ListPendingSubmissions => {
                let _ = working_event_tx.send(Event::PendingSubmissionsChanged {
                    pending: pending_submissions.snapshot(),
                });
            }
            Method::CancelPendingSubmission {
                submission_id,
                expected_revision,
            } => match pending_submissions.cancel(&submission_id, expected_revision) {
                Ok(pending_snapshot) => {
                    let _ = working_event_tx.send(Event::PendingSubmissionsChanged {
                        pending: pending_snapshot,
                    });
                }
                Err(error) => {
                    let _ = working_event_tx.send(Event::Error {
                        code: ErrorCode::InvalidRequest,
                        message: error.to_string(),
                    });
                }
            },
            Method::ClearPendingSubmissions { expected_revision } => {
                match pending_submissions.clear(expected_revision) {
                    Ok(pending_snapshot) => {
                        let _ = working_event_tx.send(Event::PendingSubmissionsChanged {
                            pending: pending_snapshot,
                        });
                    }
                    Err(error) => {
                        let _ = working_event_tx.send(Event::Error {
                            code: ErrorCode::InvalidRequest,
                            message: error.to_string(),
                        });
                    }
                }
            }
            Method::ContinuePending {
                expected_revision,
                expected_head_id,
            } => {
                if shared_state.catalog_status() != WorkerStatus::Idle {
                    let _ = working_event_tx.send(Event::Error {
                        code: ErrorCode::InvalidRequest,
                        message: "ContinuePending requires an idle Worker; Resume or Cancel a paused run first".into(),
                    });
                    continue;
                }
                match prepare_pending_run(
                    &pending_submissions,
                    &notify_buffer,
                    Some((expected_revision, &expected_head_id)),
                ) {
                    Ok(Some(next)) => pending = Some(next),
                    Ok(None) => {
                        let _ = working_event_tx.send(Event::Error {
                            code: ErrorCode::InvalidRequest,
                            message: "pending activation queue is empty".into(),
                        });
                    }
                    Err(error) => {
                        let _ = working_event_tx.send(Event::Error {
                            code: ErrorCode::Internal,
                            message: error.to_string(),
                        });
                    }
                }
            }
            Method::Resume { command } => {
                if let Err(disposition) =
                    validate_command(command, WorkerCommandKind::Resume, &shared_state)
                {
                    acknowledge_command(
                        &working_event_tx,
                        &shared_state,
                        command.command_id,
                        WorkerCommandKind::Resume,
                        disposition,
                    );
                    continue;
                }
                if !matches!(
                    shared_state.snapshot().state,
                    WorkerState::Busy(WorkerBusyState::Run(WorkerRunState::Paused))
                ) {
                    reject_invalid_command_state(
                        &working_event_tx,
                        &shared_state,
                        command,
                        WorkerCommandKind::Resume,
                    );
                    continue;
                }
                set_controller_state(
                    &shared_state,
                    &runtime_dir,
                    &working_event_tx,
                    WorkerState::Busy(WorkerBusyState::Run(WorkerRunState::Running)),
                )
                .await;
                acknowledge_command(
                    &working_event_tx,
                    &shared_state,
                    command.command_id,
                    WorkerCommandKind::Resume,
                    WorkerCommandDisposition::Accepted,
                );
                pending = Some(PendingRun::Resume);
            }

            Method::Cancel { command } => {
                if let Err(disposition) =
                    validate_command(command, WorkerCommandKind::Cancel, &shared_state)
                {
                    acknowledge_command(
                        &working_event_tx,
                        &shared_state,
                        command.command_id,
                        WorkerCommandKind::Cancel,
                        disposition,
                    );
                    continue;
                }
                if !matches!(
                    shared_state.snapshot().state,
                    WorkerState::Busy(WorkerBusyState::Run(WorkerRunState::Paused))
                ) {
                    reject_invalid_command_state(
                        &working_event_tx,
                        &shared_state,
                        command,
                        WorkerCommandKind::Cancel,
                    );
                    continue;
                }
                set_controller_state(
                    &shared_state,
                    &runtime_dir,
                    &working_event_tx,
                    WorkerState::Busy(WorkerBusyState::Run(WorkerRunState::Cancelling)),
                )
                .await;
                acknowledge_command(
                    &working_event_tx,
                    &shared_state,
                    command.command_id,
                    WorkerCommandKind::Cancel,
                    WorkerCommandDisposition::Accepted,
                );
                match worker.cancel_paused_turn() {
                    Ok(()) => {
                        worker.clear_in_flight_events();
                        set_controller_state(
                            &shared_state,
                            &runtime_dir,
                            &working_event_tx,
                            WorkerState::Idle,
                        )
                        .await;
                    }
                    Err(error) => {
                        set_controller_state(
                            &shared_state,
                            &runtime_dir,
                            &working_event_tx,
                            WorkerState::Busy(WorkerBusyState::Run(WorkerRunState::Paused)),
                        )
                        .await;
                        let _ = working_event_tx.send(Event::Error {
                            code: worker_error_code(&error),
                            message: error.to_string(),
                        });
                    }
                }
            }

            Method::Pause { command } => {
                if let Err(disposition) =
                    validate_command(command, WorkerCommandKind::Pause, &shared_state)
                {
                    acknowledge_command(
                        &working_event_tx,
                        &shared_state,
                        command.command_id,
                        WorkerCommandKind::Pause,
                        disposition,
                    );
                } else {
                    reject_invalid_command_state(
                        &working_event_tx,
                        &shared_state,
                        command,
                        WorkerCommandKind::Pause,
                    );
                }
            }

            Method::Compact { command } => {
                if let Err(disposition) =
                    validate_command(command, WorkerCommandKind::Compact, &shared_state)
                {
                    acknowledge_command(
                        &working_event_tx,
                        &shared_state,
                        command.command_id,
                        WorkerCommandKind::Compact,
                        disposition,
                    );
                    continue;
                }
                if !matches!(shared_state.snapshot().state, WorkerState::Idle) {
                    reject_invalid_command_state(
                        &working_event_tx,
                        &shared_state,
                        command,
                        WorkerCommandKind::Compact,
                    );
                    continue;
                }
                set_controller_state(
                    &shared_state,
                    &runtime_dir,
                    &working_event_tx,
                    WorkerState::Busy(WorkerBusyState::Maintenance(
                        WorkerMaintenanceState::Compacting,
                    )),
                )
                .await;
                acknowledge_command(
                    &working_event_tx,
                    &shared_state,
                    command.command_id,
                    WorkerCommandKind::Compact,
                    WorkerCommandDisposition::Accepted,
                );
                let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
                let mut shutdown_after_compaction = false;
                let result = {
                    let mut compact = Box::pin(worker.manual_compact_with_cancel(cancel_rx));
                    loop {
                        tokio::select! {
                            result = &mut compact => break result,
                            method = method_rx.recv() => {
                                match method {
                                    Some(Method::Cancel { command }) => {
                                        if let Err(disposition) = validate_command(
                                            command,
                                            WorkerCommandKind::Cancel,
                                            &shared_state,
                                        ) {
                                            acknowledge_command(
                                                &working_event_tx,
                                                &shared_state,
                                                command.command_id,
                                                WorkerCommandKind::Cancel,
                                                disposition,
                                            );
                                            continue;
                                        }
                                        acknowledge_command(
                                            &working_event_tx,
                                            &shared_state,
                                            command.command_id,
                                            WorkerCommandKind::Cancel,
                                            WorkerCommandDisposition::Accepted,
                                        );
                                        let _ = cancel_tx.send(true);
                                    }
                                    Some(Method::Shutdown { command }) => {
                                        if let Err(disposition) =
                                            validate_shutdown_command(command, &shared_state)
                                        {
                                            acknowledge_command(
                                                &working_event_tx,
                                                &shared_state,
                                                command.command_id,
                                                WorkerCommandKind::Shutdown,
                                                disposition,
                                            );
                                            continue;
                                        }
                                        shutdown_after_compaction = true;
                                        acknowledge_command(
                                            &working_event_tx,
                                            &shared_state,
                                            command.command_id,
                                            WorkerCommandKind::Shutdown,
                                            WorkerCommandDisposition::Accepted,
                                        );
                                        let _ = cancel_tx.send(true);
                                    }
                                    Some(method) => deferred_methods.push_back(method),
                                    None => {
                                        shutdown_after_compaction = true;
                                        let _ = cancel_tx.send(true);
                                    }
                                }
                            }
                        }
                    }
                };
                if !matches!(
                    result,
                    Err(WorkerError::Store(_))
                        | Err(WorkerError::WorkerStore(_))
                        | Err(WorkerError::InvalidState(_))
                ) {
                    set_controller_state(
                        &shared_state,
                        &runtime_dir,
                        &working_event_tx,
                        WorkerState::Idle,
                    )
                    .await;
                }
                if let Err(error) = result {
                    let _ = working_event_tx.send(Event::Error {
                        code: worker_error_code(&error),
                        message: error.to_string(),
                    });
                }
                if shutdown_after_compaction {
                    let _ = working_event_tx.send(Event::Shutdown);
                    break 'controller;
                }
            }

            Method::ListRewindTargets => match shared_state.catalog_status() {
                WorkerStatus::Idle | WorkerStatus::Paused => {
                    emit_rewind_targets(&worker, &working_event_tx)
                }
                WorkerStatus::Running | WorkerStatus::Stopped => {
                    let _ = working_event_tx.send(Event::Error {
                        code: ErrorCode::AlreadyRunning,
                        message: "Worker is already executing a turn; rewind can only run while idle or paused"
                            .into(),
                    });
                }
            },

            Method::RewindTo {
                target,
                expected_head_entries,
            } => match shared_state.catalog_status() {
                WorkerStatus::Idle => {
                    if apply_rewind(
                        &mut worker,
                        &working_event_tx,
                        target,
                        expected_head_entries,
                    )
                    .await
                    {
                        worker.clear_in_flight_events();
                        let snapshot = shared_state.transition(WorkerState::Idle);
                        let _ = working_event_tx.send(Event::WorkerState { snapshot });
                    }
                }
                WorkerStatus::Paused => {
                    let _ = working_event_tx.send(Event::Error {
                        code: ErrorCode::InvalidRequest,
                        message: "Cannot apply rewind while the Worker is paused; resume or wait for idle first"
                            .into(),
                    });
                }
                WorkerStatus::Running | WorkerStatus::Stopped => {
                    let _ = working_event_tx.send(Event::Error {
                        code: ErrorCode::AlreadyRunning,
                        message: "Worker is already executing a turn; rewind can only run while idle or paused"
                            .into(),
                    });
                }
            },

            Method::Shutdown { command } => {
                // Shutdown ignores the state-revision fence but remains bound to the
                // current execution generation and command payload identity.
                if let Err(disposition) = validate_shutdown_command(command, &shared_state) {
                    acknowledge_command(
                        &working_event_tx,
                        &shared_state,
                        command.command_id,
                        WorkerCommandKind::Shutdown,
                        disposition,
                    );
                    continue;
                }
                acknowledge_command(
                    &working_event_tx,
                    &shared_state,
                    command.command_id,
                    WorkerCommandKind::Shutdown,
                    WorkerCommandDisposition::Accepted,
                );
                let _ = working_event_tx.send(Event::Shutdown);
                break;
            }

            Method::ListWorkers => match discovery.list_visible().await {
                Ok(workers) => match serde_json::to_value(workers) {
                    Ok(workers) => {
                        let _ = working_event_tx.send(Event::WorkersListed { workers });
                    }
                    Err(error) => {
                        let _ = working_event_tx.send(Event::Error {
                            code: ErrorCode::Internal,
                            message: format!("serialize visible workers: {error}"),
                        });
                    }
                },
                Err(error) => {
                    let _ = working_event_tx.send(Event::Error {
                        code: ErrorCode::InvalidRequest,
                        message: error.to_string(),
                    });
                }
            },

            Method::RestoreWorker { name } => match discovery.restore(&name).await {
                Ok(result) => match serde_json::to_value(result) {
                    Ok(result) => {
                        let _ = working_event_tx.send(Event::WorkerRestored { result });
                    }
                    Err(error) => {
                        let _ = working_event_tx.send(Event::Error {
                            code: ErrorCode::Internal,
                            message: format!("serialize worker restore result: {error}"),
                        });
                    }
                },
                Err(error) => {
                    let _ = working_event_tx.send(Event::Error {
                        code: ErrorCode::InvalidRequest,
                        message: error.to_string(),
                    });
                }
            },

            Method::RegisterPeer { name } => match discovery.register_peer(&name) {
                Ok(result) => match serde_json::to_value(result) {
                    Ok(result) => {
                        let _ = working_event_tx.send(Event::PeerRegistered { result });
                    }
                    Err(error) => {
                        let _ = working_event_tx.send(Event::Error {
                            code: ErrorCode::Internal,
                            message: format!("serialize peer registration result: {error}"),
                        });
                    }
                },
                Err(error) => {
                    let _ = working_event_tx.send(Event::Error {
                        code: ErrorCode::InvalidRequest,
                        message: error.to_string(),
                    });
                }
            },

            // ListCompletions is handled at the socket layer (direct
            // response). If it reaches the controller, ignore it.
            Method::ListCompletions { .. } => {}

            Method::WorkerEvent(event) => {
                if handle_inbound_worker_event(
                    event,
                    &spawned_registry,
                    &spawner_name,
                    self_parent_socket.as_ref(),
                    &notify_buffer,
                )
                .await
                {
                    // Auto-kick a turn if the Worker is idle so the
                    // notification is not stranded. Matches the
                    // `Method::Notify` idle path.
                    if shared_state.catalog_status() == WorkerStatus::Idle {
                        pending = Some(PendingRun::RunForNotification {
                            invoke_kind: protocol::InvokeKind::WorkerEvent,
                            notification_request_id: None,
                        });
                    }
                }
            }
        }
    }

    drop(_socket_server);
    if let Err(error) = runtime_dir.close_socket().await {
        tracing::warn!(%error, "Worker runtime socket cleanup failed");
    }

    // Feature callbacks and tasks share the Worker scope. Stop them before
    // Memory/Workdir teardown so they cannot observe a partially closed Worker.
    worker.stop_feature_runtime("controller shutdown").await;

    let child_cleanup_succeeded = match spawned_registry.shutdown_internal().await {
        Ok(()) => true,
        Err(error) => {
            tracing::warn!(%error, "Internal SubWorker cleanup failed before Workdir shutdown");
            false
        }
    };

    if child_cleanup_succeeded
        && let Some(session) = worker.workdir_session()
        && let Err(error) = session.close().await
    {
        tracing::warn!(%error, "Workdir session close failed");
    }

    // Report upward that this Worker is stopping before the controller
    // task exits. Awaited (not fire-and-forget): after `shutdown_tx.send`
    // the process may exit quickly, and a spawned task would be killed
    // mid-send. The `connect_and_send` helper enforces a 5 s timeout so
    // a stuck parent cannot block process exit indefinitely.
    if let Some(parent) = self_parent_socket.as_ref() {
        if let Err(e) = crate::ipc::event::send_worker_event(
            parent,
            protocol::WorkerEvent::ShutDown {
                worker_name: spawner_name.clone(),
            },
        )
        .await
        {
            tracing::warn!(error = %e, "ShutDown WorkerEvent send failed");
        }
    }

    let _ = shutdown_tx.send(());
}

/// Apply an inbound child `WorkerEvent` exactly once.
///
/// Side effects are control-plane state updates and upward propagation; they
/// run for every event. Only agent-visible events are staged on the notify
/// buffer. The caller owns lifecycle-dependent follow-up such as idle
/// `RunForNotification` auto-kick.
async fn handle_inbound_worker_event(
    event: protocol::WorkerEvent,
    spawned_registry: &Arc<SpawnedWorkerRegistry>,
    self_name: &str,
    parent_socket: Option<&PathBuf>,
    notify_buffer: &NotifyBuffer,
) -> bool {
    let self_parent_socket = parent_socket.cloned();
    crate::ipc::event::apply_event_side_effects(
        &event,
        spawned_registry,
        self_name,
        &self_parent_socket,
    )
    .await;

    let notify_agent = event.should_notify_agent();
    if notify_agent {
        notify_buffer.push_worker_event(event);
    }
    notify_agent
}

/// Drives a Worker future (one in-flight turn) while concurrently
/// processing incoming methods through an inner select! arm. Returns
/// `(final_status, shutdown_requested)`.
///
/// `parent_socket` / `self_name` drive upward `WorkerEvent` reports
/// (`TurnEnded` on a clean Finished, `Errored` on a worker failure).
/// `None` parent skips the send (top-level Worker). Transient method
/// rejections such as `AlreadyRunning` are intentionally NOT reported
/// as `Errored` — only the worker-execution `Err` branch below fires.
///
/// `parent_originated` further restricts both upward reports to turns
/// the parent actually delegated (`Method::Submit` / `Method::Resume`).
/// `Method::Notify` / inbound `WorkerEvent` auto-kicks complete silently
/// so the parent's history does not get flooded with child-internal
/// turn boundaries.
#[allow(clippy::too_many_arguments)]
async fn drive_turn<F, St>(
    worker_future: F,
    method_rx: &mut mpsc::Receiver<Method>,
    working_event_tx: &broadcast::Sender<Event>,
    cancel_tx: &mpsc::Sender<()>,
    pause_tx: &mpsc::Sender<()>,
    shared_state: &Arc<WorkerSharedState>,
    runtime_dir: &RuntimeDir,
    mut input_commit: Option<(oneshot::Receiver<()>, crate::worker::PendingSubmission)>,
    notify_buffer: &NotifyBuffer,
    pending_submissions: &crate::worker::PendingSubmissionHandle<St>,
    parent_socket: Option<&PathBuf>,
    self_name: &str,
    spawned_registry: &Arc<SpawnedWorkerRegistry>,
    parent_originated: bool,
) -> (WorkerStatus, bool, bool)
where
    F: std::future::Future<Output = Result<WorkerRunResult, WorkerError>>,
    St: Store + Clone,
{
    tokio::pin!(worker_future);
    let mut shutdown_requested = false;
    let mut pause_requested = false;

    loop {
        tokio::select! {
            // If input commit and provider completion become ready together, expose
            // Running only after processing the commit fence. This makes the
            // Running snapshot contract deterministic even for immediate clients.
            biased;
            committed = async {
                input_commit
                    .as_mut()
                    .map(|(receiver, _)| receiver)
                    .expect("input commit receiver guarded by select condition")
                    .await
            }, if input_commit.is_some() => {
                let submission = input_commit.take().map(|(_, submission)| submission);
                if committed.is_ok() {
                    if let Some(submission) = submission {
                        pending_submissions.finish_activation(&submission.submission_id);
                        let _ = working_event_tx.send(Event::SubmissionAccepted {
                            submission_request_id: submission.submission_request_id,
                            submission_id: submission.submission_id,
                            disposition: protocol::SubmissionDisposition::Started,
                        });
                        let _ = working_event_tx.send(Event::PendingSubmissionsChanged {
                            pending: pending_submissions.snapshot(),
                        });
                    }
                    set_controller_status(
                        shared_state,
                        runtime_dir,
                        working_event_tx,
                        WorkerStatus::Running,
                    )
                    .await;
                } else if let Some(submission) = submission {
                    pending_submissions.abort_activation(submission);
                }
            }
            result = &mut worker_future => {
                if let Some((mut receiver, submission)) = input_commit.take() {
                    match receiver.try_recv() {
                        Ok(()) => {
                            pending_submissions.finish_activation(&submission.submission_id);
                            let _ = working_event_tx.send(Event::SubmissionAccepted {
                                submission_request_id: submission.submission_request_id,
                                submission_id: submission.submission_id,
                                disposition: protocol::SubmissionDisposition::Started,
                            });
                            let _ = working_event_tx.send(Event::PendingSubmissionsChanged {
                                pending: pending_submissions.snapshot(),
                            });
                        }
                        Err(_) => pending_submissions.abort_activation(submission),
                    }
                }
                return match result {
                    Ok(r) => {
                        let may_drain_pending = matches!(
                            &r,
                            WorkerRunResult::Finished | WorkerRunResult::LimitReached
                        );
                        let (status, run_result) = match r {
                            WorkerRunResult::Finished if pause_requested => {
                                (WorkerStatus::Paused, RunResult::Paused)
                            }
                            WorkerRunResult::Finished => (WorkerStatus::Idle, RunResult::Finished),
                            WorkerRunResult::Paused => (WorkerStatus::Paused, RunResult::Paused),
                            WorkerRunResult::LimitReached => (WorkerStatus::Idle, RunResult::LimitReached),
                            WorkerRunResult::RolledBack => (WorkerStatus::Idle, RunResult::RolledBack),
                            WorkerRunResult::Interrupted { .. } if pause_requested => {
                                let _ = working_event_tx.send(Event::RunEnd { result: RunResult::Paused });
                                return (WorkerStatus::Paused, shutdown_requested, false);
                            }
                            WorkerRunResult::Interrupted { code, message } => {
                                let _ = working_event_tx.send(Event::Error {
                                    code,
                                    message: message.clone(),
                                });
                                if parent_originated {
                                    crate::ipc::event::fire_and_forget(
                                        parent_socket.cloned(),
                                        protocol::WorkerEvent::Errored {
                                            worker_name: self_name.to_string(),
                                            message,
                                        },
                                    );
                                }
                                return (WorkerStatus::Idle, shutdown_requested, false);
                            }
                        };
                        let _ = working_event_tx.send(Event::RunEnd { result: run_result });
                        if parent_originated && matches!(run_result, RunResult::Finished) {
                            crate::ipc::event::fire_and_forget(
                                parent_socket.cloned(),
                                protocol::WorkerEvent::TurnEnded {
                                    worker_name: self_name.to_string(),
                                },
                            );
                        }
                        (status, shutdown_requested, may_drain_pending)
                    }
                    Err(WorkerError::Engine(EngineError::Cancelled)) if pause_requested => {
                        // User-initiated Pause. Report the transition to
                        // clients as a normal Paused run-end, and
                        // intentionally skip `WorkerEvent::Errored` upward:
                        // that channel is reserved for worker runtime
                        // failures, not deliberate interruptions.
                        let _ = working_event_tx.send(Event::RunEnd { result: RunResult::Paused });
                        (WorkerStatus::Paused, shutdown_requested, false)
                    }
                    Err(e) => {
                        let code = worker_error_code(&e);
                        let message = e.to_string();
                        let _ = working_event_tx.send(Event::Error {
                            code,
                            message: message.clone(),
                        });
                        if parent_originated {
                            crate::ipc::event::fire_and_forget(
                                parent_socket.cloned(),
                                protocol::WorkerEvent::Errored {
                                    worker_name: self_name.to_string(),
                                    message,
                                },
                            );
                        }
                        (WorkerStatus::Idle, shutdown_requested, false)
                    }
                };
            }
            method = method_rx.recv(), if input_commit.is_none() => {
                match method {
                    Some(Method::Cancel { command }) => {
                        if let Err(disposition) =
                            validate_command(command, WorkerCommandKind::Cancel, shared_state)
                        {
                            acknowledge_command(
                                working_event_tx,
                                shared_state,
                                command.command_id,
                                WorkerCommandKind::Cancel,
                                disposition,
                            );
                            continue;
                        }
                        if !matches!(
                            shared_state.snapshot().state,
                            WorkerState::Busy(WorkerBusyState::Run(WorkerRunState::Running))
                        ) {
                            reject_invalid_command_state(
                                working_event_tx,
                                shared_state,
                                command,
                                WorkerCommandKind::Cancel,
                            );
                            continue;
                        }
                        set_controller_state(
                            shared_state,
                            runtime_dir,
                            working_event_tx,
                            WorkerState::Busy(WorkerBusyState::Run(WorkerRunState::Cancelling)),
                        )
                        .await;
                        acknowledge_command(
                            working_event_tx,
                            shared_state,
                            command.command_id,
                            WorkerCommandKind::Cancel,
                            WorkerCommandDisposition::Accepted,
                        );
                        let _ = cancel_tx.try_send(());
                    }
                    Some(Method::Pause { command }) => {
                        if let Err(disposition) =
                            validate_command(command, WorkerCommandKind::Pause, shared_state)
                        {
                            acknowledge_command(
                                working_event_tx,
                                shared_state,
                                command.command_id,
                                WorkerCommandKind::Pause,
                                disposition,
                            );
                            continue;
                        }
                        if !matches!(
                            shared_state.snapshot().state,
                            WorkerState::Busy(WorkerBusyState::Run(WorkerRunState::Running))
                        ) {
                            reject_invalid_command_state(
                                working_event_tx,
                                shared_state,
                                command,
                                WorkerCommandKind::Pause,
                            );
                            continue;
                        }
                        pause_requested = true;
                        set_controller_state(
                            shared_state,
                            runtime_dir,
                            working_event_tx,
                            WorkerState::Busy(WorkerBusyState::Run(WorkerRunState::Pausing)),
                        )
                        .await;
                        acknowledge_command(
                            working_event_tx,
                            shared_state,
                            command.command_id,
                            WorkerCommandKind::Pause,
                            WorkerCommandDisposition::Accepted,
                        );
                        let _ = pause_tx.try_send(());
                    }
                    Some(Method::Shutdown { command }) => {
                        if let Err(disposition) = validate_shutdown_command(command, shared_state) {
                            acknowledge_command(
                                working_event_tx,
                                shared_state,
                                command.command_id,
                                WorkerCommandKind::Shutdown,
                                disposition,
                            );
                            continue;
                        }
                        shutdown_requested = true;
                        set_controller_state(
                            shared_state,
                            runtime_dir,
                            working_event_tx,
                            WorkerState::Busy(WorkerBusyState::Run(WorkerRunState::Cancelling)),
                        )
                        .await;
                        acknowledge_command(
                            working_event_tx,
                            shared_state,
                            command.command_id,
                            WorkerCommandKind::Shutdown,
                            WorkerCommandDisposition::Accepted,
                        );
                        let _ = cancel_tx.try_send(());
                    }
                    Some(Method::Submit {
                        submission_request_id,
                        input,
                    }) => {
                        let request_id = submission_request_id.clone();
                        match pending_submissions.accept_from_source(
                            submission_request_id,
                            input,
                            pending_submissions.direct_client_namespace(),
                            session_store::LoggedSessionHistoryOrigin::LegacyUnknown,
                            false,
                        ) {
                            Ok(acceptance) => {
                                let _ = working_event_tx.send(Event::SubmissionAccepted {
                                    submission_request_id: acceptance.submission_request_id,
                                    submission_id: acceptance.submission_id,
                                    disposition: acceptance.disposition,
                                });
                                let _ = working_event_tx.send(Event::PendingSubmissionsChanged {
                                    pending: pending_submissions.snapshot(),
                                });
                            }
                            Err(error) => {
                                let _ = working_event_tx.send(Event::SubmissionRejected {
                                    submission_request_id: request_id,
                                    message: error.to_string(),
                                });
                            }
                        }
                    }
                    Some(Method::SubmitTracked {
                        submission_request_id,
                        input,
                        source,
                    }) => {
                        let request_id = submission_request_id.clone();
                        let (source_namespace, provenance) =
                            resolved_input_source(pending_submissions, &source);
                        match pending_submissions.accept_from_source(
                            submission_request_id,
                            input,
                            source_namespace,
                            provenance,
                            false,
                        ) {
                            Ok(acceptance) => {
                                let _ = working_event_tx.send(Event::SubmissionAccepted {
                                    submission_request_id: acceptance.submission_request_id,
                                    submission_id: acceptance.submission_id,
                                    disposition: acceptance.disposition,
                                });
                                let _ = working_event_tx.send(Event::PendingSubmissionsChanged {
                                    pending: pending_submissions.snapshot(),
                                });
                            }
                            Err(error) => {
                                let _ = working_event_tx.send(Event::SubmissionRejected {
                                    submission_request_id: request_id,
                                    message: error.to_string(),
                                });
                            }
                        }
                    }
                    Some(Method::Resume { command }) => {
                        if let Err(disposition) =
                            validate_command(command, WorkerCommandKind::Resume, shared_state)
                        {
                            acknowledge_command(
                                working_event_tx,
                                shared_state,
                                command.command_id,
                                WorkerCommandKind::Resume,
                                disposition,
                            );
                        } else {
                            reject_invalid_command_state(
                                working_event_tx,
                                shared_state,
                                command,
                                WorkerCommandKind::Resume,
                            );
                        }
                    }
                    Some(Method::ContinuePending { .. }) => {
                        let _ = working_event_tx.send(Event::Error {
                            code: ErrorCode::AlreadyRunning,
                            message: "Worker is already executing a turn".into(),
                        });
                    }
                    Some(Method::ListPendingSubmissions) => {
                        let _ = working_event_tx.send(Event::PendingSubmissionsChanged {
                            pending: pending_submissions.snapshot(),
                        });
                    }
                    Some(Method::CancelPendingSubmission {
                        submission_id,
                        expected_revision,
                    }) => {
                        match pending_submissions.cancel(&submission_id, expected_revision) {
                            Ok(pending) => {
                                let _ = working_event_tx.send(Event::PendingSubmissionsChanged { pending });
                            }
                            Err(error) => {
                                let _ = working_event_tx.send(Event::Error {
                                    code: ErrorCode::InvalidRequest,
                                    message: error.to_string(),
                                });
                            }
                        }
                    }
                    Some(Method::ClearPendingSubmissions { expected_revision }) => {
                        match pending_submissions.clear(expected_revision) {
                            Ok(pending) => {
                                let _ = working_event_tx.send(Event::PendingSubmissionsChanged { pending });
                            }
                            Err(error) => {
                                let _ = working_event_tx.send(Event::Error {
                                    code: ErrorCode::InvalidRequest,
                                    message: error.to_string(),
                                });
                            }
                        }
                    }
                    Some(Method::Compact { command }) => {
                        if let Err(disposition) =
                            validate_command(command, WorkerCommandKind::Compact, shared_state)
                        {
                            acknowledge_command(
                                working_event_tx,
                                shared_state,
                                command.command_id,
                                WorkerCommandKind::Compact,
                                disposition,
                            );
                        } else {
                            reject_invalid_command_state(
                                working_event_tx,
                                shared_state,
                                command,
                                WorkerCommandKind::Compact,
                            );
                        }
                    }
                    Some(Method::ListRewindTargets | Method::RewindTo { .. }) => {
                        let _ = working_event_tx.send(Event::Error {
                            code: ErrorCode::AlreadyRunning,
                            message: "Worker is already executing a turn; rewind/compact can only run while idle or paused"
                                .into(),
                        });
                    }
                    Some(Method::Notify {
                        notification_request_id,
                        message,
                        auto_run,
                    }) => {
                        let request_id = notification_request_id.clone();
                        let source_namespace = pending_submissions.direct_client_namespace();
                        match pending_submissions.accept_notification_from_source(
                            notification_request_id,
                            message,
                            source_namespace.clone(),
                            session_store::LoggedSessionHistoryOrigin::LegacyUnknown,
                            auto_run,
                        ) {
                            Ok(_) if !auto_run => {
                                stage_pending_notification(
                                    &pending_submissions,
                                    notify_buffer,
                                    &source_namespace,
                                    &request_id,
                                );
                            }
                            Ok(_) => {}
                            Err(error) => {
                                let _ = working_event_tx.send(Event::Error {
                                    code: ErrorCode::InvalidRequest,
                                    message: error.to_string(),
                                });
                            }
                        }
                        let _ = working_event_tx.send(Event::PendingSubmissionsChanged {
                            pending: pending_submissions.snapshot(),
                        });
                    }
                    Some(Method::NotifyTracked {
                        notification_request_id,
                        message,
                        auto_run,
                        source,
                    }) => {
                        let request_id = notification_request_id.clone();
                        let (source_namespace, provenance) =
                            resolved_input_source(pending_submissions, &source);
                        match pending_submissions.accept_notification_from_source(
                            notification_request_id,
                            message,
                            source_namespace.clone(),
                            provenance,
                            auto_run,
                        ) {
                            Ok(_) if !auto_run => {
                                stage_pending_notification(
                                    &pending_submissions,
                                    notify_buffer,
                                    &source_namespace,
                                    &request_id,
                                );
                            }
                            Ok(_) => {}
                            Err(error) => {
                                let _ = working_event_tx.send(Event::Error {
                                    code: ErrorCode::InvalidRequest,
                                    message: error.to_string(),
                                });
                            }
                        }
                        let _ = working_event_tx.send(Event::PendingSubmissionsChanged {
                            pending: pending_submissions.snapshot(),
                        });
                    }
                    Some(Method::ListCompletions { .. }) => {}
                    Some(Method::ListWorkers | Method::RestoreWorker { .. } | Method::RegisterPeer { .. }) => {
                        let _ = working_event_tx.send(Event::Error {
                            code: ErrorCode::AlreadyRunning,
                            message: "Worker discovery/control requests are only handled while the Worker is idle or paused"
                                .into(),
                        });
                    }
                    Some(Method::WorkerEvent(event)) => {
                        // mpsc is consume-once, so we cannot defer this
                        // to the next main-loop iteration — drop here
                        // would lose the event entirely (children fire
                        // and forget). Auto-kick remains unnecessary here:
                        // the in-flight turn will drain agent-visible events
                        // from the notify buffer on its next history append.
                        handle_inbound_worker_event(
                            event,
                            spawned_registry,
                            self_name,
                            parent_socket,
                            notify_buffer,
                        )
                        .await;
                    }
                    None => {
                        let _ = cancel_tx.try_send(());
                        shared_state.transition(WorkerState::Idle);
                        return (WorkerStatus::Idle, false, false);
                    }
                }
            }
        }
    }
}

fn emit_rewind_targets<C, St>(worker: &Worker<C, St>, working_event_tx: &broadcast::Sender<Event>)
where
    C: LlmClient + 'static,
    St: Store,
{
    match worker.list_rewind_targets() {
        Ok((head_entries, targets)) => {
            let _ = working_event_tx.send(Event::RewindTargets {
                head_entries,
                targets,
            });
        }
        Err(err) => {
            let _ = working_event_tx.send(Event::Error {
                code: ErrorCode::Internal,
                message: err.to_string(),
            });
        }
    }
}

async fn apply_rewind<C, St>(
    worker: &mut Worker<C, St>,
    working_event_tx: &broadcast::Sender<Event>,
    target: RewindTargetId,
    expected_head_entries: usize,
) -> bool
where
    C: LlmClient + 'static,
    St: Store,
{
    match worker.rewind_to(target, expected_head_entries).await {
        Ok(applied) => {
            let session =
                session_store::public_snapshot::project_current_session_snapshot(&applied.entries);
            let _ = working_event_tx.send(Event::RewindApplied {
                session,
                input: applied.input,
                summary: applied.summary,
            });
            true
        }
        Err(err) => {
            let _ = working_event_tx.send(Event::Error {
                code: ErrorCode::InvalidRequest,
                message: err.to_string(),
            });
            false
        }
    }
}

fn model_supports_image_attachments(model: &manifest::ModelManifest) -> bool {
    manifest::model_catalog::resolve_model_manifest(model).is_ok_and(|model| {
        model.capability.is_some_and(|capability| capability.vision)
            && matches!(
                model.scheme,
                manifest::SchemeKind::OpenaiChat | manifest::SchemeKind::OpenaiResponses
            )
    })
}

fn build_greeting<C, St>(worker: &Worker<C, St>) -> protocol::Greeting
where
    C: LlmClient + 'static,
    St: Store,
{
    let manifest = worker.manifest();
    // `build_client` がここに到達する前に同じマニフェストで成功している
    // ため、カタログ解決も必ず通る。念のため失敗時は "unknown" に落とす。
    let resolved = manifest::model_catalog::resolve_model_manifest(&manifest.model).ok();
    let context_window = resolved
        .as_ref()
        .map(|cfg| cfg.context_window)
        .unwrap_or(manifest::model_catalog::DEFAULT_CONTEXT_WINDOW);
    let (provider_name, model_id) = match resolved {
        Some(cfg) => {
            let name = match cfg.scheme {
                manifest::SchemeKind::Anthropic => "anthropic",
                manifest::SchemeKind::OpenaiChat => "openai_chat",
                manifest::SchemeKind::OpenaiResponses => "openai_responses",
                manifest::SchemeKind::Gemini => "gemini",
            };
            (name.to_string(), cfg.model_id)
        }
        None => (
            "unknown".to_string(),
            manifest
                .model
                .ref_
                .clone()
                .or_else(|| manifest.model.model_id.clone())
                .unwrap_or_default(),
        ),
    };
    // Tool list reflects whatever `spawn()` ended up registering on the
    // Engine. Caller must have flushed pending factories first; without
    // a flush the tool table is empty and this returns an empty vec.
    let tool_names: Vec<String> = worker
        .engine()
        .tool_server_handle()
        .tool_definitions_sorted()
        .into_iter()
        .map(|def| def.name)
        .collect();
    protocol::Greeting {
        worker_name: manifest.worker.name.clone(),
        cwd: worker
            .local_working_directory()
            .map(|local| local.cwd.display().to_string())
            .unwrap_or_default(),
        provider: provider_name,
        model: model_id,
        scope_summary: worker.scope_snapshot().summary(),
        tools: tool_names,
        context_window,
        context_tokens: worker.total_tokens().tokens,
    }
}

fn worker_error_code(e: &WorkerError) -> ErrorCode {
    match e {
        WorkerError::Engine(we) => match we {
            EngineError::Tool(_) => ErrorCode::ToolError,
            EngineError::Client(_) => ErrorCode::ProviderError,
            _ => ErrorCode::Internal,
        },
        WorkerError::Provider(_) => ErrorCode::ProviderError,
        _ => ErrorCode::Internal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::WorkerEvent;
    use protocol::stream::{JsonLineReader, JsonLineWriter};
    use std::time::Duration;
    use tempfile::TempDir;
    use tokio::net::UnixListener;

    #[test]
    fn no_controller_parent_notification_uses_durable_pending_authority() {
        let temp = TempDir::new().unwrap();
        let pending =
            crate::worker::PendingSubmissionHandle::for_test(&temp.path().join("sessions"));
        let target = durable_parent_notification_target(pending.clone(), NotifyBuffer::new());
        let (wire_namespace, wire_provenance) =
            resolved_input_source(&pending, &protocol::AuthenticatedInputSource::UntrustedWire);
        assert_eq!(wire_namespace, pending.direct_client_namespace());
        assert!(matches!(
            wire_provenance,
            session_store::LoggedSessionHistoryOrigin::LegacyUnknown
        ));

        target.notify("child-session".into(), "completed".into(), true);

        let snapshot = pending.snapshot();
        assert_eq!(snapshot.notification_count, 1);
        assert!(snapshot.head_id.is_some());
        let notify_buffer = NotifyBuffer::new();
        assert!(matches!(
            prepare_pending_run(&pending, &notify_buffer, None).unwrap(),
            Some(PendingRun::RunForNotification {
                notification_request_id: Some(_),
                ..
            })
        ));
        assert!(notify_buffer.has_auto_run_pending());
    }

    #[test]
    fn restored_mixed_activations_preserve_global_fifo_order() {
        let temp = TempDir::new().unwrap();
        let pending =
            crate::worker::PendingSubmissionHandle::for_test(&temp.path().join("submit-first"));
        pending
            .accept(
                "submit-first".into(),
                vec![protocol::Segment::Text {
                    content: "queued submit".into(),
                }],
                false,
            )
            .unwrap();
        pending
            .accept_notification("notify-second".into(), "newer notification".into(), true)
            .unwrap();
        let notify_buffer = NotifyBuffer::new();

        assert!(matches!(
            prepare_pending_run(&pending, &notify_buffer, None).unwrap(),
            Some(PendingRun::Submit(_))
        ));

        let pending =
            crate::worker::PendingSubmissionHandle::for_test(&temp.path().join("notify-first"));
        pending
            .accept_notification("notify-first".into(), "older notification".into(), true)
            .unwrap();
        pending
            .accept(
                "submit-second".into(),
                vec![protocol::Segment::Text {
                    content: "newer submit".into(),
                }],
                false,
            )
            .unwrap();
        let notify_buffer = NotifyBuffer::new();

        assert!(matches!(
            prepare_pending_run(&pending, &notify_buffer, None).unwrap(),
            Some(PendingRun::RunForNotification {
                notification_request_id: Some(request_id),
                ..
            }) if request_id == "notify-first"
        ));

        let pending =
            crate::worker::PendingSubmissionHandle::for_test(&temp.path().join("passive-first"));
        pending
            .accept_notification("passive-first".into(), "passive notification".into(), false)
            .unwrap();
        pending
            .accept(
                "submit-after-passive".into(),
                vec![protocol::Segment::Text {
                    content: "queued after passive".into(),
                }],
                false,
            )
            .unwrap();
        let snapshot = pending.snapshot();
        let head_id = snapshot.head_id.clone().expect("queued Submit is the head");
        let notify_buffer = NotifyBuffer::new();
        assert!(stage_oldest_passive_notification(&pending, &notify_buffer));
        assert!(matches!(
            prepare_pending_run(
                &pending,
                &notify_buffer,
                Some((snapshot.revision + 1, &head_id)),
            )
            .unwrap(),
            Some(PendingRun::Submit(_))
        ));
    }

    #[test]
    fn image_attachment_gate_requires_vision_and_supported_openai_scheme() {
        let openai = manifest::ModelManifest {
            ref_: Some("codex-oauth/gpt-5.6-sol".to_string()),
            ..Default::default()
        };
        let anthropic = manifest::ModelManifest {
            ref_: Some("anthropic/claude-opus-4-8".to_string()),
            ..Default::default()
        };
        assert!(model_supports_image_attachments(&openai));
        assert!(!model_supports_image_attachments(&anthropic));
    }

    #[test]
    fn pending_run_parent_origin_table() {
        assert!(PendingRun::Resume.is_parent_originated());
        assert!(
            !PendingRun::RunForNotification {
                invoke_kind: protocol::InvokeKind::Notify,
                notification_request_id: None,
            }
            .is_parent_originated()
        );
    }

    struct DriveTurnEnv {
        // Held to keep the channel alive; without this `method_rx.recv()`
        // would observe channel-closed and confuse the select! arm.
        _method_tx: mpsc::Sender<Method>,
        method_rx: mpsc::Receiver<Method>,
        working_event_tx: broadcast::Sender<Event>,
        cancel_tx: mpsc::Sender<()>,
        _cancel_rx: mpsc::Receiver<()>,
        pause_tx: mpsc::Sender<()>,
        _pause_rx: mpsc::Receiver<()>,
        shared_state: Arc<WorkerSharedState>,
        notify_buffer: NotifyBuffer,
        pending_submissions: crate::worker::PendingSubmissionHandle<session_store::FsStore>,
        spawned_registry: Arc<SpawnedWorkerRegistry>,
        parent_socket_path: PathBuf,
        runtime_dir: Arc<RuntimeDir>,
        _temp: TempDir,
    }

    async fn make_env() -> DriveTurnEnv {
        let temp = tempfile::tempdir().expect("tempdir");
        let runtime_dir = Arc::new(
            RuntimeDir::create(temp.path(), "child-worker")
                .await
                .expect("runtime dir create"),
        );
        let (method_tx, method_rx) = mpsc::channel::<Method>(16);
        let (working_event_tx, _) = broadcast::channel::<Event>(16);
        let (cancel_tx, cancel_rx) = mpsc::channel::<()>(1);
        let (pause_tx, pause_rx) = mpsc::channel::<()>(1);
        let shared_state = Arc::new(WorkerSharedState::new(
            "child-worker".to_string(),
            session_store::new_segment_id(),
            String::new(),
            protocol::Greeting {
                worker_name: "child-worker".to_string(),
                cwd: String::new(),
                provider: String::new(),
                model: String::new(),
                scope_summary: String::new(),
                tools: Vec::new(),
                context_window: 200_000,
                context_tokens: 0,
            },
        ));
        let notify_buffer = NotifyBuffer::new();
        let pending_submissions =
            crate::worker::PendingSubmissionHandle::for_test(&temp.path().join("pending-sessions"));
        let spawned_registry = SpawnedWorkerRegistry::new(runtime_dir.clone());
        let parent_socket_path = temp.path().join("parent.sock");

        DriveTurnEnv {
            _method_tx: method_tx,
            method_rx,
            working_event_tx,
            cancel_tx,
            _cancel_rx: cancel_rx,
            pause_tx,
            _pause_rx: pause_rx,
            shared_state,
            notify_buffer,
            pending_submissions,
            spawned_registry,
            parent_socket_path,
            runtime_dir,
            _temp: temp,
        }
    }

    /// Listen on a bound UnixListener for one inbound connection and
    /// return the first `Method::WorkerEvent` read from it. Returns `None`
    /// on timeout / EOF / non-WorkerEvent.
    async fn recv_worker_event(listener: UnixListener, timeout: Duration) -> Option<WorkerEvent> {
        let accept = async {
            let (stream, _) = listener.accept().await.ok()?;
            let (r, w) = stream.into_split();
            let mut writer = JsonLineWriter::new(w);
            writer
                .write(&Event::Snapshot {
                    session: protocol::SessionSnapshot {
                        pending_submissions: protocol::PendingSubmissionsSnapshot::default(),
                        entries: Vec::new(),
                    },
                    greeting: protocol::Greeting {
                        worker_name: "parent".into(),
                        cwd: "/tmp".into(),
                        provider: "test".into(),
                        model: "test".into(),
                        scope_summary: String::new(),
                        tools: Vec::new(),
                        context_window: 200_000,
                        context_tokens: 0,
                    },
                    state: WorkerStatus::Idle.into(),
                    in_flight: Default::default(),
                    internal_workers: Vec::new(),
                })
                .await
                .ok()?;
            let mut reader = JsonLineReader::new(r);
            match reader.next::<Method>().await {
                Ok(Some(Method::WorkerEvent(e))) => Some(e),
                _ => None,
            }
        };
        tokio::time::timeout(timeout, accept).await.ok().flatten()
    }

    #[tokio::test]
    async fn parent_originated_finished_fires_turn_ended() {
        let mut env = make_env().await;
        let listener = UnixListener::bind(&env.parent_socket_path).expect("bind listener");
        let recv = tokio::spawn(recv_worker_event(listener, Duration::from_secs(2)));

        let worker_future = async { Ok::<_, WorkerError>(WorkerRunResult::Finished) };
        let (status, shutdown, _) = drive_turn(
            worker_future,
            &mut env.method_rx,
            &env.working_event_tx,
            &env.cancel_tx,
            &env.pause_tx,
            &env.shared_state,
            &env.runtime_dir,
            None,
            &env.notify_buffer,
            &env.pending_submissions,
            Some(&env.parent_socket_path),
            "child-worker",
            &env.spawned_registry,
            true,
        )
        .await;
        assert_eq!(status, WorkerStatus::Idle);
        assert!(!shutdown);

        let event = recv
            .await
            .expect("recv task")
            .expect("WorkerEvent received");
        match event {
            WorkerEvent::TurnEnded { worker_name } => assert_eq!(worker_name, "child-worker"),
            other => panic!("expected TurnEnded, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn pause_waits_for_run_boundary_and_uses_safe_pause_channel() {
        let mut env = make_env().await;
        let method_tx = env._method_tx.clone();
        env.shared_state
            .transition(WorkerState::Busy(WorkerBusyState::Run(
                WorkerRunState::Running,
            )));
        let command = WorkerCommandEnvelope::for_snapshot(1, &env.shared_state.snapshot());
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            method_tx
                .send(Method::Pause { command })
                .await
                .expect("send pause");
        });

        let worker_future = async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            Ok::<_, WorkerError>(WorkerRunResult::Finished)
        };
        let started_at = std::time::Instant::now();
        let (status, shutdown, _) = drive_turn(
            worker_future,
            &mut env.method_rx,
            &env.working_event_tx,
            &env.cancel_tx,
            &env.pause_tx,
            &env.shared_state,
            &env.runtime_dir,
            None,
            &env.notify_buffer,
            &env.pending_submissions,
            None,
            "child-worker",
            &env.spawned_registry,
            true,
        )
        .await;

        assert_eq!(status, WorkerStatus::Paused);
        assert!(!shutdown);
        assert!(started_at.elapsed() >= Duration::from_millis(100));
        assert!(env._pause_rx.try_recv().is_ok());
        assert!(env._cancel_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn non_parent_originated_finished_stays_silent() {
        let mut env = make_env().await;
        let listener = UnixListener::bind(&env.parent_socket_path).expect("bind listener");

        let worker_future = async { Ok::<_, WorkerError>(WorkerRunResult::Finished) };
        let (status, _, _) = drive_turn(
            worker_future,
            &mut env.method_rx,
            &env.working_event_tx,
            &env.cancel_tx,
            &env.pause_tx,
            &env.shared_state,
            &env.runtime_dir,
            None,
            &env.notify_buffer,
            &env.pending_submissions,
            Some(&env.parent_socket_path),
            "child-worker",
            &env.spawned_registry,
            false,
        )
        .await;
        assert_eq!(status, WorkerStatus::Idle);

        // Wait long enough for any (incorrect) fire-and-forget send to
        // land; expect the accept to time out.
        let accept = tokio::time::timeout(Duration::from_millis(200), listener.accept()).await;
        assert!(
            accept.is_err(),
            "expected no WorkerEvent for non-parent-originated turn"
        );
    }

    #[tokio::test]
    async fn parent_originated_worker_error_fires_errored() {
        let mut env = make_env().await;
        let listener = UnixListener::bind(&env.parent_socket_path).expect("bind listener");
        let recv = tokio::spawn(recv_worker_event(listener, Duration::from_secs(2)));

        let worker_future = async {
            Err::<WorkerRunResult, _>(WorkerError::Engine(EngineError::Aborted(
                "boom from test".into(),
            )))
        };
        let (status, _, _) = drive_turn(
            worker_future,
            &mut env.method_rx,
            &env.working_event_tx,
            &env.cancel_tx,
            &env.pause_tx,
            &env.shared_state,
            &env.runtime_dir,
            None,
            &env.notify_buffer,
            &env.pending_submissions,
            Some(&env.parent_socket_path),
            "child-worker",
            &env.spawned_registry,
            true,
        )
        .await;
        assert_eq!(status, WorkerStatus::Idle);

        let event = recv
            .await
            .expect("recv task")
            .expect("WorkerEvent received");
        match event {
            WorkerEvent::Errored {
                worker_name,
                message,
            } => {
                assert_eq!(worker_name, "child-worker");
                assert!(message.contains("boom from test"), "got message: {message}");
            }
            other => panic!("expected Errored, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn non_parent_originated_worker_error_stays_silent() {
        let mut env = make_env().await;
        let listener = UnixListener::bind(&env.parent_socket_path).expect("bind listener");

        let worker_future = async {
            Err::<WorkerRunResult, _>(WorkerError::Engine(EngineError::Aborted(
                "boom from notify".into(),
            )))
        };
        let (status, _, _) = drive_turn(
            worker_future,
            &mut env.method_rx,
            &env.working_event_tx,
            &env.cancel_tx,
            &env.pause_tx,
            &env.shared_state,
            &env.runtime_dir,
            None,
            &env.notify_buffer,
            &env.pending_submissions,
            Some(&env.parent_socket_path),
            "child-worker",
            &env.spawned_registry,
            false,
        )
        .await;
        assert_eq!(status, WorkerStatus::Idle);

        let accept = tokio::time::timeout(Duration::from_millis(200), listener.accept()).await;
        assert!(
            accept.is_err(),
            "expected no WorkerEvent for notification-originated worker error"
        );
    }

    #[tokio::test]
    async fn running_legacy_scope_callback_has_no_registry_authority_or_notify() {
        let mut env = make_env().await;
        env._method_tx
            .send(Method::WorkerEvent(WorkerEvent::ScopeSubDelegated {
                parent_worker: "child".into(),
                sub_worker: "grandchild".into(),
                sub_socket: "/tmp/grandchild.sock".into(),
                scope: vec![],
            }))
            .await
            .expect("send worker event");

        let worker_future = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok::<_, WorkerError>(WorkerRunResult::Finished)
        };
        let (status, shutdown, _) = drive_turn(
            worker_future,
            &mut env.method_rx,
            &env.working_event_tx,
            &env.cancel_tx,
            &env.pause_tx,
            &env.shared_state,
            &env.runtime_dir,
            None,
            &env.notify_buffer,
            &env.pending_submissions,
            Some(&env.parent_socket_path),
            "parent",
            &env.spawned_registry,
            false,
        )
        .await;

        assert_eq!(status, WorkerStatus::Idle);
        assert!(!shutdown);
        assert!(
            env.notify_buffer.is_empty(),
            "legacy ScopeSubDelegated must not enter the agent-visible notify buffer"
        );
    }

    #[tokio::test]
    async fn running_visible_worker_event_enters_notify_buffer() {
        let mut env = make_env().await;
        env._method_tx
            .send(Method::WorkerEvent(WorkerEvent::TurnEnded {
                worker_name: "child".into(),
            }))
            .await
            .expect("send worker event");

        let worker_future = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok::<_, WorkerError>(WorkerRunResult::Finished)
        };
        let (status, shutdown, _) = drive_turn(
            worker_future,
            &mut env.method_rx,
            &env.working_event_tx,
            &env.cancel_tx,
            &env.pause_tx,
            &env.shared_state,
            &env.runtime_dir,
            None,
            &env.notify_buffer,
            &env.pending_submissions,
            Some(&env.parent_socket_path),
            "parent",
            &env.spawned_registry,
            false,
        )
        .await;

        assert_eq!(status, WorkerStatus::Idle);
        assert!(!shutdown);
        assert_eq!(env.notify_buffer.len(), 1);
    }

    #[tokio::test]
    async fn running_auto_run_notify_remains_staged_for_followup_turn() {
        let mut env = make_env().await;
        env._method_tx
            .send(Method::Notify {
                notification_request_id: protocol::new_submission_request_id(),
                message: "continue".into(),
                auto_run: true,
            })
            .await
            .expect("send notify");

        let worker_future = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok::<_, WorkerError>(WorkerRunResult::Finished)
        };
        let (status, shutdown, _) = drive_turn(
            worker_future,
            &mut env.method_rx,
            &env.working_event_tx,
            &env.cancel_tx,
            &env.pause_tx,
            &env.shared_state,
            &env.runtime_dir,
            None,
            &env.notify_buffer,
            &env.pending_submissions,
            Some(&env.parent_socket_path),
            "parent",
            &env.spawned_registry,
            false,
        )
        .await;

        assert_eq!(status, WorkerStatus::Idle);
        assert!(!shutdown);
        assert_eq!(env.notify_buffer.len(), 0);
        assert_eq!(env.pending_submissions.snapshot().notification_count, 1);
    }

    #[tokio::test]
    async fn compact_method_is_rejected_while_running() {
        let mut env = make_env().await;
        let mut events = env.working_event_tx.subscribe();
        env.shared_state
            .transition(WorkerState::Busy(WorkerBusyState::Run(
                WorkerRunState::Running,
            )));
        let command = WorkerCommandEnvelope::for_snapshot(1, &env.shared_state.snapshot());
        env._method_tx
            .send(Method::Compact { command })
            .await
            .expect("send compact");

        let worker_future = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok::<_, WorkerError>(WorkerRunResult::Finished)
        };
        let (status, shutdown, _) = drive_turn(
            worker_future,
            &mut env.method_rx,
            &env.working_event_tx,
            &env.cancel_tx,
            &env.pause_tx,
            &env.shared_state,
            &env.runtime_dir,
            None,
            &env.notify_buffer,
            &env.pending_submissions,
            Some(&env.parent_socket_path),
            "child-worker",
            &env.spawned_registry,
            false,
        )
        .await;
        assert_eq!(status, WorkerStatus::Idle);
        assert!(!shutdown);

        let event = tokio::time::timeout(Duration::from_secs(1), events.recv())
            .await
            .expect("event timeout")
            .expect("event");
        match event {
            Event::CommandAcknowledged { acknowledgement } => {
                assert_eq!(acknowledgement.command, WorkerCommandKind::Compact);
                assert_eq!(
                    acknowledgement.disposition,
                    WorkerCommandDisposition::InvalidState
                );
                assert!(matches!(
                    acknowledgement.state.state,
                    WorkerState::Busy(WorkerBusyState::Run(WorkerRunState::Running))
                ));
            }
            other => panic!("expected compact rejection acknowledgement, got {other:?}"),
        }
    }

    #[test]
    fn command_admission_rejects_stale_generation_revision_and_order() {
        let shared = WorkerSharedState::new_with_generation(
            "worker".into(),
            session_store::new_segment_id(),
            String::new(),
            protocol::Greeting {
                worker_name: "worker".into(),
                cwd: "/tmp".into(),
                provider: "test".into(),
                model: "test".into(),
                scope_summary: String::new(),
                tools: Vec::new(),
                context_window: 1,
                context_tokens: 0,
            },
            9,
        );
        assert_eq!(
            validate_command(
                WorkerCommandEnvelope {
                    command_id: 1,
                    expected_execution_generation: 8,
                    expected_worker_state_revision: 0,
                },
                WorkerCommandKind::Pause,
                &shared,
            ),
            Err(WorkerCommandDisposition::StaleExecutionGeneration)
        );
        assert_eq!(
            validate_command(
                WorkerCommandEnvelope {
                    command_id: 2,
                    expected_execution_generation: 9,
                    expected_worker_state_revision: 1,
                },
                WorkerCommandKind::Pause,
                &shared,
            ),
            Err(WorkerCommandDisposition::StaleWorkerStateRevision)
        );
        assert!(
            validate_command(
                WorkerCommandEnvelope {
                    command_id: 1,
                    expected_execution_generation: 9,
                    expected_worker_state_revision: 0,
                },
                WorkerCommandKind::Pause,
                &shared,
            )
            .is_ok()
        );
        assert_eq!(
            validate_command(
                WorkerCommandEnvelope {
                    command_id: 1,
                    expected_execution_generation: 9,
                    expected_worker_state_revision: 0,
                },
                WorkerCommandKind::Pause,
                &shared,
            ),
            Err(WorkerCommandDisposition::StaleCommandId)
        );
        assert_eq!(
            validate_command(
                WorkerCommandEnvelope {
                    command_id: 1,
                    expected_execution_generation: 9,
                    expected_worker_state_revision: 0,
                },
                WorkerCommandKind::Cancel,
                &shared,
            ),
            Err(WorkerCommandDisposition::Conflict)
        );
        assert!(
            validate_command(
                WorkerCommandEnvelope {
                    command_id: 2,
                    expected_execution_generation: 9,
                    expected_worker_state_revision: 1,
                },
                WorkerCommandKind::Pause,
                &shared,
            )
            .is_ok()
        );
    }

    #[test]
    fn controller_shutdown_orders_child_cleanup_before_workdir_close() {
        let source = include_str!("controller.rs");
        let shutdown_start = source
            .rfind("worker.stop_feature_runtime(\"controller shutdown\")")
            .expect("controller shutdown block");
        let shutdown = &source[shutdown_start..];
        let children = shutdown
            .find("spawned_registry.shutdown_internal().await")
            .expect("Internal SubWorker cleanup");
        let workdir = shutdown
            .find("session.close().await")
            .expect("parent Workdir close");
        assert!(children < workdir);
        assert!(shutdown.contains("if child_cleanup_succeeded"));
    }
}
