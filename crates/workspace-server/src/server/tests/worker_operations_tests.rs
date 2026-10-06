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
            .restore(&WorkerOperationContext::Backend)
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
        handle.restore(&context).await.unwrap_err().error,
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
            .runtime_worker_restore_alias(context(), "offline-runtime".into(), "subject".into())
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
