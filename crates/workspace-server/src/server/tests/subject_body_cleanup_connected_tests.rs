//! Connected stop/restore cleanup regression for the normal Server library target.
//!
//! The entry is authenticated Backend Worker control; Workspace capability signing,
//! Runtime HTTP, FsRuntimeStore, ProfileRuntimeWorkerFactory/Controller, canonical
//! Session restore, Git materialization and RemoteWorkdir sessions are production
//! implementations. Only the external model is scripted (no turn is submitted),
//! and a byte-preserving loopback proxy gates the first stop dispatch and observes
//! the actual Runtime ACK. Neither execution nor restore state is substituted.
//!
//! The test owns the existing Worker session fence after stop's resource snapshot
//! and before its ACK. While ACKed cleanup cannot reacquire that fence, it restores
//! the same ordinary Worker through signed Runtime HTTP, opens real provider
//! sessions/commands, and rebinds the same alias/Workdir through the SQLite lifetime
//! API. Registry insertion and rebind are fixture setup under that fence, not a
//! claim about the browser attachment-management endpoint or its authorization.
//! Provider calls still use the normal signed RemoteWorkdir HTTP boundary; there
//! is no unsigned router, fake ACK, synthetic restore result, or Subject-only
//! execution path. Successful Runtime DELETEs and subsequent provider operations
//! distinguish old resource release from survival of the new lifetimes.
//!
//! Run with the `subject_body_cleanup_connected` library filter. The launching
//! shell/normal runner must supply the isolated restore paths required by
//! tests/support/restore_environment.rs; this module never changes process env.
//! This is an in-process connection/race test, not an OS-crash or live-model test.
//! The command fixture requires the normal POSIX shell and `tail` on PATH.

use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::{Notify, Semaphore};

const CLEANUP_TIMEOUT: Duration = Duration::from_secs(45);
const ALIAS: &str = "workdir";

#[derive(Debug)]
struct RuntimeHttpFailure {
    method: Method,
    path: String,
    status: StatusCode,
    code: Option<String>,
    message: Option<String>,
    closed_before_response: bool,
    // Match the shared harness's nested-envelope view without discarding the
    // flat Workdir envelope's actual code/message from our diagnostic evidence.
    harness_message: String,
}

impl RuntimeHttpFailure {
    fn is_expected_creation_probe(&self, workdir: &str) -> bool {
        self.method == Method::GET
            && self.path == format!("/v1/working-directories/{workdir}")
            && self.status == StatusCode::NOT_FOUND
            && self.code.as_deref() == Some("working_directory_not_found")
            && self.message.as_deref()
                == Some("working_directory_not_found: Runtime working directory was not found")
    }

    fn is_expected_closed_session(&self) -> bool {
        self.method == Method::POST
            && self.path.starts_with("/v1/workdir-sessions/")
            && self.path.ends_with("/operations")
            && self.status == StatusCode::NOT_FOUND
            && self.code.as_deref() == Some("not_found")
            && self.message.as_deref() == Some("Workdir session was not found")
            && self.closed_before_response
    }
}

struct StopTransport {
    upstream: String,
    client: reqwest::Client,
    hold_first_stop: AtomicBool,
    dispatched: Notify,
    release: Semaphore,
    acked: Notify,
    acks: Mutex<Vec<runtime_api::WorkerLifecycleAck>>,
    opened: Mutex<Vec<String>>,
    closed: Mutex<Vec<String>>,
    failures: Mutex<Vec<RuntimeHttpFailure>>,
}

