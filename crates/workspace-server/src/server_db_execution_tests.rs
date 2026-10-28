//! Real SQLite waits at the HTTP/authority boundary (not mock executor jobs).
#[path = "server_public_assignment_execution_tests.rs"]
mod public_assignment;
#[path = "server_residual_execution_tests.rs"]
mod residual;
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

#[track_caller]
fn observed<T>(future: impl Future<Output = T>) -> impl Future<Output = T> {
    let caller = std::panic::Location::caller();
    async move {
        tokio::time::timeout(OBSERVE_LIMIT, future)
            .await
            .unwrap_or_else(|_| {
                panic!("request did not reach the expected DB/lifecycle state at {caller}")
            })
    }
}

struct Fixture {
    // Keep temporary authority files alive until all other fixture owners drop.
    server: WorkspaceServerApi,
    api: WorkspaceApi,
    executor: DbExecutor,
    host_executor: DbExecutor,
    store: Arc<SqliteWorkspaceStore>,
    _dir: tempfile::TempDir,
}

impl Fixture {
    async fn new() -> Self {
        Self::with_repositories(Vec::new()).await
    }

    async fn with_repositories(repositories: Vec<ConfiguredRepository>) -> Self {
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
            repositories,
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
        for repository in &config.repositories {
            store
                .upsert_repository(&crate::store::RepositoryRecord {
                    workspace_id: WORKSPACE.into(),
                    repository_id: repository.id.clone(),
                    repository_key: repository.repository_key.clone(),
                    kind: "git".into(),
                    provider: Some(repository.provider.clone()),
                    source: repository.source.clone(),
                    default_ref: repository.default_selector.clone(),
                    source_fingerprint: repository.source_fingerprint.clone(),
                    observed_status: repository.observed_status.clone(),
                    observed_at: repository.observed_at.clone(),
                    created_at: config.workspace_created_at.clone(),
                    updated_at: config.workspace_created_at.clone(),
                })
                .unwrap();
        }
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
        // Explicit small Host budget proves the actual mixed mutation owner;
        // the production/default fixture limits remain 2 workers/16 queued.
        let host_executor = DbExecutor::new(DbExecutorLimits {
            workers: NonZeroUsize::new(1).unwrap(),
            queued: NonZeroUsize::new(1).unwrap(),
        })
        .unwrap();
        let host_executors = HostExecutors {
            host: Arc::new(std::sync::OnceLock::from(Some(host_executor.clone()))),
            ..Default::default()
        };
        let api =
            WorkspaceApi::new_with_host_executors(config.clone(), store.clone(), host_executors)
                .await
                .unwrap();
        let server = WorkspaceServerApi::new_for_test(config, store.clone());
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
            host_executor,
            store,
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
        "response assembly must finish in the admitted mutation operation: {body}"
    );
    fixture.host_executor.drain().await.unwrap();
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
    fixture.host_executor.drain().await.unwrap();
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
    let mut drained = Box::pin(fixture.host_executor.drain());
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
    fixture.host_executor.drain().await.unwrap();
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

fn install_remote_fixture(
    fixture: &Fixture,
    runtime_id: &str,
    base_url: &str,
    signed: bool,
) -> crate::store::WorkspaceRuntimeBinding {
    let identity = worker_runtime::auth::RuntimeIdentityMaterial::generate(runtime_id).unwrap();
    fixture
        .store
        .upsert_workspace_runtime_binding(
            crate::store::WorkspaceRuntimeBinding {
                workspace_id: WORKSPACE.into(),
                runtime_id: runtime_id.into(),
                display_name: "Held Runtime".into(),
                base_url: base_url.into(),
                public_key: identity.public_key,
                public_key_fingerprint: String::new(),
                binding_id: String::new(),
                authentication_mode: if signed {
                    crate::store::WorkspaceRuntimeAuthenticationMode::WorkspaceIdentity
                } else {
                    crate::store::WorkspaceRuntimeAuthenticationMode::LegacyServerIssuer
                },
                created_at: "2026-01-01T00:00:00Z".into(),
                updated_at: "2026-01-01T00:00:00Z".into(),
                revoked_at: None,
            },
            false,
        )
        .unwrap();
    let binding = fixture
        .store
        .get_workspace_runtime_binding(WORKSPACE, runtime_id)
        .unwrap()
        .unwrap();
    // This narrow fixture exercises the provider's own live authority callback;
    // it does not start the unrelated subscription/projection reconnect producer.
    fixture.api.runtime.set_runtime_binding_gate(|_| true);
    let mut config = crate::hosts::RemoteRuntimeConfig::new(
        runtime_id,
        "Held Runtime",
        base_url,
        Some("must-not-fall-back".into()),
    )
    .with_timeout(WATCHDOG);
    if signed {
        fixture
            .api
            .signing_identities
            .provision_existing(WORKSPACE, &format!("account-{WORKSPACE}"))
            .unwrap();
        config.workspace_authorization = Some(WorkspaceRuntimeAuthorization::new(
            fixture.store.clone(),
            fixture.api.signing_identities.clone(),
            "http://localhost:8787",
            binding.clone(),
        ));
    }
    fixture.api.runtime.register_or_replace(
        RemoteWorkerRuntime::new(config, WORKSPACE.into(), "http://localhost:8787".into()).unwrap(),
    );
    binding
}

#[tokio::test(flavor = "current_thread")]
async fn cold_protocol_source_signing_waits_off_tokio_and_never_falls_back_on_overload_or_revocation()
 {
    let fixture = Fixture::new().await;
    let binding = install_remote_fixture(&fixture, "protocol-runtime", "http://127.0.0.1:9", true);
    let worker = RuntimeWorkerRef::new(&binding.runtime_id, "source-worker");
    let (mut busy, waiting) = BusyObservation::install_store(fixture.store.clone());
    let mut lock = WriteLock::acquire(fixture.api.config.database_path.clone());
    let writer = fixture.store.clone();
    fixture
        .executor
        .dispatch(move || {
            writer
                .mark_api_token_used("missing-token", "2026-01-01T00:00:00Z")
                .unwrap()
        })
        .unwrap();
    observed(waiting).await.unwrap(); // SQL holds the actual shared connection mutex.
    let mut source = Box::pin(workspace_worker_protocol_source(&fixture.api, &worker));
    assert!(futures::poll!(&mut source).is_pending());
    runtime_progress_while_locked(&lock).await;
    assert!(futures::poll!(&mut source).is_pending());
    assert!(
        matches!(
            workspace_worker_protocol_source(&fixture.api, &worker).await,
            Err(Error::DbCapacityExceeded)
        ),
        "overload must not return an unsigned or legacy-bearer source"
    );
    lock.release();
    busy.resume();
    let RuntimeObservationSource::RemoteWs(config) = observed(source).await.unwrap() else {
        panic!("remote source expected")
    };
    let first = config
        .bearer_token
        .expect("fresh Workspace capability is required");
    assert_ne!(first, "must-not-fall-back");
    let RuntimeObservationSource::RemoteWs(second) =
        workspace_worker_protocol_source(&fixture.api, &worker)
            .await
            .unwrap()
    else {
        panic!("remote source expected")
    };
    assert_ne!(
        first,
        second.bearer_token.unwrap(),
        "cold lookup must sign a fresh request, not cache authentication success"
    );
    fixture
        .store
        .revoke_workspace_runtime_binding(WORKSPACE, &binding.runtime_id, "2026-01-02T00:00:00Z")
        .unwrap();
    let error = workspace_worker_protocol_source(&fixture.api, &worker)
        .await
        .err()
        .expect("revoked authority cannot produce any source");
    assert!(
        matches!(error, Error::RuntimeOperationFailed { ref code, .. } if code == "workspace_runtime_authorization_stale"),
        "{error:?}"
    );
    fixture.executor.drain().await.unwrap();
}

// Loopback fake at the Session/Worker Runtime HTTP boundary. It owns one reply,
// released by an explicit event; Drop unblocks accept/read and joins on failure.
struct HeldRuntimeReply {
    address: std::net::SocketAddr,
    entered: Option<oneshot::Receiver<String>>,
    release: Option<mpsc::Sender<()>>,
    upstream_released: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
impl HeldRuntimeReply {
    fn new() -> Self {
        Self::with_held_path(None)
    }
    fn for_path(path: &str) -> Self {
        Self::with_held_path(Some(path.to_owned()))
    }
    fn with_held_path(path: Option<String>) -> Self {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (entered, received) = oneshot::channel();
        let (release, permit) = mpsc::channel();
        let upstream_released = Arc::new(AtomicBool::new(false));
        let flag = upstream_released.clone();
        let thread = std::thread::spawn(move || {
            loop {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                stream.set_read_timeout(Some(WATCHDOG)).unwrap();
                let Ok(Some(request)) = read_runtime_fixture_request(&mut stream) else {
                    return;
                };
                if path.as_ref().is_some_and(|path| !request.contains(path)) {
                    // Ready's best-effort notification lookup precedes its detail
                    // source observation. Deny it promptly; hold only the named stage.
                    let _ = stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    continue;
                }
                let _ = entered.send(request);
                let _ = permit.recv_timeout(WATCHDOG);
                flag.store(true, Ordering::SeqCst);
                let _ = stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                break;
            }
        });
        Self {
            address,
            entered: Some(received),
            release: Some(release),
            upstream_released,
            thread: Some(thread),
        }
    }
    fn finish(&mut self) {
        self.release.take();
        let _ = std::net::TcpStream::connect(self.address);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}
impl Drop for HeldRuntimeReply {
    fn drop(&mut self) {
        self.finish();
    }
}

// Read one complete request before selecting a held path or denying a prior
// notification. TCP reads are not HTTP frame boundaries; replying while a POST
// body remains unread can also reset the connection instead of delivering 503.
fn read_runtime_fixture_request(reader: impl std::io::Read) -> std::io::Result<Option<String>> {
    use std::io::{BufRead, Read};
    let mut reader = std::io::BufReader::new(reader);
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let request = line.trim_end().to_owned();
    let mut header_bytes = line.len();
    let mut body_len = 0;
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        header_bytes += line.len();
        if header_bytes > 16384 {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
        if line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                body_len = value
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| std::io::ErrorKind::InvalidData)?;
                if body_len > 1024 * 1024 {
                    return Err(std::io::ErrorKind::InvalidData.into());
                }
            }
            if name.eq_ignore_ascii_case("transfer-encoding") {
                return Err(std::io::ErrorKind::InvalidData.into());
            }
        }
    }
    let mut body = vec![0; body_len];
    reader.read_exact(&mut body)?;
    Ok(Some(request))
}

