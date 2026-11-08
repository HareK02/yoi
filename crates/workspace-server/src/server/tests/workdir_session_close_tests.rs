//! Backend registry/current-attachment/WorkerRemove integration with authenticated
//! Runtime HTTP sessions and real Git materializer resource leases. Only Worker
//! execution and the failing/held transport are replaced. Attachment/operation
//! cases invoke production Backend handlers with middleware-resolved Worker headers;
//! Backend-to-Runtime requests still traverse real workspace authentication.
//! Ordinary library tests use LaunchFixture's isolated SQLite, materializations
//! and loopback Runtime.
use super::ticket_worker_launch_tests::LaunchFixture;
use super::*;
use std::{future::Future, task::Poll};
use tokio::sync::{Notify, Semaphore};
use tower::ServiceExt;

#[derive(Debug)]
pub(super) struct ProviderCloseGate {
    entered: Notify,
    release: Semaphore,
}

impl ProviderCloseGate {
    pub(super) fn release(&self) {
        self.release.add_permits(1);
    }

    pub(super) fn wrap(self: &Arc<Self>, source: WorkdirSessionHandle) -> WorkdirSessionHandle {
        Arc::new(GatedProviderClose {
            source,
            gate: self.clone(),
        })
    }
}

#[derive(Debug)]
struct GatedProviderClose {
    source: WorkdirSessionHandle,
    gate: Arc<ProviderCloseGate>,
}

// Forward the real provider operations; only suspend an already-pending close.
macro_rules! gated_provider {
    ($($name:ident($request:ident: $input:ty) -> $output:ty;)*) => {
        #[async_trait::async_trait]
        impl WorkdirSession for GatedProviderClose {
            fn workdir(&self) -> &workdir::Workdir { self.source.workdir() }
            fn capabilities(&self) -> workdir::WorkdirSessionCapabilities { self.source.capabilities() }
            fn environment(&self) -> workdir::WorkdirSessionEnvironment { self.source.environment() }
            $(async fn $name(&self, $request: $input) -> std::result::Result<$output, workdir::WorkdirError> {
                self.source.$name($request).await
            })*
            async fn close(&self) -> std::result::Result<(), workdir::WorkdirError> {
                let mut closing = Box::pin(self.source.close());
                match std::future::poll_fn(|cx| Poll::Ready(closing.as_mut().poll(cx))).await {
                    Poll::Ready(result) => result,
                    Poll::Pending => {
                        self.gate.entered.notify_one();
                        self.gate.release.acquire().await.unwrap().forget();
                        closing.await
                    }
                }
            }
        }
    };
}

gated_provider! {
        stat(request: workdir::StatRequest) -> workdir::StatResult;
        read(request: workdir::ReadRequest) -> workdir::ReadResult;
        write(request: workdir::WriteRequest) -> workdir::WriteResult;
        edit(request: workdir::EditRequest) -> workdir::EditResult;
        list(request: workdir::ListRequest) -> workdir::ListResult;
        glob(request: workdir::GlobRequest) -> workdir::GlobResult;
        grep(request: workdir::GrepRequest) -> workdir::GrepResult;
        start_command(request: workdir::CommandRequest) -> workdir::CommandHandle;
        command_status(handle: workdir::CommandHandle) -> workdir::CommandStatus;
        command_output(request: workdir::CommandOutputRequest) -> workdir::CommandOutput;
        cancel_command(handle: workdir::CommandHandle) -> ();
}

#[derive(Clone, Copy)]
enum CloseFault {
    Reject,
    HoldThenReject,
    RejectAfterRuntimeClose,
}

#[derive(Clone, Copy)]
enum OpenFault {
    RejectAfterRuntimeOpen,
    HoldAfterRuntimeOpen,
}

struct CloseTransport {
    upstream: String,
    client: reqwest::Client,
    fault: Mutex<Option<CloseFault>>,
    open_fault: Mutex<Option<OpenFault>>,
    runtime_opens: Mutex<Vec<workdir::http::OpenWorkdirSessionResponse>>,
    opened: Notify,
    open_release: Semaphore,
    attempts: Mutex<Vec<String>>,
    runtime_closes: Mutex<Vec<(String, StatusCode)>>,
    removals: std::sync::atomic::AtomicUsize,
    direct_runtime: Mutex<Option<Router>>,
    pending_dispatches: Mutex<Vec<bool>>,
    dispatched: Notify,
    entered: Notify,
    release: Semaphore,
}

