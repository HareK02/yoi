use super::*;

async fn case_response(
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
        serde_json::from_slice(&body).unwrap()
    }
}

fn signed_case_request(
    identity: &RuntimeIdentityMaterial,
    worker: Option<&str>,
    ticket: &str,
    input: &ticket::TicketCaseUpdate,
) -> Request<Body> {
    let mut body = serde_json::to_value(input).unwrap();
    body.as_object_mut().unwrap().remove("author");
    runtime_source_request(
        identity,
        worker,
        "POST",
        &format!("/workspaces/{TEST_WORKSPACE_ID}/tickets/{ticket}/case"),
        serde_json::to_vec(&body).unwrap(),
    )
}

#[tokio::test]
async fn signed_unassigned_worker_without_hints_cannot_close_reopen_or_correct_cases() {
    let dir = tempfile::tempdir().unwrap();
    let (mut api, execution) = test_api_with_recording_backend(dir.path()).await;
    let identity = RuntimeIdentityMaterial::generate("runtime-case").unwrap();
    configure_runtime_request_auth(&mut api, &identity, "runtime-case");
    seed_worker_source_member(&api, "runtime-case", "other");
    let backend = browser_ticket_backend(&api).unwrap();
    let reference = backend
        .create(ticket::NewTicket::new("Case authority"))
        .unwrap();
    seed_case_assignment(
        &api,
        &reference.id,
        &RuntimeWorkerRef::new("runtime-case", "owner"),
        "unauthorized-owner",
    );
    let app = build_router(api.clone());
    for action in [
        ticket::TicketCaseAction::Close {
            reason: ticket::TicketClosureReason::Completed,
        },
        ticket::TicketCaseAction::Reopen,
        ticket::TicketCaseAction::CorrectClosure {
            reason: ticket::TicketClosureReason::NotPlanned,
        },
    ] {
        if !matches!(action, ticket::TicketCaseAction::Close { .. }) {
            let before = backend.show(reference.id.clone().into()).unwrap();
            if before.meta.workflow_state == TicketState::Open {
                backend
                    .update_case(
                        &reference.id,
                        case_test_request(
                            &before,
                            "fixture-close",
                            ticket::TicketCaseAction::Close {
                                reason: ticket::TicketClosureReason::Completed,
                            },
                        ),
                    )
                    .unwrap();
            }
        }
        let before = backend.show(reference.id.clone().into()).unwrap();
        let input = case_test_request(&before, "unauthorized", action);
        let authority = case_authority_snapshot(&api);
        case_response(
            app.clone(),
            signed_case_request(&identity, Some("other"), &reference.id, &input),
            StatusCode::FORBIDDEN,
        )
        .await;
        assert_eq!(backend.show(reference.id.clone().into()).unwrap(), before);
        assert_eq!(case_authority_snapshot(&api), authority);
    }
    assert!(execution.protocol_methods().is_empty());
}

#[tokio::test]
async fn signed_runtime_without_worker_cannot_make_case_judgments() {
    let dir = tempfile::tempdir().unwrap();
    let mut api = test_api(dir.path()).await;
    let identity = RuntimeIdentityMaterial::generate("runtime-case").unwrap();
    configure_runtime_request_auth(&mut api, &identity, "runtime-case");
    let backend = browser_ticket_backend(&api).unwrap();
    let reference = backend
        .create(ticket::NewTicket::new("Runtime is not a case actor"))
        .unwrap();
    let before = backend.show(reference.id.clone().into()).unwrap();
    let input = case_test_request(
        &before,
        "runtime-only",
        ticket::TicketCaseAction::Close {
            reason: ticket::TicketClosureReason::Completed,
        },
    );
    case_response(
        build_router(api.clone()),
        signed_case_request(&identity, None, &reference.id, &input),
        StatusCode::FORBIDDEN,
    )
    .await;
    assert_eq!(backend.show(reference.id.into()).unwrap(), before);
}

fn seed_case_assignment(api: &WorkspaceApi, ticket: &str, worker: &RuntimeWorkerRef, key: &str) {
    seed_worker_source_member(api, &worker.runtime_id, &worker.worker_id);
    api.store
        .set_current_ticket_worker_assignment(
            &TicketWorkerAssignmentRecord {
                workspace_id: TEST_WORKSPACE_ID.into(),
                ticket_id: ticket.into(),
                assignment_id: key.into(),
                worker: worker.clone(),
                assigned_by: "test".into(),
                assigned_at: TEST_CREATED_AT.into(),
            },
            None,
            &format!("event-{key}"),
            &format!("operation-{key}"),
            false,
        )
        .unwrap();
}

