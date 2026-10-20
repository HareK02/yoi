//! Session-open integration through the production signed router, Runtime owner/binding
//! checks, HTTP DTO and tracing boundary. Only the execution backend is replaced
//! for deterministic lookup/open faults and inert owner-binding Worker fixtures;
//! no handler, middleware or grant is replaced. Runs in the normal http-server lib
//! suite with isolated loopback sockets.

use super::*;
use crate::working_directory::WorkingDirectoryDiagnostic;
use std::io::{self, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const OPEN_PATH: &str = "/v1/working-directories/private-workdir-marker/sessions";
const PRIVATE_DIAGNOSTIC: &str = concat!(
    "raw-runtime-error-marker /private/host-path-marker ",
    "command=private-command-marker contents=private-file-marker ",
    "proof=private-proof-marker Authorization: Bearer private-header-marker ",
    "working_directory_not_found authorization_stale runtime_stopped",
);
const PRIVATE_MARKERS: &[&str] = &[
    "raw-runtime-error-marker",
    "/private/host-path-marker",
    "private-command-marker",
    "private-file-marker",
    "private-proof-marker",
    "private-header-marker",
    "private-workdir-marker",
    "private-owner-marker",
];

#[derive(Clone, Default)]
struct LogCapture(Arc<Mutex<Vec<u8>>>);

impl Write for LogCapture {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl LogCapture {
    fn subscriber(&self) -> impl tracing::Subscriber + Send + Sync + 'static {
        let writer = self.clone();
        tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_max_level(tracing::Level::WARN)
            .with_writer(move || writer.clone())
            .finish()
    }

    fn take(&self) -> String {
        String::from_utf8(std::mem::take(&mut *self.0.lock().unwrap())).unwrap()
    }
}

fn assert_private_absent(text: &str) {
    for marker in PRIVATE_MARKERS {
        assert!(
            !text.contains(marker),
            "private input escaped error boundary"
        );
    }
}

fn assert_failure_log(log: &str, status: StatusCode, code: &str, reason: &str, stage: &str) {
    let events: Vec<_> = log
        .lines()
        .filter(|line| line.contains("workdir_provider_failure"))
        .collect();
    assert_eq!(events.len(), 1, "one diagnostic per failed request: {log}");
    let event = events[0];
    for field in [
        "WARN".to_string(),
        "worker_runtime::workdir".to_string(),
        format!("status={}", status.as_u16()),
        format!("code=\"{code}\""),
        format!("failure_reason=\"{reason}\""),
        format!("failure_stage=\"{stage}\""),
    ] {
        assert!(event.contains(&field), "missing {field}: {event}");
    }
    assert_private_absent(log);
}

fn assert_failure_body(body: &str, code: &str, message: &str, reason: &str) {
    assert_private_absent(body);
    // Exact safe transport vocabulary; raw diagnostics and stage are not wire fields.
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(body).unwrap(),
        serde_json::json!({"code": code, "message": message, "failure_reason": reason}),
    );
}

