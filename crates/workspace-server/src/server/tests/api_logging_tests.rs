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
    let token_hash = request
        .headers()
        .get(worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|proof| worker_runtime::auth::decode_runtime_request_source_claims(proof).ok())
        .map(|claims| crate::worker_source::diagnostic_id_hash(&claims.jti));
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
    let content_type = response.headers()[CONTENT_TYPE].clone();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        content_type,
        "application/json",
        "status={status}, body={}",
        String::from_utf8_lossy(&body)
    );
    let body: Value = serde_json::from_slice(&body).unwrap();
    let text = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    for secret in [
        "query-secret",
        "bearer-secret",
        "body-secret",
        "/private/provider/path",
        "/host/private",
        "proof-secret",
        "credential-secret",
        "key-secret",
    ] {
        assert!(!text.contains(secret), "log leaked request data: {text}");
    }
    let events: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|event| event["event"] == "api_error")
        .collect();
    for event in &events {
        if event.get("operation").is_some() && token_hash.is_some() {
            assert_eq!(
                event["request_token_id_hash"],
                serde_json::to_value(&token_hash).unwrap()
            );
        }
    }
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

#[tokio::test]
async fn generated_workdir_provider_errors_preserve_response_and_log_details() {
    use workdir::http::WorkdirTransportErrorCode as Code;

    let mut fixture = manual_worker_assignment_fixture().await;
    let identity =
        worker_runtime::auth::RuntimeIdentityMaterial::generate(&fixture.worker.runtime_id)
            .unwrap();
    configure_runtime_request_auth(&mut fixture.api, &identity, &fixture.worker.runtime_id);
    let (sender, mut commands) = tokio::sync::mpsc::channel(1);
    let session = Arc::new(ExternalProviderWorkdirSession::new(
        Arc::new(ExternalProviderConnection {
            grant_id: "logging-grant".to_string(),
            workdir_id: fixture.main_workdir_id.clone(),
            provider_instance_id: "logging-provider".to_string(),
            generation: 1,
            expires_at: None,
            capabilities: workdir::WorkdirSessionCapabilities::ALL,
            admission: Arc::new(tokio::sync::Semaphore::new(1)),
            shutdown_confirmed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            sender,
        }),
        None,
    ));
    // Seed an already registered command, without launching a real process.
    let provider_handle = CommandHandle("logging-command".to_string());
    session
        .active_commands
        .lock()
        .await
        .insert(provider_handle.0.clone(), provider_handle.clone());
    let handle = fixture
        .api
        .workdir_sessions
        .lock()
        .unwrap()
        .register_command(
            fixture.worker.clone(),
            "checkout".to_string(),
            session,
            provider_handle,
        );
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/workers/self/workdir-session/operations");
    let request = || {
        runtime_source_request(
            &identity,
            Some(&fixture.worker.worker_id),
            "POST",
            &format!("{path}?token=query-secret"),
            serde_json::to_vec(&server_api::CurrentWorkerWorkdirOperationRequest {
                target_workdir: "checkout".to_string(),
                operation: WorkdirSessionOperation::CommandStatus(handle.clone()),
            })
            .unwrap(),
        )
    };
    let app = build_router(fixture.api.clone());
    for (code, status, message, reason) in [
        (Code::Denied, 403, "Workdir operation was denied", None),
        (
            Code::Denied,
            403,
            "Workdir operation was denied",
            Some(workdir::WorkdirDenialReason::OsPermissionDenied),
        ),
        (
            Code::Denied,
            403,
            "Workdir operation was denied",
            Some(workdir::WorkdirDenialReason::ReadOnlySession),
        ),
        (
            Code::OutOfScope,
            403,
            "Workdir path is out of scope",
            Some(workdir::WorkdirDenialReason::PathOutOfScope),
        ),
        (
            Code::SymlinkOutOfScope,
            403,
            "Workdir symlink target is out of scope",
            Some(workdir::WorkdirDenialReason::SymlinkTargetOutOfScope),
        ),
        (
            Code::ReadOnly,
            403,
            "Workdir path is read-only",
            Some(workdir::WorkdirDenialReason::PathReadOnly),
        ),
        (
            Code::UnknownCommand,
            404,
            "Workdir command was not found",
            None,
        ),
        (
            Code::Unavailable,
            503,
            "Workdir session is unavailable",
            None,
        ),
    ] {
        let provider = async {
            let Some(ExternalProviderCommand::Operation {
                operation,
                response,
                ..
            }) = commands.recv().await
            else {
                panic!("expected provider operation");
            };
            assert!(matches!(
                operation,
                WorkdirSessionOperation::CommandStatus(_)
            ));
            response
                .send(Err(WorkdirTransportError {
                    denial_reason: reason,
                    code,
                    message: "body-secret /private/provider/path".to_string(),
                }))
                .unwrap();
        };
        let ((actual_status, body, events), ()) =
            tokio::join!(request_with_logs(app.clone(), request()), provider,);
        assert_eq!(actual_status.as_u16(), status);
        assert_eq!(body, json!({ "code": code.as_str(), "message": message }));
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(
            events[0]["kind"],
            format!("workdir_session_operation_{}", code.as_str())
        );
        assert_eq!(events[0]["message"], message);
        assert_eq!(events[0]["diagnostics"], "[]");
        assert_eq!(events[0]["status"], status);
        assert_eq!(events[0]["method"], "POST");
        assert_eq!(events[0]["path"], path);
        assert_eq!(events[0]["operation"], "command_status");
        assert_eq!(events[0]["workdir_stage"], "provider_dispatch");
        assert_workdir_log_identity(
            &events[0],
            &fixture,
            "checkout",
            Some(&fixture.main_workdir_id),
        );
        assert_eq!(
            events[0]["denial_reason"],
            serde_json::to_value(reason).unwrap()
        );
        assert_eq!(
            events[0]["request_token_id_hash"].as_str().unwrap().len(),
            64
        );
    }
    let provider = async {
        let Some(ExternalProviderCommand::Operation { response, .. }) = commands.recv().await
        else {
            panic!("expected provider operation");
        };
        response
            .send(Ok(WorkdirSessionOperationResult::CommandStatus(
                workdir::CommandStatus::Running,
            )))
            .unwrap();
    };
    let ((status, _, events), ()) = tokio::join!(request_with_logs(app, request()), provider);
    assert_eq!(status, StatusCode::OK);
    assert!(
        events.is_empty(),
        "successful operation logged as failure: {events:?}"
    );
}