impl CloseTransport {
    async fn forward(State(state): State<Arc<Self>>, request: Request<Body>) -> Response {
        let is_close = request.method() == Method::DELETE
            && request.uri().path().starts_with("/v1/workdir-sessions/");
        let path = request.uri().path().to_owned();
        if request.method() == Method::DELETE && path.starts_with("/v1/working-directories/") {
            state
                .removals
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        let is_open = request.method() == Method::POST
            && path.starts_with("/v1/working-directories/")
            && path.ends_with("/sessions");
        let open_fault = if is_open {
            state.open_fault.lock().unwrap().take()
        } else {
            None
        };
        let fault = if is_close {
            state.attempts.lock().unwrap().push(path.clone());
            state.fault.lock().unwrap().take()
        } else {
            None
        };
        if matches!(fault, Some(CloseFault::Reject | CloseFault::HoldThenReject)) {
            if matches!(fault, Some(CloseFault::HoldThenReject)) {
                state.entered.notify_one();
                state.release.acquire().await.unwrap().forget();
            }
            // The actual Runtime has not seen this request or released its lease.
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        let direct = state.direct_runtime.lock().unwrap().clone();
        if let Some(router) = direct.filter(|_| is_close) {
            // Poll the production authenticated handler without another socket
            // scheduling boundary, so a duplicate's Pending/204 is observable.
            let mut response = Box::pin(router.oneshot(request));
            let first_poll =
                std::future::poll_fn(|cx| Poll::Ready(response.as_mut().poll(cx))).await;
            state
                .pending_dispatches
                .lock()
                .unwrap()
                .push(first_poll.is_pending());
            state.dispatched.notify_one();
            let response = match first_poll {
                Poll::Ready(result) => result.unwrap(),
                Poll::Pending => response.await.unwrap(),
            };
            state
                .runtime_closes
                .lock()
                .unwrap()
                .push((path, response.status()));
            return response;
        }
        let (parts, body) = request.into_parts();
        let body = to_bytes(body, 1024 * 1024).await.unwrap();
        let response = state
            .client
            .request(parts.method, format!("{}{}", state.upstream, parts.uri))
            .headers(parts.headers)
            .body(body)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = response.bytes().await.unwrap();
        if is_open && status.is_success() {
            state
                .runtime_opens
                .lock()
                .unwrap()
                .push(serde_json::from_slice(&bytes).expect("real Runtime OPEN response"));
            state.opened.notify_one();
            match open_fault {
                Some(OpenFault::RejectAfterRuntimeOpen) => {
                    return StatusCode::BAD_GATEWAY.into_response();
                }
                Some(OpenFault::HoldAfterRuntimeOpen) => {
                    state.open_release.acquire().await.unwrap().forget();
                }
                None => {}
            }
        }
        if is_close {
            state.runtime_closes.lock().unwrap().push((path, status));
        }
        if matches!(fault, Some(CloseFault::RejectAfterRuntimeClose)) {
            assert_eq!(status, StatusCode::NO_CONTENT);
            // Lose only the successful acknowledgement, after Runtime has closed
            // the actual session and released its active-use/materializer leases.
            return StatusCode::BAD_GATEWAY.into_response();
        }
        let mut response = Response::new(Body::from(bytes));
        *response.status_mut() = status;
        *response.headers_mut() = headers;
        response
    }

    fn fail_next(&self, fault: CloseFault) {
        assert!(self.fault.lock().unwrap().replace(fault).is_none());
    }

    fn attempts(&self) -> Vec<String> {
        self.attempts.lock().unwrap().clone()
    }
}

struct Fixture {
    source: LaunchFixture,
    transport: Arc<CloseTransport>,
    proxy: tokio::task::JoinHandle<()>,
    runtime_id: String,
    workdir_id: String,
    materialization_root: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.proxy.abort();
    }
}

impl Fixture {
    async fn new() -> Self {
        let (source, binding) = LaunchFixture::with_source_workdir().await;
        let workdir_id = binding.working_directory.id.clone();
        let materialization_root = binding.working_directory_root().to_owned();
        // Do not accidentally retain a second materializer lease in the fixture.
        drop(binding);
        let record = source
            .api
            .store
            .get_workdir_registry(TEST_WORKSPACE_ID, &workdir_id)
            .unwrap()
            .unwrap();
        let runtime_id = record.source.runtime_id().unwrap().to_owned();
        let binding = source
            .api
            .store
            .get_workspace_runtime_binding(TEST_WORKSPACE_ID, &runtime_id)
            .await
            .unwrap()
            .unwrap();
        let transport = Arc::new(CloseTransport {
            upstream: binding.base_url.clone(),
            client: reqwest::Client::builder()
                .no_proxy()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap(),
            fault: Mutex::new(None),
            open_fault: Mutex::new(None),
            runtime_opens: Mutex::new(Vec::new()),
            opened: Notify::new(),
            open_release: Semaphore::new(0),
            attempts: Mutex::new(Vec::new()),
            runtime_closes: Mutex::new(Vec::new()),
            removals: Default::default(),
            direct_runtime: Mutex::new(None),
            pending_dispatches: Mutex::new(Vec::new()),
            dispatched: Notify::new(),
            entered: Notify::new(),
            release: Semaphore::new(0),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let router = Router::new()
            .fallback(axum::routing::any(CloseTransport::forward))
            .with_state(transport.clone());
        let proxy = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let backend_url = source.api.config.backend_base_url.clone().unwrap();
        let remote = RemoteWorkerRuntime::new(
            RemoteRuntimeConfig {
                runtime_id: runtime_id.clone(),
                workspace_id: Some(TEST_WORKSPACE_ID.into()),
                display_name: binding.display_name.clone(),
                base_url: endpoint,
                bearer_token: None,
                workspace_authorization: Some(crate::hosts::WorkspaceRuntimeAuthorization::new(
                    source.api.store.clone(),
                    source.api.signing_identities.clone(),
                    backend_url.clone(),
                    binding,
                )),
                strict_public_egress: false,
                cached_worker_creation_available: true,
                cached_os: "linux".into(),
                cached_arch: "x86_64".into(),
                cached_status: "active".into(),
                timeout: std::time::Duration::from_secs(10),
            },
            TEST_WORKSPACE_ID.into(),
            backend_url,
        )
        .unwrap();
        source.api.runtime.register_or_replace(remote);
        Self {
            source,
            transport,
            proxy,
            runtime_id,
            workdir_id,
            materialization_root,
        }
    }

    fn api(&self) -> &WorkspaceApi {
        &self.source.api
    }

    fn registry_owner(&self) -> RuntimeWorkerRef {
        RuntimeWorkerRef::new(&self.runtime_id, "registry-close-owner")
    }

    async fn create_worker(&self) -> RuntimeWorkerRef {
        self.source.publish_profile("builtin:companion");
        let Json(created) = create_workspace_worker(
            State(self.api().clone()),
            HeaderMap::new(),
            Json(CreateWorkspaceWorkerRequest {
                runtime_id: self.runtime_id.clone(),
                display_name: "session-close Worker".into(),
                singleton_key: None,
                profile: Some("builtin:companion".into()),
                ticket_assignment: None,
                initial_submit: Vec::new(),
                workdir_attachments: Vec::new(),
                feature_connections: Default::default(),
                control_operation_id: Some(uuid::Uuid::now_v7().to_string()),
                authorization: server_api::launch_policy::WorkerCreationAuthorization::Human {
                    assignment: None,
                },
            }),
        )
        .await
        .unwrap();
        RuntimeWorkerRef::new(created.runtime_id, created.worker_id)
    }

    fn worker_headers(worker: &RuntimeWorkerRef) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("x-yoi-runtime-id", worker.runtime_id.parse().unwrap());
        headers.insert("x-yoi-worker-id", worker.worker_id.parse().unwrap());
        headers
    }

    async fn attach(&self, worker: &RuntimeWorkerRef, alias: &str) -> WorkerWorkdirLinkRecord {
        self.attach_workdir(worker, alias, &self.workdir_id).await
    }

    async fn attach_workdir(
        &self,
        worker: &RuntimeWorkerRef,
        alias: &str,
        workdir_id: &str,
    ) -> WorkerWorkdirLinkRecord {
        let Json(response) = scoped_attach_current_worker_workdir(
            State(self.api().clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: TEST_WORKSPACE_ID.into(),
            }),
            Self::worker_headers(worker),
            Json(AttachCurrentWorkerWorkdirRequest {
                alias: alias.into(),
                working_directory_id: workdir_id.to_owned(),
            }),
        )
        .await
        .unwrap();
        assert!(response.attached);
        assert_eq!(response.alias, alias);
        self.api()
            .store
            .list_worker_workdir_links(TEST_WORKSPACE_ID, worker)
            .unwrap()
            .into_iter()
            .find(|link| link.alias == alias && link.unlinked_at.is_none())
            .expect("production attach persisted its live connection")
    }

    async fn detach(&self, link: &WorkerWorkdirLinkRecord) {
        let Json(response) = scoped_detach_current_worker_workdir(
            State(self.api().clone()),
            AxumPath((TEST_WORKSPACE_ID.into(), link.alias.clone())),
            Self::worker_headers(&link.worker),
            server_api::CurrentWorkerWorkdirDetachQuery {
                expected_connection_id: Some(link.connection_id.clone()),
            },
        )
        .await
        .unwrap();
        assert!(!response.attached);
        assert!(
            self.api()
                .store
                .list_worker_workdir_links(TEST_WORKSPACE_ID, &link.worker)
                .unwrap()
                .iter()
                .all(|saved| saved.connection_id != link.connection_id
                    || saved.unlinked_at.is_some())
        );
    }

    async fn operation(
        &self,
        link: &WorkerWorkdirLinkRecord,
        operation: WorkdirSessionOperation,
    ) -> WorkdirSessionOperationResult {
        let Json(result) = scoped_execute_current_worker_workdir_operation(
            State(self.api().clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: TEST_WORKSPACE_ID.into(),
            }),
            Self::worker_headers(&link.worker),
            Json(WorkspaceWorkdirSessionOperationRequest {
                target_workdir: link.alias.clone(),
                operation,
            }),
        )
        .await
        .unwrap();
        result
    }

    async fn stat_attachment(&self, link: &WorkerWorkdirLinkRecord) {
        let result = self
            .operation(
                link,
                WorkdirSessionOperation::Stat(workdir::StatRequest {
                    path: workdir::WorkdirPath::new("README.md").unwrap(),
                }),
            )
            .await;
        assert!(matches!(result, WorkdirSessionOperationResult::Stat(_)));
    }

