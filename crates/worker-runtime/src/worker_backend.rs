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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use crate::catalog::{
    CreateWorkerRequest, ProfileSourceArchiveHttpRef, ProfileSourceArchiveSource,
    WorkingDirectoryRequest, WorkingDirectoryStatus,
};
use crate::execution::{
    WorkerExecutionBackend, WorkerExecutionHandle, WorkerExecutionOperation,
    WorkerExecutionRestoreRequest, WorkerExecutionResult, WorkerExecutionRunState,
    WorkerExecutionSpawnRequest, WorkerExecutionSpawnResult,
};
use crate::identity::WorkerRef;
use crate::interaction::{WorkerInput, WorkerInputKind};
use crate::resource::{BackendResourceClient, ProfileSourceArchiveCache};
use crate::working_directory::{
    WorkingDirectoryBinding, WorkingDirectoryDiagnostic, WorkingDirectoryMaterializer,
};
use async_trait::async_trait;
use manifest::paths;
use protocol::{Method, Segment, WorkerStatus};
use session_store::{CombinedStore, FsStore, FsWorkerStore, collect_state};
use tokio::runtime::Runtime;
#[cfg(feature = "ws-server")]
use tokio::sync::broadcast;
use workdir::{LocalWorkdirSession, Workdir, WorkdirSessionCapabilities, WorkdirSessionHandle};

use worker::feature::builtin::{
    CompositeWorkerObservationProvider, WorkerObservationError, WorkerObservationProvider,
    WorkerObservationSubject, WorkerObservationSubjectRef, WorkerSessionCapture,
    WorkspaceClientWorkerObservationProvider,
};
#[cfg(feature = "ws-server")]
use worker::ipc::protocol_session::{live_log_entry_event, subscribe_worker_protocol_session};
use worker::{
    PromptLoader, RuntimeWorkspaceHttpClient, SegmentLogSink, Worker, WorkerController,
    WorkerError, WorkerFilesystemAuthority, WorkerHandle, WorkerSharedState,
    WorkerWorkspaceContext, WorkspaceClient, WorkspaceId,
};

const DEFAULT_BACKEND_ID: &str = "worker-crate";
const RUNTIME_TASK_TIMEOUT: Duration = Duration::from_secs(10);
static NEXT_RUNTIME_ARTIFACT_ROOT: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
enum RuntimeArtifactRoot {
    Owned(Arc<OwnedRuntimeArtifactRoot>),
    External(PathBuf),
}

impl RuntimeArtifactRoot {
    fn owned() -> Self {
        let sequence = NEXT_RUNTIME_ARTIFACT_ROOT.fetch_add(1, Ordering::Relaxed);
        Self::Owned(Arc::new(OwnedRuntimeArtifactRoot {
            path: std::env::temp_dir().join(format!(
                "yoi-worker-runtime-artifacts-{}-{sequence}",
                std::process::id()
            )),
        }))
    }

    fn path(&self) -> &std::path::Path {
        match self {
            Self::Owned(root) => &root.path,
            Self::External(path) => path,
        }
    }
}

struct OwnedRuntimeArtifactRoot {
    path: PathBuf,
}

impl Drop for OwnedRuntimeArtifactRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

pub struct RuntimeWorkerController {
    pub handle: WorkerHandle,
    pub workspace_client: Arc<dyn WorkspaceClient>,
}

/// Factory seam used by [`WorkerRuntimeExecutionBackend`] to construct a real
/// controller-backed Worker for a Runtime catalog entry.
#[async_trait]
pub trait RuntimeWorkerFactory: Send + Sync + 'static {
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
            let Ok(worker_ref) = grant.local_worker_ref() else {
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
        let worker_ref = grant
            .local_worker_ref()
            .map_err(|_| WorkerObservationError::NotFound)?;
        let (workspace_id, _, sink) = self
            .hub
            .get(&worker_ref)
            .ok_or(WorkerObservationError::NotFound)?;
        if workspace_id.as_deref() != Some(self.workspace_id.as_str()) {
            return Err(WorkerObservationError::NotFound);
        }
        let entries = sink.subscribe_with_snapshot().0;
        let state = collect_state(&entries);
        Ok(WorkerSessionCapture {
            segment_id: format!("runtime:{runtime_id}:worker:{worker_id}"),
            items: state.history,
        })
    }
}

