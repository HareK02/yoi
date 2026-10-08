use super::*;

async fn claim(
    fixture: &ManualCoderAssignmentFixture,
    request: server_api::SetTicketRoleAssignmentRequest,
) -> ApiResult<Json<server_api::TicketRoleAssignmentMutationResponse>> {
    scoped_set_ticket_assignment(
        State(fixture.api.clone()),
        AxumPath((
            TEST_WORKSPACE_ID.into(),
            fixture.ticket_id.clone(),
            "worker".into(),
        )),
        Json(request),
    )
    .await
}

#[tokio::test]
async fn explicit_existing_worker_claim_accepts_no_resources_and_preserves_assignment_on_retry() {
    let fixture = manual_worker_assignment_fixture().await;
    for id in [&fixture.main_workdir_id, &fixture.docs_workdir_id] {
        fixture
            .api
            .store
            .detach_worker_workdir(
                TEST_WORKSPACE_ID,
                &fixture.worker,
                Some(id),
                TEST_CREATED_AT,
            )
            .unwrap();
    }
    sync_runtime_worker_workdir_attachments(&fixture.api, &fixture.worker).unwrap();
    let backend = browser_ticket_backend(&fixture.api).unwrap();
    backend
        .edit_item(
            fixture.ticket_id.clone().into(),
            TicketItemEdit {
                targets: Some(TicketTargetsEdit::Clear),
                ..Default::default()
            },
        )
        .unwrap();
    backend
        .set_workflow_state(
            fixture.ticket_id.clone().into(),
            TicketStateChange::new(
                "ready",
                "planning",
                "Explicit investigation",
                "No repository resources required",
            ),
        )
        .unwrap();
    let request = manual_worker_assignment_request(&fixture, "claim-without-resources");
    assert!(request.workdir_bindings.is_empty());
    let Json(first) = claim(&fixture, request.clone()).await.unwrap();
    let Json(replay) = claim(&fixture, request).await.unwrap();
    assert_eq!(
        first.assignment.unwrap().assignment_id,
        replay.assignment.unwrap().assignment_id
    );
    assert_eq!(
        fixture
            .api
            .authority
            .ticket(&fixture.ticket_id)
            .unwrap()
            .state,
        "inprogress"
    );
    assert_eq!(*fixture.runtime.restore_attempts.lock().unwrap(), 1);
    assert!(
        fixture
            .runtime
            .logical_attachments(&fixture.worker.worker_id)
            .is_empty()
    );
}

#[tokio::test]
async fn explicit_existing_worker_claim_narrows_read_only_targets_without_orchestrator_conflict() {
    let fixture = manual_worker_assignment_fixture().await;
    let backend = browser_ticket_backend(&fixture.api).unwrap();
    backend
        .edit_item(
            fixture.ticket_id.clone().into(),
            TicketItemEdit {
                targets: Some(TicketTargetsEdit::Set {
                    targets: vec![
                        test_ticket_target(
                            "test-repository",
                            "develop",
                            TicketTargetAccess::ReadOnly,
                        ),
                        test_ticket_target("docs", "develop", TicketTargetAccess::ReadOnly),
                    ],
                }),
                ..Default::default()
            },
        )
        .unwrap();
    assign_test_orchestrator(&fixture.api, &fixture.ticket_id);
    let request = manual_worker_assignment_request(&fixture, "claim-read-only");
    let _ = claim(&fixture, request).await.unwrap();
    for link in fixture
        .api
        .store
        .list_worker_workdir_links(TEST_WORKSPACE_ID, &fixture.worker)
        .unwrap()
        .into_iter()
        .filter(|link| link.unlinked_at.is_none())
    {
        assert_eq!(
            link.capabilities,
            workdir::WorkdirSessionCapabilities::READ_ONLY
        );
        let workdir = fixture
            .api
            .store
            .get_workdir_registry(TEST_WORKSPACE_ID, &link.workdir_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            effective_worker_workdir_capabilities(
                &fixture.api,
                &fixture.worker,
                &workdir,
                link.capabilities
            )
            .unwrap(),
            workdir::WorkdirSessionCapabilities::READ_ONLY
        );
    }
    assert!(
        fixture
            .runtime
            .logical_attachments(&fixture.worker.worker_id)
            .iter()
            .all(|a| a.capabilities == workdir::WorkdirSessionCapabilities::READ_ONLY)
    );
    assert!(
        require_ticket_read_write_target(
            &fixture.api,
            &fixture.ticket_id,
            &test_repository_id(&fixture.api),
            "develop"
        )
        .is_err()
    );
}

