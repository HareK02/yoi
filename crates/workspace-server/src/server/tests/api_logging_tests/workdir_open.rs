//! Signed operations -> real attachment/grant SQLite records -> session opening
//! -> API error logging. Provider connections use an unconsumed channel: rejected
//! opens must not dispatch commands or reads. No live external provider is used.
use super::*;

#[tokio::test]
async fn external_session_open_logs_distinguish_unavailable_and_mismatched_connections() {
    for (state, mismatch, code) in [
        ("pending", None, "external_workdir_provider_pending"),
        ("offline", None, "external_workdir_provider_offline"),
        (
            "online",
            None,
            "external_workdir_provider_connection_missing",
        ),
        (
            "online",
            Some("generation"),
            "external_workdir_provider_identity_mismatch",
        ),
        (
            "online",
            Some("provider"),
            "external_workdir_provider_identity_mismatch",
        ),
        (
            "online",
            Some("workdir"),
            "external_workdir_provider_identity_mismatch",
        ),
    ] {
        let mut fixture = manual_worker_assignment_fixture().await;
        let identity = RuntimeIdentityMaterial::generate(&fixture.worker.runtime_id).unwrap();
        configure_runtime_request_auth(&mut fixture.api, &identity, &fixture.worker.runtime_id);
        let grant = ExternalWorkdirGrantRecord {
            grant_id: "logging-external-grant".into(),
            workspace_id: TEST_WORKSPACE_ID.into(),
            workdir_id: "logging-external-workdir".into(),
            provider_instance_id: "logging-provider".into(),
            display_name: "body-secret".into(),
            permissions: "read_write_command".into(),
            created_by: test_browser_request_actor().account_id,
            created_at: TEST_CREATED_AT.into(),
            expires_at: None,
            generation: 1,
            status: state.into(),
            updated_at: TEST_CREATED_AT.into(),
        };
        let mut record = fixture
            .api
            .store
            .list_workdir_registry(TEST_WORKSPACE_ID, 100)
            .unwrap()
            .into_iter()
            .find(|record| record.workdir_id == fixture.main_workdir_id)
            .unwrap();
        record.workdir_id = grant.workdir_id.clone();
        record.source = WorkdirRegistrySource::ExternalGrant {
            grant_id: grant.grant_id.clone(),
        };
        fixture
            .api
            .store
            .create_external_workdir_grant(&grant, &record)
            .unwrap();
        fixture
            .api
            .store
            .attach_worker_workdir(&WorkerWorkdirLinkRecord {
                connection_id: String::new(),
                workspace_id: TEST_WORKSPACE_ID.into(),
                worker: fixture.worker.clone(),
                workdir_id: grant.workdir_id.clone(),
                alias: "external".into(),
                capabilities: workdir::WorkdirSessionCapabilities::ALL,
                linked_at: TEST_CREATED_AT.into(),
                unlinked_at: None,
            })
            .unwrap();
        let (sender, mut operations) = tokio::sync::mpsc::channel(1);
        if let Some(mismatch) = mismatch {
            let connection = ExternalProviderConnection {
                grant_id: grant.grant_id.clone(),
                workdir_id: if mismatch == "workdir" {
                    "other-workdir".into()
                } else {
                    grant.workdir_id.clone()
                },
                provider_instance_id: if mismatch == "provider" {
                    "other-provider".into()
                } else {
                    grant.provider_instance_id.clone()
                },
                generation: if mismatch == "generation" {
                    2
                } else {
                    grant.generation
                },
                expires_at: None,
                capabilities: workdir::WorkdirSessionCapabilities::ALL,
                admission: Arc::new(tokio::sync::Semaphore::new(1)),
                shutdown_confirmed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                sender,
            };
            assert!(
                fixture
                    .api
                    .external_workdir_providers
                    .lock()
                    .unwrap()
                    .connected(Arc::new(connection))
                    .is_none()
            );
        }
        let links = fixture
            .api
            .store
            .list_worker_workdir_links(TEST_WORKSPACE_ID, &fixture.worker)
            .unwrap();
        let app = build_router(fixture.api.clone());
        let path = format!("/api/w/{TEST_WORKSPACE_ID}/workers/self/workdir-session/operations");
        for operation in [
            json!({"operation": "command_start", "request": {
                "command": "body-secret", "cwd": "", "timeout_secs": 1,
                "transport_byte_budget": 1048576, "tool_call_id": "proof-secret"
            }}),
            json!({"operation": "read", "request": {
                "path": "README.md", "offset": 0, "limit": 10, "max_bytes": 1024
            }}),
        ] {
            let request = runtime_source_request(
                &identity,
                Some(&fixture.worker.worker_id),
                "POST",
                &path,
                serde_json::to_vec(&json!({"target_workdir": "external", "operation": operation}))
                    .unwrap(),
            );
            let (status, body, events) = request_with_logs(app.clone(), request).await;
            assert_eq!(
                status,
                StatusCode::BAD_GATEWAY,
                "{state}/{mismatch:?}: {body}"
            );
            assert!(
                body["message"]
                    .as_str()
                    .unwrap()
                    .starts_with(&format!("{code}:")),
                "{body}"
            );
            assert_eq!(events.len(), 1, "{events:?}");
            assert_eq!(
                events[0]["kind"],
                format!("workdir_session_operation_{code}")
            );
            assert_eq!(events[0]["workdir_stage"], "session_open");
            assert_eq!(events[0]["operation"], operation["operation"]);
            assert_eq!(events[0]["message"], "Workdir operation failed");
            assert_eq!(events[0]["diagnostics"], "[]");
            assert_eq!(events[0]["failure_reason"], Value::Null);
            assert_workdir_log_identity(&events[0], &fixture, "external", Some(&grant.workdir_id));
            assert!(
                operations.try_recv().is_err(),
                "failed open dispatched an operation"
            );
            assert_eq!(
                fixture
                    .api
                    .store
                    .list_worker_workdir_links(TEST_WORKSPACE_ID, &fixture.worker)
                    .unwrap(),
                links
            );
        }
    }
}

