// Production SQLite/store tests; signed HTTP and persistent Worker delivery are tested
// in server_assignment_delivery_tests. These do not substitute for caller authentication.
fn implementation_result_input() -> server_api::TicketImplementationResultRequest {
    server_api::TicketImplementationResultRequest {
        operation_key: "result-operation".into(),
        kind: server_api::TicketImplementationResultKind::Result,
        body: "Research complete; no repository was required".into(),
        merge_request: None,
    }
}

fn implementation_result_business_state(
    conn: &Connection,
) -> Vec<Vec<Vec<rusqlite::types::Value>>> {
    [
        "typed_tickets",
        "typed_ticket_events",
        "typed_ticket_event_attributes",
        "ticket_worker_assignments",
        "ticket_current_worker_assignments",
        "ticket_worker_assignment_events",
        "typed_ticket_assignment_tracking_history",
        "ticket_assignment_work_releases",
        "worker_control_grants",
        "ticket_implementation_delegations",
    ]
    .map(|table| fixture_query_rows(conn, &format!("SELECT * FROM {table} ORDER BY rowid")))
    .into_iter()
    .collect()
}

fn implementation_result_notification_evidence(
    conn: &Connection,
) -> Vec<Vec<Vec<rusqlite::types::Value>>> {
    [
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
    .map(|table| stored_fixture_rows(conn, table))
    .into_iter()
    .collect()
}

#[tokio::test]
async fn implementation_result_without_any_assignment_is_authenticated_and_never_retargeted() {
    let (dir, store) = tracking_store_fixture().await;
    let worker = RuntimeWorkerRef::new("r", "replacement");
    let input = implementation_result_input();
    let before = store.with_conn(|conn| {
        // Retention holds are not reporting permissions.
        conn.execute("UPDATE worker_registry SET retention_state='pinned' WHERE workspace_id='w' AND worker_id='replacement'", [])?;
        assert_eq!(current_schema_version(conn)?, LATEST_SCHEMA_VERSION);
        assert_eq!(conn.query_row("SELECT count(*) FROM ticket_worker_assignments", [], |r| r.get::<_, i64>(0))?, 0);
        assert_eq!(conn.query_row("SELECT count(*) FROM pragma_table_info('ticket_implementation_results') WHERE name IN ('assignment_id','orchestrator_assignment_id')", [], |r| r.get::<_, i64>(0))?, 0);
        assert_eq!(conn.query_row("SELECT count(*) FROM sqlite_master WHERE type='trigger' AND name='implementation_delegation_capture'", [], |r| r.get::<_, i64>(0))?, 0);
        Ok(implementation_result_business_state(conn))
    }).unwrap();
    let receipt = store
        .record_implementation_result("w", "tracking", &input, &worker, "tracking-binding")
        .unwrap();
    assert_eq!(
        store
            .with_conn(|conn| Ok(implementation_result_business_state(conn)))
            .unwrap(),
        before
    );
    assert_eq!(
        store
            .implementation_result("w", "tracking", &receipt.result_id)
            .unwrap(),
        receipt
    );
    // A second, independently authenticated Worker cannot reuse this Workspace operation key.
    let other = RuntimeWorkerRef::new("r", "worker");
    assert!(
        store
            .record_implementation_result("w", "tracking", &input, &other, "tracking-binding")
            .is_err()
    );
    let mut other_input = input.clone();
    other_input.operation_key = "other-worker-result".into();
    store
        .record_implementation_result("w", "tracking", &other_input, &other, "tracking-binding")
        .unwrap();
    seed_manual_start_ticket(&store, "other-ticket", "open");
    assert!(
        store
            .record_implementation_result("w", "other-ticket", &input, &worker, "tracking-binding")
            .is_err()
    );
    let mut missing = input.clone();
    missing.operation_key = "missing-ticket-result".into();
    assert!(
        store
            .record_implementation_result("w", "missing", &missing, &worker, "tracking-binding")
            .is_err()
    );
    assert!(
        !store
            .list_pending_ticket_assignment_deliveries("w", 100)
            .unwrap()
            .iter()
            .any(|p| p.input_event_id.starts_with("implementation-result:"))
    );
    let evidence = store.with_conn(|conn| {
        let source: (String, String, String) = conn.query_row(
            "SELECT s.assignment_id,d.disposition,d.reason FROM notification_sources s JOIN notification_decisions d ON d.source_sequence=s.sequence WHERE s.input_event_id=?1",
            [format!("implementation-result:{}", receipt.result_id)], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        assert_eq!(source, ("".into(), "suppress".into(), "no_recipient".into()));
        Ok((
            fixture_query_rows(conn, "SELECT * FROM notification_sources WHERE kind='implementation_result' ORDER BY sequence"),
            fixture_query_rows(conn, "SELECT d.* FROM notification_decisions d JOIN notification_sources s ON s.sequence=d.source_sequence WHERE s.kind='implementation_result' ORDER BY s.sequence"),
        ))
    }).unwrap();
    // A later Orchestrator must not acquire previously recipient-less reports.
    tracking_store_assign(&store, "late-delegation", "assign-late");
    assert_eq!(
        store
            .record_implementation_result("w", "tracking", &input, &worker, "tracking-binding")
            .unwrap(),
        receipt
    );
    assert!(
        !store
            .list_pending_ticket_assignment_deliveries("w", 100)
            .unwrap()
            .iter()
            .any(|p| p.input_event_id.starts_with("implementation-result:"))
    );
    store.with_conn(|conn| {
        assert_eq!(conn.query_row("SELECT count(*) FROM notification_sources WHERE kind='implementation_result'", [], |r| r.get::<_, i64>(0))?, 2);
        // Assignment creation has its own evidence; old report evidence stays byte-for-byte intact.
        assert_eq!(fixture_query_rows(conn, "SELECT * FROM notification_sources WHERE kind='implementation_result' ORDER BY sequence"), evidence.0);
        assert_eq!(fixture_query_rows(conn, "SELECT d.* FROM notification_decisions d JOIN notification_sources s ON s.sequence=d.source_sequence WHERE s.kind='implementation_result' ORDER BY s.sequence"), evidence.1);
        Ok(())
    }).unwrap();
    let reopened = SqliteWorkspaceStore::open_for_test(dir.path().join("workspace.db")).unwrap();
    assert_eq!(
        reopened
            .record_implementation_result("w", "tracking", &input, &worker, "tracking-binding")
            .unwrap(),
        receipt
    );
    assert_eq!(
        reopened
            .implementation_result("w", "tracking", &receipt.result_id)
            .unwrap(),
        receipt
    );
}

#[tokio::test]
async fn implementation_result_requires_live_binding_and_registered_worker_even_for_replay() {
    for change in [
        "revoked",
        "binding-changed",
        "unregistered",
        "foreign-worker",
        "foreign-binding",
    ] {
        let (_dir, store) = tracking_store_fixture().await;
        store
            .upsert_workspace(&WorkspaceRecord {
                workspace_id: "other".into(),
                owner_account_id: "owner-account".into(),
                display_name: "Other".into(),
                state: "active".into(),
                created_at: "1".into(),
                updated_at: "1".into(),
            })
            .await
            .unwrap();
        let worker = RuntimeWorkerRef::new("r", "replacement");
        let input = implementation_result_input();
        store
            .record_implementation_result("w", "tracking", &input, &worker, "tracking-binding")
            .unwrap();
        store.with_conn(|conn| {
            match change {
                "revoked" => { conn.execute("UPDATE workspace_runtime_bindings SET revoked_at='2' WHERE workspace_id='w'", [])?; }
                "binding-changed" => { conn.execute("UPDATE workspace_runtime_bindings SET binding_id='new-binding' WHERE workspace_id='w'", [])?; }
                "unregistered" => { conn.execute("DELETE FROM worker_registry WHERE workspace_id='w' AND worker_id='replacement'", [])?; }
                "foreign-worker" => { conn.execute("UPDATE worker_registry SET workspace_id='other' WHERE workspace_id='w' AND worker_id='replacement'", [])?; }
                "foreign-binding" => { conn.execute("UPDATE workspace_runtime_bindings SET workspace_id='other' WHERE workspace_id='w'", [])?; }
                _ => unreachable!(),
            }
            Ok(())
        }).unwrap();
        let mut fresh = input.clone();
        fresh.operation_key = "fresh-after-auth-change".into();
        for request in [&input, &fresh] {
            assert!(
                matches!(
                    store.record_implementation_result(
                        "w",
                        "tracking",
                        request,
                        &worker,
                        "tracking-binding"
                    ),
                    Err(Error::WorkspacePermissionDenied(_))
                ),
                "{change}: {request:?}"
            );
        }
        store.with_conn(|conn| {
            assert_eq!(conn.query_row("SELECT count(*) FROM ticket_implementation_results", [], |r| r.get::<_, i64>(0))?, 1);
            assert_eq!(conn.query_row("SELECT count(*) FROM notification_sources WHERE kind='implementation_result'", [], |r| r.get::<_, i64>(0))?, 1);
            Ok(())
        }).unwrap();
    }
}

#[tokio::test]
async fn implementation_result_late_reports_do_not_change_ticket_assignment_or_handling_state() {
    for situation in [
        "never-assigned",
        "released",
        "reassigned",
        "closed",
        "no-orchestrator",
        "assigned-before-orchestrator",
    ] {
        let (_dir, store) = tracking_store_fixture().await;
        if situation != "assigned-before-orchestrator" {
            tracking_store_assign(&store, "delegation", "assign");
        }
        let worker = if situation == "never-assigned" {
            RuntimeWorkerRef::new("r", "replacement")
        } else {
            routing_worker_assign(&store, "implementation").worker
        };
        match situation {
            "never-assigned" => {}
            "released" => {
                assert!(
                    store
                        .clear_current_ticket_role_assignment(
                            "w",
                            "tracking",
                            TicketAssignmentRole::Worker,
                            "implementation",
                            "release-event",
                            "release-op",
                            "human",
                            "3",
                            Some("Released")
                        )
                        .unwrap()
                );
            }
            "reassigned" => {
                store
                    .set_current_ticket_role_assignment(
                        &manual_worker_assignment("tracking", "replacement-implementation"),
                        Some("implementation"),
                        "reassign-event",
                        "reassign-op",
                        true,
                    )
                    .unwrap();
            }
            "closed" => {
                store.with_conn(|conn| {
                    conn.execute("UPDATE typed_tickets SET workflow_state='closed' WHERE workspace_id='w' AND ticket_id='tracking'", [])?;
                    Ok(())
                }).unwrap();
            }
            "no-orchestrator" => {
                assert!(
                    store
                        .clear_current_ticket_role_assignment(
                            "w",
                            "tracking",
                            TicketAssignmentRole::Orchestrator,
                            "delegation",
                            "release-orchestrator-event",
                            "release-orchestrator-op",
                            "human",
                            "3",
                            Some("Returned to human")
                        )
                        .unwrap()
                );
            }
            "assigned-before-orchestrator" => {
                tracking_store_assign(&store, "late-delegation", "assign-late");
            }
            _ => unreachable!(),
        }
        let before = store
            .with_conn(|conn| Ok(implementation_result_business_state(conn)))
            .unwrap();
        let input = implementation_result_input();
        let receipt = store
            .record_implementation_result("w", "tracking", &input, &worker, "tracking-binding")
            .unwrap();
        assert_eq!(
            store
                .record_implementation_result("w", "tracking", &input, &worker, "tracking-binding")
                .unwrap(),
            receipt
        );
        store.with_conn(|conn| {
            assert_eq!(implementation_result_business_state(conn), before, "{situation}");
            let expected = match situation {
                "no-orchestrator" => "",
                "assigned-before-orchestrator" => "late-delegation",
                _ => "delegation",
            };
            assert_eq!(conn.query_row("SELECT assignment_id FROM notification_sources WHERE kind='implementation_result'", [], |r| r.get::<_, String>(0))?, expected);
            assert_eq!(conn.query_row("SELECT count(*) FROM ticket_implementation_delegations", [], |r| r.get::<_, i64>(0))?, 0, "new assignments must not create historical delegation evidence");
            Ok(())
        }).unwrap();
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
    let mut changed_kind = input.clone();
    changed_kind.kind = server_api::TicketImplementationResultKind::Blocker;
    assert!(
        store
            .record_implementation_result(
                "w",
                "tracking",
                &changed_kind,
                &worker,
                "tracking-binding"
            )
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
    assert!(
        !store
            .list_pending_ticket_assignment_deliveries("w", 100)
            .unwrap()
            .iter()
            .any(|p| p.input_event_id == format!("implementation-result:{}", receipt.result_id))
    );
    changed.operation_key = "late-result".into();
    let late = store
        .record_implementation_result("w", "tracking", &changed, &worker, "tracking-binding")
        .unwrap();
    assert_ne!(late.result_id, receipt.result_id);
    store.with_conn(|conn| {
        for (result, assignment) in [
            (&receipt.result_id, "delegation"),
            (&late.result_id, "replacement-delegation"),
        ] {
            assert_eq!(conn.query_row(
                "SELECT assignment_id FROM notification_sources WHERE input_event_id=?1 AND role='orchestrator'",
                [format!("implementation-result:{result}")], |r| r.get::<_, String>(0))?, assignment);
            assert_eq!(conn.query_row(
                "SELECT count(*) FROM notification_sources WHERE input_event_id=?1",
                [format!("implementation-result:{result}")], |r| r.get::<_, i64>(0))?, 1);
        }
        Ok(())
    }).unwrap();
    assert!(
        store
            .list_pending_ticket_assignment_deliveries("w", 100)
            .unwrap()
            .iter()
            .any(|p| p.input_event_id == format!("implementation-result:{}", late.result_id))
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

// Historical migration fixtures start from the retained Workspace96/component13
// composition. Wrapping the actual connection is test-only SQL access, not a
// current-schema open or any claim of authenticated legacy execution authority.
fn implementation_legacy96_fixture() -> (tempfile::TempDir, SqliteWorkspaceStore) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("workspace.db");
    prepare_retained_schema(&path, 96);
    let conn = Connection::open(&path).unwrap();
    configure_sqlite(&conn).unwrap();
    assert_eq!(current_schema_version(&conn).unwrap(), 96);
    assert_eq!(
        conn.query_row(
            "SELECT max(version) FROM ticket_schema_migrations",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        13
    );
    conn.execute_batch("INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at) VALUES('legacy-owner','user','legacy-owner','Legacy owner','1','1');
        INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at) VALUES('w','legacy-owner','Legacy workspace','active','1','1');
        INSERT INTO worker_registry(workspace_id,runtime_id,worker_id,display_name,retention_state,created_at,updated_at) VALUES('w','r','worker','Legacy body','normal','1','1'),('w','r','replacement','Implementation','normal','1','1');
        INSERT INTO workspace_runtime_bindings(workspace_id,runtime_id,display_name,base_url,public_key,public_key_fingerprint,binding_id,authentication_mode,created_at,updated_at) VALUES('w','r','Runtime','https://runtime.invalid','retained-key','retained-fingerprint','tracking-binding','workspace_identity','1','1');
        INSERT INTO worker_singleton_owners(workspace_id,singleton_key,runtime_id,worker_id,generation,created_at,updated_at) VALUES('w','workspace-orchestrator','r','worker',1,'1','1');
        INSERT INTO repositories(workspace_id,repository_id,repository_key,kind,provider,uri,created_at,updated_at,source_kind,source_uri,source_fingerprint) VALUES('w','main','main','git','git','file:///main','1','1','file','file:///main','sha256:fixture');").unwrap();
    let store = SqliteWorkspaceStore {
        conn: Arc::new(Mutex::new(conn)),
        db_executor: crate::db_executor::DbExecutor::new(
            crate::db_executor::DbExecutorLimits::default(),
        )
        .unwrap(),
    };
    seed_manual_start_ticket(&store, "tracking", "open");
    store.with_conn(|conn| {
        // The old principal and normal assignment triggers are genuine retained
        // data. Current assignment APIs intentionally cannot create this identity.
        conn.execute_batch("INSERT INTO ticket_worker_assignments(workspace_id,ticket_id,assignment_id,role,principal_kind,principal_id,assigned_by,assigned_at) VALUES('w','tracking','delegation','orchestrator','workspace_agent','workspace-orchestrator','legacy-owner','2');
            INSERT INTO ticket_current_worker_assignments(workspace_id,ticket_id,assignment_id,role,principal_kind,principal_id,updated_at) VALUES('w','tracking','delegation','orchestrator','workspace_agent','workspace-orchestrator','2');
            INSERT INTO ticket_worker_assignment_events(workspace_id,ticket_id,role,event_id,action,assignment_id,actor,created_at,operation_id) VALUES('w','tracking','orchestrator','legacy-assigned-event','assigned','delegation','legacy-owner','2','legacy-assign');")?;
        Ok(())
    }).unwrap();
    (directory, store)
}

// Seed accepted PRE-upgrade notification evidence as historical SQL data. The
// current router must never authorize workspace-orchestrator to recreate it.
fn implementation_seed_legacy_result_delivery(conn: &Connection, index: usize) -> Result<()> {
    let event = format!("implementation-result:legacy-result-{index}");
    let delivery_id = format!("legacy-result-delivery-{index}");
    let input = crate::store::TicketAssignmentDelivery {
        ticket_id: "tracking".into(),
        assignment_id: "delegation".into(),
        input_event_id: event.clone(),
        delivery_id: delivery_id.clone(),
        request_id: delivery_id.clone(),
        waiting_reason: None,
        next_action: None,
    };
    let source = serde_json::json!({"actor":"backend:assignment-delivery", "runtime_id":"r", "worker_id":"worker", "agent_key":"workspace-orchestrator", "authentication_binding_id":"tracking-binding"}).to_string();
    conn.execute("INSERT INTO notification_decisions(source_sequence,workspace_id,delivery_id,disposition,reason,purpose) SELECT sequence,'w',?1,'deliver','result','result' FROM notification_sources WHERE input_event_id=?2 AND role='orchestrator'", params![delivery_id,event])?;
    conn.execute(
        "INSERT INTO ticket_assignment_deliveries VALUES('w','delegation',?1,?2,?3,?4)",
        params![
            event,
            delivery_id,
            serde_json::to_string(&input).unwrap(),
            source
        ],
    )?;
    conn.execute("INSERT INTO ticket_assignment_delivery_requests VALUES('w',?1,'Retained legacy report notification')", [&delivery_id])?;
    conn.execute(
        "INSERT INTO ticket_assignment_delivery_acknowledgements VALUES('w',?1,?2)",
        params![delivery_id, source],
    )?;
    conn.execute("INSERT INTO notification_assignment_targets VALUES('w',?1,'r','worker','workspace-orchestrator','tracking-binding')", [&delivery_id])?;
    conn.execute(
        "INSERT INTO notification_requests VALUES('w',?1,'Retained legacy report notification',?2)",
        params![delivery_id, source],
    )?;
    conn.execute(
        "INSERT INTO notification_acknowledgements VALUES('w',?1,?2)",
        params![delivery_id, source],
    )?;
    conn.execute("INSERT INTO notification_failures(workspace_id,delivery_id,code,recorded_at) VALUES('w',?1,'runtime_rejected','4')", [&delivery_id])?;
    Ok(())
}

#[tokio::test]
async fn implementation_result_v97_to_v98_preserves_legacy_bytes_receipts_and_notification_evidence()
 {
    let (dir, store) = implementation_legacy96_fixture();
    let worker = routing_worker_assign(&store, "implementation").worker;
    // Apply the real forward 96 -> 97 migration before seeding schema97 reports.
    store
        .with_conn(|conn| {
            migrate_business_notifications_v96_to_v97(conn)?;
            assert_eq!(current_schema_version(conn)?, 97);
            Ok(())
        })
        .unwrap();
    let input = implementation_result_input();
    let mut second = input.clone();
    second.operation_key = "legacy-second-operation".into();
    second.body = "A second retained report".into();
    let requests = [input.clone(), second];
    let raw_requests = requests
        .iter()
        .map(|request| {
            let mut legacy = serde_json::to_value(request).unwrap();
            legacy["assignment_id"] = serde_json::json!("implementation");
            legacy["orchestrator_assignment_id"] = serde_json::json!("delegation");
            format!(" \n{}\n ", serde_json::to_string_pretty(&legacy).unwrap())
        })
        .collect::<Vec<_>>();
    store.with_conn(|conn| {
        for (index, request) in requests.iter().enumerate() {
            conn.execute("INSERT INTO ticket_implementation_results(workspace_id,ticket_id,operation_key,result_id,assignment_id,orchestrator_assignment_id,runtime_id,worker_id,request_json,created_at)
                VALUES('w','tracking',?1,?2,'implementation','delegation','r','replacement',?3,'4')",
                params![request.operation_key, format!("legacy-result-{index}"), raw_requests[index]])?;
        }
        Ok(())
    }).unwrap();
    // Persist real historical row shapes without calling current Subject-only
    // send/reserve APIs as though the legacy identity were live authority.
    store
        .with_conn(|conn| {
            for index in 0..requests.len() {
                implementation_seed_legacy_result_delivery(conn, index)?;
            }
            Ok(())
        })
        .unwrap();
    let (results, business, evidence) = store.with_conn(|conn| {
        Ok((
            fixture_query_rows(conn, "SELECT workspace_id,ticket_id,operation_key,result_id,runtime_id,worker_id,request_json,created_at FROM ticket_implementation_results ORDER BY rowid"),
            implementation_result_business_state(conn),
            implementation_result_notification_evidence(conn),
        ))
    }).unwrap();
    assert!(
        evidence.iter().all(|rows| !rows.is_empty()),
        "fixture must retain nonempty source, decision, request, target, ACK and failure evidence"
    );
    store.with_conn(|conn| {
        // The migration must roll back its trigger drops and leave every byte replayable.
        conn.execute_batch("CREATE TABLE ticket_implementation_results_v98(collision INTEGER);")?;
        assert!(migrate_result_reports_v97_to_v98(conn).is_err());
        assert_eq!(current_schema_version(conn)?, 97);
        assert_eq!(conn.query_row("SELECT count(*) FROM sqlite_master WHERE type='trigger' AND name IN ('notification_implementation_result','implementation_delegation_capture')", [], |r| r.get::<_, i64>(0))?, 2);
        assert_eq!(conn.query_row("SELECT count(*) FROM pragma_table_info('ticket_implementation_results') WHERE name IN ('assignment_id','orchestrator_assignment_id')", [], |r| r.get::<_, i64>(0))?, 2);
        assert_eq!(fixture_query_rows(conn, "SELECT workspace_id,ticket_id,operation_key,result_id,runtime_id,worker_id,request_json,created_at FROM ticket_implementation_results ORDER BY rowid"), results);
        assert_eq!(implementation_result_business_state(conn), business);
        assert_eq!(implementation_result_notification_evidence(conn), evidence);
        conn.execute_batch("DROP TABLE ticket_implementation_results_v98;")?;
        migrate_result_reports_v97_to_v98(conn)?;
        assert_eq!(current_schema_version(conn)?, 98);
        assert!(migrate_result_reports_v97_to_v98(conn).is_err(), "migration rejects the wrong starting version");
        assert_eq!(fixture_query_rows(conn, "SELECT * FROM ticket_implementation_results ORDER BY rowid"), results);
        assert_eq!(implementation_result_business_state(conn), business);
        assert_eq!(implementation_result_notification_evidence(conn), evidence);
        assert_eq!(conn.query_row("SELECT count(*) FROM pragma_table_info('ticket_implementation_results') WHERE name IN ('assignment_id','orchestrator_assignment_id')", [], |r| r.get::<_, i64>(0))?, 0);
        assert_eq!(conn.query_row("SELECT count(*) FROM sqlite_master WHERE type='trigger' AND name='implementation_delegation_capture'", [], |r| r.get::<_, i64>(0))?, 0);
        assert!(conn.execute("UPDATE ticket_implementation_results SET created_at='changed'", []).is_err());
        assert!(conn.execute("DELETE FROM ticket_implementation_results", []).is_err());
        assert!(conn.execute("UPDATE ticket_implementation_delegations SET orchestrator_assignment_id='delegation'", []).is_err());
        assert!(conn.execute("DELETE FROM ticket_implementation_delegations", []).is_err());
        assert!(conn.execute("INSERT INTO ticket_implementation_results SELECT workspace_id,'missing','missing-key','missing-result',runtime_id,worker_id,request_json,created_at FROM ticket_implementation_results LIMIT 1", []).is_err(), "Ticket foreign key remains enforced");
        assert!(conn.execute("INSERT INTO ticket_implementation_results SELECT workspace_id,ticket_id,'duplicate-result-key',result_id,runtime_id,worker_id,request_json,created_at FROM ticket_implementation_results LIMIT 1", []).is_err(), "result identity remains unique");
        assert!(conn.execute("INSERT INTO ticket_implementation_results SELECT workspace_id,ticket_id,operation_key,'duplicate-key-result',runtime_id,worker_id,request_json,created_at FROM ticket_implementation_results LIMIT 1", []).is_err(), "operation key remains unique");
        assert_eq!(conn.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r.get::<_, i64>(0))?, 0);
        Ok(())
    }).unwrap();
    let reopened = SqliteWorkspaceStore::open_for_test(dir.path().join("workspace.db")).unwrap();
    for (index, request) in requests.iter().enumerate() {
        let expected = server_api::TicketImplementationResultReceipt {
            result_id: format!("legacy-result-{index}"),
            ticket_id: "tracking".into(),
            request: request.clone(),
            created_at: "4".into(),
        };
        assert_eq!(
            reopened
                .implementation_result("w", "tracking", &expected.result_id)
                .unwrap(),
            expected
        );
        assert_eq!(
            reopened
                .record_implementation_result("w", "tracking", request, &worker, "tracking-binding")
                .unwrap(),
            expected
        );
    }
    reopened
        .with_conn(|conn| {
            assert_eq!(
                fixture_query_rows(
                    conn,
                    "SELECT * FROM ticket_implementation_results ORDER BY rowid"
                ),
                results
            );
            assert_eq!(
                implementation_result_notification_evidence(conn),
                evidence,
                "reading and replaying legacy JSON must never recapture notifications"
            );
            Ok(())
        })
        .unwrap();
    tracking_store_assign(&reopened, "new-delegation", "assign-new");
    routing_worker_assign(&reopened, "new-implementation");
    reopened
        .with_conn(|conn| {
            assert_eq!(
                stored_fixture_rows(conn, "ticket_implementation_delegations"),
                business[9],
                "historical delegation evidence must not grow"
            );
            for index in 0..requests.len() {
                assert_eq!(
                    conn.query_row(
                        "SELECT assignment_id FROM notification_sources WHERE input_event_id=?1",
                        [format!("implementation-result:legacy-result-{index}")],
                        |r| r.get::<_, String>(0)
                    )?,
                    "delegation"
                );
            }
            Ok(())
        })
        .unwrap();
    for request in &requests {
        reopened
            .record_implementation_result("w", "tracking", request, &worker, "tracking-binding")
            .unwrap();
    }
    let mut fresh = input;
    fresh.operation_key = "new-schema98-report".into();
    let receipt = reopened
        .record_implementation_result("w", "tracking", &fresh, &worker, "tracking-binding")
        .unwrap();
    reopened
        .with_conn(|conn| {
            assert_eq!(
                conn.query_row(
                    "SELECT assignment_id FROM notification_sources WHERE input_event_id=?1",
                    [format!("implementation-result:{}", receipt.result_id)],
                    |r| r.get::<_, String>(0)
                )?,
                "new-delegation"
            );
            for index in 0..requests.len() {
                assert_eq!(
                    conn.query_row(
                        "SELECT request_json FROM ticket_implementation_results WHERE result_id=?1",
                        [format!("legacy-result-{index}")],
                        |r| r.get::<_, String>(0)
                    )?,
                    raw_requests[index]
                );
                assert_eq!(
                    conn.query_row(
                        "SELECT count(*) FROM notification_sources WHERE input_event_id=?1",
                        [format!("implementation-result:legacy-result-{index}")],
                        |r| r.get::<_, i64>(0)
                    )?,
                    1
                );
            }
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn mr_notification_upgrade_recovers_events_not_comments_and_retains_unknown_legacy_results() {
    let (_dir, store) = implementation_legacy96_fixture();
    routing_worker_assign(&store, "implementation");
    routing_worker_assign(&store, "known-implementation");
    store.with_conn(|conn| {
        let legacy_source = serde_json::json!({"actor":"authenticated-worker:r/worker", "runtime_id":"r", "worker_id":"worker", "agent_key":"workspace-orchestrator", "authentication_binding_id":"tracking-binding"});
        conn.execute("INSERT INTO ticket_assignment_operation_fences VALUES('w','operation-known-implementation','delegation','r','worker','workspace-orchestrator','tracking-binding',?1)",[legacy_source.to_string()])?;
        conn.execute_batch("INSERT INTO merge_requests VALUES('w','historical-mr','main','open','refs/heads/work','refs/heads/main','3','3');
            INSERT INTO merge_request_ticket_relations VALUES('w','historical-mr','tracking','implements','3');
            INSERT INTO merge_request_thread_events VALUES('w','historical-mr','approved-event',1,'review','{\"subject_ref\":\"source-a\",\"decision\":\"approve\"}',NULL,'3');
            INSERT INTO merge_request_thread_events VALUES('w','historical-mr','old-comment',2,'comment','{\"body\":\"handoff done\"}',NULL,'3');")?;
        conn.execute_batch("CREATE TABLE ticket_implementation_results(collision INTEGER);")?;
        assert!(migrate_business_notifications_v96_to_v97(conn).is_err());
        assert!(!table_exists(conn,"merge_request_source_observations")?);
        assert!(!table_exists(conn,"ticket_implementation_delegations")?);
        assert_eq!(current_schema_version(conn)?,96);
        conn.execute_batch("DROP TABLE ticket_implementation_results;")?;
        migrate_business_notifications_v96_to_v97(conn)?;
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
            .all(|i| i.input_event_id != "mr-event:historical-mr:approved-event"),
        "retained legacy capture must not become current Subject delivery authority"
    );
    store.with_conn(|conn| {
        let decision: (String, String) = conn.query_row("SELECT d.disposition,d.reason FROM notification_sources s JOIN notification_decisions d ON d.source_sequence=s.sequence WHERE s.input_event_id='mr-event:historical-mr:approved-event' AND s.role='orchestrator'", [], |row| Ok((row.get(0)?,row.get(1)?)))?;
        assert_eq!(decision, ("suppress".into(),"unsupported_recipient".into()));
        Ok(())
    }).unwrap();
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
    let before = store
        .with_conn(|conn| Ok(implementation_result_business_state(conn)))
        .unwrap();
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
            assert_eq!(implementation_result_business_state(conn), before);
            assert_eq!(
                conn.query_row(
                    "SELECT count(*) FROM notification_sources WHERE kind='implementation_result'",
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

#[tokio::test]
async fn mr_event_actors_preserve_self_suppression_in_capture_and_backfill() {
    for historical in [false, true] {
        let (_dir, store) = if historical {
            implementation_legacy96_fixture()
        } else {
            let fixture = tracking_store_fixture().await;
            tracking_store_assign(&fixture.1, "delegation", "assign");
            fixture
        };
        routing_worker_assign(&store, "implementation");
        let events = [
            (
                "review_requested",
                Some("requested_by"),
                Some("replacement"),
                Some("worker"),
            ),
            ("review", Some("reviewer"), Some("reviewer-child"), None),
            (
                "review_revoked",
                Some("revoked_by"),
                Some("replacement"),
                Some("worker"),
            ),
            (
                "merge",
                Some("merged_by"),
                Some("worker"),
                Some("orchestrator"),
            ),
            ("review_cancelled", None, None, None),
        ];
        store.with_conn(|conn| {
            conn.execute_batch("INSERT INTO merge_requests VALUES('w','actor-mr','main','open','work','main','3','3');
                INSERT INTO merge_request_ticket_relations VALUES('w','actor-mr','tracking','implements','3');")?;
            for (index, (kind, field, worker, _)) in events.iter().enumerate() {
                let mut payload = serde_json::json!({"subject_ref":"source-a"});
                if let (Some(field), Some(worker)) = (field, worker) {
                    payload[*field] = serde_json::json!({"runtime_id":"r","worker_id":worker});
                }
                if *kind == "review_requested" {
                    // A reviewer on a request is not its actor.
                    payload["reviewer"] = serde_json::json!({"runtime_id":"r","worker_id":"worker"});
                }
                conn.execute("INSERT INTO merge_request_thread_events VALUES('w','actor-mr',?1,?2,?1,?3,NULL,'3')", params![kind,index as i64 + 1,payload.to_string()])?;
            }
            if historical { migrate_business_notifications_v96_to_v97(conn)?; }
            notification_routing::route_pending(conn,"w")?;
            for (kind, _, worker, suppressed_role) in events {
                for role in ["worker", "orchestrator"] {
                    let (actor,disposition,reason):(Option<String>,String,String) = conn.query_row(
                        "SELECT s.actor,d.disposition,d.reason FROM notification_sources s JOIN notification_decisions d ON d.source_sequence=s.sequence WHERE s.input_event_id=?1 AND s.role=?2",
                        params![format!("mr-event:actor-mr:{kind}"),role],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
                    assert_eq!(actor,worker.map(|w|format!("worker:r/{w}")),"{kind}, historical={historical}");
                    if historical && role == "orchestrator" {
                        assert_eq!((disposition.as_str(),reason.as_str()),("suppress","unsupported_recipient"),
                            "legacy capture preserves actors but never restores Subject routing authority");
                    } else if suppressed_role == Some(role) {
                        assert_eq!((disposition.as_str(),reason.as_str()),("suppress","self_notification"),"{kind}, historical={historical}");
                    } else {
                        assert_eq!((disposition.as_str(),reason.as_str()),("deliver","attention"),"{kind}, {role}, historical={historical}");
                    }
                }
            }
            Ok(())
        }).unwrap();
    }
}

#[tokio::test]
async fn implementation_result_accepts_only_observations_with_matching_mr_ticket_workspace() {
    let (_dir, store) = tracking_store_fixture().await;
    tracking_store_assign(&store, "delegation", "assign");
    let worker = routing_worker_assign(&store, "implementation").worker;
    seed_manual_start_ticket(&store, "unrelated", "open");
    store.with_conn(|conn| {
        conn.execute_batch("INSERT INTO merge_requests VALUES('w','observed-mr','main','open','work','main','3','3');
            INSERT INTO merge_request_ticket_relations VALUES('w','observed-mr','tracking','implements','3');
            INSERT INTO merge_requests VALUES('w','other-mr','main','open','other','main','3','3');
            INSERT INTO merge_request_ticket_relations VALUES('w','other-mr','unrelated','implements','3');")?;
        Ok(())
    }).unwrap();
    store
        .observe_merge_request_source("w", "main", "work", "source-b")
        .unwrap();
    store
        .observe_merge_request_source("w", "main", "other", "other-source")
        .unwrap();
    for (workspace, mr, subject, accepted) in [
        ("w", "observed-mr", "forged", false),
        ("w", "observed-mr", "other-source", false),
        ("w", "other-mr", "other-source", false),
        ("other", "observed-mr", "source-b", false),
        ("w", "observed-mr", "source-b", true),
    ] {
        let mut input = implementation_result_input();
        input.merge_request = Some(server_api::MergeRequestReviewSubject {
            merge_request_id: mr.into(),
            subject_ref: subject.into(),
        });
        let result = store.record_implementation_result(
            workspace,
            "tracking",
            &input,
            &worker,
            "tracking-binding",
        );
        assert_eq!(
            result.is_ok(),
            accepted,
            "{workspace}/{mr}/{subject}: {result:?}"
        );
    }
    store
        .with_conn(|conn| {
            assert_eq!(
                conn.query_row(
                    "SELECT count(*) FROM merge_request_thread_events",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                0
            );
            assert_eq!(
                conn.query_row(
                    "SELECT count(*) FROM ticket_implementation_results",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                1
            );
            Ok(())
        })
        .unwrap();
}
