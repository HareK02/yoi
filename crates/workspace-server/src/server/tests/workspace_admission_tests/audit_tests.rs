//! Integration coverage for signed loopback HTTP -> production admission/auth -> external
//! Workdir session -> real SQLite audit persistence. Only the external provider boundary
//! is substituted with a controlled command receiver. Temporary DBs, listeners and bounded
//! requests are isolated per case; these run in the normal workspace-server library suite.

use super::*;
use std::io::Write;
use tracing::instrument::WithSubscriber;

const AUDIT_SECRET: &str = "audit-store-password-secret";
const WRITE_SECRET: &[u8] = b"write-content-secret";
const PATH_SECRET: &str = "private-request-path-secret.txt";
const PROVIDER_SECRET: &str = "/host/private/provider-secret";

type ProviderReply =
    oneshot::Sender<std::result::Result<WorkdirSessionOperationResult, WorkdirTransportError>>;

#[derive(Clone, Default)]
struct CapturedAuditLog(Arc<std::sync::Mutex<Vec<u8>>>);

impl Write for CapturedAuditLog {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct PendingHttpWrite {
    task: tokio::task::JoinHandle<(StatusCode, Value)>,
    proof: String,
}

impl Drop for PendingHttpWrite {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct ProviderProbe {
    receiver: mpsc::Receiver<ExternalProviderCommand>,
    dispatches: usize,
}

impl ProviderProbe {
    async fn receive_operation(
        &mut self,
        pending: &mut PendingHttpWrite,
    ) -> (String, WorkdirSessionOperation, ProviderReply) {
        let command = tokio::time::timeout(BOUND, async {
            tokio::select! {
                command = self.receiver.recv() => command.expect("provider channel closed"),
                response = &mut pending.task => {
                    panic!("HTTP completed before write dispatch: {:?}", response.unwrap());
                }
            }
        })
        .await
        .expect("signed HTTP write never reached provider");
        let ExternalProviderCommand::Operation {
            operation_id,
            operation,
            response,
        } = command
        else {
            panic!("expected write dispatch, received cancellation")
        };
        if !matches!(operation, WorkdirSessionOperation::AuthorizeScope(_)) {
            self.dispatches += 1;
        }
        (operation_id.as_str().to_owned(), operation, response)
    }

    async fn receive_write_scope(&mut self, pending: &mut PendingHttpWrite) -> ProviderReply {
        // The scoped write path authorizes its admission and its operation path
        // before dispatching the mutation. Keep both provider checks real.
        for check in 0..2 {
            let (_, operation, response) = self.receive_operation(pending).await;
            let WorkdirSessionOperation::AuthorizeScope(request) = operation else {
                panic!("expected provider scope authorization, got {operation:?}")
            };
            assert_eq!(
                request.path,
                workdir::WorkdirPath::new(PATH_SECRET).unwrap()
            );
            assert_eq!(
                request.permission,
                workdir::WorkdirToolScopePermission::Write
            );
            assert_eq!(request.rules.len(), 1);
            assert_eq!(request.rules[0].target, request.path);
            assert_eq!(request.rules[0].permission, request.permission);
            if check == 1 {
                return response;
            }
            response
                .send(Ok(WorkdirSessionOperationResult::AuthorizeScope))
                .unwrap();
        }
        unreachable!()
    }

    async fn receive_write(&mut self, pending: &mut PendingHttpWrite) -> (String, ProviderReply) {
        self.receive_write_scope(pending)
            .await
            .send(Ok(WorkdirSessionOperationResult::AuthorizeScope))
            .unwrap();
        let (operation_id, operation, response) = self.receive_operation(pending).await;
        let WorkdirSessionOperation::Write(request) = operation else {
            panic!("expected mutation, got {operation:?}")
        };
        assert_eq!(
            request.path,
            workdir::WorkdirPath::new(PATH_SECRET).unwrap()
        );
        assert_eq!(request.content, WRITE_SECRET);
        assert_eq!(request.expected_hash, None);
        (operation_id, response)
    }