    async fn complete_command(&self, link: &WorkerWorkdirLinkRecord) -> CommandHandle {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            let result = self
                .operation(
                    link,
                    WorkdirSessionOperation::CommandStart(workdir::CommandRequest {
                        command: "printf lifecycle".into(),
                        timeout_secs: 5,
                        transport_byte_budget: workdir::MAX_COMMAND_TRANSPORT_BYTES,
                        cwd: workdir::WorkdirPath::root(),
                        tool_call_id: None,
                    }),
                )
                .await;
            let WorkdirSessionOperationResult::CommandStart(handle) = result else {
                panic!("command start returned {result:?}");
            };
            let mut cursor = 0;
            let mut stdout = Vec::new();
            loop {
                let result = self
                    .operation(
                        link,
                        WorkdirSessionOperation::CommandOutput(workdir::CommandOutputRequest {
                            handle: handle.clone(),
                            cursor,
                            limit: workdir::MAX_COMMAND_READ_BYTES,
                            wait: true,
                        }),
                    )
                    .await;
                let WorkdirSessionOperationResult::CommandOutput(page) = result else {
                    panic!("command output returned {result:?}");
                };
                for chunk in &page.chunks {
                    assert_eq!(chunk.stream, workdir::CommandStream::Stdout);
                    stdout.extend_from_slice(&chunk.bytes);
                }
                cursor = page.next_cursor;
                if let Some(terminal) = page.terminal {
                    assert_eq!(terminal.status, workdir::CommandStatus::Completed);
                    assert_eq!(terminal.exit_code, Some(0));
                    assert_eq!(stdout, b"lifecycle");
                    break;
                }
            }
            handle
        })
        .await
        .expect("bounded command delivery through production Backend handler")
    }

    async fn remove_worker(&self, worker: &RuntimeWorkerRef) {
        self.api()
            .runtime
            .stop_worker(
                worker,
                WorkerLifecycleRequest {
                    stop_intent: None,
                    reason: Some("prepare removal".into()),
                    ticket_assignment: None,
                },
            )
            .unwrap();
        let summary = self.api().runtime.worker(worker).unwrap();
        assert_eq!(summary.state, "stopped");
        sync_worker_observation(self.api(), &summary).unwrap();
        self.source.seed_remote_session_for_cleanup(worker);
        let result = WorkerRemovalService::new(self.api())
            .execute_target_removal(&self.api().runtime, worker, "remove stopped Worker", None)
            .await
            .unwrap();
        assert_eq!(result.status, StatusCode::OK.as_u16(), "{}", result.body);
        assert!(
            self.api()
                .store
                .get_worker_registry(TEST_WORKSPACE_ID, worker)
                .unwrap()
                .is_none()
        );
        assert!(
            self.api()
                .store
                .list_worker_workdir_links(TEST_WORKSPACE_ID, worker)
                .unwrap()
                .iter()
                .all(|link| link.unlinked_at.is_some())
        );
    }

    async fn open(&self) -> WorkdirSessionHandle {
        self.api()
            .runtime
            .open_workdir_session(&self.runtime_id, &self.workdir_id, None)
            .await
            .unwrap()
    }

    fn assert_restored(&self, worker: &RuntimeWorkerRef, expected: &WorkdirSessionHandle) {
        let registry = self.api().workdir_sessions.lock().unwrap();
        assert!(Arc::ptr_eq(
            registry
                .attachments
                .get(worker)
                .expect("unconfirmed session retained"),
            expected
        ));
    }

    async fn assert_live(&self, session: &WorkdirSessionHandle) {
        session
            .stat(workdir::StatRequest {
                path: workdir::WorkdirPath::new("README.md").unwrap(),
            })
            .await
            .expect("same Runtime session still serves filesystem operations");
    }

    async fn remove_workdir(&self) -> WorkingDirectoryRemovalResponse {
        let api = self.api().clone();
        let workdir_id = self.workdir_id.clone();
        tokio::task::spawn_blocking(move || {
            execute_workdir_removal(
                &api,
                &workdir_id,
                "account:close-test",
                "remove normally after session close",
                &uuid::Uuid::now_v7().to_string(),
            )
        })
        .await
        .unwrap()
        .unwrap()
    }

    async fn assert_busy(&self) {
        let result = self.remove_workdir().await;
        assert_eq!(
            result.disposition,
            WorkingDirectoryRemovalDisposition::AttentionRequired,
            "{result:?}"
        );
        assert_eq!(
            result.failure_category.as_deref(),
            Some("resource_busy"),
            "{result:?}"
        );
        assert!(result.retryable);
        assert!(self.materialization_root.exists());
    }

    async fn assert_removed(&self) {
        let result = self.remove_workdir().await;
        assert_eq!(
            result.disposition,
            WorkingDirectoryRemovalDisposition::Removed,
            "{result:?}"
        );
        assert!(!result.retryable);
        assert!(!self.materialization_root.exists());
        assert!(
            self.api()
                .store
                .get_workdir_registry(TEST_WORKSPACE_ID, &self.workdir_id)
                .unwrap()
                .is_none()
        );
    }
}

#[derive(Clone, Copy)]
enum ClosePath {
    Registry,
    Attachment,
}

async fn close(api: &WorkspaceApi, worker: &RuntimeWorkerRef, path: ClosePath) -> bool {
    let lock = current_worker_session_lock(api, worker);
    let _guard = lock.lock().await;
    match path {
        ClosePath::Registry => close_worker_workdir_sessions(&api.workdir_sessions, worker)
            .await
            .is_ok(),
        ClosePath::Attachment => close_current_worker_attachment_session_locked(api, worker)
            .await
            .is_ok(),
    }
}

async fn failure_restoration(path: ClosePath) {
    let fixture = Fixture::new().await;
    let worker = fixture.registry_owner();
    let session = fixture.open().await;
    fixture
        .api()
        .workdir_sessions
        .lock()
        .unwrap()
        .insert_attachment(worker.clone(), session.clone());
    fixture.assert_busy().await;
    fixture.transport.fail_next(CloseFault::Reject);
    assert!(!close(fixture.api(), &worker, path).await);
    fixture.assert_restored(&worker, &session);
    fixture.assert_live(&session).await;
    fixture.assert_busy().await;
    assert!(close(fixture.api(), &worker, path).await);
    assert!(
        !fixture
            .api()
            .workdir_sessions
            .lock()
            .unwrap()
            .attachments
            .contains_key(&worker)
    );
    let attempts = fixture.transport.attempts();
    assert_eq!(attempts.len(), 2, "retry must actually contact Runtime");
    assert_eq!(
        attempts[0], attempts[1],
        "retry closes the SAME Runtime session"
    );
    fixture.assert_removed().await;
}

