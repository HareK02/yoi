//! Public Ticket case authority contracts over isolated SQLite stores.
use rusqlite::{Connection, params};
use std::collections::BTreeMap;
use std::sync::{Arc, Barrier};
use tempfile::TempDir;
use ticket::*;

fn backend(temp: &TempDir) -> SqliteTicketBackend {
    SqliteTicketBackend::open(temp.path().join("tickets.db"), "workspace").unwrap()
}
fn request(ticket: &Ticket, key: &str, action: TicketCaseAction) -> TicketCaseUpdate {
    TicketCaseUpdate {
        expected_content_digest: ticket_content_digest(ticket),
        expected_state: ticket.meta.workflow_state,
        expected_closure_operation_key: ticket.closure_operation_key.clone(),
        operation_key: key.into(),
        action,
        explanation: "A reasoned judgment.\n\nNot an execution milestone.".into(),
        references: vec![TicketReference {
            kind: "result".into(),
            target: "document:example".into(),
        }],
        author: Some("human:alice".into()),
    }
}
fn close(reason: TicketClosureReason) -> TicketCaseAction {
    TicketCaseAction::Close { reason }
}
fn create(backend: &SqliteTicketBackend) -> Ticket {
    let mut input = NewTicket::new("Independent case");
    input.assignee = Some("historical-assignee".into());
    backend.create_with_snapshot(input).unwrap().1
}

#[test]
fn close_reopen_and_correction_preserve_reason_actor_references_and_identity() {
    let temp = TempDir::new().unwrap();
    let store = backend(&temp);
    let initial = create(&store);
    let closed = store
        .update_case(
            &initial.meta.id,
            request(&initial, "close", close(TicketClosureReason::NotPlanned)),
        )
        .unwrap();
    assert!(closed.meta.targets.is_empty());
    assert_eq!(closed.meta.workflow_state, TicketState::Closed);
    let first_judgment = closed.current_closure.clone().unwrap();
    assert_eq!(first_judgment.reason, TicketClosureReason::NotPlanned);
    assert_eq!(first_judgment.author, "human:alice");
    assert_eq!(
        first_judgment.references,
        request(&initial, "close", close(TicketClosureReason::NotPlanned)).references
    );
    let corrected = store
        .update_case(
            &initial.meta.id,
            request(
                &closed,
                "correct",
                TicketCaseAction::CorrectClosure {
                    reason: TicketClosureReason::Duplicate,
                },
            ),
        )
        .unwrap();
    assert_eq!(
        corrected.current_closure.as_ref().unwrap().reason,
        TicketClosureReason::Duplicate
    );
    assert_eq!(corrected.case_history[0], closed.case_history[0]);
    assert_eq!(
        corrected.case_history[1]
            .previous_closure_operation_key
            .as_deref(),
        Some("close")
    );
    let reopened = store
        .update_case(
            &initial.meta.id,
            request(&corrected, "reopen", TicketCaseAction::Reopen),
        )
        .unwrap();
    assert_eq!(reopened.meta.workflow_state, TicketState::Open);
    assert!(reopened.current_closure.is_none());
    assert!(reopened.resolution.is_none());
    assert_eq!(reopened.closure_operation_key.as_deref(), Some("correct"));
    assert_eq!(reopened.case_history.len(), 3);
    assert_eq!(reopened.meta.assignee, initial.meta.assignee);
    assert_eq!(reopened.meta.resource_key, initial.meta.resource_key);
    assert_eq!(
        &reopened.events[..initial.events.len()],
        initial.events.as_slice()
    );
    assert_eq!(
        store.show(initial.meta.id.clone().into()).unwrap(),
        reopened
    );
}

#[test]
fn closure_reasons_round_trip_without_repository_or_merge_request_prerequisites() {
    let temp = TempDir::new().unwrap();
    let store = backend(&temp);
    for reason in [
        TicketClosureReason::Completed,
        TicketClosureReason::NotPlanned,
        TicketClosureReason::Duplicate,
        TicketClosureReason::Unknown,
        TicketClosureReason::Other("Custom decision".into()),
    ] {
        let initial = create(&store);
        let closed = store
            .update_case(
                &initial.meta.id,
                request(&initial, "close", close(reason.clone())),
            )
            .unwrap();
        assert_eq!(closed.current_closure.unwrap().reason, reason);
    }
}

