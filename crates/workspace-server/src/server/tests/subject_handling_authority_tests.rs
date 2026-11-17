//! Assignment self-report authority through the signed in-process router.
//! Authentication, current-body ownership, delegated grants and Ticket SQLite
//! writes are real. The shared Fixture substitutes only Runtime execution: no
//! model, Workdir, TCP listener or notification ACK is needed for this rule.
use super::*;

#[tokio::test]
async fn tailored_background_handling_grant_does_not_bypass_current_subject_body() {
    let f = Fixture::new().await;
    f.adopt_policy(
        r#"{ namespaces = [
            { namespace = "worker"; keys = ["worker.create"]; },
            { namespace = "ticket"; keys = ["ticket.assignment.update"]; }
        ]; }"#,
        r#"{ namespaces = [{ namespace = "ticket"; keys = ["ticket.assignment.update"]; }]; }"#,
    )
    .await;
    let child = f
        .child(
            "handling-child",
            vec![workspace_operation("ticket.assignment.update")],
        )
        .await;
    let ticket = browser_ticket_backend(&f.api)
        .unwrap()
        .create(NewTicket::new(
            "Only the assigned Subject body may self-report",
        ))
        .unwrap()
        .id;
    let (status, assigned) = response(
        &f.api,
        human_request(
            &f.owner,
            "PUT",
            &format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignments/orchestrator"),
            Some(
                json!({"operation_id":"handling-assign","expected_assignment_id":null,
                "principal":{"kind":"subject","subject_id":f.subject}}),
            ),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{assigned}");
    let tracking = || {
        serde_json::to_value(
            f.api
                .store
                .list_current_ticket_assignment_tracking(TEST_WORKSPACE_ID, &ticket)
                .unwrap(),
        )
        .unwrap()
    };
    let before = tracking();
    let row = &before[0];
    let update = json!({"assignment_id":row["assignment_id"],
        "expected_last_event_id":row["last_event_id"],"operation_key":"handling-report",
        "tracking":{"status":"done","waiting_reason":null,"progress":null,
            "next_action":null,"result":"Subject judgment"}});
    let permission = ScopedPermission {
        operation: "ticket.assignment.update".into(),
        resource: PermissionResource::Ticket {
            workspace_id: TEST_WORKSPACE_ID.into(),
            ticket_id: ticket.clone(),
        },
    };
    // Both identities have the operation. A denial due to missing capability
    // would not test the independent current-body fence.
    for worker in [&f.source, &child] {
        let (status, body) = f.check(worker, &permission).await;
        assert_eq!(status, StatusCode::OK, "{worker:?}: {body}");
    }
    let path = format!("/workspaces/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignment-tracking");
    let (status, denied) = response(
        &f.api,
        runtime_source_request(
            &f.identity,
            Some(&child.worker_id),
            "POST",
            &path,
            serde_json::to_vec(&update).unwrap(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{denied}");
    assert!(
        denied["message"]
            .as_str()
            .unwrap()
            .contains("Assignment self-report requires current Subject body ownership"),
        "{denied}"
    );
    assert_eq!(
        tracking(),
        before,
        "rejection must not mutate handling or CAS"
    );

    // The same valid input succeeds from the current body. This rules out an
    // invalid assignment/CAS/payload as the reason the child was refused.
    let (status, accepted) = response(
        &f.api,
        runtime_source_request(
            &f.identity,
            Some(&f.source.worker_id),
            "POST",
            &path,
            serde_json::to_vec(&update).unwrap(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{accepted}");
    let after = tracking();
    assert_eq!(after[0]["assignment_id"], row["assignment_id"]);
    assert_ne!(after[0]["last_event_id"], row["last_event_id"]);
    assert_eq!(after[0]["tracking"]["status"], "done");
    assert_eq!(after[0]["tracking"]["result"], "Subject judgment");
}
