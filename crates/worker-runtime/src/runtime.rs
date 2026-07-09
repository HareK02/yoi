use crate::catalog::{
    ConfigBundleRef, CreateWorkerRequest, WorkerDetail, WorkerLifecycleAck, WorkerStatus,
    WorkerSummary, WorkingDirectoryRequest,
    WorkingDirectoryStatus as CatalogWorkingDirectoryStatus,
};
use crate::config_bundle::{
    ConfigBundle, ConfigBundleAvailability, ConfigBundleSummary, validate_config_bundle,
    validate_config_bundle_ref,
};
#[cfg(feature = "fs-store")]
use crate::diagnostics::DiagnosticSeverity;
use crate::diagnostics::RuntimeDiagnostic;
use crate::error::RuntimeError;
use crate::execution::{
    WorkerExecutionBackend, WorkerExecutionBackendKind, WorkerExecutionBackendRef,
    WorkerExecutionBindingIdentity, WorkerExecutionHandle, WorkerExecutionOperation,
    WorkerExecutionResult, WorkerExecutionRunState, WorkerExecutionSpawnRequest,
    WorkerExecutionSpawnResult, WorkerExecutionStatus,
};
#[cfg(feature = "fs-store")]
use crate::fs_store::{
    FsRuntimeStore, FsRuntimeStoreOptions, PersistedRuntimeState, PersistedWorkerRecord,
};
use crate::identity::{RuntimeId, WorkerId, WorkerRef};
use crate::interaction::{WorkerInput, WorkerInputKind, WorkerInteractionAck};
use crate::management::{
    RuntimeBackendKind, RuntimeLimits, RuntimeOptions, RuntimeStatus, RuntimeSummary,
};
use crate::observation::{
    EventCursor, EventSubscription, EventSubscriptionMode, RuntimeEvent, RuntimeEventBatch,
    RuntimeEventKind, TranscriptEntry, TranscriptProjection, TranscriptQuery, TranscriptRole,
};
#[cfg(feature = "ws-server")]
use crate::observation::{WorkerObservationCursor, WorkerObservationEvent};
use std::collections::BTreeMap;
#[cfg(feature = "ws-server")]
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
#[cfg(feature = "ws-server")]
use tokio::sync::broadcast;

static NEXT_RUNTIME_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// Concrete embedded Runtime domain entity.
///
/// The default implementation is memory-backed and tools/provider-less by
/// design.  An optional `fs-store` feature adds filesystem persistence while
/// preserving the same typed authority boundary. It can later be adapted by
/// backend registries or web servers without making sockets, sessions, or paths
/// public authority.
#[derive(Clone, Debug)]
pub struct Runtime {
    inner: Arc<Mutex<RuntimeState>>,
}

impl Runtime {
    /// Create a memory-backed Runtime with generated identity and default limits.
    pub fn new_memory() -> Self {
        Self::with_options(RuntimeOptions::default())
    }

    /// Create a memory-backed Runtime with explicit options.
    pub fn with_options(options: RuntimeOptions) -> Self {
        let runtime_id = options.runtime_id.unwrap_or_else(|| {
            RuntimeId::generated(NEXT_RUNTIME_SEQUENCE.fetch_add(1, Ordering::Relaxed))
        });
        let mut state = RuntimeState::new(runtime_id, options.display_name, options.limits);
        state.push_event(None, RuntimeEventKind::RuntimeStarted, "runtime started");
        Self {
            inner: Arc::new(Mutex::new(state)),
        }
    }

    /// Create a memory-backed Runtime with an attached execution backend.
    pub fn with_execution_backend(
        options: RuntimeOptions,
        backend: Arc<dyn WorkerExecutionBackend>,
    ) -> Result<Self, RuntimeError> {
        let runtime = Self::with_options(options);
        runtime.install_execution_backend(backend)?;
        Ok(runtime)
    }

    /// Create or restore a filesystem-backed Runtime.
    ///
    /// The store is scoped by typed Runtime identity under `options.root`; if the
    /// Runtime directory already exists, persisted state is loaded and validated.
    /// If it does not exist, a fresh Runtime is initialized and durable files are
    /// created before the Runtime is returned.
    #[cfg(feature = "fs-store")]
    pub fn with_fs_store(options: FsRuntimeStoreOptions) -> Result<Self, RuntimeError> {
        Self::with_fs_store_inner(options, None)
    }

    /// Create or restore a filesystem-backed Runtime with an execution backend.
    #[cfg(feature = "fs-store")]
    pub fn with_fs_store_and_execution_backend(
        options: FsRuntimeStoreOptions,
        backend: Arc<dyn WorkerExecutionBackend>,
    ) -> Result<Self, RuntimeError> {
        Self::with_fs_store_inner(options, Some(WorkerExecutionBackendRef::new(backend)?))
    }

    #[cfg(feature = "fs-store")]
    fn with_fs_store_inner(
        options: FsRuntimeStoreOptions,
        execution_backend: Option<WorkerExecutionBackendRef>,
    ) -> Result<Self, RuntimeError> {
        let runtime_id = options.runtime_id.unwrap_or_else(|| {
            RuntimeId::generated(NEXT_RUNTIME_SEQUENCE.fetch_add(1, Ordering::Relaxed))
        });
        let opened = FsRuntimeStore::open_or_create(options.root, runtime_id.clone())?;
        let mut state = if let Some(persisted) = opened.state {
            RuntimeState::from_persisted(persisted, opened.store)?
        } else {
            let mut state = RuntimeState::new_fs_backed(
                runtime_id,
                options.display_name,
                options.limits,
                opened.store,
            );
            let event_id =
                state.push_event(None, RuntimeEventKind::RuntimeStarted, "runtime started");
            state.persist_runtime_snapshot()?;
            state.persist_event_by_id(event_id)?;
            state
        };
        state.execution_backend = execution_backend;
        Ok(Self {
            inner: Arc::new(Mutex::new(state)),
        })
    }

    /// Runtime id half of public Worker authority.
    pub fn runtime_id(&self) -> Result<RuntimeId, RuntimeError> {
        Ok(self.lock()?.runtime_id.clone())
    }

    /// Management-plane summary.
    pub fn summary(&self) -> Result<RuntimeSummary, RuntimeError> {
        let state = self.lock()?;
        let mut active_worker_count = 0;
        let mut stopped_worker_count = 0;
        let mut cancelled_worker_count = 0;
        for worker in state.workers.values() {
            match worker.status {
                WorkerStatus::Running => active_worker_count += 1,
                WorkerStatus::Stopped => stopped_worker_count += 1,
                WorkerStatus::Cancelled => cancelled_worker_count += 1,
            }
        }

        Ok(RuntimeSummary {
            runtime_id: state.runtime_id.clone(),
            display_name: state.display_name.clone(),
            backend: state.backend,
            status: state.status,
            worker_count: state.workers.len(),
            active_worker_count,
            stopped_worker_count,
            cancelled_worker_count,
            diagnostic_count: state.diagnostics.len(),
            limits: state.limits.clone(),
            worker_creation_available: state.execution_backend.is_some(),
        })
    }

    /// Current Runtime lifecycle state.
    pub fn status(&self) -> Result<RuntimeStatus, RuntimeError> {
        Ok(self.lock()?.status)
    }

    /// Store a backend-synced Profile/config bundle for later Worker creation.
    pub fn store_config_bundle(
        &self,
        bundle: ConfigBundle,
    ) -> Result<ConfigBundleAvailability, RuntimeError> {
        validate_config_bundle(&bundle)?;
        let mut state = self.lock()?;
        state.ensure_running()?;
        let reference = ConfigBundleRef {
            id: bundle.metadata.id.clone(),
            digest: bundle.metadata.digest.clone(),
        };
        let summary = bundle.summary();
        state
            .config_bundles
            .insert(bundle.metadata.id.clone(), bundle);
        state.persist_runtime_snapshot()?;
        Ok(ConfigBundleAvailability { reference, summary })
    }

    /// List synced config bundles known to this Runtime.
    pub fn list_config_bundles(&self) -> Result<Vec<ConfigBundleSummary>, RuntimeError> {
        Ok(self
            .lock()?
            .config_bundles
            .values()
            .map(ConfigBundle::summary)
            .collect())
    }

    /// Validate that a config bundle reference is present and digest-matched.
    pub fn check_config_bundle(
        &self,
        reference: &ConfigBundleRef,
    ) -> Result<ConfigBundleAvailability, RuntimeError> {
        let state = self.lock()?;
        state.check_config_bundle_ref(reference)
    }

    /// Stop the Runtime.  v0 keeps data readable after stop, but rejects new
    /// create/send/worker lifecycle mutations.
    pub fn stop_runtime(&self) -> Result<u64, RuntimeError> {
        let mut state = self.lock()?;
        if state.status == RuntimeStatus::Stopped {
            return Ok(state.last_event_id());
        }
        state.status = RuntimeStatus::Stopped;
        let runtime_id = state.runtime_id.clone();
        for worker in state.workers.values_mut() {
            if worker.status.is_active() {
                worker.status = WorkerStatus::Stopped;
            }
        }
        let event_id = state.push_event(
            None,
            RuntimeEventKind::RuntimeStopped,
            format!("runtime {runtime_id} stopped"),
        );
        state.persist_runtime_snapshot()?;
        state.persist_workers()?;
        state.persist_event_by_id(event_id)?;
        Ok(event_id)
    }

    /// Create a Runtime-owned working directory through the attached execution backend.
    pub fn create_working_directory(
        &self,
        request: WorkingDirectoryRequest,
    ) -> Result<CatalogWorkingDirectoryStatus, RuntimeError> {
        let backend = {
            let state = self.lock()?;
            state.ensure_running()?;
            state.execution_backend.clone().ok_or_else(|| {
                RuntimeError::ExecutionBackendUnavailable {
                    message: "working directory creation requires an execution backend".to_string(),
                }
            })?
        };
        backend
            .create_working_directory(&request)
            .map_err(|diagnostic| RuntimeError::InvalidRequest(diagnostic.to_string()))
    }

    /// List Runtime-owned working directories through the attached execution backend.
    pub fn list_working_directories(
        &self,
    ) -> Result<Vec<CatalogWorkingDirectoryStatus>, RuntimeError> {
        let backend = {
            let state = self.lock()?;
            state.ensure_running()?;
            state.execution_backend.clone().ok_or_else(|| {
                RuntimeError::ExecutionBackendUnavailable {
                    message: "working directory listing requires an execution backend".to_string(),
                }
            })?
        };
        Ok(backend.list_working_directories())
    }