#[test]
fn case_cas_rejects_stale_content_state_and_closure_key_without_writes() {
    let temp = TempDir::new().unwrap();
    let store = backend(&temp);
    let initial = create(&store);
    let base = request(&initial, "close", close(TicketClosureReason::Completed));
    let mut stale = base.clone();
    stale.expected_content_digest = "stale".into();
    assert!(matches!(
        store.update_case(&initial.meta.id, stale),
        Err(TicketError::StaleContent { .. })
    ));
    let mut stale = base.clone();
    stale.expected_state = TicketState::Closed;
    assert!(matches!(
        store.update_case(&initial.meta.id, stale),
        Err(TicketError::StaleWorkflowState { .. })
    ));
    let mut stale = base;
    stale.expected_closure_operation_key = Some("unobserved".into());
    assert!(matches!(
        store.update_case(&initial.meta.id, stale),
        Err(TicketError::StaleClosureOperationKey { .. })
    ));
    assert_eq!(store.show(initial.meta.id.clone().into()).unwrap(), initial);
}

#[test]
fn closure_keys_protect_closed_correction_and_open_closed_open_aba() {
    let temp = TempDir::new().unwrap();
    let store = backend(&temp);
    let initial = create(&store);
    let stale_open = request(
        &initial,
        "late-close",
        close(TicketClosureReason::Completed),
    );
    let closed = store
        .update_case(
            &initial.meta.id,
            request(&initial, "close", close(TicketClosureReason::Unknown)),
        )
        .unwrap();
    let stale_correction = request(
        &closed,
        "late-correct",
        TicketCaseAction::CorrectClosure {
            reason: TicketClosureReason::Completed,
        },
    );
    let corrected = store
        .update_case(
            &initial.meta.id,
            request(
                &closed,
                "correct",
                TicketCaseAction::CorrectClosure {
                    reason: TicketClosureReason::NotPlanned,
                },
            ),
        )
        .unwrap();
    assert!(matches!(
        store.update_case(&initial.meta.id, stale_correction),
        Err(TicketError::StaleClosureOperationKey { .. })
    ));
    let reopened = store
        .update_case(
            &initial.meta.id,
            request(&corrected, "reopen", TicketCaseAction::Reopen),
        )
        .unwrap();
    assert_eq!(
        ticket_content_digest(&reopened),
        ticket_content_digest(&initial)
    );
    assert!(matches!(
        store.update_case(&initial.meta.id, stale_open),
        Err(TicketError::StaleClosureOperationKey { .. })
    ));
    assert_eq!(
        store.show(initial.meta.id.clone().into()).unwrap(),
        reopened
    );
}

