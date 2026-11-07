// Real SQLite through the production Store. Runtime acceptance is supplied by the
// caller here; server_assignment_delivery_tests owns the real NotifyTracked boundary.
fn routing_worker_assign(store: &SqliteWorkspaceStore, id: &str) -> TicketWorkerNotificationTarget {
    let assignment = TicketRoleAssignmentRecord {
        principal: TicketAssignmentPrincipal::Worker {
            runtime_id: "r".into(),
            worker_id: "replacement".into(),
        },
        ..manual_worker_assignment("tracking", id)
    };
    let current = store
        .get_current_ticket_role_assignment("w", "tracking", TicketAssignmentRole::Worker)
        .unwrap();
    store
        .set_current_ticket_role_assignment(
            &assignment,
            current.as_ref().map(|a| a.assignment_id.as_str()),
            &format!("event-{id}"),
            &format!("operation-{id}"),
            true,
        )
        .unwrap();
    TicketWorkerNotificationTarget {
        worker: RuntimeWorkerRef::new("r", "replacement"),
        authentication_binding_id: "tracking-binding".into(),
    }
}
fn routing_ack_worker_pending(
    store: &SqliteWorkspaceStore,
    target: &TicketWorkerNotificationTarget,
) {
    for input in store
        .list_pending_ticket_worker_notifications("w", 100)
        .unwrap()
    {
        store
            .prepare_ticket_worker_notification_content(
                "w",
                &input.delivery_id,
                "initial notice",
                target,
            )
            .unwrap();
        store
            .ack_ticket_worker_notification("w", &input.delivery_id, target)
            .unwrap();
    }
}

#[tokio::test]
async fn notification_routing_diagnostics_preview_unrouted_requests_readonly_beyond_global_batch() {
    let (_directory, store) = delivery_tracking_fixture().await;
    seed_manual_start_ticket(&store, "other-ticket", "open");
    store.with_conn_mut(|conn| {
        let tx = conn.transaction()?;
        for index in 0..260 {
            tx.execute("INSERT INTO typed_ticket_events(workspace_id,ticket_id,event_index,event_ref,kind,author,body) VALUES('w','other-ticket',?1,'other:' || ?1,'comment','human','unrelated')",[index])?;
        }
        tx.commit()?;
        Ok(())
    }).unwrap();
    delivery_tracking_assign(&store, "a", "assign-a");
    delivery_store_answer(&store, "preview-answer");
    // An answer without the assignment fence must be suppressed by the same Rust policy.
    delivery_store_external_event(&store, 1, "assignment_answer", None);
    let snapshot = || {
        store
            .with_conn(|conn| {
                let changes: i64 = conn.query_row("SELECT total_changes()", [], |r| r.get(0))?;
                let rows = [
                    "notification_sources",
                    "notification_decisions",
                    "notification_requests",
                    "notification_failures",
                    "notification_acknowledgements",
                    "notification_assignment_targets",
                    "ticket_assignment_deliveries",
                    "ticket_assignment_delivery_requests",
                    "ticket_assignment_delivery_acknowledgements",
                ]
                .map(|table| stored_fixture_rows(conn, table));
                Ok((changes, rows))
            })
            .unwrap()
    };
    let before = snapshot();
    store
        .with_conn(|conn| {
            conn.execute_batch("PRAGMA query_only=ON")?;
            Ok(())
        })
        .unwrap();
    let first = store
        .page_ticket_assignment_diagnostics("w", "tracking", "a", None, None, 1)
        .unwrap();
    assert_eq!(first.inputs.len(), 1);
    assert_eq!(
        first.inputs[0].input_event_id,
        "assignment-event:event-assign-a"
    );
    assert_eq!(
        first.inputs[0].notification,
        server_api::TicketAssignmentInputDeliveryState::NotReserved
    );
    assert_eq!(
        first.next_input_event_id.as_deref(),
        Some("assignment-event:event-assign-a")
    );
    let next = store
        .page_ticket_assignment_diagnostics(
            "w",
            "tracking",
            "a",
            first.next_input_event_id.as_deref(),
            None,
            1,
        )
        .unwrap();
    assert_eq!(next.inputs.len(), 1);
    assert_eq!(next.inputs[0].input_event_id, "ticket-event:0");
    assert!(next.next_input_event_id.is_none());
    for (ticket, assignment, input, start) in [
        ("tracking", "missing", None, None),
        ("other-ticket", "a", None, None),
        ("tracking", "a", Some("missing"), None),
        ("tracking", "a", Some("ticket-event:1"), None),
        ("tracking", "a", None, Some("missing")),
    ] {
        assert!(
            store
                .page_ticket_assignment_diagnostics("w", ticket, assignment, input, start, 1)
                .is_err()
        );
    }
    assert_eq!(
        snapshot(),
        before,
        "diagnostics must not mutate even on invalid cursors/assignment"
    );
    store
        .with_conn(|conn| {
            conn.execute_batch("PRAGMA query_only=OFF")?;
            Ok(())
        })
        .unwrap();
    // Only the delivery path persists routing, in bounded batches.
    assert!(
        store
            .list_pending_ticket_assignment_deliveries("w", 100)
            .unwrap()
            .is_empty()
    );
    let pending = store
        .list_pending_ticket_assignment_deliveries("w", 100)
        .unwrap();
    assert_eq!(
        pending
            .iter()
            .map(|d| d.input_event_id.as_str())
            .collect::<Vec<_>>(),
        vec![
            first.inputs[0].input_event_id.as_str(),
            next.inputs[0].input_event_id.as_str()
        ]
    );
    let persisted = snapshot();
    let page = store
        .page_ticket_assignment_diagnostics("w", "tracking", "a", None, None, 100)
        .unwrap();
    assert_eq!(page.inputs.len(), 2);
    assert_eq!(snapshot(), persisted);
}

