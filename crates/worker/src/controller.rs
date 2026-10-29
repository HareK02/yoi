use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use agen::EngineError;
use agen::llm_client::client::LlmClient;
use session_store::WorkerMetadataStore;
use session_store::{LogEntry, Store};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::feature::FeatureRegistryBuilder;
use crate::in_flight::{InFlightEvents, snapshot_from_guard};
use crate::ipc::alerter::Alerter;
use crate::ipc::notify_buffer::NotifyBuffer;
use crate::runtime::dir::RuntimeDir;
use crate::segment_log_sink::SegmentLogSink;
use crate::shared_state::{WorkerCommandAdmission, WorkerSharedState};
use crate::shutdown_after_idle::{
    ShutdownAfterIdleRequest, TicketIntakeReadyShutdownHook, is_ticket_intake_role,
    take_shutdown_request_after_status,
};
use crate::spawn::registry::{PreparedSpawnedWorkerRegistryLoad, SpawnedWorkerRegistry};
use crate::worker::{
    PendingSubmissionHandle, SystemItemCommitter, Worker, WorkerError, WorkerRunResult,
};
use protocol::{
    AlertLevel, AlertSource, CommandEvent as ProtocolCommandEvent,
    CommandSnapshot as ProtocolCommandSnapshot, CommandStatus as ProtocolCommandStatus,
    CommandStream as ProtocolCommandStream, CommandStreamSlice as ProtocolCommandStreamSlice,
    ErrorCode, Event, Method, RunResult, TurnResult, UploadedFileRef, WorkerBusyState,
    WorkerCommandAcknowledgement, WorkerCommandDisposition, WorkerCommandEnvelope,
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
    /// Transport closure belongs to this Controller endpoint, not a WorkerRef.
    pub fn protocol_is_closed(&self) -> bool {
        self.method_tx.is_closed()
    }

    pub async fn protocol_closed(&self) {
        self.method_tx.closed().await;
    }

    /// Compare actual endpoints when an execution-bound transport is dispatched.
    pub fn same_controller(&self, other: &Self) -> bool {
        self.method_tx.same_channel(&other.method_tx)
    }

    /// Ask the actual Controller to acknowledge quiescence. No catalog status,
    /// profile, or model protocol message grants this Host lifecycle capability.
    pub async fn reserve_subject_release(
        &self,
    ) -> Result<crate::SubjectReleaseReservation, String> {
        if self.protocol_is_closed() {
            return Err("Controller is unavailable".into());
        }
        if self.shared_state.snapshot().state != WorkerState::Idle {
            return Err("Controller is not Idle".into());
        }
        let control = self
            .shared_state
            .subject_release
            .get()
            .ok_or("Controller release authority unavailable")?;
        tokio::select! {
            result = control.request() => result,
            _ = self.method_tx.closed() => Err("Controller closed before release acknowledgement".into()),
        }
    }

    pub async fn send(&self, method: Method) -> Result<(), mpsc::error::SendError<Method>> {
        // The lease covers enqueue itself. The Controller's exclusive barrier
        // cannot overlook an accepted send between its queue check and ack.
        let _admission = if release_fenced_method(&method) {
            match self
                .shared_state
                .subject_release
                .get()
                .map(|control| control.authority.enter())
                .transpose()
            {
                Ok(lease) => lease,
                Err(_) => return Err(mpsc::error::SendError(method)),
            }
        } else {
            None
        };
        // Reject known-busy sends before channel dispatch, including maintenance
        // that cannot receive methods until it finishes. The controller checks
        // again at admission to cover an Idle -> busy race after this snapshot.
        if self.shared_state.snapshot().state != WorkerState::Idle
            && reject_idle_only_submission(&method, &self.working_event_tx)
        {
            return Ok(());
        }
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

    pub fn session_attachment(
        &self,
        session_id: &str,
        attachment_id: &str,
        max_attachment_bytes: u64,
    ) -> Result<session_store::RetainedSessionAttachment, session_store::RetainedAttachmentReadError>
    {
        if session_id == self.session_id.to_string() {
            return session_store::read_session_attachment_from_entries(
                &self.committed_entries(),
                attachment_id,
                max_attachment_bytes,
            );
        }
        self.spawned_registry
            .session_attachment(session_id, attachment_id, max_attachment_bytes)
            .unwrap_or(Err(
                session_store::RetainedAttachmentReadError::SessionMismatch,
            ))
    }

    /// Hold through protocol subscription and initial snapshot capture so child
    /// events cannot overtake the parent's snapshot publication boundary.
    pub fn protocol_snapshot_publish_guard(&self) -> std::sync::MutexGuard<'_, ()> {
        self.spawned_registry.protocol_snapshot_publish_guard()
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
            session_store::public_snapshot::project_current_session_snapshot_with_accounting(
                self.artifact_store.as_ref(),
                &entries,
            );
        session.pending_submissions = self
            .pending_activations
            .lock()
            .expect("pending activation state poisoned")
            .snapshot();
        let event = Event::Snapshot {
            session,
            greeting: self.shared_state.greeting(),
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
        context: Option<&protocol::CompletionContext>,
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
                        description: None,
                        usage: None,
                        invocation: None,
                    })
                    .collect()
            }
            protocol::CompletionKind::Feature => self
                .shared_state
                .feature_invocations()
                .feature_completions(prefix),
            protocol::CompletionKind::FeatureArgument => {
                let Some(context) = context else {
                    return Vec::new();
                };
                let registry = self.shared_state.feature_invocations();
                let Some(argument) = context.argument.as_deref() else {
                    return registry.argument_name_completions(&context.invocation, prefix);
                };
                let worker_file = registry
                    .descriptor(&context.invocation)
                    .and_then(|descriptor| {
                        descriptor
                            .arguments
                            .iter()
                            .find(|candidate| candidate.name == argument)
                    })
                    .is_some_and(|descriptor| {
                        matches!(
                            descriptor.completion,
                            protocol::InvocationCompletion::WorkerFile
                        )
                    });
                if worker_file {
                    let Some(view) = self.shared_state.fs_view() else {
                        return Vec::new();
                    };
                    let candidates = view.list_file_completions(prefix).await;
                    if registry.descriptor(&context.invocation).is_none() {
                        return Vec::new();
                    }
                    return crate::feature::invocation::bounded_completion_entries(
                        candidates
                            .into_iter()
                            .map(|candidate| protocol::CompletionEntry {
                                value: candidate.path,
                                is_dir: candidate.is_dir,
                                description: None,
                                usage: None,
                                invocation: None,
                            }),
                    );
                }
                registry
                    .argument_completions(&context.invocation, argument, prefix)
                    .await
            }
        }
    }

    /// Broadcast an event to all listeners.
    pub fn send_event(&self, event: Event) -> Result<usize, broadcast::error::SendError<Event>> {
        self.working_event_tx.send(event)
    }

    /// Emit a user-facing alert. Thin wrapper over `Alerter::alert`.
    pub fn alert(&self, level: AlertLevel, source: AlertSource, message: String) {
        self.alerter.alert(level, source, message);
    }
}

fn answer_idle_release<St: Store + Clone>(
    control: &Arc<crate::subject_release::ReleaseControl>,
    shared_state: &WorkerSharedState,
    methods: &mpsc::Receiver<Method>,
    staged: bool,
    pending: &PendingSubmissionHandle<St>,
    notifications: &NotifyBuffer,
    attention: bool,
) {
    control.respond(|| {
        // Read ingress and durable pending state only after acquiring exclusivity.
        // Actor-local staged/deferred/attention flags cannot change during this call.
        if shared_state.snapshot().state != WorkerState::Idle
            || staged
            || attention
            || !methods.is_empty()
            || methods.is_closed()
            || !notifications.is_empty()
            || pending.has_unfinished_admission()
        {
            return Err("Controller has pending, staged, busy, or unavailable work".into());
        }
        Ok(())
    });
}

fn release_fenced_method(method: &Method) -> bool {
    !matches!(
        method,
        Method::Shutdown { .. }
            | Method::ListWorkers
            | Method::ListPendingSubmissions
            | Method::ListCompletions { .. }
    )
}

async fn release_requested(shared_state: &WorkerSharedState) {
    match shared_state.subject_release.get() {
        Some(control) => control.notified().await,
        None => std::future::pending::<()>().await,
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
    }
}

fn validate_command(
    envelope: WorkerCommandEnvelope,
    kind: WorkerCommandKind,
    shared_state: &WorkerSharedState,
) -> Result<(), WorkerCommandDisposition> {
    command_admission_disposition(shared_state.admit_command(envelope, kind))
}

fn validate_shutdown_command(
    envelope: WorkerCommandEnvelope,
    shared_state: &WorkerSharedState,
) -> Result<(), WorkerCommandDisposition> {
    match shared_state.admit_command(envelope, WorkerCommandKind::Shutdown) {
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
    working_event_tx: &broadcast::Sender<Event>,
    state: WorkerState,
) -> protocol::WorkerStateSnapshot {
    let snapshot = shared_state.transition(state);
    let _ = working_event_tx.send(Event::WorkerState {
        snapshot: snapshot.clone(),
    });
    snapshot
}

fn publish_context_usage(
    shared_state: &Arc<WorkerSharedState>,
    working_event_tx: &broadcast::Sender<Event>,
    tokens: Option<u64>,
    source: protocol::ContextTokenSource,
) {
    shared_state.update_context_usage(tokens, source);
    let usage = tokens.map(|tokens| protocol::ContextUsage { tokens, source });
    let _ = working_event_tx.send(Event::ContextUsage { usage });
}

async fn set_controller_status(
    shared_state: &Arc<WorkerSharedState>,
    working_event_tx: &broadcast::Sender<Event>,
    status: WorkerStatus,
) {
    let state = match status {
        WorkerStatus::Idle | WorkerStatus::Stopped => WorkerState::Idle,
        WorkerStatus::Running => WorkerState::Busy(WorkerBusyState::Run(WorkerRunState::Running)),
        WorkerStatus::Paused => WorkerState::Busy(WorkerBusyState::Run(WorkerRunState::Paused)),
    };
    set_controller_state(shared_state, working_event_tx, state).await;
}

async fn finish_controller_run<C, St>(
    worker: &mut Worker<C, St>,
    shared_state: &Arc<WorkerSharedState>,
    working_event_tx: &broadcast::Sender<Event>,
    new_status: WorkerStatus,
    finished_submission_request_id: Option<String>,
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
    publish_context_usage(
        shared_state,
        working_event_tx,
        Some(worker.total_tokens().tokens),
        protocol::ContextTokenSource::Estimated,
    );
    if let Some(submission_request_id) = finished_submission_request_id {
        let state = match new_status {
            WorkerStatus::Idle | WorkerStatus::Stopped => WorkerState::Idle,
            WorkerStatus::Running => {
                WorkerState::Busy(WorkerBusyState::Run(WorkerRunState::Running))
            }
            WorkerStatus::Paused => WorkerState::Busy(WorkerBusyState::Run(WorkerRunState::Paused)),
        };
        let snapshot = shared_state.finish_submission_run(state, submission_request_id);
        let _ = working_event_tx.send(Event::WorkerState { snapshot });
    } else {
        set_controller_status(shared_state, working_event_tx, new_status).await;
    }
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
            receipt_scope,
            notification_request_id,
            message,
            source,
        } = method
        else {
            return;
        };
        let (source_namespace, provenance) = resolved_input_source(&pending_submissions, &source);
        let source_namespace = receipt_scope.namespace(source_namespace);
        match pending_submissions.accept_notification_from_source(
            notification_request_id,
            message,
            source_namespace,
            provenance,
        ) {
            Ok(_) => {
                stage_pending_notifications(&pending_submissions, &notify_buffer);
            }
            Err(error) => {
                tracing::warn!(%error, "failed to durably accept SubWorker notification");
            }
        }
    }))
}

fn stage_pending_notifications<St: Store + Clone>(
    pending_submissions: &crate::worker::PendingSubmissionHandle<St>,
    notify_buffer: &NotifyBuffer,
) -> usize {
    let notifications = pending_submissions.prepare_notification_batch();
    if notifications.is_empty() {
        return 0;
    }
    let count = notifications.len();
    for (notification, extension) in notifications {
        notify_buffer.push_durable_notify(notification.message, notification.provenance, extension);
    }
    count
}

/// The durable input append is the activation boundary. Clear live state in
/// the same poll, before invocation preparation can checkpoint it again; the
/// oneshot only publishes the already-completed transition to the Controller.
pub(crate) fn input_committed_hook<St: Store + Clone>(
    pending: crate::worker::PendingSubmissionHandle<St>,
    submission_id: String,
    sender: oneshot::Sender<()>,
) -> impl FnOnce() {
    move || {
        pending.finish_activation(&submission_id);
        let _ = sender.send(());
    }
}

fn notification_coalesce_remaining<St: Store + Clone>(
    pending_submissions: &crate::worker::PendingSubmissionHandle<St>,
    notify_buffer: &NotifyBuffer,
    delay: Duration,
    can_schedule_run: bool,
) -> Option<Duration> {
    if !can_schedule_run {
        return None;
    }
    let Some(accepted_at_ms) = pending_submissions.oldest_pending_notification_accepted_at_ms()
    else {
        return notify_buffer
            .has_notification_pending()
            .then_some(Duration::ZERO);
    };
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let elapsed = Duration::from_millis(now_ms.saturating_sub(accepted_at_ms));
    Some(delay.saturating_sub(elapsed))
}

fn prepare_pending_run<St: Store + Clone>(
    pending_submissions: &crate::worker::PendingSubmissionHandle<St>,
    notify_buffer: &NotifyBuffer,
    fence: Option<&str>,
    allow_notification_run: bool,
) -> Result<Option<PendingRun>, crate::worker::PendingSubmissionError> {
    Ok(match pending_submissions.prepare_next_activation(fence)? {
        Some(crate::worker::PendingActivation::Submission(submission)) => {
            stage_pending_notifications(pending_submissions, notify_buffer);
            Some(PendingRun::Submit(submission))
        }
        None if allow_notification_run
            && (stage_pending_notifications(pending_submissions, notify_buffer) > 0
                || notify_buffer.has_notification_pending()) =>
        {
            Some(PendingRun::RunForNotification {
                invoke_kind: protocol::InvokeKind::Notify,
            })
        }
        None => None,
    })
}

// ---------------------------------------------------------------------------
// WorkerController — actor that owns a Worker
// ---------------------------------------------------------------------------

pub type ShutdownReceiver = oneshot::Receiver<()>;

pub struct WorkerController;

// Own the real command receiver and all snapshot sources before starting any
// protocol listener or controller loop. Fresh startup may materialize the head;
// Runtime restore requires an already materialized source and defers canonical
// restore effects until its durable decision. Tool discovery can own external
// resources, so private initialization is not synonymous with side-effect-free.
pub(crate) struct InitializedWorkerController<C: LlmClient, St: Store> {
    pub(crate) worker: Worker<C, St>,
    pub(crate) handle: WorkerHandle,
    method_rx: mpsc::Receiver<Method>,
    shutdown_tx: oneshot::Sender<()>,
    shutdown_rx: ShutdownReceiver,
    registry_reclaim: PreparedSpawnedWorkerRegistryLoad<St>,
    shutdown_after_idle: ShutdownAfterIdleRequest,
    command_events: Option<PreparedWorkdirCommandEvents>,
}

