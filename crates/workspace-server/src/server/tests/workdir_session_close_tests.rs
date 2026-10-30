//! Backend registry/current-attachment/WorkerRemove integration with authenticated
//! Runtime HTTP sessions and real Git materializer resource leases. Only Worker
//! execution and the failing/held close transport are replaced. Ordinary library
//! tests use LaunchFixture's isolated SQLite, materializations and loopback Runtime.
use super::ticket_worker_launch_tests::LaunchFixture;
use super::*;
use tokio::sync::{Notify, Semaphore};

#[derive(Clone, Copy)]
enum CloseFault {
    Reject,
    HoldThenReject,
    RejectAfterRuntimeClose,
}

struct CloseTransport {
    upstream: String,
    client: reqwest::Client,
    fault: Mutex<Option<CloseFault>>,
    attempts: Mutex<Vec<String>>,
    runtime_closes: Mutex<Vec<(String, StatusCode)>>,
    entered: Notify,
    release: Semaphore,
}

impl CloseTransport {
    async fn forward(State(state): State<Arc<Self>>, request: Request<Body>) -> Response {
        let is_close = request.method() == Method::DELETE
            && request.uri().path().starts_with("/v1/workdir-sessions/");
        let path = request.uri().path().to_owned();
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
            attempts: Mutex::new(Vec::new()),
            runtime_closes: Mutex::new(Vec::new()),
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
async fn worker_removal_close_failure_retains_attachment_and_retry_releases_runtime_lease() {
    let fixture = Fixture::new().await;
    let api = fixture.api();
    fixture.source.publish_profile("builtin:companion");
    let Json(created) = create_workspace_worker(
        State(api.clone()),
        HeaderMap::new(),
        Json(CreateWorkspaceWorkerRequest {
            runtime_id: fixture.runtime_id.clone(),
            display_name: "session-close removal".into(),
            singleton_key: None,
            profile: Some("builtin:companion".into()),
            ticket_assignment: None,
            initial_submit: Vec::new(),
            workdir_attachments: Vec::new(),
            feature_connections: Default::default(),
            control_operation_id: None,
            execution_grant: None,
        }),
    )
    .await
    .unwrap();
    let worker = RuntimeWorkerRef::new(created.runtime_id, created.worker_id);
    api.runtime
        .stop_worker(
            &worker,
            WorkerLifecycleRequest {
                reason: Some("prepare removal".into()),
                ticket_assignment: None,
            },
        )
        .unwrap();
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
