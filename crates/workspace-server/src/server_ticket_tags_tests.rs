use super::*;

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
