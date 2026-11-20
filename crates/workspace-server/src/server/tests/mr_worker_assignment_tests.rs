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
    assert_eq!(shown["publications"], json!([]));
    assert_eq!(shown["worker_assignment"], restored);
    assert_eq!(shown["summary"]["readiness"]["ready"], false);
    assert!(shown["summary"]["readiness"]["blockers"].is_array());
    // Dedicated reads, publication dispatch and completion no longer exist. PUT /worker remains.
    for (method, removed_path) in [
        (
            "GET",
            format!(
                "/api/w/{TEST_WORKSPACE_ID}/merge-requests/{}/readiness",
                mr.merge_request_id
            ),
        ),
        (
            "POST",
            format!(
                "/api/w/{TEST_WORKSPACE_ID}/merge-requests/{}/complete",
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
