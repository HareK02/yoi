use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use agen::EngineError;
use agen::llm_client::client::LlmClient;
use session_store::WorkerMetadataStore;
use session_store::{LogEntry, SessionExtension, Store};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::discovery::WorkerDiscovery;
use crate::feature::FeatureRegistryBuilder;
use crate::in_flight::{InFlightEvents, snapshot_from_guard};
use crate::ipc::alerter::Alerter;
use crate::ipc::notify_buffer::NotifyBuffer;
use crate::ipc::server::SocketServer;
use crate::runtime::dir::RuntimeDir;
use crate::segment_log_sink::SegmentLogSink;
use crate::shared_state::WorkerSharedState;
use crate::shutdown_after_idle::{
    ShutdownAfterIdleRequest, TicketIntakeReadyShutdownHook, is_ticket_intake_role,
    take_shutdown_request_after_status,
};
use crate::spawn::registry::SpawnedWorkerRegistry;
use crate::spawn::tool::sub_worker_spawn_tool;
use crate::worker::{
    SystemItemCommitter, WORKER_INPUT_SUBMISSION_EXTENSION_DOMAIN, Worker, WorkerError,
    WorkerRunResult,
};
use protocol::{
    AlertLevel, AlertSource, CommandEvent as ProtocolCommandEvent,
    CommandSnapshot as ProtocolCommandSnapshot, CommandStatus as ProtocolCommandStatus,
    CommandStream as ProtocolCommandStream, CommandStreamSlice as ProtocolCommandStreamSlice,
    ErrorCode, Event, Method, RewindTargetId, RunResult, Segment, TurnResult, UploadedFileRef,
    WorkerStatus,
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
        let event = Event::Snapshot {
            session: session_store::public_snapshot::project_current_session_snapshot(&entries),
            greeting: self.shared_state.greeting.clone(),
            status: self.shared_state.get_status(),
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

async fn set_controller_status(
    shared_state: &Arc<WorkerSharedState>,
    runtime_dir: &RuntimeDir,
    working_event_tx: &broadcast::Sender<Event>,
    status: WorkerStatus,
) {
    shared_state.set_status(status);
    let _ = runtime_dir.write_status(shared_state).await;
    let _ = working_event_tx.send(Event::Status { status });
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
    Run(Vec<Segment>),
    RunTracked {
        input: Vec<Segment>,
        extension: SessionExtension,
    },
    /// Self-initiated turn kicked from the notify buffer. The carried
    /// `InvokeKind` is the trigger that flipped the Worker from IDLE
    /// (Notify or WorkerEvent) and is recorded by the Invoke marker
    /// committed at the start of `worker.run_for_notification`.
    RunForNotification(protocol::InvokeKind),
    Resume,
}

impl PendingRun {
    /// Whether this turn was kicked off by the parent (via `Method::Run`
    /// or `Method::Resume`). Used by [`drive_turn`] to gate upward
    /// `WorkerEvent::TurnEnded` / `WorkerEvent::Errored` reports so the parent
    /// only sees completion signals for work it actually delegated.
    /// `RunForNotification` covers self-initiated turns kicked from the
    /// notify buffer (Notify / inbound WorkerEvent) and stays silent.
    fn is_parent_originated(&self) -> bool {
        match self {
            PendingRun::Run(_) | PendingRun::RunTracked { .. } | PendingRun::Resume => true,
            PendingRun::RunForNotification(_) => false,
        }
    }
}

fn should_auto_run_notification(status: WorkerStatus, auto_run: bool) -> bool {
    auto_run && status == WorkerStatus::Idle
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
        let greeting = build_greeting(&worker);
        let shared_state = Arc::new(WorkerSharedState::new(
            worker.manifest().worker.name.clone(),
            worker.segment_id(),
            manifest_toml.clone(),
            greeting,
        ));
        if let Some(fs_for_view) = fs_for_view {
            shared_state.set_fs_view(crate::fs_view::WorkerFsView::new(fs_for_view));
        }
        runtime_dir.write_manifest(&manifest_toml).await?;
        runtime_dir.write_status(&shared_state).await?;

        let artifact_store: Arc<dyn Store> = Arc::new(worker.store().clone());
        let session_id = worker.session_id();
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

fn add_memory_lifecycle_if_configured<M>(
    registry: &mut FeatureRegistryBuilder,
    config: Option<manifest::MemoryConfig>,
    build: impl FnOnce(manifest::MemoryConfig) -> std::io::Result<M>,
) -> std::io::Result<bool>
where
    M: crate::feature::FeatureModule + 'static,
{
    let Some(config) = config else {
        return Ok(false);
    };
    registry.add_module(build(config)?);
    Ok(true)
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
) -> std::io::Result<Option<workdir::WorkdirSessionHandle>>
where
    C: LlmClient + Clone + 'static,
    St: Store + WorkerMetadataStore + Clone + 'static,
{
    // Worker-immutable snapshots taken before the mutable worker borrow
    // below so the worker borrow doesn't conflict with reads on `worker`.
    let feature_config = worker.manifest().feature.clone();
    if feature_config.manage_workdir.enabled && worker.workdir_session().is_none() {
        let workspace_client = worker.workspace_client_handle();
        worker.bind_workdir_session(Some(workdir::delegation_capable_session(
            crate::feature::builtin::manage_workdir::WorkspaceAttachedWorkdirSession::handle(
                workspace_client,
            ),
        )));
    }
    if feature_config.sub_worker.enabled
        && let Some(existing) = worker.workdir_session().cloned()
        && !existing.is_delegation_capable()
    {
        worker.bind_workdir_session(Some(workdir::delegation_capable_session(existing)));
    }
    let worker_workdir = worker.workdir_session().cloned();
    let local_filesystem = worker.local_working_directory().cloned();
    let local_workspace_root = local_filesystem.as_ref().map(|local| local.root.clone());
    let task_feature = worker.task_feature();
    let memory_config = worker.manifest().memory.clone();
    let web_config = worker.manifest().web.clone();
    let mcp_config = worker.manifest().mcp.clone();
    let spawner_name = worker.manifest().worker.name.clone();
    let spawner_manifest = worker.manifest().clone();
    let spawner_workspace_context = worker.workspace_context_handle();
    let parent_notifications = parent_method_tx
        .map(crate::spawn::tool::ParentNotificationTarget::Controller)
        .unwrap_or_else(|| {
            crate::spawn::tool::ParentNotificationTarget::Buffer(worker.notify_buffer_handle())
        });
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
    add_memory_lifecycle_if_configured(&mut feature_registry, memory_config.clone(), |config| {
        let workspace_client = worker.workspace_client_handle();
        if !workspace_client.is_available() || workspace_client.workspace_id().is_none() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Memory extraction requires Backend Workspace API authority",
            ));
        }
        Ok(
            crate::feature::builtin::memory_lifecycle::MemoryLifecycleFeature::new(
                config,
                worker.committed_session_capture_handle(),
                worker.session_extension_handle(),
                workspace_client,
                spawner_manifest.clone(),
                worker.llm_client_handle(),
                prompts.clone(),
                spawner_workspace_context.clone(),
                worker.working_event_sender(),
            ),
        )
    })?;
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
        feature_registry.add_module(
            crate::feature::builtin::manage_workdir::manage_workdir_feature(workspace_client),
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
    let source_workdir_session = worker.workdir_session().cloned();
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

        // Memory tools require explicit feature exposure. Workspace memory access
        // is authority-bound to the Backend Workspace API; the Worker must not
        // register local filesystem memory tools even when it has local cwd/root
        // authority for shell/file tools.
        if feature_config.memory.enabled {
            let _mem = memory_config.as_ref().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "[feature.memory].enabled = true requires a [memory] configuration section",
                )
            })?;
            if workspace_client.is_available() && workspace_client.workspace_id().is_some() {
                let definitions = if feature_config.memory.staging {
                    crate::feature::builtin::memory::workspace_http_memory_consolidation_tools(
                        workspace_client.clone(),
                    )
                } else {
                    crate::feature::builtin::memory::workspace_http_memory_tools(
                        workspace_client.clone(),
                    )
                };
                for definition in definitions {
                    engine.register_tool(definition);
                }
            } else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "memory tools require Backend Workspace API authority",
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
                source_workdir_session,
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
    let mut pending: Option<PendingRun> = None;

    loop {
        // Top-of-iteration: if an event handler staged a run, fire it
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
            let user_input_run = matches!(&run, PendingRun::Run(_) | PendingRun::RunTracked { .. });
            if !user_input_run {
                set_controller_status(
                    &shared_state,
                    &runtime_dir,
                    &working_event_tx,
                    WorkerStatus::Running,
                )
                .await;
            }
            let (mut new_status, shutdown) = match run {
                PendingRun::Run(input) => {
                    let (input_commit_tx, input_commit_rx) = oneshot::channel();
                    drive_turn(
                        worker.run_with_input_extensions_and_commit_hook(
                            input,
                            Vec::new(),
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
                        Some(input_commit_rx),
                        &notify_buffer,
                        self_parent_socket.as_ref(),
                        &spawner_name,
                        &spawned_registry,
                        parent_originated,
                    )
                    .await
                }
                PendingRun::RunTracked { input, extension } => {
                    let (input_commit_tx, input_commit_rx) = oneshot::channel();
                    drive_turn(
                        worker.run_with_input_extensions_and_commit_hook(
                            input,
                            vec![extension],
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
                        Some(input_commit_rx),
                        &notify_buffer,
                        self_parent_socket.as_ref(),
                        &spawner_name,
                        &spawned_registry,
                        parent_originated,
                    )
                    .await
                }
                PendingRun::RunForNotification(kind) => {
                    drive_turn(
                        worker.run_for_notification(kind),
                        &mut method_rx,
                        &working_event_tx,
                        &cancel_tx,
                        &pause_tx,
                        &shared_state,
                        &runtime_dir,
                        None,
                        &notify_buffer,
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
                        self_parent_socket.as_ref(),
                        &spawner_name,
                        &spawned_registry,
                        parent_originated,
                    )
                    .await
                }
            };
            if !shutdown && new_status == WorkerStatus::Idle && notify_buffer.has_auto_run_pending()
            {
                pending = Some(PendingRun::RunForNotification(protocol::InvokeKind::Notify));
                new_status = WorkerStatus::Running;
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

        let method = match method_rx.recv().await {
            Some(m) => m,
            None => break,
        };

        match method {
            Method::Run { input } => {
                if shared_state.get_status() == WorkerStatus::Running {
                    // Defensive: the inner select! inside drive_turn
                    // already rejects `Run` while a turn is live, so
                    // this branch is only reachable across a race window
                    // around status flips.
                    let _ = working_event_tx.send(Event::Error {
                        code: ErrorCode::AlreadyRunning,
                        message: "Worker is already executing a turn".into(),
                    });
                    continue;
                }
                // Stage the run without a speculative user-message echo.
                // `Worker::run` validates the input, commits
                // `LogEntry::AnnotatedUserInput`, and the session-log sink turns that
                // committed entry into the live `Event::UserMessage`. That
                // keeps every client ordered against `SegmentStart` replay and
                // makes persisted history the single source of visible user
                // input. Paused→Run cleanup (orphan tool_result closure +
                // interrupt system note) is applied inside `Worker::run` itself
                // when the worker's `last_run_interrupted` flag is set.
                pending = Some(PendingRun::Run(input));
            }

            Method::RunTracked {
                input,
                submission_id,
            } => {
                // Runtime-correlated submissions retain their opaque id in the
                // same durable UserInput record used for Flow state.
                let extension = SessionExtension::new(
                    WORKER_INPUT_SUBMISSION_EXTENSION_DOMAIN,
                    serde_json::json!({ "submission_id": submission_id }),
                );
                pending = Some(PendingRun::RunTracked { input, extension });
            }

            Method::Notify { message, auto_run } => {
                // Client-side live echo is delivered as `Event::SystemItem`
                // once the interceptor commits the corresponding
                // `LogEntry::AnnotatedSystemItem` entry — drained out of the
                // notify buffer + broadcast through the sink. No
                // separate echo here.
                worker.push_notify(message, auto_run);
                // RUNNING: the in-flight turn drains the buffer at its next
                // pending_history_appends; if an auto-run notification remains
                // at turn end, the Controller stages a follow-up notification
                // turn. Paused notifications remain queued until Resume/Run.
                // IDLE: `auto_run` notifications stage RunForNotification;
                // weak progress notices stay queued until an explicit run.
                if should_auto_run_notification(shared_state.get_status(), auto_run) {
                    pending = Some(PendingRun::RunForNotification(protocol::InvokeKind::Notify));
                }
            }

            Method::Resume => {
                if shared_state.get_status() != WorkerStatus::Paused {
                    let _ = working_event_tx.send(Event::Error {
                        code: ErrorCode::NotPaused,
                        message: "Worker is not paused".into(),
                    });
                    continue;
                }
                pending = Some(PendingRun::Resume);
            }

            Method::Cancel => match shared_state.get_status() {
                WorkerStatus::Paused => match worker.cancel_paused_turn() {
                    Ok(()) => {
                        worker.clear_in_flight_events();
                        set_controller_status(
                            &shared_state,
                            &runtime_dir,
                            &working_event_tx,
                            WorkerStatus::Idle,
                        )
                        .await;
                    }
                    Err(error) => {
                        let _ = working_event_tx.send(Event::Error {
                            code: worker_error_code(&error),
                            message: error.to_string(),
                        });
                    }
                },
                WorkerStatus::Idle | WorkerStatus::Stopped => {
                    let _ = working_event_tx.send(Event::Error {
                        code: ErrorCode::NotRunning,
                        message: "Worker is not running".into(),
                    });
                }
                WorkerStatus::Running => {
                    // Running turns receive Cancel through drive_turn; this is
                    // only reachable across a defensive race window.
                    let _ = cancel_tx.try_send(());
                }
            },

            Method::Pause => {
                // Already paused → idempotent no-op. Otherwise the
                // Worker is Idle (Running turns go through `drive_turn`,
                // not this outer match), so there is nothing to pause.
                if shared_state.get_status() != WorkerStatus::Paused {
                    let _ = working_event_tx.send(Event::Error {
                        code: ErrorCode::NotRunning,
                        message: "Worker is not running".into(),
                    });
                }
            }

            Method::Compact => match shared_state.get_status() {
                WorkerStatus::Idle => {
                    if let Err(error) = worker.manual_compact().await {
                        let _ = working_event_tx.send(Event::Error {
                            code: worker_error_code(&error),
                            message: error.to_string(),
                        });
                    }
                }
                WorkerStatus::Paused => {
                    let _ = working_event_tx.send(Event::Error {
                        code: ErrorCode::InvalidRequest,
                        message: "Cannot compact while the Worker is paused; resume or start a fresh turn first"
                            .into(),
                    });
                }
                WorkerStatus::Running | WorkerStatus::Stopped => {
                    let _ = working_event_tx.send(Event::Error {
                        code: ErrorCode::AlreadyRunning,
                        message:
                            "Worker is already executing a turn; compact can only run while idle"
                                .into(),
                    });
                }
            },

            Method::ListRewindTargets => match shared_state.get_status() {
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
            } => match shared_state.get_status() {
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
                        shared_state.set_status(WorkerStatus::Idle);
                        let _ = working_event_tx.send(Event::Status {
                            status: WorkerStatus::Idle,
                        });
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

            Method::Shutdown => {
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
                    if shared_state.get_status() == WorkerStatus::Idle {
                        pending = Some(PendingRun::RunForNotification(
                            protocol::InvokeKind::WorkerEvent,
                        ));
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

    if let Some(session) = worker.workdir_session()
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
/// the parent actually delegated (`Method::Run` / `Method::Resume`).
/// `Method::Notify` / inbound `WorkerEvent` auto-kicks complete silently
/// so the parent's history does not get flooded with child-internal
/// turn boundaries.
#[allow(clippy::too_many_arguments)]
async fn drive_turn<F>(
    worker_future: F,
    method_rx: &mut mpsc::Receiver<Method>,
    working_event_tx: &broadcast::Sender<Event>,
    cancel_tx: &mpsc::Sender<()>,
    pause_tx: &mpsc::Sender<()>,
    shared_state: &Arc<WorkerSharedState>,
    runtime_dir: &RuntimeDir,
    mut input_commit_rx: Option<oneshot::Receiver<()>>,
    notify_buffer: &NotifyBuffer,
    parent_socket: Option<&PathBuf>,
    self_name: &str,
    spawned_registry: &Arc<SpawnedWorkerRegistry>,
    parent_originated: bool,
) -> (WorkerStatus, bool)
where
    F: std::future::Future<Output = Result<WorkerRunResult, WorkerError>>,
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
                input_commit_rx
                    .as_mut()
                    .expect("input commit receiver guarded by select condition")
                    .await
            }, if input_commit_rx.is_some() => {
                input_commit_rx = None;
                if committed.is_ok() {
                    set_controller_status(
                        shared_state,
                        runtime_dir,
                        working_event_tx,
                        WorkerStatus::Running,
                    )
                    .await;
                }
            }
            result = &mut worker_future => {
                return match result {
                    Ok(r) => {
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
                                return (WorkerStatus::Paused, shutdown_requested);
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
                                return (WorkerStatus::Idle, shutdown_requested);
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
                        (status, shutdown_requested)
                    }
                    Err(WorkerError::Engine(EngineError::Cancelled)) if pause_requested => {
                        // User-initiated Pause. Report the transition to
                        // clients as a normal Paused run-end, and
                        // intentionally skip `WorkerEvent::Errored` upward:
                        // that channel is reserved for worker runtime
                        // failures, not deliberate interruptions.
                        let _ = working_event_tx.send(Event::RunEnd { result: RunResult::Paused });
                        (WorkerStatus::Paused, shutdown_requested)
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
                        (WorkerStatus::Idle, shutdown_requested)
                    }
                };
            }
            method = method_rx.recv() => {
                match method {
                    Some(Method::Cancel) => {
                        let _ = cancel_tx.try_send(());
                    }
                    Some(Method::Pause) => {
                        pause_requested = true;
                        let _ = pause_tx.try_send(());
                    }
                    Some(Method::Shutdown) => {
                        shutdown_requested = true;
                        let _ = cancel_tx.try_send(());
                    }
                    Some(Method::Run { .. } | Method::RunTracked { .. } | Method::Resume) => {
                        let _ = working_event_tx.send(Event::Error {
                            code: ErrorCode::AlreadyRunning,
                            message: "Worker is already executing a turn".into(),
                        });
                    }
                    Some(Method::Compact | Method::ListRewindTargets | Method::RewindTo { .. }) => {
                        let _ = working_event_tx.send(Event::Error {
                            code: ErrorCode::AlreadyRunning,
                            message: "Worker is already executing a turn; rewind/compact can only run while idle or paused"
                                .into(),
                        });
                    }
                    Some(Method::Notify { message, auto_run }) => {
                        // Live echo arrives via `Event::SystemItem` once
                        // the in-flight turn's next `pending_history_appends`
                        // drains this entry through the interceptor.
                        notify_buffer.push_notify(message, auto_run);
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
                        shared_state.set_status(WorkerStatus::Idle);
                        return (WorkerStatus::Idle, false);
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
    fn memory_lifecycle_registration_depends_only_on_memory_config_presence() {
        #[derive(Clone)]
        struct TestMemoryLifecycleModule;

        impl crate::feature::FeatureModule for TestMemoryLifecycleModule {
            fn descriptor(&self) -> crate::feature::FeatureDescriptor {
                crate::feature::FeatureDescriptor::builtin(
                    "test-memory-lifecycle",
                    "Test Memory Lifecycle",
                )
            }

            fn install(
                &self,
                _context: &mut crate::feature::FeatureInstallContext<'_>,
            ) -> Result<(), crate::feature::FeatureInstallError> {
                Ok(())
            }
        }

        let mut registry = FeatureRegistryBuilder::new();
        let configured = std::cell::Cell::new(false);
        let installed = add_memory_lifecycle_if_configured(
            &mut registry,
            Some(manifest::MemoryConfig::default()),
            |_| {
                configured.set(true);
                Ok(TestMemoryLifecycleModule)
            },
        )
        .unwrap();
        assert!(installed);
        assert!(configured.get());

        let mut registry = FeatureRegistryBuilder::new();
        let installed = add_memory_lifecycle_if_configured::<TestMemoryLifecycleModule>(
            &mut registry,
            None,
            |_| panic!("disabled Memory must not construct its lifecycle Feature"),
        )
        .unwrap();
        assert!(!installed);
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
        assert!(PendingRun::Run(Vec::new()).is_parent_originated());
        assert!(PendingRun::Resume.is_parent_originated());
        assert!(
            !PendingRun::RunForNotification(protocol::InvokeKind::Notify).is_parent_originated()
        );
    }

    #[test]
    fn notification_auto_run_gate_only_allows_idle_auto_run() {
        assert!(should_auto_run_notification(WorkerStatus::Idle, true));
        assert!(!should_auto_run_notification(WorkerStatus::Idle, false));
        assert!(!should_auto_run_notification(WorkerStatus::Running, true));
        assert!(!should_auto_run_notification(WorkerStatus::Paused, true));
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
                    status: WorkerStatus::Idle,
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
        let (status, shutdown) = drive_turn(
            worker_future,
            &mut env.method_rx,
            &env.working_event_tx,
            &env.cancel_tx,
            &env.pause_tx,
            &env.shared_state,
            &env.runtime_dir,
            None,
            &env.notify_buffer,
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
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            method_tx.send(Method::Pause).await.expect("send pause");
        });

        let worker_future = async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            Ok::<_, WorkerError>(WorkerRunResult::Finished)
        };
        let started_at = std::time::Instant::now();
        let (status, shutdown) = drive_turn(
            worker_future,
            &mut env.method_rx,
            &env.working_event_tx,
            &env.cancel_tx,
            &env.pause_tx,
            &env.shared_state,
            &env.runtime_dir,
            None,
            &env.notify_buffer,
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
        let (status, _) = drive_turn(
            worker_future,
            &mut env.method_rx,
            &env.working_event_tx,
            &env.cancel_tx,
            &env.pause_tx,
            &env.shared_state,
            &env.runtime_dir,
            None,
            &env.notify_buffer,
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
        let (status, _) = drive_turn(
            worker_future,
            &mut env.method_rx,
            &env.working_event_tx,
            &env.cancel_tx,
            &env.pause_tx,
            &env.shared_state,
            &env.runtime_dir,
            None,
            &env.notify_buffer,
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
        let (status, _) = drive_turn(
            worker_future,
            &mut env.method_rx,
            &env.working_event_tx,
            &env.cancel_tx,
            &env.pause_tx,
            &env.shared_state,
            &env.runtime_dir,
            None,
            &env.notify_buffer,
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
        let (status, shutdown) = drive_turn(
            worker_future,
            &mut env.method_rx,
            &env.working_event_tx,
            &env.cancel_tx,
            &env.pause_tx,
            &env.shared_state,
            &env.runtime_dir,
            None,
            &env.notify_buffer,
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
        let (status, shutdown) = drive_turn(
            worker_future,
            &mut env.method_rx,
            &env.working_event_tx,
            &env.cancel_tx,
            &env.pause_tx,
            &env.shared_state,
            &env.runtime_dir,
            None,
            &env.notify_buffer,
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
                message: "continue".into(),
                auto_run: true,
            })
            .await
            .expect("send notify");

        let worker_future = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok::<_, WorkerError>(WorkerRunResult::Finished)
        };
        let (status, shutdown) = drive_turn(
            worker_future,
            &mut env.method_rx,
            &env.working_event_tx,
            &env.cancel_tx,
            &env.pause_tx,
            &env.shared_state,
            &env.runtime_dir,
            None,
            &env.notify_buffer,
            Some(&env.parent_socket_path),
            "parent",
            &env.spawned_registry,
            false,
        )
        .await;

        assert_eq!(status, WorkerStatus::Idle);
        assert!(!shutdown);
        assert_eq!(env.notify_buffer.len(), 1);
        assert!(env.notify_buffer.has_auto_run_pending());
    }

    #[tokio::test]
    async fn compact_method_is_rejected_while_running() {
        let mut env = make_env().await;
        let mut events = env.working_event_tx.subscribe();
        env._method_tx
            .send(Method::Compact)
            .await
            .expect("send compact");

        let worker_future = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok::<_, WorkerError>(WorkerRunResult::Finished)
        };
        let (status, shutdown) = drive_turn(
            worker_future,
            &mut env.method_rx,
            &env.working_event_tx,
            &env.cancel_tx,
            &env.pause_tx,
            &env.shared_state,
            &env.runtime_dir,
            None,
            &env.notify_buffer,
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
            Event::Error { code, message } => {
                assert_eq!(code, ErrorCode::AlreadyRunning);
                assert!(message.contains("compact"), "got message: {message}");
            }
            other => panic!("expected compact rejection error, got {other:?}"),
        }
    }
}
