use super::*;

fn operation_input(kind: WorkerInputKind) -> WorkerInputRequest {
    WorkerInputRequest {
        kind,
        content: "operation-boundary-test".into(),
        submission_request_id: None,
        segments: None,
    }
}

fn operation_lifecycle() -> WorkerLifecycleRequest {
    WorkerLifecycleRequest {
        reason: Some("operation-boundary-test".into()),
        ticket_assignment: None,
    }
}

fn operation_cleanup_candidate(identity: &RuntimeWorkerRef) -> CleanupWorkerCandidate {
    CleanupWorkerCandidate {
        target_id: format!("worker:{}:{}", identity.runtime_id, identity.worker_id),
        action: server_api::CleanupTargetKind::WorkerDelete,
        worker_id: identity.worker_id.clone(),
        runtime_worker_id: identity.worker_id.clone(),
        runtime_id: identity.runtime_id.clone(),
        reason: "operation-boundary-test".into(),
        blocking_reason: None,
        pinned: false,
        retention_state: "normal".into(),
        linked_workdir_ids: Vec::new(),
        running_linked: false,
        estimated_reclaim_bytes: None,
    }
}

async fn browser_operation_context(api: &WorkspaceApi) -> (WorkerOperationContext, String) {
    seed_test_api_token(api.store.as_ref(), "worker-operation-browser");
    let token = "worker-operation-browser-session";
    api.store
        .create_browser_session(&BrowserSessionRecord {
            token_hash: crate::auth::token_hash(token),
            session_id: "worker-operation-session".into(),
            user_id: "user-worker-operation-browser".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            expires_at: "2099-01-01T00:00:00Z".into(),
            revoked_at: None,
        })
        .unwrap();
    let AuthConfig::Passkey {
        cookie_name,
        origin,
        ..
    } = &api.config.auth;
    let mut headers = HeaderMap::new();
    headers.insert(ORIGIN, origin.parse().unwrap());
    headers.insert(
        axum::http::header::COOKIE,
        format!("{cookie_name}={token}").parse().unwrap(),
    );
    let actor = resolve_actor(&ServerAuthApi::from(api), &headers)
        .await
        .unwrap()
        .expect("valid browser session");
    (
        WorkerOperationContext::Browser {
            headers,
            source: authenticated_browser_input_source(&actor),
        },
        crate::auth::token_hash(token),
    )
}

fn browser_operation_methods() -> Vec<protocol::Method> {
    vec![
        protocol::Method::Pause {
            command: protocol::WorkerCommandEnvelope::new(701),
        },
        protocol::Method::Resume {
            command: protocol::WorkerCommandEnvelope::new(702),
        },
        protocol::Method::Cancel {
            command: protocol::WorkerCommandEnvelope::new(703),
        },
        protocol::Method::Compact {
            command: protocol::WorkerCommandEnvelope::new(704),
        },
        protocol::Method::Submit {
            submission_request_id: "worker-operation-submit".into(),
            input: vec![protocol::Segment::Text {
                content: "must not reach execution".into(),
            }],
        },
        protocol::Method::Notify {
            notification_request_id: "worker-operation-notify".into(),
            message: "must not reach execution".into(),
        },
        protocol_test_completion("worker-operation-completion"),
    ]
}

#[tokio::test]
async fn handle_resource_keys_preserve_identity_for_equal_local_ids_across_runtimes() {
    let workspace = tempfile::tempdir().unwrap();
    let api = test_api(workspace.path()).await;
    let a = RuntimeWorkerRef::new("offline-runtime-a", "shared-local-id");
    let b = RuntimeWorkerRef::new("offline-runtime-b", "shared-local-id");
    for identity in [&a, &b] {
        seed_worker_source_member(&api, &identity.runtime_id, &identity.worker_id);
    }
    let key = |identity: &RuntimeWorkerRef| {
        api.store
            .worker_resource_key(TEST_WORKSPACE_ID, identity)
            .unwrap()
            .unwrap()
    };
    assert_ne!(key(&a), key(&b));

    for identity in [&a, &b] {
        let direct =
            WorkspaceWorker::resolve(&api, &identity.runtime_id, &identity.worker_id).unwrap();
        let canonical =
            WorkspaceWorker::resolve(&api, &identity.runtime_id, &key(identity)).unwrap();
        let pinned = canonical
            .set_pinned(&WorkerOperationContext::Backend, true)
            .await
            .unwrap();
        assert_eq!(pinned.runtime_id, identity.runtime_id);
        assert_eq!(pinned.worker_id, identity.worker_id);
        let unpinned = direct
            .set_pinned(&WorkerOperationContext::Backend, false)
            .await
            .unwrap();
        assert_eq!(unpinned.runtime_id, pinned.runtime_id);
        assert_eq!(unpinned.worker_id, pinned.worker_id);
    }
    assert!(matches!(
        WorkspaceWorker::resolve(&api, &b.runtime_id, &key(&a)),
        Err(Error::UnknownWorker { .. })
    ));
}

#[tokio::test]
async fn handle_resolution_rejects_unknown_foreign_and_internal_only_workers() {
    let workspace = tempfile::tempdir().unwrap();
    let api = test_api(workspace.path()).await;
    let known = spawn_ticket_check_source(&api, "operation-resolution-parent");
    let mut foreign = api
        .store
        .get_worker_registry(TEST_WORKSPACE_ID, &known)
        .unwrap()
        .unwrap();
    let mut other_workspace = api
        .store
        .get_workspace(TEST_WORKSPACE_ID)
        .await
        .unwrap()
        .unwrap();
    other_workspace.workspace_id = "other-workspace".into();
    api.store.upsert_workspace(&other_workspace).await.unwrap();
    foreign.workspace_id = other_workspace.workspace_id;
    foreign.worker.worker_id = "foreign-only-worker".into();
    api.store.upsert_worker_registry(&foreign).unwrap();
    assert!(
        api.store
            .get_worker_registry(TEST_WORKSPACE_ID, &foreign.worker)
            .unwrap()
            .is_none()
    );

    let RuntimeObservationSource::Embedded(source) =
        api.runtime.observation_source(&known).unwrap()
    else {
        panic!("expected embedded fixture");
    };
    let internal = protocol::InternalWorkerRef {
        session_id: "internal-only-session".into(),
        parent_session_id: None,
        name: "internal-only-child".into(),
        kind: protocol::InternalWorkerKind::SubWorker,
    };
    source
        .runtime
        .observe_worker_event(
            &source.worker_ref,
            protocol::Event::InternalWorker {
                worker: internal.clone(),
                revision: 1,
                event: Box::new(protocol::Event::WorkerState {
                    snapshot: protocol::WorkerStateSnapshot::initial(),
                }),
            },
        )
        .unwrap();

    for reference in [
        "unknown-worker",
        &foreign.worker.worker_id,
        &internal.session_id,
        &internal.name,
    ] {
        assert!(
            matches!(
                WorkspaceWorker::resolve(&api, &known.runtime_id, reference),
                Err(Error::UnknownWorker { .. })
            ),
            "unexpected registry handle for {reference}"
        );
    }
}

