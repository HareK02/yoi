use crate::catalog::{
    ConfigBundleRef, CreateWorkerRequest, ProfileSelector, WorkerDetail, WorkerLifecycleAck,
    WorkerStatus, WorkerSummary, WorkingDirectoryRequest,
    WorkingDirectoryStatus as CatalogWorkingDirectoryStatus, WorkspaceApiRef,
};
use crate::config_bundle::{
    ConfigBundle, ConfigBundleAvailability, ConfigBundleSummary, validate_config_bundle,
    validate_config_bundle_ref,
};
use crate::diagnostics::{DiagnosticSeverity, RuntimeDiagnostic};
use crate::error::RuntimeError;
use crate::execution::WorkerExecutionRestoreRequest;
use crate::execution::{
    WorkerExecutionBackend, WorkerExecutionBackendRef, WorkerExecutionHandle,
    WorkerExecutionOperation, WorkerExecutionResult, WorkerExecutionRunState,
    WorkerExecutionSpawnRequest, WorkerExecutionSpawnResult,
};
#[cfg(feature = "fs-store")]
use crate::fs_store::{
    FsRuntimeStore, FsRuntimeStoreOptions, PersistedRuntimeState, PersistedWorkerRecord,
};
use crate::identity::{WorkerId, WorkerRef};
use crate::interaction::{WorkerInput, WorkerInputKind, WorkerInteractionAck};
use crate::management::{
    RuntimeBackendKind, RuntimeOptions, RuntimeStatus, RuntimeSummary, WorkerDeleteResult,
};
#[cfg(feature = "ws-server")]
use crate::observation::{WorkerObservationCursor, WorkerObservationEvent};
use protocol::subscription::{
    EventSubscriptionSelector, SubscriptionEventPayload, SubscriptionSnapshot,
    SubscriptionValidationError, SubscriptionWorkdirId, SubscriptionWorker, SubscriptionWorkerId,
    SubscriptionWorkerState,
};
use protocol::{Event, Method};
use std::collections::BTreeMap;
#[cfg(feature = "ws-server")]
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
#[cfg(feature = "ws-server")]
use tokio::sync::broadcast;
use tokio::sync::mpsc;

/// Workspace-scoped Runtime authorization context supplied by a trusted backend.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeWorkspaceScope {
    pub workspace_id: String,
    pub server_id: String,
}

impl RuntimeWorkspaceScope {
    pub fn new(workspace_id: impl Into<String>, server_id: impl Into<String>) -> Self {
        Self {
            workspace_id: workspace_id.into(),
            server_id: server_id.into(),
        }
    }
}

const SUBSCRIPTION_QUEUE_CAPACITY: usize = 256;

#[derive(Clone, Debug)]
pub struct RuntimeSubscriptionUpdate {
    pub subject_revision: u64,
    pub payload: SubscriptionEventPayload,
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeSubscriptionRecvError {
    #[error("Runtime event subscription lagged and requires a fresh snapshot")]
    Lagged,
    #[error("Runtime event subscription closed")]
    Closed,
}

/// A gap-free snapshot/live subscription owned by one Runtime connection.
/// Dropping the subscription removes its bounded producer queue.
pub struct RuntimeEventSelectorSubscription {
    subscription_id: u64,
    selector: EventSubscriptionSelector,
    snapshot_revision: u64,
    snapshot: SubscriptionSnapshot,
    receiver: mpsc::Receiver<RuntimeSubscriptionUpdate>,
    lagged: Arc<AtomicBool>,
    runtime: Weak<Mutex<RuntimeState>>,
}

impl RuntimeEventSelectorSubscription {
    pub fn selector(&self) -> &EventSubscriptionSelector {
        &self.selector
    }

    pub fn snapshot_revision(&self) -> u64 {
        self.snapshot_revision
    }

    pub fn snapshot(&self) -> &SubscriptionSnapshot {
        &self.snapshot
    }

    pub async fn recv(
        &mut self,
    ) -> Result<RuntimeSubscriptionUpdate, RuntimeSubscriptionRecvError> {
        if self.lagged.load(Ordering::Acquire) {
            self.receiver.close();
            return Err(RuntimeSubscriptionRecvError::Lagged);
        }
        match self.receiver.recv().await {
            Some(_) if self.lagged.load(Ordering::Acquire) => {
                self.receiver.close();
                Err(RuntimeSubscriptionRecvError::Lagged)
            }
            Some(update) => Ok(update),
            None if self.lagged.load(Ordering::Acquire) => {
                Err(RuntimeSubscriptionRecvError::Lagged)
            }
            None => Err(RuntimeSubscriptionRecvError::Closed),
        }
    }
}

impl Drop for RuntimeEventSelectorSubscription {
    fn drop(&mut self) {
        let Some(runtime) = self.runtime.upgrade() else {
            return;
        };
        if let Ok(mut state) = runtime.lock() {
            state.subscriptions.remove(&self.subscription_id);
        }
    }
}

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
    /// Create a memory-backed Runtime with generated identity.
    pub fn new_memory() -> Self {
        Self::with_options(RuntimeOptions::default())
    }

    /// Create a memory-backed Runtime with explicit options.
    pub fn with_options(options: RuntimeOptions) -> Self {
        let state = RuntimeState::new(options.display_name);
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
    /// The store is scoped by `options.root`; if the directory already exists,
    /// persisted state is loaded and validated. If it does not exist, a fresh
    /// Runtime is initialized and durable files are created before return.
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
        let opened = FsRuntimeStore::open_or_create(options.root)?;
        let mut state = if let Some(persisted) = opened.state {
            RuntimeState::from_persisted(persisted, opened.store)?
        } else {
            let state = RuntimeState::new_fs_backed(options.display_name, opened.store);
            state.persist_runtime_snapshot()?;
            state
        };
        state.execution_backend = execution_backend;
        let runtime = Self {
            inner: Arc::new(Mutex::new(state)),
        };
        runtime.restore_persisted_worker_executions()?;
        Ok(runtime)
    }

