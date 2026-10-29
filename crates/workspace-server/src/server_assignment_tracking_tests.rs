use super::*;

#[tokio::test]
async fn runtime_and_account_assign_retries_return_saved_result_after_clear_and_coder_start() {
    for runtime_source in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let (api, _, identity, source, ticket) = tracking_fixture(directory.path()).await;
        let token =
            seed_test_workspace_owner_token(api.store.as_ref(), "assign-replay-owner").await;
        let request = |method: &str, path: &str, body: Vec<u8>| {
            if runtime_source {
                runtime_source_request(&identity, Some(&source.worker_id), method, path, body)
            } else {
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header(CONTENT_TYPE, "application/json")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::from(body))
                    .unwrap()
            }
        };
        let app = build_router(api.clone());
        let path = format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignments/orchestrator");
        let input = |key: &str| {
            serde_json::to_vec(&serde_json::json!({
                "operation_id": key,
                "principal": test_assignment_principal(&api)
            }))
            .unwrap()
        };
        let saved = tracking_response(
            app.clone(),
            request("PUT", &path, input("saved-assign")),
            StatusCode::OK,
        )
        .await;
        let assignment = saved["assignment"]["assignment_id"].as_str().unwrap();
        tracking_response(
            app.clone(),
            request(
                "DELETE",
                &assignment_clear_path(&ticket, assignment, "clear-before-coder"),
                Vec::new(),
            ),
            StatusCode::OK,
        )
        .await;
        let Json(candidate) = create_workspace_worker(
            State(api.clone()),
            HeaderMap::new(),
            Json(CreateWorkspaceWorkerRequest {
                runtime_id: source.runtime_id.clone(),
                display_name: "Coder candidate".into(),
                singleton_key: None,
                profile: Some("builtin:coder".into()),
                ticket_assignment: None,
                initial_submit: Vec::new(),
                workdir_attachments: Vec::new(),
                feature_connections: Default::default(),
                control_operation_id: None,
                execution_grant: None,
            }),
        )
        .await
        .unwrap();
        let worker_path = format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignments/worker");
        let worker_input = serde_json::to_vec(&serde_json::json!({
            "operation_id":"start-coder", "principal":{"kind":"worker", "runtime_id":candidate.runtime_id, "worker_id":candidate.worker_id}
        })).unwrap();
        tracking_response(
            app.clone(),
            Request::builder()
                .method("PUT")
                .uri(&worker_path)
                .header(CONTENT_TYPE, "application/json")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::from(worker_input))
                .unwrap(),
            StatusCode::OK,
        )
        .await;
        let active = api
            .store
            .get_active_ticket_role_assignment(
                TEST_WORKSPACE_ID,
                &ticket,
                TicketAssignmentRole::Worker,
            )
            .unwrap()
            .unwrap();
        let retained = api
            .store
            .get_ticket_assignment_tracking(TEST_WORKSPACE_ID, &ticket, assignment)
            .unwrap()
            .unwrap();
        assert!(!retained.current);
        let counts = || {
            let conn = rusqlite::Connection::open(&api.config.database_path).unwrap();
            [
                "ticket_worker_assignments",
                "ticket_current_worker_assignments",
                "ticket_worker_assignment_events",
                "ticket_assignment_operations",
                "typed_ticket_assignment_tracking_history",
                "typed_ticket_assignment_tracking_operations",
                "ticket_assignment_work_releases",
            ]
            .map(|table| {
                conn.query_row(
                    &format!("SELECT count(*) FROM {table} WHERE workspace_id=?1"),
                    [TEST_WORKSPACE_ID],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap()
            })
        };
        let before = counts();
        assert_eq!(
            tracking_response(
                app.clone(),
                request("PUT", &path, input("saved-assign")),
                StatusCode::OK
            )
            .await,
            saved
        );
        // Receipt replay does not recreate a delegation or transfer manual work.
        assert_eq!(counts(), before);
        let fresh = tracking_response(
            app,
            request("PUT", &path, input("new-assign")),
            StatusCode::OK,
        )
        .await;
        assert_ne!(
            fresh["assignment"]["assignment_id"].as_str(),
            Some(assignment)
        );
        assert_eq!(
            api.store
                .get_ticket_assignment_tracking(TEST_WORKSPACE_ID, &ticket, assignment)
                .unwrap()
                .unwrap(),
            retained
        );
        assert_eq!(
            api.store
                .get_active_ticket_role_assignment(
                    TEST_WORKSPACE_ID,
                    &ticket,
                    TicketAssignmentRole::Worker
                )
                .unwrap()
                .unwrap(),
            active
        );
        assert_eq!(
            api.store
                .get_current_ticket_role_assignment(
                    TEST_WORKSPACE_ID,
                    &ticket,
                    TicketAssignmentRole::Orchestrator
                )
                .unwrap()
                .unwrap()
                .assignment_id,
            fresh["assignment"]["assignment_id"].as_str().unwrap()
        );
    }
}

fn tracking_path(ticket: &str) -> String {
    format!("/workspaces/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignment-tracking")
}

fn assignment_request(
    api: &WorkspaceApi,
    identity: &RuntimeIdentityMaterial,
    worker: &str,
    ticket: &str,
    operation: &str,
) -> Request<Body> {
    runtime_source_request(identity, Some(worker), "PUT",
        &format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignments/orchestrator"),
        serde_json::to_vec(&serde_json::json!({"operation_id":operation,"principal":test_assignment_principal(&api)})).unwrap())
}

fn assignment_clear_path(ticket: &str, assignment: &str, operation: &str) -> String {
    format!(
        "/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignments/orchestrator?assignment_id={assignment}&operation_id={operation}"
    )
}

async fn tracking_embedded_fixture(
    root: &Path,
) -> (
    WorkspaceApi,
    RuntimeIdentityMaterial,
    RuntimeWorkerRef,
    String,
) {
    let (mut api, _) = assignment_test_api_with_recording_backend(root).await;
    let identity = RuntimeIdentityMaterial::generate(EMBEDDED_WORKER_RUNTIME_ID).unwrap();
    configure_runtime_request_auth(&mut api, &identity, EMBEDDED_WORKER_RUNTIME_ID);
    let subject_id = ensure_test_assignment_subject(&api);
    let source = create_test_assignment_body(&api, EMBEDDED_WORKER_RUNTIME_ID, &subject_id).await;
    bootstrap_test_assignment_session(&api, &source);
    let ticket = browser_ticket_backend(&api)
        .unwrap()
        .create(ticket::NewTicket::new("Subject body replacement"))
        .unwrap();
    grant_test_assignment_ticket(&api, &subject_id, &ticket.id);
    (api, identity, source, ticket.id)
}

async fn replace_tracking_orchestrator(
    api: &WorkspaceApi,
    source: &RuntimeWorkerRef,
) -> RuntimeWorkerRef {
    // Normal Subject release owns writer revocation and the singleton transition.
    // This fixture substitutes the Controller receiver, not the lifetime protocol.
    remove_test_assignment_body(api, source).await;
    let subject_id = ensure_test_assignment_subject(api);
    let replacement = create_test_assignment_body(api, &source.runtime_id, &subject_id).await;
    bootstrap_test_assignment_session(api, &replacement);
    assert_ne!(*source, replacement);
    assert_eq!(test_assignment_body(api).unwrap().worker, replacement);
    replacement
}

#[tokio::test]
async fn revoked_runtime_cannot_retry_assign_clear_or_record_tracking() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _, identity, source, ticket) = tracking_fixture(dir.path()).await;
    let current = assign_tracking(&api, &identity, &source, &ticket).await;
    let app = build_router(api.clone());
    let clear_path =
        assignment_clear_path(&ticket, &current.assignment_id, "clear-before-revocation");
    tracking_response(
        app.clone(),
        runtime_source_request(
            &identity,
            Some(&source.worker_id),
            "DELETE",
            &clear_path,
            Vec::new(),
        ),
        StatusCode::OK,
    )
    .await;
    let released = api
        .store
        .get_ticket_assignment_tracking(TEST_WORKSPACE_ID, &ticket, &current.assignment_id)
        .unwrap()
        .unwrap();
    let conn = rusqlite::Connection::open(&api.config.database_path).unwrap();
    conn.execute("UPDATE workspace_runtime_bindings SET revoked_at='2026-10-11T00:00:00Z' WHERE workspace_id=?1 AND runtime_id=?2", rusqlite::params![TEST_WORKSPACE_ID, source.runtime_id]).unwrap();
    for request in [
        assignment_request(
            &api,
            &identity,
            &source.worker_id,
            &ticket,
            "assign-tracking",
        ),
        runtime_source_request(
            &identity,
            Some(&source.worker_id),
            "DELETE",
            &clear_path,
            Vec::new(),
        ),
        signed_tracking_update(
            &identity,
            Some(&source.worker_id),
            &ticket,
            &tracking_update(&current, "revoked-report"),
        ),
    ] {
        tracking_response(app.clone(), request, StatusCode::UNAUTHORIZED).await;
    }
    assert_eq!(
        api.store
            .get_ticket_assignment_tracking(TEST_WORKSPACE_ID, &ticket, &current.assignment_id)
            .unwrap()
            .unwrap(),
        released
    );
}

#[tokio::test]
async fn replaced_worker_cannot_retry_assign_clear_and_new_owner_cannot_reuse_old_receipts() {
    let dir = tempfile::tempdir().unwrap();
    let (api, identity, source, ticket) = tracking_embedded_fixture(dir.path()).await;
    let current = assign_tracking(&api, &identity, &source, &ticket).await;
    let app = build_router(api.clone());
    let clear_path =
        assignment_clear_path(&ticket, &current.assignment_id, "clear-before-replacement");
    tracking_response(
        app.clone(),
        runtime_source_request(
            &identity,
            Some(&source.worker_id),
            "DELETE",
            &clear_path,
            Vec::new(),
        ),
        StatusCode::OK,
    )
    .await;
    let replacement = replace_tracking_orchestrator(&api, &source).await;
    for worker in [&source.worker_id, &replacement.worker_id] {
        let expected = if worker == &source.worker_id {
            StatusCode::UNAUTHORIZED
        } else {
            StatusCode::CONFLICT
        };
        tracking_response(
            app.clone(),
            assignment_request(&api, &identity, worker, &ticket, "assign-tracking"),
            expected,
        )
        .await;
        tracking_response(
            app.clone(),
            runtime_source_request(&identity, Some(worker), "DELETE", &clear_path, Vec::new()),
            expected,
        )
        .await;
    }
    assert!(
        api.store
            .list_current_ticket_assignment_tracking(TEST_WORKSPACE_ID, &ticket)
            .unwrap()
            .is_empty()
    );
    tracking_response(
        app,
        assignment_request(
            &api,
            &identity,
            &replacement.worker_id,
            &ticket,
            "replacement-new-request",
        ),
        StatusCode::OK,
    )
    .await;
    let next = api
        .store
        .list_current_ticket_assignment_tracking(TEST_WORKSPACE_ID, &ticket)
        .unwrap();
    assert_eq!(next.len(), 1);
    assert_ne!(next[0].assignment_id, current.assignment_id);
}

