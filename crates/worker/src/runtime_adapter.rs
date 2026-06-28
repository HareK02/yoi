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

use async_trait::async_trait;
use manifest::paths;
use protocol::{Method, Segment, WorkerStatus};
use session_store::FsStore;
use session_store::{CombinedStore, FsWorkerStore};
use tokio::runtime::Runtime;
use tokio::sync::broadcast;
use worker_runtime::execution::{
    WorkerExecutionBackend, WorkerExecutionHandle, WorkerExecutionOperation, WorkerExecutionResult,
    WorkerExecutionRunState, WorkerExecutionSpawnRequest, WorkerExecutionSpawnResult,
};
use worker_runtime::interaction::{WorkerInput, WorkerInputKind};

use crate::{Worker, WorkerController, WorkerHandle};

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
#[derive(Debug, Clone)]
pub struct ProfileRuntimeWorkerFactory {
    workspace_root: PathBuf,
    cwd: PathBuf,
    store_dir: Option<PathBuf>,
    worker_metadata_dir: Option<PathBuf>,
    profile: Option<String>,
    runtime_base_dir: Option<PathBuf>,
}

impl ProfileRuntimeWorkerFactory {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        let workspace_root = workspace_root.into();
        Self {
            cwd: workspace_root.clone(),
            workspace_root,
            store_dir: None,
            worker_metadata_dir: None,
            profile: None,
            runtime_base_dir: None,
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
            .or_else(|| crate::runtime::dir::default_base().ok())
            .ok_or_else(|| "could not resolve worker runtime directory".to_string())
    }

    fn runtime_worker_name(request: &WorkerExecutionSpawnRequest) -> String {
        request.worker_ref.worker_id.to_string()
    }

    fn runtime_profile_value(
        profile: &worker_runtime::catalog::ProfileSelector,
    ) -> Option<std::borrow::Cow<'_, str>> {
        match profile {
            worker_runtime::catalog::ProfileSelector::RuntimeDefault => None,
            worker_runtime::catalog::ProfileSelector::Named(name) => {
                Some(std::borrow::Cow::Borrowed(name.as_str()))
            }
            worker_runtime::catalog::ProfileSelector::Builtin(name) => {
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
}

#[async_trait]
impl RuntimeWorkerFactory for ProfileRuntimeWorkerFactory {
    async fn spawn_controller(
        &self,
        request: WorkerExecutionSpawnRequest,
    ) -> Result<WorkerHandle, String> {
        let worker_name = Self::runtime_worker_name(&request);
        let profile = self.runtime_profile(&request);
        let (mut manifest, loader) = crate::entrypoint::resolve_runtime_profile_manifest(
            profile.as_deref(),
            &self.workspace_root,
            &worker_name,
        )?;
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
            self.workspace_root.clone(),
            self.cwd.clone(),
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
}

/// `worker-runtime` execution backend backed by real `worker` crate Workers.
pub struct WorkerRuntimeExecutionBackend<F = ProfileRuntimeWorkerFactory> {
    backend_id: String,
    factory: Arc<F>,
    runtime: Mutex<Option<Runtime>>,
    workers: Mutex<HashMap<worker_runtime::identity::WorkerRef, RuntimeWorkerExecution>>,
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
            runtime: Mutex::new(Some(runtime)),
            workers: Mutex::new(HashMap::new()),
        })
    }

