// Included in store::tests: exercise persisted transaction boundaries, not Runtime I/O.
fn delivery_store_external_event(
    store: &SqliteWorkspaceStore,
    index: i64,
    kind: &str,
    state_field: Option<&str>,
) {
    store.with_conn(|conn| {
        conn.execute("INSERT INTO typed_ticket_events(workspace_id,ticket_id,event_index,event_ref,kind,author,at,state_field,body) VALUES('w','tracking',?1,'tracking:' || ?1,?2,'user','2026-10-12T00:00:00Z',?3,'Answer arrived')",params![index,kind,state_field])?;
        Ok(())
    }).unwrap();
}

fn delivery_store_answer(store: &SqliteWorkspaceStore, key: &str) {
    let current = store
        .list_current_ticket_assignment_tracking("w", "tracking")
        .unwrap()
        .remove(0);
    store
        .answer_ticket_assignment(
            "w",
            "tracking",
            &ticket::TicketAssignmentAnswer {
                assignment_id: current.assignment_id,
                expected_last_event_id: current.last_event_id.unwrap(),
                operation_key: key.into(),
                body: "Explicit continuing answer".into(),
            },
            &delivery_tracking_answer_source(),
            "fixture-actor",
        )
        .unwrap();
}

fn delivery_store_reserve_initial(
    store: &SqliteWorkspaceStore,
    source: &TicketAssignmentTrackingSourceContext,
) -> TicketAssignmentDelivery {
    let inputs = store
        .list_pending_ticket_assignment_deliveries("w", 100)
        .unwrap();
    assert_eq!(inputs.len(), 1);
    store
        .reserve_ticket_assignment_delivery(
            "w",
            &inputs[0].assignment_id,
            &inputs[0].input_event_id,
            source,
        )
        .unwrap()
}

#[tokio::test]
async fn assignment_delivery_waiting_self_reports_do_not_busy_loop_and_answers_are_pending() {
    let (_directory, store) = delivery_tracking_fixture().await;
    // Existing history before Assign must not become a new consideration input.
    delivery_store_external_event(&store, 0, "comment", None);
    delivery_tracking_assign(&store, "a", "assign-a");
    let source = delivery_tracking_source();
    let initial = delivery_store_reserve_initial(&store, &source);
    store
        .ack_ticket_assignment_delivery("w", &initial.delivery_id, &source)
        .unwrap();
    let mut update = tracking_store_input("a", "event-assign-a", "waiting-a");
    update.tracking.status = TicketAssignmentTrackingStatus::Waiting;
    update.tracking.waiting_reason = Some("Waiting for owner answer".into());
    update.tracking.next_action = Some("Read answer and decide".into());
    store
        .update_ticket_assignment_tracking("w", "tracking", &update, &source)
        .unwrap();
    assert!(
        store
            .list_pending_ticket_assignment_deliveries("w", 100)
            .unwrap()
            .is_empty()
    );
    delivery_store_external_event(
        &store,
        1,
        "assignment_tracking",
        Some("assignment_tracking"),
    );
    delivery_store_external_event(&store, 2, "comment", None);
    assert!(
        store
            .list_pending_ticket_assignment_deliveries("w", 100)
            .unwrap()
            .is_empty()
    );
    delivery_store_answer(&store, "answer-after-comment");
    let pending = store
        .list_pending_ticket_assignment_deliveries("w", 100)
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].input_event_id, "ticket-event:3");
    assert_eq!(
        pending[0].waiting_reason.as_deref(),
        Some("Waiting for owner answer")
    );
    assert_eq!(
        pending[0].next_action.as_deref(),
        Some("Read answer and decide")
    );
    let answer = store
        .reserve_ticket_assignment_delivery("w", "a", "ticket-event:3", &source)
        .unwrap();
    store
        .ack_ticket_assignment_delivery("w", &answer.delivery_id, &source)
        .unwrap();
    assert!(
        store
            .list_pending_ticket_assignment_deliveries("w", 100)
            .unwrap()
            .is_empty()
    );
    // Explicit fenced answers remain inputs; own tracking/history and ordinary
    // comments remain excluded.
    delivery_store_external_event(&store, 4, "assignment_tracking_updated", None);
    delivery_store_external_event(&store, 5, "history", None);
    delivery_store_answer(&store, "next-explicit-answer");
    let pending = store
        .list_pending_ticket_assignment_deliveries("w", 100)
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].input_event_id, "ticket-event:6");
    let saved = store
        .get_ticket_assignment_tracking("w", "tracking", "a")
        .unwrap()
        .unwrap();
    assert_eq!(
        saved.tracking.unwrap().status,
        TicketAssignmentTrackingStatus::Waiting
    );
}