#[tokio::test]
async fn offline_handle_can_pin_but_deleted_handle_cannot_mutate_or_connect() {
    let workspace = tempfile::tempdir().unwrap();
    let api = test_api(workspace.path()).await;
    let identity = RuntimeWorkerRef::new("unregistered-offline-runtime", "42");
    seed_worker_source_member(&api, &identity.runtime_id, &identity.worker_id);
    assert!(
        api.runtime.worker(&identity).is_err(),
        "fixture must have no live Runtime"
    );
    let handle = WorkspaceWorker::resolve(&api, &identity.runtime_id, &identity.worker_id).unwrap();
    let retained = handle
        .set_pinned(&WorkerOperationContext::Backend, true)
        .await
        .unwrap();
    assert!(retained.pinned);
    assert_eq!(
        api.store
            .get_worker_registry(TEST_WORKSPACE_ID, &identity)
            .unwrap()
            .unwrap()
            .retention_state,
        "pinned"
    );

    let mut wrong_candidate = operation_cleanup_candidate(&identity);
    wrong_candidate.runtime_worker_id = "different-worker".into();
    assert!(matches!(
        handle.delete_from_plan(&WorkerOperationContext::Backend, &wrong_candidate)
            .await.unwrap_err().error,
        Error::InvalidInput(message) if message.contains("resolved Worker")
    ));

    api.store
        .delete_worker_registry(TEST_WORKSPACE_ID, &identity)
        .unwrap();
    assert!(
        matches!(handle.set_pinned(&WorkerOperationContext::Backend, false)
        .await.unwrap_err().error, Error::UnknownWorker { worker } if worker == identity)
    );
    assert!(matches!(
        handle
            .restore(
                &WorkerOperationContext::Backend,
                server_api::WorkerRestoreRequest {
                    expected_observation_token: "stale".into(),
                    request_id: "removed-handle".into(),
                }
            )
            .await
            .unwrap_err()
            .error,
        Error::UnknownWorker { .. }
    ));
    assert!(matches!(
        handle
            .stop(&WorkerOperationContext::Backend, operation_lifecycle())
            .await
            .unwrap_err()
            .error,
        Error::UnknownWorker { .. }
    ));
    assert!(matches!(
        handle
            .cancel(&WorkerOperationContext::Backend, operation_lifecycle())
            .await
            .unwrap_err()
            .error,
        Error::UnknownWorker { .. }
    ));
    for kind in [WorkerInputKind::User, WorkerInputKind::Notify] {
        assert!(matches!(
            handle
                .input(&WorkerOperationContext::Backend, operation_input(kind))
                .await
                .unwrap_err()
                .error,
            Error::UnknownWorker { .. }
        ));
    }
    assert!(matches!(
        handle
            .delete_from_plan(
                &WorkerOperationContext::Backend,
                &operation_cleanup_candidate(&identity)
            )
            .await
            .unwrap_err()
            .error,
        Error::UnknownWorker { .. }
    ));
    assert!(matches!(
        handle
            .connect_protocol(&WorkerOperationContext::Backend)
            .await,
        Err(Error::UnknownWorker { .. })
    ));
    assert!(
        api.store
            .get_worker_registry(TEST_WORKSPACE_ID, &identity)
            .unwrap()
            .is_none(),
        "a stale handle must not recreate its registry row"
    );
}

