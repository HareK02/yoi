use super::*;
use std::io::Write;
use tracing::instrument::WithSubscriber;

#[derive(Clone, Default)]
struct CapturedApiLog(Arc<std::sync::Mutex<Vec<u8>>>);

impl Write for CapturedApiLog {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

async fn request_with_logs(app: Router, request: Request<Body>) -> (StatusCode, Value, Vec<Value>) {
    let logs = CapturedApiLog::default();
    let writer = logs.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .json()
        .flatten_event(true)
        .with_writer(move || writer.clone())
        .finish();
    let response = app
        .oneshot(request)
        .with_subscriber(subscriber)
        .await
        .unwrap();
    let status = response.status();
    assert_eq!(response.headers()[CONTENT_TYPE], "application/json");
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap();
    let text = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    for secret in ["query-secret", "bearer-secret", "body-secret"] {
        assert!(!text.contains(secret), "log leaked request data: {text}");
    }
    let events = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|event| event["event"] == "api_error")
        .collect();
    (status, body, events)
}

#[tokio::test]
async fn generated_cancellation_errors_preserve_response_and_log_details() {
    let dir = tempfile::tempdir().unwrap();
    let (api, execution) = test_api_with_recording_backend(dir.path()).await;
    let worker = api
        .runtime
        .spawn_worker(
            EMBEDDED_WORKER_RUNTIME_ID,
            test_create_binding(),
            WorkerSpawnRequest {
                requested_worker_name: Some("logging-coder".to_string()),
                intent: WorkerSpawnIntent::TicketRole {
                    ticket_id: "cancellation-logging".to_string(),
                    role: TicketWorkerRole::Worker,
                },
                singleton_key: None,
                acceptance: WorkerSpawnAcceptanceRequirement::RunAccepted {
                    expected_segments: 0,
                },
                profile: ProfileSelector::Builtin("builtin:coder".to_string()),
                ticket_assignment: None,
                initial_submit: Vec::new(),
                workdir_attachment_requests: Vec::new(),
                resolved_workdir_attachment_requests: Vec::new(),
                resolved_workdir_attachments: Vec::new(),
                resolved_config_bundle: None,
                resolved_worker_observation_enabled: false,
                resolved_worker_observation_grants: Vec::new(),
                resolved_workspace_api: Some(test_worker_workspace_api(EMBEDDED_WORKER_RUNTIME_ID)),
                resolved_memory_settings: Some(test_worker_memory_settings()),
                resolved_subjektiv_attached: false,
                resolved_control_operation: None,
            },
        )
        .unwrap()
        .worker
        .unwrap()
        .worker;
    let worker = RuntimeWorkerRef::new(EMBEDDED_WORKER_RUNTIME_ID, worker.worker_id);
    api.store
        .upsert_worker_registry(&WorkerRegistryRecord {
            workspace_id: TEST_WORKSPACE_ID.to_string(),
            worker: worker.clone(),
            display_name: "Logging Coder".to_string(),
            profile: Some("builtin:coder".to_string()),
            retention_state: "normal".to_string(),
            transcript_ref: None,
            session_ref: None,
            summary_ref: None,
            diagnostics_ref: None,
            created_at: TEST_CREATED_AT.to_string(),
            updated_at: TEST_CREATED_AT.to_string(),
        })
        .unwrap();
    let backend = browser_ticket_backend(&api).unwrap();
    let mut input = ticket::NewTicket::new("Cancellation logging");
    input.workflow_state = Some(TicketWorkflowState::InProgress);
    let ticket = backend.create(input).unwrap();
    api.store
        .set_current_ticket_worker_assignment(
            &TicketWorkerAssignmentRecord {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
                ticket_id: ticket.id.clone(),
                assignment_id: "logging-assignment".to_string(),
                worker,
                assigned_by: "test-user".to_string(),
                assigned_at: TEST_CREATED_AT.to_string(),
            },
            None,
            "logging-assignment-event",
            "logging-assignment-operation",
            false,
        )
        .unwrap();
    *execution.cancel_failure.lock().unwrap() = Some("fixture cancellation rejected".to_string());
    // The real generated route, service conversion, and logging middleware; no live Runtime needed.
    let app = build_inner_router(api.clone());
    let path = format!(
        "/api/w/{TEST_WORKSPACE_ID}/tickets/{}/implementation-cancellations",
        ticket.id
    );
    let request = |reason: &str| {
        Request::builder()
            .method(Method::POST)
            .uri(format!("{path}?access_token=query-secret"))
            .header(CONTENT_TYPE, "application/json")
            .header(axum::http::header::AUTHORIZATION, "Bearer bearer-secret")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "operation_id": "logging-cancel",
                    "assignment_id": "logging-assignment",
                    "reason": reason,
                }))
                .unwrap(),
            ))
            .unwrap()
    };

    let (status, body, events) = request_with_logs(app.clone(), request("body-secret")).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert_eq!(body["error"], "Bad Gateway");
    assert_eq!(
        body["message"],
        "workspace_ticket_implementation_cancel_rejected: Runtime did not cancel the assigned Worker Worker"
    );
    assert!(
        !body["diagnostics"].as_array().unwrap().is_empty(),
        "{body}"
    );
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["method"], "POST");
    assert_eq!(events[0]["path"], path);
    assert_eq!(events[0]["status"], 502);
    assert_eq!(events[0]["message"], body["message"]);
    assert_eq!(events[0]["kind"], body["diagnostics"][0]["code"]);
    let diagnostics = events[0]["diagnostics"].as_str().unwrap();
    assert!(
        diagnostics.contains("fixture cancellation rejected"),
        "{diagnostics}"
    );
    assert_eq!(
        api.authority.ticket(&ticket.id).unwrap().state,
        "inprogress"
    );

    // Errors without diagnostics still carry a useful kind and message.
    let (status, body, events) = request_with_logs(app.clone(), request(&"r".repeat(513))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["diagnostics"], json!([]));
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["kind"], "bad_request");
    assert_eq!(events[0]["message"], body["message"]);
    assert!(!events[0]["message"].as_str().unwrap().is_empty());

    *execution.cancel_failure.lock().unwrap() = None;
    let (status, body, events) = request_with_logs(app, request("body-secret")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["state"], "ready");
    assert!(
        events.is_empty(),
        "successful cancellation logged as failure: {events:?}"
    );
}

#[tokio::test]
async fn legacy_api_errors_keep_their_log_metadata() {
    let app = Router::new()
        .route(
            "/api/legacy-error",
            get(|| async {
                ApiError::from(Error::InvalidInput("fixture invalid input".to_string()))
            }),
        )
        .layer(middleware::from_fn(log_failed_api_response));
    let (status, body, events) = request_with_logs(
        app,
        Request::builder()
            .uri("/api/legacy-error?token=query-secret")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["kind"], "bad_request");
    assert_eq!(events[0]["message"], body["message"]);
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("fixture invalid input")
    );
}
