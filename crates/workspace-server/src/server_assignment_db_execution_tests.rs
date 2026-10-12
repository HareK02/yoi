//! Real SQLite and generated HTTP boundaries for continuing assignment input.
//! Every fixture owns its finite DB/Host lanes and all observer userdata.
use super::tests::DeterministicExecutionBackend;
use super::*;
use crate::SqliteWorkspaceStore;
use crate::db_executor::{DbExecutor, DbExecutorLimits, DbOperationLifetime};
use rusqlite::{Connection, ffi};
use std::{
    ffi::c_void,
    future::Future,
    num::NonZeroUsize,
    sync::{
        atomic::{AtomicBool, AtomicUsize},
        mpsc,
    },
    thread::JoinHandle,
    time::Duration as StdDuration,
};
use tokio::sync::oneshot;
use worker_runtime::auth::RuntimeIdentityMaterial;

// The cfg(test) direct Orchestrator adapter seeds its binding for this existing
// fixture Workspace identity. Databases and executors remain per-fixture owners.
const WORKSPACE: &str = "0192f0e8-4d84-7d6e-a000-000000000001";
const AT: &str = "2026-01-01T00:00:00Z";
const WATCHDOG: StdDuration = StdDuration::from_secs(10);
const OBSERVE_LIMIT: StdDuration = StdDuration::from_secs(5);

async fn observed<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(OBSERVE_LIMIT, future)
        .await
        .expect("assignment request did not reach the expected SQLite/publication boundary")
}
fn lane(workers: usize, queued: usize) -> DbExecutor {
    DbExecutor::new(DbExecutorLimits {
        workers: NonZeroUsize::new(workers).unwrap(),
        queued: NonZeroUsize::new(queued).unwrap(),
    })
    .unwrap()
}