    fn assert_dispatches(&mut self, expected: usize) {
        while let Ok(command) = self.receiver.try_recv() {
            if matches!(command, ExternalProviderCommand::Operation { .. }) {
                self.dispatches += 1;
            }
        }
        assert_eq!(
            self.dispatches, expected,
            "unexpected mutation dispatch/retry"
        );
    }
}

impl Fixture {
    fn attach_audited_writer(&self) -> ProviderProbe {
        let receiver = self.attach("1");
        // Setup changes only: real signed requests still traverse production authorization.
        // All three capability sources must agree before the session is first resolved.
        let connection = rusqlite::Connection::open(&self.api.config.database_path).unwrap();
        assert_eq!(
            connection
                .execute(
                    "UPDATE external_workdir_grants SET permissions='read_write' \
                     WHERE workspace_id=?1 AND grant_id='external-1'",
                    [TEST_WORKSPACE_ID],
                )
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .execute(
                    "UPDATE worker_workdir_links SET capabilities='read_write' \
                     WHERE workspace_id=?1 AND runtime_id='workdirless-runtime' \
                     AND worker_id='1' AND alias='attachment' AND unlinked_at IS NULL",
                    [TEST_WORKSPACE_ID],
                )
                .unwrap(),
            1
        );
        let mut registry = self.api.external_workdir_providers.lock().unwrap();
        let provider = registry.connections.get_mut("external-1").unwrap();
        Arc::get_mut(provider).unwrap().capabilities =
            workdir::WorkdirSessionCapabilities::READ_WRITE;
        ProviderProbe {
            receiver,
            dispatches: 0,
        }
    }
}

struct AuditHttpFixture {
    fixture: Fixture,
    logs: CapturedAuditLog,
    client: reqwest::Client,
    base_url: String,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for AuditHttpFixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl AuditHttpFixture {
    async fn new() -> (Self, ProviderProbe) {
        let fixture = Fixture::new().await;
        let provider = fixture.attach_audited_writer();
        let logs = CapturedAuditLog::default();
        let writer = logs.clone();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .json()
            .flatten_event(true)
            .with_writer(move || writer.clone())
            .finish();
        let dispatch = tracing::Dispatch::new(subscriber);
        // Axum spawns connection tasks: scope the subscriber around each request rather
        // than relying on the listener task's thread-local subscriber being inherited.
        let app = fixture.app.clone().layer(axum::middleware::from_fn(
            move |request: Request<Body>, next: axum::middleware::Next| {
                let dispatch = dispatch.clone();
                async move { next.run(request).with_subscriber(dispatch).await }
            },
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(BOUND)
            .build()
            .unwrap();
        (
            Self {
                fixture,
                logs,
                client,
                base_url,
                server,
            },
            provider,
        )
    }

    fn fail_audit(&self, before_dispatch: bool) {
        let filter = if before_dispatch { "=" } else { "<>" };
        rusqlite::Connection::open(&self.fixture.api.config.database_path)
            .unwrap()
            .execute_batch(&format!(
                "CREATE TRIGGER fail_operation_audit BEFORE INSERT ON audit_events \
                 WHEN NEW.action='external_workdir_operation' AND NEW.outcome {filter} 'requested' \
                 AND instr(NEW.summary, 'operation_kind=write') > 0 \
                 BEGIN SELECT RAISE(FAIL, '{AUDIT_SECRET}'); END;"
            ))
            .unwrap();
    }

    fn start_write(&self) -> PendingHttpWrite {
        self.start_operation(WorkdirSessionOperation::Write(workdir::WriteRequest {
            path: workdir::WorkdirPath::new(PATH_SECRET).unwrap(),
            content: WRITE_SECRET.to_vec(),
            expected_hash: None,
        }))
    }

    fn start_operation(&self, operation: WorkdirSessionOperation) -> PendingHttpWrite {
        let path = format!("/api/w/{TEST_WORKSPACE_ID}/workers/self/workdir-session/operations");
        let expected_operation = workdir::failure::WorkdirFailureOperation::from(&operation);
        let body = serde_json::to_vec(&WorkspaceWorkdirSessionOperationRequest {
            target_workdir: "attachment".into(),
            operation,
        })
        .unwrap();
        let request = self.fixture.request(
            Some("1"),
            &path,
            body.clone(),
            START,
            worker_runtime::auth::WORKSPACE_REQUEST_PERMISSION,
        );
        let proof = request.headers()[worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER]
            .to_str()
            .unwrap()
            .to_owned();
        let url = format!("{}{path}", self.base_url);
        let client = self.client.clone();
        let headers = request.headers().clone();
        let task = tokio::spawn(async move {
            let response = client
                .post(url)
                .headers(headers)
                .body(body)
                .send()
                .await
                .unwrap();
            let status = response.status();
            let content_type = response.headers()[CONTENT_TYPE]
                .to_str()
                .unwrap()
                .to_owned();
            let bytes = response.bytes().await.unwrap();
            if status.is_success() {
                assert_eq!(content_type, "application/json");
            } else {
                assert_eq!(
                    content_type,
                    error_contract::problem::PROBLEM_CONTENT_TYPE,
                    "{status}: {}",
                    String::from_utf8_lossy(&bytes)
                );
                let decoded = server_api::WorkdirHttpApiError::decode_received(
                    status.as_u16(),
                    &content_type,
                    &bytes,
                )
                .unwrap();
                assert_eq!(decoded.details().unwrap().operation, expected_operation);
                assert_eq!(
                    decoded.legacy().code.unwrap().http_status(),
                    status.as_u16()
                );
            }
            (status, serde_json::from_slice(&bytes).unwrap())
        });
        PendingHttpWrite { task, proof }
    }

    async fn finish(
        &self,
        mut pending: PendingHttpWrite,
        expected_status: StatusCode,
    ) -> (Value, Vec<Value>) {
        let (status, body) = tokio::time::timeout(BOUND, &mut pending.task)
            .await
            .expect("HTTP did not finish; possible automatic retry")
            .unwrap();
        assert_eq!(status, expected_status, "{body}");
        let text = String::from_utf8(self.logs.0.lock().unwrap().clone()).unwrap();
        let encoded_content = serde_json::to_string(WRITE_SECRET).unwrap();
        for output in [body.to_string(), text.clone()] {
            for secret in [
                AUDIT_SECRET,
                std::str::from_utf8(WRITE_SECRET).unwrap(),
                encoded_content.as_str(),
                PATH_SECRET,
                PROVIDER_SECRET,
                PROVIDER_SECRET.trim_start_matches('/'),
                pending.proof.as_str(),
                worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER,
                self.fixture.identity.private_key.as_str(),
                self.fixture.identity.public_key.as_str(),
                self.fixture._root.path().to_str().unwrap(),
            ] {
                assert!(!output.contains(secret), "sensitive data escaped: {output}");
            }
        }
        let events = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        (body, events)
    }

    fn assert_audits(&self, expected: &[(&str, &str)]) {
        self.assert_operation_audits(expected, "write");
    }

    fn assert_operation_audits(&self, expected: &[(&str, &str)], operation: &str) {
        let connection =
            rusqlite::Connection::open(&self.fixture.api.config.database_path).unwrap();
        let mut statement = connection
            .prepare(
                "SELECT request_id, outcome, summary FROM audit_events \
                 WHERE workspace_id=?1 AND action='external_workdir_operation' \
                 AND target_id='external-1' AND instr(summary, ?2) > 0 ORDER BY rowid",
            )
            .unwrap();
        let rows: Vec<(String, String, String)> = statement
            .query_map(
                rusqlite::params![TEST_WORKSPACE_ID, format!("operation_kind={operation}")],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let actual: Vec<(&str, &str)> = rows
            .iter()
            .map(|(operation_id, outcome, _)| (operation_id.as_str(), outcome.as_str()))
            .collect();
        assert_eq!(actual, expected, "persisted audits: {rows:?}");
        for (_, _, summary) in rows {
            assert!(
                summary.contains(&format!("operation_kind={operation}")),
                "{summary}"
            );
            for secret in [
                AUDIT_SECRET,
                PATH_SECRET,
                PROVIDER_SECRET,
                "write-content-secret",
            ] {
                assert!(
                    !summary.contains(secret),
                    "sensitive audit summary: {summary}"
                );
            }
        }
    }

    fn apply_provider_write(&self) {
        std::fs::write(self.effect_path(), WRITE_SECRET).unwrap();
    }

    fn effect_path(&self) -> std::path::PathBuf {
        self.fixture._root.path().join("provider-effect")
    }
}

fn write_result() -> WorkdirSessionOperationResult {
    WorkdirSessionOperationResult::Write(workdir::WriteResult {
        bytes_written: WRITE_SECRET.len(),
        created: true,
    })
}

fn assert_audit_failure(
    body: &Value,
    events: &[Value],
    stage: &str,
    debug_stage: &str,
    outcome: Value,
    debug_outcome: &str,
) -> String {
    let failures = body["audit_failures"]
        .as_array()
        .expect("independent audit receipt");
    assert_eq!(failures.len(), 1, "{body}");
    let operation_id = failures[0]["operation_id"].as_str().unwrap();
    assert!(operation_id.starts_with("op-"), "{body}");
    assert_eq!(
        failures[0],
        json!({
            "operation_id": operation_id,
            "stage": stage,
            "reason": "audit_store_unavailable",
            "operation_outcome": outcome,
        })
    );
    let notifications: Vec<&Value> = events
        .iter()
        .filter(|event| event["event"] == "external_workdir_audit_failed")
        .collect();
    assert_eq!(notifications.len(), 1, "{events:?}");
    let event = notifications[0];
    assert_eq!(event["operation_id"], operation_id);
    assert_eq!(event["operation"], "write");
    assert_eq!(event["audit_stage"], debug_stage);
    assert_eq!(event["audit_reason"], "AuditStoreUnavailable");
    assert_eq!(event["operation_outcome"], debug_outcome);
    operation_id.to_owned()
}

fn assert_failure_reason(events: &[Value], reason: &str, stage: &str) {
    let errors: Vec<&Value> = events
        .iter()
        .filter(|event| event["event"] == "api_error")
        .collect();
    assert_eq!(errors.len(), 1, "{events:?}");
    assert_eq!(errors[0]["failure_reason"], reason, "{events:?}");
    assert_eq!(errors[0]["failure_stage"], stage, "{events:?}");
}

fn assert_operational_failure(
    body: &Value,
    events: &[Value],
    reason: &str,
    stage: &str,
    code: &str,
    message: &str,
) {
    assert_failure_reason(events, reason, stage);
    assert_eq!(
        body["yoi_failure"]["details"]["classification"], code,
        "{body}"
    );
    assert_eq!(
        body["yoi_failure"]["details"]["failure_reason"], reason,
        "{body}"
    );
    assert_eq!(body["yoi_failure"]["reason"], reason, "{body}");
    assert!(body["detail"].as_str().is_some_and(|text| !text.is_empty()));
    let logged = events
        .iter()
        .find(|event| event["event"] == "api_error")
        .unwrap();
    assert_eq!(logged["message"], message);
    for field in ["failure_reason", "failure_stage"] {
        assert!(
            body.get(field).is_none(),
            "internal diagnostic escaped: {body}"
        );
    }
}

#[tokio::test]
async fn successful_write_after_audit_failure_preserves_result_and_does_not_repeat_mutation() {
    let (h, mut provider) = AuditHttpFixture::new().await;
    h.fail_audit(false);
    let mut pending = h.start_write();
    let (dispatched_id, reply) = provider.receive_write(&mut pending).await;
    h.apply_provider_write();
    reply.send(Ok(write_result())).unwrap();
    let (mut body, events) = h.finish(pending, StatusCode::OK).await;
    let receipt_id = assert_audit_failure(
        &body,
        &events,
        "response_received",
        "ResponseReceived",
        json!({"kind":"succeeded"}),
        "Succeeded",
    );
    assert_eq!(receipt_id, dispatched_id);
    body.as_object_mut().unwrap().remove("audit_failures");
    assert_eq!(body, serde_json::to_value(write_result()).unwrap());
    assert_eq!(std::fs::read(h.effect_path()).unwrap(), WRITE_SECRET);
    provider.assert_dispatches(1);
    h.assert_audits(&[(&dispatched_id, "requested")]);
}

#[tokio::test]
async fn denied_write_after_audit_failure_preserves_provider_error_and_separate_receipt() {
    let (h, mut provider) = AuditHttpFixture::new().await;
    h.fail_audit(false);
    let mut pending = h.start_write();
    let (dispatched_id, reply) = provider.receive_write(&mut pending).await;
    let mut original = WorkdirTransportError::from_workdir_error(&workdir::WorkdirError::Denied(
        PROVIDER_SECRET.into(),
    ));
    let expected_message = original.message.clone();
    original.message = PROVIDER_SECRET.into();
    reply.send(Err(original)).unwrap();
    let (body, events) = h.finish(pending, StatusCode::FORBIDDEN).await;
    assert_eq!(body["yoi_failure"]["details"]["classification"], "denied");
    assert_eq!(
        events
            .iter()
            .find(|event| event["event"] == "api_error")
            .unwrap()["message"],
        expected_message
    );
    assert_eq!(body["yoi_failure"]["reason"], "denied");
    let receipt_id = assert_audit_failure(
        &body,
        &events,
        "response_received",
        "ResponseReceived",
        json!({"kind":"failed", "code":"denied"}),
        "Failed { code: Denied }",
    );
    assert_eq!(receipt_id, dispatched_id);
    assert!(!h.effect_path().exists());
    provider.assert_dispatches(1);
    h.assert_audits(&[(&dispatched_id, "requested")]);
}

#[tokio::test]
async fn lost_write_response_after_audit_failure_stays_unknown_without_repeating_effect() {
    let (h, mut provider) = AuditHttpFixture::new().await;
    h.fail_audit(false);
    let mut pending = h.start_write();
    let (dispatched_id, reply) = provider.receive_write(&mut pending).await;
    h.apply_provider_write();
    // The provider applied the effect, but the server cannot infer that from response loss.
    drop(reply);
    let (body, events) = h.finish(pending, StatusCode::INTERNAL_SERVER_ERROR).await;
    assert_operational_failure(
        &body,
        &events,
        "provider_response_lost",
        "provider_response",
        "outcome_unknown",
        "Workdir operation may have effects; inspect before retrying",
    );
    let receipt_id = assert_audit_failure(
        &body,
        &events,
        "response_unknown",
        "ResponseUnknown",
        json!({"kind":"unknown"}),
        "Unknown",
    );
    assert_eq!(receipt_id, dispatched_id);
    assert_eq!(std::fs::read(h.effect_path()).unwrap(), WRITE_SECRET);
    provider.assert_dispatches(1);
    h.assert_audits(&[(&dispatched_id, "requested")]);
}

#[tokio::test]
async fn before_write_audit_failure_is_not_dispatched_and_has_no_provider_effect() {
    let (h, mut provider) = AuditHttpFixture::new().await;
    h.fail_audit(true);
    let mut pending = h.start_write();
    provider
        .receive_write_scope(&mut pending)
        .await
        .send(Ok(WorkdirSessionOperationResult::AuthorizeScope))
        .unwrap();
    let (body, events) = h.finish(pending, StatusCode::SERVICE_UNAVAILABLE).await;
    assert_eq!(
        body["yoi_failure"]["details"]["classification"],
        "not_dispatched"
    );
    assert_audit_failure(
        &body,
        &events,
        "before_dispatch",
        "BeforeDispatch",
        json!({"kind":"not_dispatched"}),
        "NotDispatched",
    );
    assert_failure_reason(&events, "audit_store_unavailable", "audit");
    assert!(
        body.get("failure_reason").is_none(),
        "internal reason leaked: {body}"
    );
    assert!(!h.effect_path().exists());
    provider.assert_dispatches(0);
    h.assert_audits(&[]);
}

#[tokio::test]
async fn rejected_write_send_with_audit_failure_is_not_dispatched_not_unknown() {
    let (h, mut provider) = AuditHttpFixture::new().await;
    h.fail_audit(false);
    let mut pending = h.start_write();
    let scope_reply = provider.receive_write_scope(&mut pending).await;
    provider.receiver.close();
    scope_reply
        .send(Ok(WorkdirSessionOperationResult::AuthorizeScope))
        .unwrap();
    let (body, events) = h.finish(pending, StatusCode::SERVICE_UNAVAILABLE).await;
    assert_eq!(
        body["yoi_failure"]["details"]["classification"],
        "not_dispatched"
    );
    let receipt_id = assert_audit_failure(
        &body,
        &events,
        "dispatch_rejected",
        "DispatchRejected",
        json!({"kind":"not_dispatched"}),
        "NotDispatched",
    );
    assert_operational_failure(
        &body,
        &events,
        "provider_dispatch_rejected",
        "provider_dispatch",
        "not_dispatched",
        "Workdir operation was not dispatched",
    );
    assert!(!h.effect_path().exists());
    provider.assert_dispatches(0);
    h.assert_audits(&[(&receipt_id, "requested")]);
}

#[tokio::test]
async fn expired_connection_grant_is_not_dispatched_and_logs_admission_reason() {
    let (h, mut provider) = AuditHttpFixture::new().await;
    {
        let mut registry = h.fixture.api.external_workdir_providers.lock().unwrap();
        let connection = registry.connections.get_mut("external-1").unwrap();
        // Keep the persisted grant online so the real handler reaches operate's
        // connection-expiry fence rather than the separate session-open fence.
        Arc::get_mut(connection).unwrap().expires_at =
            Some(Utc::now() - chrono::Duration::seconds(1));
    }
    h.fail_audit(false);
    let (body, events) = h
        .finish(h.start_write(), StatusCode::SERVICE_UNAVAILABLE)
        .await;
    assert_operational_failure(
        &body,
        &events,
        "provider_grant_expired",
        "provider_admission",
        "unavailable",
        "Workdir session is unavailable",
    );
    assert!(body.get("audit_failures").is_none(), "{body}");
    assert!(
        events
            .iter()
            .all(|event| event["event"] != "external_workdir_audit_failed")
    );
    assert!(!h.effect_path().exists());
    provider.assert_dispatches(0);
    h.assert_audits(&[]);
}

#[tokio::test]
async fn closed_provider_admission_is_not_dispatched_and_logs_admission_reason() {
    let (h, mut provider) = AuditHttpFixture::new().await;
    h.fixture
        .api
        .external_workdir_providers
        .lock()
        .unwrap()
        .connection("external-1")
        .unwrap()
        .admission
        .close();
    h.fail_audit(false);
    let (body, events) = h
        .finish(h.start_write(), StatusCode::SERVICE_UNAVAILABLE)
        .await;
    assert_operational_failure(
        &body,
        &events,
        "provider_admission_closed",
        "provider_admission",
        "unavailable",
        "Workdir session is unavailable",
    );
    assert!(body.get("audit_failures").is_none(), "{body}");
    assert!(
        events
            .iter()
            .all(|event| event["event"] != "external_workdir_audit_failed")
    );
    assert!(!h.effect_path().exists());
    provider.assert_dispatches(0);
    h.assert_audits(&[]);
}

#[tokio::test]
async fn timed_out_write_after_audit_failure_stays_unknown_and_logs_timeout_reason() {
    let (mut h, mut provider) = AuditHttpFixture::new().await;
    // Keep the HTTP client's independent deadline beyond the unchanged 30s
    // provider timeout. Pause only after the signed HTTP request was dispatched,
    // avoiding automatic virtual-time advancement during socket/SQLite setup.
    h.client = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(40))
        .build()
        .unwrap();
    h.fail_audit(false);
    let mut pending = h.start_write();
    let (dispatched_id, reply) = provider.receive_write(&mut pending).await;
    h.apply_provider_write();
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(31)).await;
    tokio::time::resume();
    let (body, events) = h.finish(pending, StatusCode::INTERNAL_SERVER_ERROR).await;
    assert_operational_failure(
        &body,
        &events,
        "provider_operation_timeout",
        "provider_response",
        "outcome_unknown",
        "Workdir operation may have effects; inspect before retrying",
    );
    let receipt_id = assert_audit_failure(
        &body,
        &events,
        "response_unknown",
        "ResponseUnknown",
        json!({"kind":"unknown"}),
        "Unknown",
    );
    assert_eq!(receipt_id, dispatched_id);
    assert!(reply.is_closed(), "timeout retained its response receiver");
    assert!(matches!(
        provider.receiver.try_recv().unwrap(),
        ExternalProviderCommand::Cancel { operation_id }
            if operation_id.as_str() == dispatched_id.as_str()
    ));
    assert_eq!(std::fs::read(h.effect_path()).unwrap(), WRITE_SECRET);
    provider.assert_dispatches(1);
    h.assert_audits(&[(&dispatched_id, "requested")]);
}

#[tokio::test]
async fn successful_write_persists_requested_and_completed_with_same_operation_id() {
    let (h, mut provider) = AuditHttpFixture::new().await;
    let mut pending = h.start_write();
    let (operation_id, reply) = provider.receive_write(&mut pending).await;
    h.apply_provider_write();
    reply.send(Ok(write_result())).unwrap();
    let (body, events) = h.finish(pending, StatusCode::OK).await;
    assert_eq!(body, serde_json::to_value(write_result()).unwrap());
    assert!(
        events
            .iter()
            .all(|event| event["event"] != "external_workdir_audit_failed")
    );
    assert_eq!(std::fs::read(h.effect_path()).unwrap(), WRITE_SECRET);
    provider.assert_dispatches(1);
    h.assert_audits(&[(&operation_id, "requested"), (&operation_id, "completed")]);
}

#[tokio::test]
async fn provider_reported_unknown_preserves_received_response_and_unknown_effects() {
    let (h, mut provider) = AuditHttpFixture::new().await;
    h.fail_audit(false);
    let mut pending = h.start_write();
    let (dispatched_id, reply) = provider.receive_write(&mut pending).await;
    reply
        .send(Err(WorkdirTransportError::from_workdir_error(
            &workdir::WorkdirError::OutcomeUnknown(PROVIDER_SECRET.into()),
        )))
        .unwrap();
    let (body, events) = h.finish(pending, StatusCode::INTERNAL_SERVER_ERROR).await;
    assert_eq!(
        body["yoi_failure"]["details"]["classification"],
        "outcome_unknown"
    );
    let receipt_id = assert_audit_failure(
        &body,
        &events,
        "response_received",
        "ResponseReceived",
        json!({"kind":"unknown"}),
        "Unknown",
    );
    assert_eq!(receipt_id, dispatched_id);
    provider.assert_dispatches(1);
    h.assert_audits(&[(&dispatched_id, "requested")]);
}

// Checkout has no scoped Host resource mapping yet. The public signed route
// must fail before provider/audit dispatch even with filesystem write authority.
// Provider reply normalization itself is covered by the external-provider boundary
// suite; it must not be tested here by bypassing this authorization refusal.
#[tokio::test]
async fn unmapped_checkout_refuses_before_external_dispatch_or_audit() {
    let f = Fixture::new().await;
    let mut provider = f.attach_audited_writer();
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/workers/self/workdir-session/operations");
    let operation = WorkdirSessionOperation::CheckoutExecute(workdir::CheckoutRequest {
        target: workdir::WorkdirPath::new(PATH_SECRET).unwrap(),
        validator: vec![1],
        operation: workdir::CheckoutOperation::Write {
            content: WRITE_SECRET.to_vec(),
            expected_hash: [0; 32],
        },
    });
    let body = serde_json::to_vec(&WorkspaceWorkdirSessionOperationRequest {
        target_workdir: "attachment".into(),
        operation,
    })
    .unwrap();
    let reply = f
        .app
        .clone()
        .oneshot(f.request(
            Some("1"),
            &path,
            body,
            START,
            worker_runtime::auth::WORKSPACE_REQUEST_PERMISSION,
        ))
        .await
        .unwrap();
    let body = assert_json(reply, StatusCode::FORBIDDEN).await;
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("no scoped Subject resource mapping"),
        "{body}"
    );
    provider.assert_dispatches(0);
    let connection = rusqlite::Connection::open(&f.api.config.database_path).unwrap();
    let count: i64 = connection
        .query_row(
            "SELECT count(*) FROM audit_events WHERE action='external_workdir_operation'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}