#[tokio::test]
async fn worker_control_handle_rechecks_revoked_grants_for_each_operation() {
    let workspace = tempfile::tempdir().unwrap();
    let api = test_api(workspace.path()).await;
    let controller = RuntimeWorkerRef::new("offline-runtime", "controller");
    let subject = RuntimeWorkerRef::new("offline-runtime", "subject");
    for identity in [&controller, &subject] {
        seed_worker_source_member(&api, &identity.runtime_id, &identity.worker_id);
    }
    api.store
        .create_worker_control_grant(&WorkerControlGrantRecord {
            workspace_id: TEST_WORKSPACE_ID.into(),
            grant_id: "operation-grant".into(),
            controller: controller.clone(),
            subject: subject.clone(),
            relation: "spawned".into(),
            origin: "test".into(),
            permissions: [
                "restore",
                "stop",
                "cancel",
                "send_input",
                "notify",
                "pin",
                "remove",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            operation_id: "operation-grant-test".into(),
            created_at: now_registry_timestamp(),
            revoked_at: None,
        })
        .unwrap();
    let handle = WorkspaceWorker::resolve(&api, &subject.runtime_id, &subject.worker_id).unwrap();
    let context = WorkerOperationContext::WorkerControl { controller };
    handle.set_pinned(&context, true).await.unwrap();
    api.store
        .revoke_worker_control_grant(
            TEST_WORKSPACE_ID,
            "operation-grant",
            &now_registry_timestamp(),
        )
        .unwrap();

    for permission in [
        "restore",
        "stop",
        "cancel",
        "send_input",
        "notify",
        "pin",
        "remove",
    ] {
        assert!(
            matches!(
                handle.authorize_operation(&context, permission).await,
                Err(ApiError {
                    error: Error::UnknownWorker { .. },
                    ..
                })
            ),
            "revoked grant authorized {permission}"
        );
    }
    assert!(matches!(
        handle
            .restore(
                &context,
                server_api::WorkerRestoreRequest {
                    expected_observation_token: "stale".into(),
                    request_id: "revoked-grant".into(),
                }
            )
            .await
            .unwrap_err()
            .error,
        Error::UnknownWorker { .. }
    ));
    assert!(matches!(
        handle
            .stop(&context, operation_lifecycle())
            .await
            .unwrap_err()
            .error,
        Error::UnknownWorker { .. }
    ));
    assert!(matches!(
        handle
            .cancel(&context, operation_lifecycle())
            .await
            .unwrap_err()
            .error,
        Error::UnknownWorker { .. }
    ));
    for kind in [WorkerInputKind::User, WorkerInputKind::Notify] {
        assert!(matches!(
            handle
                .input(&context, operation_input(kind))
                .await
                .unwrap_err()
                .error,
            Error::UnknownWorker { .. }
        ));
    }
    assert!(matches!(
        handle.set_pinned(&context, false).await.unwrap_err().error,
        Error::UnknownWorker { .. }
    ));
    assert!(matches!(
        handle
            .delete_from_plan(&context, &operation_cleanup_candidate(&subject))
            .await
            .unwrap_err()
            .error,
        Error::UnknownWorker { .. }
    ));
    assert_eq!(
        api.store
            .get_worker_registry(TEST_WORKSPACE_ID, &subject)
            .unwrap()
            .unwrap()
            .retention_state,
        "pinned"
    );
}

#[tokio::test]
async fn generic_and_scoped_input_aliases_match_handle_idle_only_and_notify_semantics() {
    let fixture = guarded_spawn_fixture().await;
    let response = post_guarded_spawn(
        &fixture,
        guarded_spawn_payload(&fixture, "operation-alias-parity"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let created: BrowserCreateWorkerResponse = serde_json::from_slice(&bytes).unwrap();
    let api = fixture.api.clone();
    let runtime = &fixture.runtime;
    let subject = RuntimeWorkerRef::new(created.runtime_id, created.worker_id);
    let controller = fixture.controller.clone();
    let key = api
        .store
        .worker_resource_key(TEST_WORKSPACE_ID, &subject)
        .unwrap()
        .unwrap();
    let handle = WorkspaceWorker::resolve(&api, &subject.runtime_id, &key).unwrap();
    let context = WorkerOperationContext::WorkerControl { controller };
    let idle_only = handle
        .input(&context, operation_input(WorkerInputKind::User))
        .await
        .unwrap();
    assert_eq!(idle_only.state, InternalWorkerOperationState::Rejected);
    let notify = handle
        .input(&context, operation_input(WorkerInputKind::Notify))
        .await
        .unwrap();
    assert_eq!(notify.state, InternalWorkerOperationState::Accepted);

    let request = || server_api::RuntimeWorkerInputRequest {
        kind: Some("user".into()),
        content: "human input".into(),
        segments: None,
    };
    let Json(generic) = send_runtime_worker_input(
        State(api.clone()),
        AxumPath((subject.runtime_id.clone(), key.clone())),
        Json(request()),
    )
    .await
    .unwrap();
    let Json(scoped) = scoped_send_runtime_worker_input(
        State(api.clone()),
        AxumPath(ScopedRuntimeWorkerPath {
            workspace_id: TEST_WORKSPACE_ID.into(),
            worker: RuntimeWorkerRef::new(&subject.runtime_id, &key),
        }),
        Json(request()),
    )
    .await
    .unwrap();
    assert_eq!(generic.state, scoped.state);
    assert_eq!(generic.state, server_api::WorkerOperationState::Accepted);
    assert_eq!(
        runtime
            .input_requests
            .lock()
            .unwrap()
            .iter()
            .map(|request| request.kind.clone())
            .collect::<Vec<_>>(),
        vec![
            WorkerInputKind::UserIfIdle,
            WorkerInputKind::Notify,
            WorkerInputKind::User,
            WorkerInputKind::User
        ]
    );
}

#[tokio::test]
async fn browser_protocol_sender_rejects_job_input_and_all_live_methods() {
    let workspace = tempfile::tempdir().unwrap();
    let (api, execution) = test_api_with_recording_backend(workspace.path()).await;
    let request = BackendJobRequest {
        job_id: "worker-operation-read-only-job".into(),
        purpose: "operation_boundary_test".into(),
        input_revision: "revision-1".into(),
        input_ref: "test://worker-operation/input".into(),
        input: serde_json::json!({"immutable": true}),
        instruction: "Return a structured result.".into(),
        profile: "builtin:backend-job".into(),
        source_worker: None,
        notification_target: None,
        limits: crate::backend_job::BackendJobLimits::default(),
    };
    let dispatched = api.dispatch_backend_job(&request).unwrap();
    let identity = dispatched.attempt.worker.unwrap();
    assert_eq!(execution.take_inputs().len(), 1);
    let handle = WorkspaceWorker::resolve(&api, &identity.runtime_id, &identity.worker_id).unwrap();
    let (context, _) = browser_operation_context(&api).await;
    let mut connection = handle.connect_protocol(&context).await.unwrap();
    assert!(matches!(
        tokio::time::timeout(std::time::Duration::from_secs(2), connection.events.recv())
            .await
            .unwrap()
            .unwrap(),
        protocol::Event::Snapshot { .. }
    ));
    for method in browser_operation_methods() {
        assert!(matches!(connection.sender.send(&context, method).await,
            Err(Error::InvalidInput(message)) if message.contains("read-only")));
    }
    for kind in [WorkerInputKind::User, WorkerInputKind::Notify] {
        assert!(
            matches!(handle.input(&context, operation_input(kind)).await.unwrap_err().error,
            Error::InvalidInput(message) if message.contains("read-only"))
        );
    }
    assert!(execution.protocol_methods().is_empty());
    assert!(execution.take_inputs().is_empty());
}

#[tokio::test]
async fn connected_browser_sender_rechecks_revoked_credentials_before_each_method() {
    let workspace = tempfile::tempdir().unwrap();
    let (api, execution) = test_api_with_recording_backend(workspace.path()).await;
    let identity = spawn_ticket_check_source(&api, "operation-browser-revocation");
    let handle = WorkspaceWorker::resolve(&api, &identity.runtime_id, &identity.worker_id).unwrap();
    let (context, token_hash) = browser_operation_context(&api).await;
    let mut connection = handle.connect_protocol(&context).await.unwrap();
    assert!(matches!(
        tokio::time::timeout(std::time::Duration::from_secs(2), connection.events.recv())
            .await
            .unwrap()
            .unwrap(),
        protocol::Event::Snapshot { .. }
    ));
    // Prove these credentials were usable before revocation, then keep the same sender.
    handle.set_pinned(&context, true).await.unwrap();
    assert!(
        api.store
            .revoke_browser_session(&token_hash, &now_registry_timestamp())
            .unwrap()
    );
    assert!(matches!(
        handle.connect_protocol(&context).await,
        Err(Error::WorkspacePermissionDenied(_))
    ));
    for method in browser_operation_methods() {
        assert!(matches!(
            connection.sender.send(&context, method).await,
            Err(Error::WorkspacePermissionDenied(_))
        ));
    }
    assert!(matches!(
        handle.set_pinned(&context, false).await.unwrap_err().error,
        Error::WorkspacePermissionDenied(_)
    ));
    assert!(
        execution.protocol_methods().is_empty(),
        "revoked credentials must not reach the execution channel"
    );
    assert_eq!(
        api.store
            .get_worker_registry(TEST_WORKSPACE_ID, &identity)
            .unwrap()
            .unwrap()
            .retention_state,
        "pinned"
    );
}

fn operation_request_context(
    source: Option<server_api::ServerRuntimeSource>,
) -> server_api::ServerRequestContext {
    server_api::ServerRequestContext {
        actor: None,
        worker_source: None,
        runtime_source: source,
        origin: None,
        transport_headers: Vec::new(),
    }
}

#[tokio::test]
async fn request_context_uses_verified_subject_not_headers_or_account_fallback() {
    let workspace = tempfile::tempdir().unwrap();
    let api = test_api(workspace.path()).await;
    let mut headers_only = operation_request_context(None);
    headers_only.transport_headers = vec![
        ("x-yoi-runtime-id".into(), b"runtime-a".to_vec()),
        ("x-yoi-worker-id".into(), b"controller-a".to_vec()),
    ];
    assert!(WorkerOperationContext::from_request(&api, &headers_only).is_err());
    let mut runtime_only = operation_request_context(Some(server_api::ServerRuntimeSource {
        runtime_id: "runtime-a".into(),
        worker_id: None,
    }));
    runtime_only.actor = Some(test_browser_request_actor());
    assert!(WorkerOperationContext::from_request(&api, &runtime_only).is_err());
    let mut mismatched = operation_request_context(Some(server_api::ServerRuntimeSource {
        runtime_id: "runtime-a".into(),
        worker_id: Some("controller-a".into()),
    }));
    mismatched.actor = Some(test_browser_request_actor());
    mismatched.transport_headers = vec![("x-yoi-worker-id".into(), b"controller-b".to_vec())];
    assert!(WorkerOperationContext::from_request(&api, &mismatched).is_err());
    seed_worker_source_member(&api, "runtime-a", "controller-a");
    let context = operation_request_context(Some(server_api::ServerRuntimeSource {
        runtime_id: "runtime-a".into(),
        worker_id: Some("controller-a".into()),
    }));
    assert!(
        matches!(WorkerOperationContext::from_request(&api, &context).unwrap(),
        WorkerOperationContext::WorkerControl { controller } if controller == RuntimeWorkerRef::new("runtime-a", "controller-a"))
    );
}

#[tokio::test]
async fn generic_management_aliases_cannot_bypass_worker_grants_or_target_bound_remove() {
    use server_api::ServerApi;
    let workspace = tempfile::tempdir().unwrap();
    let api = test_api(workspace.path()).await;
    seed_worker_source_member(&api, "offline-runtime", "controller");
    seed_worker_source_member(&api, "offline-runtime", "subject");
    let service = ServerApiContractService::Workspace(api.clone());
    let context = || {
        operation_request_context(Some(server_api::ServerRuntimeSource {
            runtime_id: "offline-runtime".into(),
            worker_id: Some("controller".into()),
        }))
    };
    let input = || server_api::RuntimeWorkerInputRequest {
        kind: Some("user".into()),
        content: "must not execute".into(),
        segments: None,
    };
    assert!(
        service
            .runtime_worker_input_alias(
                context(),
                "offline-runtime".into(),
                "subject".into(),
                input()
            )
            .await
            .is_err()
    );
    assert!(
        service
            .runtime_worker_input(
                context(),
                TEST_WORKSPACE_ID.into(),
                "offline-runtime".into(),
                "subject".into(),
                input()
            )
            .await
            .is_err()
    );
    assert!(
        service
            .runtime_worker_restore_alias(
                context(),
                "offline-runtime".into(),
                "subject".into(),
                server_api::WorkerRestoreRequest {
                    expected_observation_token: "old".into(),
                    request_id: "unauthorized".into(),
                }
            )
            .await
            .is_err()
    );
    let lifecycle = || server_api::RuntimeWorkerLifecycleRequest {
        reason: None,
        ticket_assignment: None,
    };
    assert!(
        service
            .runtime_worker_stop_alias(
                context(),
                "offline-runtime".into(),
                "subject".into(),
                lifecycle()
            )
            .await
            .is_err()
    );
    assert!(
        service
            .runtime_worker_cancel_alias(
                context(),
                "offline-runtime".into(),
                "subject".into(),
                lifecycle()
            )
            .await
            .is_err()
    );
    assert!(
        service
            .runtime_worker_pin(
                context(),
                TEST_WORKSPACE_ID.into(),
                "offline-runtime".into(),
                "subject".into()
            )
            .await
            .is_err()
    );
    let cleanup = service
        .runtime_cleanup_execute(
            context(),
            TEST_WORKSPACE_ID.into(),
            "offline-runtime".into(),
            ExecuteRuntimeCleanupRequest {
                expected_plan_revision: "irrelevant".into(),
                expected_plan_digest: "irrelevant".into(),
                worker_target_ids: Vec::new(),
                workdir_target_ids: Vec::new(),
                confirm_dirty_discard_target_ids: Vec::new(),
            },
        )
        .await
        .unwrap_err();
    assert!(cleanup.message.contains("target-bound WorkerRemove"));
    assert_eq!(
        api.store
            .get_worker_registry(
                TEST_WORKSPACE_ID,
                &RuntimeWorkerRef::new("offline-runtime", "subject")
            )
            .unwrap()
            .unwrap()
            .retention_state,
        "normal"
    );
}

#[test]
fn uncertain_unary_input_keeps_a_distinct_diagnostic_without_claiming_acceptance() {
    let result = worker_input_result_to_api(WorkerInputResult {
        state: InternalWorkerOperationState::Rejected,
        disposition: WorkerInputDisposition::Unknown,
        worker: RuntimeWorkerRef::new("runtime-a", "worker-a"),
        runtime_run_id: None,
        diagnostics: Vec::new(),
    });
    assert_eq!(result.state, server_api::WorkerOperationState::Rejected);
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "worker_input_outcome_unknown")
    );
}

#[tokio::test]
async fn grant_replacement_while_admission_waits_cannot_use_another_grants_lock() {
    let workspace = tempfile::tempdir().unwrap();
    let api = test_api(workspace.path()).await;
    let controller = RuntimeWorkerRef::new("offline-runtime", "controller");
    let subject = RuntimeWorkerRef::new("offline-runtime", "subject");
    for identity in [&controller, &subject] {
        seed_worker_source_member(&api, &identity.runtime_id, &identity.worker_id);
    }
    let grant = |id: &str| WorkerControlGrantRecord {
        workspace_id: TEST_WORKSPACE_ID.into(),
        grant_id: id.into(),
        controller: controller.clone(),
        subject: subject.clone(),
        relation: "spawned".into(),
        origin: "test".into(),
        permissions: vec!["pin".into()],
        operation_id: id.into(),
        created_at: now_registry_timestamp(),
        revoked_at: None,
    };
    api.store
        .create_worker_control_grant(&grant("original-grant"))
        .unwrap();
    let guard = worker_control_lock(&api, "original-grant")
        .lock_owned()
        .await;
    let handle = WorkspaceWorker::resolve(&api, &subject.runtime_id, &subject.worker_id).unwrap();
    let context = WorkerOperationContext::WorkerControl {
        controller: controller.clone(),
    };
    let admission = handle.set_pinned(&context, true);
    tokio::pin!(admission);
    // Poll deterministically to the held lock before replacing grant authority.
    assert!(futures::poll!(admission.as_mut()).is_pending());
    api.store
        .revoke_worker_control_grant(
            TEST_WORKSPACE_ID,
            "original-grant",
            &now_registry_timestamp(),
        )
        .unwrap();
    api.store
        .create_worker_control_grant(&grant("replacement-grant"))
        .unwrap();
    drop(guard);
    assert!(matches!(
        admission.await.unwrap_err().error,
        Error::RepositoryConflict(_)
    ));
    assert_eq!(
        api.store
            .get_worker_registry(TEST_WORKSPACE_ID, &subject)
            .unwrap()
            .unwrap()
            .retention_state,
        "normal"
    );
}

#[tokio::test]
async fn browser_protocol_connection_and_cleanup_reject_cookie_origin_mismatch() {
    let workspace = tempfile::tempdir().unwrap();
    let (api, execution) = test_api_with_recording_backend(workspace.path()).await;
    let identity = spawn_ticket_check_source(&api, "operation-origin");
    let handle = WorkspaceWorker::resolve(&api, &identity.runtime_id, &identity.worker_id).unwrap();
    let (mut context, _) = browser_operation_context(&api).await;
    let WorkerOperationContext::Browser { headers, .. } = &mut context else {
        unreachable!()
    };
    headers.insert(ORIGIN, "https://untrusted.example".parse().unwrap());
    assert!(matches!(
        handle.connect_protocol(&context).await,
        Err(Error::WorkspacePermissionDenied(_))
    ));
    let result = execute_runtime_cleanup_with_context(
        &api,
        EMBEDDED_WORKER_RUNTIME_ID,
        ExecuteRuntimeCleanupRequest {
            expected_plan_revision: "irrelevant".into(),
            expected_plan_digest: "irrelevant".into(),
            worker_target_ids: Vec::new(),
            workdir_target_ids: Vec::new(),
            confirm_dirty_discard_target_ids: Vec::new(),
        },
        &context,
    )
    .await
    .unwrap_err();
    assert!(matches!(result.error, Error::WorkspacePermissionDenied(_)));
    assert!(execution.protocol_methods().is_empty());
}

fn restore_request_for(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
    request_id: &str,
) -> server_api::WorkerRestoreRequest {
    server_api::WorkerRestoreRequest {
        expected_observation_token: api
            .runtime
            .worker(worker)
            .unwrap()
            .restore_observation_token
            .unwrap(),
        request_id: request_id.into(),
    }
}

#[tokio::test]
async fn restore_guard_rejects_stale_list_and_stopped_restored_stopped_aba() {
    let workspace = tempfile::tempdir().unwrap();
    let (api, execution) = test_api_with_recording_backend(workspace.path()).await;
    execution.accept_restores();
    let identity = spawn_ticket_check_source(&api, "restore-guard-aba");
    let handle = WorkspaceWorker::resolve(&api, &identity.runtime_id, &identity.worker_id).unwrap();
    handle
        .stop(&WorkerOperationContext::Backend, operation_lifecycle())
        .await
        .unwrap();
    let original = restore_request_for(&api, &identity, "restore-original");
    let stale_list = server_api::WorkerRestoreRequest {
        request_id: "restore-stale-list".into(),
        ..original.clone()
    };
    assert_eq!(
        handle
            .restore(&WorkerOperationContext::Backend, original.clone())
            .await
            .unwrap()
            .state,
        server_api::WorkerRestoreState::Accepted
    );
    let conflict = handle
        .restore(&WorkerOperationContext::Backend, stale_list.clone())
        .await
        .unwrap_err();
    assert!(matches!(conflict.error, Error::RestoreObservationConflict));
    let typed = conflict.into_repository_api_error();
    assert_eq!(
        api_error_status(&Error::RestoreObservationConflict),
        StatusCode::CONFLICT
    );
    assert_eq!(typed.error, "restore_observation_conflict");
    handle
        .stop(&WorkerOperationContext::Backend, operation_lifecycle())
        .await
        .unwrap();
    assert_ne!(
        original.expected_observation_token,
        api.runtime
            .worker(&identity)
            .unwrap()
            .restore_observation_token
            .unwrap()
    );
    assert!(matches!(
        handle
            .restore(&WorkerOperationContext::Backend, stale_list)
            .await
            .unwrap_err()
            .error,
        Error::RestoreObservationConflict
    ));
    assert_eq!(api.runtime.worker(&identity).unwrap().state, "stopped");
    assert!(
        execution.contexts.lock().unwrap().is_empty(),
        "conflict must not install execution"
    );
    // The original request is result replay, not a new intent; it stays accepted
    // without resurrecting the Worker after a subsequent stop.
    assert_eq!(
        handle
            .restore(&WorkerOperationContext::Backend, original)
            .await
            .unwrap()
            .state,
        server_api::WorkerRestoreState::Accepted
    );
    assert_eq!(api.runtime.worker(&identity).unwrap().state, "stopped");
    assert!(execution.contexts.lock().unwrap().is_empty());
}

fn restore_test_workdir(
    api: &WorkspaceApi,
    id: &str,
    source: WorkdirRegistrySource,
) -> WorkdirRegistryRecord {
    WorkdirRegistryRecord {
        workspace_id: api.workspace_id().into(),
        workdir_id: id.into(),
        display_name: None,
        source,
        creation_selector: None,
        creation_ref: None,
        creation_tree: None,
        current_selector: None,
        current_ref: None,
        current_tree: None,
        observed_at_epoch_seconds: None,
        materialization_status: "present".into(),
        cleanliness: "clean".into(),
        created_at: TEST_CREATED_AT.into(),
        updated_at: TEST_CREATED_AT.into(),
    }
}

fn attach_restore_test_workdir(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
    workdir: &WorkdirRegistryRecord,
) {
    api.store
        .attach_worker_workdir(&WorkerWorkdirLinkRecord {
            connection_id: String::new(),
            workspace_id: api.workspace_id().into(),
            worker: worker.clone(),
            workdir_id: workdir.workdir_id.clone(),
            alias: workdir.workdir_id.clone(),
            capabilities: workdir::WorkdirSessionCapabilities::READ_ONLY,
            linked_at: TEST_CREATED_AT.into(),
            unlinked_at: None,
        })
        .unwrap();
}

fn displace_restore_singleton(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
    replacement: &RuntimeWorkerRef,
) {
    api.config_store.with_conn(|conn| {
        conn.execute("UPDATE worker_create_reservations SET singleton_key='restore-test-singleton', singleton_generation=1 WHERE workspace_id=?1 AND worker_id=?2",
            rusqlite::params![api.workspace_id(), worker.worker_id])?;
        conn.execute("INSERT INTO worker_singleton_owners(workspace_id,singleton_key,runtime_id,worker_id,generation,created_at,updated_at) VALUES(?1,'restore-test-singleton',?2,?3,2,?4,?4)",
            rusqlite::params![api.workspace_id(), replacement.runtime_id, replacement.worker_id, TEST_CREATED_AT])?;
        Ok(())
    }).unwrap();
    assert!(
        api.store
            .require_current_worker_singleton_owner(api.workspace_id(), worker)
            .is_err()
    );
}

fn add_offline_restore_workdir(api: &WorkspaceApi, worker: &RuntimeWorkerRef) {
    let record = restore_test_workdir(
        api,
        "restore-offline",
        WorkdirRegistrySource::ExternalGrant {
            grant_id: "restore-offline-grant".into(),
        },
    );
    api.store
        .create_external_workdir_grant(
            &crate::store::ExternalWorkdirGrantRecord {
                grant_id: "restore-offline-grant".into(),
                workspace_id: api.workspace_id().into(),
                workdir_id: record.workdir_id.clone(),
                provider_instance_id: "offline-provider".into(),
                display_name: "Offline grant".into(),
                permissions: "read_only".into(),
                created_by: "owner-account".into(),
                created_at: TEST_CREATED_AT.into(),
                expires_at: None,
                generation: 1,
                status: "offline".into(),
                updated_at: TEST_CREATED_AT.into(),
            },
            &record,
        )
        .unwrap();
    attach_restore_test_workdir(api, worker, &record);
}

#[tokio::test]
async fn restore_guard_stale_ssh_intent_does_not_issue_workspace_resource() {
    let workspace = tempfile::tempdir().unwrap();
    let api = test_api_with_remote_repository(workspace.path()).await;
    let identity = spawn_ticket_check_source(&api, "restore-stale-ssh");
    let handle = WorkspaceWorker::resolve(&api, &identity.runtime_id, &identity.worker_id).unwrap();
    let stale = restore_request_for(&api, &identity, "stale-ssh-intent");
    handle
        .stop(&WorkerOperationContext::Backend, operation_lifecycle())
        .await
        .unwrap();
    api.repository_secrets
        .generate_credential(
            api.workspace_id(),
            GenerateRepositorySshCredentialRequest {
                operation_id: "restore-default-key".into(),
                credential_id:
                    crate::repository_access::WORKSPACE_DEFAULT_REPOSITORY_SSH_CREDENTIAL_ID.into(),
                name: "Restore test default".into(),
            },
            "owner-account",
        )
        .unwrap();
    let public_key = api
        .repository_secrets
        .credential_public_key(
            api.workspace_id(),
            crate::repository_access::WORKSPACE_DEFAULT_REPOSITORY_SSH_CREDENTIAL_ID,
        )
        .unwrap()
        .unwrap()
        .public_key;
    api.repository_secrets
        .put_host_trust(
            api.workspace_id(),
            PutRepositorySshHostTrustRequest {
                operation_id: "restore-ssh-trust".into(),
                host_trust_id: "restore-ssh-host".into(),
                hostname: "example.invalid".into(),
                port: 22,
                host_key: public_key,
                expected_revision: None,
            },
            "owner-account",
        )
        .unwrap();
    let record = restore_test_workdir(
        &api,
        "restore-ssh-workdir",
        WorkdirRegistrySource::Repository {
            runtime_id: identity.runtime_id.clone(),
            repository_id: test_repository_id(&api),
        },
    );
    api.store.upsert_workdir_registry(&record).unwrap();
    attach_restore_test_workdir(&api, &identity, &record);
    // Prove this is a fully issuable SSH fixture; the old pre-admission path would mint another handle.
    assert!(
        repository_access_request_for_workdir(
            &api,
            &identity.runtime_id,
            &record.workdir_id,
            "restore-ssh-positive-control"
        )
        .unwrap()
        .is_some()
    );
    let before = api.resource_broker.repository_ssh_access_handle_count();
    assert!(before > 0);
    let conflict = handle
        .restore(&WorkerOperationContext::Backend, stale)
        .await
        .unwrap_err();
    assert!(matches!(conflict.error, Error::RestoreObservationConflict));
    assert_eq!(
        api.resource_broker.repository_ssh_access_handle_count(),
        before
    );
    assert_eq!(api.runtime.worker(&identity).unwrap().state, "stopped");
}

#[tokio::test]
async fn restore_guard_historical_replay_ignores_new_eligibility_but_rechecks_auth() {
    let workspace = tempfile::tempdir().unwrap();
    let (api, execution) = test_api_with_recording_backend(workspace.path()).await;
    execution.accept_restores();
    let identity = spawn_ticket_check_source(&api, "restore-historical");
    let replacement = spawn_ticket_check_source(&api, "restore-replacement");
    let handle = WorkspaceWorker::resolve(&api, &identity.runtime_id, &identity.worker_id).unwrap();
    handle
        .stop(&WorkerOperationContext::Backend, operation_lifecycle())
        .await
        .unwrap();
    let request = restore_request_for(&api, &identity, "restore-historical-request");
    assert_eq!(
        handle
            .restore(&WorkerOperationContext::Backend, request.clone())
            .await
            .unwrap()
            .state,
        server_api::WorkerRestoreState::Accepted
    );
    handle
        .stop(&WorkerOperationContext::Backend, operation_lifecycle())
        .await
        .unwrap();
    displace_restore_singleton(&api, &identity, &replacement);
    add_offline_restore_workdir(&api, &identity);
    let before = execution.restore_operations.lock().unwrap().len();
    let replay = handle
        .restore(&WorkerOperationContext::Backend, request.clone())
        .await
        .unwrap();
    assert_eq!(replay.state, server_api::WorkerRestoreState::Accepted);
    assert_eq!(
        replay.worker.unwrap().state,
        "stopped",
        "never return the old idle receipt summary"
    );
    assert_eq!(execution.restore_operations.lock().unwrap().len(), before);
    let projected = restore_runtime_worker_with_context(
        api.clone(),
        identity.runtime_id.clone(),
        identity.worker_id.clone(),
        WorkerOperationContext::Backend,
        request.clone(),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(
        projected.result.worker.unwrap().state,
        "unavailable",
        "current fenced projection, never old idle receipt"
    );
    let (mut browser, _) = browser_operation_context(&api).await;
    let WorkerOperationContext::Browser { headers, .. } = &mut browser else {
        unreachable!()
    };
    headers.insert(ORIGIN, "https://untrusted.example".parse().unwrap());
    assert!(matches!(
        handle.restore(&browser, request).await.unwrap_err().error,
        Error::WorkspacePermissionDenied(_)
    ));
    assert_eq!(execution.restore_operations.lock().unwrap().len(), before);
    assert!(
        handle
            .restore(
                &WorkerOperationContext::Backend,
                restore_request_for(&api, &identity, "new-ineligible-intent")
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn restore_guard_pending_recovery_ignores_new_eligibility_and_reuses_operation() {
    let workspace = tempfile::tempdir().unwrap();
    let (api, execution) = test_api_with_recording_backend(workspace.path()).await;
    execution.accept_restores();
    let identity = spawn_ticket_check_source(&api, "restore-pending");
    let replacement = spawn_ticket_check_source(&api, "restore-pending-replacement");
    let handle = WorkspaceWorker::resolve(&api, &identity.runtime_id, &identity.worker_id).unwrap();
    handle
        .stop(&WorkerOperationContext::Backend, operation_lifecycle())
        .await
        .unwrap();
    *execution.restore_failure.lock().unwrap() = Some(
        worker_runtime::execution::WorkerExecutionSpawnResult::Errored(
            worker_runtime::execution::WorkerExecutionResult::rejected(
                worker_runtime::execution::WorkerExecutionOperation::Restore,
                "uncertain launch",
            ),
        ),
    );
    let request = restore_request_for(&api, &identity, "restore-pending-request");
    assert_eq!(
        handle
            .restore(&WorkerOperationContext::Backend, request.clone())
            .await
            .unwrap()
            .state,
        server_api::WorkerRestoreState::ReconciliationRequired
    );
    let original = execution.restore_operations.lock().unwrap()[0];
    displace_restore_singleton(&api, &identity, &replacement);
    add_offline_restore_workdir(&api, &identity);
    assert_eq!(
        handle
            .restore(&WorkerOperationContext::Backend, request)
            .await
            .unwrap()
            .state,
        server_api::WorkerRestoreState::Accepted
    );
    assert_eq!(
        execution.reconcile_operations.lock().unwrap().as_slice(),
        &[original]
    );
    assert!(
        execution
            .restore_operations
            .lock()
            .unwrap()
            .iter()
            .all(|id| id == &original)
    );
}

#[tokio::test]
async fn restore_guard_internal_stale_conflict_retires_intent_for_later_explicit_restore() {
    let workspace = tempfile::tempdir().unwrap();
    let (mut api, execution) = test_api_with_recording_backend(workspace.path()).await;
    execution.accept_restores();
    for domain in ["orchestrator-restore", "subject-restore"] {
        let identity = spawn_ticket_check_source(&api, domain);
        let handle =
            WorkspaceWorker::resolve(&api, &identity.runtime_id, &identity.worker_id).unwrap();
        handle
            .stop(&WorkerOperationContext::Backend, operation_lifecycle())
            .await
            .unwrap();
        let observed = api.runtime.worker(&identity).unwrap();
        let pinned = api
            .config_store
            .pin_internal_worker_restore_intent(
                api.workspace_id(),
                &identity,
                domain,
                observed.restore_observation_token.as_deref(),
                None,
            )
            .unwrap();
        // A distinct operation wins between the internal observation/pin and admission.
        let competing = restore_request_for(&api, &identity, &format!("{domain}-competing"));
        assert_eq!(
            handle
                .restore(&WorkerOperationContext::Backend, competing)
                .await
                .unwrap()
                .state,
            server_api::WorkerRestoreState::Accepted
        );
        handle
            .stop(&WorkerOperationContext::Backend, operation_lifecycle())
            .await
            .unwrap();
        let count = execution.restore_operations.lock().unwrap().len();
        let error = restore_internal_worker_intent(&api, &identity, domain.into()).unwrap_err();
        assert!(matches!(error.error, Error::RestoreObservationConflict));
        assert_eq!(
            execution.restore_operations.lock().unwrap().len(),
            count,
            "a conflict must not automatically exchange observation and retry"
        );
        assert_eq!(api.runtime.worker(&identity).unwrap().state, "stopped");
        // Retirement, unlike in-memory refresh, must survive caller journal restart.
        api.config_store =
            Arc::new(SqliteWorkspaceStore::open(api.config.database_path.clone()).unwrap());
        let fresh = api.runtime.worker(&identity).unwrap();
        let next = api
            .config_store
            .pin_internal_worker_restore_intent(
                api.workspace_id(),
                &identity,
                domain,
                fresh.restore_observation_token.as_deref(),
                None,
            )
            .unwrap();
        assert_ne!(pinned.request_id, next.request_id);
        assert_ne!(
            pinned.expected_observation_token,
            next.expected_observation_token
        );
        assert_eq!(
            next.expected_observation_token,
            fresh.restore_observation_token.unwrap()
        );
        // Only this separate deliberate invocation dispatches the new intent.
        assert_eq!(
            restore_internal_worker_intent(&api, &identity, domain.into())
                .unwrap()
                .state,
            server_api::WorkerRestoreState::Accepted
        );
        assert_eq!(
            execution.restore_operations.lock().unwrap().len(),
            count + 1
        );
        // Explicit historical identity is immutable, never rewritten to the new token.
        let original = api
            .config_store
            .pin_internal_worker_restore_intent(
                api.workspace_id(),
                &identity,
                domain,
                None,
                Some(&pinned.request_id),
            )
            .unwrap();
        assert_eq!(original, pinned);
    }
}

#[tokio::test]
async fn restore_guard_internal_conflict_same_generation_has_distinct_fresh_intents() {
    let workspace = tempfile::tempdir().unwrap();
    let (api, execution) = test_api_with_recording_backend(workspace.path()).await;
    execution.accept_restores();
    let identity = spawn_ticket_check_source(&api, "internal-other-owner-pending");
    let handle = WorkspaceWorker::resolve(&api, &identity.runtime_id, &identity.worker_id).unwrap();
    handle
        .stop(&WorkerOperationContext::Backend, operation_lifecycle())
        .await
        .unwrap();
    let original = restore_request_for(&api, &identity, "original-pending-owner");
    *execution.restore_failure.lock().unwrap() = Some(
        worker_runtime::execution::WorkerExecutionSpawnResult::Errored(
            worker_runtime::execution::WorkerExecutionResult::errored(
                worker_runtime::execution::WorkerExecutionOperation::Restore,
                "unknown delivery",
            ),
        ),
    );
    assert_eq!(
        handle
            .restore(&WorkerOperationContext::Backend, original.clone())
            .await
            .unwrap()
            .state,
        server_api::WorkerRestoreState::ReconciliationRequired
    );
    let operation = execution.restore_operations.lock().unwrap()[0];
    let observed = api.runtime.worker(&identity).unwrap();
    let count = execution.restore_operations.lock().unwrap().len();
    for domain in ["orchestrator-restore", "subject-restore"] {
        let mut prior = None;
        for _ in 0..2 {
            let pinned = api
                .config_store
                .pin_internal_worker_restore_intent(
                    api.workspace_id(),
                    &identity,
                    domain,
                    observed.restore_observation_token.as_deref(),
                    None,
                )
                .unwrap();
            if let Some(prior) = prior.replace(pinned.clone()) {
                assert_ne!(pinned.request_id, prior.request_id);
                assert_eq!(
                    pinned.expected_observation_token,
                    prior.expected_observation_token
                );
            }
            let error = restore_internal_worker_intent(&api, &identity, domain.into()).unwrap_err();
            assert!(matches!(error.error, Error::RestoreObservationConflict));
            assert_eq!(execution.restore_operations.lock().unwrap().len(), count);
        }
    }
    // Retiring competing rejected intents does not retire or replace an admitted owner.
    assert_eq!(
        handle
            .restore(&WorkerOperationContext::Backend, original)
            .await
            .unwrap()
            .state,
        server_api::WorkerRestoreState::Accepted
    );
    assert_eq!(
        execution.reconcile_operations.lock().unwrap().as_slice(),
        &[operation]
    );
    assert!(
        execution
            .restore_operations
            .lock()
            .unwrap()
            .iter()
            .all(|id| *id == operation)
    );
}

#[tokio::test]
async fn restore_guard_internal_intent_survives_client_store_restart_without_replacing_token() {
    let workspace = tempfile::tempdir().unwrap();
    let (mut api, execution) = test_api_with_recording_backend(workspace.path()).await;
    execution.accept_restores();
    let identity = spawn_ticket_check_source(&api, "restore-internal-restart");
    WorkspaceWorker::resolve(&api, &identity.runtime_id, &identity.worker_id)
        .unwrap()
        .stop(&WorkerOperationContext::Backend, operation_lifecycle())
        .await
        .unwrap();
    let observed = api.runtime.worker(&identity).unwrap();
    *execution.restore_failure.lock().unwrap() = Some(
        worker_runtime::execution::WorkerExecutionSpawnResult::Errored(
            worker_runtime::execution::WorkerExecutionResult::rejected(
                worker_runtime::execution::WorkerExecutionOperation::Restore,
                "uncertain launch",
            ),
        ),
    );
    assert_eq!(
        restore_internal_observed_worker(&api, &observed, "subject-restore".into())
            .unwrap()
            .state,
        server_api::WorkerRestoreState::ReconciliationRequired
    );
    let original = execution.restore_operations.lock().unwrap()[0];
    let current = api.runtime.worker(&identity).unwrap();
    assert_ne!(
        observed.restore_observation_token,
        current.restore_observation_token
    );
    // Reopen the durable caller journal, discarding all client-side memory of request identity.
    api.config_store =
        Arc::new(SqliteWorkspaceStore::open(api.config.database_path.clone()).unwrap());
    let pinned = api
        .config_store
        .pin_internal_worker_restore_intent(
            api.workspace_id(),
            &identity,
            "subject-restore",
            current.restore_observation_token.as_deref(),
            None,
        )
        .unwrap();
    assert_eq!(
        pinned.expected_observation_token,
        observed.restore_observation_token.unwrap()
    );
    assert_eq!(
        restore_internal_observed_worker(&api, &current, "subject-restore".into())
            .unwrap()
            .state,
        server_api::WorkerRestoreState::Accepted
    );
    assert_eq!(
        execution.reconcile_operations.lock().unwrap().as_slice(),
        &[original]
    );
    assert!(
        execution
            .restore_operations
            .lock()
            .unwrap()
            .iter()
            .all(|id| id == &original)
    );
}

#[tokio::test]
async fn restore_guard_workspace_interrupted_preparation_recovers_admitted_snapshot() {
    let workspace = tempfile::tempdir().unwrap();
    let (api, execution) = test_api_with_recording_backend(workspace.path()).await;
    execution.accept_restores();
    let identity = spawn_ticket_check_source(&api, "restore-prepare-interrupted");
    let replacement = spawn_ticket_check_source(&api, "restore-prepare-replacement");
    let handle = WorkspaceWorker::resolve(&api, &identity.runtime_id, &identity.worker_id).unwrap();
    handle
        .stop(&WorkerOperationContext::Backend, operation_lifecycle())
        .await
        .unwrap();
    let request = restore_request_for(&api, &identity, "restore-prepare-interrupted-request");
    let preparation = runtime_api::WorkerRestorePreparation {
        workspace_api: Some(
            runtime_contract_restore(api.workspace_api_ref(&identity.runtime_id)).unwrap(),
        ),
        workdir_attachments: Some(Vec::new()),
        ..Default::default()
    };
    let (result, retained) = api
        .runtime
        .coordinate_worker_restore(
            &identity,
            runtime_api::WorkerRestoreCoordinationRequest {
                expected_observation_token: request.expected_observation_token.clone(),
                request_id: request.request_id.clone(),
                preparation: Some(preparation.clone()),
            },
        )
        .unwrap();
    assert!(result.is_none());
    assert_eq!(retained, Some(preparation));
    assert!(execution.restore_operations.lock().unwrap().is_empty());
    // Model interruption between admission and completion, then changed current eligibility.
    displace_restore_singleton(&api, &identity, &replacement);
    add_offline_restore_workdir(&api, &identity);
    let restored = handle
        .restore(&WorkerOperationContext::Backend, request.clone())
        .await
        .unwrap();
    assert_eq!(restored.state, server_api::WorkerRestoreState::Accepted);
    assert_eq!(execution.restore_operations.lock().unwrap().len(), 1);
    let original = execution.restore_operations.lock().unwrap()[0];
    assert!(
        execution.reconcile_operations.lock().unwrap().is_empty(),
        "no execution existed to reconcile yet"
    );
    assert_eq!(
        handle
            .restore(&WorkerOperationContext::Backend, request)
            .await
            .unwrap()
            .state,
        server_api::WorkerRestoreState::Accepted
    );
    assert_eq!(
        execution.restore_operations.lock().unwrap().as_slice(),
        &[original]
    );
}

#[tokio::test]
async fn restore_guard_cross_runtime_ssh_completion_retries_pinned_snapshot_at_workdir_owner() {
    let workspace = tempfile::tempdir().unwrap();
    let mut config = test_server_config(workspace.path());
    let source = server_api::RepositorySource {
        kind: server_api::RepositorySourceKind::Ssh,
        uri: "git@example.invalid:owner/repository.git".into(),
    };
    config.repositories[0].source_fingerprint =
        crate::repository_source::repository_source_fingerprint(&source);
    config.repositories[0].source = source;
    config.repositories[0].path = None;
    let store = SqliteWorkspaceStore::open(config.database_path.clone()).unwrap();
    let execution = Arc::new(DeterministicExecutionBackend::default());
    execution.accept_restores();
    let api = WorkspaceApi::new_with_execution_backend(config, Arc::new(store), execution.clone())
        .await
        .unwrap();
    let peer = WorkdirlessFixtureRuntime::default();
    *peer.reject_repository_access.lock().unwrap() = true;
    api.runtime.register_or_replace(peer.clone());
    let identity = spawn_ticket_check_source(&api, "restore-cross-runtime-ssh");
    let handle = WorkspaceWorker::resolve(&api, &identity.runtime_id, &identity.worker_id).unwrap();
    handle
        .stop(&WorkerOperationContext::Backend, operation_lifecycle())
        .await
        .unwrap();
    let original_key = api
        .repository_secrets
        .generate_credential(
            api.workspace_id(),
            GenerateRepositorySshCredentialRequest {
                operation_id: "cross-runtime-default-key".into(),
                credential_id:
                    crate::repository_access::WORKSPACE_DEFAULT_REPOSITORY_SSH_CREDENTIAL_ID.into(),
                name: "Restore default".into(),
            },
            "owner-account",
        )
        .unwrap();
    let public_key = api
        .repository_secrets
        .credential_public_key(api.workspace_id(), &original_key.credential_id)
        .unwrap()
        .unwrap()
        .public_key;
    api.repository_secrets
        .put_host_trust(
            api.workspace_id(),
            PutRepositorySshHostTrustRequest {
                operation_id: "cross-runtime-trust".into(),
                host_trust_id: "cross-runtime-host".into(),
                hostname: "example.invalid".into(),
                port: 22,
                host_key: public_key,
                expected_revision: None,
            },
            "owner-account",
        )
        .unwrap();
    let record = restore_test_workdir(
        &api,
        "cross-runtime-restore-workdir",
        WorkdirRegistrySource::Repository {
            runtime_id: WorkdirlessFixtureRuntime::RUNTIME_ID.into(),
            repository_id: test_repository_id(&api),
        },
    );
    peer.set_workdir_repository(&record.workdir_id, &test_repository_id(&api));
    api.store.upsert_workdir_registry(&record).unwrap();
    attach_restore_test_workdir(&api, &identity, &record);
    let request = restore_request_for(&api, &identity, "cross-runtime-restore-request");
    assert_eq!(
        handle
            .restore(&WorkerOperationContext::Backend, request.clone())
            .await
            .unwrap()
            .state,
        server_api::WorkerRestoreState::ReconciliationRequired
    );
    assert!(
        execution.restore_operations.lock().unwrap().is_empty(),
        "credential completion must precede execution"
    );
    let first = peer.repository_access_requests.lock().unwrap()[0].clone();
    assert_eq!(
        first.materialization.runtime_id,
        WorkdirlessFixtureRuntime::RUNTIME_ID
    );
    assert_ne!(first.materialization.runtime_id, identity.runtime_id);
    // Rotate current host trust while the accepted operation is uncertain. Its
    // original credential/trust revisions, source and binding must remain pinned.
    let alternate = api
        .repository_secrets
        .generate_credential(
            api.workspace_id(),
            GenerateRepositorySshCredentialRequest {
                operation_id: "cross-runtime-alternate-key".into(),
                credential_id: "cross-runtime-alternate".into(),
                name: "Alternate".into(),
            },
            "owner-account",
        )
        .unwrap();
    let alternate_public = api
        .repository_secrets
        .credential_public_key(api.workspace_id(), &alternate.credential_id)
        .unwrap()
        .unwrap()
        .public_key;
    let original_trust_revision = first
        .materialization
        .ssh
        .as_ref()
        .unwrap()
        .host_trust_revision;
    api.repository_secrets
        .put_host_trust(
            api.workspace_id(),
            PutRepositorySshHostTrustRequest {
                operation_id: "cross-runtime-trust-rotate".into(),
                host_trust_id: "cross-runtime-host".into(),
                hostname: "example.invalid".into(),
                port: 22,
                host_key: alternate_public,
                expected_revision: Some(original_trust_revision),
            },
            "owner-account",
        )
        .unwrap();
    *peer.reject_repository_access.lock().unwrap() = false;
    assert_eq!(
        handle
            .restore(&WorkerOperationContext::Backend, request.clone())
            .await
            .unwrap()
            .state,
        server_api::WorkerRestoreState::Accepted
    );
    let attempts = peer.repository_access_requests.lock().unwrap();
    assert_eq!(attempts.len(), 2);
    let second = &attempts[1];
    assert_eq!(
        second.materialization.operation_id,
        first.materialization.operation_id
    );
    let mut normalized = second.clone();
    let old_ssh = first.materialization.ssh.as_ref().unwrap();
    let new_ssh = normalized.materialization.ssh.as_mut().unwrap();
    assert_ne!(new_ssh.secret_resource.nonce, old_ssh.secret_resource.nonce);
    new_ssh.secret_resource = old_ssh.secret_resource.clone();
    new_ssh.expires_at_epoch_seconds = old_ssh.expires_at_epoch_seconds;
    assert_eq!(normalized, first, "only opaque handle and expiry may renew");
    drop(attempts);
    assert_eq!(execution.restore_operations.lock().unwrap().len(), 1);
    assert_eq!(
        handle
            .restore(&WorkerOperationContext::Backend, request)
            .await
            .unwrap()
            .state,
        server_api::WorkerRestoreState::Accepted
    );
    assert_eq!(
        peer.repository_access_requests.lock().unwrap().len(),
        2,
        "terminal receipt must not renew access"
    );
}