struct Fixture {
    api: WorkspaceApi,
    store: Arc<SqliteWorkspaceStore>,
    executor: DbExecutor,
    execution: Arc<DeterministicExecutionBackend>,
    ticket: ticket::TicketRef,
    _dir: tempfile::TempDir,
}
impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let executor = lane(1, 32);
        let config = ServerConfig {
            workspace_id: WORKSPACE.into(),
            workspace_display_name: "Assignment DB fixture".into(),
            workspace_created_at: AT.into(),
            workspace_execution_root: dir.path().into(),
            database_path: dir.path().join("server.db"),
            data_root: dir.path().join("data"),
            embedded_runtime_store_root: dir.path().join("runtime"),
            static_assets_dir: None,
            auth: AuthConfig::Passkey {
                rp_id: "localhost".into(),
                origin: "http://localhost:5173".into(),
                public_base_url: "http://localhost:5173".into(),
                cookie_name: "assignment_db_session".into(),
            },
            max_records: 100,
            repositories: Vec::new(),
            runtime_event_sources: Vec::new(),
            remote_runtime_sources: Vec::new(),
            backend_base_url: Some("assignment-db-server".into()),
        };
        let store = Arc::new(
            SqliteWorkspaceStore::open_for_test(&config.database_path)
                .unwrap()
                .with_db_executor(executor.clone()),
        );
        seed_test_registered_workspace(store.as_ref(), &config)
            .await
            .unwrap();
        store
            .upsert_user(&UserRecord {
                user_id: "fixture-user".into(),
                account_id: format!("account-{WORKSPACE}"),
                handle: "fixture-user".into(),
                display_name: "Fixture user".into(),
                created_at: AT.into(),
                updated_at: AT.into(),
            })
            .unwrap();
        store
            .create_browser_session(&crate::store::BrowserSessionRecord {
                session_id: "fixture-session".into(),
                token_hash: crate::auth::token_hash("fixture-browser-token"),
                user_id: "fixture-user".into(),
                created_at: AT.into(),
                expires_at: "2999-01-01T00:00:00Z".into(),
                revoked_at: None,
            })
            .unwrap();
        store
            .create_api_token(&ApiTokenRecord {
                token_hash: crate::auth::token_hash("fixture-api-token"),
                token_id: "fixture-token".into(),
                user_id: "fixture-user".into(),
                label: "assignment fixture".into(),
                created_at: AT.into(),
                expires_at: None,
                last_used_at: None,
                revoked_at: None,
            })
            .unwrap();
        let execution = Arc::new(DeterministicExecutionBackend::default());
        // Explicit, fixture-owned finite lanes; never the production singleton.
        let host_executors = HostExecutors::default();
        host_executors
            .host
            .set(Some(lane(2, 16)))
            .unwrap_or_else(|_| panic!("Host owner initialized twice"));
        host_executors
            .cleanup
            .set(Some(lane(2, 16)))
            .unwrap_or_else(|_| panic!("cleanup owner initialized twice"));
        let api = WorkspaceApi::new_with_execution_backend_and_broker(
            config,
            store.clone(),
            execution.clone(),
            BackendResourceBroker::default(),
            host_executors,
            None,
        )
        .await
        .unwrap();
        let mut input = ticket::NewTicket::new("Continuing assignment input");
        input.workflow_state = Some(TicketState::Open);
        let ticket = browser_ticket_backend(&api).unwrap().create(input).unwrap();
        store
            .set_current_ticket_role_assignment(
                &TicketRoleAssignmentRecord {
                    workspace_id: WORKSPACE.into(),
                    ticket_id: ticket.id.clone(),
                    assignment_id: "fixture-assignment".into(),
                    role: TicketAssignmentRole::Orchestrator,
                    principal: TicketAssignmentPrincipal::WorkspaceAgent {
                        agent_key: "workspace-orchestrator".into(),
                    },
                    assigned_by: "fixture-user".into(),
                    assigned_at: AT.into(),
                },
                None,
                "fixture-assign-event",
                "fixture-assign-operation",
                false,
            )
            .unwrap();
        executor.drain().await.unwrap();
        Self {
            api,
            store,
            executor,
            execution,
            ticket,
            _dir: dir,
        }
    }
    fn path(&self, browser: bool, suffix: &str) -> String {
        let prefix = if browser { "api/w" } else { "workspaces" };
        // Exercise reference resolution, not merely an internal row ID.
        format!(
            "/{prefix}/{WORKSPACE}/tickets/{}/{suffix}",
            self.ticket.resource_key.as_deref().unwrap()
        )
    }
    fn request(method: &str, path: &str, body: serde_json::Value) -> Request {
        Request::builder()
            .method(method)
            .uri(path)
            .header(CONTENT_TYPE, "application/json")
            .header("cookie", "assignment_db_session=fixture-browser-token")
            .header(ORIGIN, "http://localhost:5173")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap()
    }
    fn account_request(method: &str, path: &str, body: serde_json::Value) -> Request {
        Request::builder()
            .method(method)
            .uri(path)
            .header(CONTENT_TYPE, "application/json")
            .header("authorization", "Bearer fixture-api-token")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap()
    }
    fn actor() -> RequestActor {
        RequestActor {
            user_id: "fixture-user".into(),
            account_id: format!("account-{WORKSPACE}"),
            handle: "fixture-user".into(),
            display_name: "Fixture user".into(),
            auth_method: ActorAuthMethod::BrowserSession,
        }
    }
    // Contention tests isolate handler SQL from authentication's earlier SQL.
    // Full signed, token and browser authentication is tested separately below.
    fn inner_router(&self) -> Router {
        build_inner_router(self.api.clone()).layer(Extension(Self::actor()))
    }
    fn answer(&self, key: &str) -> serde_json::Value {
        let current = self
            .store
            .get_ticket_assignment_tracking(WORKSPACE, &self.ticket.id, "fixture-assignment")
            .unwrap()
            .unwrap();
        serde_json::json!({
            "assignment_id": current.assignment_id,
            "expected_last_event_id": current.last_event_id.unwrap(),
            "operation_key": key, "body": "Explicit user answer: preserve this exact body"
        })
    }
    async fn start_orchestrator(&self) -> RuntimeWorkerRef {
        // Live Runtime binding is required by the real delivery acknowledger.
        let identity = (*EMBEDDED_RUNTIME_REQUEST_IDENTITY).clone();
        self.store
            .upsert_workspace_runtime_binding(
                WorkspaceRuntimeBinding {
                    workspace_id: WORKSPACE.into(),
                    runtime_id: EMBEDDED_WORKER_RUNTIME_ID.into(),
                    display_name: "Embedded fixture Runtime".into(),
                    base_url: "in-process://embedded".into(),
                    public_key: identity.public_key,
                    public_key_fingerprint: String::new(),
                    binding_id: "fixture-binding".into(),
                    authentication_mode: StoredRuntimeAuthenticationMode::LegacyServerIssuer,
                    created_at: AT.into(),
                    updated_at: AT.into(),
                    revoked_at: None,
                },
                false,
            )
            .unwrap();
        let started = scoped_start_workspace_orchestrator(
            State(self.api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: WORKSPACE.into(),
            }),
        )
        .await
        .unwrap()
        .0;
        let worker = started.worker.unwrap();
        observed(self.api.host_executors.host().unwrap().drain())
            .await
            .unwrap();
        observed(self.executor.drain()).await.unwrap();
        self.execution.take_inputs();
        self.execution.take_input_request_ids();
        RuntimeWorkerRef::new(worker.runtime_id, worker.worker_id)
    }
    async fn drain(&self) {
        observed(self.api.host_executors.host().unwrap().drain())
            .await
            .unwrap();
        observed(self.executor.drain()).await.unwrap();
    }
    async fn finish(&self) {
        // Keep the original Tokio driver alive through actual Host/cleanup work,
        // then stop projection producers before the final DB barrier.
        observed(self.api.host_executors.host().unwrap().drain())
            .await
            .unwrap();
        observed(self.api.host_executors.cleanup().unwrap().drain())
            .await
            .unwrap();
        observed(self.api.worker_projection.shutdown_async()).await;
        observed(self.executor.drain()).await.unwrap();
    }
    fn answer_count(&self) -> i64 {
        Connection::open(&self.api.config.database_path).unwrap().query_row(
            "SELECT count(*) FROM typed_ticket_events WHERE workspace_id=?1 AND ticket_id=?2 AND kind='assignment_answer'",
            rusqlite::params![WORKSPACE, self.ticket.id], |r| r.get(0)).unwrap()
    }
    fn assert_saved_answer(&self, key: &str, answer: &serde_json::Value) -> serde_json::Value {
        let conn = Connection::open(&self.api.config.database_path).unwrap();
        let (input, source, result): (String, String, String) = conn.query_row(
            "SELECT input_json,source_context_json,result_json FROM typed_ticket_assignment_tracking_operations WHERE workspace_id=?1 AND operation_key=?2",
            rusqlite::params![WORKSPACE,key], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&input).unwrap()["answer"],
            *answer
        );
        let source: serde_json::Value = serde_json::from_str(&source).unwrap();
        assert_eq!(source["source"]["kind"], "account", "{source}");
        assert_eq!(source["source"]["user_id"], "fixture-user", "{source}");
        let receipt: serde_json::Value = serde_json::from_str(&result).unwrap();
        let (body, author, audit): (String,String,String) = conn.query_row(
            "SELECT e.body,e.author,a.value FROM typed_ticket_events e JOIN typed_ticket_event_attributes a ON a.workspace_id=e.workspace_id AND a.ticket_id=e.ticket_id AND a.event_index=e.event_index AND a.key='authenticated_source' WHERE e.workspace_id=?1 AND e.ticket_id=?2 AND e.event_index=?3",
            rusqlite::params![WORKSPACE,self.ticket.id,receipt["event_index"].as_u64().unwrap()],
            |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        assert_eq!(body, answer["body"].as_str().unwrap());
        assert_eq!(author, source["actor"].as_str().unwrap());
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&audit).unwrap(),
            source
        );
        receipt
    }
}