#[tokio::test]
async fn generated_workdir_api_errors_preserve_response_and_log_details() {
    let mut fixture = manual_worker_assignment_fixture().await;
    let identity =
        worker_runtime::auth::RuntimeIdentityMaterial::generate(&fixture.worker.runtime_id)
            .unwrap();
    configure_runtime_request_auth(&mut fixture.api, &identity, &fixture.worker.runtime_id);
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/workers/self/workdir-session/operations");
    let body = serde_json::to_vec(&server_api::CurrentWorkerWorkdirOperationRequest {
        target_workdir: "checkout".to_string(),
        operation: WorkdirSessionOperation::CommandStatus(CommandHandle("body-secret".to_string())),
    })
    .unwrap();
    // Valid Runtime proof, but no Worker identity: rejection is inside the generated service.
    let request = runtime_source_request(
        &identity,
        None,
        "POST",
        &format!("{path}?token=query-secret"),
        body,
    );
    let (status, body, events) =
        request_with_logs(build_router(fixture.api.clone()), request).await;
    let expected = ApiError::from(Error::WorkspacePermissionDenied(
        "self Workdir requests require a Runtime-bound Worker identity".to_string(),
    ))
    .into_repository_api_error();
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body, serde_json::to_value(expected).unwrap());
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["kind"], "workdir_session_operation_api_403");
    assert_eq!(events[0]["message"], "Workdir operation failed");
    assert_eq!(events[0]["operation"], "command_status");
    assert_eq!(events[0]["workdir_stage"], "worker_identity");
    assert_eq!(events[0]["runtime_id_hash"], Value::Null);
    assert_eq!(events[0]["worker_id_hash"], Value::Null);
    assert_eq!(events[0]["workdir_id_hash"], Value::Null);
    assert_eq!(events[0]["diagnostics"], "[]");
    assert_eq!(events[0]["path"], path);
}

