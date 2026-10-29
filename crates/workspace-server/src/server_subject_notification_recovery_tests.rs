// Included as a child module of server::tests by the parent change.
// Component regression: real Subject connections/singleton ownership, policy grants,
// Ticket transactions, routing, recovery, request persistence and ACK admission.
// DeterministicExecutionBackend records Runtime inputs instead of running a real
// Controller/model. Runtime acceptance here is NOT live-model processing evidence.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

#[tokio::test]
async fn subject_notification_recovery_drains_two_recipients_beyond_global_scan_limit() {
    let dir = tempfile::tempdir().unwrap();
    init_clean_git_workspace(dir.path());
    let (api, execution) = assignment_test_api_with_recording_backend(dir.path()).await;
    seed_test_delivery_binding(&api);
    let subject_store = open_subjektiv_store(&api).unwrap();
    let mut recipients = Vec::new();
    for role in ["recovery-alpha", "recovery-beta"] {
        let subject = subject_store
            .create_subject_with_behavior(
                crate::subjektiv::SubjectRole::new(role).unwrap(),
                "Handle assigned Tickets; delivery does not prove processing.".into(),
            )
            .unwrap();
        // Normal Subject-connected creation, not a synthetic worker registry row
        // or a monkey-patched assigned_subject_body/candidate selector.
        let body = create_test_assignment_body(&api, EMBEDDED_WORKER_RUNTIME_ID, &subject.id).await;
        bootstrap_test_assignment_session(&api, &body);
        assert_eq!(
            current_assignment_subject_source(&api, &body).unwrap(),
            subject.id
        );
        recipients.push((body, subject.id));
    }
    // Recovery dispatches recipients in BTreeSet<(runtime_id, worker_id)> order.
    // UUIDs need not have a predictable creation order: derive alpha/beta from
    // the ACTUAL body keys, then commit beta's entire block before alpha's.
    recipients.sort_by_key(|(body, _)| (body.runtime_id.clone(), body.worker_id.clone()));
    let [(alpha, alpha_subject), (beta, beta_subject)] = recipients.as_slice() else {
        unreachable!()
    };
    assert_ne!(alpha_subject, beta_subject);
    assert_ne!(alpha, beta);
    assert!(execution.take_inputs().is_empty());
    assert!(execution.take_input_request_ids().is_empty());

    let mut tickets = BTreeMap::new();
    for (name, body, subject) in [
        ("beta", beta, beta_subject),
        ("alpha", alpha, alpha_subject),
    ] {
        for index in 0..20 {
            let ticket = browser_ticket_backend(&api)
                .unwrap()
                .create(ticket::NewTicket::new(format!(
                    "Recovery {name} {index:02}"
                )))
                .unwrap();
            grant_test_assignment_ticket(&api, subject, &ticket.id);
            // Commit through the production store transaction, but do not fire a
            // mutation wakeup: ONLY production recovery may drain this backlog.
            api.store
                .set_current_ticket_role_assignment(
                    &TicketRoleAssignmentRecord {
                        workspace_id: TEST_WORKSPACE_ID.into(),
                        ticket_id: ticket.id.clone(),
                        assignment_id: format!("recovery-{name}-{index:02}"),
                        role: TicketAssignmentRole::Orchestrator,
                        principal: TicketAssignmentPrincipal::Subject {
                            subject_id: subject.clone(),
                        },
                        assigned_by: "fixture-owner".into(),
                        assigned_at: TEST_CREATED_AT.into(),
                    },
                    None,
                    &format!("recovery-{name}-{index:02}-event"),
                    &format!("recovery-{name}-{index:02}-assign"),
                    false,
                )
                .unwrap();
            tickets.insert(ticket.id.clone(), (body.clone(), subject.clone()));
        }
    }
    let pending = api
        .store
        .list_pending_ticket_assignment_deliveries(TEST_WORKSPACE_ID, 100)
        .unwrap();
    assert_eq!(
        pending.len(),
        40,
        "all forty inputs must be current and eligible"
    );
    assert_eq!(
        pending
            .iter()
            .filter(|input| tickets[&input.ticket_id].0 == *alpha)
            .count(),
        20
    );
    assert_eq!(
        pending
            .iter()
            .filter(|input| tickets[&input.ticket_id].0 == *beta)
            .count(),
        20
    );
    assert!(
        pending[..20]
            .iter()
            .all(|input| tickets[&input.ticket_id].0 == *beta),
        "the global first page must belong to the recipient dispatched second"
    );
    let expected = pending
        .into_iter()
        .map(|input| (input.request_id.clone(), input))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(expected.len(), 40);
    for input in expected.values() {
        let (body, _) = &tickets[&input.ticket_id];
        assert_eq!(
            assigned_subject_body(&api, &input.ticket_id, &input.assignment_id)
                .unwrap()
                .as_ref(),
            Some(body)
        );
        authorize_assignment_delivery(&api, body, &input.ticket_id).unwrap();
    }
    assert!(
        execution.take_inputs().is_empty(),
        "fixture setup must not pre-deliver"
    );

    let mut observed = BTreeMap::new();
    let mut per_body = BTreeMap::<String, usize>::new();
    let mut progress = Vec::new();
    for _ in 0..5 {
        // Exercise the actual global discovery -> recipient selection -> scoped
        // attention -> durable reserve/render/admit -> Runtime -> ACK pipeline.
        notification_delivery::recover_ticket_notifications(&api);
        let inputs = execution.take_inputs();
        let request_ids = execution.take_input_request_ids();
        assert_eq!(inputs.len(), request_ids.len());
        for ((worker, content), request_id) in inputs.into_iter().zip(request_ids) {
            let request_id = request_id.expect("recovery must supply a stable request identity");
            let input = expected
                .get(&request_id)
                .expect("unexpected Runtime request");
            let (body, _) = &tickets[&input.ticket_id];
            assert_eq!(
                worker.worker_id.to_string(),
                body.worker_id,
                "cross-Subject delivery"
            );
            assert!(content.contains(&input.assignment_id));
            assert!(content.contains(&input.input_event_id));
            *per_body.entry(body.worker_id.clone()).or_default() += 1;
            assert!(
                observed.insert(request_id, content).is_none(),
                "ACKed input sent twice"
            );
        }
        progress.push((
            *per_body.get(&alpha.worker_id).unwrap_or(&0),
            *per_body.get(&beta.worker_id).unwrap_or(&0),
        ));
    }
    // Before the recipient-scoped scan fix, 26cc5e930's global scan offers all
    // 40, alpha scans/offers beta's 20 then post-filters to empty; beta scans
    // alpha's 20 and also filters to empty. Every recovery repeats (0, 0).
    assert_eq!(
        progress.last(),
        Some(&(20, 20)),
        "both connected Subjects must drain, progress by recovery round: {progress:?}"
    );
    assert_eq!(observed.len(), 40);
    assert!(
        api.store
            .list_pending_ticket_assignment_deliveries(TEST_WORKSPACE_ID, 100)
            .unwrap()
            .is_empty()
    );

    // Inspect actual persisted receipts, not a reimplementation of selection SQL.
    let conn = rusqlite::Connection::open(&api.config.database_path).unwrap();
    let mut acknowledged = BTreeSet::new();
    for (request_id, input) in &expected {
        let (body, subject) = &tickets[&input.ticket_id];
        let (content, source): (String, String) = conn.query_row(
            "SELECT r.content,a.source_json FROM ticket_assignment_delivery_requests r JOIN ticket_assignment_delivery_acknowledgements a ON a.workspace_id=r.workspace_id AND a.delivery_id=r.delivery_id WHERE r.workspace_id=?1 AND r.delivery_id=?2",
            rusqlite::params![TEST_WORKSPACE_ID, input.delivery_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        let source: Value = serde_json::from_str(&source).unwrap();
        assert_eq!(source["subject_id"], *subject);
        assert_eq!(source["worker_id"], body.worker_id);
        assert_eq!(source["runtime_id"], body.runtime_id);
        assert_eq!(
            source["authentication_binding_id"],
            orchestrator_delivery_target(&api, body)
                .unwrap()
                .authentication_binding_id
        );
        assert_eq!(
            &content, &observed[request_id],
            "persisted request bytes differ from Runtime input"
        );
        acknowledged.insert(input.delivery_id.clone());
    }
    assert_eq!(acknowledged.len(), 40);
    let ack_count: usize = conn.query_row(
        "SELECT count(*) FROM ticket_assignment_delivery_acknowledgements WHERE workspace_id=?1",
        [TEST_WORKSPACE_ID], |row| row.get(0),
    ).unwrap();
    assert_eq!(ack_count, 40);

    // Lose the process-local attention cache, then recover repeatedly. SQLite
    // ACKs, not local accepted/pending memory or a fake Runtime deduplicator,
    // must close new-send admission for every delivered input.
    for (body, _) in &recipients {
        *orchestrator_attention_state(&api, body).lock().unwrap() =
            OrchestratorAttentionState::default();
    }
    for _ in 0..3 {
        notification_delivery::recover_ticket_notifications(&api);
        assert!(
            execution.take_inputs().is_empty(),
            "recovery resent ACKed history"
        );
        assert!(execution.take_input_request_ids().is_empty());
    }
}
