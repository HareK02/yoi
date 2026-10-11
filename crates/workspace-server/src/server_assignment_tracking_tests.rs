use super::*;

#[tokio::test]
async fn runtime_and_account_assign_retries_return_saved_result_after_clear_and_coder_start() {
    for runtime_source in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let (api, _, identity, source, ticket) = tracking_fixture(directory.path()).await;
        let token = seed_test_api_token(api.store.as_ref(), "assign-replay-owner");
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
                "principal": {"kind":"workspace_agent","agent_key":"workspace-orchestrator"}
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
            request("PUT", &worker_path, worker_input),
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
        // Only a genuinely new operation is refused; a receipt never restores
        // the cleared current pointer, initializes tracking, or starts work.
        tracking_response(
            app,
            request("PUT", &path, input("new-assign")),
            StatusCode::CONFLICT,
        )
        .await;
        assert_eq!(counts(), before);
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
        assert!(
            api.store
                .get_current_ticket_role_assignment(
                    TEST_WORKSPACE_ID,
                    &ticket,
                    TicketAssignmentRole::Orchestrator
                )
                .unwrap()
                .is_none()
        );
    }
}

fn tracking_path(ticket: &str) -> String {
    format!("/workspaces/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignment-tracking")
}

fn assignment_request(
    identity: &RuntimeIdentityMaterial,
    worker: &str,
    ticket: &str,
    operation: &str,
) -> Request<Body> {
    runtime_source_request(identity, Some(worker), "PUT",
        &format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignments/orchestrator"),
        serde_json::to_vec(&serde_json::json!({"operation_id":operation,"principal":{"kind":"workspace_agent","agent_key":"workspace-orchestrator"}})).unwrap())
}

fn assignment_clear_path(ticket: &str, assignment: &str, operation: &str) -> String {
    format!(
        "/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignments/orchestrator?assignment_id={assignment}&operation_id={operation}"
    )
}