    /// Get a Runtime-owned working directory status.
    pub fn working_directory(
        &self,
        working_directory_id: &str,
    ) -> Result<CatalogWorkingDirectoryStatus, RuntimeError> {
        let backend = {
            let state = self.lock()?;
            state.ensure_running()?;
            state.execution_backend.clone().ok_or_else(|| {
                RuntimeError::ExecutionBackendUnavailable {
                    message: "working directory lookup requires an execution backend".to_string(),
                }
            })?
        };
        backend
            .working_directory(working_directory_id)
            .map_err(|diagnostic| RuntimeError::InvalidRequest(diagnostic.to_string()))
    }

    /// Cleanup a Runtime-owned working directory.
    pub fn cleanup_working_directory(
        &self,
        working_directory_id: &str,
    ) -> Result<CatalogWorkingDirectoryStatus, RuntimeError> {
        let backend = {
            let state = self.lock()?;
            state.ensure_running()?;
            state.execution_backend.clone().ok_or_else(|| {
                RuntimeError::ExecutionBackendUnavailable {
                    message: "working directory cleanup requires an execution backend".to_string(),
                }
            })?
        };
        backend
            .cleanup_working_directory(working_directory_id)
            .map_err(|diagnostic| RuntimeError::InvalidRequest(diagnostic.to_string()))
    }

    /// Create a Worker through the canonical profile-source + execution backend path.
    pub fn create_worker(
        &self,
        request: CreateWorkerRequest,
    ) -> Result<WorkerDetail, RuntimeError> {
        let (backend, worker_ref, spawn_request) = {
            let mut state = self.lock()?;
            state.ensure_running()?;
            validate_create_worker_request(&request)?;
            state.validate_worker_config_boundary(&request)?;
            let backend = state.execution_backend.clone().ok_or_else(|| {
                RuntimeError::ExecutionBackendUnavailable {
                    message: "worker creation requires an execution backend".to_string(),
                }
            })?;

            let worker_id = WorkerId::generated(state.next_worker_sequence);
            state.next_worker_sequence += 1;
            let worker_ref = WorkerRef::new(state.runtime_id.clone(), worker_id.clone());
            let event_id = state.push_event(
                Some(worker_ref.clone()),
                RuntimeEventKind::WorkerCreated,
                format!("worker {worker_id} created"),
            );

            let mut transcript = Vec::new();
            let mut next_transcript_sequence = 1;
            if let Some(input) = request.initial_input.clone() {
                transcript.push(TranscriptEntry {
                    sequence: next_transcript_sequence,
                    worker_ref: worker_ref.clone(),
                    role: TranscriptRole::User,
                    content: input.content,
                    event_id,
                });
                next_transcript_sequence += 1;
            }

            let record = WorkerRecord {
                worker_ref: worker_ref.clone(),
                worker_id: worker_id.clone(),
                status: WorkerStatus::Running,
                request: request.clone(),
                execution: WorkerExecutionStatus::unconnected(),
                execution_handle: None,
                transcript,
                next_transcript_sequence,
                last_event_id: event_id,
            };
            state.workers.insert(worker_id, record);
            let spawn_request = WorkerExecutionSpawnRequest {
                worker_ref: worker_ref.clone(),
                request,
                context: self.execution_context(worker_ref.clone()),
                working_directory: None,
                config_bundle: None,
            };
            (backend, worker_ref, spawn_request)
        };

        let spawn_result = backend.spawn_worker(spawn_request);
        let (handle, run_state, working_directory) = match spawn_result {
            WorkerExecutionSpawnResult::Connected {
                handle,
                run_state,
                working_directory,
            } => (handle, run_state, working_directory),
            WorkerExecutionSpawnResult::Rejected(result)
            | WorkerExecutionSpawnResult::Errored(result) => {
                self.rollback_failed_create(&worker_ref)?;
                return Err(RuntimeError::WorkerExecutionRejected {
                    worker_id: worker_ref.worker_id.clone(),
                    operation: result.operation,
                    outcome: result.outcome,
                    message: result.message_or_default(),
                    result,
                });
            }
        };

        if let Some(initial_input) = {
            let state = self.lock()?;
            state.worker(&worker_ref)?.request.initial_input.clone()
        } {
            let dispatch_result = backend.dispatch_input(&handle, initial_input);
            if !dispatch_result.is_accepted() {
                let _ = backend.stop_worker(&handle);
                self.rollback_failed_create(&worker_ref)?;
                return Err(RuntimeError::WorkerExecutionRejected {
                    worker_id: worker_ref.worker_id.clone(),
                    operation: dispatch_result.operation,
                    outcome: dispatch_result.outcome,
                    message: dispatch_result.message_or_default(),
                    result: dispatch_result,
                });
            }
            self.commit_created_worker(
                &worker_ref,
                handle,
                WorkerExecutionRunState::Busy,
                working_directory,
                WorkerExecutionResult::accepted(
                    WorkerExecutionOperation::Input,
                    WorkerExecutionRunState::Busy,
                ),
            )
        } else {
            self.commit_created_worker(
                &worker_ref,
                handle,
                run_state,
                working_directory,
                WorkerExecutionResult::accepted(WorkerExecutionOperation::Spawn, run_state),
            )
        }
    }

    /// List Workers known to this Runtime.
    pub fn list_workers(&self) -> Result<Vec<WorkerSummary>, RuntimeError> {
        let state = self.lock()?;
        Ok(state
            .workers
            .values()
            .map(|worker| worker.summary(&state.runtime_id))
            .collect())
    }

    /// Fetch Worker detail.  The supplied [`WorkerRef`] must match this Runtime.
    pub fn worker_detail(&self, worker_ref: &WorkerRef) -> Result<WorkerDetail, RuntimeError> {
        let state = self.lock()?;
        let worker = state.worker(worker_ref)?;
        Ok(worker.detail(&state.runtime_id))
    }

    /// Accept input into a Worker transcript.
    pub fn send_input(
        &self,
        worker_ref: &WorkerRef,
        input: WorkerInput,
    ) -> Result<WorkerInteractionAck, RuntimeError> {
        let (backend, handle) = {
            let mut state = self.lock()?;
            state.ensure_running()?;
            validate_worker_input(&input)?;
            state.ensure_worker_ref(worker_ref)?;
            let worker = state.worker(worker_ref)?;
            if !worker.status.is_active() {
                return Err(RuntimeError::InvalidRequest(format!(
                    "worker {} is not running",
                    worker_ref.worker_id
                )));
            }
            let backend = state.execution_backend.clone();
            let handle = worker.execution_handle.clone();
            match (backend, handle) {
                (Some(backend), Some(handle)) => (backend, handle),
                _ => {
                    let result = WorkerExecutionResult::rejected(
                        WorkerExecutionOperation::Input,
                        "worker has no execution backend",
                    );
                    let worker = state.worker_mut(worker_ref)?;
                    let mut execution = WorkerExecutionStatus::unconnected().with_result(result);
                    execution.binding = worker.execution.binding.clone();
                    worker.execution = execution;
                    state.persist_worker(&worker_ref.worker_id)?;
                    return Err(RuntimeError::WorkerExecutionUnavailable {
                        worker_id: worker_ref.worker_id.clone(),
                        message: "worker has no execution backend".to_string(),
                    });
                }
            }
        };

        let dispatch_result = backend.dispatch_input(&handle, input.clone());
        if !dispatch_result.is_accepted() {
            self.record_execution_result(worker_ref, dispatch_result.clone())?;
            return Err(RuntimeError::WorkerExecutionRejected {
                worker_id: worker_ref.worker_id.clone(),
                operation: dispatch_result.operation,
                outcome: dispatch_result.outcome,
                message: dispatch_result.message_or_default(),
                result: dispatch_result,
            });
        }

        let mut state = self.lock()?;
        state.ensure_running()?;
        let event_id = state.push_event(
            Some(worker_ref.clone()),
            RuntimeEventKind::WorkerInputAccepted,
            "worker input accepted",
        );
        let worker = state.worker_mut(worker_ref)?;

        let input_content = input.content;
        let role = match input.kind {
            WorkerInputKind::User => TranscriptRole::User,
            WorkerInputKind::System => TranscriptRole::System,
        };
        let transcript_sequence = worker.next_transcript_sequence;
        worker.next_transcript_sequence += 1;
        worker.last_event_id = event_id;
        worker.execution = WorkerExecutionStatus {
            backend: WorkerExecutionBackendKind::Connected,
            run_state: dispatch_result.run_state,
            binding: worker.execution.binding.clone(),
            working_directory: worker.execution.working_directory.clone(),
            last_result: Some(dispatch_result),
        };
        worker.transcript.push(TranscriptEntry {
            sequence: transcript_sequence,
            worker_ref: worker_ref.clone(),
            role,
            content: input_content.clone(),
            event_id,
        });

        let status = worker.status;
        #[cfg(feature = "ws-server")]
        {
            let payload = match role {
                TranscriptRole::User => protocol::Event::UserMessage {
                    segments: vec![protocol::Segment::Text {
                        content: input_content.clone(),
                    }],
                },
                TranscriptRole::Assistant => protocol::Event::TextDone {
                    text: input_content.clone(),
                },
                TranscriptRole::System => protocol::Event::SystemItem {
                    item: serde_json::json!({
                        "kind": "embedded_worker_system_input",
                        "content": input_content.clone(),
                    }),
                },
            };
            state.push_worker_observation_event(worker_ref.clone(), payload);
        }
        state.persist_runtime_snapshot()?;
        state.persist_worker(&worker_ref.worker_id)?;
        state.persist_event_by_id(event_id)?;
        state.persist_transcript_entry(&worker_ref.worker_id, transcript_sequence)?;

        Ok(WorkerInteractionAck {
            worker_ref: worker_ref.clone(),
            status,
            transcript_sequence,
            event_id,
        })
    }

    fn commit_created_worker(
        &self,
        worker_ref: &WorkerRef,
        handle: WorkerExecutionHandle,
        run_state: WorkerExecutionRunState,
        working_directory: Option<CatalogWorkingDirectoryStatus>,
        result: WorkerExecutionResult,
    ) -> Result<WorkerDetail, RuntimeError> {
        let mut state = self.lock()?;
        let runtime_id = state.runtime_id.clone();
        let detail = {
            let binding = WorkerExecutionBindingIdentity::from_handle(&handle);
            let worker = state.worker_mut(worker_ref)?;
            worker.execution_handle = Some(handle);
            let mut execution = WorkerExecutionStatus::connected(run_state).with_binding(binding);
            if let Some(status) = working_directory {
                execution = execution.with_working_directory(status);
            }
            worker.execution = execution.with_result(result);
            worker.detail(&runtime_id)
        };
        state.persist_runtime_snapshot()?;
        state.persist_worker(&worker_ref.worker_id)?;
        let worker = state.worker(worker_ref)?;
        for entry in &worker.transcript {
            state.persist_transcript_entry(&worker_ref.worker_id, entry.sequence)?;
        }
        state.persist_event_by_id(detail.last_event_id)?;
        Ok(detail)
    }