#[tokio::test]
async fn runtime_orchestrator_tags_without_operation_policy_do_not_grant_assignment_authority() {
    let dir = tempfile::tempdir().unwrap();
    let (api, runtime, identity, source, ticket) = tracking_fixture(dir.path()).await;
    let current = assign_tracking(&api, &identity, &source, &ticket).await;
    let unprivileged = open_subjektiv_store(&api)
        .unwrap()
        .create_subject(
            crate::subjektiv::SubjectRole::new("unprivileged-assignment-source").unwrap(),
        )
        .unwrap();
    let forged = create_test_assignment_body(&api, &source.runtime_id, &unprivileged.id).await;
    // Runtime display metadata cannot mint operation policy. Backend ownership
    // remains this real, unprivileged Subject, even if the adapter advertises tags.
    let mut workers = runtime.workers.lock().unwrap();
    let summary = workers
        .iter_mut()
        .find(|worker| worker.worker == forged)
        .unwrap();
    summary.tags = vec!["orchestrator".into(), "singleton".into()];
    summary.singleton_key = Some(crate::hosts::WORKSPACE_ORCHESTRATOR_SINGLETON_KEY.into());
    drop(workers);
    let app = build_router(api.clone());
    for request in [
        assignment_request(&api, &identity, &forged.worker_id, &ticket, "forged-assign"),
        runtime_source_request(
            &identity,
            Some(&forged.worker_id),
            "DELETE",
            &assignment_clear_path(&ticket, &current.assignment_id, "forged-clear"),
            Vec::new(),
        ),
        signed_tracking_update(
            &identity,
            Some(&forged.worker_id),
            &ticket,
            &tracking_update(&current, "forged-tracking"),
        ),
    ] {
        tracking_response(app.clone(), request, StatusCode::FORBIDDEN).await;
    }
    assert_eq!(
        api.store
            .list_current_ticket_assignment_tracking(TEST_WORKSPACE_ID, &ticket)
            .unwrap(),
        vec![current]
    );
}

#[tokio::test]
async fn authenticated_account_assign_and_clear_retries_preserve_source_and_reject_other_accounts()
{
    let dir = tempfile::tempdir().unwrap();
    let (api, _, _, _, ticket) = tracking_fixture(dir.path()).await;
    let owner =
        seed_test_workspace_owner_token(api.store.as_ref(), "assignment-account-owner").await;
    let other = seed_test_api_token(api.store.as_ref(), "assignment-account-other");
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignments/orchestrator");
    let body = serde_json::to_vec(&serde_json::json!({"operation_id":"account-assign", "principal":test_assignment_principal(&api)})).unwrap();
    let request = |token: &str, method: &str, path: &str, body: Vec<u8>| {
        Request::builder()
            .method(method)
            .uri(path)
            .header(CONTENT_TYPE, "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(body))
            .unwrap()
    };
    let app = build_router(api.clone());
    let assigned = tracking_response(
        app.clone(),
        request(&owner, "PUT", &path, body.clone()),
        StatusCode::OK,
    )
    .await;
    let actor = format!(
        "workspace-user:{}",
        serde_json::to_string(&(
            &format!("account-{TEST_WORKSPACE_ID}"),
            "user-assignment-account-owner"
        ))
        .unwrap()
    );
    assert_eq!(assigned["assignment"]["assigned_by"], actor);
    assert_eq!(
        tracking_response(
            app.clone(),
            request(&owner, "PUT", &path, body.clone()),
            StatusCode::OK
        )
        .await,
        assigned
    );
    tracking_response(
        app.clone(),
        request(&other, "PUT", &path, body),
        StatusCode::FORBIDDEN,
    )
    .await;
    let assignment_id = assigned["assignment"]["assignment_id"].as_str().unwrap();
    let clear_path = assignment_clear_path(&ticket, assignment_id, "account-clear");
    let cleared = tracking_response(
        app.clone(),
        request(&owner, "DELETE", &clear_path, Vec::new()),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        tracking_response(
            app.clone(),
            request(&owner, "DELETE", &clear_path, Vec::new()),
            StatusCode::OK
        )
        .await,
        cleared
    );
    tracking_response(
        app,
        request(&other, "DELETE", &clear_path, Vec::new()),
        StatusCode::FORBIDDEN,
    )
    .await;
    let released = api
        .store
        .get_ticket_assignment_tracking(TEST_WORKSPACE_ID, &ticket, assignment_id)
        .unwrap()
        .unwrap();
    assert!(!released.current);
    assert_eq!(released.assigned_by, actor);
    assert_eq!(released.release.unwrap().actor, actor);
}

async fn assign_tracking(
    api: &WorkspaceApi,
    identity: &RuntimeIdentityMaterial,
    source: &RuntimeWorkerRef,
    ticket: &str,
) -> ticket::TicketAssignmentTrackingProjection {
    let value = tracking_response(build_router(api.clone()), runtime_source_request(
        identity, Some(&source.worker_id), "PUT",
        &format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignments/orchestrator"),
        serde_json::to_vec(&serde_json::json!({ "operation_id": "assign-tracking", "principal": test_assignment_principal(&api) })).unwrap(),
    ), StatusCode::OK).await;
    assert_eq!(
        value["assignment"]["assigned_by"],
        format!("worker:{}/{}", source.runtime_id, source.worker_id)
    );
    let current = tracking_response(
        build_router(api.clone()),
        runtime_source_request(
            identity,
            Some(&source.worker_id),
            "GET",
            &tracking_path(ticket),
            Vec::new(),
        ),
        StatusCode::OK,
    )
    .await;
    assert_eq!(current.as_array().unwrap().len(), 1);
    serde_json::from_value(current[0].clone()).unwrap()
}

fn tracking_update(
    current: &ticket::TicketAssignmentTrackingProjection,
    key: &str,
) -> ticket::TicketAssignmentTrackingUpdate {
    ticket::TicketAssignmentTrackingUpdate {
        assignment_id: current.assignment_id.clone(),
        expected_last_event_id: current.last_event_id.clone().unwrap(),
        operation_key: key.into(),
        tracking: ticket::TicketAssignmentTracking {
            status: ticket::TicketAssignmentTrackingStatus::Done,
            waiting_reason: None,
            progress: Some("Investigated".into()),
            next_action: Some("Return to human".into()),
            result: Some("Result recorded without closing the case".into()),
        },
    }
}

fn signed_tracking_update(
    identity: &RuntimeIdentityMaterial,
    worker: Option<&str>,
    ticket: &str,
    update: &ticket::TicketAssignmentTrackingUpdate,
) -> Request<Body> {
    runtime_source_request(
        identity,
        worker,
        "POST",
        &tracking_path(ticket),
        serde_json::to_vec(update).unwrap(),
    )
}

#[tokio::test]
async fn signed_current_orchestrator_without_hints_records_done_without_closing_or_releasing_work()
{
    let dir = tempfile::tempdir().unwrap();
    let (api, _, identity, source, ticket) = tracking_fixture(dir.path()).await;
    let current = assign_tracking(&api, &identity, &source, &ticket).await;
    assert_eq!(
        current.tracking.as_ref().unwrap().status,
        ticket::TicketAssignmentTrackingStatus::Waiting
    );
    let before = browser_ticket_backend(&api)
        .unwrap()
        .show(ticket.clone().into())
        .unwrap();
    let update = tracking_update(&current, "report-done");
    let response = tracking_response(
        build_router(api.clone()),
        signed_tracking_update(&identity, Some(&source.worker_id), &ticket, &update),
        StatusCode::OK,
    )
    .await;
    assert_eq!(response["tracking"]["status"], "done");
    assert_ne!(response["last_event_id"], current.last_event_id.unwrap());
    assert_eq!(
        browser_ticket_backend(&api)
            .unwrap()
            .show(ticket.clone().into())
            .unwrap(),
        before
    );
    assert!(
        api.store
            .get_active_ticket_role_assignment(
                TEST_WORKSPACE_ID,
                &ticket,
                TicketAssignmentRole::Orchestrator
            )
            .unwrap()
            .is_some()
    );
    let detail = api.authority.ticket(&ticket).unwrap();
    assert_eq!(
        detail.assignment_tracking[0]
            .tracking
            .as_ref()
            .unwrap()
            .status,
        ticket::TicketAssignmentTrackingStatus::Done
    );
    let history = tracking_response(
        build_router(api.clone()),
        runtime_source_request(
            &identity,
            Some(&source.worker_id),
            "GET",
            &format!(
                "{}/history?assignment_id={}",
                tracking_path(&ticket),
                update.assignment_id
            ),
            Vec::new(),
        ),
        StatusCode::OK,
    )
    .await;
    let event = history["items"].as_array().unwrap().last().unwrap();
    assert_eq!(event["source_context"]["runtime_id"], source.runtime_id);
    assert_eq!(event["source_context"]["worker_id"], source.worker_id);
    assert_eq!(
        event["source_context"]["authentication_binding_id"],
        api.config_store
            .get_workspace_runtime_binding(TEST_WORKSPACE_ID, &source.runtime_id)
            .unwrap()
            .unwrap()
            .binding_id
    );
}

#[tokio::test]
async fn tracking_runtime_only_and_non_orchestrator_and_forged_source_hints_are_denied() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _, identity, source, ticket) = tracking_fixture(dir.path()).await;
    let current = assign_tracking(&api, &identity, &source, &ticket).await;
    seed_worker_source_member(&api, &source.runtime_id, "other");
    let update = tracking_update(&current, "unauthorized");
    let app = build_router(api.clone());
    for worker in [None, Some("other")] {
        tracking_response(
            app.clone(),
            signed_tracking_update(&identity, worker, &ticket, &update),
            StatusCode::FORBIDDEN,
        )
        .await;
    }
    for (runtime_hint, worker_hint) in [
        (Some(source.runtime_id.as_str()), Some("other")),
        (Some("another-runtime"), Some(source.worker_id.as_str())),
        (None, Some(source.worker_id.as_str())),
    ] {
        let mut request =
            signed_tracking_update(&identity, Some(&source.worker_id), &ticket, &update);
        if let Some(hint) = runtime_hint {
            request
                .headers_mut()
                .insert("x-yoi-runtime-id", hint.parse().unwrap());
        }
        if let Some(hint) = worker_hint {
            request
                .headers_mut()
                .insert("x-yoi-worker-id", hint.parse().unwrap());
        }
        tracking_response(app.clone(), request, StatusCode::FORBIDDEN).await;
    }
    assert_eq!(
        api.store
            .get_ticket_assignment_tracking(TEST_WORKSPACE_ID, &ticket, &current.assignment_id)
            .unwrap()
            .unwrap(),
        current
    );
}