fn replace_tracking_orchestrator(
    api: &WorkspaceApi,
    runtime: &WorkdirlessFixtureRuntime,
    source: &RuntimeWorkerRef,
) -> RuntimeWorkerRef {
    let replacement = RuntimeWorkerRef::new(&source.runtime_id, WorkerId::now_v7().to_string());
    seed_worker_source_member(api, &replacement.runtime_id, &replacement.worker_id);
    let mut summary = runtime.workers.lock().unwrap()[0].clone();
    summary.worker = replacement.clone();
    runtime.workers.lock().unwrap().push(summary);
    let conn = rusqlite::Connection::open(&api.config.database_path).unwrap();
    conn.execute("UPDATE worker_singleton_owners SET worker_id=?1,generation=generation+1 WHERE workspace_id=?2 AND singleton_key=?3", rusqlite::params![replacement.worker_id, TEST_WORKSPACE_ID, crate::hosts::WORKSPACE_ORCHESTRATOR_SINGLETON_KEY]).unwrap();
    assert_eq!(
        find_workspace_orchestrator(api).unwrap().worker,
        replacement
    );
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
        assignment_request(&identity, &source.worker_id, &ticket, "assign-tracking"),
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
    let (api, runtime, identity, source, ticket) = tracking_fixture(dir.path()).await;
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
    let replacement = replace_tracking_orchestrator(&api, &runtime, &source);
    for worker in [&source.worker_id, &replacement.worker_id] {
        let expected = if worker == &source.worker_id {
            StatusCode::UNAUTHORIZED
        } else {
            StatusCode::CONFLICT
        };
        tracking_response(
            app.clone(),
            assignment_request(&identity, worker, &ticket, "assign-tracking"),
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
async fn runtime_orchestrator_tags_without_backend_singleton_do_not_grant_assignment_authority() {
    let dir = tempfile::tempdir().unwrap();
    let (api, runtime, identity, source, ticket) = tracking_fixture(dir.path()).await;
    let current = assign_tracking(&api, &identity, &source, &ticket).await;
    let forged = RuntimeWorkerRef::new(&source.runtime_id, WorkerId::now_v7().to_string());
    seed_worker_source_member(&api, &forged.runtime_id, &forged.worker_id);
    let mut summary = runtime.workers.lock().unwrap()[0].clone();
    summary.worker = forged.clone();
    summary.singleton_key = Some(crate::hosts::WORKSPACE_ORCHESTRATOR_SINGLETON_KEY.into());
    runtime.workers.lock().unwrap().push(summary);
    let app = build_router(api.clone());
    for request in [
        assignment_request(&identity, &forged.worker_id, &ticket, "forged-assign"),
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
    let owner = seed_test_api_token(api.store.as_ref(), "assignment-account-owner");
    let other = seed_test_api_token(api.store.as_ref(), "assignment-account-other");
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignments/orchestrator");
    let body = serde_json::to_vec(&serde_json::json!({"operation_id":"account-assign", "principal":{"kind":"workspace_agent","agent_key":"workspace-orchestrator"}})).unwrap();
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
            "account-assignment-account-owner",
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
        StatusCode::CONFLICT,
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
        StatusCode::CONFLICT,
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
        serde_json::to_vec(&serde_json::json!({ "operation_id": "assign-tracking", "principal": { "kind": "workspace_agent", "agent_key": "workspace-orchestrator" } })).unwrap(),
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
    let memory = api
        .config_store
        .get_workspace_memory_settings(TEST_WORKSPACE_ID)
        .unwrap();
    let reservation = api
        .config_store
        .reserve_worker_create(
            TEST_WORKSPACE_ID,
            runtime_id,
            "tracking-orchestrator",
            "sha256:tracking-orchestrator",
            Some(crate::hosts::WORKSPACE_ORCHESTRATOR_SINGLETON_KEY),
            &memory,
        )
        .unwrap();
    let mut summary = WorkdirlessFixtureRuntime::worker_summary(
        WorkerCreateBinding {
            worker_id: reservation.worker_id,
            create_fingerprint: reservation.create_fingerprint,
        },
        &WorkerSpawnRequest {
            requested_worker_name: Some("Tracking Orchestrator".into()),
            singleton_key: Some(crate::hosts::WORKSPACE_ORCHESTRATOR_SINGLETON_KEY.into()),
            intent: WorkerSpawnIntent::WorkspaceCompanion,
            acceptance: WorkerSpawnAcceptanceRequirement::RunAccepted {
                expected_segments: 0,
            },
            profile: ProfileSelector::Builtin("builtin:orchestrator".into()),
            ticket_assignment: None,
            initial_submit: Vec::new(),
            workdir_attachment_requests: Vec::new(),
            resolved_workdir_attachment_requests: Vec::new(),
            resolved_workdir_attachments: Vec::new(),
            resolved_config_bundle: None,
            resolved_worker_observation_enabled: false,
            resolved_worker_observation_grants: Vec::new(),
            resolved_workspace_api: None,
            resolved_memory_settings: None,
            resolved_subjektiv_attached: false,
            resolved_control_operation: None,
        },
    );
    summary.tags = vec!["orchestrator".into(), "singleton".into()];
    let source = summary.worker.clone();
    runtime.workers.lock().unwrap().push(summary);
    seed_worker_source_member(&api, runtime_id, &source.worker_id);
    api.config_store
        .complete_worker_create_reservation(TEST_WORKSPACE_ID, source.worker_id.parse().unwrap())
        .unwrap();
    assert_eq!(find_workspace_orchestrator(&api).unwrap().worker, source);
    let ticket = browser_ticket_backend(&api)
        .unwrap()
        .create(ticket::NewTicket::new("Tracking authority"))
        .unwrap();
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
    tracking_response(app.clone(), runtime_source_request(&identity, Some(&source.worker_id), "PUT", &format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignments/orchestrator"), serde_json::to_vec(&serde_json::json!({"operation_id":"assign-again", "principal":{"kind":"workspace_agent", "agent_key":"workspace-orchestrator"}})).unwrap()), StatusCode::OK).await;
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
    let (api, runtime, identity, source, ticket) = tracking_fixture(dir.path()).await;
    let current = assign_tracking(&api, &identity, &source, &ticket).await;
    let update = tracking_update(&current, "old-orchestrator-result");
    let app = build_router(api.clone());
    let result = tracking_response(
        app.clone(),
        signed_tracking_update(&identity, Some(&source.worker_id), &ticket, &update),
        StatusCode::OK,
    )
    .await;
    let replacement = RuntimeWorkerRef::new(&source.runtime_id, WorkerId::now_v7().to_string());
    seed_worker_source_member(&api, &replacement.runtime_id, &replacement.worker_id);
    let mut summary = runtime.workers.lock().unwrap()[0].clone();
    summary.worker = replacement.clone();
    runtime.workers.lock().unwrap().push(summary);
    // Seed the authoritative replacement, not merely a Runtime tag or model role.
    let conn = rusqlite::Connection::open(&api.config.database_path).unwrap();
    conn.execute("UPDATE worker_singleton_owners SET worker_id=?1,generation=generation+1 WHERE workspace_id=?2 AND singleton_key=?3", rusqlite::params![replacement.worker_id, TEST_WORKSPACE_ID, crate::hosts::WORKSPACE_ORCHESTRATOR_SINGLETON_KEY]).unwrap();
    assert_eq!(
        find_workspace_orchestrator(&api).unwrap().worker,
        replacement
    );
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
    let token = seed_test_api_token(api.store.as_ref(), "assignment-replay-user");
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignments/orchestrator");
    let body = serde_json::to_vec(&serde_json::json!({"operation_id":"assign-tracking", "principal":{"kind":"workspace_agent", "agent_key":"workspace-orchestrator"}})).unwrap();
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
    let token = seed_test_api_token(api.store.as_ref(), "human-not-tracking-agent");
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
    let body = serde_json::to_vec(&serde_json::json!({"operation_id":"assign-tracking", "principal":{"kind":"workspace_agent", "agent_key":"workspace-orchestrator"}})).unwrap();
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
    let token = seed_test_api_token(api.store.as_ref(), "another-clear-actor");
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
        serde_json::json!({"kind":"workspace_agent","agent_key":"workspace-orchestrator"})
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
    let assignment = tracking_response(app.clone(), runtime_source_request(&identity, Some(&source.worker_id), "PUT", &format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignments/orchestrator"), serde_json::to_vec(&serde_json::json!({"operation_id":"request-again", "expected_assignment_id":current.assignment_id,"principal":{"kind":"workspace_agent","agent_key":"workspace-orchestrator"}})).unwrap()), StatusCode::OK).await;
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
        "principal": { "kind": "workspace_agent", "agent_key": "workspace-orchestrator" }
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