    fn rollback_failed_create(&self, worker_ref: &WorkerRef) -> Result<(), RuntimeError> {
        let mut state = self.lock()?;
        if let Some(record) = state.workers.remove(&worker_ref.worker_id) {
            state.events.retain(|event| {
                event.id != record.last_event_id || event.worker_ref.as_ref() != Some(worker_ref)
            });
        }
        Ok(())
    }

    fn record_execution_result(
        &self,
        worker_ref: &WorkerRef,
        result: WorkerExecutionResult,
    ) -> Result<(), RuntimeError> {
        let mut state = self.lock()?;
        let worker = state.worker_mut(worker_ref)?;
        worker.execution = WorkerExecutionStatus {
            backend: WorkerExecutionBackendKind::Connected,
            run_state: result.run_state,
            binding: worker.execution.binding.clone(),
            working_directory: worker.execution.working_directory.clone(),
            last_result: Some(result),
        };
        state.persist_worker(&worker_ref.worker_id)?;
        Ok(())
    }

    fn dispatch_lifecycle_to_backend(
        &self,
        worker_ref: &WorkerRef,
        operation: WorkerExecutionOperation,
    ) -> Result<(), RuntimeError> {
        let Some((backend, handle)) = ({
            let state = self.lock()?;
            state.ensure_worker_ref(worker_ref)?;
            let worker = state.worker(worker_ref)?;
            if !worker.status.is_active() {
                return Ok(());
            }
            match (
                state.execution_backend.clone(),
                worker.execution_handle.clone(),
            ) {
                (Some(backend), Some(handle)) => Some((backend, handle)),
                _ => None,
            }
        }) else {
            return Ok(());
        };

        let result = match operation {
            WorkerExecutionOperation::Stop => backend.stop_worker(&handle),
            WorkerExecutionOperation::Cancel => backend.cancel_worker(&handle),
            WorkerExecutionOperation::Spawn | WorkerExecutionOperation::Input => return Ok(()),
        };
        if result.is_accepted() {
            self.record_execution_result(worker_ref, result)?;
            return Ok(());
        }
        self.record_execution_result(worker_ref, result.clone())?;
        Err(RuntimeError::WorkerExecutionRejected {
            worker_id: worker_ref.worker_id.clone(),
            operation: result.operation,
            outcome: result.outcome,
            message: result.message_or_default(),
            result,
        })
    }

    /// Stop a Worker.  Repeated stops are idempotent and return the last event id.
    pub fn stop_worker(
        &self,
        worker_ref: &WorkerRef,
        reason: Option<String>,
    ) -> Result<WorkerLifecycleAck, RuntimeError> {
        self.dispatch_lifecycle_to_backend(worker_ref, WorkerExecutionOperation::Stop)?;
        self.transition_worker(
            worker_ref,
            WorkerStatus::Stopped,
            RuntimeEventKind::WorkerStopped,
            reason.unwrap_or_else(|| "worker stopped".to_string()),
        )
    }

    /// Cancel a Worker.  Repeated cancels are idempotent and return the last event id.
    pub fn cancel_worker(
        &self,
        worker_ref: &WorkerRef,
        reason: Option<String>,
    ) -> Result<WorkerLifecycleAck, RuntimeError> {
        self.dispatch_lifecycle_to_backend(worker_ref, WorkerExecutionOperation::Cancel)?;
        self.transition_worker(
            worker_ref,
            WorkerStatus::Cancelled,
            RuntimeEventKind::WorkerCancelled,
            reason.unwrap_or_else(|| "worker cancelled".to_string()),
        )
    }

    /// Bounded transcript projection for a Worker.
    pub fn transcript_projection(
        &self,
        worker_ref: &WorkerRef,
        query: TranscriptQuery,
    ) -> Result<TranscriptProjection, RuntimeError> {
        let state = self.lock()?;
        if query.limit > state.limits.max_transcript_projection_items {
            return Err(RuntimeError::LimitTooLarge {
                requested: query.limit,
                max: state.limits.max_transcript_projection_items,
            });
        }
        let worker = state.worker(worker_ref)?;
        let total_items = worker.transcript.len();
        let end = query.start.saturating_add(query.limit).min(total_items);
        let items = if query.start >= total_items {
            Vec::new()
        } else {
            worker.transcript[query.start..end].to_vec()
        };
        let next_start = (end < total_items).then_some(end);
        Ok(TranscriptProjection {
            worker_ref: worker_ref.clone(),
            start: query.start,
            limit: query.limit,
            total_items,
            items,
            next_start,
        })
    }

    /// Cursor pointing to the beginning of Runtime events.
    pub fn event_cursor_from_start(&self) -> Result<EventCursor, RuntimeError> {
        let state = self.lock()?;
        Ok(EventCursor {
            runtime_id: state.runtime_id.clone(),
            next_event_id: 1,
        })
    }

    /// Cursor pointing after the current last event.
    pub fn event_cursor_now(&self) -> Result<EventCursor, RuntimeError> {
        let state = self.lock()?;
        Ok(EventCursor {
            runtime_id: state.runtime_id.clone(),
            next_event_id: state.last_event_id() + 1,
        })
    }

    /// Poll Runtime events from a cursor.
    pub fn read_events(
        &self,
        cursor: &EventCursor,
        limit: usize,
    ) -> Result<RuntimeEventBatch, RuntimeError> {
        let state = self.lock()?;
        if cursor.runtime_id != state.runtime_id {
            return Err(RuntimeError::WrongRuntimeCursor {
                expected_runtime_id: state.runtime_id.clone(),
                actual_runtime_id: cursor.runtime_id.clone(),
            });
        }
        if limit > state.limits.max_event_batch_items {
            return Err(RuntimeError::LimitTooLarge {
                requested: limit,
                max: state.limits.max_event_batch_items,
            });
        }

        let mut events = Vec::new();
        for event in state
            .events
            .iter()
            .filter(|event| event.id >= cursor.next_event_id)
            .take(limit)
        {
            events.push(event.clone());
        }
        let next_event_id = events
            .last()
            .map(|event| event.id + 1)
            .unwrap_or(cursor.next_event_id);
        let has_more = state.events.iter().any(|event| event.id >= next_event_id);
        Ok(RuntimeEventBatch {
            runtime_id: state.runtime_id.clone(),
            cursor: EventCursor {
                runtime_id: state.runtime_id.clone(),
                next_event_id,
            },
            events,
            has_more,
        })
    }

    /// Create a poll-only placeholder subscription boundary for future streaming.
    pub fn subscribe_events(&self, cursor: EventCursor) -> Result<EventSubscription, RuntimeError> {
        let state = self.lock()?;
        if cursor.runtime_id != state.runtime_id {
            return Err(RuntimeError::WrongRuntimeCursor {
                expected_runtime_id: state.runtime_id.clone(),
                actual_runtime_id: cursor.runtime_id,
            });
        }
        Ok(EventSubscription {
            runtime_id: state.runtime_id.clone(),
            cursor,
            mode: EventSubscriptionMode::PollOnly,
        })
    }

    /// Cursor pointing after the current worker-scoped protocol observation event.
    #[cfg(feature = "ws-server")]
    pub fn worker_observation_cursor_now(
        &self,
        worker_ref: &WorkerRef,
    ) -> Result<WorkerObservationCursor, RuntimeError> {
        let state = self.lock()?;
        state.ensure_worker_ref(worker_ref)?;
        let sequence = state
            .observation_events
            .iter()
            .rev()
            .find(|event| &event.worker_ref == worker_ref)
            .map(|event| event.sequence)
            .unwrap_or(0);
        Ok(WorkerObservationCursor::new(sequence))
    }

    /// Build the current Worker Snapshot event used as the first observation frame.
    #[cfg(feature = "ws-server")]
    pub fn worker_observation_snapshot(
        &self,
        worker_ref: &WorkerRef,
    ) -> Result<protocol::Event, RuntimeError> {
        let state = self.lock()?;
        let _worker = state.worker(worker_ref)?;
        Ok(protocol::Event::Snapshot {
            entries: Vec::new(),
            greeting: protocol::Greeting {
                worker_name: worker_ref.worker_id.to_string(),
                cwd: String::new(),
                provider: "worker-runtime".to_string(),
                model: "worker-runtime".to_string(),
                scope_summary: "runtime worker observation".to_string(),
                tools: Vec::new(),
                context_window: 0,
                context_tokens: 0,
            },
            status: protocol::WorkerStatus::Idle,
            in_flight: protocol::InFlightSnapshot { blocks: Vec::new() },
        })
    }

    /// Replay retained worker-scoped protocol observation events after a cursor.
    #[cfg(feature = "ws-server")]
    pub fn read_worker_observation_events(
        &self,
        worker_ref: &WorkerRef,
        cursor: WorkerObservationCursor,
    ) -> Result<Vec<WorkerObservationEvent>, RuntimeError> {
        let state = self.lock()?;
        state.ensure_worker_ref(worker_ref)?;
        state.validate_worker_observation_cursor(worker_ref, cursor)?;
        Ok(state
            .observation_events
            .iter()
            .filter(|event| &event.worker_ref == worker_ref && event.sequence > cursor.sequence)
            .cloned()
            .collect())
    }

    /// Subscribe to live protocol observation events.
    #[cfg(feature = "ws-server")]
    pub fn subscribe_worker_observation(
        &self,
    ) -> Result<broadcast::Receiver<WorkerObservationEvent>, RuntimeError> {
        Ok(self.lock()?.observation_tx.subscribe())
    }

    /// Append a Worker protocol event to the observation bus.
    #[cfg(feature = "ws-server")]
    pub fn observe_worker_event(
        &self,
        worker_ref: &WorkerRef,
        payload: protocol::Event,
    ) -> Result<WorkerObservationEvent, RuntimeError> {
        let mut state = self.lock()?;
        state.ensure_worker_ref(worker_ref)?;
        let transcript_sequence = state.project_protocol_event_to_transcript(worker_ref, &payload);
        let execution_state_changed =
            state.project_protocol_event_to_execution(worker_ref, &payload);
        let event = state.push_worker_observation_event(worker_ref.clone(), payload);
        if transcript_sequence.is_some() || execution_state_changed {
            state.persist_worker(&worker_ref.worker_id)?;
        }
        if let Some(sequence) = transcript_sequence {
            state.persist_transcript_entry(&worker_ref.worker_id, sequence)?;
        }
        Ok(event)
    }

