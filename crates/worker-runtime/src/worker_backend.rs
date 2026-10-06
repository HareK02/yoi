//! Adapter from `worker-runtime` execution backend boundary to the real
//! `worker` crate controller/run lifecycle.
//!
//! The adapter intentionally owns real `WorkerHandle`s internally and exposes
//! operations addressed by `WorkerRef` to callers. Browser/API
//! projections therefore keep the existing runtime redaction boundary: no raw
//! socket paths, session paths, manifests, credentials, or handles leave this
//! module.

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock, mpsc};
use std::time::Duration;

use crate::auth::{BACKEND_RESOURCE_FETCH_PERMISSION, RuntimeIdentityMaterial};
use crate::catalog::{
    CreateWorkerRequest, ProfileSourceArchiveSource, RepositoryRefObservation,
    RepositoryRefObservationRequest, WorkingDirectoryAttachmentStatus,
    WorkingDirectoryRepositoryAccessRequest, WorkingDirectoryRequest, WorkingDirectoryStatus,
    WorkingDirectoryStatusKind,
};
use crate::config_bundle::{ConfigBundle, workspace_config_etag};
#[cfg(feature = "ws-server")]
use crate::execution::WorkerProtocolTransport;
use crate::execution::{
    WorkerExecutionBackend, WorkerExecutionOperation, WorkerExecutionRestoreRequest,
    WorkerExecutionResult, WorkerExecutionSpawnRequest, WorkerExecutionSpawnResult,
    WorkerExecutionStopRequest, WorkerSessionObservationRequest, WorkspaceConfigFetchRequest,
    WorkspaceConfigFetchResult,
};
use crate::identity::WorkerRef;
use crate::interaction::{WorkerInput, WorkerInputKind};
use crate::resource::BackendResourceClient;
use crate::worker_source::{
    EmbeddedWorkerMutationDispatcher, RuntimeOwnedWorkspaceClient, RuntimeWorkerMutationForwarder,
};
use crate::working_directory::{
    WorkingDirectoryBinding, WorkingDirectoryDiagnostic, WorkingDirectoryMaterializer,
};
use crate::workspace_request::{RuntimeWorkspaceRequest, RuntimeWorkspaceRequestClient};
use async_trait::async_trait;
#[cfg(test)]
use protocol::WorkerStatus;
use protocol::{Event, Method, Segment, WorkerCommandEnvelope};

static NEXT_INTERNAL_COMMAND_ID: AtomicU64 = AtomicU64::new(1);

enum NotificationAcceptance {
    Accepted,
    Rejected(String),
}

fn rollback_materialized_spawn_workdirs(
    materializer: &dyn WorkingDirectoryMaterializer,
    bindings: &BTreeMap<WorkdirAttachmentAlias, WorkingDirectoryBinding>,
    workdir_ids: &[String],
    failure_context: &str,
) -> Result<(), WorkerExecutionSpawnResult> {
    let mut cleanup_failures = Vec::new();
    let mut uncertain_attachments = Vec::new();
    for workdir_id in workdir_ids.iter().rev() {
        if let Err(error) = materializer.cleanup_working_directory(workdir_id) {
            cleanup_failures.push(format!("{workdir_id}: {error}"));
            if let Some((alias, binding)) = bindings
                .iter()
                .find(|(_, binding)| binding.working_directory.id == *workdir_id)
            {
                uncertain_attachments.push(WorkingDirectoryAttachmentStatus {
                    alias: alias.clone(),
                    working_directory: binding.status(),
                });
            }
        }
    }
    if cleanup_failures.is_empty() {
        return Ok(());
    }
    Err(WorkerExecutionSpawnResult::ReconciliationRequired {
        result: WorkerExecutionResult::errored(
            WorkerExecutionOperation::Spawn,
            format!(
                "{failure_context}; Workdir cleanup could not be proven: {}",
                cleanup_failures.join("; ")
            ),
        ),
        worker_state: None,
        workdir_attachments: uncertain_attachments,
    })
}

fn next_internal_command(
    state: &RwLock<protocol::WorkerStateSnapshot>,
) -> Result<WorkerCommandEnvelope, String> {
    let snapshot = state
        .read()
        .map_err(|_| "worker state lock is poisoned".to_string())?
        .clone();
    Ok(next_internal_command_for_snapshot(&snapshot))
}

fn next_internal_command_for_snapshot(
    snapshot: &protocol::WorkerStateSnapshot,
) -> WorkerCommandEnvelope {
    let floor = snapshot.last_command_id.saturating_add(1);
    let command_id = NEXT_INTERNAL_COMMAND_ID
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            Some(current.max(floor).saturating_add(1))
        })
        .unwrap_or(floor)
        .max(floor);
    WorkerCommandEnvelope::new(command_id)
}
use session_store::{
    CombinedStore, Store, WorkerAggregateStore, WorkerMetadataStore, WorkerSessionStore,
};
#[cfg(test)]
use session_store::{FsStore, FsWorkerStore};
use tokio::runtime::Runtime;
#[cfg(feature = "ws-server")]
use tokio::sync::broadcast;
use workdir::{
    LocalWorkdirSession, Workdir, WorkdirAttachmentAlias, WorkdirSessionCapabilities,
    WorkdirSessionHandle, WorkdirSessionRouter,
};

#[cfg(test)]
use worker::WorkerController;
use worker::feature::builtin::manage_workdir::WorkspaceAttachedWorkdirSession;
use worker::feature::builtin::{
    CompositeWorkerObservationProvider, WorkerObservationError, WorkerObservationProvider,
    WorkerObservationSubject, WorkerObservationSubjectRef, WorkerSessionCapture,
    WorkspaceClientWorkerObservationProvider,
};
#[cfg(feature = "ws-server")]
use worker::ipc::protocol_session::{live_log_entry_event, subscribe_worker_protocol_session};
use worker::{
    PreparedWorker, PromptCatalogSource, SegmentLogSink, SubjektivSessionAttributionLifecycle,
    Worker, WorkerBootstrap, WorkerBootstrapError, WorkerBootstrapLayout,
    WorkerControllerTransport, WorkerError, WorkerFilesystemAuthority, WorkerHandle,
    WorkerSharedState, WorkerWorkspaceContext, WorkspaceClient, WorkspaceId,
    bash_output_dir_for_worker_id,
};

const DEFAULT_BACKEND_ID: &str = "worker-crate";
const RUNTIME_TASK_TIMEOUT: Duration = Duration::from_secs(10);
const SPAWN_RESTORE_TASK_TIMEOUT: Duration = Duration::from_secs(60);
const USER_INPUT_TASK_TIMEOUT: Duration = Duration::from_secs(10);
const WORKSPACE_CONFIG_HTTP_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_WORKSPACE_CONFIG_RESPONSE_BYTES: usize = 72 * 1024 * 1024;
// Leave adapter cancellation margin after the durable submission deadline.
const USER_INPUT_COMMIT_TIMEOUT: Duration = Duration::from_secs(9);

pub struct RuntimeWorkerController {
    pub handle: WorkerHandle,
    pub shutdown: Arc<tokio::sync::Mutex<Option<worker::ShutdownReceiver>>>,
    pub controller_task: tokio::task::JoinHandle<()>,
    pub workspace_client: Arc<dyn WorkspaceClient>,
}

/// Factory seam used by [`WorkerRuntimeExecutionBackend`] to construct a real
/// controller-backed Worker for a Runtime catalog entry.
#[async_trait]
pub trait RuntimeWorkerFactory: Send + Sync + 'static {
    fn observe_workspace_prompt_projection(
        &self,
        _projection: worker::WorkspacePromptProjection,
    ) -> Result<(), String> {
        Ok(())
    }

    async fn fetch_workspace_config(
        &self,
        _request: WorkspaceConfigFetchRequest,
    ) -> Result<WorkspaceConfigFetchResult, String> {
        Err("Runtime Worker factory does not support Workspace Config fetching".to_string())
    }

    fn retained_session_snapshot(
        &self,
        _worker_ref: &WorkerRef,
    ) -> Result<session_store::RetainedSessionSnapshot, session_store::RetainedSnapshotReadError>
    {
        Err(session_store::RetainedSnapshotReadError::RetentionMissing)
    }

    fn retained_session_history_page(
        &self,
        _worker_ref: &WorkerRef,
        _cursor: Option<&str>,
        _limit: Option<usize>,
    ) -> Result<protocol::SessionHistoryPage, session_store::RetainedHistoryReadError> {
        Err(session_store::RetainedHistoryReadError::RetentionMissing)
    }

    fn retained_session_attachment(
        &self,
        _worker_ref: &WorkerRef,
        _session_id: &str,
        _attachment_id: &str,
    ) -> Result<session_store::RetainedSessionAttachment, session_store::RetainedAttachmentReadError>
    {
        Err(session_store::RetainedAttachmentReadError::RetentionMissing)
    }

    async fn spawn_controller(
        &self,
        request: WorkerExecutionSpawnRequest,
    ) -> Result<RuntimeWorkerController, String>;

    async fn preflight_restore(
        &self,
        _request: &WorkerExecutionRestoreRequest,
    ) -> Result<(), String> {
        Ok(())
    }

    async fn restore_controller(
        &self,
        request: WorkerExecutionRestoreRequest,
    ) -> Result<RuntimeWorkerController, String>;

    /// Prove cleanup of a restore future that completed without returning a
    /// controller. An error is not absence evidence; unsupported factories keep
    /// the operation-owned resource record for retry.
    async fn cleanup_failed_restore(
        &self,
        _request: &WorkerExecutionRestoreRequest,
    ) -> Result<(), String> {
        Err("Runtime Worker factory cannot prove failed restore cleanup".to_string())
    }

    /// Verify/release factory-owned resources when the backend registry has no
    /// Controller. Factories retaining resources outside returned controllers
    /// must override this; it must never start a new Controller.
    async fn reconcile_stopped_worker(&self, _worker_ref: &WorkerRef) -> Result<(), String> {
        Ok(())
    }

    fn activate_restored_controller(
        &self,
        _worker_ref: &WorkerRef,
        _workspace_id: Option<&str>,
        _handle: &WorkerHandle,
    ) {
    }
}

/// Production factory that resolves a normal Worker profile and spawns it under
/// `WorkerController`.
#[derive(Default)]
struct RuntimeWorkerObservationHub {
    workers: Mutex<HashMap<WorkerRef, RuntimeObservedWorker>>,
}

#[derive(Clone)]
struct RuntimeObservedWorker {
    workspace_id: Option<String>,
    shared_state: std::sync::Weak<WorkerSharedState>,
    sink: SegmentLogSink,
}

impl RuntimeWorkerObservationHub {
    fn register(&self, worker_ref: WorkerRef, workspace_id: Option<String>, handle: &WorkerHandle) {
        if let Ok(mut workers) = self.workers.lock() {
            workers.insert(
                worker_ref,
                RuntimeObservedWorker {
                    workspace_id,
                    shared_state: Arc::downgrade(&handle.shared_state),
                    sink: handle.sink.clone(),
                },
            );
        }
    }

    fn get(
        &self,
        worker_ref: &WorkerRef,
    ) -> Option<(Option<String>, Arc<WorkerSharedState>, SegmentLogSink)> {
        let mut workers = self.workers.lock().ok()?;
        let entry = workers.get(worker_ref)?.clone();
        let Some(shared_state) = entry.shared_state.upgrade() else {
            workers.remove(worker_ref);
            return None;
        };
        Some((entry.workspace_id, shared_state, entry.sink))
    }
}

struct RuntimeGrantedWorkerObservationProvider {
    runtime_id: String,
    workspace_id: String,
    grants: std::collections::HashSet<crate::identity::RuntimeWorkerRef>,
    hub: Arc<RuntimeWorkerObservationHub>,
}

#[async_trait]
impl WorkerObservationProvider for RuntimeGrantedWorkerObservationProvider {
    async fn list_worker_sessions(
        &self,
    ) -> Result<Vec<WorkerObservationSubject>, WorkerObservationError> {
        let mut subjects = Vec::new();
        for grant in &self.grants {
            if grant.runtime_id != self.runtime_id {
                continue;
            }
            let Ok(worker_ref) = WorkerRef::try_from(grant) else {
                continue;
            };
            let Some((workspace_id, state, _)) = self.hub.get(&worker_ref) else {
                continue;
            };
            if workspace_id.as_deref() != Some(self.workspace_id.as_str()) {
                continue;
            }
            subjects.push(WorkerObservationSubject {
                subject: WorkerObservationSubjectRef::RuntimeWorker {
                    runtime_id: grant.runtime_id.clone(),
                    worker_id: grant.worker_id.clone(),
                },
                display_name: grant.worker_id.clone(),
                relation: "granted_peer".to_string(),
                status: format!("{:?}", state.catalog_status()).to_lowercase(),
            });
        }
        subjects.sort_by(|left, right| left.subject.cmp(&right.subject));
        Ok(subjects)
    }

    async fn capture_worker_session(
        &self,
        subject: &WorkerObservationSubjectRef,
    ) -> Result<WorkerSessionCapture, WorkerObservationError> {
        let WorkerObservationSubjectRef::RuntimeWorker {
            runtime_id,
            worker_id,
        } = subject
        else {
            return Err(WorkerObservationError::NotFound);
        };
        let grant = crate::identity::RuntimeWorkerRef::new(runtime_id.clone(), worker_id.clone());
        if !self.grants.contains(&grant) || runtime_id != &self.runtime_id {
            return Err(WorkerObservationError::NotFound);
        }
        let worker_ref =
            WorkerRef::try_from(&grant).map_err(|_| WorkerObservationError::NotFound)?;
        let (workspace_id, _, sink) = self
            .hub
            .get(&worker_ref)
            .ok_or(WorkerObservationError::NotFound)?;
        if workspace_id.as_deref() != Some(self.workspace_id.as_str()) {
            return Err(WorkerObservationError::NotFound);
        }
        let entries = sink.subscribe_with_snapshot().0;
        WorkerSessionCapture::from_log_entries(
            format!("runtime:{runtime_id}:worker:{worker_id}"),
            &entries,
        )
        .map_err(WorkerObservationError::Unavailable)
    }
}

#[derive(Debug, Default)]
pub(crate) struct WorkspacePromptProjectionCache {
    active: Mutex<HashMap<String, Arc<worker::WorkspacePromptCatalogResolution>>>,
    fetch_gates: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl WorkspacePromptProjectionCache {
    pub(crate) fn fetch_gate(&self, workspace_id: &str) -> Result<Arc<Mutex<()>>, String> {
        let mut gates = self
            .fetch_gates
            .lock()
            .map_err(|_| "Workspace Prompt projection fetch gates lock was poisoned".to_string())?;
        Ok(gates
            .entry(workspace_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone())
    }

    pub(crate) fn active(
        &self,
        workspace_id: &str,
    ) -> Result<Option<Arc<worker::WorkspacePromptCatalogResolution>>, String> {
        self.active
            .lock()
            .map(|active| active.get(workspace_id).cloned())
            .map_err(|_| "Workspace Prompt projection cache lock was poisoned".to_string())
    }

    pub(crate) fn observe(
        &self,
        projection: worker::WorkspacePromptProjection,
    ) -> Result<Arc<worker::WorkspacePromptCatalogResolution>, String> {
        projection.validate().map_err(|error| error.to_string())?;
        let workspace_id = projection.workspace_id.clone();
        let resolution = Arc::new(
            worker::WorkspacePromptCatalogResolution::new(projection)
                .map_err(|error| error.to_string())?,
        );
        let mut active = self
            .active
            .lock()
            .map_err(|_| "Workspace Prompt projection cache lock was poisoned".to_string())?;
        if let Some(current) = active.get(&workspace_id) {
            if current.projection.config_revision > resolution.projection.config_revision {
                return Ok(current.clone());
            }
            if current.projection.config_revision == resolution.projection.config_revision
                && (current.projection.source_digest != resolution.projection.source_digest
                    || current.projection.projection_digest
                        != resolution.projection.projection_digest
                    || current.projection.catalog.catalog_digest
                        != resolution.projection.catalog.catalog_digest
                    || current.projection.catalog.schema_fingerprint
                        != resolution.projection.catalog.schema_fingerprint
                    || current.projection.catalog.toolchain_fingerprint
                        != resolution.projection.catalog.toolchain_fingerprint)
            {
                return Err(format!(
                    "Workspace Prompt projection identity changed without a config revision transition: workspace={workspace_id} revision={}",
                    resolution.projection.config_revision
                ));
            }
        }
        active.insert(workspace_id, resolution.clone());
        Ok(resolution)
    }
}

#[derive(Clone)]
pub struct ProfileRuntimeWorkerFactory {
    observation_hub: Arc<RuntimeWorkerObservationHub>,
    profile_base_dir: PathBuf,
    worker_aggregate_root: Option<PathBuf>,
    resource_client: Option<Arc<dyn BackendResourceClient>>,
    prompt_projection_cache: Arc<WorkspacePromptProjectionCache>,
    runtime_id: Option<String>,
    worker_mutation_identity: Option<RuntimeIdentityMaterial>,
    workspace_request_clients: Arc<HashMap<String, RuntimeWorkspaceRequestClient>>,
    embedded_worker_mutation_dispatcher: Option<Arc<dyn EmbeddedWorkerMutationDispatcher>>,
    controller_transport: WorkerControllerTransport,
    failed_restore_sessions: Arc<
        Mutex<
            HashMap<
                WorkerRef,
                (
                    crate::execution::WorkerLifecycleOperationId,
                    Arc<WorkdirSessionRouter>,
                ),
            >,
        >,
    >,
}

impl ProfileRuntimeWorkerFactory {
    pub fn new(profile_base_dir: impl Into<PathBuf>) -> Self {
        let profile_base_dir = profile_base_dir.into();
        Self {
            observation_hub: Arc::new(RuntimeWorkerObservationHub::default()),
            profile_base_dir,
            worker_aggregate_root: None,
            resource_client: None,
            prompt_projection_cache: Arc::new(WorkspacePromptProjectionCache::default()),
            runtime_id: None,
            worker_mutation_identity: None,
            workspace_request_clients: Arc::new(HashMap::new()),
            embedded_worker_mutation_dispatcher: None,
            controller_transport: WorkerControllerTransport::InProcess,
            failed_restore_sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn with_runtime_id(mut self, runtime_id: impl Into<String>) -> Self {
        self.runtime_id = Some(runtime_id.into());
        self
    }

    pub fn with_remote_worker_mutation_identity(
        mut self,
        identity: RuntimeIdentityMaterial,
    ) -> Self {
        self.runtime_id = Some(identity.identity_id.clone());
        self.worker_mutation_identity = Some(identity);
        self.embedded_worker_mutation_dispatcher = None;
        self
    }

    pub fn with_workspace_request_client(mut self, client: RuntimeWorkspaceRequestClient) -> Self {
        self.runtime_id = Some(client.runtime_id().to_string());
        Arc::make_mut(&mut self.workspace_request_clients)
            .insert(client.workspace_id().to_string(), client);
        self
    }

    pub fn with_embedded_worker_mutation_dispatcher(
        mut self,
        runtime_id: impl Into<String>,
        dispatcher: Arc<dyn EmbeddedWorkerMutationDispatcher>,
    ) -> Self {
        self.runtime_id = Some(runtime_id.into());
        self.worker_mutation_identity = None;
        self.embedded_worker_mutation_dispatcher = Some(dispatcher);
        self
    }

    pub fn with_controller_transport(
        mut self,
        controller_transport: WorkerControllerTransport,
    ) -> Self {
        self.controller_transport = controller_transport;
        self
    }

    pub fn with_runtime_store_dir(mut self, runtime_store_dir: impl Into<PathBuf>) -> Self {
        self.worker_aggregate_root = Some(runtime_store_dir.into().join("workers"));
        self
    }

    pub fn with_resource_client(mut self, resource_client: Arc<dyn BackendResourceClient>) -> Self {
        self.resource_client = Some(resource_client);
        self
    }

    fn worker_aggregate_dir(&self, worker_ref: &WorkerRef) -> Result<PathBuf, String> {
        self.worker_aggregate_root
            .as_ref()
            .map(|root| root.join(worker_ref.worker_id.to_string()))
            .ok_or_else(|| {
                "Runtime Worker aggregate root is not configured; global Session/metadata roots are migration-only"
                    .to_string()
            })
    }

    fn worker_run_dir(&self, worker_ref: &WorkerRef) -> Result<PathBuf, String> {
        Ok(self
            .worker_aggregate_dir(worker_ref)?
            .join("runs")
            .join(uuid::Uuid::now_v7().to_string()))
    }

    fn worker_restore_run_dir(
        &self,
        worker_ref: &WorkerRef,
        operation_id: crate::execution::WorkerLifecycleOperationId,
    ) -> Result<PathBuf, String> {
        Ok(self
            .worker_aggregate_dir(worker_ref)?
            .join("runs")
            .join(format!("restore-{operation_id}")))
    }

    // Called only after the backend has excluded actual and pending executions
    // under its per-Worker operation lock. Persisted run artifacts survive a
    // process restart even when the in-memory failed-session map does not.
    fn stopped_restore_run_dirs(&self, worker_ref: &WorkerRef) -> Result<Vec<PathBuf>, String> {
        let aggregate = self.worker_aggregate_dir(worker_ref)?;
        let root = aggregate
            .parent()
            .expect("Worker aggregate has configured parent");
        let runs = aggregate.join("runs");
        for path in [root, aggregate.as_path(), runs.as_path()] {
            match std::fs::symlink_metadata(path) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
                Ok(_) => {
                    return Err(format!(
                        "unsafe restore artifact directory: {}",
                        path.display()
                    ));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
                Err(error) => {
                    return Err(format!(
                        "inspect restore artifact directory {}: {error}",
                        path.display()
                    ));
                }
            }
        }
        let mut candidates = Vec::new();
        for entry in std::fs::read_dir(&runs)
            .map_err(|error| format!("read restore artifact directory: {error}"))?
        {
            let entry = entry.map_err(|error| format!("read restore artifact entry: {error}"))?;
            let name = entry.file_name();
            let name = name
                .to_str()
                .ok_or_else(|| "invalid restore artifact run name".to_string())?;
            let Some(operation) = name.strip_prefix("restore-") else {
                continue;
            };
            let id = uuid::Uuid::parse_str(operation)
                .map_err(|_| format!("invalid restore artifact operation name: {name}"))?;
            if id.to_string() != operation {
                return Err(format!(
                    "noncanonical restore artifact operation name: {name}"
                ));
            }
            let path = entry.path();
            Self::validate_restore_artifact_tree(&path, true)?;
            candidates.push(path);
        }
        candidates.sort();
        Ok(candidates)
    }

    fn validate_restore_artifact_tree(path: &Path, require_directory: bool) -> Result<(), String> {
        let metadata = std::fs::symlink_metadata(path)
            .map_err(|error| format!("inspect restore artifact {}: {error}", path.display()))?;
        if metadata.file_type().is_symlink() || (require_directory && !metadata.is_dir()) {
            return Err(format!("unsafe restore artifact: {}", path.display()));
        }
        if metadata.is_dir() {
            for entry in std::fs::read_dir(path)
                .map_err(|error| format!("read restore artifact {}: {error}", path.display()))?
            {
                let entry = entry.map_err(|error| format!("read restore artifact: {error}"))?;
                Self::validate_restore_artifact_tree(&entry.path(), false)?;
            }
        }
        Ok(())
    }

    fn remove_restore_run_artifacts(run_dir: &Path) -> Result<(), String> {
        match std::fs::symlink_metadata(run_dir) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(format!("inspect restore run artifacts: {error}")),
            Ok(_) => Self::validate_restore_artifact_tree(run_dir, true)?,
        }
        std::fs::remove_dir_all(run_dir)
            .map_err(|error| format!("failed to clean restore run artifacts: {error}"))
    }

    fn runtime_worker_name_for_ref(worker_ref: &crate::identity::WorkerRef) -> String {
        format!("worker-runtime-{}", worker_ref.worker_id)
    }

    fn runtime_worker_name(request: &WorkerExecutionSpawnRequest) -> String {
        Self::runtime_worker_name_for_ref(&request.worker_ref)
    }

    fn runtime_profile_value(
        profile: &crate::catalog::ProfileSelector,
    ) -> std::borrow::Cow<'_, str> {
        match profile {
            crate::catalog::ProfileSelector::Named(name) => {
                std::borrow::Cow::Borrowed(name.as_str())
            }
            crate::catalog::ProfileSelector::Builtin(name) => {
                if name.starts_with("builtin:") {
                    std::borrow::Cow::Borrowed(name.as_str())
                } else {
                    std::borrow::Cow::Owned(format!("builtin:{name}"))
                }
            }
        }
    }

    fn runtime_profile_for_request(request: &CreateWorkerRequest) -> std::borrow::Cow<'_, str> {
        Self::runtime_profile_value(&request.profile)
    }

    fn runtime_profile(request: &WorkerExecutionSpawnRequest) -> std::borrow::Cow<'_, str> {
        Self::runtime_profile_for_request(&request.request)
    }

    fn restore_fallback_manifest(
        worker_name: &str,
    ) -> Result<(manifest::WorkerManifest, PromptCatalogSource), String> {
        let mut config = manifest::WorkerManifestConfig::resolution_defaults();
        config.worker.name = Some(worker_name.to_string());
        let manifest = manifest::WorkerManifest::try_from(config)
            .map_err(|err| format!("failed to build restore fallback manifest: {err}"))?;
        Ok((manifest, PromptCatalogSource::builtins_only()))
    }
    fn manifest_for_restore(
        metadata: &session_store::WorkerMetadata,
        request: &CreateWorkerRequest,
    ) -> Result<(manifest::WorkerManifest, PromptCatalogSource), String> {
        let (fallback, loader) = Self::restore_fallback_manifest(&metadata.worker_name)?;
        let manifest = match metadata.resolved_manifest_snapshot.clone() {
            Some(snapshot) => manifest::read_persisted_worker_manifest_snapshot(snapshot)
                .map_err(|error| format!("failed to read saved Worker Manifest: {error}"))?,
            None if request.workspace_api.is_some() || request.subjektiv_attached => {
                return Err("Workspace Worker metadata has no saved Manifest; replacement Worker is required".to_string());
            }
            None => fallback,
        };
        // Saved policy is execution authority. Rebinding current settings here
        // would hide revision and attachment mismatches.
        validate_worker_memory_settings(&manifest, request)?;
        Ok((manifest, loader))
    }

    fn observe_bundle_prompt_projection(
        &self,
        bundle: &crate::config_bundle::ConfigBundle,
        expected_workspace_id: Option<&str>,
    ) -> Result<Option<Arc<worker::WorkspacePromptCatalogResolution>>, String> {
        let Some(prompt_catalog) = bundle.prompt_catalog.clone() else {
            return Ok(None);
        };
        if let Some(expected_workspace_id) = expected_workspace_id
            && bundle.metadata.workspace_id != expected_workspace_id
        {
            return Err(format!(
                "Workspace Prompt projection scope mismatch: expected {expected_workspace_id}, got {}",
                bundle.metadata.workspace_id
            ));
        }
        let source_digest = if prompt_catalog.source_digest.is_empty() {
            bundle
                .metadata
                .provenance
                .detail
                .as_deref()
                .and_then(|detail| {
                    detail
                        .split(';')
                        .find_map(|part| part.strip_prefix("source_tree_digest="))
                })
                .unwrap_or(&prompt_catalog.catalog_digest)
                .to_string()
        } else {
            prompt_catalog.source_digest.clone()
        };
        let projection = worker::WorkspacePromptProjection::new(
            bundle.metadata.workspace_id.clone(),
            source_digest,
            prompt_catalog.catalog_digest.clone(),
            prompt_catalog,
        )
        .map_err(|error| error.to_string())?;
        self.prompt_projection_cache.observe(projection).map(Some)
    }

    async fn resolve_profile_source_archive(
        &self,
        source: &ProfileSourceArchiveSource,
        config_bundle: Option<&ConfigBundle>,
    ) -> Result<crate::profile_archive::VerifiedProfileSourceArchive, String> {
        match source {
            ProfileSourceArchiveSource::Embedded { archive } => archive
                .verify()
                .map_err(|err| format!("failed to verify embedded profile source archive: {err}")),
            ProfileSourceArchiveSource::WorkspaceConfig { archive } => {
                let bundle = config_bundle.ok_or_else(|| {
                    "Workspace Config profile source requires a resolved Config Bundle".to_string()
                })?;
                let embedded = bundle.profile_source_archive.as_ref().ok_or_else(|| {
                    "resolved Workspace Config is missing its profile source archive".to_string()
                })?;
                if &embedded.reference != archive {
                    return Err(
                        "Workspace Config profile source archive reference mismatch".to_string()
                    );
                }
                embedded.verify().map_err(|err| {
                    format!("failed to verify Workspace Config profile source archive: {err}")
                })
            }
        }
    }
}

#[derive(Debug, Clone)]
enum RuntimeWorkspaceBackendRef {
    None,
    Http {
        workspace_id: String,
        base_url: String,
        runtime_id: String,
    },
}

impl RuntimeWorkspaceBackendRef {
    fn from_worker_request(request: &CreateWorkerRequest, runtime_id: Option<&str>) -> Self {
        if let (Some(api), Some(runtime_id)) = (request.workspace_api.as_ref(), runtime_id) {
            return Self::Http {
                workspace_id: api.workspace_id.clone(),
                base_url: api.base_url.clone(),
                runtime_id: runtime_id.to_string(),
            };
        }
        Self::None
    }

    fn worker_context(
        &self,
        worker_ref: &WorkerRef,
        workspace_scope: Option<&crate::runtime::RuntimeWorkspaceScope>,
        mutation_identity: Option<&RuntimeIdentityMaterial>,
        workspace_request_client: Option<&RuntimeWorkspaceRequestClient>,
        embedded_dispatcher: Option<&Arc<dyn EmbeddedWorkerMutationDispatcher>>,
        prompt_projection_cache: Option<Arc<WorkspacePromptProjectionCache>>,
    ) -> WorkerWorkspaceContext {
        match self {
            Self::None => WorkerWorkspaceContext::no_workspace(),
            Self::Http {
                workspace_id,
                base_url,
                runtime_id,
            } => {
                let mut client = workspace_request_client
                    .cloned()
                    .map(|request_client| {
                        RuntimeOwnedWorkspaceClient::from_request_client(
                            request_client,
                            worker_ref.worker_id.to_string(),
                        )
                    })
                    .unwrap_or_else(|| {
                        RuntimeOwnedWorkspaceClient::new(
                            workspace_id.clone(),
                            base_url.clone(),
                            runtime_id.clone(),
                            worker_ref.worker_id.to_string(),
                        )
                    });
                if let Some(cache) = prompt_projection_cache {
                    client = client.with_prompt_projection_cache(cache);
                }
                if let (Some(scope), Some(identity), Some(request_client)) =
                    (workspace_scope, mutation_identity, workspace_request_client)
                {
                    client = client.with_worker_remove(RuntimeWorkerMutationForwarder::remote(
                        identity,
                        scope.clone(),
                        worker_ref.worker_id.to_string(),
                        request_client.clone(),
                    ));
                } else if let (Some(scope), Some(dispatcher)) =
                    (workspace_scope, embedded_dispatcher)
                {
                    client = client.with_worker_remove(RuntimeWorkerMutationForwarder::embedded(
                        runtime_id,
                        scope.clone(),
                        worker_ref.worker_id.to_string(),
                        (*dispatcher).clone(),
                    ));
                }
                WorkerWorkspaceContext::with_client(
                    WorkspaceId::new(workspace_id.clone()).ok(),
                    Arc::new(client),
                )
            }
        }
    }
}

#[cfg(feature = "http-server")]
async fn fetch_workspace_config_http(
    request: &WorkspaceConfigFetchRequest,
    client: &RuntimeWorkspaceRequestClient,
) -> Result<WorkspaceConfigFetchResult, String> {
    if !client.matches_workspace(
        &request.workspace_api.workspace_id,
        &request.workspace_api.base_url,
    ) {
        return Err(format!(
            "Workspace request route does not match Workspace Config source: workspace={} base_url={}",
            request.workspace_api.workspace_id, request.workspace_api.base_url
        ));
    }
    let mut url = reqwest::Url::parse(client.base_url())
        .map_err(|error| format!("Workspace API base URL is invalid: {error}"))?;
    url.set_path(&format!(
        "/api/w/{}/runtime-config",
        request.workspace_api.workspace_id
    ));
    let profile = match &request.profile {
        crate::catalog::ProfileSelector::Builtin(value)
        | crate::catalog::ProfileSelector::Named(value) => value.clone(),
    };
    url.query_pairs_mut().append_pair("profile", &profile);
    let mut headers = reqwest::header::HeaderMap::new();
    if let Some(cached) = request.cached.as_ref() {
        headers.insert(
            reqwest::header::IF_NONE_MATCH,
            reqwest::header::HeaderValue::from_str(&workspace_config_etag(&cached.digest))
                .map_err(|error| format!("Workspace Config ETag is invalid: {error}"))?,
        );
    }
    let mut path_and_query = url.path().to_string();
    if let Some(query) = url.query() {
        path_and_query.push('?');
        path_and_query.push_str(query);
    }
    let response = client
        .execute(RuntimeWorkspaceRequest {
            method: reqwest::Method::GET,
            path_and_query,
            body: Vec::new(),
            headers,
            permission: BACKEND_RESOURCE_FETCH_PERMISSION.to_string(),
            worker_id: None,
            timeout: Some(WORKSPACE_CONFIG_HTTP_TIMEOUT),
            max_response_bytes: MAX_WORKSPACE_CONFIG_RESPONSE_BYTES,
        })
        .await
        .map_err(|error| format!("failed to fetch latest Workspace Config: {error}"))?;
    if response.status == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(WorkspaceConfigFetchResult::NotModified);
    }
    if !response.status.is_success() {
        return Err(format!(
            "latest Workspace Config fetch failed with HTTP {}",
            response.status
        ));
    }
    let response_etag = response
        .headers
        .get(reqwest::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .ok_or_else(|| "latest Workspace Config response is missing its ETag".to_string())?;
    let bundle = serde_json::from_slice::<ConfigBundle>(&response.body)
        .map_err(|error| format!("failed to decode latest Workspace Config: {error}"))?;
    let expected_etag = workspace_config_etag(&bundle.metadata.digest);
    if response_etag != expected_etag {
        return Err(format!(
            "latest Workspace Config ETag mismatch: expected {expected_etag}, got {response_etag}"
        ));
    }
    Ok(WorkspaceConfigFetchResult::Modified(bundle))
}

#[cfg(not(feature = "http-server"))]
async fn fetch_workspace_config_http<T>(
    _request: &WorkspaceConfigFetchRequest,
    _client: &T,
) -> Result<WorkspaceConfigFetchResult, String> {
    Err("Workspace Config fetch requires the worker-runtime http-server feature".to_string())
}

fn initial_workdir_session_capabilities(
    alias: &WorkdirAttachmentAlias,
    claims: &[crate::catalog::WorkingDirectoryAttachmentClaim],
) -> WorkdirSessionCapabilities {
    claims
        .iter()
        .find(|claim| &claim.alias == alias)
        .map(|claim| claim.capabilities)
        // New Runtime materialization requests have no claim and retain their
        // established full local-session behavior.
        .unwrap_or(WorkdirSessionCapabilities::ALL)
}

fn runtime_attachment_scope(
    root: &Path,
    capabilities: WorkdirSessionCapabilities,
) -> Result<manifest::SharedScope, String> {
    if capabilities == WorkdirSessionCapabilities::EMPTY {
        return Ok(manifest::SharedScope::new(manifest::Scope::empty()));
    }
    let writable = [
        workdir::WorkdirSessionCapability::Write,
        workdir::WorkdirSessionCapability::Edit,
        workdir::WorkdirSessionCapability::Command,
    ]
    .into_iter()
    .any(|capability| capabilities.supports(capability));
    let scope = manifest::Scope::from_config(&manifest::ScopeConfig {
        allow: vec![manifest::ScopeRule {
            target: root.to_path_buf(),
            permission: if writable {
                manifest::Permission::Write
            } else {
                manifest::Permission::Read
            },
            recursive: true,
            symlink_policy: manifest::SymlinkPolicy::Resolved,
        }],
        deny: Vec::new(),
    })
    .map_err(|error| format!("create Runtime-local attachment scope: {error}"))?;
    Ok(manifest::SharedScope::new(scope))
}

fn runtime_local_workdir_session(
    workdir_id: &str,
    root: &Path,
    cwd: &Path,
    scope: manifest::SharedScope,
    command_output_scope: manifest::SharedScope,
    capabilities: WorkdirSessionCapabilities,
    command_environment: std::collections::BTreeMap<String, String>,
    resources: Vec<Arc<dyn workdir::WorkdirSessionResource>>,
) -> WorkdirSessionHandle {
    Arc::new(
        LocalWorkdirSession::materialized_bound_with_environment_and_command_output_scope(
            Workdir::new(workdir_id),
            root.to_path_buf(),
            cwd.to_path_buf(),
            scope,
            command_output_scope,
            capabilities,
            command_environment,
            resources,
        ),
    )
}

fn runtime_local_workdir_router(
    attachments: &BTreeMap<WorkdirAttachmentAlias, WorkingDirectoryBinding>,
    capabilities: &BTreeMap<WorkdirAttachmentAlias, WorkdirSessionCapabilities>,
    command_output_scope: manifest::SharedScope,
) -> Result<Arc<WorkdirSessionRouter>, String> {
    let router = Arc::new(WorkdirSessionRouter::new());
    for (alias, binding) in attachments {
        let capabilities = capabilities
            .get(alias)
            .copied()
            .unwrap_or(WorkdirSessionCapabilities::READ_ONLY);
        let scope = runtime_attachment_scope(binding.root(), capabilities)?;
        router
            .attach(
                alias.clone(),
                runtime_local_workdir_session(
                    &binding.working_directory.id,
                    binding.root(),
                    binding.cwd(),
                    scope,
                    command_output_scope.clone(),
                    capabilities,
                    binding.command_environment(),
                    binding.session_resources(),
                ),
            )
            .map_err(|error| format!("bind Workdir attachment `{alias}`: {error}"))?;
    }
    Ok(router)
}

fn restored_workdir_router(
    local_attachments: &BTreeMap<WorkdirAttachmentAlias, WorkingDirectoryBinding>,
    logical_attachments: &[crate::catalog::LogicalWorkdirAttachment],
    claims: &[crate::catalog::WorkingDirectoryAttachmentClaim],
    scope: manifest::SharedScope,
    workspace_client: Arc<dyn WorkspaceClient>,
) -> Result<Arc<WorkdirSessionRouter>, String> {
    let mut local_capabilities = local_attachments
        .keys()
        .map(|alias| {
            (
                alias.clone(),
                initial_workdir_session_capabilities(alias, claims),
            )
        })
        .collect::<BTreeMap<_, _>>();
    for attachment in logical_attachments {
        if local_attachments.contains_key(&attachment.alias) {
            local_capabilities.insert(attachment.alias.clone(), attachment.capabilities);
        }
    }
    let router = runtime_local_workdir_router(local_attachments, &local_capabilities, scope)?;
    for attachment in logical_attachments {
        if let Some(local) = local_attachments.get(&attachment.alias) {
            if local.working_directory.id != attachment.working_directory_id {
                return Err(format!(
                    "logical Workdir attachment `{}` conflicts with Runtime-local Workdir {}",
                    attachment.alias, local.working_directory.id
                ));
            }
            continue;
        }
        router
            .attach(
                attachment.alias.clone(),
                WorkspaceAttachedWorkdirSession::handle_for_workdir_with_capabilities(
                    workspace_client.clone(),
                    attachment.alias.as_str(),
                    &attachment.working_directory_id,
                    attachment.capabilities,
                ),
            )
            .map_err(|error| {
                format!(
                    "bind logical Workdir attachment `{}`: {error}",
                    attachment.alias
                )
            })?;
    }
    Ok(router)
}

fn validate_backend_job_profile(
    manifest: &manifest::WorkerManifest,
    request: &CreateWorkerRequest,
) -> Result<(), String> {
    if let Some(binding) = &request.backend_job {
        let policy = &manifest.feature.subjektiv.profile;
        if policy.consolidation_tools != binding.subjektiv_consolidation
            || (binding.subjektiv_consolidation && policy.extraction.enabled)
        {
            return Err("Backend Job consolidation_tools must match its trusted consolidation grant and extraction must be disabled".into());
        }
    }
    Ok(())
}

fn bind_workspace_memory_settings(
    manifest: &mut manifest::WorkerManifest,
    request: &CreateWorkerRequest,
) -> Result<(), String> {
    validate_backend_job_profile(manifest, request)?;
    let Some(snapshot) = request.memory_settings.as_ref() else {
        if request.workspace_api.is_some() || request.subjektiv_attached {
            return Err(
                "Workspace Worker request is missing its bound Memory settings snapshot"
                    .to_string(),
            );
        }
        return Ok(());
    };
    if let Some(workspace_api) = request.workspace_api.as_ref()
        && snapshot.workspace_id != workspace_api.workspace_id
    {
        return Err(format!(
            "Memory settings workspace {} does not match Workspace API scope {}",
            snapshot.workspace_id, workspace_api.workspace_id
        ));
    }
    if manifest.feature.memory.profile.enabled {
        manifest
            .feature
            .memory
            .bind_workspace_settings(snapshot.clone())
            .map_err(str::to_string)?;
    }
    if request.subjektiv_attached {
        if !manifest.feature.subjektiv.profile.enabled {
            return Err(
                "subject-attached Worker profile does not enable subjektiv policy".to_string(),
            );
        }
        manifest
            .feature
            .subjektiv
            .bind_workspace_settings(snapshot.clone())
            .map_err(str::to_string)?;
    }
    manifest
        .feature
        .memory
        .validate_execution()
        .map_err(str::to_string)?;
    manifest
        .feature
        .subjektiv
        .validate_execution()
        .map_err(str::to_string)?;
    Ok(())
}

fn validate_worker_memory_settings(
    manifest: &manifest::WorkerManifest,
    request: &CreateWorkerRequest,
) -> Result<(), String> {
    validate_backend_job_profile(manifest, request)?;
    let Some(expected) = request.memory_settings.as_ref() else {
        if request.subjektiv_attached || request.workspace_api.is_some() {
            return Err(
                "Workspace Worker restore is missing its trusted settings snapshot".to_string(),
            );
        }
        return Ok(());
    };
    if let Some(workspace_api) = request.workspace_api.as_ref()
        && expected.workspace_id != workspace_api.workspace_id
    {
        return Err("trusted Memory settings do not match Workspace API scope".to_string());
    }
    manifest
        .feature
        .memory
        .validate_execution()
        .map_err(str::to_string)?;
    manifest
        .feature
        .subjektiv
        .validate_execution()
        .map_err(str::to_string)?;
    if manifest.feature.memory.profile.enabled
        && manifest.feature.memory.workspace_settings().as_ref() != Some(expected)
    {
        let actual = manifest
            .feature
            .memory
            .workspace_settings()
            .ok_or_else(|| {
                "Workspace Worker restored without its bound Memory settings snapshot".to_string()
            })?;
        return Err(format!(
            "Workspace Worker Memory settings snapshot mismatch: expected {} revision {}, restored {} revision {}",
            expected.workspace_id,
            expected.settings_revision,
            actual.workspace_id,
            actual.settings_revision
        ));
    }
    let subjektiv_settings = manifest.feature.subjektiv.workspace_settings();
    if request.subjektiv_attached {
        if !manifest.feature.subjektiv.profile.enabled {
            return Err(
                "subject-attached Worker restored with a Profile that disables subjektiv policy"
                    .to_string(),
            );
        }
        if subjektiv_settings.as_ref() != Some(expected) {
            let actual = subjektiv_settings.ok_or_else(|| {
                "subject-attached Worker restored without its bound subjektiv settings snapshot"
                    .to_string()
            })?;
            return Err(format!(
                "Workspace Worker subjektiv settings snapshot mismatch: expected {} revision {}, restored {} revision {}",
                expected.workspace_id,
                expected.settings_revision,
                actual.workspace_id,
                actual.settings_revision
            ));
        }
    } else if subjektiv_settings.is_some() {
        return Err(
            "ordinary Workspace Worker restored with an unauthorized subjektiv attachment"
                .to_string(),
        );
    }
    Ok(())
}

#[async_trait]
impl RuntimeWorkerFactory for ProfileRuntimeWorkerFactory {
    async fn fetch_workspace_config(
        &self,
        request: WorkspaceConfigFetchRequest,
    ) -> Result<WorkspaceConfigFetchResult, String> {
        let client = self
            .workspace_request_clients
            .get(&request.workspace_api.workspace_id)
            .ok_or_else(|| {
                format!(
                    "Workspace request client is unavailable for workspace {}",
                    request.workspace_api.workspace_id
                )
            })?;
        fetch_workspace_config_http(&request, client).await
    }

    fn retained_session_snapshot(
        &self,
        worker_ref: &WorkerRef,
    ) -> Result<session_store::RetainedSessionSnapshot, session_store::RetainedSnapshotReadError>
    {
        let aggregate_dir = self
            .worker_aggregate_dir(worker_ref)
            .map_err(|_| session_store::RetainedSnapshotReadError::RetentionMissing)?;
        let worker_name = Self::runtime_worker_name_for_ref(worker_ref);
        session_store::read_retained_session_snapshot(
            &aggregate_dir,
            &worker_name,
            session_store::DEFAULT_RETAINED_SNAPSHOT_MAX_BYTES,
        )
    }

    fn retained_session_history_page(
        &self,
        worker_ref: &WorkerRef,
        cursor: Option<&str>,
        limit: Option<usize>,
    ) -> Result<protocol::SessionHistoryPage, session_store::RetainedHistoryReadError> {
        let aggregate_dir = self
            .worker_aggregate_dir(worker_ref)
            .map_err(|_| session_store::RetainedHistoryReadError::RetentionMissing)?;
        let worker_name = Self::runtime_worker_name_for_ref(worker_ref);
        session_store::read_retained_session_history_page(
            &aggregate_dir,
            &worker_name,
            cursor,
            limit,
            session_store::RetainedHistoryReadLimits::default(),
        )
    }

    fn retained_session_attachment(
        &self,
        worker_ref: &WorkerRef,
        session_id: &str,
        attachment_id: &str,
    ) -> Result<session_store::RetainedSessionAttachment, session_store::RetainedAttachmentReadError>
    {
        let aggregate_dir = self
            .worker_aggregate_dir(worker_ref)
            .map_err(|_| session_store::RetainedAttachmentReadError::RetentionMissing)?;
        let worker_name = Self::runtime_worker_name_for_ref(worker_ref);
        session_store::read_retained_session_attachment(
            &aggregate_dir,
            &worker_name,
            session_id,
            attachment_id,
            session_store::DEFAULT_RETAINED_HISTORY_MAX_SCAN_BYTES,
            10 * 1024 * 1024,
        )
    }

    fn observe_workspace_prompt_projection(
        &self,
        projection: worker::WorkspacePromptProjection,
    ) -> Result<(), String> {
        self.prompt_projection_cache.observe(projection).map(|_| ())
    }

    async fn spawn_controller(
        &self,
        request: WorkerExecutionSpawnRequest,
    ) -> Result<RuntimeWorkerController, String> {
        let worker_name = Self::runtime_worker_name(&request);
        let profile = Self::runtime_profile(&request);
        let has_local_filesystem = request.workdir_attachments.len() == 1;
        let only_binding = (request.workdir_attachments.len() == 1)
            .then(|| request.workdir_attachments.values().next())
            .flatten();
        let worker_root = only_binding
            .map(|binding| binding.root().to_path_buf())
            .unwrap_or_else(|| self.profile_base_dir.clone());
        let filesystem_authority = only_binding
            .map(|binding| {
                WorkerFilesystemAuthority::local(
                    binding.root().to_path_buf(),
                    binding.cwd().to_path_buf(),
                )
            })
            .unwrap_or(WorkerFilesystemAuthority::None);
        let workspace_backend_ref = RuntimeWorkspaceBackendRef::from_worker_request(
            &request.request,
            self.runtime_id.as_deref(),
        );
        let observation_runtime_id = self.runtime_id.clone();
        let observation_workspace_id = request
            .request
            .workspace_api
            .as_ref()
            .map(|api| api.workspace_id.clone());
        let observation_grants = request.request.worker_observation_grants.clone();
        let observation_enabled = request.request.worker_observation_enabled;
        let workspace_request_client = request
            .request
            .workspace_api
            .as_ref()
            .and_then(|api| self.workspace_request_clients.get(&api.workspace_id));
        let workspace_context = workspace_backend_ref.worker_context(
            &request.worker_ref,
            request.workspace_scope.as_ref(),
            self.worker_mutation_identity.as_ref(),
            workspace_request_client,
            self.embedded_worker_mutation_dispatcher.as_ref(),
            Some(self.prompt_projection_cache.clone()),
        );
        let selector = profile.as_ref();
        let archive = self
            .resolve_profile_source_archive(
                &request.request.profile_source,
                request.config_bundle.as_ref(),
            )
            .await?;
        let (mut manifest, mut loader) = {
            let manifest = archive
                .resolve_profile(selector, &worker_root, &worker_name)
                .map_err(|err| format!("failed to resolve profile source archive: {err}"))?;
            if has_local_filesystem {
                worker::entrypoint::resolve_runtime_profile_manifest_from_manifest(
                    manifest,
                    &worker_root,
                    &worker_name,
                )?
            } else {
                worker::entrypoint::resolve_runtime_profile_manifest_from_manifest_without_filesystem(
                    manifest,
                    &worker_root,
                    &worker_name,
                )?
            }
        };
        bind_workspace_memory_settings(&mut manifest, &request.request)?;
        if let Some(bundle) = request.config_bundle.as_ref()
            && let Some(resolution) =
                self.observe_bundle_prompt_projection(bundle, observation_workspace_id.as_deref())?
        {
            loader = loader.with_effective_catalog(resolution.projection.catalog.clone());
        }
        let flow_transition_enabled = manifest.feature.flow.enabled;

        let worker_aggregate_dir = self.worker_aggregate_dir(&request.worker_ref)?;
        let session_dir = worker_aggregate_dir.join("session");
        let session_store = WorkerSessionStore::new(&session_dir).map_err(|err| {
            format!(
                "failed to initialize canonical Worker Session store at {}: {err}",
                session_dir.display()
            )
        })?;
        let worker_metadata_store =
            WorkerAggregateStore::new(&worker_aggregate_dir, worker_name.clone()).map_err(
                |err| {
                    format!(
                        "failed to initialize canonical Worker metadata store at {}: {err}",
                        worker_aggregate_dir.display()
                    )
                },
            )?;
        let store = CombinedStore::new(session_store, worker_metadata_store);

        let run_dir = self.worker_run_dir(&request.worker_ref)?;
        let bash_output_dir = bash_output_dir_for_worker_id(&request.worker_ref.worker_id);
        let mut prepared = WorkerBootstrap::new(
            manifest,
            store,
            loader,
            workspace_context,
            filesystem_authority,
            WorkerBootstrapLayout::RuntimeManagedRun {
                run_dir: run_dir.clone(),
                bash_output_dir,
            },
            self.controller_transport,
        )
        .prepare()
        .await
        .map_err(|error| match error {
            WorkerBootstrapError::Worker(source) => {
                format!("failed to create Worker from profile: {source}")
            }
            WorkerBootstrapError::Controller { source, .. } => {
                format!("failed to prepare Worker controller: {source}")
            }
        })?;
        let worker = prepared.worker_mut();
        if let Some(binding) = request.request.backend_job.as_ref() {
            worker
                .bind_backend_job(binding.clone())
                .map_err(str::to_string)?;
        }
        validate_worker_memory_settings(worker.manifest(), &request.request)?;
        worker
            .finalize_subjektiv_session_attribution(
                request.request.subjektiv_attached,
                SubjektivSessionAttributionLifecycle::NewSession,
            )
            .map_err(|error| format!("finalize Worker Session attribution: {error}"))?;
        let workdir_capabilities = request
            .workdir_attachments
            .keys()
            .map(|alias| {
                (
                    alias.clone(),
                    initial_workdir_session_capabilities(
                        alias,
                        &request.request.workdir_attachments,
                    ),
                )
            })
            .collect::<BTreeMap<_, _>>();
        worker.bind_workdir_sessions(runtime_local_workdir_router(
            &request.workdir_attachments,
            &workdir_capabilities,
            worker.scope().clone(),
        )?);
        if let (Some(runtime_id), Some(workspace_id)) =
            (observation_runtime_id, observation_workspace_id.clone())
            && observation_enabled
        {
            let mut providers: Vec<Arc<dyn WorkerObservationProvider>> = vec![Arc::new(
                WorkspaceClientWorkerObservationProvider::new(worker.workspace_client_handle()),
            )];
            if !observation_grants.is_empty() {
                providers.push(Arc::new(RuntimeGrantedWorkerObservationProvider {
                    runtime_id,
                    workspace_id,
                    grants: observation_grants.into_iter().take(100).collect(),
                    hub: self.observation_hub.clone(),
                }));
            }
            worker.bind_worker_observation_provider(Some(Arc::new(
                CompositeWorkerObservationProvider::new(providers),
            )));
        }
        if flow_transition_enabled {
            let report = worker
                .install_runtime_flow_transition_feature()
                .map_err(|error| format!("install Flow transition feature: {error}"))?;
            if report.reports.iter().any(|report| !report.installed) {
                return Err(format!(
                    "install Flow transition feature failed: {:?}",
                    report.reports
                ));
            }
        }

        let workspace_client = worker.workspace_client_handle();
        let started = prepared.start().await.map_err(|error| match error {
            WorkerBootstrapError::Worker(source) => {
                format!("failed to prepare Worker before controller start: {source}")
            }
            WorkerBootstrapError::Controller { source, .. } => format!(
                "failed to spawn Worker controller in {}: {source}",
                run_dir.display()
            ),
        })?;
        let (handle, shutdown_rx, controller_task) =
            (started.handle, started.shutdown, started.controller_task);
        if flow_transition_enabled {
            handle.shared_state.enable_flow_transition();
        }
        self.observation_hub.register(
            request.worker_ref.clone(),
            observation_workspace_id,
            &handle,
        );
        Ok(RuntimeWorkerController {
            handle,
            shutdown: Arc::new(tokio::sync::Mutex::new(Some(shutdown_rx))),
            controller_task,
            workspace_client,
        })
    }

    async fn preflight_restore(
        &self,
        request: &WorkerExecutionRestoreRequest,
    ) -> Result<(), String> {
        let worker_name = Self::runtime_worker_name_for_ref(&request.worker_ref);
        let worker_aggregate_dir = self.worker_aggregate_dir(&request.worker_ref)?;
        if !worker_aggregate_dir.is_dir() {
            return Err("Persisted Worker aggregate metadata is unavailable".to_string());
        }
        if !worker_aggregate_dir.join("session").is_dir() {
            return Err("Persisted Worker Session metadata is unavailable".to_string());
        }
        let metadata_store = WorkerAggregateStore::new(&worker_aggregate_dir, &worker_name)
            .map_err(|error| format!("failed to read Worker aggregate metadata: {error}"))?;
        let metadata = metadata_store
            .read_by_name(&worker_name)
            .map_err(|error| format!("failed to read Worker metadata: {error}"))?
            .ok_or_else(|| "Persisted Worker metadata is unavailable".to_string())?;
        let (manifest, loader) = Self::manifest_for_restore(&metadata, &request.request)?;
        if let Some(active) = metadata.active.as_ref()
            && let Some(segment_id) = active.segment_id
        {
            let session_store = WorkerSessionStore::new(worker_aggregate_dir.join("session"))
                .map_err(|error| format!("failed to read Worker Session store: {error}"))?;
            if !session_store
                .exists(active.session_id, segment_id)
                .map_err(|error| format!("failed to inspect Worker Session segment: {error}"))?
            {
                return Err("Persisted Worker Session segment is unavailable".to_string());
            }
            session_store
                .read_all(active.session_id, segment_id)
                .map_err(|error| format!("failed to read Worker Session segment: {error}"))?;
        }
        if let Some(api) = request.request.workspace_api.as_ref()
            && metadata
                .active
                .as_ref()
                .is_some_and(|active| active.segment_id.is_none())
        {
            let bundle = request.config_bundle.as_ref().ok_or_else(|| {
                "Pending Workspace Worker restore requires operation-owned launch material"
                    .to_string()
            })?;
            crate::config_bundle::validate_config_bundle(bundle)
                .map_err(|error| format!("invalid pending Worker launch material: {error}"))?;
            if bundle.metadata.workspace_id != api.workspace_id {
                return Err(format!(
                    "Workspace Prompt projection scope mismatch: expected {}, got {}",
                    api.workspace_id, bundle.metadata.workspace_id
                ));
            }
            let prompt_catalog = bundle.prompt_catalog.as_ref().ok_or_else(|| {
                "pending Workspace Worker restore requires a saved Workspace Prompt projection"
                    .to_string()
            })?;
            // Validate the exact launch catalog without publishing it into the
            // current projection cache or allocating live Worker resources.
            worker::SystemPromptTemplate::parse(
                &manifest.engine.instruction,
                loader.with_effective_catalog(prompt_catalog.clone()),
            )
            .map_err(|error| format!("invalid pending Worker launch Prompt: {error}"))?;
        }
        Ok(())
    }

    async fn restore_controller(
        &self,
        request: WorkerExecutionRestoreRequest,
    ) -> Result<RuntimeWorkerController, String> {
        self.preflight_restore(&request).await?;
        let worker_name = Self::runtime_worker_name_for_ref(&request.worker_ref);
        let only_binding = (request.workdir_attachments.len() == 1)
            .then(|| request.workdir_attachments.values().next())
            .flatten();
        let filesystem_authority = only_binding
            .map(|binding| {
                WorkerFilesystemAuthority::local(
                    binding.root().to_path_buf(),
                    binding.cwd().to_path_buf(),
                )
            })
            .unwrap_or(WorkerFilesystemAuthority::None);
        let workspace_backend_ref = RuntimeWorkspaceBackendRef::from_worker_request(
            &request.request,
            self.runtime_id.as_deref(),
        );
        let observation_runtime_id = self.runtime_id.clone();
        let observation_workspace_id = request
            .request
            .workspace_api
            .as_ref()
            .map(|api| api.workspace_id.clone());
        let observation_grants = request.request.worker_observation_grants.clone();
        let observation_enabled = request.request.worker_observation_enabled;
        let workspace_request_client = request
            .request
            .workspace_api
            .as_ref()
            .and_then(|api| self.workspace_request_clients.get(&api.workspace_id));
        let workspace_context = workspace_backend_ref.worker_context(
            &request.worker_ref,
            request.workspace_scope.as_ref(),
            self.worker_mutation_identity.as_ref(),
            workspace_request_client,
            self.embedded_worker_mutation_dispatcher.as_ref(),
            Some(self.prompt_projection_cache.clone()),
        );

        let worker_aggregate_dir = self.worker_aggregate_dir(&request.worker_ref)?;
        let session_dir = worker_aggregate_dir.join("session");
        let session_store = WorkerSessionStore::new(&session_dir).map_err(|err| {
            format!(
                "failed to initialize canonical Worker Session store at {}: {err}",
                session_dir.display()
            )
        })?;
        let worker_metadata_store =
            WorkerAggregateStore::new(&worker_aggregate_dir, worker_name.clone()).map_err(
                |err| {
                    format!(
                        "failed to initialize canonical Worker metadata store at {}: {err}",
                        worker_aggregate_dir.display()
                    )
                },
            )?;
        let metadata = worker_metadata_store
            .read_by_name(&worker_name)
            .map_err(|error| format!("failed to read Worker metadata: {error}"))?
            .ok_or_else(|| "Persisted Worker metadata is unavailable".to_string())?;
        let (manifest, loader) = Self::manifest_for_restore(&metadata, &request.request)?;
        let store = CombinedStore::new(session_store, worker_metadata_store);

        let mut worker = match Worker::restore_from_worker_metadata_with_context(
            &worker_name,
            manifest.clone(),
            store.clone(),
            loader.clone(),
            workspace_context.clone(),
            filesystem_authority.clone(),
        )
        .await
        {
            Ok(worker) => worker,
            Err(WorkerError::WorkerMetadataPending { .. })
                if request.request.initial_input.is_none() =>
            {
                let pending_loader = if workspace_context.workspace_id().is_some() {
                    let bundle = request.config_bundle.as_ref().ok_or_else(|| {
                        "pending Workspace Worker restore requires operation-owned launch material; generic restore must not reconstruct it from current Workspace config"
                            .to_string()
                    })?;
                    let resolution = self
                        .observe_bundle_prompt_projection(
                            bundle,
                            observation_workspace_id.as_deref(),
                        )?
                        .ok_or_else(|| {
                            "pending Workspace Worker restore requires a saved Workspace Prompt projection"
                                .to_string()
                        })?;
                    loader
                        .clone()
                        .with_effective_catalog(resolution.projection.catalog.clone())
                } else {
                    loader.clone()
                };
                Worker::restore_pending_from_worker_metadata_with_context(
                    &worker_name,
                    manifest.clone(),
                    store,
                    pending_loader,
                    workspace_context,
                    filesystem_authority,
                )
                .await
                .map_err(|err| format!("failed to recreate pending Worker from metadata: {err}"))?
            }
            Err(err) => return Err(format!("failed to restore Worker from metadata: {err}")),
        };
        if let Some(binding) = request.request.backend_job.as_ref() {
            worker
                .bind_backend_job(binding.clone())
                .map_err(str::to_string)?;
        }
        validate_worker_memory_settings(worker.manifest(), &request.request)?;
        worker
            .finalize_subjektiv_session_attribution(
                request.request.subjektiv_attached,
                SubjektivSessionAttributionLifecycle::RestoredSession,
            )
            .map_err(|error| format!("finalize restored Worker Session attribution: {error}"))?;
        let flow_transition_enabled = worker.manifest().feature.flow.enabled;
        let workdir_sessions = restored_workdir_router(
            &request.workdir_attachments,
            &request.logical_workdir_attachments,
            &request.request.workdir_attachments,
            worker.scope().clone(),
            worker.workspace_client_handle(),
        )?;
        worker.bind_workdir_sessions(workdir_sessions);
        if let (Some(runtime_id), Some(workspace_id)) =
            (observation_runtime_id, observation_workspace_id.clone())
            && observation_enabled
        {
            let mut providers: Vec<Arc<dyn WorkerObservationProvider>> = vec![Arc::new(
                WorkspaceClientWorkerObservationProvider::new(worker.workspace_client_handle()),
            )];
            if !observation_grants.is_empty() {
                providers.push(Arc::new(RuntimeGrantedWorkerObservationProvider {
                    runtime_id,
                    workspace_id,
                    grants: observation_grants.into_iter().take(100).collect(),
                    hub: self.observation_hub.clone(),
                }));
            }
            worker.bind_worker_observation_provider(Some(Arc::new(
                CompositeWorkerObservationProvider::new(providers),
            )));
        }
        if flow_transition_enabled {
            let report = worker
                .install_runtime_flow_transition_feature()
                .map_err(|error| format!("install Flow transition feature: {error}"))?;
            if report.reports.iter().any(|report| !report.installed) {
                return Err(format!(
                    "install Flow transition feature failed: {:?}",
                    report.reports
                ));
            }
        }

        let workspace_client = worker.workspace_client_handle();
        let run_dir = self.worker_restore_run_dir(&request.worker_ref, request.operation_id)?;
        let bash_output_dir = bash_output_dir_for_worker_id(&request.worker_ref.worker_id);
        let cleanup_sessions = worker.workdir_sessions();
        let started = PreparedWorker::new(
            worker,
            WorkerBootstrapLayout::RuntimeManagedRun {
                run_dir: run_dir.clone(),
                bash_output_dir,
            },
            self.controller_transport,
        )
        .start()
        .await
        .map_err(|error| match error {
            WorkerBootstrapError::Worker(source) => {
                format!("failed to prepare restored Worker: {source}")
            }
            WorkerBootstrapError::Controller {
                source,
                cleanup_failed,
            } => {
                if cleanup_failed {
                    self.failed_restore_sessions
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .insert(
                            request.worker_ref.clone(),
                            (request.operation_id, cleanup_sessions),
                        );
                }
                format!(
                    "failed to spawn restored Worker controller in {}: {source}",
                    run_dir.display()
                )
            }
        })?;
        let (handle, shutdown_rx, controller_task) =
            (started.handle, started.shutdown, started.controller_task);
        if flow_transition_enabled {
            handle.shared_state.enable_flow_transition();
        }
        Ok(RuntimeWorkerController {
            handle,
            shutdown: Arc::new(tokio::sync::Mutex::new(Some(shutdown_rx))),
            controller_task,
            workspace_client,
        })
    }

    async fn cleanup_failed_restore(
        &self,
        request: &WorkerExecutionRestoreRequest,
    ) -> Result<(), String> {
        let retained = self
            .failed_restore_sessions
            .lock()
            .map_err(|_| "failed restore sessions lock is poisoned".to_string())?
            .get(&request.worker_ref)
            .cloned();
        if let Some((operation_id, sessions)) = retained {
            if operation_id != request.operation_id {
                return Err("failed restore sessions belong to another operation".to_string());
            }
            sessions
                .close_all()
                .await
                .map_err(|error| format!("failed restore session cleanup: {error}"))?;
        }
        // restore_controller starts the Controller only as its final fallible
        // step. A completed error leaves no live Controller; remove only this
        // operation's disposable run artifacts, never Session/metadata authority.
        self.stopped_restore_run_dirs(&request.worker_ref)?;
        let run_dir = self.worker_restore_run_dir(&request.worker_ref, request.operation_id)?;
        Self::remove_restore_run_artifacts(&run_dir)?;
        self.failed_restore_sessions
            .lock()
            .map_err(|_| "failed restore sessions lock is poisoned".to_string())?
            .remove(&request.worker_ref);
        Ok(())
    }

    async fn reconcile_stopped_worker(&self, worker_ref: &WorkerRef) -> Result<(), String> {
        let retained = self
            .failed_restore_sessions
            .lock()
            .map_err(|_| "failed restore sessions lock is poisoned".to_string())?
            .get(worker_ref)
            .cloned();
        if let Some((_, sessions)) = retained.as_ref() {
            sessions
                .close_all()
                .await
                .map_err(|error| format!("failed restore session cleanup: {error}"))?;
        }
        // A restart loses the map, not the operation-owned filesystem artifacts.
        // Validate every candidate before deleting any, and retain map/journal
        // retry authority whenever validation or cleanup remains unproven.
        for run_dir in self.stopped_restore_run_dirs(worker_ref)? {
            Self::remove_restore_run_artifacts(&run_dir)?;
        }
        if retained.is_some() {
            self.failed_restore_sessions
                .lock()
                .map_err(|_| "failed restore sessions lock is poisoned".to_string())?
                .remove(worker_ref);
        }
        Ok(())
    }

    fn activate_restored_controller(
        &self,
        worker_ref: &WorkerRef,
        workspace_id: Option<&str>,
        handle: &WorkerHandle,
    ) {
        self.observation_hub
            .register(worker_ref.clone(), workspace_id.map(str::to_string), handle);
    }
}

#[derive(Clone)]
struct RuntimeExecutionTaskScope {
    tasks: Arc<Mutex<Vec<Arc<RuntimeExecutionTask>>>>,
    shutdown_admission: Arc<tokio::sync::Mutex<Option<RuntimeShutdownAdmission>>>,
    // Actual unconnected Controllers whose cleanup failed, owned by this
    // execution's structured scope. No WorkerRef index or presence mirror.
    cleanup_candidates: Arc<Mutex<Vec<Arc<RuntimeWorkerExecution>>>>,
}

// Evidence for one exact command on this actual Controller. Keep the send task
// and receiver across bounded waits: unknown admission never authorizes resend.
struct RuntimeShutdownAdmission {
    command_id: u64,
    send: Option<tokio::task::JoinHandle<Result<(), String>>>,
    events: tokio::sync::broadcast::Receiver<Event>,
    acknowledgement: Option<protocol::WorkerCommandAcknowledgement>,
    uncertainty: Option<String>,
}

impl RuntimeShutdownAdmission {
    async fn wait(&mut self, timeout: Duration) -> Result<(), String> {
        let result = tokio::time::timeout(timeout, async {
            if let Some(send) = self.send.as_mut() {
                match send.await {
                    Ok(Ok(())) => { self.send.take(); }
                    Ok(Err(message)) => {
                        self.send.take();
                        self.uncertainty = Some(message.clone());
                        return Err(message);
                    }
                    Err(error) => {
                        self.send.take();
                        let message = format!("Shutdown send task failed: {error}; admission is unknown");
                        self.uncertainty = Some(message.clone());
                        return Err(message);
                    }
                }
            }
            if self.acknowledgement.is_some() {
                return Ok(());
            }
            loop {
                match self.events.recv().await {
                    Ok(Event::CommandAcknowledged { acknowledgement })
                        if acknowledgement.command_id == self.command_id
                            && acknowledgement.command == protocol::WorkerCommandKind::Shutdown => {
                        self.acknowledgement = Some(acknowledgement);
                        self.uncertainty = None;
                        return Ok(());
                    }
                    Ok(_) => {}
                    Err(error) => {
                        let message = format!("Shutdown command {} acknowledgement unavailable: {error}; admission is unknown", self.command_id);
                        self.uncertainty = Some(message.clone());
                        return Err(message);
                    }
                }
            }
        }).await;
        match result {
            Ok(result) => result,
            Err(_) => {
                let message = format!(
                    "Shutdown command {} acknowledgement timed out; admission is unknown and waiter is retained",
                    self.command_id
                );
                self.uncertainty = Some(message.clone());
                Err(message)
            }
        }
    }
}

struct RuntimeExecutionTask {
    name: &'static str,
    completion: tokio::sync::Mutex<RuntimeExecutionTaskCompletion>,
    abort_before_join: bool,
}

// The scope retains the actual handle while awaiting it and retains its abnormal
// completion after it has been consumed. Neither cancellation of a cleanup
// future nor another Stop attempt can turn an unknown cleanup into an empty scope.
struct RuntimeExecutionTaskCompletion {
    task: Option<tokio::task::JoinHandle<()>>,
    failure: Option<String>,
}

impl RuntimeExecutionTaskScope {
    fn new(controller_task: tokio::task::JoinHandle<()>) -> Self {
        let scope = Self {
            tasks: Arc::new(Mutex::new(Vec::new())),
            shutdown_admission: Arc::new(tokio::sync::Mutex::new(None)),
            cleanup_candidates: Arc::new(Mutex::new(Vec::new())),
        };
        scope.push("controller", controller_task, false);
        scope
    }

    fn has_cleanup_candidates(&self) -> Result<bool, String> {
        self.cleanup_candidates
            .lock()
            .map(|candidates| !candidates.is_empty())
            .map_err(|_| "execution cleanup candidate lock is poisoned".to_string())
    }

    async fn request_shutdown(
        &self,
        handle: &WorkerHandle,
        shutdown_requested: &AtomicBool,
        timeout: Duration,
    ) -> Result<(), String> {
        let mut retained = self.shutdown_admission.lock().await;
        if handle.protocol_is_closed() {
            // Endpoint closure alone is not cleanup proof. Drain our send task
            // too, then let the existing completion + all-task join barrier prove it.
            if let Some(pending) = retained.as_mut() {
                if let Some(send) = pending.send.as_mut() {
                    let _ = tokio::time::timeout(timeout, send).await.map_err(|_| {
                        "Shutdown send task still pending on closed endpoint".to_string()
                    })?;
                    pending.send.take();
                }
            }
            shutdown_requested.store(true, Ordering::Release);
            return Ok(());
        }
        if retained.is_none() {
            if shutdown_requested.load(Ordering::Acquire) {
                return Err("Shutdown admission evidence missing; cleanup is unproven".to_string());
            }
            // The bridge cache can lag admission of already queued commands.
            // Rejection retries use the real Controller snapshot, never that cache.
            let command = next_internal_command_for_snapshot(&handle.shared_state.snapshot());
            let events = handle.subscribe();
            let worker = handle.clone();
            *retained = Some(RuntimeShutdownAdmission {
                command_id: command.command_id,
                send: Some(tokio::spawn(async move {
                    worker
                        .send(Method::Shutdown { command })
                        .await
                        .map_err(|error| format!("failed to send Shutdown: {error}"))
                })),
                events,
                acknowledgement: None,
                uncertainty: None,
            });
            shutdown_requested.store(true, Ordering::Release);
        }
        let pending = retained.as_mut().unwrap();
        pending.wait(timeout).await?;
        let acknowledgement = pending.acknowledgement.as_ref().unwrap();
        match acknowledgement.disposition {
            protocol::WorkerCommandDisposition::Accepted => Ok(()),
            disposition => {
                let message = format!(
                    "Shutdown command {} rejected: {disposition:?}; stop remains retryable",
                    acknowledgement.command_id
                );
                retained.take();
                shutdown_requested.store(false, Ordering::Release);
                Err(message)
            }
        }
    }

    fn push(&self, name: &'static str, task: tokio::task::JoinHandle<()>, abort_before_join: bool) {
        let mut tasks = match self.tasks.lock() {
            Ok(tasks) => tasks,
            Err(poisoned) => poisoned.into_inner(),
        };
        tasks.push(Arc::new(RuntimeExecutionTask {
            name,
            completion: tokio::sync::Mutex::new(RuntimeExecutionTaskCompletion {
                task: Some(task),
                failure: None,
            }),
            abort_before_join,
        }));
    }

    fn retain_completion_failure(&self, message: String) -> Result<(), String> {
        self.tasks
            .lock()
            .map_err(|_| "execution task registry lock is poisoned".to_string())?
            .push(Arc::new(RuntimeExecutionTask {
                name: "controller shutdown completion",
                completion: tokio::sync::Mutex::new(RuntimeExecutionTaskCompletion {
                    task: None,
                    failure: Some(message),
                }),
                abort_before_join: false,
            }));
        Ok(())
    }

    async fn confirm_shutdown_and_join(
        &self,
        shutdown: &Arc<tokio::sync::Mutex<Option<worker::ShutdownReceiver>>>,
    ) -> Result<(), String> {
        // Do not consume a completion receiver if its failure cannot be retained.
        drop(
            self.tasks
                .lock()
                .map_err(|_| "execution task registry lock is poisoned".to_string())?,
        );
        let confirmation = {
            let mut guard = shutdown.lock().await;
            if let Some(receiver) = guard.as_mut() {
                match tokio::time::timeout(Duration::from_secs(5), receiver).await {
                    Ok(Ok(())) => {
                        guard.take();
                        Ok(())
                    }
                    Ok(Err(_)) => {
                        let message =
                            "Worker shutdown completion channel closed; cleanup is unproven"
                                .to_string();
                        self.retain_completion_failure(message.clone())?;
                        guard.take();
                        Err(message)
                    }
                    Err(_) => Err(
                        "Worker shutdown confirmation timed out; stop remains retryable"
                            .to_string(),
                    ),
                }
            } else {
                Ok(())
            }
        };
        // Drain bridges/relays even after an abnormal completion, while keeping
        // the failed completion and all timed-out handles in their owning scope.
        self.join().await?;
        confirmation
    }

    async fn join(&self) -> Result<(), String> {
        let tasks = self
            .tasks
            .lock()
            .map_err(|_| "execution task registry lock is poisoned".to_string())?
            .clone();
        let mut first_failure = None;
        for task in tasks {
            let mut completion = task.completion.lock().await;
            if let Some(message) = completion.failure.as_ref() {
                first_failure.get_or_insert_with(|| message.clone());
                continue;
            }
            if let Some(handle) = completion.task.as_mut() {
                if task.abort_before_join {
                    handle.abort();
                }
                match tokio::time::timeout(Duration::from_secs(5), handle).await {
                    Ok(Ok(())) => {
                        completion.task.take();
                    }
                    Ok(Err(error)) if task.abort_before_join && error.is_cancelled() => {
                        completion.task.take();
                    }
                    Ok(Err(error)) => {
                        let message = format!(
                            "{} task failed while stopping Worker: {error}; cleanup is unproven",
                            task.name
                        );
                        completion.task.take();
                        completion.failure = Some(message.clone());
                        first_failure.get_or_insert(message);
                        continue;
                    }
                    Err(_) => {
                        first_failure.get_or_insert_with(|| {
                            format!(
                                "{} task did not stop before timeout; stop remains retryable",
                                task.name
                            )
                        });
                        continue;
                    }
                }
            }
            drop(completion);
            self.tasks
                .lock()
                .map_err(|_| "execution task registry lock is poisoned".to_string())?
                .retain(|retained| !Arc::ptr_eq(retained, &task));
        }
        match first_failure {
            Some(message) => Err(message),
            None if self.has_cleanup_candidates()? => Err(
                "unconnected Controller cleanup remains unproven; scope is retained".to_string(),
            ),
            None => Ok(()),
        }
    }
}

#[derive(Clone)]
struct RuntimeWorkerExecution {
    handle: WorkerHandle,
    shutdown: Arc<tokio::sync::Mutex<Option<worker::ShutdownReceiver>>>,
    shutdown_requested: Arc<AtomicBool>,
    tasks: RuntimeExecutionTaskScope,
    worker_state: Arc<RwLock<protocol::WorkerStateSnapshot>>,
    workspace_client: Option<Arc<dyn WorkspaceClient>>,
    workspace_id: Option<String>,
    restore_operation_id: Option<crate::execution::WorkerLifecycleOperationId>,
    workdir_attachments: Vec<crate::catalog::WorkingDirectoryAttachmentStatus>,
}

// Actual operation-owned resources, not a second Worker catalog or presence
// marker. Timeout never drops the factory future or a late Controller result.
struct PendingRuntimeRestore {
    request: WorkerExecutionRestoreRequest,
    task: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    result: Mutex<Option<Result<RuntimeWorkerController, String>>>,
    failure: Mutex<Option<String>>,
}

impl PendingRuntimeRestore {
    async fn wait(&self, timeout: Duration) -> Result<(), String> {
        let mut task = self.task.lock().await;
        if let Some(message) = self
            .failure
            .lock()
            .map_err(|_| "restore factory failure lock is poisoned".to_string())?
            .as_ref()
        {
            return Err(message.clone());
        }
        if let Some(handle) = task.as_mut() {
            match tokio::time::timeout(timeout, handle).await {
                Ok(Ok(())) => {
                    task.take();
                }
                Ok(Err(error)) => {
                    task.take();
                    let message =
                        format!("restore factory task failed: {error}; cleanup is unproven");
                    *self
                        .failure
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(message.clone());
                    return Err(message);
                }
                Err(_) => {
                    return Err("restore factory is still pending; cleanup is unproven".to_string());
                }
            }
        }
        Ok(())
    }
}

/// Worker execution backend backed by real worker crate Workers.
pub struct WorkerRuntimeExecutionBackend<F = ProfileRuntimeWorkerFactory> {
    backend_id: String,
    factory: Arc<F>,
    working_directory_materializer: Option<Arc<dyn WorkingDirectoryMaterializer>>,
    runtime: Mutex<Option<Runtime>>,
    workers: Mutex<HashMap<crate::identity::WorkerRef, RuntimeWorkerExecution>>,
    pending_restores: Mutex<HashMap<WorkerRef, Arc<PendingRuntimeRestore>>>,
    worker_locks: Mutex<HashMap<WorkerRef, Arc<Mutex<()>>>>,
    spawn_restore_timeout: Duration,
}

impl<F> WorkerRuntimeExecutionBackend<F>
where
    F: RuntimeWorkerFactory,
{
    pub fn new(factory: F) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .thread_name("yoi-runtime-worker-adapter")
            .enable_all()
            .build()
            .map_err(|err| format!("failed to build worker adapter runtime: {err}"))?;
        Ok(Self {
            backend_id: DEFAULT_BACKEND_ID.to_string(),
            factory: Arc::new(factory),
            working_directory_materializer: None,
            runtime: Mutex::new(Some(runtime)),
            workers: Mutex::new(HashMap::new()),
            pending_restores: Mutex::new(HashMap::new()),
            worker_locks: Mutex::new(HashMap::new()),
            spawn_restore_timeout: SPAWN_RESTORE_TASK_TIMEOUT,
        })
    }

    pub fn with_backend_id(mut self, backend_id: impl Into<String>) -> Self {
        self.backend_id = backend_id.into();
        self
    }

    pub fn with_working_directory_materializer(
        mut self,
        materializer: impl WorkingDirectoryMaterializer,
    ) -> Self {
        self.working_directory_materializer = Some(Arc::new(materializer));
        self
    }

    #[cfg(test)]
    fn with_spawn_restore_timeout(mut self, timeout: Duration) -> Self {
        self.spawn_restore_timeout = timeout;
        self
    }

    fn worker_lock(&self, worker_ref: &WorkerRef) -> Result<Arc<Mutex<()>>, String> {
        let mut locks = self
            .worker_locks
            .lock()
            .map_err(|_| "worker operation lock registry is poisoned".to_string())?;
        Ok(locks
            .entry(worker_ref.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone())
    }

    fn wait_for_runtime_task<T>(receiver: mpsc::Receiver<Result<T, String>>) -> Result<T, String> {
        receiver
            .recv_timeout(RUNTIME_TASK_TIMEOUT)
            .map_err(|err| format!("worker adapter task did not complete: {err}"))?
    }

    fn spawn_on_adapter_runtime<Fut>(
        &self,
        task: Fut,
    ) -> Result<tokio::task::JoinHandle<()>, String>
    where
        Fut: Future<Output = ()> + Send + 'static,
    {
        let runtime = self
            .runtime
            .lock()
            .map_err(|_| "worker adapter runtime lock is poisoned".to_string())?;
        let runtime = runtime
            .as_ref()
            .ok_or_else(|| "worker adapter runtime is shutting down".to_string())?;
        Ok(runtime.spawn(task))
    }

    fn run_on_adapter_runtime<T, Fut>(&self, task: Fut) -> Result<T, String>
    where
        T: Send + 'static,
        Fut: Future<Output = Result<T, String>> + Send + 'static,
    {
        let (tx, rx) = mpsc::sync_channel(1);
        self.spawn_on_adapter_runtime(async move {
            let handle = tokio::spawn(task);
            let result = match handle.await {
                Ok(result) => result,
                Err(err) => Err(format!("worker adapter task failed: {err}")),
            };
            let _ = tx.send(result);
        })?;
        Self::wait_for_runtime_task(rx)
    }

    // Cleanup contains its own bounded waits. Do not return while an outer
    // waiter still owns removed task handles: a retry must see complete scope
    // ownership, rather than mistake an in-flight join for an empty scope.
    fn run_joined_on_adapter_runtime<T, Fut>(&self, task: Fut) -> Result<T, String>
    where
        T: Send + 'static,
        Fut: Future<Output = Result<T, String>> + Send + 'static,
    {
        let (tx, rx) = mpsc::sync_channel(1);
        self.spawn_on_adapter_runtime(async move {
            let result = match tokio::spawn(task).await {
                Ok(result) => result,
                Err(error) => Err(format!("worker adapter cleanup task failed: {error}")),
            };
            let _ = tx.send(result);
        })?;
        rx.recv()
            .map_err(|error| format!("worker adapter cleanup did not complete: {error}"))?
    }

    fn run_cancellable_on_adapter_runtime<T, Fut>(
        &self,
        timeout: Duration,
        task: Fut,
    ) -> Result<T, String>
    where
        T: Send + 'static,
        Fut: Future<Output = Result<T, String>> + Send + 'static,
    {
        let (tx, rx) = mpsc::sync_channel(1);
        self.spawn_on_adapter_runtime(async move {
            let mut handle = tokio::spawn(task);
            let result = tokio::select! {
                biased;
                result = &mut handle => match result {
                    Ok(result) => result,
                    Err(err) => Err(format!("worker adapter task failed: {err}")),
                },
                _ = tokio::time::sleep(timeout) => {
                    handle.abort();
                    match handle.await {
                        Ok(result) => result,
                        Err(err) if err.is_cancelled() => Err(format!(
                            "worker adapter task did not complete within {} seconds and was cancelled",
                            timeout.as_secs_f64()
                        )),
                        Err(err) => Err(format!("worker adapter task failed: {err}")),
                    }
                }
            };
            let _ = tx.send(result);
        })?;
        rx.recv()
            .map_err(|err| format!("worker adapter task did not complete: {err}"))?
    }

    fn pending_restore(
        &self,
        worker_ref: &WorkerRef,
    ) -> Result<Option<Arc<PendingRuntimeRestore>>, String> {
        self.pending_restores
            .lock()
            .map(|pending| pending.get(worker_ref).cloned())
            .map_err(|_| "pending restore resources lock is poisoned".to_string())
    }

    fn wait_pending_restore(
        &self,
        pending: &Arc<PendingRuntimeRestore>,
        timeout: Duration,
    ) -> Result<(), String> {
        let pending = Arc::clone(pending);
        self.run_cancellable_on_adapter_runtime(timeout + Duration::from_secs(1), async move {
            pending.wait(timeout).await
        })
    }

    fn remove_pending_restore(&self, worker_ref: &WorkerRef) -> Result<(), String> {
        self.pending_restores
            .lock()
            .map_err(|_| "pending restore resources lock is poisoned".to_string())?
            .remove(worker_ref);
        Ok(())
    }

    fn get_execution(
        &self,
        worker_ref: &WorkerRef,
    ) -> Result<
        (
            WorkerHandle,
            Arc<RwLock<protocol::WorkerStateSnapshot>>,
            Option<Arc<dyn WorkspaceClient>>,
        ),
        WorkerExecutionResult,
    > {
        let workers = self.workers.lock().map_err(|_| {
            WorkerExecutionResult::errored(
                WorkerExecutionOperation::Input,
                "worker adapter registry lock is poisoned",
            )
        })?;
        workers
            .get(worker_ref)
            .map(|execution| {
                (
                    execution.handle.clone(),
                    execution.worker_state.clone(),
                    execution.workspace_client.clone(),
                )
            })
            .ok_or_else(|| {
                WorkerExecutionResult::rejected(
                    WorkerExecutionOperation::Input,
                    "WorkerRef does not reference a live Worker execution",
                )
            })
    }

    #[cfg(feature = "ws-server")]
    fn validate_protocol_execution_under_lock(
        &self,
        worker_ref: &WorkerRef,
        execution: &RuntimeWorkerExecution,
    ) -> Result<(), WorkerExecutionResult> {
        if execution.shutdown_requested.load(Ordering::Acquire)
            || execution.handle.protocol_is_closed()
        {
            return Err(WorkerExecutionResult::rejected(
                WorkerExecutionOperation::ProtocolMethod,
                "attached Worker protocol endpoint is closed",
            ));
        }
        if self
            .pending_restore(worker_ref)
            .map_err(|message| {
                WorkerExecutionResult::errored(WorkerExecutionOperation::ProtocolMethod, message)
            })?
            .is_some()
        {
            return Err(WorkerExecutionResult::busy(
                WorkerExecutionOperation::ProtocolMethod,
                "Worker restore still owns pending resources",
            ));
        }
        let workers = self.workers.lock().map_err(|_| {
            WorkerExecutionResult::errored(
                WorkerExecutionOperation::ProtocolMethod,
                "worker adapter registry lock is poisoned",
            )
        })?;
        if workers
            .get(worker_ref)
            .is_none_or(|current| !current.handle.same_controller(&execution.handle))
        {
            return Err(WorkerExecutionResult::rejected(
                WorkerExecutionOperation::ProtocolMethod,
                "attached Worker protocol endpoint is no longer current",
            ));
        }
        Ok(())
    }

    #[cfg(feature = "ws-server")]
    fn dispatch_protocol_execution_under_lock(
        &self,
        worker_ref: &WorkerRef,
        execution: &RuntimeWorkerExecution,
        method: Method,
    ) -> Result<Vec<Event>, WorkerExecutionResult> {
        self.validate_protocol_execution_under_lock(worker_ref, execution)?;
        match method {
            Method::ListCompletions {
                request_id,
                kind,
                prefix,
                context,
            } => {
                let handle = execution.handle.clone();
                self.run_cancellable_on_adapter_runtime(USER_INPUT_TASK_TIMEOUT, async move {
                    let entries = handle
                        .completion_entries(kind, &prefix, context.as_ref())
                        .await;
                    Ok(vec![Event::Completions {
                        request_id,
                        kind,
                        prefix,
                        context,
                        entries,
                    }])
                })
                .map_err(|message| {
                    WorkerExecutionResult::errored(
                        WorkerExecutionOperation::ProtocolMethod,
                        message,
                    )
                })
            }
            Method::Shutdown { .. } => {
                // Identity was checked against the captured Controller while
                // this same operation lock is held. Never unlock/rebind here.
                let result = self.stop_worker_under_lock(worker_ref);
                if result.is_accepted() {
                    Ok(Vec::new())
                } else {
                    Err(result)
                }
            }
            method => {
                let result = self.send_method(
                    WorkerExecutionOperation::ProtocolMethod,
                    execution.handle.clone(),
                    method,
                );
                if result.is_accepted() {
                    Ok(Vec::new())
                } else {
                    Err(result)
                }
            }
        }
    }

    fn send_method(
        &self,
        operation: WorkerExecutionOperation,
        worker: WorkerHandle,
        method: Method,
    ) -> WorkerExecutionResult {
        self.run_on_adapter_runtime(async move {
            worker
                .send(method)
                .await
                .map_err(|err| format!("failed to send Worker method: {err}"))
        })
        .map(|_| WorkerExecutionResult::accepted(operation))
        .unwrap_or_else(|message| WorkerExecutionResult::errored(operation, message))
    }

    fn send_submit_and_wait_for_acceptance(
        &self,
        operation: WorkerExecutionOperation,
        worker: WorkerHandle,
        method: Method,
        submission_request_id: String,
    ) -> WorkerExecutionResult {
        let request_id = submission_request_id.clone();
        self.run_cancellable_on_adapter_runtime(USER_INPUT_TASK_TIMEOUT, async move {
            // Subscribe before enqueueing so a fast durable acceptance cannot
            // race the Runtime acknowledgement.
            let mut events = worker.subscribe();
            worker
                .send(method)
                .await
                .map_err(|err| format!("failed to send Worker method: {err}"))?;

            tokio::time::timeout(USER_INPUT_COMMIT_TIMEOUT, async move {
                loop {
                    match events.recv().await {
                        Ok(Event::SubmissionAccepted {
                            submission_request_id,
                            submission_id,
                            disposition,
                        }) if submission_request_id == request_id => {
                            return Ok((submission_id, disposition));
                        }
                        Ok(Event::SubmissionRejected {
                            submission_request_id,
                            message,
                        }) if submission_request_id == request_id => {
                            return Err(format!("worker rejected Submit: {message}"));
                        }
                        Ok(Event::Error { message, .. }) => {
                            return Err(format!(
                                "worker rejected Submit before durable acceptance: {message}"
                            ));
                        }
                        Ok(Event::Shutdown) => {
                            return Err(
                                "worker shut down before Submit was durably accepted".to_string()
                            );
                        }
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                            return Err(format!(
                                "worker Submit acknowledgement lagged by {skipped} protocol event(s)"
                            ));
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            return Err(
                                "worker event stream closed before Submit was durably accepted"
                                    .to_string(),
                            );
                        }
                    }
                }
            })
            .await
            .map_err(|_| "timed out waiting for durable Worker Submit acceptance".to_string())?
        })
        .map(|(submission_id, disposition)| {
            WorkerExecutionResult::accepted_submission(
                operation,
                submission_request_id,
                submission_id,
                disposition,
            )
        })
        .unwrap_or_else(|message| WorkerExecutionResult::errored(operation, message))
    }

    fn send_notification_and_wait_for_acceptance(
        &self,
        operation: WorkerExecutionOperation,
        worker: WorkerHandle,
        method: Method,
        notification_request_id: String,
    ) -> WorkerExecutionResult {
        let request_id = notification_request_id.clone();
        let result = self.run_cancellable_on_adapter_runtime(USER_INPUT_TASK_TIMEOUT, async move {
            // Subscribe before enqueueing so a fast durable acceptance cannot
            // race the Runtime acknowledgement.
            let mut events = worker.subscribe();
            worker
                .send(method)
                .await
                .map_err(|err| format!("failed to send Worker method: {err}"))?;

            tokio::time::timeout(USER_INPUT_COMMIT_TIMEOUT, async move {
                loop {
                    match events.recv().await {
                        Ok(Event::NotificationAccepted {
                            notification_request_id,
                        }) if notification_request_id == request_id => {
                            return Ok(NotificationAcceptance::Accepted);
                        }
                        Ok(Event::NotificationRejected {
                            notification_request_id,
                            message,
                        }) if notification_request_id == request_id => {
                            return Ok(NotificationAcceptance::Rejected(message));
                        }
                        Ok(Event::Shutdown) => {
                            return Err(
                                "worker shut down before notification was durably accepted"
                                    .to_string(),
                            );
                        }
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                            return Err(format!(
                                "worker notification acknowledgement lagged by {skipped} protocol event(s)"
                            ));
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            return Err(
                                "worker event stream closed before notification was durably accepted"
                                    .to_string(),
                            );
                        }
                    }
                }
            })
            .await
            .map_err(|_| {
                "timed out waiting for durable Worker notification acceptance".to_string()
            })?
        });

        match result {
            Ok(NotificationAcceptance::Accepted) => {
                WorkerExecutionResult::accepted_notification(operation, notification_request_id)
            }
            Ok(NotificationAcceptance::Rejected(message)) => {
                WorkerExecutionResult::rejected(operation, message)
            }
            Err(message) => WorkerExecutionResult::errored(operation, message),
        }
    }

    fn retain_uncertain_unconnected_controller(
        &self,
        worker_ref: &crate::identity::WorkerRef,
        handle: WorkerHandle,
        shutdown: Arc<tokio::sync::Mutex<Option<worker::ShutdownReceiver>>>,
        shutdown_requested: Arc<AtomicBool>,
        tasks: RuntimeExecutionTaskScope,
        worker_state: Arc<RwLock<protocol::WorkerStateSnapshot>>,
        workspace_client: Option<Arc<dyn WorkspaceClient>>,
        restore_operation_id: Option<crate::execution::WorkerLifecycleOperationId>,
        workdir_attachments: Vec<crate::catalog::WorkingDirectoryAttachmentStatus>,
    ) {
        // Retention must not return the candidate as an error and drop it. Even
        // a poisoned registry keeps its existing actual resource ownership.
        let mut workers = self
            .workers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let workspace_id = workspace_client
            .as_ref()
            .and_then(|client| client.workspace_id().map(str::to_string));
        let candidate = RuntimeWorkerExecution {
            handle,
            shutdown,
            shutdown_requested,
            tasks,
            worker_state,
            workspace_client,
            workspace_id,
            restore_operation_id,
            workdir_attachments,
        };
        if let Some(existing) = workers.get(worker_ref) {
            // Keep the actual endpoint, admission waiter, shutdown receiver and
            // all task handles without replacing the registered Controller.
            existing
                .tasks
                .cleanup_candidates
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(Arc::new(candidate));
        } else {
            workers.insert(worker_ref.clone(), candidate);
        }
    }

    fn cleanup_unconnected_controller(
        &self,
        handle: &WorkerHandle,
        shutdown: &Arc<tokio::sync::Mutex<Option<worker::ShutdownReceiver>>>,
        tasks: &RuntimeExecutionTaskScope,
        shutdown_requested: &Arc<AtomicBool>,
        _worker_state: &Arc<RwLock<protocol::WorkerStateSnapshot>>,
    ) -> Result<(), String> {
        let handle = handle.clone();
        let shutdown_requested = Arc::clone(shutdown_requested);
        let shutdown = shutdown.clone();
        let tasks_for_join = tasks.clone();
        let cleanup = self.run_joined_on_adapter_runtime(async move {
            tasks_for_join
                .request_shutdown(&handle, &shutdown_requested, Duration::from_secs(5))
                .await?;
            tasks_for_join.confirm_shutdown_and_join(&shutdown).await
        });
        cleanup
    }

    fn connect_handle(
        &self,
        operation: WorkerExecutionOperation,
        worker_ref: crate::identity::WorkerRef,
        bridge_context: crate::execution::WorkerExecutionContext,
        handle: WorkerHandle,
        shutdown: Arc<tokio::sync::Mutex<Option<worker::ShutdownReceiver>>>,
        controller_task: tokio::task::JoinHandle<()>,
        restore_operation_id: Option<crate::execution::WorkerLifecycleOperationId>,
        workdir_attachments: BTreeMap<WorkdirAttachmentAlias, WorkingDirectoryBinding>,
        workspace_client: Option<Arc<dyn WorkspaceClient>>,
    ) -> WorkerExecutionSpawnResult {
        self.connect_controller_scope(
            operation,
            worker_ref,
            bridge_context,
            handle,
            shutdown,
            RuntimeExecutionTaskScope::new(controller_task),
            Arc::new(AtomicBool::new(false)),
            restore_operation_id,
            workdir_attachments,
            workspace_client,
        )
    }

    // Own the actual Controller scope before doing any fallible connection work.
    // The split also permits testing duplicate cleanup with an already-pending
    // real admission waiter, rather than relying on a scheduler race.
    fn connect_controller_scope(
        &self,
        operation: WorkerExecutionOperation,
        worker_ref: WorkerRef,
        bridge_context: crate::execution::WorkerExecutionContext,
        handle: WorkerHandle,
        shutdown: Arc<tokio::sync::Mutex<Option<worker::ShutdownReceiver>>>,
        tasks: RuntimeExecutionTaskScope,
        shutdown_requested: Arc<AtomicBool>,
        restore_operation_id: Option<crate::execution::WorkerLifecycleOperationId>,
        workdir_attachments: BTreeMap<WorkdirAttachmentAlias, WorkingDirectoryBinding>,
        workspace_client: Option<Arc<dyn WorkspaceClient>>,
    ) -> WorkerExecutionSpawnResult {
        #[cfg(feature = "ws-server")]
        let streams = subscribe_worker_protocol_session(&handle);
        #[cfg(feature = "ws-server")]
        let worker_state_snapshot = match &streams.snapshot_event {
            Event::Snapshot { state, .. } => state.clone(),
            _ => unreachable!("Worker protocol subscription snapshot must be a snapshot event"),
        };
        #[cfg(not(feature = "ws-server"))]
        let worker_state_snapshot = handle.shared_state.snapshot();
        let worker_state = Arc::new(RwLock::new(worker_state_snapshot));
        let workdir_attachment_statuses = workdir_attachments
            .iter()
            .map(
                |(alias, binding)| crate::catalog::WorkingDirectoryAttachmentStatus {
                    alias: alias.clone(),
                    working_directory: binding.status(),
                },
            )
            .collect::<Vec<_>>();
        let mut workers = self
            .workers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if workers.contains_key(&worker_ref) {
            drop(workers);
            let cleanup = self.cleanup_unconnected_controller(
                &handle,
                &shutdown,
                &tasks,
                &shutdown_requested,
                &worker_state,
            );
            let result = WorkerExecutionResult::busy(
                operation,
                match &cleanup {
                    Ok(()) => "Worker is already connected to execution backend".to_string(),
                    Err(cleanup) => format!(
                        "Worker is already connected to execution backend; controller cleanup failed: {cleanup}"
                    ),
                },
            );
            return match cleanup {
                Ok(()) => WorkerExecutionSpawnResult::RolledBack(result),
                Err(_) => {
                    self.retain_uncertain_unconnected_controller(
                        &worker_ref,
                        handle,
                        shutdown,
                        shutdown_requested,
                        tasks,
                        worker_state,
                        workspace_client,
                        restore_operation_id,
                        workdir_attachment_statuses.clone(),
                    );
                    WorkerExecutionSpawnResult::ReconciliationRequired {
                        result,
                        worker_state: None,
                        workdir_attachments: workdir_attachment_statuses,
                    }
                }
            };
        }
        #[cfg(feature = "ws-server")]
        {
            let mut events = streams.events;
            let mut entry_events = streams.log_entries;
            let bridge_worker_state = worker_state.clone();
            let bridge_task = match self.spawn_on_adapter_runtime(async move {
                loop {
                    tokio::select! {
                        event = events.recv() => {
                            match event {
                                Ok(mut event) => {
                                    match apply_protocol_worker_state(&bridge_worker_state, &mut event) {
                                        Ok(true) => {
                                            let _ = bridge_context.publish_protocol_event(event);
                                        }
                                        Ok(false) => {}
                                        Err(message) => {
                                            let _ = bridge_context.publish_protocol_event(Event::Error {
                                                code: protocol::ErrorCode::Internal,
                                                message: format!("worker state stream rejected: {message}"),
                                            });
                                            break;
                                        }
                                    }
                                }
                                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                                Err(broadcast::error::RecvError::Closed) => break,
                            }
                        }
                        entry = entry_events.recv() => {
                            match entry {
                                Ok(entry) => {
                                    if let Some(event) = live_log_entry_event(entry) {
                                        let _ = bridge_context.publish_protocol_event(event);
                                    }
                                }
                                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                                Err(broadcast::error::RecvError::Closed) => break,
                            }
                        }
                    }
                }
            }) {
                Ok(task) => task,
                Err(message) => {
                    drop(workers);
                    let cleanup = self.cleanup_unconnected_controller(
                        &handle,
                        &shutdown,
                        &tasks,
                        &shutdown_requested,
                        &worker_state,
                    );
                    let result = WorkerExecutionResult::errored(
                        operation,
                        match &cleanup {
                            Ok(()) => message,
                            Err(cleanup) => {
                                format!("{message}; controller cleanup failed: {cleanup}")
                            }
                        },
                    );
                    return match cleanup {
                        Ok(()) => WorkerExecutionSpawnResult::RolledBack(result),
                        Err(_) => {
                            let retained_state = worker_state.read().ok().map(|state| state.clone());
                            self.retain_uncertain_unconnected_controller(
                                &worker_ref,
                                handle,
                                shutdown,
                                shutdown_requested,
                                tasks,
                                worker_state,
                                workspace_client,
                                restore_operation_id,
                                workdir_attachment_statuses.clone(),
                            );
                            WorkerExecutionSpawnResult::ReconciliationRequired {
                                result,
                                worker_state: retained_state,
                                workdir_attachments: workdir_attachment_statuses,
                            }
                        }
                    };
                }
            };
            tasks.push("protocol bridge", bridge_task, true);
        }
        #[cfg(not(feature = "ws-server"))]
        {
            let _ = bridge_context;
        }

        let connected_worker_state = worker_state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let workspace_id = workspace_client
            .as_ref()
            .and_then(|client| client.workspace_id().map(str::to_string));
        workers.insert(
            worker_ref.clone(),
            RuntimeWorkerExecution {
                handle,
                shutdown,
                shutdown_requested,
                tasks,
                worker_state,
                workspace_client,
                workspace_id,
                restore_operation_id,
                workdir_attachments: workdir_attachment_statuses.clone(),
            },
        );

        WorkerExecutionSpawnResult::Connected {
            worker_state: connected_worker_state,
            workdir_attachments: workdir_attachment_statuses,
        }
    }

    // Called under the same per-Worker operation lock as connect/Stop. Each
    // candidate remains in its actual owning scope until its own cleanup proves
    // success. A failed child does not prevent joining the other Controllers.
    fn cleanup_execution_resources(
        &self,
        execution: &RuntimeWorkerExecution,
    ) -> Result<(), String> {
        let candidates = execution
            .tasks
            .cleanup_candidates
            .lock()
            .map_err(|_| "execution cleanup candidate lock is poisoned".to_string())?
            .clone();
        let mut first_failure = None;
        for candidate in candidates {
            match self.cleanup_execution_resources(&candidate) {
                Ok(()) => {
                    execution
                        .tasks
                        .cleanup_candidates
                        .lock()
                        .map_err(|_| "execution cleanup candidate lock is poisoned".to_string())?
                        .retain(|retained| !Arc::ptr_eq(retained, &candidate));
                }
                Err(message) => {
                    first_failure.get_or_insert(message);
                }
            }
        }
        let handle = execution.handle.clone();
        let shutdown_requested = execution.shutdown_requested.clone();
        let shutdown = execution.shutdown.clone();
        let tasks = execution.tasks.clone();
        let cleanup = self.run_joined_on_adapter_runtime(async move {
            tasks
                .request_shutdown(&handle, &shutdown_requested, Duration::from_secs(5))
                .await?;
            tasks.confirm_shutdown_and_join(&shutdown).await?;
            handle.delete_uncommitted_uploaded_files().map_err(|error| {
                format!("uploaded_file_cleanup_failed: {error}; stop remains retryable")
            })
        });
        if let Err(message) = cleanup {
            first_failure.get_or_insert(message);
        }
        match first_failure {
            Some(message) => Err(message),
            None => Ok(()),
        }
    }

    // Caller owns the per-Worker operation lock for this entire cleanup.
    fn stop_worker_under_lock(&self, worker_ref: &WorkerRef) -> WorkerExecutionResult {
        let pending = match self.pending_restore(worker_ref) {
            Ok(pending) => pending,
            Err(message) => {
                return WorkerExecutionResult::errored(WorkerExecutionOperation::Stop, message);
            }
        };
        if let Some(pending) = pending {
            if let Err(message) = self.wait_pending_restore(&pending, Duration::from_secs(5)) {
                return WorkerExecutionResult::errored(WorkerExecutionOperation::Stop, message);
            }
            if self.workers.lock().is_err() {
                return WorkerExecutionResult::errored(
                    WorkerExecutionOperation::Stop,
                    "worker adapter registry lock is poisoned",
                );
            }
            let mut retained = match pending.result.lock() {
                Ok(retained) => retained,
                Err(_) => {
                    return WorkerExecutionResult::errored(
                        WorkerExecutionOperation::Stop,
                        "restore result lock is poisoned",
                    );
                }
            };
            match retained.as_ref() {
                Some(Ok(_)) => {
                    let controller = match retained.take().unwrap() {
                        Ok(controller) => controller,
                        Err(_) => unreachable!(),
                    };
                    let state = Arc::new(RwLock::new(controller.handle.shared_state.snapshot()));
                    self.retain_uncertain_unconnected_controller(
                        worker_ref,
                        controller.handle,
                        controller.shutdown,
                        Arc::new(AtomicBool::new(false)),
                        RuntimeExecutionTaskScope::new(controller.controller_task),
                        state,
                        Some(controller.workspace_client),
                        Some(pending.request.operation_id),
                        Vec::new(),
                    );
                }
                Some(Err(_)) => {
                    let factory = Arc::clone(&self.factory);
                    let request = pending.request.clone();
                    if let Err(message) = self
                        .run_cancellable_on_adapter_runtime(RUNTIME_TASK_TIMEOUT, async move {
                            factory.cleanup_failed_restore(&request).await
                        })
                    {
                        return WorkerExecutionResult::errored(
                            WorkerExecutionOperation::Stop,
                            message,
                        );
                    }
                }
                None => {
                    return WorkerExecutionResult::errored(
                        WorkerExecutionOperation::Stop,
                        "restore factory has no completion evidence; cleanup is unproven",
                    );
                }
            }
            drop(retained);
            if let Err(message) = self.remove_pending_restore(worker_ref) {
                return WorkerExecutionResult::errored(WorkerExecutionOperation::Stop, message);
            }
        }
        let execution = match self.workers.lock() {
            Ok(workers) => workers.get(worker_ref).cloned(),
            Err(_) => {
                return WorkerExecutionResult::errored(
                    WorkerExecutionOperation::Stop,
                    "worker adapter registry lock is poisoned",
                );
            }
        };
        let Some(execution) = execution else {
            // The execution backend cleanup may have committed before the
            // Runtime catalog commit failed. Treat the retry as converged so
            // the Runtime can durably finish its Stopped transition.
            let factory = Arc::clone(&self.factory);
            let worker_ref = worker_ref.clone();
            return match self.run_cancellable_on_adapter_runtime(RUNTIME_TASK_TIMEOUT, async move {
                factory.reconcile_stopped_worker(&worker_ref).await
            }) {
                Ok(()) => WorkerExecutionResult::accepted(WorkerExecutionOperation::Stop),
                Err(message) => {
                    WorkerExecutionResult::errored(WorkerExecutionOperation::Stop, message)
                }
            };
        };

        if let Err(message) = self.cleanup_execution_resources(&execution) {
            return WorkerExecutionResult::errored(WorkerExecutionOperation::Stop, message);
        }

        match self.workers.lock() {
            Ok(mut workers) => {
                workers.remove(worker_ref);
            }
            Err(_) => {
                return WorkerExecutionResult::errored(
                    WorkerExecutionOperation::Stop,
                    "worker adapter registry lock is poisoned; cleanup commit remains retryable",
                );
            }
        }
        WorkerExecutionResult::accepted(WorkerExecutionOperation::Stop)
    }
}

impl<F> Drop for WorkerRuntimeExecutionBackend<F> {
    fn drop(&mut self) {
        if let Ok(mut runtime) = self.runtime.lock()
            && let Some(runtime) = runtime.take()
        {
            let _ = std::thread::spawn(move || drop(runtime)).join();
        }
    }
}

fn apply_protocol_worker_state(
    current: &Arc<RwLock<protocol::WorkerStateSnapshot>>,
    event: &mut Event,
) -> Result<bool, String> {
    let incoming = match event {
        Event::WorkerState { snapshot } => snapshot,
        Event::Snapshot { state, .. } => state,
        Event::CommandAcknowledged { acknowledgement } => &mut acknowledgement.state,
        _ => return Ok(true),
    };
    let mut current = current
        .write()
        .map_err(|_| "worker state projection lock is poisoned".to_string())?;
    *current = incoming.clone();
    Ok(true)
}

impl<F> WorkerExecutionBackend for WorkerRuntimeExecutionBackend<F>
where
    F: RuntimeWorkerFactory,
{
    fn backend_id(&self) -> &str {
        &self.backend_id
    }

    fn fetch_workspace_config(
        &self,
        request: WorkspaceConfigFetchRequest,
    ) -> Result<WorkspaceConfigFetchResult, String> {
        let factory = self.factory.clone();
        self.run_on_adapter_runtime(async move { factory.fetch_workspace_config(request).await })
    }

    fn worker_session(
        &self,
        request: WorkerSessionObservationRequest,
    ) -> runtime_api::WorkerSessionAvailability {
        let unavailable = |message: String| runtime_api::WorkerSessionAvailability::Unavailable {
            reason: runtime_api::WorkerSessionUnavailableReason::StorageUnavailable,
            message,
        };
        let operation_lock = match self.worker_lock(&request.worker_ref) {
            Ok(lock) => lock,
            Err(message) => return unavailable(message),
        };
        let _operation_guard = match operation_lock.lock() {
            Ok(guard) => guard,
            Err(_) => return unavailable("worker operation lock is poisoned".to_string()),
        };
        let workers = match self.workers.lock() {
            Ok(workers) => workers,
            Err(_) => return unavailable("worker adapter registry lock is poisoned".to_string()),
        };
        if workers.contains_key(&request.worker_ref) {
            return runtime_api::WorkerSessionAvailability::LiveProtocol;
        }
        drop(workers);

        match self.factory.retained_session_snapshot(&request.worker_ref) {
            Ok(retained) => runtime_api::WorkerSessionAvailability::RetainedSnapshot {
                identity: runtime_api::RetainedSessionIdentity {
                    session_id: retained.identity.session_id,
                    segment_id: retained.identity.segment_id,
                    entry_count: retained.identity.entry_count,
                },
                snapshot: retained.snapshot,
            },
            Err(error) => {
                let reason = match error {
                    session_store::RetainedSnapshotReadError::RetentionMissing => {
                        runtime_api::WorkerSessionUnavailableReason::RetentionMissing
                    }
                    session_store::RetainedSnapshotReadError::ActivePointerMissing => {
                        runtime_api::WorkerSessionUnavailableReason::ActivePointerMissing
                    }
                    session_store::RetainedSnapshotReadError::MigrationRequired => {
                        runtime_api::WorkerSessionUnavailableReason::MigrationRequired
                    }
                    session_store::RetainedSnapshotReadError::CorruptLog => {
                        runtime_api::WorkerSessionUnavailableReason::CorruptLog
                    }
                    session_store::RetainedSnapshotReadError::StorageUnavailable => {
                        runtime_api::WorkerSessionUnavailableReason::StorageUnavailable
                    }
                    session_store::RetainedSnapshotReadError::SnapshotTooLarge => {
                        runtime_api::WorkerSessionUnavailableReason::SnapshotTooLarge
                    }
                };
                runtime_api::WorkerSessionAvailability::Unavailable {
                    reason,
                    message: error.to_string(),
                }
            }
        }
    }

    fn worker_session_history(
        &self,
        request: crate::execution::WorkerSessionHistoryRequest,
    ) -> runtime_api::WorkerSessionHistoryAvailability {
        match self.factory.retained_session_history_page(
            &request.worker_ref,
            request.cursor.as_deref(),
            request.limit,
        ) {
            Ok(page) => runtime_api::WorkerSessionHistoryAvailability::Page { page },
            Err(error) => {
                let reason = match error {
                    session_store::RetainedHistoryReadError::RetentionMissing => {
                        runtime_api::WorkerSessionHistoryUnavailableReason::RetentionMissing
                    }
                    session_store::RetainedHistoryReadError::ActivePointerMissing => {
                        runtime_api::WorkerSessionHistoryUnavailableReason::ActivePointerMissing
                    }
                    session_store::RetainedHistoryReadError::MigrationRequired => {
                        runtime_api::WorkerSessionHistoryUnavailableReason::MigrationRequired
                    }
                    session_store::RetainedHistoryReadError::CorruptLog => {
                        runtime_api::WorkerSessionHistoryUnavailableReason::CorruptLog
                    }
                    session_store::RetainedHistoryReadError::StorageUnavailable => {
                        runtime_api::WorkerSessionHistoryUnavailableReason::StorageUnavailable
                    }
                    session_store::RetainedHistoryReadError::InvalidCursor => {
                        runtime_api::WorkerSessionHistoryUnavailableReason::InvalidCursor
                    }
                    session_store::RetainedHistoryReadError::ResourceLimit => {
                        runtime_api::WorkerSessionHistoryUnavailableReason::ResourceLimit
                    }
                };
                runtime_api::WorkerSessionHistoryAvailability::Unavailable {
                    reason,
                    message: error.to_string(),
                }
            }
        }
    }

    fn worker_session_attachment(
        &self,
        request: crate::execution::WorkerSessionAttachmentRequest,
    ) -> Result<session_store::RetainedSessionAttachment, session_store::RetainedAttachmentReadError>
    {
        let operation_lock = self
            .worker_lock(&request.worker_ref)
            .map_err(|_| session_store::RetainedAttachmentReadError::StorageUnavailable)?;
        let _operation_guard = operation_lock
            .lock()
            .map_err(|_| session_store::RetainedAttachmentReadError::StorageUnavailable)?;
        let live = self
            .workers
            .lock()
            .map_err(|_| session_store::RetainedAttachmentReadError::StorageUnavailable)?
            .get(&request.worker_ref)
            .map(|execution| execution.handle.clone());
        if let Some(handle) = live {
            match handle.session_attachment(
                &request.session_id,
                &request.attachment_id,
                10 * 1024 * 1024,
            ) {
                Ok(attachment) => return Ok(attachment),
                Err(session_store::RetainedAttachmentReadError::SessionMismatch)
                | Err(session_store::RetainedAttachmentReadError::NotFound) => {}
                Err(error) => return Err(error),
            }
        }
        self.factory.retained_session_attachment(
            &request.worker_ref,
            &request.session_id,
            &request.attachment_id,
        )
    }

    fn observe_workspace_prompt_projection(
        &self,
        projection: worker::WorkspacePromptProjection,
    ) -> Result<(), String> {
        self.factory.observe_workspace_prompt_projection(projection)
    }

    fn create_working_directory(
        &self,
        request: &WorkingDirectoryRequest,
    ) -> Result<WorkingDirectoryStatus, WorkingDirectoryDiagnostic> {
        let Some(materializer) = self.working_directory_materializer.as_ref() else {
            return Err(WorkingDirectoryDiagnostic::rejected(
                "working_directory_materializer_unavailable",
                "working directory materialization requested, but no materializer is configured for this runtime backend",
            ));
        };
        Ok(materializer.create(request)?.status())
    }

    fn authorize_working_directory_repository_access(
        &self,
        request: &WorkingDirectoryRepositoryAccessRequest,
    ) -> Result<(), WorkingDirectoryDiagnostic> {
        let materializer = self.working_directory_materializer.as_ref().ok_or_else(|| {
            WorkingDirectoryDiagnostic::rejected(
                "working_directory_materializer_unavailable",
                "working directory Repository access requested, but no materializer is configured for this runtime backend",
            )
        })?;
        materializer.authorize_repository_access(request)
    }

    fn observe_repository_ref(
        &self,
        request: &RepositoryRefObservationRequest,
    ) -> Result<RepositoryRefObservation, WorkingDirectoryDiagnostic> {
        let materializer = self.working_directory_materializer.as_ref().ok_or_else(|| {
            WorkingDirectoryDiagnostic::rejected(
                "repository_ref_provider_unavailable",
                "Repository ref observation requested, but no materializer is configured for this Runtime backend",
            )
        })?;
        materializer.observe_repository_ref(request)
    }

    fn list_working_directories(&self) -> Vec<WorkingDirectoryStatus> {
        self.working_directory_materializer
            .as_ref()
            .and_then(|materializer| materializer.list_working_directories().ok())
            .unwrap_or_default()
    }

    fn working_directory(
        &self,
        working_directory_id: &str,
    ) -> Result<WorkingDirectoryStatus, WorkingDirectoryDiagnostic> {
        let Some(materializer) = self.working_directory_materializer.as_ref() else {
            return Err(WorkingDirectoryDiagnostic::rejected(
                "working_directory_materializer_unavailable",
                "working directory lookup requested, but no materializer is configured for this runtime backend",
            ));
        };
        materializer.working_directory_status(working_directory_id)
    }

    fn open_workdir_session(
        &self,
        working_directory_id: &str,
    ) -> Result<WorkdirSessionHandle, WorkingDirectoryDiagnostic> {
        let Some(materializer) = self.working_directory_materializer.as_ref() else {
            return Err(WorkingDirectoryDiagnostic::rejected(
                "working_directory_materializer_unavailable",
                "Workdir session requested, but no materializer is configured for this runtime backend",
            ));
        };
        let binding = materializer.bind_working_directory(working_directory_id, None)?;
        let scope = manifest::Scope::writable(binding.root()).map_err(|error| {
            WorkingDirectoryDiagnostic::rejected(
                "workdir_session_scope_invalid",
                format!("failed to create Workdir session scope: {error}"),
            )
        })?;
        let scope = manifest::SharedScope::new(scope);
        Ok(runtime_local_workdir_session(
            working_directory_id,
            binding.root(),
            binding.cwd(),
            scope.clone(),
            scope,
            WorkdirSessionCapabilities::ALL,
            binding.command_environment(),
            binding.session_resources(),
        ))
    }

    fn cleanup_working_directory(
        &self,
        working_directory_id: &str,
    ) -> Result<WorkingDirectoryStatus, WorkingDirectoryDiagnostic> {
        let Some(materializer) = self.working_directory_materializer.as_ref() else {
            return Err(WorkingDirectoryDiagnostic::rejected(
                "working_directory_materializer_unavailable",
                "working directory cleanup requested, but no materializer is configured for this runtime backend",
            ));
        };
        materializer.cleanup_working_directory(working_directory_id)
    }

    fn spawn_worker(&self, request: WorkerExecutionSpawnRequest) -> WorkerExecutionSpawnResult {
        let operation_lock = match self.worker_lock(&request.worker_ref) {
            Ok(lock) => lock,
            Err(message) => {
                return WorkerExecutionSpawnResult::Rejected(WorkerExecutionResult::errored(
                    WorkerExecutionOperation::Spawn,
                    message,
                ));
            }
        };
        let _operation_guard = match operation_lock.lock() {
            Ok(guard) => guard,
            Err(_) => {
                let message = "worker operation lock is poisoned".to_string();
                return WorkerExecutionSpawnResult::Rejected(WorkerExecutionResult::errored(
                    WorkerExecutionOperation::Spawn,
                    message,
                ));
            }
        };
        match self.pending_restore(&request.worker_ref) {
            Ok(None) => {}
            Ok(Some(_)) => {
                return WorkerExecutionSpawnResult::Rejected(WorkerExecutionResult::busy(
                    WorkerExecutionOperation::Spawn,
                    "Worker restore still owns pending resources",
                ));
            }
            Err(message) => {
                return WorkerExecutionSpawnResult::Rejected(WorkerExecutionResult::errored(
                    WorkerExecutionOperation::Spawn,
                    message,
                ));
            }
        }
        let workers = match self.workers.lock() {
            Ok(workers) => workers,
            Err(_) => {
                return WorkerExecutionSpawnResult::Rejected(WorkerExecutionResult::errored(
                    WorkerExecutionOperation::Spawn,
                    "worker adapter registry lock is poisoned",
                ));
            }
        };
        if workers.contains_key(&request.worker_ref) {
            return WorkerExecutionSpawnResult::Rejected(WorkerExecutionResult::busy(
                WorkerExecutionOperation::Spawn,
                "Worker is already connected to execution backend",
            ));
        }
        drop(workers);

        let mut request = request;
        let Some(materializer) = self.working_directory_materializer.as_ref().or_else(|| {
            (request.request.workdir_attachment_requests.is_empty()
                && request.request.workdir_attachments.is_empty())
            .then_some(())
            .and(None)
        }) else {
            if request.request.workdir_attachment_requests.is_empty()
                && request.request.workdir_attachments.is_empty()
            {
                // No provider is needed for a Workdir-less Worker.
                let factory = self.factory.clone();
                let bridge_context = request.context.clone();
                let worker_ref = request.worker_ref.clone();
                let spawn_result = self
                    .run_cancellable_on_adapter_runtime(self.spawn_restore_timeout, async move {
                        factory.spawn_controller(request).await
                    });
                let controller = match spawn_result {
                    Ok(controller) => controller,
                    Err(message) => {
                        return WorkerExecutionSpawnResult::Errored(
                            WorkerExecutionResult::errored(
                                WorkerExecutionOperation::Spawn,
                                message,
                            ),
                        );
                    }
                };
                return self.connect_handle(
                    WorkerExecutionOperation::Spawn,
                    worker_ref,
                    bridge_context,
                    controller.handle,
                    controller.shutdown,
                    controller.controller_task,
                    None,
                    BTreeMap::new(),
                    Some(controller.workspace_client),
                );
            }
            return WorkerExecutionSpawnResult::Rejected(WorkerExecutionResult::rejected(
                WorkerExecutionOperation::Spawn,
                "Workdir attachments were requested, but no materializer is configured for this Runtime backend",
            ));
        };
        let mut rollback_workdirs: Vec<String> = Vec::new();
        for attachment in request.request.workdir_attachment_requests.clone() {
            if request.workdir_attachments.contains_key(&attachment.alias) {
                let message = format!("duplicate Workdir attachment alias `{}`", attachment.alias);
                if let Err(result) = rollback_materialized_spawn_workdirs(
                    materializer.as_ref(),
                    &request.workdir_attachments,
                    &rollback_workdirs,
                    &message,
                ) {
                    return result;
                }
                return WorkerExecutionSpawnResult::Rejected(WorkerExecutionResult::rejected(
                    WorkerExecutionOperation::Spawn,
                    message,
                ));
            }
            match materializer.materialize(&request.worker_ref, &attachment.working_directory) {
                Ok(binding) => {
                    rollback_workdirs.push(binding.working_directory.id.clone());
                    request
                        .workdir_attachments
                        .insert(attachment.alias, binding);
                }
                Err(error) => {
                    let message = error.to_string();
                    if let Err(result) = rollback_materialized_spawn_workdirs(
                        materializer.as_ref(),
                        &request.workdir_attachments,
                        &rollback_workdirs,
                        &message,
                    ) {
                        return result;
                    }
                    return WorkerExecutionSpawnResult::Rejected(WorkerExecutionResult::rejected(
                        WorkerExecutionOperation::Spawn,
                        message,
                    ));
                }
            }
        }
        for attachment in request.request.workdir_attachments.clone() {
            if request.workdir_attachments.contains_key(&attachment.alias) {
                let message = format!("duplicate Workdir attachment alias `{}`", attachment.alias);
                if let Err(result) = rollback_materialized_spawn_workdirs(
                    materializer.as_ref(),
                    &request.workdir_attachments,
                    &rollback_workdirs,
                    &message,
                ) {
                    return result;
                }
                return WorkerExecutionSpawnResult::Rejected(WorkerExecutionResult::rejected(
                    WorkerExecutionOperation::Spawn,
                    message,
                ));
            }
            match materializer.bind_working_directory(
                &attachment.working_directory_id,
                attachment.relative_cwd.as_deref(),
            ) {
                Ok(binding) => {
                    request
                        .workdir_attachments
                        .insert(attachment.alias, binding);
                }
                Err(error) => {
                    let message = error.to_string();
                    if let Err(result) = rollback_materialized_spawn_workdirs(
                        materializer.as_ref(),
                        &request.workdir_attachments,
                        &rollback_workdirs,
                        &message,
                    ) {
                        return result;
                    }
                    return WorkerExecutionSpawnResult::Rejected(WorkerExecutionResult::rejected(
                        WorkerExecutionOperation::Spawn,
                        message,
                    ));
                }
            }
        }
        let workdir_attachments = request.workdir_attachments.clone();

        let factory = self.factory.clone();
        let bridge_context = request.context.clone();
        let worker_ref = request.worker_ref.clone();
        let spawn_result = self
            .run_cancellable_on_adapter_runtime(self.spawn_restore_timeout, async move {
                factory.spawn_controller(request).await
            });

        let controller = match spawn_result {
            Ok(controller) => controller,
            Err(message) => {
                if let Err(result) = rollback_materialized_spawn_workdirs(
                    materializer.as_ref(),
                    &workdir_attachments,
                    &rollback_workdirs,
                    &message,
                ) {
                    return result;
                }
                return WorkerExecutionSpawnResult::Errored(WorkerExecutionResult::errored(
                    WorkerExecutionOperation::Spawn,
                    message,
                ));
            }
        };

        self.connect_handle(
            WorkerExecutionOperation::Spawn,
            worker_ref,
            bridge_context,
            controller.handle,
            controller.shutdown,
            controller.controller_task,
            None,
            workdir_attachments,
            Some(controller.workspace_client),
        )
    }

    fn preflight_restore(
        &self,
        request: &WorkerExecutionRestoreRequest,
    ) -> Result<(), WorkerExecutionResult> {
        let operation_lock = match self.worker_lock(&request.worker_ref) {
            Ok(lock) => lock,
            Err(message) => {
                return Err(WorkerExecutionResult::errored(
                    WorkerExecutionOperation::Restore,
                    message,
                ));
            }
        };
        let _operation_guard = match operation_lock.lock() {
            Ok(guard) => guard,
            Err(_) => {
                let message = "worker operation lock is poisoned".to_string();
                return Err(WorkerExecutionResult::errored(
                    WorkerExecutionOperation::Restore,
                    message,
                ));
            }
        };
        if self
            .pending_restore(&request.worker_ref)
            .map_err(|message| {
                WorkerExecutionResult::errored(WorkerExecutionOperation::Restore, message)
            })?
            .is_some()
        {
            return Err(WorkerExecutionResult::busy(
                WorkerExecutionOperation::Restore,
                "Worker restore still owns pending resources",
            ));
        }
        let workers = self.workers.lock().map_err(|_| {
            WorkerExecutionResult::errored(
                WorkerExecutionOperation::Restore,
                "worker adapter registry lock is poisoned",
            )
        })?;
        if let Some(existing) = workers.get(&request.worker_ref) {
            let result = WorkerExecutionResult::busy(
                WorkerExecutionOperation::Restore,
                "Worker is already connected to execution backend",
            );
            if existing.shutdown_requested.load(Ordering::Acquire)
                || existing.tasks.has_cleanup_candidates().map_err(|message| {
                    WorkerExecutionResult::errored(WorkerExecutionOperation::Restore, message)
                })?
            {
                return Err(result);
            }
            let snapshot = existing
                .worker_state
                .read()
                .map_err(|_| {
                    WorkerExecutionResult::errored(
                        WorkerExecutionOperation::Restore,
                        "worker state lock is poisoned",
                    )
                })?
                .clone();
            // Read-only AlreadyConnected evidence. The Runtime may accept this
            // exact restore without journaling or replacing the current owner.
            return Err(result.with_worker_state(snapshot));
        }
        drop(workers);

        if !request.previous_workdir_attachments.is_empty() {
            let materializer = self
                .working_directory_materializer
                .as_ref()
                .ok_or_else(|| {
                    WorkerExecutionResult::rejected(
                        WorkerExecutionOperation::Restore,
                        "Persisted Worker Workdir attachments cannot be restored by this Runtime",
                    )
                })?;
            for previous in &request.previous_workdir_attachments {
                let current = materializer
                    .working_directory_status(
                        &previous.working_directory.summary.working_directory_id,
                    )
                    .map_err(|message| {
                        WorkerExecutionResult::rejected(
                            WorkerExecutionOperation::Restore,
                            format!(
                                "Persisted Worker Workdir attachment `{}` is unavailable: {message}",
                                previous.alias
                            ),
                        )
                    })?;
                if current.summary.status != WorkingDirectoryStatusKind::Active {
                    return Err(WorkerExecutionResult::rejected(
                        WorkerExecutionOperation::Restore,
                        format!(
                            "Persisted Worker Workdir attachment `{}` is not active",
                            previous.alias
                        ),
                    ));
                }
            }
        }
        if request.previous_workdir_attachments.is_empty()
            && !request.request.workdir_attachment_requests.is_empty()
        {
            return Err(WorkerExecutionResult::rejected(
                WorkerExecutionOperation::Restore,
                "Persisted Worker Workdir allocations are unavailable",
            ));
        }
        if request.previous_workdir_attachments.is_empty()
            && !request.request.workdir_attachments.is_empty()
            && self.working_directory_materializer.is_none()
        {
            return Err(WorkerExecutionResult::rejected(
                WorkerExecutionOperation::Restore,
                "Persisted Worker Workdir claims cannot be restored by this Runtime",
            ));
        }
        if let Some(workspace_api) = request.request.workspace_api.as_ref() {
            let Some(scope) = request.workspace_scope.as_ref() else {
                return Err(WorkerExecutionResult::rejected(
                    WorkerExecutionOperation::Restore,
                    "Persisted Workspace restore authorization is unavailable",
                ));
            };
            if scope.workspace_id != workspace_api.workspace_id {
                return Err(WorkerExecutionResult::rejected(
                    WorkerExecutionOperation::Restore,
                    "Persisted Workspace restore authorization does not match Worker authority",
                ));
            }
        }

        let factory = Arc::clone(&self.factory);
        let request = request.clone();
        self.run_on_adapter_runtime(async move { factory.preflight_restore(&request).await })
            .map_err(|message| {
                WorkerExecutionResult::rejected(WorkerExecutionOperation::Restore, message)
            })?;
        Ok(())
    }

    fn restore_worker(
        &self,
        mut request: WorkerExecutionRestoreRequest,
    ) -> WorkerExecutionSpawnResult {
        let operation_lock = match self.worker_lock(&request.worker_ref) {
            Ok(lock) => lock,
            Err(message) => {
                return WorkerExecutionSpawnResult::Rejected(WorkerExecutionResult::errored(
                    WorkerExecutionOperation::Restore,
                    message,
                ));
            }
        };
        let _operation_guard = match operation_lock.lock() {
            Ok(guard) => guard,
            Err(_) => {
                let message = "worker operation lock is poisoned".to_string();
                return WorkerExecutionSpawnResult::Rejected(WorkerExecutionResult::errored(
                    WorkerExecutionOperation::Restore,
                    message,
                ));
            }
        };
        let workers = self.workers.lock().map_err(|_| {
            WorkerExecutionResult::errored(
                WorkerExecutionOperation::Restore,
                "worker adapter registry lock is poisoned",
            )
        });
        let workers = match workers {
            Ok(workers) => workers,
            Err(result) => return WorkerExecutionSpawnResult::Rejected(result),
        };
        if let Some(existing) = workers.get(&request.worker_ref) {
            let pending_cleanup = existing.tasks.has_cleanup_candidates();
            if existing.shutdown_requested.load(Ordering::Acquire)
                || !matches!(pending_cleanup.as_ref(), Ok(false))
            {
                return WorkerExecutionSpawnResult::ReconciliationRequired {
                    result: WorkerExecutionResult::errored(
                        WorkerExecutionOperation::Restore,
                        pending_cleanup.err().unwrap_or_else(|| {
                            "Worker still owns unproven Controller cleanup".to_string()
                        }),
                    ),
                    worker_state: None,
                    workdir_attachments: existing.workdir_attachments.clone(),
                };
            }
            if existing.restore_operation_id == Some(request.operation_id) {
                let worker_state = existing
                    .worker_state
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                return WorkerExecutionSpawnResult::Connected {
                    worker_state,
                    workdir_attachments: existing.workdir_attachments.clone(),
                };
            }
            return WorkerExecutionSpawnResult::Rejected(WorkerExecutionResult::busy(
                WorkerExecutionOperation::Restore,
                "Worker is already connected to execution backend by another lifecycle operation",
            ));
        }
        drop(workers);

        let mut workdir_attachments = BTreeMap::new();
        if !request.previous_workdir_attachments.is_empty() {
            let Some(materializer) = self.working_directory_materializer.as_ref() else {
                return WorkerExecutionSpawnResult::Rejected(WorkerExecutionResult::rejected(
                    WorkerExecutionOperation::Restore,
                    "persisted Worker has Workdir attachments, but no materializer is configured for this Runtime backend",
                ));
            };
            for status in request.previous_workdir_attachments.clone() {
                let relative_cwd = request
                    .request
                    .workdir_attachments
                    .iter()
                    .find(|claim| claim.alias == status.alias)
                    .and_then(|claim| claim.relative_cwd.as_deref());
                match materializer.bind_working_directory(
                    &status.working_directory.summary.working_directory_id,
                    relative_cwd,
                ) {
                    Ok(binding) => {
                        if workdir_attachments
                            .insert(status.alias.clone(), binding)
                            .is_some()
                        {
                            return WorkerExecutionSpawnResult::Rejected(
                                WorkerExecutionResult::rejected(
                                    WorkerExecutionOperation::Restore,
                                    format!(
                                        "duplicate persisted Workdir attachment alias `{}`",
                                        status.alias
                                    ),
                                ),
                            );
                        }
                    }
                    Err(error) => {
                        return WorkerExecutionSpawnResult::Rejected(
                            WorkerExecutionResult::rejected(
                                WorkerExecutionOperation::Restore,
                                error.to_string(),
                            ),
                        );
                    }
                }
            }
        } else if !request.request.workdir_attachments.is_empty() {
            let Some(materializer) = self.working_directory_materializer.as_ref() else {
                return WorkerExecutionSpawnResult::Rejected(WorkerExecutionResult::rejected(
                    WorkerExecutionOperation::Restore,
                    "persisted Worker has Workdir claims, but no materializer is configured for this Runtime backend",
                ));
            };
            for claim in request.request.workdir_attachments.clone() {
                match materializer.bind_working_directory(
                    &claim.working_directory_id,
                    claim.relative_cwd.as_deref(),
                ) {
                    Ok(binding) => {
                        if workdir_attachments
                            .insert(claim.alias.clone(), binding)
                            .is_some()
                        {
                            return WorkerExecutionSpawnResult::Rejected(
                                WorkerExecutionResult::rejected(
                                    WorkerExecutionOperation::Restore,
                                    format!("duplicate Workdir attachment alias `{}`", claim.alias),
                                ),
                            );
                        }
                    }
                    Err(error) => {
                        return WorkerExecutionSpawnResult::Rejected(
                            WorkerExecutionResult::rejected(
                                WorkerExecutionOperation::Restore,
                                error.to_string(),
                            ),
                        );
                    }
                }
            }
        }
        request.workdir_attachments = workdir_attachments.clone();

        let worker_ref = request.worker_ref.clone();
        let operation_id = request.operation_id;
        let pending = match self.pending_restore(&worker_ref) {
            Ok(Some(pending)) if pending.request.operation_id == operation_id => pending,
            Ok(Some(_)) => {
                return WorkerExecutionSpawnResult::ReconciliationRequired {
                    result: WorkerExecutionResult::busy(
                        WorkerExecutionOperation::Restore,
                        "another restore operation still owns pending resources",
                    ),
                    worker_state: None,
                    workdir_attachments: Vec::new(),
                };
            }
            Ok(None) => {
                let factory = Arc::clone(&self.factory);
                let preflight_request = request.clone();
                if let Err(message) = self.run_on_adapter_runtime(async move {
                    factory.preflight_restore(&preflight_request).await
                }) {
                    return WorkerExecutionSpawnResult::Rejected(WorkerExecutionResult::rejected(
                        WorkerExecutionOperation::Restore,
                        message,
                    ));
                }
                let pending = Arc::new(PendingRuntimeRestore {
                    request: request.clone(),
                    task: tokio::sync::Mutex::new(None),
                    result: Mutex::new(None),
                    failure: Mutex::new(None),
                });
                let mut resources = match self.pending_restores.lock() {
                    Ok(resources) => resources,
                    Err(_) => {
                        return WorkerExecutionSpawnResult::Rejected(
                            WorkerExecutionResult::errored(
                                WorkerExecutionOperation::Restore,
                                "pending restore resources lock is poisoned",
                            ),
                        );
                    }
                };
                resources.insert(worker_ref.clone(), Arc::clone(&pending));
                let factory = Arc::clone(&self.factory);
                let result_owner = Arc::clone(&pending);
                let task = self.spawn_on_adapter_runtime(async move {
                    let result = factory.restore_controller(request).await;
                    let mut retained = result_owner
                        .result
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    *retained = Some(result);
                });
                match task {
                    Ok(task) => {
                        *pending.task.try_lock().expect("new restore task lock") = Some(task)
                    }
                    Err(message) => {
                        resources.remove(&worker_ref);
                        return WorkerExecutionSpawnResult::Rejected(
                            WorkerExecutionResult::errored(
                                WorkerExecutionOperation::Restore,
                                message,
                            ),
                        );
                    }
                }
                drop(resources);
                pending
            }
            Err(message) => {
                return WorkerExecutionSpawnResult::ReconciliationRequired {
                    result: WorkerExecutionResult::errored(
                        WorkerExecutionOperation::Restore,
                        message,
                    ),
                    worker_state: None,
                    workdir_attachments: Vec::new(),
                };
            }
        };
        if let Err(message) = self.wait_pending_restore(&pending, self.spawn_restore_timeout) {
            return WorkerExecutionSpawnResult::ReconciliationRequired {
                result: WorkerExecutionResult::errored(WorkerExecutionOperation::Restore, message),
                worker_state: None,
                workdir_attachments: Vec::new(),
            };
        }
        let controller = {
            let mut retained = match pending.result.lock() {
                Ok(retained) => retained,
                Err(_) => {
                    return WorkerExecutionSpawnResult::ReconciliationRequired {
                        result: WorkerExecutionResult::errored(
                            WorkerExecutionOperation::Restore,
                            "restore result lock is poisoned",
                        ),
                        worker_state: None,
                        workdir_attachments: Vec::new(),
                    };
                }
            };
            match retained.as_ref() {
                Some(Ok(_)) => match retained.take().unwrap() {
                    Ok(controller) => controller,
                    Err(_) => unreachable!(),
                },
                Some(Err(message)) => {
                    return WorkerExecutionSpawnResult::ReconciliationRequired {
                        result: WorkerExecutionResult::errored(
                            WorkerExecutionOperation::Restore,
                            message.clone(),
                        ),
                        worker_state: None,
                        workdir_attachments: Vec::new(),
                    };
                }
                None => {
                    return WorkerExecutionSpawnResult::ReconciliationRequired {
                        result: WorkerExecutionResult::errored(
                            WorkerExecutionOperation::Restore,
                            "restore factory completion has no result; cleanup is unproven",
                        ),
                        worker_state: None,
                        workdir_attachments: Vec::new(),
                    };
                }
            }
        };

        let result = self.connect_handle(
            WorkerExecutionOperation::Restore,
            worker_ref.clone(),
            pending.request.context.clone(),
            controller.handle,
            controller.shutdown,
            controller.controller_task,
            Some(operation_id),
            pending.request.workdir_attachments.clone(),
            Some(controller.workspace_client),
        );
        if let Err(message) = self.remove_pending_restore(&worker_ref) {
            return WorkerExecutionSpawnResult::ReconciliationRequired {
                result: WorkerExecutionResult::errored(WorkerExecutionOperation::Restore, message),
                worker_state: None,
                workdir_attachments: Vec::new(),
            };
        }
        result
    }

    fn activate_restored_worker(
        &self,
        operation_id: crate::execution::WorkerLifecycleOperationId,
        worker_ref: &WorkerRef,
    ) -> Result<(), String> {
        let operation_lock = match self.worker_lock(worker_ref) {
            Ok(lock) => lock,
            Err(message) => return Err(message),
        };
        let _operation_guard = match operation_lock.lock() {
            Ok(guard) => guard,
            Err(_) => {
                let message = "worker operation lock is poisoned".to_string();
                return Err(message);
            }
        };
        let workers = self
            .workers
            .lock()
            .map_err(|_| "worker adapter registry lock is poisoned".to_string())?;
        let execution = workers
            .get(worker_ref)
            .ok_or_else(|| "restored Worker execution is not registered".to_string())?;
        if execution.restore_operation_id != Some(operation_id) {
            return Err("restored Worker operation identity does not match".to_string());
        }
        self.factory.activate_restored_controller(
            worker_ref,
            execution.workspace_id.as_deref(),
            &execution.handle,
        );
        Ok(())
    }

    fn dispatch_input(&self, worker_ref: &WorkerRef, input: WorkerInput) -> WorkerExecutionResult {
        let operation_lock = match self.worker_lock(worker_ref) {
            Ok(lock) => lock,
            Err(message) => {
                return WorkerExecutionResult::errored(WorkerExecutionOperation::Input, message);
            }
        };
        let _operation_guard = match operation_lock.lock() {
            Ok(guard) => guard,
            Err(_) => {
                let message = "worker operation lock is poisoned".to_string();
                return WorkerExecutionResult::errored(WorkerExecutionOperation::Input, message);
            }
        };
        if let Err(error) = crate::runtime::validate_worker_input(&input) {
            return WorkerExecutionResult::rejected(
                WorkerExecutionOperation::Input,
                error.to_string(),
            );
        }
        let (worker, worker_state, _workspace_client) = match self.get_execution(worker_ref) {
            Ok(execution) => execution,
            Err(mut result) => {
                result.operation = WorkerExecutionOperation::Input;
                return result;
            }
        };

        if input.kind == WorkerInputKind::Notify {
            let notification_request_id = input
                .submission_request_id
                .unwrap_or_else(protocol::new_submission_request_id);
            return self.send_notification_and_wait_for_acceptance(
                WorkerExecutionOperation::Input,
                worker,
                Method::NotifyTracked {
                    notification_request_id: notification_request_id.clone(),
                    message: input.content,
                    source: protocol::AuthenticatedInputSource::Backend {
                        operation_id: notification_request_id.clone(),
                    },
                },
                notification_request_id,
            );
        }

        if input.kind == WorkerInputKind::Compact {
            let command = match next_internal_command(&worker_state) {
                Ok(command) => command,
                Err(error) => {
                    return WorkerExecutionResult::errored(WorkerExecutionOperation::Input, error);
                }
            };
            return self.send_method(
                WorkerExecutionOperation::Input,
                worker,
                Method::Compact { command },
            );
        }

        let (method, submission_request_id) = match input.kind {
            WorkerInputKind::User | WorkerInputKind::UserIfIdle => {
                let Some(submission_id) = input
                    .submission_request_id
                    .filter(|submission_id| !submission_id.trim().is_empty())
                else {
                    return WorkerExecutionResult::rejected(
                        WorkerExecutionOperation::Input,
                        "Runtime user input is missing its internal submission id",
                    );
                };
                let segments = input
                    .segments
                    .unwrap_or_else(|| vec![Segment::text(input.content.trim().to_string())]);
                let source = protocol::AuthenticatedInputSource::Backend {
                    operation_id: submission_id.clone(),
                };
                let method = if input.kind == WorkerInputKind::UserIfIdle {
                    Method::SubmitIfIdle {
                        submission_request_id: submission_id.clone(),
                        input: segments,
                        source,
                    }
                } else {
                    Method::SubmitTracked {
                        submission_request_id: submission_id.clone(),
                        input: segments,
                        source,
                    }
                };
                (method, Some(submission_id))
            }
            WorkerInputKind::Notify => {
                unreachable!("Notify input is dispatched before ordinary input mapping")
            }
            WorkerInputKind::Compact => unreachable!("compact input is dispatched above"),
            WorkerInputKind::ListRewindTargets => (Method::ListRewindTargets, None),
            WorkerInputKind::RegisterPeer => (
                Method::RegisterPeer {
                    name: input.content.trim().to_string(),
                },
                None,
            ),
        };
        let waits_for_submission_acceptance = submission_request_id.is_some();

        if waits_for_submission_acceptance {
            self.send_submit_and_wait_for_acceptance(
                WorkerExecutionOperation::Input,
                worker,
                method,
                submission_request_id.expect("Submit must have a submission request id"),
            )
        } else {
            self.send_method(WorkerExecutionOperation::Input, worker, method)
        }
    }

    fn upload_file(
        &self,
        worker_ref: &WorkerRef,
        file_name: &str,
        media_type: &str,
        content: &[u8],
        context: Option<&session_store::UploadedFileUploadContext>,
    ) -> Result<protocol::UploadedFileRef, WorkerExecutionResult> {
        let operation_lock = match self.worker_lock(worker_ref) {
            Ok(lock) => lock,
            Err(message) => {
                return Err(WorkerExecutionResult::errored(
                    WorkerExecutionOperation::UploadFile,
                    message,
                ));
            }
        };
        let _operation_guard = match operation_lock.lock() {
            Ok(guard) => guard,
            Err(_) => {
                let message = "worker operation lock is poisoned".to_string();
                return Err(WorkerExecutionResult::errored(
                    WorkerExecutionOperation::UploadFile,
                    message,
                ));
            }
        };
        let (worker, _, _) = self.get_execution(worker_ref).map_err(|mut result| {
            result.operation = WorkerExecutionOperation::UploadFile;
            result
        })?;
        let uploaded = match context {
            Some(context) => {
                worker.upload_file_with_context(file_name, media_type, content, context)
            }
            None => worker.upload_file(file_name, media_type, content),
        };
        uploaded.map_err(|error| {
            WorkerExecutionResult::rejected(
                WorkerExecutionOperation::UploadFile,
                format!("uploaded_file_rejected: {error}"),
            )
        })
    }

    fn delete_uploaded_file(
        &self,
        worker_ref: &WorkerRef,
        artifact_id: &str,
    ) -> WorkerExecutionResult {
        let operation_lock = match self.worker_lock(worker_ref) {
            Ok(lock) => lock,
            Err(message) => {
                return WorkerExecutionResult::errored(
                    WorkerExecutionOperation::DeleteUploadedFile,
                    message,
                );
            }
        };
        let _operation_guard = match operation_lock.lock() {
            Ok(guard) => guard,
            Err(_) => {
                let message = "worker operation lock is poisoned".to_string();
                return WorkerExecutionResult::errored(
                    WorkerExecutionOperation::DeleteUploadedFile,
                    message,
                );
            }
        };
        let (worker, _, _) = match self.get_execution(worker_ref) {
            Ok(execution) => execution,
            Err(mut result) => {
                result.operation = WorkerExecutionOperation::DeleteUploadedFile;
                return result;
            }
        };
        match worker.delete_uploaded_file(artifact_id) {
            Ok(_) => WorkerExecutionResult::accepted(WorkerExecutionOperation::DeleteUploadedFile),
            Err(error) => WorkerExecutionResult::rejected(
                WorkerExecutionOperation::DeleteUploadedFile,
                format!("uploaded_file_delete_rejected: {error}"),
            ),
        }
    }

    fn dispatch_method(&self, worker_ref: &WorkerRef, method: Method) -> WorkerExecutionResult {
        let operation_lock = match self.worker_lock(worker_ref) {
            Ok(lock) => lock,
            Err(message) => {
                return WorkerExecutionResult::errored(
                    WorkerExecutionOperation::ProtocolMethod,
                    message,
                );
            }
        };
        let _operation_guard = match operation_lock.lock() {
            Ok(guard) => guard,
            Err(_) => {
                let message = "worker operation lock is poisoned".to_string();
                return WorkerExecutionResult::errored(
                    WorkerExecutionOperation::ProtocolMethod,
                    message,
                );
            }
        };
        let (worker, _worker_state, _workspace_client) = match self.get_execution(worker_ref) {
            Ok(execution) => execution,
            Err(mut result) => {
                result.operation = WorkerExecutionOperation::ProtocolMethod;
                return result;
            }
        };

        self.send_method(WorkerExecutionOperation::ProtocolMethod, worker, method)
    }

    fn stop_worker_operation(&self, request: WorkerExecutionStopRequest) -> WorkerExecutionResult {
        self.stop_worker(&request.worker_ref)
    }

    fn stop_worker(&self, worker_ref: &WorkerRef) -> WorkerExecutionResult {
        let operation_lock = match self.worker_lock(worker_ref) {
            Ok(lock) => lock,
            Err(message) => {
                return WorkerExecutionResult::errored(WorkerExecutionOperation::Stop, message);
            }
        };
        let _operation_guard = match operation_lock.lock() {
            Ok(guard) => guard,
            Err(_) => {
                let message = "worker operation lock is poisoned".to_string();
                return WorkerExecutionResult::errored(WorkerExecutionOperation::Stop, message);
            }
        };
        self.stop_worker_under_lock(worker_ref)
    }

    fn cancel_worker(&self, worker_ref: &WorkerRef) -> WorkerExecutionResult {
        let operation_lock = match self.worker_lock(worker_ref) {
            Ok(lock) => lock,
            Err(message) => {
                return WorkerExecutionResult::errored(WorkerExecutionOperation::Cancel, message);
            }
        };
        let _operation_guard = match operation_lock.lock() {
            Ok(guard) => guard,
            Err(_) => {
                let message = "worker operation lock is poisoned".to_string();
                return WorkerExecutionResult::errored(WorkerExecutionOperation::Cancel, message);
            }
        };
        let (worker, worker_state, _workspace_client) = match self.get_execution(worker_ref) {
            Ok(execution) => execution,
            Err(mut result) => {
                result.operation = WorkerExecutionOperation::Cancel;
                return result;
            }
        };
        let command = match next_internal_command(&worker_state) {
            Ok(command) => command,
            Err(error) => {
                return WorkerExecutionResult::errored(WorkerExecutionOperation::Cancel, error);
            }
        };
        self.send_method(
            WorkerExecutionOperation::Cancel,
            worker,
            Method::Cancel { command },
        )
    }

    #[cfg(feature = "ws-server")]
    fn attach_worker_protocol(
        self: Arc<Self>,
        worker_ref: &WorkerRef,
    ) -> Result<WorkerProtocolTransport, WorkerExecutionResult> {
        let operation_lock = self.worker_lock(worker_ref).map_err(|message| {
            WorkerExecutionResult::errored(WorkerExecutionOperation::ProtocolMethod, message)
        })?;
        let _operation_guard = operation_lock.lock().map_err(|_| {
            WorkerExecutionResult::errored(
                WorkerExecutionOperation::ProtocolMethod,
                "worker operation lock is poisoned",
            )
        })?;
        let execution = self
            .workers
            .lock()
            .map_err(|_| {
                WorkerExecutionResult::errored(
                    WorkerExecutionOperation::ProtocolMethod,
                    "worker adapter registry lock is poisoned",
                )
            })?
            .get(worker_ref)
            .cloned()
            .ok_or_else(|| {
                WorkerExecutionResult::rejected(
                    WorkerExecutionOperation::ProtocolMethod,
                    "Worker has no live protocol endpoint",
                )
            })?;
        self.validate_protocol_execution_under_lock(worker_ref, &execution)?;
        let streams = subscribe_worker_protocol_session(&execution.handle);
        let snapshot = streams.snapshot_event;
        let (tx, events) = tokio::sync::mpsc::channel(128);
        let relay_handle = execution.handle.clone();
        let task = self
            .spawn_on_adapter_runtime(async move {
                for alert in streams.alert_snapshot {
                    tokio::select! {
                        biased;
                        _ = relay_handle.protocol_closed() => return,
                        sent = tx.send(Event::Alert(alert)) => if sent.is_err() { return; },
                    }
                }
                let mut protocol_events = streams.events;
                let mut log_entries = streams.log_entries;
                loop {
                    let event = tokio::select! {
                        biased;
                        _ = relay_handle.protocol_closed() => break,
                        _ = tx.closed() => break,
                        event = protocol_events.recv() => match event {
                            Ok(event) => Some(event),
                            Err(_) => break,
                        },
                        entry = log_entries.recv() => match entry {
                            Ok(entry) => live_log_entry_event(entry),
                            Err(_) => break,
                        },
                    };
                    if let Some(event) = event {
                        tokio::select! {
                            biased;
                            _ = relay_handle.protocol_closed() => break,
                            sent = tx.send(event) => if sent.is_err() { break; },
                        }
                    }
                }
            })
            .map_err(|message| {
                WorkerExecutionResult::errored(WorkerExecutionOperation::ProtocolMethod, message)
            })?;
        // Stop owns the Controller and every attached relay in this same scope.
        // Its abort/join barrier closes the bounded stream before cleanup succeeds.
        execution.tasks.push("protocol transport", task, true);

        let dispatcher_backend = Arc::clone(&self);
        let dispatcher_lock = Arc::clone(&operation_lock);
        let dispatcher_execution = execution.clone();
        let dispatcher_worker_ref = worker_ref.clone();
        let dispatch = Arc::new(move |method| {
            let _guard = dispatcher_lock.lock().map_err(|_| {
                WorkerExecutionResult::errored(
                    WorkerExecutionOperation::ProtocolMethod,
                    "worker operation lock is poisoned",
                )
            })?;
            dispatcher_backend.dispatch_protocol_execution_under_lock(
                &dispatcher_worker_ref,
                &dispatcher_execution,
                method,
            )
        });
        let validator_backend = Arc::clone(&self);
        let validator_lock = Arc::clone(&operation_lock);
        let validator_worker_ref = worker_ref.clone();
        let validate = Arc::new(move || {
            let _guard = validator_lock.lock().map_err(|_| {
                WorkerExecutionResult::errored(
                    WorkerExecutionOperation::ProtocolMethod,
                    "worker operation lock is poisoned",
                )
            })?;
            validator_backend
                .validate_protocol_execution_under_lock(&validator_worker_ref, &execution)
        });
        Ok(WorkerProtocolTransport::new(
            worker_ref.clone(),
            snapshot,
            events,
            dispatch,
            validate,
        ))
    }

    #[cfg(feature = "ws-server")]
    fn worker_snapshot(&self, worker_ref: &WorkerRef) -> Option<protocol::Event> {
        let operation_lock = match self.worker_lock(worker_ref) {
            Ok(lock) => lock,
            Err(_) => return None,
        };
        let _operation_guard = match operation_lock.lock() {
            Ok(guard) => guard,
            Err(_) => return None,
        };
        let workers = self.workers.lock().ok()?;
        workers
            .get(worker_ref)
            .map(|execution| execution.handle.snapshot_event())
    }

    fn worker_completions(
        &self,
        worker_ref: &WorkerRef,
        kind: protocol::CompletionKind,
        prefix: &str,
        context: Option<&protocol::CompletionContext>,
    ) -> Vec<protocol::CompletionEntry> {
        let operation_lock = match self.worker_lock(worker_ref) {
            Ok(lock) => lock,
            Err(_) => return Vec::new(),
        };
        let _operation_guard = match operation_lock.lock() {
            Ok(guard) => guard,
            Err(_) => return Vec::new(),
        };
        let Ok(workers) = self.workers.lock() else {
            return Vec::new();
        };
        workers
            .get(worker_ref)
            .map(|execution| {
                futures::executor::block_on(
                    execution.handle.completion_entries(kind, prefix, context),
                )
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::pin::Pin;
    use std::process::Command;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use crate::Runtime as EmbeddedRuntime;
    use crate::catalog::{
        ConfigBundleRef, CreateWorkerRequest, LogicalWorkdirAttachment, MaterializerKind,
        ProfileSelector, RepositorySelector, WorkingDirectoryAttachmentClaim,
        WorkingDirectoryAttachmentRequest, WorkingDirectoryRepository, WorkingDirectoryRequest,
        WorkspaceApiRef,
    };
    use crate::execution::WorkerExecutionContext;
    use crate::identity::WorkerId;
    use crate::identity::WorkerRef;
    use crate::management::RuntimeOptions;
    #[cfg(feature = "ws-server")]
    use crate::observation::WorkerObservationCursor;
    use crate::working_directory::RuntimeGitMaterializer;
    use agen::Engine;
    use agen::llm_client::event::{Event as LlmEvent, ResponseStatus, StatusEvent};
    use agen::llm_client::{ClientError, LlmClient, Request, RequestConfig};
    use async_trait::async_trait;
    use futures::{Stream, StreamExt};
    use manifest::{Scope, WorkerManifest};
    use session_store::{
        LogEntry, LoggedContentPart, LoggedHistoryEntry, LoggedItem, LoggedRole,
        LoggedSessionHistoryEntryId, LoggedSessionHistoryMetadata, LoggedSessionHistoryOrigin,
        Store, WorkerActiveSegmentRef, WorkerMetadata, WorkerMetadataStore,
    };

    #[test]
    fn production_source_has_no_repository_derived_runtime_store() {
        let production = include_str!("worker_backend.rs")
            .split_once("#[cfg(test)]\nmod tests")
            .map(|(production, _)| production)
            .expect("worker backend test module marker");
        for forbidden in ["from_workspace", ".yoi/runtime-store"] {
            assert!(
                !production.contains(forbidden),
                "repository-derived Runtime store returned through {forbidden}"
            );
        }
    }

    fn persisted_files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        fn collect(root: &Path, current: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
            if !current.exists() {
                return;
            }
            for entry in fs::read_dir(current).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    collect(root, &path, files);
                } else {
                    files.insert(
                        path.strip_prefix(root).unwrap().to_path_buf(),
                        fs::read(path).unwrap(),
                    );
                }
            }
        }

        let mut files = BTreeMap::new();
        collect(root, root, &mut files);
        files
    }

    fn write_retained_profile_worker(
        runtime_store_dir: &Path,
        worker_ref: &WorkerRef,
    ) -> (session_store::SessionId, session_store::SegmentId, String) {
        let worker_name = ProfileRuntimeWorkerFactory::runtime_worker_name_for_ref(worker_ref);
        let aggregate_dir = runtime_store_dir
            .join("workers")
            .join(worker_ref.worker_id.to_string());
        let aggregate = WorkerAggregateStore::new(&aggregate_dir, &worker_name).unwrap();
        let session_id = session_store::new_session_id();
        let segment_id = session_store::new_segment_id();
        aggregate
            .write(&WorkerMetadata::new(
                &worker_name,
                Some(WorkerActiveSegmentRef::active_segment(
                    session_id, segment_id,
                )),
            ))
            .unwrap();
        let entry_id = LoggedSessionHistoryEntryId::new();
        let expected_entry_id = entry_id.0.clone();
        let log = [LogEntry::AnnotatedSegmentStart {
            ts: 17,
            session_id,
            system_prompt: None,
            config: RequestConfig::default(),
            history: vec![LoggedHistoryEntry {
                item: LoggedItem::Message {
                    role: LoggedRole::Assistant,
                    content: vec![LoggedContentPart::Text {
                        text: "retained response".to_string(),
                    }],
                },
                metadata: LoggedSessionHistoryMetadata {
                    entry_id,
                    origin: LoggedSessionHistoryOrigin::LegacyUnknown,
                    derivation: None,
                },
            }],
            forked_from: None,
            compacted_from: None,
        }];
        WorkerSessionStore::new(aggregate_dir.join("session"))
            .unwrap()
            .create_segment(session_id, segment_id, &log)
            .unwrap();
        (session_id, segment_id, expected_entry_id)
    }

    const SAVED_SUBJEKTIV_SYSTEM_PROMPT: &str = "Committed saved Subject Worker system prompt";

    async fn saved_subjektiv_restore_fixture(
        root: &Path,
        materialize_head: bool,
    ) -> WorkerExecutionRestoreRequest {
        let worker_ref = WorkerRef::new(WorkerId::now_v7());
        let worker_name = ProfileRuntimeWorkerFactory::runtime_worker_name_for_ref(&worker_ref);
        let aggregate_dir = root.join("workers").join(worker_ref.worker_id.to_string());
        let session_store = WorkerSessionStore::new(aggregate_dir.join("session")).unwrap();
        let store = WorkerAggregateStore::new(&aggregate_dir, &worker_name).unwrap();
        let mut saved = WorkerManifest::from_toml(&format!(
            r#"
            [worker]
            name = "{worker_name}"
            [model]
            scheme = "anthropic"
            model_id = "saved-test-model"
            auth = {{ kind = "none" }}
            [engine]
            instruction = "default"
            max_tokens = 100
            [[scope.allow]]
            target = "{}"
            permission = "read"
            recursive = true
            symlink_policy = "resolved"
            [feature.memory.profile]
            enabled = false
            [feature.subjektiv.profile]
            enabled = true
            [feature.subjektiv.profile.extraction]
            enabled = false
        "#,
            root.display()
        ))
        .unwrap();
        Scope::from_config(&saved.scope)
            .expect("saved fixture must declare a valid resolved scope");
        let mut request = create_request("saved subject policy");
        request.worker_id = worker_ref.worker_id;
        request.workspace_api = Some(WorkspaceApiRef {
            workspace_id: "workspace-saved-subjektiv".to_string(),
            base_url: "http://workspace.invalid".to_string(),
        });
        request.subjektiv_attached = true;
        request.memory_settings = Some(manifest::WorkspaceMemorySettingsSnapshot {
            workspace_id: "workspace-saved-subjektiv".to_string(),
            settings_revision: 17,
            language: "English".to_string(),
        });
        bind_workspace_memory_settings(&mut saved, &request).unwrap();
        let client = MockClient::new(Vec::new());
        let mut engine =
            Engine::<_, agen::state::Mutable, worker::SessionHistoryMetadata>::new_annotated(
                client.clone(),
            );
        engine.set_system_prompt(SAVED_SUBJEKTIV_SYSTEM_PROMPT);
        let mut worker = Worker::new(
            saved.clone(),
            engine,
            CombinedStore::new(session_store.clone(), store.clone()),
            WorkerWorkspaceContext::unavailable(
                Some(WorkspaceId::new("workspace-saved-subjektiv").unwrap()),
                "saved session fixture has no network client",
            ),
            WorkerFilesystemAuthority::None,
            Scope::empty(),
        )
        .await
        .unwrap();
        worker.enable_worker_metadata_write_through().unwrap();
        let session_id = worker.session_id();
        let segment_id = worker.segment_id();
        if materialize_head {
            // Use the Worker's normal typed startup writer, not the retained
            // public-history fixture (which intentionally has no system prompt).
            worker.materialize_durable_session_head().await.unwrap();
            let entries = session_store.read_all(session_id, segment_id).unwrap();
            assert!(
                matches!(entries.as_slice(), [LogEntry::AnnotatedSegmentStart {
                session_id: committed_session_id,
                system_prompt: Some(prompt),
                history,
                forked_from: None,
                compacted_from: None,
                ..
            }] if *committed_session_id == session_id
                && prompt == SAVED_SUBJEKTIV_SYSTEM_PROMPT
                && history.is_empty())
            );
            assert_eq!(
                session_store::collect_state(&entries)
                    .system_prompt
                    .as_deref(),
                Some(SAVED_SUBJEKTIV_SYSTEM_PROMPT)
            );
        } else {
            assert!(matches!(
                session_store.exists(session_id, segment_id),
                Err(session_store::StoreError::Corrupt { message, .. })
                    if message.contains("no materialized Session")
            ));
        }
        assert_eq!(client.call_count.load(Ordering::SeqCst), 0);
        drop(worker);
        let mut metadata = store.read_by_name(&worker_name).unwrap().unwrap();
        let (manifest, _) = ProfileRuntimeWorkerFactory::manifest_for_restore(&metadata, &request)
            .expect("fixture must persist valid saved Manifest authority");
        assert_eq!(
            manifest::write_persisted_worker_manifest_snapshot(&manifest).unwrap(),
            manifest::write_persisted_worker_manifest_snapshot(&saved).unwrap()
        );
        assert_eq!(
            metadata.active.as_ref().unwrap().session_id,
            session_id,
            "metadata write-through must preserve the real Worker Session"
        );
        assert_eq!(
            metadata.active.as_ref().unwrap().segment_id,
            materialize_head.then_some(segment_id)
        );
        assert_eq!(
            metadata.workspace_id.as_deref(),
            Some("workspace-saved-subjektiv")
        );
        // Attribution already committed before restart; the real restore needs
        // no network fixture or replacement current Profile.
        metadata.subjektiv_session_attribution =
            Some(session_store::SubjektivSessionAttributionState::Confirmed {
                session_id,
                subject_id: "saved-subject".to_string(),
            });
        store.write(&metadata).unwrap();
        WorkerExecutionRestoreRequest {
            operation_id: crate::execution::WorkerLifecycleOperationId::new(),
            worker_ref: worker_ref.clone(),
            request,
            workspace_scope: Some(crate::runtime::RuntimeWorkspaceScope::new(
                "workspace-saved-subjektiv",
                "server-main",
            )),
            context: test_execution_context(worker_ref),
            previous_workdir_attachments: Vec::new(),
            logical_workdir_attachments: Vec::new(),
            workdir_attachments: BTreeMap::new(),
            config_bundle: None,
        }
    }

    async fn shutdown_profile_controller(controller: RuntimeWorkerController) {
        let state = Arc::new(RwLock::new(controller.handle.shared_state.snapshot()));
        controller
            .handle
            .send(Method::Shutdown {
                command: next_internal_command(&state).unwrap(),
            })
            .await
            .unwrap();
        if let Some(receiver) = controller.shutdown.lock().await.take() {
            tokio::time::timeout(Duration::from_secs(5), receiver)
                .await
                .unwrap()
                .unwrap();
        }
        tokio::time::timeout(Duration::from_secs(5), controller.controller_task)
            .await
            .unwrap()
            .unwrap();
    }

    #[test]
    #[serial_test::serial(worker_allocation)]
    fn profile_backend_existing_restore_attestation_is_read_only_under_concurrency() {
        let root = tempfile::tempdir().unwrap();
        let runtime_store = root.path().join("runtime");
        let backend = Arc::new(
            WorkerRuntimeExecutionBackend::new(
                ProfileRuntimeWorkerFactory::new(root.path())
                    .with_runtime_store_dir(&runtime_store)
                    .with_runtime_id("runtime-subjektiv-test"),
            )
            .unwrap(),
        );
        let fixture_root = runtime_store.clone();
        let request = backend
            .run_on_adapter_runtime(async move {
                Ok(saved_subjektiv_restore_fixture(&fixture_root, true).await)
            })
            .unwrap();
        backend.preflight_restore(&request).unwrap();
        let expected = match backend.restore_worker(request.clone()) {
            WorkerExecutionSpawnResult::Connected { worker_state, .. } => worker_state,
            other => panic!("real Profile backend restore failed: {other:?}"),
        };
        assert!(matches!(
            backend.worker_session(WorkerSessionObservationRequest {
                worker_ref: request.worker_ref.clone(),
            }),
            runtime_api::WorkerSessionAvailability::LiveProtocol
        ));
        let current = backend
            .workers
            .lock()
            .unwrap()
            .get(&request.worker_ref)
            .unwrap()
            .clone();
        let authority_files = || {
            let mut files = persisted_files(&runtime_store);
            files.retain(|path, _| {
                !path
                    .components()
                    .any(|component| component.as_os_str() == "runs")
            });
            files
        };
        let files_before = authority_files();
        let runs = backend
            .factory
            .worker_aggregate_dir(&request.worker_ref)
            .unwrap()
            .join("runs");
        let run_directories = || {
            fs::read_dir(&runs)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect::<BTreeSet<_>>()
        };
        let runs_before = run_directories();
        std::thread::scope(|threads| {
            for _ in 0..8 {
                let backend = Arc::clone(&backend);
                let request = request.clone();
                let expected = expected.clone();
                threads.spawn(move || {
                    for _ in 0..2 {
                        let connected = backend.preflight_restore(&request).unwrap_err();
                        assert_eq!(
                            connected.outcome,
                            crate::execution::WorkerExecutionOutcome::Busy
                        );
                        assert_eq!(connected.worker_state, Some(expected.clone()));
                        match backend.reconcile_restore(request.clone()) {
                            WorkerExecutionSpawnResult::Connected { worker_state, .. } => {
                                assert_eq!(worker_state, expected)
                            }
                            other => {
                                panic!("repeated exact operation was not connected: {other:?}")
                            }
                        }
                    }
                });
            }
        });
        let mut repeated_request = request.clone();
        repeated_request.operation_id = crate::execution::WorkerLifecycleOperationId::new();
        let connected = backend.preflight_restore(&repeated_request).unwrap_err();
        assert_eq!(
            connected.outcome,
            crate::execution::WorkerExecutionOutcome::Busy
        );
        assert_eq!(connected.worker_state, Some(expected));
        let workers = backend.workers.lock().unwrap();
        let unchanged = workers.get(&request.worker_ref).unwrap();
        assert!(Arc::ptr_eq(
            &unchanged.handle.shared_state,
            &current.handle.shared_state
        ));
        assert!(Arc::ptr_eq(&unchanged.worker_state, &current.worker_state));
        assert_eq!(unchanged.restore_operation_id, Some(request.operation_id));
        assert_eq!(workers.len(), 1);
        drop(workers);
        assert_eq!(authority_files(), files_before);
        assert_eq!(run_directories(), runs_before);
        assert!(
            backend
                .pending_restore(&request.worker_ref)
                .unwrap()
                .is_none()
        );
        current.shutdown_requested.store(true, Ordering::Release);
        let pending_cleanup = backend.preflight_restore(&request).unwrap_err();
        assert_eq!(
            pending_cleanup.outcome,
            crate::execution::WorkerExecutionOutcome::Busy
        );
        assert!(
            pending_cleanup.worker_state.is_none(),
            "pending cleanup is not AlreadyConnected evidence"
        );
        current.shutdown_requested.store(false, Ordering::Release);
        assert!(backend.stop_worker(&request.worker_ref).is_accepted());
        assert!(matches!(
            backend.worker_session(WorkerSessionObservationRequest {
                worker_ref: request.worker_ref,
            }),
            runtime_api::WorkerSessionAvailability::RetainedSnapshot { .. }
        ));
    }

    #[cfg(feature = "ws-server")]
    #[test]
    #[serial_test::serial(worker_allocation)]
    fn protocol_transport_stays_bound_to_stopped_execution_after_restore() {
        let root = tempfile::tempdir().unwrap();
        let runtime_store = root.path().join("runtime");
        let backend = Arc::new(
            WorkerRuntimeExecutionBackend::new(
                ProfileRuntimeWorkerFactory::new(root.path())
                    .with_runtime_store_dir(&runtime_store)
                    .with_runtime_id("runtime-subjektiv-test"),
            )
            .unwrap(),
        );
        let fixture_root = runtime_store.clone();
        let mut request = backend
            .run_on_adapter_runtime(async move {
                Ok(saved_subjektiv_restore_fixture(&fixture_root, true).await)
            })
            .unwrap();
        assert!(matches!(
            backend.restore_worker(request.clone()),
            WorkerExecutionSpawnResult::Connected { .. }
        ));
        let mut transport_a = Arc::clone(&backend)
            .attach_worker_protocol(&request.worker_ref)
            .unwrap();
        assert!(matches!(&transport_a.snapshot, Event::Snapshot { .. }));
        transport_a.validate().unwrap();
        let completion = |id: &str| Method::ListCompletions {
            request_id: Some(id.to_string()),
            kind: protocol::CompletionKind::File,
            prefix: String::new(),
            context: None,
        };
        assert!(
            matches!(transport_a.dispatch(completion("a-completion")).unwrap().as_slice(),
            [Event::Completions { request_id: Some(id), .. }] if id == "a-completion")
        );
        assert!(
            transport_a
                .dispatch(Method::Shutdown {
                    command: WorkerCommandEnvelope::new(1),
                })
                .unwrap()
                .is_empty()
        );
        assert!(
            transport_a.events.is_closed(),
            "stop must join the connection relay before success"
        );
        while transport_a.events.try_recv().is_ok() {}
        assert!(matches!(
            transport_a.events.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected)
        ));
        request.operation_id = crate::execution::WorkerLifecycleOperationId::new();
        assert!(matches!(
            backend.restore_worker(request.clone()),
            WorkerExecutionSpawnResult::Connected { .. }
        ));
        let current_b = backend
            .workers
            .lock()
            .unwrap()
            .get(&request.worker_ref)
            .unwrap()
            .clone();
        let snapshot_b = current_b.handle.shared_state.snapshot();
        assert!(transport_a.validate().is_err());
        for delayed in [
            Method::Cancel {
                command: WorkerCommandEnvelope::new(900),
            },
            completion("delayed-a-completion"),
            Method::Shutdown {
                command: WorkerCommandEnvelope::new(901),
            },
        ] {
            let rejected = transport_a.dispatch(delayed).unwrap_err();
            assert_eq!(
                rejected.outcome,
                crate::execution::WorkerExecutionOutcome::Rejected
            );
            assert!(rejected.worker_state.is_none());
        }
        assert_eq!(current_b.handle.shared_state.snapshot(), snapshot_b);
        assert_eq!(
            backend
                .workers
                .lock()
                .unwrap()
                .get(&request.worker_ref)
                .unwrap()
                .restore_operation_id,
            Some(request.operation_id)
        );
        let transport_b = Arc::clone(&backend)
            .attach_worker_protocol(&request.worker_ref)
            .unwrap();
        transport_b.validate().unwrap();
        assert!(
            matches!(transport_b.dispatch(completion("b-completion")).unwrap().as_slice(),
            [Event::Completions { request_id: Some(id), .. }] if id == "b-completion")
        );
        let command_id = snapshot_b.last_command_id.saturating_add(1);
        transport_b
            .dispatch(Method::Cancel {
                command: WorkerCommandEnvelope::new(command_id),
            })
            .unwrap();
        let mut b_events = transport_b.events;
        backend.run_on_adapter_runtime(async move {
            tokio::time::timeout(Duration::from_secs(5), async {
                while let Some(event) = b_events.recv().await {
                    if matches!(event, Event::CommandAcknowledged { acknowledgement } if acknowledgement.command_id == command_id) {
                        return Ok(());
                    }
                }
                Err("fresh B protocol stream closed before command acknowledgement".to_string())
            }).await.map_err(|_| "fresh B protocol command acknowledgement timed out".to_string())?
        }).unwrap();
        assert!(
            matches!(
                transport_a.events.try_recv(),
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected)
            ),
            "A must receive no restored B protocol events"
        );
        assert!(backend.stop_worker(&request.worker_ref).is_accepted());
    }

    fn profile_stop_fixture(
        root: &Path,
    ) -> (WorkerRuntimeExecutionBackend, WorkerExecutionRestoreRequest) {
        let runtime_store = root.join("runtime");
        let backend = WorkerRuntimeExecutionBackend::new(
            ProfileRuntimeWorkerFactory::new(root)
                .with_runtime_store_dir(&runtime_store)
                .with_runtime_id("runtime-subjektiv-test"),
        )
        .unwrap();
        let request = backend
            .run_on_adapter_runtime(async move {
                Ok(saved_subjektiv_restore_fixture(&runtime_store, true).await)
            })
            .unwrap();
        assert!(matches!(
            backend.restore_worker(request.clone()),
            WorkerExecutionSpawnResult::Connected { .. }
        ));
        (backend, request)
    }

    // Queue a real Cancel first so the exact generated Shutdown ID is either
    // already used by another kind (Conflict) or below the Controller floor.
    // Install the same owned evidence as request_shutdown before enqueueing.
    async fn prepare_rejected_shutdown(
        execution: &RuntimeWorkerExecution,
        stale: bool,
        release: Option<tokio::sync::oneshot::Receiver<()>>,
    ) -> u64 {
        let command = next_internal_command_for_snapshot(&execution.handle.shared_state.snapshot());
        let cancel_id = command.command_id + u64::from(stale);
        let mut cancel_events = execution.handle.subscribe();
        execution
            .handle
            .send(Method::Cancel {
                command: WorkerCommandEnvelope::new(cancel_id),
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if matches!(cancel_events.recv().await.unwrap(), Event::CommandAcknowledged { acknowledgement }
                    if acknowledgement.command_id == cancel_id
                        && acknowledgement.command == protocol::WorkerCommandKind::Cancel) {
                    break;
                }
            }
        }).await.unwrap();
        let events = execution.handle.subscribe();
        let handle = execution.handle.clone();
        *execution.tasks.shutdown_admission.lock().await = Some(RuntimeShutdownAdmission {
            command_id: command.command_id,
            send: Some(tokio::spawn(async move {
                if let Some(release) = release {
                    release.await.map_err(|error| error.to_string())?;
                }
                handle
                    .send(Method::Shutdown { command })
                    .await
                    .map_err(|error| error.to_string())
            })),
            events,
            acknowledgement: None,
            uncertainty: None,
        });
        execution.shutdown_requested.store(true, Ordering::Release);
        command.command_id
    }

    #[test]
    #[serial_test::serial(worker_allocation)]
    fn stop_rejected_shutdown_resets_request_and_retries_from_actual_snapshot() {
        for stale in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let (backend, request) = profile_stop_fixture(root.path());
            let execution = backend
                .workers
                .lock()
                .unwrap()
                .get(&request.worker_ref)
                .unwrap()
                .clone();
            let preparing = execution.clone();
            let rejected_id = backend
                .run_on_adapter_runtime(async move {
                    Ok(prepare_rejected_shutdown(&preparing, stale, None).await)
                })
                .unwrap();
            // Explicitly stale bridge cache must not determine the retry ID.
            execution.worker_state.write().unwrap().last_command_id = 0;
            let result = backend.stop_worker(&request.worker_ref);
            assert!(!result.is_accepted());
            let message = result.message.unwrap();
            assert!(
                message.contains(if stale { "StaleCommandId" } else { "Conflict" }),
                "{message}"
            );
            assert!(!execution.shutdown_requested.load(Ordering::Acquire));
            assert!(
                backend
                    .workers
                    .lock()
                    .unwrap()
                    .contains_key(&request.worker_ref)
            );
            assert!(backend.stop_worker(&request.worker_ref).is_accepted());
            assert!(execution.handle.shared_state.snapshot().last_command_id > rejected_id);
            assert!(backend.workers.lock().unwrap().is_empty());
            assert!(execution.tasks.tasks.lock().unwrap().is_empty());
        }
    }

    #[test]
    #[serial_test::serial(worker_allocation)]
    fn stop_late_shutdown_rejection_after_timeout_retains_waiter_without_double_enqueue() {
        let root = tempfile::tempdir().unwrap();
        let (backend, request) = profile_stop_fixture(root.path());
        let execution = backend
            .workers
            .lock()
            .unwrap()
            .get(&request.worker_ref)
            .unwrap()
            .clone();
        let checking = execution.clone();
        backend
            .run_on_adapter_runtime(async move {
                let (release, gate) = tokio::sync::oneshot::channel();
                let command_id = prepare_rejected_shutdown(&checking, false, Some(gate)).await;
                for _ in 0..2 {
                    let error = checking
                        .tasks
                        .request_shutdown(
                            &checking.handle,
                            &checking.shutdown_requested,
                            Duration::from_millis(10),
                        )
                        .await
                        .unwrap_err();
                    assert!(error.contains("timed out"), "{error}");
                    assert!(checking.shutdown_requested.load(Ordering::Acquire));
                    let retained = checking.tasks.shutdown_admission.lock().await;
                    let pending = retained.as_ref().unwrap();
                    assert_eq!(pending.command_id, command_id);
                    assert!(
                        pending
                            .send
                            .as_ref()
                            .is_some_and(|task| !task.is_finished())
                    );
                    assert!(pending.acknowledgement.is_none());
                }
                // A mismatched ID or command kind is not this Shutdown's verdict.
                for (id, kind) in [
                    (command_id + 1, protocol::WorkerCommandKind::Shutdown),
                    (command_id, protocol::WorkerCommandKind::Cancel),
                ] {
                    checking
                        .handle
                        .send_event(Event::CommandAcknowledged {
                            acknowledgement: protocol::WorkerCommandAcknowledgement {
                                command_id: id,
                                command: kind,
                                disposition: protocol::WorkerCommandDisposition::Accepted,
                                state: checking.handle.shared_state.snapshot(),
                            },
                        })
                        .unwrap();
                }
                release.send(()).unwrap();
                let error = checking
                    .tasks
                    .request_shutdown(
                        &checking.handle,
                        &checking.shutdown_requested,
                        Duration::from_secs(5),
                    )
                    .await
                    .unwrap_err();
                assert!(error.contains("Conflict"), "{error}");
                assert!(!checking.shutdown_requested.load(Ordering::Acquire));
                assert!(checking.tasks.shutdown_admission.lock().await.is_none());
                Ok(())
            })
            .unwrap();
        assert!(backend.stop_worker(&request.worker_ref).is_accepted());
        assert!(execution.tasks.tasks.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn shutdown_ack_timeout_then_late_rejection_keeps_exact_receiver() {
        let (events, receiver) = tokio::sync::broadcast::channel(4);
        let mut pending = RuntimeShutdownAdmission {
            command_id: 37,
            send: None,
            events: receiver,
            acknowledgement: None,
            uncertainty: None,
        };
        assert!(
            pending
                .wait(Duration::from_millis(10))
                .await
                .unwrap_err()
                .contains("timed out")
        );
        assert!(pending.uncertainty.as_ref().unwrap().contains("timed out"));
        let acknowledgement = protocol::WorkerCommandAcknowledgement {
            command_id: 37,
            command: protocol::WorkerCommandKind::Shutdown,
            disposition: protocol::WorkerCommandDisposition::StaleCommandId,
            state: protocol::WorkerStateSnapshot::initial(),
        };
        events
            .send(Event::CommandAcknowledged {
                acknowledgement: acknowledgement.clone(),
            })
            .unwrap();
        pending.wait(Duration::from_secs(1)).await.unwrap();
        assert_eq!(pending.acknowledgement, Some(acknowledgement));
    }

    #[tokio::test]
    async fn shutdown_ack_lag_and_closed_retain_correlated_unknown_evidence() {
        let (events, receiver) = tokio::sync::broadcast::channel(1);
        let mut pending = RuntimeShutdownAdmission {
            command_id: 38,
            send: None,
            events: receiver,
            acknowledgement: None,
            uncertainty: None,
        };
        for _ in 0..2 {
            events.send(Event::Shutdown).unwrap();
        }
        assert!(pending.wait(Duration::from_secs(1)).await.is_err());
        assert!(pending.uncertainty.is_some());
        assert!(pending.acknowledgement.is_none());
        drop(events);
        assert!(pending.wait(Duration::from_secs(1)).await.is_err());
        assert_eq!(pending.command_id, 38);
        assert!(pending.uncertainty.is_some());
        assert!(pending.acknowledgement.is_none());
    }

    #[test]
    #[serial_test::serial(worker_allocation)]
    fn unconnected_cleanup_rejected_shutdown_retains_scope_for_retry() {
        let root = tempfile::tempdir().unwrap();
        let (backend, request) = profile_stop_fixture(root.path());
        let execution = backend
            .workers
            .lock()
            .unwrap()
            .get(&request.worker_ref)
            .unwrap()
            .clone();
        let preparing = execution.clone();
        backend
            .run_on_adapter_runtime(async move {
                prepare_rejected_shutdown(&preparing, true, None).await;
                Ok(())
            })
            .unwrap();
        let cleanup = || {
            backend.cleanup_unconnected_controller(
                &execution.handle,
                &execution.shutdown,
                &execution.tasks,
                &execution.shutdown_requested,
                &execution.worker_state,
            )
        };
        assert!(cleanup().unwrap_err().contains("StaleCommandId"));
        assert!(!execution.shutdown_requested.load(Ordering::Acquire));
        assert!(!execution.tasks.tasks.lock().unwrap().is_empty());
        assert!(cleanup().is_ok());
        assert!(execution.tasks.tasks.lock().unwrap().is_empty());
        assert!(backend.stop_worker(&request.worker_ref).is_accepted());
    }

    #[test]
    #[serial_test::serial(worker_allocation)]
    fn duplicate_connect_rejected_shutdown_retains_actual_candidate_until_stop_retry() {
        for operation in [
            WorkerExecutionOperation::Spawn,
            WorkerExecutionOperation::Restore,
        ] {
            let root = tempfile::tempdir().unwrap();
            let candidate_root = tempfile::tempdir().unwrap();
            let (backend, request) = profile_stop_fixture(root.path());
            let primary = backend
                .workers
                .lock()
                .unwrap()
                .get(&request.worker_ref)
                .unwrap()
                .clone();
            let (candidate_backend, candidate_request) =
                profile_stop_fixture(candidate_root.path());
            let candidate = candidate_backend
                .workers
                .lock()
                .unwrap()
                .remove(&candidate_request.worker_ref)
                .unwrap();
            assert!(!primary.handle.same_controller(&candidate.handle));
            let preparing = candidate.clone();
            candidate_backend
                .run_on_adapter_runtime(async move {
                    prepare_rejected_shutdown(&preparing, true, None).await;
                    Ok(())
                })
                .unwrap();
            let result = {
                let lock = backend.worker_lock(&request.worker_ref).unwrap();
                let _guard = lock.lock().unwrap();
                backend.connect_controller_scope(
                    operation,
                    request.worker_ref.clone(),
                    request.context.clone(),
                    candidate.handle.clone(),
                    candidate.shutdown.clone(),
                    candidate.tasks.clone(),
                    candidate.shutdown_requested.clone(),
                    Some(request.operation_id),
                    BTreeMap::new(),
                    candidate.workspace_client.clone(),
                )
            };
            match result {
                WorkerExecutionSpawnResult::ReconciliationRequired {
                    result,
                    worker_state,
                    ..
                } => {
                    assert!(result.message.unwrap().contains("StaleCommandId"));
                    assert!(
                        worker_state.is_none(),
                        "existing snapshot is not candidate cleanup proof"
                    );
                }
                other => panic!(
                    "duplicate cleanup failure was classified as side-effect-free: {other:?}"
                ),
            }
            let retained = primary.tasks.cleanup_candidates.lock().unwrap()[0].clone();
            assert!(retained.handle.same_controller(&candidate.handle));
            assert!(Arc::ptr_eq(&retained.shutdown, &candidate.shutdown));
            assert!(Arc::ptr_eq(&retained.tasks.tasks, &candidate.tasks.tasks));
            assert!(Arc::ptr_eq(
                &retained.tasks.shutdown_admission,
                &candidate.tasks.shutdown_admission
            ));
            let registered = backend
                .workers
                .lock()
                .unwrap()
                .get(&request.worker_ref)
                .unwrap()
                .clone();
            assert!(registered.handle.same_controller(&primary.handle));
            assert_eq!(backend.workers.lock().unwrap().len(), 1);
            assert!(
                backend
                    .preflight_restore(&request)
                    .unwrap_err()
                    .worker_state
                    .is_none()
            );
            assert!(matches!(
                backend.restore_worker(request.clone()),
                WorkerExecutionSpawnResult::ReconciliationRequired { .. }
            ));

            // Fail candidate cleanup once more during Stop. The main Controller
            // must still join, but that cannot turn the failed child into Stopped.
            let preparing = candidate.clone();
            candidate_backend
                .run_on_adapter_runtime(async move {
                    prepare_rejected_shutdown(&preparing, false, None).await;
                    Ok(())
                })
                .unwrap();
            let stopped = backend.stop_worker(&request.worker_ref);
            assert!(!stopped.is_accepted());
            assert!(stopped.message.unwrap().contains("Conflict"));
            assert!(primary.handle.protocol_is_closed());
            assert!(primary.tasks.tasks.lock().unwrap().is_empty());
            let checking_scope = primary.tasks.clone();
            let incomplete_scope =
                backend.run_on_adapter_runtime(async move { checking_scope.join().await });
            assert!(
                incomplete_scope
                    .unwrap_err()
                    .contains("unconnected Controller cleanup remains unproven")
            );
            assert!(
                backend
                    .workers
                    .lock()
                    .unwrap()
                    .contains_key(&request.worker_ref)
            );
            assert_eq!(primary.tasks.cleanup_candidates.lock().unwrap().len(), 1);
            assert!(!candidate.tasks.tasks.lock().unwrap().is_empty());
            assert!(backend.stop_worker(&request.worker_ref).is_accepted());
            assert!(candidate.tasks.tasks.lock().unwrap().is_empty());
            assert!(primary.tasks.cleanup_candidates.lock().unwrap().is_empty());
            assert!(backend.workers.lock().unwrap().is_empty());
        }
    }

    #[test]
    #[serial_test::serial(worker_allocation)]
    fn duplicate_connect_timeout_retains_candidate_waiter_and_no_false_stop() {
        let root = tempfile::tempdir().unwrap();
        let candidate_root = tempfile::tempdir().unwrap();
        let (backend, request) = profile_stop_fixture(root.path());
        let primary = backend
            .workers
            .lock()
            .unwrap()
            .get(&request.worker_ref)
            .unwrap()
            .clone();
        let (candidate_backend, candidate_request) = profile_stop_fixture(candidate_root.path());
        let candidate = candidate_backend
            .workers
            .lock()
            .unwrap()
            .remove(&candidate_request.worker_ref)
            .unwrap();
        let (release, gate) = tokio::sync::oneshot::channel();
        let preparing = candidate.clone();
        let command_id = candidate_backend
            .run_on_adapter_runtime(async move {
                Ok(prepare_rejected_shutdown(&preparing, false, Some(gate)).await)
            })
            .unwrap();
        let result = {
            let lock = backend.worker_lock(&request.worker_ref).unwrap();
            let _guard = lock.lock().unwrap();
            backend.connect_controller_scope(
                WorkerExecutionOperation::Restore,
                request.worker_ref.clone(),
                request.context.clone(),
                candidate.handle.clone(),
                candidate.shutdown.clone(),
                candidate.tasks.clone(),
                candidate.shutdown_requested.clone(),
                Some(request.operation_id),
                BTreeMap::new(),
                candidate.workspace_client.clone(),
            )
        };
        assert!(matches!(
            result,
            WorkerExecutionSpawnResult::ReconciliationRequired { .. }
        ));
        let stopped = backend.stop_worker(&request.worker_ref);
        assert!(!stopped.is_accepted());
        assert!(stopped.message.unwrap().contains("timed out"));
        assert!(primary.handle.protocol_is_closed());
        let retained = primary.tasks.cleanup_candidates.lock().unwrap()[0].clone();
        assert!(retained.handle.same_controller(&candidate.handle));
        assert!(Arc::ptr_eq(
            &retained.tasks.shutdown_admission,
            &candidate.tasks.shutdown_admission
        ));
        let checking = retained.clone();
        backend
            .run_on_adapter_runtime(async move {
                let admission = checking.tasks.shutdown_admission.lock().await;
                let pending = admission.as_ref().unwrap();
                assert_eq!(
                    pending.command_id, command_id,
                    "unknown admission must not enqueue another command"
                );
                assert!(
                    pending
                        .send
                        .as_ref()
                        .is_some_and(|task| !task.is_finished())
                );
                assert!(pending.uncertainty.as_ref().unwrap().contains("timed out"));
                assert!(pending.acknowledgement.is_none());
                Ok(())
            })
            .unwrap();
        assert!(
            backend
                .workers
                .lock()
                .unwrap()
                .contains_key(&request.worker_ref)
        );
        release.send(()).unwrap();
        let rejected = backend.stop_worker(&request.worker_ref);
        assert!(!rejected.is_accepted());
        assert!(rejected.message.unwrap().contains("Conflict"));
        assert!(!candidate.shutdown_requested.load(Ordering::Acquire));
        assert_eq!(primary.tasks.cleanup_candidates.lock().unwrap().len(), 1);
        assert!(backend.stop_worker(&request.worker_ref).is_accepted());
        assert!(candidate.tasks.tasks.lock().unwrap().is_empty());
        assert!(primary.tasks.tasks.lock().unwrap().is_empty());
        assert!(primary.tasks.cleanup_candidates.lock().unwrap().is_empty());
        assert!(backend.workers.lock().unwrap().is_empty());
    }

    #[test]
    #[serial_test::serial(worker_allocation)]
    fn pending_restore_join_failure_never_becomes_absence_on_stop_retry() {
        let root = tempfile::tempdir().unwrap();
        let runtime_store = root.path().join("runtime");
        let backend = WorkerRuntimeExecutionBackend::new(
            ProfileRuntimeWorkerFactory::new(root.path())
                .with_runtime_store_dir(&runtime_store)
                .with_runtime_id("runtime-subjektiv-test"),
        )
        .unwrap();
        let request = backend
            .run_on_adapter_runtime(async move {
                Ok(saved_subjektiv_restore_fixture(&runtime_store, true).await)
            })
            .unwrap();
        let failed = backend
            .spawn_on_adapter_runtime(async {
                panic!("factory scope failed before publishing result")
            })
            .unwrap();
        let pending = Arc::new(PendingRuntimeRestore {
            request: request.clone(),
            task: tokio::sync::Mutex::new(Some(failed)),
            result: Mutex::new(None),
            failure: Mutex::new(None),
        });
        backend
            .pending_restores
            .lock()
            .unwrap()
            .insert(request.worker_ref.clone(), pending.clone());
        for _ in 0..3 {
            let stopped = backend.stop_worker(&request.worker_ref);
            assert!(!stopped.is_accepted());
            assert!(
                stopped
                    .message
                    .unwrap()
                    .contains("restore factory task failed")
            );
            assert!(pending.failure.lock().unwrap().is_some());
            assert!(pending.result.lock().unwrap().is_none());
            assert!(Arc::ptr_eq(
                &backend
                    .pending_restore(&request.worker_ref)
                    .unwrap()
                    .unwrap(),
                &pending
            ));
            assert!(backend.workers.lock().unwrap().is_empty());
        }
        assert!(matches!(
            backend.restore_worker(request),
            WorkerExecutionSpawnResult::ReconciliationRequired { .. }
        ));
    }

    #[test]
    #[serial_test::serial(worker_allocation)]
    fn stop_joins_healthy_self_terminated_controller_with_closed_endpoint() {
        let root = tempfile::tempdir().unwrap();
        let (backend, request) = profile_stop_fixture(root.path());
        let execution = backend
            .workers
            .lock()
            .unwrap()
            .get(&request.worker_ref)
            .unwrap()
            .clone();
        let handle = execution.handle.clone();
        let controller = execution.tasks.tasks.lock().unwrap()[0].clone();
        backend
            .run_on_adapter_runtime(async move {
                handle
                    .send(Method::Shutdown {
                        command: WorkerCommandEnvelope::new(1),
                    })
                    .await
                    .map_err(|error| error.to_string())?;
                tokio::time::timeout(Duration::from_secs(5), async {
                    loop {
                        if handle.protocol_is_closed()
                            && controller
                                .completion
                                .lock()
                                .await
                                .task
                                .as_ref()
                                .unwrap()
                                .is_finished()
                        {
                            break;
                        }
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .map_err(|error| error.to_string())?;
                Ok(())
            })
            .unwrap();
        assert!(!execution.shutdown_requested.load(Ordering::Acquire));
        assert!(backend.stop_worker(&request.worker_ref).is_accepted());
        assert!(backend.workers.lock().unwrap().is_empty());
        assert!(execution.tasks.tasks.lock().unwrap().is_empty());
        assert!(backend.stop_worker(&request.worker_ref).is_accepted());
    }

    #[test]
    #[serial_test::serial(worker_allocation)]
    fn stop_and_unconnected_cleanup_retain_abnormal_controller_completion_across_retries() {
        let root = tempfile::tempdir().unwrap();
        let (backend, request) = profile_stop_fixture(root.path());
        let execution = backend
            .workers
            .lock()
            .unwrap()
            .get(&request.worker_ref)
            .unwrap()
            .clone();
        let controller = execution.tasks.tasks.lock().unwrap()[0].clone();
        let aborted_controller = Arc::clone(&controller);
        backend
            .run_on_adapter_runtime(async move {
                aborted_controller
                    .completion
                    .lock()
                    .await
                    .task
                    .as_ref()
                    .unwrap()
                    .abort();
                tokio::time::timeout(Duration::from_secs(5), async {
                    while !aborted_controller
                        .completion
                        .lock()
                        .await
                        .task
                        .as_ref()
                        .unwrap()
                        .is_finished()
                    {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .map_err(|error| error.to_string())?;
                Ok(())
            })
            .unwrap();
        for _ in 0..3 {
            let cleanup = backend
                .cleanup_unconnected_controller(
                    &execution.handle,
                    &execution.shutdown,
                    &execution.tasks,
                    &execution.shutdown_requested,
                    &execution.worker_state,
                )
                .unwrap_err();
            assert!(cleanup.contains("controller task failed"), "{cleanup}");
            let stopped = backend.stop_worker(&request.worker_ref);
            assert_eq!(
                stopped.outcome,
                crate::execution::WorkerExecutionOutcome::Errored
            );
            assert!(stopped.message.unwrap().contains("controller task failed"));
            let workers = backend.workers.lock().unwrap();
            let retained = workers
                .get(&request.worker_ref)
                .expect("unknown cleanup must retain owner");
            assert!(retained.handle.same_controller(&execution.handle));
            assert!(Arc::ptr_eq(
                &retained.tasks.tasks.lock().unwrap()[0],
                &controller
            ));
        }
        let retained = execution.tasks.tasks.lock().unwrap().clone();
        assert_eq!(
            retained.len(),
            2,
            "failed task and failed shutdown proof remain; relays drain"
        );
    }

    #[tokio::test]
    #[serial_test::serial(worker_allocation)]
    async fn profile_factory_saved_subjektiv_manifest_survives_restart_and_explicit_restore() {
        let root = tempfile::tempdir().unwrap();
        let runtime_store = root.path().join("runtime");
        let mut request = saved_subjektiv_restore_fixture(&runtime_store, true).await;
        let worker_name =
            ProfileRuntimeWorkerFactory::runtime_worker_name_for_ref(&request.worker_ref);
        let aggregate_dir = runtime_store
            .join("workers")
            .join(request.worker_ref.worker_id.to_string());
        let metadata_store = WorkerAggregateStore::new(&aggregate_dir, &worker_name).unwrap();
        let saved_metadata = metadata_store.read_by_name(&worker_name).unwrap().unwrap();
        let saved_active = saved_metadata.active.as_ref().unwrap();
        let session_store = WorkerSessionStore::new(aggregate_dir.join("session")).unwrap();
        for _ in 0..2 {
            // Fresh factory: exercise the same saved authority after both a
            // restart and another explicit restore, not a mock Controller path.
            let factory =
                ProfileRuntimeWorkerFactory::new(root.path().join("nonexistent-current-profiles"))
                    .with_runtime_store_dir(&runtime_store)
                    .with_runtime_id("runtime-subjektiv-test");
            let before = persisted_files(&runtime_store);
            factory.preflight_restore(&request).await.unwrap();
            assert_eq!(
                persisted_files(&runtime_store),
                before,
                "preflight must be read-only"
            );
            assert!(factory.observation_hub.workers.lock().unwrap().is_empty());
            let controller = factory
                .restore_controller(request.clone())
                .await
                .unwrap_or_else(|error| panic!("saved Subjektiv restore failed: {error}"));
            assert_eq!(
                controller.handle.shared_state.catalog_status(),
                WorkerStatus::Idle
            );
            shutdown_profile_controller(controller).await;
            let restored_metadata = metadata_store.read_by_name(&worker_name).unwrap().unwrap();
            assert_eq!(restored_metadata.active, saved_metadata.active);
            assert_eq!(
                restored_metadata.resolved_manifest_snapshot,
                saved_metadata.resolved_manifest_snapshot
            );
            assert_eq!(
                restored_metadata.subjektiv_session_attribution,
                saved_metadata.subjektiv_session_attribution
            );
            let entries = session_store
                .read_all(saved_active.session_id, saved_active.segment_id.unwrap())
                .unwrap();
            assert!(
                matches!(entries.first(), Some(LogEntry::AnnotatedSegmentStart {
                system_prompt: Some(prompt),
                ..
            }) if prompt == SAVED_SUBJEKTIV_SYSTEM_PROMPT)
            );
            assert_eq!(
                session_store::collect_state(&entries)
                    .system_prompt
                    .as_deref(),
                Some(SAVED_SUBJEKTIV_SYSTEM_PROMPT),
                "actual restore must replay the committed prompt rather than re-render it"
            );
            request.operation_id = crate::execution::WorkerLifecycleOperationId::new();
        }
        assert!(!root.path().join("nonexistent-current-profiles").exists());
    }

    #[tokio::test]
    #[serial_test::serial(worker_allocation)]
    async fn profile_factory_pending_subjektiv_restore_requires_operation_owned_launch_material() {
        let root = tempfile::tempdir().unwrap();
        let runtime_store = root.path().join("runtime");
        let mut request = saved_subjektiv_restore_fixture(&runtime_store, false).await;
        let factory = ProfileRuntimeWorkerFactory::new(root.path())
            .with_runtime_store_dir(&runtime_store)
            .with_runtime_id("runtime-subjektiv-test");
        let worker_name =
            ProfileRuntimeWorkerFactory::runtime_worker_name_for_ref(&request.worker_ref);
        let store = WorkerAggregateStore::new(
            factory.worker_aggregate_dir(&request.worker_ref).unwrap(),
            &worker_name,
        )
        .unwrap();
        let metadata = store.read_by_name(&worker_name).unwrap().unwrap();
        let pending_session_id = metadata.active.as_ref().unwrap().session_id;
        assert!(metadata.active.as_ref().unwrap().segment_id.is_none());
        let before = persisted_files(&runtime_store);
        let error = factory.preflight_restore(&request).await.unwrap_err();
        assert!(
            error.contains("Pending Workspace Worker restore requires"),
            "{error}"
        );
        assert_eq!(persisted_files(&runtime_store), before);
        let mut bundle = test_bundle();
        bundle.metadata.workspace_id = "workspace-saved-subjektiv".to_string();
        let builtins = worker::PromptCatalog::builtins_only().unwrap();
        let projection = builtins.projection();
        let mut templates = projection.templates.clone();
        templates.insert(
            "default".to_string(),
            "OPERATION-OWNED-PENDING-SYSTEM-PROMPT".to_string(),
        );
        templates.insert(
            "internal.notify_wrapper".to_string(),
            "SAVED {{ message }}".to_string(),
        );
        let mut catalog = worker::EffectivePromptCatalog::new(
            templates,
            17,
            projection.schema_fingerprint.clone(),
            projection.toolchain_fingerprint.clone(),
        )
        .unwrap();
        catalog.source_digest = "saved-source".to_string();
        bundle.prompt_catalog = Some(catalog);
        let bundle = bundle.with_computed_digest();
        let mut missing_projection = request.clone();
        let mut missing_projection_bundle = bundle.clone();
        missing_projection_bundle.prompt_catalog = None;
        missing_projection.config_bundle = Some(missing_projection_bundle.with_computed_digest());
        let mut wrong_workspace = request.clone();
        let mut wrong_workspace_bundle = bundle.clone();
        wrong_workspace_bundle.metadata.workspace_id = "another-workspace".to_string();
        wrong_workspace.config_bundle = Some(wrong_workspace_bundle.with_computed_digest());
        let mut missing_instruction = request.clone();
        let mut missing_instruction_bundle = bundle.clone();
        let mut templates = projection.templates.clone();
        templates.remove("default");
        missing_instruction_bundle.prompt_catalog = Some(
            worker::EffectivePromptCatalog::new(
                templates,
                17,
                projection.schema_fingerprint.clone(),
                projection.toolchain_fingerprint.clone(),
            )
            .unwrap(),
        );
        missing_instruction.config_bundle = Some(missing_instruction_bundle.with_computed_digest());
        for (invalid, expected) in [
            (request.clone(), "requires operation-owned launch material"),
            (
                missing_projection,
                "requires a saved Workspace Prompt projection",
            ),
            (
                wrong_workspace,
                "Workspace Prompt projection scope mismatch",
            ),
            (missing_instruction, "invalid pending Worker launch Prompt"),
        ] {
            let error = factory.preflight_restore(&invalid).await.unwrap_err();
            assert!(error.contains(expected), "{error}");
            let error = match factory.restore_controller(invalid).await {
                Ok(controller) => {
                    shutdown_profile_controller(controller).await;
                    panic!("invalid pending launch authority started a Controller");
                }
                Err(error) => error,
            };
            assert!(error.contains(expected), "{error}");
            assert_eq!(persisted_files(&runtime_store), before);
            assert!(
                factory
                    .prompt_projection_cache
                    .active("workspace-saved-subjektiv")
                    .unwrap()
                    .is_none()
            );
            assert!(factory.observation_hub.workers.lock().unwrap().is_empty());
        }
        request.config_bundle = Some(bundle);
        factory.preflight_restore(&request).await.unwrap();
        assert_eq!(persisted_files(&runtime_store), before);
        assert!(
            factory
                .prompt_projection_cache
                .active("workspace-saved-subjektiv")
                .unwrap()
                .is_none()
        );
        let controller = factory
            .restore_controller(request.clone())
            .await
            .unwrap_or_else(|error| panic!("pending saved Subjektiv restore failed: {error}"));
        assert_eq!(
            controller.handle.shared_state.catalog_status(),
            WorkerStatus::Idle
        );
        shutdown_profile_controller(controller).await;
        let restored = store.read_by_name(&worker_name).unwrap().unwrap();
        assert_eq!(
            restored.resolved_manifest_snapshot, metadata.resolved_manifest_snapshot,
            "pending launch material must not replace the saved Manifest"
        );
        assert_eq!(
            restored.subjektiv_session_attribution, metadata.subjektiv_session_attribution,
            "pending restore must retain committed Subject attribution"
        );
        let active = restored.active.unwrap();
        assert_eq!(active.session_id, pending_session_id);
        let segment_id = active
            .segment_id
            .expect("operation-owned launch must commit the initial Segment before exposure");
        let entries = WorkerSessionStore::new(
            factory
                .worker_aggregate_dir(&request.worker_ref)
                .unwrap()
                .join("session"),
        )
        .unwrap()
        .read_all(active.session_id, segment_id)
        .unwrap();
        assert!(
            matches!(entries.first(), Some(LogEntry::AnnotatedSegmentStart {
            session_id,
            system_prompt: Some(prompt),
            history,
            forked_from: None,
            compacted_from: None,
            ..
        }) if *session_id == pending_session_id
            && prompt.contains("OPERATION-OWNED-PENDING-SYSTEM-PROMPT")
            && history.is_empty())
        );
        assert!(
            session_store::collect_state(&entries)
                .system_prompt
                .is_some()
        );
    }

    #[tokio::test]
    #[serial_test::serial(worker_allocation)]
    async fn profile_factory_saved_manifest_mismatch_rejected_before_live_work() {
        let root = tempfile::tempdir().unwrap();
        let runtime_store = root.path().join("runtime");
        let request = saved_subjektiv_restore_fixture(&runtime_store, true).await;
        let factory = ProfileRuntimeWorkerFactory::new(root.path())
            .with_runtime_store_dir(&runtime_store)
            .with_runtime_id("runtime-subjektiv-test");
        let before = persisted_files(&runtime_store);
        let mut wrong_revision = request.clone();
        wrong_revision
            .request
            .memory_settings
            .as_mut()
            .unwrap()
            .settings_revision = 18;
        let mut detached = request.clone();
        detached.request.subjektiv_attached = false;
        let mut missing_settings = request.clone();
        missing_settings.request.memory_settings = None;
        for (invalid, expected) in [
            (wrong_revision, "subjektiv settings snapshot mismatch"),
            (detached, "unauthorized subjektiv attachment"),
            (missing_settings, "missing its trusted settings snapshot"),
        ] {
            let error = factory.preflight_restore(&invalid).await.unwrap_err();
            assert!(error.contains(expected), "{error}");
            let error = match factory.restore_controller(invalid).await {
                Ok(controller) => {
                    shutdown_profile_controller(controller).await;
                    panic!("invalid restore started a Controller")
                }
                Err(error) => error,
            };
            assert!(error.contains(expected), "{error}");
            assert_eq!(persisted_files(&runtime_store), before);
            assert!(factory.observation_hub.workers.lock().unwrap().is_empty());
        }
        let run_dir = factory
            .worker_aggregate_dir(&request.worker_ref)
            .unwrap()
            .join("runs");
        assert!(
            !run_dir.exists(),
            "mismatch must reject before creating any run resources"
        );
    }

    #[test]
    fn profile_factory_reads_retained_session_from_canonical_worker_aggregate() {
        let root = tempfile::tempdir().unwrap();
        let runtime_store_dir = root.path().join("runtime");
        let worker_ref = WorkerRef::new(WorkerId::now_v7());
        let (session_id, segment_id, entry_id) =
            write_retained_profile_worker(&runtime_store_dir, &worker_ref);
        let canonical_dir = runtime_store_dir
            .join("workers")
            .join(worker_ref.worker_id.to_string());
        let legacy_prefixed_dir = runtime_store_dir.join("workers").join(
            ProfileRuntimeWorkerFactory::runtime_worker_name_for_ref(&worker_ref),
        );
        assert!(!legacy_prefixed_dir.exists());
        let files_before = persisted_files(&runtime_store_dir);
        let backend = WorkerRuntimeExecutionBackend::new(
            ProfileRuntimeWorkerFactory::new(root.path().join("profiles"))
                .with_runtime_store_dir(&runtime_store_dir),
        )
        .unwrap();

        let retained = backend.worker_session(WorkerSessionObservationRequest {
            worker_ref: worker_ref.clone(),
        });

        let runtime_api::WorkerSessionAvailability::RetainedSnapshot { identity, snapshot } =
            retained
        else {
            panic!("canonical aggregate must produce a retained snapshot");
        };
        assert_eq!(identity.session_id, session_id.to_string());
        assert_eq!(identity.segment_id, segment_id.to_string());
        assert_eq!(identity.entry_count, 1);
        assert_eq!(snapshot.entries.len(), 1);
        assert_eq!(snapshot.entries[0].entry_id, entry_id);
        assert_eq!(
            snapshot.entries[0].data,
            protocol::SessionSnapshotEntryData::Message {
                role: protocol::SessionMessageRole::Assistant,
                content: vec![protocol::SessionContentPart::Text {
                    text: "retained response".to_string(),
                }],
            }
        );
        assert!(canonical_dir.is_dir());
        assert!(!legacy_prefixed_dir.exists());
        assert_eq!(persisted_files(&runtime_store_dir), files_before);
        assert!(backend.workers.lock().unwrap().is_empty());
        assert!(
            backend
                .factory
                .observation_hub
                .workers
                .lock()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn profile_factory_retained_read_keeps_missing_aggregate_typed_and_absent() {
        let root = tempfile::tempdir().unwrap();
        let runtime_store_dir = root.path().join("runtime");
        let worker_ref = WorkerRef::new(WorkerId::now_v7());
        let unconfigured_backend = WorkerRuntimeExecutionBackend::new(
            ProfileRuntimeWorkerFactory::new(root.path().join("unconfigured-profiles")),
        )
        .unwrap();
        assert!(matches!(
            unconfigured_backend.worker_session(WorkerSessionObservationRequest {
                worker_ref: worker_ref.clone(),
            }),
            runtime_api::WorkerSessionAvailability::Unavailable {
                reason: runtime_api::WorkerSessionUnavailableReason::RetentionMissing,
                ..
            }
        ));
        assert!(!root.path().join("unconfigured-profiles").exists());
        assert!(unconfigured_backend.workers.lock().unwrap().is_empty());

        let canonical_dir = runtime_store_dir
            .join("workers")
            .join(worker_ref.worker_id.to_string());
        let backend = WorkerRuntimeExecutionBackend::new(
            ProfileRuntimeWorkerFactory::new(root.path().join("profiles"))
                .with_runtime_store_dir(&runtime_store_dir),
        )
        .unwrap();

        let unavailable = backend.worker_session(WorkerSessionObservationRequest {
            worker_ref: worker_ref.clone(),
        });

        assert!(matches!(
            unavailable,
            runtime_api::WorkerSessionAvailability::Unavailable {
                reason: runtime_api::WorkerSessionUnavailableReason::RetentionMissing,
                ..
            }
        ));
        assert!(!canonical_dir.exists());
        assert!(!runtime_store_dir.exists());
        assert!(backend.workers.lock().unwrap().is_empty());
    }

    #[test]
    fn profile_factory_retained_read_validates_metadata_worker_identity() {
        let root = tempfile::tempdir().unwrap();
        let runtime_store_dir = root.path().join("runtime");
        let worker_ref = WorkerRef::new(WorkerId::now_v7());
        let aggregate_dir = runtime_store_dir
            .join("workers")
            .join(worker_ref.worker_id.to_string());
        fs::create_dir_all(&aggregate_dir).unwrap();
        fs::write(
            aggregate_dir.join("metadata.json"),
            serde_json::to_vec(&WorkerMetadata::new("different-worker", None)).unwrap(),
        )
        .unwrap();
        let files_before = persisted_files(&runtime_store_dir);
        let backend = WorkerRuntimeExecutionBackend::new(
            ProfileRuntimeWorkerFactory::new(root.path().join("profiles"))
                .with_runtime_store_dir(&runtime_store_dir),
        )
        .unwrap();

        let unavailable = backend.worker_session(WorkerSessionObservationRequest { worker_ref });

        assert!(matches!(
            unavailable,
            runtime_api::WorkerSessionAvailability::Unavailable {
                reason: runtime_api::WorkerSessionUnavailableReason::CorruptLog,
                ref message,
            } if message == "retained session log is corrupt"
        ));
        assert_eq!(persisted_files(&runtime_store_dir), files_before);
        assert!(backend.workers.lock().unwrap().is_empty());
    }

    #[test]
    fn restored_router_uses_workspace_proxy_for_logical_only_attachment() {
        let root = tempfile::tempdir().unwrap();
        let scope = manifest::SharedScope::new(manifest::Scope::writable(root.path()).unwrap());
        let client = WorkerWorkspaceContext::local_filesystem(Some(
            WorkspaceId::new("workspace-a").unwrap(),
        ))
        .client_handle();
        let alias = WorkdirAttachmentAlias::new("checkout").unwrap();

        let router = restored_workdir_router(
            &BTreeMap::new(),
            &[LogicalWorkdirAttachment {
                alias: alias.clone(),
                working_directory_id: "remote-workdir".to_string(),
                capabilities: workdir::WorkdirSessionCapabilities::READ_ONLY,
            }],
            &[],
            scope,
            client,
        )
        .unwrap();

        let session = router.session(&alias).unwrap();
        assert_eq!(session.workdir().id().as_str(), "remote-workdir");
        assert_eq!(
            session.capabilities(),
            workdir::WorkdirSessionCapabilities::READ_ONLY
        );
    }

    #[test]
    fn runtime_attachment_scope_matches_session_capabilities() {
        let root = tempfile::tempdir().unwrap();
        let writable = runtime_attachment_scope(root.path(), WorkdirSessionCapabilities::ALL)
            .unwrap()
            .snapshot();
        assert_eq!(
            writable.permission_at(root.path()),
            Some(manifest::Permission::Write)
        );

        let read_only =
            runtime_attachment_scope(root.path(), WorkdirSessionCapabilities::READ_ONLY)
                .unwrap()
                .snapshot();
        assert_eq!(
            read_only.permission_at(root.path()),
            Some(manifest::Permission::Read)
        );
    }

    #[tokio::test]
    async fn restored_router_preserves_read_only_capabilities_for_local_attachment() {
        let runtime_base = tempfile::tempdir().unwrap();
        let repo = create_clean_repo();
        let materializer = RuntimeGitMaterializer::new(runtime_base.path());
        let worker_ref = WorkerRef::new(crate::identity::WorkerId::now_v7());
        let binding = materializer
            .materialize(&worker_ref, &working_directory_request(repo.path()))
            .unwrap();
        let alias = WorkdirAttachmentAlias::new("docs").unwrap();
        let workdir_id = binding.working_directory.id.clone();
        let command_output_scope = manifest::SharedScope::new(manifest::Scope::empty());
        let attachments = BTreeMap::from([(alias.clone(), binding)]);
        let logical = [LogicalWorkdirAttachment {
            alias: alias.clone(),
            working_directory_id: workdir_id,
            capabilities: WorkdirSessionCapabilities::READ_ONLY,
        }];
        let client = WorkerWorkspaceContext::local_filesystem(Some(
            WorkspaceId::new("workspace-a").unwrap(),
        ))
        .client_handle();

        let router =
            restored_workdir_router(&attachments, &logical, &[], command_output_scope, client)
                .unwrap();

        let session = router.session(&alias).unwrap();
        assert_eq!(
            session.capabilities(),
            WorkdirSessionCapabilities::READ_ONLY
        );
        let read = session
            .read(workdir::ReadRequest {
                path: workdir::WorkdirPath::new("README.md").unwrap(),
                offset: 0,
                limit: 10,
                max_bytes: 1024,
            })
            .await
            .unwrap();
        assert_eq!(read.bytes, b"clean\n");
        session
            .authorize_scope_path(workdir::WorkdirScopeAuthorizationRequest {
                rules: vec![workdir::WorkdirToolScopeRule {
                    target: workdir::WorkdirPath::root(),
                    permission: workdir::WorkdirToolScopePermission::Read,
                    recursive: true,
                    symlink_policy: manifest::SymlinkPolicy::Resolved,
                }],
                path: workdir::WorkdirPath::new("README.md").unwrap(),
                permission: workdir::WorkdirToolScopePermission::Read,
            })
            .await
            .unwrap();
    }

    #[test]
    fn profile_factory_routes_workspace_requests_by_workspace_id() {
        let profiles = tempfile::tempdir().unwrap();
        let identity = RuntimeIdentityMaterial::generate("runtime-a").unwrap();
        let factory = ProfileRuntimeWorkerFactory::new(profiles.path())
            .with_workspace_request_client(
                RuntimeWorkspaceRequestClient::new(
                    "workspace-a",
                    "https://workspace-a.example.test",
                    "runtime-a",
                )
                .with_runtime_request_source(&identity, "server-a"),
            )
            .with_workspace_request_client(
                RuntimeWorkspaceRequestClient::new(
                    "workspace-b",
                    "https://workspace-b.example.test",
                    "runtime-a",
                )
                .with_runtime_request_source(&identity, "server-b"),
            );

        assert_eq!(
            factory
                .workspace_request_clients
                .get("workspace-a")
                .and_then(RuntimeWorkspaceRequestClient::audience),
            Some("server-a")
        );
        assert_eq!(
            factory
                .workspace_request_clients
                .get("workspace-b")
                .and_then(RuntimeWorkspaceRequestClient::audience),
            Some("server-b")
        );
    }

    #[tokio::test]
    async fn workspace_config_refresh_uses_workspace_scoped_request_client() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut chunk = [0_u8; 1024];
            loop {
                let read = stream.read(&mut chunk).await.unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            stream
                .write_all(b"HTTP/1.1 304 Not Modified\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
            String::from_utf8(request).unwrap()
        });
        let identity = RuntimeIdentityMaterial::generate("runtime-a").unwrap();
        let base_url = format!("http://{address}");
        let client =
            RuntimeWorkspaceRequestClient::new("workspace-b", base_url.clone(), "runtime-a")
                .with_runtime_request_source(&identity, "server-b");
        let bundle = test_bundle();
        let bundle_ref = ConfigBundleRef {
            id: bundle.metadata.id.clone(),
            digest: bundle.metadata.digest.clone(),
        };
        let request = WorkspaceConfigFetchRequest {
            workspace_api: WorkspaceApiRef {
                workspace_id: "workspace-b".to_string(),
                base_url,
            },
            profile: crate::catalog::ProfileSelector::Named("coder".to_string()),
            expected: bundle_ref.clone(),
            cached: Some(bundle_ref),
        };

        let result = fetch_workspace_config_http(&request, &client)
            .await
            .unwrap();
        assert!(matches!(result, WorkspaceConfigFetchResult::NotModified));
        let raw_request = server.await.unwrap();
        let proof = raw_request
            .lines()
            .find_map(|line| {
                line.split_once(':').and_then(|(name, value)| {
                    name.eq_ignore_ascii_case(crate::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER)
                        .then(|| value.trim().to_string())
                })
            })
            .unwrap();
        let claims = crate::auth::decode_runtime_request_source_claims(&proof).unwrap();
        assert_eq!(claims.aud, "server-b");
        assert_eq!(claims.workspace_id, "workspace-b");
        assert_eq!(claims.worker_id, None);
        assert_eq!(claims.method, "GET");
        assert_eq!(
            claims.path,
            "/api/w/workspace-b/runtime-config?profile=coder"
        );
    }

    #[tokio::test]
    async fn execution_scope_drains_remaining_tasks_and_retains_abnormal_completion() {
        let failed = tokio::spawn(async { panic!("injected controller failure") });
        let scope = RuntimeExecutionTaskScope::new(failed);
        scope.push(
            "protocol bridge",
            tokio::spawn(std::future::pending()),
            true,
        );

        let error = scope.join().await.unwrap_err();
        assert!(error.contains("controller task failed"));
        let retained = scope.tasks.lock().unwrap().clone();
        assert_eq!(retained.len(), 1, "only the failed completion must remain");
        assert_eq!(retained[0].name, "controller");
        assert!(retained[0].completion.lock().await.failure.is_some());
        for _ in 0..3 {
            assert_eq!(scope.join().await.unwrap_err(), error);
        }
    }

    #[tokio::test]
    async fn execution_scope_retains_closed_shutdown_completion_after_other_tasks_drain() {
        let scope = RuntimeExecutionTaskScope::new(tokio::spawn(async {}));
        scope.push(
            "protocol bridge",
            tokio::spawn(std::future::pending()),
            true,
        );
        let (sender, receiver) = tokio::sync::oneshot::channel();
        drop(sender);
        let shutdown = Arc::new(tokio::sync::Mutex::new(Some(receiver)));
        for _ in 0..3 {
            let error = scope
                .confirm_shutdown_and_join(&shutdown)
                .await
                .unwrap_err();
            assert!(error.contains("completion channel closed"), "{error}");
            assert_eq!(scope.tasks.lock().unwrap().len(), 1);
        }
        assert!(shutdown.lock().await.is_none());
    }

    #[tokio::test]
    async fn cancelled_execution_scope_join_keeps_actual_task_owned_for_retry() {
        let release = Arc::new(tokio::sync::Notify::new());
        let task_release = Arc::clone(&release);
        let scope = RuntimeExecutionTaskScope::new(tokio::spawn(async move {
            task_release.notified().await;
        }));
        let owned = scope.tasks.lock().unwrap()[0].clone();
        let joining_scope = scope.clone();
        let joining = tokio::spawn(async move { joining_scope.join().await });
        tokio::time::timeout(Duration::from_secs(5), async {
            while owned.completion.try_lock().is_ok() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        joining.abort();
        assert!(joining.await.unwrap_err().is_cancelled());
        assert!(Arc::ptr_eq(&scope.tasks.lock().unwrap()[0], &owned));
        assert!(owned.completion.lock().await.task.is_some());
        release.notify_one();
        scope.join().await.unwrap();
        assert!(scope.tasks.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn execution_scope_timeout_retains_exact_task_until_completion_is_proven() {
        let release = Arc::new(tokio::sync::Notify::new());
        let task_release = Arc::clone(&release);
        let scope = RuntimeExecutionTaskScope::new(tokio::spawn(async move {
            task_release.notified().await;
        }));
        let owned = scope.tasks.lock().unwrap()[0].clone();
        let error = scope.join().await.unwrap_err();
        assert!(
            error.contains("task did not stop before timeout"),
            "{error}"
        );
        assert!(Arc::ptr_eq(&scope.tasks.lock().unwrap()[0], &owned));
        assert!(owned.completion.lock().await.task.is_some());
        release.notify_one();
        scope.join().await.unwrap();
        assert!(scope.tasks.lock().unwrap().is_empty());
    }

    fn adapter_command(
        backend: &WorkerRuntimeExecutionBackend<MockFactory>,
        worker_ref: &WorkerRef,
    ) -> WorkerCommandEnvelope {
        let workers = backend.workers.lock().unwrap();
        let state = workers
            .get(worker_ref)
            .expect("worker execution")
            .worker_state
            .read()
            .unwrap()
            .clone();
        WorkerCommandEnvelope::new(state.last_command_id.saturating_add(1))
    }

    #[test]
    fn protocol_bridge_replaces_every_full_state_snapshot() {
        let running = protocol::WorkerStateSnapshot {
            state: protocol::WorkerState::Busy(protocol::WorkerBusyState::Run(
                protocol::WorkerRunState::Running,
            )),
            last_command_id: 2,
            last_finished_submission_request_id: Some("request-old".to_string()),
        };
        let current = Arc::new(RwLock::new(running));
        let idle = protocol::WorkerStateSnapshot {
            state: protocol::WorkerState::Idle,
            last_command_id: 0,
            last_finished_submission_request_id: None,
        };
        let mut replacement = Event::WorkerState {
            snapshot: idle.clone(),
        };
        assert!(apply_protocol_worker_state(&current, &mut replacement).unwrap());
        assert_eq!(*current.read().unwrap(), idle);

        let paused = protocol::WorkerStateSnapshot {
            state: protocol::WorkerState::Busy(protocol::WorkerBusyState::Run(
                protocol::WorkerRunState::Paused,
            )),
            last_command_id: 3,
            last_finished_submission_request_id: None,
        };
        let mut acknowledgement = Event::CommandAcknowledged {
            acknowledgement: protocol::WorkerCommandAcknowledgement {
                command_id: 3,
                command: protocol::WorkerCommandKind::Pause,
                disposition: protocol::WorkerCommandDisposition::Accepted,
                state: paused.clone(),
            },
        };
        assert!(apply_protocol_worker_state(&current, &mut acknowledgement).unwrap());
        assert_eq!(*current.read().unwrap(), paused);
    }

    #[test]
    fn workspace_prompt_projection_notification_advances_shared_cache() {
        let cache = WorkspacePromptProjectionCache::default();
        let catalog_v1 = worker::EffectivePromptCatalog::new(
            BTreeMap::from([("default".to_string(), "prompt-v1".to_string())]),
            8,
            "schema",
            "toolchain",
        )
        .unwrap();
        let projection_v1 = worker::WorkspacePromptProjection::new(
            "workspace-a",
            "source-v1",
            catalog_v1.catalog_digest.clone(),
            catalog_v1,
        )
        .unwrap();
        let catalog_v2 = worker::EffectivePromptCatalog::new(
            BTreeMap::from([("default".to_string(), "prompt-v2".to_string())]),
            9,
            "schema",
            "toolchain",
        )
        .unwrap();
        let projection_v2 = worker::WorkspacePromptProjection::new(
            "workspace-a",
            "source-v2",
            catalog_v2.catalog_digest.clone(),
            catalog_v2.clone(),
        )
        .unwrap();

        cache.observe(projection_v1).unwrap();
        cache.observe(projection_v2).unwrap();

        let active = cache.active("workspace-a").unwrap().unwrap();
        assert_eq!(active.projection.config_revision, 9);
        assert_eq!(
            active.projection.catalog.catalog_digest,
            catalog_v2.catalog_digest
        );
    }

    #[test]
    fn workspace_prompt_projection_cache_rejects_same_revision_source_drift() {
        let catalog = worker::EffectivePromptCatalog::new(
            BTreeMap::from([("default".to_string(), "prompt".to_string())]),
            8,
            "schema",
            "toolchain",
        )
        .unwrap();
        let first = worker::WorkspacePromptProjection::new(
            "workspace-a",
            "source-a",
            catalog.catalog_digest.clone(),
            catalog.clone(),
        )
        .unwrap();
        let drifted = worker::WorkspacePromptProjection::new(
            "workspace-a",
            "source-b",
            catalog.catalog_digest.clone(),
            catalog,
        )
        .unwrap();
        let cache = WorkspacePromptProjectionCache::default();

        cache.observe(first).unwrap();
        let error = cache.observe(drifted).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("without a config revision transition")
        );
    }

    #[test]
    fn workspace_prompt_projection_cache_rejects_same_revision_schema_drift() {
        let templates = BTreeMap::from([("default".to_string(), "prompt".to_string())]);
        let first_catalog =
            worker::EffectivePromptCatalog::new(templates.clone(), 8, "schema-a", "toolchain")
                .unwrap();
        let drifted_catalog =
            worker::EffectivePromptCatalog::new(templates, 8, "schema-b", "toolchain").unwrap();
        let first = worker::WorkspacePromptProjection::new(
            "workspace-a",
            "source-a",
            first_catalog.catalog_digest.clone(),
            first_catalog,
        )
        .unwrap();
        let drifted = worker::WorkspacePromptProjection::new(
            "workspace-a",
            "source-a",
            drifted_catalog.catalog_digest.clone(),
            drifted_catalog,
        )
        .unwrap();
        let cache = WorkspacePromptProjectionCache::default();

        cache.observe(first).unwrap();
        let error = cache.observe(drifted).unwrap_err();
        assert!(error.contains("without a config revision transition"));
    }

    #[test]
    fn restart_restore_reconstructs_runtime_owned_worker_mutation_client() {
        let identity = RuntimeIdentityMaterial::generate("runtime-source").unwrap();
        let worker_ref = WorkerRef::new(crate::identity::WorkerId::from_legacy_u64(17));
        let backend = RuntimeWorkspaceBackendRef::Http {
            workspace_id: "workspace-a".to_string(),
            base_url: "https://server.invalid".to_string(),
            runtime_id: "runtime-source".to_string(),
        };
        let scope = crate::runtime::RuntimeWorkspaceScope::new("workspace-a", "server-main");

        let before_restart =
            backend.worker_context(&worker_ref, Some(&scope), Some(&identity), None, None, None);
        let adapter = WorkerRuntimeExecutionBackend::new(FailingFactory).unwrap();
        let (after_restore_kind, after_restore_workspace_id) = adapter
            .run_on_adapter_runtime(async move {
                let after_restore = backend.worker_context(
                    &worker_ref,
                    Some(&scope),
                    Some(&identity),
                    None,
                    None,
                    None,
                );
                let client = after_restore.client_handle();
                Ok((
                    client.kind().to_string(),
                    client.workspace_id().map(str::to_string),
                ))
            })
            .expect("restore must reconstruct its Workspace client inside the adapter Runtime");

        assert_eq!(
            before_restart.client_handle().kind(),
            "runtime-owned-workspace-client"
        );
        assert_eq!(after_restore_kind, "runtime-owned-workspace-client");
        assert_eq!(after_restore_workspace_id.as_deref(), Some("workspace-a"));
    }

    #[derive(Clone)]
    enum MockResponse {
        Complete(Vec<LlmEvent>),
        Hang(Vec<LlmEvent>),
    }

    #[derive(Clone)]
    struct MockClient {
        responses: Arc<Vec<MockResponse>>,
        call_count: Arc<AtomicUsize>,
        captured: Arc<Mutex<Vec<Request>>>,
    }

    impl MockClient {
        fn new(events: Vec<LlmEvent>) -> Self {
            Self::sequential(vec![MockResponse::Complete(events)])
        }

        fn sequential(responses: Vec<MockResponse>) -> Self {
            Self {
                responses: Arc::new(responses),
                call_count: Arc::new(AtomicUsize::new(0)),
                captured: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    #[async_trait]
    impl LlmClient for MockClient {
        fn clone_boxed(&self) -> Box<dyn LlmClient> {
            Box::new(self.clone())
        }

        async fn stream(
            &self,
            request: Request,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent, ClientError>> + Send>>, ClientError>
        {
            self.captured.lock().unwrap().push(request);
            let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
            let response = self
                .responses
                .get(idx)
                .cloned()
                .unwrap_or_else(|| MockResponse::Complete(Vec::new()));
            match response {
                MockResponse::Complete(events) => {
                    Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
                }
                MockResponse::Hang(events) => Ok(Box::pin(
                    futures::stream::iter(events.into_iter().map(Ok))
                        .chain(futures::stream::pending()),
                )),
            }
        }
    }

    #[cfg(feature = "ws-server")]
    fn test_execution_context(worker_ref: WorkerRef) -> WorkerExecutionContext {
        WorkerExecutionContext::new(
            worker_ref,
            Arc::new(|worker_ref, payload| {
                Ok(crate::observation::WorkerObservationEvent::new(
                    1, worker_ref, payload,
                ))
            }),
        )
    }

    #[cfg(not(feature = "ws-server"))]
    fn test_execution_context(worker_ref: WorkerRef) -> WorkerExecutionContext {
        WorkerExecutionContext::new(worker_ref)
    }

    #[test]
    fn input_commit_budget_leaves_adapter_cancellation_margin() {
        assert!(USER_INPUT_TASK_TIMEOUT > USER_INPUT_COMMIT_TIMEOUT);
        assert_eq!(
            USER_INPUT_TASK_TIMEOUT - USER_INPUT_COMMIT_TIMEOUT,
            Duration::from_secs(1)
        );
    }

    struct DelayedFactory {
        completed: Arc<AtomicBool>,
        delay: Duration,
    }

    #[async_trait]
    impl RuntimeWorkerFactory for DelayedFactory {
        async fn spawn_controller(
            &self,
            _request: WorkerExecutionSpawnRequest,
        ) -> Result<RuntimeWorkerController, String> {
            tokio::time::sleep(self.delay).await;
            self.completed.store(true, Ordering::SeqCst);
            Err("delayed factory completed".to_string())
        }

        async fn restore_controller(
            &self,
            _request: WorkerExecutionRestoreRequest,
        ) -> Result<RuntimeWorkerController, String> {
            tokio::time::sleep(self.delay).await;
            self.completed.store(true, Ordering::SeqCst);
            Err("delayed factory completed".to_string())
        }
    }

    #[test]
    fn create_timeout_cancels_factory_and_removes_persisted_worker() {
        let root = tempfile::tempdir().unwrap();
        let runtime_store_dir = root.path().join("runtime");
        let completed = Arc::new(AtomicBool::new(false));
        let backend = Arc::new(
            WorkerRuntimeExecutionBackend::new(DelayedFactory {
                completed: completed.clone(),
                delay: Duration::from_millis(200),
            })
            .unwrap()
            .with_spawn_restore_timeout(Duration::from_millis(20)),
        );
        let runtime = EmbeddedRuntime::with_fs_store_and_execution_backend(
            crate::fs_store::FsRuntimeStoreOptions {
                root: runtime_store_dir.clone(),
                runtime_id: "create-timeout-runtime".to_string(),
                display_name: None,
            },
            backend.clone(),
        )
        .unwrap();
        runtime.store_config_bundle(test_bundle()).unwrap();
        let request = create_request("create timeout");
        let worker_id = request.worker_id;
        let create_runtime = runtime.clone();
        let create = std::thread::spawn(move || create_runtime.create_worker(request));
        std::thread::sleep(Duration::from_millis(5));

        let delete_error = runtime
            .delete_worker(&crate::identity::WorkerRef::new(worker_id))
            .unwrap_err();
        let error = create.join().unwrap().unwrap_err();

        assert!(matches!(
            delete_error,
            crate::error::RuntimeError::WorkerNotFound { worker_id: missing } if missing == worker_id
        ));
        assert!(error.to_string().contains("was cancelled"));
        std::thread::sleep(Duration::from_millis(250));
        assert!(
            !completed.load(Ordering::SeqCst),
            "timed out factory future must not resume after create returns"
        );
        assert!(runtime.list_workers().unwrap().is_empty());
        assert!(backend.workers.lock().unwrap().is_empty());
        assert!(
            !runtime_store_dir
                .join("workers")
                .join(worker_id.to_string())
                .exists(),
            "failed create must remove its persisted Worker aggregate"
        );
    }

    struct MockFactory {
        client: MockClient,
        runtime_base: PathBuf,
        cwd: PathBuf,
        store_dir: PathBuf,
        worker_metadata_dir: PathBuf,
        observed_cwds: Arc<Mutex<Vec<PathBuf>>>,
        observed_workspace_clients: Arc<Mutex<Vec<(String, Option<String>, bool)>>>,
    }

    #[async_trait]
    impl RuntimeWorkerFactory for MockFactory {
        async fn spawn_controller(
            &self,
            request: WorkerExecutionSpawnRequest,
        ) -> Result<RuntimeWorkerController, String> {
            let manifest = WorkerManifest::from_toml(
                r#"
                [worker]
                name = "runtime-adapter-test"
                pwd = "./"

                [model]
                scheme = "anthropic"
                model_id = "test-model"
                auth = { kind = "none" }

                [engine]
                max_tokens = 100

                [[scope.allow]]
                target = "./"
                permission = "write"
                "#,
            )
            .map_err(|err| err.to_string())?;
            let store = CombinedStore::new(
                FsStore::new(&self.store_dir).map_err(|err| err.to_string())?,
                FsWorkerStore::new(&self.worker_metadata_dir).map_err(|err| err.to_string())?,
            );
            let only_workdir = (request.workdir_attachments.len() == 1)
                .then(|| request.workdir_attachments.values().next())
                .flatten();
            let filesystem_authority = only_workdir
                .map(|binding| {
                    let cwd = binding.cwd().to_path_buf();
                    self.observed_cwds.lock().unwrap().push(cwd.clone());
                    WorkerFilesystemAuthority::local(binding.root().to_path_buf(), cwd)
                })
                .unwrap_or(WorkerFilesystemAuthority::None);
            let scope_root = only_workdir
                .map(|binding| binding.root().to_path_buf())
                .unwrap_or_else(|| self.cwd.clone());
            let workspace_backend_ref = RuntimeWorkspaceBackendRef::from_worker_request(
                &request.request,
                Some("runtime-test"),
            );
            let workspace_context = workspace_backend_ref.worker_context(
                &request.worker_ref,
                request.workspace_scope.as_ref(),
                None,
                None,
                None,
                None,
            );
            let workspace_client = workspace_context.client_handle();
            self.observed_workspace_clients.lock().unwrap().push((
                workspace_client.kind().to_string(),
                workspace_client.workspace_id().map(str::to_string),
                workspace_client.is_available(),
            ));
            let scope = Scope::writable(&scope_root).map_err(|err| err.to_string())?;
            let worker = Worker::new(
                manifest,
                Engine::<_, agen::state::Mutable, worker::SessionHistoryMetadata>::new_annotated(
                    self.client.clone(),
                ),
                store,
                workspace_context,
                filesystem_authority,
                scope,
            )
            .await
            .map_err(|err| err.to_string())?;
            let bash_output_dir = self.runtime_base.join("bash-output");
            let (handle, shutdown_rx) = WorkerController::spawn_runtime_managed(
                worker,
                &self.runtime_base,
                &bash_output_dir,
            )
            .await
            .map_err(|err| err.to_string())?;
            Ok(RuntimeWorkerController {
                handle,
                shutdown: Arc::new(tokio::sync::Mutex::new(Some(shutdown_rx))),
                controller_task: tokio::spawn(async {}),
                workspace_client,
            })
        }
        async fn restore_controller(
            &self,
            request: WorkerExecutionRestoreRequest,
        ) -> Result<RuntimeWorkerController, String> {
            let request = WorkerExecutionSpawnRequest {
                worker_ref: request.worker_ref,
                request: request.request,
                workspace_scope: request.workspace_scope,
                context: request.context,
                workdir_attachments: request.workdir_attachments,
                config_bundle: request.config_bundle,
            };
            self.spawn_controller(request).await
        }
    }

    fn core_filesystem_tool_names() -> BTreeSet<&'static str> {
        ["Read", "Write", "Edit", "Glob", "Grep", "Bash"]
            .into_iter()
            .collect()
    }

    fn captured_tool_names(client: &MockClient, index: usize) -> BTreeSet<String> {
        client.captured.lock().unwrap()[index]
            .tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect()
    }

    fn wait_for_adapter_command(
        backend: &WorkerRuntimeExecutionBackend<MockFactory>,
        worker_ref: &WorkerRef,
        expected_command_id: u64,
    ) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let observed = {
                let workers = backend.workers.lock().unwrap();
                workers
                    .get(worker_ref)
                    .expect("live Worker execution")
                    .worker_state
                    .read()
                    .unwrap()
                    .last_command_id
            };
            if observed >= expected_command_id {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for adapter command {expected_command_id}; last observed={observed}",
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait_for_adapter_state(
        backend: &WorkerRuntimeExecutionBackend<MockFactory>,
        worker_ref: &WorkerRef,
        expected_status: WorkerStatus,
    ) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let observed = {
                let workers = backend.workers.lock().unwrap();
                let execution = workers.get(worker_ref).expect("live Worker execution");
                let projected = execution.worker_state.read().unwrap().catalog_status();
                (execution.handle.shared_state.catalog_status(), projected)
            };
            if observed == (expected_status, expected_status) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for adapter state {expected_status:?}; last observed controller={:?}, projected={:?}",
                observed.0,
                observed.1,
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn simple_text_events() -> Vec<LlmEvent> {
        vec![
            LlmEvent::text_block_start(0),
            LlmEvent::text_delta(0, "hello"),
            LlmEvent::text_delta(0, " from worker"),
            LlmEvent::text_block_stop(0, None),
            LlmEvent::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }),
        ]
    }

    fn test_bundle() -> crate::config_bundle::ConfigBundle {
        crate::config_bundle::ConfigBundle {
            metadata: crate::config_bundle::ConfigBundleMetadata {
                id: "adapter-test-bundle".to_string(),
                digest: String::new(),
                revision: "test".to_string(),
                workspace_id: "adapter-test".to_string(),
                created_at: "test".to_string(),
                provenance: crate::config_bundle::ConfigBundleProvenance {
                    source: "test".to_string(),
                    detail: None,
                },
            },
            profiles: vec![crate::config_bundle::ConfigProfileDescriptor {
                selector: ProfileSelector::Builtin("builtin:companion".to_string()),
                label: Some("adapter-test".to_string()),
            }],
            declarations: Vec::new(),
            prompt_catalog: None,
            profile_source_archive: Some(sample_profile_archive()),
            profile_source_archive_handle: None,
        }
        .with_computed_digest()
    }

    fn sample_profile_archive() -> crate::profile_archive::ProfileSourceArchive {
        let entrypoints = BTreeMap::from([
            ("default".to_string(), "profiles/default.dcdl".to_string()),
            (
                "builtin:default".to_string(),
                "profiles/default.dcdl".to_string(),
            ),
            (
                "builtin:companion".to_string(),
                "profiles/default.dcdl".to_string(),
            ),
        ]);
        let sources = BTreeMap::from([(
            "profiles/default.dcdl".to_string(),
            r#"{
                slug = "default";
                description = "Default";
                scope = "workspace_read";
                model = {
                    scheme = "anthropic";
                    model_id = "test-model";
                    auth = { kind = "none"; };
                };
                engine = { max_tokens = 100; };
            }"#
            .to_string(),
        )]);
        crate::profile_archive::ProfileSourceArchive::build(
            crate::profile_archive::ProfileSourceArchiveInput {
                id: "profile-source-archive:test".to_string(),
                entrypoints,
                imports: BTreeMap::new(),
                sources,
            },
        )
        .unwrap()
    }

    fn create_request(_name: &str) -> CreateWorkerRequest {
        let bundle = test_bundle();
        CreateWorkerRequest {
            worker_id: WorkerId::now_v7(),
            create_fingerprint: "test-create".to_string(),
            profile: ProfileSelector::Builtin("builtin:companion".to_string()),
            display_name: None,
            profile_source: crate::catalog::ProfileSourceArchiveSource::Embedded {
                archive: bundle.profile_source_archive.clone().unwrap(),
            },
            config_bundle: Some(ConfigBundleRef {
                id: bundle.metadata.id,
                digest: bundle.metadata.digest,
            }),
            initial_input: None,
            workdir_attachment_requests: Vec::new(),
            workdir_attachments: Vec::new(),
            worker_observation_enabled: false,
            worker_observation_grants: Vec::new(),
            workspace_api: None,
            memory_settings: None,
            subjektiv_attached: false,
            backend_job: None,
        }
    }

    #[test]
    fn backend_job_profile_policy_matches_trusted_grant_without_implicit_changes() {
        let mut manifest = WorkerManifest::from_toml(
            r#"
            [worker]
            name = "job-policy-test"
            pwd = "./"
            [model]
            scheme = "anthropic"
            model_id = "chosen-model"
            auth = { kind = "none" }
            [engine]
            instruction = "default"
            [[scope.allow]]
            target = "./"
            permission = "read"
        "#,
        )
        .unwrap();
        let mut request = create_request("policy");
        for granted in [false, true] {
            request.backend_job = Some(crate::catalog::BackendJobExecutionBinding {
                job_id: "job-1".into(),
                attempt_id: "attempt-1".into(),
                input_revision: None,
                subjektiv_consolidation: granted,
            });
            for tools in [false, true] {
                for extraction in [false, true] {
                    manifest.feature.subjektiv.profile.consolidation_tools = tools;
                    manifest.feature.subjektiv.profile.extraction.enabled = extraction;
                    let expected = tools == granted && (!granted || !extraction);
                    let before = serde_json::to_value(&manifest).unwrap();
                    assert_eq!(
                        validate_backend_job_profile(&manifest, &request).is_ok(),
                        expected
                    );
                    assert_eq!(
                        bind_workspace_memory_settings(&mut manifest, &request).is_ok(),
                        expected
                    );
                    assert_eq!(
                        validate_worker_memory_settings(&manifest, &request).is_ok(),
                        expected
                    );
                    assert_eq!(serde_json::to_value(&manifest).unwrap(), before);
                }
            }
        }
    }

    #[test]
    fn host_attachment_alone_activates_subjektiv_profile_policy() {
        let manifest = || {
            let mut manifest = WorkerManifest::from_toml(
                r#"
                [worker]
                name = "subjektiv-attachment-test"
                pwd = "./"

                [model]
                scheme = "anthropic"
                model_id = "test-model"
                auth = { kind = "none" }

                [engine]
                max_tokens = 100

                [[scope.allow]]
                target = "./"
                permission = "read"
                "#,
            )
            .unwrap();
            manifest.feature.subjektiv.profile.enabled = true;
            manifest.feature.subjektiv.profile.extraction.enabled = true;
            manifest
        };
        let settings = manifest::WorkspaceMemorySettingsSnapshot {
            workspace_id: "workspace-subject".to_string(),
            settings_revision: 3,
            language: "English".to_string(),
        };
        let mut ordinary_request = create_request("ordinary");
        ordinary_request.workspace_api = Some(WorkspaceApiRef {
            workspace_id: settings.workspace_id.clone(),
            base_url: "http://workspace.invalid".to_string(),
        });
        ordinary_request.memory_settings = Some(settings.clone());
        let mut ordinary = manifest();
        bind_workspace_memory_settings(&mut ordinary, &ordinary_request).unwrap();
        assert!(ordinary.feature.subjektiv.profile.enabled);
        assert!(!ordinary.feature.subjektiv.execution_enabled());
        validate_worker_memory_settings(&ordinary, &ordinary_request).unwrap();

        let mut attached_request = ordinary_request.clone();
        attached_request.subjektiv_attached = true;
        let mut attached = manifest();
        bind_workspace_memory_settings(&mut attached, &attached_request).unwrap();
        assert!(attached.feature.subjektiv.execution_enabled());
        assert_eq!(
            attached.feature.subjektiv.workspace_settings(),
            Some(settings.clone())
        );
        validate_worker_memory_settings(&attached, &attached_request).unwrap();
        assert!(validate_worker_memory_settings(&attached, &ordinary_request).is_err());

        attached_request.memory_settings = None;
        assert!(bind_workspace_memory_settings(&mut manifest(), &attached_request).is_err());
    }

    fn git(path: &std::path::Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {:?} failed", args);
    }

    #[derive(Clone)]
    struct FailingFactory;

    #[async_trait]
    impl RuntimeWorkerFactory for FailingFactory {
        async fn spawn_controller(
            &self,
            _request: WorkerExecutionSpawnRequest,
        ) -> Result<RuntimeWorkerController, String> {
            Err("spawn failed".to_string())
        }

        async fn restore_controller(
            &self,
            _request: WorkerExecutionRestoreRequest,
        ) -> Result<RuntimeWorkerController, String> {
            Err("restore failed".to_string())
        }
    }

    #[cfg(feature = "fs-store")]
    struct RestartArtifactFailureFactory {
        inner: MockFactory,
        runtime_store: PathBuf,
        restore_id: Arc<Mutex<Option<crate::execution::WorkerLifecycleOperationId>>>,
    }

    #[cfg(feature = "fs-store")]
    #[async_trait]
    impl RuntimeWorkerFactory for RestartArtifactFailureFactory {
        async fn spawn_controller(
            &self,
            request: WorkerExecutionSpawnRequest,
        ) -> Result<RuntimeWorkerController, String> {
            self.inner.spawn_controller(request).await
        }

        async fn restore_controller(
            &self,
            request: WorkerExecutionRestoreRequest,
        ) -> Result<RuntimeWorkerController, String> {
            *self.restore_id.lock().unwrap() = Some(request.operation_id);
            let runs = self
                .runtime_store
                .join("workers")
                .join(request.worker_ref.worker_id.to_string())
                .join("runs");
            fs::create_dir_all(&runs).unwrap();
            // Deterministic cleanup-validation failure, including for root users.
            fs::write(
                runs.join(format!("restore-{}", request.operation_id)),
                b"incomplete restore artifact",
            )
            .unwrap();
            Err("injected restore failure with persisted artifacts".to_string())
        }

        async fn cleanup_failed_restore(
            &self,
            _: &WorkerExecutionRestoreRequest,
        ) -> Result<(), String> {
            Err("injected restore artifact cleanup failure".to_string())
        }
    }

    #[cfg(feature = "fs-store")]
    #[test]
    fn fs_restart_stop_cleans_prior_restore_artifacts_and_retains_journal_on_failure() {
        let root = tempfile::tempdir().unwrap();
        let runtime_store = root.path().join("runtime");
        let options = crate::fs_store::FsRuntimeStoreOptions {
            root: runtime_store.clone(),
            runtime_id: "restart-artifact-runtime".to_string(),
            display_name: None,
        };
        let restore_id = Arc::new(Mutex::new(None));
        let backend = Arc::new(
            WorkerRuntimeExecutionBackend::new(RestartArtifactFailureFactory {
                inner: MockFactory {
                    client: MockClient::sequential(vec![]),
                    runtime_base: root.path().join("controllers"),
                    cwd: root.path().to_path_buf(),
                    store_dir: root.path().join("sessions"),
                    worker_metadata_dir: root.path().join("metadata"),
                    observed_cwds: Arc::new(Mutex::new(Vec::new())),
                    observed_workspace_clients: Arc::new(Mutex::new(Vec::new())),
                },
                runtime_store: runtime_store.clone(),
                restore_id: Arc::clone(&restore_id),
            })
            .unwrap(),
        );
        let runtime =
            EmbeddedRuntime::with_fs_store_and_execution_backend(options.clone(), backend.clone())
                .unwrap();
        runtime.store_config_bundle(test_bundle()).unwrap();
        let worker = runtime
            .create_worker(create_request("restart cleanup"))
            .unwrap();
        runtime.stop_worker(&worker.worker_ref, None).unwrap();
        assert!(
            runtime
                .restore_worker(
                    &worker.worker_ref,
                    runtime.test_restore_request(&worker.worker_ref)
                )
                .is_err()
        );
        assert!(runtime.stop_worker(&worker.worker_ref, None).is_err());
        let prior_restore_id = restore_id.lock().unwrap().unwrap();
        let aggregate = runtime_store
            .join("workers")
            .join(worker.worker_id.to_string());
        let journal_path = aggregate.join("worker.json");
        let journal_before = fs::read(&journal_path).unwrap();
        let journal: serde_json::Value = serde_json::from_slice(&journal_before).unwrap();
        assert_eq!(journal["execution_state"]["execution"]["operation"], "stop");
        assert_eq!(
            journal["execution_state"]["execution"]["intent"]["pending_restore"]["operation_id"],
            prior_restore_id.to_string()
        );
        let artifact = aggregate
            .join("runs")
            .join(format!("restore-{prior_restore_id}"));
        let create_run = aggregate
            .join("runs")
            .join(uuid::Uuid::now_v7().to_string());
        fs::create_dir_all(&create_run).unwrap();
        fs::write(create_run.join("keep.log"), "ordinary creation run").unwrap();
        let another_worker = runtime_store
            .join("workers")
            .join(WorkerId::now_v7().to_string())
            .join("runs")
            .join(format!(
                "restore-{}",
                crate::execution::WorkerLifecycleOperationId::new()
            ));
        fs::create_dir_all(&another_worker).unwrap();
        fs::write(another_worker.join("keep.log"), "another Worker").unwrap();
        fs::create_dir_all(aggregate.join("session")).unwrap();
        fs::write(
            aggregate.join("session").join("keep-authority"),
            "saved Session authority",
        )
        .unwrap();
        drop(runtime);
        drop(backend);

        // All backend maps are empty after process restart. The nested journal
        // restore id is not part of StopRequest; artifacts are discovered only
        // below this exact Worker's aggregate, after excluding live resources.
        let profile_base = root.path().join("must-not-launch-current-profile");
        let backend = Arc::new(
            WorkerRuntimeExecutionBackend::new(
                ProfileRuntimeWorkerFactory::new(&profile_base)
                    .with_runtime_store_dir(&runtime_store),
            )
            .unwrap(),
        );
        let restarted =
            EmbeddedRuntime::with_fs_store_and_execution_backend(options.clone(), backend.clone())
                .unwrap();
        assert_eq!(fs::read(&journal_path).unwrap(), journal_before);
        assert!(backend.workers.lock().unwrap().is_empty());
        assert!(backend.pending_restores.lock().unwrap().is_empty());
        assert!(
            backend
                .factory
                .failed_restore_sessions
                .lock()
                .unwrap()
                .is_empty()
        );
        let failed = restarted.stop_worker(&worker.worker_ref, None).unwrap_err();
        assert!(
            failed.to_string().contains("unsafe restore artifact"),
            "{failed}"
        );
        assert_eq!(fs::read(&journal_path).unwrap(), journal_before);
        assert!(artifact.is_file());
        assert!(!profile_base.exists());

        fs::remove_file(&artifact).unwrap();
        fs::create_dir(&artifact).unwrap();
        fs::write(artifact.join("worker.err.log"), "stale restore diagnostics").unwrap();
        restarted.stop_worker(&worker.worker_ref, None).unwrap();
        assert!(!artifact.exists());
        assert_eq!(
            restarted.worker_detail(&worker.worker_ref).unwrap().status,
            crate::catalog::WorkerStatus::Stopped
        );
        let journal: serde_json::Value =
            serde_json::from_slice(&fs::read(&journal_path).unwrap()).unwrap();
        assert_ne!(
            journal["execution_state"]["state"],
            "reconciliation_required"
        );
        assert_eq!(
            fs::read_to_string(aggregate.join("session").join("keep-authority")).unwrap(),
            "saved Session authority"
        );
        assert!(create_run.join("keep.log").is_file());
        assert!(another_worker.join("keep.log").is_file());
        assert!(!profile_base.exists());
        assert!(backend.workers.lock().unwrap().is_empty());
        restarted.stop_worker(&worker.worker_ref, None).unwrap();
    }

    #[tokio::test]
    async fn profile_restore_artifact_failure_retains_failed_session_owner_for_retry() {
        let root = tempfile::tempdir().unwrap();
        let factory =
            ProfileRuntimeWorkerFactory::new(root.path()).with_runtime_store_dir(root.path());
        let worker_ref = WorkerRef::new(WorkerId::now_v7());
        let request = direct_restore_request(worker_ref.clone());
        let run = factory
            .worker_restore_run_dir(&worker_ref, request.operation_id)
            .unwrap();
        fs::create_dir_all(run.parent().unwrap()).unwrap();
        fs::write(&run, "invalid restore artifact").unwrap();
        let sessions = Arc::new(WorkdirSessionRouter::new());
        factory.failed_restore_sessions.lock().unwrap().insert(
            worker_ref.clone(),
            (request.operation_id, Arc::clone(&sessions)),
        );
        assert!(
            factory
                .cleanup_failed_restore(&request)
                .await
                .unwrap_err()
                .contains("unsafe restore artifact")
        );
        assert!(
            factory
                .reconcile_stopped_worker(&worker_ref)
                .await
                .unwrap_err()
                .contains("unsafe restore artifact")
        );
        let retained = factory
            .failed_restore_sessions
            .lock()
            .unwrap()
            .get(&worker_ref)
            .cloned()
            .unwrap();
        assert_eq!(retained.0, request.operation_id);
        assert!(Arc::ptr_eq(&retained.1, &sessions));
        fs::remove_file(&run).unwrap();
        fs::create_dir(&run).unwrap();
        fs::write(run.join("worker.out.log"), "stale restore artifact").unwrap();
        factory.reconcile_stopped_worker(&worker_ref).await.unwrap();
        assert!(!run.exists());
        assert!(factory.failed_restore_sessions.lock().unwrap().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn stopped_restore_artifact_cleanup_rejects_symlinks_and_noncanonical_names() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("keep"), "outside authority").unwrap();
        let factory =
            ProfileRuntimeWorkerFactory::new(root.path()).with_runtime_store_dir(root.path());
        let worker_ref = WorkerRef::new(WorkerId::now_v7());
        let run = factory
            .worker_restore_run_dir(
                &worker_ref,
                crate::execution::WorkerLifecycleOperationId::new(),
            )
            .unwrap();
        let runs = run.parent().unwrap();
        fs::create_dir_all(runs).unwrap();
        symlink(outside.path(), &run).unwrap();
        assert!(
            factory
                .stopped_restore_run_dirs(&worker_ref)
                .unwrap_err()
                .contains("unsafe restore artifact")
        );
        fs::remove_file(&run).unwrap();
        fs::create_dir(&run).unwrap();
        symlink(outside.path(), run.join("escaped")).unwrap();
        assert!(factory.stopped_restore_run_dirs(&worker_ref).is_err());
        assert!(run.exists());
        fs::remove_file(run.join("escaped")).unwrap();
        let invalid = runs.join("restore-not-an-operation");
        fs::create_dir(&invalid).unwrap();
        assert!(
            factory
                .stopped_restore_run_dirs(&worker_ref)
                .unwrap_err()
                .contains("invalid restore artifact operation name")
        );
        fs::remove_dir(&invalid).unwrap();
        let noncanonical = runs.join("restore-550E8400-E29B-41D4-A716-446655440000");
        fs::create_dir(&noncanonical).unwrap();
        assert!(
            factory
                .stopped_restore_run_dirs(&worker_ref)
                .unwrap_err()
                .contains("noncanonical restore artifact operation name")
        );
        fs::remove_dir(&noncanonical).unwrap();
        fs::remove_dir(&run).unwrap();
        fs::remove_dir(runs).unwrap();
        symlink(outside.path(), runs).unwrap();
        assert!(
            factory
                .stopped_restore_run_dirs(&worker_ref)
                .unwrap_err()
                .contains("unsafe restore artifact directory")
        );
        assert_eq!(
            fs::read_to_string(outside.path().join("keep")).unwrap(),
            "outside authority"
        );
    }

    struct PendingFailureFactory {
        restore_calls: Arc<AtomicUsize>,
        cleanup_calls: Arc<AtomicUsize>,
        cleanup_allowed: Arc<AtomicBool>,
    }

    #[async_trait]
    impl RuntimeWorkerFactory for PendingFailureFactory {
        async fn spawn_controller(
            &self,
            _request: WorkerExecutionSpawnRequest,
        ) -> Result<RuntimeWorkerController, String> {
            panic!("stop must not spawn a Controller")
        }
        async fn restore_controller(
            &self,
            _request: WorkerExecutionRestoreRequest,
        ) -> Result<RuntimeWorkerController, String> {
            self.restore_calls.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(50)).await;
            Err("injected uncertain restore side effect".to_string())
        }
        async fn cleanup_failed_restore(
            &self,
            _request: &WorkerExecutionRestoreRequest,
        ) -> Result<(), String> {
            self.cleanup_calls.fetch_add(1, Ordering::SeqCst);
            if self.cleanup_allowed.load(Ordering::SeqCst) {
                Ok(())
            } else {
                Err("injected pending restore cleanup failure".to_string())
            }
        }
    }

    fn direct_restore_request(worker_ref: WorkerRef) -> WorkerExecutionRestoreRequest {
        WorkerExecutionRestoreRequest {
            operation_id: crate::execution::WorkerLifecycleOperationId::new(),
            request: create_request("pending operation"),
            workspace_scope: None,
            context: test_execution_context(worker_ref.clone()),
            worker_ref,
            previous_workdir_attachments: Vec::new(),
            logical_workdir_attachments: Vec::new(),
            workdir_attachments: BTreeMap::new(),
            config_bundle: None,
        }
    }

    #[test]
    fn absent_execution_stop_reconciles_pending_restore_cleanup_without_launching() {
        let restore_calls = Arc::new(AtomicUsize::new(0));
        let cleanup_calls = Arc::new(AtomicUsize::new(0));
        let cleanup_allowed = Arc::new(AtomicBool::new(false));
        let backend = WorkerRuntimeExecutionBackend::new(PendingFailureFactory {
            restore_calls: Arc::clone(&restore_calls),
            cleanup_calls: Arc::clone(&cleanup_calls),
            cleanup_allowed: Arc::clone(&cleanup_allowed),
        })
        .unwrap()
        .with_spawn_restore_timeout(Duration::from_millis(1));
        let worker_ref = WorkerRef::new(WorkerId::now_v7());
        let request = direct_restore_request(worker_ref.clone());
        let operation_id = request.operation_id;
        assert!(matches!(
            backend.restore_worker(request.clone()),
            WorkerExecutionSpawnResult::ReconciliationRequired { .. }
        ));
        assert!(backend.workers.lock().unwrap().is_empty());
        assert_eq!(
            backend
                .pending_restore(&worker_ref)
                .unwrap()
                .unwrap()
                .request
                .operation_id,
            operation_id
        );
        // Same operation must await/reuse the original future, never start again.
        assert!(matches!(
            backend.reconcile_restore(request.clone()),
            WorkerExecutionSpawnResult::ReconciliationRequired { .. }
        ));
        let pending = backend.preflight_restore(&request).unwrap_err();
        assert_eq!(
            pending.outcome,
            crate::execution::WorkerExecutionOutcome::Busy
        );
        assert!(
            pending.worker_state.is_none(),
            "pending resources must not attest AlreadyConnected"
        );
        let stop = WorkerExecutionStopRequest {
            operation_id: crate::execution::WorkerLifecycleOperationId::new(),
            worker_ref: worker_ref.clone(),
        };
        let failed = backend.stop_worker_operation(stop.clone());
        assert_eq!(
            failed.outcome,
            crate::execution::WorkerExecutionOutcome::Errored
        );
        assert!(
            failed
                .message
                .unwrap()
                .contains("pending restore cleanup failure")
        );
        assert_eq!(
            backend
                .pending_restore(&worker_ref)
                .unwrap()
                .unwrap()
                .request
                .operation_id,
            operation_id
        );
        cleanup_allowed.store(true, Ordering::SeqCst);
        assert!(backend.stop_worker_operation(stop.clone()).is_accepted());
        assert!(backend.pending_restore(&worker_ref).unwrap().is_none());
        assert!(
            backend.stop_worker_operation(stop).is_accepted(),
            "cleanup then catalog-commit retry must converge"
        );
        assert_eq!(restore_calls.load(Ordering::SeqCst), 1);
        assert_eq!(cleanup_calls.load(Ordering::SeqCst), 2);
    }

    struct LateControllerFactory {
        inner: MockFactory,
        restores: Arc<AtomicUsize>,
        release: Arc<tokio::sync::Notify>,
        actual_handles: Arc<Mutex<Vec<WorkerHandle>>>,
    }

    #[async_trait]
    impl RuntimeWorkerFactory for LateControllerFactory {
        async fn spawn_controller(
            &self,
            request: WorkerExecutionSpawnRequest,
        ) -> Result<RuntimeWorkerController, String> {
            self.inner.spawn_controller(request).await
        }
        async fn restore_controller(
            &self,
            request: WorkerExecutionRestoreRequest,
        ) -> Result<RuntimeWorkerController, String> {
            let first = self.restores.fetch_add(1, Ordering::SeqCst) == 0;
            if first {
                self.release.notified().await;
            }
            let controller = self.inner.restore_controller(request).await?;
            self.actual_handles
                .lock()
                .unwrap()
                .push(controller.handle.clone());
            Ok(controller)
        }
    }

    #[test]
    fn stop_joins_late_restore_controller_and_old_methods_do_not_follow_new_execution() {
        let root = tempfile::tempdir().unwrap();
        let restores = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(tokio::sync::Notify::new());
        let actual_handles = Arc::new(Mutex::new(Vec::new()));
        let backend = WorkerRuntimeExecutionBackend::new(LateControllerFactory {
            inner: MockFactory {
                client: MockClient::sequential(vec![]),
                runtime_base: root.path().join("runtime"),
                cwd: root.path().to_path_buf(),
                store_dir: root.path().join("sessions"),
                worker_metadata_dir: root.path().join("workers"),
                observed_cwds: Arc::new(Mutex::new(Vec::new())),
                observed_workspace_clients: Arc::new(Mutex::new(Vec::new())),
            },
            restores: Arc::clone(&restores),
            release: Arc::clone(&release),
            actual_handles: Arc::clone(&actual_handles),
        })
        .unwrap()
        .with_spawn_restore_timeout(Duration::from_millis(1));
        let worker_ref = WorkerRef::new(WorkerId::now_v7());
        assert!(matches!(
            backend.restore_worker(direct_restore_request(worker_ref.clone())),
            WorkerExecutionSpawnResult::ReconciliationRequired { .. }
        ));
        assert!(backend.workers.lock().unwrap().is_empty());
        release.notify_one();
        assert!(backend.stop_worker(&worker_ref).is_accepted());
        assert_eq!(
            restores.load(Ordering::SeqCst),
            1,
            "stop must await, not relaunch"
        );
        assert!(backend.workers.lock().unwrap().is_empty());
        assert!(backend.pending_restore(&worker_ref).unwrap().is_none());
        #[cfg(feature = "ws-server")]
        assert!(backend.worker_snapshot(&worker_ref).is_none());
        let old_handle = actual_handles.lock().unwrap()[0].clone();
        // Restore the same identity, but long-lived consumers keep the actual
        // old Controller transport rather than acquiring a WorkerRef token.
        let mut backend = backend;
        backend.spawn_restore_timeout = Duration::from_secs(5);
        assert!(matches!(
            backend.restore_worker(direct_restore_request(worker_ref.clone())),
            WorkerExecutionSpawnResult::Connected { .. }
        ));
        let old_send = backend.run_on_adapter_runtime(async move {
            old_handle
                .send(Method::ListRewindTargets)
                .await
                .map_err(|error| error.to_string())
        });
        assert!(
            old_send.is_err(),
            "old protocol transport must stay closed after restore"
        );
        assert_eq!(restores.load(Ordering::SeqCst), 2);
        assert!(backend.stop_worker(&worker_ref).is_accepted());
        #[cfg(feature = "ws-server")]
        assert!(backend.worker_snapshot(&worker_ref).is_none());
    }

    #[test]
    fn absent_execution_has_typed_input_rejection_and_safe_stop() {
        let backend = WorkerRuntimeExecutionBackend::new(FailingFactory).unwrap();
        let worker_ref = WorkerRef::new(WorkerId::now_v7());
        let rejected = backend.dispatch_input(&worker_ref, WorkerInput::user("input"));
        assert_eq!(rejected.operation, WorkerExecutionOperation::Input);
        assert_eq!(
            rejected.outcome,
            crate::execution::WorkerExecutionOutcome::Rejected
        );
        assert!(rejected.message.unwrap().contains("live Worker execution"));
        assert!(backend.stop_worker(&worker_ref).is_accepted());
        assert!(backend.workers.lock().unwrap().is_empty());
    }

    #[test]
    fn poisoned_pending_resources_are_not_absence_evidence_for_stop() {
        let backend = WorkerRuntimeExecutionBackend::new(FailingFactory).unwrap();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = backend.pending_restores.lock().unwrap();
            panic!("injected resource registry poison");
        }));
        let stopped = backend.stop_worker(&WorkerRef::new(WorkerId::now_v7()));
        assert_eq!(
            stopped.outcome,
            crate::execution::WorkerExecutionOutcome::Errored
        );
        assert!(
            stopped
                .message
                .unwrap()
                .contains("pending restore resources lock is poisoned")
        );
    }

    #[test]
    fn adapter_runtime_reports_task_panic() {
        let backend = WorkerRuntimeExecutionBackend::new(FailingFactory).unwrap();

        let error = backend
            .run_on_adapter_runtime(async {
                panic!("adapter boom");
                #[allow(unreachable_code)]
                Ok::<(), String>(())
            })
            .unwrap_err();

        assert!(error.contains("worker adapter task failed"));
        assert!(error.contains("adapter boom") || error.contains("panicked"));
    }

    fn create_clean_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init"]);
        git(
            dir.path(),
            &["config", "user.email", "test@example.invalid"],
        );
        git(dir.path(), &["config", "user.name", "Yoi Test"]);
        fs::write(dir.path().join("README.md"), "clean\n").unwrap();
        git(dir.path(), &["add", "README.md"]);
        git(dir.path(), &["commit", "-m", "init"]);
        dir
    }

    fn working_directory_request(repo: &std::path::Path) -> WorkingDirectoryRequest {
        WorkingDirectoryRequest {
            repository: WorkingDirectoryRepository {
                id: "repo-main".to_string(),
                provider: "git".to_string(),
                source: server_api::RepositorySource {
                    kind: server_api::RepositorySourceKind::LocalPath,
                    uri: repo.display().to_string(),
                },
                source_revision: 1,
                source_fingerprint: "sha256:test".to_string(),
                selector: Some(RepositorySelector::from("HEAD")),
            },
            display_name: None,
            materializer: MaterializerKind::RuntimeGitClone,
            backend_workdir_id: None,
            materialization: None,
        }
    }

    fn materialized_clone_root(
        runtime_base: &std::path::Path,
        working_directory_id: &str,
    ) -> PathBuf {
        runtime_base.join(working_directory_id).join("checkout")
    }

    #[tokio::test]
    async fn runtime_provider_projects_only_explicit_live_canonical_grants() {
        let hub = Arc::new(RuntimeWorkerObservationHub::default());
        let worker_id = crate::identity::WorkerId::from_legacy_u64(7);
        let worker_ref = WorkerRef::new(worker_id);
        let shared_state = Arc::new(WorkerSharedState::new(
            "peer-worker".to_string(),
            session_store::new_segment_id(),
            "[worker]\nname = \"peer-worker\"".to_string(),
            protocol::Greeting {
                worker_name: "peer-worker".to_string(),
                cwd: "/tmp".to_string(),
                provider: "test".to_string(),
                model: "test".to_string(),
                scope_summary: String::new(),
                tools: Vec::new(),
                context_window: 1_000,
                context_tokens: 0,
                reasoning: None,
                context_usage: None,
            },
        ));
        hub.workers.lock().unwrap().insert(
            worker_ref,
            RuntimeObservedWorker {
                workspace_id: Some("workspace-1".to_string()),
                shared_state: Arc::downgrade(&shared_state),
                sink: SegmentLogSink::new(),
            },
        );
        let grant = crate::identity::RuntimeWorkerRef::new("runtime-1", worker_id.to_string());
        let provider = RuntimeGrantedWorkerObservationProvider {
            runtime_id: "runtime-1".to_string(),
            workspace_id: "workspace-1".to_string(),
            grants: std::collections::HashSet::from([grant.clone()]),
            hub: hub.clone(),
        };

        let listed = provider.list_worker_sessions().await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(
            listed[0].subject,
            WorkerObservationSubjectRef::RuntimeWorker {
                runtime_id: "runtime-1".to_string(),
                worker_id: worker_id.to_string(),
            }
        );
        provider
            .capture_worker_session(&listed[0].subject)
            .await
            .expect("granted live peer should be capturable");
        let hidden = provider
            .capture_worker_session(&WorkerObservationSubjectRef::RuntimeWorker {
                runtime_id: "runtime-1".to_string(),
                worker_id: "8".to_string(),
            })
            .await
            .unwrap_err();
        assert!(matches!(hidden, WorkerObservationError::NotFound));

        let cross_workspace = RuntimeGrantedWorkerObservationProvider {
            runtime_id: "runtime-1".to_string(),
            workspace_id: "workspace-2".to_string(),
            grants: std::collections::HashSet::from([grant]),
            hub: hub.clone(),
        };
        assert!(
            cross_workspace
                .list_worker_sessions()
                .await
                .unwrap()
                .is_empty()
        );
        let hidden = cross_workspace
            .capture_worker_session(&listed[0].subject)
            .await
            .unwrap_err();
        assert!(matches!(hidden, WorkerObservationError::NotFound));

        drop(shared_state);
        assert!(provider.list_worker_sessions().await.unwrap().is_empty());
    }

    #[test]
    fn runtime_worker_name_uses_workspace_worker_identity() {
        let worker_ref =
            crate::identity::WorkerRef::new(crate::identity::WorkerId::from_legacy_u64(1));
        let request = WorkerExecutionSpawnRequest {
            worker_ref: worker_ref.clone(),
            request: create_request("1"),
            workspace_scope: None,
            context: test_execution_context(worker_ref),
            workdir_attachments: BTreeMap::new(),
            config_bundle: None,
        };

        assert_eq!(
            ProfileRuntimeWorkerFactory::runtime_worker_name(&request),
            format!("worker-runtime-{}", request.worker_ref.worker_id)
        );
        assert_ne!(
            ProfileRuntimeWorkerFactory::runtime_worker_name(&request),
            "00000001"
        );
    }

    #[test]
    fn initial_claim_capabilities_override_full_materialization_default() {
        let checkout = WorkdirAttachmentAlias::new("checkout").unwrap();
        let docs = WorkdirAttachmentAlias::new("docs").unwrap();
        let claims = [WorkingDirectoryAttachmentClaim {
            alias: docs.clone(),
            working_directory_id: "workdir-docs".to_string(),
            relative_cwd: None,
            capabilities: WorkdirSessionCapabilities::READ_ONLY,
        }];

        assert_eq!(
            initial_workdir_session_capabilities(&docs, &claims),
            WorkdirSessionCapabilities::READ_ONLY
        );
        assert_eq!(
            initial_workdir_session_capabilities(&checkout, &claims),
            WorkdirSessionCapabilities::ALL
        );
    }

    #[test]
    fn restore_opens_a_fresh_session_with_the_same_read_only_capabilities() {
        let root = tempfile::tempdir().unwrap();
        let spawned_scope = manifest::SharedScope::new(Scope::writable(root.path()).unwrap());
        let spawned = runtime_local_workdir_session(
            "working-directory-42",
            root.path(),
            root.path(),
            spawned_scope.clone(),
            spawned_scope,
            WorkdirSessionCapabilities::READ_ONLY,
            Default::default(),
            Vec::new(),
        );
        let restored_scope = manifest::SharedScope::new(Scope::writable(root.path()).unwrap());
        let restored = runtime_local_workdir_session(
            "working-directory-42",
            root.path(),
            root.path(),
            restored_scope.clone(),
            restored_scope,
            WorkdirSessionCapabilities::READ_ONLY,
            Default::default(),
            Vec::new(),
        );

        assert_eq!(spawned.workdir().id().as_str(), "working-directory-42");
        assert_eq!(restored.workdir().id().as_str(), "working-directory-42");
        assert_eq!(
            spawned.capabilities(),
            WorkdirSessionCapabilities::READ_ONLY
        );
        assert_eq!(
            restored.capabilities(),
            WorkdirSessionCapabilities::READ_ONLY
        );
        assert!(!Arc::ptr_eq(&spawned, &restored));
    }

    #[tokio::test]
    async fn embedded_profile_source_archive_does_not_require_backend_resource_fetch() {
        let factory = ProfileRuntimeWorkerFactory::new(tempfile::tempdir().unwrap().path());
        let bundle = test_bundle();
        let source = crate::catalog::ProfileSourceArchiveSource::Embedded {
            archive: bundle.profile_source_archive.clone().unwrap(),
        };
        factory
            .resolve_profile_source_archive(&source, None)
            .await
            .expect("embedded archive should resolve without Backend resource client");
    }

    #[test]
    fn pending_restore_launch_material_preserves_workspace_prompt_catalog() {
        let root = tempfile::tempdir().unwrap();
        let factory = ProfileRuntimeWorkerFactory::new(root.path());
        let builtins = worker::PromptCatalog::builtins_only().unwrap();
        let projection = builtins.projection();
        let mut templates = projection.templates.clone();
        templates.insert(
            "internal.notify_wrapper".to_string(),
            "PENDING-LAUNCH {{ message }}".to_string(),
        );
        let mut effective = worker::EffectivePromptCatalog::new(
            templates,
            7,
            projection.schema_fingerprint.clone(),
            projection.toolchain_fingerprint.clone(),
        )
        .unwrap();
        effective.source_digest = "source-7".to_string();
        let mut bundle = test_bundle();
        bundle.metadata.workspace_id = "workspace-restore".to_string();
        bundle.prompt_catalog = Some(effective);
        bundle = bundle.with_computed_digest();

        let resolution = factory
            .observe_bundle_prompt_projection(&bundle, Some("workspace-restore"))
            .unwrap()
            .unwrap();

        assert_eq!(resolution.projection.config_revision, 7);
        assert_eq!(
            resolution.catalog.notify_wrapper("restored").unwrap(),
            "PENDING-LAUNCH restored"
        );
    }

    #[tokio::test]
    #[serial_test::serial(worker_allocation)]
    async fn restore_legacy_workspace_worker_without_manifest_snapshot_requires_replacement() {
        let root = tempfile::tempdir().unwrap();
        let runtime_store_dir = root.path().join("runtime");
        let worker_ref = WorkerRef::new(crate::identity::WorkerId::from_legacy_u64(1));
        let worker_aggregate_dir = runtime_store_dir
            .join("workers")
            .join(worker_ref.worker_id.to_string());
        WorkerSessionStore::new(worker_aggregate_dir.join("session")).unwrap();
        let worker_name = ProfileRuntimeWorkerFactory::runtime_worker_name_for_ref(&worker_ref);
        let session_id = session_store::new_session_id();
        WorkerAggregateStore::new(&worker_aggregate_dir, &worker_name)
            .unwrap()
            .set_active(
                &worker_name,
                Some(session_store::WorkerActiveSegmentRef::pending_segment(
                    session_id,
                )),
                None,
            )
            .unwrap();

        let mut request = create_request("restore");
        request.workspace_api = Some(crate::catalog::WorkspaceApiRef {
            workspace_id: "workspace-restore".to_string(),
            base_url: "http://workspace.invalid".to_string(),
        });
        request.memory_settings = Some(manifest::WorkspaceMemorySettingsSnapshot {
            workspace_id: "workspace-restore".to_string(),
            settings_revision: 1,
            language: "English".to_string(),
        });
        let identity = RuntimeIdentityMaterial::generate("runtime-restore").unwrap();
        let error = match ProfileRuntimeWorkerFactory::new(root.path())
            .with_runtime_store_dir(&runtime_store_dir)
            .with_remote_worker_mutation_identity(identity)
            .restore_controller(WorkerExecutionRestoreRequest {
                operation_id: crate::execution::WorkerLifecycleOperationId::new(),
                worker_ref: worker_ref.clone(),
                request,
                workspace_scope: Some(crate::runtime::RuntimeWorkspaceScope::new(
                    "workspace-restore",
                    "server-main",
                )),
                context: test_execution_context(worker_ref),
                previous_workdir_attachments: Vec::new(),
                logical_workdir_attachments: Vec::new(),
                workdir_attachments: BTreeMap::new(),
                config_bundle: None,
            })
            .await
        {
            Ok(_) => panic!("legacy Workspace Worker restore unexpectedly succeeded"),
            Err(error) => error,
        };
        assert!(error.contains("replacement Worker is required"), "{error}");
    }

    #[test]
    fn profile_runtime_factory_defaults_to_in_process_controller_transport() {
        let root = tempfile::tempdir().unwrap();
        let factory = ProfileRuntimeWorkerFactory::new(root.path());

        assert!(matches!(
            factory.controller_transport,
            WorkerControllerTransport::InProcess
        ));
    }

    #[tokio::test]
    #[serial_test::serial(worker_allocation)]
    async fn in_process_restore_does_not_bind_unix_socket_under_overlong_store_path() {
        let root = tempfile::tempdir().unwrap();
        let long_component = "embedded-workspace-store-segment".repeat(4);
        let runtime_store_dir = root.path().join(long_component);
        let worker_ref = WorkerRef::new(crate::identity::WorkerId::now_v7());
        let worker_aggregate_dir = runtime_store_dir
            .join("workers")
            .join(worker_ref.worker_id.to_string());
        WorkerSessionStore::new(worker_aggregate_dir.join("session")).unwrap();
        let worker_name = ProfileRuntimeWorkerFactory::runtime_worker_name_for_ref(&worker_ref);
        let session_id = session_store::new_session_id();
        let manifest = manifest::WorkerManifest::from_toml(&format!(
            r#"
                [worker]
                name = "{}"
                pwd = "{}"

                [model]
                scheme = "anthropic"
                model_id = "test-model"
                auth = {{ kind = "none" }}

                [engine]
                max_tokens = 100

                [[scope.allow]]
                target = "{}"
                permission = "write"
            "#,
            worker_name,
            root.path().display(),
            root.path().display(),
        ))
        .unwrap();
        WorkerAggregateStore::new(&worker_aggregate_dir, &worker_name)
            .unwrap()
            .set_active(
                &worker_name,
                Some(session_store::WorkerActiveSegmentRef::pending_segment(
                    session_id,
                )),
                Some(manifest::write_persisted_worker_manifest_snapshot(&manifest).unwrap()),
            )
            .unwrap();

        let runs_dir = runtime_store_dir
            .join("workers")
            .join(worker_ref.worker_id.to_string())
            .join("runs");
        let socket_path = runs_dir
            .join(uuid::Uuid::nil().to_string())
            .join("worker.sock");
        assert!(
            socket_path.as_os_str().as_encoded_bytes().len() > 107,
            "test path must exceed Linux sockaddr_un.sun_path capacity: {}",
            socket_path.display()
        );

        let controller = ProfileRuntimeWorkerFactory::new(root.path())
            .with_runtime_store_dir(&runtime_store_dir)
            .with_controller_transport(WorkerControllerTransport::InProcess)
            .restore_controller(WorkerExecutionRestoreRequest {
                operation_id: crate::execution::WorkerLifecycleOperationId::new(),
                worker_ref: worker_ref.clone(),
                request: create_request("embedded restore"),
                workspace_scope: None,
                context: test_execution_context(worker_ref),
                previous_workdir_attachments: Vec::new(),
                logical_workdir_attachments: Vec::new(),
                workdir_attachments: BTreeMap::new(),
                config_bundle: None,
            })
            .await
            .expect("in-process restore must not bind the overlong Unix socket path");

        assert_eq!(
            controller.handle.shared_state.catalog_status(),
            WorkerStatus::Idle
        );
        let run_dir = std::fs::read_dir(&runs_dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .next()
            .expect("restore must create one run artifact directory");
        assert!(!run_dir.join("worker.sock").exists());
        assert!(run_dir.join("worker.out.log").is_file());
        assert!(run_dir.join("worker.err.log").is_file());
        let worker_state = Arc::new(RwLock::new(controller.handle.shared_state.snapshot()));
        controller
            .handle
            .send(Method::Shutdown {
                command: next_internal_command(&worker_state).unwrap(),
            })
            .await
            .unwrap();
        if let Some(receiver) = controller.shutdown.lock().await.take() {
            tokio::time::timeout(Duration::from_secs(5), receiver)
                .await
                .expect("controller shutdown signal timed out")
                .unwrap();
        }
        tokio::time::timeout(Duration::from_secs(5), controller.controller_task)
            .await
            .expect("controller task join timed out")
            .unwrap();
        assert!(!socket_path.exists());
    }

    #[test]
    fn profile_runtime_factory_uses_shared_worker_bootstrap_seams() {
        let source = include_str!("worker_backend.rs");
        let production = source
            .split_once("#[cfg(test)]\nmod tests")
            .map(|(production, _)| production)
            .expect("worker backend test module marker");
        let factory = production
            .split_once("impl RuntimeWorkerFactory for ProfileRuntimeWorkerFactory")
            .map(|(_, factory)| factory)
            .expect("profile runtime factory implementation");
        let (fresh, restore) = factory
            .split_once("async fn restore_controller")
            .expect("fresh and restore factory paths");
        let assert_in_order = |path: &str, markers: &[&str]| {
            let mut offset = 0;
            for marker in markers {
                let relative = path[offset..]
                    .find(marker)
                    .unwrap_or_else(|| panic!("missing ordered factory marker {marker}"));
                offset += relative + marker.len();
            }
        };
        assert_in_order(
            fresh,
            &[
                "WorkerBootstrap::new(",
                ".prepare()",
                "finalize_subjektiv_session_attribution(",
                "worker.bind_workdir_sessions(",
                "worker.bind_worker_observation_provider(",
                "install_runtime_flow_transition_feature()",
                "prepared.start()",
            ],
        );
        assert_in_order(
            restore,
            &[
                "Worker::restore_from_worker_metadata_with_context(",
                "finalize_subjektiv_session_attribution(",
                "worker.bind_workdir_sessions(",
                "worker.bind_worker_observation_provider(",
                "install_runtime_flow_transition_feature()",
                "PreparedWorker::new(",
                ".start()",
            ],
        );
        assert!(
            production.contains("WorkerBootstrap::new("),
            "fresh runtime Workers must use the shared construction bootstrap"
        );
        assert!(
            production.contains("PreparedWorker::new("),
            "restored runtime Workers must use the shared pre-exposure lifecycle"
        );
        assert!(
            !production.contains("WorkerController::spawn_runtime_managed_run_with_transport"),
            "runtime factory paths must not bypass the shared controller lifecycle"
        );
    }

    #[test]
    #[serial_test::serial(worker_allocation)]
    fn shared_bootstrap_preserves_in_process_transport_for_fresh_and_restored_runtime_workers() {
        let root = tempfile::tempdir().unwrap();
        let long_component = "embedded-workspace-store-segment".repeat(4);
        let runtime_store_dir = root.path().join(long_component);
        let runtime_options = crate::fs_store::FsRuntimeStoreOptions {
            root: runtime_store_dir.clone(),
            runtime_id: "test-runtime".to_string(),
            display_name: Some("embedded".to_string()),
        };

        let backend = Arc::new(
            WorkerRuntimeExecutionBackend::new(
                ProfileRuntimeWorkerFactory::new(root.path())
                    .with_runtime_store_dir(&runtime_store_dir),
            )
            .unwrap(),
        );
        let runtime = EmbeddedRuntime::with_fs_store_and_execution_backend(
            runtime_options.clone(),
            backend.clone(),
        )
        .unwrap();
        runtime.store_config_bundle(test_bundle()).unwrap();
        let mut request = create_request("embedded singleton");
        request.profile = ProfileSelector::Builtin("default".to_string());
        let worker = runtime.create_worker(request).unwrap();
        let runs_dir = runtime_store_dir
            .join("workers")
            .join(worker.worker_id.to_string())
            .join("runs");
        let first_run = std::fs::read_dir(&runs_dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .next()
            .expect("fresh launch must create one run artifact directory");
        let first_run_socket = first_run.join("worker.sock");
        assert!(
            first_run_socket.as_os_str().as_encoded_bytes().len() > 107,
            "test path must exceed Linux sockaddr_un.sun_path capacity: {}",
            first_run_socket.display()
        );
        assert!(!first_run_socket.exists());

        let worker_ref = worker.worker_ref.clone();
        assert!(backend.stop_worker(&worker_ref).is_accepted());
        drop(runtime);
        drop(backend);

        let restored_backend = Arc::new(
            WorkerRuntimeExecutionBackend::new(
                ProfileRuntimeWorkerFactory::new(root.path())
                    .with_runtime_store_dir(&runtime_store_dir),
            )
            .unwrap(),
        );
        let restored = EmbeddedRuntime::with_fs_store_and_execution_backend(
            runtime_options,
            restored_backend.clone(),
        )
        .expect("persisted in-process Worker must restore without binding its run path");
        let restored_worker = restored.worker_detail(&worker.worker_ref).unwrap();
        let diagnostics = restored.diagnostics().unwrap();
        assert_eq!(
            restored_worker.status,
            crate::catalog::WorkerStatus::Idle,
            "restore diagnostics: {diagnostics:#?}"
        );
        assert!(!diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "worker_execution_restore_failed"
                && diagnostic.worker_ref.as_ref() == Some(&worker.worker_ref)
        }));
        let restored_run = std::fs::read_dir(&runs_dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path != &first_run)
            .expect("restore must create a distinct run artifact directory");
        assert!(!restored_run.join("worker.sock").exists());
        assert!(restored_run.join("worker.out.log").is_file());
        assert!(restored_run.join("worker.err.log").is_file());

        restored
            .stop_worker(&worker.worker_ref, Some("test cleanup".to_string()))
            .unwrap();
        drop(restored);
        drop(restored_backend);
    }

    #[test]
    #[serial_test::serial(worker_allocation)]
    fn restore_reconciliation_reuses_the_operation_controller() {
        let client = MockClient::sequential(Vec::new());
        let runtime_base = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let observed_workspace_clients = Arc::new(Mutex::new(Vec::new()));
        let factory = MockFactory {
            client,
            runtime_base: runtime_base.path().to_path_buf(),
            cwd: cwd.path().to_path_buf(),
            store_dir: store.path().join("sessions"),
            worker_metadata_dir: store.path().join("workers"),
            observed_cwds: Arc::new(Mutex::new(Vec::new())),
            observed_workspace_clients: observed_workspace_clients.clone(),
        };
        let backend = Arc::new(WorkerRuntimeExecutionBackend::new(factory).unwrap());
        let runtime =
            EmbeddedRuntime::with_execution_backend(RuntimeOptions::default(), backend.clone())
                .unwrap();
        runtime.store_config_bundle(test_bundle()).unwrap();
        let request = create_request("idempotent restore reconciliation");
        let worker = runtime.create_worker(request.clone()).unwrap();
        runtime.stop_worker(&worker.worker_ref, None).unwrap();
        let operation_id = crate::execution::WorkerLifecycleOperationId::new();
        let restore_request = WorkerExecutionRestoreRequest {
            operation_id,
            worker_ref: worker.worker_ref.clone(),
            request,
            workspace_scope: None,
            context: test_execution_context(worker.worker_ref.clone()),
            previous_workdir_attachments: Vec::new(),
            logical_workdir_attachments: Vec::new(),
            workdir_attachments: BTreeMap::new(),
            config_bundle: None,
        };

        let first = backend.restore_worker(restore_request.clone());
        match first {
            WorkerExecutionSpawnResult::Connected { .. } => (),
            other => panic!("initial restore was not connected: {other:?}"),
        };
        let factory_calls_after_restore = observed_workspace_clients.lock().unwrap().len();

        let reconciled = backend.reconcile_restore(restore_request);
        assert!(matches!(
            reconciled,
            WorkerExecutionSpawnResult::Connected { .. }
        ));
        assert_eq!(
            observed_workspace_clients.lock().unwrap().len(),
            factory_calls_after_restore,
            "reconciliation must not construct a second controller"
        );
        let workers = backend.workers.lock().unwrap();
        let execution = workers.get(&worker.worker_ref).unwrap();
        assert_eq!(execution.restore_operation_id, Some(operation_id));
        assert_eq!(workers.len(), 1);
        drop(workers);

        assert!(backend.stop_worker(&worker.worker_ref).is_accepted());
    }

    #[test]
    fn profile_restore_run_directory_is_operation_scoped() {
        let root = tempfile::tempdir().unwrap();
        let runtime_store_dir = root.path().join("runtime");
        let factory = ProfileRuntimeWorkerFactory::new(root.path())
            .with_runtime_store_dir(&runtime_store_dir);
        let worker_ref = WorkerRef::new(WorkerId::now_v7());
        let operation_id = crate::execution::WorkerLifecycleOperationId::new();

        let first = factory
            .worker_restore_run_dir(&worker_ref, operation_id)
            .unwrap();
        let retry = factory
            .worker_restore_run_dir(&worker_ref, operation_id)
            .unwrap();
        let different = factory
            .worker_restore_run_dir(
                &worker_ref,
                crate::execution::WorkerLifecycleOperationId::new(),
            )
            .unwrap();

        assert_eq!(first, retry);
        assert_ne!(first, different);
        assert_eq!(
            first.file_name().unwrap().to_string_lossy(),
            format!("restore-{operation_id}")
        );
    }

    #[test]
    fn builtin_profile_selector_is_not_double_prefixed() {
        assert_eq!(
            ProfileRuntimeWorkerFactory::runtime_profile_value(
                &crate::catalog::ProfileSelector::Builtin("coder".to_string())
            )
            .as_ref(),
            "builtin:coder"
        );
        assert_eq!(
            ProfileRuntimeWorkerFactory::runtime_profile_value(
                &crate::catalog::ProfileSelector::Builtin("builtin:coder".to_string())
            )
            .as_ref(),
            "builtin:coder"
        );
    }

    #[test]
    fn running_worker_replays_retry_and_queues_a_new_submit() {
        let client = MockClient::sequential(vec![MockResponse::Hang(vec![])]);
        let runtime_base = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let factory = MockFactory {
            client,
            runtime_base: runtime_base.path().to_path_buf(),
            cwd: cwd.path().to_path_buf(),
            store_dir: store.path().join("sessions"),
            worker_metadata_dir: store.path().join("workers"),
            observed_cwds: Arc::new(Mutex::new(Vec::new())),
            observed_workspace_clients: Arc::new(Mutex::new(Vec::new())),
        };
        let backend = Arc::new(WorkerRuntimeExecutionBackend::new(factory).unwrap());
        let runtime =
            EmbeddedRuntime::with_execution_backend(RuntimeOptions::default(), backend).unwrap();
        runtime.store_config_bundle(test_bundle()).unwrap();
        let detail = runtime
            .create_worker(create_request("queued-submit"))
            .unwrap();

        let mut first_input = WorkerInput::user("first");
        first_input.submission_request_id = Some("request-first".into());
        let first = runtime
            .send_input(&detail.worker_ref, first_input.clone())
            .unwrap();
        assert_eq!(
            first.submission.as_ref().map(|ack| ack.disposition),
            Some(protocol::SubmissionDisposition::Started)
        );
        let retry = runtime.send_input(&detail.worker_ref, first_input).unwrap();
        assert_eq!(retry.submission, first.submission);
        let mut conflicting_retry = WorkerInput::user("different");
        conflicting_retry.submission_request_id = Some("request-first".into());
        assert!(
            runtime
                .send_input(&detail.worker_ref, conflicting_retry)
                .is_err(),
            "same request id with a different payload must fail"
        );

        let mut second_input = WorkerInput::user("second");
        second_input.submission_request_id = Some("request-second".into());
        let second = runtime
            .send_input(&detail.worker_ref, second_input.clone())
            .expect("a new Submit must queue while the Worker is running");
        assert_eq!(
            second.submission.as_ref().map(|ack| ack.disposition),
            Some(protocol::SubmissionDisposition::Queued)
        );
        let retry = runtime
            .send_input(&detail.worker_ref, second_input)
            .unwrap();
        assert_eq!(retry.submission, second.submission);
    }

    #[test]
    fn running_worker_waits_for_durable_notify_receipt() {
        let client = MockClient::sequential(vec![MockResponse::Hang(vec![])]);
        let runtime_base = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let factory = MockFactory {
            client,
            runtime_base: runtime_base.path().to_path_buf(),
            cwd: cwd.path().to_path_buf(),
            store_dir: store.path().join("sessions"),
            worker_metadata_dir: store.path().join("workers"),
            observed_cwds: Arc::new(Mutex::new(Vec::new())),
            observed_workspace_clients: Arc::new(Mutex::new(Vec::new())),
        };
        let backend = Arc::new(WorkerRuntimeExecutionBackend::new(factory).unwrap());
        let runtime =
            EmbeddedRuntime::with_execution_backend(RuntimeOptions::default(), backend.clone())
                .unwrap();
        runtime.store_config_bundle(test_bundle()).unwrap();
        let detail = runtime
            .create_worker(create_request("running-notify"))
            .unwrap();

        runtime
            .send_input(&detail.worker_ref, WorkerInput::user("first"))
            .unwrap();
        wait_for_adapter_state(&backend, &detail.worker_ref, WorkerStatus::Running);

        let mut notification = WorkerInput::notify("advisory context");
        notification.submission_request_id = Some("notification-request".into());
        let acknowledgement = runtime
            .send_input(&detail.worker_ref, notification)
            .expect("Running Worker must accept Notify without a Submit receipt");

        assert!(acknowledgement.submission.is_none());
        assert_eq!(
            acknowledgement
                .notification
                .as_ref()
                .map(|ack| ack.notification_request_id.as_str()),
            Some("notification-request")
        );
        wait_for_adapter_state(&backend, &detail.worker_ref, WorkerStatus::Running);
    }

    #[test]
    fn create_with_initial_input_returns_after_durable_submission_acceptance() {
        let client = MockClient::new(simple_text_events());
        let runtime_base = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let factory = MockFactory {
            client,
            runtime_base: runtime_base.path().to_path_buf(),
            cwd: cwd.path().to_path_buf(),
            store_dir: store.path().join("sessions"),
            worker_metadata_dir: store.path().join("workers"),
            observed_cwds: Arc::new(Mutex::new(Vec::new())),
            observed_workspace_clients: Arc::new(Mutex::new(Vec::new())),
        };
        let backend = Arc::new(WorkerRuntimeExecutionBackend::new(factory).unwrap());
        let runtime =
            EmbeddedRuntime::with_execution_backend(RuntimeOptions::default(), backend.clone())
                .unwrap();
        runtime.store_config_bundle(test_bundle()).unwrap();
        let mut request = create_request("initial-commit");
        request.initial_input = Some(WorkerInput::user("start the ticket"));

        let detail = runtime.create_worker(request).unwrap();

        let handle = backend
            .workers
            .lock()
            .unwrap()
            .get(&detail.worker_ref)
            .expect("live Worker execution")
            .handle
            .clone();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let entries = loop {
            let entries = handle.committed_entries();
            if entries.iter().any(|entry| {
                matches!(
                    entry,
                    LogEntry::AnnotatedUserInput { segments, .. }
                        if segments == &vec![Segment::text("start the ticket")]
                )
            }) {
                break entries;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "durably accepted initial input must eventually commit to history"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        let submission_id = entries
            .iter()
            .find_map(|entry| {
                let extensions = match entry {
                    LogEntry::AnnotatedUserInput { extensions, .. } => extensions,
                    _ => return None,
                };
                extensions
                    .iter()
                    .find(|extension| extension.domain == "worker.pending_activations.v1")
                    .and_then(|extension| {
                        extension.payload["receipts"][0]["submission_id"].as_str()
                    })
            })
            .expect("committed input submission id");
        uuid::Uuid::parse_str(submission_id).expect("opaque submission id is a UUID");
    }

    #[cfg(feature = "ws-server")]
    #[test]
    fn adapter_dispatches_user_input_through_worker_run_lifecycle() {
        let client = MockClient::new(simple_text_events());
        let runtime_base = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let observed_cwds = Arc::new(Mutex::new(Vec::new()));
        let observed_workspace_clients = Arc::new(Mutex::new(Vec::new()));
        let factory = MockFactory {
            client: client.clone(),
            runtime_base: runtime_base.path().to_path_buf(),
            cwd: cwd.path().to_path_buf(),
            store_dir: store.path().join("sessions"),
            worker_metadata_dir: store.path().join("workers"),
            observed_cwds: observed_cwds.clone(),
            observed_workspace_clients: observed_workspace_clients.clone(),
        };
        let backend = WorkerRuntimeExecutionBackend::new(factory).unwrap();
        let runtime =
            EmbeddedRuntime::with_execution_backend(RuntimeOptions::default(), Arc::new(backend))
                .unwrap();
        runtime.store_config_bundle(test_bundle()).unwrap();
        let mut request = create_request("chat");
        request.workspace_api = Some(crate::catalog::WorkspaceApiRef {
            workspace_id: "ws-test".to_string(),
            base_url: "http://127.0.0.1:3999".to_string(),
        });
        let detail = runtime.create_worker(request).unwrap();

        runtime
            .send_input(&detail.worker_ref, WorkerInput::user("say hello"))
            .unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let observations = runtime
                .read_worker_observation_events(&detail.worker_ref, WorkerObservationCursor::zero())
                .unwrap();
            if observations.iter().any(|event| {
                matches!(
                    &event.payload,
                    protocol::Event::TextDone { text } if text == "hello from worker"
                )
            }) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for assistant protocol observation"
            );
            std::thread::sleep(Duration::from_millis(20));
        }

        assert_eq!(client.captured.lock().unwrap().len(), 1);
        assert!(observed_cwds.lock().unwrap().is_empty());
        assert_eq!(
            observed_workspace_clients.lock().unwrap().as_slice(),
            &[(
                "runtime-owned-workspace-client".to_string(),
                Some("ws-test".to_string()),
                true,
            )]
        );
        let names = captured_tool_names(&client, 0);
        for expected in core_filesystem_tool_names() {
            assert!(
                names.contains(expected),
                "no-workdir Worker did not expose stable routed tool {expected}; tools={names:?}"
            );
        }
        let observations = runtime
            .read_worker_observation_events(&detail.worker_ref, WorkerObservationCursor::zero())
            .unwrap();
        assert!(
            observations
                .iter()
                .any(|event| matches!(event.payload, protocol::Event::TextDone { .. }))
        );
    }

    #[test]
    fn worker_spawn_receives_materialized_workspace_cwd_instead_of_source_repo() {
        let client = MockClient::new(simple_text_events());
        let runtime_base = tempfile::tempdir().unwrap();
        let repo = create_clean_repo();
        let store = tempfile::tempdir().unwrap();
        let observed_cwds = Arc::new(Mutex::new(Vec::new()));
        let observed_workspace_clients = Arc::new(Mutex::new(Vec::new()));
        let factory = MockFactory {
            client: client.clone(),
            runtime_base: runtime_base.path().to_path_buf(),
            cwd: repo.path().to_path_buf(),
            store_dir: store.path().join("sessions"),
            worker_metadata_dir: store.path().join("workers"),
            observed_cwds: observed_cwds.clone(),
            observed_workspace_clients: observed_workspace_clients.clone(),
        };
        let backend = WorkerRuntimeExecutionBackend::new(factory)
            .unwrap()
            .with_working_directory_materializer(RuntimeGitMaterializer::new(runtime_base.path()));
        let runtime =
            EmbeddedRuntime::with_execution_backend(RuntimeOptions::default(), Arc::new(backend))
                .unwrap();
        runtime.store_config_bundle(test_bundle()).unwrap();
        let mut request = create_request("chat");
        request.workdir_attachment_requests = vec![WorkingDirectoryAttachmentRequest {
            alias: workdir::WorkdirAttachmentAlias::new("checkout").unwrap(),
            working_directory: working_directory_request(repo.path()),
        }];

        let detail = runtime.create_worker(request).unwrap();
        assert!(
            !repo.path().join(".yoi").exists(),
            "Worker launch must not create repository-local authority"
        );
        runtime
            .send_input(&detail.worker_ref, WorkerInput::user("inspect tools"))
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while client.captured.lock().unwrap().is_empty() {
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for materialized-worker request"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let names = captured_tool_names(&client, 0);
        for expected in core_filesystem_tool_names() {
            assert!(
                names.contains(expected),
                "local Worker did not expose {expected}; tools={names:?}"
            );
        }

        assert_eq!(detail.workdir_attachments.len(), 1);
        let cwds = observed_cwds.lock().unwrap();
        assert_eq!(cwds.len(), 1);
        let cwd = &cwds[0];
        assert!(cwd.starts_with(runtime_base.path()));
        assert!(!cwd.starts_with(repo.path()));
        assert!(cwd.join("README.md").exists());
        assert_eq!(
            observed_workspace_clients.lock().unwrap().as_slice(),
            &[("unavailable".to_string(), None, false)]
        );

        let detached = runtime
            .replace_worker_workdir_attachments(&detail.worker_ref, Vec::new())
            .unwrap();
        assert!(detached.workdir_attachments.is_empty());
        runtime.stop_worker(&detail.worker_ref, None).unwrap();
        let restored_without_workdir = runtime
            .restore_worker(
                &detail.worker_ref,
                runtime.test_restore_request(&detail.worker_ref),
            )
            .unwrap();
        assert!(restored_without_workdir.workdir_attachments.is_empty());
        assert!(
            !repo.path().join(".yoi").exists(),
            "Worker restore must not create repository-local authority"
        );
    }

    #[test]
    #[cfg(feature = "ws-server")]
    #[serial_test::serial(worker_allocation)]
    fn adapter_resumes_paused_turn_once_and_preserves_idle_not_paused_error() {
        let hanging_events = || simple_text_events().into_iter().take(2).collect::<Vec<_>>();
        let client = MockClient::sequential(vec![
            MockResponse::Hang(hanging_events()),
            MockResponse::Hang(hanging_events()),
            MockResponse::Complete(simple_text_events()),
        ]);
        let call_count = client.call_count.clone();
        let runtime_base = tempfile::tempdir().unwrap();
        let repo = create_clean_repo();
        let store = tempfile::tempdir().unwrap();
        let factory = MockFactory {
            client,
            runtime_base: runtime_base.path().to_path_buf(),
            cwd: repo.path().to_path_buf(),
            store_dir: store.path().join("sessions"),
            worker_metadata_dir: store.path().join("workers"),
            observed_cwds: Arc::new(Mutex::new(Vec::new())),
            observed_workspace_clients: Arc::new(Mutex::new(Vec::new())),
        };
        let backend = Arc::new(WorkerRuntimeExecutionBackend::new(factory).unwrap());
        let runtime =
            EmbeddedRuntime::with_execution_backend(RuntimeOptions::default(), backend.clone())
                .unwrap();
        runtime.store_config_bundle(test_bundle()).unwrap();
        let detail = runtime
            .create_worker(create_request("paused-resume"))
            .unwrap();

        runtime
            .send_input(&detail.worker_ref, WorkerInput::user("pause and resume"))
            .expect("start initial turn");
        wait_for_adapter_state(&backend, &detail.worker_ref, WorkerStatus::Running);

        let running_resume = adapter_command(&backend, &detail.worker_ref);
        runtime
            .send_protocol_method(
                &detail.worker_ref,
                Method::Resume {
                    command: running_resume,
                },
            )
            .expect("running Resume is forwarded for controller admission");
        wait_for_adapter_command(&backend, &detail.worker_ref, running_resume.command_id);

        runtime
            .send_protocol_method(
                &detail.worker_ref,
                Method::Pause {
                    command: adapter_command(&backend, &detail.worker_ref),
                },
            )
            .expect("pause initial turn");
        wait_for_adapter_state(&backend, &detail.worker_ref, WorkerStatus::Paused);

        runtime
            .send_protocol_method(
                &detail.worker_ref,
                Method::Resume {
                    command: adapter_command(&backend, &detail.worker_ref),
                },
            )
            .expect("resume paused turn");
        wait_for_adapter_state(&backend, &detail.worker_ref, WorkerStatus::Running);

        let duplicate_resume = adapter_command(&backend, &detail.worker_ref);
        runtime
            .send_protocol_method(
                &detail.worker_ref,
                Method::Resume {
                    command: duplicate_resume,
                },
            )
            .expect("duplicate Resume is forwarded for controller admission");
        wait_for_adapter_command(&backend, &detail.worker_ref, duplicate_resume.command_id);

        runtime
            .send_protocol_method(
                &detail.worker_ref,
                Method::Pause {
                    command: adapter_command(&backend, &detail.worker_ref),
                },
            )
            .expect("pause resumed turn");
        wait_for_adapter_state(&backend, &detail.worker_ref, WorkerStatus::Paused);
        runtime
            .send_protocol_method(
                &detail.worker_ref,
                Method::Resume {
                    command: adapter_command(&backend, &detail.worker_ref),
                },
            )
            .expect("resume paused turn a second time");
        wait_for_adapter_state(&backend, &detail.worker_ref, WorkerStatus::Idle);
        assert_eq!(call_count.load(Ordering::SeqCst), 3);

        let idle_resume = adapter_command(&backend, &detail.worker_ref);
        runtime
            .send_protocol_method(
                &detail.worker_ref,
                Method::Resume {
                    command: idle_resume,
                },
            )
            .expect("Idle Resume preserves controller NotPaused semantics");
        wait_for_adapter_command(&backend, &detail.worker_ref, idle_resume.command_id);
        wait_for_adapter_state(&backend, &detail.worker_ref, WorkerStatus::Idle);
        let events = runtime
            .read_worker_observation_events(&detail.worker_ref, WorkerObservationCursor::zero())
            .expect("read protocol events");
        assert!(events.iter().any(|event| {
            matches!(
                &event.payload,
                Event::CommandAcknowledged { acknowledgement }
                    if acknowledgement.command == protocol::WorkerCommandKind::Resume
                        && acknowledgement.disposition
                            == protocol::WorkerCommandDisposition::InvalidState
            )
        }));
        assert_eq!(call_count.load(Ordering::SeqCst), 3);
    }

    #[test]
    #[cfg(feature = "ws-server")]
    #[serial_test::serial(worker_allocation)]
    fn idle_only_input_acknowledges_idle_and_rejects_running_paused_and_stopped() {
        let client = MockClient::sequential(vec![MockResponse::Hang(
            simple_text_events().into_iter().take(2).collect(),
        )]);
        let runtime_base = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let factory = MockFactory {
            client,
            runtime_base: runtime_base.path().to_path_buf(),
            cwd: cwd.path().to_path_buf(),
            store_dir: store.path().join("sessions"),
            worker_metadata_dir: store.path().join("workers"),
            observed_cwds: Arc::new(Mutex::new(Vec::new())),
            observed_workspace_clients: Arc::new(Mutex::new(Vec::new())),
        };
        let backend = Arc::new(WorkerRuntimeExecutionBackend::new(factory).unwrap());
        let runtime =
            EmbeddedRuntime::with_execution_backend(RuntimeOptions::default(), backend.clone())
                .unwrap();
        runtime.store_config_bundle(test_bundle()).unwrap();
        let detail = runtime.create_worker(create_request("idle-only")).unwrap();
        let input = |id: &str| {
            let mut input = WorkerInput::user(id);
            input.kind = WorkerInputKind::UserIfIdle;
            input.submission_request_id = Some(id.to_string());
            input
        };
        let ack = runtime
            .send_input(&detail.worker_ref, input("idle-input"))
            .unwrap();
        assert_eq!(ack.submission.unwrap().submission_request_id, "idle-input");
        wait_for_adapter_state(&backend, &detail.worker_ref, WorkerStatus::Running);
        let error = runtime
            .send_input(&detail.worker_ref, input("running-input"))
            .unwrap_err();
        assert!(error.to_string().contains("WorkerNotify"), "{error}");
        runtime
            .send_protocol_method(
                &detail.worker_ref,
                Method::Pause {
                    command: adapter_command(&backend, &detail.worker_ref),
                },
            )
            .unwrap();
        wait_for_adapter_state(&backend, &detail.worker_ref, WorkerStatus::Paused);
        let error = runtime
            .send_input(&detail.worker_ref, input("paused-input"))
            .unwrap_err();
        assert!(error.to_string().contains("WorkerNotify"), "{error}");
        let handle = backend
            .workers
            .lock()
            .unwrap()
            .get(&detail.worker_ref)
            .unwrap()
            .handle
            .clone();
        let pending = backend
            .run_on_adapter_runtime(async move {
                let mut events = handle.subscribe();
                handle
                    .send(Method::ListPendingSubmissions)
                    .await
                    .map_err(|e| e.to_string())?;
                tokio::time::timeout(Duration::from_secs(2), async {
                    loop {
                        if let Ok(Event::PendingSubmissionsChanged { pending }) =
                            events.recv().await
                        {
                            break pending;
                        }
                    }
                })
                .await
                .map_err(|e| e.to_string())
            })
            .unwrap();
        assert!(pending.submissions.is_empty());
        // Human Submit still queues while paused, and Notify remains advisory.
        let ack = runtime
            .send_input(&detail.worker_ref, WorkerInput::user("human-input"))
            .unwrap();
        assert_eq!(
            ack.submission.unwrap().disposition,
            protocol::SubmissionDisposition::Queued
        );
        let mut notify = WorkerInput::user("advisory");
        notify.kind = WorkerInputKind::Notify;
        assert!(
            runtime
                .send_input(&detail.worker_ref, notify)
                .unwrap()
                .notification
                .is_some()
        );
        wait_for_adapter_state(&backend, &detail.worker_ref, WorkerStatus::Paused);
        let handle = backend
            .workers
            .lock()
            .unwrap()
            .get(&detail.worker_ref)
            .unwrap()
            .handle
            .clone();
        let entries = serde_json::to_string(&handle.committed_entries()).unwrap();
        assert!(!entries.contains("running-input"));
        assert!(!entries.contains("paused-input"));
        runtime.stop_worker(&detail.worker_ref, None).unwrap();
        let error = runtime
            .send_input(&detail.worker_ref, input("stopped-input"))
            .unwrap_err();
        assert!(error.to_string().contains("WorkerNotify"), "{error}");
    }

    #[test]
    fn stopped_runtime_worker_can_restore_and_accept_input() {
        let client = MockClient::new(simple_text_events());
        let runtime_base = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let factory = MockFactory {
            client,
            runtime_base: runtime_base.path().to_path_buf(),
            cwd: cwd.path().to_path_buf(),
            store_dir: store.path().join("sessions"),
            worker_metadata_dir: store.path().join("workers"),
            observed_cwds: Arc::new(Mutex::new(Vec::new())),
            observed_workspace_clients: Arc::new(Mutex::new(Vec::new())),
        };
        let backend = Arc::new(WorkerRuntimeExecutionBackend::new(factory).unwrap());
        let runtime =
            EmbeddedRuntime::with_execution_backend(RuntimeOptions::default(), backend.clone())
                .unwrap();
        runtime.store_config_bundle(test_bundle()).unwrap();
        let detail = runtime
            .create_worker(create_request("restore-after-stop"))
            .unwrap();
        runtime.stop_worker(&detail.worker_ref, None).unwrap();
        assert_eq!(
            runtime.worker_detail(&detail.worker_ref).unwrap().status,
            crate::catalog::WorkerStatus::Stopped
        );
        assert!(
            !backend
                .workers
                .lock()
                .unwrap()
                .contains_key(&detail.worker_ref)
        );

        runtime
            .restore_worker(
                &detail.worker_ref,
                runtime.test_restore_request(&detail.worker_ref),
            )
            .unwrap();
        assert_eq!(
            runtime.worker_detail(&detail.worker_ref).unwrap().status,
            crate::catalog::WorkerStatus::Idle
        );
        assert!(
            backend
                .workers
                .lock()
                .unwrap()
                .contains_key(&detail.worker_ref)
        );
        runtime
            .send_input(&detail.worker_ref, WorkerInput::user("continue"))
            .unwrap();
    }

    #[test]
    fn stopping_and_deleting_worker_preserves_bound_working_directory() {
        let client = MockClient::new(simple_text_events());
        let runtime_base = tempfile::tempdir().unwrap();
        let repo = create_clean_repo();
        let store = tempfile::tempdir().unwrap();
        let factory = MockFactory {
            client,
            runtime_base: runtime_base.path().to_path_buf(),
            cwd: repo.path().to_path_buf(),
            store_dir: store.path().join("sessions"),
            worker_metadata_dir: store.path().join("workers"),
            observed_cwds: Arc::new(Mutex::new(Vec::new())),
            observed_workspace_clients: Arc::new(Mutex::new(Vec::new())),
        };
        let backend = WorkerRuntimeExecutionBackend::new(factory)
            .unwrap()
            .with_working_directory_materializer(RuntimeGitMaterializer::new(runtime_base.path()));
        let runtime =
            EmbeddedRuntime::with_execution_backend(RuntimeOptions::default(), Arc::new(backend))
                .unwrap();
        runtime.store_config_bundle(test_bundle()).unwrap();
        let mut request = create_request("chat");
        request.workdir_attachment_requests = vec![WorkingDirectoryAttachmentRequest {
            alias: workdir::WorkdirAttachmentAlias::new("checkout").unwrap(),
            working_directory: working_directory_request(repo.path()),
        }];
        let detail = runtime.create_worker(request).unwrap();
        let workdir_id = detail.workdir_attachments[0]
            .working_directory
            .summary
            .working_directory_id
            .clone();
        let clone_root = materialized_clone_root(runtime_base.path(), &workdir_id);
        assert!(clone_root.join("README.md").exists());

        runtime.stop_worker(&detail.worker_ref, None).unwrap();
        runtime.delete_worker(&detail.worker_ref).unwrap();

        assert!(clone_root.join("README.md").exists());
        let status = runtime.working_directory(&workdir_id).unwrap();
        assert_eq!(
            status.summary.status,
            crate::catalog::WorkingDirectoryStatusKind::Active
        );
        assert_eq!(status.summary.cleanliness.as_deref(), Some("clean"));
    }

    #[test]
    fn spawn_failure_with_existing_working_directory_preserves_workdir() {
        let runtime_base = tempfile::tempdir().unwrap();
        let repo = create_clean_repo();
        let backend = WorkerRuntimeExecutionBackend::new(FailingFactory)
            .unwrap()
            .with_working_directory_materializer(RuntimeGitMaterializer::new(runtime_base.path()));
        let runtime =
            EmbeddedRuntime::with_execution_backend(RuntimeOptions::default(), Arc::new(backend))
                .unwrap();
        runtime.store_config_bundle(test_bundle()).unwrap();
        let status = runtime
            .create_working_directory(working_directory_request(repo.path()))
            .unwrap();
        let workdir_id = status.summary.working_directory_id.clone();
        let clone_root = materialized_clone_root(runtime_base.path(), &workdir_id);
        assert!(clone_root.join("README.md").exists());
        let mut request = create_request("chat");
        request.workdir_attachments = vec![WorkingDirectoryAttachmentClaim {
            alias: workdir::WorkdirAttachmentAlias::new("workdir").unwrap(),
            working_directory_id: workdir_id.clone(),
            relative_cwd: None,
            capabilities: workdir::WorkdirSessionCapabilities::ALL,
        }];

        let error = runtime.create_worker(request).unwrap_err();

        assert!(format!("{error:?}").contains("spawn failed"));
        assert!(clone_root.join("README.md").exists());
        let status = runtime.working_directory(&workdir_id).unwrap();
        assert_eq!(
            status.summary.status,
            crate::catalog::WorkingDirectoryStatusKind::Active
        );
    }

    #[test]
    fn spawn_failure_with_new_materialization_rolls_back_workdir_record() {
        let runtime_base = tempfile::tempdir().unwrap();
        let repo = create_clean_repo();
        let backend = WorkerRuntimeExecutionBackend::new(FailingFactory)
            .unwrap()
            .with_working_directory_materializer(RuntimeGitMaterializer::new(runtime_base.path()));
        let runtime =
            EmbeddedRuntime::with_execution_backend(RuntimeOptions::default(), Arc::new(backend))
                .unwrap();
        runtime.store_config_bundle(test_bundle()).unwrap();
        let mut request = create_request("chat");
        let second_repo = create_clean_repo();
        request.workdir_attachment_requests = vec![
            WorkingDirectoryAttachmentRequest {
                alias: workdir::WorkdirAttachmentAlias::new("checkout").unwrap(),
                working_directory: working_directory_request(repo.path()),
            },
            WorkingDirectoryAttachmentRequest {
                alias: workdir::WorkdirAttachmentAlias::new("docs").unwrap(),
                working_directory: working_directory_request(second_repo.path()),
            },
        ];

        let error = runtime.create_worker(request).unwrap_err();

        assert!(format!("{error:?}").contains("spawn failed"));
        let working_directories_root = runtime_base.path();
        let remaining_workdirs = fs::read_dir(working_directories_root)
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
                    .count()
            })
            .unwrap_or(0);
        assert_eq!(remaining_workdirs, 0);
        assert!(!working_directories_root.join(".repository-cache").exists());
    }

    #[test]
    fn second_materialization_failure_rolls_back_first_attachment() {
        let runtime_base = tempfile::tempdir().unwrap();
        let repo = create_clean_repo();
        let missing_repo = runtime_base.path().join("missing-repository");
        let backend = WorkerRuntimeExecutionBackend::new(FailingFactory)
            .unwrap()
            .with_working_directory_materializer(RuntimeGitMaterializer::new(runtime_base.path()));
        let runtime =
            EmbeddedRuntime::with_execution_backend(RuntimeOptions::default(), Arc::new(backend))
                .unwrap();
        runtime.store_config_bundle(test_bundle()).unwrap();
        let mut request = create_request("chat");
        request.workdir_attachment_requests = vec![
            WorkingDirectoryAttachmentRequest {
                alias: workdir::WorkdirAttachmentAlias::new("checkout").unwrap(),
                working_directory: working_directory_request(repo.path()),
            },
            WorkingDirectoryAttachmentRequest {
                alias: workdir::WorkdirAttachmentAlias::new("docs").unwrap(),
                working_directory: working_directory_request(&missing_repo),
            },
        ];

        let error = runtime.create_worker(request).unwrap_err();
        assert!(format!("{error:?}").contains("repository"));
        let remaining_workdirs = fs::read_dir(runtime_base.path())
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
                    .count()
            })
            .unwrap_or(0);
        assert_eq!(remaining_workdirs, 0);
    }
}
