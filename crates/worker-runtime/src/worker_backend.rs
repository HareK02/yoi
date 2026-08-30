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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use crate::auth::{
    BACKEND_RESOURCE_FETCH_PERMISSION, RUNTIME_REQUEST_SOURCE_PROOF_HEADER,
    RuntimeIdentityMaterial, RuntimeRequestSourceSigner, unix_now_seconds,
};
use crate::catalog::{
    CreateWorkerRequest, ProfileSourceArchiveHttpRef, ProfileSourceArchiveSource,
    WorkingDirectoryRepositoryAccessRequest, WorkingDirectoryRequest, WorkingDirectoryStatus,
};
use crate::execution::{
    WorkerExecutionBackend, WorkerExecutionHandle, WorkerExecutionOperation,
    WorkerExecutionRestoreRequest, WorkerExecutionResult, WorkerExecutionRunState,
    WorkerExecutionSpawnRequest, WorkerExecutionSpawnResult,
};
use crate::identity::WorkerRef;
use crate::interaction::{WorkerInput, WorkerInputKind};
use crate::resource::{BackendResourceClient, ProfileSourceArchiveCache};
use crate::worker_source::{
    EmbeddedWorkerMutationDispatcher, RuntimeOwnedWorkspaceClient, RuntimeWorkerMutationForwarder,
};
use crate::working_directory::{
    WorkingDirectoryBinding, WorkingDirectoryDiagnostic, WorkingDirectoryMaterializer,
};
use async_trait::async_trait;
use protocol::{Event, Method, Segment, WorkerStatus};
use session_store::{CombinedStore, LogEntry, WorkerAggregateStore, WorkerSessionStore};
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
    PreparedWorker, PromptCatalogSource, SegmentLogSink, WORKER_INPUT_SUBMISSION_EXTENSION_DOMAIN,
    Worker, WorkerBootstrap, WorkerBootstrapError, WorkerBootstrapLayout,
    WorkerControllerTransport, WorkerError, WorkerFilesystemAuthority, WorkerHandle,
    WorkerSharedState, WorkerWorkspaceContext, WorkspaceClient, WorkspaceId,
};

const DEFAULT_BACKEND_ID: &str = "worker-crate";
const RUNTIME_TASK_TIMEOUT: Duration = Duration::from_secs(10);
// Keep this below the adapter task timeout so a failed acknowledgement task
// returns a typed execution error instead of leaving the outer waiter to time out.
const USER_INPUT_COMMIT_TIMEOUT: Duration = Duration::from_secs(9);

