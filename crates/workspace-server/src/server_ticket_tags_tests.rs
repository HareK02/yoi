use super::*;

// Exercise the generated tag routes behind real Runtime authentication. Each
// operation retry gets a fresh proof; an HTTP proof itself is single-use.
async fn runtime_tag_response(
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
        serde_json::from_slice(&body).unwrap_or_else(|error| {
            panic!(
                "invalid JSON response: {error}: {}",
                String::from_utf8_lossy(&body)
            )
        })
    }
}

async fn runtime_tag_json(
    app: Router,
    identity: &RuntimeIdentityMaterial,
    worker_id: &str,
    method: &str,
    path: &str,
    input: serde_json::Value,
    expected: StatusCode,
) -> serde_json::Value {
    runtime_tag_response(
        app,
        runtime_source_request(
            identity,
            Some(worker_id),
            method,
            path,
            serde_json::to_vec(&input).unwrap(),
        ),
        expected,
    )
    .await
}

fn tag_execution_authority_snapshot(api: &WorkspaceApi) -> Vec<Vec<Vec<rusqlite::types::Value>>> {
    let conn = rusqlite::Connection::open(&api.config.database_path).unwrap();
    [
        "worker_registry",
        "worker_control_grants",
        "ticket_current_worker_assignments",
        "ticket_assignment_operations",
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
async fn runtime_tag_mutations_replay_original_receipts_and_preserve_execution_authority() {
    let dir = tempfile::tempdir().unwrap();
    let (mut api, execution) = test_api_with_recording_backend(dir.path()).await;
    let identity = RuntimeIdentityMaterial::generate("runtime-tags").unwrap();
    configure_runtime_request_auth(&mut api, &identity, "runtime-tags");
    let source = RuntimeWorkerRef::new("runtime-tags", "classifier");
    let recipient = RuntimeWorkerRef::new("runtime-tags", "recipient");
    seed_worker_source_member(&api, &source.runtime_id, &source.worker_id);
    seed_worker_source_member(&api, &recipient.runtime_id, &recipient.worker_id);
    seed_worker_control_grant(&api, &source, &recipient, "existing-control");
    let backend = browser_ticket_backend(&api).unwrap();
    let reference = backend
        .create(ticket::NewTicket::new("Classification is not a decision"))
        .unwrap();
    api.store
        .set_current_ticket_role_assignment(
            &TicketRoleAssignmentRecord {
                workspace_id: TEST_WORKSPACE_ID.into(),
                ticket_id: reference.id.clone(),
                assignment_id: "existing-assignment".into(),
                role: TicketAssignmentRole::Worker,
                principal: TicketAssignmentPrincipal::Worker {
                    runtime_id: recipient.runtime_id.clone(),
                    worker_id: recipient.worker_id.clone(),
                },
                assigned_by: "test".into(),
                assigned_at: TEST_CREATED_AT.into(),
            },
            None,
            "existing-assignment-event",
            "existing-assignment-operation",
            false,
        )
        .unwrap();
    let authority_before = tag_execution_authority_snapshot(&api);
    let ticket_before = backend.show(reference.id.clone().into()).unwrap();
    let app = build_router(api.clone());
    let tags_path = format!("/api/w/{TEST_WORKSPACE_ID}/ticket-tags");
    let membership_path = format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{}/tags", reference.id);
    let create =
        json!({"operation_key":"runtime-create", "action":{"kind":"create", "name":" ready "}});
    let tag = runtime_tag_json(
        app.clone(),
        &identity,
        &source.worker_id,
        "POST",
        &tags_path,
        create.clone(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(tag["name"], "ready");
    assert_eq!(tag["workspace_id"], TEST_WORKSPACE_ID);
    let attach =
        json!({"operation_key":"runtime-attach", "expected_tag_ids":[], "tag_ids":[tag["id"]]});
    let attached = runtime_tag_json(
        app.clone(),
        &identity,
        &source.worker_id,
        "PUT",
        &membership_path,
        attach.clone(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(attached, json!([tag.clone()]));
    assert_eq!(
        runtime_tag_json(
            app.clone(),
            &identity,
            &source.worker_id,
            "PUT",
            &membership_path,
            attach.clone(),
            StatusCode::OK
        )
        .await,
        attached
    );
    let rename = json!({"operation_key":"runtime-rename", "action":{"kind":"rename", "tag_id":tag["id"], "expected_last_operation_key":"runtime-create", "name":"Ready now"}});
    let renamed = runtime_tag_json(
        app.clone(),
        &identity,
        &source.worker_id,
        "POST",
        &tags_path,
        rename.clone(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(renamed["id"], tag["id"]);
    assert_eq!(renamed["name"], "Ready now");
    assert_eq!(
        serde_json::to_value(&backend.show(reference.id.clone().into()).unwrap().tags).unwrap(),
        json!([renamed.clone()])
    );
    let delete = json!({"operation_key":"runtime-delete", "action":{"kind":"delete", "tag_id":tag["id"], "expected_last_operation_key":"runtime-rename"}});
    let deleted = runtime_tag_json(
        app.clone(),
        &identity,
        &source.worker_id,
        "POST",
        &tags_path,
        delete.clone(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(deleted["deleted"], true);
    assert!(
        backend
            .show(reference.id.clone().into())
            .unwrap()
            .tags
            .is_empty()
    );
    // Receipts return their original snapshots, even after rename/delete changed
    // live state. Retries must not recreate membership or duplicate audit events.
    for (input, expected) in [(create, tag), (rename, renamed), (delete, deleted)] {
        assert_eq!(
            runtime_tag_json(
                app.clone(),
                &identity,
                &source.worker_id,
                "POST",
                &tags_path,
                input,
                StatusCode::OK
            )
            .await,
            expected
        );
    }
    assert_eq!(
        runtime_tag_json(
            app.clone(),
            &identity,
            &source.worker_id,
            "PUT",
            &membership_path,
            attach,
            StatusCode::OK
        )
        .await,
        attached
    );
    runtime_tag_json(app.clone(), &identity, &source.worker_id, "PUT", &membership_path,
        json!({"operation_key":"runtime-stale-set", "expected_tag_ids":[attached[0]["id"]], "tag_ids":[]}), StatusCode::CONFLICT).await;
    let empty = runtime_tag_json(
        app,
        &identity,
        &source.worker_id,
        "PUT",
        &membership_path,
        json!({"operation_key":"runtime-empty-set", "expected_tag_ids":[], "tag_ids":[]}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(empty, json!([]));
    let ticket_after = backend.show(reference.id.into()).unwrap();
    assert_eq!(
        ticket::ticket_content_digest(&ticket_before),
        ticket::ticket_content_digest(&ticket_after)
    );
    assert_eq!(
        ticket_before.meta.workflow_state,
        ticket_after.meta.workflow_state
    );
    assert!(ticket_after.tags.is_empty());
    assert_eq!(tag_execution_authority_snapshot(&api), authority_before);
    assert!(
        execution.protocol_methods().is_empty(),
        "classification dispatched execution/routing"
    );
    let tag_events: Vec<_> = ticket_after
        .events
        .iter()
        .filter(|event| event.kind == ticket::TicketEventKind::TagsChanged)
        .collect();
    assert_eq!(
        tag_events.len(),
        2,
        "attach/delete retries duplicated membership audit"
    );
    let conn = rusqlite::Connection::open(&api.config.database_path).unwrap();
    let mut statement = conn.prepare("SELECT operation_key,source_context_json FROM workspace_tag_operations WHERE workspace_id=?1 ORDER BY operation_key").unwrap();
    let receipts = statement
        .query_map([TEST_WORKSPACE_ID], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(receipts.len(), 5);
    for (key, context) in receipts {
        let context: serde_json::Value = serde_json::from_str(&context).unwrap();
        assert_eq!(
            context["source_runtime_id"], source.runtime_id,
            "receipt {key}: {context}"
        );
        assert_eq!(
            context["source_worker_id"], source.worker_id,
            "receipt {key}: {context}"
        );
        assert!(
            context.get("source_account_id").is_none(),
            "signed source became a human actor: {context}"
        );
    }
    for event in tag_events {
        assert_eq!(
            event.attributes.get("source_runtime_id"),
            Some(&source.runtime_id)
        );
        assert_eq!(
            event.attributes.get("source_worker_id"),
            Some(&source.worker_id)
        );
    }
}

#[tokio::test]
async fn runtime_tag_receipt_replay_is_bound_to_verified_worker_not_transport_proof() {
    let dir = tempfile::tempdir().unwrap();
    let mut api = test_api(dir.path()).await;
    let identity = RuntimeIdentityMaterial::generate("runtime-tags").unwrap();
    configure_runtime_request_auth(&mut api, &identity, "runtime-tags");
    for worker in ["classifier", "other"] {
        seed_worker_source_member(&api, "runtime-tags", worker);
    }
    let app = build_router(api.clone());
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/ticket-tags");
    let input =
        json!({"operation_key":"worker-receipt", "action":{"kind":"create", "name":"ready"}});
    let request = runtime_source_request(
        &identity,
        Some("classifier"),
        "POST",
        &path,
        serde_json::to_vec(&input).unwrap(),
    );
    let proof =
        request.headers()[worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER].clone();
    let tag = runtime_tag_response(app.clone(), request, StatusCode::OK).await;
    runtime_tag_response(
        app.clone(),
        Request::builder()
            .method("POST")
            .uri(&path)
            .header(CONTENT_TYPE, "application/json")
            .header(
                worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER,
                proof,
            )
            .body(Body::from(serde_json::to_vec(&input).unwrap()))
            .unwrap(),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    assert_eq!(
        runtime_tag_json(
            app.clone(),
            &identity,
            "classifier",
            "POST",
            &path,
            input.clone(),
            StatusCode::OK
        )
        .await,
        tag
    );
    runtime_tag_json(
        app.clone(),
        &identity,
        "classifier",
        "POST",
        &path,
        json!({"operation_key":"worker-receipt", "action":{"kind":"create", "name":"changed"}}),
        StatusCode::CONFLICT,
    )
    .await;
    runtime_tag_json(
        app,
        &identity,
        "other",
        "POST",
        &path,
        input,
        StatusCode::CONFLICT,
    )
    .await;
    assert_eq!(
        browser_ticket_backend(&api)
            .unwrap()
            .list_workspace_tags(ticket::TagListRequest::default())
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn runtime_tag_mutations_reject_invalid_proofs_and_changed_binding_authority() {
    let dir = tempfile::tempdir().unwrap();
    let (mut api, execution) = test_api_with_recording_backend(dir.path()).await;
    let identity = RuntimeIdentityMaterial::generate("runtime-tags").unwrap();
    configure_runtime_request_auth(&mut api, &identity, "runtime-tags");
    seed_worker_source_member(&api, "runtime-tags", "classifier");
    let backend = browser_ticket_backend(&api).unwrap();
    let ticket = backend
        .create(ticket::NewTicket::new("Rejected classification"))
        .unwrap();
    let ticket_before = backend.show(ticket.id.clone().into()).unwrap();
    let authority_before = tag_execution_authority_snapshot(&api);
    let app = build_router(api.clone());
    let tags_path = format!("/api/w/{TEST_WORKSPACE_ID}/ticket-tags");
    let membership_path = format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{}/tags", ticket.id);
    let attacker = RuntimeIdentityMaterial::generate("runtime-tags").unwrap();
    // Both mutation routes must reject before writing tags, memberships, receipts
    // or audit events. There is no persisted verification status in this fixture.
    for (method, path, input) in [
        (
            "POST",
            tags_path.as_str(),
            json!({"operation_key":"denied-create", "action":{"kind":"create", "name":"denied"}}),
        ),
        (
            "PUT",
            membership_path.as_str(),
            json!({"operation_key":"denied-set", "expected_tag_ids":[], "tag_ids":[]}),
        ),
    ] {
        let body = serde_json::to_vec(&input).unwrap();
        let unsigned = Request::builder()
            .method(method)
            .uri(path)
            .header(CONTENT_TYPE, "application/json")
            .header("x-yoi-runtime-id", "runtime-tags")
            .header("x-yoi-worker-id", "classifier")
            .body(Body::from(body.clone()))
            .unwrap();
        runtime_tag_response(app.clone(), unsigned, StatusCode::UNAUTHORIZED).await;
        runtime_tag_json(
            app.clone(),
            &attacker,
            "classifier",
            method,
            path,
            input.clone(),
            StatusCode::UNAUTHORIZED,
        )
        .await;
        runtime_tag_json(
            app.clone(),
            &identity,
            "not-a-member",
            method,
            path,
            input.clone(),
            StatusCode::UNAUTHORIZED,
        )
        .await;
        let proof = worker_runtime::auth::RuntimeRequestSourceSigner::from_identity(&identity)
            .issue(
                "server-test",
                "another-workspace",
                Some("classifier"),
                worker_runtime::auth::WORKSPACE_REQUEST_PERMISSION,
                method,
                path,
                &body,
                i64::try_from(worker_runtime::auth::unix_now_seconds()).unwrap(),
                30,
            )
            .unwrap();
        runtime_tag_response(
            app.clone(),
            Request::builder()
                .method(method)
                .uri(path)
                .header(CONTENT_TYPE, "application/json")
                .header(
                    worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER,
                    proof,
                )
                .body(Body::from(body.clone()))
                .unwrap(),
            StatusCode::UNAUTHORIZED,
        )
        .await;
        let mut tampered =
            runtime_source_request(&identity, Some("classifier"), method, path, body);
        *tampered.body_mut() = Body::from(format!("{} ", input));
        runtime_tag_response(app.clone(), tampered, StatusCode::UNAUTHORIZED).await;
    }
    let store = SqliteWorkspaceStore::open(&api.config.database_path).unwrap();
    let original = api
        .store
        .get_workspace_runtime_binding(TEST_WORKSPACE_ID, "runtime-tags")
        .await
        .unwrap()
        .unwrap();
    let mut replacement = original.clone();
    replacement.public_key = attacker.public_key.clone();
    replacement.public_key_fingerprint = String::new();
    store
        .upsert_workspace_runtime_binding(replacement, true)
        .unwrap();
    for (method, path, input) in [
        (
            "POST",
            tags_path.as_str(),
            json!({"operation_key":"stale-key-create", "action":{"kind":"create", "name":"stale"}}),
        ),
        (
            "PUT",
            membership_path.as_str(),
            json!({"operation_key":"stale-key-set", "expected_tag_ids":[], "tag_ids":[]}),
        ),
    ] {
        runtime_tag_json(
            app.clone(),
            &identity,
            "classifier",
            method,
            path,
            input,
            StatusCode::UNAUTHORIZED,
        )
        .await;
    }
    store
        .upsert_workspace_runtime_binding(original, true)
        .unwrap();
    store
        .revoke_workspace_runtime_binding(TEST_WORKSPACE_ID, "runtime-tags", TEST_CREATED_AT)
        .unwrap();
    for (method, path, input) in [
        (
            "POST",
            tags_path.as_str(),
            json!({"operation_key":"revoked-create", "action":{"kind":"create", "name":"revoked"}}),
        ),
        (
            "PUT",
            membership_path.as_str(),
            json!({"operation_key":"revoked-set", "expected_tag_ids":[], "tag_ids":[]}),
        ),
    ] {
        runtime_tag_json(
            app.clone(),
            &identity,
            "classifier",
            method,
            path,
            input,
            StatusCode::UNAUTHORIZED,
        )
        .await;
    }
    assert!(
        backend
            .list_workspace_tags(ticket::TagListRequest::default())
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        serde_json::to_value(backend.show(ticket.id.into()).unwrap()).unwrap(),
        serde_json::to_value(ticket_before).unwrap()
    );
    assert_eq!(tag_execution_authority_snapshot(&api), authority_before);
    assert!(execution.protocol_methods().is_empty());
    let conn = rusqlite::Connection::open(&api.config.database_path).unwrap();
    let receipts: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM workspace_tag_operations WHERE workspace_id=?1",
            [TEST_WORKSPACE_ID],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(receipts, 0);
}

#[tokio::test]
async fn ticket_tags_round_trip_filter_and_preserve_execution_and_decision_projections() {
    let dir = tempfile::tempdir().unwrap();
    let (api, execution) = test_api_with_recording_backend(dir.path()).await;
    let backend = browser_ticket_backend(&api).unwrap();
    let reference = backend.create(ticket::NewTicket::new("Classification only")).unwrap();
    let other = backend.create(ticket::NewTicket::new("Other ticket")).unwrap();
    let app = build_inner_router(api.clone());
    let tags_path = format!("/api/w/{TEST_WORKSPACE_ID}/ticket-tags");
    let ticket_path = format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{}", reference.id);
    let before = get_json(app.clone(), &ticket_path).await;
    let ready = post_json(app.clone(), &tags_path, json!({
        "operation_key":"create-ready", "action":{"kind":"create", "name":" ready "}
    })).await;
    let bug = post_json(app.clone(), &tags_path, json!({
        "operation_key":"create-bug", "action":{"kind":"create", "name":"Bug"}
    })).await;
    let ready_id = ready["id"].as_str().unwrap();
    let bug_id = bug["id"].as_str().unwrap();
    let assignment = json!({"operation_key":"classify", "expected_tag_ids":[], "tag_ids":[ready_id,bug_id]});
    let attached = request_json(app.clone(), "PUT", &format!("{ticket_path}/tags"), Some(assignment.clone()), StatusCode::OK).await;
    assert_eq!(attached.as_array().unwrap().len(), 2);
    let replay = request_json(app.clone(), "PUT", &format!("{ticket_path}/tags"), Some(assignment), StatusCode::OK).await;
    assert_eq!(attached, replay);
    let after = get_json(app.clone(), &ticket_path).await;
    for field in ["state", "resolution", "content_digest", "current_worker", "assignments", "action_eligibility", "targets", "evidence", "merge_requests", "queued_by", "queued_at"] {
        assert_eq!(before[field], after[field], "classification changed {field}");
    }
    assert_eq!(after["tags"].as_array().unwrap().len(), 2);
    assert!(execution.protocol_methods().is_empty());
    let list = get_json(app.clone(), &format!("/api/w/{TEST_WORKSPACE_ID}/tickets")).await;
    let item = list["items"].as_array().unwrap().iter().find(|item| item["id"] == reference.id).unwrap();
    assert_eq!(item["tags"].as_array().unwrap().len(), 2);
    let query_path = format!("/api/w/{TEST_WORKSPACE_ID}/tickets/query");
    for (filter, count) in [
        (json!({"tag_ids":[ready_id,bug_id]}), 1),
        (json!({"tag_ids_any":[ready_id,"absent"]}), 1),
        (json!({"tag_ids_none":[ready_id]}), 1),
        (json!({"tag_ids":[ready_id,"absent"]}), 0),
    ] {
        let result = post_json(app.clone(), &query_path, filter.clone()).await;
        assert_eq!(result["items"].as_array().unwrap().len(), count, "filter {filter}");
        if count == 1 && filter.get("tag_ids_none").is_none() {
            assert_eq!(result["items"][0]["tags"].as_array().unwrap().len(), 2);
        } else if filter.get("tag_ids_none").is_some() {
            assert_eq!(result["items"][0]["id"], other.id);
        }
    }
    let renamed = post_json(app.clone(), &tags_path, json!({
        "operation_key":"rename", "action":{"kind":"rename", "tag_id":bug_id,
        "expected_last_operation_key":"create-bug", "name":"Defect"}
    })).await;
    assert_eq!(renamed["id"], bug["id"]);
    let shown = get_json(app.clone(), &ticket_path).await;
    assert!(shown["tags"].as_array().unwrap().iter().any(|tag| tag["name"] == "Defect"));
    request_json(app.clone(), "POST", &tags_path, Some(json!({
        "operation_key":"stale-rename", "action":{"kind":"rename", "tag_id":bug_id,
        "expected_last_operation_key":"create-bug", "name":"Wrong"}
    })), StatusCode::CONFLICT).await;
    post_json(app.clone(), &tags_path, json!({
        "operation_key":"delete", "action":{"kind":"delete", "tag_id":bug_id,
        "expected_last_operation_key":"rename"}
    })).await;
    let deleted = get_json(app.clone(), &ticket_path).await;
    assert_eq!(deleted["tags"].as_array().unwrap().len(), 1);
    request_json(app.clone(), "PUT", &format!("{ticket_path}/tags"), Some(json!({
        "operation_key":"stale-set", "expected_tag_ids":[ready_id,bug_id], "tag_ids":[]
    })), StatusCode::CONFLICT).await;
    let untagged = request_json(app.clone(), "PUT", &format!("{ticket_path}/tags"), Some(json!({
        "operation_key":"detach", "expected_tag_ids":[ready_id], "tag_ids":[]
    })), StatusCode::OK).await;
    assert_eq!(untagged, json!([]));
    let final_detail = get_json(app, &ticket_path).await;
    assert_eq!(final_detail["state"], before["state"]);
    assert_eq!(final_detail["content_digest"], before["content_digest"]);
}

#[tokio::test]
async fn ticket_tag_routes_require_workspace_authority_and_reject_forged_worker_source() {
    let dir = tempfile::tempdir().unwrap();
    let api = test_api(dir.path()).await;
    let owner_token = seed_test_api_token(api.store.as_ref(), TEST_WORKSPACE_ID);
    let collaborator_token = seed_test_api_token(api.store.as_ref(), "tags-collaborator");
    let app = build_router(api.clone());
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/ticket-tags");
    let input = json!({"operation_key":"authorized", "action":{"kind":"create", "name":"ready"}});
    request_json(app.clone(), "POST", &path, Some(input.clone()), StatusCode::UNAUTHORIZED).await;

    let tag = request_json_authenticated(app.clone(), "POST", &path, Some(input), &owner_token, StatusCode::OK).await;
    assert_eq!(tag["workspace_id"], TEST_WORKSPACE_ID);
    // Classification follows shared Ticket Workspace write authority, not the
    // owner-only repository/secrets-management policy. A distinct authenticated
    // actor may manage tags but cannot replay another actor's receipt.
    request_json_authenticated(app.clone(), "POST", &path, Some(json!({"operation_key":"authorized", "action":{"kind":"create", "name":"ready"}})), &collaborator_token, StatusCode::CONFLICT).await;
    request_json_authenticated(app.clone(), "POST", "/api/w/other-workspace/ticket-tags", Some(json!({"operation_key":"foreign", "action":{"kind":"create", "name":"foreign"}})), &collaborator_token, StatusCode::NOT_FOUND).await;
    let response = app.oneshot(Request::builder().method("POST").uri(&path)
        .header(axum::http::header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .header("x-yoi-runtime-id", "untrusted-runtime")
        .header("x-yoi-worker-id", "untrusted-worker")
        .header(CONTENT_TYPE,"application/json")
        .body(Body::from(json!({"operation_key":"forged", "action":{"kind":"create", "name":"forged"}}).to_string())).unwrap()).await.unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST, "{}", String::from_utf8_lossy(&body));
    let tags = browser_ticket_backend(&api).unwrap().list_workspace_tags(ticket::TagListRequest::default()).unwrap();
    assert_eq!(tags.len(), 1);
}

#[tokio::test]
async fn tag_query_filters_precede_bounded_candidate_scan_and_bind_cursors() {
    let dir = tempfile::tempdir().unwrap();
    let api = test_api(dir.path()).await;
    let backend = browser_ticket_backend(&api).unwrap();
    for i in 0..5 { backend.create(ticket::NewTicket::new(format!("a unrelated {i}"))).unwrap(); }
    let tag = backend.mutate_workspace_tag(ticket::WorkspaceTagMutation {
        operation_key: "create".into(), action: ticket::WorkspaceTagAction::Create { name: "bug".into() },
    }).unwrap();
    for i in 0..3 {
        let id = backend.create(ticket::NewTicket::new(format!("z tagged {i}"))).unwrap().id;
        backend.set_ticket_tags(id.into(), ticket::TicketTagSetRequest {
            expected_tag_ids: vec![], tag_ids: vec![tag.id.clone()], operation_key: format!("set-{i}"),
        }).unwrap();
    }
    let app = build_inner_router(api);
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/tickets/query");
    let mut input = json!({"tag_ids":[tag.id], "sort":"title", "limit":1});
    for i in 0..3 {
        let page = post_json(app.clone(), &path, input.clone()).await;
        assert_eq!(page["items"].as_array().unwrap().len(),1);
        assert_eq!(page["items"][0]["title"], format!("z tagged {i}"));
        if i < 2 {
            let cursor = page["page"]["next_cursor"].clone();
            request_json(app.clone(), "POST", &path, Some(json!({"tag_ids_any":[tag.id], "sort":"title", "limit":1, "cursor":cursor})), StatusCode::NOT_FOUND).await;
            input["cursor"] = cursor;
        } else { assert_eq!(page["page"]["has_more"], false); }
    }
}