#[test]
fn held_runtime_fixture_consumes_fragmented_headers_and_body_before_selecting_the_path() {
    struct Fragmented(std::io::Cursor<Vec<u8>>);
    impl std::io::Read for Fragmented {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let length = buffer.len().min(2);
            std::io::Read::read(&mut self.0, &mut buffer[..length])
        }
    }
    let request = b"POST /v1/repository-refs/observe HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\n\r\nnotes";
    let mut reader = Fragmented(std::io::Cursor::new(request.to_vec()));
    assert_eq!(
        read_runtime_fixture_request(&mut reader)
            .unwrap()
            .as_deref(),
        Some("POST /v1/repository-refs/observe HTTP/1.1")
    );
    assert_eq!(reader.0.position(), request.len() as u64);
    let truncated = &request[..request.len() - 1];
    assert_eq!(
        read_runtime_fixture_request(truncated).unwrap_err().kind(),
        std::io::ErrorKind::UnexpectedEof
    );
}

struct CountJobLifetime(Arc<std::sync::atomic::AtomicUsize>);
struct CountJobLease(Arc<std::sync::atomic::AtomicUsize>);
impl DbOperationLifetime for CountJobLifetime {
    fn retain(&self) -> Box<dyn Send> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::new(CountJobLease(self.0.clone()))
    }
}
impl Drop for CountJobLease {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn subjektiv_runtime_waits_leave_db_and_control_free_and_cancel_retains_the_actual_host_lease()
 {
    // Different mixed helpers carry this rule: Session Read/Search, subject
    // projection, and legacy consolidation scope authorization (even refusal).
    for kind in ["read", "search", "subject", "legacy_scope"] {
        let fixture = Fixture::new().await;
        let mut reply = HeldRuntimeReply::new();
        install_remote_fixture(
            &fixture,
            "subject-runtime",
            &format!("http://{}", reply.address),
            false,
        );
        let Json(subject) = scoped_create_subjektiv_subject(
            State(fixture.api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: WORKSPACE.into(),
            }),
            Json(server_api::SubjektivSubjectCreateRequest {
                role: "network subject".into(),
                behavior_md: String::new(),
            }),
        )
        .await
        .unwrap()
        .1;
        if let Some(operation) = match kind {
            "read" => Some("session.read"),
            "search" => Some("session.search"),
            _ => None,
        } {
            open_subjektiv_store(&fixture.api)
                .unwrap()
                .set_subject_permissions(
                    &subject.id,
                    &server_api::authorization::SubjectPermissionsUpdate {
                        operation_id: format!("session-wait-grant-{kind}"),
                        expected_change_id: None,
                        execution: vec![server_api::authorization::ScopedPermission {
                            operation: operation.into(),
                            resource: server_api::authorization::PermissionResource::Subject {
                                workspace_id: WORKSPACE.into(),
                                subject_id: subject.id.clone(),
                            },
                        }],
                        delegation: vec![],
                    },
                )
                .unwrap();
        }
        let singleton = if kind == "legacy_scope" {
            format!("{SUBJEKTIV_CONSOLIDATION_SINGLETON_PREFIX}{}", subject.id)
        } else {
            crate::subjektiv::subject_worker_singleton_key(&subject.id).unwrap()
        };
        let memory = fixture
            .store
            .get_workspace_memory_settings(WORKSPACE)
            .unwrap();
        let reservation = fixture
            .store
            .reserve_worker_create(
                WORKSPACE,
                "subject-runtime",
                "network-fixture",
                "network-fixture-fingerprint",
                Some(&singleton),
                &memory,
            )
            .unwrap();
        let worker = RuntimeWorkerRef::new("subject-runtime", reservation.worker_id.to_string());
        fixture
            .store
            .upsert_worker_registry(&WorkerRegistryRecord {
                workspace_id: WORKSPACE.into(),
                worker: worker.clone(),
                display_name: "network fixture".into(),
                profile: None,
                retention_state: "normal".into(),
                transcript_ref: None,
                session_ref: None,
                summary_ref: None,
                diagnostics_ref: None,
                created_at: "2026-01-01T00:00:00Z".into(),
                updated_at: "2026-01-01T00:00:00Z".into(),
            })
            .unwrap();
        fixture
            .store
            .complete_worker_create_reservation(WORKSPACE, reservation.worker_id)
            .unwrap();
        open_subjektiv_store(&fixture.api)
            .unwrap()
            .record_session_attribution(
                crate::subjektiv::SubjectSessionAttribution::new(
                    subject.id.clone(),
                    worker.runtime_id.clone(),
                    worker.worker_id.clone(),
                    "session",
                )
                .unwrap(),
            )
            .unwrap();
        fixture
            .store
            .create_worker_control_grant(&WorkerControlGrantRecord {
                workspace_id: WORKSPACE.into(),
                grant_id: "session-observe".into(),
                controller: worker.clone(),
                subject: worker.clone(),
                relation: "subject_session".into(),
                origin: "test".into(),
                permissions: vec!["observe".into()],
                operation_id: "session-observe".into(),
                created_at: "2026-01-01T00:00:00Z".into(),
                revoked_at: None,
            })
            .unwrap();
        let context = server_api::ServerRequestContext {
            actor: None,
            worker_source: None,
            runtime_source: Some(server_api::ServerRuntimeSource {
                authentication_binding_id: "fixture-binding".into(),
                runtime_id: worker.runtime_id,
                worker_id: Some(worker.worker_id),
            }),
            origin: None,
            transport_headers: Vec::new(),
        };
        let counts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let lifetime = Arc::new(CountJobLifetime(counts.clone()));
        let api = fixture.api.clone();
        let task = tokio::spawn(crate::db_executor::with_operation_lifetime(
            lifetime,
            async move {
                if kind == "subject" {
                    scoped_get_subjektiv_subject(
                        State(api),
                        AxumPath(ScopedSubjektivSubjectPath {
                            workspace_id: WORKSPACE.into(),
                            subject_id: subject.id,
                        }),
                    )
                    .await
                    .map(|_| ())
                } else {
                    let operation = match kind {
                        "read" => server_api::SubjektivSessionBackendOperation::Read(
                            server_api::SubjektivSessionReadRequest {
                                session_id: "session".into(),
                                segment_id: "segment".into(),
                                entry_ref: "E1".into(),
                                mode: Default::default(),
                                cursor: None,
                            },
                        ),
                        "search" => server_api::SubjektivSessionBackendOperation::Search(
                            serde_json::from_value(
                                serde_json::json!({"session_id":"session","query":"needle"}),
                            )
                            .unwrap(),
                        ),
                        _ => server_api::SubjektivSessionBackendOperation::List(
                            serde_json::from_value(serde_json::json!({})).unwrap(),
                        ),
                    };
                    scoped_subjektiv_session_backend(
                        State(api),
                        AxumPath(ScopedWorkspacePath {
                            workspace_id: WORKSPACE.into(),
                        }),
                        context,
                        Json(server_api::SubjektivSessionBackendRequest { operation }),
                    )
                    .await
                    .map(|_| ())
                }
            },
        ));
        let request = observed(reply.entered.take().unwrap()).await.unwrap();
        assert!(
            request.contains(if kind == "read" || kind == "search" {
                "/v1/session-public/"
            } else {
                "/v1/workers/"
            }),
            "{kind}: {request}"
        );
        let read_store = fixture.store.clone();
        observed(
            fixture
                .executor
                .run(move || read_store.get_workspace_memory_settings(WORKSPACE)),
        )
        .await
        .unwrap();
        assert_eq!(
            observed(tokio::spawn(async { "control" })).await.unwrap(),
            "control"
        );
        observed(tokio::time::sleep(StdDuration::from_millis(1))).await; // timer contract, not a wait guess
        assert!(
            !task.is_finished(),
            "{kind} completed before upstream release"
        );
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(
            counts.load(Ordering::SeqCst),
            1,
            "{kind}: cancellation lost actual Host authority"
        );
        reply.finish();
        observed(fixture.host_executor.drain()).await.unwrap();
        assert_eq!(
            counts.load(Ordering::SeqCst),
            0,
            "{kind}: completed Host job leaked its lease"
        );
        fixture.executor.drain().await.unwrap();
    }
}

