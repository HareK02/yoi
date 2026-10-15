use super::*;

#[tokio::test]
async fn ticket_browse_http_projects_only_list_fields_and_rejects_evidence_queries() {
    let temp = tempfile::tempdir().unwrap();
    let api = test_api(temp.path()).await;
    let backend = browser_ticket_backend(&api).unwrap();
    let created = backend
        .create(ticket::NewTicket::new("Lightweight browse HTTP"))
        .unwrap();
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/tickets/browse");
    request_json(
        build_router(api.clone()),
        "POST",
        &path,
        Some(json!({})),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    let app = build_inner_router(api);
    let response = request_json(
        app.clone(),
        "POST",
        &path,
        Some(json!({"states":["open"],"query":"Lightweight browse HTTP","limit":30})),
        StatusCode::OK,
    )
    .await;
    let parsed: server_api::TicketBrowseQueryResponse =
        serde_json::from_value(response.clone()).unwrap();
    assert_eq!(parsed.items.len(), 1);
    assert_eq!(parsed.items[0].id, created.id);
    assert_eq!(parsed.record_authority, "workspace-sqlite");
    for absent in [
        "evidence",
        "merge_requests",
        "body",
        "events",
        "content_digest",
    ] {
        assert!(response["items"][0].get(absent).is_none(), "{absent}");
    }
    // Browse cannot silently weaken an evidence query into a metadata filter.
    request_json(
        app.clone(),
        "POST",
        &path,
        Some(json!({"review_status":"approved"})),
        StatusCode::UNPROCESSABLE_ENTITY,
    )
    .await;
    request_json(
        app.clone(),
        "POST",
        &path,
        Some(json!({"sort":"relevance"})),
        StatusCode::BAD_REQUEST,
    )
    .await;
    let foreign = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/w/other-workspace/tickets/browse")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(!foreign.status().is_success());
}
