// Production SQLite/store tests; signed HTTP and persistent Worker delivery are tested
// in server_assignment_delivery_tests. These do not substitute for caller authentication.
fn implementation_result_input() -> server_api::TicketImplementationResultRequest {
    server_api::TicketImplementationResultRequest {
        operation_key: "result-operation".into(),
        assignment_id: "implementation".into(),
        orchestrator_assignment_id: "delegation".into(),
        kind: server_api::TicketImplementationResultKind::Result,
        body: "Research complete; no repository was required".into(),
        merge_request: None,
    }
}

#[tokio::test]
async fn implementation_result_replay_is_atomic_and_cannot_be_retargeted() {
    let (_dir, store) = tracking_store_fixture().await;
    tracking_store_assign(&store, "delegation", "assign");
    let worker = routing_worker_assign(&store, "implementation").worker;
    let input = implementation_result_input();
    let before = store
        .get_ticket_assignment_tracking("w", "tracking", "delegation")
        .unwrap();
    let receipt = store
        .record_implementation_result("w", "tracking", &input, &worker, "tracking-binding")
        .unwrap();
    assert_eq!(
        store
            .record_implementation_result("w", "tracking", &input, &worker, "tracking-binding")
            .unwrap(),
        receipt
    );
    assert_eq!(
        store
            .implementation_result("w", "tracking", &receipt.result_id)
            .unwrap(),
        receipt
    );
    assert!(
        store
            .implementation_result("w", "other", &receipt.result_id)
            .is_err()
    );
    assert_eq!(
        store
            .get_ticket_assignment_tracking("w", "tracking", "delegation")
            .unwrap(),
        before
    );
    let mut changed = input.clone();
    changed.body.push('!');
    assert!(
        store
            .record_implementation_result("w", "tracking", &changed, &worker, "tracking-binding")
            .is_err()
    );
    for (workspace, ticket, source, binding) in [
        ("other", "tracking", worker.clone(), "tracking-binding"),
        ("w", "other", worker.clone(), "tracking-binding"),
        (
            "w",
            "tracking",
            RuntimeWorkerRef::new("r", "worker"),
            "tracking-binding",
        ),
        ("w", "tracking", worker.clone(), "wrong-binding"),
    ] {
        assert!(
            store
                .record_implementation_result(workspace, ticket, &input, &source, binding)
                .is_err()
        );
    }
    store
        .with_conn(|conn| {
            assert_eq!(
                conn.query_row(
                    "SELECT count(*) FROM ticket_implementation_results",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                1
            );
            assert_eq!(
                conn.query_row(
                    "SELECT count(*) FROM notification_sources WHERE kind='implementation_result'",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                1
            );
            Ok(())
        })
        .unwrap();
    let pending = store
        .list_pending_ticket_assignment_deliveries("w", 100)
        .unwrap();
    assert!(
        pending
            .iter()
            .any(|p| p.input_event_id == format!("implementation-result:{}", receipt.result_id))
    );
    // Reassignment does not rewrite a receipt or deliver old results as new work.
    tracking_store_assign(&store, "replacement-delegation", "replace");
    assert_eq!(
        store
            .record_implementation_result("w", "tracking", &input, &worker, "tracking-binding")
            .unwrap(),
        receipt
    );
    changed.operation_key = "late-result".into();
    assert!(
        store
            .record_implementation_result("w", "tracking", &changed, &worker, "tracking-binding")
            .is_err()
    );
    changed.orchestrator_assignment_id = "replacement-delegation".into();
    assert!(
        store
            .record_implementation_result("w", "tracking", &changed, &worker, "tracking-binding")
            .is_err()
    );
    assert!(
        !store
            .list_pending_ticket_assignment_deliveries("w", 100)
            .unwrap()
            .iter()
            .any(|p| p.input_event_id.starts_with("implementation-result:"))
    );
}

#[tokio::test]
async fn implementation_result_rejects_forged_evidence_and_rolls_back_without_notification() {
    let (_dir, store) = tracking_store_fixture().await;
    tracking_store_assign(&store, "delegation", "assign");
    let worker = routing_worker_assign(&store, "implementation").worker;
    let mut input = implementation_result_input();
    input.merge_request = Some(server_api::MergeRequestReviewSubject {
        merge_request_id: "fake".into(),
        subject_ref: "fake".into(),
    });
    assert!(
        store
            .record_implementation_result("w", "tracking", &input, &worker, "tracking-binding")
            .is_err()
    );
    store
        .with_conn(|conn| {
            assert_eq!(
                conn.query_row(
                    "SELECT count(*) FROM ticket_implementation_results",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                0
            );
            assert_eq!(
                conn.query_row(
                    "SELECT count(*) FROM notification_sources WHERE kind='implementation_result'",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                0
            );
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn mr_notification_upgrade_recovers_events_not_comments_and_retains_unknown_legacy_results() {
    let (_dir, store) = tracking_store_fixture().await;
    tracking_store_assign(&store, "delegation", "assign");
    routing_worker_assign(&store, "implementation");
    routing_worker_assign(&store, "known-implementation");
    store.with_conn(|conn| {
        conn.execute("INSERT INTO ticket_assignment_operation_fences VALUES('w','operation-known-implementation','delegation','r','worker','workspace-orchestrator','tracking-binding',?1)",[serde_json::to_string(&tracking_store_source()).unwrap()])?;
        // Reconstruct the prior composition before its new business capture existed.
        conn.execute_batch("DROP TRIGGER notification_mr_event; DROP TRIGGER notification_mr_source; DROP TRIGGER notification_implementation_result; DROP TRIGGER implementation_delegation_capture;
            DROP TABLE ticket_implementation_results; DROP TABLE ticket_implementation_delegations; DROP TABLE merge_request_source_observations;
            UPDATE __yoi_schema_migrations SET version=95 WHERE version=96;")?;
        conn.execute_batch("INSERT INTO merge_requests VALUES('w','historical-mr','main','open','refs/heads/work','refs/heads/main','3','3');
            INSERT INTO merge_request_ticket_relations VALUES('w','historical-mr','tracking','implements','3');
            INSERT INTO merge_request_thread_events VALUES('w','historical-mr','approved-event',1,'review','{\"subject_ref\":\"source-a\",\"decision\":\"approve\"}',NULL,'3');
            INSERT INTO merge_request_thread_events VALUES('w','historical-mr','old-comment',2,'comment','{\"body\":\"handoff done\"}',NULL,'3');")?;
        conn.execute_batch("CREATE TABLE ticket_implementation_results(collision INTEGER);")?;
        assert!(migrate_business_notifications_v95_to_v96(conn).is_err());
        assert!(!table_exists(conn,"merge_request_source_observations")?);
        assert!(!table_exists(conn,"ticket_implementation_delegations")?);
        assert_eq!(current_schema_version(conn)?,95);
        conn.execute_batch("DROP TABLE ticket_implementation_results;")?;
        migrate_business_notifications_v95_to_v96(conn)?;
        assert_eq!(conn.query_row("SELECT count(*) FROM notification_sources WHERE input_event_id='mr-event:historical-mr:approved-event'",[],|r|r.get::<_,i64>(0))?,2);
        assert_eq!(conn.query_row("SELECT assignment_id FROM notification_sources WHERE input_event_id='mr-event:historical-mr:approved-event' AND role='worker'",[],|r|r.get::<_,String>(0))?,"", "equal legacy assignment timestamps must not guess ownership");
        assert_eq!(conn.query_row("SELECT count(*) FROM notification_sources WHERE input_event_id LIKE '%old-comment%'",[],|r|r.get::<_,i64>(0))?,0);
        assert_eq!(conn.query_row("SELECT orchestrator_assignment_id FROM ticket_implementation_delegations WHERE assignment_id='implementation'",[],|r|r.get::<_,Option<String>>(0))?,None);
        assert_eq!(conn.query_row("SELECT orchestrator_assignment_id FROM ticket_implementation_delegations WHERE assignment_id='known-implementation'",[],|r|r.get::<_,Option<String>>(0))?,Some("delegation".into()));
        assert!(conn.execute("UPDATE ticket_implementation_delegations SET orchestrator_assignment_id='delegation'",[]).is_err());
        assert_eq!(conn.query_row("SELECT count(*) FROM pragma_foreign_key_check",[],|r|r.get::<_,i64>(0))?,0);
        Ok(())
    }).unwrap();
    let inputs = store
        .list_pending_ticket_assignment_deliveries("w", 100)
        .unwrap();
    assert!(
        inputs
            .iter()
            .any(|i| i.input_event_id == "mr-event:historical-mr:approved-event")
    );
    for _ in 0..2 {
        store
            .observe_merge_request_source("w", "main", "refs/heads/work", "source-b")
            .unwrap();
    }
    store
        .with_conn(|conn| {
            assert_eq!(
                conn.query_row(
                    "SELECT count(*) FROM notification_sources WHERE kind='mr_source_changed'",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                2
            );
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn implementation_result_notification_failure_rolls_back_and_concurrent_retries_share_receipt()
 {
    let (dir, store) = tracking_store_fixture().await;
    tracking_store_assign(&store, "delegation", "assign");
    let worker = routing_worker_assign(&store, "implementation").worker;
    let input = implementation_result_input();
    store.with_conn(|conn| {
        conn.execute_batch("CREATE TRIGGER reject_result_notification BEFORE INSERT ON notification_sources WHEN NEW.kind='implementation_result' BEGIN SELECT RAISE(ABORT,'injected capture failure'); END;")?;
        Ok(())
    }).unwrap();
    assert!(
        store
            .record_implementation_result("w", "tracking", &input, &worker, "tracking-binding")
            .is_err()
    );
    store
        .with_conn(|conn| {
            assert_eq!(
                conn.query_row(
                    "SELECT count(*) FROM ticket_implementation_results",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                0
            );
            conn.execute_batch("DROP TRIGGER reject_result_notification;")?;
            Ok(())
        })
        .unwrap();
    let barrier = std::sync::Barrier::new(4);
    let path = dir.path().join("workspace.db");
    let receipts = std::thread::scope(|scope| {
        let handles = (0..4)
            .map(|_| {
                let worker = &worker;
                let input = &input;
                let path = &path;
                let barrier = &barrier;
                scope.spawn(move || {
                    let store = SqliteWorkspaceStore::open_for_test(path).unwrap();
                    barrier.wait();
                    store
                        .record_implementation_result(
                            "w",
                            "tracking",
                            input,
                            worker,
                            "tracking-binding",
                        )
                        .unwrap()
                })
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert!(receipts.iter().all(|r| r == &receipts[0]));
    store
        .with_conn(|conn| {
            assert_eq!(
                conn.query_row(
                    "SELECT count(*) FROM ticket_implementation_results",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                1
            );
            assert_eq!(
                conn.query_row(
                    "SELECT count(*) FROM notification_sources WHERE kind='implementation_result'",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                1
            );
            Ok(())
        })
        .unwrap();
}