fn case_authority_snapshot(api: &WorkspaceApi) -> Vec<Vec<Vec<rusqlite::types::Value>>> {
    let conn = rusqlite::Connection::open(&api.config.database_path).unwrap();
    [
        "worker_registry",
        "worker_control_grants",
        "ticket_current_worker_assignments",
        "ticket_assignment_operations",
        "ticket_assignment_work_releases",
        "backend_jobs",
    ]
    .into_iter()
    .map(|table| {
        let mut statement = conn
            .prepare(&format!(
                "SELECT * FROM {table} WHERE workspace_id=?1 ORDER BY rowid"
            ))
            .unwrap();
        let columns = statement.column_count();
        statement
            .query_map([TEST_WORKSPACE_ID], |row| {
                (0..columns)
                    .map(|column| row.get(column))
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    })
    .collect()
}

#[tokio::test]
async fn signed_assigned_worker_case_replay_keeps_original_actor_after_release_without_restoring_grant()
 {
    let dir = tempfile::tempdir().unwrap();
    let (mut api, execution) = test_api_with_recording_backend(dir.path()).await;
    let identity = RuntimeIdentityMaterial::generate("runtime-case").unwrap();
    configure_runtime_request_auth(&mut api, &identity, "runtime-case");
    let source = RuntimeWorkerRef::new("runtime-case", "owner");
    let backend = browser_ticket_backend(&api).unwrap();
    let reference = backend
        .create(ticket::NewTicket::new("Source-bound case receipt"))
        .unwrap();
    seed_case_assignment(&api, &reference.id, &source, "original-work");
    let before = backend.show(reference.id.clone().into()).unwrap();
    let input = case_test_request(
        &before,
        "signed-close",
        ticket::TicketCaseAction::Close {
            reason: ticket::TicketClosureReason::Completed,
        },
    );
    let app = build_router(api.clone());
    let first = signed_case_request(&identity, Some("owner"), &reference.id, &input);
    let old_proof =
        first.headers()[worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER].clone();
    let response = case_response(app.clone(), first, StatusCode::OK).await;
    assert_eq!(response["workflow_state"], "closed");
    let closed = backend.show(reference.id.clone().into()).unwrap();
    let event = closed.events.last().unwrap();
    assert_eq!(
        closed.case_history.last().unwrap().author,
        "worker:runtime-case/owner"
    );
    assert_eq!(event.author.as_deref(), Some("worker:runtime-case/owner"));
    for (key, expected) in [
        ("source_runtime_id", "runtime-case"),
        ("source_worker_id", "owner"),
        ("source_actor_role", "ticket_worker"),
        ("source_assignment_id", "original-work"),
    ] {
        assert_eq!(
            event.attributes.get(key).map(String::as_str),
            Some(expected)
        );
    }
    assert!(
        api.store
            .get_active_ticket_worker_assignment(TEST_WORKSPACE_ID, &reference.id)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        api.store
            .get_current_ticket_worker_assignment(TEST_WORKSPACE_ID, &reference.id)
            .unwrap()
            .unwrap()
            .worker,
        source
    );
    let released = case_authority_snapshot(&api);
    // Proof tokens are single-use; operation receipts survive fresh signatures.
    let mut reused_proof = signed_case_request(&identity, Some("owner"), &reference.id, &input);
    reused_proof.headers_mut().insert(
        worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER,
        old_proof,
    );
    case_response(app.clone(), reused_proof, StatusCode::UNAUTHORIZED).await;
    assert_eq!(
        case_response(
            app.clone(),
            signed_case_request(&identity, Some("owner"), &reference.id, &input),
            StatusCode::OK
        )
        .await,
        response
    );
    let mut changed = input.clone();
    changed.explanation = "Changed judgment under the same key".into();
    case_response(
        app.clone(),
        signed_case_request(&identity, Some("owner"), &reference.id, &changed),
        StatusCode::CONFLICT,
    )
    .await;
    let reopen = case_test_request(
        &closed,
        "released-worker-reopen",
        ticket::TicketCaseAction::Reopen,
    );
    case_response(
        app.clone(),
        signed_case_request(&identity, Some("owner"), &reference.id, &reopen),
        StatusCode::FORBIDDEN,
    )
    .await;
    let correction = case_test_request(
        &closed,
        "released-worker-correction",
        ticket::TicketCaseAction::CorrectClosure {
            reason: ticket::TicketClosureReason::NotPlanned,
        },
    );
    case_response(
        app,
        signed_case_request(&identity, Some("owner"), &reference.id, &correction),
        StatusCode::FORBIDDEN,
    )
    .await;
    assert_eq!(backend.show(reference.id.into()).unwrap(), closed);
    assert_eq!(case_authority_snapshot(&api), released);
    assert!(execution.protocol_methods().is_empty());
}

#[tokio::test]
async fn signed_case_hints_cannot_replace_verified_worker_or_human_identity() {
    let dir = tempfile::tempdir().unwrap();
    let mut api = test_api(dir.path()).await;
    let identity = RuntimeIdentityMaterial::generate("runtime-case").unwrap();
    configure_runtime_request_auth(&mut api, &identity, "runtime-case");
    let backend = browser_ticket_backend(&api).unwrap();
    let reference = backend
        .create(ticket::NewTicket::new("Hints are assertions"))
        .unwrap();
    seed_case_assignment(
        &api,
        &reference.id,
        &RuntimeWorkerRef::new("runtime-case", "owner"),
        "hint-owner",
    );
    seed_worker_source_member(&api, "runtime-case", "other");
    let before = backend.show(reference.id.clone().into()).unwrap();
    let authority = case_authority_snapshot(&api);
    let input = case_test_request(
        &before,
        "hint-close",
        ticket::TicketCaseAction::Close {
            reason: ticket::TicketClosureReason::Completed,
        },
    );
    let app = build_router(api.clone());
    for (worker, runtime_hint, worker_hint) in [
        (Some("other"), Some("runtime-case"), Some("owner")),
        (Some("owner"), Some("another-runtime"), Some("owner")),
        (Some("owner"), Some("runtime-case"), None),
        (None, Some("runtime-case"), Some("owner")),
    ] {
        let mut request = signed_case_request(&identity, worker, &reference.id, &input);
        if let Some(runtime) = runtime_hint {
            request
                .headers_mut()
                .insert("x-yoi-runtime-id", HeaderValue::from_str(runtime).unwrap());
        }
        if let Some(worker) = worker_hint {
            request
                .headers_mut()
                .insert("x-yoi-worker-id", HeaderValue::from_str(worker).unwrap());
        }
        case_response(app.clone(), request, StatusCode::FORBIDDEN).await;
        assert_eq!(backend.show(reference.id.clone().into()).unwrap(), before);
        assert_eq!(case_authority_snapshot(&api), authority);
    }
    let token = seed_test_api_token(api.store.as_ref(), "hint-human");
    let mut human = signed_case_request(&identity, Some("owner"), &reference.id, &input);
    human
        .headers_mut()
        .remove(worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER);
    human.headers_mut().insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
    );
    human
        .headers_mut()
        .insert("x-yoi-runtime-id", HeaderValue::from_static("runtime-case"));
    human
        .headers_mut()
        .insert("x-yoi-worker-id", HeaderValue::from_static("owner"));
    case_response(app.clone(), human, StatusCode::FORBIDDEN).await;
    assert_eq!(backend.show(reference.id.clone().into()).unwrap(), before);
    assert_eq!(case_authority_snapshot(&api), authority);
    let mut matching = signed_case_request(&identity, Some("owner"), &reference.id, &input);
    matching
        .headers_mut()
        .insert("x-yoi-runtime-id", HeaderValue::from_static("runtime-case"));
    matching
        .headers_mut()
        .insert("x-yoi-worker-id", HeaderValue::from_static("owner"));
    case_response(app, matching, StatusCode::OK).await;
}

#[tokio::test]
async fn case_receipt_cannot_be_replayed_by_different_authenticated_worker() {
    let dir = tempfile::tempdir().unwrap();
    let mut api = test_api(dir.path()).await;
    let identity = RuntimeIdentityMaterial::generate("runtime-case").unwrap();
    configure_runtime_request_auth(&mut api, &identity, "runtime-case");
    let backend = browser_ticket_backend(&api).unwrap();
    let reference = backend
        .create(ticket::NewTicket::new("Distinct Worker receipt identity"))
        .unwrap();
    seed_case_assignment(
        &api,
        &reference.id,
        &RuntimeWorkerRef::new("runtime-case", "first"),
        "first-work",
    );
    let before = backend.show(reference.id.clone().into()).unwrap();
    let input = case_test_request(
        &before,
        "identity-key",
        ticket::TicketCaseAction::Close {
            reason: ticket::TicketClosureReason::Completed,
        },
    );
    let app = build_router(api.clone());
    case_response(
        app.clone(),
        signed_case_request(&identity, Some("first"), &reference.id, &input),
        StatusCode::OK,
    )
    .await;
    let closed = backend.show(reference.id.clone().into()).unwrap();
    backend
        .update_case(
            &reference.id,
            case_test_request(&closed, "fixture-reopen", ticket::TicketCaseAction::Reopen),
        )
        .unwrap();
    api.store
        .clear_current_ticket_worker_assignment(
            TEST_WORKSPACE_ID,
            &reference.id,
            Some("first-work"),
            "replace-operation",
            "replace-event",
            "test",
            TEST_CREATED_AT,
        )
        .unwrap();
    seed_case_assignment(
        &api,
        &reference.id,
        &RuntimeWorkerRef::new("runtime-case", "second"),
        "second-work",
    );
    let reopened = backend.show(reference.id.clone().into()).unwrap();
    let authority = case_authority_snapshot(&api);
    case_response(
        app,
        signed_case_request(&identity, Some("second"), &reference.id, &input),
        StatusCode::CONFLICT,
    )
    .await;
    assert_eq!(backend.show(reference.id.into()).unwrap(), reopened);
    assert_eq!(case_authority_snapshot(&api), authority);
}

// The generic dispatcher has no public UpdateCase route of its own. Compose its
// module boundary with the same real middleware, rather than inventing claims.
fn generic_case_router(api: WorkspaceApi) -> Router {
    async fn update(
        State(api): State<WorkspaceApi>,
        axum::Extension(context): axum::Extension<server_api::ServerRequestContext>,
        AxumPath((workspace, ticket)): AxumPath<(String, String)>,
        headers: HeaderMap,
        Json(request): Json<ticket::TicketCaseUpdate>,
    ) -> ApiResult<Json<ticket::Ticket>> {
        let result = execute_ticket_rest_operation_with_context(
            &api,
            &workspace,
            headers,
            Some(&context),
            TicketBackendOperation::UpdateCase { ticket, request },
        )
        .await?;
        match result {
            TicketBackendOperationResult::Ticket(ticket) => Ok(Json(ticket)),
            _ => panic!("case dispatch returned a non-Ticket result"),
        }
    }
    Router::new()
        .route(
            "/workspaces/{workspace_id}/tickets/{ticket_id}/case",
            post(update),
        )
        .with_state(api.clone())
        .layer(middleware::from_fn_with_state(
            ServerApiContractService::Workspace(api.clone()),
            attach_server_request_context,
        ))
        .layer(middleware::from_fn_with_state(
            api,
            authorize_workspace_api_request,
        ))
}

#[tokio::test]
async fn typed_and_generic_case_dispatch_keep_the_same_verified_worker_and_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let mut api = test_api(dir.path()).await;
    let identity = RuntimeIdentityMaterial::generate("runtime-case").unwrap();
    configure_runtime_request_auth(&mut api, &identity, "runtime-case");
    let source = RuntimeWorkerRef::new("runtime-case", "owner");
    let backend = browser_ticket_backend(&api).unwrap();
    let reference = backend
        .create(ticket::NewTicket::new("Dispatch parity"))
        .unwrap();
    seed_case_assignment(&api, &reference.id, &source, "dispatch-work");
    seed_worker_source_member(&api, "runtime-case", "other");
    let before = backend.show(reference.id.clone().into()).unwrap();
    let mut input = case_test_request(
        &before,
        "generic-close",
        ticket::TicketCaseAction::Close {
            reason: ticket::TicketClosureReason::Completed,
        },
    );
    input.author = Some("forged-client-actor".into());
    let path = format!(
        "/workspaces/{TEST_WORKSPACE_ID}/tickets/{}/case",
        reference.id
    );
    let generic = generic_case_router(api.clone());
    for worker in [Some("other"), None] {
        case_response(
            generic.clone(),
            signed_case_request(&identity, worker, &reference.id, &input),
            StatusCode::FORBIDDEN,
        )
        .await;
        assert_eq!(backend.show(reference.id.clone().into()).unwrap(), before);
    }
    let closed = case_response(
        generic.clone(),
        runtime_source_request(
            &identity,
            Some("owner"),
            "POST",
            &path,
            serde_json::to_vec(&input).unwrap(),
        ),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        closed["case_history"][0]["author"],
        "worker:runtime-case/owner"
    );
    case_response(
        build_router(api.clone()),
        signed_case_request(&identity, Some("owner"), &reference.id, &input),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        case_response(
            generic,
            runtime_source_request(
                &identity,
                Some("owner"),
                "POST",
                &path,
                serde_json::to_vec(&input).unwrap()
            ),
            StatusCode::OK
        )
        .await,
        closed
    );
    let error = execute_ticket_rest_operation_with_context(
        &api,
        TEST_WORKSPACE_ID,
        HeaderMap::new(),
        None,
        TicketBackendOperation::UpdateCase {
            ticket: reference.id.clone(),
            request: input,
        },
    )
    .await
    .unwrap_err();
    assert_eq!(error.into_response().status(), StatusCode::FORBIDDEN);
    assert_eq!(
        serde_json::to_value(backend.show(reference.id.into()).unwrap()).unwrap(),
        closed
    );
}

