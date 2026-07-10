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
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use crate::catalog::{
    ProfileSourceArchiveHttpRef, ProfileSourceArchiveSource, WorkingDirectoryRequest,
    WorkingDirectoryStatus,
};
use crate::execution::{
    WorkerExecutionBackend, WorkerExecutionHandle, WorkerExecutionOperation, WorkerExecutionResult,
    WorkerExecutionRunState, WorkerExecutionSpawnRequest, WorkerExecutionSpawnResult,
};
use crate::interaction::{WorkerInput, WorkerInputKind};
use crate::resource::{BackendResourceClient, ProfileSourceArchiveCache};
use crate::working_directory::{
    WorkingDirectoryBinding, WorkingDirectoryDiagnostic, WorkingDirectoryMaterializer,
};
use async_trait::async_trait;
use manifest::paths;
use protocol::{Method, Segment, WorkerStatus};
use session_store::FsStore;
use session_store::{CombinedStore, FsWorkerStore};
use tokio::runtime::Runtime;
use tokio::sync::broadcast;

use worker::{Worker, WorkerController, WorkerFilesystemAuthority, WorkerHandle};

const DEFAULT_BACKEND_ID: &str = "worker-crate";
const RUNTIME_TASK_TIMEOUT: Duration = Duration::from_secs(10);

/// Factory seam used by [`WorkerRuntimeExecutionBackend`] to construct a real
/// controller-backed Worker for a Runtime catalog entry.
#[async_trait]
pub trait RuntimeWorkerFactory: Send + Sync + 'static {
    async fn spawn_controller(
        &self,
        request: WorkerExecutionSpawnRequest,
    ) -> Result<WorkerHandle, String>;
}

/// Production factory that resolves a normal Worker profile and spawns it under
/// `WorkerController`.
#[derive(Clone)]
pub struct ProfileRuntimeWorkerFactory {
    profile_base_dir: PathBuf,
    cwd: PathBuf,
    store_dir: Option<PathBuf>,
    worker_metadata_dir: Option<PathBuf>,
    profile: Option<String>,
    runtime_base_dir: Option<PathBuf>,
    resource_client: Option<Arc<dyn BackendResourceClient>>,
    profile_archive_cache: Arc<ProfileSourceArchiveCache>,
}

impl ProfileRuntimeWorkerFactory {
    pub fn new(profile_base_dir: impl Into<PathBuf>) -> Self {
        let profile_base_dir = profile_base_dir.into();
        Self {
            cwd: profile_base_dir.clone(),
            profile_base_dir,
            store_dir: None,
            worker_metadata_dir: None,
            profile: None,
            runtime_base_dir: None,
            resource_client: None,
            profile_archive_cache: Arc::new(ProfileSourceArchiveCache::default()),
        }
    }

    pub fn with_cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = cwd.into();
        self
    }

    pub fn with_store_dir(mut self, store_dir: impl Into<PathBuf>) -> Self {
        self.store_dir = Some(store_dir.into());
        self
    }

    pub fn with_worker_metadata_dir(mut self, worker_metadata_dir: impl Into<PathBuf>) -> Self {
        self.worker_metadata_dir = Some(worker_metadata_dir.into());
        self
    }

    /// Set the profile selector used for Runtime-created Workers. When unset,
    /// normal default profile discovery is used.
    pub fn with_profile(mut self, profile: impl Into<String>) -> Self {
        self.profile = Some(profile.into());
        self
    }

    pub fn with_runtime_base_dir(mut self, runtime_base_dir: impl Into<PathBuf>) -> Self {
        self.runtime_base_dir = Some(runtime_base_dir.into());
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
                "could not resolve sessions directory (set YOI_HOME, YOI_DATA_DIR, or HOME)"
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
        self.runtime_base_dir
            .clone()
            .or_else(|| worker::runtime::dir::default_base().ok())
            .ok_or_else(|| "could not resolve worker runtime directory".to_string())
    }

    fn runtime_worker_name(request: &WorkerExecutionSpawnRequest) -> String {
        format!(
            "runtime-{}-{}",
            sanitize_worker_name_component(request.worker_ref.runtime_id.as_str()),
            request.worker_ref.worker_id
        )
    }

    fn runtime_profile_value(
        profile: &crate::catalog::ProfileSelector,
    ) -> Option<std::borrow::Cow<'_, str>> {
        match profile {
            crate::catalog::ProfileSelector::RuntimeDefault => None,
            crate::catalog::ProfileSelector::Named(name) => {
                Some(std::borrow::Cow::Borrowed(name.as_str()))
            }
            crate::catalog::ProfileSelector::Builtin(name) => {
                if name.starts_with("builtin:") {
                    Some(std::borrow::Cow::Borrowed(name.as_str()))
                } else {
                    Some(std::borrow::Cow::Owned(format!("builtin:{name}")))
                }
            }
        }
    }

    fn runtime_profile<'a>(
        &'a self,
        request: &'a WorkerExecutionSpawnRequest,
    ) -> Option<std::borrow::Cow<'a, str>> {
        if let Some(profile) = self.profile.as_deref() {
            return Some(std::borrow::Cow::Borrowed(profile));
        }
        Self::runtime_profile_value(&request.request.profile)
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