#[tokio::test]
async fn tracking_closed_ticket_current_orchestrator_can_record_result_without_restoring_grants() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _, identity, source, ticket) = tracking_fixture(dir.path()).await;
    let current = assign_tracking(&api, &identity, &source, &ticket).await;
    let backend = browser_ticket_backend(&api).unwrap();
    let before = backend.show(ticket.clone().into()).unwrap();
    let close = case_test_request(
        &before,
        "close-tracking",
        ticket::TicketCaseAction::Close {
            reason: ticket::TicketClosureReason::Completed,
        },
    );
    tracking_response(
        build_router(api.clone()),
        runtime_source_request(
            &identity,
            Some(&source.worker_id),
            "POST",
            &format!("/workspaces/{TEST_WORKSPACE_ID}/tickets/{ticket}/case"),
            serde_json::to_vec(&serde_json::json!({
                "expected_content_digest": close.expected_content_digest,
                "expected_state": close.expected_state,
                "expected_closure_operation_key": close.expected_closure_operation_key,
                "operation_key": close.operation_key,
                "action": close.action,
                "explanation": close.explanation,
                "references": close.references,
            }))
            .unwrap(),
        ),
        StatusCode::OK,
    )
    .await;
    let closed = backend.show(ticket.clone().into()).unwrap();
    assert_eq!(closed.meta.workflow_state, TicketState::Closed);
    assert!(
        api.store
            .get_active_ticket_role_assignment(
                TEST_WORKSPACE_ID,
                &ticket,
                TicketAssignmentRole::Orchestrator
            )
            .unwrap()
            .is_none()
    );
    let update = tracking_update(&current, "post-close-result");
    tracking_response(
        build_router(api.clone()),
        signed_tracking_update(&identity, Some(&source.worker_id), &ticket, &update),
        StatusCode::OK,
    )
    .await;
    assert_eq!(backend.show(ticket.clone().into()).unwrap(), closed);
    assert!(
        api.store
            .get_active_ticket_role_assignment(
                TEST_WORKSPACE_ID,
                &ticket,
                TicketAssignmentRole::Orchestrator
            )
            .unwrap()
            .is_none()
    );
    assert_eq!(
        api.store
            .get_current_ticket_role_assignment(
                TEST_WORKSPACE_ID,
                &ticket,
                TicketAssignmentRole::Orchestrator
            )
            .unwrap()
            .unwrap()
            .assignment_id,
        current.assignment_id
    );
}

async fn tracking_response(
    app: Router,
    request: Request<Body>,
    expected: StatusCode,
) -> serde_json::Value {
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(status, expected, "{}", String::from_utf8_lossy(&body));
    if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or_else(|_| {
            assert_eq!(
                status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "non-JSON API response: {}",
                String::from_utf8_lossy(&body)
            );
            serde_json::Value::String(String::from_utf8(body.to_vec()).unwrap())
        })
    }
}

async fn tracking_fixture(
    root: &Path,
) -> (
    WorkspaceApi,
    WorkdirlessFixtureRuntime,
    RuntimeIdentityMaterial,
    RuntimeWorkerRef,
    String,
) {
    let mut api = test_api(root).await;
    let runtime_id = WorkdirlessFixtureRuntime::RUNTIME_ID;
    register_test_runtime(&api, runtime_id).await;
    let runtime = WorkdirlessFixtureRuntime::default();
    api.runtime.register_or_replace(runtime.clone());
    let identity = RuntimeIdentityMaterial::generate(runtime_id).unwrap();
    configure_runtime_request_auth(&mut api, &identity, runtime_id);
    let subject_id = ensure_test_assignment_subject(&api);
    let source = create_test_assignment_body(&api, runtime_id, &subject_id).await;
    assert_eq!(test_assignment_body(&api).unwrap().worker, source);
    let ticket = browser_ticket_backend(&api)
        .unwrap()
        .create(ticket::NewTicket::new("Tracking authority"))
        .unwrap();
    grant_test_assignment_ticket(&api, &subject_id, &ticket.id);
    (api, runtime, identity, source, ticket.id)
}

#[tokio::test]
async fn tracking_exact_replay_is_source_bound_and_stale_event_cas_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _, identity, source, ticket) = tracking_fixture(dir.path()).await;
    let current = assign_tracking(&api, &identity, &source, &ticket).await;
    let update = tracking_update(&current, "replay-key");
    let app = build_router(api.clone());
    let first_request =
        signed_tracking_update(&identity, Some(&source.worker_id), &ticket, &update);
    let proof =
        first_request.headers()[worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER].clone();
    let result = tracking_response(app.clone(), first_request, StatusCode::OK).await;
    let mut same_proof =
        signed_tracking_update(&identity, Some(&source.worker_id), &ticket, &update);
    same_proof.headers_mut().insert(
        worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER,
        proof,
    );
    tracking_response(app.clone(), same_proof, StatusCode::UNAUTHORIZED).await;
    assert_eq!(
        tracking_response(
            app.clone(),
            signed_tracking_update(&identity, Some(&source.worker_id), &ticket, &update),
            StatusCode::OK
        )
        .await,
        result
    );
    let mut changed = update.clone();
    changed.tracking.result = Some("Changed result".into());
    tracking_response(
        app.clone(),
        signed_tracking_update(&identity, Some(&source.worker_id), &ticket, &changed),
        StatusCode::CONFLICT,
    )
    .await;
    let stale = tracking_update(&current, "stale-last-event");
    tracking_response(
        app.clone(),
        signed_tracking_update(&identity, Some(&source.worker_id), &ticket, &stale),
        StatusCode::CONFLICT,
    )
    .await;
    seed_worker_source_member(&api, &source.runtime_id, "other");
    tracking_response(
        app.clone(),
        signed_tracking_update(&identity, Some("other"), &ticket, &update),
        StatusCode::FORBIDDEN,
    )
    .await;
    let history = api
        .store
        .list_ticket_assignment_tracking_history(TEST_WORKSPACE_ID, &ticket, &current.assignment_id)
        .unwrap();
    assert_eq!(
        history.len(),
        2,
        "Assign plus one committed tracking update only"
    );
    assert_eq!(
        api.store
            .get_ticket_assignment_tracking(TEST_WORKSPACE_ID, &ticket, &current.assignment_id)
            .unwrap()
            .unwrap()
            .tracking
            .unwrap(),
        update.tracking
    );
}