pub(crate) struct PreparedControllerPublication {
    command_observer: Option<tokio::task::JoinHandle<()>>,
}

impl WorkerController {
    /// Explicit recovery of an unmaterialized Session head. This installs the
    /// saved Feature composition needed to produce the seed, but creates no
    /// Controller, input channel, socket, observer, or queue consumer. The caller
    /// owns the same operation's Feature/session/allocation cleanup on every exit.
    pub async fn recover_pending_head<C, St>(
        worker: &mut Worker<C, St>,
        runtime_base: &Path,
    ) -> std::io::Result<()>
    where
        C: LlmClient + Clone + 'static,
        St: Store + WorkerMetadataStore + Clone + Send + Sync + 'static,
    {
        if worker.needs_initial_session_head_materialization() {
            worker
                .validate_subjektiv_body_publication()
                .map_err(std::io::Error::other)?;
            let registry = SpawnedWorkerRegistry::new_internal(
                worker.manifest().worker.name.clone(),
                worker.scope().clone(),
            );
            register_worker_tools(worker, runtime_base.to_path_buf(), registry, None, None).await?;
            worker
                .materialize_durable_session_head()
                .await
                .map_err(std::io::Error::other)?;
        }
        Ok(())
    }

    pub async fn spawn<C, St>(
        worker: Worker<C, St>,
        runtime_base: &Path,
    ) -> Result<(WorkerHandle, ShutdownReceiver), std::io::Error>
    where
        C: LlmClient + Clone + 'static,
        St: Store + WorkerMetadataStore + Clone + Send + Sync + 'static,
    {
        let (handle, shutdown, _task) = Self::spawn_inner(worker, runtime_base, None).await?;
        Ok((handle, shutdown))
    }

    pub(crate) async fn spawn_owned<C, St>(
        worker: Worker<C, St>,
        runtime_base: &Path,
    ) -> Result<(WorkerHandle, ShutdownReceiver, tokio::task::JoinHandle<()>), std::io::Error>
    where
        C: LlmClient + Clone + 'static,
        St: Store + WorkerMetadataStore + Clone + Send + Sync + 'static,
    {
        Self::spawn_inner(worker, runtime_base, None).await
    }

    /// Spawn a Worker owned by `worker-runtime`.
    ///
    /// The controller uses an ephemeral directory for run artifacts. Bash output is
    /// owned by its live Feature instance, independently of filesystem layout.
    /// Runtime-managed Workers do not write legacy pid/status/manifest liveness
    /// projections.
    pub async fn spawn_runtime_managed<C, St>(
        worker: Worker<C, St>,
        runtime_base: &Path,
    ) -> Result<(WorkerHandle, ShutdownReceiver), std::io::Error>
    where
        C: LlmClient + Clone + 'static,
        St: Store + WorkerMetadataStore + Clone + Send + Sync + 'static,
    {
        let (handle, shutdown, _task) = Self::spawn_inner(worker, runtime_base, None).await?;
        Ok((handle, shutdown))
    }

    /// Spawn into an exact persistent `runs/<generation>` directory.
    pub async fn spawn_runtime_managed_run<C, St>(
        worker: Worker<C, St>,
        run_dir: &Path,
    ) -> Result<(WorkerHandle, ShutdownReceiver), std::io::Error>
    where
        C: LlmClient + Clone + 'static,
        St: Store + WorkerMetadataStore + Clone + Send + Sync + 'static,
    {
        let (handle, shutdown, _task) =
            Self::spawn_runtime_managed_run_owned(worker, run_dir).await?;
        Ok((handle, shutdown))
    }

    pub(crate) async fn spawn_runtime_managed_run_owned<C, St>(
        worker: Worker<C, St>,
        run_dir: &Path,
    ) -> Result<(WorkerHandle, ShutdownReceiver, tokio::task::JoinHandle<()>), std::io::Error>
    where
        C: LlmClient + Clone + 'static,
        St: Store + WorkerMetadataStore + Clone + Send + Sync + 'static,
    {
        let parent = run_dir
            .parent()
            .ok_or_else(|| std::io::Error::other("run path has no parent"))?;
        Self::spawn_inner(worker, parent, Some(run_dir)).await
    }

    async fn spawn_inner<C, St>(
        worker: Worker<C, St>,
        runtime_base: &Path,
        runtime_run: Option<&Path>,
    ) -> Result<(WorkerHandle, ShutdownReceiver, tokio::task::JoinHandle<()>), std::io::Error>
    where
        C: LlmClient + Clone + 'static,
        St: Store + WorkerMetadataStore + Clone + Send + Sync + 'static,
    {
        let sessions = worker.workdir_sessions();
        let factory_owned_cleanup = worker.startup_feature_cleanup_handle().is_some();
        let result = Self::spawn_initialized(worker, runtime_base, runtime_run).await;
        // An opted-in factory must quiesce its captured Feature tasks before
        // closing their Workdir sessions; this wrapper cannot confirm that order.
        if result.is_err()
            && !factory_owned_cleanup
            && let Err(error) = sessions.close_all().await
        {
            tracing::warn!(%error, "Workdir sessions close after controller startup failure failed");
        }
        result
    }

    async fn spawn_initialized<C, St>(
        worker: Worker<C, St>,
        runtime_base: &Path,
        runtime_run: Option<&Path>,
    ) -> Result<(WorkerHandle, ShutdownReceiver, tokio::task::JoinHandle<()>), std::io::Error>
    where
        C: LlmClient + Clone + 'static,
        St: Store + WorkerMetadataStore + Clone + Send + Sync + 'static,
    {
        Self::initialize(worker, runtime_base, runtime_run)
            .await?
            .publish()
            .await
    }

    async fn initialize<C, St>(
        worker: Worker<C, St>,
        runtime_base: &Path,
        runtime_run: Option<&Path>,
    ) -> Result<InitializedWorkerController<C, St>, std::io::Error>
    where
        C: LlmClient + Clone + 'static,
        St: Store + WorkerMetadataStore + Clone + Send + Sync + 'static,
    {
        Self::initialize_inner(worker, runtime_base, runtime_run, true).await
    }

    pub(crate) async fn initialize_restored<St>(
        worker: Worker<Box<dyn LlmClient>, St>,
        run_dir: &Path,
    ) -> std::io::Result<InitializedWorkerController<Box<dyn LlmClient>, St>>
    where
        St: Store + WorkerMetadataStore + Clone + Send + Sync + 'static,
    {
        let parent = run_dir
            .parent()
            .ok_or_else(|| std::io::Error::other("run path has no parent"))?;
        Self::initialize_inner(worker, parent, Some(run_dir), false).await
    }

    async fn initialize_inner<C, St>(
        mut worker: Worker<C, St>,
        runtime_base: &Path,
        runtime_run: Option<&Path>,
        canonical_startup: bool,
    ) -> Result<InitializedWorkerController<C, St>, std::io::Error>
    where
        C: LlmClient + Clone + 'static,
        St: Store + WorkerMetadataStore + Clone + Send + Sync + 'static,
    {
        if !canonical_startup && worker.needs_initial_session_head_materialization() {
            return Err(std::io::Error::other(
                "restore source requires operation-owned head recovery",
            ));
        }
        worker
            .validate_subjektiv_body_publication()
            .map_err(std::io::Error::other)?;

        // === 1. Initialization (channels / RuntimeDir / worker-immutable
        //         snapshots / SpawnedWorkerRegistry / alerter attach) ===
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let (method_tx, method_rx) = mpsc::channel::<Method>(32);
        let (working_event_tx, _) = broadcast::channel::<Event>(256);
        let alerter = Alerter::new(working_event_tx.clone());
        let in_flight = InFlightEvents::new(working_event_tx.clone());
        worker.attach_in_flight_events(in_flight.clone());

        // Retain run artifacts before registering the Worker tools.
        let runtime_dir = Arc::new(if let Some(run_dir) = runtime_run {
            RuntimeDir::create_worker_run(run_dir).await?
        } else {
            RuntimeDir::create(runtime_base, &worker.manifest().worker.name).await?
        });

        let spawner_name = worker.manifest().worker.name.clone();
        let registry_reclaim = SpawnedWorkerRegistry::prepare_from_worker_state_with_reclaim(
            runtime_dir.clone(),
            worker.store().clone(),
            spawner_name.clone(),
            Some(worker.scope().clone()),
        )?;
        let spawned_registry = registry_reclaim.registry.clone();

        // Hand the alerter to the Worker so internal operations (compaction,
        // AGENTS.md ingestion during the first turn) can emit user-facing
        // notifications on the same channel.
        worker.attach_alerter(alerter.clone());
        // Also hand the raw broadcast sender so Worker-internal operations
        // can emit typed lifecycle `Event`s (currently: compact progress).
        worker.attach_internal_worker_registry(spawned_registry.clone());
        worker.attach_working_event_tx(working_event_tx.clone());

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
            runtime_base.to_path_buf(),
            spawned_registry.clone(),
            Some(method_tx.downgrade()),
            None,
        )
        .await?;

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

        // Fresh Workers persist their current model-visible head before
        // controller exposure. Feature installation above must happen first so a
        // fresh subject Worker captures the current resident surface in its durable
        // prompt. Restored Workers defer generic Feature restore notification until
        // the next model-visible context materialization; only an installed
        // Feature may then request a typed durable history append.
        if worker.needs_initial_session_head_materialization() {
            worker
                .materialize_durable_session_head()
                .await
                .map_err(|error| std::io::Error::other(error.to_string()))?;
        }

        // === 4. WorkerSharedState + WorkerHandle ===
        let manifest_toml = toml::to_string_pretty(worker.manifest()).unwrap_or_default();
        worker
            .recover_unfinished_compaction()
            .await
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        let greeting = build_greeting(&worker);
        let shared_state = Arc::new(WorkerSharedState::new(
            worker.manifest().worker.name.clone(),
            worker.segment_id(),
            manifest_toml,
            greeting,
        ));
        shared_state.set_feature_invocations(worker.feature_invocations());
        let usage_state = shared_state.clone();
        let usage_events = working_event_tx.clone();
        worker.engine_mut().on_usage(move |event| {
            publish_context_usage(
                &usage_state,
                &usage_events,
                event.input_tokens,
                protocol::ContextTokenSource::Measured,
            );
        });
        if let Some(fs_for_view) = fs_for_view.as_ref() {
            shared_state.set_fs_view(crate::fs_view::WorkerFsView::new(fs_for_view.clone()));
        }
        let artifact_store: Arc<dyn Store> = Arc::new(worker.store().clone());
        let session_id = worker.session_id();
        let pending_activations = worker.pending_activation_state();
        let release_control =
            crate::subject_release::ReleaseControl::new(worker.mutation_authority());
        shared_state
            .subject_release
            .set(release_control)
            .ok()
            .expect("one Controller release authority");
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

        let command_events = fs_for_view
            .as_ref()
            .map(|session| PreparedWorkdirCommandEvents::new(session, &handle.in_flight));
        Ok(InitializedWorkerController {
            worker,
            handle,
            method_rx,
            shutdown_tx,
            shutdown_rx,
            registry_reclaim,
            shutdown_after_idle,
            command_events,
        })
    }
}

impl<C, St> InitializedWorkerController<C, St>
where
    C: LlmClient + Clone + 'static,
    St: Store + WorkerMetadataStore + Clone + Send + Sync + 'static,
{
    async fn publish(
        mut self,
    ) -> Result<(WorkerHandle, ShutdownReceiver, tokio::task::JoinHandle<()>), std::io::Error> {
        let publication = self.prepare_publication().await?;
        Ok(self.start(publication))
    }

    pub(crate) async fn prepare_publication(
        &mut self,
    ) -> std::io::Result<PreparedControllerPublication> {
        let loaded_registry = self.registry_reclaim.apply().await?;
        debug_assert!(Arc::ptr_eq(
            &loaded_registry.registry,
            &self.handle.spawned_registry
        ));
        if loaded_registry.reclaimed_unreachable {
            self.worker.push_notify("Restored Worker state contained unreachable delegated child Workers; their delegated write scopes were reclaimed before resume.".to_string());
        }
        let command_observer = self
            .command_events
            .take()
            .and_then(PreparedWorkdirCommandEvents::start);
        Ok(PreparedControllerPublication { command_observer })
    }

    pub(crate) fn start(
        self,
        publication: PreparedControllerPublication,
    ) -> (WorkerHandle, ShutdownReceiver, tokio::task::JoinHandle<()>) {
        let Self {
            mut worker,
            handle,
            method_rx,
            shutdown_tx,
            shutdown_rx,
            shutdown_after_idle,
            ..
        } = self;
        let PreparedControllerPublication { command_observer } = publication;
        let runtime_dir = handle.runtime_dir.clone();
        let shared_state = handle.shared_state.clone();
        let working_event_tx = handle.working_event_tx.clone();
        let spawned_registry = handle.spawned_registry.clone();

        // === 5. controller_loop ===
        // Clone cancel sender and notification buffer before moving worker
        // into the controller task so the in-flight turn can be reached
        // via these handles while worker itself is borrowed by drive_turn.
        let cancel_tx = worker.engine_mut().cancel_sender();
        let pause_tx = worker.engine_mut().pause_sender();
        let notify_buffer = worker.notify_buffer_handle();

        worker.finish_startup_feature_cleanup_capture();
        let controller_task = tokio::spawn(controller_loop(
            worker,
            method_rx,
            working_event_tx,
            shared_state,
            runtime_dir,
            cancel_tx,
            pause_tx,
            notify_buffer,
            spawned_registry,
            shutdown_tx,
            shutdown_after_idle,
            command_observer,
        ));

        (handle, shutdown_rx, controller_task)
    }
}

pub(crate) fn wire_workdir_command_events(
    session: &Arc<dyn WorkdirSession>,
    in_flight: &InFlightEvents,
) -> Option<tokio::task::JoinHandle<()>> {
    PreparedWorkdirCommandEvents::new(session, in_flight).start()
}

struct PreparedWorkdirCommandEvents {
    session: std::sync::Weak<dyn WorkdirSession>,
    in_flight: InFlightEvents,
    events: Option<broadcast::Receiver<WorkdirCommandEvent>>,
}

impl PreparedWorkdirCommandEvents {
    fn new(session: &Arc<dyn WorkdirSession>, in_flight: &InFlightEvents) -> Self {
        // Subscribe before reading command state; hold the receiver privately
        // across the durable decision instead of creating a publication gap.
        let events = session.subscribe_command_events();
        in_flight.replace_command_snapshot(protocol_command_snapshots(session.as_ref()));
        Self {
            session: Arc::downgrade(session),
            in_flight: in_flight.clone(),
            events,
        }
    }

    fn start(self) -> Option<tokio::task::JoinHandle<()>> {
        let Self {
            session,
            in_flight,
            events,
        } = self;
        let mut events = events?;
        Some(tokio::spawn(async move {
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
        }))
    }
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

