//! MR Worker API integration: signed HTTP, real policy and SQLite ownership.
//! Runtime identity/catalog uses the shared Workdirless fixture; Git is not exercised.
use super::*;

struct InitialMrAuthority(merge_request::CurrentAssignment, String);
impl merge_request::AssignmentSource for InitialMrAuthority {
    fn current_assignment(
        &self,
        _: &str,
        ticket: &str,
    ) -> std::result::Result<Option<merge_request::CurrentAssignment>, String> {
        Ok((ticket == self.0.ticket_id).then(|| self.0.clone()))
    }
}
impl merge_request::RepositorySource for InitialMrAuthority {
    fn repository_belongs_to_workspace(
        &self,
        workspace: &str,
        repository: &str,
    ) -> std::result::Result<bool, String> {
        Ok(workspace == TEST_WORKSPACE_ID && repository == self.1)
    }
}

#[tokio::test]
async fn mr_worker_http_assignment_requires_permission_and_preserves_cas_history() {
    let f = Fixture::new().await;
    let repository_id = f
        .api
        .store
        .get_repository_by_key(TEST_WORKSPACE_ID, "test-repository")
        .unwrap()
        .unwrap()
        .repository_id;
    f.adopt_policy(
        r#"{ namespaces = [{ namespace = "merge_request"; keys = ["merge_request.read"]; }]; }"#,
        "{}",
    )
    .await;
    let ticket = browser_ticket_backend(&f.api)
        .unwrap()
        .create(NewTicket::new("MR ownership API"))
        .unwrap()
        .id;
    let initial = Arc::new(InitialMrAuthority(
        merge_request::CurrentAssignment {
            assignment_id: "initial-ticket-assignment".into(),
            ticket_id: ticket.clone(),
            runtime_id: f.source.runtime_id.clone(),
            worker_id: f.source.worker_id.clone(),
        },
        repository_id.clone(),
    ));
    let store = merge_request::MergeRequestStore::open(
        &f.api.config.database_path,
        initial.clone(),
        initial,
    )
    .unwrap();
    let mr = store
        .open_merge_request(merge_request::OpenMergeRequest {
            merge_request_id: "mr-worker-boundary".into(),
            ticket_id: ticket.clone(),
            repository_id: repository_id.clone(),
            selector_from: "refs/heads/source".into(),
            selector_to: "refs/heads/target".into(),
            summary: String::new(),
            auth: merge_request::MergeRequestAuth {
                workspace_id: TEST_WORKSPACE_ID.into(),
                repository_id: repository_id.clone(),
                runtime_id: f.source.runtime_id.clone(),
                worker_id: f.source.worker_id.clone(),
                assignment_id: "initial-ticket-assignment".into(),
            },
            now: Utc::now(),
        })
        .unwrap();
    let path = format!(
        "/api/w/{TEST_WORKSPACE_ID}/merge-requests/{}/worker",
        mr.merge_request_id
    );
    let show_path = format!(
        "/api/w/{TEST_WORKSPACE_ID}/merge-requests/{}?view=summary",
        mr.merge_request_id
    );
    let (status, current) = response(
        &f.api,
        runtime_source_request(
            &f.identity,
            Some(&f.source.worker_id),
            "GET",
            &show_path,
            vec![],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{current}");
    let initial_operation = current["worker_assignment"]["operation_id"].clone();
    let release = json!({"operation_id":"release-mr-worker", "expected_operation_id":initial_operation, "worker":null});
    let send = |body: &Value| {
        runtime_source_request(
            &f.identity,
            Some(&f.source.worker_id),
            "PUT",
            &path,
            serde_json::to_vec(body).unwrap(),
        )
    };
    let (status, denied) = response(&f.api, send(&release)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{denied}");
    assert_eq!(
        store
            .get_by_id(TEST_WORKSPACE_ID, &mr.merge_request_id)
            .unwrap(),
        mr
    );
    f.adopt_policy(r#"{ namespaces = [{ namespace = "merge_request"; keys = ["merge_request.read", "merge_request.assign"]; }]; }"#, "{}").await;
    let (status, receipt) = response(&f.api, send(&release)).await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    assert!(receipt["worker"].is_null());
    let (status, released) = response(
        &f.api,
        runtime_source_request(
            &f.identity,
            Some(&f.source.worker_id),
            "GET",
            &show_path,
            vec![],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{released}");
    assert_eq!(released["worker_assignment"], receipt);
    assert_eq!(released["thread"], json!([]));
    let (status, replay) = response(&f.api, send(&release)).await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(receipt, replay);
    let mut stale = release.clone();
    stale["operation_id"] = json!("stale-mr-owner");
    stale["worker"] = json!({"runtime_id":f.source.runtime_id,"worker_id":f.source.worker_id});
    let (status, conflict) = response(&f.api, send(&stale)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{conflict}");
    stale["operation_id"] = json!("restore-mr-owner");
    stale["expected_operation_id"] = receipt["operation_id"].clone();
    let (status, restored) = response(&f.api, send(&stale)).await;
    assert_eq!(status, StatusCode::OK, "{restored}");
    assert_eq!(restored["worker"]["worker_id"], f.source.worker_id);
    let observed = store
        .get_by_id(TEST_WORKSPACE_ID, &mr.merge_request_id)
        .unwrap();
    assert_eq!(observed.state, mr.state);
    assert_eq!(observed.thread.len(), mr.thread.len() + 2);
    // No Ticket assignment was needed for changes to an existing MR.
    assert!(
        f.api
            .store
            .get_current_ticket_worker_assignment(TEST_WORKSPACE_ID, &ticket)
            .unwrap()
            .is_none()
    );
    let comments_path = format!(
        "/api/w/{TEST_WORKSPACE_ID}/merge-requests/{}/comments",
        mr.merge_request_id
    );
    let comment = json!({"operation_id":"integration-explanation", "expected_owner_operation_id":restored["operation_id"], "body":"Resolved conflict.\nValidation passed."});
    let send_comment = |body: &Value| {
        runtime_source_request(
            &f.identity,
            Some(&f.source.worker_id),
            "POST",
            &comments_path,
            serde_json::to_vec(body).unwrap(),
        )
    };
    let (status, denied) = response(&f.api, send_comment(&comment)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{denied}");
    f.adopt_policy(r#"{ namespaces = [{ namespace = "merge_request"; keys = ["merge_request.read", "merge_request.comment"]; }]; }"#, "{}").await;
    let (status, posted) = response(&f.api, send_comment(&comment)).await;
    assert_eq!(status, StatusCode::OK, "{posted}");
    assert_eq!(posted["body"], comment["body"]);
    let (status, replay) = response(&f.api, send_comment(&comment)).await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(posted, replay);
    let mut stale_comment = comment.clone();
    stale_comment["operation_id"] = json!("late-comment");
    stale_comment["expected_owner_operation_id"] = receipt["operation_id"].clone();
    let (status, stale_reply) = response(&f.api, send_comment(&stale_comment)).await;
    assert!(
        matches!(status, StatusCode::CONFLICT | StatusCode::FORBIDDEN),
        "{status}: {stale_reply}"
    );
    let (status, shown) = response(
        &f.api,
        runtime_source_request(
            &f.identity,
            Some(&f.source.worker_id),
            "GET",
            &show_path,
            vec![],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{shown}");
    assert!(shown.get("publications").is_none());
    assert_eq!(shown["worker_assignment"], restored);
    assert_eq!(shown["summary"]["readiness"]["ready"], false);
    assert!(shown["summary"]["readiness"]["blockers"].is_array());
    // Completion is a current-owner operation, not a removed publication route.
    // This Workdirless fixture rejects before provider access; the connected Git
    // suite owns successful manual-push completion.
    let complete_path = format!(
        "/api/w/{TEST_WORKSPACE_ID}/merge-requests/{}/complete",
        mr.merge_request_id
    );
    let completion = json!({
        "operation_id": "manual-completion",
        "expected_owner_operation_id": restored["operation_id"],
        "approval_event_id": "missing-approval",
        "expected_source_ref": "source"
    });
    let send_completion = |body: &Value| {
        runtime_source_request(
            &f.identity,
            Some(&f.source.worker_id),
            "POST",
            &complete_path,
            serde_json::to_vec(body).unwrap(),
        )
    };
    let before_completion = store
        .get_by_id(TEST_WORKSPACE_ID, &mr.merge_request_id)
        .unwrap();
    let (status, denied) = response(&f.api, send_completion(&completion)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{denied}");
    f.adopt_policy(
        r#"{ namespaces = [{ namespace = "merge_request"; keys = ["merge_request.complete"]; }]; }"#,
        "{}",
    )
    .await;
    let (status, denied) = response(&f.api, send_completion(&completion)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "read is required: {denied}");
    f.adopt_policy(
        r#"{ namespaces = [{ namespace = "merge_request"; keys = ["merge_request.read", "merge_request.complete"]; }]; }"#,
        "{}",
    )
    .await;
    let mut stale_completion = completion.clone();
    stale_completion["expected_owner_operation_id"] = receipt["operation_id"].clone();
    let (status, denied) = response(&f.api, send_completion(&stale_completion)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "stale owner: {denied}");
    let (status, unapproved) = response(&f.api, send_completion(&completion)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{unapproved}");
    assert!(
        unapproved
            .to_string()
            .contains("completion approval was not found on this MR"),
        "current owner must reach approval validation: {unapproved}"
    );
    assert_eq!(
        store
            .get_by_id(TEST_WORKSPACE_ID, &mr.merge_request_id)
            .unwrap(),
        before_completion,
        "rejected completion must not change MR state or history"
    );
    // Dedicated reads and publication dispatch remain removed. PUT /worker remains.
    for (method, removed_path) in [
        (
            "GET",
            format!(
                "/api/w/{TEST_WORKSPACE_ID}/merge-requests/{}/readiness",
                mr.merge_request_id
            ),
        ),
        ("GET", path.clone()),
        (
            "GET",
            format!(
                "/api/w/{TEST_WORKSPACE_ID}/merge-requests/{}/publications",
                mr.merge_request_id
            ),
        ),
        (
            "POST",
            format!(
                "/api/w/{TEST_WORKSPACE_ID}/merge-requests/{}/publications",
                mr.merge_request_id
            ),
        ),
    ] {
        let (status, body) = response(
            &f.api,
            runtime_source_request(
                &f.identity,
                Some(&f.source.worker_id),
                method,
                &removed_path,
                vec![],
            ),
        )
        .await;
        assert!(
            matches!(
                status,
                StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
            ),
            "{method} {removed_path}: {status} {body}"
        );
    }
    let observed = store
        .get_by_id(TEST_WORKSPACE_ID, &mr.merge_request_id)
        .unwrap();
    f.adopt_policy("{}", "{}").await;
    let (status, denied) = response(&f.api, send(&stale)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{denied}");
    assert_eq!(
        store
            .get_by_id(TEST_WORKSPACE_ID, &mr.merge_request_id)
            .unwrap(),
        observed
    );
}

#[tokio::test]
async fn mr_query_http_checks_collection_and_item_permissions_and_authenticated_self() {
    let f = Fixture::new().await;
    let repository_id = f
        .api
        .store
        .get_repository_by_key(TEST_WORKSPACE_ID, "test-repository")
        .unwrap()
        .unwrap()
        .repository_id;
    let ticket = browser_ticket_backend(&f.api)
        .unwrap()
        .create(NewTicket::new("query target"))
        .unwrap()
        .id;
    let initial = Arc::new(InitialMrAuthority(
        merge_request::CurrentAssignment {
            assignment_id: "initial-query".into(),
            ticket_id: ticket.clone(),
            runtime_id: f.source.runtime_id.clone(),
            worker_id: f.source.worker_id.clone(),
        },
        repository_id.clone(),
    ));
    let store = merge_request::MergeRequestStore::open(
        &f.api.config.database_path,
        initial.clone(),
        initial,
    )
    .unwrap();
    store
        .open_merge_request(merge_request::OpenMergeRequest {
            merge_request_id: "query-mr".into(),
            ticket_id: ticket.clone(),
            repository_id: repository_id.clone(),
            selector_from: "work/query".into(),
            selector_to: "refs/heads/develop".into(),
            summary: String::new(),
            auth: merge_request::MergeRequestAuth {
                workspace_id: TEST_WORKSPACE_ID.into(),
                repository_id,
                runtime_id: f.source.runtime_id.clone(),
                worker_id: f.source.worker_id.clone(),
                assignment_id: "initial-query".into(),
            },
            now: Utc::now(),
        })
        .unwrap();
    let path = format!(
        "/api/w/{TEST_WORKSPACE_ID}/merge-requests?repository_key=test-repository&selector_to=develop&phase=implementation&assigned_to_self=true&limit=1"
    );
    let send = |path: &str| {
        runtime_source_request(&f.identity, Some(&f.source.worker_id), "GET", path, vec![])
    };
    let query_permission = ScopedPermission {
        operation: "merge_request.query".into(),
        resource: PermissionResource::Workspace {
            workspace_id: TEST_WORKSPACE_ID.into(),
        },
    };
    let read_permission = ScopedPermission {
        operation: "merge_request.read".into(),
        resource: PermissionResource::MergeRequest {
            workspace_id: TEST_WORKSPACE_ID.into(),
            merge_request_id: "query-mr".into(),
        },
    };
    f.seed_saved_policy(vec![read_permission.clone()], vec![]);
    let (status, _) = response(&f.api, send(&path)).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "item read is not collection query"
    );
    f.seed_saved_policy(vec![query_permission.clone()], vec![]);
    let (status, denied) = response(&f.api, send(&path)).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "query is not item read: {denied}"
    );
    assert!(denied.get("items").is_none());
    f.seed_saved_policy(vec![query_permission.clone(), read_permission], vec![]);
    let (status, result) = response(&f.api, send(&path)).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["items"].as_array().unwrap().len(), 1, "{result}");
    assert_eq!(
        result["items"][0]["summary"]["merge_request_id"],
        "query-mr"
    );
    assert!(result["next_cursor"].is_null());
    let item = &result["items"][0];
    let detail_path = format!("/api/w/{TEST_WORKSPACE_ID}/merge-requests/query-mr?view=summary");
    let (status, detail) = response(&f.api, send(&detail_path)).await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(item["worker_assignment"], detail["worker_assignment"]);
    assert_eq!(
        item["worker_assignment"]["worker"]["worker_id"],
        f.source.worker_id
    );
    let key = f
        .api
        .store
        .worker_resource_key(TEST_WORKSPACE_ID, &f.source)
        .unwrap()
        .unwrap();
    assert!(key.starts_with("W-"));
    assert_eq!(item["worker_resource_key"], key);
    assert_eq!(detail["worker_resource_key"], key);
    assert!(item.get("publications").is_none());
    assert!(item["summary"].get("worker_assignment").is_none());
    assert!(item["summary"].get("publications").is_none());
    assert!(item.get("readiness").is_none());
    for (suffix, expected) in [
        ("&phase=bogus", StatusCode::BAD_REQUEST),
        ("&limit=0", StatusCode::BAD_REQUEST),
    ] {
        // Invalid values are tested without duplicate query keys.
        let invalid = format!("/api/w/{TEST_WORKSPACE_ID}/merge-requests?{}", &suffix[1..]);
        let (status, _) = response(&f.api, send(&invalid)).await;
        assert!(
            status == expected || status == StatusCode::UNPROCESSABLE_ENTITY,
            "{invalid}: {status}"
        );
    }
    let (status, empty) = response(
        &f.api,
        send(&path.replace("phase=implementation", "phase=integration")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{empty}");
    assert!(empty["items"].as_array().unwrap().is_empty());
    // Self is authenticated, never selected by query arguments or historical Ticket assignment.
    let current = store
        .current_worker(TEST_WORKSPACE_ID, "query-mr")
        .unwrap()
        .unwrap();
    store
        .set_worker(
            merge_request::SetMergeRequestWorker {
                workspace_id: TEST_WORKSPACE_ID.into(),
                merge_request_id: "query-mr".into(),
                operation_id: "query-release".into(),
                expected_operation_id: Some(current.operation_id),
                worker: None,
                assigned_by: merge_request::WorkerIdentity {
                    runtime_id: f.source.runtime_id.clone(),
                    worker_id: f.source.worker_id.clone(),
                },
                now: Utc::now(),
            },
            |_| Ok(()),
        )
        .unwrap();
    let (status, empty) = response(&f.api, send(&path)).await;
    assert_eq!(status, StatusCode::OK, "{empty}");
    assert!(empty["items"].as_array().unwrap().is_empty());
    let (status, all) = response(
        &f.api,
        send(&path.replace("assigned_to_self=true", "assigned_to_self=false")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{all}");
    assert_eq!(all["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        all["items"][0]["worker_assignment"]["operation_id"],
        "query-release"
    );
    assert!(all["items"][0]["worker_assignment"]["worker"].is_null());
    assert!(all["items"][0]["worker_resource_key"].is_null());
    let (status, released_detail) = response(&f.api, send(&detail_path)).await;
    assert_eq!(status, StatusCode::OK, "{released_detail}");
    assert_eq!(
        all["items"][0]["worker_assignment"],
        released_detail["worker_assignment"]
    );

    // Publication failures are ordinary thread comments, not an operation ledger.
    let listing = format!("/api/w/{TEST_WORKSPACE_ID}/merge-requests");
    store
        .record_publication_problem(
            TEST_WORKSPACE_ID,
            "query-mr",
            "Workdir unavailable; Subject action required",
            Utc::now(),
        )
        .unwrap();
    let (status, detail) = response(&f.api, send(&detail_path)).await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert!(detail.get("publications").is_none());
    f.seed_saved_policy(vec![query_permission], vec![]);
    let (status, denied) = response(&f.api, send(&listing)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{denied}");
    assert!(
        denied.get("items").is_none(),
        "saved ownership/publication cannot bypass item read"
    );
    let (status, _) = response(
        &f.api,
        Request::builder().uri(&path).body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "anonymous query is not a Worker identity"
    );
    let (status, _) = response(&f.api, human_request(&f.owner, "GET", &path, None)).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "browser has no Runtime Worker self"
    );
    f.seed_saved_policy(vec![], vec![]);
    let (status, _) = response(&f.api, send(&path)).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "revocation is checked even on an empty filtered page"
    );
}