fn assert_workdir_log_identity(
    event: &Value,
    fixture: &ManualCoderAssignmentFixture,
    alias: &str,
    workdir_id: Option<&str>,
) {
    let hash = crate::worker_source::diagnostic_id_hash;
    assert_eq!(event["workspace_id_hash"], hash(TEST_WORKSPACE_ID));
    assert_eq!(event["runtime_id_hash"], hash(&fixture.worker.runtime_id));
    assert_eq!(event["worker_id_hash"], hash(&fixture.worker.worker_id));
    assert_eq!(event["attachment_alias_hash"], hash(alias));
    assert_eq!(
        event["workdir_id_hash"],
        serde_json::to_value(workdir_id.map(hash)).unwrap()
    );
}

#[tokio::test]
async fn self_workdir_provider_refusals_log_operation_identity_without_request_text() {
    use workdir::{WorkdirDenialReason as Reason, WorkdirSessionCapabilities as Caps};
    let mut fixture = manual_worker_assignment_fixture().await;
    let root = tempfile::tempdir().unwrap();
    let provider_root = root.path().to_path_buf();
    // Fresh sessions exercise the real opening/broker/dispatch path on each request.
    fixture.runtime.workdir_session_factory = Some(Arc::new(move |id| {
        Ok(Arc::new(workdir::LocalWorkdirSession::materialized_bound(
            workdir::Workdir::new(id),
            provider_root.clone(),
            provider_root.clone(),
            manifest::SharedScope::new(manifest::Scope::writable(&provider_root).unwrap()),
            Caps::READ_ONLY,
        )) as WorkdirSessionHandle)
    }));
    fixture
        .api
        .runtime
        .register_or_replace(fixture.runtime.clone());
    let identity =
        worker_runtime::auth::RuntimeIdentityMaterial::generate(&fixture.worker.runtime_id)
            .unwrap();
    configure_runtime_request_auth(&mut fixture.api, &identity, &fixture.worker.runtime_id);
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/workers/self/workdir-session/operations");
    let app = build_router(fixture.api.clone());
    let secret = format!(
        "body-secret credential-secret key-secret proof-secret{}",
        "z".repeat(20_000)
    );
    let cases = [
        (
            json!({"operation": "read", "request": {"path": "/host/private/secret.txt", "offset": 0, "limit": 10, "max_bytes": 1024}}),
            "read",
            "out_of_scope",
            Reason::PathOutOfScope,
        ),
        (
            json!({"operation": "write", "request": {"path": "secret.txt", "content": secret.as_bytes()}}),
            "write",
            "denied",
            Reason::ScopedCapabilityDenied,
        ),
        (
            json!({"operation": "command_start", "request": {"command": secret, "timeout_secs": 1, "output_limit": 1024, "cwd": "", "tool_call_id": secret}}),
            "command_start",
            "denied",
            Reason::ScopedCapabilityDenied,
        ),
        (
            // Scoped forwarding validates this READ_ONLY parent's capabilities
            // before provider resolution. Local now supports scope resolution;
            // denying write rules is not a missing-resolver failure.
            json!({"operation": "authorize_scope", "request": {"path": "secret.txt", "permission": "write", "rules": [{"target": "", "permission": "write", "recursive": true}]}}),
            "authorize_scope",
            "denied",
            Reason::ParentCapabilityDenied,
        ),
    ];
    for (operation, label, code, reason) in cases {
        let _: WorkdirSessionOperation =
            serde_json::from_value(operation.clone()).expect("valid operation fixture");
        let request = runtime_source_request(
            &identity,
            Some(&fixture.worker.worker_id),
            "POST",
            &format!("{path}?token=query-secret"),
            serde_json::to_vec(&json!({"target_workdir": "checkout", "operation": operation}))
                .unwrap(),
        );
        let (status, body, events) = request_with_logs(app.clone(), request).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert_eq!(body["code"], code, "{body}");
        assert!(
            body.get("denial_reason").is_none(),
            "internal diagnostics leaked: {body}"
        );
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0]["operation"], label);
        assert_eq!(events[0]["workdir_stage"], "provider_dispatch");
        assert_eq!(
            events[0]["denial_reason"],
            serde_json::to_value(reason).unwrap()
        );
        assert_workdir_log_identity(
            &events[0],
            &fixture,
            "checkout",
            Some(&fixture.main_workdir_id),
        );
        assert_eq!(
            events[0]["request_token_id_hash"].as_str().unwrap().len(),
            64
        );
        assert!(
            events[0].to_string().len() < 2000,
            "unbounded log: {events:?}"
        );
    }
    // Control: the same READ_ONLY provider can authorize read rules through
    // scoped forwarding and native resolution. Keep a capability refusal
    // distinct from an unavailable scope resolver; successful authorization
    // must not emit an api_error or mutate the filesystem.
    let read_scope = runtime_source_request(
        &identity,
        Some(&fixture.worker.worker_id),
        "POST",
        &format!("{path}?token=query-secret"),
        serde_json::to_vec(&json!({
            "target_workdir": "checkout",
            "operation": {"operation": "authorize_scope", "request": {
                "path": "", "permission": "read",
                "rules": [{"target": "", "permission": "read", "recursive": true}],
            }},
        }))
        .unwrap(),
    );
    let (status, body, events) = request_with_logs(app, read_scope).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        serde_json::to_value(WorkdirSessionOperationResult::AuthorizeScope).unwrap(),
    );
    assert!(
        events.is_empty(),
        "successful scope authorization logged as failure: {events:?}"
    );
    assert!(
        !root.path().join("secret.txt").exists(),
        "refusal changed filesystem"
    );
}