#[tokio::test]
async fn explicit_existing_worker_claim_requires_connections_instead_of_ambient_attachments() {
    let fixture = manual_worker_assignment_fixture().await;
    let mut request = manual_worker_assignment_request(&fixture, "claim-ambient-denied");
    request.workdir_bindings.clear();
    let response = claim(&fixture, request).await.unwrap_err().into_response();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    assert!(String::from_utf8_lossy(&bytes).contains("exact live Workdir connections"));
    assert_eq!(*fixture.runtime.restore_attempts.lock().unwrap(), 0);
    assert!(
        fixture
            .api
            .store
            .get_ticket_assignment_operation(TEST_WORKSPACE_ID, "claim-ambient-denied")
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn ambiguous_existing_worker_restore_keeps_claim_pending_until_accepted_recovery() {
    let fixture = manual_worker_assignment_fixture().await;
    *fixture.runtime.next_restore_outcome.lock().unwrap() =
        Some(server_api::WorkerRestoreState::ReconciliationRequired);
    let request = manual_worker_assignment_request(&fixture, "claim-unknown");
    assert!(claim(&fixture, request.clone()).await.is_err());
    let operation = fixture
        .api
        .store
        .get_ticket_assignment_operation(TEST_WORKSPACE_ID, "claim-unknown")
        .unwrap()
        .unwrap();
    assert_eq!(operation.claim_state, "pending");
    assert!(operation.assignment_id.is_none());
    assert_eq!(operation.worker, Some(fixture.worker.clone()));
    assert!(operation.request_fingerprint.is_some());
    let recovery = fixture
        .api
        .config_store
        .get_ticket_claim_recovery(TEST_WORKSPACE_ID, "claim-unknown")
        .unwrap()
        .unwrap();
    assert_eq!(
        recovery.item_revision,
        fixture
            .api
            .authority
            .ticket(&fixture.ticket_id)
            .unwrap()
            .item_revision
    );
    assert_eq!(recovery.selected.len(), request.workdir_bindings.len());
    for selected in &request.workdir_bindings {
        assert!(recovery.selected.contains(selected));
    }
    assert!(
        recovery
            .original_links
            .iter()
            .all(|link| link.capabilities == workdir::WorkdirSessionCapabilities::ALL)
    );
    assert_eq!(
        recovery
            .effective_links
            .iter()
            .find(|link| link.alias == "docs")
            .unwrap()
            .capabilities,
        workdir::WorkdirSessionCapabilities::READ_ONLY
    );
    // This fake proves the domain transition, not execution-level restore deduplication.
    *fixture.runtime.next_restore_outcome.lock().unwrap() =
        Some(server_api::WorkerRestoreState::ReconciliationRequired);
    assert!(claim(&fixture, request.clone()).await.is_err());
    let pending = fixture
        .api
        .store
        .get_ticket_assignment_operation(TEST_WORKSPACE_ID, "claim-unknown")
        .unwrap()
        .unwrap();
    assert_eq!(pending.claim_state, "pending");
    assert!(pending.assignment_id.is_none());
    assert_eq!(pending.request_fingerprint, operation.request_fingerprint);
    assert_eq!(*fixture.runtime.restore_attempts.lock().unwrap(), 2);
    assert!(
        fixture
            .api
            .store
            .reserve_ticket_assignment_operation(
                TEST_WORKSPACE_ID,
                "competing-launch",
                &fixture.ticket_id,
                &fixture.worker.runtime_id,
                None,
                "other-intent",
                TEST_CREATED_AT
            )
            .is_err()
    );
    let worker = WorkspaceWorker::resolve(
        &fixture.api,
        &fixture.worker.runtime_id,
        &fixture.worker.worker_id,
    )
    .unwrap();
    let denied = worker
        .input(
            &WorkerOperationContext::Backend,
            WorkerInputRequest {
                kind: WorkerInputKind::User,
                content: "Do not run while claim is unresolved".into(),
                segments: None,
                submission_request_id: None,
            },
        )
        .await
        .unwrap_err();
    assert!(denied.error.to_string().contains("pending Ticket claim"));
    assert_eq!(
        fixture
            .api
            .authority
            .ticket(&fixture.ticket_id)
            .unwrap()
            .state,
        "ready"
    );
    assert!(
        fixture
            .api
            .store
            .get_active_ticket_worker_assignment(TEST_WORKSPACE_ID, &fixture.ticket_id)
            .unwrap()
            .is_none()
    );

    let Json(accepted) = claim(&fixture, request.clone()).await.unwrap();
    let assignment = accepted.assignment.unwrap();
    let committed = fixture
        .api
        .store
        .get_ticket_assignment_operation(TEST_WORKSPACE_ID, "claim-unknown")
        .unwrap()
        .unwrap();
    assert_eq!(committed.claim_state, "committed");
    assert_eq!(
        committed.assignment_id,
        Some(assignment.assignment_id.clone())
    );
    assert_eq!(
        fixture
            .api
            .authority
            .ticket(&fixture.ticket_id)
            .unwrap()
            .state,
        "inprogress"
    );
    assert_eq!(*fixture.runtime.restore_attempts.lock().unwrap(), 3);
    assert_eq!(
        fixture
            .api
            .config_store
            .get_ticket_claim_recovery(TEST_WORKSPACE_ID, "claim-unknown")
            .unwrap()
            .unwrap(),
        recovery
    );
    let Json(replay) = claim(&fixture, request).await.unwrap();
    assert_eq!(
        replay.assignment.unwrap().assignment_id,
        assignment.assignment_id
    );
    assert_eq!(*fixture.runtime.restore_attempts.lock().unwrap(), 3);
}

#[tokio::test]
async fn pending_manual_worker_claim_rejects_changed_bindings_without_restore() {
    let fixture = manual_worker_assignment_fixture().await;
    *fixture.runtime.next_restore_outcome.lock().unwrap() =
        Some(server_api::WorkerRestoreState::ReconciliationRequired);
    let mut request = manual_worker_assignment_request(&fixture, "claim-changed-bindings");
    assert!(claim(&fixture, request.clone()).await.is_err());
    let admitted = fixture
        .api
        .store
        .get_ticket_assignment_operation(TEST_WORKSPACE_ID, &request.operation_id)
        .unwrap()
        .unwrap();
    request.workdir_bindings[0].connection_id = "changed-connection".into();
    let response = claim(&fixture, request.clone())
        .await
        .unwrap_err()
        .into_response();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    assert!(String::from_utf8_lossy(&bytes).contains("different input"));
    let pending = fixture
        .api
        .store
        .get_ticket_assignment_operation(TEST_WORKSPACE_ID, &request.operation_id)
        .unwrap()
        .unwrap();
    assert_eq!(pending.claim_state, "pending");
    assert!(pending.assignment_id.is_none());
    assert_eq!(pending.request_fingerprint, admitted.request_fingerprint);
    assert_eq!(*fixture.runtime.restore_attempts.lock().unwrap(), 1);
}

#[tokio::test]
async fn pending_manual_worker_claim_without_pinned_intent_rejects_replay_without_restore() {
    let fixture = manual_worker_assignment_fixture().await;
    *fixture.runtime.next_restore_outcome.lock().unwrap() =
        Some(server_api::WorkerRestoreState::ReconciliationRequired);
    let request = manual_worker_assignment_request(&fixture, "claim-missing-intent");
    assert!(claim(&fixture, request.clone()).await.is_err());
    // Simulate an interrupted/legacy pending receipt with no exact restore intent.
    let request_id = format!("manual-worker-restore:{}", request.operation_id);
    fixture.api.config_store.with_conn(|conn| {
        let deleted = conn.execute(
            "DELETE FROM worker_restore_intents WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3 AND request_id=?4",
            rusqlite::params![TEST_WORKSPACE_ID, fixture.worker.runtime_id, fixture.worker.worker_id, request_id],
        )?;
        assert_eq!(deleted, 1);
        Ok(())
    }).unwrap();
    let response = claim(&fixture, request.clone())
        .await
        .unwrap_err()
        .into_response();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    assert!(String::from_utf8_lossy(&bytes).contains("no pinned restore intent"));
    let pending = fixture
        .api
        .store
        .get_ticket_assignment_operation(TEST_WORKSPACE_ID, &request.operation_id)
        .unwrap()
        .unwrap();
    assert_eq!(pending.claim_state, "pending");
    assert!(pending.assignment_id.is_none());
    assert_eq!(*fixture.runtime.restore_attempts.lock().unwrap(), 1);
    assert!(
        fixture
            .api
            .config_store
            .get_internal_worker_restore_intent(TEST_WORKSPACE_ID, &fixture.worker, &request_id)
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn pending_manual_worker_claim_reconciles_same_runtime_restore_and_commits_without_input() {
    let workspace = tempfile::tempdir().unwrap();
    let (api, execution) = test_api_with_recording_backend(workspace.path()).await;
    execution.accept_restores();
    let worker = spawn_ticket_check_source(&api, "manual-claim-reconciliation");
    assert!(
        api.store
            .list_worker_workdir_links(TEST_WORKSPACE_ID, &worker)
            .unwrap()
            .is_empty()
    );
    let backend = browser_ticket_backend(&api).unwrap();
    let mut input = ticket::NewTicket::new("Manual investigation without resources");
    input.workflow_state = Some(TicketWorkflowState::Ready);
    let ticket_id = backend.create(input).unwrap().id;
    backend
        .edit_item(
            ticket_id.clone().into(),
            TicketItemEdit {
                targets: Some(TicketTargetsEdit::Clear),
                ..Default::default()
            },
        )
        .unwrap();
    backend
        .set_workflow_state(
            ticket_id.clone().into(),
            TicketStateChange::new(
                "ready",
                "planning",
                "Explicit investigation",
                "No repository resources required",
            ),
        )
        .unwrap();
    let admitted_revision = api.authority.ticket(&ticket_id).unwrap().item_revision;
    let request = server_api::SetTicketRoleAssignmentRequest {
        operation_id: "claim-runtime-reconciliation".into(),
        principal: server_api::TicketAssignmentPrincipal::Worker {
            runtime_id: worker.runtime_id.clone(),
            worker_id: worker.worker_id.clone(),
        },
        expected_assignment_id: None,
        workdir_bindings: Vec::new(),
    };
    let submit_claim = || {
        scoped_set_ticket_assignment(
            State(api.clone()),
            AxumPath((TEST_WORKSPACE_ID.into(), ticket_id.clone(), "worker".into())),
            Json(request.clone()),
        )
    };
    let uncertain_restore = || {
        worker_runtime::execution::WorkerExecutionSpawnResult::Errored(
            worker_runtime::execution::WorkerExecutionResult::rejected(
                worker_runtime::execution::WorkerExecutionOperation::Restore,
                "uncertain manual claim restore",
            ),
        )
    };
    *execution.restore_failure.lock().unwrap() = Some(uncertain_restore());
    let response = submit_claim().await.unwrap_err().into_response();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    assert!(String::from_utf8_lossy(&bytes).contains("unknown"));
    let pending = api
        .store
        .get_ticket_assignment_operation(TEST_WORKSPACE_ID, &request.operation_id)
        .unwrap()
        .unwrap();
    assert_eq!(pending.claim_state, "pending");
    assert!(pending.assignment_id.is_none());
    assert_eq!(pending.worker, Some(worker.clone()));
    assert!(pending.request_fingerprint.is_some());
    assert_eq!(api.authority.ticket(&ticket_id).unwrap().state, "planning");
    assert!(
        api.store
            .get_active_ticket_worker_assignment(TEST_WORKSPACE_ID, &ticket_id)
            .unwrap()
            .is_none()
    );
    let recovery = api
        .config_store
        .get_ticket_claim_recovery(TEST_WORKSPACE_ID, &request.operation_id)
        .unwrap()
        .unwrap();
    assert_eq!(recovery.item_revision, admitted_revision);
    assert!(recovery.selected.is_empty());
    assert!(recovery.original_links.is_empty());
    assert!(recovery.effective_links.is_empty());
    let request_id = format!("manual-worker-restore:{}", request.operation_id);
    let pinned = api
        .config_store
        .get_internal_worker_restore_intent(TEST_WORKSPACE_ID, &worker, &request_id)
        .unwrap()
        .unwrap();
    assert_eq!(pinned.request_id, request_id);
    let original = execution.restore_operations.lock().unwrap()[0];
    assert_eq!(
        execution.restore_operations.lock().unwrap().as_slice(),
        &[original]
    );
    assert!(execution.reconcile_operations.lock().unwrap().is_empty());

    let handle = WorkspaceWorker::resolve(&api, &worker.runtime_id, &worker.worker_id).unwrap();
    let denied = handle
        .input(
            &WorkerOperationContext::Backend,
            WorkerInputRequest {
                kind: WorkerInputKind::User,
                content: "Do not submit while the manual claim is pending".into(),
                segments: None,
                submission_request_id: None,
            },
        )
        .await
        .unwrap_err();
    assert!(denied.error.to_string().contains("pending Ticket claim"));

    *execution.restore_failure.lock().unwrap() = Some(uncertain_restore());
    let response = submit_claim().await.unwrap_err().into_response();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    assert!(String::from_utf8_lossy(&bytes).contains("pending"));
    let still_pending = api
        .store
        .get_ticket_assignment_operation(TEST_WORKSPACE_ID, &request.operation_id)
        .unwrap()
        .unwrap();
    assert_eq!(still_pending.claim_state, "pending");
    assert!(still_pending.assignment_id.is_none());
    assert_eq!(
        still_pending.request_fingerprint,
        pending.request_fingerprint
    );
    assert_eq!(
        api.authority.ticket(&ticket_id).unwrap().item_revision,
        admitted_revision
    );
    assert_eq!(
        api.config_store
            .get_internal_worker_restore_intent(TEST_WORKSPACE_ID, &worker, &request_id)
            .unwrap()
            .unwrap(),
        pinned
    );
    assert_eq!(
        execution.reconcile_operations.lock().unwrap().as_slice(),
        &[original]
    );

    let Json(accepted) = submit_claim().await.unwrap();
    let assignment = accepted.assignment.unwrap();
    assert_eq!(assignment.principal, request.principal);
    let committed = api
        .store
        .get_ticket_assignment_operation(TEST_WORKSPACE_ID, &request.operation_id)
        .unwrap()
        .unwrap();
    assert_eq!(committed.claim_state, "committed");
    assert_eq!(
        committed.assignment_id,
        Some(assignment.assignment_id.clone())
    );
    assert_eq!(committed.request_fingerprint, pending.request_fingerprint);
    assert_eq!(
        api.authority.ticket(&ticket_id).unwrap().state,
        "inprogress"
    );
    assert_eq!(
        api.store
            .get_active_ticket_worker_assignment(TEST_WORKSPACE_ID, &ticket_id)
            .unwrap()
            .unwrap()
            .assignment_id,
        assignment.assignment_id
    );
    assert_eq!(
        api.config_store
            .get_ticket_claim_recovery(TEST_WORKSPACE_ID, &request.operation_id)
            .unwrap()
            .unwrap(),
        recovery
    );
    assert!(
        api.store
            .list_worker_workdir_links(TEST_WORKSPACE_ID, &worker)
            .unwrap()
            .is_empty()
    );
    // The executor logs reconcile_restore in restore_operations as well. There is
    // exactly one fresh dispatch; both subsequent entries reconcile its same id.
    assert_eq!(
        execution.restore_operations.lock().unwrap().as_slice(),
        &[original, original, original]
    );
    assert_eq!(
        execution.reconcile_operations.lock().unwrap().as_slice(),
        &[original, original]
    );
    let Json(replay) = submit_claim().await.unwrap();
    assert_eq!(
        replay.assignment.unwrap().assignment_id,
        assignment.assignment_id
    );
    assert_eq!(execution.restore_operations.lock().unwrap().len(), 3);
    assert_eq!(execution.reconcile_operations.lock().unwrap().len(), 2);
    assert!(
        !execution
            .protocol_methods()
            .iter()
            .any(|(_, method)| matches!(
                method,
                protocol::Method::Submit { .. }
                    | protocol::Method::SubmitTracked { .. }
                    | protocol::Method::SubmitIfIdle { .. }
            )),
        "manual claims and denied input must not generate a Submit"
    );
}