fn user_input_has_submission(entry: &LogEntry, submission_id: &str) -> bool {
    let extensions = match entry {
        LogEntry::UserInput { extensions, .. }
        | LogEntry::AnnotatedUserInput { extensions, .. } => extensions,
        _ => return false,
    };
    extensions.iter().any(|extension| {
        extension.domain == WORKER_INPUT_SUBMISSION_EXTENSION_DOMAIN
            && extension.payload["submission_id"].as_str() == Some(submission_id)
    })
}

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
                status: format!("{:?}", state.get_status()).to_lowercase(),
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
    profile_archive_cache: Arc<ProfileSourceArchiveCache>,
    prompt_projection_cache: Arc<WorkspacePromptProjectionCache>,
    runtime_id: Option<String>,
    worker_mutation_identity: Option<RuntimeIdentityMaterial>,
    runtime_request_audience: Option<String>,
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
            profile_archive_cache: Arc::new(ProfileSourceArchiveCache::default()),
            prompt_projection_cache: Arc::new(WorkspacePromptProjectionCache::default()),
            runtime_id: None,
            worker_mutation_identity: None,
            runtime_request_audience: None,
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

    pub fn with_runtime_request_identity(
        mut self,
        identity: RuntimeIdentityMaterial,
        audience: impl Into<String>,
    ) -> Self {
        self.runtime_id = Some(identity.identity_id.clone());
        self.worker_mutation_identity = Some(identity);
        self.runtime_request_audience = Some(audience.into());
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
        request_audience: Option<&str>,
    ) -> Result<crate::profile_archive::VerifiedProfileSourceArchive, String> {
        match source {
            ProfileSourceArchiveSource::Embedded { archive } => archive
                .verify()
                .map_err(|err| format!("failed to verify embedded profile source archive: {err}")),
            ProfileSourceArchiveSource::Http { location } => {
                self.fetch_profile_source_archive(location, request_audience)
                    .await
            }
        }
    }

    async fn fetch_profile_source_archive(
        &self,
        location: &ProfileSourceArchiveHttpRef,
        request_audience: Option<&str>,
    ) -> Result<crate::profile_archive::VerifiedProfileSourceArchive, String> {
        if let Some(cached) = self.profile_archive_cache.get(&location.archive.digest) {
            let response = fetch_profile_source_archive_http(
                location,
                Some(&location.archive.digest),
                self.worker_mutation_identity.as_ref(),
                self.runtime_request_audience
                    .as_deref()
                    .or(request_audience),
            )
            .await?;
            if let Some(fetched) = response {
                self.profile_archive_cache.insert(fetched.clone());
                fetched.verify().map_err(|err| {
                    format!("failed to verify fetched profile source archive: {err}")
                })
            } else {
                cached
                    .verify()
                    .map_err(|err| format!("failed to verify cached profile source archive: {err}"))
            }
        } else {
            let archive = fetch_profile_source_archive_http(
                location,
                None,
                self.worker_mutation_identity.as_ref(),
                self.runtime_request_audience
                    .as_deref()
                    .or(request_audience),
            )
            .await?
            .ok_or_else(|| {
                "profile source archive HTTP revalidation returned 304 without a cached archive"
                    .to_string()
            })?;
            self.profile_archive_cache.insert(archive.clone());
            archive
                .verify()
                .map_err(|err| format!("failed to verify fetched profile source archive: {err}"))
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
        runtime_request_audience: Option<&str>,
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
                let mut client = RuntimeOwnedWorkspaceClient::new(
                    workspace_id.clone(),
                    base_url.clone(),
                    runtime_id.clone(),
                    worker_ref.worker_id.to_string(),
                );
                if let Some(cache) = prompt_projection_cache {
                    client = client.with_prompt_projection_cache(cache);
                }
                if let Some(identity) = mutation_identity {
                    let audience = runtime_request_audience
                        .or_else(|| workspace_scope.map(|scope| scope.server_id.as_str()));
                    if let Some(audience) = audience {
                        client = client.with_runtime_request_source(identity, audience.to_owned());
                    }
                }
                if let (Some(scope), Some(identity)) = (workspace_scope, mutation_identity) {
                    client = client.with_worker_remove(RuntimeWorkerMutationForwarder::remote(
                        identity,
                        scope.clone(),
                        worker_ref.worker_id.to_string(),
                        base_url.clone(),
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
async fn fetch_profile_source_archive_http(
    location: &ProfileSourceArchiveHttpRef,
    cached_digest: Option<&str>,
    identity: Option<&RuntimeIdentityMaterial>,
    audience: Option<&str>,
) -> Result<Option<crate::profile_archive::ProfileSourceArchive>, String> {
    let client = reqwest::Client::new();
    let url = reqwest::Url::parse(&location.url)
        .map_err(|error| format!("profile source archive URL is invalid: {error}"))?;
    let path = url.path().to_owned();
    let workspace_id = path
        .split('/')
        .collect::<Vec<_>>()
        .windows(2)
        .find_map(|parts| (parts[0] == "w").then_some(parts[1]))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "profile source archive URL is not workspace-scoped".to_owned())?;
    let mut request = client.get(url);
    if let Some(identity) = identity {
        let audience = audience.ok_or_else(|| {
            "profile source archive request proof audience is unavailable".to_owned()
        })?;
        let proof = RuntimeRequestSourceSigner::from_identity(identity)
            .issue(
                audience,
                workspace_id,
                None,
                BACKEND_RESOURCE_FETCH_PERMISSION,
                "GET",
                &path,
                b"",
                i64::try_from(unix_now_seconds()).unwrap_or(i64::MAX),
                30,
            )
            .map_err(|error| error.to_string())?;
        request = request.header(RUNTIME_REQUEST_SOURCE_PROOF_HEADER, proof);
    }
    if cached_digest == Some(location.archive.digest.as_str()) {
        if let Some(etag) = location.etag.as_deref() {
            request = request.header(reqwest::header::IF_NONE_MATCH, etag);
        }
    }
    let response = request
        .send()
        .await
        .map_err(|err| format!("failed to fetch profile source archive: {err}"))?;
    if response.status() == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(None);
    }
    if !response.status().is_success() {
        let status = response.status();
        return Err(format!(
            "profile source archive fetch failed with HTTP {status}"
        ));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|err| format!("failed to read profile source archive response: {err}"))?
        .to_vec();
    let archive = crate::profile_archive::ProfileSourceArchive {
        reference: location.archive.clone(),
        content: bytes,
    };
    if archive.content.len() as u64 != archive.reference.size_bytes {
        return Err(format!(
            "profile source archive size mismatch: expected {}, got {}",
            archive.reference.size_bytes,
            archive.content.len()
        ));
    }
    Ok(Some(archive))
}

#[cfg(not(feature = "http-server"))]
async fn fetch_profile_source_archive_http(
    _location: &ProfileSourceArchiveHttpRef,
    _cached_digest: Option<&str>,
    _identity: Option<&RuntimeIdentityMaterial>,
    _audience: Option<&str>,
) -> Result<Option<crate::profile_archive::ProfileSourceArchive>, String> {
    Err(
        "HTTP profile source archive fetch requires the worker-runtime http-server feature"
            .to_string(),
    )
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
        .memory
        .get_or_insert_with(manifest::MemoryConfig::default)
        .bind_workspace_settings(snapshot);
    Ok(())
}

fn validate_worker_memory_settings(
    manifest: &manifest::WorkerManifest,
    request: &CreateWorkerRequest,
) -> Result<(), String> {
    let Some(expected) = request.memory_settings.as_ref() else {
        return Ok(());
    };
    let actual = manifest
        .memory
        .as_ref()
        .and_then(manifest::MemoryConfig::workspace_settings)
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
        let workspace_context = workspace_backend_ref.worker_context(
            &request.worker_ref,
            request.workspace_scope.as_ref(),
            self.worker_mutation_identity.as_ref(),
            self.runtime_request_audience.as_deref(),
            self.embedded_worker_mutation_dispatcher.as_ref(),
            Some(self.prompt_projection_cache.clone()),
        );
        let selector = profile.as_ref();
        let archive = self
            .resolve_profile_source_archive(
                &request.request.profile_source,
                request
                    .workspace_scope
                    .as_ref()
                    .map(|scope| scope.server_id.as_str()),
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
        let mut prepared = WorkerBootstrap::new(
            manifest,
            store,
            loader,
            workspace_context,
            filesystem_authority,
            WorkerBootstrapLayout::RuntimeManagedRun {
                run_dir: run_dir.clone(),
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
        let workspace_context = workspace_backend_ref.worker_context(
            &request.worker_ref,
            request.workspace_scope.as_ref(),
            self.worker_mutation_identity.as_ref(),
            self.runtime_request_audience.as_deref(),
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
        let started = PreparedWorker::new(
            worker,
            WorkerBootstrapLayout::RuntimeManagedRun {
                run_dir: run_dir.clone(),
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

struct RuntimeWorkerExecution {
    handle: WorkerHandle,
    shutdown: Arc<tokio::sync::Mutex<Option<worker::ShutdownReceiver>>>,
    busy: Arc<AtomicBool>,
    workspace_client: Option<Arc<dyn WorkspaceClient>>,
}

/// `worker-runtime` execution backend backed by real `worker` crate Workers.
pub struct WorkerRuntimeExecutionBackend<F = ProfileRuntimeWorkerFactory> {
    backend_id: String,
    factory: Arc<F>,
    working_directory_materializer: Option<Arc<dyn WorkingDirectoryMaterializer>>,
    runtime: Mutex<Option<Runtime>>,
    workers: Mutex<HashMap<crate::identity::WorkerRef, RuntimeWorkerExecution>>,
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

    fn get_execution(
        &self,
        handle: &WorkerExecutionHandle,
    ) -> Result<
        (
            WorkerHandle,
            Arc<AtomicBool>,
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
                    execution.busy.clone(),
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
        accepted_run_state: WorkerExecutionRunState,
    ) -> WorkerExecutionResult {
        self.run_on_adapter_runtime(async move {
            worker
                .send(method)
                .await
                .map_err(|err| format!("failed to send Worker method: {err}"))
        })
        .map(|_| WorkerExecutionResult::accepted(operation, accepted_run_state))
        .unwrap_or_else(|message| WorkerExecutionResult::errored(operation, message))
    }

    fn send_user_input_and_wait_for_commit(
        &self,
        operation: WorkerExecutionOperation,
        worker: WorkerHandle,
        method: Method,
        submission_id: String,
        accepted_run_state: WorkerExecutionRunState,
    ) -> WorkerExecutionResult {
        let acknowledged_submission_id = submission_id.clone();
        self.run_on_adapter_runtime(async move {
            // Subscribe before enqueueing the input so the acknowledgement cannot
            // race with a fast Worker commit. The opaque submission id is stored in
            // the same UserInput entry as the transformed Flow input and its state.
            let (_, mut committed_entries) = worker.sink.subscribe_with_snapshot();
            let committed_probe = worker.clone();
            let mut events = worker.subscribe();
            worker
                .send(method)
                .await
                .map_err(|err| format!("failed to send Worker method: {err}"))?;

            let timeout_probe = committed_probe.clone();
            let timeout_submission_id = submission_id.clone();
            let acknowledgement = tokio::time::timeout(USER_INPUT_COMMIT_TIMEOUT, async move {
                let input_was_committed = || {
                    committed_probe
                        .committed_entries()
                        .iter()
                        .any(|entry| user_input_has_submission(entry, &submission_id))
                };
                loop {
                    tokio::select! {
                        entry = committed_entries.recv() => {
                            match entry {
                                Ok(entry) if user_input_has_submission(&entry, &submission_id) => {
                                    return Ok(());
                                }
                                Ok(_) => {}
                                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                                    if input_was_committed() {
                                        return Ok(());
                                    }
                                    return Err(format!(
                                        "worker input commit acknowledgement lagged by {skipped} entry event(s)"
                                    ));
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                    if input_was_committed() {
                                        return Ok(());
                                    }
                                    return Err(
                                        "worker entry stream closed before user input was committed"
                                            .to_string(),
                                    );
                                }
                            }
                        }
                        event = events.recv() => {
                            match event {
                                Ok(Event::Error { message, .. }) => {
                                    if input_was_committed() {
                                        return Ok(());
                                    }
                                    return Err(format!(
                                        "worker rejected user input before session commit: {message}"
                                    ));
                                }
                                Ok(Event::Shutdown) => {
                                    if input_was_committed() {
                                        return Ok(());
                                    }
                                    return Err(
                                        "worker shut down before user input was committed".to_string()
                                    );
                                }
                                Ok(_) => {}
                                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                                    if input_was_committed() {
                                        return Ok(());
                                    }
                                    return Err(format!(
                                        "worker input commit acknowledgement lagged by {skipped} protocol event(s)"
                                    ));
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                    if input_was_committed() {
                                        return Ok(());
                                    }
                                    return Err(
                                        "worker event stream closed before user input was committed"
                                            .to_string(),
                                    );
                                }
                            }
                        }
                    }
                }
            })
            .await;

            match acknowledgement {
                Ok(result) => result,
                Err(_) => {
                    if timeout_probe
                        .committed_entries()
                        .iter()
                        .any(|entry| user_input_has_submission(entry, &timeout_submission_id))
                    {
                        Ok(())
                    } else {
                        Err("timed out waiting for worker user input commit".to_string())
                    }
                }
            }
        })
        .map(|_| {
            WorkerExecutionResult::accepted_input_committed(
                operation,
                accepted_run_state,
                acknowledged_submission_id,
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
        let busy = Arc::new(AtomicBool::new(false));
        #[cfg(feature = "ws-server")]
        {
            let streams = subscribe_worker_protocol_session(&handle);
            let mut events = streams.events;
            let mut entry_events = streams.log_entries;
            let bridge_handle = handle.clone();
            let bridge_busy = busy.clone();
            if let Err(message) = self.spawn_on_adapter_runtime(async move {
                loop {
                    tokio::select! {
                        event = events.recv() => {
                            match event {
                                Ok(event) => {
                                    let _ = bridge_context.publish_protocol_event(event);
                                    if matches!(
                                        bridge_handle.shared_state.get_status(),
                                        WorkerStatus::Idle | WorkerStatus::Paused
                                    ) {
                                        bridge_busy.store(false, Ordering::SeqCst);
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
        workers.insert(
            worker_ref.clone(),
            RuntimeWorkerExecution {
                handle,
                shutdown,
                busy,
                workspace_client,
            },
        );

        WorkerExecutionSpawnResult::Connected {
            handle: WorkerExecutionHandle::new(worker_ref, self.backend_id()),
            run_state: WorkerExecutionRunState::Idle,
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

fn method_starts_turn(method: &Method) -> bool {
    matches!(
        method,
        Method::Run { .. }
            | Method::RunTracked { .. }
            | Method::Notify { auto_run: true, .. }
            | Method::Resume
            | Method::Compact
    )
}

fn method_can_start_turn_from_status(method: &Method, status: WorkerStatus) -> bool {
    match method {
        Method::Resume => matches!(status, WorkerStatus::Idle | WorkerStatus::Paused),
        _ => status == WorkerStatus::Idle,
    }
}

fn accepted_notify_run_state(status: WorkerStatus, auto_run: bool) -> WorkerExecutionRunState {
    match status {
        WorkerStatus::Running => WorkerExecutionRunState::Busy,
        WorkerStatus::Idle if auto_run => WorkerExecutionRunState::Busy,
        WorkerStatus::Idle | WorkerStatus::Paused | WorkerStatus::Stopped => {
            WorkerExecutionRunState::Idle
        }
    }
}

fn accepted_run_state_for_method(method: &Method) -> WorkerExecutionRunState {
    match method {
        Method::Run { .. }
        | Method::RunTracked { .. }
        | Method::Notify { auto_run: true, .. }
        | Method::Resume
        | Method::Compact => WorkerExecutionRunState::Busy,
        Method::Shutdown => WorkerExecutionRunState::Stopped,
        _ => WorkerExecutionRunState::Idle,
    }
}

impl<F> WorkerExecutionBackend for WorkerRuntimeExecutionBackend<F>
where
    F: RuntimeWorkerFactory,
{
    fn backend_id(&self) -> &str {
        &self.backend_id
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
        let spawn_result =
            self.run_on_adapter_runtime(async move { factory.spawn_controller(request).await });

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
        let restore_result =
            self.run_on_adapter_runtime(async move { factory.restore_controller(request).await });

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
        let (worker, busy, _workspace_client) = match self.get_execution(handle) {
            Ok(execution) => execution,
            Err(mut result) => {
                result.operation = WorkerExecutionOperation::Input;
                return result;
            }
        };

        if input.kind == WorkerInputKind::Notify {
            let status = worker.shared_state.get_status();
            let accepted_run_state = accepted_notify_run_state(status, true);
            let claimed_here = status == WorkerStatus::Idle
                && busy
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok();
            let result = self.send_method(
                WorkerExecutionOperation::Input,
                worker,
                Method::Notify {
                    message: input.content,
                    auto_run: true,
                },
                accepted_run_state,
            );
            if claimed_here && result.outcome != crate::execution::WorkerExecutionOutcome::Accepted
            {
                busy.store(false, Ordering::SeqCst);
            }
            return result;
        }

        if worker.shared_state.get_status() != WorkerStatus::Idle
            || busy
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
        {
            return WorkerExecutionResult::busy(
                WorkerExecutionOperation::Input,
                "Worker is already running; runtime adapter v0 does not queue input",
            );
        }

        let (method, submission_id) = match input.kind {
            WorkerInputKind::User => {
                let Some(submission_id) = input
                    .submission_id
                    .filter(|submission_id| !submission_id.trim().is_empty())
                else {
                    busy.store(false, Ordering::SeqCst);
                    return WorkerExecutionResult::rejected(
                        WorkerExecutionOperation::Input,
                        "Runtime user input is missing its internal submission id",
                    );
                };
                (
                    Method::RunTracked {
                        input: input.segments.unwrap_or_else(|| {
                            vec![Segment::text(input.content.trim().to_string())]
                        }),
                        submission_id: submission_id.clone(),
                    },
                    Some(submission_id),
                )
            }
            WorkerInputKind::Notify => {
                unreachable!("Notify input is dispatched before the turn-start busy guard")
            }
            WorkerInputKind::Compact => (Method::Compact, None),
            WorkerInputKind::ListRewindTargets => (Method::ListRewindTargets, None),
            WorkerInputKind::RegisterPeer => (
                Method::RegisterPeer {
                    name: input.content.trim().to_string(),
                },
                None,
            ),
        };
        let accepted_run_state = match method {
            Method::Run { .. }
            | Method::RunTracked { .. }
            | Method::Notify { .. }
            | Method::Compact => WorkerExecutionRunState::Busy,
            _ => WorkerExecutionRunState::Idle,
        };
        let accepted_is_idle = accepted_run_state == WorkerExecutionRunState::Idle;
        let waits_for_user_input_commit = submission_id.is_some();

        let result = if waits_for_user_input_commit {
            self.send_user_input_and_wait_for_commit(
                WorkerExecutionOperation::Input,
                worker,
                method,
                submission_id.expect("tracked Run has submission id"),
                accepted_run_state,
            )
        } else {
            self.send_method(
                WorkerExecutionOperation::Input,
                worker,
                method,
                accepted_run_state,
            )
        };
        if accepted_is_idle || result.outcome != crate::execution::WorkerExecutionOutcome::Accepted
        {
            busy.store(false, Ordering::SeqCst);
        }
        result
    }

    fn dispatch_method(
        &self,
        handle: &WorkerExecutionHandle,
        method: Method,
    ) -> WorkerExecutionResult {
        let (worker, busy, _workspace_client) = match self.get_execution(handle) {
            Ok(execution) => execution,
            Err(mut result) => {
                result.operation = WorkerExecutionOperation::ProtocolMethod;
                return result;
            }
        };

        if let Method::Notify { auto_run, .. } = &method {
            let auto_run = *auto_run;
            let status = worker.shared_state.get_status();
            let accepted_run_state = accepted_notify_run_state(status, auto_run);
            let claimed_here = status == WorkerStatus::Idle
                && auto_run
                && busy
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok();
            let result = self.send_method(
                WorkerExecutionOperation::ProtocolMethod,
                worker,
                method,
                accepted_run_state,
            );
            if claimed_here && result.outcome != crate::execution::WorkerExecutionOutcome::Accepted
            {
                busy.store(false, Ordering::SeqCst);
            }
            return result;
        }

        let starts_turn = method_starts_turn(&method);
        if starts_turn
            && (!method_can_start_turn_from_status(&method, worker.shared_state.get_status())
                || busy
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_err())
        {
            return WorkerExecutionResult::busy(
                WorkerExecutionOperation::ProtocolMethod,
                "Worker is already running; runtime adapter v0 does not queue protocol methods",
            );
        }

        let accepted_run_state = accepted_run_state_for_method(&method);
        let accepted_is_idle = accepted_run_state == WorkerExecutionRunState::Idle;
        let result = self.send_method(
            WorkerExecutionOperation::ProtocolMethod,
            worker,
            method,
            accepted_run_state,
        );
        if (starts_turn && accepted_is_idle)
            || (starts_turn && result.outcome != crate::execution::WorkerExecutionOutcome::Accepted)
        {
            busy.store(false, Ordering::SeqCst);
        }
        result
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
            Ok(mut workers) => workers.remove(handle.worker_ref()),
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
        let shutdown = execution.shutdown.clone();
        let result = self.send_method(
            WorkerExecutionOperation::Stop,
            execution.handle,
            Method::Shutdown,
            WorkerExecutionRunState::Stopped,
        );
        if result.outcome != crate::execution::WorkerExecutionOutcome::Accepted {
            return result;
        }
        match self.run_on_adapter_runtime(async move {
            let receiver = shutdown.lock().await.take();
            if let Some(receiver) = receiver {
                receiver
                    .await
                    .map_err(|_| "Worker shutdown completion channel closed".to_string())?;
            }
            Ok(())
        }) {
            Ok(()) => result,
            Err(message) => WorkerExecutionResult::errored(WorkerExecutionOperation::Stop, message),
        }
    }

    fn cancel_worker(&self, handle: &WorkerExecutionHandle) -> WorkerExecutionResult {
        let (worker, _busy, _workspace_client) = match self.get_execution(handle) {
            Ok(execution) => execution,
            Err(mut result) => {
                result.operation = WorkerExecutionOperation::Cancel;
                return result;
            }
        };
        self.send_method(
            WorkerExecutionOperation::Cancel,
            worker,
            Method::Cancel,
            WorkerExecutionRunState::Idle,
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
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::Runtime as EmbeddedRuntime;
    use crate::catalog::{
        ConfigBundleRef, CreateWorkerRequest, MaterializerKind, ProfileSelector,
        RepositorySelector, WorkingDirectoryClaim, WorkingDirectoryRepository,
        WorkingDirectoryRequest,
    };
    use crate::execution::WorkerExecutionContext;
    use crate::identity::WorkerId;
    use crate::identity::WorkerRef;
    use crate::management::RuntimeOptions;
    use crate::observation::WorkerObservationCursor;
    use crate::working_directory::RuntimeGitCacheMaterializer;
    use agen::Engine;
    use agen::llm_client::event::{Event as LlmEvent, ResponseStatus, StatusEvent};
    use agen::llm_client::{ClientError, LlmClient, Request};
    use async_trait::async_trait;
    use futures::{Stream, StreamExt};
    use manifest::{Scope, WorkerManifest};
    use session_store::{LogEntry, WorkerMetadataStore};

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

    #[test]
    fn notify_run_state_allows_running_worker_inbox_delivery() {
        assert_eq!(
            accepted_notify_run_state(WorkerStatus::Running, true),
            WorkerExecutionRunState::Busy
        );
        assert_eq!(
            accepted_notify_run_state(WorkerStatus::Idle, true),
            WorkerExecutionRunState::Busy
        );
        assert_eq!(
            accepted_notify_run_state(WorkerStatus::Idle, false),
            WorkerExecutionRunState::Idle
        );
        assert_eq!(
            accepted_notify_run_state(WorkerStatus::Paused, true),
            WorkerExecutionRunState::Idle
        );
    }

    #[test]
    fn resume_turn_claim_accepts_paused_and_idle_but_not_running_status() {
        assert!(method_can_start_turn_from_status(
            &Method::Resume,
            WorkerStatus::Paused
        ));
        assert!(method_can_start_turn_from_status(
            &Method::Resume,
            WorkerStatus::Idle
        ));
        assert!(!method_can_start_turn_from_status(
            &Method::Resume,
            WorkerStatus::Running
        ));
        assert!(!method_can_start_turn_from_status(
            &Method::Compact,
            WorkerStatus::Paused
        ));
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
            let (handle, shutdown_rx) =
                WorkerController::spawn_runtime_managed(worker, &self.runtime_base)
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

    fn wait_for_adapter_state(
        backend: &WorkerRuntimeExecutionBackend<MockFactory>,
        worker_ref: &WorkerRef,
        expected_status: WorkerStatus,
        expected_busy: bool,
    ) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let matches = {
                let workers = backend.workers.lock().unwrap();
                let execution = workers.get(worker_ref).expect("live Worker execution");
                execution.handle.shared_state.get_status() == expected_status
                    && execution.busy.load(Ordering::SeqCst) == expected_busy
            };
            if matches {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for adapter state {expected_status:?}, busy={expected_busy}"
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
            materializer: MaterializerKind::RuntimeGitCache,
            backend_workdir_id: None,
            materialization: None,
        }
    }

    fn materialized_worktree_root(
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
                Some(serde_json::to_value(&manifest).unwrap()),
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
            controller.handle.shared_state.get_status(),
            WorkerStatus::Idle
        );
        assert!(!socket_path.exists());
        assert!(run_dir.join("worker.out.log").is_file());
        assert!(run_dir.join("worker.err.log").is_file());
        controller.handle.send(Method::Shutdown).await.unwrap();
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
                    .send(Method::Shutdown)
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
    fn create_with_initial_input_returns_after_session_commit() {
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

        let entries = backend
            .workers
            .lock()
            .unwrap()
            .get(&detail.worker_ref)
            .expect("live Worker execution")
            .handle
            .committed_entries();
        assert!(entries.iter().any(|entry| {
            matches!(
                entry,
                LogEntry::UserInput { segments, .. }
                | LogEntry::AnnotatedUserInput { segments, .. }
                    if segments == &vec![Segment::text("start the ticket")]
            )
        }));
        let submission_id = entries
            .iter()
            .find_map(|entry| {
                let extensions = match entry {
                    LogEntry::UserInput { extensions, .. }
                    | LogEntry::AnnotatedUserInput { extensions, .. } => extensions,
                    _ => return None,
                };
                extensions
                    .iter()
                    .find(|extension| extension.domain == WORKER_INPUT_SUBMISSION_EXTENSION_DOMAIN)
                    .and_then(|extension| extension.payload["submission_id"].as_str())
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
            .with_working_directory_materializer(RuntimeGitCacheMaterializer::new(
                runtime_base.path(),
            ));
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
        wait_for_adapter_state(&backend, &detail.worker_ref, WorkerStatus::Running, true);

        let running_resume = runtime
            .send_protocol_method(&detail.worker_ref, Method::Resume)
            .expect_err("Resume while Running must be rejected");
        assert!(
            running_resume
                .to_string()
                .contains("does not queue protocol methods"),
            "unexpected Running Resume error: {running_resume}"
        );

        runtime
            .send_protocol_method(&detail.worker_ref, Method::Pause)
            .expect("pause initial turn");
        wait_for_adapter_state(&backend, &detail.worker_ref, WorkerStatus::Paused, false);

        runtime
            .send_protocol_method(&detail.worker_ref, Method::Resume)
            .expect("resume paused turn");
        wait_for_adapter_state(&backend, &detail.worker_ref, WorkerStatus::Running, true);

        let duplicate_resume = runtime
            .send_protocol_method(&detail.worker_ref, Method::Resume)
            .expect_err("duplicate Resume must be rejected");
        assert!(
            duplicate_resume
                .to_string()
                .contains("does not queue protocol methods"),
            "unexpected duplicate Resume error: {duplicate_resume}"
        );

        runtime
            .send_protocol_method(&detail.worker_ref, Method::Pause)
            .expect("pause resumed turn");
        wait_for_adapter_state(&backend, &detail.worker_ref, WorkerStatus::Paused, false);
        runtime
            .send_protocol_method(&detail.worker_ref, Method::Resume)
            .expect("resume paused turn a second time");
        wait_for_adapter_state(&backend, &detail.worker_ref, WorkerStatus::Idle, false);
        assert_eq!(call_count.load(Ordering::SeqCst), 3);

        runtime
            .send_protocol_method(&detail.worker_ref, Method::Resume)
            .expect("Idle Resume preserves controller NotPaused semantics");
        wait_for_adapter_state(&backend, &detail.worker_ref, WorkerStatus::Idle, false);
        let events = runtime
            .read_worker_observation_events(&detail.worker_ref, WorkerObservationCursor::zero())
            .expect("read protocol events");
        assert!(events.iter().any(|event| {
            matches!(
                &event.payload,
                Event::Error {
                    code: protocol::ErrorCode::NotPaused,
                    ..
                }
            )
        }));
        assert_eq!(call_count.load(Ordering::SeqCst), 3);
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
            .with_working_directory_materializer(RuntimeGitCacheMaterializer::new(
                runtime_base.path(),
            ));
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
        let worktree_root = materialized_worktree_root(runtime_base.path(), &workdir_id);
        assert!(worktree_root.join("README.md").exists());

        runtime.stop_worker(&detail.worker_ref, None).unwrap();
        runtime.delete_worker(&detail.worker_ref).unwrap();

        assert!(worktree_root.join("README.md").exists());
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
            .with_working_directory_materializer(RuntimeGitCacheMaterializer::new(
                runtime_base.path(),
            ));
        let runtime =
            EmbeddedRuntime::with_execution_backend(RuntimeOptions::default(), Arc::new(backend))
                .unwrap();
        runtime.store_config_bundle(test_bundle()).unwrap();
        let status = runtime
            .create_working_directory(working_directory_request(repo.path()))
            .unwrap();
        let workdir_id = status.summary.working_directory_id.clone();
        let worktree_root = materialized_worktree_root(runtime_base.path(), &workdir_id);
        assert!(worktree_root.join("README.md").exists());
        let mut request = create_request("chat");
        request.working_directory = Some(WorkingDirectoryClaim {
            working_directory_id: workdir_id.clone(),
            relative_cwd: None,
        });

        let error = runtime.create_worker(request).unwrap_err();

        assert!(format!("{error:?}").contains("spawn failed"));
        assert!(worktree_root.join("README.md").exists());
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
            .with_working_directory_materializer(RuntimeGitCacheMaterializer::new(
                runtime_base.path(),
            ));
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
        assert!(working_directories_root.join(".repository-cache").is_dir());
    }
}
