//! Adapter from `worker-runtime` execution backend boundary to the real
//! `worker` crate controller/run lifecycle.
//!
//! The adapter intentionally owns real `WorkerHandle`s internally and exposes
//! only the opaque `worker-runtime` execution handle to callers. Browser/API
//! projections therefore keep the existing runtime redaction boundary: no raw
//! socket paths, session paths, manifests, credentials, or handles leave this
//! module.

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock, mpsc};
use std::time::Duration;

use crate::auth::{BACKEND_RESOURCE_FETCH_PERMISSION, RuntimeIdentityMaterial};
use crate::catalog::{
    CreateWorkerRequest, ProfileSourceArchiveSource, RepositoryRefObservation,
    RepositoryRefObservationRequest, WorkingDirectoryRepositoryAccessRequest,
    WorkingDirectoryRequest, WorkingDirectoryStatus,
};
use crate::config_bundle::{ConfigBundle, workspace_config_etag};
use crate::execution::{
    WorkerExecutionBackend, WorkerExecutionHandle, WorkerExecutionOperation,
    WorkerExecutionRestoreRequest, WorkerExecutionResult, WorkerExecutionSpawnRequest,
    WorkerExecutionSpawnResult, WorkspaceConfigFetchRequest, WorkspaceConfigFetchResult,
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

fn next_internal_command(
    state: &RwLock<protocol::WorkerStateSnapshot>,
) -> Result<WorkerCommandEnvelope, String> {
    let snapshot = state
        .read()
        .map_err(|_| "worker state lock is poisoned".to_string())?
        .clone();
    let floor = snapshot.last_command_id.saturating_add(1);
    let command_id = NEXT_INTERNAL_COMMAND_ID
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            Some(current.max(floor).saturating_add(1))
        })
        .unwrap_or(floor)
        .max(floor);
    Ok(WorkerCommandEnvelope::for_snapshot(command_id, &snapshot))
}
use session_store::{CombinedStore, WorkerAggregateStore, WorkerSessionStore};
#[cfg(test)]
use session_store::{FsStore, FsWorkerStore};
use tokio::runtime::Runtime;
#[cfg(feature = "ws-server")]
use tokio::sync::broadcast;
use workdir::{LocalWorkdirSession, Workdir, WorkdirSessionCapabilities, WorkdirSessionHandle};