    /// Snapshot current diagnostics.
    pub fn diagnostics(&self) -> Result<Vec<RuntimeDiagnostic>, RuntimeError> {
        Ok(self.lock()?.diagnostics.clone())
    }

    fn transition_worker(
        &self,
        worker_ref: &WorkerRef,
        status: WorkerStatus,
        event_kind: RuntimeEventKind,
        reason: String,
    ) -> Result<WorkerLifecycleAck, RuntimeError> {
        let mut state = self.lock()?;
        state.ensure_running()?;
        state.ensure_worker_ref(worker_ref)?;

        {
            let worker = state.worker(worker_ref)?;
            if !worker.status.is_active() {
                return Ok(WorkerLifecycleAck {
                    worker_ref: worker_ref.clone(),
                    status: worker.status,
                    event_id: worker.last_event_id,
                });
            }
        }

        let event_id = state.push_event(Some(worker_ref.clone()), event_kind, reason);
        let worker = state.worker_mut(worker_ref)?;
        worker.status = status;
        worker.last_event_id = event_id;
        let status = worker.status;
        state.persist_runtime_snapshot()?;
        state.persist_worker(&worker_ref.worker_id)?;
        state.persist_event_by_id(event_id)?;
        Ok(WorkerLifecycleAck {
            worker_ref: worker_ref.clone(),
            status,
            event_id,
        })
    }

    fn install_execution_backend(
        &self,
        backend: Arc<dyn WorkerExecutionBackend>,
    ) -> Result<(), RuntimeError> {
        let backend = WorkerExecutionBackendRef::new(backend)?;
        let mut state = self.lock()?;
        state.execution_backend = Some(backend);
        Ok(())
    }

    #[cfg(feature = "ws-server")]
    fn execution_context(&self, worker_ref: WorkerRef) -> crate::execution::WorkerExecutionContext {
        let runtime = self.clone();
        crate::execution::WorkerExecutionContext::new(
            worker_ref,
            Arc::new(move |worker_ref, payload| runtime.observe_worker_event(&worker_ref, payload)),
        )
    }

    #[cfg(not(feature = "ws-server"))]
    fn execution_context(&self, worker_ref: WorkerRef) -> crate::execution::WorkerExecutionContext {
        crate::execution::WorkerExecutionContext::new(worker_ref)
    }

    fn lock(&self) -> Result<MutexGuard<'_, RuntimeState>, RuntimeError> {
        self.inner.lock().map_err(|_| RuntimeError::StatePoisoned)
    }
}

#[cfg_attr(not(feature = "fs-store"), allow(dead_code))]
#[derive(Clone, Debug)]
enum RuntimePersistence {
    Memory,
    #[cfg(feature = "fs-store")]
    Fs(FsRuntimeStore),
}

#[derive(Debug)]
struct RuntimeState {
    runtime_id: RuntimeId,
    display_name: Option<String>,
    backend: RuntimeBackendKind,
    #[cfg_attr(not(feature = "fs-store"), allow(dead_code))]
    persistence: RuntimePersistence,
    status: RuntimeStatus,
    limits: RuntimeLimits,
    execution_backend: Option<WorkerExecutionBackendRef>,
    next_worker_sequence: u64,
    next_event_id: u64,
    #[cfg(feature = "fs-store")]
    next_diagnostic_id: u64,
    workers: BTreeMap<WorkerId, WorkerRecord>,
    config_bundles: BTreeMap<String, ConfigBundle>,
    events: Vec<RuntimeEvent>,
    diagnostics: Vec<RuntimeDiagnostic>,
    #[cfg(feature = "ws-server")]
    next_observation_sequence: u64,
    #[cfg(feature = "ws-server")]
    observation_events: VecDeque<WorkerObservationEvent>,
    #[cfg(feature = "ws-server")]
    observation_tx: broadcast::Sender<WorkerObservationEvent>,
}

impl RuntimeState {
    fn new(runtime_id: RuntimeId, display_name: Option<String>, limits: RuntimeLimits) -> Self {
        Self {
            runtime_id,
            display_name,
            backend: RuntimeBackendKind::Memory,
            persistence: RuntimePersistence::Memory,
            status: RuntimeStatus::Running,
            limits,
            execution_backend: None,
            next_worker_sequence: 1,
            next_event_id: 1,
            #[cfg(feature = "fs-store")]
            next_diagnostic_id: 1,
            workers: BTreeMap::new(),
            config_bundles: BTreeMap::new(),
            events: Vec::new(),
            diagnostics: Vec::new(),
            #[cfg(feature = "ws-server")]
            next_observation_sequence: 1,
            #[cfg(feature = "ws-server")]
            observation_events: VecDeque::new(),
            #[cfg(feature = "ws-server")]
            observation_tx: broadcast::channel(256).0,
        }
    }

    #[cfg(feature = "fs-store")]
    fn new_fs_backed(
        runtime_id: RuntimeId,
        display_name: Option<String>,
        limits: RuntimeLimits,
        store: FsRuntimeStore,
    ) -> Self {
        Self {
            runtime_id,
            display_name,
            backend: RuntimeBackendKind::FsStore,
            persistence: RuntimePersistence::Fs(store),
            status: RuntimeStatus::Running,
            limits,
            execution_backend: None,
            next_worker_sequence: 1,
            next_event_id: 1,
            #[cfg(feature = "fs-store")]
            next_diagnostic_id: 1,
            workers: BTreeMap::new(),
            config_bundles: BTreeMap::new(),
            events: Vec::new(),
            diagnostics: Vec::new(),
            #[cfg(feature = "ws-server")]
            next_observation_sequence: 1,
            #[cfg(feature = "ws-server")]
            observation_events: VecDeque::new(),
            #[cfg(feature = "ws-server")]
            observation_tx: broadcast::channel(256).0,
        }
    }

    #[cfg(feature = "fs-store")]
    fn from_persisted(
        persisted: PersistedRuntimeState,
        store: FsRuntimeStore,
    ) -> Result<Self, RuntimeError> {
        if persisted.runtime_id != *store.runtime_id() {
            return Err(RuntimeError::StoreCorrupt {
                operation: "restore runtime state",
                path: store.runtime_dir().to_path_buf(),
                message: format!(
                    "persisted runtime id {} does not match store runtime {}",
                    persisted.runtime_id,
                    store.runtime_id()
                ),
            });
        }

        let mut workers = BTreeMap::new();
        let mut diagnostics = persisted.diagnostics;
        let mut next_diagnostic_id = persisted.next_diagnostic_id;
        for (worker_id, worker) in persisted.workers {
            let execution = if worker.execution.binding.is_some()
                && worker.execution.backend == WorkerExecutionBackendKind::Connected
            {
                let stale = WorkerExecutionStatus::stale(worker.execution);
                diagnostics.push(RuntimeDiagnostic {
                    id: next_diagnostic_id,
                    severity: DiagnosticSeverity::Warning,
                    code: "worker_execution_mapping_stale".to_string(),
                    message: format!(
                        "worker {} has persisted execution binding identity but no live execution handle was restored",
                        worker.worker_id
                    ),
                    worker_ref: Some(worker.worker_ref.clone()),
                });
                next_diagnostic_id += 1;
                stale
            } else {
                worker.execution
            };
            workers.insert(
                worker_id,
                WorkerRecord {
                    worker_ref: worker.worker_ref,
                    worker_id: worker.worker_id,
                    status: worker.status,
                    request: worker.request,
                    execution,
                    execution_handle: None,
                    transcript: worker.transcript,
                    next_transcript_sequence: worker.next_transcript_sequence,
                    last_event_id: worker.last_event_id,
                },
            );
        }

        Ok(Self {
            runtime_id: persisted.runtime_id,
            display_name: persisted.display_name,
            backend: RuntimeBackendKind::FsStore,
            persistence: RuntimePersistence::Fs(store),
            status: persisted.status,
            limits: persisted.limits,
            execution_backend: None,
            next_worker_sequence: persisted.next_worker_sequence,
            next_event_id: persisted.next_event_id,
            next_diagnostic_id,
            workers,
            config_bundles: persisted.config_bundles,
            events: persisted.events,
            diagnostics,
            #[cfg(feature = "ws-server")]
            next_observation_sequence: 1,
            #[cfg(feature = "ws-server")]
            observation_events: VecDeque::new(),
            #[cfg(feature = "ws-server")]
            observation_tx: broadcast::channel(256).0,
        })
    }

    #[cfg(feature = "fs-store")]
    fn persisted_state(&self) -> PersistedRuntimeState {
        PersistedRuntimeState {
            runtime_id: self.runtime_id.clone(),
            display_name: self.display_name.clone(),
            status: self.status,
            limits: self.limits.clone(),
            next_worker_sequence: self.next_worker_sequence,
            next_event_id: self.next_event_id,
            next_diagnostic_id: self.next_diagnostic_id,
            workers: self
                .workers
                .iter()
                .map(|(worker_id, worker)| (worker_id.clone(), worker.persisted_record()))
                .collect(),
            config_bundles: self.config_bundles.clone(),
            events: self.events.clone(),
            diagnostics: self.diagnostics.clone(),
        }
    }

    #[cfg(feature = "fs-store")]
    fn fs_store(&self) -> Option<&FsRuntimeStore> {
        match &self.persistence {
            RuntimePersistence::Memory => None,
            RuntimePersistence::Fs(store) => Some(store),
        }
    }

    #[cfg(feature = "fs-store")]
    fn persist_runtime_snapshot(&self) -> Result<(), RuntimeError> {
        if let Some(store) = self.fs_store() {
            store.write_runtime_snapshot(&self.persisted_state())?;
        }
        Ok(())
    }

    #[cfg(feature = "fs-store")]
    fn persist_worker(&self, worker_id: &WorkerId) -> Result<(), RuntimeError> {
        if let Some(store) = self.fs_store() {
            let worker =
                self.workers
                    .get(worker_id)
                    .ok_or_else(|| RuntimeError::WorkerNotFound {
                        runtime_id: self.runtime_id.clone(),
                        worker_id: worker_id.clone(),
                    })?;
            store.write_worker_snapshot(&worker.persisted_record())?;
        }
        Ok(())
    }

