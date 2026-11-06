//! Backend result fixture: real SQLite reservation -> coordinator helper -> typed
//! read -> public ApiError response -> HTTP middleware observation. Only the route's
//! startup producer is supplied by the fixture; this does not test authorization,
//! Session callbacks, Runtime startup or the Browser creation route. Those belong
//! to session_registration_connected_tests. Runs in the ordinary server lib suite;
//! each case owns its tempdir/DB and subscriber, with no external services.

use super::*;
use protocol::session_failure::{SessionPrincipal, SessionRegistrationCause};
use protocol::startup_failure::{StartupFailureAttachment, StartupPersistenceOutcome};
use runtime_api::startup::{
    CompensationOutcome, CreationAcceptance, DecodeStartupProblem, DecodeStoredWorkerCreateFailure,
    ExecutionOutcome, FinalizeFacts, FinalizeStage, StartupBoundary, StartupCause,
    StartupOperationFacts, WorkerCreateFailure, decode_startup_problem,
};
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use tracing::instrument::WithSubscriber;

const SECRETS: &[&str] = &[
    "attachment-secret",
    "context-secret",
    "diagnostic-secret",
    "source-secret",
    "sqlite-secret",
    "/private/startup-provider",
];

fn failure_fixture(compensation: CompensationOutcome) -> WorkerCreateFailure {
    let clean = WorkerCreateFailure::from_cause(
        StartupCause::Session(SessionRegistrationCause::denied(SessionPrincipal::Subject)),
        StartupOperationFacts {
            acceptance: CreationAcceptance::Accepted,
            execution: ExecutionOutcome::Errored,
            finalize: Some(FinalizeFacts {
                stage: FinalizeStage::SessionRegistration,
                compensation,
            }),
            backend_compensation: Some(compensation),
            correlation: None,
        },
    )
    .unwrap();
    let (mut startup, operation) = clean.into_parts();
    // Real untrusted attachment bytes, not a source string which the codec never
    // sees. Projection must reconstruct the cause and discard received prose/context.
    let encoded: String =
        serde_json::from_value(serde_json::to_value(&startup.primary).unwrap()).unwrap();
    let mut envelope: Value = serde_json::from_str(&encoded).unwrap();
    envelope["explanation"] = json!("attachment-secret /private/startup-provider");
    envelope["context"]["operations"] = json!([{
        "operation": "context-secret", "stage": "context-secret",
        "operation_ref": "context-secret"
    }]);
    envelope["context"]["diagnostics"] = json!([{
        "code": "diagnostic-secret", "explanation": "diagnostic-secret"
    }]);
    startup.primary = serde_json::from_value::<StartupFailureAttachment>(json!(
        serde_json::to_string(&envelope).unwrap()
    ))
    .unwrap();
    // Session metadata and Backend result persistence have independent owners.
    startup.persistence = Some(StartupPersistenceOutcome::NotSaved);
    let failure = WorkerCreateFailure::new(startup, operation).unwrap();
    assert_eq!(
        primary_cause(&failure),
        StartupCause::Session(SessionRegistrationCause::denied(SessionPrincipal::Subject))
    );
    failure
}

fn primary_cause(failure: &WorkerCreateFailure) -> StartupCause {
    match failure.startup().primary.decode::<StartupCause>() {
        error_contract::DecodeFailure::Known(received) => received.facts().cause().clone(),
        _ => panic!("fixture/result must retain a known typed primary"),
    }
}

fn assert_semantics(actual: &WorkerCreateFailure, expected: &WorkerCreateFailure) {
    assert_eq!(primary_cause(actual), primary_cause(expected));
    assert_eq!(actual.public_facts(), expected.public_facts());
}

fn reserve(api: &WorkspaceApi) -> RuntimeWorkerRef {
    let settings = api
        .config_store
        .get_workspace_memory_settings(TEST_WORKSPACE_ID)
        .unwrap();
    let reservation = api
        .config_store
        .reserve_worker_create(
            TEST_WORKSPACE_ID,
            EMBEDDED_WORKER_RUNTIME_ID,
            "manual:startup-result-fixture",
            "sha256:startup-result-fixture",
            None,
            &settings,
        )
        .unwrap();
    RuntimeWorkerRef {
        runtime_id: EMBEDDED_WORKER_RUNTIME_ID.into(),
        worker_id: reservation.worker_id.to_string(),
    }
}