impl StopTransport {
    async fn forward(State(state): State<Arc<Self>>, request: Request<Body>) -> Response {
        let method = request.method().clone();
        let path = request.uri().path().to_owned();
        let is_stop = request.method() == Method::POST
            && path.starts_with("/v1/workers/")
            && path.ends_with("/stop");
        if is_stop && state.hold_first_stop.swap(false, Ordering::SeqCst) {
            // stop_impl has already released its pre-dispatch session fence.
            state.dispatched.notify_one();
            state.release.acquire().await.unwrap().forget();
        }
        let is_open = request.method() == Method::POST
            && path.starts_with("/v1/working-directories/")
            && path.ends_with("/sessions");
        let is_close =
            request.method() == Method::DELETE && path.starts_with("/v1/workdir-sessions/");
        let (parts, body) = request.into_parts();
        let body = to_bytes(body, 2 * 1024 * 1024).await.unwrap();
        // Keep method, URI, signed headers and exact body bytes. Runtime's
        // production verifier, rather than this proxy, decides authorization.
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
        if status.is_client_error() || status.is_server_error() {
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            // REST uses {error:{code,message}}, whereas Workdir transport errors
            // are {code,message}. Keep only sanitized diagnostic fields, never
            // authorization headers, request bodies or raw response bodies.
            let error = body.get("error").unwrap_or(&body);
            let closed_before_response = path
                .strip_suffix("/operations")
                .filter(|session_path| session_path.starts_with("/v1/workdir-sessions/"))
                .is_some_and(|session_path| {
                    state
                        .closed
                        .lock()
                        .unwrap()
                        .iter()
                        .any(|closed| closed == session_path)
                });
            state.failures.lock().unwrap().push(RuntimeHttpFailure {
                method,
                path: path.clone(),
                status,
                code: error["code"].as_str().map(str::to_owned),
                message: error["message"].as_str().map(str::to_owned),
                closed_before_response,
                harness_message: body["error"]["message"]
                    .as_str()
                    .unwrap_or("unknown fixture error")
                    .to_owned(),
            });
        }
        if is_stop && status.is_success() {
            let response: runtime_api::WorkerLifecycleResponse =
                serde_json::from_slice(&bytes).unwrap();
            state.acks.lock().unwrap().push(response.ack);
            // This is a real completed Runtime stop, not merely proxy admission.
            state.acked.notify_one();
        }
        if is_open && status.is_success() {
            let response: workdir::http::OpenWorkdirSessionResponse =
                serde_json::from_slice(&bytes).unwrap();
            state
                .opened
                .lock()
                .unwrap()
                .push(response.session_id.as_str().to_owned());
        }
        if is_close && status == StatusCode::NO_CONTENT {
            state.closed.lock().unwrap().push(path);
        }
        let mut response = Response::new(Body::from(bytes));
        *response.status_mut() = status;
        *response.headers_mut() = headers;
        response
    }

    fn closed_ids(&self) -> Vec<String> {
        let mut ids: Vec<_> = self
            .closed
            .lock()
            .unwrap()
            .iter()
            .map(|path| {
                path.strip_prefix("/v1/workdir-sessions/")
                    .unwrap()
                    .to_owned()
            })
            .collect();
        ids.sort();
        ids
    }
}

struct StopProxy {
    transport: Arc<StopTransport>,
    _server: ConnectedRuntimeServer,
}

impl Drop for StopProxy {
    fn drop(&mut self) {
        // A failed assertion must not strand an admitted Host workflow or the
        // proxy's graceful shutdown behind the test-only dispatch barrier.
        self.transport.release.add_permits(1);
    }
}