struct Harness {
    server: tokio::task::JoinHandle<()>,
    client: reqwest::Client,
    base: String,
    identity: RuntimeIdentityMaterial,
    trust: WorkspaceIssuerTrustRecord,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Harness {
    async fn new(runtime: Runtime) -> Self {
        let identity = RuntimeIdentityMaterial::generate("runtime-failure-test-key").unwrap();
        let trust = workspace_trust(&identity);
        let app = runtime_http_router_with_workspace_auth(
            runtime,
            None,
            workspace_test_auth(vec![trust.clone()]),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            server,
            client: reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap(),
            base,
            identity,
            trust,
        }
    }

    async fn send(
        &self,
        claims: &WorkspaceCapabilityClaims,
        body: Vec<u8>,
        workspace: &str,
    ) -> (StatusCode, String, String) {
        let token = issue_workspace_capability_token(&self.identity.signing_key().unwrap(), claims)
            .unwrap();
        let response = self
            .client
            .post(format!("{}{OPEN_PATH}", self.base))
            .bearer_auth(&token)
            .header(header::CONTENT_TYPE, "application/json")
            .header(RUNTIME_WORKSPACE_SCOPE_HEADER, workspace)
            .header("x-private-test-header", "private-header-marker")
            .body(body)
            .send()
            .await
            .unwrap();
        let status = StatusCode::from_u16(response.status().as_u16()).unwrap();
        let headers = format!("{:?}", response.headers());
        let body = response.text().await.unwrap();
        assert_private_absent(&headers);
        for output in [&body, &headers] {
            assert!(!output.contains(&token), "signed bearer token leaked");
            assert!(
                !output.contains(&self.identity.private_key),
                "signing key leaked"
            );
        }
        (status, body, token)
    }

    async fn open(&self) -> (StatusCode, String, String) {
        let body = serde_json::to_vec(&OpenWorkdirSessionRequest::default()).unwrap();
        let claims = signed_claims(&self.trust, "POST", OPEN_PATH, &body);
        self.send(&claims, body, "workspace-a").await
    }
}

#[derive(Clone, Copy)]
enum FailureAt {
    Lookup,
    Open,
}

struct FailingBackend {
    at: FailureAt,
    code: &'static str,
    lookups: AtomicUsize,
    opens: AtomicUsize,
}

impl FailingBackend {
    fn runtime(at: FailureAt, code: &'static str) -> (Runtime, Arc<Self>) {
        let backend = Arc::new(Self {
            at,
            code,
            lookups: AtomicUsize::new(0),
            opens: AtomicUsize::new(0),
        });
        let runtime =
            Runtime::with_execution_backend(RuntimeOptions::default(), backend.clone()).unwrap();
        (runtime, backend)
    }

    fn failure(&self) -> WorkingDirectoryDiagnostic {
        WorkingDirectoryDiagnostic::rejected(self.code, PRIVATE_DIAGNOSTIC)
    }
}

impl WorkerExecutionBackend for FailingBackend {
    fn backend_id(&self) -> &str {
        "runtime-failure-test"
    }

    fn spawn_worker(&self, request: WorkerExecutionSpawnRequest) -> WorkerExecutionSpawnResult {
        AcceptingBackend.spawn_worker(request)
    }

    fn dispatch_input(&self, worker: &WorkerRef, input: WorkerInput) -> WorkerExecutionResult {
        AcceptingBackend.dispatch_input(worker, input)
    }

    fn stop_worker(&self, worker: &WorkerRef) -> WorkerExecutionResult {
        AcceptingBackend.stop_worker(worker)
    }

    fn working_directory(
        &self,
        id: &str,
    ) -> Result<WorkingDirectoryStatus, WorkingDirectoryDiagnostic> {
        self.lookups.fetch_add(1, Ordering::SeqCst);
        match self.at {
            FailureAt::Lookup => Err(self.failure()),
            FailureAt::Open => Ok(WorkingDirectoryStatus {
                summary: crate::catalog::WorkingDirectorySummary {
                    working_directory_id: id.into(),
                    display_name: None,
                    repository_id: "repository-test".into(),
                    creation_selector: None,
                    creation_ref: None,
                    creation_tree: None,
                    current_selector: None,
                    current_ref: None,
                    current_tree: None,
                    observed_at_epoch_seconds: None,
                    materializer_kind: crate::catalog::MaterializerKind::RuntimeGitClone,
                    cleanup_target: None,
                    status: crate::catalog::WorkingDirectoryStatusKind::Active,
                    cleanliness: None,
                    occupied_by: None,
                },
            }),
        }
    }

    fn open_workdir_session(
        &self,
        _: &str,
    ) -> Result<WorkdirSessionHandle, WorkingDirectoryDiagnostic> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        Err(self.failure())
    }
}

