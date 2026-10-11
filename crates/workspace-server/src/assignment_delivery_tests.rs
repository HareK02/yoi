// Included in store::tests: exercise persisted transaction boundaries, not Runtime I/O.
fn delivery_store_external_event(
    store: &SqliteWorkspaceStore,
    index: i64,
    kind: &str,
    state_field: Option<&str>,
) {
    store.with_conn(|conn| {
        conn.execute("INSERT INTO typed_ticket_events(workspace_id,ticket_id,event_index,kind,author,at,state_field,body) VALUES('w','tracking',?1,?2,'user','2026-10-12T00:00:00Z',?3,'Answer arrived')",params![index,kind,state_field])?;
        Ok(())
    }).unwrap();
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
    let (_directory, store) = tracking_store_fixture().await;
    // Existing history before Assign must not become a new consideration input.
    delivery_store_external_event(&store, 0, "comment", None);
    tracking_store_assign(&store, "a", "assign-a");
    let source = tracking_store_source();
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
    let pending = store
        .list_pending_ticket_assignment_deliveries("w", 100)
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].input_event_id, "ticket-event:2");
    assert_eq!(
        pending[0].waiting_reason.as_deref(),
        Some("Waiting for owner answer")
    );
    assert_eq!(
        pending[0].next_action.as_deref(),
        Some("Read answer and decide")
    );
    let answer = store
        .reserve_ticket_assignment_delivery("w", "a", "ticket-event:2", &source)
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
    // Typed extension inputs must not disappear merely because they are not
    // comments; own tracking/history events remain excluded.
    delivery_store_external_event(&store, 3, "assignment_tracking_updated", None);
    delivery_store_external_event(&store, 4, "history", None);
    delivery_store_external_event(&store, 5, "owner_answer", None);
    let pending = store
        .list_pending_ticket_assignment_deliveries("w", 100)
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].input_event_id, "ticket-event:5");
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
    let (directory, store) = tracking_store_fixture().await;
    tracking_store_assign(&store, "a", "assign-a");
    let source = tracking_store_source();
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
    let (directory, store) = tracking_store_fixture().await;
    tracking_store_assign(&store, "a", "assign-a");
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
                    .reserve_ticket_assignment_delivery("w", "a", &input, &tracking_store_source())
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
        let (_directory, store) = tracking_store_fixture().await;
        tracking_store_assign(&store, "a", "assign-a");
        let source = tracking_store_source();
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
    let (_directory, store) = tracking_store_fixture().await;
    tracking_store_assign(&store, "a", "assign-a");
    let source = tracking_store_source();
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
    let new = tracking_store_assign(&store, "b", "assign-b");
    assert_eq!(new.assignment_id, "b");
    assert!(
        store
            .reserve_ticket_assignment_delivery("w", "a", &old.input_event_id, &source)
            .is_err()
    );
    assert!(
        store
            .ack_ticket_assignment_delivery("w", &old.delivery_id, &source)
            .is_err()
    );
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
async fn assignment_delivery_old_worker_or_authenticated_binding_cannot_reserve_or_ack() {
    for change in ["replacement", "revoke", "binding-change"] {
        let (_directory, store) = tracking_store_fixture().await;
        tracking_store_assign(&store, "a", "assign-a");
        let source = tracking_store_source();
        let old = delivery_store_reserve_initial(&store, &source);
        store.with_conn(|conn| {
            match change {
                "replacement" => {conn.execute("UPDATE worker_singleton_owners SET worker_id='replacement' WHERE workspace_id='w'",[])?;},
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
        assert!(
            matches!(
                store.ack_ticket_assignment_delivery("w", &old.delivery_id, &source),
                Err(Error::WorkspacePermissionDenied(_))
            ),
            "{change}"
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
        let (_directory, store) = tracking_store_fixture().await;
        tracking_store_assign(&store, "a", "assign-a");
        let source = tracking_store_source();
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
                tracking_store_assign(&store, "b", "assign-b");
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
    let (_directory, store) = tracking_store_fixture().await;
    tracking_store_assign(&store, "a", "assign-a");
    let source = tracking_store_source();
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
    let (_directory, store) = tracking_store_fixture().await;
    tracking_store_assign(&store, "a", "assign-a");
    let source = tracking_store_source();
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
    let (directory, store) = tracking_store_fixture().await;
    tracking_store_assign(&store, "a", "assign-a");
    let source = tracking_store_source();
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
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("schema92.db");
    // A genuine retained Ticket-10/Workspace-91 fixture goes through its actual
    // applied tracking migration; no current DB markers or columns are lowered.
    let conn = tracking_schema91_fixture(&path);
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
    assert_eq!(
        pending
            .iter()
            .map(|d| d.ticket_id.as_str())
            .collect::<Vec<_>>(),
        vec!["pending", "progress"]
    );
    assert_eq!(
        pending[0].input_event_id,
        "assignment-event:migration-tracking:a-pending"
    );
    store
        .with_conn(|conn| {
            assert_eq!(current_schema_version(conn)?, 93);
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
}

#[tokio::test]
async fn assignment_startup_simultaneous_reservations_admit_runtime_effects_once() {
    let (directory, store) = tracking_store_fixture().await;
    tracking_store_assign(&store, "a", "assign-a");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let threads = (0..2)
        .map(|_| {
            let barrier = barrier.clone();
            let path = directory.path().join("workspace.db");
            std::thread::spawn(move || {
                let store = SqliteWorkspaceStore::open_for_test(path).unwrap();
                barrier.wait();
                delivery_store_start_reserve(&store, "start-a", "a", &tracking_store_source())
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
    let (directory, store) = tracking_store_fixture().await;
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
    let (_directory, store) = tracking_store_fixture().await;
    tracking_store_assign(&store, "a", "assign-a");
    let source = tracking_store_source();
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
