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
    async fn receive_write(&mut self, pending: &mut PendingHttpWrite) -> (String, ProviderReply) {
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
        self.dispatches += 1;
        let WorkdirSessionOperation::Write(request) = operation else {
            panic!("expected mutation, got {operation:?}")
        };
        assert_eq!(
            request.path,
            workdir::WorkdirPath::new(PATH_SECRET).unwrap()
        );
        assert_eq!(request.content, WRITE_SECRET);
        assert_eq!(request.expected_hash, None);
        (operation_id.as_str().to_owned(), response)
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
                 BEGIN SELECT RAISE(FAIL, '{AUDIT_SECRET}'); END;"
            ))
            .unwrap();
    }

    fn start_write(&self) -> PendingHttpWrite {
        let path = format!("/api/w/{TEST_WORKSPACE_ID}/workers/self/workdir-session/operations");
        let body = serde_json::to_vec(&WorkspaceWorkdirSessionOperationRequest {
            target_workdir: "attachment".into(),
            operation: WorkdirSessionOperation::Write(workdir::WriteRequest {
                path: workdir::WorkdirPath::new(PATH_SECRET).unwrap(),
                content: WRITE_SECRET.to_vec(),
                expected_hash: None,
            }),
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
            assert_eq!(response.headers()[CONTENT_TYPE], "application/json");
            let bytes = response.bytes().await.unwrap();
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
                pending.proof.as_str(),
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
        let connection =
            rusqlite::Connection::open(&self.fixture.api.config.database_path).unwrap();
        let mut statement = connection
            .prepare(
                "SELECT request_id, outcome, summary FROM audit_events \
                 WHERE workspace_id=?1 AND action='external_workdir_operation' \
                 AND target_id='external-1' ORDER BY rowid",
            )
            .unwrap();
        let rows: Vec<(String, String, String)> = statement
            .query_map([TEST_WORKSPACE_ID], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let actual: Vec<(&str, &str)> = rows
            .iter()
            .map(|(operation_id, outcome, _)| (operation_id.as_str(), outcome.as_str()))
            .collect();
        assert_eq!(actual, expected, "persisted audits: {rows:?}");
        for (_, _, summary) in rows {
            assert!(summary.contains("operation_kind=write"), "{summary}");
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

fn assert_failure_reason(events: &[Value], reason: &str) {
    let errors: Vec<&Value> = events
        .iter()
        .filter(|event| event["event"] == "api_error")
        .collect();
    assert_eq!(errors.len(), 1, "{events:?}");
    assert_eq!(errors[0]["failure_reason"], reason, "{events:?}");
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
    assert_eq!(body["code"], "denied");
    assert_eq!(body["message"], expected_message);
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
    assert_eq!(body["code"], "outcome_unknown");
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
    let (body, events) = h
        .finish(h.start_write(), StatusCode::SERVICE_UNAVAILABLE)
        .await;
    assert_eq!(body["code"], "not_dispatched");
    assert_audit_failure(
        &body,
        &events,
        "before_dispatch",
        "BeforeDispatch",
        json!({"kind":"not_dispatched"}),
        "NotDispatched",
    );
    assert_failure_reason(&events, "audit_store_unavailable");
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
    provider.receiver.close();
    let (body, events) = h
        .finish(h.start_write(), StatusCode::SERVICE_UNAVAILABLE)
        .await;
    assert_eq!(body["code"], "not_dispatched");
    let receipt_id = assert_audit_failure(
        &body,
        &events,
        "dispatch_rejected",
        "DispatchRejected",
        json!({"kind":"not_dispatched"}),
        "NotDispatched",
    );
    assert_failure_reason(&events, "provider_unavailable");
    assert!(!h.effect_path().exists());
    provider.assert_dispatches(0);
    h.assert_audits(&[(&receipt_id, "requested")]);
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
    assert_eq!(body["code"], "outcome_unknown");
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