// Current-thread execution keeps the server tasks inside the thread-local tracing
// capture across awaits, without replacing the process-global subscriber.
#[tokio::test(flavor = "current_thread")]
async fn signed_http_session_open_preserves_runtime_availability_and_owner_failures() {
    let capture = LogCapture::default();
    let _guard = tracing::subscriber::set_default(capture.subscriber());
    tracing::callsite::rebuild_interest_cache();
    tokio::time::timeout(Duration::from_secs(15), async {
        let stopped = Runtime::new_memory();
        stopped.stop_runtime().unwrap();
        let (owned, backend) = FailingBackend::runtime(FailureAt::Lookup, "must_not_reach_backend");
        let mut request = task_request("bind owner");
        request.config_bundle = None;
        request.memory_settings.as_mut().unwrap().workspace_id = "workspace-a".into();
        owned
            .create_worker_scoped(
                &RuntimeWorkspaceScope::new("workspace-a", "private-owner-marker"),
                request,
            )
            .unwrap();
        for (runtime, status, code, message, reason) in [
            (
                stopped,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Workdir session is unavailable",
                "runtime_stopped",
            ),
            (
                Runtime::new_memory(),
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Workdir session is unavailable",
                "runtime_backend_unavailable",
            ),
            (
                owned,
                StatusCode::NOT_FOUND,
                "not_found",
                "Workdir session was not found",
                "runtime_owner_mismatch",
            ),
        ] {
            let harness = Harness::new(runtime).await;
            capture.take();
            let (actual, body, token) = harness.open().await;
            assert_eq!(actual, status);
            assert_failure_body(&body, code, message, reason);
            let log = capture.take();
            assert_failure_log(&log, status, code, reason, "runtime");
            assert!(!log.contains(&token));
            assert!(!log.contains(&harness.identity.private_key));
        }
        assert_eq!(backend.lookups.load(Ordering::SeqCst), 0);
        assert_eq!(backend.opens.load(Ordering::SeqCst), 0);
    })
    .await
    .expect("bounded Runtime failure HTTP requests");
}

#[tokio::test(flavor = "current_thread")]
async fn signed_http_session_open_allowlists_diagnostic_codes_without_guessing_unknown_causes() {
    let capture = LogCapture::default();
    let _guard = tracing::subscriber::set_default(capture.subscriber());
    tracing::callsite::rebuild_interest_cache();
    tokio::time::timeout(Duration::from_secs(20), async {
        for (diagnostic, reason) in [
            ("working_directory_not_found", "working_directory_not_found"),
            (
                "working_directory_not_active",
                "working_directory_not_active",
            ),
            (
                "working_directory_record_invalid",
                "working_directory_record_invalid",
            ),
            (
                "working_directory_repository_access_unavailable",
                "working_directory_access_unavailable",
            ),
            // Plausible provider text/code is not evidence of a typed cause.
            ("authorization_stale", "session_open_failed"),
            (
                "working_directory_not_found_private-proof-marker",
                "session_open_failed",
            ),
        ] {
            for at in [FailureAt::Lookup, FailureAt::Open] {
                let (runtime, backend) = FailingBackend::runtime(at, diagnostic);
                let harness = Harness::new(runtime).await;
                capture.take();
                let (status, body, token) = harness.open().await;
                let (expected, code, message) = if diagnostic == "working_directory_not_found" {
                    (
                        StatusCode::NOT_FOUND,
                        "not_found",
                        "Workdir session was not found",
                    )
                } else {
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        "unavailable",
                        "Workdir session is unavailable",
                    )
                };
                assert_eq!(status, expected, "{diagnostic}");
                assert_failure_body(&body, code, message, reason);
                let log = capture.take();
                assert_failure_log(&log, status, code, reason, "session_open");
                assert!(!log.contains(&token));
                assert!(!log.contains(&harness.identity.private_key));
                assert_eq!(backend.lookups.load(Ordering::SeqCst), 1, "no hidden retry");
                assert_eq!(
                    backend.opens.load(Ordering::SeqCst),
                    usize::from(matches!(at, FailureAt::Open)),
                );
            }
        }
    })
    .await
    .expect("bounded diagnostic fault matrix");
}