    #[cfg(feature = "fs-store")]
    fn persist_event_by_id(&self, event_id: u64) -> Result<(), RuntimeError> {
        if let Some(store) = self.fs_store() {
            let event = self
                .events
                .iter()
                .find(|event| event.id == event_id)
                .ok_or_else(|| RuntimeError::StoreCorrupt {
                    operation: "persist event",
                    path: store.runtime_dir().to_path_buf(),
                    message: format!("event {event_id} is missing from runtime state"),
                })?;
            store.append_event(event)?;
        }
        Ok(())
    }

    #[cfg(feature = "fs-store")]
    fn persist_transcript_entry(
        &self,
        worker_id: &WorkerId,
        sequence: u64,
    ) -> Result<(), RuntimeError> {
        if let Some(store) = self.fs_store() {
            let worker =
                self.workers
                    .get(worker_id)
                    .ok_or_else(|| RuntimeError::WorkerNotFound {
                        runtime_id: self.runtime_id.clone(),
                        worker_id: worker_id.clone(),
                    })?;
            let entry = worker
                .transcript
                .iter()
                .find(|entry| entry.sequence == sequence)
                .ok_or_else(|| RuntimeError::StoreCorrupt {
                    operation: "persist transcript",
                    path: store.runtime_dir().to_path_buf(),
                    message: format!(
                        "transcript sequence {sequence} is missing from worker {worker_id}"
                    ),
                })?;
            store.append_transcript_entry(entry)?;
        }
        Ok(())
    }

    #[cfg(feature = "fs-store")]
    fn persist_workers(&self) -> Result<(), RuntimeError> {
        if self.fs_store().is_some() {
            for worker_id in self.workers.keys() {
                self.persist_worker(worker_id)?;
            }
        }
        Ok(())
    }

    #[cfg(not(feature = "fs-store"))]
    fn persist_runtime_snapshot(&self) -> Result<(), RuntimeError> {
        Ok(())
    }

    #[cfg(not(feature = "fs-store"))]
    fn persist_worker(&self, _worker_id: &WorkerId) -> Result<(), RuntimeError> {
        Ok(())
    }

    #[cfg(not(feature = "fs-store"))]
    fn persist_event_by_id(&self, _event_id: u64) -> Result<(), RuntimeError> {
        Ok(())
    }

    #[cfg(not(feature = "fs-store"))]
    fn persist_transcript_entry(
        &self,
        _worker_id: &WorkerId,
        _sequence: u64,
    ) -> Result<(), RuntimeError> {
        Ok(())
    }

    #[cfg(not(feature = "fs-store"))]
    fn persist_workers(&self) -> Result<(), RuntimeError> {
        Ok(())
    }

    fn ensure_running(&self) -> Result<(), RuntimeError> {
        if self.status == RuntimeStatus::Stopped {
            Err(RuntimeError::RuntimeStopped {
                runtime_id: self.runtime_id.clone(),
            })
        } else {
            Ok(())
        }
    }

    fn check_config_bundle_ref(
        &self,
        reference: &ConfigBundleRef,
    ) -> Result<ConfigBundleAvailability, RuntimeError> {
        validate_config_bundle_ref(reference)?;
        let bundle = self.config_bundles.get(&reference.id).ok_or_else(|| {
            RuntimeError::ConfigBundleMissing {
                bundle_id: reference.id.clone(),
            }
        })?;
        if bundle.metadata.digest != reference.digest {
            return Err(RuntimeError::ConfigBundleDigestMismatch {
                bundle_id: reference.id.clone(),
                expected_digest: reference.digest.clone(),
                actual_digest: bundle.metadata.digest.clone(),
            });
        }
        Ok(ConfigBundleAvailability {
            reference: reference.clone(),
            summary: bundle.summary(),
        })
    }

    fn validate_worker_config_boundary(
        &self,
        _request: &CreateWorkerRequest,
    ) -> Result<(), RuntimeError> {
        Ok(())
    }

    fn ensure_worker_ref(&self, worker_ref: &WorkerRef) -> Result<(), RuntimeError> {
        if worker_ref.runtime_id != self.runtime_id {
            return Err(RuntimeError::WrongRuntime {
                expected_runtime_id: self.runtime_id.clone(),
                actual_runtime_id: worker_ref.runtime_id.clone(),
                worker_id: worker_ref.worker_id.clone(),
            });
        }
        if !self.workers.contains_key(&worker_ref.worker_id) {
            return Err(RuntimeError::WorkerNotFound {
                runtime_id: self.runtime_id.clone(),
                worker_id: worker_ref.worker_id.clone(),
            });
        }
        Ok(())
    }

    fn worker(&self, worker_ref: &WorkerRef) -> Result<&WorkerRecord, RuntimeError> {
        self.ensure_worker_ref(worker_ref)?;
        self.workers
            .get(&worker_ref.worker_id)
            .ok_or_else(|| RuntimeError::WorkerNotFound {
                runtime_id: self.runtime_id.clone(),
                worker_id: worker_ref.worker_id.clone(),
            })
    }

    fn worker_mut(&mut self, worker_ref: &WorkerRef) -> Result<&mut WorkerRecord, RuntimeError> {
        self.ensure_worker_ref(worker_ref)?;
        self.workers
            .get_mut(&worker_ref.worker_id)
            .ok_or_else(|| RuntimeError::WorkerNotFound {
                runtime_id: self.runtime_id.clone(),
                worker_id: worker_ref.worker_id.clone(),
            })
    }

    fn push_event(
        &mut self,
        worker_ref: Option<WorkerRef>,
        kind: RuntimeEventKind,
        message: impl Into<String>,
    ) -> u64 {
        let id = self.next_event_id;
        self.next_event_id += 1;
        self.events.push(RuntimeEvent {
            id,
            worker_ref,
            kind,
            message: message.into(),
        });
        id
    }

    fn last_event_id(&self) -> u64 {
        self.next_event_id.saturating_sub(1)
    }

    #[cfg(feature = "ws-server")]
    fn validate_worker_observation_cursor(
        &self,
        worker_ref: &WorkerRef,
        cursor: WorkerObservationCursor,
    ) -> Result<(), RuntimeError> {
        if let Some(first) = self
            .observation_events
            .iter()
            .find(|event| &event.worker_ref == worker_ref)
        {
            if cursor.sequence != 0 && cursor.sequence < first.sequence {
                return Err(RuntimeError::InvalidRequest(format!(
                    "worker observation cursor {} is expired for worker {}",
                    cursor.encode(),
                    worker_ref.worker_id
                )));
            }
        }
        if cursor.sequence >= self.next_observation_sequence {
            return Err(RuntimeError::InvalidRequest(format!(
                "worker observation cursor {} is unknown for worker {}",
                cursor.encode(),
                worker_ref.worker_id
            )));
        }
        Ok(())
    }

    #[cfg(feature = "ws-server")]
    fn push_worker_observation_event(
        &mut self,
        worker_ref: WorkerRef,
        payload: protocol::Event,
    ) -> WorkerObservationEvent {
        const MAX_OBSERVATION_BACKLOG: usize = 1024;

        let sequence = self.next_observation_sequence;
        self.next_observation_sequence += 1;
        let event = WorkerObservationEvent::new(sequence, worker_ref, payload);
        self.observation_events.push_back(event.clone());
        while self.observation_events.len() > MAX_OBSERVATION_BACKLOG {
            self.observation_events.pop_front();
        }
        let _ = self.observation_tx.send(event.clone());
        event
    }

    #[cfg(feature = "ws-server")]
    fn append_worker_transcript_entry(
        &mut self,
        worker_ref: &WorkerRef,
        role: TranscriptRole,
        content: impl Into<String>,
    ) -> Option<u64> {
        let content = content.into();
        if content.trim().is_empty() {
            return None;
        }
        let event_id = self.last_event_id();
        let worker = self.workers.get_mut(&worker_ref.worker_id)?;
        let sequence = worker.next_transcript_sequence;
        worker.next_transcript_sequence += 1;
        worker.transcript.push(TranscriptEntry {
            sequence,
            worker_ref: worker_ref.clone(),
            role,
            content,
            event_id,
        });
        Some(sequence)
    }

    #[cfg(feature = "ws-server")]
    fn project_protocol_event_to_transcript(
        &mut self,
        worker_ref: &WorkerRef,
        event: &protocol::Event,
    ) -> Option<u64> {
        match event {
            protocol::Event::TextDone { text, .. } => self.append_worker_transcript_entry(
                worker_ref,
                TranscriptRole::Assistant,
                text.clone(),
            ),
            protocol::Event::Error { message, .. } => self.append_worker_transcript_entry(
                worker_ref,
                TranscriptRole::System,
                format!("error: {message}"),
            ),
            protocol::Event::ToolResult {
                id,
                summary,
                is_error,
                ..
            } => self.append_worker_transcript_entry(
                worker_ref,
                TranscriptRole::System,
                format!(
                    "tool result {id}: {}{}",
                    if *is_error { "error: " } else { "" },
                    summary
                ),
            ),
            _ => None,
        }
    }

    #[cfg(feature = "ws-server")]
    fn project_protocol_event_to_execution(
        &mut self,
        worker_ref: &WorkerRef,
        event: &protocol::Event,
    ) -> bool {
        let Some(worker) = self.workers.get_mut(&worker_ref.worker_id) else {
            return false;
        };
        let status = &mut worker.execution;
        let next_run_state = match event {
            protocol::Event::Status {
                status: protocol::WorkerStatus::Running,
            } => Some(WorkerExecutionRunState::Busy),
            protocol::Event::Status {
                status: protocol::WorkerStatus::Idle,
            } => Some(WorkerExecutionRunState::Idle),
            protocol::Event::Status {
                status: protocol::WorkerStatus::Paused,
            } => Some(WorkerExecutionRunState::Busy),
            protocol::Event::Snapshot { status, .. } => match status {
                protocol::WorkerStatus::Running => Some(WorkerExecutionRunState::Busy),
                protocol::WorkerStatus::Idle => Some(WorkerExecutionRunState::Idle),
                protocol::WorkerStatus::Paused => Some(WorkerExecutionRunState::Busy),
            },
            protocol::Event::RunEnd { result } => match result {
                protocol::RunResult::Finished | protocol::RunResult::RolledBack => {
                    Some(WorkerExecutionRunState::Idle)
                }
                protocol::RunResult::Paused => Some(WorkerExecutionRunState::Busy),
                protocol::RunResult::LimitReached => Some(WorkerExecutionRunState::Errored),
            },
            protocol::Event::Error { .. } => Some(WorkerExecutionRunState::Errored),
            _ => None,
        };
        let Some(next_run_state) = next_run_state else {
            return false;
        };
        if status.run_state == next_run_state {
            return false;
        }
        status.run_state = next_run_state;
        true
    }
}