#[tokio::test]
async fn self_workdir_attachment_rejection_hashes_oversized_alias_and_omits_unknown_workdir() {
    let mut fixture = manual_worker_assignment_fixture().await;
    let identity =
        worker_runtime::auth::RuntimeIdentityMaterial::generate(&fixture.worker.runtime_id)
            .unwrap();
    configure_runtime_request_auth(&mut fixture.api, &identity, &fixture.worker.runtime_id);
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/workers/self/workdir-session/operations");
    let alias = format!(
        "body-secret /host/private credential-secret key-secret proof-secret{}",
        "あ".repeat(10_000)
    );
    let request = runtime_source_request(
        &identity,
        Some(&fixture.worker.worker_id),
        "POST",
        &path,
        serde_json::to_vec(&json!({
            "target_workdir": alias,
            "operation": {"operation": "command_status", "request": "body-secret"}
        }))
        .unwrap(),
    );
    let (status, body, events) =
        request_with_logs(build_router(fixture.api.clone()), request).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["workdir_stage"], "attachment_validation");
    assert_eq!(events[0]["operation"], "command_status");
    assert_workdir_log_identity(&events[0], &fixture, &alias, None);
    assert_eq!(events[0]["denial_reason"], Value::Null);
    assert!(
        events[0].to_string().len() < 2000,
        "unbounded log: {events:?}"
    );
}

#[tokio::test]
async fn self_workdir_session_open_refusals_keep_reason_before_registry_conversion() {
    use workdir::WorkdirDenialReason as Reason;
    for (reason, os_error) in [
        (Reason::ReadOnlySession, false),
        (Reason::OsPermissionDenied, true),
    ] {
        let mut fixture = manual_worker_assignment_fixture().await;
        fixture.runtime.workdir_session_factory = Some(Arc::new(move |_| {
            Err(if os_error {
                workdir::WorkdirError::Io {
                    path: PathBuf::from("/host/private/key-secret"),
                    source: std::io::Error::from_raw_os_error(13),
                }
            } else {
                workdir::WorkdirError::denied(reason, "body-secret credential-secret /host/private")
            })
        }));
        fixture
            .api
            .runtime
            .register_or_replace(fixture.runtime.clone());
        let identity =
            worker_runtime::auth::RuntimeIdentityMaterial::generate(&fixture.worker.runtime_id)
                .unwrap();
        configure_runtime_request_auth(&mut fixture.api, &identity, &fixture.worker.runtime_id);
        let path = format!("/api/w/{TEST_WORKSPACE_ID}/workers/self/workdir-session/operations");
        let request = runtime_source_request(&identity, Some(&fixture.worker.worker_id), "POST", &path, serde_json::to_vec(&json!({
            "target_workdir": "checkout",
            "operation": {"operation": "read", "request": {"path": "README.md", "offset": 0, "limit": 10, "max_bytes": 1024}}
        })).unwrap());
        let (status, body, events) =
            request_with_logs(build_router(fixture.api.clone()), request).await;
        // Opening retained its existing Registry HTTP meaning, not an operation's 403.
        assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
        assert_eq!(
            body["message"],
            "workdir_session_open_failed: Workdir operation was denied"
        );
        assert!(body.get("denial_reason").is_none(), "{body}");
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0]["workdir_stage"], "session_open");
        assert_eq!(events[0]["operation"], "read");
        assert_eq!(
            events[0]["denial_reason"],
            serde_json::to_value(reason).unwrap()
        );
        assert_workdir_log_identity(
            &events[0],
            &fixture,
            "checkout",
            Some(&fixture.main_workdir_id),
        );
        assert!(!body.to_string().contains("body-secret"));
        assert!(!body.to_string().contains("/host/private"));
    }
}

