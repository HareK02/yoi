// Included in the production persistent-Worker delivery fixture. Only the LLM is
// scripted. Signed Runtime HTTP, assignment fencing, SQLite and session receipts are real.
async fn result_signed_http(
    api: &WorkspaceApi,
    identity: &RuntimeIdentityMaterial,
    worker: &RuntimeWorkerRef,
    ticket: &str,
    body: &Value,
    expected: StatusCode,
) -> Value {
    let path = format!("/workspaces/{TEST_WORKSPACE_ID}/tickets/{ticket}/implementation-results");
    let response = build_router(api.clone())
        .oneshot(runtime_source_request(
            identity,
            Some(&worker.worker_id),
            "POST",
            &path,
            serde_json::to_vec(body).unwrap(),
        ))
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(status, expected, "{}", String::from_utf8_lossy(&bytes));
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn implementation_result_signed_http_reaches_persistent_orchestrator_once_without_mr() {
    let dir = tempfile::tempdir().unwrap();
    let (mut api, execution, _) = delivery_real_fixture(dir.path(), false).await;
    let identity = RuntimeIdentityMaterial::generate(EMBEDDED_WORKER_RUNTIME_ID).unwrap();
    configure_runtime_request_auth(&mut api, &identity, EMBEDDED_WORKER_RUNTIME_ID);
    let ticket = delivery_ticket(&api, "Return repository-free research");
    let assigned = delivery_assign(&api, &ticket.id, "research-delegation").await;
    let delegation = assigned["assignment"]["assignment_id"].as_str().unwrap();
    let orchestrator = delivery_worker(start_attention_test_orchestrator(&api).await);
    seed_worker_source_member(&api, &orchestrator.runtime_id, &orchestrator.worker_id);
    delivery_wait_idle(&api, &orchestrator, 1).await;
    let initial = delivery_tracking(&api, &ticket.id, delegation);
    delivery_self_report(
        &api,
        &identity,
        &orchestrator,
        &initial,
        "inprogress",
        "research-progress",
    )
    .await;
    let coder = spawn_ticket_check_source(&api, "research-coder");
    seed_worker_source_member(&api, &coder.runtime_id, &coder.worker_id);
    delivery_assign_worker(&api, &ticket.id, &coder, "research-implementation", None);
    recover_ticket_notifications(&api);
    let before = delivery_tracking(&api, &ticket.id, delegation);
    let input = json!({"operation_key":"research-result","assignment_id":"research-implementation","orchestrator_assignment_id":delegation,"kind":"result","body":"Documented findings; no repository or MR was required.","merge_request":null});
    // The registered Orchestrator is not the implementation assignee and cannot forge this result.
    result_signed_http(
        &api,
        &identity,
        &orchestrator,
        &ticket.id,
        &input,
        StatusCode::FORBIDDEN,
    )
    .await;
    let receipt =
        result_signed_http(&api, &identity, &coder, &ticket.id, &input, StatusCode::OK).await;
    delivery_wait_idle(&api, &orchestrator, 2).await;
    assert_eq!(
        result_signed_http(&api, &identity, &coder, &ticket.id, &input, StatusCode::OK).await,
        receipt
    );
    let mut changed = input.clone();
    changed["body"] = json!("Different result");
    result_signed_http(
        &api,
        &identity,
        &coder,
        &ticket.id,
        &changed,
        StatusCode::CONFLICT,
    )
    .await;
    let mut forged = input.clone();
    forged["operation_key"] = json!("forged-result");
    forged["merge_request"] = json!({"merge_request_id":"missing","subject_ref":"fake"});
    result_signed_http(
        &api,
        &identity,
        &coder,
        &ticket.id,
        &forged,
        StatusCode::BAD_REQUEST,
    )
    .await;
    let id = receipt["result_id"].as_str().unwrap();
    assert!(
        delivery_commits(&api, &orchestrator)[1].contains(&format!("implementation-result:{id}"))
    );
    assert_eq!(delivery_tracking(&api, &ticket.id, delegation), before);
    assert_eq!(
        api.authority.ticket(&ticket.id).unwrap().state,
        server_api::BrowserTicketState::Open
    );
    let saved = delivery_http(
        &api,
        "GET",
        &format!(
            "/workspaces/{TEST_WORKSPACE_ID}/tickets/{}/implementation-results/{id}",
            ticket.id
        ),
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(saved, receipt);
    let attempts = execution.attempts.lock().unwrap().len();
    for _ in 0..3 {
        recover_ticket_notifications(&api);
    }
    assert_eq!(execution.attempts.lock().unwrap().len(), attempts);
    assert_eq!(delivery_receipts(&api, &orchestrator).len(), 2);
    delivery_stop(&api, &coder);
    delivery_stop(&api, &orchestrator);
}

// MR event creation uses the production store and registered reviewer capability.
// Provider observations/child registration are fixture boundaries here; the existing
// authenticated MR router suite owns Git publication and trusted child attestation.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mr_result_recovery_and_handoff_are_distinct_persistent_inputs() {
    let dir = tempfile::tempdir().unwrap();
    let (mut api, execution, _) = delivery_real_fixture(dir.path(), false).await;
    let identity = RuntimeIdentityMaterial::generate(EMBEDDED_WORKER_RUNTIME_ID).unwrap();
    configure_runtime_request_auth(&mut api, &identity, EMBEDDED_WORKER_RUNTIME_ID);
    let ticket = delivery_ticket(&api, "MR result followed by implementation handoff");
    let assigned = delivery_assign(&api, &ticket.id, "mr-delegation").await;
    let delegation = assigned["assignment"]["assignment_id"].as_str().unwrap();
    let orchestrator = delivery_worker(start_attention_test_orchestrator(&api).await);
    seed_worker_source_member(&api, &orchestrator.runtime_id, &orchestrator.worker_id);
    delivery_wait_idle(&api, &orchestrator, 1).await;
    let initial = delivery_tracking(&api, &ticket.id, delegation);
    delivery_self_report(
        &api,
        &identity,
        &orchestrator,
        &initial,
        "inprogress",
        "mr-progress",
    )
    .await;
    let coder = spawn_ticket_check_source(&api, "mr-coder");
    seed_worker_source_member(&api, &coder.runtime_id, &coder.worker_id);
    delivery_assign_worker(&api, &ticket.id, &coder, "mr-implementation", None);
    seed_test_repository(&api, TEST_REPOSITORY_ID);
    api.config.repositories = test_server_config(dir.path()).repositories;
    let store = merge_request_store(&api, TEST_WORKSPACE_ID).unwrap();
    let auth = merge_request::MergeRequestAuth {
        workspace_id: TEST_WORKSPACE_ID.into(),
        repository_id: TEST_REPOSITORY_ID.into(),
        runtime_id: coder.runtime_id.clone(),
        worker_id: coder.worker_id.clone(),
        assignment_id: "mr-implementation".into(),
    };
    store
        .open_merge_request(merge_request::OpenMergeRequest {
            merge_request_id: "result-mr".into(),
            ticket_id: ticket.id.clone(),
            repository_id: TEST_REPOSITORY_ID.into(),
            selector_from: "refs/heads/work".into(),
            selector_to: "refs/heads/develop".into(),
            summary: String::new(),
            auth: auth.clone(),
            now: Utc::now(),
        })
        .unwrap();
    store
        .register_reviewer_child_session(merge_request::RegisterReviewerChildSession {
            workspace_id: TEST_WORKSPACE_ID.into(),
            parent_runtime_id: coder.runtime_id.clone(),
            parent_worker_id: coder.worker_id.clone(),
            child_session_id: "result-reviewer".into(),
            reviewer_profile: "builtin:reviewer".into(),
            now: Utc::now(),
        })
        .unwrap();
    let digest = api.authority.ticket(&ticket.id).unwrap().content_digest;
    let requested = store
        .request_review(merge_request::RequestMergeRequestReview {
            merge_request_id: "result-mr".into(),
            ticket_id: ticket.id.clone(),
            ticket_content_digest: digest,
            ticket_merge_request_subjects: vec![merge_request::MergeRequestReviewSubject {
                merge_request_id: "result-mr".into(),
                subject_ref: "source-a".into(),
            }],
            subject_ref: "source-a".into(),
            child_session_id: "result-reviewer".into(),
            capability_token: "result-review-capability".into(),
            auth: auth.clone(),
            now: Utc::now(),
        })
        .unwrap();
    let approved = store
        .submit_review(merge_request::SubmitMergeRequestReview {
            merge_request_id: "result-mr".into(),
            ticket_id: ticket.id.clone(),
            current_subject_ref: "source-a".into(),
            capability_token: "result-review-capability".into(),
            decision: merge_request::ReviewDecision::Approve,
            body: "Approved exact source".into(),
            findings: vec![],
            now: Utc::now(),
        })
        .unwrap();
    // No mutation callback: both saved MR events must be found by normal recovery.
    recover_ticket_notifications(&api);
    delivery_wait_idle(&api, &orchestrator, 3).await;
    let input = json!({"operation_key":"approved-handoff","assignment_id":"mr-implementation","orchestrator_assignment_id":delegation,"kind":"handoff","body":"Implementation and validation complete; integration remains separate.","merge_request":{"merge_request_id":"result-mr","subject_ref":"source-a"}});
    let receipt =
        result_signed_http(&api, &identity, &coder, &ticket.id, &input, StatusCode::OK).await;
    delivery_wait_idle(&api, &orchestrator, 4).await;
    let commits = delivery_commits(&api, &orchestrator);
    assert!(
        commits
            .iter()
            .any(|s| s.contains(&requested.request_event.event_id))
    );
    assert!(commits.iter().any(|s| s.contains(&approved.event_id)));
    assert!(
        commits
            .iter()
            .any(|s| s.contains(receipt["result_id"].as_str().unwrap()))
    );
    store
        .revoke_review(merge_request::RevokeMergeRequestReview {
            merge_request_id: "result-mr".into(),
            ticket_id: ticket.id.clone(),
            review_event_id: approved.event_id,
            reason: "Approval withdrawn".into(),
            auth: auth.clone(),
            now: Utc::now(),
        })
        .unwrap();
    api.config_store
        .observe_merge_request_source(
            TEST_WORKSPACE_ID,
            TEST_REPOSITORY_ID,
            "refs/heads/work",
            "source-b",
        )
        .unwrap();
    api.config_store
        .observe_merge_request_source(
            TEST_WORKSPACE_ID,
            TEST_REPOSITORY_ID,
            "refs/heads/work",
            "source-b",
        )
        .unwrap();
    recover_ticket_notifications(&api);
    delivery_wait_idle(&api, &orchestrator, 6).await;
    for (child, requested_source, observed_source) in [
        ("cancelled-reviewer", "source-b", "source-c"),
        ("changes-reviewer", "source-c", "source-c"),
    ] {
        store
            .register_reviewer_child_session(merge_request::RegisterReviewerChildSession {
                workspace_id: TEST_WORKSPACE_ID.into(),
                parent_runtime_id: coder.runtime_id.clone(),
                parent_worker_id: coder.worker_id.clone(),
                child_session_id: child.into(),
                reviewer_profile: "builtin:reviewer".into(),
                now: Utc::now(),
            })
            .unwrap();
        store
            .request_review(merge_request::RequestMergeRequestReview {
                merge_request_id: "result-mr".into(),
                ticket_id: ticket.id.clone(),
                ticket_content_digest: api.authority.ticket(&ticket.id).unwrap().content_digest,
                ticket_merge_request_subjects: vec![merge_request::MergeRequestReviewSubject {
                    merge_request_id: "result-mr".into(),
                    subject_ref: requested_source.into(),
                }],
                subject_ref: requested_source.into(),
                child_session_id: child.into(),
                capability_token: child.into(),
                auth: auth.clone(),
                now: Utc::now(),
            })
            .unwrap();
        let submitted = store.submit_review(merge_request::SubmitMergeRequestReview {
            merge_request_id: "result-mr".into(),
            ticket_id: ticket.id.clone(),
            current_subject_ref: observed_source.into(),
            capability_token: child.into(),
            decision: merge_request::ReviewDecision::RequestChanges,
            body: "Recheck implementation".into(),
            findings: vec![],
            now: Utc::now(),
        });
        assert_eq!(submitted.is_err(), requested_source != observed_source);
    }
    recover_ticket_notifications(&api);
    delivery_wait_idle(&api, &orchestrator, 10).await;
    let attempts = execution.attempts.lock().unwrap().len();
    result_signed_http(&api, &identity, &coder, &ticket.id, &input, StatusCode::OK).await;
    recover_ticket_notifications(&api);
    assert_eq!(execution.attempts.lock().unwrap().len(), attempts);
    assert_eq!(delivery_receipts(&api, &orchestrator).len(), 10);
    assert_eq!(
        api.authority.ticket(&ticket.id).unwrap().state,
        server_api::BrowserTicketState::Open
    );
    assert_eq!(
        delivery_tracking(&api, &ticket.id, delegation)
            .tracking
            .unwrap()
            .status,
        ticket::TicketAssignmentTrackingStatus::Inprogress
    );
    delivery_stop(&api, &coder);
    delivery_stop(&api, &orchestrator);
}