    // Usage is published from committed LlmUsage records through the session-log
    // lane. Snapshot and live usage therefore share one exact, gap-free boundary.

    let tx = working_event_tx.clone();
    worker.on_error(move |event| {
        let _ = tx.send(Event::Error {
            code: ErrorCode::ProviderError,
            message: event.message.clone(),
        });
    });

    wire_engine_warnings(worker, alerter);

    // History-append broadcasts (previously `Event::SystemMessage`)
    // have been removed: every persistent history item is now committed
    // through the session-log sink as a typed `LogEntry`, and clients
    // see it via `Event::Snapshot` + live `Event::Entry`. The
    // per-item commit channel is wired at the top of this function.
}

/// Forward genuine Engine warnings to both live clients and the alert snapshot.
fn wire_engine_warnings<C: LlmClient, A: Send + Sync>(
    engine: &mut agen::Engine<C, agen::state::Mutable, A>,
    alerter: &Alerter,
) {
    let alerter = alerter.clone();
    engine.on_warning(move |message| {
        alerter.alert(AlertLevel::Warn, AlertSource::Engine, message.to_owned());
    });
}

fn validate_memory_lifecycle_targets(
    memory: &manifest::MemoryFeatureProfileConfig,
    subjektiv: &manifest::SubjektivFeatureProfileConfig,
) -> std::io::Result<()> {
    let legacy_lifecycle =
        memory.enabled && (memory.extraction.enabled || memory.consolidation.request_enabled);
    let subjektiv_lifecycle = subjektiv.enabled && subjektiv.extraction.enabled;
    if legacy_lifecycle && subjektiv_lifecycle {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Workspace Memory and subjektiv lifecycles cannot both be enabled for one Worker",
        ));
    }
    Ok(())
}

/// Compose Worker Features with Host-owned live bindings and install their
/// staged contributions. Remaining direct registrations are Worker-lifetime
/// infrastructure (paste/Workdir) and the image/web migration follow-ups.
pub(crate) async fn register_worker_tools<C, St>(
    worker: &mut Worker<C, St>,

    runtime_base: PathBuf,
    spawned_registry: Arc<SpawnedWorkerRegistry>,
    parent_method_tx: Option<mpsc::WeakSender<Method>>,
    inherited_workdir_tool_broker: Option<workdir::WorkdirToolBrokerRouter>,
) -> std::io::Result<Option<workdir::WorkdirSessionHandle>>
where
    C: LlmClient + Clone + 'static,
    St: Store + WorkerMetadataStore + Clone + 'static,
{
    // Worker-immutable snapshots taken before the mutable worker borrow
    // below so the worker borrow doesn't conflict with reads on `worker`.
    let feature_config = worker.manifest().feature.clone();
    // New activation requires admission before any initialization side effect.
    // Restore reconstructs saved composition; operation authority stays live.
    worker
        .authorize_feature_configuration()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::PermissionDenied, error))?;
    let workdir_sessions = worker.workdir_sessions();
    let restrict_workdir_tools_to_current_capabilities = inherited_workdir_tool_broker.is_some();
    let workdir_tool_broker = Some(
        inherited_workdir_tool_broker
            .unwrap_or_else(|| workdir::WorkdirToolBrokerRouter::new(workdir_sessions.clone())),
    );
    let workdir_for_view = workdir_sessions.only_session();
    let local_filesystem = worker.local_working_directory().cloned();
    let local_workspace_root = local_filesystem.as_ref().map(|local| local.root.clone());
    let task_feature = worker.task_feature();
    let web_config = worker.manifest().web.clone();
    let mcp_config = worker.manifest().mcp.clone();
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
    let backend_job = worker.backend_job().cloned();
    let subjektiv_host = worker.subjektiv_host_connection()?;
    // Bounded Jobs complete their lifecycle synchronously, never by post-run background work.
    let lifecycle_enabled = worker.manifest_lifecycle_features_enabled() && backend_job.is_none();
    let prompts = worker.prompts().clone();
    let tracker = tools::Tracker::new();
    let bash_output = spawned_registry.bash_output();
    {
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
        // Keep the alias-routed schemas installed for the Worker lifetime. The
        // attachment set may be empty or mutate after registration, so target
        // resolution and provider capability checks happen per invocation.
        let mut workdir_tools = if restrict_workdir_tools_to_current_capabilities {
            tools::routed_builtin_tools_for_current_capabilities(
                workdir_sessions.clone(),
                tracker.clone(),
                bash_output.clone(),
            )
        } else {
            tools::routed_builtin_tools(
                workdir_sessions.clone(),
                tracker.clone(),
                bash_output.clone(),
            )
        };
        if let Some(policy) = &spawner_manifest.permissions {
            workdir_tools.retain(|definition| {
                !crate::permission::tool_is_unconditionally_denied(policy, &definition().0.name)
            });
        }
        worker.engine_mut().register_tools(workdir_tools);
    }
    if feature_config.image.enabled && model_supports_image_attachments(&spawner_manifest.model) {
        worker
            .engine_mut()
            .register_tool(tools::routed_view_image_tool(workdir_sessions));
    }
    if feature_config.web.enabled {
        let web = tools::WebTools::new(web_config).with_authorizer(Arc::new(
            crate::feature::builtin::web::WorkspaceWebAuthorizer(worker.workspace_client_handle()),
        ));
        worker.engine_mut().register_tools(vec![
            tools::web_search_tool(web.clone()),
            tools::web_fetch_tool(web),
        ]);
    }

    let worker_enabled = feature_config.worker.enabled;
    let sub_worker_enabled = feature_config.sub_worker.enabled;
    let journal_grant = backend_job
        .as_ref()
        .filter(|binding| binding.subjektiv_journal)
        .map(|binding| {
            crate::subjektiv::BackendJournalJobHost::new(worker.workspace_client_handle(), binding)
                .map(|host| host.into_grant())
        })
        .transpose()?;
    let (mut feature_registry, journal_finalizer) = if let Some(grant) = journal_grant {
        (grant.features, grant.finalizer)
    } else {
        (FeatureRegistryBuilder::new(), None)
    };
    match worker.plugin_tools() {
        Some(tools) => {
            feature_registry.add_module(tools);
        }
        None if !worker.manifest().plugins.is_empty() => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Plugin selection requires a persisted plan and current Host preparation",
            ));
        }
        None => {}
    }
    feature_registry
        .add_module(crate::feature::builtin::chat_invocation::AttachmentInvocationFeature);
    if let Some(binding) = backend_job.as_ref() {
        let workspace_client = worker.workspace_client_handle();
        if !workspace_client.is_available() || workspace_client.workspace_id().is_none() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Backend Job result capability requires Backend Workspace API authority",
            ));
        }
        if binding.subjektiv_consolidation || binding.subjektiv_journal {
            feature_registry.add_module(
                crate::feature::builtin::backend_job_input::SubjectJobInputFeature::new(
                    workspace_client.clone(),
                    if binding.subjektiv_consolidation {
                        "memory.consolidation.supply"
                    } else {
                        "journal.job.input.supply"
                    },
                ),
            );
        }
        let mut result_feature =
            crate::feature::builtin::backend_job_result::BackendJobResultFeature::new(
                workspace_client.clone(),
            );
        if binding.subjektiv_consolidation {
            let surface = crate::feature::builtin::memory_surface_lifecycle::SubjektivSurfaceLifecycleFeature::for_host(
                subjektiv_host.clone().ok_or_else(|| std::io::Error::other("Backend Job requires subjektiv Host capability"))?, spawner_manifest.clone(), worker.llm_client_handle(),
                prompts.clone(), spawner_workspace_context.clone(),
            )?;
            result_feature = result_feature.with_surface(surface);
        }
        if let Some(finalizer) = journal_finalizer {
            result_feature = result_feature.with_finalizer(
                finalizer,
                spawner_manifest.clone(),
                worker.llm_client_handle(),
            )?;
        }
        feature_registry.add_module(result_feature);
    }
    let memory_profile = &worker.manifest().feature.memory.profile;
    let subjektiv_profile = &worker.manifest().feature.subjektiv.profile;
    if subjektiv_host.is_some() {
        validate_memory_lifecycle_targets(memory_profile, subjektiv_profile)?;
    }
    let memory_install_plan = crate::feature::builtin::memory::MemoryFeatureInstallPlan::prepare(
        worker.manifest(),
        worker.workspace_client_handle(),
        worker.prompts().load_full(),
    )?;
    let mut feature_prompt_contribution = memory_install_plan.as_ref().map(|plan| {
        (
            plan.resident_summary_source.clone(),
            if backend_job.is_none() {
                plan.system_prompt_override.clone()
            } else {
                None
            },
        )
    });
    let memory_lifecycle_config = worker.manifest().feature.memory.clone();
    if let Some(plan) = memory_install_plan {
        feature_registry.add_module(plan.module);
    }
    let subjektiv_consolidation_plan =
        crate::feature::builtin::memory::SubjektivConsolidationFeatureInstallPlan::prepare(
            worker.manifest(),
            subjektiv_host.clone(),
            worker.prompts().load_full(),
        )?;
    if let Some(plan) = subjektiv_consolidation_plan {
        if backend_job.is_none() {
            feature_prompt_contribution = Some((None, Some(plan.system_prompt_override)));
        }
        feature_registry.add_module(plan.module);
    }
    let ordinary_subjektiv_features_enabled = subjektiv_host.is_some()
        && crate::feature::builtin::memory::ordinary_subjektiv_features_enabled(worker.manifest());
    if let Some(resident_summary_source) =
        crate::feature::builtin::memory::ordinary_subjektiv_resident_summary_source(
            worker.manifest(),
            subjektiv_host.clone(),
            worker.prompts(),
        )?
    {
        let workspace_id = subjektiv_host
            .as_ref()
            .and_then(|host| host.host.workspace_id());
        if let Some(refresh_feature) =
            crate::feature::builtin::memory::SubjektivResidentRestoreRefreshFeature::for_connection(
                lifecycle_enabled,
                Arc::clone(&resident_summary_source),
                worker.prompts(),
                workspace_id,
            )
        {
            feature_registry.add_module(refresh_feature);
        }
        feature_prompt_contribution = Some((Some(resident_summary_source), None));
    }
    if backend_job.is_none()
        && let Some(chat) =
            crate::feature::builtin::subjektiv_chat::SubjektivChatFeature::from_resolved_config(
                &worker.manifest().feature.subjektiv,
                subjektiv_host.clone(),
            )?
    {
        feature_registry.add_module(chat);
    }
    if ordinary_subjektiv_features_enabled
        && let Some(subjektiv_memory) =
            crate::feature::builtin::subjektiv_memory::SubjektivMemoryFeature::from_resolved_config(
                &worker.manifest().feature.subjektiv,
                worker.committed_session_capture_handle(),
                subjektiv_host.clone(),
            )?
    {
        feature_registry.add_module(subjektiv_memory);
    }
    if ordinary_subjektiv_features_enabled
        && let Some(subjektiv_sessions) =
            crate::feature::builtin::subjektiv_session::SubjektivSessionFeature::from_resolved_config(
                &worker.manifest().feature.subjektiv,
                subjektiv_host.clone(),
            )?
    {
        feature_registry.add_module(subjektiv_sessions);
    }
    if ordinary_subjektiv_features_enabled
        && lifecycle_enabled
        && let Some(host) = subjektiv_host.as_ref()
    {
        worker.install_session_context_source(Arc::new(
            crate::feature::builtin::journal::JournalContextSource::new(
                host.clone(),
                worker.prompts(),
            ),
        ));
    }
    if ordinary_subjektiv_features_enabled
        && lifecycle_enabled
        && let Some(host) = subjektiv_host.as_ref()
        && host.host.journal_recording_granted()
    {
        feature_registry.add_module(crate::subjektiv::JournalRecordingFeature::new(
            host.clone(),
        )?);
    }
    if let Some(memory_lifecycle) =
        crate::feature::builtin::memory_lifecycle::MemoryLifecycleFeature::from_resolved_config(
            lifecycle_enabled,
            memory_lifecycle_config,
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
    if backend_job.is_none() && let Some(surface_lifecycle) = crate::feature::builtin::memory_surface_lifecycle::SubjektivSurfaceLifecycleFeature::from_manifest(
        lifecycle_enabled,
        subjektiv_host.clone(),
        spawner_manifest.clone(),
        worker.llm_client_handle(),
        prompts.clone(),
        spawner_workspace_context.clone(),
    )? {
        feature_registry.add_module(surface_lifecycle);
    }
    if ordinary_subjektiv_features_enabled
        && let Some(subjektiv_lifecycle) =
            crate::feature::builtin::memory_lifecycle::SubjektivLifecycleFeature::from_resolved_config(
                lifecycle_enabled,
                worker.manifest().feature.subjektiv.clone(),
                worker.committed_session_capture_handle(),
                worker.session_extension_handle(),
                subjektiv_host.clone(),
                spawner_manifest.clone(),
                worker.llm_client_handle(),
                prompts.clone(),
                spawner_workspace_context.clone(),
                worker.working_event_sender(),
            )?
    {
        feature_registry.add_module(subjektiv_lifecycle);
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
        feature_registry.add_module(crate::feature::builtin::ticket::ticket_tools_feature(
            workspace_client,
            ticket_access,
        ));
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
    let wip_mode = worker.manifest().worker.mode == manifest::WorkerMode::Wip;
    let drive_client = worker.workspace_client_handle();
    if feature_config.drive.enabled
        && (!drive_client.is_available()
            || drive_client.workspace_id().is_none_or(|id| id.is_empty()))
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotConnected,
            "Drive is configured but its Backend Workspace connection is unavailable; no Drive access started",
        ));
    }
    let drive_feature = crate::feature::builtin::drive::DriveFeature::configured(
        drive_client,
        feature_config.drive.enabled,
        worker.workdir_sessions(),
    )
    .map(|feature| feature.with_permissions(worker.manifest().permissions.clone()));
    if let Some(module) = &drive_feature {
        feature_registry.add_module(module.clone());
    }
    let workspace_config_feature =
        crate::feature::builtin::workspace_config::WorkspaceConfigFeature::configured(
            worker.workspace_client_handle(),
            feature_config.workspace_config.enabled,
            wip_mode,
        );
    if let Some(module) = &workspace_config_feature {
        feature_registry.add_module(module.clone());
    }
    let mut workdir_wip_feature = None;
    if feature_config.manage_workdir.enabled
        || (wip_mode
            && feature_config.workdir_catalog.enabled
            && worker.workspace_client_handle().is_available())
    {
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
        let module =
            crate::feature::builtin::manage_workdir::ManageWorkdirFeature::with_child_lifecycle(
                workspace_client,
                worker.workdir_sessions(),
                Arc::new(move || {
                    let child_registry = shutdown_registry.clone();
                    Box::pin(async move { child_registry.shutdown_internal().await })
                }),
                Arc::new(move || reopen_registry.reopen_internal()),
            );
        if feature_config.manage_workdir.enabled {
            feature_registry.add_module(module.clone());
        }
        workdir_wip_feature = Some(module);
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
    if let Some(workspace_root) = local_workspace_root.as_ref() {
        let module = if let Some(cleanup) = worker.startup_feature_cleanup_handle() {
            crate::feature::mcp::discover_stdio_tool_feature_with_cleanup(
                &mcp_config,
                workspace_root,
                &cleanup.mcp_discovery(),
            )
            .await?
        } else {
            crate::feature::mcp::discover_stdio_tool_feature(&mcp_config, workspace_root).await
        };
        if let Some(module) = module {
            feature_registry = feature_registry.with_module(module);
        }
    }

    let host_worker_observation_provider = worker.worker_observation_provider();
    let wip_permissions = worker.manifest().permissions.clone();
    let mut wip_mount_registry = crate::wip::WipMountRegistry::new();
    if wip_mode && let Some(feature) = &drive_feature {
        crate::feature::builtin::drive::wip::mount_drive_wip(&mut wip_mount_registry, feature)
            .map_err(|error| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("mount Drive WIP projection: {error}"),
                )
            })?;
    }
    if let Some(feature) = &workspace_config_feature {
        crate::feature::builtin::workspace_config::wip::mount_workspace_config_wip(
            &mut wip_mount_registry,
            feature,
        )
        .map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("mount Workspace config WIP projection: {error}"),
            )
        })?;
    }
    if wip_mode {
        crate::checkout::mount_checkouts(
            &mut wip_mount_registry,
            worker.workdir_sessions(),
            tracker.clone(),
            wip_permissions.clone(),
        )
        .map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("mount checkout WIP projection: {error}"),
            )
        })?;
    }
    if wip_mode {
        crate::feature::builtin::bash::mount(
            &mut wip_mount_registry,
            worker.workdir_sessions(),
            bash_output.clone(),
            wip_permissions.clone(),
        )
        .map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("mount Bash output WIP projection: {error}"),
            )
        })?;
    }
    if wip_mode && let Some(feature) = &workdir_wip_feature {
        crate::feature::builtin::manage_workdir::wip::mount_workspace_workdir_wip(
            &mut wip_mount_registry,
            feature,
            feature_config.workdir_catalog.enabled,
            feature_config.manage_workdir.enabled,
            wip_permissions.clone(),
        )
        .map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("mount Workdir WIP projection: {error}"),
            )
        })?;
    }
    {
        let workspace_client = worker.workspace_client_handle();

        // Allocate cross-domain input reference coordinates before mounting either provider.
        // Allocation is not publication or authorization; current handlers still check both.
        let mut allocate = |enabled, owner, name| {
            if enabled && wip_mode {
                wip_mount_registry
                    .allocate_namespace(owner, name)
                    .map(Some)
                    .map_err(|error| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            format!("allocate {owner} WIP namespace: {error}"),
                        )
                    })
            } else {
                Ok(None)
            }
        };
        let merge_request_namespace = allocate(
            feature_config.merge_request.any() || feature_config.ticket.enabled,
            "merge-request",
            "merge-requests",
        )?;
        let ticket_namespace = allocate(
            feature_config.ticket.enabled || feature_config.merge_request.open,
            "ticket",
            "tickets",
        )?;
        let objective_namespace = allocate(
            feature_config.objective.enabled || feature_config.ticket.enabled,
            "objective",
            "objectives",
        )?;

        if feature_config.merge_request.any()
            && let Some(namespace_route) = &merge_request_namespace
        {
            crate::feature::builtin::merge_request::mount_workspace_http_merge_request_wip(
                &mut wip_mount_registry,
                workspace_client.clone(),
                feature_config.merge_request,
                wip_permissions.clone(),
                &namespace_route,
            )
            .map_err(|error| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("mount Merge Request WIP projection: {error}"),
                )
            })?;
        }

        if feature_config.ticket.enabled
            && let Some(namespace_route) = &ticket_namespace
        {
            let ticket_access = crate::feature::builtin::ticket::TicketFeatureAccess {
                authoring: feature_config.ticket.authoring,
                thread: feature_config.ticket.thread,
                intake: feature_config.ticket.intake,
                workflow: feature_config.ticket.workflow,
            };
            crate::feature::builtin::ticket::mount_workspace_http_ticket_wip(
                &mut wip_mount_registry,
                workspace_client.clone(),
                ticket_access,
                wip_permissions.clone(),
                &namespace_route,
            )
            .map_err(|error| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("mount Ticket WIP projection: {error}"),
                )
            })?;
        }

        crate::feature::builtin::objective::composition::compose(
            &mut feature_registry,
            feature_config.objective.enabled,
            workspace_client.clone(),
            objective_namespace.map(|route| (route, wip_permissions.clone())),
        )?;

        let mut observation_providers: Vec<
            Arc<dyn crate::feature::contracts::WorkerObservationProvider>,
        > = Vec::new();

        if let Some(provider) = crate::feature::builtin::sub_worker::compose(
            &mut feature_registry,
            spawner_manifest,
            spawner_workspace_context,
            parent_notifications,
            runtime_base,
            local_workspace_root,
            workdir_tool_broker,
            spawned_registry,
            prompts,
        ) {
            observation_providers.push(provider);
        }
        // A scoped Backend provider belongs to the Worker Feature's composition,
        // including ordinary spawn and restore. Runtime's optional local peer
        // provider is not its enablement or authority: every actual capture still
        // requires current operation use and the target's Backend control grant.
        if worker_enabled {
            observation_providers.push(Arc::new(
                crate::feature::builtin::worker_observation::WorkspaceClientWorkerObservationProvider::new(workspace_client),
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
    let feature_install_report =
        worker.install_features_with_wip(feature_registry, Some(&mut wip_mount_registry));
    if feature_install_report.has_errors() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "Worker feature installation failed: {}",
                feature_install_report.error_message()
            ),
        ));
    }
    if let Some((resident_summary, system_prompt_override)) = feature_prompt_contribution {
        worker.install_system_prompt_contribution(resident_summary, system_prompt_override);
    }
    if wip_mode {
        let security_context = format!(
            "workspace={};worker={};session={}",
            worker.workspace_id().map_or("standalone", |id| id.as_str()),
            worker.manifest().worker.name,
            worker.session_id()
        );
        crate::wip::install_wip_mode_with_mounts(
            worker.engine_mut(),
            wip_permissions,
            security_context,
            wip_mount_registry,
        )
        .map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("install WIP Worker surface: {error}"),
            )
        })?;
    }
    worker.attach_tracker(tracker);
    Ok(workdir_for_view)
}