#[tokio::test]
async fn authenticated_human_case_actor_and_replay_are_bound_to_verified_account_and_user() {
    let dir = tempfile::tempdir().unwrap();
    let api = test_api(dir.path()).await;
    let token = seed_test_api_token(api.store.as_ref(), "case-human");
    let other_token = seed_test_api_token(api.store.as_ref(), "other-human");
    let backend = browser_ticket_backend(&api).unwrap();
    let reference = backend
        .create(ticket::NewTicket::new("Human case identity"))
        .unwrap();
    let before = backend.show(reference.id.clone().into()).unwrap();
    let mut input = case_test_request(
        &before,
        "human-close",
        ticket::TicketCaseAction::Close {
            reason: ticket::TicketClosureReason::Completed,
        },
    );
    input.author = Some("forged-author".into());
    let path = format!(
        "/workspaces/{TEST_WORKSPACE_ID}/tickets/{}/case",
        reference.id
    );
    let human_request = |token: &str, typed: bool| {
        let mut body = serde_json::to_value(&input).unwrap();
        if typed {
            body.as_object_mut().unwrap().remove("author");
        }
        Request::builder()
            .method("POST")
            .uri(&path)
            .header("authorization", format!("Bearer {token}"))
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap()
    };
    let typed = build_router(api.clone());
    let response = case_response(typed.clone(), human_request(&token, true), StatusCode::OK).await;
    let closed = backend.show(reference.id.clone().into()).unwrap();
    let event = closed.events.last().unwrap();
    assert_eq!(event.attributes["source_account_id"], "account-case-human");
    assert_eq!(event.attributes["source_user_id"], "user-case-human");
    assert_eq!(
        event.author.as_deref(),
        Some("workspace-user:[\"account-case-human\",\"user-case-human\"]")
    );
    assert_eq!(
        closed.case_history.last().unwrap().author,
        event.author.clone().unwrap()
    );
    assert_eq!(
        case_response(typed.clone(), human_request(&token, true), StatusCode::OK).await,
        response
    );
    case_response(
        typed,
        human_request(&other_token, true),
        StatusCode::CONFLICT,
    )
    .await;
    // Generic dispatch receives an explicit untrusted author but must override it.
    case_response(
        generic_case_router(api.clone()),
        human_request(&token, false),
        StatusCode::OK,
    )
    .await;
    assert_eq!(backend.show(reference.id.clone().into()).unwrap(), closed);
    // A human reopen preserves the previous immutable judgment and its actor.
    let reopen = case_test_request(&closed, "human-reopen", ticket::TicketCaseAction::Reopen);
    let mut body = serde_json::to_value(reopen).unwrap();
    body.as_object_mut().unwrap().remove("author");
    case_response(
        build_router(api.clone()),
        Request::builder()
            .method("POST")
            .uri(path)
            .header("authorization", format!("Bearer {token}"))
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    let reopened = backend.show(reference.id.into()).unwrap();
    assert_eq!(reopened.meta.workflow_state, TicketState::Open);
    assert_eq!(reopened.case_history[0], closed.case_history[0]);
    assert_eq!(
        reopened.events.last().unwrap().attributes["source_user_id"],
        "user-case-human"
    );
}

#[tokio::test]
async fn public_case_route_requires_valid_authentication_and_workspace_scope_before_writes() {
    let dir = tempfile::tempdir().unwrap();
    let mut api = test_api(dir.path()).await;
    let identity = RuntimeIdentityMaterial::generate("runtime-case").unwrap();
    configure_runtime_request_auth(&mut api, &identity, "runtime-case");
    let backend = browser_ticket_backend(&api).unwrap();
    let reference = backend
        .create(ticket::NewTicket::new("Authenticated route"))
        .unwrap();
    seed_case_assignment(
        &api,
        &reference.id,
        &RuntimeWorkerRef::new("runtime-case", "owner"),
        "auth-work",
    );
    let before = backend.show(reference.id.clone().into()).unwrap();
    let authority = case_authority_snapshot(&api);
    let input = case_test_request(
        &before,
        "invalid-proof",
        ticket::TicketCaseAction::Close {
            reason: ticket::TicketClosureReason::Completed,
        },
    );
    let app = build_router(api.clone());
    let mut unsigned = signed_case_request(&identity, Some("owner"), &reference.id, &input);
    unsigned
        .headers_mut()
        .remove(worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER);
    case_response(app.clone(), unsigned, StatusCode::UNAUTHORIZED).await;
    let attacker = RuntimeIdentityMaterial::generate("runtime-case").unwrap();
    case_response(
        app.clone(),
        signed_case_request(&attacker, Some("owner"), &reference.id, &input),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    let mut tampered = signed_case_request(&identity, Some("owner"), &reference.id, &input);
    *tampered.body_mut() = Body::from("{}");
    case_response(app.clone(), tampered, StatusCode::UNAUTHORIZED).await;
    let mut wrong_scope = signed_case_request(&identity, Some("owner"), &reference.id, &input);
    *wrong_scope.uri_mut() = format!("/workspaces/other-workspace/tickets/{}/case", reference.id)
        .parse()
        .unwrap();
    case_response(app, wrong_scope, StatusCode::UNAUTHORIZED).await;
    assert_eq!(backend.show(reference.id.into()).unwrap(), before);
    assert_eq!(case_authority_snapshot(&api), authority);
}

#[tokio::test]
async fn catalog_case_dispatch_authenticates_and_routes_the_canonical_workspace_path() {
    let dir = tempfile::tempdir().unwrap();
    let mut api = test_api(dir.path()).await;
    let identity = RuntimeIdentityMaterial::generate("runtime-case").unwrap();
    configure_runtime_request_auth(&mut api, &identity, "runtime-case");
    let backend = browser_ticket_backend(&api).unwrap();
    let reference = backend
        .create(ticket::NewTicket::new("Catalog routing boundary"))
        .unwrap();
    seed_case_assignment(
        &api,
        &reference.id,
        &RuntimeWorkerRef::new("runtime-case", "owner"),
        "catalog-work",
    );
    let before = backend.show(reference.id.clone().into()).unwrap();
    let input = case_test_request(
        &before,
        "catalog-close",
        ticket::TicketCaseAction::Close {
            reason: ticket::TicketClosureReason::Completed,
        },
    );
    let catalog = WorkspaceServerApi::new_for_test(api.config.clone(), api.store.clone());
    // Retain only this in-process fixture: do not create a real Runtime or a
    // background Orchestrator hook while testing catalog authentication/routing.
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
    let mut unsigned = signed_case_request(&identity, Some("owner"), &reference.id, &input);
    unsigned
        .headers_mut()
        .remove(worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER);
    case_response(app.clone(), unsigned, StatusCode::UNAUTHORIZED).await;
    assert_eq!(backend.show(reference.id.clone().into()).unwrap(), before);
    let response = case_response(
        app,
        signed_case_request(&identity, Some("owner"), &reference.id, &input),
        StatusCode::OK,
    )
    .await;
    assert_eq!(response["workflow_state"], "closed");
    assert_eq!(
        backend.show(reference.id.into()).unwrap().case_history[0].author,
        "worker:runtime-case/owner"
    );
}