// SQLite owns the actual contention. Watchdog is a panic/hang escape, not test timing.
struct WriteLock {
    release: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
    released: Arc<AtomicBool>,
}
impl WriteLock {
    fn acquire(path: PathBuf) -> Self {
        let (ready, held) = mpsc::sync_channel(1);
        let (release, permit) = mpsc::channel();
        let released = Arc::new(AtomicBool::new(false));
        let flag = released.clone();
        let thread = std::thread::spawn(move || {
            let conn = Connection::open(path).unwrap();
            conn.execute_batch("BEGIN IMMEDIATE").unwrap();
            ready.send(()).unwrap();
            let _ = permit.recv_timeout(WATCHDOG);
            conn.execute_batch("ROLLBACK").unwrap();
            flag.store(true, Ordering::SeqCst);
        });
        held.recv_timeout(WATCHDOG)
            .expect("SQLite blocker did not acquire ownership");
        Self {
            release: Some(release),
            thread: Some(thread),
            released,
        }
    }
    fn release(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}
impl Drop for WriteLock {
    fn drop(&mut self) {
        self.release();
    }
}

struct BusyGate {
    entered: Option<oneshot::Sender<()>>,
    resume: mpsc::Receiver<()>,
}
struct BusyObservation {
    store: Arc<SqliteWorkspaceStore>,
    gate: Box<BusyGate>,
    resume: Option<mpsc::Sender<()>>,
}
impl BusyObservation {
    fn install(store: Arc<SqliteWorkspaceStore>) -> (Self, oneshot::Receiver<()>) {
        unsafe extern "C" fn busy(context: *mut c_void, _: i32) -> i32 {
            // SAFETY: boxed fixture userdata stays alive until mutex-serialized unregister.
            let gate = unsafe { &mut *context.cast::<BusyGate>() };
            if let Some(entered) = gate.entered.take() {
                let _ = entered.send(());
                return i32::from(gate.resume.recv_timeout(WATCHDOG).is_ok());
            }
            0
        }
        let (entered, waiting) = oneshot::channel();
        let (resume, permit) = mpsc::channel();
        let mut gate = Box::new(BusyGate {
            entered: Some(entered),
            resume: permit,
        });
        store
            .with_conn(|conn| {
                // SAFETY: connection owns handle; Box gives userdata a stable address.
                assert_eq!(
                    unsafe {
                        ffi::sqlite3_busy_handler(
                            conn.handle(),
                            Some(busy),
                            (&mut *gate as *mut BusyGate).cast(),
                        )
                    },
                    ffi::SQLITE_OK
                );
                Ok(())
            })
            .unwrap();
        (
            Self {
                store,
                gate,
                resume: Some(resume),
            },
            waiting,
        )
    }
    fn resume(&mut self) {
        if let Some(resume) = self.resume.take() {
            let _ = resume.send(());
        }
    }
}
impl Drop for BusyObservation {
    fn drop(&mut self) {
        self.resume();
        self.store
            .with_conn(|conn| {
                // SAFETY: unregister before the owned Box is freed.
                unsafe {
                    ffi::sqlite3_busy_handler(conn.handle(), None, std::ptr::null_mut());
                }
                Ok(())
            })
            .unwrap();
        let _ = &self.gate;
    }
}
async fn runtime_progress_while_locked(lock: &WriteLock) {
    assert_eq!(
        observed(tokio::spawn(async { "independent control" }))
            .await
            .unwrap(),
        "independent control"
    );
    // This timer IS the responsiveness contract; it does not guess SQL acceptance.
    observed(tokio::time::sleep(StdDuration::from_millis(1))).await;
    assert!(
        !lock.released.load(Ordering::SeqCst),
        "watchdog released lock before control/timer progressed"
    );
}
async fn response_json(response: Response, expected: StatusCode) -> serde_json::Value {
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(status, expected, "{}", String::from_utf8_lossy(&bytes));
    if bytes.is_empty() {
        // Authentication middleware deliberately returns status-only denials;
        // generated handler/domain successes and errors must still be JSON.
        assert!(
            matches!(expected, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN),
            "unexpected empty HTTP body for {expected}"
        );
        return serde_json::Value::Null;
    }
    serde_json::from_slice(&bytes).expect("typed JSON HTTP response")
}

// Request lifecycle source and actual retained leases, not waiter liveness.
#[derive(Default)]
struct Lifetimes {
    live: Arc<AtomicUsize>,
    admissions: AtomicUsize,
}
struct Lease(Arc<AtomicUsize>);
impl Drop for Lease {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
impl DbOperationLifetime for Lifetimes {
    fn retain(&self) -> Box<dyn Send> {
        self.admissions.fetch_add(1, Ordering::SeqCst);
        self.live.fetch_add(1, Ordering::SeqCst);
        Box::new(Lease(self.live.clone()))
    }
}

// Finite admitted blockers release through RAII even if an HTTP assertion fails.
struct Saturation {
    releases: Vec<mpsc::Sender<()>>,
}
impl Saturation {
    fn fill(executor: &DbExecutor, workers: usize, queued: usize) -> Self {
        let mut releases = Vec::new();
        for _ in 0..workers {
            let (started, running) = mpsc::channel();
            let (release, permit) = mpsc::channel();
            executor
                .dispatch(move || {
                    started.send(()).unwrap();
                    let _ = permit.recv_timeout(WATCHDOG);
                })
                .unwrap();
            running
                .recv_timeout(WATCHDOG)
                .expect("admitted blocker did not start");
            releases.push(release);
        }
        for _ in 0..queued {
            let (release, permit) = mpsc::channel();
            executor
                .dispatch(move || {
                    let _ = permit.recv_timeout(WATCHDOG);
                })
                .unwrap();
            releases.push(release);
        }
        Self { releases }
    }
}
impl Drop for Saturation {
    fn drop(&mut self) {
        self.releases.clear();
    }
}

#[tokio::test(flavor = "current_thread")]
async fn assignment_answer_sqlite_writer_wait_keeps_timer_control_and_receipt_atomic() {
    for browser in [false, true] {
        let fixture = Fixture::new().await;
        let answer = fixture.answer("busy-answer");
        let mut lock = WriteLock::acquire(fixture.api.config.database_path.clone());
        let (mut busy, waiting) = BusyObservation::install(fixture.store.clone());
        let response = tokio::spawn(fixture.inner_router().oneshot(Fixture::request(
            "POST",
            &fixture.path(browser, "assignment-answer"),
            answer.clone(),
        )));
        observed(waiting).await.unwrap(); // actual SQLite busy callback, not first poll
        runtime_progress_while_locked(&lock).await;
        assert!(
            !response.is_finished(),
            "answer responded before write ownership"
        );
        assert_eq!(
            fixture.answer_count(),
            0,
            "uncommitted answer became visible"
        );
        lock.release();
        busy.resume();
        let receipt =
            response_json(observed(response).await.unwrap().unwrap(), StatusCode::OK).await;
        fixture.drain().await;
        assert_eq!(
            fixture.answer_count(),
            1,
            "one call commits once; no mutation retries"
        );
        assert_eq!(fixture.assert_saved_answer("busy-answer", &answer), receipt);
        fixture.finish().await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn saturated_db_assignment_routes_return_503_without_accepting_answer() {
    let fixture = Fixture::new().await;
    let answer = fixture.answer("unaccepted-answer");
    let full = Saturation::fill(&fixture.executor, 1, 32);
    let lifetime = Arc::new(Lifetimes::default());
    for browser in [false, true] {
        for (method, suffix, input) in [
            (
                "GET",
                "assignment-tracking/assignments",
                serde_json::Value::Null,
            ),
            (
                "GET",
                "assignment-diagnostics?assignment_id=fixture-assignment",
                serde_json::Value::Null,
            ),
            ("POST", "assignment-answer", answer.clone()),
        ] {
            let response = observed(crate::db_executor::with_operation_lifetime(
                lifetime.clone(),
                fixture.inner_router().oneshot(Fixture::request(
                    method,
                    &fixture.path(browser, suffix),
                    input,
                )),
            ))
            .await
            .unwrap();
            let body = response_json(response, StatusCode::SERVICE_UNAVAILABLE).await;
            assert_eq!(
                body["diagnostics"][0]["code"], "db_capacity_exceeded",
                "{body}"
            );
        }
    }
    let response = observed(fixture.inner_router().oneshot(Fixture::request(
        "POST",
        &format!("/api/w/{WORKSPACE}/tickets/query"),
        serde_json::json!({"handling_status":"waiting"}),
    )))
    .await
    .unwrap();
    let body = response_json(response, StatusCode::SERVICE_UNAVAILABLE).await;
    assert_eq!(
        body["diagnostics"][0]["code"], "db_capacity_exceeded",
        "{body}"
    );
    assert_eq!(lifetime.live.load(Ordering::SeqCst), 0);
    drop(full);
    fixture.drain().await;
    assert_eq!(fixture.answer_count(), 0);
    assert_eq!(Connection::open(&fixture.api.config.database_path).unwrap().query_row(
        "SELECT count(*) FROM typed_ticket_assignment_tracking_operations WHERE operation_key='unaccepted-answer'", [], |r| r.get::<_,i64>(0)).unwrap(), 0);
    fixture.finish().await;
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_answer_sql_job_keeps_audit_receipt_publication_and_inherited_lifecycle() {
    let fixture = Fixture::new().await;
    let orchestrator = fixture.start_orchestrator().await;
    let answer = fixture.answer("cancelled-answer");
    let (published, publication) = oneshot::channel();
    let (release, permit) = mpsc::channel();
    fixture.execution.before_input_returns(move |_| {
        let _ = published.send(());
        permit
            .recv_timeout(WATCHDOG)
            .expect("publication was not released");
    });
    let mut lock = WriteLock::acquire(fixture.api.config.database_path.clone());
    let (mut busy, waiting) = BusyObservation::install(fixture.store.clone());
    let lifetime = Arc::new(Lifetimes::default());
    let response = tokio::spawn(crate::db_executor::with_operation_lifetime(
        lifetime.clone(),
        fixture.inner_router().oneshot(Fixture::request(
            "POST",
            &fixture.path(true, "assignment-answer"),
            answer.clone(),
        )),
    ));
    observed(waiting).await.unwrap();
    assert!(lifetime.live.load(Ordering::SeqCst) > 0);
    response.abort();
    assert!(observed(response).await.unwrap_err().is_cancelled());
    assert!(
        lifetime.live.load(Ordering::SeqCst) > 0,
        "waiter cancellation released accepted SQL lifecycle"
    );
    // Queue capacity remains bounded after waiter loss.
    let mut queued_releases: Vec<mpsc::Sender<()>> = Vec::new();
    for _ in 0..32 {
        let (queued_release, queued_permit) = mpsc::channel();
        fixture
            .executor
            .dispatch(move || {
                let _ = queued_permit.recv_timeout(WATCHDOG);
            })
            .unwrap();
        queued_releases.push(queued_release);
    }
    assert!(matches!(
        fixture.executor.dispatch(|| ()),
        Err(Error::DbCapacityExceeded)
    ));
    lock.release();
    busy.resume();
    drop(queued_releases);
    observed(publication).await.unwrap(); // real Runtime dispatch hook reached after SQL commit
    observed(fixture.executor.drain()).await.unwrap();
    assert_eq!(fixture.answer_count(), 1);
    fixture.assert_saved_answer("cancelled-answer", &answer);
    // Runtime wait belongs to Host: the sole DB lane remains available for callbacks.
    assert_eq!(
        observed(
            fixture
                .executor
                .run(|| Ok::<_, Error>("DB callback progressed"))
        )
        .await
        .unwrap(),
        "DB callback progressed"
    );
    assert!(
        lifetime.live.load(Ordering::SeqCst) > 0,
        "Host publication did not inherit admitted lifecycle"
    );
    assert!(
        lifetime.admissions.load(Ordering::SeqCst) >= 2,
        "no inherited Host admission"
    );
    release.send(()).unwrap();
    fixture.drain().await;
    assert_eq!(
        lifetime.live.load(Ordering::SeqCst),
        0,
        "actual cleanup leaked lifecycle"
    );
    let inputs = fixture.execution.take_inputs();
    assert_eq!(
        inputs.len(),
        1,
        "cancelled HTTP answer must still publish exactly once"
    );
    assert_eq!(inputs[0].0.worker_id.to_string(), orchestrator.worker_id);
    assert!(
        inputs[0]
            .1
            .contains(fixture.ticket.resource_key.as_deref().unwrap())
    );
    assert!(inputs[0].1.contains("input:"));
    assert_eq!(fixture.execution.take_input_request_ids().len(), 1);
    fixture.finish().await;
}

#[tokio::test(flavor = "current_thread")]
async fn assignment_answer_http_success_waits_for_publication_without_holding_db_worker() {
    let fixture = Fixture::new().await;
    fixture.start_orchestrator().await;
    let (published, publication) = oneshot::channel();
    let (release, permit) = mpsc::channel();
    fixture.execution.before_input_returns(move |_| {
        let _ = published.send(());
        permit
            .recv_timeout(WATCHDOG)
            .expect("publication was not released");
    });
    let answer = fixture.answer("await-publication");
    let response = tokio::spawn(build_router(fixture.api.clone()).oneshot(Fixture::request(
        "POST",
        &fixture.path(true, "assignment-answer"),
        answer.clone(),
    )));
    observed(publication).await.unwrap();
    assert!(
        !response.is_finished(),
        "normal HTTP waiter returned before publication completion"
    );
    assert_eq!(fixture.answer_count(), 1, "publication must follow commit");
    assert_eq!(
        observed(fixture.executor.run(|| Ok::<_, Error>(42)))
            .await
            .unwrap(),
        42,
        "Runtime dispatch blocked the sole DB worker"
    );
    release.send(()).unwrap();
    let receipt = response_json(observed(response).await.unwrap().unwrap(), StatusCode::OK).await;
    fixture.drain().await;
    assert_eq!(
        fixture.assert_saved_answer("await-publication", &answer),
        receipt
    );
    assert_eq!(fixture.execution.take_inputs().len(), 1);
    fixture.finish().await;
}

// WAL readers do not wait for an IMMEDIATE writer. For read-only callers, hold
// the actual Store connection mutex instead: it is part of the same whole DB
// job as reference resolution and the page/query read (not a mock executor job).
struct ConnectionLock {
    release: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
    released: Arc<AtomicBool>,
}
impl ConnectionLock {
    fn acquire(store: Arc<SqliteWorkspaceStore>) -> Self {
        let (ready, held) = mpsc::sync_channel(1);
        let (release, permit) = mpsc::channel();
        let released = Arc::new(AtomicBool::new(false));
        let flag = released.clone();
        let thread = std::thread::spawn(move || {
            store
                .with_conn(|conn| {
                    assert_eq!(conn.query_row("SELECT 1", [], |r| r.get::<_, i64>(0))?, 1);
                    ready.send(()).unwrap();
                    let _ = permit.recv_timeout(WATCHDOG);
                    Ok(())
                })
                .unwrap();
            flag.store(true, Ordering::SeqCst);
        });
        held.recv_timeout(WATCHDOG)
            .expect("Store connection owner did not start");
        Self {
            release: Some(release),
            thread: Some(thread),
            released,
        }
    }
    fn release(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}
impl Drop for ConnectionLock {
    fn drop(&mut self) {
        self.release();
    }
}
struct Admission(Mutex<Option<oneshot::Sender<()>>>);
impl DbOperationLifetime for Admission {
    fn retain(&self) -> Box<dyn Send> {
        if let Some(entered) = self.0.lock().unwrap().take() {
            let _ = entered.send(());
        }
        Box::new(())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn assignment_read_pages_and_reference_resolution_wait_off_executor_on_store_connection() {
    let fixture = Fixture::new().await;
    for browser in [false, true] {
        for suffix in [
            "assignment-tracking/assignments?limit=1",
            "assignment-diagnostics?assignment_id=fixture-assignment&limit=1",
        ] {
            let mut lock = ConnectionLock::acquire(fixture.store.clone());
            let (entered, admission) = oneshot::channel();
            let response = tokio::spawn(crate::db_executor::with_operation_lifetime(
                Arc::new(Admission(Mutex::new(Some(entered)))),
                fixture.inner_router().oneshot(Fixture::request(
                    "GET",
                    &fixture.path(browser, suffix),
                    serde_json::Value::Null,
                )),
            ));
            observed(admission).await.unwrap();
            assert_eq!(observed(tokio::spawn(async { 42 })).await.unwrap(), 42);
            observed(tokio::time::sleep(StdDuration::from_millis(1))).await;
            assert!(
                !lock.released.load(Ordering::SeqCst),
                "synchronous pre-call reference read stalled Tokio until watchdog"
            );
            assert!(
                !response.is_finished(),
                "read bypassed held Store connection"
            );
            lock.release();
            let body =
                response_json(observed(response).await.unwrap().unwrap(), StatusCode::OK).await;
            if suffix.starts_with("assignment-tracking") {
                assert_eq!(
                    body["items"][0]["assignment_id"], "fixture-assignment",
                    "{body}"
                );
                assert!(body["next_assignment_id"].is_null(), "{body}");
            } else {
                assert_eq!(body["assignment_id"], "fixture-assignment", "{body}");
                assert_eq!(body["input_origin_known"], true, "{body}");
                assert!(body.get("payload_json").is_none(), "{body}");
            }
            fixture.drain().await;
        }
    }
    fixture.finish().await;
}

#[tokio::test(flavor = "current_thread")]
async fn typed_ticket_query_offloads_principal_handling_and_closure_filters_before_page_limit() {
    let fixture = Fixture::new().await;
    let backend = browser_ticket_backend(&fixture.api).unwrap();
    let closed = backend
        .create(ticket::NewTicket::new("Closed unrelated case"))
        .unwrap();
    let before = backend.show(closed.id.clone().into()).unwrap();
    backend
        .update_case(
            &closed.id,
            ticket::TicketCaseUpdate {
                expected_content_digest: ticket::ticket_content_digest(&before),
                expected_state: before.meta.workflow_state,
                expected_closure_operation_key: before.closure_operation_key.clone(),
                operation_key: "query-close".into(),
                action: ticket::TicketCaseAction::Close {
                    reason: ticket::TicketClosureReason::Completed,
                },
                explanation: "Confirmed result".into(),
                references: vec![],
                author: None,
            },
        )
        .unwrap();
    for n in 0..3 {
        backend
            .create(ticket::NewTicket::new(format!("Newer unassigned {n}")))
            .unwrap();
    }
    let path = format!("/api/w/{WORKSPACE}/tickets/query");
    for (input, expected) in [
        (
            serde_json::json!({"assigned_principal":{"kind":"workspace_agent","agent_key":"workspace-orchestrator"},"handling_status":"waiting","limit":1}),
            Some(fixture.ticket.id.as_str()),
        ),
        (
            serde_json::json!({"assigned_principal":{"kind":"workspace_agent","agent_key":"workspace-orchestrator"},"handling_status":"done","limit":1}),
            None,
        ),
        (
            serde_json::json!({"assigned_principal":{"kind":"worker","runtime_id":"unavailable","worker_id":"unavailable"},"limit":1}),
            None,
        ),
        (
            serde_json::json!({"closure_reason":"completed","limit":1}),
            Some(closed.id.as_str()),
        ),
        (
            serde_json::json!({"closure_reason":"completed","handling_status":"waiting","limit":1}),
            None,
        ),
    ] {
        // Candidate SQL has its own Authority connection, including empty-result
        // queries that never reach API Store enrichment.
        let mut lock = ConnectionLock::acquire(Arc::new(
            fixture.api.authority.candidate_query_store_for_test(),
        ));
        let (entered, admission) = oneshot::channel();
        let response = tokio::spawn(crate::db_executor::with_operation_lifetime(
            Arc::new(Admission(Mutex::new(Some(entered)))),
            fixture
                .inner_router()
                .oneshot(Fixture::request("POST", &path, input)),
        ));
        observed(admission).await.unwrap();
        assert_eq!(
            observed(tokio::spawn(async { "query control" }))
                .await
                .unwrap(),
            "query control"
        );
        observed(tokio::time::sleep(StdDuration::from_millis(1))).await;
        assert!(
            !lock.released.load(Ordering::SeqCst),
            "query blocked Tokio instead of its admitted DB worker"
        );
        if response.is_finished() {
            lock.release();
            let response = observed(response).await.unwrap().unwrap();
            let status = response.status();
            let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap();
            panic!(
                "query returned before Store connection release: status={status}, body={}",
                String::from_utf8_lossy(&body)
            );
        }
        lock.release();
        let body = response_json(observed(response).await.unwrap().unwrap(), StatusCode::OK).await;
        let items = body["items"].as_array().unwrap();
        if let Some(id) = expected {
            assert_eq!(items.len(), 1, "{body}");
            assert_eq!(items[0]["id"], id, "{body}");
        } else {
            assert!(items.is_empty(), "{body}");
        }
        fixture.drain().await;
    }
    // Also cross the complete browser and Account authentication boundaries.
    for request in [
        Fixture::request(
            "POST",
            &path,
            serde_json::json!({"handling_status":"waiting"}),
        ),
        Fixture::account_request(
            "POST",
            &path,
            serde_json::json!({"closure_reason":"completed"}),
        ),
    ] {
        let body = response_json(
            observed(build_router(fixture.api.clone()).oneshot(request))
                .await
                .unwrap(),
            StatusCode::OK,
        )
        .await;
        assert_eq!(body["items"].as_array().unwrap().len(), 1, "{body}");
    }
    fixture.finish().await;
}

#[tokio::test(flavor = "current_thread")]
async fn browser_and_account_assignment_routes_preserve_receipt_replay_and_map_cas_cursor_errors() {
    let fixture = Fixture::new().await;
    let app = build_router(fixture.api.clone());
    let answer = fixture.answer("http-replay");
    let mut saved = None;
    for browser in [false, true] {
        for (suffix, input) in [
            (
                "assignment-tracking/assignments?limit=1",
                serde_json::Value::Null,
            ),
            (
                "assignment-diagnostics?assignment_id=fixture-assignment&limit=1",
                serde_json::Value::Null,
            ),
        ] {
            let path = fixture.path(browser, suffix);
            let body = response_json(
                observed(app.clone().oneshot(Fixture::request("GET", &path, input)))
                    .await
                    .unwrap(),
                StatusCode::OK,
            )
            .await;
            assert!(!body.is_null());
        }
        let path = fixture.path(browser, "assignment-answer");
        let response = observed(app.clone().oneshot(Fixture::request(
            "POST",
            &path,
            answer.clone(),
        )))
        .await
        .unwrap();
        let receipt = response_json(response, StatusCode::OK).await;
        if let Some(saved) = &saved {
            assert_eq!(&receipt, saved);
        } else {
            saved = Some(receipt);
        }
        let receipt = response_json(
            observed(
                app.clone()
                    .oneshot(Fixture::account_request("POST", &path, answer.clone())),
            )
            .await
            .unwrap(),
            StatusCode::OK,
        )
        .await;
        assert_eq!(
            Some(receipt),
            saved,
            "same authenticated Account/User through token and cookie must replay"
        );
        let mut changed = answer.clone();
        changed["body"] = serde_json::json!("different replay body");
        let error = response_json(
            observed(
                app.clone()
                    .oneshot(Fixture::request("POST", &path, changed)),
            )
            .await
            .unwrap(),
            StatusCode::CONFLICT,
        )
        .await;
        assert!(error.to_string().contains("fingerprint"), "{error}");
        let mut stale = answer.clone();
        stale["operation_key"] = serde_json::json!(format!("stale-{browser}"));
        stale["expected_last_event_id"] = serde_json::json!("unobserved-event");
        response_json(
            observed(app.clone().oneshot(Fixture::request("POST", &path, stale)))
                .await
                .unwrap(),
            StatusCode::CONFLICT,
        )
        .await;
        for suffix in [
            "assignment-tracking/assignments?after_assignment_id=foreign",
            "assignment-diagnostics?assignment_id=fixture-assignment&after_input_event_id=ticket-event:999",
            "assignment-diagnostics?assignment_id=fixture-assignment&limit=101",
        ] {
            response_json(
                observed(app.clone().oneshot(Fixture::request(
                    "GET",
                    &fixture.path(browser, suffix),
                    serde_json::Value::Null,
                )))
                .await
                .unwrap(),
                StatusCode::BAD_REQUEST,
            )
            .await;
        }
        let mut without_origin = Fixture::request("POST", &path, answer.clone());
        without_origin.headers_mut().remove(ORIGIN);
        response_json(
            observed(app.clone().oneshot(without_origin)).await.unwrap(),
            StatusCode::FORBIDDEN,
        )
        .await;
        let missing = path.replace(
            fixture.ticket.resource_key.as_deref().unwrap(),
            "T-999999999",
        );
        response_json(
            observed(
                app.clone()
                    .oneshot(Fixture::request("POST", &missing, answer.clone())),
            )
            .await
            .unwrap(),
            StatusCode::NOT_FOUND,
        )
        .await;
        let mut unauthenticated = Fixture::request("POST", &path, answer.clone());
        unauthenticated.headers_mut().remove("cookie");
        response_json(
            observed(app.clone().oneshot(unauthenticated))
                .await
                .unwrap(),
            StatusCode::UNAUTHORIZED,
        )
        .await;
    }
    fixture.drain().await;
    assert_eq!(fixture.answer_count(), 1);
    assert_eq!(
        Some(fixture.assert_saved_answer("http-replay", &answer)),
        saved
    );
    fixture.finish().await;
}

// Uses the same request-proof signer and registered source records as existing
// HTTP auth fixtures. Runtime identity comes only from the verified proof.
fn signed_request(
    identity: &RuntimeIdentityMaterial,
    worker: &str,
    method: &str,
    path: &str,
    body: serde_json::Value,
) -> Request {
    let body = if method == "GET" {
        Vec::new()
    } else {
        serde_json::to_vec(&body).unwrap()
    };
    let proof = worker_runtime::auth::RuntimeRequestSourceSigner::from_identity(identity)
        .issue(
            "assignment-db-server",
            WORKSPACE,
            Some(worker),
            worker_runtime::auth::WORKSPACE_REQUEST_PERMISSION,
            method,
            path,
            &body,
            i64::try_from(worker_runtime::auth::unix_now_seconds()).unwrap(),
            30,
        )
        .unwrap();
    Request::builder()
        .method(method)
        .uri(path)
        .header(CONTENT_TYPE, "application/json")
        .header(
            worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER,
            proof,
        )
        .body(axum::body::Body::from(body))
        .unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn signed_assignment_answer_rejects_non_orchestrator_and_replayed_proof_without_a_receipt() {
    let fixture = Fixture::new().await;
    let identity = RuntimeIdentityMaterial::generate("signed-source").unwrap();
    fixture
        .store
        .upsert_workspace_runtime_binding(
            WorkspaceRuntimeBinding {
                workspace_id: WORKSPACE.into(),
                runtime_id: "signed-source".into(),
                display_name: "Signed source".into(),
                base_url: "https://runtime.example.invalid".into(),
                public_key: identity.public_key.clone(),
                public_key_fingerprint: String::new(),
                binding_id: "signed-binding".into(),
                authentication_mode: StoredRuntimeAuthenticationMode::WorkspaceIdentity,
                created_at: AT.into(),
                updated_at: AT.into(),
                revoked_at: None,
            },
            true,
        )
        .unwrap();
    fixture
        .store
        .upsert_worker_registry(&WorkerRegistryRecord {
            workspace_id: WORKSPACE.into(),
            worker: RuntimeWorkerRef::new("signed-source", "source-worker"),
            display_name: "Source worker".into(),
            profile: Some("builtin:coder".into()),
            retention_state: "normal".into(),
            transcript_ref: None,
            session_ref: None,
            summary_ref: None,
            diagnostics_ref: None,
            created_at: AT.into(),
            updated_at: AT.into(),
        })
        .unwrap();
    let app = build_router(fixture.api.clone());
    for browser in [false, true] {
        let path = fixture.path(browser, "assignment-answer");
        let answer = fixture.answer("denied-source");
        let request = signed_request(&identity, "source-worker", "POST", &path, answer.clone());
        let proof =
            request.headers()[worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER].clone();
        response_json(
            observed(app.clone().oneshot(request)).await.unwrap(),
            StatusCode::FORBIDDEN,
        )
        .await;
        let mut replay = signed_request(&identity, "source-worker", "POST", &path, answer.clone());
        replay.headers_mut().insert(
            worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER,
            proof,
        );
        response_json(
            observed(app.clone().oneshot(replay)).await.unwrap(),
            StatusCode::UNAUTHORIZED,
        )
        .await;
        // The identical registered signed Worker can read; 403 above is domain
        // authorization, not accidentally failed proof authentication.
        for suffix in [
            "assignment-tracking/assignments?limit=1",
            "assignment-diagnostics?assignment_id=fixture-assignment",
        ] {
            response_json(
                observed(app.clone().oneshot(signed_request(
                    &identity,
                    "source-worker",
                    "GET",
                    &fixture.path(browser, suffix),
                    serde_json::Value::Null,
                )))
                .await
                .unwrap(),
                StatusCode::OK,
            )
            .await;
        }
    }
    fixture.drain().await;
    assert_eq!(fixture.answer_count(), 0);
    assert_eq!(Connection::open(&fixture.api.config.database_path).unwrap().query_row(
        "SELECT count(*) FROM typed_ticket_assignment_tracking_operations WHERE operation_key='denied-source'",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    fixture.finish().await;
}

#[tokio::test(flavor = "current_thread")]
async fn registered_current_signed_orchestrator_answers_both_routes_with_source_bound_receipt() {
    let fixture = Fixture::new().await;
    let orchestrator = fixture.start_orchestrator().await;
    let identity = (*EMBEDDED_RUNTIME_REQUEST_IDENTITY).clone();
    let binding_id = fixture
        .api
        .config_store
        .get_workspace_runtime_binding(WORKSPACE, EMBEDDED_WORKER_RUNTIME_ID)
        .unwrap()
        .unwrap()
        .binding_id;
    let answer = fixture.answer("signed-current-answer");
    let mut saved = None;
    let app = build_router(fixture.api.clone());
    for browser in [false, true] {
        let request = signed_request(
            &identity,
            &orchestrator.worker_id,
            "POST",
            &fixture.path(browser, "assignment-answer"),
            answer.clone(),
        );
        let receipt = response_json(
            observed(app.clone().oneshot(request)).await.unwrap(),
            StatusCode::OK,
        )
        .await;
        if let Some(saved) = &saved {
            assert_eq!(&receipt, saved);
        } else {
            saved = Some(receipt);
        }
    }
    fixture.drain().await;
    assert_eq!(fixture.answer_count(), 1);
    assert_eq!(
        fixture.execution.take_inputs().len(),
        1,
        "receipt replay must not republish input"
    );
    let conn = Connection::open(&fixture.api.config.database_path).unwrap();
    let source: String = conn.query_row(
        "SELECT source_context_json FROM typed_ticket_assignment_tracking_operations WHERE workspace_id=?1 AND operation_key='signed-current-answer'",
        [WORKSPACE],|r|r.get(0)).unwrap();
    let source: serde_json::Value = serde_json::from_str(&source).unwrap();
    assert_eq!(source["source"]["kind"], "runtime", "{source}");
    assert_eq!(
        source["source"]["worker_id"], orchestrator.worker_id,
        "{source}"
    );
    assert_eq!(
        source["source"]["authentication_binding_id"], binding_id,
        "{source}"
    );
    // Same operation key/body from an Account is not the signed Worker's receipt.
    response_json(
        observed(app.oneshot(Fixture::account_request(
            "POST",
            &fixture.path(true, "assignment-answer"),
            answer,
        )))
        .await
        .unwrap(),
        StatusCode::CONFLICT,
    )
    .await;
    assert_eq!(fixture.answer_count(), 1);
    fixture.finish().await;
}

#[tokio::test(flavor = "current_thread")]
async fn saturated_signed_assignment_live_discovery_returns_503_before_sql_while_db_callbacks_progress()
 {
    let fixture = Fixture::new().await;
    let orchestrator = fixture.start_orchestrator().await;
    let binding_id = fixture
        .api
        .config_store
        .get_workspace_runtime_binding(WORKSPACE, &orchestrator.runtime_id)
        .unwrap()
        .unwrap()
        .binding_id;
    let context = server_api::ServerRequestContext {
        actor: None,
        worker_source: Some(server_api::ServerWorkerSource {
            runtime_id: orchestrator.runtime_id.clone(),
            worker_id: orchestrator.worker_id.clone(),
        }),
        runtime_source: Some(server_api::ServerRuntimeSource {
            authentication_binding_id: binding_id,
            runtime_id: orchestrator.runtime_id.clone(),
            worker_id: Some(orchestrator.worker_id.clone()),
        }),
        origin: None,
        transport_headers: Vec::new(),
    };
    // Isolate the generated handler's live-discovery admission from earlier
    // authentication Host permits. Actual signed proof authentication is covered
    // by the current-source/denial tests above; this context is the trusted input
    // to the same canonical/Browser generated HTTP handlers.
    let service = Arc::new(ServerApiContractService::Workspace(fixture.api.clone()));
    let app = server_api::server_api_axum::ticket_assignment_answer(service.clone())
        .merge(server_api::server_api_axum::ticket_assignment_answer_browser(service))
        .layer(Extension(context));
    let full = Saturation::fill(&fixture.api.host_executors.host().unwrap(), 2, 16);
    for browser in [false, true] {
        let key = format!("host-denied-answer-{browser}");
        let lifetime = Arc::new(Lifetimes::default());
        let response = observed(crate::db_executor::with_operation_lifetime(
            lifetime.clone(),
            app.clone().oneshot(Fixture::request(
                "POST",
                &fixture.path(browser, "assignment-answer"),
                fixture.answer(&key),
            )),
        ))
        .await
        .unwrap();
        let body = response_json(response, StatusCode::SERVICE_UNAVAILABLE).await;
        assert_eq!(
            body["diagnostics"][0]["code"], "db_capacity_exceeded",
            "{body}"
        );
        assert_eq!(
            lifetime.admissions.load(Ordering::SeqCst),
            1,
            "rejected live discovery must not proceed to SQL admission"
        );
        assert_eq!(
            lifetime.live.load(Ordering::SeqCst),
            0,
            "rejected discovery retained a request lifecycle lease"
        );
        assert_eq!(
            observed(
                fixture
                    .executor
                    .run(|| Ok::<_, Error>("DB callback progressed"))
            )
            .await
            .unwrap(),
            "DB callback progressed",
            "Host saturation consumed the sole DB worker"
        );
        assert_eq!(
            fixture.answer_count(),
            0,
            "discovery rejection committed an answer/audit"
        );
        assert_eq!(Connection::open(&fixture.api.config.database_path).unwrap().query_row(
            "SELECT count(*) FROM typed_ticket_assignment_tracking_operations WHERE workspace_id=?1 AND operation_key=?2",
            rusqlite::params![WORKSPACE, key], |r| r.get::<_, i64>(0)).unwrap(), 0,
            "discovery rejection committed a receipt");
        assert!(
            fixture.execution.take_inputs().is_empty(),
            "unaccepted answer dispatched Runtime input"
        );
    }
    drop(full);
    fixture.finish().await;
}