#[tokio::test]
async fn notification_routing_orchestrator_ack_retains_authorized_target_after_responsibility_changes()
 {
    for change in [
        "done",
        "release",
        "reassign",
        "revoked",
        "binding-replaced",
        "worker-replaced",
    ] {
        for prepare in [false, true] {
            let (_directory, store) = delivery_tracking_fixture().await;
            delivery_tracking_assign(&store, "a", "assign-a");
            let source = delivery_tracking_source();
            let delivery = delivery_store_reserve_initial(&store, &source);
            if prepare {
                store
                    .prepare_ticket_assignment_delivery_content(
                        "w",
                        &delivery.delivery_id,
                        "immutable request",
                        &source,
                    )
                    .unwrap();
            }
            match change {
                "done" => { store.update_ticket_assignment_tracking("w", "tracking", &tracking_store_input("a", "event-assign-a", "done-a"), &source).unwrap(); }
                "release" => { store.clear_current_ticket_role_assignment("w", "tracking", TicketAssignmentRole::Orchestrator, "a", "clear-a", "clear-a", "human", "now", None).unwrap(); }
                "reassign" => { delivery_tracking_assign(&store, "b", "assign-b"); }
                _ => store.with_conn(|conn| {
                    match change {
                        "revoked" => conn.execute("UPDATE workspace_runtime_bindings SET revoked_at='revoked' WHERE workspace_id='w'", [])?,
                        "binding-replaced" => conn.execute("UPDATE workspace_runtime_bindings SET binding_id='replacement-binding' WHERE workspace_id='w'", [])?,
                        _ => conn.execute("UPDATE worker_singleton_owners SET worker_id='replacement' WHERE workspace_id='w'", [])?,
                    };
                    Ok(())
                }).unwrap(),
            }
            for field in ["runtime", "worker", "agent", "binding"] {
                let mut forged = source.clone();
                match field {
                    "runtime" => forged.runtime_id = "forged".into(),
                    "worker" => forged.worker_id = "replacement".into(),
                    "agent" => forged.subject_id = "forged".into(),
                    _ => forged.authentication_binding_id = "replacement-binding".into(),
                }
                assert!(
                    matches!(
                        store.ack_ticket_assignment_delivery("w", &delivery.delivery_id, &forged),
                        Err(Error::WorkspacePermissionDenied(_))
                    ),
                    "{change}/{field}"
                );
            }
            assert!(
                store
                    .ack_ticket_assignment_delivery("other", &delivery.delivery_id, &source)
                    .is_err()
            );
            store
                .ack_ticket_assignment_delivery("w", &delivery.delivery_id, &source)
                .unwrap();
            store
                .ack_ticket_assignment_delivery("w", &delivery.delivery_id, &source)
                .unwrap();
            assert!(
                store
                    .reserve_ticket_assignment_delivery("w", "a", &delivery.input_event_id, &source)
                    .is_err(),
                "{change}"
            );
            assert!(
                store
                    .prepare_ticket_assignment_delivery_content(
                        "w",
                        &delivery.delivery_id,
                        "retry",
                        &source
                    )
                    .is_err(),
                "{change}"
            );
            let page = store
                .page_ticket_assignment_diagnostics("w", "tracking", "a", None, None, 100)
                .unwrap();
            let input = page
                .inputs
                .iter()
                .find(|i| i.input_event_id == delivery.input_event_id)
                .unwrap();
            assert_eq!(
                input.notification,
                server_api::TicketAssignmentInputDeliveryState::RuntimeAccepted
            );
            assert!(!input.acceptance_outcome_unknown);
            store.with_conn(|conn| {
                assert_eq!(stored_fixture_rows(conn, "ticket_assignment_delivery_acknowledgements").len(), 1);
                let payload: String = conn.query_row("SELECT payload_json FROM ticket_assignment_deliveries WHERE delivery_id=?1", [&delivery.delivery_id], |r| r.get(0))?;
                assert_eq!(serde_json::from_str::<TicketAssignmentDelivery>(&payload).unwrap(), delivery);
                Ok(())
            }).unwrap();
        }
    }
}

#[tokio::test]
async fn notification_routing_replacement_target_requires_explicit_authorized_reserve_or_prepare() {
    for prepare in [false, true] {
        let (_directory, store) = delivery_tracking_fixture().await;
        delivery_tracking_assign(&store, "a", "assign-a");
        let source = delivery_tracking_source();
        let delivery = delivery_store_reserve_initial(&store, &source);
        store
            .prepare_ticket_assignment_delivery_content(
                "w",
                &delivery.delivery_id,
                "original request",
                &source,
            )
            .unwrap();
        delivery_tracking_replace_body(&store);
        let replacement = TicketAssignmentTrackingSourceContext {
            worker_id: "replacement".into(),
            ..source.clone()
        };
        assert!(
            store
                .ack_ticket_assignment_delivery("w", &delivery.delivery_id, &replacement)
                .is_err()
        );
        if prepare {
            assert_eq!(
                store
                    .prepare_ticket_assignment_delivery_content(
                        "w",
                        &delivery.delivery_id,
                        "different rendering",
                        &replacement
                    )
                    .unwrap(),
                "original request"
            );
        } else {
            assert_eq!(
                store
                    .reserve_ticket_assignment_delivery(
                        "w",
                        "a",
                        &delivery.input_event_id,
                        &replacement
                    )
                    .unwrap(),
                delivery
            );
        }
        store
            .ack_ticket_assignment_delivery("w", &delivery.delivery_id, &replacement)
            .unwrap();
        // A different target's receipt cannot overwrite the first ACK, even if both were authorized.
        assert!(
            store
                .ack_ticket_assignment_delivery("w", &delivery.delivery_id, &source)
                .is_err()
        );
        store
            .with_conn(|conn| {
                assert_eq!(
                    stored_fixture_rows(conn, "notification_assignment_targets").len(),
                    2
                );
                assert_eq!(
                    stored_fixture_rows(conn, "ticket_assignment_deliveries").len(),
                    1
                );
                assert_eq!(
                    stored_fixture_rows(conn, "ticket_assignment_delivery_requests").len(),
                    1
                );
                assert_eq!(
                    stored_fixture_rows(conn, "ticket_worker_assignments").len(),
                    1
                );
                assert!(
                    conn.execute(
                        "UPDATE notification_assignment_targets SET worker_id='forged'",
                        []
                    )
                    .is_err()
                );
                assert!(
                    conn.execute("DELETE FROM notification_assignment_targets", [])
                        .is_err()
                );
                Ok(())
            })
            .unwrap();
    }
}