#[cfg(test)]
use worker::WorkerController;
use worker::feature::builtin::{
    CompositeWorkerObservationProvider, WorkerObservationError, WorkerObservationProvider,
    WorkerObservationSubject, WorkerObservationSubjectRef, WorkerSessionCapture,
    WorkspaceClientWorkerObservationProvider,
};
#[cfg(feature = "ws-server")]
use worker::ipc::protocol_session::{live_log_entry_event, subscribe_worker_protocol_session};
use worker::{
    PreparedWorker, PromptCatalogSource, SegmentLogSink, Worker, WorkerBootstrap,
    WorkerBootstrapError, WorkerBootstrapLayout, WorkerControllerTransport, WorkerError,
    WorkerFilesystemAuthority, WorkerHandle, WorkerSharedState, WorkerWorkspaceContext,
    WorkspaceClient, WorkspaceId, bash_output_dir_for_worker_id,
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

    async fn spawn_controller(
        &self,
        request: WorkerExecutionSpawnRequest,
    ) -> Result<RuntimeWorkerController, String>;

    async fn restore_controller(
        &self,
        request: WorkerExecutionRestoreRequest,
    ) -> Result<RuntimeWorkerController, String>;
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
            controller_transport: WorkerControllerTransport::UnixSocket,
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

fn runtime_local_workdir_session(
    workdir_id: &str,
    root: &Path,
    cwd: &Path,
    scope: manifest::SharedScope,
    command_environment: std::collections::BTreeMap<String, String>,
    resources: Vec<Arc<dyn workdir::WorkdirSessionResource>>,
) -> WorkdirSessionHandle {
    Arc::new(LocalWorkdirSession::materialized_bound_with_environment(
        Workdir::new(workdir_id),
        root.to_path_buf(),
        cwd.to_path_buf(),
        scope,
        WorkdirSessionCapabilities::ALL,
        command_environment,
        resources,
    ))
}

fn bind_workspace_memory_settings(
    manifest: &mut manifest::WorkerManifest,
    request: &CreateWorkerRequest,
) -> Result<(), String> {
    let Some(snapshot) = request.memory_settings.as_ref() else {
        if request.workspace_api.is_some() {
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
    manifest
        .feature
        .memory
        .bind_workspace_settings(snapshot.clone())
        .map_err(str::to_string)?;
    manifest
        .feature
        .memory
        .validate_execution()
        .map_err(str::to_string)?;
    Ok(())
}

fn validate_worker_memory_settings(
    manifest: &manifest::WorkerManifest,
    request: &CreateWorkerRequest,
) -> Result<(), String> {
    let Some(expected) = request.memory_settings.as_ref() else {
        return Ok(());
    };
    manifest
        .feature
        .memory
        .validate_execution()
        .map_err(str::to_string)?;
    if !manifest.feature.memory.profile.enabled {
        return Ok(());
    }
    let actual = manifest
        .feature
        .memory
        .workspace_settings()
        .ok_or_else(|| {
            "Workspace Worker restored without its bound Memory settings snapshot".to_string()
        })?;
    if &actual != expected {
        return Err(format!(
            "Workspace Worker Memory settings snapshot mismatch: expected {} revision {}, restored {} revision {}",
            expected.workspace_id,
            expected.settings_revision,
            actual.workspace_id,
            actual.settings_revision
        ));
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
        let has_local_filesystem = request.working_directory.is_some();
        let worker_root = request
            .working_directory
            .as_ref()
            .map(|binding| binding.root().to_path_buf())
            .unwrap_or_else(|| self.profile_base_dir.clone());
        let filesystem_authority = request
            .working_directory
            .as_ref()
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

        let run_dir = worker_aggregate_dir
            .join("runs")
            .join(request.run_generation.to_string());
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
        validate_worker_memory_settings(worker.manifest(), &request.request)?;
        if let Some(binding) = request.working_directory.as_ref() {
            worker.bind_workdir_session(Some(runtime_local_workdir_session(
                &binding.working_directory.id,
                binding.root(),
                binding.cwd(),
                worker.scope().clone(),
                binding.command_environment(),
                binding.session_resources(),
            )));
        } else {
            worker.bind_workdir_session(None);
        }
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
        let (handle, shutdown_rx) = (started.handle, started.shutdown);
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
            workspace_client,
        })
    }

    async fn restore_controller(
        &self,
        request: WorkerExecutionRestoreRequest,
    ) -> Result<RuntimeWorkerController, String> {
        let worker_name = Self::runtime_worker_name_for_ref(&request.worker_ref);
        let filesystem_authority = request
            .working_directory
            .as_ref()
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
        let (mut manifest, loader) = Self::restore_fallback_manifest(&worker_name)?;
        bind_workspace_memory_settings(&mut manifest, &request.request)?;

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
        validate_worker_memory_settings(worker.manifest(), &request.request)?;
        let flow_transition_enabled = worker.manifest().feature.flow.enabled;
        if let Some(binding) = request.working_directory.as_ref() {
            worker.bind_workdir_session(Some(runtime_local_workdir_session(
                &binding.working_directory.id,
                binding.root(),
                binding.cwd(),
                worker.scope().clone(),
                binding.command_environment(),
                binding.session_resources(),
            )));
        } else {
            worker.bind_workdir_session(None);
        }
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
        let run_dir = worker_aggregate_dir
            .join("runs")
            .join(request.run_generation.to_string());
        let bash_output_dir = bash_output_dir_for_worker_id(&request.worker_ref.worker_id);
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
            WorkerBootstrapError::Controller { source, .. } => format!(
                "failed to spawn restored Worker controller in {}: {source}",
                run_dir.display()
            ),
        })?;
        let (handle, shutdown_rx) = (started.handle, started.shutdown);
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
            workspace_client,
        })
    }
}

#[derive(Clone)]
struct RuntimeWorkerExecution {
    handle: WorkerHandle,
    shutdown: Arc<tokio::sync::Mutex<Option<worker::ShutdownReceiver>>>,
    worker_state: Arc<RwLock<protocol::WorkerStateSnapshot>>,
    workspace_client: Option<Arc<dyn WorkspaceClient>>,
}

/// `worker-runtime` execution backend backed by real `worker` crate Workers.
pub struct WorkerRuntimeExecutionBackend<F = ProfileRuntimeWorkerFactory> {
    backend_id: String,
    factory: Arc<F>,
    working_directory_materializer: Option<Arc<dyn WorkingDirectoryMaterializer>>,
    runtime: Mutex<Option<Runtime>>,
    workers: Mutex<HashMap<crate::identity::WorkerRef, RuntimeWorkerExecution>>,
    spawn_restore_timeout: Duration,
}