#[derive(Clone)]
pub struct ProfileRuntimeWorkerFactory {
    observation_hub: Arc<RuntimeWorkerObservationHub>,
    profile_base_dir: PathBuf,
    store_dir: Option<PathBuf>,
    worker_metadata_dir: Option<PathBuf>,
    runtime_base_dir: RuntimeArtifactRoot,
    resource_client: Option<Arc<dyn BackendResourceClient>>,
    profile_archive_cache: Arc<ProfileSourceArchiveCache>,
}

impl ProfileRuntimeWorkerFactory {
    pub fn new(profile_base_dir: impl Into<PathBuf>) -> Self {
        let profile_base_dir = profile_base_dir.into();
        Self {
            observation_hub: Arc::new(RuntimeWorkerObservationHub::default()),
            profile_base_dir,
            store_dir: None,
            worker_metadata_dir: None,
            runtime_base_dir: RuntimeArtifactRoot::owned(),
            resource_client: None,
            profile_archive_cache: Arc::new(ProfileSourceArchiveCache::default()),
        }
    }

    pub fn with_store_dir(mut self, store_dir: impl Into<PathBuf>) -> Self {
        self.store_dir = Some(store_dir.into());
        self
    }

    pub fn with_worker_metadata_dir(mut self, worker_metadata_dir: impl Into<PathBuf>) -> Self {
        self.worker_metadata_dir = Some(worker_metadata_dir.into());
        self
    }

    pub fn with_runtime_base_dir(mut self, runtime_base_dir: impl Into<PathBuf>) -> Self {
        self.runtime_base_dir = RuntimeArtifactRoot::External(runtime_base_dir.into());
        self
    }

    pub fn with_resource_client(mut self, resource_client: Arc<dyn BackendResourceClient>) -> Self {
        self.resource_client = Some(resource_client);
        self
    }

    fn store_dir(&self) -> Result<PathBuf, String> {
        self.store_dir
            .clone()
            .or_else(paths::sessions_dir)
            .ok_or_else(|| {
                "could not resolve sessions directory (set YOI_DATA_DIR, YOI_HOME, XDG_DATA_HOME, or HOME)"
                    .to_string()
            })
    }

    fn worker_metadata_dir(&self, store_dir: &std::path::Path) -> PathBuf {
        self.worker_metadata_dir
            .clone()
            .or_else(|| paths::data_dir().map(|data_dir| data_dir.join("workers")))
            .or_else(|| store_dir.parent().map(|parent| parent.join("workers")))
            .unwrap_or_else(|| PathBuf::from("workers"))
    }

    fn runtime_base_dir(&self) -> Result<PathBuf, String> {
        Ok(self.runtime_base_dir.path().to_path_buf())
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
    ) -> Result<(manifest::WorkerManifest, PromptLoader), String> {
        let mut config = manifest::WorkerManifestConfig::builtin_defaults();
        config.worker.name = Some(worker_name.to_string());
        let manifest = manifest::WorkerManifest::try_from(config)
            .map_err(|err| format!("failed to build restore fallback manifest: {err}"))?;
        Ok((manifest, PromptLoader::builtins_only()))
    }
    async fn resolve_profile_source_archive(
        &self,
        source: &ProfileSourceArchiveSource,
    ) -> Result<crate::profile_archive::VerifiedProfileSourceArchive, String> {
        match source {
            ProfileSourceArchiveSource::Embedded { archive } => archive
                .verify()
                .map_err(|err| format!("failed to verify embedded profile source archive: {err}")),
            ProfileSourceArchiveSource::Http { location } => {
                self.fetch_profile_source_archive(location).await
            }
        }
    }