#[tokio::test]
async fn notification_routing_commit_without_callback_recovers_and_rollback_has_no_source() {
    let (directory, store) = delivery_tracking_fixture().await;
    let target = routing_worker_assign(&store, "worker-a");
    routing_ack_worker_pending(&store, &target);
    store.with_conn_mut(|conn| {
        let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("INSERT INTO typed_ticket_events(workspace_id,ticket_id,event_index,event_ref,kind,author,body) VALUES('w','tracking',0,'tracking:0','comment','human','rollback')",[])?;
        tx.rollback()?;
        let count:i64=conn.query_row("SELECT COUNT(*) FROM notification_sources WHERE input_event_id='ticket-event:0'",[],|r|r.get(0))?;
        assert_eq!(count,0);Ok(())
    }).unwrap();
    delivery_store_external_event(&store, 0, "comment", None);
    store.with_conn(|conn| {
        let count:i64=conn.query_row("SELECT COUNT(*) FROM notification_sources s LEFT JOIN notification_decisions d ON d.source_sequence=s.sequence WHERE s.input_event_id='ticket-event:0' AND d.source_sequence IS NULL",[],|r|r.get(0))?;
        assert_eq!(count,2,"both logical destinations are captured before routing");Ok(())
    }).unwrap();
    drop(store);
    let store = SqliteWorkspaceStore::open_for_test(directory.path().join("workspace.db")).unwrap();
    let inputs = store
        .list_pending_ticket_worker_notifications("w", 100)
        .unwrap();
    assert_eq!(inputs.len(), 1);
    assert_eq!(inputs[0].input_event_id, "ticket-event:0");
    assert_eq!(inputs[0].assignment_id, "worker-a");
    assert_eq!(inputs[0].purpose, NotificationPurpose::Attention);
    store.with_conn(|conn| {
        let reason:String=conn.query_row("SELECT d.reason FROM notification_sources s JOIN notification_decisions d ON d.source_sequence=s.sequence WHERE s.input_event_id='ticket-event:0' AND s.role='orchestrator'",[],|r|r.get(0))?;
        assert_eq!(reason,"no_recipient");Ok(())
    }).unwrap();
}

