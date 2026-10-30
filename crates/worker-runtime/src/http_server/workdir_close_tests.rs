//! Runtime HTTP close integration: production local-token authentication, session registry,
//! Local command teardown, and Runtime active-use guard. The execution-backend mapping is
//! a fixture, not a materializer; a provider wrapper gates a genuinely pending Local close.
//! Run with `cargo test -p worker-runtime --lib http_server::workdir_close_tests`.

use super::*;
use crate::RuntimeOptions;
use crate::execution::{
    WorkerExecutionBackend, WorkerExecutionResult, WorkerExecutionSpawnRequest,
    WorkerExecutionSpawnResult,
};
use crate::working_directory::WorkingDirectoryDiagnostic;
use axum::body::to_bytes;
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::Poll;
use tokio::sync::{Notify, Semaphore};
use tower::ServiceExt;
use workdir::{LocalWorkdirSession, Workdir, WorkdirError, WorkdirSession};

#[derive(Debug)]
struct CloseProvider {
    local: LocalWorkdirSession,
    calls: AtomicUsize,
    cancelled: AtomicUsize,
    pause_first: bool,
    fail_first: bool,
    entered: Notify,
    release: Semaphore,
}

struct AttemptGuard<'a> {
    cancelled: &'a AtomicUsize,
    finished: bool,
}

impl Drop for AttemptGuard<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.cancelled.fetch_add(1, Ordering::SeqCst);
        }
    }
}

// Only close scheduling/error injection is substituted. All session operations and
// actual teardown remain Local; in particular its early closed.swap is exercised.
#[async_trait::async_trait]
impl WorkdirSession for CloseProvider {
    fn workdir(&self) -> &Workdir {
        self.local.workdir()
    }

    fn capabilities(&self) -> workdir::WorkdirSessionCapabilities {
        self.local.capabilities()
    }

    async fn stat(
        &self,
        request: workdir::StatRequest,
    ) -> Result<workdir::StatResult, WorkdirError> {
        self.local.stat(request).await
    }
    async fn read(
        &self,
        request: workdir::ReadRequest,
    ) -> Result<workdir::ReadResult, WorkdirError> {
        self.local.read(request).await
    }
    async fn write(
        &self,
        request: workdir::WriteRequest,
    ) -> Result<workdir::WriteResult, WorkdirError> {
        self.local.write(request).await
    }
    async fn edit(
        &self,
        request: workdir::EditRequest,
    ) -> Result<workdir::EditResult, WorkdirError> {
        self.local.edit(request).await
    }
    async fn list(
        &self,
        request: workdir::ListRequest,
    ) -> Result<workdir::ListResult, WorkdirError> {
        self.local.list(request).await
    }
    async fn glob(
        &self,
        request: workdir::GlobRequest,
    ) -> Result<workdir::GlobResult, WorkdirError> {
        self.local.glob(request).await
    }
    async fn grep(
        &self,
        request: workdir::GrepRequest,
    ) -> Result<workdir::GrepResult, WorkdirError> {
        self.local.grep(request).await
    }
    async fn start_command(
        &self,
        request: workdir::CommandRequest,
    ) -> Result<workdir::CommandHandle, WorkdirError> {
        self.local.start_command(request).await
    }
    async fn command_status(
        &self,
        request: workdir::CommandHandle,
    ) -> Result<workdir::CommandStatus, WorkdirError> {
        self.local.command_status(request).await
    }
    async fn command_output(
        &self,
        request: workdir::CommandOutputRequest,
    ) -> Result<workdir::CommandOutput, WorkdirError> {
        self.local.command_output(request).await
    }
    async fn cancel_command(&self, request: workdir::CommandHandle) -> Result<(), WorkdirError> {
        self.local.cancel_command(request).await
    }