impl StopProxy {
    async fn install(harness: &ConnectedWorkerHarness) -> Self {
        let api = &harness.api;
        let binding = api
            .store
            .get_workspace_runtime_binding(TEST_WORKSPACE_ID, CONNECTED_RUNTIME)
            .await
            .unwrap()
            .unwrap();
        let transport = Arc::new(StopTransport {
            upstream: binding.base_url.clone(),
            client: reqwest::Client::builder()
                .no_proxy()
                .timeout(CLEANUP_TIMEOUT)
                .build()
                .unwrap(),
            hold_first_stop: AtomicBool::new(true),
            dispatched: Notify::new(),
            release: Semaphore::new(0),
            acked: Notify::new(),
            acks: Mutex::new(Vec::new()),
            opened: Mutex::new(Vec::new()),
            closed: Mutex::new(Vec::new()),
            failures: Mutex::new(Vec::new()),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = ConnectedRuntimeServer::start(
            listener,
            Router::new()
                .fallback(axum::routing::any(StopTransport::forward))
                .with_state(transport.clone()),
        );
        // Retain the real trusted binding and authorization issuer. Only the
        // transport address changes, as in workdir_session_close_tests.
        api.runtime.register_or_replace(
            RemoteWorkerRuntime::new(
                RemoteRuntimeConfig {
                    runtime_id: CONNECTED_RUNTIME.into(),
                    workspace_id: Some(TEST_WORKSPACE_ID.into()),
                    display_name: binding.display_name.clone(),
                    base_url: endpoint,
                    bearer_token: None,
                    workspace_authorization: Some(
                        crate::hosts::WorkspaceRuntimeAuthorization::new(
                            api.store.clone(),
                            api.signing_identities.clone(),
                            harness.backend_url.clone(),
                            binding,
                        ),
                    ),
                    strict_public_egress: false,
                    cached_worker_creation_available: true,
                    cached_os: "linux".into(),
                    cached_arch: "x86_64".into(),
                    cached_status: "active".into(),
                    timeout: CLEANUP_TIMEOUT,
                },
                TEST_WORKSPACE_ID.into(),
                harness.backend_url.clone(),
            )
            .unwrap(),
        );
        Self {
            transport,
            _server: server,
        }
    }

    async fn open(
        &self,
        api: &WorkspaceApi,
        worker: &RuntimeWorkerRef,
        workdir: &str,
    ) -> (WorkdirSessionHandle, String) {
        let before = self.transport.opened.lock().unwrap().len();
        let session = api
            .runtime
            .open_workdir_session(&worker.runtime_id, workdir, Some(&worker.worker_id))
            .await
            .unwrap();
        let opened = self.transport.opened.lock().unwrap();
        assert_eq!(opened.len(), before + 1, "one actual Runtime session open");
        (session, opened[before].clone())
    }
}

// The harness owns servers/temporary roots; this guard also stops an active
// restored Controller on assertion failure before those servers are dropped.
struct StopControllerOnDrop {
    runtime: worker_runtime::Runtime,
    worker: worker_runtime::identity::WorkerRef,
}
impl Drop for StopControllerOnDrop {
    fn drop(&mut self) {
        let _ = self.runtime.stop_worker(
            &self.worker,
            Some("cleanup connected stop/restore fixture".into()),
        );
    }
}

async fn start_held_command(session: &WorkdirSessionHandle) -> CommandHandle {
    let command = session
        .start_command(workdir::CommandRequest {
            // A long-lived resource, not a timer used to infer progress. Session
            // close must cancel and reap it; no external service is contacted.
            command: "printf cleanup-command; exec tail -f /dev/null".into(),
            timeout_secs: 120,
            transport_byte_budget: workdir::MAX_COMMAND_TRANSPORT_BYTES,
            cwd: workdir::WorkdirPath::root(),
            tool_call_id: None,
        })
        .await
        .unwrap();
    assert_eq!(
        session.command_status(command.clone()).await.unwrap(),
        workdir::CommandStatus::Running,
        "fixture command must really be live rather than a failed shell launch"
    );
    command
}

async fn assert_live(session: &WorkdirSessionHandle) {
    session
        .stat(workdir::StatRequest {
            path: workdir::WorkdirPath::new("README.md").unwrap(),
        })
        .await
        .expect("new Runtime session must still serve the real materialized checkout");
}

async fn assert_closed(session: &WorkdirSessionHandle) {
    session
        .stat(workdir::StatRequest {
            path: workdir::WorkdirPath::new("README.md").unwrap(),
        })
        .await
        .expect_err("observed Runtime session must no longer serve filesystem operations");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn stop_ack_cleanup_preserves_real_restored_sessions_and_rebound_connection() {
    crate::restore_test_environment::require_restore_test_environment();
    let (completed, _completion) = tokio::sync::mpsc::channel(4);
    let harness = ConnectedWorkerHarness::start(ConnectedAuthorizationModel { completed }).await;
    let proxy = StopProxy::install(&harness).await;
    let api = &harness.api;
    let creation_probe_index = proxy.transport.failures.lock().unwrap().len();
    let (status, created) = harness
        .human(
            reqwest::Method::POST,
            &format!("/api/w/{TEST_WORKSPACE_ID}/working-directories"),
            json!({"runtime_id":CONNECTED_RUNTIME, "repository_key":"test-repository", "selector":"HEAD", "operation_id":"cleanup-connected-workdir"}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let workdir = created["item"]["working_directory_id"]
        .as_str()
        .unwrap()
        .to_owned();
    {
        // create_workspace_working_directory probes its reserved Runtime ID
        // before materializing it. Restrict this expectation to the sole failure
        // during that completed creation request and to the returned exact ID.
        // Later misses of the same Workdir must not inherit this exemption.
        let failures = proxy.transport.failures.lock().unwrap();
        assert_eq!(
            failures.len(),
            creation_probe_index + 1,
            "creation must produce exactly one initial absence probe: {failures:?}"
        );
        assert!(
            failures[creation_probe_index].is_expected_creation_probe(&workdir),
            "unexpected creation probe response: {failures:?}"
        );
    }
    let (status, created) = harness
        .human(
            reqwest::Method::POST,
            &format!("/api/w/{TEST_WORKSPACE_ID}/workers"),
            json!({
                "runtime_id":CONNECTED_RUNTIME, "display_name":"Connected stop cleanup",
                "profile":"builtin:coder", "initial_submit":[],
                "workdir_attachments":[{"alias":ALIAS,"working_directory_id":workdir}],
                "feature_connections":{}
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let worker = RuntimeWorkerRef::new(
        created["runtime_id"].as_str().unwrap(),
        created["worker_id"].as_str().unwrap(),
    );
    let local_worker = worker_runtime::identity::WorkerRef::try_from(&worker).unwrap();
    let _controller_cleanup = StopControllerOnDrop {
        runtime: harness.runtime.clone(),
        worker: local_worker.clone(),
    };
    assert!(
        api.store
            .require_current_worker_singleton_owner(TEST_WORKSPACE_ID, &worker)
            .unwrap()
            .is_none(),
        "this must remain the ordinary Worker stop/restore path"
    );
    let old_link = api
        .store
        .list_worker_workdir_links(TEST_WORKSPACE_ID, &worker)
        .unwrap()
        .into_iter()
        .find(|link| link.unlinked_at.is_none() && link.alias == ALIAS)
        .unwrap();

    // Retain two independent pre-stop command sources. An attachment replacement
    // normally closes the displaced session itself; do not manufacture an orphan
    // by overwriting one and then demand that stop follow a removed registry row.
    let (old_first_source, old_first_source_id) = proxy.open(api, &worker, &workdir).await;
    let (old_command_source, old_command_id) = proxy.open(api, &worker, &workdir).await;
    let old_first_provider_command = start_held_command(&old_first_source).await;
    let old_provider_command = start_held_command(&old_command_source).await;
    let (old_first_command, old_command) = {
        let _guard = current_worker_session_lock(api, &worker).lock_owned().await;
        let mut registry = api.workdir_sessions.lock().unwrap();
        assert!(!registry.attachments.contains_key(&worker));
        let first = registry.register_command(
            worker.clone(),
            ALIAS.into(),
            old_first_source.clone(),
            old_first_provider_command,
        );
        let second = registry.register_command(
            worker.clone(),
            ALIAS.into(),
            old_command_source.clone(),
            old_provider_command,
        );
        (first, second)
    };
    let stop_path = format!(
        "/api/w/{TEST_WORKSPACE_ID}/runtimes/{}/workers/{}/stop",
        worker.runtime_id, worker.worker_id
    );
    let stop_api = api.clone();
    let owner = harness.owner.clone();
    let first_stop_path = stop_path.clone();
    let stopping = tokio::spawn(async move {
        response(
            &stop_api,
            human_request(
                &owner,
                "POST",
                &first_stop_path,
                Some(json!({"reason":"first real stop before restore"})),
            ),
        )
        .await
    });
    tokio::time::timeout(CLEANUP_TIMEOUT, proxy.transport.dispatched.notified())
        .await
        .expect("Backend stop must reach transport after its resource observation");
    let session_guard = tokio::time::timeout(
        CLEANUP_TIMEOUT,
        current_worker_session_lock(api, &worker).lock_owned(),
    )
    .await
    .expect("stop dispatch must not retain the pre-ACK session fence");
    proxy.transport.release.add_permits(1);
    tokio::time::timeout(CLEANUP_TIMEOUT, proxy.transport.acked.notified())
        .await
        .expect("real Runtime must acknowledge stop while Backend cleanup is fenced");
    {
        let acks = proxy.transport.acks.lock().unwrap();
        assert_eq!(acks.len(), 1);
        assert_eq!(acks[0].status, runtime_api::WorkerStatus::Stopped);
        assert!(
            !acks[0].replayed,
            "must exercise fresh-ACK cleanup, not replay skip"
        );
    }
    assert!(
        !stopping.is_finished(),
        "ACK must not bypass the session fence"
    );
    assert!(proxy.transport.closed_ids().is_empty());
    // Provider sessions are not Controller state: actual Runtime stop alone has
    // not closed the Backend-observed command sources.
    assert_live(&old_first_source).await;
    assert_live(&old_command_source).await;

    let restore_api = api.clone();
    let restore_worker = worker.clone();
    let restored = tokio::time::timeout(
        CLEANUP_TIMEOUT,
        tokio::task::spawn_blocking(move || {
            let stopped = restore_api.runtime.worker(&restore_worker).unwrap();
            assert_eq!(stopped.state, "stopped");
            restore_api
                .runtime
                .restore_worker(
                    &restore_worker,
                    runtime_api::WorkerRestoreRequest {
                        expected_observation_token: stopped.restore_observation_token.unwrap(),
                        request_id: "cleanup-connected-real-restore".into(),
                        preparation: Some(runtime_api::WorkerRestorePreparation::default()),
                    },
                )
                .unwrap()
        }),
    )
    .await
    .expect("real restore must not need the cleanup-owned Worker control lane")
    .unwrap();
    assert_eq!(
        restored.state,
        server_api::WorkerRestoreState::Accepted,
        "{restored:?}"
    );
    assert_eq!(
        harness.runtime.worker_detail(&local_worker).unwrap().status,
        worker_runtime::catalog::WorkerStatus::Idle,
        "the actual Runtime catalog must expose the reconstructed Controller"
    );

    // The fixture owns the production session fence throughout rebind and
    // resource publication. Keep worker/alias/workdir/capabilities identical;
    // only the storage-issued connection lifetime is different.
    let detached = api
        .store
        .detach_worker_workdir_connection(
            TEST_WORKSPACE_ID,
            &worker,
            ALIAS,
            &old_link.connection_id,
            &now_registry_timestamp(),
        )
        .unwrap();
    assert_eq!(detached.connection_id, old_link.connection_id);
    assert!(detached.unlinked_at.is_some());
    let new_link = api.store.attach_worker_workdir(&old_link).unwrap();
    assert_ne!(new_link.connection_id, old_link.connection_id);
    assert_eq!(new_link.worker, old_link.worker);
    assert_eq!(new_link.alias, old_link.alias);
    assert_eq!(new_link.workdir_id, old_link.workdir_id);
    assert_eq!(new_link.capabilities, old_link.capabilities);

    let (new_attachment, new_attachment_id) = proxy.open(api, &worker, &workdir).await;
    let (new_command_source, new_command_id) = proxy.open(api, &worker, &workdir).await;
    let new_provider_command = start_held_command(&new_command_source).await;
    let new_command = {
        let mut registry = api.workdir_sessions.lock().unwrap();
        assert!(
            registry
                .insert_attachment(worker.clone(), new_attachment.clone())
                .is_none()
        );
        registry.register_command(
            worker.clone(),
            ALIAS.into(),
            new_command_source.clone(),
            new_provider_command.clone(),
        )
    };
    let mut record = api
        .store
        .get_workdir_registry(TEST_WORKSPACE_ID, &workdir)
        .unwrap()
        .unwrap();
    record.display_name = Some("new connection projection must survive old stop".into());
    api.store.upsert_workdir_registry(&record).unwrap();
    assert_live(&new_attachment).await;
    assert_eq!(
        new_command_source
            .command_status(new_provider_command.clone())
            .await
            .unwrap(),
        workdir::CommandStatus::Running,
    );
    assert!(!stopping.is_finished());
    drop(session_guard);

    let (status, stopped) = tokio::time::timeout(CLEANUP_TIMEOUT, stopping)
        .await
        .expect("old cleanup must finish after the session fence is released")
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{stopped}");
    assert_eq!(stopped["state"], "accepted", "{stopped}");
    let mut old_ids = vec![old_first_source_id, old_command_id];
    old_ids.sort();
    assert_eq!(
        proxy.transport.closed_ids(),
        old_ids,
        "close only pre-dispatch sessions"
    );
    assert_closed(&old_first_source).await;
    assert_closed(&old_command_source).await;
    assert_live(&new_attachment).await;
    assert_live(&new_command_source).await;
    assert_eq!(
        new_command_source
            .command_status(new_provider_command)
            .await
            .unwrap(),
        workdir::CommandStatus::Running,
        "old cleanup must not cancel a post-restore command"
    );
    {
        let registry = api.workdir_sessions.lock().unwrap();
        assert!(Arc::ptr_eq(
            registry.attachments.get(&worker).unwrap(),
            &new_attachment
        ));
        assert!(
            registry
                .command(&worker, ALIAS, &old_first_command)
                .is_none()
        );
        assert!(registry.command(&worker, ALIAS, &old_command).is_none());
        assert!(Arc::ptr_eq(
            &registry
                .command(&worker, ALIAS, &new_command)
                .unwrap()
                .source,
            &new_command_source,
        ));
    }
    assert_eq!(
        api.store
            .get_workdir_registry(TEST_WORKSPACE_ID, &workdir)
            .unwrap()
            .unwrap()
            .display_name,
        record.display_name,
        "old stop must not project a new connection even for identical worker/alias/workdir"
    );
    assert_eq!(
        api.store
            .list_worker_workdir_links(TEST_WORKSPACE_ID, &worker)
            .unwrap()
            .into_iter()
            .find(|link| link.unlinked_at.is_none() && link.alias == ALIAS)
            .unwrap()
            .connection_id,
        new_link.connection_id,
    );

    // Surviving resources are not leaked: a later explicit, non-replayed stop
    // observes their own lifetimes and closes their actual Runtime sessions.
    let (status, stopped) = harness
        .human(
            reqwest::Method::POST,
            &stop_path,
            json!({"reason":"explicit stop of restored Worker"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{stopped}");
    assert_eq!(stopped["state"], "accepted", "{stopped}");
    {
        let acks = proxy.transport.acks.lock().unwrap();
        assert_eq!(acks.len(), 2);
        assert_eq!(acks[1].status, runtime_api::WorkerStatus::Stopped);
        assert!(!acks[1].replayed);
    }
    old_ids.extend([new_attachment_id, new_command_id]);
    old_ids.sort();
    assert_eq!(
        proxy.transport.closed_ids(),
        old_ids,
        "each actual session closes once"
    );
    assert_closed(&new_attachment).await;
    assert_closed(&new_command_source).await;
    {
        let registry = api.workdir_sessions.lock().unwrap();
        assert!(!registry.attachments.contains_key(&worker));
        assert!(registry.command(&worker, ALIAS, &new_command).is_none());
    }
    assert_ne!(
        api.store
            .get_workdir_registry(TEST_WORKSPACE_ID, &workdir)
            .unwrap()
            .unwrap()
            .display_name,
        record.display_name,
        "later stop must still project its current connection normally"
    );
    let runtime_errors = harness.runtime_errors.lock().unwrap().clone();
    let failures = proxy.transport.failures.lock().unwrap();
    let unexpected: Vec<_> = failures
        .iter()
        .enumerate()
        .filter(|(index, failure)| {
            *index != creation_probe_index && !failure.is_expected_closed_session()
        })
        .map(|(_, failure)| failure)
        .collect();
    assert!(
        unexpected.is_empty(),
        "unexpected Runtime HTTP errors: {unexpected:?}; runtime_errors={runtime_errors:?}; all HTTP failures={failures:?}"
    );
    // Do not silently lose failures from any Runtime request that bypassed our
    // proxy. Reconcile even the shared harness's lossy flat-envelope messages.
    // Apart from the single creation probe validated at its request boundary,
    // a NotFound is expected only for operations on a session whose DELETE was
    // already confirmed; a 401/403 or any other NotFound remains a failure.
    let mut captured = runtime_errors.clone();
    let mut classified: Vec<_> = failures
        .iter()
        .map(|failure| failure.harness_message.clone())
        .collect();
    captured.sort();
    classified.sort();
    assert_eq!(
        captured, classified,
        "unclassified Runtime errors: runtime_errors={runtime_errors:?}; HTTP failures={failures:?}"
    );
}