#[tokio::test]
async fn notification_routing_parallel_scan_and_render_reuse_request_after_response_loss() {
    let (directory, store) = delivery_tracking_fixture().await;
    let target = routing_worker_assign(&store, "worker-a");
    routing_ack_worker_pending(&store, &target);
    delivery_store_external_event(&store, 0, "comment", None);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let threads = (0..2)
        .map(|i| {
            let barrier = barrier.clone();
            let path = directory.path().join("workspace.db");
            let target = target.clone();
            std::thread::spawn(move || {
                let store = SqliteWorkspaceStore::open_for_test(path).unwrap();
                barrier.wait();
                let input = store
                    .list_pending_ticket_worker_notifications("w", 100)
                    .unwrap()
                    .remove(0);
                let text = store
                    .prepare_ticket_worker_notification_content(
                        "w",
                        &input.delivery_id,
                        &format!("rendered {i}"),
                        &target,
                    )
                    .unwrap();
                (input, text)
            })
        })
        .collect::<Vec<_>>();
    let receipts = threads
        .into_iter()
        .map(|t| t.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(receipts[0], receipts[1]);
    let (input, content) = &receipts[0];
    drop(store);
    let store = SqliteWorkspaceStore::open_for_test(directory.path().join("workspace.db")).unwrap();
    assert_eq!(
        &store
            .prepare_ticket_worker_notification_content(
                "w",
                &input.delivery_id,
                "new rendering",
                &target
            )
            .unwrap(),
        content
    );
    assert_eq!(
        store
            .list_pending_ticket_worker_notifications("w", 100)
            .unwrap(),
        vec![input.clone()]
    );
    store
        .ack_ticket_worker_notification("w", &input.delivery_id, &target)
        .unwrap();
    store
        .ack_ticket_worker_notification("w", &input.delivery_id, &target)
        .unwrap();
    assert!(
        store
            .list_pending_ticket_worker_notifications("w", 100)
            .unwrap()
            .is_empty()
    );
    store
        .with_conn(|conn| {
            assert!(
                conn.execute("UPDATE notification_requests SET content='changed'", [])
                    .is_err()
            );
            assert!(
                conn.execute("DELETE FROM notification_acknowledgements", [])
                    .is_err()
            );
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM notification_requests WHERE delivery_id=?1",
                [&input.delivery_id],
                |r| r.get(0),
            )?;
            assert_eq!(count, 1);
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn notification_routing_assignment_change_between_commit_and_scan_never_transfers_input() {
    let (_directory, store) = delivery_tracking_fixture().await;
    let target = routing_worker_assign(&store, "worker-a");
    routing_ack_worker_pending(&store, &target);
    delivery_store_external_event(&store, 0, "comment", None);
    routing_worker_assign(&store, "worker-b");
    let pending = store
        .list_pending_ticket_worker_notifications("w", 100)
        .unwrap();
    assert!(pending.iter().all(|i| i.assignment_id == "worker-b"));
    assert!(pending.iter().all(|i| i.input_event_id != "ticket-event:0"));
    store.with_conn(|conn| {
        let captured:String=conn.query_row("SELECT assignment_id FROM notification_sources WHERE input_event_id='ticket-event:0' AND role='worker'",[],|r|r.get(0))?;
        assert_eq!(captured,"worker-a");
        let failure:String=conn.query_row("SELECT f.code FROM notification_failures f JOIN notification_decisions d USING(workspace_id,delivery_id) JOIN notification_sources s ON s.sequence=d.source_sequence WHERE s.input_event_id='ticket-event:0' AND s.role='worker'",[],|r|r.get(0))?;
        assert_eq!(failure,"assignment_changed");Ok(())
    }).unwrap();
}

#[tokio::test]
async fn notification_routing_ack_retains_acceptance_after_responsibility_changes() {
    let (_directory, store) = delivery_tracking_fixture().await;
    let target = routing_worker_assign(&store, "worker-a");
    routing_ack_worker_pending(&store, &target);
    delivery_store_external_event(&store, 0, "comment", None);
    let input = store
        .list_pending_ticket_worker_notifications("w", 100)
        .unwrap()
        .remove(0);
    store
        .prepare_ticket_worker_notification_content(
            "w",
            &input.delivery_id,
            "accepted during reassignment",
            &target,
        )
        .unwrap();
    routing_worker_assign(&store, "worker-b");
    assert!(
        store
            .prepare_ticket_worker_notification_content("w", &input.delivery_id, "retry", &target)
            .is_err()
    );
    store
        .ack_ticket_worker_notification("w", &input.delivery_id, &target)
        .unwrap();
    store
        .with_conn(|conn| {
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM notification_acknowledgements WHERE delivery_id=?1",
                [input.delivery_id],
                |r| r.get(0),
            )?;
            assert_eq!(count, 1);
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn notification_routing_editor_is_not_original_author_and_unknown_events_are_deferred() {
    let (directory, store) = delivery_tracking_fixture().await;
    let target = routing_worker_assign(&store, "worker-a");
    routing_ack_worker_pending(&store, &target);
    use ticket::TicketBackend;
    let backend =
        ticket::SqliteTicketBackend::open_verified(directory.path().join("workspace.db"), "w")
            .unwrap();
    backend
        .edit_item(
            "tracking".into(),
            ticket::TicketItemEdit {
                body: Some(ticket::MarkdownText::new("Worker edited")),
                author: Some("worker:r/replacement".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(
        store
            .list_pending_ticket_worker_notifications("w", 100)
            .unwrap()
            .is_empty()
    );
    backend
        .edit_item(
            "tracking".into(),
            ticket::TicketItemEdit {
                title: Some("Worker title edit".into()),
                author: Some("worker:r/replacement".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(
        store
            .list_pending_ticket_worker_notifications("w", 100)
            .unwrap()
            .is_empty(),
        "title-only edits retain the editor and suppress self-notification"
    );
    backend
        .edit_item(
            "tracking".into(),
            ticket::TicketItemEdit {
                body: Some(ticket::MarkdownText::new("Human edited")),
                author: Some("human".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let pending = store
        .list_pending_ticket_worker_notifications("w", 100)
        .unwrap();
    assert_eq!(pending.len(), 1);
    delivery_store_external_event(&store, 0, "future_event", None);
    store
        .list_pending_ticket_worker_notifications("w", 100)
        .unwrap();
    store.with_conn(|conn| {
        let self_reason:String=conn.query_row("SELECT d.reason FROM notification_sources s JOIN notification_decisions d ON d.source_sequence=s.sequence WHERE s.actor='worker:r/replacement' AND s.role='worker'",[],|r|r.get(0))?;
        assert_eq!(self_reason,"self_notification");
        let decision:(String,String)=conn.query_row("SELECT d.disposition,d.reason FROM notification_sources s JOIN notification_decisions d ON d.source_sequence=s.sequence WHERE s.kind='future_event' AND s.role='worker'",[],|r|Ok((r.get(0)?,r.get(1)?)))?;
        assert_eq!(decision,("defer".into(),"unknown_event".into()));Ok(())
    }).unwrap();
}

#[tokio::test]
async fn notification_routing_failures_do_not_starve_other_inputs_or_replace_requests() {
    let (_directory, store) = delivery_tracking_fixture().await;
    let target = routing_worker_assign(&store, "worker-a");
    routing_ack_worker_pending(&store, &target);
    delivery_store_external_event(&store, 0, "comment", None);
    delivery_store_external_event(&store, 1, "comment", None);
    let first = store
        .list_pending_ticket_worker_notifications("w", 1)
        .unwrap()
        .remove(0);
    let key = NotificationDeliveryKey::WorkerNotification {
        delivery_id: first.delivery_id.clone(),
    };
    store
        .prepare_ticket_worker_notification_content("w", &first.delivery_id, "pinned", &target)
        .unwrap();
    for _ in 0..2 {
        store
            .record_notification_delivery_failure(
                "w",
                &key,
                NotificationFailureCode::AcceptanceUnknown,
            )
            .unwrap();
    }
    let second = store
        .list_pending_ticket_worker_notifications("w", 1)
        .unwrap()
        .remove(0);
    assert_ne!(first.delivery_id, second.delivery_id);
    assert_eq!(
        store
            .prepare_ticket_worker_notification_content("w", &first.delivery_id, "changed", &target)
            .unwrap(),
        "pinned"
    );
    store
        .with_conn(|conn| {
            let failures: Vec<String> = conn
                .prepare("SELECT code FROM notification_failures WHERE delivery_id=?1")?
                .query_map([first.delivery_id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            assert_eq!(failures, vec!["acceptance_unknown"]);
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn notification_routing_target_binding_and_workspace_are_rechecked_before_prepare() {
    let (_directory, store) = delivery_tracking_fixture().await;
    let target = routing_worker_assign(&store, "worker-a");
    let input = store
        .list_pending_ticket_worker_notifications("w", 100)
        .unwrap()
        .remove(0);
    let wrong = TicketWorkerNotificationTarget {
        worker: RuntimeWorkerRef::new("r", "worker"),
        ..target.clone()
    };
    assert!(
        store
            .prepare_ticket_worker_notification_content("w", &input.delivery_id, "text", &wrong)
            .is_err()
    );
    assert!(
        store
            .prepare_ticket_worker_notification_content(
                "other",
                &input.delivery_id,
                "text",
                &target
            )
            .is_err()
    );
    store
        .with_conn(|conn| {
            conn.execute(
                "UPDATE workspace_runtime_bindings SET revoked_at='revoked' WHERE workspace_id='w'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    assert!(
        store
            .prepare_ticket_worker_notification_content("w", &input.delivery_id, "text", &target)
            .is_err()
    );
}

#[test]
fn notification_routing_migration_preserves_receipts_does_not_replay_comments_and_rolls_back() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("workspace.db");
    let conn = tracking_schema91_fixture(&path);
    migrate_ticket_tracking_v91_to_v92(&conn).unwrap();
    migrate_assignment_delivery_v92_to_v93(&conn).unwrap();
    migrate_ticket_posts_v93_to_v94(&conn).unwrap();
    conn.execute("INSERT INTO typed_ticket_events(workspace_id,ticket_id,event_index,event_ref,kind,author,body) VALUES('w','pending',999,'pending:999','comment','human','historical')",[]).unwrap();
    let payload = TicketAssignmentDelivery {
        ticket_id: "pending".into(),
        assignment_id: "a-pending".into(),
        input_event_id: "assignment-event:migration-tracking:a-pending".into(),
        delivery_id: "preserved-id".into(),
        request_id: "preserved-request".into(),
        waiting_reason: None,
        next_action: None,
    };
    // Retained94 stores an agent-key context, not today's Subject context.
    let reserved_source_json = r#"{"actor":"authenticated-worker:r/worker","runtime_id":"r","worker_id":"worker","agent_key":"workspace-orchestrator","authentication_binding_id":"tracking-binding"}"#;
    let acknowledged_source_json = r#"{"actor":"authenticated-worker:r/worker","runtime_id":"r","worker_id":"old-replacement","agent_key":"workspace-orchestrator","authentication_binding_id":"tracking-binding"}"#;
    conn.execute(
        "INSERT INTO ticket_assignment_deliveries VALUES('w','a-pending',?1,'preserved-id',?2,?3)",
        params![
            payload.input_event_id,
            serde_json::to_string(&payload).unwrap(),
            reserved_source_json
        ],
    )
    .unwrap();
    conn.execute("INSERT INTO ticket_assignment_delivery_requests VALUES('w','preserved-id','immutable request')",[]).unwrap();
    let acknowledged_target = TicketAssignmentTrackingSourceContext {
        worker_id: "old-replacement".into(),
        subject_id: "workspace-orchestrator".into(),
        ..delivery_tracking_source()
    };
    conn.execute(
        "INSERT INTO ticket_assignment_delivery_acknowledgements VALUES('w','preserved-id',?1)",
        [acknowledged_source_json],
    )
    .unwrap();
    let requests_before = stored_fixture_rows(&conn, "ticket_assignment_delivery_requests");
    let acks_before = stored_fixture_rows(&conn, "ticket_assignment_delivery_acknowledgements");
    let before = stored_fixture_rows(&conn, "ticket_assignment_deliveries");
    conn.execute_batch("CREATE TRIGGER notification_ticket_event AFTER INSERT ON typed_ticket_events BEGIN SELECT 1; END;").unwrap();
    assert!(migrate_notification_routing_v94_to_v95(&conn).is_err());
    assert_eq!(current_schema_version(&conn).unwrap(), 94);
    let missing: bool = conn
        .query_row(
            "SELECT NOT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='notification_sources')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(missing);
    assert!(
        conn.prepare("SELECT actor FROM ticket_assignment_content_inputs")
            .is_err()
    );
    conn.execute_batch("DROP TRIGGER notification_ticket_event;")
        .unwrap();
    migrate_notification_routing_v94_to_v95(&conn).unwrap();
    assert_eq!(
        stored_fixture_rows(&conn, "ticket_assignment_deliveries"),
        before
    );
    assert_eq!(
        stored_fixture_rows(&conn, "ticket_assignment_delivery_requests"),
        requests_before
    );
    assert_eq!(
        stored_fixture_rows(&conn, "ticket_assignment_delivery_acknowledgements"),
        acks_before
    );
    assert_eq!(
        stored_fixture_rows(&conn, "notification_assignment_targets").len(),
        2
    );
    let old_comments: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM notification_sources WHERE input_event_id='ticket-event:999'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(old_comments, 0);
    drop(conn);
    let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    let pending = store
        .list_pending_ticket_assignment_deliveries("w", 100)
        .unwrap();
    assert!(
        !pending.iter().any(|p| p.assignment_id == "a-pending"),
        "saved ACK remains authoritative even with non-derived legacy delivery ID"
    );
    // A new Subject-shaped context must not reinterpret the immutable agent ACK.
    assert!(matches!(
        store.ack_ticket_assignment_delivery("w", "preserved-id", &acknowledged_target),
        Err(Error::TicketAssignmentConflict(_))
    ));
    store.with_conn(|conn| {
        assert_eq!(stored_fixture_rows(conn, "ticket_assignment_delivery_requests"), requests_before);
        assert_eq!(stored_fixture_rows(conn, "ticket_assignment_delivery_acknowledgements"), acks_before);
        assert_eq!(stored_fixture_rows(conn, "ticket_assignment_deliveries"), before);
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM notification_assignment_targets WHERE delivery_id='preserved-id' AND agent_key='workspace-orchestrator'", [], |row| row.get::<_, i64>(0))?, 2);
        Ok(())
    }).unwrap();
}

#[tokio::test]
async fn notification_routing_missing_authorized_target_guard_refuses_startup() {
    for guard in [
        "notification_assignment_targets_no_update",
        "notification_assignment_targets_no_delete",
    ] {
        let (directory, store) = delivery_tracking_fixture().await;
        store
            .with_conn(|conn| {
                conn.execute_batch(&format!("DROP TRIGGER {guard}"))?;
                Ok(())
            })
            .unwrap();
        drop(store);
        let error = match SqliteWorkspaceStore::open_for_test(directory.path().join("workspace.db"))
        {
            Ok(_) => panic!("missing target evidence guard accepted"),
            Err(error) => error,
        };
        assert!(error.to_string().contains(guard));
    }
}

#[tokio::test]
async fn notification_routing_fresh_and_upgraded_schema_and_guards_match() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("upgraded.db");
    let conn = tracking_schema91_fixture(&path);
    migrate_ticket_tracking_v91_to_v92(&conn).unwrap();
    migrate_assignment_delivery_v92_to_v93(&conn).unwrap();
    migrate_ticket_posts_v93_to_v94(&conn).unwrap();
    migrate_notification_routing_v94_to_v95(&conn).unwrap();
    migrate_job_notifications_v95_to_v96(&conn).unwrap();
    migrate_business_notifications_v96_to_v97(&conn).unwrap();
    migrate_result_reports_v97_to_v98(&conn).unwrap();
    let snapshot = |conn: &Connection| -> Vec<(String, String)> {
        conn.prepare("SELECT name,sql FROM sqlite_schema WHERE name LIKE 'notification_%' OR name LIKE 'ticket_assignment_content%' OR name IN ('ticket_assignment_title_input','ticket_assignment_initial_post_input') ORDER BY name").unwrap().query_map([],|r|Ok((r.get(0)?,r.get(1)?))).unwrap().collect::<rusqlite::Result<_>>().unwrap()
    };
    assert_eq!(current_schema_version(&conn).unwrap(), 98);
    drop(conn);
    // Compare the complete current composition, not upgraded98 against fresh current schema.
    let upgraded_store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    let upgraded = upgraded_store
        .with_conn(|conn| {
            assert_eq!(current_schema_version(conn)?, LATEST_SCHEMA_VERSION);
            Ok(snapshot(conn))
        })
        .unwrap();
    let (fresh_directory, fresh) = delivery_tracking_fixture().await;
    fresh
        .with_conn(|conn| {
            assert_eq!(snapshot(conn), upgraded);
            Ok(())
        })
        .unwrap();
    fresh
        .with_conn(|conn| {
            conn.execute_batch("DROP TRIGGER notification_sources_no_update;")?;
            Ok(())
        })
        .unwrap();
    drop(fresh);
    let error =
        match SqliteWorkspaceStore::open_for_test(fresh_directory.path().join("workspace.db")) {
            Ok(_) => panic!("missing guard accepted"),
            Err(error) => error,
        };
    assert!(error.to_string().contains("notification_sources_no_update"));
}

#[tokio::test]
async fn notification_routing_content_without_recipient_records_reason_and_domain_reference() {
    let (directory, store) = delivery_tracking_fixture().await;
    use ticket::TicketBackend;
    let backend =
        ticket::SqliteTicketBackend::open_verified(directory.path().join("workspace.db"), "w")
            .unwrap();
    backend
        .edit_item(
            "tracking".into(),
            ticket::TicketItemEdit {
                title: Some("No recipient yet".into()),
                author: Some("human".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(
        store
            .list_pending_ticket_worker_notifications("w", 100)
            .unwrap()
            .is_empty()
    );
    store.with_conn(|conn| {
        let count:i64=conn.query_row("SELECT COUNT(*) FROM notification_sources s JOIN notification_decisions d ON d.source_sequence=s.sequence JOIN typed_ticket_content_changes c ON s.input_event_id='ticket-content:' || c.change_id WHERE d.reason='no_recipient' AND s.actor='human'",[],|r|r.get(0))?;
        assert_eq!(count,2);Ok(())
    }).unwrap();
}

#[tokio::test]
async fn notification_routing_clear_retains_original_assignment_and_suppression_reason() {
    let (_directory, store) = delivery_tracking_fixture().await;
    let target = routing_worker_assign(&store, "worker-a");
    routing_ack_worker_pending(&store, &target);
    store
        .clear_current_ticket_role_assignment(
            "w",
            "tracking",
            TicketAssignmentRole::Worker,
            "worker-a",
            "clear-a",
            "clear-a",
            "human",
            "now",
            None,
        )
        .unwrap();
    assert!(
        store
            .list_pending_ticket_worker_notifications("w", 100)
            .unwrap()
            .is_empty()
    );
    store.with_conn(|conn| {
        let row:(String,String)=conn.query_row("SELECT s.assignment_id,d.reason FROM notification_sources s JOIN notification_decisions d ON d.source_sequence=s.sequence WHERE s.input_event_id='assignment-change:clear-a' AND s.assignment_id='worker-a'",[],|r|Ok((r.get(0)?,r.get(1)?)))?;
        assert_eq!(row,("worker-a".into(),"historical_only".into()));Ok(())
    }).unwrap();
}

#[tokio::test]
async fn subject_notification_capture_uses_each_subject_current_body_and_results_never_retarget() {
    let (_directory, store) = subject_notification_store().await;
    subject_notification_assign(&store, "subject-ticket-a", "assigned-a", "subject-a");
    subject_notification_assign(&store, "subject-ticket-b", "assigned-b", "subject-b");
    let report = |operation: &str| server_api::TicketImplementationResultRequest {
        operation_key: operation.into(),
        kind: server_api::TicketImplementationResultKind::Result,
        body: "Saved background result".into(),
        merge_request: None,
    };
    // A Subject body's own result is not sent back to itself. The same Worker can
    // report to another Subject without a global singleton stealing the recipient.
    let body_a = RuntimeWorkerRef::new("r", "worker");
    let own = store
        .record_implementation_result(
            "w",
            "subject-ticket-a",
            &report("own-a"),
            &body_a,
            "subject-binding",
        )
        .unwrap();
    let other = store
        .record_implementation_result(
            "w",
            "subject-ticket-b",
            &report("a-to-b"),
            &body_a,
            "subject-binding",
        )
        .unwrap();
    store
        .list_pending_ticket_assignment_deliveries("w", 100)
        .unwrap();
    store.with_conn(|conn| {
        let own_reason: String = conn.query_row("SELECT d.reason FROM notification_sources s JOIN notification_decisions d ON d.source_sequence=s.sequence WHERE s.input_event_id=?1", [format!("implementation-result:{}", own.result_id)], |row| row.get(0))?;
        assert_eq!(own_reason, "self_notification");
        let recipient: (String, String, String, String) = conn.query_row("SELECT s.assignment_id,json_extract(s.facts_json,'$.principal_id'),json_extract(s.facts_json,'$.worker_id'),d.reason FROM notification_sources s JOIN notification_decisions d ON d.source_sequence=s.sequence WHERE s.input_event_id=?1", [format!("implementation-result:{}", other.result_id)], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?;
        assert_eq!(recipient, ("assigned-b".into(), "subject-b".into(), "body-b".into(), "result".into()));
        Ok(())
    }).unwrap();
    subject_notification_replace_body(&store, "subject-b");
    let replacement = RuntimeWorkerRef::new("r", "body-replacement");
    let self_after_swap = store
        .record_implementation_result(
            "w",
            "subject-ticket-b",
            &report("new-body-self"),
            &replacement,
            "subject-binding",
        )
        .unwrap();
    store
        .list_pending_ticket_assignment_deliveries("w", 100)
        .unwrap();
    store.with_conn(|conn| {
        let self_reason: String = conn.query_row("SELECT d.reason FROM notification_sources s JOIN notification_decisions d ON d.source_sequence=s.sequence WHERE s.input_event_id=?1", [format!("implementation-result:{}", self_after_swap.result_id)], |row| row.get(0))?;
        assert_eq!(self_reason, "self_notification");
        Ok(())
    }).unwrap();
    subject_notification_assign(&store, "subject-ticket-b", "new-a-assignment", "subject-a");
    assert!(
        !store
            .list_pending_ticket_assignment_deliveries("w", 100)
            .unwrap()
            .iter()
            .any(|input| input.input_event_id
                == format!("implementation-result:{}", other.result_id))
    );
    // Replaying the saved report is still evidence about the original assignment.
    assert_eq!(
        store
            .record_implementation_result(
                "w",
                "subject-ticket-b",
                &report("a-to-b"),
                &body_a,
                "subject-binding"
            )
            .unwrap(),
        other
    );
    store
        .with_conn(|conn| {
            let recipient: String = conn.query_row(
                "SELECT assignment_id FROM notification_sources WHERE input_event_id=?1",
                [format!("implementation-result:{}", other.result_id)],
                |row| row.get(0),
            )?;
            assert_eq!(recipient, "assigned-b");
            Ok(())
        })
        .unwrap();
}

#[test]
fn subject_notification_upgrade_preserves_legacy_sources_requests_and_receipts_and_rolls_back() {
    let directory = tempfile::tempdir().unwrap();
    let conn = tracking_schema91_fixture(&directory.path().join("workspace.db"));
    migrate_ticket_tracking_v91_to_v92(&conn).unwrap();
    migrate_assignment_delivery_v92_to_v93(&conn).unwrap();
    migrate_ticket_posts_v93_to_v94(&conn).unwrap();
    migrate_notification_routing_v94_to_v95(&conn).unwrap();
    migrate_job_notifications_v95_to_v96(&conn).unwrap();
    migrate_business_notifications_v96_to_v97(&conn).unwrap();
    migrate_result_reports_v97_to_v98(&conn).unwrap();
    let legacy_source = TicketAssignmentTrackingSourceContext {
        actor: "legacy-delivery".into(),
        runtime_id: "old-runtime".into(),
        worker_id: "old-worker".into(),
        subject_id: "workspace-orchestrator".into(),
        authentication_binding_id: "old-binding".into(),
    };
    let delivery = TicketAssignmentDelivery {
        ticket_id: "pending".into(),
        assignment_id: "a-pending".into(),
        input_event_id: "assignment-event:migration-tracking:a-pending".into(),
        delivery_id: "legacy-delivery".into(),
        request_id: "legacy-request".into(),
        waiting_reason: None,
        next_action: None,
    };
    conn.execute(
        "INSERT INTO ticket_assignment_deliveries VALUES('w','a-pending',?1,?2,?3,?4)",
        params![
            delivery.input_event_id,
            delivery.delivery_id,
            serde_json::to_string(&delivery).unwrap(),
            serde_json::to_string(&legacy_source).unwrap()
        ],
    )
    .unwrap();
    conn.execute("INSERT INTO ticket_assignment_delivery_requests VALUES('w','legacy-delivery','legacy bytes')", []).unwrap();
    conn.execute(
        "INSERT INTO ticket_assignment_delivery_acknowledgements VALUES('w','legacy-delivery',?1)",
        [serde_json::to_string(&legacy_source).unwrap()],
    )
    .unwrap();
    let tables = [
        "ticket_worker_assignments",
        "notification_sources",
        "notification_decisions",
        "ticket_assignment_deliveries",
        "ticket_assignment_delivery_requests",
        "ticket_assignment_delivery_acknowledgements",
    ];
    let before: Vec<_> = tables
        .iter()
        .map(|table| stored_fixture_rows(&conn, table))
        .collect();
    let triggers_before = fixture_query_rows(
        &conn,
        "SELECT name,sql FROM sqlite_schema WHERE type='trigger' ORDER BY name",
    );
    // Fail after some triggers were refreshed. The caller's transaction restores
    // the entire previous capture schema, not a half-upgraded notification path.
    {
        let tx = conn.unchecked_transaction().unwrap();
        tx.execute_batch("DROP TRIGGER notification_mr_event; CREATE TRIGGER notification_mr_event AFTER INSERT ON merge_request_thread_events BEGIN SELECT 1; END;").unwrap();
        assert!(notification_routing::install_subject_recipients(&tx).is_err());
    }
    assert_eq!(
        fixture_query_rows(
            &conn,
            "SELECT name,sql FROM sqlite_schema WHERE type='trigger' ORDER BY name"
        ),
        triggers_before
    );
    let tx = conn.unchecked_transaction().unwrap();
    notification_routing::install_subject_recipients(&tx).unwrap();
    tx.commit().unwrap();
    let after: Vec<_> = tables
        .iter()
        .map(|table| stored_fixture_rows(&conn, table))
        .collect();
    assert_eq!(after, before, "migration updates future capture only");
    for trigger in [
        "notification_ticket_event",
        "notification_assignment_history",
        "notification_assignment_change",
        "notification_assignment_release",
        "notification_content_input",
        "notification_unassigned_content",
        "notification_mr_event",
        "notification_mr_source",
        "notification_implementation_result",
    ] {
        let sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE name=?1",
                [trigger],
                |row| row.get(0),
            )
            .unwrap();
        assert!(sql.contains("a.principal_kind='subject'"), "{trigger}");
        assert!(
            !sql.contains("a.principal_kind='workspace_agent'"),
            "{trigger}"
        );
    }
    assert!(
        notification_routing::pending_sources(&conn, "w", "orchestrator", 100)
            .unwrap()
            .is_empty(),
        "old principals and receipts are evidence, not new Subject requests"
    );
}

#[tokio::test]
async fn notification_routing_cached_worker_attention_stops_after_ack_not_after_unknown_acceptance()
{
    let (directory, store) = delivery_tracking_fixture().await;
    let target = routing_worker_assign(&store, "worker-a");
    let cached = store
        .list_pending_ticket_worker_notifications("w", 100)
        .unwrap()
        .remove(0);
    assert!(
        notification_routing::admit_worker_send(&store, "w", &cached.delivery_id, &target).is_err()
    );
    store
        .prepare_ticket_worker_notification_content(
            "w",
            &cached.delivery_id,
            "pinned attention",
            &target,
        )
        .unwrap();
    let mut sends = Vec::new();
    let mut dispatch_cached = |store: &SqliteWorkspaceStore| {
        if let Some(content) =
            notification_routing::admit_worker_send(store, "w", &cached.delivery_id, &target)
                .unwrap()
        {
            sends.push((cached.request_id.clone(), content));
        }
    };
    dispatch_cached(&store);
    store
        .record_notification_delivery_failure(
            "w",
            &NotificationDeliveryKey::WorkerNotification {
                delivery_id: cached.delivery_id.clone(),
            },
            NotificationFailureCode::AcceptanceUnknown,
        )
        .unwrap();
    assert_eq!(
        store
            .prepare_ticket_worker_notification_content(
                "w",
                &cached.delivery_id,
                "different rendering",
                &target
            )
            .unwrap(),
        "pinned attention"
    );
    dispatch_cached(&store);
    let other = SqliteWorkspaceStore::open_for_test(directory.path().join("workspace.db")).unwrap();
    other
        .ack_ticket_worker_notification("w", &cached.delivery_id, &target)
        .unwrap();
    dispatch_cached(&store);
    assert_eq!(
        sends,
        vec![(cached.request_id.clone(), "pinned attention".into()); 2]
    );
    assert!(matches!(
        store.prepare_ticket_worker_notification_content(
            "w",
            &cached.delivery_id,
            "late render",
            &target
        ),
        Err(Error::TicketAssignmentConflict(_))
    ));
    store
        .ack_ticket_worker_notification("w", &cached.delivery_id, &target)
        .unwrap();
    assert!(
        store
            .list_pending_ticket_worker_notifications("w", 100)
            .unwrap()
            .is_empty()
    );
    store.with_conn(|conn| {
        let content: String = conn.query_row("SELECT content FROM notification_requests WHERE workspace_id='w' AND delivery_id=?1", [&cached.delivery_id], |row| row.get(0))?;
        assert_eq!(content, "pinned attention", "historical request bytes remain readable");
        Ok(())
    }).unwrap();
}
