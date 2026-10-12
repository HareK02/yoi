//! Real SQLite waits at the HTTP/authority boundary (not mock executor jobs).
use super::*;
use crate::SqliteWorkspaceStore;
use crate::db_executor::{DbExecutor, DbExecutorLimits, DbOperationLifetime};
use rusqlite::{Connection, ffi};
use std::{
    ffi::c_void,
    future::Future,
    num::NonZeroUsize,
    sync::{atomic::AtomicBool, mpsc},
    thread::JoinHandle,
    time::Duration as StdDuration,
};
use tokio::sync::oneshot;

const WORKSPACE: &str = "db-execution";
const SUBJECTS: &str = "/api/w/db-execution/subjektiv/subjects";
const OBSERVE_LIMIT: StdDuration = StdDuration::from_secs(2);
const WATCHDOG: StdDuration = StdDuration::from_secs(5);

async fn observed<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(OBSERVE_LIMIT, future)
        .await
        .expect("request did not reach the expected DB/lifecycle state")
}

struct Fixture {
    // Keep temporary authority files alive until all other fixture owners drop.
    server: WorkspaceServerApi,
    api: WorkspaceApi,
    executor: DbExecutor,
    _dir: tempfile::TempDir,
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let executor = DbExecutor::new(DbExecutorLimits {
            workers: NonZeroUsize::new(1).unwrap(),
            queued: NonZeroUsize::new(1).unwrap(),
        })
        .unwrap();
        // Explicit startup config: never resolve the developer's live data dir.
        let config = ServerConfig {
            workspace_id: WORKSPACE.into(),
            workspace_display_name: "DB execution fixture".into(),
            workspace_created_at: "2026-01-01T00:00:00Z".into(),
            workspace_execution_root: dir.path().into(),
            database_path: dir.path().join("server.db"),
            data_root: dir.path().join("data"),
            embedded_runtime_store_root: dir.path().join("runtime"),
            static_assets_dir: None,
            auth: AuthConfig::Passkey {
                rp_id: "localhost".into(),
                origin: "http://localhost:5173".into(),
                public_base_url: "http://localhost:5173".into(),
                cookie_name: "db_execution_session".into(),
            },
            max_records: 100,
            repositories: Vec::new(),
            runtime_event_sources: Vec::new(),
            remote_runtime_sources: Vec::new(),
            backend_base_url: Some("http://localhost:8787".into()),
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
                created_at: config.workspace_created_at.clone(),
                updated_at: config.workspace_created_at.clone(),
            })
            .unwrap();
        store
            .create_browser_session(&crate::store::BrowserSessionRecord {
                session_id: "fixture-session".into(),
                token_hash: crate::auth::token_hash("fixture-browser-token"),
                user_id: "fixture-user".into(),
                created_at: config.workspace_created_at.clone(),
                expires_at: "2999-01-01T00:00:00Z".into(),
                revoked_at: None,
            })
            .unwrap();
        let api = WorkspaceApi::new_for_test(config.clone(), store.clone())
            .await
            .unwrap();
        let server = WorkspaceServerApi::new_for_test(config, store);
        // Cache only the real inner router; avoid starting the unrelated hook.
        server
            .apis
            .lock()
            .await
            .insert(WORKSPACE.into(), api.clone());
        server
            .routers
            .lock()
            .await
            .insert(WORKSPACE.into(), build_inner_router(api.clone()));
        executor.drain().await.unwrap();
        Self {
            server,
            api,
            executor,
            _dir: dir,
        }
    }

    fn request(method: &str, path: &str, body: serde_json::Value) -> Request {
        Request::builder()
            .method(method)
            .uri(path)
            .header(CONTENT_TYPE, "application/json")
            .header("cookie", "db_execution_session=fixture-browser-token")
            .header(ORIGIN, "http://localhost:5173")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap()
    }

    fn subject_request(role: &str) -> Request {
        Self::request(
            "POST",
            SUBJECTS,
            serde_json::json!({ "role": role, "behavior_md": "" }),
        )
    }

    fn dispatch(&self, request: Request) -> tokio::task::JoinHandle<Response> {
        let server = self.server.clone();
        tokio::spawn(async move { dispatch_workspace_request(State(server), request).await })
    }

    // A pre-authenticated generated router isolates handler admission from the
    // earlier browser-session DB read. Full authentication/dispatch is exercised
    // separately by the cancellation and warm-Feature tests below.
    fn inner_router(&self) -> Router {
        build_inner_router(self.api.clone()).layer(Extension(RequestActor {
            user_id: "fixture-user".into(),
            account_id: format!("account-{WORKSPACE}"),
            handle: "fixture-user".into(),
            display_name: "Fixture user".into(),
            auth_method: ActorAuthMethod::BrowserSession,
        }))
    }

    fn feature_database(&self) -> (crate::FeatureDatabase, PathBuf) {
        let database = self
            .api
            .feature_storage
            .open(&self.api.subjektiv_registration)
            .unwrap();
        // Initialize the trusted scope, then obtain SQLite's actual file path
        // from the managed handle rather than guessing the Feature directory.
        crate::subjektiv::SubjektivStore::from_database(database.clone()).unwrap();
        let path = database
            .with_connection(|conn| {
                Ok(PathBuf::from(conn.query_row(
                    "SELECT file FROM pragma_database_list WHERE name = 'main'",
                    [],
                    |row| row.get::<_, String>(0),
                )?))
            })
            .unwrap();
        (database, path)
    }
}