    async fn close(&self) -> Result<(), WorkdirError> {
        let attempt = self.calls.fetch_add(1, Ordering::SeqCst);
        if attempt == 0 && self.fail_first {
            return Err(WorkdirError::Unavailable("injected close failure".into()));
        }
        let mut guard = AttemptGuard {
            cancelled: &self.cancelled,
            finished: false,
        };
        if attempt == 0 && self.pause_first {
            self.local
                .start_command(workdir::CommandRequest {
                    command: "printf close-probe".into(),
                    timeout_secs: 5,
                    transport_byte_budget: workdir::MAX_COMMAND_TRANSPORT_BYTES,
                    cwd: workdir::WorkdirPath::root(),
                    tool_call_id: None,
                })
                .await?;
        }
        let mut closing = Box::pin(self.local.close());
        if attempt == 0 && self.pause_first {
            // On the current-thread executor the command task cannot run before
            // this poll. Local close sets closed, drains/cancels the command,
            // then is genuinely Pending on command.task.await (reviewer's probe).
            assert!(
                std::future::poll_fn(|cx| Poll::Ready(closing.as_mut().poll(cx)))
                    .await
                    .is_pending()
            );
            self.entered.notify_one();
            // Hold the already-polled provider future, not a fake release flag.
            // Cancelling this future would strand Local's unconfirmed teardown.
            let _permit = self.release.acquire().await.unwrap();
        }
        let result = closing.await;
        guard.finished = true;
        result
    }
}

struct Backend {
    session: Arc<CloseProvider>,
}

impl WorkerExecutionBackend for Backend {
    fn backend_id(&self) -> &str {
        "close-confirmation-fixture"
    }

    fn spawn_worker(&self, _: WorkerExecutionSpawnRequest) -> WorkerExecutionSpawnResult {
        panic!("not used")
    }

    fn dispatch_input(&self, _: &WorkerRef, _: WorkerInput) -> WorkerExecutionResult {
        panic!("not used")
    }

    fn working_directory(
        &self,
        _: &str,
    ) -> Result<WorkingDirectoryStatus, WorkingDirectoryDiagnostic> {
        Ok(serde_json::from_value(serde_json::json!({
            "summary": {
                "working_directory_id": "w", "repository_id": "r",
                "materializer_kind": "runtime_git_clone", "status": "active"
            }
        }))
        .unwrap())
    }

    fn open_workdir_session(
        &self,
        _: &str,
    ) -> Result<WorkdirSessionHandle, WorkingDirectoryDiagnostic> {
        Ok(self.session.clone())
    }

    fn cleanup_working_directory(
        &self,
        id: &str,
    ) -> Result<WorkingDirectoryStatus, WorkingDirectoryDiagnostic> {
        self.working_directory(id)
    }
}

struct Fixture {
    _temp: tempfile::TempDir,
    provider: Arc<CloseProvider>,
    runtime: Runtime,
    router: Router,
    path: String,
}

impl Fixture {
    async fn new(pause_first: bool, fail_first: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("hello.txt"), "hello").unwrap();
        let provider = Arc::new(CloseProvider {
            local: LocalWorkdirSession::materialized_bound(
                Workdir::new("w"),
                temp.path().to_owned(),
                temp.path().to_owned(),
                manifest::SharedScope::new(manifest::Scope::writable(temp.path()).unwrap()),
                workdir::WorkdirSessionCapabilities::ALL,
            ),
            calls: AtomicUsize::new(0),
            cancelled: AtomicUsize::new(0),
            pause_first,
            fail_first,
            entered: Notify::new(),
            release: Semaphore::new(0),
        });
        let runtime = Runtime::with_execution_backend(
            RuntimeOptions::default(),
            Arc::new(Backend {
                session: provider.clone(),
            }),
        )
        .unwrap();
        let router = runtime_http_router(runtime.clone(), "probe-token".into());
        let opened = router
            .clone()
            .oneshot(request("POST", "/v1/working-directories/w/sessions", "{}"))
            .await
            .unwrap();
        assert_eq!(opened.status(), StatusCode::OK);
        let opened: OpenWorkdirSessionResponse =
            serde_json::from_slice(&to_bytes(opened.into_body(), 65536).await.unwrap()).unwrap();
        let path = format!("/v1/workdir-sessions/{}", opened.session_id.as_str());
        Self {
            _temp: temp,
            provider,
            runtime,
            router,
            path,
        }
    }

    fn close(&self) -> impl Future<Output = Response> + use<> {
        let router = self.router.clone();
        let request = request("DELETE", &self.path, "");
        async move { router.oneshot(request).await.unwrap() }
    }

    fn assert_busy(&self) {
        let error = self
            .runtime
            .cleanup_working_directory("w")
            .expect_err("unconfirmed close must retain the real Runtime use guard");
        assert!(
            matches!(error, RuntimeError::WorkingDirectory(ref d)
            if d.code == "working_directory_cleanup_resource_busy"),
            "{error:?}"
        );
    }

    async fn assert_operations_unavailable(&self) {
        let response = self
            .router
            .clone()
            .oneshot(request(
                "POST",
                &format!("{}/operations", self.path),
                r#"{"operation":{"operation":"stat","request":{"path":"hello.txt"}}}"#,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Let a detached close finish even if a test assertion unwinds.
        self.provider.release.add_permits(1);
    }
}

fn request(method: &str, path: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", "Bearer probe-token")
        .header("content-type", "application/json")
        .body(Body::from(body.to_owned()))
        .unwrap()
}

async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(std::time::Duration::from_secs(10), future)
        .await
        .expect("close probe must not hang")
}