    async fn fetch_profile_source_archive(
        &self,
        location: &ProfileSourceArchiveHttpRef,
    ) -> Result<crate::profile_archive::VerifiedProfileSourceArchive, String> {
        if let Some(cached) = self.profile_archive_cache.get(&location.archive.digest) {
            let response =
                fetch_profile_source_archive_http(location, Some(&location.archive.digest)).await?;
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
            let archive = fetch_profile_source_archive_http(location, None)
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
    fn from_worker_request(request: &CreateWorkerRequest) -> Self {
        if let Some(api) = request.workspace_api.as_ref()
            && let Some(runtime_id) = api
                .runtime_id
                .as_ref()
                .filter(|runtime_id| !runtime_id.trim().is_empty())
        {
            return Self::Http {
                workspace_id: api.workspace_id.clone(),
                base_url: api.base_url.clone(),
                runtime_id: runtime_id.clone(),
            };
        }
        Self::None
    }

    fn worker_context(&self, worker_ref: &WorkerRef) -> WorkerWorkspaceContext {
        match self {
            Self::None => WorkerWorkspaceContext::no_workspace(),
            Self::Http {
                workspace_id,
                base_url,
                runtime_id,
            } => WorkerWorkspaceContext::with_client(
                WorkspaceId::new(workspace_id.clone()).ok(),
                Arc::new(RuntimeWorkspaceHttpClient::new(
                    workspace_id.clone(),
                    base_url.clone(),
                    runtime_id.clone(),
                    worker_ref.worker_id.to_string(),
                )),
            ),
        }
    }
}

#[cfg(feature = "http-server")]
async fn fetch_profile_source_archive_http(
    location: &ProfileSourceArchiveHttpRef,
    cached_digest: Option<&str>,
) -> Result<Option<crate::profile_archive::ProfileSourceArchive>, String> {
    let client = reqwest::Client::new();
    let mut request = client.get(&location.url);
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
) -> WorkdirSessionHandle {
    Arc::new(LocalWorkdirSession::materialized_bound(
        Workdir::new(workdir_id),
        root.to_path_buf(),
        cwd.to_path_buf(),
        scope,
        WorkdirSessionCapabilities::ALL,
    ))
}

#[async_trait]
impl RuntimeWorkerFactory for ProfileRuntimeWorkerFactory {
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
        let workspace_backend_ref =
            RuntimeWorkspaceBackendRef::from_worker_request(&request.request);
        let observation_runtime_id = request
            .request
            .workspace_api
            .as_ref()
            .and_then(|api| api.runtime_id.clone());
        let observation_workspace_id = request
            .request
            .workspace_api
            .as_ref()
            .map(|api| api.workspace_id.clone());
        let observation_grants = request.request.worker_observation_grants.clone();
        let observation_enabled = request.request.worker_observation_enabled;
        let workspace_context = workspace_backend_ref.worker_context(&request.worker_ref);
        let selector = profile.as_ref();
        let archive = self
            .resolve_profile_source_archive(&request.request.profile_source)
            .await?;
        let (manifest, loader) = {
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
        let flow_transition_enabled = manifest.feature.flow.enabled;

        let store_dir = self.store_dir()?;
        let session_store = FsStore::new(&store_dir).map_err(|err| {
            format!(
                "failed to initialize session store at {}: {err}",
                store_dir.display()
            )
        })?;
        let worker_metadata_dir = self.worker_metadata_dir(&store_dir);
        let worker_metadata_store = FsWorkerStore::new(&worker_metadata_dir).map_err(|err| {
            format!(
                "failed to initialize worker metadata store at {}: {err}",
                worker_metadata_dir.display()
            )
        })?;
        let store = CombinedStore::new(session_store, worker_metadata_store);

        let mut worker = Worker::from_manifest_with_context(
            manifest,
            store,
            loader,
            workspace_context,
            filesystem_authority,
        )
        .await
        .map_err(|err| format!("failed to create Worker from profile: {err}"))?;
        if let Some(binding) = request.working_directory.as_ref() {
            worker.bind_workdir_session(Some(runtime_local_workdir_session(
                &binding.working_directory.id,
                binding.root(),
                binding.cwd(),
                worker.scope().clone(),
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
        let runtime_base = self.runtime_base_dir()?;
        let (handle, _shutdown_rx) = WorkerController::spawn_runtime_managed(worker, &runtime_base)
            .await
            .map_err(|err| format!("failed to spawn Worker controller: {err}"))?;
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
        let workspace_backend_ref =
            RuntimeWorkspaceBackendRef::from_worker_request(&request.request);
        let observation_runtime_id = request
            .request
            .workspace_api
            .as_ref()
            .and_then(|api| api.runtime_id.clone());
        let observation_workspace_id = request
            .request
            .workspace_api
            .as_ref()
            .map(|api| api.workspace_id.clone());
        let observation_grants = request.request.worker_observation_grants.clone();
        let observation_enabled = request.request.worker_observation_enabled;
        let workspace_context = workspace_backend_ref.worker_context(&request.worker_ref);
        let (manifest, loader) = Self::restore_fallback_manifest(&worker_name)?;

        let store_dir = self.store_dir()?;
        let session_store = FsStore::new(&store_dir).map_err(|err| {
            format!(
                "failed to initialize session store at {}: {err}",
                store_dir.display()
            )
        })?;
        let worker_metadata_dir = self.worker_metadata_dir(&store_dir);
        let worker_metadata_store = FsWorkerStore::new(&worker_metadata_dir).map_err(|err| {
            format!(
                "failed to initialize worker metadata store at {}: {err}",
                worker_metadata_dir.display()
            )
        })?;
        let store = CombinedStore::new(session_store, worker_metadata_store);

        let mut worker = match Worker::restore_from_worker_metadata_with_context(
            &worker_name,
            manifest.clone(),
            store,
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
                let session_store = FsStore::new(&store_dir).map_err(|err| {
                    format!(
                        "failed to initialize session store at {}: {err}",
                        store_dir.display()
                    )
                })?;
                let worker_metadata_store =
                    FsWorkerStore::new(&worker_metadata_dir).map_err(|err| {
                        format!(
                            "failed to initialize worker metadata store at {}: {err}",
                            worker_metadata_dir.display()
                        )
                    })?;
                let store = CombinedStore::new(session_store, worker_metadata_store);
                Worker::restore_pending_from_worker_metadata_with_context(
                    &worker_name,
                    manifest.clone(),
                    store,
                    loader,
                    workspace_context,
                    filesystem_authority,
                )
                .await
                .map_err(|err| format!("failed to recreate pending Worker from metadata: {err}"))?
            }
            Err(err) => return Err(format!("failed to restore Worker from metadata: {err}")),
        };
        let flow_transition_enabled = worker.manifest().feature.flow.enabled;
        if let Some(binding) = request.working_directory.as_ref() {
            worker.bind_workdir_session(Some(runtime_local_workdir_session(
                &binding.working_directory.id,
                binding.root(),
                binding.cwd(),
                worker.scope().clone(),
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
        let runtime_base = self.runtime_base_dir()?;
        let (handle, _shutdown_rx) = WorkerController::spawn_runtime_managed(worker, &runtime_base)
            .await
            .map_err(|err| format!("failed to spawn restored Worker controller: {err}"))?;
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
            workspace_client,
        })
    }
}

struct RuntimeWorkerExecution {
    handle: WorkerHandle,
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
        Self::new(ProfileRuntimeWorkerFactory::new(workspace_root))
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

    fn connect_handle(
        &self,
        operation: WorkerExecutionOperation,
        worker_ref: crate::identity::WorkerRef,
        bridge_context: crate::execution::WorkerExecutionContext,
        handle: WorkerHandle,
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
                                    if bridge_handle.shared_state.get_status() == WorkerStatus::Idle {
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
            | Method::Notify { auto_run: true, .. }
            | Method::Resume
            | Method::Compact
    )
}

fn accepted_notify_run_state(status: WorkerStatus, auto_run: bool) -> WorkerExecutionRunState {
    match status {
        WorkerStatus::Running => WorkerExecutionRunState::Busy,
        WorkerStatus::Idle if auto_run => WorkerExecutionRunState::Busy,
        WorkerStatus::Idle | WorkerStatus::Paused => WorkerExecutionRunState::Idle,
    }
}

fn accepted_run_state_for_method(method: &Method) -> WorkerExecutionRunState {
    match method {
        Method::Run { .. }
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

        let method = match input.kind {
            WorkerInputKind::User => Method::Run {
                input: input
                    .segments
                    .unwrap_or_else(|| vec![Segment::text(input.content.trim().to_string())]),
            },
            WorkerInputKind::Notify => {
                unreachable!("Notify input is dispatched before the turn-start busy guard")
            }
            WorkerInputKind::Compact => Method::Compact,
            WorkerInputKind::ListRewindTargets => Method::ListRewindTargets,
            WorkerInputKind::RegisterPeer => Method::RegisterPeer {
                name: input.content.trim().to_string(),
            },
        };
        let accepted_run_state = match method {
            Method::Run { .. } | Method::Notify { .. } | Method::Compact => {
                WorkerExecutionRunState::Busy
            }
            _ => WorkerExecutionRunState::Idle,
        };
        let accepted_is_idle = accepted_run_state == WorkerExecutionRunState::Idle;

        let result = self.send_method(
            WorkerExecutionOperation::Input,
            worker,
            method,
            accepted_run_state,
        );
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
            && (worker.shared_state.get_status() != WorkerStatus::Idle
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
        self.send_method(
            WorkerExecutionOperation::Stop,
            execution.handle,
            Method::Shutdown,
            WorkerExecutionRunState::Stopped,
        )
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
    use crate::identity::WorkerRef;
    use crate::management::RuntimeOptions;
    use crate::observation::WorkerObservationCursor;
    use crate::working_directory::LocalGitWorktreeMaterializer;
    use async_trait::async_trait;
    use futures::Stream;
    use llm_engine::Engine;
    use llm_engine::llm_client::event::{Event as LlmEvent, ResponseStatus, StatusEvent};
    use llm_engine::llm_client::{ClientError, LlmClient, Request};
    use manifest::{Scope, WorkerManifest};
    use session_store::WorkerMetadataStore;

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

    #[derive(Clone)]
    struct MockClient {
        responses: Arc<Vec<Vec<LlmEvent>>>,
        call_count: Arc<AtomicUsize>,
        captured: Arc<Mutex<Vec<Request>>>,
    }

    impl MockClient {
        fn new(events: Vec<LlmEvent>) -> Self {
            Self {
                responses: Arc::new(vec![events]),
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
            let events = self.responses.get(idx).cloned().unwrap_or_default();
            Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
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
            let workspace_backend_ref =
                RuntimeWorkspaceBackendRef::from_worker_request(&request.request);
            let workspace_context = workspace_backend_ref.worker_context(&request.worker_ref);
            let workspace_client = workspace_context.client_handle();
            self.observed_workspace_clients.lock().unwrap().push((
                workspace_client.kind().to_string(),
                workspace_client.workspace_id().map(str::to_string),
                workspace_client.is_available(),
            ));
            let scope = Scope::writable(&scope_root).map_err(|err| err.to_string())?;
            let worker = Worker::new(
                manifest,
                Engine::new(self.client.clone()),
                store,
                workspace_context,
                filesystem_authority,
                scope,
            )
            .await
            .map_err(|err| err.to_string())?;
            let (handle, _shutdown_rx) =
                WorkerController::spawn_runtime_managed(worker, &self.runtime_base)
                    .await
                    .map_err(|err| err.to_string())?;
            Ok(RuntimeWorkerController {
                handle,
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
            profile_source_archive: Some(sample_profile_archive()),
            profile_source_archive_handle: None,
        }
        .with_computed_digest()
    }

    fn sample_profile_archive() -> crate::profile_archive::ProfileSourceArchive {
        let entrypoints =
            BTreeMap::from([("default".to_string(), "profiles/default.dcdl".to_string())]);
        let sources = BTreeMap::from([(
            "profiles/default.dcdl".to_string(),
            r#"{
                slug = "default";
                description = "Default";
                scope = "workspace_read";
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
            idempotency_key: None,
            idempotency_fingerprint: None,
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
                uri: ".".to_string(),
                local_path: Some(repo.to_path_buf()),
                selector: Some(RepositorySelector::from("HEAD")),
            },
            materializer: MaterializerKind::LocalGitWorktree,
            backend_workdir_id: None,
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
        let worker_ref = WorkerRef::new(crate::identity::WorkerId::new(7));
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
        let grant = crate::identity::RuntimeWorkerRef::new("runtime-1", "7");
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
                worker_id: "7".to_string(),
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
    fn runtime_worker_name_is_runtime_local() {
        let worker_ref = crate::identity::WorkerRef::new(crate::identity::WorkerId::new(1));
        let request = WorkerExecutionSpawnRequest {
            worker_ref: worker_ref.clone(),
            request: create_request("1"),
            context: test_execution_context(worker_ref),
            working_directory: None,
            config_bundle: None,
        };

        assert_eq!(
            ProfileRuntimeWorkerFactory::runtime_worker_name(&request),
            "worker-runtime-1"
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
        );
        let restored = runtime_local_workdir_session(
            "working-directory-42",
            root.path(),
            root.path(),
            manifest::SharedScope::new(Scope::writable(root.path()).unwrap()),
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
            .resolve_profile_source_archive(&source)
            .await
            .expect("embedded archive should resolve without Backend resource client");
    }

    #[tokio::test]
    async fn restore_pending_worker_uses_saved_manifest_snapshot() {
        let root = tempfile::tempdir().unwrap();
        let store_dir = root.path().join("sessions");
        let worker_metadata_dir = root.path().join("workers");
        let worker_ref = WorkerRef::new(crate::identity::WorkerId::new(1));
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

                [feature.flow]
                enabled = true

                [[scope.allow]]
                target = "{}"
                permission = "write"
            "#,
            worker_name,
            root.path().display(),
            root.path().display(),
        ))
        .unwrap();
        FsWorkerStore::new(&worker_metadata_dir)
            .unwrap()
            .set_active(
                &worker_name,
                Some(session_store::WorkerActiveSegmentRef::pending_segment(
                    session_id,
                )),
                Some(serde_json::to_value(&manifest).unwrap()),
            )
            .unwrap();

        let mut request = create_request("restore");
        request.workspace_api = Some(crate::catalog::WorkspaceApiRef {
            workspace_id: "workspace-restore".to_string(),
            base_url: "http://workspace.invalid".to_string(),
            runtime_id: Some("runtime-restore".to_string()),
        });
        let controller = ProfileRuntimeWorkerFactory::new(root.path())
            .with_store_dir(&store_dir)
            .with_worker_metadata_dir(&worker_metadata_dir)
            .restore_controller(WorkerExecutionRestoreRequest {
                worker_ref: worker_ref.clone(),
                request,
                context: test_execution_context(worker_ref),
                previous_working_directory: None,
                working_directory: None,
                config_bundle: None,
            })
            .await
            .expect("pending restore should use the saved manifest snapshot");
        assert!(controller.handle.shared_state.flow_transition_enabled());

        controller.handle.send(Method::Shutdown).await.unwrap();
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
            runtime_id: Some("runtime-test".to_string()),
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
                "runtime-http-proxy".to_string(),
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
            .with_working_directory_materializer(LocalGitWorktreeMaterializer::new(
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
            .with_working_directory_materializer(LocalGitWorktreeMaterializer::new(
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
            .with_working_directory_materializer(LocalGitWorktreeMaterializer::new(
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
            .with_working_directory_materializer(LocalGitWorktreeMaterializer::new(
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
        let remaining_entries = fs::read_dir(working_directories_root)
            .map(|entries| entries.count())
            .unwrap_or(0);
        assert_eq!(remaining_entries, 0);
    }
}