fn sanitize_worker_name_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '-'
            }
        })
        .collect()
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

#[async_trait]
impl RuntimeWorkerFactory for ProfileRuntimeWorkerFactory {
    async fn spawn_controller(
        &self,
        request: WorkerExecutionSpawnRequest,
    ) -> Result<WorkerHandle, String> {
        let worker_name = Self::runtime_worker_name(&request);
        let profile = self.runtime_profile(&request);
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
        let selector = profile.as_deref().unwrap_or("builtin:default");
        let archive = self
            .resolve_profile_source_archive(&request.request.profile_source)
            .await?;
        let (mut manifest, loader) = {
            let manifest = archive
                .resolve_profile(selector, &worker_root, &worker_name)
                .map_err(|err| format!("failed to resolve profile source archive: {err}"))?;
            worker::entrypoint::resolve_runtime_profile_manifest_from_manifest(
                manifest,
                &worker_root,
                &worker_name,
            )?
        };
        manifest.worker.name = worker_name;

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

        let worker = Worker::from_manifest_with_context(
            manifest,
            store,
            loader,
            worker_root,
            filesystem_authority,
        )
        .await
        .map_err(|err| format!("failed to create Worker from profile: {err}"))?;

        let runtime_base = self.runtime_base_dir()?;
        let (handle, _shutdown_rx) = WorkerController::spawn(worker, &runtime_base)
            .await
            .map_err(|err| format!("failed to spawn Worker controller: {err}"))?;
        Ok(handle)
    }
}