#[tokio::test]
async fn assignment_delivery_response_loss_returns_immutable_payload_after_progress_changes_and_reopen()
 {
    let (directory, store) = delivery_tracking_fixture().await;
    delivery_tracking_assign(&store, "a", "assign-a");
    let source = delivery_tracking_source();
    let initial = delivery_store_reserve_initial(&store, &source);
    let mut update = tracking_store_input("a", "event-assign-a", "waiting-a");
    update.tracking.status = TicketAssignmentTrackingStatus::Waiting;
    update.tracking.waiting_reason = Some("New reason not part of original delivery".into());
    store
        .update_ticket_assignment_tracking("w", "tracking", &update, &source)
        .unwrap();
    drop(store);
    let store = SqliteWorkspaceStore::open_for_test(directory.path().join("workspace.db")).unwrap();
    assert_eq!(
        store
            .list_pending_ticket_assignment_deliveries("w", 100)
            .unwrap(),
        vec![initial.clone()]
    );
    assert_eq!(
        store
            .reserve_ticket_assignment_delivery("w", "a", &initial.input_event_id, &source)
            .unwrap(),
        initial
    );
    store
        .ack_ticket_assignment_delivery("w", &initial.delivery_id, &source)
        .unwrap();
    store
        .ack_ticket_assignment_delivery("w", &initial.delivery_id, &source)
        .unwrap();
    assert!(
        store
            .list_pending_ticket_assignment_deliveries("w", 100)
            .unwrap()
            .is_empty()
    );
    store
        .with_conn(|conn| {
            assert!(
                conn.execute(
                    "UPDATE ticket_assignment_deliveries SET payload_json='{}'",
                    []
                )
                .is_err()
            );
            assert!(
                conn.execute(
                    "DELETE FROM ticket_assignment_delivery_acknowledgements",
                    []
                )
                .is_err()
            );
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn assignment_delivery_simultaneous_reservations_converge_to_one_receipt() {
    let (directory, store) = delivery_tracking_fixture().await;
    delivery_tracking_assign(&store, "a", "assign-a");
    let pending = store
        .list_pending_ticket_assignment_deliveries("w", 100)
        .unwrap()
        .remove(0);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let threads = (0..2)
        .map(|_| {
            let barrier = barrier.clone();
            let path = directory.path().join("workspace.db");
            let input = pending.input_event_id.clone();
            std::thread::spawn(move || {
                let store = SqliteWorkspaceStore::open_for_test(path).unwrap();
                barrier.wait();
                store
                    .reserve_ticket_assignment_delivery(
                        "w",
                        "a",
                        &input,
                        &delivery_tracking_source(),
                    )
                    .unwrap()
            })
        })
        .collect::<Vec<_>>();
    let receipts = threads
        .into_iter()
        .map(|t| t.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(receipts[0], receipts[1]);
    store
        .with_conn(|conn| {
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM ticket_assignment_deliveries",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(count, 1);
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn assignment_delivery_done_close_reopen_and_unknown_history_do_not_revive_delegation() {
    for change in ["done", "close-reopen", "unknown"] {
        let (_directory, store) = delivery_tracking_fixture().await;
        delivery_tracking_assign(&store, "a", "assign-a");
        let source = delivery_tracking_source();
        let initial = delivery_store_reserve_initial(&store, &source);
        match change {
            "done" => {
                store
                    .update_ticket_assignment_tracking(
                        "w",
                        "tracking",
                        &tracking_store_input("a", "event-assign-a", "done-a"),
                        &source,
                    )
                    .unwrap();
            }
            "close-reopen" => {
                store.with_conn(|conn| {conn.execute("UPDATE typed_tickets SET workflow_state='closed' WHERE ticket_id='tracking'",[])?;conn.execute("UPDATE typed_tickets SET workflow_state='open' WHERE ticket_id='tracking'",[])?;Ok(())}).unwrap();
            }
            _ => {
                store.with_conn(|conn| {conn.execute("UPDATE ticket_worker_assignments SET tracking_json=NULL,last_tracking_event_id=NULL WHERE assignment_id='a'",[])?;Ok(())}).unwrap();
            }
        }
        delivery_store_external_event(&store, 1, "comment", None);
        assert!(
            store
                .list_pending_ticket_assignment_deliveries("w", 100)
                .unwrap()
                .is_empty(),
            "{change}"
        );
        assert!(
            store
                .reserve_ticket_assignment_delivery("w", "a", &initial.input_event_id, &source)
                .is_err(),
            "{change}"
        );
    }
}

#[tokio::test]
async fn assignment_delivery_clear_reassign_fences_scanned_and_reserved_inputs() {
    let (_directory, store) = delivery_tracking_fixture().await;
    delivery_tracking_assign(&store, "a", "assign-a");
    let source = delivery_tracking_source();
    let old = delivery_store_reserve_initial(&store, &source);
    store
        .clear_current_ticket_role_assignment(
            "w",
            "tracking",
            TicketAssignmentRole::Orchestrator,
            "a",
            "clear-a",
            "clear-a",
            "owner",
            "2",
            None,
        )
        .unwrap();
    let new = delivery_tracking_assign(&store, "b", "assign-b");
    assert_eq!(new.assignment_id, "b");
    assert!(
        store
            .reserve_ticket_assignment_delivery("w", "a", &old.input_event_id, &source)
            .is_err()
    );
    // Acceptance of the previously authorized request is evidence, not permission
    // to resend or transfer work to the new assignment.
    store
        .ack_ticket_assignment_delivery("w", &old.delivery_id, &source)
        .unwrap();
    let pending = store
        .list_pending_ticket_assignment_deliveries("w", 100)
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].assignment_id, "b");
    assert_ne!(pending[0].request_id, old.request_id);
    store
        .with_conn(|conn| {
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM ticket_worker_assignments WHERE role='orchestrator'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(count, 2);
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn assignment_delivery_old_target_cannot_resend_but_can_record_authorized_acceptance() {
    for change in ["replacement", "revoke", "binding-change"] {
        let (_directory, store) = delivery_tracking_fixture().await;
        delivery_tracking_assign(&store, "a", "assign-a");
        let source = delivery_tracking_source();
        let old = delivery_store_reserve_initial(&store, &source);
        if change == "replacement" {
            delivery_tracking_replace_body(&store);
        }
        store.with_conn(|conn| {
            match change {
                "replacement" => {},
                "revoke" => {conn.execute("UPDATE workspace_runtime_bindings SET revoked_at='2' WHERE workspace_id='w'",[])?;},
                _ => {conn.execute("UPDATE workspace_runtime_bindings SET binding_id='new-binding' WHERE workspace_id='w'",[])?;},
            } Ok(())
        }).unwrap();
        assert!(
            matches!(
                store.reserve_ticket_assignment_delivery("w", "a", &old.input_event_id, &source),
                Err(Error::WorkspacePermissionDenied(_))
            ),
            "{change}"
        );
        let mut unreserved = source.clone();
        unreserved.worker_id = "never-authorized-target".into();
        assert!(
            store
                .ack_ticket_assignment_delivery("w", &old.delivery_id, &unreserved)
                .is_err()
        );
        if change == "replacement" {
            let mut replacement = source.clone();
            replacement.worker_id = "replacement".into();
            assert_eq!(
                store
                    .reserve_ticket_assignment_delivery("w", "a", &old.input_event_id, &replacement)
                    .unwrap(),
                old
            );
            store
                .ack_ticket_assignment_delivery("w", &old.delivery_id, &replacement)
                .unwrap();
            assert_eq!(
                store
                    .list_current_ticket_assignment_tracking("w", "tracking")
                    .unwrap()
                    .len(),
                1
            );
        } else {
            store
                .ack_ticket_assignment_delivery("w", &old.delivery_id, &source)
                .unwrap();
        }
    }
}

fn delivery_store_start_reserve(
    store: &SqliteWorkspaceStore,
    key: &str,
    assignment: &str,
    source: &TicketAssignmentTrackingSourceContext,
) -> Result<bool> {
    store.reserve_ticket_assignment_operation_for_orchestrator(
        "w",
        key,
        "tracking",
        "r",
        Some("replacement"),
        "start-input",
        "3",
        assignment,
        source,
    )
}

#[tokio::test]
async fn assignment_startup_clear_reassign_fences_reservation_replay_bind_and_finalize() {
    for change in ["clear", "reassign", "close-reopen"] {
        let (_directory, store) = delivery_tracking_fixture().await;
        delivery_tracking_assign(&store, "a", "assign-a");
        let source = delivery_tracking_source();
        assert!(delivery_store_start_reserve(&store, "start-a", "a", &source).unwrap());
        assert!(!delivery_store_start_reserve(&store, "start-a", "a", &source).unwrap());
        if change == "close-reopen" {
            store.with_conn(|conn| {conn.execute("UPDATE typed_tickets SET workflow_state='closed' WHERE ticket_id='tracking'",[])?;conn.execute("UPDATE typed_tickets SET workflow_state='open' WHERE ticket_id='tracking'",[])?;Ok(())}).unwrap();
        } else {
            // Clear/reassign must not be blocked by a pending startup claim:
            // assignment authority can invalidate it without Runtime side effects.
            store
                .clear_current_ticket_role_assignment(
                    "w",
                    "tracking",
                    TicketAssignmentRole::Orchestrator,
                    "a",
                    "clear-a",
                    "clear-a",
                    "owner",
                    "4",
                    None,
                )
                .unwrap();
            if change == "reassign" {
                delivery_tracking_assign(&store, "b", "assign-b");
            }
        }
        assert!(
            delivery_store_start_reserve(&store, "start-a", "a", &source).is_err(),
            "{change}"
        );
        assert!(
            delivery_store_start_reserve(&store, "start-new", "a", &source).is_err(),
            "{change}"
        );
        assert!(
            store
                .validate_ticket_assignment_operation_fence("w", "start-a")
                .is_err(),
            "{change}"
        );
        assert!(
            store
                .bind_ticket_assignment_operation_worker("w", "start-a", "replacement")
                .is_err(),
            "{change}"
        );
        let record = TicketWorkerAssignmentRecord {
            workspace_id: "w".into(),
            ticket_id: "tracking".into(),
            assignment_id: "target-a".into(),
            worker: RuntimeWorkerRef::new("r", "replacement"),
            assigned_by: "owner".into(),
            assigned_at: "4".into(),
        };
        assert!(
            store
                .set_current_ticket_worker_assignment(
                    &record,
                    None,
                    "worker-event",
                    "start-a",
                    false
                )
                .is_err(),
            "{change}"
        );
        assert!(
            store
                .get_active_ticket_role_assignment("w", "tracking", TicketAssignmentRole::Worker)
                .unwrap()
                .is_none()
        );
    }
}

#[tokio::test]
async fn assignment_startup_replaced_worker_cannot_start_or_replay_saved_operations() {
    let (_directory, store) = delivery_tracking_fixture().await;
    delivery_tracking_assign(&store, "a", "assign-a");
    let source = delivery_tracking_source();
    delivery_store_start_reserve(&store, "start-a", "a", &source).unwrap();
    store
        .with_conn(|conn| {
            conn.execute(
                "UPDATE worker_singleton_owners SET worker_id='replacement' WHERE workspace_id='w'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    assert!(matches!(
        delivery_store_start_reserve(&store, "start-a", "a", &source),
        Err(Error::WorkspacePermissionDenied(_))
    ));
    assert!(matches!(
        delivery_store_start_reserve(&store, "start-new", "a", &source),
        Err(Error::WorkspacePermissionDenied(_))
    ));
    assert!(matches!(
        store.validate_ticket_assignment_operation_fence("w", "start-a"),
        Err(Error::WorkspacePermissionDenied(_))
    ));
    assert!(matches!(
        store.bind_ticket_assignment_operation_worker("w", "start-a", "replacement"),
        Err(Error::WorkspacePermissionDenied(_))
    ));
    // Even the new Worker cannot adopt the old sender's startup operation key.
    let mut replacement = source.clone();
    replacement.worker_id = "replacement".into();
    assert!(delivery_store_start_reserve(&store, "start-a", "a", &replacement).is_err());
}

#[tokio::test]
async fn assignment_startup_saved_fence_cannot_be_bypassed_or_attached_to_manual_receipt() {
    let (_directory, store) = delivery_tracking_fixture().await;
    delivery_tracking_assign(&store, "a", "assign-a");
    let source = delivery_tracking_source();
    delivery_store_start_reserve(&store, "start-a", "a", &source).unwrap();
    assert!(
        store
            .reserve_ticket_assignment_operation(
                "w",
                "start-a",
                "tracking",
                "r",
                Some("replacement"),
                "start-input",
                "3"
            )
            .is_err()
    );
    store.with_conn(|conn| {assert!(conn.execute("UPDATE ticket_assignment_operation_fences SET orchestrator_assignment_id='different'",[]).is_err());Ok(())}).unwrap();
    store
        .fail_ticket_assignment_operation("w", "start-a", "explicit cancellation")
        .unwrap();
    store
        .reserve_ticket_assignment_operation(
            "w",
            "manual",
            "tracking",
            "r",
            Some("replacement"),
            "start-input",
            "3",
        )
        .unwrap();
    assert!(delivery_store_start_reserve(&store, "manual", "a", &source).is_err());
}

#[tokio::test]
async fn assignment_delivery_rendered_content_is_immutable_across_restart_and_concurrent_renderers()
{
    let (directory, store) = delivery_tracking_fixture().await;
    delivery_tracking_assign(&store, "a", "assign-a");
    let source = delivery_tracking_source();
    let initial = delivery_store_reserve_initial(&store, &source);
    assert_eq!(
        store
            .prepare_ticket_assignment_delivery_content(
                "w",
                &initial.delivery_id,
                "original rendered prompt",
                &source
            )
            .unwrap(),
        "original rendered prompt"
    );
    drop(store);
    let store = SqliteWorkspaceStore::open_for_test(directory.path().join("workspace.db")).unwrap();
    assert_eq!(
        store
            .prepare_ticket_assignment_delivery_content(
                "w",
                &initial.delivery_id,
                "different title/handling/config prompt",
                &source
            )
            .unwrap(),
        "original rendered prompt"
    );
    store
        .with_conn(|conn| {
            assert!(
                conn.execute(
                    "UPDATE ticket_assignment_delivery_requests SET content='changed'",
                    []
                )
                .is_err()
            );
            assert!(
                conn.execute("DELETE FROM ticket_assignment_delivery_requests", [])
                    .is_err()
            );
            Ok(())
        })
        .unwrap();
    store
        .clear_current_ticket_role_assignment(
            "w",
            "tracking",
            TicketAssignmentRole::Orchestrator,
            "a",
            "clear-a",
            "clear-a",
            "owner",
            "4",
            None,
        )
        .unwrap();
    assert!(
        store
            .prepare_ticket_assignment_delivery_content(
                "w",
                &initial.delivery_id,
                "original rendered prompt",
                &source
            )
            .is_err()
    );
}

#[test]
fn assignment_delivery_schema92_upgrade_uses_reconciled_migration_evidence_not_legacy_history() {
    use crate::authority::{SqliteWorkspaceAuthority, TicketAuthority};
    use server_api::{TicketQueryRequest, TicketShowRequest};
    use ticket::{SqliteTicketBackend, TicketBackend, TicketIdOrSlug};

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("schema92.db");
    // A genuine retained Ticket-10/Workspace-91 fixture goes through its actual
    // applied tracking migration; no current DB markers or columns are lowered.
    let conn = tracking_schema91_fixture(&path);
    // Public browse needs the resource-key rows a real old Workspace already
    // stored. Seed that old input before migration, never patch its projection.
    let ids: Vec<String> = conn
        .prepare("SELECT ticket_id FROM typed_tickets ORDER BY ticket_id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    for id in ids {
        allocate_resource_key(
            &conn,
            "w",
            WorkspaceResourceKind::Ticket,
            &id,
            "2026-10-10T00:00:00Z",
        )
        .unwrap();
    }
    migrate_ticket_tracking_v91_to_v92(&conn).unwrap();
    assert_eq!(current_schema_version(&conn).unwrap(), 92);
    let assignments = stored_fixture_rows(&conn, "ticket_worker_assignments");
    let operations = stored_fixture_rows(&conn, "ticket_assignment_operations");
    let tracking = stored_fixture_rows(&conn, "typed_ticket_assignment_tracking_history");
    drop(conn);
    let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    let pending = store
        .list_pending_ticket_assignment_deliveries("w", 100)
        .unwrap();
    assert!(
        pending.is_empty(),
        "unmapped legacy recipients never become new Subject execution authority"
    );
    store.with_conn(|conn| {
        let origins: i64 = conn.query_row("SELECT COUNT(*) FROM ticket_assignment_delivery_origins WHERE assignment_id IN ('a-pending','a-progress')", [], |row| row.get(0))?;
        assert_eq!(origins, 2, "reconciled legacy input origins remain evidence");
        Ok(())
    }).unwrap();
    store
        .with_conn(|conn| {
            assert_eq!(current_schema_version(conn)?, LATEST_SCHEMA_VERSION);
            assert_eq!(
                stored_fixture_rows(conn, "ticket_worker_assignments"),
                assignments
            );
            assert_eq!(
                stored_fixture_rows(conn, "ticket_assignment_operations"),
                operations
            );
            assert_eq!(
                stored_fixture_rows(conn, "typed_ticket_assignment_tracking_history"),
                tracking
            );
            let violations: i64 =
                conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| {
                    r.get(0)
                })?;
            assert_eq!(violations, 0);
            Ok(())
        })
        .unwrap();
    drop(store);
    let reopened = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    let authority = SqliteWorkspaceAuthority::new(&path, "w").unwrap();
    let backend = SqliteTicketBackend::open_verified(&path, "w").unwrap();
    let principal = ticket::TicketAssignmentPrincipalSnapshot::WorkspaceAgent {
        agent_key: "workspace-orchestrator".into(),
    };
    for (id, status, legacy_state) in [
        (
            "pending",
            Some(TicketAssignmentTrackingStatus::Waiting),
            "queued",
        ),
        (
            "progress",
            Some(TicketAssignmentTrackingStatus::Inprogress),
            "inprogress",
        ),
        ("closed-reopen", None, "queued"),
        ("historical-done", None, "done"),
    ] {
        let stored = reopened
            .get_ticket_assignment_tracking("w", id, &format!("a-{id}"))
            .unwrap()
            .unwrap();
        assert_eq!(stored.tracking.as_ref().map(|t| t.status), status, "{id}");
        assert_eq!(stored.principal, principal, "{id}");
        assert!(stored.current, "{id}");
        assert!(stored.last_event_id.is_some(), "{id}");
        assert!(stored.migration_diagnostic.is_some(), "{id}");
        let detail = authority.ticket(id).unwrap();
        assert_eq!(detail.assignment_tracking, vec![stored], "{id}");
        assert_eq!(
            authority.ticket_assignment_tracking(id).unwrap(),
            detail.assignment_tracking,
            "{id}"
        );
        let shown = authority
            .show_ticket(id, TicketShowRequest::default())
            .unwrap();
        assert_eq!(
            shown.assignment_tracking, detail.assignment_tracking,
            "{id}"
        );
        let saved_ticket = backend.show(TicketIdOrSlug::Id(id.into())).unwrap();
        assert_eq!(
            detail.state.as_str(),
            saved_ticket.meta.workflow_state.as_str(),
            "{id}"
        );
        assert_eq!(detail.closure_history, saved_ticket.case_history, "{id}");
        assert_eq!(
            detail.legacy_handoff, saved_ticket.legacy_workflow_snapshot,
            "{id}"
        );
        assert_eq!(
            detail.legacy_handoff.as_ref().unwrap().workflow_state,
            legacy_state,
            "{id}"
        );
        assert_eq!(shown.closure_history, detail.closure_history, "{id}");
        assert_eq!(shown.legacy_handoff, detail.legacy_handoff, "{id}");
        assert_eq!(
            detail.state.as_str(),
            if id == "historical-done" {
                "closed"
            } else {
                "open"
            },
            "{id}"
        );
        if id == "closed-reopen" {
            assert_eq!(detail.closure_history.len(), 2);
            assert_eq!(
                detail.closure_history[0].action,
                ticket::TicketCaseAction::Close {
                    reason: ticket::TicketClosureReason::Unknown
                }
            );
            assert_eq!(
                detail.closure_history[1].action,
                ticket::TicketCaseAction::Reopen
            );
        }
    }
    let assigned = authority
        .query_tickets(TicketQueryRequest {
            assigned_principal: Some(principal.clone()),
            limit: Some(100),
            ..Default::default()
        })
        .unwrap();
    let mut assigned_ids = assigned
        .items
        .iter()
        .map(|item| item.id.as_str())
        .collect::<Vec<_>>();
    assigned_ids.sort_unstable();
    assert_eq!(
        assigned_ids,
        vec![
            "closed-reopen",
            "historical-done",
            "invalid-fence",
            "no-snapshot",
            "pending",
            "progress",
            "replaced"
        ]
    );
    assert!(!assigned.page.has_more);
    for item in &assigned.items {
        assert_eq!(
            item.assignment_tracking,
            reopened
                .list_current_ticket_assignment_tracking("w", &item.id)
                .unwrap(),
            "{}",
            item.id
        );
        let detail = authority.ticket(&item.id).unwrap();
        assert_eq!(item.legacy_handoff, detail.legacy_handoff, "{}", item.id);
        assert_eq!(item.state, detail.state, "{}", item.id);
    }
    for (status, expected) in [
        (TicketAssignmentTrackingStatus::Waiting, vec!["pending"]),
        (TicketAssignmentTrackingStatus::Inprogress, vec!["progress"]),
        (TicketAssignmentTrackingStatus::Done, vec![]),
    ] {
        let filtered = authority
            .query_tickets(TicketQueryRequest {
                assigned_principal: Some(principal.clone()),
                handling_status: Some(status),
                limit: Some(1),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            filtered
                .items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            expected,
            "{status:?}"
        );
        assert!(!filtered.page.has_more, "{status:?}");
        assert!(filtered.page.next_cursor.is_none(), "{status:?}");
    }
    assert!(
        authority
            .query_tickets(TicketQueryRequest {
                assigned_principal: Some(
                    ticket::TicketAssignmentPrincipalSnapshot::WorkspaceAgent {
                        agent_key: "other-agent".into(),
                    }
                ),
                limit: Some(1),
                ..Default::default()
            })
            .unwrap()
            .items
            .is_empty()
    );
    assert_eq!(
        reopened
            .list_pending_ticket_assignment_deliveries("w", 100)
            .unwrap(),
        pending
    );
}

#[tokio::test]
async fn assignment_startup_simultaneous_reservations_admit_runtime_effects_once() {
    let (directory, store) = delivery_tracking_fixture().await;
    delivery_tracking_assign(&store, "a", "assign-a");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let threads = (0..2)
        .map(|_| {
            let barrier = barrier.clone();
            let path = directory.path().join("workspace.db");
            std::thread::spawn(move || {
                let store = SqliteWorkspaceStore::open_for_test(path).unwrap();
                barrier.wait();
                delivery_store_start_reserve(&store, "start-a", "a", &delivery_tracking_source())
                    .unwrap()
            })
        })
        .collect::<Vec<_>>();
    let admitted = threads
        .into_iter()
        .map(|t| t.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(admitted.iter().filter(|result| **result).count(), 1);
    store
        .with_conn(|conn| {
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM ticket_assignment_operation_fences",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(count, 1);
            Ok(())
        })
        .unwrap();
}

#[test]
fn assignment_delivery_fresh_and_upgraded_operational_schemas_are_identical() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("retained.db");
    let conn = tracking_schema91_fixture(&path);
    migrate_ticket_tracking_v91_to_v92(&conn).unwrap();
    drop(conn);
    let upgraded = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    let fresh = SqliteWorkspaceStore::in_memory_for_test().unwrap();
    let objects = |store: &SqliteWorkspaceStore| {
        store.with_conn(|conn| {
        let mut statement=conn.prepare("SELECT type,name,sql FROM sqlite_schema WHERE name LIKE 'ticket_assignment_delivery_%' OR name LIKE 'ticket_assignment_operation_fence%' ORDER BY name")?;
        Ok(statement.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()?)
    }).unwrap()
    };
    assert_eq!(objects(&fresh), objects(&upgraded));
}

#[tokio::test]
async fn assignment_delivery_missing_immutable_request_guard_refuses_startup() {
    let (directory, store) = delivery_tracking_fixture().await;
    store
        .with_conn(|conn| {
            conn.execute(
                "DROP TRIGGER ticket_assignment_delivery_request_no_update",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    drop(store);
    assert!(
        matches!(SqliteWorkspaceStore::open_for_test(directory.path().join("workspace.db")),Err(Error::Store(message)) if message.contains("ticket_assignment_delivery_request_no_update"))
    );
}

#[tokio::test]
async fn assignment_delivery_fabricated_inputs_and_foreign_workspace_cannot_create_receipts() {
    let (_directory, store) = delivery_tracking_fixture().await;
    delivery_tracking_assign(&store, "a", "assign-a");
    let source = delivery_tracking_source();
    assert!(
        store
            .reserve_ticket_assignment_delivery("w", "a", "ticket-event:9999", &source)
            .is_err()
    );
    assert!(
        store
            .reserve_ticket_assignment_delivery(
                "other",
                "a",
                "assignment-event:event-assign-a",
                &source
            )
            .is_err()
    );
    store
        .with_conn(|conn| {
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM ticket_assignment_deliveries",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(count, 0);
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn assignment_startup_unfinished_dependencies_remain_diagnostics_not_store_admission_gates() {
    let (_directory, store) = manual_worker_start_store().await;
    seed_manual_start_ticket(&store, "partial", "open");
    seed_manual_start_ticket(&store, "dependency", "open");
    store.with_conn(|conn| {
        conn.execute("INSERT INTO typed_ticket_relations(workspace_id,ticket_id,kind,target,author,at) VALUES('w','partial','depends_on','dependency','owner','1')",[])?;
        Ok(())
    }).unwrap();
    let assignment = manual_worker_assignment("partial", "partial-assignment");
    store
        .start_ticket_with_worker_assignment(
            &assignment,
            "partial-event",
            "partial-operation",
            None,
        )
        .unwrap();
    assert!(
        store
            .get_active_ticket_role_assignment("w", "partial", TicketAssignmentRole::Worker)
            .unwrap()
            .is_some()
    );
    assert!(
        store
            .get_active_ticket_role_assignment("w", "dependency", TicketAssignmentRole::Worker)
            .unwrap()
            .is_none()
    );
    store
        .with_conn(|conn| {
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM typed_ticket_relations WHERE kind='depends_on'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(count, 1);
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn assignment_diagnostics_keep_persisted_notification_acceptance_and_history_distinct_without_payload()
 {
    let (_dir, store) = delivery_tracking_fixture().await;
    delivery_tracking_assign(&store, "a", "assign-a");
    let initial = store
        .list_pending_ticket_assignment_deliveries("w", 10)
        .unwrap()
        .remove(0);
    let page = store
        .page_ticket_assignment_diagnostics("w", "tracking", "a", None, None, 1)
        .unwrap();
    assert!(page.current && page.actionable && page.input_origin_known);
    assert_eq!(
        page.inputs[0].notification,
        server_api::TicketAssignmentInputDeliveryState::NotReserved
    );
    let initial = store
        .reserve_ticket_assignment_delivery(
            "w",
            "a",
            &initial.input_event_id,
            &delivery_tracking_source(),
        )
        .unwrap();
    assert_eq!(
        store
            .page_ticket_assignment_diagnostics("w", "tracking", "a", None, None, 1)
            .unwrap()
            .inputs[0]
            .notification,
        server_api::TicketAssignmentInputDeliveryState::Reserved
    );
    store
        .prepare_ticket_assignment_delivery_content(
            "w",
            &initial.delivery_id,
            "SECRET RENDERED PAYLOAD",
            &delivery_tracking_source(),
        )
        .unwrap();
    let pending = store
        .page_ticket_assignment_diagnostics("w", "tracking", "a", None, None, 1)
        .unwrap();
    assert_eq!(
        pending.inputs[0].notification,
        server_api::TicketAssignmentInputDeliveryState::RequestPersisted
    );
    assert!(pending.inputs[0].acceptance_outcome_unknown);
    store
        .ack_ticket_assignment_delivery("w", &initial.delivery_id, &delivery_tracking_source())
        .unwrap();
    delivery_store_answer(&store, "diagnostic-answer");
    delivery_store_external_event(&store, 1, "history", None);
    let accepted = store
        .page_ticket_assignment_diagnostics("w", "tracking", "a", None, None, 1)
        .unwrap();
    assert_eq!(
        accepted.inputs[0].notification,
        server_api::TicketAssignmentInputDeliveryState::RuntimeAccepted
    );
    assert!(!accepted.inputs[0].acceptance_outcome_unknown);
    assert_eq!(
        accepted.next_input_event_id.as_deref(),
        Some(initial.input_event_id.as_str())
    );
    let next = store
        .page_ticket_assignment_diagnostics(
            "w",
            "tracking",
            "a",
            accepted.next_input_event_id.as_deref(),
            None,
            1,
        )
        .unwrap();
    assert_eq!(next.inputs[0].input_event_id, "ticket-event:0");
    assert!(next.next_input_event_id.is_none());
    let wire = serde_json::to_string(&accepted).unwrap();
    for forbidden in [
        "SECRET",
        "tracking-binding",
        "authenticated-worker",
        "payload_json",
        "source_json",
        "content",
    ] {
        assert!(!wire.contains(forbidden), "{wire}");
    }
    store
        .update_ticket_assignment_tracking(
            "w",
            "tracking",
            &tracking_store_input("a", "event-assign-a", "done"),
            &delivery_tracking_source(),
        )
        .unwrap();
    let history = store
        .page_ticket_assignment_diagnostics("w", "tracking", "a", None, None, 100)
        .unwrap();
    assert!(history.current && !history.actionable);
    assert_eq!(history.inputs.len(), 2); // retained inputs are history, not fresh work
    assert_eq!(
        store
            .page_ticket_assignment_diagnostics(
                "w",
                "tracking",
                "a",
                accepted.next_input_event_id.as_deref(),
                None,
                100
            )
            .unwrap()
            .inputs[0]
            .input_event_id,
        "ticket-event:0"
    );
    assert!(
        store
            .page_ticket_assignment_diagnostics(
                "w",
                "tracking",
                "a",
                Some("ticket-event:999"),
                None,
                1
            )
            .is_err()
    );
    assert!(
        store
            .page_ticket_assignment_diagnostics("other", "tracking", "a", None, None, 1)
            .is_err()
    );
}

#[tokio::test]
async fn assignment_diagnostics_unknown_retained_relationship_does_not_invent_input_or_acceptance()
{
    // Seed the relationship in its genuine pre-Subject schema, not by bypassing
    // current guards against creating a new legacy assignment.
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("legacy-unknown.db");
    drop(tracking_schema91_fixture(&path));
    let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    let page = store
        .page_ticket_assignment_diagnostics("w", "no-snapshot", "a-no-snapshot", None, None, 100)
        .unwrap();
    assert!(page.current);
    assert!(!page.actionable && !page.input_origin_known);
    assert!(page.inputs.is_empty() && page.starts.is_empty());
    assert!(
        store
            .list_pending_ticket_assignment_deliveries("w", 100)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn assignment_diagnostic_start_cursor_pages_existing_claims_without_input_causality_or_processing_inference()
 {
    let (_dir, store) = delivery_tracking_fixture().await;
    delivery_tracking_assign(&store, "a", "assign-a");
    let source = delivery_tracking_source();
    delivery_store_start_reserve(&store, "z-first-start", "a", &source).unwrap();
    store
        .fail_ticket_assignment_operation("w", "z-first-start", "External resource refused start")
        .unwrap();
    delivery_store_start_reserve(&store, "a-second-start", "a", &source).unwrap();
    let first = store
        .page_ticket_assignment_diagnostics("w", "tracking", "a", None, None, 1)
        .unwrap();
    assert_eq!(first.starts.len(), 1);
    assert_eq!(first.starts[0].operation_id, "z-first-start");
    assert_eq!(first.starts[0].claim_state, "failed");
    assert!(first.starts[0].actual_processing_unknown);
    assert_eq!(
        first.next_start_operation_id.as_deref(),
        Some("z-first-start")
    );
    let second = store
        .page_ticket_assignment_diagnostics(
            "w",
            "tracking",
            "a",
            None,
            first.next_start_operation_id.as_deref(),
            1,
        )
        .unwrap();
    assert_eq!(second.starts[0].operation_id, "a-second-start");
    assert_eq!(second.starts[0].claim_state, "pending");
    assert!(second.starts[0].actual_processing_unknown);
    assert!(second.next_start_operation_id.is_none());
    assert!(
        store
            .page_ticket_assignment_diagnostics(
                "w",
                "tracking",
                "a",
                None,
                Some("foreign-operation"),
                1
            )
            .is_err()
    );
    let wire = serde_json::to_string(&first).unwrap();
    assert!(!wire.contains("tracking-binding") && !wire.contains("External resource"));
}

#[tokio::test]
async fn assignment_content_edits_are_atomic_durable_inputs_without_appending_posts() {
    use ticket::TicketBackend;
    let (directory, store) = delivery_tracking_fixture().await;
    delivery_tracking_assign(&store, "a", "assign-a");
    let source = delivery_tracking_source();
    let initial = delivery_store_reserve_initial(&store, &source);
    store
        .ack_ticket_assignment_delivery("w", &initial.delivery_id, &source)
        .unwrap();
    let path = directory.path().join("workspace.db");
    let backend = ticket::SqliteTicketBackend::open_verified(&path, "w").unwrap();
    let original = backend.show("tracking".into()).unwrap();
    backend
        .edit_item(
            "tracking".into(),
            ticket::TicketItemEdit {
                title: Some("Changed title".into()),
                ..Default::default()
            },
        )
        .unwrap();
    backend
        .edit_item(
            "tracking".into(),
            ticket::TicketItemEdit {
                title: Some("Changed title".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let mut latest = backend.show("tracking".into()).unwrap();
    for body in ["Changed requirements", "Final requirements"] {
        backend
            .edit_post(
                "tracking".into(),
                ticket::TicketPostEdit {
                    event_ref: latest.events[0].event_ref.clone(),
                    expected_body: latest.document.body.clone(),
                    body: ticket::MarkdownText::new(body),
                    author: "fixture-editor".into(),
                },
            )
            .unwrap();
        latest = backend.show("tracking".into()).unwrap();
    }
    assert_eq!(latest.events.len(), original.events.len());
    assert_eq!(latest.events[0].event_ref, original.events[0].event_ref);
    backend
        .add_event(
            "tracking".into(),
            ticket::NewTicketEvent::new(ticket::TicketEventKind::Comment, "Discussion"),
        )
        .unwrap();
    let comment = backend
        .show("tracking".into())
        .unwrap()
        .events
        .last()
        .unwrap()
        .clone();
    backend
        .edit_post(
            "tracking".into(),
            ticket::TicketPostEdit {
                event_ref: comment.event_ref,
                expected_body: comment.body,
                body: ticket::MarkdownText::new("Edited discussion"),
                author: "fixture-editor".into(),
            },
        )
        .unwrap();
    // Input insertion participates in the content write, including rollback.
    store
        .with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute(
                "UPDATE typed_tickets SET title='Rolled back' WHERE ticket_id='tracking'",
                [],
            )?;
            tx.rollback()?;
            assert!(
                conn.execute(
                    "UPDATE ticket_assignment_content_inputs SET assignment_id='other'",
                    []
                )
                .is_err()
            );
            assert!(
                conn.execute("DELETE FROM ticket_assignment_content_inputs", [])
                    .is_err()
            );
            Ok(())
        })
        .unwrap();
    let pending = store
        .list_pending_ticket_assignment_deliveries("w", 100)
        .unwrap();
    assert_eq!(
        pending.len(),
        3,
        "no-op and discussion edits are not new request inputs"
    );
    assert!(
        pending
            .iter()
            .all(|input| input.input_event_id.starts_with("ticket-edit:"))
    );
    let mut cursor = None;
    let mut diagnostics = Vec::new();
    loop {
        let page = store
            .page_ticket_assignment_diagnostics("w", "tracking", "a", cursor.as_deref(), None, 1)
            .unwrap();
        diagnostics.extend(page.inputs.into_iter().map(|input| input.input_event_id));
        cursor = page.next_input_event_id;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(
        diagnostics,
        std::iter::once(initial.input_event_id.clone())
            .chain(pending.iter().map(|input| input.input_event_id.clone()))
            .collect::<Vec<_>>()
    );
    drop(store);
    let reopened = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    assert_eq!(
        reopened
            .list_pending_ticket_assignment_deliveries("w", 100)
            .unwrap(),
        pending
    );
    for input in pending {
        let reserved = reopened
            .reserve_ticket_assignment_delivery("w", "a", &input.input_event_id, &source)
            .unwrap();
        assert_eq!(reserved, input);
        reopened
            .ack_ticket_assignment_delivery("w", &reserved.delivery_id, &source)
            .unwrap();
    }
    assert!(
        reopened
            .list_pending_ticket_assignment_deliveries("w", 100)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn assignment_content_edits_never_revive_done_closed_or_reissued_delegations() {
    for change in ["done", "close-reopen", "reissue"] {
        let (directory, store) = delivery_tracking_fixture().await;
        delivery_tracking_assign(&store, "a", "assign-a");
        let source = delivery_tracking_source();
        let initial = delivery_store_reserve_initial(&store, &source);
        store
            .ack_ticket_assignment_delivery("w", &initial.delivery_id, &source)
            .unwrap();
        use ticket::TicketBackend;
        let backend =
            ticket::SqliteTicketBackend::open_verified(directory.path().join("workspace.db"), "w")
                .unwrap();
        backend
            .edit_item(
                "tracking".into(),
                ticket::TicketItemEdit {
                    title: Some("Input before authority change".into()),
                    author: Some("fixture-editor".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        let old = store
            .list_pending_ticket_assignment_deliveries("w", 100)
            .unwrap()
            .remove(0);
        match change {
            "done" => {
                store
                    .update_ticket_assignment_tracking(
                        "w",
                        "tracking",
                        &tracking_store_input("a", "event-assign-a", "done-a"),
                        &source,
                    )
                    .unwrap();
            }
            "close-reopen" => {
                store.with_conn(|conn| { conn.execute_batch("UPDATE typed_tickets SET workflow_state='closed' WHERE ticket_id='tracking'; UPDATE typed_tickets SET workflow_state='open' WHERE ticket_id='tracking';")?; Ok(()) }).unwrap();
            }
            _ => {
                delivery_tracking_assign(&store, "b", "assign-b");
            }
        }
        backend
            .edit_item(
                "tracking".into(),
                ticket::TicketItemEdit {
                    body: Some(ticket::MarkdownText::new("New requirements")),
                    author: Some("fixture-editor".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        let pending = store
            .list_pending_ticket_assignment_deliveries("w", 100)
            .unwrap();
        if change == "reissue" {
            assert_eq!(pending.len(), 2, "new delegation and its content edit only");
            assert!(pending.iter().all(|input| input.assignment_id == "b"));
        } else {
            assert!(pending.is_empty(), "{change}");
        }
        assert!(
            store
                .reserve_ticket_assignment_delivery("w", "a", &old.input_event_id, &source)
                .is_err()
        );
        let page = store
            .page_ticket_assignment_diagnostics("w", "tracking", "a", None, None, 100)
            .unwrap();
        assert!(!page.actionable);
        assert_eq!(
            page.inputs.len(),
            2,
            "retain old input evidence without creating new authority"
        );
    }
}

#[tokio::test]
async fn assignment_content_input_missing_guards_refuse_startup() {
    for trigger in [
        "ticket_assignment_title_input",
        "ticket_assignment_initial_post_input",
        "ticket_assignment_content_input_no_update",
        "ticket_assignment_content_input_no_delete",
    ] {
        let (directory, store) = delivery_tracking_fixture().await;
        store
            .with_conn(|conn| {
                conn.execute_batch(&format!("DROP TRIGGER {trigger}"))?;
                Ok(())
            })
            .unwrap();
        drop(store);
        let error = match SqliteWorkspaceStore::open_for_test(directory.path().join("workspace.db"))
        {
            Ok(_) => panic!("missing {trigger} was accepted"),
            Err(error) => error,
        };
        assert!(error.to_string().contains(trigger), "{error}");
    }
}

// Subject bodies use the existing Workspace singleton/Runtime binding tables. No
// Workspace-global orchestrator owner is created by this fixture.
async fn subject_notification_store() -> (tempfile::TempDir, SqliteWorkspaceStore) {
    let (directory, store) = manual_worker_start_store().await;
    for ticket in ["subject-ticket-a", "subject-ticket-b"] {
        seed_manual_start_ticket(&store, ticket, "open");
    }
    store.with_conn(|conn| {
        conn.execute("INSERT INTO workspace_runtime_bindings(workspace_id,runtime_id,display_name,base_url,public_key,public_key_fingerprint,binding_id,authentication_mode,created_at,updated_at) VALUES('w','r','Runtime','https://runtime.invalid','key','fingerprint','subject-binding','workspace_identity','1','1')", [])?;
        for worker in ["body-b", "body-replacement"] {
            conn.execute("INSERT INTO worker_registry(workspace_id,runtime_id,worker_id,display_name,retention_state,created_at,updated_at) VALUES('w','r',?1,?1,'normal','1','1')", [worker])?;
        }
        for (subject, worker) in [("subject-a", "worker"), ("subject-b", "body-b")] {
            let key = crate::subjektiv::subject_worker_singleton_key(subject).unwrap();
            conn.execute("INSERT INTO worker_singleton_owners(workspace_id,singleton_key,runtime_id,worker_id,generation,created_at,updated_at) VALUES('w',?1,'r',?2,1,'1','1')", params![key, worker])?;
            conn.execute("INSERT INTO worker_create_reservations(workspace_id,allocation_key,worker_id,runtime_id,create_fingerprint,state,created_at,updated_at,singleton_key,singleton_generation) VALUES('w',?1,?2,'r','fixture','created','1','1',?3,1)", params![format!("allocation-{subject}"), worker, key])?;
        }
        Ok(())
    }).unwrap();
    (directory, store)
}

fn subject_notification_replace_body(store: &SqliteWorkspaceStore, subject: &str) {
    store.with_conn(|conn| {
        let key = crate::subjektiv::subject_worker_singleton_key(subject).unwrap();
        conn.execute("UPDATE worker_create_reservations SET state='removed' WHERE workspace_id='w' AND singleton_key=?1", [&key])?;
        conn.execute("UPDATE worker_singleton_owners SET worker_id='body-replacement',generation=2 WHERE workspace_id='w' AND singleton_key=?1", [&key])?;
        conn.execute("INSERT INTO worker_create_reservations(workspace_id,allocation_key,worker_id,runtime_id,create_fingerprint,state,created_at,updated_at,singleton_key,singleton_generation) VALUES('w','replacement-allocation','body-replacement','r','fixture','created','2','2',?1,2)", [&key])?;
        Ok(())
    }).unwrap();
}

fn subject_notification_source(
    subject: &str,
    worker: &str,
) -> TicketAssignmentTrackingSourceContext {
    TicketAssignmentTrackingSourceContext {
        actor: format!("authenticated-worker:r/{worker}"),
        runtime_id: "r".into(),
        worker_id: worker.into(),
        // Current source identity is the Subject, independent of the body Worker.
        subject_id: subject.into(),
        authentication_binding_id: "subject-binding".into(),
    }
}

fn subject_notification_assign(
    store: &SqliteWorkspaceStore,
    ticket: &str,
    id: &str,
    subject: &str,
) {
    let current = store
        .get_current_ticket_role_assignment("w", ticket, TicketAssignmentRole::Orchestrator)
        .unwrap();
    store
        .set_current_ticket_role_assignment(
            &TicketRoleAssignmentRecord {
                role: TicketAssignmentRole::Orchestrator,
                principal: TicketAssignmentPrincipal::Subject {
                    subject_id: subject.into(),
                },
                ..manual_worker_assignment(ticket, id)
            },
            current.as_ref().map(|row| row.assignment_id.as_str()),
            &format!("event-{id}"),
            &format!("operation-{id}"),
            true,
        )
        .unwrap();
}

#[tokio::test]
async fn subject_assignment_delivery_fences_subject_target_and_current_body_before_receipt_replay()
{
    let (directory, store) = subject_notification_store().await;
    subject_notification_assign(&store, "subject-ticket-a", "assigned-a", "subject-a");
    subject_notification_assign(&store, "subject-ticket-b", "assigned-b", "subject-b");
    let a = subject_notification_source("subject-a", "worker");
    let b = subject_notification_source("subject-b", "body-b");
    let pending = store
        .list_pending_ticket_assignment_deliveries("w", 100)
        .unwrap();
    assert_eq!(pending.len(), 2);
    let input_a = pending
        .iter()
        .find(|input| input.assignment_id == "assigned-a")
        .unwrap();
    let input_b = pending
        .iter()
        .find(|input| input.assignment_id == "assigned-b")
        .unwrap();
    // B is a valid live body, but neither initial reservation nor replay grants A's input.
    assert!(matches!(
        store.reserve_ticket_assignment_delivery("w", "assigned-a", &input_a.input_event_id, &b),
        Err(Error::WorkspacePermissionDenied(_))
    ));
    let reserved = store
        .reserve_ticket_assignment_delivery("w", "assigned-a", &input_a.input_event_id, &a)
        .unwrap();
    assert_eq!(
        store
            .prepare_ticket_assignment_delivery_content(
                "w",
                &reserved.delivery_id,
                "original A bytes",
                &a
            )
            .unwrap(),
        "original A bytes"
    );
    assert!(matches!(
        store.reserve_ticket_assignment_delivery("w", "assigned-a", &input_a.input_event_id, &b),
        Err(Error::WorkspacePermissionDenied(_))
    ));
    assert!(matches!(
        store.prepare_ticket_assignment_delivery_content("w", &reserved.delivery_id, "B bytes", &b),
        Err(Error::WorkspacePermissionDenied(_))
    ));
    let reserved_b = store
        .reserve_ticket_assignment_delivery("w", "assigned-b", &input_b.input_event_id, &b)
        .unwrap();
    assert_ne!(reserved_b.delivery_id, reserved.delivery_id);
    subject_notification_replace_body(&store, "subject-a");
    assert!(matches!(
        store.prepare_ticket_assignment_delivery_content(
            "w",
            &reserved.delivery_id,
            "old body retry",
            &a
        ),
        Err(Error::WorkspacePermissionDenied(_))
    ));
    let replacement = subject_notification_source("subject-a", "body-replacement");
    assert_eq!(
        store
            .prepare_ticket_assignment_delivery_content(
                "w",
                &reserved.delivery_id,
                "new body render",
                &replacement
            )
            .unwrap(),
        "original A bytes"
    );
    // A late ACK from the formerly authorized body is retained as acceptance evidence,
    // not rejected or interpreted as authorization to send again.
    store
        .ack_ticket_assignment_delivery("w", &reserved.delivery_id, &a)
        .unwrap();
    assert!(
        store
            .ack_ticket_assignment_delivery("w", &reserved.delivery_id, &b)
            .is_err()
    );
    subject_notification_assign(&store, "subject-ticket-a", "reassigned-b", "subject-b");
    assert!(
        store
            .prepare_ticket_assignment_delivery_content(
                "w",
                &reserved.delivery_id,
                "historical replay",
                &replacement
            )
            .is_err()
    );
    drop(store);
    let store = SqliteWorkspaceStore::open_for_test(directory.path().join("workspace.db")).unwrap();
    let page = store
        .page_ticket_assignment_diagnostics("w", "subject-ticket-a", "assigned-a", None, None, 100)
        .unwrap();
    assert!(!page.current);
    assert!(!page.actionable);
    assert!(page.inputs.iter().any(|input| input.notification
        == server_api::TicketAssignmentInputDeliveryState::RuntimeAccepted));
    assert!(
        !store
            .list_pending_ticket_assignment_deliveries("w", 100)
            .unwrap()
            .iter()
            .any(|input| input.assignment_id == "assigned-a")
    );
}

// Delivery contract fixtures are separate from the retained legacy tracking/migration
// fixtures: new operational input requires an assigned Subject and a live body lease.
fn delivery_tracking_source() -> TicketAssignmentTrackingSourceContext {
    TicketAssignmentTrackingSourceContext {
        actor: "authenticated-worker:r/worker".into(),
        runtime_id: "r".into(),
        worker_id: "worker".into(),
        subject_id: "subject-tracking".into(),
        authentication_binding_id: "tracking-binding".into(),
    }
}

fn delivery_tracking_answer_source() -> TicketOrchestratorAssignmentSourceContext {
    let source = delivery_tracking_source();
    TicketOrchestratorAssignmentSourceContext::Runtime {
        runtime_id: source.runtime_id,
        worker_id: source.worker_id,
        agent_key: source.subject_id,
        authentication_binding_id: source.authentication_binding_id,
    }
}

async fn delivery_tracking_fixture() -> (tempfile::TempDir, SqliteWorkspaceStore) {
    let (directory, store) = manual_worker_start_store().await;
    seed_manual_start_ticket(&store, "tracking", "open");
    store.with_conn(|conn| {
        conn.execute("INSERT INTO workspace_runtime_bindings(workspace_id,runtime_id,display_name,base_url,public_key,public_key_fingerprint,binding_id,authentication_mode,created_at,updated_at) VALUES('w','r','Runtime','https://runtime.invalid','verified-fixture-key','verified-fixture-fingerprint','tracking-binding','workspace_identity','1','1')",[])?;
        let key = crate::subjektiv::subject_worker_singleton_key("subject-tracking").unwrap();
        conn.execute("INSERT INTO worker_singleton_owners(workspace_id,singleton_key,runtime_id,worker_id,generation,created_at,updated_at) VALUES('w',?1,'r','worker',1,'1','1')",[&key])?;
        conn.execute("INSERT INTO worker_create_reservations(workspace_id,allocation_key,worker_id,runtime_id,create_fingerprint,state,created_at,updated_at,singleton_key,singleton_generation) VALUES('w','tracking-body','worker','r','tracking-body','created','1','1',?1,1)",[&key])?;
        conn.execute("INSERT INTO worker_registry(workspace_id,runtime_id,worker_id,display_name,retention_state,created_at,updated_at) VALUES('w','r','replacement','Replacement','normal','1','1')",[])?;
        Ok(())
    }).unwrap();
    (directory, store)
}

fn delivery_tracking_assign(
    store: &SqliteWorkspaceStore,
    id: &str,
    operation: &str,
) -> TicketRoleAssignmentRecord {
    store
        .set_current_ticket_role_assignment(
            &TicketRoleAssignmentRecord {
                assignment_id: id.into(),
                role: TicketAssignmentRole::Orchestrator,
                principal: TicketAssignmentPrincipal::Subject {
                    subject_id: "subject-tracking".into(),
                },
                ..manual_worker_assignment("tracking", id)
            },
            None,
            &format!("event-{operation}"),
            operation,
            false,
        )
        .unwrap()
}

fn delivery_tracking_replace_body(store: &SqliteWorkspaceStore) {
    store.with_conn(|conn| {
        let key = crate::subjektiv::subject_worker_singleton_key("subject-tracking").unwrap();
        conn.execute("UPDATE worker_create_reservations SET state='removed' WHERE workspace_id='w' AND singleton_key=?1",[&key])?;
        conn.execute("UPDATE worker_singleton_owners SET worker_id='replacement',generation=2 WHERE workspace_id='w' AND singleton_key=?1",[&key])?;
        conn.execute("INSERT INTO worker_create_reservations(workspace_id,allocation_key,worker_id,runtime_id,create_fingerprint,state,created_at,updated_at,singleton_key,singleton_generation) VALUES('w','replacement-body','replacement','r','replacement-body','created','2','2',?1,2)",[&key])?;
        Ok(())
    }).unwrap();
}

#[tokio::test]
async fn subject_assignment_startup_fences_other_subject_before_replay_and_after_reassignment_at_finalize()
 {
    let (_directory, store) = subject_notification_store().await;
    subject_notification_assign(&store, "subject-ticket-a", "assigned-a", "subject-a");
    let a = subject_notification_source("subject-a", "worker");
    let b = subject_notification_source("subject-b", "body-b");
    let reserve = |key: &str, source: &TicketAssignmentTrackingSourceContext| {
        store.reserve_ticket_assignment_operation_for_orchestrator(
            "w",
            key,
            "subject-ticket-a",
            "r",
            Some("body-replacement"),
            "subject-start-input",
            "3",
            "assigned-a",
            source,
        )
    };
    assert!(matches!(
        reserve("wrong-subject", &b),
        Err(Error::WorkspacePermissionDenied(_))
    ));
    assert!(reserve("subject-start", &a).unwrap());
    assert!(!reserve("subject-start", &a).unwrap());
    assert!(
        matches!(
            reserve("subject-start", &b),
            Err(Error::WorkspacePermissionDenied(_))
        ),
        "a saved start receipt cannot authorize another Subject"
    );
    store
        .validate_ticket_assignment_operation_fence("w", "subject-start")
        .unwrap();
    subject_notification_assign(&store, "subject-ticket-a", "reassigned-b", "subject-b");
    assert!(
        store
            .validate_ticket_assignment_operation_fence("w", "subject-start")
            .is_err()
    );
    assert!(
        store
            .bind_ticket_assignment_operation_worker("w", "subject-start", "body-replacement")
            .is_err()
    );
    assert!(
        store
            .set_current_ticket_worker_assignment(
                &TicketWorkerAssignmentRecord {
                    workspace_id: "w".into(),
                    ticket_id: "subject-ticket-a".into(),
                    assignment_id: "implementation-attempt".into(),
                    worker: RuntimeWorkerRef::new("r", "body-replacement"),
                    assigned_by: "owner".into(),
                    assigned_at: "4".into(),
                },
                None,
                "implementation-event",
                "subject-start",
                false,
            )
            .is_err(),
        "finalization rechecks the saved Subject assignment fence"
    );
    assert!(
        store
            .get_active_ticket_role_assignment(
                "w",
                "subject-ticket-a",
                TicketAssignmentRole::Worker
            )
            .unwrap()
            .is_none()
    );
    store.with_conn(|conn| {
        let wrong_receipts: i64 = conn.query_row("SELECT COUNT(*) FROM ticket_assignment_operations WHERE workspace_id='w' AND operation_id='wrong-subject'", [], |row| row.get(0))?;
        assert_eq!(wrong_receipts, 0, "source rejection rolls back the reservation");
        Ok(())
    }).unwrap();
}

#[tokio::test]
async fn subject_assignment_delivery_rechecks_body_lease_and_binding_even_for_saved_requests() {
    for change in [
        "missing-reservation",
        "generation",
        "removed",
        "binding-revoked",
        "deleted",
    ] {
        let (_directory, store) = delivery_tracking_fixture().await;
        delivery_tracking_assign(&store, "a", "assign-a");
        let source = delivery_tracking_source();
        let reserved = delivery_store_reserve_initial(&store, &source);
        store
            .prepare_ticket_assignment_delivery_content(
                "w",
                &reserved.delivery_id,
                "saved request",
                &source,
            )
            .unwrap();
        store.with_conn(|conn| {
            match change {
                "missing-reservation" => { conn.execute("DELETE FROM worker_create_reservations WHERE workspace_id='w'", [])?; },
                "generation" => { conn.execute("UPDATE worker_singleton_owners SET generation=2 WHERE workspace_id='w'", [])?; },
                "removed" => { conn.execute("UPDATE worker_create_reservations SET state='removed' WHERE workspace_id='w'", [])?; },
                "binding-revoked" => { conn.execute("UPDATE workspace_runtime_bindings SET revoked_at='revoked' WHERE workspace_id='w'", [])?; },
                _ => { conn.execute("INSERT INTO subject_deletions(workspace_id,subject_id,operation_id) VALUES('w','subject-tracking','delete-subject')", [])?; },
            }
            Ok(())
        }).unwrap();
        assert!(
            matches!(
                store.reserve_ticket_assignment_delivery(
                    "w",
                    "a",
                    &reserved.input_event_id,
                    &source
                ),
                Err(Error::WorkspacePermissionDenied(_))
            ),
            "reserve after {change}"
        );
        assert!(
            matches!(
                store.prepare_ticket_assignment_delivery_content(
                    "w",
                    &reserved.delivery_id,
                    "changed bytes",
                    &source
                ),
                Err(Error::WorkspacePermissionDenied(_))
            ),
            "prepare after {change}"
        );
        assert!(
            matches!(
                assignment_delivery::admit_send(&store, "w", &reserved.delivery_id, &source),
                Err(Error::WorkspacePermissionDenied(_))
            ),
            "final admission after {change}"
        );
        // Late acceptance remains valid evidence and never revalidates the sender.
        store
            .ack_ticket_assignment_delivery("w", &reserved.delivery_id, &source)
            .unwrap();
    }
}

#[tokio::test]
async fn subject_assignment_delivery_reassignment_denies_saved_receipt_to_both_still_live_subjects()
{
    let (_directory, store) = subject_notification_store().await;
    subject_notification_assign(&store, "subject-ticket-a", "assigned-a", "subject-a");
    let a = subject_notification_source("subject-a", "worker");
    let b = subject_notification_source("subject-b", "body-b");
    let input = store
        .list_pending_ticket_assignment_deliveries("w", 100)
        .unwrap()
        .remove(0);
    let saved = store
        .reserve_ticket_assignment_delivery("w", "assigned-a", &input.input_event_id, &a)
        .unwrap();
    store
        .prepare_ticket_assignment_delivery_content(
            "w",
            &saved.delivery_id,
            "original assignment bytes",
            &a,
        )
        .unwrap();
    subject_notification_assign(&store, "subject-ticket-a", "reassigned-b", "subject-b");
    // Neither body was stopped/replaced/revoked. The assignment fence, not sender
    // liveness, must reject replay for both the former and replacement Subject.
    for source in [&a, &b] {
        assert!(matches!(
            store.reserve_ticket_assignment_delivery(
                "w",
                "assigned-a",
                &input.input_event_id,
                source
            ),
            Err(Error::TicketAssignmentConflict(_))
        ));
        assert!(matches!(
            store.prepare_ticket_assignment_delivery_content(
                "w",
                &saved.delivery_id,
                "replayed bytes",
                source
            ),
            Err(Error::TicketAssignmentConflict(_))
        ));
    }
    for source in [&a, &b] {
        assert!(
            matches!(
                assignment_delivery::admit_send(&store, "w", &saved.delivery_id, source),
                Err(Error::TicketAssignmentConflict(_))
            ),
            "final admission is not a historical receipt read"
        );
    }
    store.with_conn(|conn| {
        let targets: i64 = conn.query_row("SELECT COUNT(*) FROM notification_assignment_targets WHERE workspace_id='w' AND delivery_id=?1", [&saved.delivery_id], |row| row.get(0))?;
        assert_eq!(targets, 1, "failed replay cannot authorize an additional target");
        Ok(())
    }).unwrap();
    // Acceptance and historical reads remain evidence after responsibility changes.
    store
        .ack_ticket_assignment_delivery("w", &saved.delivery_id, &a)
        .unwrap();
    let historical = store
        .page_ticket_assignment_diagnostics("w", "subject-ticket-a", "assigned-a", None, None, 100)
        .unwrap();
    assert!(!historical.current && !historical.actionable);
    assert_eq!(
        historical.inputs[0].notification,
        server_api::TicketAssignmentInputDeliveryState::RuntimeAccepted
    );
    assert!(
        !store
            .list_pending_ticket_assignment_deliveries("w", 100)
            .unwrap()
            .iter()
            .any(|pending| pending.assignment_id == "assigned-a")
    );
}

#[tokio::test]
async fn assignment_delivery_cached_pending_stops_after_positive_ack_but_unknown_acceptance_retries()
 {
    let (directory, store) = delivery_tracking_fixture().await;
    delivery_tracking_assign(&store, "a", "assign-a");
    let source = delivery_tracking_source();
    let cached = delivery_store_reserve_initial(&store, &source);
    assert!(
        assignment_delivery::admit_send(&store, "w", &cached.delivery_id, &source).is_err(),
        "reservation without pinned content is not send admission"
    );
    store
        .prepare_ticket_assignment_delivery_content(
            "w",
            &cached.delivery_id,
            "pinned bytes",
            &source,
        )
        .unwrap();
    let mut sends = Vec::new();
    let mut dispatch_cached = |store: &SqliteWorkspaceStore| {
        if let Some(content) =
            assignment_delivery::admit_send(store, "w", &cached.delivery_id, &source).unwrap()
        {
            sends.push((cached.request_id.clone(), content));
        }
    };
    dispatch_cached(&store);
    store
        .record_notification_delivery_failure(
            "w",
            &NotificationDeliveryKey::AssignmentInput {
                assignment_id: cached.assignment_id.clone(),
                input_event_id: cached.input_event_id.clone(),
            },
            NotificationFailureCode::AcceptanceUnknown,
        )
        .unwrap();
    // No ACK: retry admission preserves the same durable ID/bytes. A transport
    // unknown outcome is never converted into a positive acceptance receipt.
    assert_eq!(
        store
            .reserve_ticket_assignment_delivery("w", "a", &cached.input_event_id, &source)
            .unwrap(),
        cached
    );
    assert_eq!(
        store
            .prepare_ticket_assignment_delivery_content(
                "w",
                &cached.delivery_id,
                "rerendered bytes",
                &source
            )
            .unwrap(),
        "pinned bytes"
    );
    dispatch_cached(&store);
    // Another sender can commit ACK while this process still retains its cache.
    let other = SqliteWorkspaceStore::open_for_test(directory.path().join("workspace.db")).unwrap();
    other
        .ack_ticket_assignment_delivery("w", &cached.delivery_id, &source)
        .unwrap();
    dispatch_cached(&store);
    assert_eq!(
        sends,
        vec![(cached.request_id.clone(), "pinned bytes".into()); 2],
        "the ACKed cached input causes no new Runtime send"
    );
    assert!(matches!(
        store.reserve_ticket_assignment_delivery("w", "a", &cached.input_event_id, &source),
        Err(Error::TicketAssignmentConflict(_))
    ));
    assert!(matches!(
        store.prepare_ticket_assignment_delivery_content(
            "w",
            &cached.delivery_id,
            "late render",
            &source
        ),
        Err(Error::TicketAssignmentConflict(_))
    ));
    // ACK replay and historical evidence reads are independent of new admission.
    store
        .ack_ticket_assignment_delivery("w", &cached.delivery_id, &source)
        .unwrap();
    let evidence = store
        .page_ticket_assignment_diagnostics("w", "tracking", "a", None, None, 100)
        .unwrap();
    assert_eq!(
        evidence.inputs[0].notification,
        server_api::TicketAssignmentInputDeliveryState::RuntimeAccepted
    );
    assert!(
        store
            .list_pending_ticket_assignment_deliveries("w", 100)
            .unwrap()
            .is_empty()
    );
    delivery_tracking_replace_body(&store);
    let replacement = TicketAssignmentTrackingSourceContext {
        worker_id: "replacement".into(),
        ..source.clone()
    };
    assert!(
        store
            .reserve_ticket_assignment_delivery("w", "a", &cached.input_event_id, &replacement)
            .is_err()
    );
    store.with_conn(|conn| {
        let targets: i64 = conn.query_row("SELECT COUNT(*) FROM notification_assignment_targets WHERE workspace_id='w' AND delivery_id=?1", [&cached.delivery_id], |row| row.get(0))?;
        assert_eq!(targets, 1, "a positive ACK cannot authorize a replacement target");
        Ok(())
    }).unwrap();
}