fn retained(api: &WorkspaceApi, worker: &RuntimeWorkerRef) -> WorkerCreateFailure {
    match api.worker_create_failure(worker).unwrap() {
        Some(DecodeStoredWorkerCreateFailure::Known(failure)) => failure,
        other => panic!("typed public read must return retained result: {other:?}"),
    }
}

// Snapshot all reservation columns except the observation, plus allocation/lease
// rows. Saving a diagnostic must not complete/remove/reallocate the reservation.
fn lifecycle_snapshot(api: &WorkspaceApi) -> Vec<Vec<rusqlite::types::Value>> {
    api.config_store.with_conn(|conn| {
        let mut columns = conn.prepare("PRAGMA table_info(worker_create_reservations)")?;
        let names = columns.query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let columns = names.into_iter().filter(|name| name != "startup_failure_json")
            .map(|name| format!("\"{name}\"" )).collect::<Vec<_>>().join(",");
        let queries = [
            format!("SELECT {columns} FROM worker_create_reservations ORDER BY workspace_id,allocation_key"),
            "SELECT * FROM workspace_resource_keys ORDER BY workspace_id,resource_kind,resource_id".into(),
            "SELECT * FROM worker_singleton_owners ORDER BY workspace_id,singleton_key".into(),
        ];
        let mut snapshot = Vec::new();
        for query in queries {
            let mut statement = conn.prepare(&query)?;
            let count = statement.column_count();
            snapshot.extend(statement.query_map([], |row| {
                (0..count).map(|i| row.get(i)).collect::<rusqlite::Result<Vec<_>>>()
            })?.collect::<rusqlite::Result<Vec<_>>>()?);
        }
        Ok(snapshot)
    }).unwrap()
}

fn api_error(failure: WorkerCreateFailure) -> ApiError {
    ApiError::with_diagnostics(
        Error::WorkerCreateFailed {
            failure: Box::new(failure),
            source: Some(Box::new(Error::InvalidInput(
                "source-secret /private/startup-provider".into(),
            ))),
        },
        Vec::new(),
    )
}

fn result_router(failure: WorkerCreateFailure) -> Router {
    Router::new()
        .route(
            "/api/startup-result-fixture",
            axum::routing::get(move || {
                let failure = failure.clone();
                async move { api_error(failure).into_response() }
            }),
        )
        .layer(middleware::from_fn(log_failed_api_response))
}

async fn public_result(app: Router) -> (StatusCode, Value, WorkerCreateFailure) {
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/startup-result-fixture")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    assert_eq!(
        response.headers()[CONTENT_TYPE],
        runtime_api::startup::PROBLEM_CONTENT_TYPE
    );
    let body = to_bytes(
        response.into_body(),
        runtime_api::startup::MAX_STARTUP_PROBLEM_BYTES,
    )
    .await
    .unwrap();
    let decoded = decode_startup_problem(
        &body,
        status.as_u16(),
        runtime_api::startup::PROBLEM_CONTENT_TYPE,
        StartupBoundary::Browser,
    );
    let DecodeStartupProblem::Known(failure) = decoded else {
        panic!("actual HTTP response must satisfy shared bounded contract: {decoded:?}");
    };
    let value = serde_json::from_slice(&body).unwrap();
    assert_no_secrets(std::str::from_utf8(&body).unwrap());
    (status, value, failure)
}

#[derive(Clone, Default)]
struct Capture(Arc<std::sync::Mutex<Vec<u8>>>);