/// SQLite write ownership lives on an OS thread, including a bounded watchdog.
/// Drop releases and joins it even when an assertion panics on the async side.
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
            .expect("SQLite lock holder did not acquire its transaction");
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

/// rusqlite's fn-pointer busy_handler cannot capture a fixture. SQLite's
/// userdata API can, without process globals or changing production hooks.
enum BusyDatabase {
    Feature(crate::FeatureDatabase),
    Store(Arc<SqliteWorkspaceStore>),
}
impl BusyDatabase {
    fn access(&self, access: impl FnOnce(&Connection)) {
        match self {
            Self::Feature(database) => database
                .with_connection(|conn| {
                    access(conn);
                    Ok(())
                })
                .unwrap(),
            Self::Store(store) => store
                .with_conn(|conn| {
                    access(conn);
                    Ok(())
                })
                .unwrap(),
        }
    }
}
struct BusyObservation {
    database: BusyDatabase,
    gate: Box<BusyGate>,
    resume: Option<mpsc::Sender<()>>,
}
impl BusyObservation {
    fn install(database: crate::FeatureDatabase) -> (Self, oneshot::Receiver<()>) {
        Self::install_on(BusyDatabase::Feature(database))
    }
    fn install_store(store: Arc<SqliteWorkspaceStore>) -> (Self, oneshot::Receiver<()>) {
        Self::install_on(BusyDatabase::Store(store))
    }
    fn install_on(database: BusyDatabase) -> (Self, oneshot::Receiver<()>) {
        unsafe extern "C" fn busy(context: *mut c_void, _: i32) -> i32 {
            // SAFETY: owned Box remains registered until Drop unregisters it,
            // serialized by the managed connection mutex.
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
        database.access(|conn| {
            // SAFETY: handle belongs to this connection; the Box is stable.
            let code = unsafe {
                ffi::sqlite3_busy_handler(
                    conn.handle(),
                    Some(busy),
                    (&mut *gate as *mut BusyGate).cast(),
                )
            };
            assert_eq!(code, ffi::SQLITE_OK);
        });
        (
            Self {
                database,
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
        self.database.access(|conn| {
            // SAFETY: unregister before the boxed userdata is freed.
            unsafe {
                ffi::sqlite3_busy_handler(conn.handle(), None, std::ptr::null_mut());
            }
        });
        let _ = &self.gate;
    }
}

// A handler with one DB stage can expose its admission without adding a test
// hook to production. Polling its future to Pending proves run accepted it.
struct AdmissionObservation(Mutex<Option<oneshot::Sender<()>>>);
impl DbOperationLifetime for AdmissionObservation {
    fn retain(&self) -> Box<dyn Send> {
        if let Some(accepted) = self.0.lock().unwrap().take() {
            let _ = accepted.send(());
        }
        Box::new(())
    }
}
fn admission_observation() -> (Arc<dyn DbOperationLifetime>, oneshot::Receiver<()>) {
    let (accepted, receiver) = oneshot::channel();
    (
        Arc::new(AdmissionObservation(Mutex::new(Some(accepted)))),
        receiver,
    )
}

struct StateCheckSaturation {
    executor: DbExecutor,
    stages: std::sync::atomic::AtomicUsize,
    releases: Arc<Mutex<Vec<mpsc::Sender<()>>>>,
}
impl DbOperationLifetime for StateCheckSaturation {
    fn retain(&self) -> Box<dyn Send> {
        if self.stages.fetch_add(1, Ordering::SeqCst) == 1 {
            // Authentication has completed; saturate just before the lifecycle
            // lookup is admitted. The callback's own dispatch admissions are
            // distinct stages and do not recurse into this branch.
            let (started, running) = mpsc::channel();
            let (release, permit) = mpsc::channel();
            self.executor
                .dispatch(move || {
                    let _ = started.send(());
                    let _ = permit.recv_timeout(WATCHDOG);
                })
                .unwrap();
            running
                .recv_timeout(WATCHDOG)
                .expect("state-check blocker did not start");
            self.releases.lock().unwrap().push(release);
            let (release, permit) = mpsc::channel();
            self.executor
                .dispatch(move || {
                    let _ = permit.recv_timeout(WATCHDOG);
                })
                .unwrap();
            self.releases.lock().unwrap().push(release);
        }
        Box::new(())
    }
}
struct ReleaseStateCheckBlockers(Arc<Mutex<Vec<mpsc::Sender<()>>>>);
impl Drop for ReleaseStateCheckBlockers {
    fn drop(&mut self) {
        self.0.lock().unwrap().clear();
    }
}

#[tokio::test(flavor = "current_thread")]
async fn saturated_mutation_lifecycle_lookup_returns_availability_not_deleting_conflict() {
    let fixture = Fixture::new().await;
    let releases = Arc::new(Mutex::new(Vec::new()));
    let release = ReleaseStateCheckBlockers(releases.clone());
    let source = Arc::new(StateCheckSaturation {
        executor: fixture.executor.clone(),
        stages: std::sync::atomic::AtomicUsize::new(0),
        releases,
    });
    let router = Router::new()
        .route(
            SUBJECTS,
            axum::routing::post(|| async { StatusCode::NO_CONTENT }),
        )
        .layer(axum::middleware::from_fn_with_state(
            fixture.api.clone(),
            authorize_workspace_api_request,
        ));
    let response = observed(crate::db_executor::with_operation_lifetime(
        source,
        router.oneshot(Fixture::subject_request("not-executed")),
    ))
    .await
    .unwrap();
    drop(release);
    observed(fixture.executor.drain()).await.unwrap();
    let body = response_json(response, StatusCode::SERVICE_UNAVAILABLE).await;
    assert!(
        !body.to_string().contains("Workspace is deleting"),
        "{body}"
    );
}

async fn runtime_progress_while_locked(lock: &WriteLock) {
    let control = tokio::spawn(async { "control progressed" });
    assert_eq!(observed(control).await.unwrap(), "control progressed");
    // The timer is the contract, not a guessed wait for request completion.
    observed(tokio::time::sleep(StdDuration::from_millis(1))).await;
    assert!(
        !lock.released.load(Ordering::SeqCst),
        "SQLite watchdog released the lock before runtime progress"
    );
}
async fn response_json(response: Response, expected: StatusCode) -> serde_json::Value {
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(status, expected, "{}", String::from_utf8_lossy(&bytes));
    serde_json::from_slice(&bytes).expect("typed JSON response")
}
fn subject_count(path: &Path, role: &str) -> i64 {
    Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT count(*) FROM subjects WHERE role = ?1",
            [role],
            |row| row.get(0),
        )
        .unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn ticket_create_waiting_for_sqlite_does_not_block_timer_or_control() {
    let fixture = Fixture::new().await;
    let mut lock = WriteLock::acquire(fixture.api.config.database_path.clone());
    let (lifetime, accepted) = admission_observation();
    let request = Fixture::request(
        "POST",
        "/api/w/db-execution/tickets",
        serde_json::to_value(ticket::NewTicket::new("locked Ticket")).unwrap(),
    );
    let mut response = Box::pin(crate::db_executor::with_operation_lifetime(
        lifetime,
        fixture.inner_router().oneshot(request),
    ));
    assert!(futures::poll!(&mut response).is_pending());
    observed(accepted).await.unwrap();
    runtime_progress_while_locked(&lock).await;
    assert!(
        futures::poll!(&mut response).is_pending(),
        "mutation responded before the SQLite lock was released"
    );
    let reader = Connection::open(&fixture.api.config.database_path).unwrap();
    let count: i64 = reader
        .query_row(
            "SELECT count(*) FROM typed_tickets WHERE workspace_id = ?1",
            [WORKSPACE],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0, "Ticket must not be visible before commit");
    lock.release();
    let body = response_json(observed(response).await.unwrap(), StatusCode::OK).await;
    assert!(body["id"].is_string(), "{body}");
    // Ticket dispatch can call Runtime callbacks, so its operation owner is
    // the separate bounded host executor, not the store executor.
    observed(fixture.api.host_executors.host().unwrap().drain())
        .await
        .unwrap();
    fixture.executor.drain().await.unwrap();
    let count: i64 = reader.query_row("SELECT count(*) FROM typed_tickets WHERE workspace_id = ?1 AND title = 'locked Ticket'", [WORKSPACE], |row| row.get(0)).unwrap();
    assert_eq!(count, 1, "one accepted HTTP mutation must commit once");
}

#[tokio::test(flavor = "current_thread")]
async fn cold_subjektiv_open_waiting_for_scope_transaction_keeps_runtime_responsive() {
    let fixture = Fixture::new().await;
    let (database, path) = fixture.feature_database();
    drop(database); // FeatureStorage retains Weak handles: the route must cold-open.
    let mut lock = WriteLock::acquire(path);
    let (lifetime, accepted) = admission_observation();
    let request = Fixture::request("GET", SUBJECTS, serde_json::Value::Null);
    let mut response = Box::pin(crate::db_executor::with_operation_lifetime(
        lifetime,
        fixture.inner_router().oneshot(request),
    ));
    assert!(futures::poll!(&mut response).is_pending());
    observed(accepted).await.unwrap();
    runtime_progress_while_locked(&lock).await;
    assert!(
        futures::poll!(&mut response).is_pending(),
        "cold open must wait for its SQLite scope transaction"
    );
    lock.release();
    let body = response_json(observed(response).await.unwrap(), StatusCode::OK).await;
    assert_eq!(body["items"], serde_json::json!([]), "{body}");
    fixture.executor.drain().await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn authenticated_subjektiv_mutation_waits_for_commit_before_response() {
    let fixture = Fixture::new().await;
    let (database, path) = fixture.feature_database();
    let mut lock = WriteLock::acquire(path.clone());
    let (mut busy, waiting) = BusyObservation::install(database);
    let response = fixture.dispatch(Fixture::subject_request("locked-subject"));
    observed(waiting).await.unwrap();
    runtime_progress_while_locked(&lock).await;
    assert!(
        !response.is_finished(),
        "HTTP response must not publish an uncommitted subject"
    );
    assert_eq!(subject_count(&path, "locked-subject"), 0);
    lock.release();
    busy.resume();
    let body = response_json(observed(response).await.unwrap(), StatusCode::CREATED).await;
    assert_eq!(body["role"], "locked-subject", "{body}");
    assert!(
        body["memory_fingerprint"].is_string(),
        "response assembly must finish in the DB operation: {body}"
    );
    fixture.executor.drain().await.unwrap();
    assert_eq!(subject_count(&path, "locked-subject"), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_http_waiter_keeps_deletion_pending_until_sql_finishes() {
    let fixture = Fixture::new().await;
    let (database, path) = fixture.feature_database();
    let mut lock = WriteLock::acquire(path.clone());
    let (mut busy, waiting) = BusyObservation::install(database);
    let response = fixture.dispatch(Fixture::subject_request("cancelled-subject"));
    observed(waiting).await.unwrap(); // Actual SQLite wait, not inferred task scheduling.
    response.abort();
    assert!(observed(response).await.unwrap_err().is_cancelled());
    let admission = fixture.server.admission_for_workspace(WORKSPACE).await;
    let mut deletion = Box::pin(admission.lock_deletion());
    assert!(
        futures::poll!(&mut deletion).is_pending(),
        "aborting HTTP must not drop the accepted SQL RequestLease"
    );
    assert!(
        admission.admit(false).is_none(),
        "deletion must fence fresh requests"
    );
    assert_eq!(subject_count(&path, "cancelled-subject"), 0);
    lock.release();
    busy.resume();
    fixture.executor.drain().await.unwrap();
    let deletion = observed(deletion).await;
    assert_eq!(
        subject_count(&path, "cancelled-subject"),
        1,
        "cancelled waiter must neither cancel nor retry accepted SQL"
    );
    drop(deletion);
    assert!(
        admission.admit(false).is_some(),
        "deletion lease cleanup must reopen admission"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_queued_http_mutation_keeps_capacity_until_commit_and_drain() {
    let fixture = Fixture::new().await;
    let (database, path) = fixture.feature_database();
    let mut lock = WriteLock::acquire(path.clone());
    let (mut busy, waiting) = BusyObservation::install(database);
    let running = fixture.dispatch(Fixture::subject_request("running-subject"));
    observed(waiting).await.unwrap();
    let (lifetime, accepted) = admission_observation();
    let mut queued = Box::pin(crate::db_executor::with_operation_lifetime(
        lifetime,
        fixture
            .inner_router()
            .oneshot(Fixture::subject_request("queued-subject")),
    ));
    assert!(futures::poll!(&mut queued).is_pending());
    observed(accepted).await.unwrap();
    drop(queued); // Accepted job still owns the only waiting slot.
    let rejected = observed(
        fixture
            .inner_router()
            .oneshot(Fixture::subject_request("rejected-subject")),
    )
    .await
    .unwrap();
    let body = response_json(rejected, StatusCode::SERVICE_UNAVAILABLE).await;
    let error: server_api::RepositoryApiError = serde_json::from_value(body.clone()).unwrap();
    assert!(
        error.message.contains("capacity"),
        "typed overload error: {body}"
    );
    assert_eq!(subject_count(&path, "queued-subject"), 0);
    assert_eq!(subject_count(&path, "rejected-subject"), 0);
    let mut drained = Box::pin(fixture.executor.drain());
    assert!(futures::poll!(&mut drained).is_pending());
    lock.release();
    busy.resume();
    response_json(observed(running).await.unwrap(), StatusCode::CREATED).await;
    observed(drained).await.unwrap();
    assert_eq!(
        subject_count(&path, "queued-subject"),
        1,
        "cancelled accepted mutation persists exactly once"
    );
    assert_eq!(
        subject_count(&path, "rejected-subject"),
        0,
        "overload must not execute rejected SQL"
    );
    let retry = fixture.dispatch(Fixture::subject_request("after-drain"));
    response_json(observed(retry).await.unwrap(), StatusCode::CREATED).await;
    fixture.executor.drain().await.unwrap();
    assert_eq!(
        subject_count(&path, "after-drain"),
        1,
        "drain must release actual-job capacity"
    );
}

#[tokio::test(start_paused = true)]
async fn saturated_cancelled_host_jobs_keep_their_budget_but_not_other_fixture_or_cleanup_admission()
 {
    use std::sync::{
        Condvar,
        atomic::{AtomicUsize, Ordering},
    };
    // A drop guard releases workers even when an assertion panics.
    struct Release(Arc<(Mutex<bool>, Condvar)>);
    impl Drop for Release {
        fn drop(&mut self) {
            *self.0.0.lock().unwrap() = true;
            self.0.1.notify_all();
        }
    }
    let owner = HostExecutors::default();
    let other = HostExecutors::default();
    let host = owner.host().unwrap();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let release = Release(gate.clone());
    let completed = Arc::new(AtomicUsize::new(0));
    let mut waiters = Vec::new();
    for _ in 0..2 {
        let host = host.clone();
        let gate = gate.clone();
        let completed = completed.clone();
        let (started, accepted) = tokio::sync::oneshot::channel();
        waiters.push(tokio::spawn(async move {
            host.run(move || {
                started.send(()).unwrap();
                let mut released = gate.0.lock().unwrap();
                while !*released {
                    released = gate.1.wait(released).unwrap();
                }
                completed.fetch_add(1, Ordering::SeqCst);
                Ok::<_, Error>(())
            })
            .await
        }));
        accepted.await.unwrap();
    }
    for _ in 0..16 {
        let completed = completed.clone();
        host.dispatch(move || {
            completed.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
    }
    for waiter in waiters {
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
    }
    assert!(matches!(
        host.run(|| Ok::<_, Error>(())).await,
        Err(Error::DbCapacityExceeded)
    ));
    // Paused Tokio time can advance without completing or releasing actual Host
    // work. Neither another fixture nor cleanup needs this saturated Host lane.
    tokio::time::advance(std::time::Duration::from_secs(31)).await;
    assert_eq!(completed.load(Ordering::SeqCst), 0);
    other
        .host()
        .unwrap()
        .run(|| Ok::<_, Error>(()))
        .await
        .unwrap();
    owner
        .cleanup()
        .unwrap()
        .run(|| Ok::<_, Error>(()))
        .await
        .unwrap();
    assert!(matches!(
        host.run(|| Ok::<_, Error>(())).await,
        Err(Error::DbCapacityExceeded)
    ));
    drop(release);
    host.drain().await.unwrap();
    assert_eq!(completed.load(Ordering::SeqCst), 18);
    host.run(|| Ok::<_, Error>(())).await.unwrap();
    host.drain().await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn restore_intent_pins_once_after_write_ownership_without_a_deferred_read_upgrade() {
    let fixture = Fixture::new().await;
    let store = fixture.api.config_store.clone();
    let (mut busy, waiting) = BusyObservation::install_store(store.clone());
    let mut lock = WriteLock::acquire(fixture.api.config.database_path.clone());
    let worker = RuntimeWorkerRef::new("restore-runtime", "restore-worker");
    let pin_store = store.clone();
    let pin_worker = worker.clone();
    let mut pinned = Box::pin(fixture.executor.run(move || {
        pin_store.pin_internal_worker_restore_intent(
            WORKSPACE,
            &pin_worker,
            "restore-domain",
            Some("observed-before-dispatch"),
            Some("restore-request"),
        )
    }));
    tokio::select! {
        result = &mut pinned => panic!("restore intent must wait for write ownership before reading, not fail a deferred read upgrade: {result:?}"),
        arrived = observed(waiting) => arrived.unwrap(),
    }
    runtime_progress_while_locked(&lock).await;
    assert!(futures::poll!(&mut pinned).is_pending());
    lock.release();
    busy.resume();
    let request = observed(pinned).await.unwrap();
    assert_eq!(request.request_id, "restore-request");
    assert_eq!(
        request.expected_observation_token,
        "observed-before-dispatch"
    );
    fixture.executor.drain().await.unwrap();
    let saved = store
        .get_internal_worker_restore_intent(WORKSPACE, &worker, &request.request_id)
        .unwrap()
        .unwrap();
    assert_eq!(saved.request_id, request.request_id);
    assert_eq!(
        saved.expected_observation_token,
        request.expected_observation_token
    );
    let count: i64 = store
        .with_conn(|conn| {
            Ok(conn.query_row(
        "SELECT count(*) FROM worker_restore_intents WHERE workspace_id=?1 AND request_id=?2",
        rusqlite::params![WORKSPACE, request.request_id], |row| row.get(0)
    )?)
        })
        .unwrap();
    assert_eq!(
        count, 1,
        "one admitted intent must persist exactly once, with no operation retry"
    );
}