#[derive(Debug)]
struct WorkerRecord {
    worker_ref: WorkerRef,
    worker_id: WorkerId,
    status: WorkerStatus,
    request: CreateWorkerRequest,
    execution: WorkerExecutionStatus,
    execution_handle: Option<WorkerExecutionHandle>,
    transcript: Vec<TranscriptEntry>,
    next_transcript_sequence: u64,
    last_event_id: u64,
}

impl WorkerRecord {
    fn summary(&self, runtime_id: &RuntimeId) -> WorkerSummary {
        WorkerSummary {
            worker_ref: self.worker_ref.clone(),
            runtime_id: runtime_id.clone(),
            worker_id: self.worker_id.clone(),
            status: self.status,
            execution: self.execution.clone(),
            profile: self.request.profile.clone(),
            profile_source: self.request.profile_source.reference(),
            config_bundle: self.request.config_bundle.clone(),
            transcript_len: self.transcript.len(),
            last_event_id: self.last_event_id,
        }
    }

    fn detail(&self, runtime_id: &RuntimeId) -> WorkerDetail {
        WorkerDetail {
            worker_ref: self.worker_ref.clone(),
            runtime_id: runtime_id.clone(),
            worker_id: self.worker_id.clone(),
            status: self.status,
            execution: self.execution.clone(),
            profile: self.request.profile.clone(),
            profile_source: self.request.profile_source.reference(),
            config_bundle: self.request.config_bundle.clone(),
            transcript_len: self.transcript.len(),
            last_event_id: self.last_event_id,
        }
    }

    #[cfg(feature = "fs-store")]
    fn persisted_record(&self) -> PersistedWorkerRecord {
        PersistedWorkerRecord {
            worker_ref: self.worker_ref.clone(),
            worker_id: self.worker_id.clone(),
            status: self.status,
            request: self.request.clone(),
            execution: self.execution.clone(),
            transcript: self.transcript.clone(),
            next_transcript_sequence: self.next_transcript_sequence,
            last_event_id: self.last_event_id,
        }
    }
}

fn validate_create_worker_request(request: &CreateWorkerRequest) -> Result<(), RuntimeError> {
    match &request.profile_source {
        crate::catalog::ProfileSourceArchiveSource::Embedded { archive } => {
            archive.verify().map_err(|err| {
                RuntimeError::InvalidRequest(format!("profile_source archive is invalid: {err}"))
            })?;
        }
        crate::catalog::ProfileSourceArchiveSource::Http { location } => {
            if location.url.trim().is_empty() {
                return Err(RuntimeError::InvalidRequest(
                    "profile_source.location.url must not be empty".to_string(),
                ));
            }
            if location.archive.digest.trim().is_empty() {
                return Err(RuntimeError::InvalidRequest(
                    "profile_source.location.archive.digest must not be empty".to_string(),
                ));
            }
        }
    }
    if let Some(input) = &request.initial_input {
        if input.kind != WorkerInputKind::User {
            return Err(RuntimeError::InvalidInitialInputKind {
                kind: format!("{:?}", input.kind),
            });
        }
        if input.content.trim().is_empty() {
            return Err(RuntimeError::InvalidRequest(
                "initial_input.content must not be empty".to_string(),
            ));
        }
    }
    Ok(())
}