#[test]
fn exact_replay_after_edit_reopen_and_backend_restart_does_not_reapply_a_close() {
    let temp = TempDir::new().unwrap();
    let store = backend(&temp);
    let initial = create(&store);
    let operation = request(&initial, "close", close(TicketClosureReason::Completed));
    let closed = store
        .update_case(&initial.meta.id, operation.clone())
        .unwrap();
    let reopened = store
        .update_case(
            &initial.meta.id,
            request(&closed, "reopen", TicketCaseAction::Reopen),
        )
        .unwrap();
    let edited = store
        .edit_item(
            initial.meta.id.clone().into(),
            TicketItemEdit {
                title: Some("Changed requirements".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(edited.case_history, reopened.case_history);
    let restored =
        SqliteTicketBackend::open_verified(temp.path().join("tickets.db"), "workspace").unwrap();
    assert_eq!(
        restored.update_case(&initial.meta.id, operation).unwrap(),
        edited
    );
}

#[test]
fn replay_fingerprint_binds_every_request_field_and_authenticated_source_identity() {
    let temp = TempDir::new().unwrap();
    let store = backend(&temp);
    let initial = create(&store);
    let operation = request(&initial, "close", close(TicketClosureReason::Completed));
    let closed = store
        .update_case(&initial.meta.id, operation.clone())
        .unwrap();
    let mut changed = Vec::new();
    let mut x = operation.clone();
    x.expected_content_digest.push('x');
    changed.push(x);
    let mut x = operation.clone();
    x.expected_state = TicketState::Closed;
    changed.push(x);
    let mut x = operation.clone();
    x.expected_closure_operation_key = Some("close".into());
    changed.push(x);
    let mut x = operation.clone();
    x.action = close(TicketClosureReason::NotPlanned);
    changed.push(x);
    let mut x = operation.clone();
    x.action = TicketCaseAction::Reopen;
    changed.push(x);
    let mut x = operation.clone();
    x.explanation.push('x');
    changed.push(x);
    let mut x = operation.clone();
    x.references.clear();
    changed.push(x);
    let mut x = operation.clone();
    x.author = None;
    changed.push(x);
    for input in changed {
        assert!(matches!(
            store.update_case(&initial.meta.id, input),
            Err(TicketError::OperationFingerprintMismatch { .. })
        ));
    }
    let other_source = store.clone().with_event_attributes(BTreeMap::from([(
        "source_runtime_id".into(),
        "other-runtime".into(),
    )]));
    assert!(matches!(
        other_source.update_case(&initial.meta.id, operation),
        Err(TicketError::OperationFingerprintMismatch { .. })
    ));
    assert_eq!(store.show(initial.meta.id.clone().into()).unwrap(), closed);
}

#[test]
fn invalid_action_does_not_close_correct_or_reopen_the_wrong_case() {
    let temp = TempDir::new().unwrap();
    let store = backend(&temp);
    let initial = create(&store);
    for action in [
        TicketCaseAction::Reopen,
        TicketCaseAction::CorrectClosure {
            reason: TicketClosureReason::Completed,
        },
    ] {
        assert!(
            store
                .update_case(&initial.meta.id, request(&initial, "invalid", action))
                .is_err()
        );
    }
    let closed = store
        .update_case(
            &initial.meta.id,
            request(&initial, "close", close(TicketClosureReason::Unknown)),
        )
        .unwrap();
    assert!(
        store
            .update_case(
                &initial.meta.id,
                request(
                    &closed,
                    "double-close",
                    close(TicketClosureReason::Completed)
                )
            )
            .is_err()
    );
    assert_eq!(store.show(initial.meta.id.clone().into()).unwrap(), closed);
}

#[test]
fn custom_reason_limits_are_utf8_bytes_and_validation_keeps_long_markdown_explanations() {
    let temp = TempDir::new().unwrap();
    let store = backend(&temp);
    let initial = create(&store);
    for reason in [
        "".into(),
        "  \n".into(),
        "bad\0reason".into(),
        "あ".repeat(342),
    ] {
        assert!(
            store
                .update_case(
                    &initial.meta.id,
                    request(&initial, "bad", close(TicketClosureReason::Other(reason)))
                )
                .is_err()
        );
    }
    let mut valid = request(
        &initial,
        "close",
        close(TicketClosureReason::Other("x".repeat(1024))),
    );
    valid.explanation = format!(
        "## Decision\n{}\n<!-- typed SQLite text -->",
        "long".repeat(8000)
    );
    let closed = store.update_case(&initial.meta.id, valid.clone()).unwrap();
    assert_eq!(
        closed.current_closure.as_ref().unwrap().explanation,
        valid.explanation
    );
    assert_eq!(store.update_case(&initial.meta.id, valid).unwrap(), closed);
}

#[test]
fn blank_explanation_actor_reference_or_key_rejects_without_recording_receipts() {
    let temp = TempDir::new().unwrap();
    let store = backend(&temp);
    let initial = create(&store);
    let base = request(&initial, "key", close(TicketClosureReason::Unknown));
    let mut cases = Vec::new();
    let mut x = base.clone();
    x.explanation = "  ".into();
    cases.push(x);
    let mut x = base.clone();
    x.explanation = "bad\0text".into();
    cases.push(x);
    let mut x = base.clone();
    x.author = Some(" ".into());
    cases.push(x);
    let mut x = base.clone();
    x.references[0].target.clear();
    cases.push(x);
    let mut x = base.clone();
    x.references[0].kind.clear();
    cases.push(x);
    let mut x = base;
    x.operation_key.clear();
    cases.push(x);
    for input in cases {
        assert!(store.update_case(&initial.meta.id, input).is_err());
    }
    assert_eq!(store.show(initial.meta.id.clone().into()).unwrap(), initial);
}

#[test]
fn case_history_is_immutable_in_sqlite_and_hook_failure_rolls_back_every_case_write() {
    let temp = TempDir::new().unwrap();
    let store = backend(&temp);
    let initial = create(&store);
    let rejected = store.clone().with_mutation_hook(Arc::new(|_, _| {
        Err(TicketError::Conflict("hook rejected".into()))
    }));
    let operation = request(&initial, "close", close(TicketClosureReason::Unknown));
    assert!(
        rejected
            .update_case(&initial.meta.id, operation.clone())
            .is_err()
    );
    assert_eq!(store.show(initial.meta.id.clone().into()).unwrap(), initial);
    let closed = store.update_case(&initial.meta.id, operation).unwrap();
    let conn = Connection::open(temp.path().join("tickets.db")).unwrap();
    for sql in [
        "UPDATE typed_ticket_case_history SET judgment_json = '{}'",
        "DELETE FROM typed_ticket_case_history",
    ] {
        assert!(conn.execute(sql, []).is_err());
    }
    assert_eq!(store.show(initial.meta.id.clone().into()).unwrap(), closed);
}

#[test]
fn concurrent_exact_replay_creates_one_judgment_and_competing_closes_have_one_winner() {
    let temp = TempDir::new().unwrap();
    let store = backend(&temp);
    let initial = create(&store);
    let barrier = Arc::new(Barrier::new(2));
    let operation = request(&initial, "close", close(TicketClosureReason::Completed));
    let mut joins = Vec::new();
    for _ in 0..2 {
        let store = store.clone();
        let barrier = barrier.clone();
        let operation = operation.clone();
        let id = initial.meta.id.clone();
        joins.push(std::thread::spawn(move || {
            barrier.wait();
            store.update_case(&id, operation)
        }));
    }
    for join in joins {
        join.join().unwrap().unwrap();
    }
    assert_eq!(
        store
            .show(initial.meta.id.clone().into())
            .unwrap()
            .case_history
            .len(),
        1
    );
    let initial = create(&store);
    let barrier = Arc::new(Barrier::new(2));
    let mut joins = Vec::new();
    for key in ["winner-a", "winner-b"] {
        let store = store.clone();
        let barrier = barrier.clone();
        let operation = request(&initial, key, close(TicketClosureReason::Unknown));
        let id = initial.meta.id.clone();
        joins.push(std::thread::spawn(move || {
            barrier.wait();
            store.update_case(&id, operation)
        }));
    }
    let outcomes: Vec<_> = joins.into_iter().map(|j| j.join().unwrap()).collect();
    assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
    assert!(
        outcomes
            .iter()
            .any(|result| matches!(result, Err(TicketError::StaleWorkflowState { .. })))
    );
    assert_eq!(
        store
            .show(initial.meta.id.clone().into())
            .unwrap()
            .case_history
            .len(),
        1
    );
}

#[test]
fn queue_and_readiness_are_routing_audit_and_never_change_case_or_reopen_closed() {
    let temp = TempDir::new().unwrap();
    let store = backend(&temp);
    let initial = create(&store);
    let ready = store
        .mark_ready(
            initial.meta.id.clone().into(),
            TicketMarkReady {
                operation_key: "ready".into(),
                reason: None,
                intake_summary: None,
                author: None,
            },
        )
        .unwrap();
    assert_eq!(ready.meta.workflow_state, TicketState::Open);
    assert_eq!(ready.meta.readiness.as_deref(), Some("ready"));
    store
        .queue_ready(initial.meta.id.clone().into(), "router")
        .unwrap();
    let queued = store.show(initial.meta.id.clone().into()).unwrap();
    assert_eq!(queued.meta.workflow_state, TicketState::Open);
    assert_eq!(queued.meta.queued_by.as_deref(), Some("router"));
    let closed = store
        .update_case(
            &initial.meta.id,
            request(&queued, "close", close(TicketClosureReason::NotPlanned)),
        )
        .unwrap();
    assert!(
        store
            .queue_ready(initial.meta.id.clone().into(), "router")
            .is_err()
    );
    assert_eq!(store.show(initial.meta.id.clone().into()).unwrap(), closed);
}

#[test]
fn workspace_boundary_rejects_other_workspace_ticket_and_shared_operation_dispatch_uses_case_api() {
    let temp = TempDir::new().unwrap();
    let store = backend(&temp);
    let initial = create(&store);
    let other =
        SqliteTicketBackend::open_verified(temp.path().join("tickets.db"), "other-workspace")
            .unwrap();
    let operation = request(&initial, "close", close(TicketClosureReason::Completed));
    assert!(matches!(
        other.update_case(&initial.meta.id, operation.clone()),
        Err(TicketError::NotFound(_))
    ));
    let output = execute_ticket_backend_operation(
        &store,
        TicketBackendOperation::UpdateCase {
            ticket: initial.meta.id.clone(),
            request: operation,
        },
    )
    .unwrap();
    assert!(
        matches!(output, TicketBackendOperationResult::Ticket(ticket) if ticket.meta.workflow_state == TicketState::Closed)
    );
}

#[test]
fn historical_events_and_merge_request_evidence_are_not_rewritten_by_case_judgments() {
    let temp = TempDir::new().unwrap();
    let store = backend(&temp);
    let initial = create(&store);
    let historical = store.clone().with_event_attributes(BTreeMap::from([
        ("operation_key".into(), "old-completion".into()),
        ("merge_request_id".into(), "MR-existing".into()),
    ]));
    let mut event = NewTicketEvent::new(
        TicketEventKind::Other("ticket_completion".into()),
        "Historic completed result",
    );
    event.references = vec![TicketReference {
        kind: "merge_request".into(),
        target: "MR-existing".into(),
    }];
    historical
        .add_event(initial.meta.id.clone().into(), event)
        .unwrap();
    let before = store.show(initial.meta.id.clone().into()).unwrap();
    let after = store
        .update_case(
            &initial.meta.id,
            request(&before, "new-close", close(TicketClosureReason::NotPlanned)),
        )
        .unwrap();
    assert_eq!(
        &after.events[..before.events.len()],
        before.events.as_slice()
    );
    assert!(
        store
            .update_case(
                &initial.meta.id,
                request(
                    &before,
                    "old-completion",
                    close(TicketClosureReason::Completed)
                )
            )
            .is_err()
    );
}

#[test]
fn external_case_json_rejects_six_state_authority_and_round_trips_typed_action() {
    for state in [
        "planning",
        "ready",
        "queued",
        "inprogress",
        "done",
        "in_progress",
    ] {
        assert!(serde_json::from_value::<TicketState>(serde_json::json!(state)).is_err());
    }
    let encoded = serde_json::json!({"action": "correct_closure", "reason": {"other": "Custom"}});
    let action: TicketCaseAction = serde_json::from_value(encoded.clone()).unwrap();
    assert_eq!(serde_json::to_value(action).unwrap(), encoded);
    assert!(
        serde_json::from_value::<TicketCaseAction>(
            serde_json::json!({"action": "reopen", "reason": "completed"})
        )
        .is_err()
    );
}

fn assignment_source() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("source_runtime_id".into(), "runtime-a".into()),
        ("source_worker_id".into(), "worker-a".into()),
        ("source_actor_role".into(), "ticket_worker".into()),
        ("source_assignment_id".into(), "assignment-a".into()),
    ])
}
fn assignment_fixture(temp: &TempDir, id: &str) -> Connection {
    let conn = Connection::open(temp.path().join("tickets.db")).unwrap();
    conn.execute_batch("CREATE TABLE test_worker_assignments (workspace_id TEXT, ticket_id TEXT, runtime_id TEXT, worker_id TEXT, assignment_id TEXT, status TEXT);
        CREATE VIEW ticket_active_worker_assignments AS SELECT workspace_id, ticket_id, runtime_id, worker_id, assignment_id FROM test_worker_assignments WHERE status = 'active';").unwrap();
    conn.execute("INSERT INTO test_worker_assignments VALUES ('workspace', ?1, 'runtime-a', 'worker-a', 'assignment-a', 'active')", params![id]).unwrap();
    conn
}

#[test]
fn assigned_worker_requires_exact_live_identity_but_recorded_replay_survives_release() {
    let temp = TempDir::new().unwrap();
    let store = backend(&temp);
    let initial = create(&store);
    let conn = assignment_fixture(&temp, &initial.meta.id);
    let operation = request(&initial, "close", close(TicketClosureReason::Completed));
    for key in [
        "source_runtime_id",
        "source_worker_id",
        "source_assignment_id",
        "source_actor_role",
    ] {
        let mut wrong = assignment_source();
        wrong.insert(key.into(), "mismatch".into());
        assert!(
            store
                .clone()
                .with_event_attributes(wrong)
                .update_case(&initial.meta.id, operation.clone())
                .is_err()
        );
    }
    for key in [
        "source_runtime_id",
        "source_worker_id",
        "source_assignment_id",
    ] {
        let mut missing = assignment_source();
        missing.remove(key);
        assert!(
            store
                .clone()
                .with_event_attributes(missing)
                .update_case(&initial.meta.id, operation.clone())
                .is_err()
        );
    }
    assert_eq!(store.show(initial.meta.id.clone().into()).unwrap(), initial);
    let source = store.clone().with_event_attributes(assignment_source());
    let closed = source
        .update_case(&initial.meta.id, operation.clone())
        .unwrap();
    // The Ticket domain does not itself release or stop workers. Parent Workspace triggers own this.
    assert_eq!(
        conn.query_row("SELECT status FROM test_worker_assignments", [], |r| r
            .get::<_, String>(
            0
        ))
        .unwrap(),
        "active"
    );
    conn.execute("UPDATE test_worker_assignments SET status = 'released'", [])
        .unwrap();
    let reopened = store
        .update_case(
            &initial.meta.id,
            request(&closed, "reopen", TicketCaseAction::Reopen),
        )
        .unwrap();
    assert_eq!(
        source.update_case(&initial.meta.id, operation).unwrap(),
        reopened
    );
    assert!(
        source
            .update_case(
                &initial.meta.id,
                request(&reopened, "new-close", close(TicketClosureReason::Unknown))
            )
            .is_err()
    );
    assert_eq!(
        conn.query_row("SELECT status FROM test_worker_assignments", [], |r| r
            .get::<_, String>(
            0
        ))
        .unwrap(),
        "released"
    );
}

#[test]
fn content_digest_changes_with_targets_or_requirements_not_case_judgment_history() {
    let temp = TempDir::new().unwrap();
    let store = backend(&temp);
    let initial = create(&store);
    let digest = ticket_content_digest(&initial);
    let closed = store
        .update_case(
            &initial.meta.id,
            request(&initial, "close", close(TicketClosureReason::Unknown)),
        )
        .unwrap();
    assert_eq!(ticket_content_digest(&closed), digest);
    let edited = store
        .edit_item(
            initial.meta.id.clone().into(),
            TicketItemEdit {
                body: Some(MarkdownText::new("New criteria")),
                ..Default::default()
            },
        )
        .unwrap();
    assert_ne!(ticket_content_digest(&edited), digest);
    let restored = store
        .edit_item(
            initial.meta.id.clone().into(),
            TicketItemEdit {
                body: Some(initial.document.body.clone()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(ticket_content_digest(&restored), digest);
    let targeted = store
        .edit_item(
            initial.meta.id.clone().into(),
            TicketItemEdit {
                targets: Some(TicketTargetsEdit::Set {
                    targets: vec![TicketTarget {
                        repository_key: "main".into(),
                        ref_selector: None,
                        access: TicketTargetAccess::ReadOnly,
                    }],
                }),
                ..Default::default()
            },
        )
        .unwrap();
    assert_ne!(ticket_content_digest(&targeted), digest);
}

#[test]
fn external_case_request_requires_an_explicit_nullable_closure_observation() {
    let temp = TempDir::new().unwrap();
    let store = backend(&temp);
    let initial = create(&store);
    let original = request(&initial, "close", close(TicketClosureReason::Unknown));
    let mut json = serde_json::to_value(&original).unwrap();
    assert_eq!(
        serde_json::from_value::<TicketCaseUpdate>(json.clone()).unwrap(),
        original
    );
    json.as_object_mut()
        .unwrap()
        .remove("expected_closure_operation_key");
    assert!(serde_json::from_value::<TicketCaseUpdate>(json).is_err());
}

#[test]
fn conflicting_event_context_rolls_back_case_judgment_instead_of_corrupting_its_receipt() {
    let temp = TempDir::new().unwrap();
    let store = backend(&temp);
    let initial = create(&store);
    let context = store.clone().with_event_attributes(BTreeMap::from([(
        "operation_key".into(),
        "wrong-key".into(),
    )]));
    assert!(
        context
            .update_case(
                &initial.meta.id,
                request(&initial, "close", close(TicketClosureReason::Unknown))
            )
            .is_err()
    );
    assert_eq!(store.show(initial.meta.id.clone().into()).unwrap(), initial);
}