#[test]
fn workdir_backend_diagnostics_keep_only_known_codes_not_messages_or_unknown_codes() {
    for (code, expected) in [
        (
            "external_workdir_provider_revoked",
            "workdir_session_operation_external_workdir_provider_revoked",
        ),
        (
            "external_workdir_provider_expired",
            "workdir_session_operation_external_workdir_provider_expired",
        ),
        (
            "external_workdir_grant_not_found",
            "workdir_session_operation_external_workdir_grant_not_found",
        ),
        (
            "workdir_session_registration_failed",
            "workdir_session_operation_workdir_session_registration_failed",
        ),
        (
            "external_workdir_provider_body-secret",
            "workdir_session_operation_api_502",
        ),
        (
            "unknown_provider_error",
            "workdir_session_operation_api_502",
        ),
    ] {
        let error = InternalWorkdirOperationError::Contextual {
            error: Box::new(
                Error::RuntimeOperationFailed {
                    runtime_id: "workspace-backend".into(),
                    code: code.into(),
                    message: "external_workdir_provider_offline body-secret /host/private".into(),
                }
                .into(),
            ),
            context: server_api::WorkdirOperationLogContext {
                operation: "command_start",
                stage: "session_open",
                ..Default::default()
            },
        }
        .into_contract_error();
        let log = ApiErrorLog::from_workdir_operation_error(&error, StatusCode::BAD_GATEWAY);
        assert_eq!(log.kind, expected);
        assert_eq!(log.message, "Workdir operation failed");
        assert!(log.diagnostics.is_empty());
        assert!(log.failure_reason.is_none());
        assert!(!format!("{log:?}").contains("secret"));
        assert!(!format!("{log:?}").contains("/host/private"));

        // A matching message/classification without an actual diagnostic code
        // is not evidence of that Backend failure.
        let mut without_diagnostics = error;
        without_diagnostics.diagnostics = None;
        without_diagnostics.error = Some(code.into());
        without_diagnostics.message = code.into();
        let log = ApiErrorLog::from_workdir_operation_error(
            &without_diagnostics,
            StatusCode::BAD_GATEWAY,
        );
        assert_eq!(log.kind, "workdir_session_operation_api_502");
    }
}