fn validate_worker_input(input: &WorkerInput) -> Result<(), RuntimeError> {
    if input.content.trim().is_empty() {
        return Err(RuntimeError::InvalidRequest(
            "worker input content must not be empty".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{ConfigBundleRef, ProfileSelector};
    use crate::config_bundle::{
        ConfigBundle, ConfigBundleMetadata, ConfigBundleProvenance, ConfigDeclaration,
        ConfigDeclarationKind, ConfigProfileDescriptor,
    };
    use crate::execution::{
        WorkerExecutionBackend, WorkerExecutionContext, WorkerExecutionHandle,
        WorkerExecutionRunState,
    };
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    fn task_request(_objective: &str) -> CreateWorkerRequest {
        let profile = ProfileSelector::Builtin("builtin:coder".to_string());
        let bundle = test_bundle_for_profile(profile.clone());
        CreateWorkerRequest {
            profile,
            profile_source: crate::catalog::ProfileSourceArchiveSource::Http {
                location: crate::catalog::ProfileSourceArchiveHttpRef {
                    url: "http://127.0.0.1/profile-source.tar".to_string(),
                    etag: None,
                    archive: crate::profile_archive::ProfileSourceArchiveRef {
                        id: "test-profile-source".to_string(),
                        digest: "test-digest".to_string(),
                        size_bytes: 0,
                        source_graph: crate::profile_archive::ProfileSourceGraphSummary {
                            source_count: 0,
                            total_source_bytes: 0,
                            entrypoints: BTreeMap::new(),
                            import_count: 0,
                        },
                    },
                },
            },
            config_bundle: Some(ConfigBundleRef {
                id: bundle.metadata.id,
                digest: bundle.metadata.digest,
            }),
            initial_input: None,
            working_directory_request: None,
            working_directory: None,
        }
    }

    fn test_bundle_for_profile(profile: ProfileSelector) -> ConfigBundle {
        ConfigBundle {
            metadata: ConfigBundleMetadata {
                id: "bundle-1".to_string(),
                digest: String::new(),
                revision: "rev-1".to_string(),
                workspace_id: "workspace-1".to_string(),
                created_at: "2026-06-26T00:00:00Z".to_string(),
                provenance: ConfigBundleProvenance {
                    source: "workspace-backend".to_string(),
                    detail: Some("profile-sync".to_string()),
                },
            },
            profiles: vec![ConfigProfileDescriptor {
                selector: profile,
                label: Some("Coder".to_string()),
            }],
            declarations: vec![ConfigDeclaration {
                kind: ConfigDeclarationKind::CapabilityGrant,
                name: "read".to_string(),
                reference: "capability:read".to_string(),
            }],
            profile_source_archive: None,
            profile_source_archive_handle: None,
        }
        .with_computed_digest()
    }

    #[derive(Default)]
    struct TestExecutionBackend {
        dispatch_result: Mutex<Option<WorkerExecutionResult>>,
        contexts: Mutex<BTreeMap<WorkerId, WorkerExecutionContext>>,
    }

    impl TestExecutionBackend {
        fn set_dispatch_result(&self, result: WorkerExecutionResult) {
            *self.dispatch_result.lock().unwrap() = Some(result);
        }

        #[cfg(feature = "ws-server")]
        fn publish_text_delta(
            &self,
            worker_ref: &WorkerRef,
            text: &str,
        ) -> Result<crate::observation::WorkerObservationEvent, RuntimeError> {
            let contexts = self.contexts.lock().unwrap();
            let context = contexts.get(&worker_ref.worker_id).expect("context stored");
            context.publish_protocol_event(protocol::Event::TextDelta { text: text.into() })
        }
    }

    impl WorkerExecutionBackend for TestExecutionBackend {
        fn backend_id(&self) -> &str {
            "test-execution-backend"
        }

        fn spawn_worker(&self, request: WorkerExecutionSpawnRequest) -> WorkerExecutionSpawnResult {
            self.contexts
                .lock()
                .unwrap()
                .insert(request.worker_ref.worker_id.clone(), request.context);
            WorkerExecutionSpawnResult::Connected {
                handle: WorkerExecutionHandle::new(request.worker_ref, self.backend_id()),
                run_state: WorkerExecutionRunState::Idle,
                working_directory: request
                    .working_directory
                    .as_ref()
                    .map(|binding| binding.status()),
            }
        }

        fn dispatch_input(
            &self,
            _handle: &WorkerExecutionHandle,
            _input: WorkerInput,
        ) -> WorkerExecutionResult {
            self.dispatch_result
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| {
                    WorkerExecutionResult::accepted(
                        WorkerExecutionOperation::Input,
                        WorkerExecutionRunState::Idle,
                    )
                })
        }

        fn stop_worker(&self, _handle: &WorkerExecutionHandle) -> WorkerExecutionResult {
            WorkerExecutionResult::accepted(
                WorkerExecutionOperation::Stop,
                WorkerExecutionRunState::Stopped,
            )
        }

        fn cancel_worker(&self, _handle: &WorkerExecutionHandle) -> WorkerExecutionResult {
            WorkerExecutionResult::accepted(
                WorkerExecutionOperation::Cancel,
                WorkerExecutionRunState::Stopped,
            )
        }
    }

    fn runtime_with_backend() -> Runtime {
        let runtime = Runtime::with_execution_backend(
            RuntimeOptions::default(),
            Arc::new(TestExecutionBackend::default()),
        )
        .unwrap();
        runtime.store_config_bundle(test_bundle()).unwrap();
        runtime
    }

    fn runtime_and_backend() -> (Runtime, Arc<TestExecutionBackend>) {
        let backend = Arc::new(TestExecutionBackend::default());
        let runtime =
            Runtime::with_execution_backend(RuntimeOptions::default(), backend.clone()).unwrap();
        runtime.store_config_bundle(test_bundle()).unwrap();
        (runtime, backend)
    }

    fn test_bundle() -> ConfigBundle {
        test_bundle_for_profile(ProfileSelector::Builtin("builtin:coder".to_string()))
    }

    fn bundled_task_request(objective: &str, bundle: &ConfigBundle) -> CreateWorkerRequest {
        let mut request = task_request(objective);
        request.config_bundle = Some(ConfigBundleRef {
            id: bundle.metadata.id.clone(),
            digest: bundle.metadata.digest.clone(),
        });
        request
    }

    #[test]
    fn create_list_and_detail_preserve_runtime_worker_authority() {
        let runtime = runtime_with_backend();
        let detail = runtime.create_worker(task_request("implement v0")).unwrap();

        assert_eq!(detail.worker_ref.runtime_id, runtime.runtime_id().unwrap());
        assert_eq!(detail.status, WorkerStatus::Running);
        assert_eq!(detail.config_bundle.as_ref().unwrap().id, "bundle-1");

        let list = runtime.list_workers().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].worker_ref, detail.worker_ref);
        assert_eq!(list[0].config_bundle, detail.config_bundle);

        let fetched = runtime.worker_detail(&detail.worker_ref).unwrap();
        assert_eq!(fetched.worker_id, detail.worker_id);
        assert_eq!(fetched.profile, detail.profile);
    }

    #[test]
    fn synced_config_bundle_is_stored_checked_and_used_for_worker_creation() {
        let runtime = runtime_with_backend();
        let bundle = test_bundle();
        let availability = runtime.store_config_bundle(bundle.clone()).unwrap();
        assert_eq!(availability.reference.id, "bundle-1");
        assert_eq!(availability.reference.digest, bundle.metadata.digest);

        let listed = runtime.list_config_bundles().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "bundle-1");

        let checked = runtime
            .check_config_bundle(&availability.reference)
            .unwrap();
        assert_eq!(checked.summary.digest, availability.summary.digest);

        let detail = runtime
            .create_worker(bundled_task_request("synced", &bundle))
            .unwrap();
        assert_eq!(detail.config_bundle, Some(availability.reference));
    }

    #[test]
    fn config_bundle_errors_are_typed() {
        let runtime = Runtime::new_memory();
        let bundle = test_bundle();

        runtime.store_config_bundle(bundle.clone()).unwrap();
        let mismatch = runtime
            .check_config_bundle(&ConfigBundleRef {
                id: bundle.metadata.id.clone(),
                digest: "0".repeat(64),
            })
            .unwrap_err();
        assert!(matches!(
            mismatch,
            RuntimeError::ConfigBundleDigestMismatch { .. }
        ));

        let mut unsupported = test_bundle();
        unsupported.declarations.push(ConfigDeclaration {
            kind: ConfigDeclarationKind::Unsupported,
            name: "plugin-registry".to_string(),
            reference: "plugin-registry:v0".to_string(),
        });
        unsupported = unsupported.with_computed_digest();
        let unsupported_err = runtime.store_config_bundle(unsupported).unwrap_err();
        assert!(matches!(
            unsupported_err,
            RuntimeError::UnsupportedConfigDeclaration { .. }
        ));
    }

    #[test]
    fn rejects_worker_refs_from_another_runtime() {
        let runtime_a = runtime_with_backend();
        let runtime_b = runtime_with_backend();
        let detail = runtime_a.create_worker(task_request("runtime a")).unwrap();

        let err = runtime_b.worker_detail(&detail.worker_ref).unwrap_err();
        assert!(matches!(err, RuntimeError::WrongRuntime { .. }));
    }

    #[test]
    fn create_worker_rejects_system_initial_input_without_persisting_worker() {
        let runtime = runtime_with_backend();
        let mut request = task_request("system initial input");
        request.initial_input = Some(WorkerInput::system("role/system belongs in config bundle"));

        let error = runtime.create_worker(request).unwrap_err();
        assert!(matches!(
            error,
            RuntimeError::InvalidInitialInputKind { .. }
        ));
        assert!(runtime.list_workers().unwrap().is_empty());
        let events = runtime
            .read_events(&runtime.event_cursor_from_start().unwrap(), 16)
            .unwrap();
        assert!(
            events
                .events
                .iter()
                .all(|event| event.kind != RuntimeEventKind::WorkerCreated)
        );
    }

    #[test]
    fn create_worker_without_execution_backend_is_rejected_and_not_persisted() {
        let runtime = Runtime::new_memory();
        runtime.store_config_bundle(test_bundle()).unwrap();
        let error = runtime
            .create_worker(task_request("no backend"))
            .unwrap_err();
        assert!(matches!(
            error,
            RuntimeError::ExecutionBackendUnavailable { .. }
        ));
        assert!(runtime.list_workers().unwrap().is_empty());
    }

    #[test]
    fn create_worker_without_execution_backend_is_rejected_before_persisting_worker() {
        let runtime = Runtime::new_memory();
        let error = runtime
            .create_worker(task_request("missing backend"))
            .unwrap_err();
        assert!(matches!(
            error,
            RuntimeError::ExecutionBackendUnavailable { .. }
        ));
        assert!(runtime.list_workers().unwrap().is_empty());
    }

    #[test]
    fn connected_backend_busy_dispatch_is_typed_and_not_transcribed() {
        let (runtime, backend) = runtime_and_backend();
        backend.set_dispatch_result(WorkerExecutionResult::busy(
            WorkerExecutionOperation::Input,
            "worker is already running",
        ));
        let detail = runtime.create_worker(task_request("busy")).unwrap();

        let err = runtime
            .send_input(&detail.worker_ref, WorkerInput::user("wait"))
            .unwrap_err();
        assert!(matches!(
            err,
            RuntimeError::WorkerExecutionRejected {
                outcome: crate::execution::WorkerExecutionOutcome::Busy,
                ..
            }
        ));
        let refreshed = runtime.worker_detail(&detail.worker_ref).unwrap();
        assert_eq!(refreshed.execution.run_state, WorkerExecutionRunState::Busy);
        assert_eq!(
            runtime
                .transcript_projection(&detail.worker_ref, TranscriptQuery::new(0, 1))
                .unwrap()
                .total_items,
            0
        );
    }

    #[cfg(feature = "ws-server")]
    #[test]
    fn backend_protocol_publish_hook_writes_observation_bus() {
        let (runtime, backend) = runtime_and_backend();
        let detail = runtime.create_worker(task_request("observe")).unwrap();

        let observation = backend
            .publish_text_delta(&detail.worker_ref, "from backend")
            .unwrap();
        assert_eq!(observation.worker_ref, detail.worker_ref);

        let observations = runtime
            .read_worker_observation_events(&detail.worker_ref, WorkerObservationCursor::zero())
            .unwrap();
        assert_eq!(observations.len(), 1);
        assert!(matches!(
            observations[0].payload,
            protocol::Event::TextDelta { .. }
        ));
    }

    struct InputOnlyBackend;

    impl WorkerExecutionBackend for InputOnlyBackend {
        fn backend_id(&self) -> &str {
            "input-only"
        }

        fn spawn_worker(&self, request: WorkerExecutionSpawnRequest) -> WorkerExecutionSpawnResult {
            WorkerExecutionSpawnResult::Connected {
                handle: WorkerExecutionHandle::new(request.worker_ref, self.backend_id()),
                run_state: WorkerExecutionRunState::Idle,
                working_directory: request
                    .working_directory
                    .as_ref()
                    .map(|binding| binding.status()),
            }
        }

        fn dispatch_input(
            &self,
            _handle: &WorkerExecutionHandle,
            _input: WorkerInput,
        ) -> WorkerExecutionResult {
            WorkerExecutionResult::accepted(
                WorkerExecutionOperation::Input,
                WorkerExecutionRunState::Idle,
            )
        }
    }

    #[test]
    fn connected_backend_stop_unsupported_is_typed_rejection() {
        let runtime =
            Runtime::with_execution_backend(RuntimeOptions::default(), Arc::new(InputOnlyBackend))
                .unwrap();
        runtime.store_config_bundle(test_bundle()).unwrap();
        let detail = runtime.create_worker(task_request("no stop")).unwrap();

        let err = runtime
            .stop_worker(&detail.worker_ref, Some("stop".to_string()))
            .unwrap_err();
        assert!(matches!(
            err,
            RuntimeError::WorkerExecutionRejected {
                outcome: crate::execution::WorkerExecutionOutcome::Unsupported,
                ..
            }
        ));
        assert_eq!(
            runtime.worker_detail(&detail.worker_ref).unwrap().status,
            WorkerStatus::Running
        );
    }

    #[test]
    fn send_input_and_project_bounded_transcript() {
        let runtime = Runtime::with_execution_backend(
            RuntimeOptions {
                limits: RuntimeLimits {
                    max_transcript_projection_items: 2,
                    max_event_batch_items: 16,
                },
                ..RuntimeOptions::default()
            },
            Arc::new(TestExecutionBackend::default()),
        )
        .unwrap();
        runtime.store_config_bundle(test_bundle()).unwrap();
        let detail = runtime.create_worker(task_request("chat")).unwrap();

        let first = runtime
            .send_input(&detail.worker_ref, WorkerInput::user("hello"))
            .unwrap();
        assert_eq!(first.transcript_sequence, 1);
        runtime
            .send_input(&detail.worker_ref, WorkerInput::system("note"))
            .unwrap();
        runtime
            .send_input(&detail.worker_ref, WorkerInput::user("again"))
            .unwrap();

        let projection = runtime
            .transcript_projection(&detail.worker_ref, TranscriptQuery::new(0, 2))
            .unwrap();
        assert_eq!(projection.total_items, 3);
        assert_eq!(projection.items.len(), 2);
        assert_eq!(projection.items[0].content, "hello");
        assert_eq!(projection.items[1].role, TranscriptRole::System);
        assert_eq!(projection.next_start, Some(2));

        let err = runtime
            .transcript_projection(&detail.worker_ref, TranscriptQuery::new(0, 3))
            .unwrap_err();
        assert!(matches!(err, RuntimeError::LimitTooLarge { .. }));
    }

    #[test]
    fn stop_and_cancel_workers_update_summary() {
        let runtime = runtime_with_backend();
        let stopped = runtime.create_worker(task_request("stop me")).unwrap();
        let cancelled = runtime.create_worker(task_request("cancel me")).unwrap();

        let stop_ack = runtime
            .stop_worker(&stopped.worker_ref, Some("done".to_string()))
            .unwrap();
        assert_eq!(stop_ack.status, WorkerStatus::Stopped);

        let cancel_ack = runtime
            .cancel_worker(&cancelled.worker_ref, Some("abort".to_string()))
            .unwrap();
        assert_eq!(cancel_ack.status, WorkerStatus::Cancelled);

        let summary = runtime.summary().unwrap();
        assert_eq!(summary.worker_count, 2);
        assert_eq!(summary.active_worker_count, 0);
        assert_eq!(summary.stopped_worker_count, 1);
        assert_eq!(summary.cancelled_worker_count, 1);
    }

    #[test]
    fn stop_then_cancel_preserves_stopped_terminal_state() {
        let runtime = runtime_with_backend();
        let cursor = runtime.event_cursor_from_start().unwrap();
        let worker = runtime
            .create_worker(task_request("stable stopped"))
            .unwrap();

        let stop_ack = runtime
            .stop_worker(&worker.worker_ref, Some("done".to_string()))
            .unwrap();
        let cancel_ack = runtime
            .cancel_worker(&worker.worker_ref, Some("late cancel".to_string()))
            .unwrap();

        assert_eq!(stop_ack.status, WorkerStatus::Stopped);
        assert_eq!(cancel_ack.status, WorkerStatus::Stopped);
        assert_eq!(cancel_ack.event_id, stop_ack.event_id);
        assert_eq!(
            runtime.worker_detail(&worker.worker_ref).unwrap().status,
            WorkerStatus::Stopped
        );

        let summary = runtime.summary().unwrap();
        assert_eq!(summary.active_worker_count, 0);
        assert_eq!(summary.stopped_worker_count, 1);
        assert_eq!(summary.cancelled_worker_count, 0);

        let events = runtime.read_events(&cursor, 10).unwrap().events;
        assert_eq!(
            events
                .iter()
                .filter(|event| event.kind == RuntimeEventKind::WorkerStopped)
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.kind == RuntimeEventKind::WorkerCancelled)
                .count(),
            0
        );
    }

    #[test]
    fn cancel_then_stop_preserves_cancelled_terminal_state() {
        let runtime = runtime_with_backend();
        let cursor = runtime.event_cursor_from_start().unwrap();
        let worker = runtime
            .create_worker(task_request("stable cancelled"))
            .unwrap();

        let cancel_ack = runtime
            .cancel_worker(&worker.worker_ref, Some("abort".to_string()))
            .unwrap();
        let stop_ack = runtime
            .stop_worker(&worker.worker_ref, Some("late stop".to_string()))
            .unwrap();

        assert_eq!(cancel_ack.status, WorkerStatus::Cancelled);
        assert_eq!(stop_ack.status, WorkerStatus::Cancelled);
        assert_eq!(stop_ack.event_id, cancel_ack.event_id);
        assert_eq!(
            runtime.worker_detail(&worker.worker_ref).unwrap().status,
            WorkerStatus::Cancelled
        );

        let summary = runtime.summary().unwrap();
        assert_eq!(summary.active_worker_count, 0);
        assert_eq!(summary.stopped_worker_count, 0);
        assert_eq!(summary.cancelled_worker_count, 1);

        let events = runtime.read_events(&cursor, 10).unwrap().events;
        assert_eq!(
            events
                .iter()
                .filter(|event| event.kind == RuntimeEventKind::WorkerCancelled)
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.kind == RuntimeEventKind::WorkerStopped)
                .count(),
            0
        );
    }

    #[test]
    fn event_cursor_and_poll_only_subscription_are_bounded_placeholders() {
        let runtime = runtime_with_backend();
        let cursor = runtime.event_cursor_from_start().unwrap();
        let subscription = runtime.subscribe_events(cursor.clone()).unwrap();
        assert_eq!(subscription.mode, EventSubscriptionMode::PollOnly);

        let worker = runtime.create_worker(task_request("events")).unwrap();
        runtime
            .send_input(&worker.worker_ref, WorkerInput::user("eventful"))
            .unwrap();

        let batch = runtime.read_events(&cursor, 2).unwrap();
        assert_eq!(batch.events.len(), 2);
        assert!(batch.has_more);
        assert_eq!(batch.events[0].kind, RuntimeEventKind::RuntimeStarted);
        assert_eq!(batch.events[1].kind, RuntimeEventKind::WorkerCreated);

        let next = runtime.read_events(&batch.cursor, 2).unwrap();
        assert_eq!(next.events.len(), 1);
        assert_eq!(next.events[0].kind, RuntimeEventKind::WorkerInputAccepted);
        assert!(!next.has_more);
    }

    #[cfg(feature = "fs-store")]
    static NEXT_FS_TEST_ROOT: AtomicU64 = AtomicU64::new(1);

    #[cfg(feature = "fs-store")]
    fn fs_store_root(label: &str) -> std::path::PathBuf {
        let sequence = NEXT_FS_TEST_ROOT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "worker-runtime-fs-store-{label}-{}-{sequence}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        root
    }

    #[cfg(feature = "fs-store")]
    fn runtime_store(runtime: &Runtime) -> FsRuntimeStore {
        let state = runtime.lock().unwrap();
        match &state.persistence {
            RuntimePersistence::Fs(store) => store.clone(),
            RuntimePersistence::Memory => panic!("expected fs-backed runtime"),
        }
    }

    #[cfg(feature = "fs-store")]
    #[test]
    fn fs_store_restores_workers_events_and_transcripts() {
        let root = fs_store_root("restore");
        let runtime_id = RuntimeId::new("runtime-fs-authority").unwrap();
        let runtime = Runtime::with_fs_store_and_execution_backend(
            crate::fs_store::FsRuntimeStoreOptions {
                root: root.clone(),
                runtime_id: Some(runtime_id.clone()),
                display_name: Some("filesystem runtime".to_string()),
                limits: RuntimeLimits {
                    max_transcript_projection_items: 2,
                    max_event_batch_items: 2,
                },
            },
            Arc::new(TestExecutionBackend::default()),
        )
        .unwrap();
        assert_eq!(
            runtime.summary().unwrap().backend,
            RuntimeBackendKind::FsStore
        );
        runtime.store_config_bundle(test_bundle()).unwrap();

        let worker = runtime.create_worker(task_request("persist me")).unwrap();
        runtime
            .send_input(&worker.worker_ref, WorkerInput::user("first"))
            .unwrap();
        runtime
            .send_input(&worker.worker_ref, WorkerInput::system("second"))
            .unwrap();
        runtime
            .stop_worker(&worker.worker_ref, Some("finished".to_string()))
            .unwrap();
        let store = runtime_store(&runtime);
        drop(runtime);

        let restored = Runtime::with_fs_store(crate::fs_store::FsRuntimeStoreOptions {
            root: root.clone(),
            runtime_id: Some(runtime_id.clone()),
            display_name: None,
            limits: RuntimeLimits::default(),
        })
        .unwrap();
        let restored_worker = restored.worker_detail(&worker.worker_ref).unwrap();
        assert_eq!(restored_worker.status, WorkerStatus::Stopped);
        assert_eq!(
            restored_worker.execution.backend,
            WorkerExecutionBackendKind::Stale
        );
        assert_eq!(
            restored_worker
                .execution
                .binding
                .as_ref()
                .map(|binding| binding.backend_id.as_str()),
            Some("test-execution-backend")
        );
        assert!(
            restored
                .diagnostics()
                .unwrap()
                .iter()
                .any(
                    |diagnostic| diagnostic.code == "worker_execution_mapping_stale"
                        && diagnostic.worker_ref.as_ref() == Some(&worker.worker_ref)
                )
        );
        assert_eq!(restored_worker.transcript_len, 2);

        let projection = restored
            .transcript_projection(&worker.worker_ref, TranscriptQuery::new(0, 1))
            .unwrap();
        assert_eq!(projection.total_items, 2);
        assert_eq!(projection.items[0].content, "first");
        assert_eq!(projection.next_start, Some(1));

        let cursor = restored.event_cursor_from_start().unwrap();
        let batch = restored.read_events(&cursor, 2).unwrap();
        assert_eq!(batch.events.len(), 2);
        assert!(batch.has_more);
        assert_eq!(batch.events[0].kind, RuntimeEventKind::RuntimeStarted);
        assert_eq!(batch.events[1].kind, RuntimeEventKind::WorkerCreated);

        let direct_events = store.read_events(&cursor, 2, 2).unwrap();
        assert_eq!(direct_events.events, batch.events);
        let direct_transcript = store
            .read_transcript(&worker.worker_ref, TranscriptQuery::new(1, 1), 2)
            .unwrap();
        assert_eq!(direct_transcript.items[0].content, "second");

        #[cfg(feature = "ws-server")]
        {
            let observation = restored
                .observe_worker_event(
                    &worker.worker_ref,
                    protocol::Event::TextDelta {
                        text: "restored observation bus".to_string(),
                    },
                )
                .unwrap();
            assert_eq!(observation.sequence, 1);
            let observations = restored
                .read_worker_observation_events(&worker.worker_ref, WorkerObservationCursor::zero())
                .unwrap();
            assert_eq!(observations.len(), 1);
            assert_eq!(observations[0].cursor, observation.cursor);
        }

        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(feature = "fs-store")]
    #[test]
    fn fs_store_reports_corrupt_and_missing_data() {
        let corrupt_root = fs_store_root("corrupt");
        let corrupt_runtime_id = RuntimeId::new("runtime-corrupt").unwrap();
        let corrupt_runtime = Runtime::with_fs_store(crate::fs_store::FsRuntimeStoreOptions {
            root: corrupt_root.clone(),
            runtime_id: Some(corrupt_runtime_id.clone()),
            display_name: None,
            limits: RuntimeLimits::default(),
        })
        .unwrap();
        let corrupt_store = runtime_store(&corrupt_runtime);
        std::fs::write(
            corrupt_store.runtime_dir().join("runtime.json"),
            b"not json",
        )
        .unwrap();
        drop(corrupt_runtime);
        let err = Runtime::with_fs_store(crate::fs_store::FsRuntimeStoreOptions {
            root: corrupt_root.clone(),
            runtime_id: Some(corrupt_runtime_id),
            display_name: None,
            limits: RuntimeLimits::default(),
        })
        .unwrap_err();
        assert!(matches!(err, RuntimeError::StoreCorrupt { .. }));
        let _ = std::fs::remove_dir_all(corrupt_root);

        let missing_root = fs_store_root("missing");
        let missing_runtime_id = RuntimeId::new("runtime-missing").unwrap();
        let missing_runtime = Runtime::with_fs_store_and_execution_backend(
            crate::fs_store::FsRuntimeStoreOptions {
                root: missing_root.clone(),
                runtime_id: Some(missing_runtime_id.clone()),
                display_name: None,
                limits: RuntimeLimits::default(),
            },
            Arc::new(TestExecutionBackend::default()),
        )
        .unwrap();
        missing_runtime.store_config_bundle(test_bundle()).unwrap();
        missing_runtime
            .create_worker(task_request("missing worker snapshot"))
            .unwrap();
        let missing_store = runtime_store(&missing_runtime);
        let mut worker_dirs = std::fs::read_dir(missing_store.runtime_dir().join("workers"))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        worker_dirs.sort_by_key(|entry| entry.path());
        std::fs::remove_file(worker_dirs[0].path().join("worker.json")).unwrap();
        drop(missing_runtime);
        let err = Runtime::with_fs_store(crate::fs_store::FsRuntimeStoreOptions {
            root: missing_root.clone(),
            runtime_id: Some(missing_runtime_id),
            display_name: None,
            limits: RuntimeLimits::default(),
        })
        .unwrap_err();
        assert!(matches!(err, RuntimeError::StoreMissing { .. }));
        let _ = std::fs::remove_dir_all(missing_root);
    }
}