#[tokio::test]
async fn self_workdir_post_start_registration_failure_does_not_claim_pre_dispatch_refusal() {
    let mut fixture = manual_worker_assignment_fixture().await;
    let identity =
        worker_runtime::auth::RuntimeIdentityMaterial::generate(&fixture.worker.runtime_id)
            .unwrap();
    configure_runtime_request_auth(&mut fixture.api, &identity, &fixture.worker.runtime_id);
    let (sender, mut commands) = tokio::sync::mpsc::channel(1);
    let connection = Arc::new(ExternalProviderConnection {
        grant_id: "logging-grant".to_string(),
        workdir_id: fixture.main_workdir_id.clone(),
        provider_instance_id: "logging-provider".to_string(),
        generation: 1,
        expires_at: None,
        capabilities: workdir::WorkdirSessionCapabilities::ALL,
        admission: Arc::new(tokio::sync::Semaphore::new(1)),
        shutdown_confirmed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        sender,
    });
    fixture.runtime.workdir_session_factory = Some(Arc::new(move |_| {
        Ok(Arc::new(ExternalProviderWorkdirSession::new(
            connection.clone(),
            None,
        )) as WorkdirSessionHandle)
    }));
    fixture
        .api
        .runtime
        .register_or_replace(fixture.runtime.clone());
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/workers/self/workdir-session/operations");
    let request = runtime_source_request(
        &identity,
        Some(&fixture.worker.worker_id),
        "POST",
        &path,
        serde_json::to_vec(&json!({
            "target_workdir": "checkout",
            "operation": {"operation": "command_start", "request": {
                "command": "body-secret", "timeout_secs": 1, "output_limit": 1024,
                "cwd": "", "spill_dir": null, "tool_call_id": "body-secret"
            }}
        }))
        .unwrap(),
    );
    // Observe command dispatch, then lose only the fixture registry entry before
    // completing start. No OS process is launched, no sleep or retry is needed.
    let provider = async {
        let Some(ExternalProviderCommand::Operation {
            operation,
            response,
            ..
        }) = commands.recv().await
        else {
            panic!("expected provider start operation");
        };
        assert!(matches!(
            operation,
            WorkdirSessionOperation::CommandStart(_)
        ));
        fixture
            .api
            .workdir_sessions
            .lock()
            .unwrap()
            .remove_attachment(&fixture.worker)
            .unwrap();
        response
            .send(Ok(WorkdirSessionOperationResult::CommandStart(
                CommandHandle("fixture-provider-handle".into()),
            )))
            .unwrap();
    };
    let ((status, body, events), ()) = tokio::join!(
        request_with_logs(build_router(fixture.api.clone()), request),
        provider
    );
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .starts_with("workdir_session_registration_failed:")
    );
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["operation"], "command_start");
    assert_eq!(events[0]["workdir_stage"], "command_registration");
    assert_eq!(events[0]["message"], "Workdir operation failed");
    assert_eq!(events[0]["denial_reason"], Value::Null);
    assert_workdir_log_identity(
        &events[0],
        &fixture,
        "checkout",
        Some(&fixture.main_workdir_id),
    );
    assert!(
        commands.try_recv().is_err(),
        "post-dispatch failure was retried"
    );
}