impl Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Capture {
    fn events(&self) -> Vec<Value> {
        let bytes = self.0.lock().unwrap().clone();
        let text = std::str::from_utf8(&bytes).unwrap();
        assert_no_secrets(text);
        text.lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

fn subscriber(capture: Capture) -> impl tracing::Subscriber + Send + Sync {
    tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .json()
        .flatten_event(true)
        .with_writer(move || capture.clone())
        .finish()
}

fn assert_no_secrets(text: &str) {
    for secret in SECRETS {
        assert!(
            !text.contains(secret),
            "private value {secret} leaked: {text}"
        );
    }
}

fn assert_log(event: &Value, failure: &WorkerCreateFailure, status: StatusCode) {
    assert_eq!(event["target"], "yoi::worker_startup");
    assert_eq!(event["event"], "worker_create_result");
    assert_eq!(event["domain"], "session_registration");
    assert_eq!(event["reason"], "permission_denied");
    assert_eq!(
        event["correlation"],
        failure.operation().correlation.as_ref().unwrap().as_str()
    );
    assert_eq!(event["status"], status.as_u16());
    assert_eq!(
        event["acceptance"],
        format!("{:?}", failure.operation().acceptance)
    );
    assert_eq!(
        event["execution"],
        format!("{:?}", failure.operation().execution)
    );
    assert_eq!(
        event["finalize"],
        format!("{:?}", failure.operation().finalize)
    );
    assert_eq!(
        event["backend_compensation"],
        format!("{:?}", failure.operation().backend_compensation)
    );
    assert_eq!(
        event["backend_result_persistence"],
        format!("{:?}", failure.backend_result_persistence())
    );
    assert_eq!(
        event["persistence"],
        format!("{:?}", failure.startup().persistence)
    );
}

#[tokio::test]
async fn persisted_result_public_problem_and_single_http_log_have_matching_semantics() {
    let dir = tempfile::tempdir().unwrap();
    let api = test_api(dir.path()).await;
    let worker = reserve(&api);
    let before = lifecycle_snapshot(&api);
    let original = failure_fixture(CompensationOutcome::Residual);
    let returned = worker_startup::persist(&api, &worker, original.clone());
    assert_eq!(
        returned.backend_result_persistence(),
        Some(StartupPersistenceOutcome::Persisted)
    );
    assert_eq!(
        returned.startup().persistence,
        Some(StartupPersistenceOutcome::NotSaved)
    );
    assert_eq!(primary_cause(&returned), primary_cause(&original));
    assert_eq!(lifecycle_snapshot(&api), before);
    let saved = retained(&api, &worker);
    assert_semantics(&saved, &returned);
    // Storage projection changes envelope bytes; equality must be semantic.
    assert_ne!(saved.startup().primary, original.startup().primary);
    let capture = Capture::default();
    let (status, problem, public) = public_result(result_router(returned.clone()))
        .with_subscriber(subscriber(capture.clone()))
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        problem["detail"]
            .as_str()
            .unwrap()
            .contains("session.record")
    );
    assert!(problem["detail"].as_str().unwrap().contains("Subject"));
    assert!(
        problem["instance"]
            .as_str()
            .unwrap()
            .contains(returned.operation().correlation.as_ref().unwrap().as_str())
    );
    assert_semantics(&public, &saved);
    let events = capture.events();
    assert_eq!(
        events.len(),
        1,
        "only HTTP responsibility boundary records primary: {events:?}"
    );
    assert_log(&events[0], &public, status);
    assert_eq!(lifecycle_snapshot(&api), before);
}

#[tokio::test]
async fn sqlite_result_write_failure_preserves_primary_facts_prior_bytes_and_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let api = test_api(dir.path()).await;
    let worker = reserve(&api);
    let prior = worker_startup::persist(
        &api,
        &worker,
        failure_fixture(CompensationOutcome::Completed),
    );
    let prior_bytes = api
        .config_store
        .worker_create_failure(
            TEST_WORKSPACE_ID,
            &worker.runtime_id,
            WorkerId::parse(&worker.worker_id).unwrap(),
        )
        .unwrap()
        .unwrap();
    let before = lifecycle_snapshot(&api);
    api.config_store
        .with_conn(|conn| {
            conn.execute_batch(
                "CREATE TRIGGER fail_startup_result BEFORE UPDATE OF startup_failure_json
            ON worker_create_reservations BEGIN SELECT RAISE(FAIL, 'sqlite-secret'); END;",
            )?;
            Ok(())
        })
        .unwrap();
    let attempted = failure_fixture(CompensationOutcome::Unknown);
    let returned = worker_startup::persist(&api, &worker, attempted.clone());
    assert_eq!(
        returned.backend_result_persistence(),
        Some(StartupPersistenceOutcome::OutcomeUnknown)
    );
    assert_eq!(primary_cause(&returned), primary_cause(&attempted));
    let mut expected_operation = attempted.operation().clone();
    expected_operation.correlation = returned.operation().correlation.clone();
    assert_eq!(returned.operation(), &expected_operation);
    assert_eq!(returned.startup(), attempted.startup());
    assert_eq!(lifecycle_snapshot(&api), before);
    assert_eq!(
        api.config_store
            .worker_create_failure(
                TEST_WORKSPACE_ID,
                &worker.runtime_id,
                WorkerId::parse(&worker.worker_id).unwrap(),
            )
            .unwrap()
            .unwrap(),
        prior_bytes
    );
    assert_semantics(&retained(&api, &worker), &prior);
    let capture = Capture::default();
    let (status, _, public) = public_result(result_router(returned.clone()))
        .with_subscriber(subscriber(capture.clone()))
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_semantics(&public, &returned);
    let events = capture.events();
    assert_eq!(events.len(), 1);
    assert_log(&events[0], &returned, status);
}