struct RuntimeWorkerExecution {
    handle: WorkerHandle,
    busy: Arc<AtomicBool>,
    working_directory: Option<WorkingDirectoryBinding>,
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
            let _ = tx.send(task.await);
        })?;
        Self::wait_for_runtime_task(rx)
    }

    fn get_execution(
        &self,
        handle: &WorkerExecutionHandle,
    ) -> Result<(WorkerHandle, Arc<AtomicBool>), WorkerExecutionResult> {
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
            .map(|execution| (execution.handle.clone(), execution.busy.clone()))
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

        let handle = match spawn_result {
            Ok(handle) => handle,
            Err(message) => {
                if let (Some(materializer), Some(binding)) = (
                    self.working_directory_materializer.as_ref(),
                    working_directory.as_ref(),
                ) {
                    let _ = materializer.cleanup(binding);
                }
                return WorkerExecutionSpawnResult::Errored(WorkerExecutionResult::errored(
                    WorkerExecutionOperation::Spawn,
                    message,
                ));
            }
        };

        let mut events = handle.subscribe();
        let bridge_handle = handle.clone();
        let busy = Arc::new(AtomicBool::new(false));
        let bridge_busy = busy.clone();
        if let Err(message) = self.spawn_on_adapter_runtime(async move {
            loop {
                match events.recv().await {
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
        }) {
            return WorkerExecutionSpawnResult::Errored(WorkerExecutionResult::errored(
                WorkerExecutionOperation::Spawn,
                message,
            ));
        }

        let mut workers = match self.workers.lock() {
            Ok(workers) => workers,
            Err(_) => {
                return WorkerExecutionSpawnResult::Errored(WorkerExecutionResult::errored(
                    WorkerExecutionOperation::Spawn,
                    "worker adapter registry lock is poisoned",
                ));
            }
        };
        workers.insert(
            worker_ref.clone(),
            RuntimeWorkerExecution {
                handle,
                busy,
                working_directory: working_directory.clone(),
            },
        );

        WorkerExecutionSpawnResult::Connected {
            handle: WorkerExecutionHandle::new(worker_ref, self.backend_id()),
            run_state: WorkerExecutionRunState::Idle,
            working_directory: working_directory.map(|binding| binding.status()),
        }
    }

    fn dispatch_input(
        &self,
        handle: &WorkerExecutionHandle,
        input: WorkerInput,
    ) -> WorkerExecutionResult {
        let (worker, busy) = match self.get_execution(handle) {
            Ok(execution) => execution,
            Err(mut result) => {
                result.operation = WorkerExecutionOperation::Input;
                return result;
            }
        };

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

        let WorkerInputKind::User = input.kind else {
            busy.store(false, Ordering::SeqCst);
            return WorkerExecutionResult::unsupported(
                WorkerExecutionOperation::Input,
                "runtime adapter currently dispatches user input only",
            );
        };
        let content = input.content.trim().to_string();
        if content.is_empty() {
            busy.store(false, Ordering::SeqCst);
            return WorkerExecutionResult::rejected(
                WorkerExecutionOperation::Input,
                "runtime adapter rejects empty user input",
            );
        }

        let result = self.send_method(
            WorkerExecutionOperation::Input,
            worker,
            Method::Run {
                input: vec![Segment::text(content)],
            },
            WorkerExecutionRunState::Busy,
        );
        if result.outcome != crate::execution::WorkerExecutionOutcome::Accepted {
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
        let result = self.send_method(
            WorkerExecutionOperation::Stop,
            execution.handle,
            Method::Shutdown,
            WorkerExecutionRunState::Stopped,
        );
        if let (Some(materializer), Some(binding)) = (
            self.working_directory_materializer.as_ref(),
            execution.working_directory.as_ref(),
        ) {
            let _ = materializer.cleanup(binding);
        }
        result
    }

    fn cancel_worker(&self, handle: &WorkerExecutionHandle) -> WorkerExecutionResult {
        let (worker, _busy) = match self.get_execution(handle) {
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
        ConfigBundleRef, CreateWorkerRequest, DirtyStatePolicy, MaterializerKind, ProfileSelector,
        RepositorySelector, WorkingDirectoryRepository, WorkingDirectoryRequest,
    };
    use crate::execution::WorkerExecutionContext;
    use crate::identity::RuntimeId;
    use crate::management::RuntimeOptions;
    use crate::observation::WorkerObservationCursor;
    use crate::working_directory::LocalGitWorktreeMaterializer;
    use async_trait::async_trait;
    use futures::Stream;
    use llm_engine::Engine;
    use llm_engine::llm_client::event::{Event as LlmEvent, ResponseStatus, StatusEvent};
    use llm_engine::llm_client::{ClientError, LlmClient, Request};
    use manifest::{Scope, WorkerManifest};

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

    struct MockFactory {
        client: MockClient,
        runtime_base: PathBuf,
        cwd: PathBuf,
        store_dir: PathBuf,
        worker_metadata_dir: PathBuf,
        observed_cwds: Arc<Mutex<Vec<PathBuf>>>,
    }

    #[async_trait]
    impl RuntimeWorkerFactory for MockFactory {
        async fn spawn_controller(
            &self,
            request: WorkerExecutionSpawnRequest,
        ) -> Result<WorkerHandle, String> {
            let manifest = WorkerManifest::from_toml(
                r#"
                [worker]
                name = "runtime-adapter-test"
                pwd = "./"

                [model]
                scheme = "anthropic"
                model_id = "test-model"

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
            let scope = Scope::writable(&scope_root).map_err(|err| err.to_string())?;
            let worker = Worker::new(
                manifest,
                Engine::new(self.client.clone()),
                store,
                self.cwd.clone(),
                filesystem_authority,
                scope,
            )
            .await
            .map_err(|err| err.to_string())?;
            let (handle, _shutdown_rx) = WorkerController::spawn(worker, &self.runtime_base)
                .await
                .map_err(|err| err.to_string())?;
            Ok(handle)
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
                selector: ProfileSelector::RuntimeDefault,
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
            profile: ProfileSelector::RuntimeDefault,
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
            dirty_state_policy: DirtyStatePolicy::CleanPointOnly,
            backend_workdir_id: None,
        }
    }

    #[test]
    fn runtime_worker_name_is_namespaced_by_runtime_id() {
        let runtime_id = RuntimeId::new("arc:remote".to_string()).unwrap();
        let worker_ref = crate::identity::WorkerRef::new(
            runtime_id,
            crate::identity::WorkerId::new("worker-00000001".to_string()).unwrap(),
        );
        let request = WorkerExecutionSpawnRequest {
            worker_ref: worker_ref.clone(),
            request: create_request("worker-00000001"),
            context: WorkerExecutionContext::new(worker_ref, Arc::new(|_, _| panic!("unused"))),
            working_directory: None,
            config_bundle: None,
        };

        assert_eq!(
            ProfileRuntimeWorkerFactory::runtime_worker_name(&request),
            "runtime-arc-remote-worker-00000001"
        );
        assert_ne!(
            ProfileRuntimeWorkerFactory::runtime_worker_name(&request),
            "worker-00000001"
        );
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

    #[test]
    fn builtin_profile_selector_is_not_double_prefixed() {
        assert_eq!(
            ProfileRuntimeWorkerFactory::runtime_profile_value(
                &crate::catalog::ProfileSelector::Builtin("coder".to_string())
            )
            .as_deref(),
            Some("builtin:coder")
        );
        assert_eq!(
            ProfileRuntimeWorkerFactory::runtime_profile_value(
                &crate::catalog::ProfileSelector::Builtin("builtin:coder".to_string())
            )
            .as_deref(),
            Some("builtin:coder")
        );
    }

    #[test]
    fn adapter_dispatches_user_input_through_worker_run_lifecycle() {
        let client = MockClient::new(simple_text_events());
        let runtime_base = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let observed_cwds = Arc::new(Mutex::new(Vec::new()));
        let factory = MockFactory {
            client: client.clone(),
            runtime_base: runtime_base.path().to_path_buf(),
            cwd: cwd.path().to_path_buf(),
            store_dir: store.path().join("sessions"),
            worker_metadata_dir: store.path().join("workers"),
            observed_cwds: observed_cwds.clone(),
        };
        let backend = WorkerRuntimeExecutionBackend::new(factory).unwrap();
        let runtime = EmbeddedRuntime::with_execution_backend(
            RuntimeOptions {
                runtime_id: RuntimeId::new("embedded"),
                ..RuntimeOptions::default()
            },
            Arc::new(backend),
        )
        .unwrap();
        runtime.store_config_bundle(test_bundle()).unwrap();
        let detail = runtime.create_worker(create_request("chat")).unwrap();

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
        let factory = MockFactory {
            client: client.clone(),
            runtime_base: runtime_base.path().to_path_buf(),
            cwd: repo.path().to_path_buf(),
            store_dir: store.path().join("sessions"),
            worker_metadata_dir: store.path().join("workers"),
            observed_cwds: observed_cwds.clone(),
        };
        let backend = WorkerRuntimeExecutionBackend::new(factory)
            .unwrap()
            .with_working_directory_materializer(LocalGitWorktreeMaterializer::new(
                runtime_base.path(),
            ));
        let runtime = EmbeddedRuntime::with_execution_backend(
            RuntimeOptions {
                runtime_id: RuntimeId::new("embedded-materialized"),
                ..RuntimeOptions::default()
            },
            Arc::new(backend),
        )
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

        assert!(detail.execution.working_directory.is_some());
        let cwds = observed_cwds.lock().unwrap();
        assert_eq!(cwds.len(), 1);
        let cwd = &cwds[0];
        assert!(cwd.starts_with(runtime_base.path()));
        assert!(!cwd.starts_with(repo.path()));
        assert!(cwd.join("README.md").exists());
    }
}