#[tokio::test(flavor = "current_thread")]
async fn runtime_registration_scan_keeps_sql_and_tokio_free_and_retains_cancelled_host_budget() {
    let fixture = Fixture::new().await;
    let mut reply = HeldRuntimeReply::new();
    install_remote_fixture(
        &fixture,
        "scan-runtime",
        &format!("http://{}", reply.address),
        true,
    );
    let leases = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let api = fixture.api.clone();
    let task = tokio::spawn(crate::db_executor::with_operation_lifetime(
        Arc::new(CountJobLifetime(leases.clone())),
        async move { scan_runtime_registration_workers(&api, "scan-runtime").await },
    ));
    let request = observed(reply.entered.take().unwrap()).await.unwrap();
    assert!(request.contains("/v1/workers"), "{request}");
    assert!(
        !reply.upstream_released.load(Ordering::SeqCst),
        "registration scan blocked Tokio until watchdog bailout"
    );
    let store = fixture.store.clone();
    observed(
        fixture
            .executor
            .run(move || store.get_workspace_memory_settings(WORKSPACE)),
    )
    .await
    .unwrap();
    observed(tokio::time::sleep(StdDuration::from_millis(1))).await;
    let mut queued_control = Box::pin(fixture.host_executor.run(|| Ok::<_, Error>(())));
    assert!(futures::poll!(&mut queued_control).is_pending());
    let rejected = scan_runtime_registration_workers(&fixture.api, "scan-runtime")
        .await
        .unwrap_err();
    response_json(rejected.into_response(), StatusCode::SERVICE_UNAVAILABLE).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(leases.load(Ordering::SeqCst), 1);
    assert!(!reply.upstream_released.load(Ordering::SeqCst));
    reply.finish();
    observed(queued_control).await.unwrap();
    observed(fixture.host_executor.drain()).await.unwrap();
    assert_eq!(leases.load(Ordering::SeqCst), 0);
    fixture.executor.drain().await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn runtime_management_authority_waits_off_tokio_and_retains_cancelled_db_admission() {
    for stage in ["identity", "binding"] {
        let fixture = Fixture::new().await;
        let binding =
            install_remote_fixture(&fixture, "authority-runtime", "http://127.0.0.1:9", true);
        let (mut busy, waiting) = BusyObservation::install_store(fixture.store.clone());
        let mut lock = WriteLock::acquire(fixture.api.config.database_path.clone());
        let writer = fixture.store.clone();
        fixture
            .executor
            .dispatch(move || {
                writer
                    .mark_api_token_used("missing-token", "2026-01-01T00:00:00Z")
                    .unwrap();
            })
            .unwrap();
        observed(waiting).await.unwrap();
        let leases = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let api = fixture.api.clone();
        let expected = binding.clone();
        let mut authority = Box::pin(crate::db_executor::with_operation_lifetime(
            Arc::new(CountJobLifetime(leases.clone())),
            async move {
                if stage == "identity" {
                    require_active_runtime_registration_identity(&api).await
                } else {
                    require_current_runtime_test_binding(&api, expected)
                        .await
                        .map(|_| ())
                }
            },
        ));
        assert!(
            futures::poll!(&mut authority).is_pending(),
            "{stage}: authority must queue without blocking Tokio on the shared mutex"
        );
        runtime_progress_while_locked(&lock).await;
        let overload = if stage == "identity" {
            require_active_runtime_registration_identity(&fixture.api)
                .await
                .unwrap_err()
        } else {
            require_current_runtime_test_binding(&fixture.api, binding.clone())
                .await
                .unwrap_err()
        };
        response_json(overload.into_response(), StatusCode::SERVICE_UNAVAILABLE).await;
        drop(authority);
        assert_eq!(
            leases.load(Ordering::SeqCst),
            1,
            "{stage}: cancelled DB authority lost its lease"
        );
        lock.release();
        busy.resume();
        observed(fixture.executor.drain()).await.unwrap();
        assert_eq!(leases.load(Ordering::SeqCst), 0);
        require_active_runtime_registration_identity(&fixture.api)
            .await
            .unwrap();
        assert_eq!(
            require_current_runtime_test_binding(&fixture.api, binding.clone())
                .await
                .unwrap()
                .binding_id,
            binding.binding_id
        );
        fixture
            .store
            .revoke_workspace_runtime_binding(
                WORKSPACE,
                &binding.runtime_id,
                "2026-01-02T00:00:00Z",
            )
            .unwrap();
        let stale = require_current_runtime_test_binding(&fixture.api, binding)
            .await
            .unwrap_err();
        response_json(stale.into_response(), StatusCode::CONFLICT).await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn runtime_connection_post_ping_sql_waits_off_tokio_while_host_control_progresses() {
    let fixture = Fixture::new().await;
    let mut reply = HeldRuntimeReply::new();
    let binding = install_remote_fixture(
        &fixture,
        "ping-runtime",
        &format!("http://{}", reply.address),
        true,
    );
    let mut test = Box::pin(test_runtime_connection(
        State(fixture.api.clone()),
        AxumPath(binding.runtime_id.clone()),
    ));
    assert!(futures::poll!(&mut test).is_pending());
    observed(fixture.executor.drain()).await.unwrap();
    assert!(futures::poll!(&mut test).is_pending());
    let request = observed(reply.entered.take().unwrap()).await.unwrap();
    assert!(request.contains("/ping"), "{request}");
    let (mut busy, waiting) = BusyObservation::install_store(fixture.store.clone());
    let mut lock = WriteLock::acquire(fixture.api.config.database_path.clone());
    let writer = fixture.store.clone();
    fixture
        .executor
        .dispatch(move || {
            writer
                .mark_api_token_used("missing-token", "2026-01-01T00:00:00Z")
                .unwrap();
        })
        .unwrap();
    observed(waiting).await.unwrap();
    reply.finish();
    observed(fixture.host_executor.drain()).await.unwrap();
    assert!(
        futures::poll!(&mut test).is_pending(),
        "post-ping binding fence must await DB without a synchronous mutex wait"
    );
    runtime_progress_while_locked(&lock).await;
    assert_eq!(
        observed(fixture.host_executor.run(|| Ok::<_, Error>("control")))
            .await
            .unwrap(),
        "control"
    );
    lock.release();
    busy.resume();
    let Json(result) = observed(test).await.unwrap();
    assert_eq!(result.binding_id, binding.binding_id);
    fixture.executor.drain().await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn saturated_host_rejects_runtime_key_revoke_without_mutating_trust() {
    let fixture = Fixture::new().await;
    fixture.api.worker_projection.shutdown_async().await;
    observed(fixture.executor.drain()).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let binding = install_remote_fixture(
        &fixture,
        "revoke-runtime",
        &format!("http://{}", listener.local_addr().unwrap()),
        true,
    );
    fixture
        .api
        .runtime_binding_expectations
        .write()
        .unwrap()
        .insert(
            (WORKSPACE.into(), binding.runtime_id.clone()),
            binding.clone(),
        );
    let initial_registrations = fixture.api.runtime_subscription_broker.runtime_ids();
    // Sender Drop also releases each controlled job if a route assertion panics.
    let mut releases: Vec<mpsc::Sender<()>> = Vec::new();
    let (started, running) = oneshot::channel();
    let (release, permit) = mpsc::channel();
    releases.push(release);
    fixture
        .host_executor
        .dispatch(move || {
            let _ = started.send(());
            let _ = permit.recv_timeout(WATCHDOG);
        })
        .unwrap();
    observed(running).await.unwrap();
    let (release, permit) = mpsc::channel();
    releases.push(release);
    fixture
        .host_executor
        .dispatch(move || {
            let _ = permit.recv_timeout(WATCHDOG);
        })
        .unwrap();
    let body = response_json(
        observed(fixture.dispatch(Fixture::request(
            "DELETE",
            "/api/w/db-execution/runtimes/revoke-runtime/trust-key",
            serde_json::json!({"expected_binding_id": binding.binding_id}),
        )))
        .await
        .unwrap(),
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .await;
    assert!(body.to_string().contains("db_capacity_exceeded"), "{body}");
    assert_eq!(
        fixture
            .store
            .get_workspace_runtime_binding(WORKSPACE, &binding.runtime_id)
            .unwrap()
            .unwrap(),
        binding
    );
    assert!(
        fixture
            .store
            .list_workspace_runtime_binding_audit(WORKSPACE, &binding.runtime_id, 10)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fixture.api.runtime_subscription_broker.runtime_ids(),
        initial_registrations
    );
    assert_eq!(
        fixture
            .api
            .runtime_binding_expectations
            .read()
            .unwrap()
            .get(&(WORKSPACE.into(), binding.runtime_id.clone())),
        Some(&binding)
    );
    drop(releases);
    observed(fixture.host_executor.drain()).await.unwrap();
    observed(fixture.executor.drain()).await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_runtime_key_revoke_commits_audit_and_removes_broker_registration() {
    let fixture = Fixture::new().await;
    // Stop unrelated projection writes before observing this mutation's SQLite
    // wait. Keep the original runtime alive through actual workflow cleanup.
    fixture.api.worker_projection.shutdown_async().await;
    observed(fixture.executor.drain()).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let binding = install_remote_fixture(
        &fixture,
        "revoke-runtime",
        &format!("http://{}", listener.local_addr().unwrap()),
        true,
    );
    fixture
        .api
        .runtime_binding_expectations
        .write()
        .unwrap()
        .insert(
            (WORKSPACE.into(), binding.runtime_id.clone()),
            binding.clone(),
        );
    let mut config = remote_runtime_config_from_binding(&binding).unwrap();
    config.workspace_authorization = Some(WorkspaceRuntimeAuthorization::new(
        fixture.store.clone(),
        fixture.api.signing_identities.clone(),
        "http://localhost:8787",
        binding.clone(),
    ));
    fixture
        .api
        .runtime_subscription_broker
        .register_remote_runtime(config);
    let mut subscription = fixture
        .api
        .runtime_subscription_broker
        .subscribe(
            &binding.runtime_id,
            protocol::subscription::EventSubscriptionSelector::RuntimeWorkers,
        )
        .unwrap();
    // Broker registration starts an independent cold signing DB job. Observe
    // its actual handshake before occupying SQL for revoke: otherwise this
    // fixture races that producer against HTTP auth in its 1+1 DB budget.
    // Keep the handshake pending so downstream shutdown is still exercised.
    let (mut upstream, _) = observed(listener.accept()).await.unwrap();
    let handshake = observed(async {
        use tokio::io::AsyncReadExt;
        let mut bytes = Vec::new();
        while !bytes.ends_with(b"\r\n\r\n") {
            let mut chunk = [0; 1024];
            let count = upstream.read(&mut chunk).await.unwrap();
            assert_ne!(count, 0, "broker closed before sending its handshake");
            bytes.extend_from_slice(&chunk[..count]);
            assert!(bytes.len() <= 8192, "unexpectedly large handshake");
        }
        String::from_utf8(bytes).unwrap()
    })
    .await;
    assert!(handshake.starts_with("GET /v1/protocol/ws "));
    assert!(
        handshake
            .to_ascii_lowercase()
            .contains("authorization: bearer ")
    );
    observed(fixture.executor.drain()).await.unwrap();
    let (mut busy, waiting) = BusyObservation::install_store(fixture.store.clone());
    let mut lock = WriteLock::acquire(fixture.api.config.database_path.clone());
    let server = fixture.server.clone();
    let request = Fixture::request(
        "DELETE",
        "/api/w/db-execution/runtimes/revoke-runtime/trust-key",
        serde_json::json!({"expected_binding_id": binding.binding_id}),
    );
    let mut revoke =
        tokio::spawn(async move { dispatch_workspace_request(State(server), request).await });
    tokio::select! {
        reached = observed(waiting) => reached.unwrap(), // Actual authenticated BEGIN IMMEDIATE.
        response = &mut revoke => {
            let response = response.unwrap();
            let status = response.status();
            let body = axum::body::to_bytes(response.into_body(), 65536).await.unwrap();
            panic!("revoke returned before its SQLite wait: {status}: {}", String::from_utf8_lossy(&body));
        }
    }
    runtime_progress_while_locked(&lock).await;
    revoke.abort();
    assert!(revoke.await.unwrap_err().is_cancelled());
    let admission = fixture.server.admission_for_workspace(WORKSPACE).await;
    let mut deletion = Box::pin(admission.lock_deletion());
    assert!(
        futures::poll!(&mut deletion).is_pending(),
        "cancelled HTTP must retain actual revoke ownership until publication"
    );
    assert!(
        fixture
            .api
            .runtime_subscription_broker
            .runtime_ids()
            .contains(&binding.runtime_id)
    );
    lock.release();
    busy.resume();
    observed(fixture.host_executor.drain()).await.unwrap();
    observed(fixture.executor.drain()).await.unwrap();
    let _deletion = observed(deletion).await;
    let saved = fixture
        .store
        .get_workspace_runtime_binding(WORKSPACE, &binding.runtime_id)
        .unwrap()
        .unwrap();
    assert!(saved.revoked_at.is_some());
    assert_ne!(saved.binding_id, binding.binding_id);
    let audit = fixture
        .store
        .list_workspace_runtime_binding_audit(WORKSPACE, &binding.runtime_id, 10)
        .unwrap();
    assert_eq!(
        audit.len(),
        1,
        "cancelled mutation must commit one audit without retry"
    );
    assert_eq!(audit[0].action, "revoked");
    assert_eq!(audit[0].actor_account_id, format!("account-{WORKSPACE}"));
    assert_eq!(audit[0].binding_id, saved.binding_id);
    assert_eq!(audit[0].at, saved.revoked_at.unwrap());
    assert!(
        !fixture
            .api
            .runtime_subscription_broker
            .runtime_ids()
            .contains(&binding.runtime_id),
        "committed revoke must unregister even after HTTP waiter cancellation"
    );
    assert!(
        !fixture
            .api
            .runtime_binding_expectations
            .read()
            .unwrap()
            .contains_key(&(WORKSPACE.into(), binding.runtime_id.clone()))
    );
    // Exercise downstream termination at the broker protocol boundary, not a
    // product process or complete browser/Runtime E2E flow.
    observed(async {
        loop {
            match subscription.recv().await {
                Some(crate::runtime_subscription::BrokerSubscriptionEvent::Closed {
                    code, ..
                }) => {
                    assert_eq!(
                        code,
                        protocol::subscription::SubscriptionTerminationCode::ServerShutdown
                    );
                    break;
                }
                Some(crate::runtime_subscription::BrokerSubscriptionEvent::Disconnected {
                    ..
                }) => {}
                event => panic!("expected revoke shutdown, got {event:?}"),
            }
        }
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_runtime_metadata_mutation_keeps_commit_and_registration_publication_owned() {
    let fixture = Fixture::new().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().fallback(|| async { StatusCode::SERVICE_UNAVAILABLE }),
        )
        .await
        .unwrap();
    });
    install_remote_fixture(&fixture, "metadata-runtime", &endpoint, true);
    let (mut busy, waiting) = BusyObservation::install_store(fixture.store.clone());
    let mut lock = WriteLock::acquire(fixture.api.config.database_path.clone());
    let leases = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut update = Box::pin(crate::db_executor::with_operation_lifetime(
        Arc::new(CountJobLifetime(leases.clone())),
        update_remote_runtime_metadata_owned(
            fixture.api.clone(),
            WORKSPACE.into(),
            "metadata-runtime".into(),
            "updated metadata".into(),
            endpoint,
        ),
    ));
    assert!(futures::poll!(&mut update).is_pending());
    observed(waiting).await.unwrap(); // The real metadata SQL mutation is waiting.
    runtime_progress_while_locked(&lock).await;
    drop(update);
    assert_eq!(
        leases.load(Ordering::SeqCst),
        2,
        "actual Host workflow and its accepted SQL both retain authority"
    );
    lock.release();
    busy.resume();
    observed(fixture.host_executor.drain()).await.unwrap();
    observed(fixture.executor.drain()).await.unwrap();
    assert_eq!(leases.load(Ordering::SeqCst), 0);
    let saved = fixture
        .store
        .get_workspace_runtime_binding(WORKSPACE, "metadata-runtime")
        .unwrap()
        .unwrap();
    assert_eq!(saved.display_name, "updated metadata");
    let published = fixture
        .api
        .runtime_binding_expectations
        .read()
        .unwrap()
        .get(&(WORKSPACE.into(), "metadata-runtime".into()))
        .cloned()
        .unwrap();
    assert_eq!(
        published, saved,
        "SQL completion must publish even after waiter cancellation"
    );
    fixture
        .api
        .runtime_subscription_broker
        .unregister_runtime("metadata-runtime");
    fixture.api.worker_projection.shutdown_async().await;
    fixture.executor.drain().await.unwrap();
    server.abort();
    let _ = server.await;
}

#[tokio::test(flavor = "current_thread")]
async fn runtime_metadata_reads_wait_off_tokio_and_db_and_retain_cancelled_host_ownership() {
    for read in ["resources", "launch_options"] {
        let fixture = Fixture::new().await;
        let mut reply = HeldRuntimeReply::new();
        install_remote_fixture(
            &fixture,
            "summary-runtime",
            &format!("http://{}", reply.address),
            true,
        );
        let leases = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let api = fixture.api.clone();
        let task = tokio::spawn(crate::db_executor::with_operation_lifetime(
            Arc::new(CountJobLifetime(leases.clone())),
            async move {
                if read == "resources" {
                    workspace_runtime_resources_response(&api, WORKSPACE)
                        .await
                        .map(|_| ())
                } else {
                    get_worker_launch_options(State(api)).await.map(|_| ())
                }
            },
        ));
        let request = observed(reply.entered.take().unwrap()).await.unwrap();
        assert!(request.contains("/v1/runtime"), "{request}");
        assert!(
            !reply.upstream_released.load(Ordering::SeqCst),
            "upstream watchdog released before Tokio could observe the request"
        );
        let read_store = fixture.store.clone();
        observed(
            fixture
                .executor
                .run(move || read_store.get_workspace_memory_settings(WORKSPACE)),
        )
        .await
        .unwrap();
        assert_eq!(
            observed(tokio::spawn(async { "control" })).await.unwrap(),
            "control"
        );
        observed(tokio::time::sleep(StdDuration::from_millis(1))).await; // timer contract only
        assert!(!reply.upstream_released.load(Ordering::SeqCst));
        assert!(!task.is_finished());
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(
            leases.load(Ordering::SeqCst),
            1,
            "cancel released actual Runtime summary owner"
        );
        reply.finish();
        observed(fixture.host_executor.drain()).await.unwrap();
        // Runtime metadata also awaits binding reads owned by the DB executor.
        // Its reply can wake Host before DB reply/lifetime cleanup has finished;
        // draining Host alone is not a barrier for that separately-owned job.
        observed(fixture.executor.drain()).await.unwrap();
        assert_eq!(leases.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn worker_discovery_sql_waits_off_tokio_and_keeps_admission_until_actual_cleanup() {
    // Exercise the exact post-auth handler boundary: signed source verification
    // has separate route coverage. Auth itself also awaits DB and would otherwise
    // hide a handler's direct mutex acquisition until after the writer is released.
    for cancel_waiter in [false, true] {
        let fixture = Fixture::new().await;
        let (mut busy, waiting) = BusyObservation::install_store(fixture.store.clone());
        let mut lock = WriteLock::acquire(fixture.api.config.database_path.clone());
        let writer = fixture.store.clone();
        fixture
            .executor
            .dispatch(move || {
                writer
                    .mark_api_token_used("missing-token", "2026-01-01T00:00:00Z")
                    .unwrap();
            })
            .unwrap();
        observed(waiting).await.unwrap(); // Actual SQL writer holds shared connection mutex.
        let query = || server_api::WorkspaceWorkerDiscoveryQuery {
            limit: Some(1),
            cursor: None,
            query: Some("missing-peer".into()),
        };
        let leases = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let lifetime = Arc::new(CountJobLifetime(leases.clone()));
        let mut discovery = Box::pin(crate::db_executor::with_operation_lifetime(
            lifetime,
            discover_workspace_workers(fixture.api.clone(), WORKSPACE.into(), query(), true),
        ));
        assert!(
            futures::poll!(&mut discovery).is_pending(),
            "discovery must queue, not synchronously wait for the SQL mutex"
        );
        assert_eq!(leases.load(Ordering::SeqCst), 1);
        runtime_progress_while_locked(&lock).await;
        assert_eq!(
            observed(fixture.host_executor.run(|| Ok::<_, Error>("Host control")))
                .await
                .unwrap(),
            "Host control"
        );
        assert!(futures::poll!(&mut discovery).is_pending());
        let rejected =
            discover_workspace_workers(fixture.api.clone(), WORKSPACE.into(), query(), true)
                .await
                .unwrap_err()
                .into_response();
        let body = response_json(rejected, StatusCode::SERVICE_UNAVAILABLE).await;
        assert!(body.to_string().contains("db_capacity_exceeded"), "{body}");
        let denied =
            discover_workspace_workers(fixture.api.clone(), WORKSPACE.into(), query(), false)
                .await
                .unwrap_err()
                .into_response();
        response_json(denied, StatusCode::FORBIDDEN).await;
        // Failure cleanup never makes the progress assertions above true.
        assert!(!lock.released.load(Ordering::SeqCst));
        if cancel_waiter {
            drop(discovery);
            assert_eq!(
                leases.load(Ordering::SeqCst),
                1,
                "cancel released accepted discovery authority"
            );
            let rejected =
                discover_workspace_workers(fixture.api.clone(), WORKSPACE.into(), query(), true)
                    .await
                    .unwrap_err()
                    .into_response();
            response_json(rejected, StatusCode::SERVICE_UNAVAILABLE).await;
            lock.release();
            busy.resume();
        } else {
            lock.release();
            busy.resume();
            let page = observed(discovery).await.unwrap();
            assert!(
                page.workers.is_empty(),
                "empty filtered projection must remain empty"
            );
            assert!(page.next_cursor.is_none());
        }
        observed(fixture.executor.drain()).await.unwrap();
        assert_eq!(
            leases.load(Ordering::SeqCst),
            0,
            "actual discovery cleanup leaked its lease"
        );
        fixture.host_executor.drain().await.unwrap();
    }
}