    /// Management-plane summary.
    pub fn summary(&self) -> Result<RuntimeSummary, RuntimeError> {
        let state = self.lock()?;
        let mut active_worker_count = 0;
        let mut stopped_worker_count = 0;
        let mut cancelled_worker_count = 0;
        for worker in state.workers.values() {
            match worker.status {
                WorkerStatus::Idle | WorkerStatus::Running | WorkerStatus::Paused => {
                    active_worker_count += 1;
                }
                WorkerStatus::Stopped => stopped_worker_count += 1,
                WorkerStatus::Cancelled => cancelled_worker_count += 1,
            }
        }

        Ok(RuntimeSummary {
            display_name: state.display_name.clone(),
            backend: state.backend,
            status: state.status,
            worker_count: state.workers.len(),
            active_worker_count,
            stopped_worker_count,
            cancelled_worker_count,
            diagnostic_count: state.diagnostics.len(),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
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
    pub fn stop_runtime(&self) -> Result<(), RuntimeError> {
        let mut state = self.lock()?;
        if state.status == RuntimeStatus::Stopped {
            return Ok(());
        }
        state.status = RuntimeStatus::Stopped;
        let mut stopped = Vec::new();
        for (worker_id, worker) in &mut state.workers {
            if worker.status.is_active() {
                worker.status = WorkerStatus::Stopped;
                stopped.push(*worker_id);
            }
        }
        for worker_id in stopped {
            state.publish_worker_upsert(worker_id)?;
        }
        state.persist_runtime_snapshot()?;
        state.persist_workers()?;
        Ok(())
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
            .map_err(RuntimeError::from)
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
        let statuses = backend.list_working_directories();
        self.annotate_working_directory_statuses(statuses)
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
        let status = backend
            .working_directory(working_directory_id)
            .map_err(RuntimeError::from)?;
        self.annotate_working_directory_status(status)
    }

    /// Cleanup a Runtime-owned working directory.
    pub fn cleanup_working_directory(
        &self,
        working_directory_id: &str,
    ) -> Result<CatalogWorkingDirectoryStatus, RuntimeError> {
        let backend = {
            let state = self.lock()?;
            state.ensure_running()?;
            if let Some(worker_id) = state.primary_worker_id_for_workdir(working_directory_id) {
                return Err(RuntimeError::InvalidRequest(format!(
                    "working directory {working_directory_id} is assigned to worker {worker_id}"
                )));
            }
            state.execution_backend.clone().ok_or_else(|| {
                RuntimeError::ExecutionBackendUnavailable {
                    message: "working directory cleanup requires an execution backend".to_string(),
                }
            })?
        };
        backend
            .cleanup_working_directory(working_directory_id)
            .map_err(RuntimeError::from)
    }

    fn annotate_working_directory_statuses(
        &self,
        statuses: Vec<CatalogWorkingDirectoryStatus>,
    ) -> Result<Vec<CatalogWorkingDirectoryStatus>, RuntimeError> {
        statuses
            .into_iter()
            .map(|status| self.annotate_working_directory_status(status))
            .collect()
    }

    fn annotate_working_directory_status(
        &self,
        mut status: CatalogWorkingDirectoryStatus,
    ) -> Result<CatalogWorkingDirectoryStatus, RuntimeError> {
        let state = self.lock()?;
        status.summary.primary_worker_id =
            state.primary_worker_id_for_workdir(status.summary.working_directory_id.as_str());
        Ok(status)
    }

    /// Create a Worker through the canonical profile-source + execution backend path.
    pub fn create_worker(
        &self,
        request: CreateWorkerRequest,
    ) -> Result<WorkerDetail, RuntimeError> {
        self.create_worker_with_workspace(request, None)
    }

    /// Create a Worker scoped to a workspace authorization context.
    pub fn create_worker_scoped(
        &self,
        scope: &RuntimeWorkspaceScope,
        request: CreateWorkerRequest,
    ) -> Result<WorkerDetail, RuntimeError> {
        self.create_worker_with_workspace(request, Some(scope))
    }

    fn create_worker_with_workspace(
        &self,
        request: CreateWorkerRequest,
        scope: Option<&RuntimeWorkspaceScope>,
    ) -> Result<WorkerDetail, RuntimeError> {
        if request.idempotency_key.is_some() != request.idempotency_fingerprint.is_some() {
            return Err(RuntimeError::InvalidRequest(
                "idempotency_key and idempotency_fingerprint must be provided together".to_string(),
            ));
        }
        let (backend, worker_ref, spawn_request) = {
            let mut state = self.lock()?;
            state.ensure_running()?;
            validate_create_worker_request(&request)?;
            validate_create_workspace_scope(
                &request,
                scope.map(|scope| scope.workspace_id.as_str()),
            )?;
            if let Some(scope) = scope {
                state.ensure_workspace_owner(scope, true)?;
            };
            if let Some(idempotency_key) = request.idempotency_key.as_deref() {
                let workspace_id = scope.map(|scope| scope.workspace_id.as_str());
                if let Some(existing) = state.workers.values().find(|record| {
                    record.workspace_id.as_deref() == workspace_id
                        && record.request.idempotency_key.as_deref() == Some(idempotency_key)
                }) {
                    if existing.request.idempotency_fingerprint != request.idempotency_fingerprint {
                        return Err(RuntimeError::InvalidRequest(format!(
                            "worker creation idempotency key {idempotency_key} was already used with different input"
                        )));
                    }
                    return Ok(existing.detail());
                }
            }
            state.validate_worker_config_boundary(&request)?;
            if let Some(working_directory_id) = requested_primary_workdir_id(&request) {
                if let Some(owner_worker_id) =
                    state.primary_worker_id_for_workdir(working_directory_id)
                {
                    return Err(RuntimeError::InvalidRequest(format!(
                        "working directory {working_directory_id} is already assigned to worker {owner_worker_id}"
                    )));
                }
            }
            let backend = state.execution_backend.clone().ok_or_else(|| {
                RuntimeError::ExecutionBackendUnavailable {
                    message: "worker creation requires an execution backend".to_string(),
                }
            })?;

            let worker_id = WorkerId::generated(state.next_worker_sequence);
            state.next_worker_sequence += 1;
            let worker_ref = WorkerRef::new(worker_id.clone());

            let record = WorkerRecord {
                worker_ref: worker_ref.clone(),
                worker_id: worker_id.clone(),
                status: WorkerStatus::Stopped,
                workspace_id: scope.map(|scope| scope.workspace_id.clone()),
                request: request.clone(),
                working_directory: None,
                execution_handle: None,
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
            let dispatch_result = backend.dispatch_input(&handle, initial_input.clone());
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
            let detail = self.commit_created_worker(
                &worker_ref,
                handle,
                WorkerExecutionRunState::Busy,
                working_directory,
                WorkerExecutionResult::accepted(
                    WorkerExecutionOperation::Input,
                    WorkerExecutionRunState::Busy,
                ),
            )?;
            self.record_input_observation(&worker_ref, initial_input)?;
            Ok(detail)
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
        Ok(state.workers.values().map(WorkerRecord::summary).collect())
    }

    pub fn subscribe_event_selector(
        &self,
        selector: EventSubscriptionSelector,
    ) -> Result<RuntimeEventSelectorSubscription, RuntimeError> {
        self.subscribe_event_selector_for_workspace(None, selector)
    }

    pub fn subscribe_event_selector_scoped(
        &self,
        scope: &RuntimeWorkspaceScope,
        selector: EventSubscriptionSelector,
    ) -> Result<RuntimeEventSelectorSubscription, RuntimeError> {
        self.subscribe_event_selector_for_workspace(Some(scope), selector)
    }

    fn subscribe_event_selector_for_workspace(
        &self,
        scope: Option<&RuntimeWorkspaceScope>,
        selector: EventSubscriptionSelector,
    ) -> Result<RuntimeEventSelectorSubscription, RuntimeError> {
        selector.validate().map_err(subscription_validation_error)?;
        if matches!(
            selector,
            EventSubscriptionSelector::WorkerProtocol { .. }
                | EventSubscriptionSelector::WorkspaceWorkers
                | EventSubscriptionSelector::WorkspaceWorkdirs
        ) {
            return Err(RuntimeError::InvalidRequest(
                "Runtime event subscriptions support only runtime_workers and worker_lifecycle selectors"
                    .to_string(),
            ));
        }

        let mut state = self.lock()?;
        if let Some(scope) = scope {
            state.ensure_workspace_owner_for_existing_workers(scope)?;
            state.persist_runtime_snapshot()?;
        }
        let snapshot = state.subscription_snapshot(scope, &selector)?;
        let snapshot_revision = state.subscription_revision;
        let subscription_id = state.next_event_subscription_id;
        state.next_event_subscription_id = state.next_event_subscription_id.saturating_add(1);
        let (sender, receiver) = mpsc::channel(SUBSCRIPTION_QUEUE_CAPACITY);
        let lagged = Arc::new(AtomicBool::new(false));
        state.subscriptions.insert(
            subscription_id,
            SubscriptionSink {
                selector: selector.clone(),
                workspace_id: scope.map(|scope| scope.workspace_id.clone()),
                sender,
                lagged: lagged.clone(),
            },
        );

        Ok(RuntimeEventSelectorSubscription {
            subscription_id,
            selector,
            snapshot_revision,
            snapshot,
            receiver,
            lagged,
            runtime: Arc::downgrade(&self.inner),
        })
    }

    /// List stopped Workers known to this Runtime.
    pub fn list_stopped_workers(&self) -> Result<Vec<WorkerSummary>, RuntimeError> {
        let state = self.lock()?;
        Ok(state
            .workers
            .values()
            .filter(|worker| worker.status == WorkerStatus::Stopped)
            .map(WorkerRecord::summary)
            .collect())
    }

    /// List Workers visible to a workspace-scoped Runtime authorization context.
    pub fn list_workers_scoped(
        &self,
        scope: &RuntimeWorkspaceScope,
    ) -> Result<Vec<WorkerSummary>, RuntimeError> {
        let mut state = self.lock()?;
        let visible_workspace = state.ensure_workspace_owner_for_existing_workers(scope)?;
        if !visible_workspace {
            return Ok(Vec::new());
        }
        state.persist_runtime_snapshot()?;
        Ok(state
            .workers
            .values()
            .filter(|worker| worker.belongs_to_workspace(&scope.workspace_id))
            .map(WorkerRecord::summary)
            .collect())
    }

    /// List stopped Workers visible to a workspace-scoped Runtime authorization context.
    pub fn list_stopped_workers_scoped(
        &self,
        scope: &RuntimeWorkspaceScope,
    ) -> Result<Vec<WorkerSummary>, RuntimeError> {
        let mut state = self.lock()?;
        let visible_workspace = state.ensure_workspace_owner_for_existing_workers(scope)?;
        if !visible_workspace {
            return Ok(Vec::new());
        }
        state.persist_runtime_snapshot()?;
        Ok(state
            .workers
            .values()
            .filter(|worker| {
                worker.status == WorkerStatus::Stopped
                    && worker.belongs_to_workspace(&scope.workspace_id)
            })
            .map(WorkerRecord::summary)
            .collect())
    }

    /// Fetch Worker detail through a workspace-scoped Runtime authorization context.
    pub fn worker_detail_scoped(
        &self,
        scope: &RuntimeWorkspaceScope,
        worker_ref: &WorkerRef,
    ) -> Result<WorkerDetail, RuntimeError> {
        let mut state = self.lock()?;
        let worker = state.worker(worker_ref)?;
        if !worker.belongs_to_workspace(&scope.workspace_id) {
            return Err(RuntimeError::WorkerNotFound {
                worker_id: worker_ref.worker_id,
            });
        }
        state.ensure_workspace_owner(scope, true)?;
        state.persist_runtime_snapshot()?;
        Ok(state.worker(worker_ref)?.detail())
    }

    /// Fetch Worker detail.  The supplied [`WorkerRef`] must match this Runtime.
    pub fn worker_detail(&self, worker_ref: &WorkerRef) -> Result<WorkerDetail, RuntimeError> {
        let state = self.lock()?;
        let worker = state.worker(worker_ref)?;
        Ok(worker.detail())
    }

    fn ensure_worker_in_workspace(
        &self,
        scope: &RuntimeWorkspaceScope,
        worker_ref: &WorkerRef,
    ) -> Result<(), RuntimeError> {
        let mut state = self.lock()?;
        let worker = state.worker(worker_ref)?;
        if !worker.belongs_to_workspace(&scope.workspace_id) {
            return Err(RuntimeError::WorkerNotFound {
                worker_id: worker_ref.worker_id,
            });
        }
        state.ensure_workspace_owner(scope, true)?;
        state.persist_runtime_snapshot()?;
        Ok(())
    }

    /// Replace the Workspace API binding persisted for a Worker and update the
    /// live execution when one is connected.
    pub fn replace_worker_workspace_api_scoped(
        &self,
        scope: &RuntimeWorkspaceScope,
        worker_ref: &WorkerRef,
        workspace_api: WorkspaceApiRef,
    ) -> Result<WorkerDetail, RuntimeError> {
        self.ensure_worker_in_workspace(scope, worker_ref)?;
        if workspace_api.workspace_id != scope.workspace_id {
            return Err(RuntimeError::InvalidRequest(format!(
                "Workspace API scope `{}` does not match authorized workspace `{}`",
                workspace_api.workspace_id, scope.workspace_id
            )));
        }
        self.replace_worker_workspace_api(worker_ref, workspace_api)
    }

    pub fn replace_worker_workspace_api(
        &self,
        worker_ref: &WorkerRef,
        workspace_api: WorkspaceApiRef,
    ) -> Result<WorkerDetail, RuntimeError> {
        let access_token = workspace_api
            .access_token
            .as_ref()
            .filter(|token| !token.trim().is_empty())
            .cloned()
            .ok_or_else(|| {
                RuntimeError::InvalidRequest(
                    "Workspace API replacement requires an access token".to_string(),
                )
            })?;
        let (previous_workspace_api, live_execution) = {
            let state = self.lock()?;
            let worker = state.worker(worker_ref)?;
            if let Some(existing) = worker.request.workspace_api.as_ref()
                && (existing.workspace_id != workspace_api.workspace_id
                    || existing.base_url.trim_end_matches('/')
                        != workspace_api.base_url.trim_end_matches('/')
                    || existing.runtime_id.as_ref().is_some_and(|runtime_id| {
                        workspace_api.runtime_id.as_ref() != Some(runtime_id)
                    }))
            {
                return Err(RuntimeError::InvalidRequest(
                    "Workspace API replacement cannot change Worker Workspace identity, Runtime identity, or base URL"
                        .to_string(),
                ));
            }
            let live_execution = match (
                state.execution_backend.clone(),
                worker.execution_handle.clone(),
            ) {
                (Some(backend), Some(handle)) => Some((backend, handle)),
                _ => None,
            };
            (worker.request.workspace_api.clone(), live_execution)
        };

        {
            let mut state = self.lock()?;
            state.worker_mut(worker_ref)?.request.workspace_api = Some(workspace_api);
            if let Err(error) = state.persist_runtime_snapshot() {
                state.worker_mut(worker_ref)?.request.workspace_api = previous_workspace_api;
                return Err(error);
            }
        }

        if let Some((backend, handle)) = live_execution {
            let result = backend.replace_workspace_access_token(&handle, access_token);
            if !result.is_accepted() {
                let mut state = self.lock()?;
                state.worker_mut(worker_ref)?.request.workspace_api = previous_workspace_api;
                state.persist_runtime_snapshot()?;
                return Err(RuntimeError::WorkerExecutionRejected {
                    worker_id: worker_ref.worker_id.clone(),
                    operation: result.operation,
                    outcome: result.outcome,
                    message: result.message_or_default(),
                    result,
                });
            }
        }

        let state = self.lock()?;
        Ok(state.worker(worker_ref)?.detail())
    }

    /// Attach a live execution through a workspace-scoped Runtime authorization context.
    pub fn restore_worker_scoped(
        &self,
        scope: &RuntimeWorkspaceScope,
        worker_ref: &WorkerRef,
    ) -> Result<WorkerDetail, RuntimeError> {
        self.ensure_worker_in_workspace(scope, worker_ref)?;
        self.restore_worker(worker_ref)
    }

    /// Attach a live execution to a persisted Worker definition.
    ///
    /// Current liveness is never read from disk. If a handle is already
    /// present this is idempotent; otherwise the configured backend is tried.
    pub fn restore_worker(&self, worker_ref: &WorkerRef) -> Result<WorkerDetail, RuntimeError> {
        let (backend, request) = {
            let state = self.lock()?;
            state.ensure_running()?;
            let worker = state.worker(worker_ref)?;
            if worker.execution_handle.is_some() {
                return Ok(worker.detail());
            }
            if worker.status == WorkerStatus::Cancelled {
                return Err(RuntimeError::InvalidRequest(format!(
                    "worker {} is cancelled",
                    worker_ref.worker_id
                )));
            }
            let backend = state.execution_backend.clone().ok_or_else(|| {
                RuntimeError::WorkerExecutionUnavailable {
                    worker_id: worker_ref.worker_id.clone(),
                    message: "runtime has no execution backend".to_string(),
                }
            })?;
            let config_bundle = worker
                .request
                .config_bundle
                .as_ref()
                .and_then(|bundle_ref| state.config_bundles.get(&bundle_ref.id))
                .cloned();
            let request = WorkerExecutionRestoreRequest {
                worker_ref: worker_ref.clone(),
                request: worker.request.clone(),
                context: self.execution_context(worker_ref.clone()),
                previous_working_directory: worker.working_directory.clone(),
                working_directory: None,
                config_bundle,
            };
            (backend, request)
        };

        match backend.restore_worker(request) {
            WorkerExecutionSpawnResult::Connected {
                handle,
                run_state,
                working_directory,
            } => {
                self.commit_restored_worker_execution(
                    worker_ref,
                    handle,
                    run_state,
                    working_directory,
                )?;
                self.worker_detail(worker_ref)
            }
            WorkerExecutionSpawnResult::Rejected(result)
            | WorkerExecutionSpawnResult::Errored(result) => {
                #[cfg(feature = "fs-store")]
                {
                    self.lock()?
                        .record_restore_failure(worker_ref, result.clone())?;
                }
                Err(RuntimeError::WorkerExecutionRejected {
                    worker_id: worker_ref.worker_id.clone(),
                    operation: result.operation,
                    outcome: result.outcome,
                    message: result.message_or_default(),
                    result,
                })
            }
        }
    }

    fn ensure_worker_execution(&self, worker_ref: &WorkerRef) -> Result<(), RuntimeError> {
        let has_handle = {
            let state = self.lock()?;
            state
                .worker(worker_ref)?
                .execution_handle
                .as_ref()
                .is_some()
        };
        if has_handle {
            return Ok(());
        }
        self.restore_worker(worker_ref).map(|_| ())
    }

    /// Accept input into a Worker through a workspace-scoped Runtime authorization context.
    pub fn send_input_scoped(
        &self,
        scope: &RuntimeWorkspaceScope,
        worker_ref: &WorkerRef,
        input: WorkerInput,
    ) -> Result<WorkerInteractionAck, RuntimeError> {
        self.ensure_worker_in_workspace(scope, worker_ref)?;
        self.send_input(worker_ref, input)
    }

    /// Accept input into a Worker.
    pub fn send_input(
        &self,
        worker_ref: &WorkerRef,
        input: WorkerInput,
    ) -> Result<WorkerInteractionAck, RuntimeError> {
        validate_worker_input(&input)?;
        self.ensure_worker_execution(worker_ref)?;
        let (backend, handle) = {
            let state = self.lock()?;
            state.ensure_running()?;
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
                    return Err(RuntimeError::WorkerExecutionUnavailable {
                        worker_id: worker_ref.worker_id.clone(),
                        message: "worker has no live execution handle".to_string(),
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
        let worker = state.worker_mut(worker_ref)?;
        worker.status = worker_status_from_run_state(dispatch_result.run_state);
        let status = worker.status;
        #[cfg(feature = "ws-server")]
        {
            let payload = input_protocol_event(&input);
            state.push_worker_observation_event(worker_ref.clone(), payload);
        }
        state.publish_worker_upsert(worker_ref.worker_id)?;
        state.persist_runtime_snapshot()?;
        state.persist_worker(&worker_ref.worker_id)?;

        Ok(WorkerInteractionAck {
            worker_ref: worker_ref.clone(),
            status,
        })
    }

    /// Return live completion entries through a workspace-scoped Runtime authorization context.
    pub fn worker_completions_scoped(
        &self,
        scope: &RuntimeWorkspaceScope,
        worker_ref: &WorkerRef,
        kind: protocol::CompletionKind,
        prefix: &str,
    ) -> Result<Vec<protocol::CompletionEntry>, RuntimeError> {
        self.ensure_worker_in_workspace(scope, worker_ref)?;
        self.worker_completions(worker_ref, kind, prefix)
    }

    /// Return live completion entries for the Worker composer.
    pub fn worker_completions(
        &self,
        worker_ref: &WorkerRef,
        kind: protocol::CompletionKind,
        prefix: &str,
    ) -> Result<Vec<protocol::CompletionEntry>, RuntimeError> {
        let (backend, handle) = {
            let state = self.lock()?;
            state.ensure_worker_ref(worker_ref)?;
            let worker = state.worker(worker_ref)?;
            (
                state.execution_backend.clone(),
                worker.execution_handle.clone(),
            )
        };
        let Some((backend, handle)) = backend.zip(handle) else {
            return Ok(Vec::new());
        };
        Ok(backend.worker_completions(&handle, kind, prefix))
    }

    /// Accept a protocol method through a workspace-scoped Runtime authorization context.
    pub fn send_protocol_method_scoped(
        &self,
        scope: &RuntimeWorkspaceScope,
        worker_ref: &WorkerRef,
        method: Method,
    ) -> Result<Vec<Event>, RuntimeError> {
        self.ensure_worker_in_workspace(scope, worker_ref)?;
        self.send_protocol_method(worker_ref, method)
    }

    /// Accept a protocol method for a Worker through a Backend/runtime transport.
    ///
    /// Most methods are delivered to the execution backend unchanged. Methods with
    /// direct same-connection replies in the local socket protocol return those
    /// events from this function so WebSocket transports can write them back to the
    /// requesting client without rebroadcasting them.
    pub fn send_protocol_method(
        &self,
        worker_ref: &WorkerRef,
        method: Method,
    ) -> Result<Vec<Event>, RuntimeError> {
        if let Method::ListCompletions { kind, prefix } = method {
            let entries = self.worker_completions(worker_ref, kind, &prefix)?;
            return Ok(vec![Event::Completions { kind, entries }]);
        }
        if matches!(&method, Method::Shutdown) {
            self.stop_worker(worker_ref, Some("worker protocol shutdown".to_string()))?;
            return Ok(Vec::new());
        }
        self.ensure_worker_execution(worker_ref)?;

        let (backend, handle) = {
            let state = self.lock()?;
            state.ensure_running()?;
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
                    return Err(RuntimeError::WorkerExecutionUnavailable {
                        worker_id: worker_ref.worker_id.clone(),
                        message: "worker has no live execution handle".to_string(),
                    });
                }
            }
        };

        let dispatch_result = backend.dispatch_method(&handle, method);
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

        self.record_execution_result(worker_ref, dispatch_result)?;
        Ok(Vec::new())
    }

    fn commit_created_worker(
        &self,
        worker_ref: &WorkerRef,
        handle: WorkerExecutionHandle,
        run_state: WorkerExecutionRunState,
        working_directory: Option<CatalogWorkingDirectoryStatus>,
        _result: WorkerExecutionResult,
    ) -> Result<WorkerDetail, RuntimeError> {
        let mut state = self.lock()?;
        let detail = {
            let worker = state.worker_mut(worker_ref)?;
            worker.execution_handle = Some(handle);
            worker.status = worker_status_from_run_state(run_state);
            worker.working_directory = working_directory;
            worker.detail()
        };
        state.publish_worker_upsert(worker_ref.worker_id)?;
        state.persist_runtime_snapshot()?;
        state.persist_worker(&worker_ref.worker_id)?;
        Ok(detail)
    }

    fn rollback_failed_create(&self, worker_ref: &WorkerRef) -> Result<(), RuntimeError> {
        let mut state = self.lock()?;
        if let Some(record) = state.workers.remove(&worker_ref.worker_id) {
            let workspace_id = record.workspace_id.clone();
            if let Some(workspace_id) = workspace_id.as_deref() {
                state.forget_workspace_owner_if_unused(workspace_id);
            }
            state.publish_worker_removed(worker_ref.worker_id, workspace_id.as_deref())?;
        }
        Ok(())
    }

    fn record_execution_result(
        &self,
        worker_ref: &WorkerRef,
        result: WorkerExecutionResult,
    ) -> Result<(), RuntimeError> {
        let mut state = self.lock()?;
        if result.is_accepted() {
            state.worker_mut(worker_ref)?.status = worker_status_from_run_state(result.run_state);
            state.publish_worker_upsert(worker_ref.worker_id)?;
        }
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
            WorkerExecutionOperation::Spawn
            | WorkerExecutionOperation::Restore
            | WorkerExecutionOperation::Input
            | WorkerExecutionOperation::ProtocolMethod
            | WorkerExecutionOperation::ReplaceWorkspaceAccessToken => return Ok(()),
        };
        if result.is_accepted() {
            return Ok(());
        }
        Err(RuntimeError::WorkerExecutionRejected {
            worker_id: worker_ref.worker_id.clone(),
            operation: result.operation,
            outcome: result.outcome,
            message: result.message_or_default(),
            result,
        })
    }

    /// Stop a Worker through a workspace-scoped Runtime authorization context.
    pub fn stop_worker_scoped(
        &self,
        scope: &RuntimeWorkspaceScope,
        worker_ref: &WorkerRef,
        reason: Option<String>,
    ) -> Result<WorkerLifecycleAck, RuntimeError> {
        self.ensure_worker_in_workspace(scope, worker_ref)?;
        self.stop_worker(worker_ref, reason)
    }

    /// Stop a Worker. Repeated stops are idempotent.
    pub fn stop_worker(
        &self,
        worker_ref: &WorkerRef,
        reason: Option<String>,
    ) -> Result<WorkerLifecycleAck, RuntimeError> {
        self.dispatch_lifecycle_to_backend(worker_ref, WorkerExecutionOperation::Stop)?;
        let _ = reason;
        self.transition_worker(worker_ref, WorkerStatus::Stopped)
    }

    /// Cancel a Worker through a workspace-scoped Runtime authorization context.
    pub fn cancel_worker_scoped(
        &self,
        scope: &RuntimeWorkspaceScope,
        worker_ref: &WorkerRef,
        reason: Option<String>,
    ) -> Result<WorkerLifecycleAck, RuntimeError> {
        self.ensure_worker_in_workspace(scope, worker_ref)?;
        self.cancel_worker(worker_ref, reason)
    }

    /// Cancel a Worker. Repeated cancels are idempotent.
    pub fn cancel_worker(
        &self,
        worker_ref: &WorkerRef,
        reason: Option<String>,
    ) -> Result<WorkerLifecycleAck, RuntimeError> {
        self.dispatch_lifecycle_to_backend(worker_ref, WorkerExecutionOperation::Cancel)?;
        let _ = reason;
        self.transition_worker(worker_ref, WorkerStatus::Cancelled)
    }

    /// Delete a non-running Worker through a workspace-scoped Runtime authorization context.
    pub fn delete_worker_scoped(
        &self,
        scope: &RuntimeWorkspaceScope,
        worker_ref: &WorkerRef,
    ) -> Result<WorkerDeleteResult, RuntimeError> {
        self.ensure_worker_in_workspace(scope, worker_ref)?;
        self.delete_worker(worker_ref)
    }

    /// Delete a non-running Worker from Runtime state and persisted Worker storage.
    pub fn delete_worker(
        &self,
        worker_ref: &WorkerRef,
    ) -> Result<WorkerDeleteResult, RuntimeError> {
        let mut state = self.lock()?;
        state.ensure_running()?;
        state.ensure_worker_ref(worker_ref)?;
        let worker = state.worker(worker_ref)?;
        if worker.status.is_active() {
            return Err(RuntimeError::InvalidRequest(format!(
                "worker {} is running and must be stopped before deletion",
                worker_ref.worker_id
            )));
        }
        let removed = state.workers.remove(&worker_ref.worker_id).ok_or_else(|| {
            RuntimeError::WorkerNotFound {
                worker_id: worker_ref.worker_id,
            }
        })?;
        let removed_workspace_id = removed.workspace_id.clone();
        if let Some(workspace_id) = removed_workspace_id.as_deref() {
            state.forget_workspace_owner_if_unused(workspace_id);
        }
        #[cfg(feature = "ws-server")]
        state
            .observation_events
            .retain(|event| event.worker_ref != *worker_ref);
        state.publish_worker_removed(worker_ref.worker_id, removed_workspace_id.as_deref())?;
        state.persist_runtime_snapshot()?;
        state.delete_worker_snapshot(&worker_ref.worker_id)?;
        Ok(WorkerDeleteResult {
            worker_id: removed.worker_id,
            deleted: true,
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
        let (backend, handle) = {
            let state = self.lock()?;
            let worker = state.worker(worker_ref)?;
            (
                state.execution_backend.clone(),
                worker.execution_handle.clone(),
            )
        };
        if let (Some(backend), Some(handle)) = (backend, handle) {
            if let Some(snapshot) = backend.worker_snapshot(&handle) {
                return Ok(snapshot);
            }
        }
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
        let status_changed = state.project_protocol_event_to_status(worker_ref, &payload);
        if status_changed {
            state.publish_worker_upsert(worker_ref.worker_id)?;
        }
        let event = state.push_worker_observation_event(worker_ref.clone(), payload);
        Ok(event)
    }

    /// Snapshot current diagnostics.
    pub fn diagnostics(&self) -> Result<Vec<RuntimeDiagnostic>, RuntimeError> {
        Ok(self.lock()?.diagnostics.clone())
    }

    #[cfg(feature = "ws-server")]
    fn record_input_observation(
        &self,
        worker_ref: &WorkerRef,
        input: WorkerInput,
    ) -> Result<(), RuntimeError> {
        let mut state = self.lock()?;
        state.ensure_worker_ref(worker_ref)?;
        state.push_worker_observation_event(worker_ref.clone(), input_protocol_event(&input));
        Ok(())
    }

    #[cfg(not(feature = "ws-server"))]
    fn record_input_observation(
        &self,
        _worker_ref: &WorkerRef,
        _input: WorkerInput,
    ) -> Result<(), RuntimeError> {
        Ok(())
    }

    fn transition_worker(
        &self,
        worker_ref: &WorkerRef,
        status: WorkerStatus,
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
                });
            }
        }

        let worker = state.worker_mut(worker_ref)?;
        worker.status = status;
        worker.execution_handle = None;
        let status = worker.status;
        state.publish_worker_upsert(worker_ref.worker_id)?;
        state.persist_runtime_snapshot()?;
        state.persist_worker(&worker_ref.worker_id)?;
        Ok(WorkerLifecycleAck {
            worker_ref: worker_ref.clone(),
            status,
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

    #[cfg(feature = "fs-store")]
    fn restore_persisted_worker_executions(&self) -> Result<(), RuntimeError> {
        #[derive(Clone)]
        struct RestoreCandidate {
            worker_ref: WorkerRef,
            request: CreateWorkerRequest,
            previous_working_directory: Option<CatalogWorkingDirectoryStatus>,
            config_bundle: Option<ConfigBundle>,
        }

        let candidates = {
            let state = self.lock()?;
            if state.execution_backend.is_none() {
                return Ok(());
            }
            state
                .workers
                .values()
                .filter(|worker| worker.execution_handle.is_none())
                .map(|worker| {
                    let config_bundle = worker
                        .request
                        .config_bundle
                        .as_ref()
                        .and_then(|bundle_ref| state.config_bundles.get(&bundle_ref.id))
                        .cloned();
                    RestoreCandidate {
                        worker_ref: worker.worker_ref.clone(),
                        request: worker.request.clone(),
                        previous_working_directory: worker.working_directory.clone(),
                        config_bundle,
                    }
                })
                .collect::<Vec<_>>()
        };

        for candidate in candidates {
            let backend = {
                let state = self.lock()?;
                state.execution_backend.clone()
            };
            let Some(backend) = backend else {
                return Ok(());
            };
            let request = WorkerExecutionRestoreRequest {
                worker_ref: candidate.worker_ref.clone(),
                request: candidate.request,
                context: self.execution_context(candidate.worker_ref.clone()),
                previous_working_directory: candidate.previous_working_directory,
                working_directory: None,
                config_bundle: candidate.config_bundle,
            };
            match backend.restore_worker(request) {
                WorkerExecutionSpawnResult::Connected {
                    handle,
                    run_state,
                    working_directory,
                } => self.commit_restored_worker_execution(
                    &candidate.worker_ref,
                    handle,
                    run_state,
                    working_directory,
                )?,
                WorkerExecutionSpawnResult::Rejected(result)
                | WorkerExecutionSpawnResult::Errored(result) => {
                    let mut state = self.lock()?;
                    state.record_restore_failure(&candidate.worker_ref, result)?;
                }
            }
        }
        Ok(())
    }

    fn commit_restored_worker_execution(
        &self,
        worker_ref: &WorkerRef,
        handle: WorkerExecutionHandle,
        run_state: WorkerExecutionRunState,
        working_directory: Option<CatalogWorkingDirectoryStatus>,
    ) -> Result<(), RuntimeError> {
        let mut state = self.lock()?;
        state.ensure_worker_ref(worker_ref)?;
        {
            let worker = state.worker_mut(worker_ref)?;
            worker.execution_handle = Some(handle);
            worker.status = worker_status_from_run_state(run_state);
            worker.working_directory = working_directory;
        }
        state.publish_worker_upsert(worker_ref.worker_id)?;
        state.persist_runtime_snapshot()?;
        state.persist_worker(&worker_ref.worker_id)?;
        Ok(())
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
struct SubscriptionSink {
    selector: EventSubscriptionSelector,
    workspace_id: Option<String>,
    sender: mpsc::Sender<RuntimeSubscriptionUpdate>,
    lagged: Arc<AtomicBool>,
}

#[derive(Debug)]
struct RuntimeState {
    display_name: Option<String>,
    backend: RuntimeBackendKind,
    #[cfg_attr(not(feature = "fs-store"), allow(dead_code))]
    persistence: RuntimePersistence,
    status: RuntimeStatus,
    execution_backend: Option<WorkerExecutionBackendRef>,
    next_worker_sequence: u64,
    #[cfg(feature = "fs-store")]
    next_diagnostic_id: u64,
    workers: BTreeMap<WorkerId, WorkerRecord>,
    workspace_owners: BTreeMap<String, String>,
    config_bundles: BTreeMap<String, ConfigBundle>,
    diagnostics: Vec<RuntimeDiagnostic>,
    subscription_revision: u64,
    worker_subject_revisions: BTreeMap<WorkerId, u64>,
    next_event_subscription_id: u64,
    subscriptions: BTreeMap<u64, SubscriptionSink>,
    #[cfg(feature = "ws-server")]
    next_observation_sequence: u64,
    #[cfg(feature = "ws-server")]
    observation_events: VecDeque<WorkerObservationEvent>,
    #[cfg(feature = "ws-server")]
    observation_tx: broadcast::Sender<WorkerObservationEvent>,
}

impl RuntimeState {
    fn new(display_name: Option<String>) -> Self {
        Self {
            display_name,
            backend: RuntimeBackendKind::Memory,
            persistence: RuntimePersistence::Memory,
            status: RuntimeStatus::Running,
            execution_backend: None,
            next_worker_sequence: 1,
            #[cfg(feature = "fs-store")]
            next_diagnostic_id: 1,
            workers: BTreeMap::new(),
            workspace_owners: BTreeMap::new(),
            config_bundles: BTreeMap::new(),
            diagnostics: Vec::new(),
            subscription_revision: 0,
            worker_subject_revisions: BTreeMap::new(),
            next_event_subscription_id: 1,
            subscriptions: BTreeMap::new(),
            #[cfg(feature = "ws-server")]
            next_observation_sequence: 1,
            #[cfg(feature = "ws-server")]
            observation_events: VecDeque::new(),
            #[cfg(feature = "ws-server")]
            observation_tx: broadcast::channel(256).0,
        }
    }

    #[cfg(feature = "fs-store")]
    fn new_fs_backed(display_name: Option<String>, store: FsRuntimeStore) -> Self {
        Self {
            display_name,
            backend: RuntimeBackendKind::FsStore,
            persistence: RuntimePersistence::Fs(store),
            status: RuntimeStatus::Running,
            execution_backend: None,
            next_worker_sequence: 1,
            #[cfg(feature = "fs-store")]
            next_diagnostic_id: 1,
            workers: BTreeMap::new(),
            workspace_owners: BTreeMap::new(),
            config_bundles: BTreeMap::new(),
            diagnostics: Vec::new(),
            subscription_revision: 0,
            worker_subject_revisions: BTreeMap::new(),
            next_event_subscription_id: 1,
            subscriptions: BTreeMap::new(),
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
        let mut workers = BTreeMap::new();
        let diagnostics = persisted.diagnostics;
        let next_diagnostic_id = persisted.next_diagnostic_id;
        for (worker_id, worker) in persisted.workers {
            workers.insert(
                worker_id,
                WorkerRecord {
                    worker_ref: worker.worker_ref,
                    worker_id: worker.worker_id,
                    status: WorkerStatus::Stopped,
                    workspace_id: worker.workspace_id,
                    request: worker.request,
                    working_directory: worker.working_directory,
                    execution_handle: None,
                },
            );
        }

        Ok(Self {
            display_name: persisted.display_name,
            backend: RuntimeBackendKind::FsStore,
            persistence: RuntimePersistence::Fs(store),
            status: persisted.status,
            execution_backend: None,
            next_worker_sequence: persisted.next_worker_sequence,
            next_diagnostic_id,
            workers,
            config_bundles: persisted.config_bundles,
            workspace_owners: persisted.workspace_owners,
            diagnostics,
            subscription_revision: 0,
            worker_subject_revisions: BTreeMap::new(),
            next_event_subscription_id: 1,
            subscriptions: BTreeMap::new(),
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
            display_name: self.display_name.clone(),
            status: self.status,
            next_worker_sequence: self.next_worker_sequence,
            next_diagnostic_id: self.next_diagnostic_id,
            workers: self
                .workers
                .iter()
                .map(|(worker_id, worker)| (worker_id.clone(), worker.persisted_record()))
                .collect(),
            config_bundles: self.config_bundles.clone(),
            workspace_owners: self.workspace_owners.clone(),
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
                        worker_id: *worker_id,
                    })?;
            store.write_worker_snapshot(&worker.persisted_record())?;
        }
        Ok(())
    }

    #[cfg(feature = "fs-store")]
    fn delete_worker_snapshot(&self, worker_id: &WorkerId) -> Result<(), RuntimeError> {
        if let Some(store) = self.fs_store() {
            store.delete_worker_snapshot(worker_id)?;
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
    fn delete_worker_snapshot(&self, _worker_id: &WorkerId) -> Result<(), RuntimeError> {
        Ok(())
    }

    #[cfg(not(feature = "fs-store"))]
    fn persist_workers(&self) -> Result<(), RuntimeError> {
        Ok(())
    }

    fn ensure_running(&self) -> Result<(), RuntimeError> {
        if self.status == RuntimeStatus::Stopped {
            Err(RuntimeError::RuntimeStopped)
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
        if !self.workers.contains_key(&worker_ref.worker_id) {
            return Err(RuntimeError::WorkerNotFound {
                worker_id: worker_ref.worker_id,
            });
        }
        Ok(())
    }

    fn ensure_workspace_owner(
        &mut self,
        scope: &RuntimeWorkspaceScope,
        claim_if_missing: bool,
    ) -> Result<bool, RuntimeError> {
        if scope.workspace_id.trim().is_empty() {
            return Err(RuntimeError::InvalidRequest(
                "Runtime auth workspace_id must not be empty".to_string(),
            ));
        }
        if scope.server_id.trim().is_empty() {
            return Err(RuntimeError::InvalidRequest(
                "Runtime auth server_id must not be empty".to_string(),
            ));
        }
        match self.workspace_owners.get(&scope.workspace_id) {
            Some(owner_server_id) if owner_server_id == &scope.server_id => Ok(true),
            Some(owner_server_id) => Err(RuntimeError::WorkspaceOwnerMismatch {
                workspace_id: scope.workspace_id.clone(),
                owner_server_id: owner_server_id.clone(),
                requester_server_id: scope.server_id.clone(),
            }),
            None if claim_if_missing => {
                self.workspace_owners
                    .insert(scope.workspace_id.clone(), scope.server_id.clone());
                Ok(true)
            }
            None => Ok(false),
        }
    }

    fn ensure_workspace_owner_for_existing_workers(
        &mut self,
        scope: &RuntimeWorkspaceScope,
    ) -> Result<bool, RuntimeError> {
        let has_workspace_worker = self
            .workers
            .values()
            .any(|worker| worker.belongs_to_workspace(&scope.workspace_id));
        self.ensure_workspace_owner(scope, has_workspace_worker)
    }

    fn forget_workspace_owner_if_unused(&mut self, workspace_id: &str) -> bool {
        if self
            .workers
            .values()
            .any(|worker| worker.belongs_to_workspace(workspace_id))
        {
            return false;
        }
        self.workspace_owners.remove(workspace_id).is_some()
    }

    fn worker(&self, worker_ref: &WorkerRef) -> Result<&WorkerRecord, RuntimeError> {
        self.ensure_worker_ref(worker_ref)?;
        self.workers
            .get(&worker_ref.worker_id)
            .ok_or_else(|| RuntimeError::WorkerNotFound {
                worker_id: worker_ref.worker_id,
            })
    }

    fn worker_mut(&mut self, worker_ref: &WorkerRef) -> Result<&mut WorkerRecord, RuntimeError> {
        self.ensure_worker_ref(worker_ref)?;
        self.workers
            .get_mut(&worker_ref.worker_id)
            .ok_or_else(|| RuntimeError::WorkerNotFound {
                worker_id: worker_ref.worker_id,
            })
    }

    fn subscription_snapshot(
        &self,
        scope: Option<&RuntimeWorkspaceScope>,
        selector: &EventSubscriptionSelector,
    ) -> Result<SubscriptionSnapshot, RuntimeError> {
        let workers = match selector {
            EventSubscriptionSelector::RuntimeWorkers => self
                .workers
                .values()
                .filter(|worker| {
                    scope.is_none_or(|scope| worker.belongs_to_workspace(&scope.workspace_id))
                })
                .map(|worker| self.subscription_worker(worker))
                .collect::<Result<Vec<_>, _>>()?,
            EventSubscriptionSelector::WorkerLifecycle { worker_ids } => {
                let mut selected = Vec::with_capacity(worker_ids.as_slice().len());
                for worker_id in worker_ids.as_slice() {
                    let runtime_worker_id = WorkerId::parse(worker_id.as_str()).ok_or_else(|| {
                        RuntimeError::InvalidRequest(format!(
                            "worker_lifecycle selector contains invalid Runtime Worker id {worker_id}"
                        ))
                    })?;
                    let worker = self.workers.get(&runtime_worker_id).ok_or(
                        RuntimeError::WorkerNotFound {
                            worker_id: runtime_worker_id,
                        },
                    )?;
                    if scope.is_some_and(|scope| !worker.belongs_to_workspace(&scope.workspace_id))
                    {
                        return Err(RuntimeError::WorkerNotFound {
                            worker_id: runtime_worker_id,
                        });
                    }
                    selected.push(self.subscription_worker(worker)?);
                }
                selected
            }
            EventSubscriptionSelector::WorkerProtocol { .. }
            | EventSubscriptionSelector::WorkspaceWorkers
            | EventSubscriptionSelector::WorkspaceWorkdirs => {
                return Err(RuntimeError::InvalidRequest(
                    "selector is not produced by the Runtime lifecycle subscription".to_string(),
                ));
            }
        };
        Ok(SubscriptionSnapshot::Workers { workers })
    }

    fn subscription_worker(
        &self,
        worker: &WorkerRecord,
    ) -> Result<SubscriptionWorker, RuntimeError> {
        let worker_id = SubscriptionWorkerId::new(worker.worker_id.to_string())
            .map_err(subscription_validation_error)?;
        let repository_id = worker
            .working_directory
            .as_ref()
            .map(|working_directory| working_directory.summary.repository_id.clone());
        let working_directory_id = worker
            .working_directory
            .as_ref()
            .map(|working_directory| {
                SubscriptionWorkdirId::new(working_directory.summary.working_directory_id.clone())
                    .map_err(subscription_validation_error)
            })
            .transpose()?;
        let profile = match &worker.request.profile {
            ProfileSelector::Builtin(name) | ProfileSelector::Named(name) => Some(name.clone()),
        };
        Ok(SubscriptionWorker {
            worker_id,
            runtime_id: None,
            subject_revision: self
                .worker_subject_revisions
                .get(&worker.worker_id)
                .copied()
                .unwrap_or(0),
            state: subscription_worker_state(worker.status),
            workspace_id: worker.workspace_id.clone(),
            display_name: worker.request.display_name.clone(),
            profile,
            repository_id,
            working_directory_id,
        })
    }

    fn publish_worker_upsert(&mut self, worker_id: WorkerId) -> Result<(), RuntimeError> {
        self.subscription_revision = self.subscription_revision.saturating_add(1);
        let subject_revision = {
            let revision = self.worker_subject_revisions.entry(worker_id).or_insert(0);
            *revision = revision.saturating_add(1);
            *revision
        };
        let worker = self
            .workers
            .get(&worker_id)
            .ok_or(RuntimeError::WorkerNotFound { worker_id })?;
        let workspace_id = worker.workspace_id.clone();
        let mut projected = self.subscription_worker(worker)?;
        projected.subject_revision = subject_revision;
        let projected_worker_id = projected.worker_id.clone();
        self.deliver_worker_subscription_update(
            &projected_worker_id,
            workspace_id.as_deref(),
            RuntimeSubscriptionUpdate {
                subject_revision,
                payload: SubscriptionEventPayload::WorkerUpserted { worker: projected },
            },
        );
        Ok(())
    }

    fn publish_worker_removed(
        &mut self,
        worker_id: WorkerId,
        workspace_id: Option<&str>,
    ) -> Result<(), RuntimeError> {
        self.subscription_revision = self.subscription_revision.saturating_add(1);
        let subject_revision = {
            let revision = self.worker_subject_revisions.entry(worker_id).or_insert(0);
            *revision = revision.saturating_add(1);
            *revision
        };
        let worker_id = SubscriptionWorkerId::new(worker_id.to_string())
            .map_err(subscription_validation_error)?;
        self.deliver_worker_subscription_update(
            &worker_id,
            workspace_id,
            RuntimeSubscriptionUpdate {
                subject_revision,
                payload: SubscriptionEventPayload::WorkerRemoved {
                    worker_id: worker_id.clone(),
                    runtime_id: None,
                },
            },
        );
        Ok(())
    }

    fn deliver_worker_subscription_update(
        &mut self,
        worker_id: &SubscriptionWorkerId,
        workspace_id: Option<&str>,
        update: RuntimeSubscriptionUpdate,
    ) {
        let mut closed = Vec::new();
        for (subscription_id, sink) in &self.subscriptions {
            if sink
                .workspace_id
                .as_deref()
                .is_some_and(|expected| workspace_id != Some(expected))
            {
                continue;
            }
            let selected = match &sink.selector {
                EventSubscriptionSelector::RuntimeWorkers => true,
                EventSubscriptionSelector::WorkerLifecycle { worker_ids } => {
                    worker_ids.contains(worker_id)
                }
                EventSubscriptionSelector::WorkerProtocol { .. }
                | EventSubscriptionSelector::WorkspaceWorkers
                | EventSubscriptionSelector::WorkspaceWorkdirs => false,
            };
            if !selected {
                continue;
            }
            match sink.sender.try_send(update.clone()) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(_)) => {
                    sink.lagged.store(true, Ordering::Release);
                    closed.push(*subscription_id);
                }
                Err(mpsc::error::TrySendError::Closed(_)) => closed.push(*subscription_id),
            }
        }
        for subscription_id in closed {
            self.subscriptions.remove(&subscription_id);
        }
    }

    fn primary_worker_id_for_workdir(&self, working_directory_id: &str) -> Option<WorkerId> {
        self.workers.values().find_map(|worker| {
            if worker
                .working_directory
                .as_ref()
                .is_some_and(|binding| binding.summary.working_directory_id == working_directory_id)
                || requested_primary_workdir_id(&worker.request) == Some(working_directory_id)
            {
                Some(worker.worker_id)
            } else {
                None
            }
        })
    }

    #[cfg(feature = "fs-store")]
    fn record_restore_failure(
        &mut self,
        worker_ref: &WorkerRef,
        result: WorkerExecutionResult,
    ) -> Result<(), RuntimeError> {
        let message = result
            .message
            .clone()
            .unwrap_or_else(|| "worker execution restore failed".to_string());
        let diagnostic_id = self.next_diagnostic_id;
        self.next_diagnostic_id += 1;
        self.diagnostics.push(RuntimeDiagnostic {
            id: diagnostic_id,
            severity: DiagnosticSeverity::Warning,
            code: "worker_execution_restore_failed".to_string(),
            message: format!(
                "worker {} execution restore failed: {message}",
                worker_ref.worker_id
            ),
            worker_ref: Some(worker_ref.clone()),
        });
        let worker = self.worker_mut(worker_ref)?;
        worker.execution_handle = None;
        worker.status = WorkerStatus::Stopped;
        self.publish_worker_upsert(worker_ref.worker_id)?;
        self.persist_runtime_snapshot()?;
        Ok(())
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
    fn project_protocol_event_to_status(
        &mut self,
        worker_ref: &WorkerRef,
        event: &protocol::Event,
    ) -> bool {
        let Some(worker) = self.workers.get_mut(&worker_ref.worker_id) else {
            return false;
        };
        let next_status = match event {
            protocol::Event::Status {
                status: protocol::WorkerStatus::Running,
            } => Some(WorkerStatus::Running),
            protocol::Event::Status {
                status: protocol::WorkerStatus::Idle,
            } => Some(WorkerStatus::Idle),
            protocol::Event::Status {
                status: protocol::WorkerStatus::Paused,
            } => Some(WorkerStatus::Paused),
            protocol::Event::Snapshot { status, .. } => match status {
                protocol::WorkerStatus::Running => Some(WorkerStatus::Running),
                protocol::WorkerStatus::Idle => Some(WorkerStatus::Idle),
                protocol::WorkerStatus::Paused => Some(WorkerStatus::Paused),
            },
            protocol::Event::RunEnd { result } => match result {
                protocol::RunResult::Finished | protocol::RunResult::RolledBack => {
                    Some(WorkerStatus::Idle)
                }
                protocol::RunResult::Paused => Some(WorkerStatus::Paused),
                protocol::RunResult::LimitReached => Some(WorkerStatus::Idle),
            },
            _ => None,
        };
        if let Some(next_status) = next_status {
            let changed = worker.status != next_status;
            worker.status = next_status;
            changed
        } else {
            false
        }
    }
}

#[derive(Debug)]
struct WorkerRecord {
    worker_ref: WorkerRef,
    worker_id: WorkerId,
    status: WorkerStatus,
    workspace_id: Option<String>,
    request: CreateWorkerRequest,
    working_directory: Option<CatalogWorkingDirectoryStatus>,
    execution_handle: Option<WorkerExecutionHandle>,
}

impl WorkerRecord {
    fn belongs_to_workspace(&self, workspace_id: &str) -> bool {
        self.workspace_id.as_deref() == Some(workspace_id)
    }

    fn summary(&self) -> WorkerSummary {
        WorkerSummary {
            worker_ref: self.worker_ref.clone(),
            worker_id: self.worker_id,
            status: self.status,
            workspace_id: self.workspace_id.clone(),
            working_directory: self.working_directory.clone(),
            profile: self.request.profile.clone(),
            display_name: self.request.display_name.clone(),
            profile_source: self.request.profile_source.reference(),
            config_bundle: self.request.config_bundle.clone(),
        }
    }

    fn detail(&self) -> WorkerDetail {
        WorkerDetail {
            worker_ref: self.worker_ref.clone(),
            worker_id: self.worker_id,
            status: self.status,
            workspace_id: self.workspace_id.clone(),
            working_directory: self.working_directory.clone(),
            profile: self.request.profile.clone(),
            display_name: self.request.display_name.clone(),
            profile_source: self.request.profile_source.reference(),
            config_bundle: self.request.config_bundle.clone(),
        }
    }

    #[cfg(feature = "fs-store")]
    fn persisted_record(&self) -> PersistedWorkerRecord {
        PersistedWorkerRecord {
            worker_ref: self.worker_ref.clone(),
            worker_id: self.worker_id.clone(),
            request: self.request.clone(),
            workspace_id: self.workspace_id.clone(),
            working_directory: self.working_directory.clone(),
        }
    }
}

fn worker_status_from_run_state(run_state: WorkerExecutionRunState) -> WorkerStatus {
    match run_state {
        WorkerExecutionRunState::Idle => WorkerStatus::Idle,
        WorkerExecutionRunState::Busy => WorkerStatus::Running,
        WorkerExecutionRunState::Stopped
        | WorkerExecutionRunState::Rejected
        | WorkerExecutionRunState::Errored => WorkerStatus::Stopped,
    }
}

fn requested_primary_workdir_id(request: &CreateWorkerRequest) -> Option<&str> {
    request
        .working_directory
        .as_ref()
        .map(|claim| claim.working_directory_id.as_str())
        .or_else(|| {
            request
                .working_directory_request
                .as_ref()
                .and_then(|request| request.backend_workdir_id.as_deref())
        })
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

fn validate_create_workspace_scope(
    request: &CreateWorkerRequest,
    workspace_id: Option<&str>,
) -> Result<(), RuntimeError> {
    let Some(workspace_id) = workspace_id else {
        return Ok(());
    };
    if workspace_id.trim().is_empty() {
        return Err(RuntimeError::InvalidRequest(
            "Runtime auth workspace_id must not be empty".to_string(),
        ));
    }
    if let Some(request_workspace_id) = request
        .workspace_api
        .as_ref()
        .map(|workspace_api| workspace_api.workspace_id.as_str())
    {
        if request_workspace_id != workspace_id {
            return Err(RuntimeError::InvalidRequest(format!(
                "request workspace_id {request_workspace_id} does not match Runtime auth workspace_id {workspace_id}"
            )));
        }
    }
    Ok(())
}

fn validate_worker_input(input: &WorkerInput) -> Result<(), RuntimeError> {
    if !input.kind.is_empty_content_allowed() && input.content.trim().is_empty() {
        return Err(RuntimeError::InvalidRequest(
            "worker input content must not be empty".to_string(),
        ));
    }
    if input
        .segments
        .as_ref()
        .is_some_and(|segments| segments.is_empty())
    {
        return Err(RuntimeError::InvalidRequest(
            "worker input segments must not be empty".to_string(),
        ));
    }
    Ok(())
}

#[cfg(feature = "ws-server")]
fn input_protocol_event(input: &WorkerInput) -> protocol::Event {
    match input.kind {
        WorkerInputKind::User => protocol::Event::UserMessage {
            segments: input.segments.clone().unwrap_or_else(|| {
                vec![protocol::Segment::Text {
                    content: input.content.clone(),
                }]
            }),
        },
        WorkerInputKind::System => protocol::Event::SystemItem {
            item: serde_json::json!({
                "kind": "embedded_worker_system_input",
                "content": input.content.clone(),
            }),
        },
        WorkerInputKind::Compact
        | WorkerInputKind::ListRewindTargets
        | WorkerInputKind::RegisterPeer => protocol::Event::SystemItem {
            item: serde_json::json!({
                "kind": "embedded_worker_command_input",
                "command": input.kind,
                "content": input.content.clone(),
            }),
        },
    }
}

fn subscription_validation_error(error: SubscriptionValidationError) -> RuntimeError {
    RuntimeError::InvalidRequest(format!("invalid event subscription: {error}"))
}

fn subscription_worker_state(status: WorkerStatus) -> SubscriptionWorkerState {
    match status {
        WorkerStatus::Idle => SubscriptionWorkerState::Idle,
        WorkerStatus::Running => SubscriptionWorkerState::Running,
        WorkerStatus::Paused => SubscriptionWorkerState::Paused,
        WorkerStatus::Stopped => SubscriptionWorkerState::Stopped,
        WorkerStatus::Cancelled => SubscriptionWorkerState::Cancelled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{
        ConfigBundleRef, ProfileSelector, WorkingDirectoryClaim, WorkspaceApiRef,
    };
    use crate::config_bundle::{
        ConfigBundle, ConfigBundleMetadata, ConfigBundleProvenance, ConfigDeclaration,
        ConfigDeclarationKind, ConfigProfileDescriptor,
    };
    use crate::execution::{
        WorkerExecutionBackend, WorkerExecutionContext, WorkerExecutionHandle,
        WorkerExecutionRestoreRequest, WorkerExecutionRunState,
    };
    use std::collections::BTreeMap;
    #[cfg(feature = "fs-store")]
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    fn task_request(_objective: &str) -> CreateWorkerRequest {
        let profile = ProfileSelector::Builtin("builtin:coder".to_string());
        let bundle = test_bundle_for_profile(profile.clone());
        CreateWorkerRequest {
            idempotency_key: None,
            idempotency_fingerprint: None,
            profile,
            display_name: None,
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
            workspace_api: None,
        }
    }

    fn scoped_task_request(objective: &str, workspace_id: &str) -> CreateWorkerRequest {
        let mut request = task_request(objective);
        request.workspace_api = Some(WorkspaceApiRef {
            workspace_id: workspace_id.to_string(),
            base_url: format!("https://workspace.example/{workspace_id}"),
            runtime_id: None,
            access_token: None,
        });
        request
    }

    fn scope(workspace_id: &str, server_id: &str) -> RuntimeWorkspaceScope {
        RuntimeWorkspaceScope::new(workspace_id, server_id)
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
        restore_result: Mutex<Option<WorkerExecutionSpawnResult>>,
        restore_count: Mutex<u64>,
        contexts: Mutex<BTreeMap<WorkerId, WorkerExecutionContext>>,
        workspace_access_tokens: Mutex<BTreeMap<WorkerId, String>>,
        #[cfg(feature = "ws-server")]
        snapshots: Mutex<BTreeMap<WorkerId, protocol::Event>>,
    }

    impl TestExecutionBackend {
        fn set_dispatch_result(&self, result: WorkerExecutionResult) {
            *self.dispatch_result.lock().unwrap() = Some(result);
        }

        #[cfg(feature = "ws-server")]
        fn set_worker_snapshot(&self, worker_ref: &WorkerRef, snapshot: protocol::Event) {
            self.snapshots
                .lock()
                .unwrap()
                .insert(worker_ref.worker_id.clone(), snapshot);
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

        fn restore_worker(
            &self,
            request: WorkerExecutionRestoreRequest,
        ) -> WorkerExecutionSpawnResult {
            *self.restore_count.lock().unwrap() += 1;
            if let Some(result) = self.restore_result.lock().unwrap().clone() {
                return result;
            }
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

        fn replace_workspace_access_token(
            &self,
            handle: &WorkerExecutionHandle,
            access_token: String,
        ) -> WorkerExecutionResult {
            self.workspace_access_tokens
                .lock()
                .unwrap()
                .insert(handle.worker_ref().worker_id.clone(), access_token);
            WorkerExecutionResult::accepted(
                WorkerExecutionOperation::ReplaceWorkspaceAccessToken,
                WorkerExecutionRunState::Idle,
            )
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

        #[cfg(feature = "ws-server")]
        fn worker_snapshot(&self, handle: &WorkerExecutionHandle) -> Option<protocol::Event> {
            self.snapshots
                .lock()
                .unwrap()
                .get(&handle.worker_ref().worker_id)
                .cloned()
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

    fn receive_subscription_update(
        subscription: &mut RuntimeEventSelectorSubscription,
    ) -> Result<RuntimeSubscriptionUpdate, RuntimeSubscriptionRecvError> {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(subscription.recv())
    }

    #[test]
    fn runtime_worker_subscription_has_gap_free_snapshot_and_live_updates() {
        let runtime = runtime_with_backend();
        let mut subscription = runtime
            .subscribe_event_selector(EventSubscriptionSelector::RuntimeWorkers)
            .unwrap();
        assert_eq!(subscription.snapshot_revision(), 0);
        let SubscriptionSnapshot::Workers { workers } = subscription.snapshot() else {
            panic!("runtime_workers must return a Worker snapshot");
        };
        assert!(workers.is_empty());

        let created = runtime.create_worker(task_request("live")).unwrap();
        let update = receive_subscription_update(&mut subscription).unwrap();
        assert_eq!(update.subject_revision, 1);
        match update.payload {
            SubscriptionEventPayload::WorkerUpserted { worker } => {
                assert_eq!(worker.worker_id.as_str(), created.worker_id.to_string());
                assert_eq!(worker.subject_revision, 1);
                assert_eq!(worker.state, SubscriptionWorkerState::Idle);
            }
            payload => panic!("unexpected subscription payload: {payload:?}"),
        }

        runtime.stop_runtime().unwrap();
        let update = receive_subscription_update(&mut subscription).unwrap();
        assert_eq!(update.subject_revision, 2);
        match update.payload {
            SubscriptionEventPayload::WorkerUpserted { worker } => {
                assert_eq!(worker.worker_id.as_str(), created.worker_id.to_string());
                assert_eq!(worker.state, SubscriptionWorkerState::Stopped);
            }
            payload => panic!("unexpected subscription payload: {payload:?}"),
        }
    }

    #[test]
    fn worker_lifecycle_subscription_delivers_only_selected_workers() {
        let runtime = runtime_with_backend();
        let first = runtime.create_worker(task_request("first")).unwrap();
        let second = runtime.create_worker(task_request("second")).unwrap();
        let selected_id = SubscriptionWorkerId::new(first.worker_id.to_string()).unwrap();
        let mut subscription = runtime
            .subscribe_event_selector(EventSubscriptionSelector::WorkerLifecycle {
                worker_ids: protocol::subscription::SubscriptionWorkerIds::new([
                    selected_id.clone()
                ])
                .unwrap(),
            })
            .unwrap();
        let SubscriptionSnapshot::Workers { workers } = subscription.snapshot() else {
            panic!("worker_lifecycle must return a Worker snapshot");
        };
        assert_eq!(workers.len(), 1);
        assert_eq!(workers[0].worker_id, selected_id);

        runtime.stop_worker(&second.worker_ref, None).unwrap();
        assert!(matches!(
            subscription.receiver.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));

        runtime.stop_worker(&first.worker_ref, None).unwrap();
        let update = receive_subscription_update(&mut subscription).unwrap();
        assert!(matches!(
            update.payload,
            SubscriptionEventPayload::WorkerUpserted { worker }
                if worker.worker_id == selected_id
        ));
    }

    #[test]
    fn scoped_runtime_worker_subscription_hides_other_workspaces() {
        let runtime = runtime_with_backend();
        let workspace_a = runtime
            .create_worker_scoped(
                &scope("workspace-a", "server-a"),
                scoped_task_request("a", "workspace-a"),
            )
            .unwrap();
        let workspace_b = runtime
            .create_worker_scoped(
                &scope("workspace-b", "server-b"),
                scoped_task_request("b", "workspace-b"),
            )
            .unwrap();
        let mut subscription = runtime
            .subscribe_event_selector_scoped(
                &scope("workspace-a", "server-a"),
                EventSubscriptionSelector::RuntimeWorkers,
            )
            .unwrap();
        let SubscriptionSnapshot::Workers { workers } = subscription.snapshot() else {
            panic!("runtime_workers must return a Worker snapshot");
        };
        assert_eq!(workers.len(), 1);
        assert_eq!(
            workers[0].worker_id.as_str(),
            workspace_a.worker_id.to_string()
        );

        runtime.stop_worker(&workspace_b.worker_ref, None).unwrap();
        assert!(matches!(
            subscription.receiver.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        runtime.stop_worker(&workspace_a.worker_ref, None).unwrap();
        assert!(receive_subscription_update(&mut subscription).is_ok());
    }

    #[test]
    fn lagged_runtime_subscription_closes_without_blocking_mutation() {
        let runtime = runtime_with_backend();
        let created = runtime.create_worker(task_request("lag")).unwrap();
        let mut subscription = runtime
            .subscribe_event_selector(EventSubscriptionSelector::RuntimeWorkers)
            .unwrap();
        {
            let mut state = runtime.lock().unwrap();
            for _ in 0..=SUBSCRIPTION_QUEUE_CAPACITY {
                state.publish_worker_upsert(created.worker_id).unwrap();
            }
        }
        assert_eq!(subscription.receiver.len(), SUBSCRIPTION_QUEUE_CAPACITY);
        assert!(matches!(
            receive_subscription_update(&mut subscription),
            Err(RuntimeSubscriptionRecvError::Lagged)
        ));
    }

    #[test]
    fn dropping_runtime_subscription_releases_producer_state() {
        let runtime = runtime_with_backend();
        let subscription = runtime
            .subscribe_event_selector(EventSubscriptionSelector::RuntimeWorkers)
            .unwrap();
        assert_eq!(runtime.lock().unwrap().subscriptions.len(), 1);
        drop(subscription);
        assert!(runtime.lock().unwrap().subscriptions.is_empty());
    }

    #[test]
    fn scoped_worker_access_hides_other_workspace_workers() {
        let runtime = runtime_with_backend();
        let workspace_a = runtime
            .create_worker_scoped(
                &scope("workspace-a", "server-a"),
                scoped_task_request("a", "workspace-a"),
            )
            .unwrap();
        let workspace_b = runtime
            .create_worker_scoped(
                &scope("workspace-b", "server-b"),
                scoped_task_request("b", "workspace-b"),
            )
            .unwrap();

        assert_eq!(workspace_a.workspace_id.as_deref(), Some("workspace-a"));
        assert_eq!(workspace_b.workspace_id.as_deref(), Some("workspace-b"));
        assert_eq!(
            runtime
                .list_workers_scoped(&scope("workspace-a", "server-a"))
                .unwrap()
                .into_iter()
                .map(|worker| worker.worker_ref)
                .collect::<Vec<_>>(),
            vec![workspace_a.worker_ref.clone()]
        );
        assert_eq!(
            runtime
                .list_workers_scoped(&scope("workspace-b", "server-b"))
                .unwrap()
                .into_iter()
                .map(|worker| worker.worker_ref)
                .collect::<Vec<_>>(),
            vec![workspace_b.worker_ref.clone()]
        );

        let detail_error = runtime
            .worker_detail_scoped(&scope("workspace-a", "server-a"), &workspace_b.worker_ref)
            .unwrap_err();
        assert!(matches!(detail_error, RuntimeError::WorkerNotFound { .. }));

        let input_error = runtime
            .send_input_scoped(
                &scope("workspace-a", "server-a"),
                &workspace_b.worker_ref,
                WorkerInput::user("cross workspace"),
            )
            .unwrap_err();
        assert!(matches!(input_error, RuntimeError::WorkerNotFound { .. }));

        let protocol_error = runtime
            .send_protocol_method_scoped(
                &scope("workspace-a", "server-a"),
                &workspace_b.worker_ref,
                Method::Shutdown,
            )
            .unwrap_err();
        assert!(matches!(
            protocol_error,
            RuntimeError::WorkerNotFound { .. }
        ));
        assert_eq!(
            runtime
                .worker_detail(&workspace_b.worker_ref)
                .unwrap()
                .status,
            WorkerStatus::Idle
        );
    }

    #[test]
    fn workspace_api_replacement_updates_live_execution_and_persisted_request() {
        let (runtime, backend) = runtime_and_backend();
        let scope = scope("workspace-a", "server-a");
        let worker = runtime
            .create_worker_scoped(
                &scope,
                scoped_task_request("repair credential", "workspace-a"),
            )
            .unwrap();
        let replacement = WorkspaceApiRef {
            workspace_id: "workspace-a".to_string(),
            base_url: "https://workspace.example/workspace-a/".to_string(),
            runtime_id: Some("runtime-a".to_string()),
            access_token: Some("replacement-token".to_string()),
        };

        runtime
            .replace_worker_workspace_api_scoped(&scope, &worker.worker_ref, replacement.clone())
            .unwrap();

        assert_eq!(
            backend
                .workspace_access_tokens
                .lock()
                .unwrap()
                .get(&worker.worker_ref.worker_id),
            Some(&"replacement-token".to_string())
        );
        let state = runtime.lock().unwrap();
        assert_eq!(
            state
                .worker(&worker.worker_ref)
                .unwrap()
                .request
                .workspace_api,
            Some(replacement)
        );
    }

    #[test]
    fn workspace_owner_binding_rejects_other_backend_and_forgets_after_last_worker_delete() {
        let runtime = runtime_with_backend();
        let server_a = scope("workspace-a", "server-a");
        let server_b = scope("workspace-a", "server-b");
        let first = runtime
            .create_worker_scoped(&server_a, scoped_task_request("first", "workspace-a"))
            .unwrap();
        let second = runtime
            .create_worker_scoped(&server_a, scoped_task_request("second", "workspace-a"))
            .unwrap();

        let create_error = runtime
            .create_worker_scoped(&server_b, scoped_task_request("stolen", "workspace-a"))
            .unwrap_err();
        assert!(matches!(
            create_error,
            RuntimeError::WorkspaceOwnerMismatch { .. }
        ));
        let read_error = runtime
            .worker_detail_scoped(&server_b, &first.worker_ref)
            .unwrap_err();
        assert!(matches!(
            read_error,
            RuntimeError::WorkspaceOwnerMismatch { .. }
        ));

        runtime.stop_worker(&first.worker_ref, None).unwrap();
        runtime.stop_worker(&second.worker_ref, None).unwrap();
        runtime
            .delete_worker_scoped(&server_a, &first.worker_ref)
            .unwrap();
        let still_owned_error = runtime
            .create_worker_scoped(&server_b, scoped_task_request("still owned", "workspace-a"))
            .unwrap_err();
        assert!(matches!(
            still_owned_error,
            RuntimeError::WorkspaceOwnerMismatch { .. }
        ));

        runtime
            .delete_worker_scoped(&server_a, &second.worker_ref)
            .unwrap();
        let rebound = runtime
            .create_worker_scoped(&server_b, scoped_task_request("rebound", "workspace-a"))
            .unwrap();
        assert_eq!(rebound.workspace_id.as_deref(), Some("workspace-a"));
    }

    #[test]
    fn scoped_create_rejects_request_workspace_mismatch() {
        let runtime = runtime_with_backend();
        let error = runtime
            .create_worker_scoped(
                &scope("workspace-a", "server-a"),
                scoped_task_request("mismatch", "workspace-b"),
            )
            .unwrap_err();
        assert!(matches!(error, RuntimeError::InvalidRequest(_)));
        assert!(runtime.list_workers().unwrap().is_empty());
    }

    #[test]
    fn unscoped_legacy_workers_are_hidden_from_scoped_access() {
        let runtime = runtime_with_backend();
        let legacy = runtime.create_worker(task_request("legacy")).unwrap();

        assert!(legacy.workspace_id.is_none());
        assert!(
            runtime
                .list_workers_scoped(&scope("workspace-a", "server-a"))
                .unwrap()
                .is_empty()
        );
        let detail_error = runtime
            .worker_detail_scoped(&scope("workspace-a", "server-a"), &legacy.worker_ref)
            .unwrap_err();
        assert!(matches!(detail_error, RuntimeError::WorkerNotFound { .. }));
    }

    #[test]
    fn stopped_worker_scoped_list_uses_workspace_boundary() {
        let runtime = runtime_with_backend();
        let workspace_a = runtime
            .create_worker_scoped(
                &scope("workspace-a", "server-a"),
                scoped_task_request("a", "workspace-a"),
            )
            .unwrap();
        let workspace_b = runtime
            .create_worker_scoped(
                &scope("workspace-b", "server-b"),
                scoped_task_request("b", "workspace-b"),
            )
            .unwrap();
        runtime.stop_worker(&workspace_a.worker_ref, None).unwrap();
        runtime.stop_worker(&workspace_b.worker_ref, None).unwrap();

        let stopped = runtime
            .list_stopped_workers_scoped(&scope("workspace-a", "server-a"))
            .unwrap();
        assert_eq!(stopped.len(), 1);
        assert_eq!(stopped[0].worker_ref, workspace_a.worker_ref);
    }

    #[test]
    fn create_list_and_detail_preserve_runtime_local_worker_authority() {
        let runtime = runtime_with_backend();
        let detail = runtime.create_worker(task_request("implement v0")).unwrap();

        assert_eq!(detail.status, WorkerStatus::Idle);
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
    fn stopped_worker_list_excludes_alive_and_cancelled_workers() {
        let runtime = runtime_with_backend();
        let alive = runtime.create_worker(task_request("alive")).unwrap();
        let stopped = runtime.create_worker(task_request("stopped")).unwrap();
        let cancelled = runtime.create_worker(task_request("cancelled")).unwrap();

        runtime.stop_worker(&stopped.worker_ref, None).unwrap();
        runtime.cancel_worker(&cancelled.worker_ref, None).unwrap();

        let candidates = runtime.list_stopped_workers().unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].worker_ref, stopped.worker_ref);
        assert_eq!(candidates[0].status, WorkerStatus::Stopped);
        assert_ne!(candidates[0].worker_ref, alive.worker_ref);
        assert_ne!(candidates[0].worker_ref, cancelled.worker_ref);
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
    fn create_worker_idempotency_reuses_worker_and_rejects_different_input() {
        let runtime = runtime_with_backend();
        let mut request = task_request("idempotent");
        request.idempotency_key = Some("operation-1".to_string());
        request.idempotency_fingerprint = Some("sha256:input-1".to_string());
        request.working_directory = Some(WorkingDirectoryClaim {
            working_directory_id: "workdir-idempotent".to_string(),
            relative_cwd: None,
        });

        let first = runtime.create_worker(request.clone()).unwrap();
        let workdir_count_after_first = runtime.list_working_directories().unwrap().len();
        let replayed = runtime.create_worker(request.clone()).unwrap();
        assert_eq!(replayed.worker_ref, first.worker_ref);
        assert_eq!(runtime.list_workers().unwrap().len(), 1);
        assert_eq!(
            runtime.list_working_directories().unwrap().len(),
            workdir_count_after_first
        );

        request.idempotency_fingerprint = Some("sha256:different".to_string());
        let error = runtime.create_worker(request).unwrap_err();
        assert!(matches!(error, RuntimeError::InvalidRequest(_)));
        assert_eq!(runtime.list_workers().unwrap().len(), 1);
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
        assert_eq!(refreshed.status, WorkerStatus::Idle);
        #[cfg(feature = "ws-server")]
        assert!(
            runtime
                .read_worker_observation_events(&detail.worker_ref, WorkerObservationCursor::zero())
                .unwrap()
                .is_empty()
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

    #[cfg(feature = "ws-server")]
    #[test]
    fn observation_snapshot_prefers_live_backend_snapshot() {
        let (runtime, backend) = runtime_and_backend();
        let detail = runtime
            .create_worker(task_request("observe snapshot"))
            .unwrap();
        let expected_entry = serde_json::json!({"kind": "restored-log-entry"});
        backend.set_worker_snapshot(
            &detail.worker_ref,
            protocol::Event::Snapshot {
                entries: vec![expected_entry.clone()],
                greeting: protocol::Greeting {
                    worker_name: "live-worker".to_string(),
                    cwd: "/tmp/live".to_string(),
                    provider: "test-provider".to_string(),
                    model: "test-model".to_string(),
                    scope_summary: "live snapshot".to_string(),
                    tools: Vec::new(),
                    context_window: 128,
                    context_tokens: 64,
                },
                status: protocol::WorkerStatus::Running,
                in_flight: protocol::InFlightSnapshot { blocks: Vec::new() },
            },
        );

        let snapshot = runtime
            .worker_observation_snapshot(&detail.worker_ref)
            .unwrap();
        match snapshot {
            protocol::Event::Snapshot {
                entries,
                greeting,
                status,
                ..
            } => {
                assert_eq!(entries, vec![expected_entry]);
                assert_eq!(greeting.worker_name, "live-worker");
                assert_eq!(status, protocol::WorkerStatus::Running);
            }
            other => panic!("expected snapshot, got {other:?}"),
        }
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
            WorkerStatus::Idle
        );
    }

    #[test]
    fn protocol_shutdown_transitions_worker_to_stopped() {
        let runtime = runtime_with_backend();
        let detail = runtime
            .create_worker(task_request("shutdown from client"))
            .unwrap();

        runtime
            .send_protocol_method(&detail.worker_ref, Method::Shutdown)
            .unwrap();

        assert_eq!(
            runtime.worker_detail(&detail.worker_ref).unwrap().status,
            WorkerStatus::Stopped
        );
    }

    #[test]
    fn input_restores_stopped_worker_without_persisted_connection_state() {
        let (runtime, backend) = runtime_and_backend();
        let detail = runtime
            .create_worker(task_request("restore on input"))
            .unwrap();
        runtime
            .send_protocol_method(&detail.worker_ref, Method::Shutdown)
            .unwrap();

        runtime
            .send_input(&detail.worker_ref, WorkerInput::user("wake up"))
            .unwrap();

        assert_eq!(*backend.restore_count.lock().unwrap(), 1);
        assert_eq!(
            runtime.worker_detail(&detail.worker_ref).unwrap().status,
            WorkerStatus::Idle
        );
    }

    #[cfg(feature = "ws-server")]
    #[test]
    fn send_input_records_protocol_observations() {
        let runtime = Runtime::with_execution_backend(
            RuntimeOptions {
                ..RuntimeOptions::default()
            },
            Arc::new(TestExecutionBackend::default()),
        )
        .unwrap();
        runtime.store_config_bundle(test_bundle()).unwrap();
        let detail = runtime.create_worker(task_request("chat")).unwrap();

        runtime
            .send_input(&detail.worker_ref, WorkerInput::user("hello"))
            .unwrap();
        runtime
            .send_input(&detail.worker_ref, WorkerInput::system("note"))
            .unwrap();

        let observations = runtime
            .read_worker_observation_events(&detail.worker_ref, WorkerObservationCursor::zero())
            .unwrap();
        assert_eq!(observations.len(), 2);
        assert!(matches!(
            observations[0].payload,
            protocol::Event::UserMessage { .. }
        ));
        assert!(matches!(
            observations[1].payload,
            protocol::Event::SystemItem { .. }
        ));
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
    fn delete_worker_removes_stopped_worker_from_runtime() {
        let runtime = runtime_with_backend();
        let worker = runtime.create_worker(task_request("delete me")).unwrap();
        assert!(runtime.delete_worker(&worker.worker_ref).is_err());
        runtime
            .stop_worker(&worker.worker_ref, Some("done".to_string()))
            .unwrap();

        let result = runtime.delete_worker(&worker.worker_ref).unwrap();
        assert!(result.deleted);
        assert_eq!(result.worker_id, worker.worker_id);
        assert!(matches!(
            runtime.worker_detail(&worker.worker_ref),
            Err(RuntimeError::WorkerNotFound { .. })
        ));
        let summary = runtime.summary().unwrap();
        assert_eq!(summary.worker_count, 0);
    }

    #[test]
    fn stop_then_cancel_preserves_stopped_terminal_state() {
        let runtime = runtime_with_backend();
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
        assert_eq!(
            runtime.worker_detail(&worker.worker_ref).unwrap().status,
            WorkerStatus::Stopped
        );

        let summary = runtime.summary().unwrap();
        assert_eq!(summary.active_worker_count, 0);
        assert_eq!(summary.stopped_worker_count, 1);
        assert_eq!(summary.cancelled_worker_count, 0);
    }

    #[test]
    fn cancel_then_stop_preserves_cancelled_terminal_state() {
        let runtime = runtime_with_backend();
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
        assert_eq!(
            runtime.worker_detail(&worker.worker_ref).unwrap().status,
            WorkerStatus::Cancelled
        );

        let summary = runtime.summary().unwrap();
        assert_eq!(summary.active_worker_count, 0);
        assert_eq!(summary.stopped_worker_count, 0);
        assert_eq!(summary.cancelled_worker_count, 1);
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
    fn fs_store_restores_workers_without_legacy_event_or_protocol_observation_logs() {
        let root = fs_store_root("restore");
        let runtime = Runtime::with_fs_store_and_execution_backend(
            crate::fs_store::FsRuntimeStoreOptions {
                root: root.clone(),
                display_name: Some("filesystem runtime".to_string()),
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
        let worker_store_dir = root.join("workers").join(worker.worker_id.to_string());
        let worker_snapshot: serde_json::Value =
            serde_json::from_slice(&std::fs::read(worker_store_dir.join("worker.json")).unwrap())
                .unwrap();
        assert!(worker_snapshot.get("status").is_none());
        assert!(worker_snapshot.get("execution").is_none());
        assert!(!root.join("events.jsonl").exists());
        std::fs::write(
            worker_store_dir.join("observations.jsonl"),
            b"{\"legacy\":true}\n",
        )
        .unwrap();
        std::fs::write(root.join("events.jsonl"), b"obsolete runtime event\n").unwrap();
        drop(runtime);

        let restored = Runtime::with_fs_store(crate::fs_store::FsRuntimeStoreOptions {
            root: root.clone(),
            display_name: None,
        })
        .unwrap();
        let restored_worker = restored.worker_detail(&worker.worker_ref).unwrap();
        assert_eq!(restored_worker.status, WorkerStatus::Stopped);
        assert!(!root.join("events.jsonl").exists());
        assert!(!worker_store_dir.join("observations.jsonl").exists());
        #[cfg(feature = "ws-server")]
        {
            let observations = restored
                .read_worker_observation_events(&worker.worker_ref, WorkerObservationCursor::zero())
                .unwrap();
            assert!(observations.is_empty());
        }

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
    fn fs_store_restores_workspace_scope_and_hides_legacy_workers_from_scoped_access() {
        let root = fs_store_root("workspace-scope");
        let runtime = Runtime::with_fs_store_and_execution_backend(
            crate::fs_store::FsRuntimeStoreOptions {
                root: root.clone(),
                display_name: None,
            },
            Arc::new(TestExecutionBackend::default()),
        )
        .unwrap();
        runtime.store_config_bundle(test_bundle()).unwrap();
        let scoped = runtime
            .create_worker_scoped(
                &scope("workspace-a", "server-a"),
                scoped_task_request("persist workspace", "workspace-a"),
            )
            .unwrap();
        let legacy = runtime
            .create_worker(task_request("legacy persist"))
            .unwrap();
        let recoverable_legacy = runtime
            .create_worker(scoped_task_request(
                "legacy recoverable persist",
                "workspace-b",
            ))
            .unwrap();
        drop(runtime);

        let restored = Runtime::with_fs_store(crate::fs_store::FsRuntimeStoreOptions {
            root: root.clone(),
            display_name: None,
        })
        .unwrap();
        let restored_scoped = restored.worker_detail(&scoped.worker_ref).unwrap();
        assert_eq!(restored_scoped.workspace_id.as_deref(), Some("workspace-a"));
        assert_eq!(
            restored
                .list_workers_scoped(&scope("workspace-a", "server-a"))
                .unwrap()
                .into_iter()
                .map(|worker| worker.worker_ref)
                .collect::<Vec<_>>(),
            vec![scoped.worker_ref.clone()]
        );
        let legacy_error = restored
            .worker_detail_scoped(&scope("workspace-a", "server-a"), &legacy.worker_ref)
            .unwrap_err();
        assert!(matches!(legacy_error, RuntimeError::WorkerNotFound { .. }));
        let recovered_legacy = restored
            .worker_detail_scoped(
                &scope("workspace-b", "server-b"),
                &recoverable_legacy.worker_ref,
            )
            .unwrap();
        assert_eq!(
            recovered_legacy.workspace_id.as_deref(),
            Some("workspace-b")
        );
        let stolen_legacy_error = restored
            .worker_detail_scoped(
                &scope("workspace-b", "server-c"),
                &recoverable_legacy.worker_ref,
            )
            .unwrap_err();
        assert!(matches!(
            stolen_legacy_error,
            RuntimeError::WorkspaceOwnerMismatch { .. }
        ));

        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(feature = "fs-store")]
    #[test]
    fn fs_store_restores_active_worker_execution_handles() {
        let root = fs_store_root("execution-restore");
        let runtime = Runtime::with_fs_store_and_execution_backend(
            crate::fs_store::FsRuntimeStoreOptions {
                root: root.clone(),
                display_name: None,
            },
            Arc::new(TestExecutionBackend::default()),
        )
        .unwrap();
        runtime.store_config_bundle(test_bundle()).unwrap();
        let worker = runtime
            .create_worker(task_request("restore active worker"))
            .unwrap();
        drop(runtime);

        let backendless = Runtime::with_fs_store(crate::fs_store::FsRuntimeStoreOptions {
            root: root.clone(),
            display_name: None,
        })
        .unwrap();
        let stopped_worker = backendless.worker_detail(&worker.worker_ref).unwrap();
        assert_eq!(stopped_worker.status, WorkerStatus::Stopped);
        drop(backendless);

        let restoring_backend = Arc::new(TestExecutionBackend::default());
        let restored = Runtime::with_fs_store_and_execution_backend(
            crate::fs_store::FsRuntimeStoreOptions {
                root: root.clone(),
                display_name: None,
            },
            restoring_backend.clone(),
        )
        .unwrap();

        assert_eq!(*restoring_backend.restore_count.lock().unwrap(), 1);
        let restored_worker = restored.worker_detail(&worker.worker_ref).unwrap();
        assert_eq!(restored_worker.status, WorkerStatus::Idle);
        restored
            .send_input(&worker.worker_ref, WorkerInput::user("after restart"))
            .unwrap();

        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(feature = "fs-store")]
    #[test]
    fn fs_store_stops_worker_and_reports_when_execution_restore_fails() {
        let root = fs_store_root("execution-restore-failed");
        let runtime = Runtime::with_fs_store_and_execution_backend(
            crate::fs_store::FsRuntimeStoreOptions {
                root: root.clone(),
                display_name: None,
            },
            Arc::new(TestExecutionBackend::default()),
        )
        .unwrap();
        runtime.store_config_bundle(test_bundle()).unwrap();
        let worker = runtime
            .create_worker(task_request("restore failure"))
            .unwrap();
        drop(runtime);

        let restoring_backend = Arc::new(TestExecutionBackend::default());
        *restoring_backend.restore_result.lock().unwrap() =
            Some(WorkerExecutionSpawnResult::Errored(
                WorkerExecutionResult::errored(WorkerExecutionOperation::Restore, "restore boom"),
            ));
        let restored = Runtime::with_fs_store_and_execution_backend(
            crate::fs_store::FsRuntimeStoreOptions {
                root: root.clone(),
                display_name: None,
            },
            restoring_backend.clone(),
        )
        .unwrap();

        assert_eq!(*restoring_backend.restore_count.lock().unwrap(), 1);
        let restored_worker = restored.worker_detail(&worker.worker_ref).unwrap();
        assert_eq!(restored_worker.status, WorkerStatus::Stopped);
        assert!(
            restored
                .diagnostics()
                .unwrap()
                .iter()
                .any(
                    |diagnostic| diagnostic.code == "worker_execution_restore_failed"
                        && diagnostic.worker_ref.as_ref() == Some(&worker.worker_ref)
                )
        );
        let err = restored
            .send_input(
                &worker.worker_ref,
                WorkerInput::user("after failed restore"),
            )
            .unwrap_err();
        assert!(matches!(err, RuntimeError::WorkerExecutionRejected { .. }));

        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(feature = "fs-store")]
    #[test]
    fn fs_store_reports_corrupt_and_missing_data() {
        let corrupt_root = fs_store_root("corrupt");
        let corrupt_runtime = Runtime::with_fs_store(crate::fs_store::FsRuntimeStoreOptions {
            root: corrupt_root.clone(),
            display_name: None,
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
            display_name: None,
        })
        .unwrap_err();
        assert!(matches!(err, RuntimeError::StoreCorrupt { .. }));
        let _ = std::fs::remove_dir_all(corrupt_root);

        let missing_root = fs_store_root("missing");
        let missing_runtime = Runtime::with_fs_store_and_execution_backend(
            crate::fs_store::FsRuntimeStoreOptions {
                root: missing_root.clone(),
                display_name: None,
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
        let loaded = Runtime::with_fs_store(crate::fs_store::FsRuntimeStoreOptions {
            root: missing_root.clone(),
            display_name: None,
        })
        .expect("invalid worker snapshot should not make runtime store unreadable");
        assert!(loaded.list_workers().unwrap().is_empty());
        assert!(
            loaded
                .diagnostics()
                .unwrap()
                .iter()
                .any(|diagnostic| diagnostic.code == "worker_snapshot_ignored")
        );
        let _ = std::fs::remove_dir_all(missing_root);
    }
}