#[tokio::test]
async fn finalize_compensation_does_not_replace_received_primary_or_acceptance() {
    for compensation in [
        CompensationOutcome::Completed,
        CompensationOutcome::Residual,
        CompensationOutcome::Unknown,
    ] {
        let original = failure_fixture(CompensationOutcome::Residual);
        let error = worker_startup::finalize_error(
            api_error(original.clone()),
            WorkerSpawnFinalizeStage::WorkdirAttachment,
            compensation,
        );
        let Error::WorkerCreateFailed { failure, .. } = &error.error else {
            panic!("finalize must keep a typed startup result");
        };
        assert_eq!(primary_cause(failure), primary_cause(&original));
        assert_eq!(failure.startup(), original.startup());
        assert_eq!(
            failure.operation().acceptance,
            original.operation().acceptance
        );
        assert_eq!(
            failure.operation().execution,
            original.operation().execution
        );
        assert_eq!(
            failure.operation().finalize,
            Some(FinalizeFacts {
                stage: FinalizeStage::WorkdirAttachment,
                compensation
            })
        );
        assert_eq!(failure.operation().backend_compensation, Some(compensation));
        let expected = failure.as_ref().clone();
        let (_, _, public) = public_result(result_router(expected.clone())).await;
        assert_semantics(&public, &expected);
    }
}

#[tokio::test]
async fn context_and_public_projection_are_silent_until_explicit_http_observation() {
    let capture = Capture::default();
    let expected = async {
        let worker = RuntimeWorkerRef {
            runtime_id: "context-secret".into(),
            worker_id: "source-secret".into(),
        };
        let failure =
            worker_startup::correlate(failure_fixture(CompensationOutcome::Unknown), &worker);
        let public = api_error(failure.clone()).into_response();
        assert_eq!(public.status(), StatusCode::CONFLICT);
        assert!(
            capture.events().is_empty(),
            "context/encoder must not log the primary"
        );
        worker_startup::observe_http(&failure, public.status());
        failure
    }
    .with_subscriber(subscriber(capture.clone()))
    .await;
    let events = capture.events();
    assert_eq!(events.len(), 1);
    assert_log(&events[0], &expected, StatusCode::CONFLICT);
}

#[derive(Clone)]
struct FailingWriter(Arc<AtomicUsize>);

impl Write for FailingWriter {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(std::io::Error::other("sink unavailable"))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn failed_log_sink_still_returns_original_http_problem_and_retained_result() {
    let dir = tempfile::tempdir().unwrap();
    let api = test_api(dir.path()).await;
    let worker = reserve(&api);
    let saved = worker_startup::persist(
        &api,
        &worker,
        failure_fixture(CompensationOutcome::Residual),
    );
    let before = lifecycle_snapshot(&api);
    let capture = Capture::default();
    let baseline = public_result(result_router(saved.clone()))
        .with_subscriber(subscriber(capture.clone()))
        .await;
    assert_eq!(capture.events().len(), 1);
    let attempts = Arc::new(AtomicUsize::new(0));
    let writer = FailingWriter(attempts.clone());
    let failed_subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .json()
        .flatten_event(true)
        .with_writer(move || writer.clone())
        .finish();
    let actual = public_result(result_router(saved.clone()))
        .with_subscriber(failed_subscriber)
        .await;
    assert!(
        attempts.load(Ordering::SeqCst) > 0,
        "must exercise actual observation sink failure"
    );
    assert_eq!(actual.0, baseline.0);
    assert_eq!(
        actual.1, baseline.1,
        "sink failure must not replace the outgoing Problem"
    );
    assert_semantics(&actual.2, &saved);
    assert_semantics(&retained(&api, &worker), &saved);
    assert_eq!(lifecycle_snapshot(&api), before);
}