impl WorkerRuntimeExecutionBackend<ProfileRuntimeWorkerFactory> {
    pub fn from_workspace(workspace_root: impl Into<PathBuf>) -> Result<Self, String> {
        let workspace_root = workspace_root.into();
        let factory = ProfileRuntimeWorkerFactory::new(&workspace_root)
            .with_runtime_store_dir(workspace_root.join(".yoi/runtime-store"));
        Self::new(factory)
    }
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

    fn wait_for_runtime_task<T>(receiver: mpsc::Receiver<Result<T, String>>) -> Result<T, String> {
        receiver
            .recv_timeout(RUNTIME_TASK_TIMEOUT)
            .map_err(|err| format!("worker adapter task did not complete: {err}"))?
    }

    fn spawn_on_adapter_runtime<Fut>(&self, task: Fut) -> Result<(), String>
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
        runtime.spawn(task);
        Ok(())
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

    fn get_execution(
        &self,
        handle: &WorkerExecutionHandle,
    ) -> Result<
        (
            WorkerHandle,
            Arc<RwLock<protocol::WorkerStateSnapshot>>,
            Option<Arc<dyn WorkspaceClient>>,
        ),
        WorkerExecutionResult,
    > {
        if handle.backend_id() != self.backend_id() {
            return Err(WorkerExecutionResult::rejected(
                WorkerExecutionOperation::Input,
                format!(
                    "execution handle belongs to backend {}, not {}",
                    handle.backend_id(),
                    self.backend_id()
                ),
            ));
        }
        let workers = self.workers.lock().map_err(|_| {
            WorkerExecutionResult::errored(
                WorkerExecutionOperation::Input,
                "worker adapter registry lock is poisoned",
            )
        })?;
        workers
            .get(handle.worker_ref())
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
                    "execution handle does not reference a live Worker",
                )
            })
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

    fn connect_handle(
        &self,
        operation: WorkerExecutionOperation,
        worker_ref: crate::identity::WorkerRef,
        bridge_context: crate::execution::WorkerExecutionContext,
        handle: WorkerHandle,
        shutdown: Arc<tokio::sync::Mutex<Option<worker::ShutdownReceiver>>>,
        working_directory: Option<WorkingDirectoryBinding>,
        workspace_client: Option<Arc<dyn WorkspaceClient>>,
    ) -> WorkerExecutionSpawnResult {
        let worker_state = Arc::new(RwLock::new(handle.shared_state.snapshot()));
        #[cfg(feature = "ws-server")]
        {
            let streams = subscribe_worker_protocol_session(&handle);
            let mut events = streams.events;
            let mut entry_events = streams.log_entries;
            let bridge_worker_state = worker_state.clone();
            if let Err(message) = self.spawn_on_adapter_runtime(async move {
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
                return WorkerExecutionSpawnResult::Errored(WorkerExecutionResult::errored(
                    operation, message,
                ));
            }
        }
        #[cfg(not(feature = "ws-server"))]
        {
            let _ = bridge_context;
        }

        let mut workers = match self.workers.lock() {
            Ok(workers) => workers,
            Err(_) => {
                return WorkerExecutionSpawnResult::Errored(WorkerExecutionResult::errored(
                    operation,
                    "worker adapter registry lock is poisoned",
                ));
            }
        };
        let connected_worker_state = worker_state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        workers.insert(
            worker_ref.clone(),
            RuntimeWorkerExecution {
                handle,
                shutdown,
                worker_state,
                workspace_client,
            },
        );

        WorkerExecutionSpawnResult::Connected {
            handle: WorkerExecutionHandle::new(worker_ref, self.backend_id()),
            worker_state: connected_worker_state,
            working_directory: working_directory.map(|binding| binding.status()),
        }
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
    let (incoming, replace_stale) = match event {
        Event::WorkerState { snapshot } => (snapshot, false),
        Event::Snapshot { state, .. } => (state, true),
        Event::CommandAcknowledged { acknowledgement } => (&mut acknowledgement.state, true),
        _ => return Ok(true),
    };
    let mut current = current
        .write()
        .map_err(|_| "worker state projection lock is poisoned".to_string())?;
    match protocol::apply_worker_state_snapshot(&mut current, incoming) {
        Ok(protocol::WorkerStateSnapshotApply::Applied)
        | Ok(protocol::WorkerStateSnapshotApply::Duplicate) => Ok(true),
        Ok(protocol::WorkerStateSnapshotApply::Stale) if replace_stale => {
            *incoming = current.clone();
            Ok(true)
        }
        Ok(protocol::WorkerStateSnapshotApply::Stale) => Ok(false),
        Err(error) => Err(error.to_string()),
    }
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
        Ok(runtime_local_workdir_session(
            working_directory_id,
            binding.root(),
            binding.cwd(),
            manifest::SharedScope::new(scope),
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
        if self
            .workers
            .lock()
            .map(|workers| workers.contains_key(&request.worker_ref))
            .unwrap_or(false)
        {
            return WorkerExecutionSpawnResult::Rejected(WorkerExecutionResult::busy(
                WorkerExecutionOperation::Spawn,
                "Worker is already connected to execution backend",
            ));
        }

        let mut request = request;
        let mut rollback_working_directory = None;
        let working_directory = match (
            request.request.working_directory_request.as_ref(),
            request.request.working_directory.as_ref(),
        ) {
            (Some(_), Some(_)) => {
                return WorkerExecutionSpawnResult::Rejected(WorkerExecutionResult::rejected(
                    WorkerExecutionOperation::Spawn,
                    "worker spawn cannot specify both working_directory_request and working_directory",
                ));
            }
            (Some(working_directory_request), None) => {
                let Some(materializer) = self.working_directory_materializer.as_ref() else {
                    return WorkerExecutionSpawnResult::Rejected(WorkerExecutionResult::rejected(
                        WorkerExecutionOperation::Spawn,
                        "working directory materialization requested, but no materializer is configured for this runtime backend",
                    ));
                };
                match materializer.materialize(&request.worker_ref, working_directory_request) {
                    Ok(binding) => {
                        request.working_directory = Some(binding.clone());
                        rollback_working_directory = Some(binding.clone());
                        Some(binding)
                    }
                    Err(error) => {
                        return WorkerExecutionSpawnResult::Rejected(
                            WorkerExecutionResult::rejected(
                                WorkerExecutionOperation::Spawn,
                                error.to_string(),
                            ),
                        );
                    }
                }
            }
            (None, Some(working_directory)) => {
                let Some(materializer) = self.working_directory_materializer.as_ref() else {
                    return WorkerExecutionSpawnResult::Rejected(WorkerExecutionResult::rejected(
                        WorkerExecutionOperation::Spawn,
                        "working directory working_directory requested, but no materializer is configured for this runtime backend",
                    ));
                };
                match materializer.bind_working_directory(
                    &working_directory.working_directory_id,
                    working_directory.relative_cwd.as_deref(),
                ) {
                    Ok(binding) => {
                        request.working_directory = Some(binding.clone());
                        Some(binding)
                    }
                    Err(error) => {
                        return WorkerExecutionSpawnResult::Rejected(
                            WorkerExecutionResult::rejected(
                                WorkerExecutionOperation::Spawn,
                                error.to_string(),
                            ),
                        );
                    }
                }
            }
            (None, None) => None,
        };

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
                if let (Some(materializer), Some(binding)) = (
                    self.working_directory_materializer.as_ref(),
                    rollback_working_directory.as_ref(),
                ) {
                    let _ = materializer.cleanup_working_directory(&binding.working_directory.id);
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
            working_directory,
            Some(controller.workspace_client),
        )
    }

    fn restore_worker(
        &self,
        mut request: WorkerExecutionRestoreRequest,
    ) -> WorkerExecutionSpawnResult {
        let working_directory = match request.previous_working_directory.clone() {
            Some(status) => {
                let Some(materializer) = self.working_directory_materializer.as_ref() else {
                    return WorkerExecutionSpawnResult::Rejected(WorkerExecutionResult::rejected(
                        WorkerExecutionOperation::Restore,
                        "persisted worker has a working directory binding, but no materializer is configured for this runtime backend",
                    ));
                };
                let relative_cwd = request
                    .request
                    .working_directory
                    .as_ref()
                    .and_then(|working_directory| working_directory.relative_cwd.as_deref());
                match materializer
                    .bind_working_directory(&status.summary.working_directory_id, relative_cwd)
                {
                    Ok(binding) => {
                        request.working_directory = Some(binding.clone());
                        Some(binding)
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
            None if request.request.working_directory_request.is_some() => {
                return WorkerExecutionSpawnResult::Rejected(WorkerExecutionResult::rejected(
                    WorkerExecutionOperation::Restore,
                    "persisted worker requested a working directory, but no persisted working directory binding is available to restore",
                ));
            }
            None if request.request.working_directory.is_some() => {
                let Some(materializer) = self.working_directory_materializer.as_ref() else {
                    return WorkerExecutionSpawnResult::Rejected(WorkerExecutionResult::rejected(
                        WorkerExecutionOperation::Restore,
                        "persisted worker has a working directory claim, but no materializer is configured for this runtime backend",
                    ));
                };
                let working_directory =
                    request.request.working_directory.as_ref().expect("checked");
                match materializer.bind_working_directory(
                    &working_directory.working_directory_id,
                    working_directory.relative_cwd.as_deref(),
                ) {
                    Ok(binding) => {
                        request.working_directory = Some(binding.clone());
                        Some(binding)
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
            None => None,
        };

        let factory = self.factory.clone();
        let bridge_context = request.context.clone();
        let worker_ref = request.worker_ref.clone();
        let restore_result = self
            .run_cancellable_on_adapter_runtime(self.spawn_restore_timeout, async move {
                factory.restore_controller(request).await
            });

        let controller = match restore_result {
            Ok(controller) => controller,
            Err(message) => {
                return WorkerExecutionSpawnResult::Errored(WorkerExecutionResult::errored(
                    WorkerExecutionOperation::Restore,
                    message,
                ));
            }
        };

        self.connect_handle(
            WorkerExecutionOperation::Restore,
            worker_ref,
            bridge_context,
            controller.handle,
            controller.shutdown,
            working_directory,
            Some(controller.workspace_client),
        )
    }

    fn dispatch_input(
        &self,
        handle: &WorkerExecutionHandle,
        input: WorkerInput,
    ) -> WorkerExecutionResult {
        let (worker, worker_state, _workspace_client) = match self.get_execution(handle) {
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
            return self.send_method(
                WorkerExecutionOperation::Input,
                worker,
                Method::NotifyTracked {
                    notification_request_id: notification_request_id.clone(),
                    message: input.content,
                    auto_run: true,
                    source: protocol::AuthenticatedInputSource::Backend {
                        operation_id: notification_request_id,
                    },
                },
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
            WorkerInputKind::User => {
                let Some(submission_id) = input
                    .submission_request_id
                    .filter(|submission_id| !submission_id.trim().is_empty())
                else {
                    return WorkerExecutionResult::rejected(
                        WorkerExecutionOperation::Input,
                        "Runtime user input is missing its internal submission id",
                    );
                };
                (
                    Method::SubmitTracked {
                        submission_request_id: submission_id.clone(),
                        input: input.segments.unwrap_or_else(|| {
                            vec![Segment::text(input.content.trim().to_string())]
                        }),
                        source: protocol::AuthenticatedInputSource::Backend {
                            operation_id: submission_id.clone(),
                        },
                    },
                    Some(submission_id),
                )
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
        handle: &WorkerExecutionHandle,
        file_name: &str,
        media_type: &str,
        content: &[u8],
        context: Option<&session_store::UploadedFileUploadContext>,
    ) -> Result<protocol::UploadedFileRef, WorkerExecutionResult> {
        let (worker, _, _) = self.get_execution(handle).map_err(|mut result| {
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
        handle: &WorkerExecutionHandle,
        artifact_id: &str,
    ) -> WorkerExecutionResult {
        let (worker, _, _) = match self.get_execution(handle) {
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

    fn dispatch_method(
        &self,
        handle: &WorkerExecutionHandle,
        method: Method,
    ) -> WorkerExecutionResult {
        let (worker, _worker_state, _workspace_client) = match self.get_execution(handle) {
            Ok(execution) => execution,
            Err(mut result) => {
                result.operation = WorkerExecutionOperation::ProtocolMethod;
                return result;
            }
        };

        self.send_method(WorkerExecutionOperation::ProtocolMethod, worker, method)
    }

    fn stop_worker(&self, handle: &WorkerExecutionHandle) -> WorkerExecutionResult {
        if handle.backend_id() != self.backend_id() {
            return WorkerExecutionResult::rejected(
                WorkerExecutionOperation::Stop,
                format!(
                    "execution handle belongs to backend {}, not {}",
                    handle.backend_id(),
                    self.backend_id()
                ),
            );
        }
        let execution = match self.workers.lock() {
            Ok(workers) => workers.get(handle.worker_ref()).cloned(),
            Err(_) => {
                return WorkerExecutionResult::errored(
                    WorkerExecutionOperation::Stop,
                    "worker adapter registry lock is poisoned",
                );
            }
        };
        let Some(execution) = execution else {
            return WorkerExecutionResult::rejected(
                WorkerExecutionOperation::Stop,
                "execution handle does not reference a live Worker",
            );
        };
        let artifact_cleanup = execution.handle.clone();
        let shutdown = execution.shutdown.clone();
        let command = match next_internal_command(&execution.worker_state) {
            Ok(command) => command,
            Err(error) => {
                return WorkerExecutionResult::errored(WorkerExecutionOperation::Stop, error);
            }
        };
        let result = self.send_method(
            WorkerExecutionOperation::Stop,
            execution.handle.clone(),
            Method::Shutdown { command },
        );
        if result.outcome != crate::execution::WorkerExecutionOutcome::Accepted {
            return result;
        }
        let shutdown_wait = self.run_on_adapter_runtime(async move {
            let mut guard = shutdown.lock().await;
            let Some(mut receiver) = guard.take() else {
                return Ok(());
            };
            match tokio::time::timeout(Duration::from_secs(5), &mut receiver).await {
                Ok(Ok(())) => Ok(()),
                Ok(Err(_)) => Err("Worker shutdown completion channel closed".to_string()),
                Err(_) => {
                    *guard = Some(receiver);
                    Err("Worker shutdown confirmation timed out; stop remains retryable".into())
                }
            }
        });
        if let Err(message) = shutdown_wait {
            return WorkerExecutionResult::errored(WorkerExecutionOperation::Stop, message);
        }
        let artifact_cleanup_error = artifact_cleanup
            .delete_uncommitted_uploaded_files()
            .err()
            .map(|error| format!("uploaded_file_cleanup_failed: {error}"));
        match self.workers.lock() {
            Ok(mut workers) => {
                workers.remove(handle.worker_ref());
            }
            Err(poisoned) => {
                poisoned.into_inner().remove(handle.worker_ref());
            }
        }
        if let Some(message) = artifact_cleanup_error {
            let mut result = result;
            result.message = Some(message);
            result
        } else {
            result
        }
    }

    fn cancel_worker(&self, handle: &WorkerExecutionHandle) -> WorkerExecutionResult {
        let (worker, worker_state, _workspace_client) = match self.get_execution(handle) {
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
    fn worker_snapshot(&self, handle: &WorkerExecutionHandle) -> Option<protocol::Event> {
        if handle.backend_id() != self.backend_id() {
            return None;
        }
        let workers = self.workers.lock().ok()?;
        workers
            .get(handle.worker_ref())
            .map(|execution| execution.handle.snapshot_event())
    }

    fn worker_completions(
        &self,
        handle: &WorkerExecutionHandle,
        kind: protocol::CompletionKind,
        prefix: &str,
    ) -> Vec<protocol::CompletionEntry> {
        if handle.backend_id() != self.backend_id() {
            return Vec::new();
        }
        let Ok(workers) = self.workers.lock() else {
            return Vec::new();
        };
        workers
            .get(handle.worker_ref())
            .map(|execution| {
                futures::executor::block_on(execution.handle.completion_entries(kind, prefix))
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
        ConfigBundleRef, CreateWorkerRequest, MaterializerKind, ProfileSelector,
        RepositorySelector, WorkingDirectoryClaim, WorkingDirectoryRepository,
        WorkingDirectoryRequest, WorkspaceApiRef,
    };
    use crate::execution::WorkerExecutionContext;
    use crate::identity::WorkerId;
    use crate::identity::WorkerRef;
    use crate::management::RuntimeOptions;
    use crate::observation::WorkerObservationCursor;
    use crate::working_directory::RuntimeGitMaterializer;
    use agen::Engine;
    use agen::llm_client::event::{Event as LlmEvent, ResponseStatus, StatusEvent};
    use agen::llm_client::{ClientError, LlmClient, Request};
    use async_trait::async_trait;
    use futures::{Stream, StreamExt};
    use manifest::{Scope, WorkerManifest};
    use session_store::{LogEntry, WorkerMetadataStore};

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

    fn test_command() -> WorkerCommandEnvelope {
        WorkerCommandEnvelope {
            command_id: 1,
            expected_execution_generation: 1,
            expected_worker_state_revision: 0,
        }
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
        WorkerCommandEnvelope::for_snapshot(state.last_command_id.saturating_add(1), &state)
    }

    #[test]
    fn protocol_bridge_applies_state_and_acknowledgement_monotonically() {
        let running = protocol::WorkerStateSnapshot {
            execution_generation: 4,
            revision: 3,
            state: protocol::WorkerState::Busy(protocol::WorkerBusyState::Run(
                protocol::WorkerRunState::Running,
            )),
            last_command_id: 2,
        };
        let current = Arc::new(RwLock::new(running.clone()));
        let mut stale = Event::WorkerState {
            snapshot: protocol::WorkerStateSnapshot {
                revision: 2,
                state: protocol::WorkerState::Idle,
                ..running.clone()
            },
        };
        assert!(!apply_protocol_worker_state(&current, &mut stale).unwrap());
        assert_eq!(*current.read().unwrap(), running);

        let paused = protocol::WorkerStateSnapshot {
            revision: 4,
            state: protocol::WorkerState::Busy(protocol::WorkerBusyState::Run(
                protocol::WorkerRunState::Paused,
            )),
            last_command_id: 3,
            ..running.clone()
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

        let mut conflict = Event::WorkerState {
            snapshot: protocol::WorkerStateSnapshot {
                state: protocol::WorkerState::Idle,
                ..paused.clone()
            },
        };
        assert!(apply_protocol_worker_state(&current, &mut conflict).is_err());
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
            Arc::new(|_, _| panic!("unused test event sink")),
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
            let filesystem_authority = request
                .working_directory
                .as_ref()
                .map(|binding| {
                    let cwd = binding.cwd().to_path_buf();
                    self.observed_cwds.lock().unwrap().push(cwd.clone());
                    WorkerFilesystemAuthority::local(binding.root().to_path_buf(), cwd)
                })
                .unwrap_or(WorkerFilesystemAuthority::None);
            let scope_root = request
                .working_directory
                .as_ref()
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
                workspace_client,
            })
        }
        async fn restore_controller(
            &self,
            request: WorkerExecutionRestoreRequest,
        ) -> Result<RuntimeWorkerController, String> {
            let request = WorkerExecutionSpawnRequest {
                worker_ref: request.worker_ref,
                run_generation: request.run_generation,
                request: request.request,
                workspace_scope: request.workspace_scope,
                context: request.context,
                working_directory: request.working_directory,
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
            working_directory_request: None,
            working_directory: None,
            worker_observation_enabled: false,
            worker_observation_grants: Vec::new(),
            workspace_api: None,
            memory_settings: None,
        }
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
                source: workspace_api::RepositorySource {
                    kind: workspace_api::RepositorySourceKind::LocalPath,
                    uri: repo.display().to_string(),
                },
                source_revision: 1,
                source_fingerprint: "sha256:test".to_string(),
                selector: Some(RepositorySelector::from("HEAD")),
            },
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
            run_generation: 1,
            request: create_request("1"),
            workspace_scope: None,
            context: test_execution_context(worker_ref),
            working_directory: None,
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
    fn restore_opens_a_fresh_session_for_the_same_workdir_identity() {
        let root = tempfile::tempdir().unwrap();
        let spawned = runtime_local_workdir_session(
            "working-directory-42",
            root.path(),
            root.path(),
            manifest::SharedScope::new(Scope::writable(root.path()).unwrap()),
            Default::default(),
            Vec::new(),
        );
        let restored = runtime_local_workdir_session(
            "working-directory-42",
            root.path(),
            root.path(),
            manifest::SharedScope::new(Scope::writable(root.path()).unwrap()),
            Default::default(),
            Vec::new(),
        );

        assert_eq!(spawned.workdir().id().as_str(), "working-directory-42");
        assert_eq!(restored.workdir().id().as_str(), "working-directory-42");
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
                worker_ref: worker_ref.clone(),
                run_generation: 1,
                request,
                workspace_scope: Some(crate::runtime::RuntimeWorkspaceScope::new(
                    "workspace-restore",
                    "server-main",
                )),
                context: test_execution_context(worker_ref),
                previous_working_directory: None,
                working_directory: None,
                config_bundle: None,
            })
            .await
        {
            Ok(_) => panic!("legacy Workspace Worker restore unexpectedly succeeded"),
            Err(error) => error,
        };
        assert!(error.contains("replacement Worker is required"), "{error}");
    }

    #[tokio::test]
    #[serial_test::serial(worker_allocation)]
    async fn in_process_restore_does_not_bind_unix_socket_under_overlong_store_path() {
        let root = tempfile::tempdir().unwrap();
        let long_component = "embedded-workspace-store-segment".repeat(4);
        let runtime_store_dir = root.path().join(long_component);
        let worker_ref = WorkerRef::new(crate::identity::WorkerId::from_legacy_u64(1));
        let worker_aggregate_dir = runtime_store_dir
            .join("workers")
            .join(worker_ref.worker_id.to_string());
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

        let run_dir = runtime_store_dir
            .join("workers")
            .join(worker_ref.worker_id.to_string())
            .join("runs/2");
        let socket_path = run_dir.join("worker.sock");
        assert!(
            socket_path.as_os_str().as_encoded_bytes().len() > 107,
            "test path must exceed Linux sockaddr_un.sun_path capacity: {}",
            socket_path.display()
        );

        let controller = ProfileRuntimeWorkerFactory::new(root.path())
            .with_runtime_store_dir(&runtime_store_dir)
            .with_controller_transport(WorkerControllerTransport::InProcess)
            .restore_controller(WorkerExecutionRestoreRequest {
                worker_ref: worker_ref.clone(),
                run_generation: 2,
                request: create_request("embedded restore"),
                workspace_scope: None,
                context: test_execution_context(worker_ref),
                previous_working_directory: None,
                working_directory: None,
                config_bundle: None,
            })
            .await
            .expect("in-process restore must not bind the overlong Unix socket path");

        assert_eq!(
            controller.handle.shared_state.catalog_status(),
            WorkerStatus::Idle
        );
        assert!(!socket_path.exists());
        assert!(run_dir.join("worker.out.log").is_file());
        assert!(run_dir.join("worker.err.log").is_file());
        controller
            .handle
            .send(Method::Shutdown {
                command: test_command(),
            })
            .await
            .unwrap();
        if let Some(receiver) = controller.shutdown.lock().await.take() {
            receiver.await.unwrap();
        }
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
                "worker.bind_workdir_session(",
                "worker.bind_worker_observation_provider(",
                "install_runtime_flow_transition_feature()",
                "prepared.start()",
            ],
        );
        assert_in_order(
            restore,
            &[
                "Worker::restore_from_worker_metadata_with_context(",
                "worker.bind_workdir_session(",
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
                    .with_runtime_store_dir(&runtime_store_dir)
                    .with_controller_transport(WorkerControllerTransport::InProcess),
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
        let first_run_socket = runtime_store_dir
            .join("workers")
            .join(worker.worker_id.to_string())
            .join("runs/1/worker.sock");
        assert!(
            first_run_socket.as_os_str().as_encoded_bytes().len() > 107,
            "test path must exceed Linux sockaddr_un.sun_path capacity: {}",
            first_run_socket.display()
        );
        assert!(!first_run_socket.exists());

        let (handle, shutdown) = {
            let workers = backend.workers.lock().unwrap();
            let execution = workers.get(&worker.worker_ref).unwrap();
            (execution.handle.clone(), execution.shutdown.clone())
        };
        backend
            .run_on_adapter_runtime(async move {
                handle
                    .send(Method::Shutdown {
                        command: test_command(),
                    })
                    .await
                    .map_err(|error| error.to_string())?;
                if let Some(receiver) = shutdown.lock().await.take() {
                    receiver.await.map_err(|error| error.to_string())?;
                }
                Ok(())
            })
            .unwrap();
        drop(runtime);
        drop(backend);

        let restored_backend = Arc::new(
            WorkerRuntimeExecutionBackend::new(
                ProfileRuntimeWorkerFactory::new(root.path())
                    .with_runtime_store_dir(&runtime_store_dir)
                    .with_controller_transport(WorkerControllerTransport::InProcess),
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
        let restored_run = runtime_store_dir
            .join("workers")
            .join(worker.worker_id.to_string())
            .join("runs/2");
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
    fn running_worker_accepts_a_second_submit_as_queued() {
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
            .send_input(&detail.worker_ref, second_input)
            .unwrap();
        assert_eq!(
            second.submission.as_ref().map(|ack| ack.disposition),
            Some(protocol::SubmissionDisposition::Queued)
        );
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
        for forbidden in core_filesystem_tool_names() {
            assert!(
                !names.contains(forbidden),
                "no-workdir Worker unexpectedly exposed {forbidden}; tools={names:?}"
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
        request.working_directory_request = Some(working_directory_request(repo.path()));

        let detail = runtime.create_worker(request).unwrap();
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

        assert!(detail.working_directory.is_some());
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

        runtime.restore_worker(&detail.worker_ref).unwrap();
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
        request.working_directory_request = Some(working_directory_request(repo.path()));
        let detail = runtime.create_worker(request).unwrap();
        let workdir_id = detail
            .working_directory
            .as_ref()
            .unwrap()
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
        assert_eq!(status.summary.primary_worker_id, None);
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
        request.working_directory = Some(WorkingDirectoryClaim {
            working_directory_id: workdir_id.clone(),
            relative_cwd: None,
        });

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
        request.working_directory_request = Some(working_directory_request(repo.path()));

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
}