fn durably_accept_notification<St>(
    notification_request_id: String,
    message: String,
    source_namespace: String,
    provenance: session_store::LoggedSessionHistoryOrigin,
    pending_submissions: &PendingSubmissionHandle<St>,
    working_event_tx: &broadcast::Sender<Event>,
) -> bool
where
    St: session_store::Store + Clone,
{
    let request_id = notification_request_id.clone();
    match pending_submissions.accept_notification_from_source(
        notification_request_id,
        message,
        source_namespace,
        provenance,
    ) {
        Ok(_) => {
            let _ = working_event_tx.send(Event::NotificationAccepted {
                notification_request_id: request_id,
            });
            true
        }
        Err(error) => {
            let _ = working_event_tx.send(Event::NotificationRejected {
                notification_request_id: request_id,
                message: error.to_string(),
            });
            false
        }
    }
}

fn reject_idle_only_submission(method: &Method, events: &broadcast::Sender<Event>) -> bool {
    let Method::SubmitIfIdle {
        submission_request_id,
        ..
    } = method
    else {
        return false;
    };
    let _ = events.send(Event::SubmissionRejected {
        submission_request_id: submission_request_id.clone(),
        message: "WorkerSendInput requires an Idle Worker and never queues input. Use WorkerNotify for information about work already in progress.".to_string(),
    });
    true
}

fn durably_accept_method_while_busy<St>(
    method: Method,
    pending_submissions: &PendingSubmissionHandle<St>,
    working_event_tx: &broadcast::Sender<Event>,
) -> Option<Method>
where
    St: session_store::Store + Clone,
{
    if reject_idle_only_submission(&method, working_event_tx) {
        return None;
    }
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
                false,
            ) {
                Ok(acceptance) => {
                    let _ = working_event_tx.send(Event::SubmissionAccepted {
                        submission_request_id: acceptance.submission_request_id,
                        submission_id: acceptance.submission_id,
                        disposition: acceptance.disposition,
                    });
                }
                Err(error) => {
                    let _ = working_event_tx.send(Event::SubmissionRejected {
                        submission_request_id: request_id,
                        message: error.to_string(),
                    });
                }
            }
            None
        }
        Method::SubmitTracked {
            submission_request_id,
            input,
            source,
        } => {
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
                }
                Err(error) => {
                    let _ = working_event_tx.send(Event::SubmissionRejected {
                        submission_request_id: request_id,
                        message: error.to_string(),
                    });
                }
            }
            None
        }
        Method::Notify {
            notification_request_id,
            message,
        } => {
            durably_accept_notification(
                notification_request_id,
                message,
                pending_submissions.direct_client_namespace(),
                session_store::LoggedSessionHistoryOrigin::LegacyUnknown,
                pending_submissions,
                working_event_tx,
            );
            None
        }
        Method::NotifyTracked {
            receipt_scope,
            notification_request_id,
            message,
            source,
        } => {
            let (source_namespace, provenance) =
                resolved_input_source(pending_submissions, &source);
            let source_namespace = receipt_scope.namespace(source_namespace);
            durably_accept_notification(
                notification_request_id,
                message,
                source_namespace,
                provenance,
                pending_submissions,
                working_event_tx,
            );
            None
        }
        method => Some(method),
    }
}