#[tokio::test]
async fn tracking_unassign_and_reassign_keep_results_and_fence_old_assignment() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _, identity, source, ticket) = tracking_fixture(dir.path()).await;
    let current = assign_tracking(&api, &identity, &source, &ticket).await;
    let update = tracking_update(&current, "before-reassign");
    let app = build_router(api.clone());
    let result = tracking_response(
        app.clone(),
        signed_tracking_update(&identity, Some(&source.worker_id), &ticket, &update),
        StatusCode::OK,
    )
    .await;
    tracking_response(app.clone(), runtime_source_request(&identity, Some(&source.worker_id), "DELETE", &format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignments/orchestrator?assignment_id={}&operation_id=release-tracking", current.assignment_id), Vec::new()), StatusCode::OK).await;
    let released = api
        .store
        .get_ticket_assignment_tracking(TEST_WORKSPACE_ID, &ticket, &current.assignment_id)
        .unwrap()
        .unwrap();
    assert!(!released.current);
    assert_eq!(released.tracking.unwrap(), update.tracking);
    let mut stale = update.clone();
    stale.operation_key = "released-update".into();
    stale.expected_last_event_id = result["last_event_id"].as_str().unwrap().into();
    tracking_response(
        app.clone(),
        signed_tracking_update(&identity, Some(&source.worker_id), &ticket, &stale),
        StatusCode::CONFLICT,
    )
    .await;
    assert_eq!(
        tracking_response(
            app.clone(),
            signed_tracking_update(&identity, Some(&source.worker_id), &ticket, &update),
            StatusCode::OK
        )
        .await,
        result
    );
    tracking_response(app.clone(), runtime_source_request(&identity, Some(&source.worker_id), "PUT", &format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignments/orchestrator"), serde_json::to_vec(&serde_json::json!({"operation_id":"assign-again", "principal":test_assignment_principal(&api)})).unwrap()), StatusCode::OK).await;
    let next = api
        .store
        .list_current_ticket_assignment_tracking(TEST_WORKSPACE_ID, &ticket)
        .unwrap();
    assert_ne!(next[0].assignment_id, current.assignment_id);
    assert_eq!(
        next[0].tracking.as_ref().unwrap().status,
        ticket::TicketAssignmentTrackingStatus::Waiting
    );
    tracking_response(
        app,
        signed_tracking_update(&identity, Some(&source.worker_id), &ticket, &stale),
        StatusCode::CONFLICT,
    )
    .await;
    assert_eq!(
        api.store
            .list_ticket_assignment_tracking_history(
                TEST_WORKSPACE_ID,
                &ticket,
                &current.assignment_id
            )
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn tracking_other_workspace_routes_and_unsigned_reads_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _, identity, source, ticket) = tracking_fixture(dir.path()).await;
    let current = assign_tracking(&api, &identity, &source, &ticket).await;
    let update = tracking_update(&current, "foreign-workspace");
    let app = build_router(api.clone());
    tracking_response(
        app.clone(),
        Request::builder()
            .uri(tracking_path(&ticket))
            .body(Body::empty())
            .unwrap(),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    let path = format!("/workspaces/another-workspace/tickets/{ticket}/assignment-tracking");
    tracking_response(
        app.clone(),
        runtime_source_request(
            &identity,
            Some(&source.worker_id),
            "POST",
            &path,
            serde_json::to_vec(&update).unwrap(),
        ),
        StatusCode::NOT_FOUND,
    )
    .await;
    for path in [
        path.clone(),
        format!("{path}/history?assignment_id={}", current.assignment_id),
    ] {
        tracking_response(
            app.clone(),
            runtime_source_request(&identity, Some(&source.worker_id), "GET", &path, Vec::new()),
            StatusCode::NOT_FOUND,
        )
        .await;
    }
    assert_eq!(
        api.store
            .get_ticket_assignment_tracking(TEST_WORKSPACE_ID, &ticket, &current.assignment_id)
            .unwrap()
            .unwrap(),
        current
    );
}

#[tokio::test]
async fn replaced_orchestrator_cannot_update_or_replay_and_current_replacement_inherits_tracking() {
    let dir = tempfile::tempdir().unwrap();
    let (api, identity, source, ticket) = tracking_embedded_fixture(dir.path()).await;
    let current = assign_tracking(&api, &identity, &source, &ticket).await;
    let update = tracking_update(&current, "old-orchestrator-result");
    let app = build_router(api.clone());
    let result = tracking_response(
        app.clone(),
        signed_tracking_update(&identity, Some(&source.worker_id), &ticket, &update),
        StatusCode::OK,
    )
    .await;
    let replacement = replace_tracking_orchestrator(&api, &source).await;
    tracking_response(
        app.clone(),
        signed_tracking_update(&identity, Some(&source.worker_id), &ticket, &update),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    let mut new_update = update.clone();
    new_update.operation_key = "new-orchestrator-result".into();
    new_update.expected_last_event_id = result["last_event_id"].as_str().unwrap().into();
    tracking_response(
        app.clone(),
        signed_tracking_update(&identity, Some(&source.worker_id), &ticket, &new_update),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    // A different authenticated current actor cannot reuse the old actor's receipt.
    tracking_response(
        app.clone(),
        signed_tracking_update(&identity, Some(&replacement.worker_id), &ticket, &update),
        StatusCode::CONFLICT,
    )
    .await;
    let inherited = tracking_response(
        app,
        signed_tracking_update(
            &identity,
            Some(&replacement.worker_id),
            &ticket,
            &new_update,
        ),
        StatusCode::OK,
    )
    .await;
    assert_eq!(inherited["assignment_id"], current.assignment_id);
    assert_eq!(
        api.store
            .list_current_ticket_assignment_tracking(TEST_WORKSPACE_ID, &ticket)
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn tracking_changed_runtime_binding_cannot_replay_original_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _, identity, source, ticket) = tracking_fixture(dir.path()).await;
    let current = assign_tracking(&api, &identity, &source, &ticket).await;
    let update = tracking_update(&current, "binding-replay");
    let app = build_router(api.clone());
    tracking_response(
        app.clone(),
        signed_tracking_update(&identity, Some(&source.worker_id), &ticket, &update),
        StatusCode::OK,
    )
    .await;
    let conn = rusqlite::Connection::open(&api.config.database_path).unwrap();
    conn.execute("UPDATE workspace_runtime_bindings SET binding_id='replacement-binding' WHERE workspace_id=?1 AND runtime_id=?2", rusqlite::params![TEST_WORKSPACE_ID, source.runtime_id]).unwrap();
    tracking_response(
        app,
        signed_tracking_update(&identity, Some(&source.worker_id), &ticket, &update),
        StatusCode::CONFLICT,
    )
    .await;
    assert_eq!(
        api.store
            .list_ticket_assignment_tracking_history(
                TEST_WORKSPACE_ID,
                &ticket,
                &current.assignment_id
            )
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn assignment_retry_is_idempotent_but_another_authenticated_actor_cannot_replay_it() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _, identity, source, ticket) = tracking_fixture(dir.path()).await;
    let current = assign_tracking(&api, &identity, &source, &ticket).await;
    let replay = assign_tracking(&api, &identity, &source, &ticket).await;
    assert_eq!(replay, current);
    let token = seed_test_workspace_owner_token(api.store.as_ref(), "assignment-replay-user").await;
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignments/orchestrator");
    let body = serde_json::to_vec(&serde_json::json!({"operation_id":"assign-tracking", "principal":test_assignment_principal(&api)})).unwrap();
    tracking_response(
        build_router(api.clone()),
        Request::builder()
            .method("PUT")
            .uri(&path)
            .header(CONTENT_TYPE, "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(body))
            .unwrap(),
        StatusCode::CONFLICT,
    )
    .await;
    assert_eq!(
        api.store
            .list_current_ticket_assignment_tracking(TEST_WORKSPACE_ID, &ticket)
            .unwrap(),
        vec![current]
    );
}

#[tokio::test]
async fn tracking_history_is_assignment_scoped_and_paged_and_rejects_unknown_cursors() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _, identity, source, ticket) = tracking_fixture(dir.path()).await;
    let current = assign_tracking(&api, &identity, &source, &ticket).await;
    let update = tracking_update(&current, "history-done");
    let app = build_router(api.clone());
    tracking_response(
        app.clone(),
        signed_tracking_update(&identity, Some(&source.worker_id), &ticket, &update),
        StatusCode::OK,
    )
    .await;
    let path = format!(
        "{}/history?assignment_id={}&limit=1",
        tracking_path(&ticket),
        current.assignment_id
    );
    let first = tracking_response(
        app.clone(),
        runtime_source_request(&identity, Some(&source.worker_id), "GET", &path, Vec::new()),
        StatusCode::OK,
    )
    .await;
    assert_eq!(first["items"].as_array().unwrap().len(), 1);
    assert_eq!(first["items"][0]["tracking"]["status"], "waiting");
    let second = tracking_response(
        app.clone(),
        runtime_source_request(
            &identity,
            Some(&source.worker_id),
            "GET",
            &format!(
                "{path}&after_event_id={}",
                first["next_event_id"].as_str().unwrap()
            ),
            Vec::new(),
        ),
        StatusCode::OK,
    )
    .await;
    assert_eq!(second["items"].as_array().unwrap().len(), 1);
    assert_eq!(second["items"][0]["tracking"]["status"], "done");
    assert!(second["next_event_id"].is_null());
    for suffix in [
        "&after_event_id=another-assignment-event",
        "&limit=0",
        "&limit=101",
    ] {
        let path = if suffix.starts_with("&limit=") {
            format!(
                "{}/history?assignment_id={}{}",
                tracking_path(&ticket),
                current.assignment_id,
                suffix
            )
        } else {
            format!("{path}{suffix}")
        };
        tracking_response(
            app.clone(),
            runtime_source_request(&identity, Some(&source.worker_id), "GET", &path, Vec::new()),
            StatusCode::BAD_REQUEST,
        )
        .await;
    }
}

#[tokio::test]
async fn tracking_model_authorship_and_role_fields_are_rejected_and_human_is_not_self_reporter() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _, identity, source, ticket) = tracking_fixture(dir.path()).await;
    let current = assign_tracking(&api, &identity, &source, &ticket).await;
    let update = tracking_update(&current, "claimed-role");
    let app = build_router(api.clone());
    for (key, value) in [
        ("author", serde_json::json!("orchestrator")),
        ("role", serde_json::json!("orchestrator")),
        (
            "source_context",
            serde_json::json!({"worker_id":source.worker_id,"runtime_id":source.runtime_id}),
        ),
    ] {
        let mut input = serde_json::to_value(&update).unwrap();
        input[key] = value;
        tracking_response(
            app.clone(),
            runtime_source_request(
                &identity,
                Some(&source.worker_id),
                "POST",
                &tracking_path(&ticket),
                serde_json::to_vec(&input).unwrap(),
            ),
            StatusCode::UNPROCESSABLE_ENTITY,
        )
        .await;
    }
    let token =
        seed_test_workspace_owner_token(api.store.as_ref(), "human-not-tracking-agent").await;
    tracking_response(
        app,
        Request::builder()
            .method("POST")
            .uri(tracking_path(&ticket))
            .header(CONTENT_TYPE, "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(serde_json::to_vec(&update).unwrap()))
            .unwrap(),
        StatusCode::FORBIDDEN,
    )
    .await;
    assert_eq!(
        api.store
            .get_ticket_assignment_tracking(TEST_WORKSPACE_ID, &ticket, &current.assignment_id)
            .unwrap()
            .unwrap(),
        current
    );
}

#[tokio::test]
async fn assignment_receipt_cannot_be_replayed_under_a_new_runtime_binding() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _, identity, source, ticket) = tracking_fixture(dir.path()).await;
    let current = assign_tracking(&api, &identity, &source, &ticket).await;
    let conn = rusqlite::Connection::open(&api.config.database_path).unwrap();
    conn.execute("UPDATE workspace_runtime_bindings SET binding_id='new-assignment-binding' WHERE workspace_id=?1 AND runtime_id=?2", rusqlite::params![TEST_WORKSPACE_ID, source.runtime_id]).unwrap();
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignments/orchestrator");
    let body = serde_json::to_vec(&serde_json::json!({"operation_id":"assign-tracking", "principal":test_assignment_principal(&api)})).unwrap();
    tracking_response(
        build_router(api.clone()),
        runtime_source_request(&identity, Some(&source.worker_id), "PUT", &path, body),
        StatusCode::CONFLICT,
    )
    .await;
    assert_eq!(
        api.store
            .list_current_ticket_assignment_tracking(TEST_WORKSPACE_ID, &ticket)
            .unwrap(),
        vec![current]
    );
}

#[tokio::test]
async fn assignment_clear_receipt_binds_verified_actor_and_runtime_binding() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _, identity, source, ticket) = tracking_fixture(dir.path()).await;
    let current = assign_tracking(&api, &identity, &source, &ticket).await;
    let path = format!(
        "/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignments/orchestrator?assignment_id={}&operation_id=source-bound-clear",
        current.assignment_id
    );
    let app = build_router(api.clone());
    let result = tracking_response(
        app.clone(),
        runtime_source_request(
            &identity,
            Some(&source.worker_id),
            "DELETE",
            &path,
            Vec::new(),
        ),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        tracking_response(
            app.clone(),
            runtime_source_request(
                &identity,
                Some(&source.worker_id),
                "DELETE",
                &path,
                Vec::new()
            ),
            StatusCode::OK
        )
        .await,
        result
    );
    let token = seed_test_workspace_owner_token(api.store.as_ref(), "another-clear-actor").await;
    tracking_response(
        app.clone(),
        Request::builder()
            .method("DELETE")
            .uri(&path)
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap(),
        StatusCode::CONFLICT,
    )
    .await;
    let conn = rusqlite::Connection::open(&api.config.database_path).unwrap();
    conn.execute("UPDATE workspace_runtime_bindings SET binding_id='new-clear-binding' WHERE workspace_id=?1 AND runtime_id=?2", rusqlite::params![TEST_WORKSPACE_ID, source.runtime_id]).unwrap();
    tracking_response(
        app,
        runtime_source_request(
            &identity,
            Some(&source.worker_id),
            "DELETE",
            &path,
            Vec::new(),
        ),
        StatusCode::CONFLICT,
    )
    .await;
    assert!(
        api.store
            .list_current_ticket_assignment_tracking(TEST_WORKSPACE_ID, &ticket)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn released_assignment_query_retains_principal_assign_audit_and_last_tracking_without_becoming_current()
 {
    let dir = tempfile::tempdir().unwrap();
    let (api, _, identity, source, ticket) = tracking_fixture(dir.path()).await;
    let current = assign_tracking(&api, &identity, &source, &ticket).await;
    let app = build_router(api.clone());
    tracking_response(app.clone(), runtime_source_request(&identity, Some(&source.worker_id), "DELETE", &format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignments/orchestrator?assignment_id={}&operation_id=release-snapshot", current.assignment_id), Vec::new()), StatusCode::OK).await;
    let values = tracking_response(
        app.clone(),
        runtime_source_request(
            &identity,
            Some(&source.worker_id),
            "GET",
            &format!(
                "{}?assignment_id={}",
                tracking_path(&ticket),
                current.assignment_id
            ),
            Vec::new(),
        ),
        StatusCode::OK,
    )
    .await;
    assert_eq!(values.as_array().unwrap().len(), 1);
    let released = &values[0];
    assert_eq!(released["assignment_id"], current.assignment_id);
    assert_eq!(released["role"], "orchestrator");
    assert_eq!(
        released["principal"],
        serde_json::json!(test_assignment_principal(&api))
    );
    assert_eq!(released["assigned_by"], current.assigned_by);
    assert_eq!(released["assigned_at"], current.assigned_at);
    assert_eq!(released["current"], false);
    assert_eq!(released["tracking"]["status"], "waiting");
    assert_eq!(released["release"]["action"], "unassigned");
    assert_eq!(released["release"]["operation_key"], "release-snapshot");
    assert_eq!(released["release"]["actor"], current.assigned_by);
    assert_eq!(released["release"]["last_tracking"]["status"], "waiting");
    assert!(released["release"]["replacement_assignment_id"].is_null());
    assert_eq!(
        tracking_response(
            app.clone(),
            runtime_source_request(
                &identity,
                Some(&source.worker_id),
                "GET",
                &tracking_path(&ticket),
                Vec::new()
            ),
            StatusCode::OK
        )
        .await,
        serde_json::json!([])
    );
    let other_ticket = browser_ticket_backend(&api)
        .unwrap()
        .create(ticket::NewTicket::new("Different Ticket"))
        .unwrap();
    grant_test_assignment_ticket(
        &api,
        &ensure_test_assignment_subject(&api),
        &other_ticket.id,
    );
    assert_eq!(
        tracking_response(
            app,
            runtime_source_request(
                &identity,
                Some(&source.worker_id),
                "GET",
                &format!(
                    "{}?assignment_id={}",
                    tracking_path(&other_ticket.id),
                    current.assignment_id
                ),
                Vec::new()
            ),
            StatusCode::OK
        )
        .await,
        serde_json::json!([])
    );
}

#[tokio::test]
async fn same_principal_reassignment_retains_old_result_and_projects_replacement_identity() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _, identity, source, ticket) = tracking_fixture(dir.path()).await;
    let current = assign_tracking(&api, &identity, &source, &ticket).await;
    let update = tracking_update(&current, "done-before-new-request");
    let app = build_router(api.clone());
    tracking_response(
        app.clone(),
        signed_tracking_update(&identity, Some(&source.worker_id), &ticket, &update),
        StatusCode::OK,
    )
    .await;
    let assignment = tracking_response(app.clone(), runtime_source_request(&identity, Some(&source.worker_id), "PUT", &format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignments/orchestrator"), serde_json::to_vec(&serde_json::json!({"operation_id":"request-again", "expected_assignment_id":current.assignment_id,"principal":test_assignment_principal(&api)})).unwrap()), StatusCode::OK).await;
    let next_id = assignment["assignment"]["assignment_id"].as_str().unwrap();
    assert_ne!(next_id, current.assignment_id);
    let old = tracking_response(
        app.clone(),
        runtime_source_request(
            &identity,
            Some(&source.worker_id),
            "GET",
            &format!(
                "{}?assignment_id={}",
                tracking_path(&ticket),
                current.assignment_id
            ),
            Vec::new(),
        ),
        StatusCode::OK,
    )
    .await;
    assert_eq!(old[0]["current"], false);
    assert_eq!(old[0]["tracking"]["status"], "done");
    assert_eq!(
        old[0]["tracking"]["result"],
        update.tracking.result.unwrap()
    );
    assert_eq!(old[0]["release"]["action"], "replaced");
    assert_eq!(old[0]["release"]["replacement_assignment_id"], next_id);
    assert_eq!(old[0]["release"]["operation_key"], "request-again");
    let next = tracking_response(
        app,
        runtime_source_request(
            &identity,
            Some(&source.worker_id),
            "GET",
            &tracking_path(&ticket),
            Vec::new(),
        ),
        StatusCode::OK,
    )
    .await;
    assert_eq!(next.as_array().unwrap().len(), 1);
    assert_eq!(next[0]["assignment_id"], next_id);
    assert_eq!(next[0]["tracking"]["status"], "waiting");
    assert!(next[0]["tracking"]["result"].is_null());
    assert!(next[0]["release"].is_null());
}

#[tokio::test]
async fn assignment_signed_runtime_without_worker_cannot_self_assign_service_role() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _, identity, _, ticket) = tracking_fixture(dir.path()).await;
    let input = serde_json::json!({
        "operation_id": "runtime-only-assign",
        "principal": test_assignment_principal(&api)
    });
    tracking_response(
        build_router(api.clone()),
        runtime_source_request(
            &identity,
            None,
            "PUT",
            &format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignments/orchestrator"),
            serde_json::to_vec(&input).unwrap(),
        ),
        StatusCode::FORBIDDEN,
    )
    .await;
    assert!(
        api.store
            .list_current_ticket_role_assignments(TEST_WORKSPACE_ID, &ticket)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn public_assignment_answer_authenticates_source_replays_saved_result_and_rejects_read_reassign_race()
 {
    let directory = tempfile::tempdir().unwrap();
    let (api, _, identity, source, ticket) = tracking_fixture(directory.path()).await;
    let current = assign_tracking(&api, &identity, &source, &ticket).await;
    let token = seed_test_workspace_owner_token(api.store.as_ref(), "answer-owner").await;
    let app = build_router(api.clone());
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignment-answer");
    let body = serde_json::json!({"assignment_id":current.assignment_id,"expected_last_event_id":current.last_event_id,"operation_key":"answer-op","body":"Choose the bounded option"});
    let account_request = |body: &serde_json::Value| {
        Request::builder()
            .method("POST")
            .uri(&path)
            .header(CONTENT_TYPE, "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(serde_json::to_vec(body).unwrap()))
            .unwrap()
    };
    tracking_response(
        app.clone(),
        Request::builder()
            .method("POST")
            .uri(&path)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap(),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    let saved = tracking_response(app.clone(), account_request(&body), StatusCode::OK).await;
    assert_eq!(saved["assignment_id"], current.assignment_id);
    let backend = browser_ticket_backend(&api).unwrap();
    let authored = backend.show(ticket.clone().into()).unwrap();
    assert_eq!(
        authored
            .events
            .iter()
            .filter(|e| e.kind.as_str() == "assignment_answer")
            .count(),
        1
    );
    let event = authored
        .events
        .iter()
        .find(|e| e.kind.as_str() == "assignment_answer")
        .unwrap();
    assert!(
        event
            .author
            .as_deref()
            .unwrap()
            .starts_with("workspace-user:")
    );
    assert_eq!(event.body.as_str(), "Choose the bounded option");
    assert_eq!(
        api.store
            .get_ticket_assignment_tracking(TEST_WORKSPACE_ID, &ticket, &current.assignment_id)
            .unwrap()
            .unwrap(),
        current
    );
    assert_eq!(
        tracking_response(app.clone(), account_request(&body), StatusCode::OK).await,
        saved
    );
    let mut changed = body.clone();
    changed["body"] = "Changed payload".into();
    tracking_response(app.clone(), account_request(&changed), StatusCode::CONFLICT).await;
    let mut spoof = body.clone();
    spoof["actor"] = "owner".into();
    tracking_response(
        app.clone(),
        account_request(&spoof),
        StatusCode::UNPROCESSABLE_ENTITY,
    )
    .await;
    let assignment_path =
        format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignments/orchestrator");
    tracking_response(app.clone(),runtime_source_request(&identity,Some(&source.worker_id),"PUT",&assignment_path,serde_json::to_vec(&serde_json::json!({"operation_id":"replace-after-read","expected_assignment_id":current.assignment_id,"principal":test_assignment_principal(&api)})).unwrap()),StatusCode::OK).await;
    let mut stale = body.clone();
    stale["operation_key"] = "fresh-stale-answer".into();
    tracking_response(app.clone(), account_request(&stale), StatusCode::CONFLICT).await;
    assert_eq!(
        tracking_response(app, account_request(&body), StatusCode::OK).await,
        saved
    );
    assert_eq!(
        backend
            .show(ticket.into())
            .unwrap()
            .events
            .iter()
            .filter(|e| e.kind.as_str() == "assignment_answer")
            .count(),
        1
    );
}

#[tokio::test]
async fn public_assignment_pages_and_diagnostics_require_scoped_read_and_reject_foreign_cursors() {
    let directory = tempfile::tempdir().unwrap();
    let (api, _, identity, source, ticket) = tracking_fixture(directory.path()).await;
    let current = assign_tracking(&api, &identity, &source, &ticket).await;
    let token = seed_test_api_token(api.store.as_ref(), "diagnostics-reader");
    let app = build_router(api.clone());
    for prefix in [
        format!("/api/w/{TEST_WORKSPACE_ID}"),
        format!("/workspaces/{TEST_WORKSPACE_ID}"),
    ] {
        let path = format!("{prefix}/tickets/{ticket}/assignment-tracking/assignments?limit=1");
        tracking_response(
            app.clone(),
            Request::builder().uri(&path).body(Body::empty()).unwrap(),
            StatusCode::UNAUTHORIZED,
        )
        .await;
        let request = |path: String| {
            Request::builder()
                .uri(path)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap()
        };
        let page = tracking_response(app.clone(), request(path), StatusCode::OK).await;
        assert_eq!(page["items"][0]["assignment_id"], current.assignment_id);
        assert!(page["next_assignment_id"].is_null());
        let path = format!(
            "{prefix}/tickets/{ticket}/assignment-diagnostics?assignment_id={}&limit=1",
            current.assignment_id
        );
        let diagnostic =
            tracking_response(app.clone(), request(path.clone()), StatusCode::OK).await;
        assert_eq!(diagnostic["assignment_id"], current.assignment_id);
        assert_eq!(diagnostic["input_origin_known"], true);
        assert!(diagnostic.get("payload_json").is_none());
        tracking_response(
            app.clone(),
            request(format!("{path}&after_input_event_id=ticket-event:999")),
            StatusCode::BAD_REQUEST,
        )
        .await;
        tracking_response(app.clone(),request(format!("{prefix}/tickets/{ticket}/assignment-tracking/assignments?after_assignment_id=foreign")),StatusCode::BAD_REQUEST).await;
        tracking_response(
            app.clone(),
            request(format!(
                "{prefix}/tickets/{ticket}/assignment-diagnostics?assignment_id={}&limit=101",
                current.assignment_id
            )),
            StatusCode::BAD_REQUEST,
        )
        .await;
    }
    let path = format!(
        "/api/w/another-workspace/tickets/{ticket}/assignment-diagnostics?assignment_id={}",
        current.assignment_id
    );
    let response = app
        .oneshot(
            Request::builder()
                .uri(path)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(!response.status().is_success());
}

#[tokio::test]
async fn ticket_query_principal_and_handling_filter_before_single_item_page_without_worker_inference()
 {
    let directory = tempfile::tempdir().unwrap();
    let (api, _, identity, source, ticket) = tracking_fixture(directory.path()).await;
    let current = assign_tracking(&api, &identity, &source, &ticket).await;
    tracking_response(
        build_router(api.clone()),
        signed_tracking_update(
            &identity,
            Some(&source.worker_id),
            &ticket,
            &tracking_update(&current, "query-done"),
        ),
        StatusCode::OK,
    )
    .await;
    let backend = browser_ticket_backend(&api).unwrap();
    for n in 0..8 {
        backend
            .create(ticket::NewTicket::new(format!("Unassigned newer {n}")))
            .unwrap();
    }
    let query:server_api::TicketQueryRequest=serde_json::from_value(serde_json::json!({"handling_status":"done","assigned_principal":test_assignment_principal(&api),"limit":1})).unwrap();
    let found = api.authority.query_tickets(query).unwrap();
    assert_eq!(found.items.len(), 1);
    assert_eq!(found.items[0].id, ticket);
    let query:server_api::TicketQueryRequest=serde_json::from_value(serde_json::json!({"handling_status":"waiting","assigned_principal":test_assignment_principal(&api),"limit":1})).unwrap();
    assert!(api.authority.query_tickets(query).unwrap().items.is_empty());
    let query:server_api::TicketQueryRequest=serde_json::from_value(serde_json::json!({"assigned_principal":{"kind":"worker","runtime_id":source.runtime_id,"worker_id":source.worker_id},"limit":1})).unwrap();
    assert!(api.authority.query_tickets(query).unwrap().items.is_empty());
}

#[test]
fn canonical_assignment_operations_are_explicit_scoped_api_routes_not_static_shell_paths() {
    for suffix in [
        "case",
        "assignment-tracking",
        "assignment-tracking/history",
        "assignment-tracking/assignments",
        "assignment-diagnostics",
        "assignment-answer",
    ] {
        let path = format!("/workspaces/workspace-a/tickets/ticket-a/{suffix}");
        assert_eq!(
            canonical_ticket_operation_workspace_id(&path),
            Some("workspace-a")
        );
        assert_eq!(scoped_workspace_id(&path), Some("workspace-a"));
        assert!(!is_server_static_forward(&path));
        assert!(!is_server_global_forward(&path));
        assert!(canonical_ticket_operation_workspace_id(&format!("{path}/extra")).is_none());
    }
    for invalid in [
        "/workspaces//tickets/t/assignment-answer",
        "/workspaces/w/tickets//assignment-diagnostics",
        "/workspaces/w/tickets/t",
        "/workspaces/w/tickets/t/unknown",
        "/workspaces/w/tickets/t/assignment-tracking/unknown",
        "/workspaces/w/workers/t/assignment-answer",
    ] {
        assert!(
            canonical_ticket_operation_workspace_id(invalid).is_none(),
            "{invalid}"
        );
    }
}

#[tokio::test]
async fn catalog_assignment_routes_apply_cookie_workspace_auth_and_origin_to_browser_and_canonical_paths()
 {
    let directory = tempfile::tempdir().unwrap();
    let (api, _, identity, source, ticket) = tracking_fixture(directory.path()).await;
    let current = assign_tracking(&api, &identity, &source, &ticket).await;
    seed_test_workspace_owner_token(api.store.as_ref(), "assignment-cookie").await;
    api.store
        .create_browser_session(&BrowserSessionRecord {
            token_hash: crate::auth::token_hash("assignment-browser-session"),
            session_id: "assignment-session".into(),
            user_id: "user-assignment-cookie".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            expires_at: "2099-01-01T00:00:00Z".into(),
            revoked_at: None,
        })
        .unwrap();
    let AuthConfig::Passkey {
        origin,
        cookie_name,
        ..
    } = &api.config.auth;
    let cookie = format!("{cookie_name}=assignment-browser-session");
    let catalog = WorkspaceServerApi::new(api.config.clone(), api.store.clone());
    catalog
        .apis
        .lock()
        .await
        .insert(TEST_WORKSPACE_ID.into(), api.clone());
    catalog
        .routers
        .lock()
        .await
        .insert(TEST_WORKSPACE_ID.into(), build_inner_router(api.clone()));
    let app = workspace_server_router(catalog);
    let body = serde_json::json!({"assignment_id":current.assignment_id,"expected_last_event_id":current.last_event_id,"operation_key":"cookie-answer","body":"Authorized browser continuation"});
    let mut receipt = None;
    for prefix in [
        format!("/api/w/{TEST_WORKSPACE_ID}"),
        format!("/workspaces/{TEST_WORKSPACE_ID}"),
    ] {
        let identity_path =
            format!("{prefix}/tickets/{ticket}/assignment-tracking/assignments?limit=1");
        let diagnostic_path = format!(
            "{prefix}/tickets/{ticket}/assignment-diagnostics?assignment_id={}",
            current.assignment_id
        );
        for path in [identity_path, diagnostic_path] {
            tracking_response(
                app.clone(),
                Request::builder().uri(&path).body(Body::empty()).unwrap(),
                StatusCode::UNAUTHORIZED,
            )
            .await;
            tracking_response(
                app.clone(),
                Request::builder()
                    .uri(path)
                    .header(axum::http::header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
                StatusCode::OK,
            )
            .await;
        }
        let answer_path = format!("{prefix}/tickets/{ticket}/assignment-answer");
        let request = |with_origin: bool| {
            let mut request = Request::builder()
                .method("POST")
                .uri(&answer_path)
                .header(CONTENT_TYPE, "application/json")
                .header(axum::http::header::COOKIE, &cookie);
            if with_origin {
                request = request.header(ORIGIN, origin);
            }
            request
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap()
        };
        tracking_response(app.clone(), request(false), StatusCode::FORBIDDEN).await;
        let saved = tracking_response(app.clone(), request(true), StatusCode::OK).await;
        if let Some(previous) = &receipt {
            assert_eq!(&saved, previous);
        } else {
            receipt = Some(saved);
        }
        let cross = answer_path.replace(TEST_WORKSPACE_ID, "foreign-workspace");
        tracking_response(
            app.clone(),
            Request::builder()
                .method("POST")
                .uri(cross)
                .header(CONTENT_TYPE, "application/json")
                .header(axum::http::header::COOKIE, &cookie)
                .header(ORIGIN, origin)
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
            StatusCode::FORBIDDEN,
        )
        .await;
    }
    let events = browser_ticket_backend(&api)
        .unwrap()
        .show(ticket.into())
        .unwrap()
        .events;
    let answers = events
        .iter()
        .filter(|event| event.kind.as_str() == "assignment_answer")
        .collect::<Vec<_>>();
    assert_eq!(answers.len(), 1);
    assert!(
        answers[0]
            .author
            .as_deref()
            .unwrap()
            .contains(&format!("account-{TEST_WORKSPACE_ID}"))
    );
}

#[tokio::test]
async fn canonical_answer_uses_actual_runtime_binding_and_rejects_tampered_source_or_binding_replay()
 {
    let directory = tempfile::tempdir().unwrap();
    let (api, _, identity, source, ticket) = tracking_fixture(directory.path()).await;
    let current = assign_tracking(&api, &identity, &source, &ticket).await;
    let path = format!("/workspaces/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignment-answer");
    let input = serde_json::json!({"assignment_id":current.assignment_id,"expected_last_event_id":current.last_event_id,"operation_key":"runtime-answer","body":"Runtime-authenticated continuation"});
    let request = || {
        runtime_source_request(
            &identity,
            Some(&source.worker_id),
            "POST",
            &path,
            serde_json::to_vec(&input).unwrap(),
        )
    };
    let app = build_router(api.clone());
    let mut unsigned = request();
    unsigned
        .headers_mut()
        .remove(worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER);
    tracking_response(app.clone(), unsigned, StatusCode::UNAUTHORIZED).await;
    let mut tampered = request();
    *tampered.body_mut() = Body::from("{}");
    tracking_response(app.clone(), tampered, StatusCode::UNAUTHORIZED).await;
    let attacker = RuntimeIdentityMaterial::generate(&source.runtime_id).unwrap();
    tracking_response(
        app.clone(),
        runtime_source_request(
            &attacker,
            Some(&source.worker_id),
            "POST",
            &path,
            serde_json::to_vec(&input).unwrap(),
        ),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    tracking_response(
        app.clone(),
        runtime_source_request(
            &identity,
            None,
            "POST",
            &path,
            serde_json::to_vec(&input).unwrap(),
        ),
        StatusCode::FORBIDDEN,
    )
    .await;
    let saved = tracking_response(app.clone(), request(), StatusCode::OK).await;
    assert_eq!(
        tracking_response(app.clone(), request(), StatusCode::OK).await,
        saved
    );
    let conn = rusqlite::Connection::open(&api.config.database_path).unwrap();
    let binding:String=conn.query_row("SELECT binding_id FROM workspace_runtime_bindings WHERE workspace_id=?1 AND runtime_id=?2",rusqlite::params![TEST_WORKSPACE_ID,source.runtime_id],|r| r.get(0)).unwrap();
    let receipt_source:String=conn.query_row("SELECT source_context_json FROM typed_ticket_assignment_tracking_operations WHERE workspace_id=?1 AND operation_key='runtime-answer'",[TEST_WORKSPACE_ID],|r| r.get(0)).unwrap();
    let receipt_source: serde_json::Value = serde_json::from_str(&receipt_source).unwrap();
    assert_eq!(
        receipt_source["source"]["authentication_binding_id"],
        binding
    );
    assert_eq!(receipt_source["source"]["worker_id"], source.worker_id);
    conn.execute("UPDATE workspace_runtime_bindings SET binding_id='new-answer-binding' WHERE workspace_id=?1 AND runtime_id=?2",rusqlite::params![TEST_WORKSPACE_ID,source.runtime_id]).unwrap();
    tracking_response(app.clone(), request(), StatusCode::CONFLICT).await;
    let foreign = path.replace(TEST_WORKSPACE_ID, "foreign-workspace");
    let response = app
        .oneshot(runtime_source_request(
            &identity,
            Some(&source.worker_id),
            "POST",
            &foreign,
            serde_json::to_vec(&input).unwrap(),
        ))
        .await
        .unwrap();
    assert!(!response.status().is_success());
    assert_eq!(
        browser_ticket_backend(&api)
            .unwrap()
            .show(ticket.into())
            .unwrap()
            .events
            .iter()
            .filter(|event| event.kind.as_str() == "assignment_answer")
            .count(),
        1
    );
}

#[tokio::test]
async fn closure_reason_filter_matches_current_judgment_before_paging_and_text_search_retains_old_reasons()
 {
    let directory = tempfile::tempdir().unwrap();
    let api = test_api(directory.path()).await;
    let backend = browser_ticket_backend(&api).unwrap();
    let reference = backend
        .create(ticket::NewTicket::new("Reason classification"))
        .unwrap();
    let mutate = |key: &str, action: ticket::TicketCaseAction| {
        let observed = backend.show(reference.id.clone().into()).unwrap();
        backend
            .update_case(
                &reference.id,
                ticket::TicketCaseUpdate {
                    expected_content_digest: ticket::ticket_content_digest(&observed),
                    expected_state: observed.meta.workflow_state,
                    expected_closure_operation_key: observed.closure_operation_key,
                    operation_key: key.into(),
                    action,
                    explanation: "Case decision explanation, no reason name here".into(),
                    references: vec![],
                    author: Some("human".into()),
                },
            )
            .unwrap();
    };
    mutate(
        "close-not-planned",
        ticket::TicketCaseAction::Close {
            reason: ticket::TicketClosureReason::NotPlanned,
        },
    );
    for n in 0..8 {
        backend
            .create(ticket::NewTicket::new(format!("Newer unrelated {n}")))
            .unwrap();
    }
    let query = |value: serde_json::Value| {
        api.authority
            .query_tickets(serde_json::from_value(value).unwrap())
            .unwrap()
    };
    assert_eq!(
        query(json!({"closure_reason":"not_planned","limit":1})).items[0].id,
        reference.id
    );
    mutate(
        "correct-duplicate",
        ticket::TicketCaseAction::CorrectClosure {
            reason: ticket::TicketClosureReason::Duplicate,
        },
    );
    assert!(
        query(json!({"closure_reason":"not_planned","limit":1}))
            .items
            .is_empty()
    );
    assert_eq!(
        query(json!({"closure_reason":"duplicate","limit":1})).items[0].id,
        reference.id
    );
    assert_eq!(
        query(json!({"query":"not_planned","limit":1})).items[0].matched_fields,
        vec!["closure_reason"]
    );
    mutate("reopen", ticket::TicketCaseAction::Reopen);
    assert!(
        query(json!({"closure_reason":"duplicate","limit":1}))
            .items
            .is_empty()
    );
    assert_eq!(
        query(json!({"query":"duplicate","limit":1})).items[0].id,
        reference.id
    );
    mutate(
        "close-custom",
        ticket::TicketCaseAction::Close {
            reason: ticket::TicketClosureReason::Other("Superseded plan".into()),
        },
    );
    assert_eq!(
        query(json!({"closure_reason":{"other":"Superseded plan"},"limit":1})).items[0].id,
        reference.id
    );
    assert!(
        query(json!({"closure_reason":{"other":"Other plan"},"limit":1}))
            .items
            .is_empty()
    );
    assert_eq!(
        query(json!({"query":"Superseded plan","limit":1})).items[0].id,
        reference.id
    );
}

// FeatureStorage policy, body singleton ownership and signed Runtime HTTP are real.
// Bodies are Workdir-free. Connected concurrent delegate/tool execution is tested
// separately in subject_authorization_tests, not claimed by this identity test.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_subjects_signed_assignment_isolates_current_subjects_and_stale_receipts() {
    let dir = tempfile::tempdir().unwrap();
    init_clean_git_workspace(dir.path());
    for branch in ["subject-a-work", "subject-b-work"] {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(["branch", branch])
                .status()
                .unwrap()
                .success()
        );
    }
    let (mut api, _execution) = test_api_with_recording_backend(dir.path()).await;
    let identity = RuntimeIdentityMaterial::generate(EMBEDDED_WORKER_RUNTIME_ID).unwrap();
    configure_runtime_request_auth(&mut api, &identity, EMBEDDED_WORKER_RUNTIME_ID);
    let token = seed_test_workspace_owner_token(api.store.as_ref(), "parallel-subject-owner").await;
    let app = build_router(api.clone());
    let mut cases = Vec::new();
    for (name, branch) in [
        ("subject-a", "subject-a-work"),
        ("subject-b", "subject-b-work"),
    ] {
        let subject = open_subjektiv_store(&api)
            .unwrap()
            .create_subject_with_behavior(
                crate::subjektiv::SubjectRole::new(name).unwrap(),
                format!("Own only {name}'s Ticket."),
            )
            .unwrap();
        let mut input = ticket::NewTicket::new(name);
        set_test_ticket_target(&mut input, "test-repository", branch);
        let ticket = browser_ticket_backend(&api).unwrap().create(input).unwrap();
        grant_test_assignment_ticket(&api, &subject.id, &ticket.id);
        let created = request_json_authenticated(
            app.clone(),
            "POST",
            &format!("/api/w/{TEST_WORKSPACE_ID}/workers"),
            Some(
                json!({"runtime_id":EMBEDDED_WORKER_RUNTIME_ID, "display_name":name,
                "profile":"builtin:companion", "initial_submit":[],
                "feature_connections":{"subjektiv":{"subject_id":subject.id}},
                "workdir_attachments":[]}),
            ),
            &token,
            StatusCode::OK,
        )
        .await;
        let body = RuntimeWorkerRef::new(
            EMBEDDED_WORKER_RUNTIME_ID,
            created["worker_id"].as_str().unwrap(),
        );
        let assign_path = format!(
            "/api/w/{TEST_WORKSPACE_ID}/tickets/{}/assignments/orchestrator",
            ticket.id
        );
        let assign = json!({"operation_id":format!("assign-{name}"),
            "principal":{"kind":"subject", "subject_id":subject.id}});
        let receipt = tracking_response(
            app.clone(),
            runtime_source_request(
                &identity,
                Some(&body.worker_id),
                "PUT",
                &assign_path,
                serde_json::to_vec(&assign).unwrap(),
            ),
            StatusCode::OK,
        )
        .await;
        assert_eq!(receipt["assignment"]["principal"], assign["principal"]);
        assert_eq!(receipt["assignment"]["role"], "orchestrator");
        let current = api
            .store
            .list_current_ticket_assignment_tracking(TEST_WORKSPACE_ID, &ticket.id)
            .unwrap()
            .remove(0);
        assert_eq!(
            current.tracking.as_ref().unwrap().status,
            ticket::TicketAssignmentTrackingStatus::Waiting
        );
        let links = api
            .store
            .list_worker_workdir_links(TEST_WORKSPACE_ID, &body)
            .unwrap();
        assert!(links.is_empty());
        let key = crate::subjektiv::subject_worker_singleton_key(&subject.id).unwrap();
        assert_eq!(
            api.store
                .current_worker_singleton_owner(TEST_WORKSPACE_ID, &key)
                .unwrap()
                .unwrap()
                .worker,
            body
        );
        cases.push((subject.id, ticket.id, body, current));
    }
    assert_ne!(cases[0].0, cases[1].0);
    assert_ne!(cases[0].2, cases[1].2);
    assert!(
        api.store
            .current_worker_singleton_owner(
                TEST_WORKSPACE_ID,
                crate::hosts::WORKSPACE_ORCHESTRATOR_SINGLETON_KEY
            )
            .unwrap()
            .is_none()
    );
    let (subject_a, ticket_a, body_a, current_a) = &cases[0];
    let (subject_b, ticket_b, body_b, current_b) = &cases[1];
    let update_a = tracking_update(current_a, "a-result");
    // Exact operation grant is required even for a live body of another Subject.
    tracking_response(
        app.clone(),
        signed_tracking_update(&identity, Some(&body_b.worker_id), ticket_a, &update_a),
        StatusCode::FORBIDDEN,
    )
    .await;
    grant_test_assignment_ticket(&api, subject_b, ticket_a);
    // Granting the operation still cannot impersonate the assigned Subject.
    tracking_response(
        app.clone(),
        signed_tracking_update(&identity, Some(&body_b.worker_id), ticket_a, &update_a),
        StatusCode::CONFLICT,
    )
    .await;
    let saved = tracking_response(
        app.clone(),
        signed_tracking_update(&identity, Some(&body_a.worker_id), ticket_a, &update_a),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        api.store
            .get_ticket_assignment_tracking(TEST_WORKSPACE_ID, ticket_b, &current_b.assignment_id)
            .unwrap()
            .unwrap(),
        *current_b
    );
    // Same Subject body may author reassignment if granted, but ownership of A
    // immediately moves to B. A's old done receipt remains evidence, not authority.
    let reassign = json!({"operation_id":"a-to-b", "expected_assignment_id":current_a.assignment_id,
        "principal":{"kind":"subject", "subject_id":subject_b}});
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket_a}/assignments/orchestrator");
    tracking_response(
        app.clone(),
        runtime_source_request(
            &identity,
            Some(&body_a.worker_id),
            "PUT",
            &path,
            serde_json::to_vec(&reassign).unwrap(),
        ),
        StatusCode::OK,
    )
    .await;
    let next = api
        .store
        .list_current_ticket_assignment_tracking(TEST_WORKSPACE_ID, ticket_a)
        .unwrap()
        .remove(0);
    assert_ne!(next.assignment_id, current_a.assignment_id);
    let fresh = tracking_update(&next, "old-body-new-delegation");
    tracking_response(
        app.clone(),
        signed_tracking_update(&identity, Some(&body_a.worker_id), ticket_a, &fresh),
        StatusCode::CONFLICT,
    )
    .await;
    let mut forged_receipt = update_a.clone();
    forged_receipt.assignment_id = next.assignment_id.clone();
    forged_receipt.expected_last_event_id = next.last_event_id.clone().unwrap();
    tracking_response(
        app.clone(),
        signed_tracking_update(
            &identity,
            Some(&body_b.worker_id),
            ticket_a,
            &forged_receipt,
        ),
        StatusCode::CONFLICT,
    )
    .await;
    // A retained self-report receipt is never rewritten to point at the new row.
    assert_eq!(saved["assignment_id"], current_a.assignment_id);
    tracking_response(
        app.clone(),
        runtime_source_request(
            &identity,
            Some(&body_b.worker_id),
            "DELETE",
            &assignment_clear_path(ticket_a, &next.assignment_id, "clear-a-from-b"),
            Vec::new(),
        ),
        StatusCode::OK,
    )
    .await;
    let after_clear = tracking_update(&next, "after-clear");
    tracking_response(
        app.clone(),
        signed_tracking_update(&identity, Some(&body_b.worker_id), ticket_a, &after_clear),
        StatusCode::CONFLICT,
    )
    .await;
    assert!(
        api.store
            .list_current_ticket_assignment_tracking(TEST_WORKSPACE_ID, ticket_a)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        api.store
            .get_ticket_assignment_tracking(TEST_WORKSPACE_ID, ticket_b, &current_b.assignment_id)
            .unwrap()
            .unwrap(),
        *current_b
    );
    assert_eq!(
        api.authority.ticket(ticket_a).unwrap().state,
        server_api::BrowserTicketState::Open
    );
    assert_eq!(
        api.authority.ticket(ticket_b).unwrap().state,
        server_api::BrowserTicketState::Open
    );
    assert_eq!(
        open_subjektiv_store(&api)
            .unwrap()
            .subject(subject_a)
            .unwrap()
            .unwrap()
            .behavior_md,
        "Own only subject-a's Ticket."
    );
}

#[tokio::test]
async fn subject_operation_policy_revocation_rejects_saved_mutation_receipts_without_releasing_assignment()
 {
    use server_api::authorization::SubjectPermissionsUpdate;
    let dir = tempfile::tempdir().unwrap();
    let (api, _, identity, source, ticket) = tracking_fixture(dir.path()).await;
    let current = assign_tracking(&api, &identity, &source, &ticket).await;
    let input = tracking_update(&current, "result-before-policy-revocation");
    let app = build_router(api.clone());
    let saved = tracking_response(
        app.clone(),
        signed_tracking_update(&identity, Some(&source.worker_id), &ticket, &input),
        StatusCode::OK,
    )
    .await;
    let before = api
        .store
        .get_ticket_assignment_tracking(TEST_WORKSPACE_ID, &ticket, &current.assignment_id)
        .unwrap()
        .unwrap();
    let subject_id = ensure_test_assignment_subject(&api);
    let store = open_subjektiv_store(&api).unwrap();
    let policy = store.subject_permissions(&subject_id).unwrap().unwrap();
    store
        .set_subject_permissions(
            &subject_id,
            &SubjectPermissionsUpdate {
                operation_id: "revoke-assignment-operations".into(),
                expected_change_id: Some(policy.change_id),
                execution: vec![],
                delegation: vec![],
            },
        )
        .unwrap();
    let answer = json!({"assignment_id":current.assignment_id,
        "expected_last_event_id":saved["last_event_id"], "operation_key":"answer-after-revoke", "body":"No authority"});
    let answer_path = format!("/workspaces/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignment-answer");
    for request in [
        assignment_request(
            &api,
            &identity,
            &source.worker_id,
            &ticket,
            "assign-tracking",
        ),
        signed_tracking_update(&identity, Some(&source.worker_id), &ticket, &input),
        runtime_source_request(
            &identity,
            Some(&source.worker_id),
            "DELETE",
            &assignment_clear_path(&ticket, &current.assignment_id, "clear-after-revoke"),
            Vec::new(),
        ),
        runtime_source_request(
            &identity,
            Some(&source.worker_id),
            "POST",
            &answer_path,
            serde_json::to_vec(&answer).unwrap(),
        ),
    ] {
        tracking_response(app.clone(), request, StatusCode::FORBIDDEN).await;
    }
    assert_eq!(
        api.store
            .get_ticket_assignment_tracking(TEST_WORKSPACE_ID, &ticket, &current.assignment_id)
            .unwrap()
            .unwrap(),
        before
    );
    assert!(before.current);
    assert_eq!(
        api.store
            .list_ticket_assignment_tracking_history(
                TEST_WORKSPACE_ID,
                &ticket,
                &current.assignment_id
            )
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        api.authority.ticket(&ticket).unwrap().state,
        server_api::BrowserTicketState::Open
    );
}

#[tokio::test]
async fn generic_delegated_worker_authors_with_operation_grants_but_cannot_self_report_subject_handling()
 {
    use server_api::authorization::{
        PermissionResource, ScopedPermission, SubjectPermissionsUpdate,
    };
    let dir = tempfile::tempdir().unwrap();
    let (api, _, identity, body, ticket) = tracking_fixture(dir.path()).await;
    let subject_id = ensure_test_assignment_subject(&api);
    let store = open_subjektiv_store(&api).unwrap();
    let policy = store.subject_permissions(&subject_id).unwrap().unwrap();
    let permissions: Vec<_> = [
        "ticket.assignment.assign",
        "ticket.assignment.clear",
        "ticket.assignment.answer",
        "ticket.assignment.update",
    ]
    .into_iter()
    .map(|operation| ScopedPermission {
        operation: operation.into(),
        resource: PermissionResource::Ticket {
            workspace_id: TEST_WORKSPACE_ID.into(),
            ticket_id: ticket.clone(),
        },
    })
    .collect();
    let policy = store
        .set_subject_permissions(
            &subject_id,
            &SubjectPermissionsUpdate {
                operation_id: "allow-generic-assignment-author".into(),
                expected_change_id: Some(policy.change_id),
                execution: policy.execution,
                delegation: permissions.clone(),
            },
        )
        .unwrap();
    let app = build_router(api.clone());
    // Existing Worker control/FeatureStorage grant path, not a Ticket executor
    // or a role fixture. No implementation start or T-803 integration is claimed.
    let child = tracking_response(app.clone(), runtime_source_request(&identity, Some(&body.worker_id), "POST",
        &format!("/api/w/{TEST_WORKSPACE_ID}/worker-control/workers"),
        serde_json::to_vec(&json!({"runtime_id":body.runtime_id, "display_name":"Generic assignment author",
            "profile":"builtin:intake", "initial_submit":[], "workdir_attachments":[], "feature_connections":{},
            "control_operation_id":"spawn-generic-author",
            "execution_grant":{"permissions":permissions, "redelegation":[]}})).unwrap()), StatusCode::OK).await;
    let worker_id = child["worker_id"].as_str().unwrap();
    assert!(child["worker"]["singleton_key"].is_null());
    assert_eq!(child["worker"]["profile"], "builtin:intake");
    let saved = tracking_response(
        app.clone(),
        assignment_request(&api, &identity, worker_id, &ticket, "generic-assign"),
        StatusCode::OK,
    )
    .await;
    let current = api
        .store
        .list_current_ticket_assignment_tracking(TEST_WORKSPACE_ID, &ticket)
        .unwrap()
        .remove(0);
    assert_eq!(
        saved["assignment"]["principal"],
        test_assignment_principal(&api)
    );
    assert_eq!(
        saved["assignment"]["assigned_by"],
        format!("worker:{}/{worker_id}", body.runtime_id)
    );
    // Operation policy alone never makes a background Worker the Subject body.
    tracking_response(
        app.clone(),
        signed_tracking_update(
            &identity,
            Some(worker_id),
            &ticket,
            &tracking_update(&current, "child-cannot-report"),
        ),
        StatusCode::FORBIDDEN,
    )
    .await;
    let answer = json!({"assignment_id":current.assignment_id,
        "expected_last_event_id":current.last_event_id, "operation_key":"generic-answer", "body":"Additional authorized input"});
    let answer_path = format!("/workspaces/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignment-answer");
    let receipt = tracking_response(
        app.clone(),
        runtime_source_request(
            &identity,
            Some(worker_id),
            "POST",
            &answer_path,
            serde_json::to_vec(&answer).unwrap(),
        ),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        tracking_response(
            app.clone(),
            runtime_source_request(
                &identity,
                Some(worker_id),
                "POST",
                &answer_path,
                serde_json::to_vec(&answer).unwrap()
            ),
            StatusCode::OK
        )
        .await,
        receipt
    );
    let clear = assignment_clear_path(&ticket, &current.assignment_id, "generic-clear");
    tracking_response(
        app.clone(),
        runtime_source_request(&identity, Some(worker_id), "DELETE", &clear, Vec::new()),
        StatusCode::OK,
    )
    .await;
    store
        .set_subject_permissions(
            &subject_id,
            &SubjectPermissionsUpdate {
                operation_id: "revoke-generic-author".into(),
                expected_change_id: Some(policy.change_id),
                execution: policy.execution,
                delegation: vec![],
            },
        )
        .unwrap();
    for request in [
        assignment_request(&api, &identity, worker_id, &ticket, "generic-assign"),
        runtime_source_request(&identity, Some(worker_id), "DELETE", &clear, Vec::new()),
        runtime_source_request(
            &identity,
            Some(worker_id),
            "POST",
            &answer_path,
            serde_json::to_vec(&answer).unwrap(),
        ),
    ] {
        tracking_response(app.clone(), request, StatusCode::FORBIDDEN).await;
    }
    let retained = api
        .store
        .get_ticket_assignment_tracking(TEST_WORKSPACE_ID, &ticket, &current.assignment_id)
        .unwrap()
        .unwrap();
    assert!(!retained.current);
    assert_eq!(
        retained.tracking.unwrap().status,
        ticket::TicketAssignmentTrackingStatus::Waiting
    );
    assert!(
        api.store
            .list_current_ticket_assignment_tracking(TEST_WORKSPACE_ID, &ticket)
            .unwrap()
            .is_empty()
    );
}
