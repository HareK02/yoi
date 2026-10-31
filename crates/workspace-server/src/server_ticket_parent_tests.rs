// Signed router + real Ticket DB + Host permissions. Runtime execution alone is
// substituted by the existing shared fixture; no live Workspace data is used.
use super::*;

fn parent_request(
    expected: serde_json::Value,
    parent: Option<&str>,
    key: &str,
) -> serde_json::Value {
    json!({"expected":expected,"parent":parent,"operation_key":key})
}

#[tokio::test]
async fn signed_parent_routes_replay_reparent_query_and_preserve_independent_authorities() {
    let dir = tempfile::tempdir().unwrap();
    let (mut api, _execution) = test_api_with_recording_backend(dir.path()).await;
    let identity = RuntimeIdentityMaterial::generate("runtime-parents").unwrap();
    configure_runtime_request_auth(&mut api, &identity, "runtime-parents");
    seed_worker_source_member(&api, "runtime-parents", "linker");
    let backend = browser_ticket_backend(&api).unwrap();
    let a = backend.create(ticket::NewTicket::new("A")).unwrap().id;
    let b = backend.create(ticket::NewTicket::new("B")).unwrap().id;
    let c = backend.create(ticket::NewTicket::new("C")).unwrap().id;
    let d = backend.create(ticket::NewTicket::new("D")).unwrap().id;
    backend
        .add_ticket_relation(
            b.clone().into(),
            ticket::NewTicketRelation {
                kind: ticket::TicketRelationKind::Related,
                target: d.clone(),
                note: None,
                author: None,
            },
        )
        .unwrap();
    let original = backend.show(b.clone().into()).unwrap();
    let authority_before = tag_execution_authority_snapshot(&api);
    let app = build_router(api.clone());
    let parent_path = |id: &str| format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{id}/parent");
    let absent = json!({"parent_ticket_id":null,"last_operation_key":null});
    let first_request = parent_request(absent.clone(), Some(&a), "b-a");
    let first = runtime_tag_json(
        app.clone(),
        &identity,
        "linker",
        "PUT",
        &parent_path(&b),
        first_request.clone(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(first["observation"]["parent_ticket_id"], a);
    assert!(
        first["parent"]["resource_key"]
            .as_str()
            .unwrap()
            .starts_with("T-")
    );
    runtime_tag_json(
        app.clone(),
        &identity,
        "linker",
        "PUT",
        &parent_path(&c),
        parent_request(absent.clone(), Some(&b), "c-b"),
        StatusCode::OK,
    )
    .await;
    runtime_tag_json(
        app.clone(),
        &identity,
        "linker",
        "PUT",
        &parent_path(&a),
        parent_request(absent.clone(), Some(&c), "cycle"),
        StatusCode::CONFLICT,
    )
    .await;
    assert_eq!(
        backend
            .get_ticket_parent(a.clone().into())
            .unwrap()
            .observation
            .parent_ticket_id,
        None
    );
    let moved = runtime_tag_json(
        app.clone(),
        &identity,
        "linker",
        "PUT",
        &parent_path(&b),
        parent_request(first["observation"].clone(), Some(&d), "b-d"),
        StatusCode::OK,
    )
    .await;
    runtime_tag_json(
        app.clone(),
        &identity,
        "linker",
        "PUT",
        &parent_path(&b),
        parent_request(first["observation"].clone(), None, "stale"),
        StatusCode::CONFLICT,
    )
    .await;
    assert_eq!(
        runtime_tag_json(
            app.clone(),
            &identity,
            "linker",
            "PUT",
            &parent_path(&b),
            first_request,
            StatusCode::OK
        )
        .await,
        first
    );
    let reread = runtime_tag_json(
        app.clone(),
        &identity,
        "linker",
        "GET",
        &parent_path(&b),
        json!(null),
        StatusCode::OK,
    )
    .await;
    assert_eq!(reread, moved);
    let query_path = format!("/api/w/{TEST_WORKSPACE_ID}/tickets/query");
    let query = runtime_tag_json(
        app.clone(),
        &identity,
        "linker",
        "POST",
        &query_path,
        json!({"parent":{"kind":"parent","ticket":d}}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(query["items"].as_array().unwrap().len(), 1);
    assert_eq!(query["items"][0]["id"], b);
    let roots = runtime_tag_json(
        app.clone(),
        &identity,
        "linker",
        "POST",
        &query_path,
        json!({"parent":{"kind":"none"}}),
        StatusCode::OK,
    )
    .await;
    assert!(
        roots["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["id"] != b && item["id"] != c)
    );
    runtime_tag_json(
        app.clone(),
        &identity,
        "linker",
        "PUT",
        &parent_path(&b),
        parent_request(moved["observation"].clone(), None, "unlink"),
        StatusCode::OK,
    )
    .await;
    let after = backend.show(b.clone().into()).unwrap();
    assert_eq!(after.relations, original.relations);
    assert_eq!(after.meta.workflow_state, original.meta.workflow_state);
    assert_eq!(after.meta.targets, original.meta.targets);
    assert_eq!(after.current_closure, original.current_closure);
    assert_eq!(tag_execution_authority_snapshot(&api), authority_before);
    let conn = rusqlite::Connection::open(&api.config.database_path).unwrap();
    let (events,outbox):(i64,i64) = conn.query_row("SELECT (SELECT COUNT(*) FROM typed_ticket_events WHERE workspace_id=?1 AND ticket_id=?2 AND kind='parent_changed'), (SELECT COUNT(*) FROM typed_ticket_service_outbox o JOIN typed_ticket_events e ON e.workspace_id=o.workspace_id AND e.ticket_id=o.ticket_id AND e.event_ref=o.domain_event_ref WHERE e.workspace_id=?1 AND e.ticket_id=?2 AND e.kind='parent_changed')",rusqlite::params![TEST_WORKSPACE_ID,b],|row| Ok((row.get(0)?,row.get(1)?))).unwrap();
    assert_eq!(events, 3);
    assert_eq!(outbox, events);
}

#[tokio::test]
async fn signed_parent_routes_reject_missing_wire_and_scoped_cursor_without_partial_writes() {
    let dir = tempfile::tempdir().unwrap();
    let (mut api, _execution) = test_api_with_recording_backend(dir.path()).await;
    let identity = RuntimeIdentityMaterial::generate("runtime-parents").unwrap();
    configure_runtime_request_auth(&mut api, &identity, "runtime-parents");
    seed_worker_source_member(&api, "runtime-parents", "linker");
    let backend = browser_ticket_backend(&api).unwrap();
    let parent = backend.create(ticket::NewTicket::new("Parent")).unwrap().id;
    let other = backend
        .create(ticket::NewTicket::new("Other parent"))
        .unwrap()
        .id;
    for i in 0..2 {
        let child = backend
            .create(ticket::NewTicket::new(format!("Child {i}")))
            .unwrap()
            .id;
        backend
            .set_ticket_parent(
                child.into(),
                ticket::TicketParentSetRequest {
                    expected: Default::default(),
                    parent: Some(parent.clone()),
                    operation_key: format!("link-{i}"),
                },
            )
            .unwrap();
    }
    let before = backend.show(parent.clone().into()).unwrap();
    let app = build_router(api.clone());
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{parent}/parent");
    for body in [
        json!({"expected":{},"parent":null,"operation_key":"bad"}),
        json!({"expected":{"parent_ticket_id":null,"last_operation_key":null},"operation_key":"bad"}),
    ] {
        let response = app
            .clone()
            .oneshot(runtime_source_request(
                &identity,
                Some("linker"),
                "PUT",
                &path,
                serde_json::to_vec(&body).unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let rejection = to_bytes(response.into_body(), 16_384).await.unwrap();
        assert!(String::from_utf8_lossy(&rejection).contains("missing field"));
    }
    let page_path = format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{parent}/children?limit=1");
    let page = runtime_tag_json(
        app.clone(),
        &identity,
        "linker",
        "GET",
        &page_path,
        json!(null),
        StatusCode::OK,
    )
    .await;
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["has_more"], true);
    let encoded: String =
        url::form_urlencoded::byte_serialize(page["next_cursor"].as_str().unwrap().as_bytes())
            .collect();
    let wrong_scope =
        format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{other}/children?limit=1&cursor={encoded}");
    runtime_tag_json(
        app.clone(),
        &identity,
        "linker",
        "GET",
        &wrong_scope,
        json!(null),
        StatusCode::CONFLICT,
    )
    .await;
    assert_eq!(backend.show(parent.clone().into()).unwrap(), before);
    let unsigned = Request::builder()
        .method("PUT")
        .uri(&path)
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            parent_request(
                json!({"parent_ticket_id":null,"last_operation_key":null}),
                Some(&other),
                "unsigned",
            )
            .to_string(),
        ))
        .unwrap();
    runtime_tag_response(app, unsigned, StatusCode::UNAUTHORIZED).await;
    assert_eq!(backend.show(parent.into()).unwrap(), before);
}

#[tokio::test]
async fn parent_reparent_requires_existing_record_and_remove_grants_on_actual_endpoints() {
    use server_api::authorization::{
        PermissionResource, ScopedPermission, SubjectPermissionsUpdate,
    };
    let workspace = tempfile::tempdir().unwrap();
    let mut api = test_api(workspace.path()).await;
    register_test_runtime(&api, WorkdirlessFixtureRuntime::RUNTIME_ID).await;
    api.runtime
        .register_or_replace(WorkdirlessFixtureRuntime::default());
    let store = open_subjektiv_store(&api).unwrap();
    let subject = store
        .create_subject_with_behavior(
            crate::subjektiv::SubjectRole::new("parent-linker").unwrap(),
            "Link work decomposition".into(),
        )
        .unwrap();
    let Json(created) = create_workspace_worker(
        State(api.clone()),
        HeaderMap::new(),
        Json(CreateWorkspaceWorkerRequest {
            runtime_id: WorkdirlessFixtureRuntime::RUNTIME_ID.into(),
            display_name: "Parent linker".into(),
            singleton_key: None,
            profile: Some("builtin:companion".into()),
            ticket_assignment: None,
            initial_submit: vec![],
            workdir_attachments: vec![],
            feature_connections: server_api::WorkspaceWorkerFeatureConnectionsRequest {
                subjektiv: Some(server_api::SubjektivWorkerConnectionRequest {
                    subject_id: subject.id.clone(),
                }),
            },
            control_operation_id: None,
            execution_grant: None,
        }),
    )
    .await
    .unwrap();
    let identity = RuntimeIdentityMaterial::generate(&created.runtime_id).unwrap();
    configure_runtime_request_auth(&mut api, &identity, &created.runtime_id);
    let backend = browser_ticket_backend(&api).unwrap();
    let a = backend.create(ticket::NewTicket::new("A")).unwrap().id;
    let b = backend.create(ticket::NewTicket::new("B")).unwrap().id;
    let child = backend.create(ticket::NewTicket::new("Child")).unwrap().id;
    let grant = |operation: &str, id: &str| ScopedPermission {
        operation: operation.into(),
        resource: PermissionResource::Ticket {
            workspace_id: TEST_WORKSPACE_ID.into(),
            ticket_id: id.into(),
        },
    };
    let set_raw = |permissions: Vec<ScopedPermission>| {
        let previous = store.subject_permissions(&subject.id).unwrap();
        store
            .set_subject_permissions(
                &subject.id,
                &SubjectPermissionsUpdate {
                    operation_id: Uuid::now_v7().to_string(),
                    expected_change_id: previous.map(|policy| policy.change_id),
                    execution: permissions,
                    delegation: vec![],
                },
            )
            .unwrap();
    };
    let set = |mut permissions: Vec<ScopedPermission>| {
        permissions.extend([
            grant("ticket.relation.read", &child),
            grant("ticket.relation.read", &a),
            grant("ticket.relation.read", &b),
        ]);
        set_raw(permissions);
    };
    let app = build_router(api.clone());
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{child}/parent");
    let absent = json!({"parent_ticket_id":null,"last_operation_key":null});
    set(vec![grant("ticket.relation.record", &child)]);
    let first_request = parent_request(absent, Some(&a), "first");
    // A write grant cannot be used to disclose metadata of the new parent.
    set_raw(vec![
        grant("ticket.relation.record", &child),
        grant("ticket.relation.record", &a),
    ]);
    runtime_tag_json(
        app.clone(),
        &identity,
        &created.worker_id,
        "PUT",
        &path,
        first_request.clone(),
        StatusCode::FORBIDDEN,
    )
    .await;
    assert_eq!(
        backend
            .get_ticket_parent(child.clone().into())
            .unwrap()
            .observation,
        Default::default()
    );
    set(vec![grant("ticket.relation.record", &child)]);
    runtime_tag_json(
        app.clone(),
        &identity,
        &created.worker_id,
        "PUT",
        &path,
        first_request.clone(),
        StatusCode::FORBIDDEN,
    )
    .await;
    assert_eq!(
        backend
            .get_ticket_parent(child.clone().into())
            .unwrap()
            .observation
            .parent_ticket_id,
        None
    );
    set(vec![
        grant("ticket.relation.record", &child),
        grant("ticket.relation.record", &a),
        grant("ticket.relation.record", &b),
    ]);
    let first = runtime_tag_json(
        app.clone(),
        &identity,
        &created.worker_id,
        "PUT",
        &path,
        first_request,
        StatusCode::OK,
    )
    .await;

    // Direct signed reads must enforce both ends, independently of Tool-side checks.
    set_raw(vec![grant("ticket.relation.read", &child)]);
    let summary_path = format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{child}/link-summary");
    let metadata = runtime_tag_json(
        app.clone(),
        &identity,
        &created.worker_id,
        "GET",
        &summary_path,
        json!(null),
        StatusCode::OK,
    )
    .await;
    assert_eq!(metadata["id"], child);
    assert_eq!(metadata.as_object().unwrap().len(), 5);
    for protected in [
        "body",
        "parent",
        "parent_link",
        "targets",
        "assignment_tracking",
    ] {
        assert!(
            metadata.get(protected).is_none(),
            "metadata included {protected}"
        );
    }
    let unauthorized_summary = format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{a}/link-summary");
    runtime_tag_json(
        app.clone(),
        &identity,
        &created.worker_id,
        "GET",
        &unauthorized_summary,
        json!(null),
        StatusCode::FORBIDDEN,
    )
    .await;

    runtime_tag_json(
        app.clone(),
        &identity,
        &created.worker_id,
        "GET",
        &path,
        json!(null),
        StatusCode::FORBIDDEN,
    )
    .await;
    let detail_path = format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{child}");
    runtime_tag_json(
        app.clone(),
        &identity,
        &created.worker_id,
        "GET",
        &detail_path,
        json!(null),
        StatusCode::FORBIDDEN,
    )
    .await;
    set_raw(vec![grant("ticket.relation.read", &a)]);
    let children_path = format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{a}/children?limit=1");
    runtime_tag_json(
        app.clone(),
        &identity,
        &created.worker_id,
        "GET",
        &children_path,
        json!(null),
        StatusCode::FORBIDDEN,
    )
    .await;
    assert_eq!(
        backend
            .get_ticket_parent(child.clone().into())
            .unwrap()
            .observation,
        serde_json::from_value(first["observation"].clone()).unwrap()
    );
    set(vec![
        grant("ticket.relation.record", &child),
        grant("ticket.relation.record", &a),
        grant("ticket.relation.record", &b),
    ]);
    let reparent = parent_request(first["observation"].clone(), Some(&b), "reparent");
    runtime_tag_json(
        app.clone(),
        &identity,
        &created.worker_id,
        "PUT",
        &path,
        reparent.clone(),
        StatusCode::FORBIDDEN,
    )
    .await;
    set(vec![
        grant("ticket.relation.record", &child),
        grant("ticket.relation.record", &b),
        grant("ticket.relation.remove", &child),
        grant("ticket.relation.remove", &a),
    ]);
    let moved = runtime_tag_json(
        app.clone(),
        &identity,
        &created.worker_id,
        "PUT",
        &path,
        reparent,
        StatusCode::OK,
    )
    .await;
    let unlink = parent_request(moved["observation"].clone(), None, "unlink");
    runtime_tag_json(
        app.clone(),
        &identity,
        &created.worker_id,
        "PUT",
        &path,
        unlink.clone(),
        StatusCode::FORBIDDEN,
    )
    .await;
    set(vec![
        grant("ticket.relation.remove", &child),
        grant("ticket.relation.remove", &b),
    ]);
    runtime_tag_json(
        app,
        &identity,
        &created.worker_id,
        "PUT",
        &path,
        unlink,
        StatusCode::OK,
    )
    .await;
}

// Service-bound projection only. Router tests above cover signed mutation authority;
// this eligibility is presentation metadata, not a retained permission or write grant.
#[tokio::test]
async fn parent_editor_eligibility_requires_current_request_owner_not_ticket_contents() {
    let dir = tempfile::tempdir().unwrap();
    let api = test_api(dir.path()).await;
    let backend = browser_ticket_backend(&api).unwrap();
    let child = backend.create(ticket::NewTicket::new("Child")).unwrap().id;
    assert!(
        !browser_ticket_detail(&api, &child)
            .unwrap()
            .0
            .action_eligibility
            .can_set_parent
    );
    let service = ServerApiContractService::Workspace(api);
    let context = |actor, runtime_source| server_api::ServerRequestContext {
        actor,
        runtime_source,
        worker_source: None,
        origin: None,
        transport_headers: vec![],
    };
    let mut other = test_owner_actor();
    other.account_id = "other-account".into();
    for (request, expected) in [
        (context(None, None), false),
        (context(Some(other), None), false),
        (context(Some(test_owner_actor()), None), true),
    ] {
        let detail = server_api::ServerApi::ticket_get(
            &service,
            request,
            TEST_WORKSPACE_ID.into(),
            child.clone(),
        )
        .await
        .unwrap();
        assert_eq!(detail.action_eligibility.can_set_parent, expected);
        assert_eq!(detail.parent_link.observation, Default::default());
    }
}