#[tokio::test(flavor = "current_thread")]
async fn signed_http_session_open_rejects_bad_capability_scope_and_owner_binding_before_backend() {
    let capture = LogCapture::default();
    let _guard = tracing::subscriber::set_default(capture.subscriber());
    tracing::callsite::rebuild_interest_cache();
    tokio::time::timeout(Duration::from_secs(15), async {
        let (runtime, backend) =
            FailingBackend::runtime(FailureAt::Open, "working_directory_record_invalid");
        let mut request = task_request("foreign workspace binding");
        request.config_bundle = None;
        request.memory_settings.as_mut().unwrap().workspace_id = "other-workspace".into();
        let foreign_worker = runtime
            .create_worker_scoped(
                &RuntimeWorkspaceScope::new("other-workspace", "https://backend.test"),
                request,
            )
            .unwrap()
            .worker_ref;
        let harness = Harness::new(runtime).await;
        for case in ["permission", "body", "workspace", "owner_binding"] {
            let body = if case == "owner_binding" {
                serde_json::to_vec(&OpenWorkdirSessionRequest {
                    owner_worker_id: Some(foreign_worker.worker_id.to_string()),
                })
                .unwrap()
            } else {
                b"{}".to_vec()
            };
            let mut claims = signed_claims(&harness.trust, "POST", OPEN_PATH, &body);
            if case == "permission" {
                claims.operation = "workers:read".into();
            }
            if case == "workspace" {
                claims.issuer_workspace_id = "other-workspace".into();
            }
            let sent = if case == "body" {
                b"{ }".to_vec()
            } else {
                body
            };
            capture.take();
            let (status, body, token) = harness.send(&claims, sent, "workspace-a").await;
            assert_eq!(
                status,
                if case == "owner_binding" {
                    StatusCode::NOT_FOUND
                } else {
                    StatusCode::UNAUTHORIZED
                },
                "{case}",
            );
            assert_private_absent(&body);
            let log = capture.take();
            assert_private_absent(&log);
            assert!(!log.contains(&token));
            assert!(!log.contains(&harness.identity.private_key));
            if case == "owner_binding" {
                assert_failure_body(
                    &body,
                    "not_found",
                    "Workdir session was not found",
                    "working_directory_not_found",
                );
                assert_failure_log(
                    &log,
                    status,
                    "not_found",
                    "working_directory_not_found",
                    "session_open",
                );
            } else {
                let wire: serde_json::Value = serde_json::from_str(&body).unwrap();
                assert_eq!(wire["error"]["code"], "unauthorized");
                assert!(
                    !log.contains("workdir_provider_failure"),
                    "auth rejection is not a provider fault",
                );
            }
        }
        assert_eq!(backend.lookups.load(Ordering::SeqCst), 0);
        assert_eq!(backend.opens.load(Ordering::SeqCst), 0);
    })
    .await
    .expect("bounded authorization rejection requests");
}

#[tokio::test(flavor = "current_thread")]
async fn runtime_error_response_keeps_raw_details_private_and_unclassified_causes_generic() {
    let capture = LogCapture::default();
    let _guard = tracing::subscriber::set_default(capture.subscriber());
    tracing::callsite::rebuild_interest_cache();
    for (error, status, code, message, reason, stage) in [
        (
            RuntimeError::ExecutionBackendUnavailable {
                message: PRIVATE_DIAGNOSTIC.into(),
            },
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Workdir session is unavailable",
            "runtime_backend_unavailable",
            "runtime",
        ),
        (
            RuntimeError::StatePoisoned,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Workdir session is unavailable",
            "runtime_state_unavailable",
            "runtime",
        ),
        (
            RuntimeError::InvalidRequest(PRIVATE_DIAGNOSTIC.into()),
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Workdir operation request is invalid",
            "runtime_invalid_request",
            "runtime",
        ),
        (
            RuntimeError::StoreCorrupt {
                operation: "session_open",
                path: PathBuf::from("/private/host-path-marker"),
                message: PRIVATE_DIAGNOSTIC.into(),
            },
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Workdir session is unavailable",
            "session_open_failed",
            "session_open",
        ),
    ] {
        capture.take();
        let response = RuntimeHttpWorkdirError::runtime(error).into_response();
        assert_eq!(response.status(), status);
        let bytes = to_bytes(response.into_body(), 16384).await.unwrap();
        let body = std::str::from_utf8(&bytes).unwrap();
        assert_failure_body(body, code, message, reason);
        assert_failure_log(&capture.take(), status, code, reason, stage);
    }
}