    pub fn with_backend_id(mut self, backend_id: impl Into<String>) -> Self {
        self.backend_id = backend_id.into();
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

        let factory = self.factory.clone();
        let bridge_context = request.context.clone();
        let worker_ref = request.worker_ref.clone();
        let spawn_result =
            self.run_on_adapter_runtime(async move { factory.spawn_controller(request).await });

        let handle = match spawn_result {
            Ok(handle) => handle,
            Err(message) => {
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
        workers.insert(worker_ref.clone(), RuntimeWorkerExecution { handle, busy });

        WorkerExecutionSpawnResult::Connected {
            handle: WorkerExecutionHandle::new(worker_ref, self.backend_id()),
            run_state: WorkerExecutionRunState::Idle,
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
        if result.outcome != worker_runtime::execution::WorkerExecutionOutcome::Accepted {
            busy.store(false, Ordering::SeqCst);
        }
        result
    }

    fn stop_worker(&self, handle: &WorkerExecutionHandle) -> WorkerExecutionResult {
        let (worker, _busy) = match self.get_execution(handle) {
            Ok(execution) => execution,
            Err(mut result) => {
                result.operation = WorkerExecutionOperation::Stop;
                return result;
            }
        };
        self.send_method(
            WorkerExecutionOperation::Stop,
            worker,
            Method::Shutdown,
            WorkerExecutionRunState::Stopped,
        )
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
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use futures::Stream;
    use llm_engine::Engine;
    use llm_engine::llm_client::event::{Event as LlmEvent, ResponseStatus, StatusEvent};
    use llm_engine::llm_client::{ClientError, LlmClient, Request};
    use manifest::{Scope, WorkerManifest};
    use worker_runtime::Runtime as EmbeddedRuntime;
    use worker_runtime::catalog::{CreateWorkerRequest, ProfileSelector, WorkerIntent};
    use worker_runtime::identity::RuntimeId;
    use worker_runtime::management::RuntimeOptions;
    use worker_runtime::observation::{TranscriptQuery, TranscriptRole};

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
    }

    #[async_trait]
    impl RuntimeWorkerFactory for MockFactory {
        async fn spawn_controller(
            &self,
            _request: WorkerExecutionSpawnRequest,
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
            let scope = Scope::writable(&self.cwd).map_err(|err| err.to_string())?;
            let worker = Worker::new(
                manifest,
                Engine::new(self.client.clone()),
                store,
                self.cwd.clone(),
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

    fn create_request(name: &str) -> CreateWorkerRequest {
        CreateWorkerRequest {
            intent: WorkerIntent::Role {
                role: name.to_string(),
                purpose: None,
            },
            profile: ProfileSelector::RuntimeDefault,
            config_bundle: None,
            requested_capabilities: Vec::new(),
            workspace_refs: Vec::new(),
            mount_refs: Vec::new(),
        }
    }

    #[test]
    fn builtin_profile_selector_is_not_double_prefixed() {
        assert_eq!(
            ProfileRuntimeWorkerFactory::runtime_profile_value(
                &worker_runtime::catalog::ProfileSelector::Builtin("coder".to_string())
            )
            .as_deref(),
            Some("builtin:coder")
        );
        assert_eq!(
            ProfileRuntimeWorkerFactory::runtime_profile_value(
                &worker_runtime::catalog::ProfileSelector::Builtin("builtin:coder".to_string())
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
        let factory = MockFactory {
            client: client.clone(),
            runtime_base: runtime_base.path().to_path_buf(),
            cwd: cwd.path().to_path_buf(),
            store_dir: store.path().join("sessions"),
            worker_metadata_dir: store.path().join("workers"),
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
        let detail = runtime.create_worker(create_request("chat")).unwrap();

        runtime
            .send_input(&detail.worker_ref, WorkerInput::user("say hello"))
            .unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let projection = runtime
                .transcript_projection(&detail.worker_ref, TranscriptQuery::new(0, 10))
                .unwrap();
            if projection.items.iter().any(|item| {
                item.role == TranscriptRole::Assistant && item.content == "hello from worker"
            }) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for assistant transcript projection"
            );
            std::thread::sleep(Duration::from_millis(20));
        }

        assert_eq!(client.captured.lock().unwrap().len(), 1);
        let observations = runtime
            .read_worker_observation_events(
                &detail.worker_ref,
                worker_runtime::observation::WorkerObservationCursor::zero(),
            )
            .unwrap();
        assert!(
            observations
                .iter()
                .any(|event| matches!(event.payload, protocol::Event::TextDone { .. }))
        );
    }
}