#[tokio::test]
async fn cancelled_backend_close_retry_waits_for_actual_provider_teardown_before_confirmation() {
    let fixture = Fixture::new().await;
    let gate = Arc::new(ProviderCloseGate {
        entered: Notify::new(),
        release: Semaphore::new(0),
    });
    fixture.source.hold_provider_close(gate.clone());
    *fixture.transport.direct_runtime.lock().unwrap() = Some(fixture.source.runtime_router());
    let worker = fixture.registry_owner();
    let session = fixture.open().await;
    session
        .start_command(workdir::CommandRequest {
            command: "sleep 60".into(),
            timeout_secs: 60,
            transport_byte_budget: workdir::MAX_COMMAND_TRANSPORT_BYTES,
            cwd: workdir::WorkdirPath::root(),
            tool_call_id: None,
        })
        .await
        .unwrap();
    fixture
        .api()
        .workdir_sessions
        .lock()
        .unwrap()
        .insert_attachment(worker.clone(), session.clone());
    fixture.assert_busy().await;
    let api = fixture.api().clone();
    let first_worker = worker.clone();
    let first = tokio::spawn(async move { close(&api, &first_worker, ClosePath::Registry).await });
    tokio::time::timeout(std::time::Duration::from_secs(5), gate.entered.notified())
        .await
        .expect("actual Local provider teardown reached Pending");
    fixture.transport.dispatched.notified().await;
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    fixture.assert_restored(&worker, &session);
    fixture.assert_busy().await;

    let api = fixture.api().clone();
    let retry_worker = worker.clone();
    let retry = tokio::spawn(async move { close(&api, &retry_worker, ClosePath::Registry).await });
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        fixture.transport.dispatched.notified(),
    )
    .await
    .expect("retry reached production Runtime handler");
    assert_eq!(
        *fixture.transport.pending_dispatches.lock().unwrap(),
        vec![true, true],
        "duplicate DELETE must wait, not acknowledge an extracted record"
    );
    assert!(
        !retry.is_finished(),
        "Backend cannot confirm while provider close is held"
    );
    assert!(fixture.transport.runtime_closes.lock().unwrap().is_empty());
    fixture.assert_busy().await;
    gate.release.add_permits(1);
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(5), retry)
            .await
            .unwrap()
            .unwrap()
    );
    assert!(
        !fixture
            .api()
            .workdir_sessions
            .lock()
            .unwrap()
            .attachments
            .contains_key(&worker)
    );
    let attempts = fixture.transport.attempts();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0], attempts[1]);
    fixture.assert_removed().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registry_close_failure_restores_same_remote_session_and_retry_releases_runtime_lease() {
    failure_restoration(ClosePath::Registry).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attachment_close_failure_restores_same_remote_session_and_retry_releases_runtime_lease() {
    failure_restoration(ClosePath::Attachment).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lost_close_response_restores_handle_and_retry_confirms_actual_runtime_idempotent_release()
{
    let fixture = Fixture::new().await;
    let worker = fixture.registry_owner();
    let session = fixture.open().await;
    fixture
        .api()
        .workdir_sessions
        .lock()
        .unwrap()
        .insert_attachment(worker.clone(), session.clone());
    fixture.assert_busy().await;
    fixture
        .transport
        .fail_next(CloseFault::RejectAfterRuntimeClose);

    assert!(!close(fixture.api(), &worker, ClosePath::Registry).await);
    fixture.assert_restored(&worker, &session);
    let first_attempt = fixture.transport.attempts();
    assert_eq!(first_attempt.len(), 1);
    assert_eq!(
        *fixture.transport.runtime_closes.lock().unwrap(),
        vec![(first_attempt[0].clone(), StatusCode::NO_CONTENT)],
        "fault must follow the real authenticated Runtime's successful DELETE"
    );
    let absent = session
        .stat(workdir::StatRequest {
            path: workdir::WorkdirPath::new("README.md").unwrap(),
        })
        .await
        .expect_err("original session must already be absent on Runtime");
    assert!(
        matches!(
            absent.classification_source(),
            workdir::WorkdirError::NotFound(_)
        ),
        "must observe Runtime absence, not a local closed flag: {absent:?}"
    );

    assert!(close(fixture.api(), &worker, ClosePath::Registry).await);
    let attempts = fixture.transport.attempts();
    assert_eq!(
        attempts,
        vec![first_attempt[0].clone(), first_attempt[0].clone()],
        "retry must send DELETE to the same already-absent Runtime session"
    );
    assert_eq!(
        *fixture.transport.runtime_closes.lock().unwrap(),
        vec![
            (attempts[0].clone(), StatusCode::NO_CONTENT),
            (attempts[1].clone(), StatusCode::NO_CONTENT)
        ],
        "Runtime itself must acknowledge the idempotent retry"
    );
    assert!(
        !fixture
            .api()
            .workdir_sessions
            .lock()
            .unwrap()
            .attachments
            .contains_key(&worker)
    );
    // Runtime has released its leases, but only now has Backend confirmed it.
    fixture.assert_removed().await;
}

async fn cancellation_restoration(path: ClosePath) {
    let fixture = Fixture::new().await;
    let worker = fixture.registry_owner();
    let session = fixture.open().await;
    fixture
        .api()
        .workdir_sessions
        .lock()
        .unwrap()
        .insert_attachment(worker.clone(), session.clone());
    // A second real Runtime session proves the batch retains not-yet-attempted
    // command sources as well as the close currently awaiting a response.
    let command = if matches!(path, ClosePath::Registry) {
        let source = fixture.open().await;
        let handle = source
            .start_command(workdir::CommandRequest {
                command: "printf close-test".into(),
                timeout_secs: 5,
                transport_byte_budget: workdir::MAX_COMMAND_TRANSPORT_BYTES,
                cwd: workdir::WorkdirPath::root(),
                tool_call_id: None,
            })
            .await
            .unwrap();
        let external = fixture
            .api()
            .workdir_sessions
            .lock()
            .unwrap()
            .register_command(worker.clone(), "checkout".into(), source.clone(), handle);
        Some((external, source))
    } else {
        None
    };
    fixture.assert_busy().await;
    fixture.transport.fail_next(CloseFault::HoldThenReject);
    let api = fixture.api().clone();
    let closing_worker = worker.clone();
    let closing = tokio::spawn(async move { close(&api, &closing_worker, path).await });
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        fixture.transport.entered.notified(),
    )
    .await
    .expect("close reached pre-Runtime hold");
    closing.abort();
    assert!(closing.await.unwrap_err().is_cancelled());
    fixture.assert_restored(&worker, &session);
    fixture.assert_live(&session).await;
    if let Some((external, source)) = &command {
        let restored = fixture
            .api()
            .workdir_sessions
            .lock()
            .unwrap()
            .command(&worker, "checkout", external)
            .expect("unattempted command session retained");
        assert!(Arc::ptr_eq(&restored.source, source));
    }
    fixture.transport.release.add_permits(1);
    fixture.assert_busy().await;
    assert!(close(fixture.api(), &worker, path).await);
    let attempts = fixture.transport.attempts();
    assert_eq!(attempts.len(), if command.is_some() { 3 } else { 2 });
    assert_eq!(
        attempts[0], attempts[1],
        "retry closes cancelled session, not a replacement"
    );
    fixture.assert_removed().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registry_close_cancellation_restores_inflight_and_unattempted_remote_sessions() {
    cancellation_restoration(ClosePath::Registry).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attachment_close_cancellation_restores_remote_session_for_retry() {
    cancellation_restoration(ClosePath::Attachment).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn observed_stop_close_failure_retains_old_command_without_replacing_new_attachment() {
    let fixture = Fixture::new().await;
    let worker = fixture.registry_owner();
    let old = fixture.open().await;
    let new = fixture.open().await;
    let handle = fixture
        .api()
        .workdir_sessions
        .lock()
        .unwrap()
        .register_command(
            worker.clone(),
            "checkout".into(),
            old.clone(),
            CommandHandle("observed-provider-command".into()),
        );
    let observed = fixture
        .api()
        .workdir_sessions
        .lock()
        .unwrap()
        .observe_worker(&worker);
    fixture
        .api()
        .workdir_sessions
        .lock()
        .unwrap()
        .insert_attachment(worker.clone(), new.clone());
    *fixture.transport.fault.lock().unwrap() = Some(CloseFault::Reject);
    let _guard = current_worker_session_lock(fixture.api(), &worker)
        .lock_owned()
        .await;
    assert!(
        close_observed_worker_workdir_sessions(&fixture.api().workdir_sessions, &observed)
            .await
            .is_err()
    );
    {
        let registry = fixture.api().workdir_sessions.lock().unwrap();
        assert!(Arc::ptr_eq(
            registry.attachments.get(&worker).unwrap(),
            &new
        ));
        assert!(Arc::ptr_eq(
            &registry
                .command(&worker, "checkout", &handle)
                .unwrap()
                .source,
            &old
        ));
    }
    fixture.assert_live(&old).await;
    fixture.assert_live(&new).await;
    close_observed_worker_workdir_sessions(&fixture.api().workdir_sessions, &observed)
        .await
        .unwrap();
    assert_eq!(fixture.transport.attempts().len(), 2);
    assert_eq!(
        fixture.transport.attempts()[0],
        fixture.transport.attempts()[1]
    );
    fixture.assert_live(&new).await;
    assert!(
        fixture
            .api()
            .workdir_sessions
            .lock()
            .unwrap()
            .command(&worker, "checkout", &handle)
            .is_none()
    );
    close_worker_workdir_sessions(&fixture.api().workdir_sessions, &worker)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_projection_does_not_follow_a_rebound_connection_with_identical_worker_alias_and_workdir()
 {
    let fixture = Fixture::new().await;
    let api = fixture.api();
    let worker = fixture.create_worker().await;
    let link = WorkerWorkdirLinkRecord {
        connection_id: String::new(),
        workspace_id: TEST_WORKSPACE_ID.into(),
        worker: worker.clone(),
        workdir_id: fixture.workdir_id.clone(),
        alias: "checkout".into(),
        capabilities: workdir::WorkdirSessionCapabilities::READ_ONLY,
        linked_at: TEST_CREATED_AT.into(),
        unlinked_at: None,
    };
    let _guard = current_worker_session_lock(api, &worker).lock_owned().await;
    let old = api.store.attach_worker_workdir(&link).unwrap();
    api.store
        .detach_worker_workdir_connection(
            TEST_WORKSPACE_ID,
            &worker,
            &old.alias,
            &old.connection_id,
            TEST_CREATED_AT,
        )
        .unwrap();
    let new = api.store.attach_worker_workdir(&link).unwrap();
    assert_ne!(old.connection_id, new.connection_id);
    let mut record = api
        .store
        .get_workdir_registry(TEST_WORKSPACE_ID, &fixture.workdir_id)
        .unwrap()
        .unwrap();
    record.display_name = Some("new binding not owned by old stop".into());
    api.store.upsert_workdir_registry(&record).unwrap();
    sync_linked_workdir_after_worker_stop(api, &worker, &[old]).unwrap();
    assert_eq!(
        api.store
            .get_workdir_registry(TEST_WORKSPACE_ID, &fixture.workdir_id)
            .unwrap()
            .unwrap()
            .display_name,
        record.display_name
    );
    sync_linked_workdir_after_worker_stop(api, &worker, &[new]).unwrap();
    assert_ne!(
        api.store
            .get_workdir_registry(TEST_WORKSPACE_ID, &fixture.workdir_id)
            .unwrap()
            .unwrap()
            .display_name,
        record.display_name,
        "the same current connection still projects normally"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_removal_close_failure_retains_attachment_and_retry_releases_runtime_lease() {
    let fixture = Fixture::new().await;
    let api = fixture.api();
    let worker = fixture.create_worker().await;
    let stopped = api
        .runtime
        .stop_worker(
            &worker,
            WorkerLifecycleRequest {
                stop_intent: None,
                reason: Some("prepare removal".into()),
                ticket_assignment: None,
            },
        )
        .unwrap();
    assert_eq!(
        stopped.state,
        InternalWorkerOperationState::Accepted,
        "{stopped:?}"
    );
    let summary = api.runtime.worker(&worker).unwrap();
    assert_eq!(summary.state, "stopped");
    sync_worker_observation(api, &summary).unwrap();
    fixture.source.seed_remote_session_for_cleanup(&worker);
    let session = fixture.open().await;
    api.workdir_sessions
        .lock()
        .unwrap()
        .insert_attachment(worker.clone(), session.clone());
    fixture.assert_busy().await;
    api.store
        .attach_worker_workdir(&WorkerWorkdirLinkRecord {
            connection_id: String::new(),
            workspace_id: TEST_WORKSPACE_ID.into(),
            worker: worker.clone(),
            workdir_id: fixture.workdir_id.clone(),
            alias: "checkout".into(),
            capabilities: workdir::WorkdirSessionCapabilities::ALL,
            linked_at: TEST_CREATED_AT.into(),
            unlinked_at: None,
        })
        .unwrap();
    fixture.transport.fail_next(CloseFault::Reject);
    let removal = WorkerRemovalService::new(api);
    let failed = removal
        .execute_target_removal(&api.runtime, &worker, "remove stopped Worker", None)
        .await
        .unwrap();
    assert_eq!(
        failed.status,
        StatusCode::SERVICE_UNAVAILABLE.as_u16(),
        "{}",
        failed.body
    );
    assert!(
        failed.body.contains("attachment_close_failed"),
        "{}",
        failed.body
    );
    fixture.assert_restored(&worker, &session);
    fixture.assert_live(&session).await;
    assert!(
        api.store
            .list_worker_workdir_links(TEST_WORKSPACE_ID, &worker)
            .unwrap()
            .iter()
            .any(|link| link.unlinked_at.is_none())
    );
    assert!(
        api.store
            .get_worker_registry(TEST_WORKSPACE_ID, &worker)
            .unwrap()
            .is_some()
    );
    let operation = api
        .store
        .recover_worker_removal_execution(TEST_WORKSPACE_ID, &worker)
        .unwrap()
        .unwrap();
    assert_eq!(
        operation.plan.state,
        crate::retention::WorkerRemovalPlanState::Failed
    );
    let retried = removal
        .execute_target_removal(&api.runtime, &worker, "remove stopped Worker", None)
        .await
        .unwrap();
    assert_eq!(retried.status, StatusCode::OK.as_u16(), "{}", retried.body);
    assert!(
        api.store
            .get_worker_registry(TEST_WORKSPACE_ID, &worker)
            .unwrap()
            .is_none()
    );
    assert!(
        api.store
            .list_worker_workdir_links(TEST_WORKSPACE_ID, &worker)
            .unwrap()
            .iter()
            .all(|link| link.unlinked_at.is_some())
    );
    let attempts = fixture.transport.attempts();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0], attempts[1]);
    fixture.assert_removed().await;
}

#[tokio::test]
async fn session_close_failure_retains_provider_cause_and_restores_the_same_registry_session() {
    use error_contract::{FailureFacts, Tool, project_failure};
    use workdir::failure::{WorkdirFailureOperation, WorkdirFailureVisibility};
    let fixture = Fixture::new().await;
    let worker = fixture.registry_owner();
    let session = fixture.open().await;
    fixture
        .api()
        .workdir_sessions
        .lock()
        .unwrap()
        .insert_attachment(worker.clone(), session.clone());
    fixture.transport.fail_next(CloseFault::Reject);
    let error = close_worker_workdir_sessions(&fixture.api().workdir_sessions, &worker)
        .await
        .unwrap_err();
    assert_eq!(error.session_errors().len(), 1);
    let original =
        workdir::http::WorkdirTransportError::from_workdir_error(&error.session_errors()[0]);
    let details = error.details();
    assert_eq!(details.len(), 1);
    assert_eq!(details[0].operation, WorkdirFailureOperation::SessionClose);
    assert_eq!(details[0].classification, original.code);
    assert_eq!(details[0].failure_reason, original.failure_reason);
    let projected = project_failure::<Tool, _>(
        &FailureFacts::new(details[0]).unwrap(),
        &WorkdirFailureVisibility {
            can_view_cause: true,
        },
    )
    .unwrap();
    assert!(projected.explanation().contains("session closing"));
    assert!(
        !projected
            .explanation()
            .contains(&fixture.materialization_root.display().to_string())
    );
    fixture.assert_restored(&worker, &session);
    fixture.assert_live(&session).await;
    assert_eq!(
        fixture.transport.attempts().len(),
        1,
        "unconfirmed close does not resend"
    );
    close_worker_workdir_sessions(&fixture.api().workdir_sessions, &worker)
        .await
        .unwrap();
    assert_eq!(
        fixture.transport.attempts()[0],
        fixture.transport.attempts()[1]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accepted_stop_http_keeps_cleanup_failure_separate_and_does_not_retry_stop_or_close() {
    let fixture = Fixture::new().await;
    let api = fixture.api();
    let worker = fixture.create_worker().await;
    let owner = seed_test_workspace_owner_token(api.store.as_ref(), "stop-cleanup-owner").await;
    let session = fixture.open().await;
    api.workdir_sessions
        .lock()
        .unwrap()
        .insert_attachment(worker.clone(), session.clone());
    fixture.transport.fail_next(CloseFault::Reject);
    let response = build_router(api.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/api/w/{TEST_WORKSPACE_ID}/runtimes/{}/workers/{}/stop",
                    worker.runtime_id, worker.worker_id
                ))
                .header("authorization", format!("Bearer {owner}"))
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"reason":"verify independent post-stop cleanup"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let body: server_api::RuntimeWorkerLifecycleResult = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    assert_eq!(
        body.state,
        server_api::WorkerOperationState::Accepted,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    assert_eq!(body.cleanup_failures.len(), 1);
    assert_eq!(api.runtime.worker(&worker).unwrap().state, "stopped");
    match body.cleanup_failures[0].decode::<workdir::failure::WorkdirFailureDetails>() {
        error_contract::DecodeFailure::Known(received) => assert_eq!(
            received.facts().cause().operation,
            workdir::failure::WorkdirFailureOperation::SessionClose
        ),
        _ => panic!(
            "the actual close cause must cross the HTTP result independently of accepted Stop"
        ),
    }
    fixture.assert_restored(&worker, &session);
    fixture.assert_live(&session).await;
    assert_eq!(fixture.transport.attempts().len(), 1);
    close_worker_workdir_sessions(&api.workdir_sessions, &worker)
        .await
        .unwrap();
    assert_eq!(fixture.transport.attempts().len(), 2);
    assert_eq!(
        fixture.transport.attempts()[0],
        fixture.transport.attempts()[1]
    );
    assert_eq!(api.runtime.worker(&worker).unwrap().state, "stopped");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn management_failure_crosses_signed_runtime_http_and_remote_without_losing_busy_or_not_found()
 {
    use error_contract::{Browser, FailureFacts, Tool, project_failure};
    use workdir::management_failure::{
        WorkdirManagementOperation as Operation, WorkdirManagementReason as Reason,
        WorkdirManagementVisibility,
    };
    let fixture = Fixture::new().await;
    let live = fixture.open().await;
    let registry = fixture.api().runtime.clone();
    let runtime = fixture.runtime_id.clone();
    let id = fixture.workdir_id.clone();
    let rejected =
        tokio::task::spawn_blocking(move || registry.cleanup_working_directory(&runtime, &id))
            .await
            .unwrap()
            .unwrap();
    assert_eq!(rejected.state, InternalWorkerOperationState::Rejected);
    let diagnostic = rejected
        .diagnostics
        .iter()
        .find(|value| value.workdir.is_some())
        .unwrap();
    let cause = diagnostic.workdir.unwrap();
    assert_eq!(cause.operation, Operation::Cleanup);
    assert_eq!(cause.reason, Reason::CleanupResourceBusy);
    let reopened: RuntimeDiagnostic =
        serde_json::from_str(&serde_json::to_string(diagnostic).unwrap()).unwrap();
    assert_eq!(reopened.workdir, Some(cause));
    let facts = FailureFacts::new(cause).unwrap();
    let visibility = WorkdirManagementVisibility {
        can_view_cause: true,
    };
    let browser = project_failure::<Browser, _>(&facts, &visibility).unwrap();
    let tool = project_failure::<Tool, _>(&facts, &visibility).unwrap();
    assert_eq!(browser.details(), tool.details());
    assert_eq!(browser.explanation(), tool.explanation());
    assert!(browser.explanation().contains("removal") && browser.explanation().contains("in use"));
    assert!(
        !serde_json::to_string(&reopened)
            .unwrap()
            .contains(&fixture.materialization_root.display().to_string())
    );
    fixture.assert_live(&live).await;
    let registry = fixture.api().runtime.clone();
    let runtime = fixture.runtime_id.clone();
    let missing = tokio::task::spawn_blocking(move || {
        registry.working_directory(&runtime, "never-created-workdir")
    })
    .await
    .unwrap()
    .unwrap();
    let failure = missing
        .diagnostics
        .iter()
        .find_map(|value| value.workdir)
        .unwrap();
    assert_eq!(failure.operation, Operation::Detail);
    assert_eq!(failure.reason, Reason::NotFound);
    live.close().await.unwrap();
    fixture.assert_removed().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handler_detach_after_stat_and_delivered_command_allows_ordinary_workdir_delete() {
    let fixture = Fixture::new().await;
    let worker = fixture.create_worker().await;
    let link = fixture.attach(&worker, "checkout").await;
    fixture.stat_attachment(&link).await;
    fixture.complete_command(&link).await;
    fixture.detach(&link).await;
    fixture.assert_removed().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handler_worker_removal_after_stat_and_delivered_command_allows_ordinary_workdir_delete() {
    let fixture = Fixture::new().await;
    let worker = fixture.create_worker().await;
    let link = fixture.attach(&worker, "checkout").await;
    fixture.stat_attachment(&link).await;
    fixture.complete_command(&link).await;
    fixture.remove_worker(&worker).await;
    fixture.assert_removed().await;
}

async fn delivered_terminal_overflow_releases_sessions(remove_worker: bool) {
    let fixture = Fixture::new().await;
    let worker = fixture.create_worker().await;
    let link = fixture.attach(&worker, "checkout").await;
    let mut handles = Vec::new();
    // Cross the production retry-retention bound using delivered terminal output,
    // not injected registry entries or status-only completion observations.
    for index in 0..132 {
        if index == 128 {
            fixture.transport.fail_next(CloseFault::Reject);
        }
        handles.push(fixture.complete_command(&link).await);
    }
    {
        let registry = fixture.api().workdir_sessions.lock().unwrap();
        assert_eq!(registry.active_command_count(&worker), 0);
        assert!(registry.command(&worker, "checkout", &handles[0]).is_none());
        assert!(
            registry
                .command(&worker, "checkout", handles.last().unwrap())
                .is_some()
        );
    }
    let attempts = fixture.transport.attempts();
    assert!(
        attempts.windows(2).any(|pair| pair[0] == pair[1]),
        "failed eviction close must be retried without losing terminal delivery: {attempts:?}"
    );
    if remove_worker {
        fixture.remove_worker(&worker).await;
    } else {
        fixture.detach(&link).await;
    }
    fixture.assert_removed().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handler_terminal_command_overflow_then_detach_has_no_cumulative_runtime_use() {
    delivered_terminal_overflow_releases_sessions(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handler_terminal_command_overflow_then_worker_removal_has_no_cumulative_runtime_use() {
    delivered_terminal_overflow_releases_sessions(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handler_repeated_attachment_switches_reject_old_detach_and_release_completed_lifetimes() {
    let fixture = Fixture::new().await;
    let worker = fixture.create_worker().await;
    let unrelated = fixture.open().await;
    let mut previous: Option<WorkerWorkdirLinkRecord> = None;
    for _ in 0..4 {
        let current = fixture.attach(&worker, "checkout").await;
        if let Some(old) = &previous {
            assert_ne!(old.connection_id, current.connection_id);
            assert!(
                scoped_detach_current_worker_workdir(
                    State(fixture.api().clone()),
                    AxumPath((TEST_WORKSPACE_ID.into(), old.alias.clone())),
                    Fixture::worker_headers(&worker),
                    server_api::CurrentWorkerWorkdirDetachQuery {
                        expected_connection_id: Some(old.connection_id.clone()),
                    },
                )
                .await
                .is_err(),
                "stale lifetime must not detach the new connection"
            );
        }
        fixture.stat_attachment(&current).await;
        fixture.complete_command(&current).await;
        fixture.detach(&current).await;
        fixture.assert_live(&unrelated).await;
        fixture.assert_busy().await;
        previous = Some(current);
    }
    unrelated.close().await.unwrap();
    fixture.assert_removed().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropped_remote_close_failure_is_retried_by_delete_without_closing_new_live_attachment() {
    let fixture = Fixture::new().await;
    let abandoned = fixture.open().await;
    fixture.assert_live(&abandoned).await;
    fixture.transport.fail_next(CloseFault::HoldThenReject);
    drop(abandoned);
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        fixture.transport.entered.notified(),
    )
    .await
    .expect("last-handle Drop attempts real remote close");
    let first = fixture.transport.attempts();
    assert_eq!(first.len(), 1);
    let worker = fixture.create_worker().await;
    let link = fixture.attach(&worker, "checkout").await;
    fixture.stat_attachment(&link).await;
    fixture.transport.release.add_permits(1);
    fixture.detach(&link).await;
    // An unrelated live user has no Backend attachment record to guard it; only
    // the real Runtime operation lease must keep ordinary Delete busy.
    let live = fixture.open().await;
    fixture.assert_busy().await;
    fixture.assert_live(&live).await;
    let attempts = fixture.transport.attempts();
    assert!(
        attempts.iter().filter(|path| *path == &first[0]).count() >= 2,
        "ordinary Delete retries the failed orphan close: {attempts:?}"
    );
    live.close().await.unwrap();
    fixture.assert_removed().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lost_successful_runtime_open_response_is_recovered_by_ordinary_workdir_delete() {
    let fixture = Fixture::new().await;
    *fixture.transport.open_fault.lock().unwrap() = Some(OpenFault::RejectAfterRuntimeOpen);
    let result = fixture
        .api()
        .runtime
        .open_workdir_session(&fixture.runtime_id, &fixture.workdir_id, None)
        .await;
    assert!(result.is_err(), "only the successful OPEN response is lost");
    let opened = fixture.transport.runtime_opens.lock().unwrap().clone();
    assert_eq!(opened.len(), 1, "upstream must actually create the session");
    assert_eq!(opened[0].workdir_id.as_str(), fixture.workdir_id);
    fixture.assert_removed().await;
    assert!(
        fixture
            .transport
            .runtime_closes
            .lock()
            .unwrap()
            .iter()
            .any(|(path, status)| {
                path.ends_with(opened[0].session_id.as_str()) && *status == StatusCode::NO_CONTENT
            }),
        "close must target the identity from the actual lost OPEN response"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_open_waiter_after_runtime_success_does_not_lose_close_identity() {
    let fixture = Fixture::new().await;
    *fixture.transport.open_fault.lock().unwrap() = Some(OpenFault::HoldAfterRuntimeOpen);
    let api = fixture.api().clone();
    let runtime_id = fixture.runtime_id.clone();
    let workdir_id = fixture.workdir_id.clone();
    let opening = tokio::spawn(async move {
        api.runtime
            .open_workdir_session(&runtime_id, &workdir_id, None)
            .await
    });
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        fixture.transport.opened.notified(),
    )
    .await
    .expect("real authenticated Runtime OPEN succeeded before caller cancellation");
    let opened = fixture.transport.runtime_opens.lock().unwrap().clone();
    assert_eq!(opened.len(), 1);
    opening.abort();
    assert!(opening.await.unwrap_err().is_cancelled());
    fixture.transport.fail_next(CloseFault::HoldThenReject);
    fixture.transport.open_release.add_permits(1);
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        fixture.transport.entered.notified(),
    )
    .await
    .expect("cancelled caller's OPEN result still schedules its own close");
    assert!(fixture.transport.attempts()[0].ends_with(opened[0].session_id.as_str()));
    fixture.transport.release.add_permits(1);
    fixture.assert_removed().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropped_unregistered_remote_session_does_not_strand_runtime_use_after_worker_removal() {
    let fixture = Fixture::new().await;
    let worker = fixture.create_worker().await;
    fixture
        .api()
        .runtime
        .stop_worker(
            &worker,
            WorkerLifecycleRequest {
                stop_intent: None,
                reason: Some("prepare removal".into()),
                ticket_assignment: None,
            },
        )
        .unwrap();
    let summary = fixture.api().runtime.worker(&worker).unwrap();
    sync_worker_observation(fixture.api(), &summary).unwrap();
    fixture.source.seed_remote_session_for_cleanup(&worker);
    // A Runtime session opened by the Backend need not have reached the
    // attachment cache before the request is cancelled or its handle is dropped.
    let source = fixture.open().await;
    fixture.assert_live(&source).await;
    fixture.assert_busy().await;
    drop(source);
    let removal = WorkerRemovalService::new(fixture.api());
    let response = removal
        .execute_target_removal(
            &fixture.api().runtime,
            &worker,
            "remove stopped Worker",
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        response.status,
        StatusCode::OK.as_u16(),
        "{}",
        response.body
    );
    fixture.assert_removed().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn replacing_remote_connection_retains_failed_orphan_close_at_its_original_destination() {
    let fixture = Fixture::new().await;
    let abandoned = fixture.open().await;
    fixture.transport.fail_next(CloseFault::HoldThenReject);
    drop(abandoned);
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        fixture.transport.entered.notified(),
    )
    .await
    .unwrap();
    let first = fixture.transport.attempts();
    let binding = fixture
        .api()
        .store
        .get_workspace_runtime_binding(TEST_WORKSPACE_ID, &fixture.runtime_id)
        .await
        .unwrap()
        .unwrap();
    // Normal binding refresh replaces the connection object, not the Runtime.
    // Its current endpoint is direct; the orphan descriptor still uses the proxy.
    fixture
        .api()
        .register_workspace_runtime_binding(binding)
        .unwrap();
    fixture.transport.release.add_permits(1);
    let current = fixture.open().await;
    fixture.assert_busy().await;
    fixture.assert_live(&current).await;
    assert!(
        fixture
            .transport
            .attempts()
            .iter()
            .filter(|path| *path == &first[0])
            .count()
            >= 2
    );
    current.close().await.unwrap();
    fixture.assert_removed().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn removing_one_worker_keeps_another_workers_attachment_and_runtime_session_live() {
    let fixture = Fixture::new().await;
    let first = fixture.create_worker().await;
    let second = fixture.create_worker().await;
    let first_link = fixture.attach(&first, "checkout").await;
    let (_, Json(created)) = create_workspace_working_directory(
        fixture.api(),
        TEST_WORKSPACE_ID,
        Some(&fixture.runtime_id),
        BrowserWorkingDirectoryCreateRequest {
            runtime_id: Some(fixture.runtime_id.clone()),
            display_name: None,
            repository_key: "test-repository".into(),
            selector: Some("HEAD".into()),
            operation_id: Some("unrelated-worker-workdir".into()),
        },
    )
    .await
    .unwrap();
    let second_id = created.item.working_directory_id;
    let second_link = fixture
        .attach_workdir(&second, "checkout", &second_id)
        .await;
    fixture.stat_attachment(&first_link).await;
    fixture.stat_attachment(&second_link).await;
    fixture.remove_worker(&first).await;
    fixture.stat_attachment(&second_link).await;
    assert!(
        fixture
            .api()
            .store
            .list_worker_workdir_links(TEST_WORKSPACE_ID, &second)
            .unwrap()
            .iter()
            .any(|link| link.connection_id == second_link.connection_id
                && link.unlinked_at.is_none())
    );
    fixture.assert_removed().await;
    fixture.stat_attachment(&second_link).await;
    fixture.detach(&second_link).await;
    let api = fixture.api().clone();
    let result = tokio::task::spawn_blocking(move || {
        execute_workdir_removal(
            &api,
            &second_id,
            "account:close-test",
            "remove independent fixture",
            &uuid::Uuid::now_v7().to_string(),
        )
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        result.disposition,
        WorkingDirectoryRemovalDisposition::Removed
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_detach_failure_and_worker_remove_retry_close_the_same_owned_session() {
    let fixture = Fixture::new().await;
    let worker = fixture.create_worker().await;
    let link = fixture.attach(&worker, "checkout").await;
    fixture.stat_attachment(&link).await;
    fixture
        .api()
        .runtime
        .stop_worker(
            &worker,
            WorkerLifecycleRequest {
                stop_intent: None,
                reason: Some("prepare concurrent teardown".into()),
                ticket_assignment: None,
            },
        )
        .unwrap();
    sync_worker_observation(
        fixture.api(),
        &fixture.api().runtime.worker(&worker).unwrap(),
    )
    .unwrap();
    fixture.source.seed_remote_session_for_cleanup(&worker);
    let before = fixture.transport.attempts().len();
    fixture.transport.fail_next(CloseFault::HoldThenReject);
    let detach = {
        let api = fixture.api().clone();
        let link = link.clone();
        tokio::spawn(async move {
            scoped_detach_current_worker_workdir(
                State(api),
                AxumPath((TEST_WORKSPACE_ID.into(), link.alias)),
                Fixture::worker_headers(&link.worker),
                server_api::CurrentWorkerWorkdirDetachQuery {
                    expected_connection_id: Some(link.connection_id),
                },
            )
            .await
        })
    };
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        fixture.transport.entered.notified(),
    )
    .await
    .unwrap();
    let remove = {
        let api = fixture.api().clone();
        let worker = worker.clone();
        tokio::spawn(async move {
            WorkerRemovalService::new(&api)
                .execute_target_removal(&api.runtime, &worker, "concurrent teardown", None)
                .await
        })
    };
    fixture.transport.release.add_permits(1);
    let error = detach.await.unwrap().unwrap_err();
    assert!(matches!(
        &error.error,
        Error::WorkdirSessionCloseFailed { .. }
    ));
    let public = error.into_workdir_attachment_api_error(
        workdir::management_failure::WorkdirManagementOperation::Detach,
    );
    assert!(matches!(
        public.cause(),
        Some(server_api::workdir_management_error::WorkdirManagementCause::Session(cause))
            if cause.operation == workdir::failure::WorkdirFailureOperation::SessionClose
    ));
    let result = remove.await.unwrap().unwrap();
    assert_eq!(result.status, StatusCode::OK.as_u16(), "{}", result.body);
    let attempts = fixture.transport.attempts();
    assert_eq!(attempts.len() - before, 2);
    assert_eq!(attempts[before], attempts[before + 1]);
    fixture.assert_removed().await;
}

async fn cancelled_attachment_open_is_drained_by_terminal_owner(remove: bool) {
    let fixture = Fixture::new().await;
    let worker = fixture.create_worker().await;
    *fixture.transport.open_fault.lock().unwrap() = Some(OpenFault::HoldAfterRuntimeOpen);
    let attach = {
        let api = fixture.api().clone();
        let worker = worker.clone();
        let workdir_id = fixture.workdir_id.clone();
        tokio::spawn(async move {
            scoped_attach_current_worker_workdir(
                State(api),
                AxumPath(ScopedWorkspacePath {
                    workspace_id: TEST_WORKSPACE_ID.into(),
                }),
                Fixture::worker_headers(&worker),
                Json(AttachCurrentWorkerWorkdirRequest {
                    alias: "checkout".into(),
                    working_directory_id: workdir_id,
                }),
            )
            .await
        })
    };
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        fixture.transport.opened.notified(),
    )
    .await
    .unwrap();
    let opened = fixture.transport.runtime_opens.lock().unwrap()[0].clone();
    attach.abort();
    assert!(attach.await.unwrap_err().is_cancelled());
    let link = fixture
        .api()
        .store
        .list_worker_workdir_links(TEST_WORKSPACE_ID, &worker)
        .unwrap()
        .into_iter()
        .find(|link| link.unlinked_at.is_none())
        .unwrap();
    fixture.transport.open_release.add_permits(1);
    if remove {
        fixture.remove_worker(&worker).await;
    } else {
        fixture.detach(&link).await;
    }
    assert!(
        fixture
            .transport
            .runtime_closes
            .lock()
            .unwrap()
            .iter()
            .any(|(path, status)| path.ends_with(opened.session_id.as_str())
                && *status == StatusCode::NO_CONTENT),
        "terminal owner must confirm the otherwise-unregistered open before reporting success"
    );
    fixture.assert_removed().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn detach_drains_a_cancelled_attachment_open_not_present_in_the_backend_session_cache() {
    cancelled_attachment_open_is_drained_by_terminal_owner(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_remove_drains_a_cancelled_attachment_open_not_present_in_the_backend_session_cache()
{
    cancelled_attachment_open_is_drained_by_terminal_owner(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abandoned_close_failure_preserves_session_cause_and_never_dispatches_directory_removal() {
    use error_contract::{Browser, DecodeFailure, FailureFacts, Tool, project_failure};
    use workdir::failure::{
        WorkdirFailureDetails, WorkdirFailureOperation, WorkdirFailureVisibility,
    };
    let fixture = Fixture::new().await;
    let session = fixture.open().await;
    fixture.transport.fail_next(CloseFault::HoldThenReject);
    drop(session);
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        fixture.transport.entered.notified(),
    )
    .await
    .expect("last handle starts its retained close");
    fixture.transport.fail_next(CloseFault::Reject);
    fixture.transport.release.add_permits(1);
    let api = fixture.api().clone();
    let runtime_id = fixture.runtime_id.clone();
    let workdir_id = fixture.workdir_id.clone();
    let result = tokio::task::spawn_blocking(move || {
        api.runtime
            .cleanup_working_directory(&runtime_id, &workdir_id)
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(result.state, InternalWorkerOperationState::Rejected);
    assert_eq!(
        workdir_cleanup_failure_category(&result),
        "session_close_unconfirmed"
    );
    assert_eq!(
        fixture
            .transport
            .removals
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    assert!(fixture.materialization_root.exists());
    let saved = serde_json::to_vec(&result.diagnostics).unwrap();
    let mut reopened: Vec<RuntimeDiagnostic> = serde_json::from_slice(&saved).unwrap();
    let cause = reopened[0].session.unwrap();
    assert_eq!(cause.operation, WorkdirFailureOperation::SessionClose);
    assert!(reopened[0].workdir.is_none() && reopened[0].control.is_none());
    let facts = FailureFacts::new(cause).unwrap();
    let visibility = WorkdirFailureVisibility {
        can_view_cause: true,
    };
    let browser = project_failure::<Browser, _>(&facts, &visibility).unwrap();
    let tool = project_failure::<Tool, _>(&facts, &visibility).unwrap();
    assert_eq!(browser.details(), tool.details());
    assert_eq!(browser.explanation(), tool.explanation());
    reopened[0].message = "TEST_ONLY_SECRET /private/provider raw prose now says success".into();
    reopened[0].code = "arbitrary_changed_diagnostic_code".into();
    let public = workdir_management_diagnostics(reopened);
    assert_eq!(public[0].message, browser.explanation());
    match public[0]
        .failure
        .as_ref()
        .unwrap()
        .decode::<WorkdirFailureDetails>()
    {
        DecodeFailure::Known(received) => assert_eq!(received.facts().cause(), &cause),
        _ => panic!("typed close evidence must survive current Browser projection"),
    }
    assert!(
        !serde_json::to_string(&public)
            .unwrap()
            .contains("TEST_ONLY_SECRET")
    );
    assert_eq!(
        fixture
            .transport
            .removals
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    fixture.assert_removed().await;
    assert_eq!(
        fixture
            .transport
            .removals
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    let attempts = fixture.transport.attempts();
    assert!(attempts.len() >= 3);
    assert!(attempts.iter().all(|path| path == &attempts[0]));
}