async fn assert_pending<T>(mut future: std::pin::Pin<&mut impl Future<Output = T>>) {
    assert!(
        std::future::poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx)))
            .await
            .is_pending(),
        "close acknowledged before teardown confirmation"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn duplicate_delete_waits_for_actual_local_teardown_and_runtime_guard_release() {
    let fixture = Fixture::new(true, false).await;
    fixture.assert_busy();
    let mut first = Box::pin(fixture.close());
    assert_pending(first.as_mut()).await;
    bounded(fixture.provider.entered.notified()).await;
    let mut duplicate = Box::pin(fixture.close());
    assert_pending(duplicate.as_mut()).await;
    fixture.assert_busy();
    fixture.assert_operations_unavailable().await;
    assert_eq!(fixture.provider.calls.load(Ordering::SeqCst), 1);

    fixture.provider.release.add_permits(1);
    assert_eq!(bounded(first).await.status(), StatusCode::NO_CONTENT);
    assert_eq!(bounded(duplicate).await.status(), StatusCode::NO_CONTENT);
    fixture
        .runtime
        .cleanup_working_directory("w")
        .expect("confirmed close releases use guard");
    assert_eq!(
        bounded(fixture.close()).await.status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(fixture.provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_http_waiter_does_not_cancel_provider_or_let_retries_ack_early() {
    let fixture = Fixture::new(true, false).await;
    let mut first = Box::pin(fixture.close());
    assert_pending(first.as_mut()).await;
    bounded(fixture.provider.entered.notified()).await;
    drop(first);
    let mut retry = Box::pin(fixture.close());
    let mut duplicate = Box::pin(fixture.close());
    assert_pending(retry.as_mut()).await;
    assert_pending(duplicate.as_mut()).await;
    fixture.assert_busy();
    assert_eq!(fixture.provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.provider.cancelled.load(Ordering::SeqCst), 0);

    fixture.provider.release.add_permits(1);
    assert_eq!(bounded(retry).await.status(), StatusCode::NO_CONTENT);
    assert_eq!(bounded(duplicate).await.status(), StatusCode::NO_CONTENT);
    assert_eq!(fixture.provider.cancelled.load(Ordering::SeqCst), 0);
    fixture
        .runtime
        .cleanup_working_directory("w")
        .expect("task completes real provider close");
    assert_eq!(
        bounded(fixture.close()).await.status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(fixture.provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn provider_close_error_retains_session_and_runtime_guard_for_retry() {
    let fixture = Fixture::new(false, true).await;
    let mut first = Box::pin(fixture.close());
    let mut duplicate = Box::pin(fixture.close());
    assert_pending(first.as_mut()).await;
    assert_pending(duplicate.as_mut()).await;
    let response = bounded(first).await;
    assert_eq!(
        bounded(duplicate).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let error: WorkdirTransportError =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(error.code, WorkdirTransportErrorCode::Unavailable);
    fixture.assert_busy();
    assert_eq!(fixture.provider.calls.load(Ordering::SeqCst), 1);

    assert_eq!(
        bounded(fixture.close()).await.status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(fixture.provider.calls.load(Ordering::SeqCst), 2);
    fixture
        .runtime
        .cleanup_working_directory("w")
        .expect("retry releases the retained Runtime guard");
    assert_eq!(
        bounded(fixture.close()).await.status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(fixture.provider.calls.load(Ordering::SeqCst), 2);
}