fn reject_method_while_attention_locked(
    method: &Method,
    working_event_tx: &broadcast::Sender<Event>,
    message: &str,
) -> bool {
    match method {
        Method::Shutdown { .. } | Method::ListPendingSubmissions => false,
        Method::Submit {
            submission_request_id,
            ..
        }
        | Method::SubmitTracked {
            submission_request_id,
            ..
        }
        | Method::SubmitIfIdle {
            submission_request_id,
            ..
        } => {
            let _ = working_event_tx.send(Event::SubmissionRejected {
                submission_request_id: submission_request_id.clone(),
                message: message.to_owned(),
            });
            true
        }
        Method::Notify {
            notification_request_id,
            ..
        }
        | Method::NotifyTracked {
            notification_request_id,
            ..
        } => {
            let _ = working_event_tx.send(Event::NotificationRejected {
                notification_request_id: notification_request_id.clone(),
                message: message.to_owned(),
            });
            true
        }
        _ => {
            let _ = working_event_tx.send(Event::Error {
                code: ErrorCode::InvalidRequest,
                message: message.to_owned(),
            });
            true
        }
    }
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
    _runtime_dir: Arc<RuntimeDir>,
    cancel_tx: mpsc::Sender<()>,
    pause_tx: mpsc::Sender<()>,
    notify_buffer: NotifyBuffer,
    spawned_registry: Arc<SpawnedWorkerRegistry>,
    shutdown_tx: oneshot::Sender<()>,
    shutdown_after_idle: ShutdownAfterIdleRequest,
    mut command_observer: Option<tokio::task::JoinHandle<()>>,
) where
    C: LlmClient + Clone + 'static,
    St: Store + WorkerMetadataStore + Clone + 'static,
{
    let release_control = shared_state
        .subject_release
        .get()
        .expect("Controller release authority installed")
        .clone();
    let _release_lifetime = crate::subject_release::ControllerLifetime(release_control.clone());
    let notification_coalesce_delay = worker.notification_coalesce_delay();
    let pending_submissions = worker.pending_submission_handle();
    let mut pending = match prepare_pending_run(&pending_submissions, &notify_buffer, None, false) {
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
        if worker.has_failed_segment_activation() || worker.has_pending_compaction_cleanup() {
            // A queued activation may have been accepted while the failing turn
            // was still running. Preserve its durable queue entry, but never
            // start it while repair/cleanup attention is required.
            pending = None;
        }
        // here so the status flip → drive_turn → finish sequence lives
        // in one place, regardless of which Method caused it.
        if let Some(run) = pending.take() {
            // A submit/resume/run always provides an immediate model boundary,
            // so any coalescing window ends and the whole FIFO notification
            // batch is injected into that run.
            stage_pending_notifications(&pending_submissions, &notify_buffer);
            // Cancellation is meaningful only for an accepted running turn. Clear
            // idle/stale signals before the status flip; any Cancel/Pause received
            // after this point is delivered to the turn and must not be discarded by
            // the Engine at run start.
            worker.engine_mut().clear_pending_cancel();
            // In-flight display state belongs to the active run only. Defensive
            // clear at run start prevents stale partial output left by an older
            // interrupted/error turn from being carried into the next snapshot.
            worker.clear_in_flight_events();
            let finished_submission_request_id = match &run {
                PendingRun::Submit(submission) => Some(submission.submission_request_id.clone()),
                PendingRun::RunForNotification { .. } | PendingRun::Resume => None,
            };
            let user_input_submit = matches!(&run, PendingRun::Submit(_));
            if !user_input_submit {
                set_controller_status(&shared_state, &working_event_tx, WorkerStatus::Running)
                    .await;
            }
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
                            input_committed_hook(
                                pending_submissions.clone(),
                                committed_submission.submission_id.clone(),
                                input_commit_tx,
                            ),
                        ),
                        &mut method_rx,
                        &working_event_tx,
                        &cancel_tx,
                        &pause_tx,
                        &shared_state,
                        Some((input_commit_rx, committed_submission)),
                        &notify_buffer,
                        &pending_submissions,
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
                        None,
                        &notify_buffer,
                        &pending_submissions,
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
                        None,
                        &notify_buffer,
                        &pending_submissions,
                    )
                    .await
                }
            };
            if !shutdown && may_drain_pending && new_status == WorkerStatus::Idle {
                match prepare_pending_run(&pending_submissions, &notify_buffer, None, false) {
                    Ok(Some(next)) => {
                        pending = Some(next);
                        new_status = WorkerStatus::Running;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        let _ = working_event_tx.send(Event::Error {
                            code: ErrorCode::Internal,
                            message: error.to_string(),
                        });
                    }
                }
            }
            let finished_submission_request_id = (new_status != WorkerStatus::Paused)
                .then_some(finished_submission_request_id)
                .flatten();
            finish_controller_run(
                &mut worker,
                &shared_state,
                &working_event_tx,
                new_status,
                finished_submission_request_id,
            )
            .await;
            if shutdown {
                while let Err(error) = worker
                    .finish_pending_compaction_cleanup_for_shutdown()
                    .await
                {
                    let _ = working_event_tx.send(Event::Error {
                        code: worker_error_code(&error),
                        message: error.to_string(),
                    });
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                let _ = working_event_tx.send(Event::Shutdown);
                break;
            }
            if take_shutdown_request_after_status(&shutdown_after_idle, new_status) {
                let _ = working_event_tx.send(Event::Shutdown);
                break;
            }
            continue;
        }

        // An expired Notify deadline must not repeatedly schedule a run that
        // cannot cross the invocation fence. Keep the accepted notification for
        // a fresh input/recovery boundary instead of retrying business effects.
        let notification_delay = (shared_state.catalog_status() == WorkerStatus::Idle)
            .then(|| {
                notification_coalesce_remaining(
                    &pending_submissions,
                    &notify_buffer,
                    notification_coalesce_delay,
                    worker.can_schedule_notification_run(),
                )
            })
            .flatten();
        let method = if let Some(method) = deferred_methods.pop_front() {
            method
        } else if let Some(remaining) = notification_delay {
            tokio::select! {
                method = method_rx.recv() => match method {
                    Some(method) => method,
                    None => {
                        while let Err(error) =
                            worker.finish_pending_compaction_cleanup_for_shutdown().await
                        {
                            tracing::warn!(error = %error, "shutdown cleanup retry failed");
                            tokio::time::sleep(Duration::from_millis(50)).await;
                        }
                        break 'controller;
                    }
                },
                _ = release_control.notified() => {
                    answer_idle_release(&release_control, &shared_state, &method_rx,
                        pending.is_some() || !deferred_methods.is_empty(),
                        &pending_submissions, &notify_buffer,
                        worker.has_pending_compaction_cleanup() || worker.has_failed_segment_activation());
                    continue;
                }
                _ = notify_buffer.notified() => continue,
                _ = tokio::time::sleep(remaining) => {
                    match prepare_pending_run(
                        &pending_submissions,
                        &notify_buffer,
                        None,
                        true,
                    ) {
                        Ok(Some(next)) => pending = Some(next),
                        Ok(None) => {}
                        Err(error) => {
                            let _ = working_event_tx.send(Event::Error {
                                code: ErrorCode::Internal,
                                message: error.to_string(),
                            });
                        }
                    }
                    continue;
                }
            }
        } else {
            tokio::select! {
                method = method_rx.recv() => match method {
                    Some(method) => method,
                    None => {
                        while let Err(error) =
                            worker.finish_pending_compaction_cleanup_for_shutdown().await
                        {
                            tracing::warn!(error = %error, "shutdown cleanup retry failed");
                            tokio::time::sleep(Duration::from_millis(50)).await;
                        }
                        break 'controller;
                    }
                },
                _ = release_control.notified() => {
                    answer_idle_release(&release_control, &shared_state, &method_rx,
                        pending.is_some() || !deferred_methods.is_empty(),
                        &pending_submissions, &notify_buffer,
                        worker.has_pending_compaction_cleanup() || worker.has_failed_segment_activation());
                    continue;
                }
                _ = notify_buffer.notified() => continue,
            }
        };

        let _method_admission = if release_fenced_method(&method) {
            match release_control.authority.enter() {
                Ok(lease) => Some(lease),
                Err(error) => {
                    let _ = working_event_tx.send(Event::Error {
                        code: ErrorCode::InvalidRequest,
                        message: error.to_string(),
                    });
                    continue;
                }
            }
        } else {
            None
        };

        if shared_state.snapshot().state != WorkerState::Idle
            && reject_idle_only_submission(&method, &working_event_tx)
        {
            continue;
        }

        if worker.has_pending_compaction_cleanup() {
            match worker.retry_pending_compaction_cleanup().await {
                Ok(()) => {
                    set_controller_state(&shared_state, &working_event_tx, WorkerState::Idle).await;
                }
                Err(error) => {
                    if matches!(&method, Method::Shutdown { .. }) {
                        if let Err(error) = worker
                            .finish_pending_compaction_cleanup_for_shutdown()
                            .await
                        {
                            let _ = working_event_tx.send(Event::Error {
                                code: worker_error_code(&error),
                                message: error.to_string(),
                            });
                            continue;
                        }
                    } else {
                        let message = error.to_string();
                        let _ = working_event_tx.send(Event::Error {
                            code: worker_error_code(&error),
                            message: message.clone(),
                        });
                        if reject_method_while_attention_locked(
                            &method,
                            &working_event_tx,
                            &message,
                        ) {
                            continue;
                        }
                    }
                }
            }
        }

        if worker.has_failed_segment_activation()
            && reject_method_while_attention_locked(
                &method,
                &working_event_tx,
                "Segment activation is incomplete; restore or shut down the Worker",
            )
        {
            continue;
        }

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
                    shared_state.catalog_status() == WorkerStatus::Idle,
                ) {
                    Ok(acceptance) => {
                        let _ = working_event_tx.send(Event::SubmissionAccepted {
                            submission_request_id: acceptance.submission_request_id.clone(),
                            submission_id: acceptance.submission_id.clone(),
                            disposition: acceptance.disposition,
                        });
                        if let Some(activation) = acceptance.activation {
                            pending = Some(PendingRun::Submit(activation));
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
            }
            | Method::SubmitIfIdle {
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
                    shared_state.catalog_status() == WorkerStatus::Idle,
                ) {
                    Ok(acceptance) => {
                        let _ = working_event_tx.send(Event::SubmissionAccepted {
                            submission_request_id: acceptance.submission_request_id.clone(),
                            submission_id: acceptance.submission_id.clone(),
                            disposition: acceptance.disposition,
                        });
                        if let Some(activation) = acceptance.activation {
                            pending = Some(PendingRun::Submit(activation));
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
            } => {
                durably_accept_notification(
                    notification_request_id,
                    message,
                    pending_submissions.direct_client_namespace(),
                    session_store::LoggedSessionHistoryOrigin::LegacyUnknown,
                    &pending_submissions,
                    &working_event_tx,
                );
            }

            Method::NotifyTracked {
                receipt_scope,
                notification_request_id,
                message,
                source,
            } => {
                let (source_namespace, provenance) =
                    resolved_input_source(&pending_submissions, &source);
                let source_namespace = receipt_scope.namespace(source_namespace);
                durably_accept_notification(
                    notification_request_id,
                    message,
                    source_namespace,
                    provenance,
                    &pending_submissions,
                    &working_event_tx,
                );
            }

            Method::ListPendingSubmissions => {
                let _ = working_event_tx.send(Event::PendingSubmissionsChanged {
                    pending: pending_submissions.snapshot(),
                });
            }
            Method::CancelPendingSubmission { submission_id } => {
                match pending_submissions.cancel(&submission_id) {
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
            Method::ClearPendingSubmissions {
                expected_submission_ids,
            } => match pending_submissions.clear(&expected_submission_ids) {
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
            Method::ContinuePending { expected_head_id } => {
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
                    Some(&expected_head_id),
                    true,
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
                        set_controller_state(&shared_state, &working_event_tx, WorkerState::Idle)
                            .await;
                    }
                    Err(error) => {
                        set_controller_state(
                            &shared_state,
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
                            _ = release_control.notified() => { release_control.reject_busy(); }
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
                                    Some(method) => {
                                        if let Some(method) = durably_accept_method_while_busy(
                                            method,
                                            &pending_submissions,
                                            &working_event_tx,
                                        ) {
                                            deferred_methods.push_back(method);
                                        }
                                    }
                                    None => {
                                        shutdown_after_compaction = true;
                                        let _ = cancel_tx.send(true);
                                    }
                                }
                            }
                        }
                    }
                };
                publish_context_usage(
                    &shared_state,
                    &working_event_tx,
                    Some(worker.total_tokens().tokens),
                    protocol::ContextTokenSource::Estimated,
                );
                if !matches!(
                    result,
                    Err(WorkerError::Store(_))
                        | Err(WorkerError::WorkerStore(_))
                        | Err(WorkerError::InvalidState(_))
                        | Err(WorkerError::SegmentActivationIncomplete { .. })
                        | Err(WorkerError::CompactionCleanupPending { .. })
                ) {
                    set_controller_state(&shared_state, &working_event_tx, WorkerState::Idle).await;
                }
                if let Err(error) = result
                    && !matches!(&error, WorkerError::CompactCancelled)
                {
                    let _ = working_event_tx.send(Event::Error {
                        code: worker_error_code(&error),
                        message: error.to_string(),
                    });
                }
                if shutdown_after_compaction {
                    while let Err(error) = worker
                        .finish_pending_compaction_cleanup_for_shutdown()
                        .await
                    {
                        let _ = working_event_tx.send(Event::Error {
                            code: worker_error_code(&error),
                            message: error.to_string(),
                        });
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                    let _ = working_event_tx.send(Event::Shutdown);
                    break 'controller;
                }
            }

            Method::RewindTo { entry_id } => match shared_state.catalog_status() {
                WorkerStatus::Idle => {
                    if apply_rewind(&mut worker, &working_event_tx, entry_id).await {
                        worker.clear_in_flight_events();
                        publish_context_usage(
                            &shared_state,
                            &working_event_tx,
                            Some(worker.total_tokens().tokens),
                            protocol::ContextTokenSource::Estimated,
                        );
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
                        message:
                            "Worker is already executing a turn; rewind can only run while idle"
                                .into(),
                    });
                }
            },

            Method::Shutdown { command } => {
                // Shutdown applies to this controller at admission time, not
                // a caller-observed Run. Exact command-id/kind retries remain
                // valid so every shutdown path can reach the cleanup barrier.
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

            Method::ListWorkers | Method::RestoreWorker { .. } => {
                let _ = working_event_tx.send(Event::Error {
                    code: ErrorCode::InvalidRequest,
                    message: "Worker discovery and restore are owned by the Host or Runtime".into(),
                });
            }

            Method::RegisterPeer { name } => {
                match crate::peer::register_peer(
                    worker.store(),
                    &worker.manifest().worker.name,
                    &name,
                ) {
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
                }
            }

            // ListCompletions is handled by the Host protocol adapter (direct
            // response). If it reaches the controller, ignore it.
            Method::ListCompletions { .. } => {}

            Method::WorkerEvent(event) => {
                if handle_inbound_worker_event(event, &notify_buffer) {
                    // Auto-kick a turn if the Worker is idle so the
                    // notification is not stranded. Matches the
                    // `Method::Notify` idle path.
                    if shared_state.catalog_status() == WorkerStatus::Idle {
                        pending = Some(PendingRun::RunForNotification {
                            invoke_kind: protocol::InvokeKind::WorkerEvent,
                        });
                    }
                }
            }
        }
    }

    release_control.close();
    // Close method admission before any fallible child/session cleanup. Existing
    // senders then fail instead of accepting work that this execution can no
    // longer process.
    drop(method_rx);
    // Feature callbacks and tasks share the Worker scope. Stop them before
    // Memory/Workdir teardown so they cannot observe a partially closed Worker.
    worker.stop_feature_runtime("controller shutdown").await;

    loop {
        match spawned_registry.shutdown_internal().await {
            Ok(()) => break,
            Err(error) => {
                tracing::warn!(%error, "Internal SubWorker cleanup failed; retrying");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }

    let sessions = worker.workdir_sessions();
    while !sessions.is_empty() {
        match sessions.close_all().await {
            Ok(()) => break,
            Err(error) => {
                tracing::warn!(%error, "Workdir session set close failed; retrying");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }

    spawned_registry.bash_output().revoke();

    if let Some(observer) = command_observer.take() {
        observer.abort();
        let _ = observer.await;
    }

    let _ = shutdown_tx.send(());
}

/// Stage an agent-visible Internal SubWorker lifecycle event.
fn handle_inbound_worker_event(event: protocol::WorkerEvent, notify_buffer: &NotifyBuffer) -> bool {
    let notify_agent = event.should_notify_agent();
    if notify_agent {
        notify_buffer.push_worker_event(event);
    }
    notify_agent
}

/// Drives a Worker future (one in-flight turn) while concurrently
/// processing incoming methods through an inner select! arm. Returns
/// `(final_status, shutdown_requested, may_drain_pending)`.
#[allow(clippy::too_many_arguments)]
async fn drive_turn<F, St>(
    worker_future: F,
    method_rx: &mut mpsc::Receiver<Method>,
    working_event_tx: &broadcast::Sender<Event>,
    cancel_tx: &mpsc::Sender<()>,
    pause_tx: &mpsc::Sender<()>,
    shared_state: &Arc<WorkerSharedState>,
    mut input_commit: Option<(oneshot::Receiver<()>, crate::worker::PendingSubmission)>,
    notify_buffer: &NotifyBuffer,
    pending_submissions: &crate::worker::PendingSubmissionHandle<St>,
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
            _ = release_requested(shared_state) => {
                if let Some(control) = shared_state.subject_release.get() { control.reject_busy(); }
            }
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
                        let _ = working_event_tx.send(Event::PendingSubmissionsChanged {
                            pending: pending_submissions.snapshot(),
                        });
                    }
                    set_controller_status(
                        shared_state,
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
                            WorkerRunResult::Cancelled if pause_requested => {
                                (WorkerStatus::Paused, RunResult::Paused)
                            }
                            WorkerRunResult::Cancelled => {
                                (WorkerStatus::Idle, RunResult::Cancelled)
                            }
                            WorkerRunResult::Interrupted { .. } if pause_requested => {
                                let _ = working_event_tx.send(Event::RunEnd { result: RunResult::Paused });
                                return (WorkerStatus::Paused, shutdown_requested, false);
                            }
                            WorkerRunResult::Interrupted { code, message } => {
                                let _ = working_event_tx.send(Event::Error {
                                    code,
                                    message: message.clone(),
                                });
                                return (WorkerStatus::Idle, shutdown_requested, false);
                            }
                        };
                        let _ = working_event_tx.send(Event::RunEnd { result: run_result });
                        (status, shutdown_requested, may_drain_pending)
                    }
                    Err(WorkerError::Engine(EngineError::Cancelled)) if pause_requested => {
                        // User-initiated Pause is a normal Paused run-end.
                        let _ = working_event_tx.send(Event::RunEnd { result: RunResult::Paused });
                        (WorkerStatus::Paused, shutdown_requested, false)
                    }
                    Err(e) => {
                        let status = if worker_error_requires_attention(&e) {
                            // Metadata has already committed the replacement, but
                            // one live authority failed to publish. Keep the
                            // controller visibly busy/fail-closed; publishing Idle
                            // would admit another turn against inconsistent state.
                            WorkerStatus::Running
                        } else {
                            WorkerStatus::Idle
                        };
                        if !matches!(&e, WorkerError::RunFailureAlreadyDelivered { .. }) {
                            let code = worker_error_code(&e);
                            let message = e.to_string();
                            let _ = working_event_tx.send(Event::Error {
                                code,
                                message,
                            });
                        }
                        (status, shutdown_requested, false)
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
                    Some(method @ Method::SubmitIfIdle { .. }) => {
                        reject_idle_only_submission(&method, working_event_tx);
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
                    }) => {
                        match pending_submissions.cancel(&submission_id) {
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
                    Some(Method::ClearPendingSubmissions { expected_submission_ids }) => {
                        match pending_submissions.clear(&expected_submission_ids) {
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
                    Some(Method::RewindTo { .. }) => {
                        let _ = working_event_tx.send(Event::Error {
                            code: ErrorCode::AlreadyRunning,
                            message: "Worker is already executing a turn; rewind/compact can only run while idle"
                                .into(),
                        });
                    }
                    Some(Method::Notify {
                        notification_request_id,
                        message,
                    }) => {
                        if durably_accept_notification(
                            notification_request_id,
                            message,
                            pending_submissions.direct_client_namespace(),
                            session_store::LoggedSessionHistoryOrigin::LegacyUnknown,
                            pending_submissions,
                            working_event_tx,
                        ) {
                            stage_pending_notifications(&pending_submissions, notify_buffer);
                        }
                        let _ = working_event_tx.send(Event::PendingSubmissionsChanged {
                            pending: pending_submissions.snapshot(),
                        });
                    }
                    Some(Method::NotifyTracked {
                        receipt_scope,
                        notification_request_id,
                        message,
                        source,
                    }) => {
                        let (source_namespace, provenance) =
                            resolved_input_source(pending_submissions, &source);
                        let source_namespace = receipt_scope.namespace(source_namespace);
                        if durably_accept_notification(
                            notification_request_id,
                            message,
                            source_namespace,
                            provenance,
                            pending_submissions,
                            working_event_tx,
                        ) {
                            stage_pending_notifications(&pending_submissions, notify_buffer);
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
                            notify_buffer,
                        );
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

async fn apply_rewind<C, St>(
    worker: &mut Worker<C, St>,
    working_event_tx: &broadcast::Sender<Event>,
    entry_id: String,
) -> bool
where
    C: LlmClient + 'static,
    St: Store,
{
    match worker.rewind_to(entry_id).await {
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

fn public_reasoning_config(
    reasoning: &agen::llm_client::capability::ReasoningControl,
) -> protocol::ReasoningConfig {
    match reasoning {
        agen::llm_client::capability::ReasoningControl::Effort(effort) => {
            protocol::ReasoningConfig::Effort {
                effort: effort.as_str().to_owned(),
            }
        }
        agen::llm_client::capability::ReasoningControl::BudgetTokens(budget_tokens) => {
            protocol::ReasoningConfig::BudgetTokens {
                budget_tokens: *budget_tokens,
            }
        }
    }
}

pub(crate) fn build_greeting<C, St>(worker: &Worker<C, St>) -> protocol::Greeting
where
    C: LlmClient + 'static,
    St: Store,
{
    let manifest = worker.manifest();
    // `build_client` already succeeded with this manifest, so catalog resolution
    // must also succeed. Fall back to "unknown" on failure as a precaution.
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
    let context_tokens = worker.total_tokens().tokens;
    protocol::Greeting {
        worker_name: manifest.worker.name.clone(),
        cwd: worker
            .local_working_directory()
            .map(|local| local.cwd.display().to_string())
            .unwrap_or_default(),
        provider: provider_name,
        model: model_id,
        reasoning: manifest
            .engine
            .reasoning
            .as_ref()
            .map(public_reasoning_config),
        scope_summary: worker.scope_snapshot().summary(),
        tools: tool_names,
        context_window,
        context_tokens,
        context_usage: Some(protocol::ContextUsage {
            tokens: context_tokens,
            source: protocol::ContextTokenSource::Estimated,
        }),
    }
}

fn worker_error_requires_attention(error: &WorkerError) -> bool {
    match error {
        WorkerError::SegmentActivationIncomplete { .. }
        | WorkerError::CompactionCleanupPending { .. } => true,
        WorkerError::RunFailureAlreadyDelivered { source }
        | WorkerError::RunTerminalPersistence {
            run_failure: source,
            ..
        } => worker_error_requires_attention(source),
        _ => false,
    }
}

fn worker_error_code(e: &WorkerError) -> ErrorCode {
    match e {
        WorkerError::RunFailureAlreadyDelivered { source } => worker_error_code(source),
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
#[path = "controller_subject_release_tests.rs"]
mod subject_release_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::WorkerEvent;
    use std::time::Duration;
    use tempfile::TempDir;

    mod engine_alerts {
        use super::*;
        use agen::llm_client::event::Event as LlmEvent;
        use agen::llm_client::{ClientError, Item, Request, ToolCallCompletionSupport};
        use agen::tool::{
            Tool, ToolError, ToolExecutionContext, ToolMeta, ToolOutput, ToolOutputLimits,
        };
        use agen::{Engine, EngineRunExit, History, ToolCallDispatchMode};
        use std::collections::VecDeque;
        use std::pin::Pin;
        use std::sync::Mutex;

        #[derive(Clone)]
        struct ScriptedClient {
            responses: Arc<Mutex<VecDeque<Vec<LlmEvent>>>>,
            requests: Arc<Mutex<Vec<Request>>>,
        }

        impl ScriptedClient {
            fn new(responses: Vec<Vec<LlmEvent>>) -> Self {
                Self {
                    responses: Arc::new(Mutex::new(responses.into())),
                    requests: Arc::new(Mutex::new(Vec::new())),
                }
            }
        }

        #[async_trait::async_trait]
        impl LlmClient for ScriptedClient {
            fn clone_boxed(&self) -> Box<dyn LlmClient> {
                Box::new(self.clone())
            }

            fn tool_call_completion_support(&self) -> ToolCallCompletionSupport {
                ToolCallCompletionSupport::ResponseComplete
            }

            async fn stream(
                &self,
                request: Request,
            ) -> Result<
                Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, ClientError>> + Send>>,
                ClientError,
            > {
                self.requests.lock().unwrap().push(request);
                let events = self
                    .responses
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("unexpected LLM request");
                Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
            }
        }

        // Component connection: real Worker/Store, initialized endpoint, paired
        // protocol subscription and Controller loop. The model boundary is an
        // empty non-network script; this test must not request any generation.
        #[tokio::test]
        async fn initialized_controller_publishes_the_same_endpoint_and_subscription() {
            let root = TempDir::new().unwrap();
            let manifest = manifest::WorkerManifest::from_toml(
                "[worker]\nname = 'p'\n[model]\nscheme = 'anthropic'\nmodel_id = 'test'\n[engine]\n[scope]\n",
            )
            .unwrap();
            let client = ScriptedClient::new(vec![]);
            let requests = client.requests.clone();
            let engine =
                Engine::<_, agen::state::Mutable, crate::SessionHistoryMetadata>::new_annotated(
                    client,
                );
            let store = session_store::CombinedStore::new(
                session_store::FsStore::new(root.path().join("sessions")).unwrap(),
                session_store::FsWorkerStore::new(root.path().join("workers")).unwrap(),
            );
            let mut worker = Worker::new(
                manifest,
                engine,
                store,
                crate::worker::WorkerWorkspaceContext::local_filesystem(None),
                crate::worker::WorkerFilesystemAuthority::None,
                manifest::Scope::empty(),
            )
            .await
            .unwrap();
            worker.set_system_prompt_template(
                crate::SystemPromptTemplate::parse(
                    "default",
                    crate::PromptCatalogSource::builtins_only(),
                )
                .unwrap(),
            );
            let prepared = WorkerController::initialize(worker, root.path(), None)
                .await
                .unwrap();
            let endpoint = prepared.handle.clone();
            let mut streams =
                crate::ipc::protocol_session::subscribe_worker_protocol_session(&endpoint);
            let entries = endpoint.committed_entries();
            assert!(matches!(
                &streams.snapshot_event,
                Event::Snapshot { greeting, state, session, .. }
                    if !greeting.tools.is_empty()
                        && state.state == WorkerState::Idle
                        && session == &session_store::public_snapshot::project_current_session_snapshot(&entries)
            ));
            assert!(!endpoint.protocol_is_closed());
            assert!(!endpoint.runtime_dir.path().join("sock").exists());
            assert!(!endpoint.runtime_dir.path().join("status.json").exists());
            assert!(!endpoint.runtime_dir.path().join("manifest.toml").exists());
            assert!(matches!(
                streams.events.try_recv(),
                Err(broadcast::error::TryRecvError::Empty)
            ));
            assert!(requests.lock().unwrap().is_empty());

            let (handle, shutdown, task) = prepared.publish().await.unwrap();
            assert!(handle.same_controller(&endpoint));
            assert!(!handle.runtime_dir.path().join("sock").exists());
            assert!(!handle.runtime_dir.path().join("status.json").exists());
            assert!(!handle.runtime_dir.path().join("manifest.toml").exists());
            handle.send(Method::ListPendingSubmissions).await.unwrap();
            let pending = tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if let Event::PendingSubmissionsChanged { pending } =
                        streams.events.recv().await.unwrap()
                    {
                        break pending;
                    }
                }
            })
            .await
            .unwrap();
            assert_eq!(pending, protocol::PendingSubmissionsSnapshot::default());
            assert_eq!(
                serde_json::to_value(handle.committed_entries()).unwrap(),
                serde_json::to_value(entries).unwrap(),
            );
            handle
                .send(Method::Shutdown {
                    command: WorkerCommandEnvelope { command_id: 1 },
                })
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_secs(5), shutdown)
                .await
                .unwrap()
                .unwrap();
            tokio::time::timeout(Duration::from_secs(5), task)
                .await
                .unwrap()
                .unwrap();
            assert!(endpoint.protocol_is_closed());
            assert!(!endpoint.runtime_dir.path().join("sock").exists());
            assert!(requests.lock().unwrap().is_empty());
        }

        #[tokio::test]
        async fn tool_initialization_failure_does_not_reclaim_saved_children() {
            let root = TempDir::new().unwrap();
            let mut manifest = manifest::WorkerManifest::from_toml(
                "[worker]\nname = 'p'\n[model]\nscheme = 'anthropic'\nmodel_id = 'test'\n[engine]\n[scope]\n",
            ).unwrap();
            manifest.feature.ticket.enabled = true;
            let client = ScriptedClient::new(vec![]);
            let requests = client.requests.clone();
            let store = session_store::CombinedStore::new(
                session_store::FsStore::new(root.path().join("sessions")).unwrap(),
                session_store::FsWorkerStore::new(root.path().join("workers")).unwrap(),
            );
            // New feature activation is refused by the real unavailable host;
            // legacy child metadata must not be consumed before that refusal.
            let worker = Worker::new(
                manifest,
                Engine::<_, agen::state::Mutable, crate::SessionHistoryMetadata>::new_annotated(
                    client,
                ),
                store.clone(),
                crate::worker::WorkerWorkspaceContext::no_workspace(),
                crate::worker::WorkerFilesystemAuthority::None,
                manifest::Scope::empty(),
            )
            .await
            .unwrap();
            let mut metadata = session_store::WorkerMetadata::new(
                "p",
                Some(session_store::WorkerActiveSegmentRef {
                    session_id: worker.session_id(),
                    segment_id: Some(worker.segment_id()),
                }),
            );
            metadata
                .spawned_children
                .push(session_store::WorkerSpawnedChild {
                    worker_name: "legacy".into(),
                    socket_path: root.path().join("legacy.sock"),
                    callback_address: root.path().join("p/sock"),
                    scope_delegated: Vec::new(),
                });
            store.write(&metadata).unwrap();
            let before = store.read_by_name("p").unwrap();
            let result = WorkerController::initialize(worker, root.path(), None).await;
            assert!(result.is_err());
            assert_eq!(store.read_by_name("p").unwrap(), before);
            assert!(!root.path().join("p/spawned_workers.json").exists());
            assert!(requests.lock().unwrap().is_empty());
        }

        #[derive(Debug)]
        struct DriveWorkspaceHost {
            available: bool,
            workspace: Option<&'static str>,
            granted: bool,
            operations: Arc<std::sync::Mutex<Vec<String>>>,
        }

        impl crate::worker::WorkspaceClient for DriveWorkspaceHost {
            fn workspace_id(&self) -> Option<&str> {
                self.workspace
            }
            fn kind(&self) -> &str {
                "drive-test"
            }
            fn is_available(&self) -> bool {
                self.available
            }
            fn execute(
                &self,
                request: crate::worker::WorkspaceRequest,
            ) -> Result<crate::worker::WorkspaceResponse, crate::worker::WorkspaceClientError>
            {
                if request.path == "/api/w/workspace-1/authorization/check" {
                    assert_eq!(request.method, crate::worker::WorkspaceRequestMethod::Post);
                    let input: server_api::authorization::OperationAuthorizationRequest =
                        serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
                    assert!(matches!(
                        input.operation.as_str(),
                        "drive.capabilities" | "drive.root"
                    ));
                    assert_eq!(
                        input.resource,
                        server_api::authorization::PermissionResource::Workspace {
                            workspace_id: "workspace-1".into()
                        }
                    );
                    self.operations.lock().unwrap().push(input.operation);
                    return Ok(crate::worker::WorkspaceResponse {
                        status: if self.granted { 200 } else { 403 },
                        body: serde_json::json!({"scope":input.resource}).to_string(),
                    });
                }
                self.operations.lock().unwrap().push(request.path.clone());
                // Native discovery reads metadata and the current caller's ceiling, never inventory or mutations.
                assert_eq!(request.method, crate::worker::WorkspaceRequestMethod::Get);
                if request.path == "/api/w/workspace-1/drive/capabilities" {
                    return Ok(crate::worker::WorkspaceResponse {
                        status: 200,
                        body: serde_json::json!({"write":true}).to_string(),
                    });
                }
                assert_eq!(request.path, "/api/w/workspace-1/drive/root");
                Ok(crate::worker::WorkspaceResponse { status: 200, body: serde_json::json!({
                    "entry": {"workspace_id": "workspace-1", "node_id": "1"},
                    "parent": null, "name": "", "kind": "folder", "last_mutation_id": "",
                    "size": null, "content_type": null, "updated_by": "test",
                    "updated_at": "2026-10-08T00:00:00Z",
                    "latest_url": "/api/w/workspace-1/drive/download?entry_workspace_id=workspace-1&id=1"
                }).to_string() })
            }
        }

        #[tokio::test]
        async fn scoped_observation_is_composed_only_with_runtime_worker_service_without_startup_access()
         {
            for enabled in [false, true] {
                let root = TempDir::new().unwrap();
                let mut manifest = manifest::WorkerManifest::from_toml(
                    "[worker]\nname = 'observation-wiring'\n[model]\nscheme = 'anthropic'\nmodel_id = 'test'\n[engine]\n[scope]\n"
                ).unwrap();
                manifest.feature.worker.enabled = enabled;
                manifest.feature.sub_worker.enabled = false;
                manifest.feature.workdir_catalog.enabled = false;
                let model = ScriptedClient::new(vec![]);
                let engine =
                    Engine::<_, agen::state::Mutable, crate::SessionHistoryMetadata>::new_annotated(
                        model,
                    );
                let store = session_store::CombinedStore::new(
                    session_store::FsStore::new(root.path().join("sessions")).unwrap(),
                    session_store::FsWorkerStore::new(root.path().join("workers")).unwrap(),
                );
                let operations = Arc::new(std::sync::Mutex::new(Vec::new()));
                let host = Arc::new(DriveWorkspaceHost {
                    available: true,
                    workspace: Some("workspace-1"),
                    granted: false,
                    operations: operations.clone(),
                });
                let mut worker = Worker::new(
                    manifest,
                    engine,
                    store,
                    crate::worker::WorkerWorkspaceContext::with_client(
                        Some(crate::worker::WorkspaceId::new("workspace-1").unwrap()),
                        host,
                    ),
                    crate::worker::WorkerFilesystemAuthority::None,
                    manifest::Scope::empty(),
                )
                .await
                .unwrap();
                worker.set_system_prompt_template(
                    crate::SystemPromptTemplate::parse(
                        "default",
                        crate::PromptCatalogSource::builtins_only(),
                    )
                    .unwrap(),
                );
                let prepared = WorkerController::initialize(worker, root.path(), None)
                    .await
                    .unwrap();
                let Event::Snapshot { greeting, .. } =
                    crate::ipc::protocol_session::subscribe_worker_protocol_session(
                        &prepared.handle,
                    )
                    .snapshot_event
                else {
                    panic!("initial committed greeting");
                };
                for name in [
                    "ViewSessionOverview",
                    "SearchSessionEntries",
                    "ReadSessionEntry",
                ] {
                    assert_eq!(
                        greeting.tools.iter().any(|tool| tool == name),
                        enabled,
                        "Worker service configuration must determine observation publication, not client availability alone"
                    );
                }
                assert!(
                    operations.lock().unwrap().is_empty(),
                    "provider composition may not authorize or read protected resources"
                );
            }
        }

        #[tokio::test]
        async fn drive_controller_registers_without_authorization_and_checks_each_access_in_both_modes()
         {
            use crate::feature::FeatureModule as _;
            for mode in [manifest::WorkerMode::Tools, manifest::WorkerMode::Wip] {
                for (enabled, available, workspace, granted) in [
                    (false, true, Some("workspace-1"), false),
                    (true, false, Some("workspace-1"), false),
                    (true, true, None, false),
                    (true, true, Some("workspace-1"), false),
                    (true, true, Some("workspace-1"), true),
                ] {
                    let root = TempDir::new().unwrap();
                    let mut manifest = manifest::WorkerManifest::from_toml(
                        "[worker]\nname = 'drive-wiring'\n[model]\nscheme = 'anthropic'\nmodel_id = 'test'\n[engine]\n[scope]\n"
                    ).unwrap();
                    manifest.worker.mode = mode;
                    manifest.feature.drive.enabled = enabled;
                    // Isolate Drive discovery from the independent Workspace catalog.
                    manifest.feature.workdir_catalog.enabled = false;
                    let client = ScriptedClient::new(vec![finished_response()]);
                    let requests = client.requests.clone();
                    let engine = Engine::<_, agen::state::Mutable, crate::SessionHistoryMetadata>::new_annotated(client);
                    let store = session_store::CombinedStore::new(
                        session_store::FsStore::new(root.path().join("sessions")).unwrap(),
                        session_store::FsWorkerStore::new(root.path().join("workers")).unwrap(),
                    );
                    let operations = Arc::new(std::sync::Mutex::new(Vec::new()));
                    let host: Arc<dyn crate::worker::WorkspaceClient> =
                        Arc::new(DriveWorkspaceHost {
                            available,
                            workspace,
                            granted,
                            operations: operations.clone(),
                        });
                    let context = crate::worker::WorkerWorkspaceContext::with_client(
                        workspace.map(|id| crate::worker::WorkspaceId::new(id).unwrap()),
                        host.clone(),
                    );
                    let mut worker = Worker::new(
                        manifest,
                        engine,
                        store,
                        context,
                        crate::worker::WorkerFilesystemAuthority::local(
                            root.path().into(),
                            root.path().into(),
                        ),
                        manifest::Scope::writable(root.path()).unwrap(),
                    )
                    .await
                    .unwrap();
                    worker.set_system_prompt_template(
                        crate::SystemPromptTemplate::parse(
                            "default",
                            crate::PromptCatalogSource::builtins_only(),
                        )
                        .unwrap(),
                    );
                    let descriptor = crate::feature::builtin::drive::DriveFeature::new(
                        host,
                        worker.workdir_sessions(),
                    )
                    .descriptor();
                    let registered = register_worker_tools(
                        &mut worker,
                        root.path().join("runtime"),
                        SpawnedWorkerRegistry::new_for_internal_services(),
                        None,
                        None,
                    )
                    .await;
                    if enabled && (!available || workspace.is_none()) {
                        assert_eq!(
                            registered.unwrap_err().kind(),
                            std::io::ErrorKind::NotConnected
                        );
                        assert!(operations.lock().unwrap().is_empty());
                        assert!(requests.lock().unwrap().is_empty());
                        continue;
                    }
                    registered.unwrap();
                    assert!(
                        operations.lock().unwrap().is_empty(),
                        "registration must not access Drive or authorize activation"
                    );
                    let configured = enabled && available && workspace.is_some();
                    let tools = worker.engine().tool_server_handle();
                    tools.flush_pending();
                    for tool in &descriptor.tools {
                        assert_eq!(
                            tools.get_tool(&tool.name).is_some(),
                            configured && mode == manifest::WorkerMode::Tools,
                            "{mode:?}, enabled={enabled}, available={available}, workspace={workspace:?}, tool={}",
                            tool.name
                        );
                    }
                    if configured && mode == manifest::WorkerMode::Tools {
                        let (_, root_tool) = tools.get_tool("DriveRoot").unwrap();
                        let output = root_tool
                            .execute("{}", ToolExecutionContext::direct())
                            .await;
                        assert_eq!(output.is_ok(), granted, "{output:?}");
                    }
                    if mode == manifest::WorkerMode::Wip {
                        let (_, tree) = tools.get_tool("Tree").unwrap();
                        let output = tree
                            .execute(
                                r#"{"path":"/drive","depth":1}"#,
                                ToolExecutionContext::new("drive-wiring-tree", "batch", 0),
                            )
                            .await;
                        assert_eq!(output.is_ok(), configured && granted, "{output:?}");
                        if let Ok(output) = output {
                            let tree: serde_json::Value =
                                serde_json::from_str(&output.content.unwrap()).unwrap();
                            assert_eq!(tree["path"], "/drive");
                            // A self-scoped entrance is an enumeration boundary,
                            // not an observed empty Workspace hierarchy.
                            assert_eq!(tree["children"], serde_json::Value::Null);
                        }
                    }
                    if configured && !granted {
                        assert!(
                            operations
                                .lock()
                                .unwrap()
                                .iter()
                                .all(|operation| !operation.starts_with('/')),
                            "denied access must not reach Drive provider"
                        );
                    }
                    worker.run_text("Inspect enabled features").await.unwrap();
                    let requests = requests.lock().unwrap();
                    let prompt = requests[0].system_prompt.as_deref().unwrap_or("");
                    assert_eq!(
                        prompt.contains("## Workspace Drive"),
                        configured,
                        "{mode:?}, enabled={enabled}, available={available}, workspace={workspace:?}"
                    );
                }
            }
        }

        include!("controller/native_record_tests.rs");

        struct LargeOutputTool;

        #[async_trait::async_trait]
        impl Tool for LargeOutputTool {
            async fn execute(
                &self,
                _input_json: &str,
                _ctx: ToolExecutionContext,
            ) -> Result<ToolOutput, ToolError> {
                Ok(ToolOutput {
                    summary: "Fixed large output".into(),
                    content: Some("useful output line\n".repeat(512)),
                    attachments: Vec::new(),
                })
            }
        }

        fn finished_response() -> Vec<LlmEvent> {
            vec![
                LlmEvent::text_block_start(0),
                LlmEvent::text_delta(0, "Done"),
                LlmEvent::text_block_stop(0, None),
            ]
        }

        #[tokio::test]
        async fn truncated_tool_output_preserves_model_result_without_live_or_buffered_alerts() {
            let client = ScriptedClient::new(vec![
                vec![
                    LlmEvent::tool_use_start(0, "large-call", "large_output"),
                    LlmEvent::tool_input_delta(0, "{}"),
                    LlmEvent::tool_use_stop(0),
                ],
                finished_response(),
            ]);
            let requests = client.requests.clone();
            let mut engine = Engine::new(client);
            // Isolate output limiting from the independent early-dispatch warning.
            engine.set_tool_call_dispatch_mode(ToolCallDispatchMode::AfterResponse);
            engine.set_tool_output_limits(Some(ToolOutputLimits {
                default_max_bytes: 256,
                per_tool: Default::default(),
            }));
            engine.register_tool(Arc::new(|| {
                (
                    ToolMeta::new("large_output")
                        .input_schema(serde_json::json!({"type": "object"})),
                    Arc::new(LargeOutputTool),
                )
            }));
            let (tx, _) = broadcast::channel(8);
            let alerter = Alerter::new(tx);
            let (_, mut live) = alerter.subscribe_with_snapshot();
            wire_engine_warnings(&mut engine, &alerter);

            let output = engine.run(&mut History::new(), "Run the tool").await;
            assert!(
                matches!(output.result, EngineRunExit::Finished),
                "{:?}",
                output.result
            );
            let requests = requests.lock().unwrap();
            assert_eq!(requests.len(), 2);
            let result = requests[1].items.iter().find(|item| {
                matches!(item, Item::ToolResult { call_id, .. } if call_id == "large-call")
            }).expect("the next model request must include the bounded tool result");
            match result {
                Item::ToolResult {
                    summary,
                    content: Some(content),
                    is_error,
                    disposition,
                    ..
                } => {
                    assert_eq!(summary, "Fixed large output");
                    assert!(!is_error);
                    assert_eq!(*disposition, agen::ToolResultDisposition::Success);
                    assert!(content.len() <= 256, "{content}");
                    assert!(content.starts_with("useful output line\n"), "{content}");
                    assert!(content.contains("[truncated:"), "{content}");
                    assert!(content.contains("refine your query]"), "{content}");
                }
                other => panic!("unexpected model-visible result: {other:?}"),
            }
            assert!(matches!(
                live.try_recv(),
                Err(broadcast::error::TryRecvError::Empty)
            ));
            let (snapshot, _) = alerter.subscribe_with_snapshot();
            assert!(snapshot.is_empty(), "unexpected alerts: {snapshot:?}");
        }

        #[tokio::test]
        async fn unsupported_early_dispatch_warning_reaches_live_and_buffered_engine_alerts() {
            let mut engine = Engine::new(ScriptedClient::new(vec![finished_response()]));
            engine.set_tool_call_dispatch_mode(ToolCallDispatchMode::OnToolCallComplete);
            let (tx, _) = broadcast::channel(8);
            let alerter = Alerter::new(tx);
            let (_, mut live) = alerter.subscribe_with_snapshot();
            wire_engine_warnings(&mut engine, &alerter);

            let output = engine.run(&mut History::new(), "Finish").await;
            assert!(
                matches!(output.result, EngineRunExit::Finished),
                "{:?}",
                output.result
            );
            let alert = match live.try_recv() {
                Ok(Event::Alert(alert)) => alert,
                other => panic!("expected live Engine warning: {other:?}"),
            };
            assert_eq!(alert.level, AlertLevel::Warn);
            assert_eq!(alert.source, AlertSource::Engine);
            assert!(
                alert.message.contains(
                    "provider does not expose a trustworthy per-call completion boundary"
                ),
                "{}",
                alert.message
            );
            assert!(matches!(
                live.try_recv(),
                Err(broadcast::error::TryRecvError::Empty)
            ));
            let (snapshot, mut late_live) = alerter.subscribe_with_snapshot();
            assert_eq!(snapshot.len(), 1, "unexpected alerts: {snapshot:?}");
            assert_eq!(snapshot[0].level, alert.level);
            assert_eq!(snapshot[0].source, alert.source);
            assert_eq!(snapshot[0].message, alert.message);
            assert_eq!(snapshot[0].timestamp_ms, alert.timestamp_ms);
            assert!(matches!(
                late_live.try_recv(),
                Err(broadcast::error::TryRecvError::Empty)
            ));
        }
    }

    #[test]
    fn fresh_session_head_is_materialized_after_feature_installation_before_exposure() {
        let production = include_str!("controller.rs")
            .split_once("#[cfg(test)]\nmod tests")
            .map(|(production, _)| production)
            .expect("controller test module marker");
        let startup = production
            .split_once("async fn spawn_initialized")
            .map(|(_, startup)| startup)
            .expect("controller startup implementation");
        let feature_install = startup
            .find("let fs_for_view = register_worker_tools(")
            .expect("feature installation call");
        let materialize = startup
            .find(".materialize_durable_session_head()")
            .expect("runtime session-head materialization");
        let exposure = startup
            .find("let handle = WorkerHandle")
            .expect("controller handle exposure");
        let materialize_gate = startup[..materialize]
            .rfind("if worker.needs_initial_session_head_materialization() {")
            .expect("fresh materialization gate");

        assert!(feature_install < materialize_gate);
        assert!(materialize_gate < materialize);
        assert!(materialize < exposure);
        let observer_preparation = startup
            .find("PreparedWorkdirCommandEvents::new(session")
            .unwrap();
        let recover = startup.find(".recover_unfinished_compaction()").unwrap();
        let reclaim = startup
            .find("self.registry_reclaim.apply().await?")
            .unwrap();
        let observer_start = startup
            .find(".and_then(PreparedWorkdirCommandEvents::start)")
            .unwrap();
        let loop_start = startup.find("tokio::spawn(controller_loop(").unwrap();
        assert!(recover < observer_preparation && observer_preparation < reclaim);
        assert!(reclaim < observer_start && observer_start < loop_start);
        assert!(
            startup[materialize_gate..materialize]
                .contains("needs_initial_session_head_materialization")
        );
    }

    #[test]
    fn public_reasoning_config_preserves_effort_and_signed_budget() {
        let effort =
            public_reasoning_config(&agen::llm_client::capability::ReasoningControl::Effort(
                agen::llm_client::capability::ReasoningEffort::Other("provider-native".into()),
            ));
        assert_eq!(
            effort,
            protocol::ReasoningConfig::Effort {
                effort: "provider-native".into(),
            }
        );

        let budget = public_reasoning_config(
            &agen::llm_client::capability::ReasoningControl::BudgetTokens(-1),
        );
        assert_eq!(
            budget,
            protocol::ReasoningConfig::BudgetTokens { budget_tokens: -1 }
        );
    }

    #[test]
    fn context_usage_publication_updates_reconnect_state_and_live_clients() {
        let shared_state = Arc::new(WorkerSharedState::new(
            "worker".into(),
            session_store::new_segment_id(),
            String::new(),
            protocol::Greeting {
                worker_name: "worker".into(),
                cwd: "/tmp".into(),
                provider: "provider".into(),
                model: "model".into(),
                reasoning: None,
                scope_summary: String::new(),
                tools: Vec::new(),
                context_window: 100_000,
                context_tokens: 0,
                context_usage: None,
            },
        ));
        let (events, mut receiver) = broadcast::channel(4);

        publish_context_usage(
            &shared_state,
            &events,
            Some(25_000),
            protocol::ContextTokenSource::Estimated,
        );

        let expected = protocol::ContextUsage {
            tokens: 25_000,
            source: protocol::ContextTokenSource::Estimated,
        };
        assert_eq!(shared_state.greeting().context_usage, Some(expected));
        assert!(matches!(
            receiver.try_recv(),
            Ok(Event::ContextUsage {
                usage: Some(usage)
            }) if usage == expected
        ));
    }

    #[test]
    fn legacy_and_subjektiv_lifecycles_are_mutually_exclusive() {
        let mut memory = manifest::MemoryFeatureProfileConfig::default();
        memory.enabled = true;
        memory.extraction.enabled = true;
        let mut subjektiv = manifest::SubjektivFeatureProfileConfig::default();
        subjektiv.enabled = true;
        subjektiv.extraction.enabled = true;

        let error = validate_memory_lifecycle_targets(&memory, &subjektiv).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("cannot both be enabled"));

        memory.extraction.enabled = false;
        assert!(
            validate_memory_lifecycle_targets(&memory, &subjektiv).is_err(),
            "legacy consolidation still installs the legacy lifecycle"
        );
        memory.consolidation.request_enabled = false;
        validate_memory_lifecycle_targets(&memory, &subjektiv).unwrap();
    }

    #[test]
    fn no_controller_parent_notification_uses_durable_pending_authority() {
        let temp = TempDir::new().unwrap();
        let pending =
            crate::worker::PendingSubmissionHandle::for_test(&temp.path().join("sessions"));
        let notify_buffer = NotifyBuffer::new();
        let target = durable_parent_notification_target(pending.clone(), notify_buffer.clone());
        let (wire_namespace, wire_provenance) =
            resolved_input_source(&pending, &protocol::AuthenticatedInputSource::UntrustedWire);
        assert_eq!(wire_namespace, pending.direct_client_namespace());
        assert!(matches!(
            wire_provenance,
            session_store::LoggedSessionHistoryOrigin::LegacyUnknown
        ));

        target.notify("child-session".into(), "completed".into());

        let snapshot = pending.snapshot();
        assert_eq!(snapshot.notification_count, 0);
        assert!(matches!(
            prepare_pending_run(&pending, &notify_buffer, None, true).unwrap(),
            Some(PendingRun::RunForNotification { .. })
        ));
        assert!(notify_buffer.has_notification_pending());
    }

    #[test]
    fn invocation_fence_suspends_expired_notification_deadline_without_dropping_input() {
        let temp = TempDir::new().unwrap();
        let pending = crate::worker::PendingSubmissionHandle::for_test(temp.path());
        let buffer = NotifyBuffer::new();
        pending
            .accept_notification("held".into(), "held".into())
            .unwrap();
        let before = pending.persisted_entries_for_test().len();
        assert_eq!(
            notification_coalesce_remaining(&pending, &buffer, Duration::ZERO, true),
            Some(Duration::ZERO)
        );
        for _ in 0..4 {
            assert_eq!(
                notification_coalesce_remaining(&pending, &buffer, Duration::ZERO, false),
                None
            );
        }
        assert_eq!(pending.persisted_entries_for_test().len(), before);
        assert_eq!(pending.snapshot().notification_count, 1);
        // A busy-time staged notification is equally retained behind the fence.
        stage_pending_notifications(&pending, &buffer);
        assert_eq!(
            notification_coalesce_remaining(&pending, &buffer, Duration::ZERO, false),
            None
        );
        assert!(buffer.has_notification_pending());
        assert_eq!(
            notification_coalesce_remaining(&pending, &buffer, Duration::ZERO, true),
            Some(Duration::ZERO)
        );
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
            .accept_notification("notify-second".into(), "newer notification".into())
            .unwrap();
        let notify_buffer = NotifyBuffer::new();

        assert!(matches!(
            prepare_pending_run(&pending, &notify_buffer, None, true).unwrap(),
            Some(PendingRun::Submit(_))
        ));

        let pending =
            crate::worker::PendingSubmissionHandle::for_test(&temp.path().join("notify-first"));
        pending
            .accept_notification("notify-first".into(), "older notification".into())
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
            prepare_pending_run(&pending, &notify_buffer, None, true).unwrap(),
            Some(PendingRun::Submit(_))
        ));
        assert_eq!(notify_buffer.len(), 1);
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
        _temp: TempDir,
    }

    async fn make_env() -> DriveTurnEnv {
        let temp = tempfile::tempdir().expect("tempdir");
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
                reasoning: None,
                context_usage: None,
            },
        ));
        let notify_buffer = NotifyBuffer::new();
        let pending_submissions =
            crate::worker::PendingSubmissionHandle::for_test(&temp.path().join("pending-sessions"));

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
            _temp: temp,
        }
    }

    #[tokio::test]
    async fn idle_only_input_is_rejected_in_busy_maintenance_admission() {
        let env = make_env().await;
        let mut events = env.working_event_tx.subscribe();
        let method = Method::SubmitIfIdle {
            submission_request_id: "maintenance-input".to_string(),
            input: vec![protocol::Segment::text("must not queue")],
            source: protocol::AuthenticatedInputSource::Backend {
                operation_id: "maintenance-input".to_string(),
            },
        };
        assert!(
            durably_accept_method_while_busy(
                method,
                &env.pending_submissions,
                &env.working_event_tx
            )
            .is_none()
        );
        let event = events.try_recv().unwrap();
        assert!(
            matches!(event, Event::SubmissionRejected { submission_request_id, message }
            if submission_request_id == "maintenance-input" && message.contains("WorkerNotify"))
        );
        assert!(
            events.try_recv().is_err(),
            "must not emit durable acceptance or queue changes"
        );
    }

    #[test]
    fn busy_submit_is_durable_and_idempotent_before_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        let pending = PendingSubmissionHandle::for_test(dir.path());
        let (event_tx, mut event_rx) = broadcast::channel(8);
        for method in [
            Method::submit_text("direct-request", "next direct turn"),
            Method::SubmitTracked {
                submission_request_id: "tracked-request".into(),
                input: vec![protocol::Segment::text("next tracked turn")],
                source: protocol::AuthenticatedInputSource::Backend {
                    operation_id: "operation-1".into(),
                },
            },
        ] {
            for _ in 0..2 {
                assert!(
                    durably_accept_method_while_busy(method.clone(), &pending, &event_tx).is_none()
                );
                assert!(matches!(
                    event_rx.try_recv().unwrap(),
                    Event::SubmissionAccepted {
                        disposition: protocol::SubmissionDisposition::Queued,
                        ..
                    }
                ));
            }
        }
        assert_eq!(pending.snapshot().submissions.len(), 2);
        let entries = pending.persisted_entries_for_test();
        assert_eq!(
            entries
                .iter()
                .filter(|entry| matches!(entry,
                    LogEntry::Extension { domain, .. } if domain == "worker.pending_activations.v1"
                ))
                .count(),
            2
        );
    }

    #[test]
    fn busy_notification_receipt_follows_durable_idempotent_acceptance() {
        let dir = tempfile::tempdir().unwrap();
        let pending = PendingSubmissionHandle::for_test(dir.path());
        let (event_tx, mut event_rx) = broadcast::channel(4);
        let method = Method::NotifyTracked {
            receipt_scope: protocol::NotificationReceiptScope::Source,
            notification_request_id: "completion-1".into(),
            message: "subworker completed".into(),
            source: protocol::AuthenticatedInputSource::UntrustedWire,
        };

        for _ in 0..2 {
            assert!(
                durably_accept_method_while_busy(method.clone(), &pending, &event_tx).is_none()
            );
            let entries = pending.persisted_entries_for_test();
            assert!(entries.iter().any(|entry| matches!(
                entry,
                LogEntry::Extension { domain, .. }
                    if domain == "worker.pending_activations.v1"
            )));
            assert!(matches!(
                event_rx.try_recv().unwrap(),
                Event::NotificationAccepted {
                    notification_request_id,
                } if notification_request_id == "completion-1"
            ));
        }
        assert_eq!(pending.snapshot().notification_count, 1);
        assert_eq!(
            pending
                .persisted_entries_for_test()
                .iter()
                .filter(|entry| matches!(
                    entry,
                    LogEntry::Extension { domain, .. }
                        if domain == "worker.pending_activations.v1"
                ))
                .count(),
            1
        );
    }

    #[test]
    fn attention_gate_rejects_submit_before_dispatch() {
        let (event_tx, mut event_rx) = broadcast::channel(4);
        let method = Method::Submit {
            submission_request_id: "request-1".into(),
            input: Vec::new(),
        };

        assert!(reject_method_while_attention_locked(
            &method,
            &event_tx,
            "repair required",
        ));
        assert!(matches!(
            event_rx.try_recv().unwrap(),
            Event::SubmissionRejected {
                submission_request_id,
                message,
            } if submission_request_id == "request-1" && message == "repair required"
        ));
    }

    #[tokio::test]
    async fn segment_activation_failure_from_run_never_publishes_idle() {
        let mut env = make_env().await;
        let worker_future = async {
            Err::<WorkerRunResult, _>(WorkerError::SegmentActivationIncomplete {
                source: crate::runtime::worker_allocation::ScopeLockError::UnknownWorker(
                    "worker".into(),
                ),
            })
        };

        let (status, shutdown, may_drain_pending) = drive_turn(
            worker_future,
            &mut env.method_rx,
            &env.working_event_tx,
            &env.cancel_tx,
            &env.pause_tx,
            &env.shared_state,
            None,
            &env.notify_buffer,
            &env.pending_submissions,
        )
        .await;

        assert_eq!(status, WorkerStatus::Running);
        assert!(!shutdown);
        assert!(!may_drain_pending);
    }

    #[tokio::test]
    async fn pending_compaction_cleanup_preserves_shutdown_request_for_outer_barrier() {
        let mut env = make_env().await;
        let method_tx = env._method_tx.clone();
        env.shared_state
            .transition(WorkerState::Busy(WorkerBusyState::Run(
                WorkerRunState::Running,
            )));
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            method_tx
                .send(Method::Shutdown {
                    command: WorkerCommandEnvelope::new(1),
                })
                .await
                .expect("send shutdown");
        });
        let worker_future = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Err::<WorkerRunResult, _>(WorkerError::CompactionCleanupPending {
                source: crate::runtime::worker_allocation::ScopeLockError::UnknownWorker(
                    "worker".into(),
                ),
            })
        };

        let (status, shutdown, may_drain_pending) = drive_turn(
            worker_future,
            &mut env.method_rx,
            &env.working_event_tx,
            &env.cancel_tx,
            &env.pause_tx,
            &env.shared_state,
            None,
            &env.notify_buffer,
            &env.pending_submissions,
        )
        .await;

        assert_eq!(status, WorkerStatus::Running);
        assert!(shutdown);
        assert!(!may_drain_pending);
    }

    #[tokio::test]
    async fn pause_waits_for_run_boundary_and_uses_safe_pause_channel() {
        let mut env = make_env().await;
        let method_tx = env._method_tx.clone();
        env.shared_state
            .transition(WorkerState::Busy(WorkerBusyState::Run(
                WorkerRunState::Running,
            )));
        let command = WorkerCommandEnvelope::new(1);
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
            None,
            &env.notify_buffer,
            &env.pending_submissions,
        )
        .await;

        assert_eq!(status, WorkerStatus::Paused);
        assert!(!shutdown);
        assert!(started_at.elapsed() >= Duration::from_millis(100));
        assert!(env._pause_rx.try_recv().is_ok());
        assert!(env._cancel_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn delivered_run_failure_is_not_reported_again_by_controller() {
        let mut env = make_env().await;
        let mut events = env.working_event_tx.subscribe();
        let worker_future = async {
            Err::<WorkerRunResult, _>(WorkerError::RunFailureAlreadyDelivered {
                source: Box::new(WorkerError::CompactSummaryMissing),
            })
        };

        let (status, _, may_drain_pending) = drive_turn(
            worker_future,
            &mut env.method_rx,
            &env.working_event_tx,
            &env.cancel_tx,
            &env.pause_tx,
            &env.shared_state,
            None,
            &env.notify_buffer,
            &env.pending_submissions,
        )
        .await;

        assert_eq!(status, WorkerStatus::Idle);
        assert!(!may_drain_pending, "failed runs must not become successful");
        assert!(events.try_recv().is_err(), "must not emit a generic Error");
    }

    #[tokio::test]
    async fn terminal_persistence_failure_reports_both_causes_once() {
        let mut env = make_env().await;
        let mut events = env.working_event_tx.subscribe();
        let worker_future = async {
            Err::<WorkerRunResult, _>(WorkerError::RunTerminalPersistence {
                run_failure: Box::new(WorkerError::CompactSummaryMissing),
                persist_error: Box::new(WorkerError::Store(session_store::StoreError::Io(
                    std::io::Error::other("terminal append failed"),
                ))),
            })
        };

        let (status, _, may_drain_pending) = drive_turn(
            worker_future,
            &mut env.method_rx,
            &env.working_event_tx,
            &env.cancel_tx,
            &env.pause_tx,
            &env.shared_state,
            None,
            &env.notify_buffer,
            &env.pending_submissions,
        )
        .await;

        assert_eq!(status, WorkerStatus::Idle);
        assert!(!may_drain_pending);
        let Event::Error { message, .. } = events.try_recv().expect("one error event") else {
            panic!("expected Error event");
        };
        assert!(message.contains("compact worker did not produce a summary"));
        assert!(message.contains("terminal append failed"));
        assert!(events.try_recv().is_err(), "must report once");
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
            None,
            &env.notify_buffer,
            &env.pending_submissions,
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
            None,
            &env.notify_buffer,
            &env.pending_submissions,
        )
        .await;

        assert_eq!(status, WorkerStatus::Idle);
        assert!(!shutdown);
        assert_eq!(env.notify_buffer.len(), 1);
    }

    #[tokio::test]
    async fn running_notify_remains_staged_for_the_next_model_boundary() {
        let mut env = make_env().await;
        env._method_tx
            .send(Method::Notify {
                notification_request_id: protocol::new_submission_request_id(),
                message: "continue".into(),
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
            None,
            &env.notify_buffer,
            &env.pending_submissions,
        )
        .await;

        assert_eq!(status, WorkerStatus::Idle);
        assert!(!shutdown);
        assert_eq!(env.notify_buffer.len(), 1);
        assert_eq!(env.pending_submissions.snapshot().notification_count, 0);
    }

    #[tokio::test]
    async fn compact_method_is_rejected_while_running() {
        let mut env = make_env().await;
        let mut events = env.working_event_tx.subscribe();
        env.shared_state
            .transition(WorkerState::Busy(WorkerBusyState::Run(
                WorkerRunState::Running,
            )));
        let command = WorkerCommandEnvelope::new(1);
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
            None,
            &env.notify_buffer,
            &env.pending_submissions,
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
    fn command_admission_rejects_stale_ids_and_reuse_conflicts() {
        let shared = WorkerSharedState::new(
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
                reasoning: None,
                context_usage: None,
            },
        );
        assert!(
            validate_command(
                WorkerCommandEnvelope { command_id: 1 },
                WorkerCommandKind::Pause,
                &shared,
            )
            .is_ok()
        );
        assert_eq!(
            validate_command(
                WorkerCommandEnvelope { command_id: 1 },
                WorkerCommandKind::Pause,
                &shared,
            ),
            Err(WorkerCommandDisposition::StaleCommandId)
        );
        assert_eq!(
            validate_command(
                WorkerCommandEnvelope { command_id: 1 },
                WorkerCommandKind::Cancel,
                &shared,
            ),
            Err(WorkerCommandDisposition::Conflict)
        );
        assert!(
            validate_command(
                WorkerCommandEnvelope { command_id: 2 },
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
