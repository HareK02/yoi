//! Normal profile publication does not override an explicitly reduced conversation policy.
//! Reuses the ordinary connected model and signed public API harness, not SQL policy.
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn conversation_only_policy_exposes_but_refuses_coder_delegation() {
    crate::restore_test_environment::require_restore_test_environment();
    let (edited, _edits) = tokio::sync::mpsc::channel(1);
    let (evaluated, mut evaluations) = tokio::sync::mpsc::channel(1);
    let h = ConnectedWorkerHarness::start(NormalModel {
        edited,
        evaluated,
        overlap: Arc::new(tokio::sync::Barrier::new(1)),
        seen: Default::default(),
    })
    .await;
    h.api
        .runtime
        .unregister_if_idle(EMBEDDED_WORKER_RUNTIME_ID, 100)
        .unwrap();
    let (status,created) = h.human(reqwest::Method::POST,
        &format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects"),
        json!({"operation_id":"conversation-only-create","role":"conversation-only","behavior_md":""})).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let subject = created["id"].as_str().unwrap();
    let subjects = open_subjektiv_store(&h.api).unwrap();
    let policy = subjects.current_subject_policy(subject).unwrap().unwrap();
    assert_eq!(
        policy.execution,
        server_api::authorization::subject_default_permission_assignment()
    );
    assert_eq!(
        policy.delegation,
        server_api::authorization::subject_default_delegation_assignment()
    );
    // Conversation-only is an explicit reduced policy, not the creation default.
    // Keep only the Ticket reads used below alongside conversation, but deny
    // orchestration even with visible Tools and no coordination set.
    super::super::super::chat_tests::adopt_subject_policy(
        &h.api,
        &h.owner,
        subject,
        r#"{ sets = ["conversation"]; namespaces = [{ namespace = "ticket"; keys = ["ticket.read", "ticket.relation.read", "ticket.assignment.query"]; }]; exclude_namespaces = [{ namespace = "worker"; keys = ["worker.create"]; }, { namespace = "ticket"; keys = ["ticket.assignment.assign"]; }, { namespace = "merge_request"; keys = ["merge_request.complete"]; }]; }"#,
        "{}",
    )
    .await;
    let policy = subjects.current_subject_policy(subject).unwrap().unwrap();
    assert!(policy.delegation.grants.is_empty());
    let body = start(&h, subject, "conversation-only-start").await;
    assert_eq!(
        body["observation"]["worker"]["profile"],
        "builtin:async-chat"
    );
    let backend = browser_ticket_backend(&h.api).unwrap();
    let mut input = NewTicket::new("Conversation-only cannot start code work");
    input.body = "normal requirement".into();
    let ticket = backend.create(input).unwrap().id;
    backend
        .add_event(
            ticket.clone().into(),
            ticket::NewTicketEvent::new(ticket::TicketEventKind::Comment, "normal discussion"),
        )
        .unwrap();
    let _assigned = ok(&h,"PUT",&format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignments/orchestrator"),
        json!({"operation_id":"conversation-only-assign","expected_assignment_id":null,"principal":{"kind":"subject","subject_id":subject}})).await;
    let conversation = ok(
        &h,
        "POST",
        &format!("/api/w/{TEST_WORKSPACE_ID}/chat/conversations"),
        json!({"operation_id":"conversation-only-chat","subject_id":subject}),
    )
    .await;
    let input = json!({"subject":subject,"ticket":ticket,"deny":true,"content":"must not edit",
        "spawn":{"runtime_id":CONNECTED_RUNTIME,"profile":"builtin:coder",
            "operation_id":"conversation-only-denied-spawn","initial_submit":[],"workdir_attachments":[],
            "authorization":{"kind":"assignment","assignment":standalone_assignment("{}")}}});
    send(
        &h,
        conversation["id"].as_str().unwrap(),
        "conversation-only-request",
        NORMAL,
        &input,
    )
    .await;
    assert_eq!(next(&mut evaluations).await["denied"], ticket);
    assert_eq!(
        subjects.current_subject_policy(subject).unwrap().unwrap(),
        policy
    );
    let workers = h
        .api
        .runtime
        .list_workers_for_runtime(CONNECTED_RUNTIME, 100)
        .unwrap();
    assert_eq!(
        workers.items.len(),
        1,
        "Tool publication must not create unauthorized work"
    );
}
