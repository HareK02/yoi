// Included in store::tests to reuse the existing hermetic Workspace fixtures.
fn tracking_store_source() -> TicketAssignmentTrackingSourceContext {
    TicketAssignmentTrackingSourceContext {
        actor: "authenticated-worker:r/worker".into(),
        runtime_id: "r".into(),
        worker_id: "worker".into(),
        agent_key: "workspace-orchestrator".into(),
        authentication_binding_id: "tracking-binding".into(),
    }
}

async fn tracking_store_fixture() -> (tempfile::TempDir, SqliteWorkspaceStore) {
    let (directory, store) = manual_worker_start_store().await;
    seed_manual_start_ticket(&store, "tracking", "open");
    store.with_conn(|conn| {
        conn.execute("INSERT INTO workspace_runtime_bindings(workspace_id,runtime_id,display_name,base_url,public_key,public_key_fingerprint,binding_id,authentication_mode,created_at,updated_at) VALUES('w','r','Runtime','https://runtime.invalid','verified-fixture-key','verified-fixture-fingerprint','tracking-binding','workspace_identity','1','1')",[])?;
        conn.execute("INSERT INTO worker_singleton_owners(workspace_id,singleton_key,runtime_id,worker_id,generation,created_at,updated_at) VALUES('w',?1,'r','worker',1,'1','1')",[crate::hosts::WORKSPACE_ORCHESTRATOR_SINGLETON_KEY])?;
        conn.execute("INSERT INTO worker_registry(workspace_id,runtime_id,worker_id,display_name,retention_state,created_at,updated_at) VALUES('w','r','replacement','Replacement','normal','1','1')",[])?;
        Ok(())
    }).unwrap();
    (directory, store)
}

fn tracking_store_assign(
    store: &SqliteWorkspaceStore,
    id: &str,
    operation: &str,
) -> TicketRoleAssignmentRecord {
    let assignment = TicketRoleAssignmentRecord {
        assignment_id: id.into(),
        role: TicketAssignmentRole::Orchestrator,
        principal: TicketAssignmentPrincipal::WorkspaceAgent {
            agent_key: "workspace-orchestrator".into(),
        },
        ..manual_worker_assignment("tracking", id)
    };
    store
        .set_current_ticket_role_assignment(
            &assignment,
            None,
            &format!("event-{operation}"),
            operation,
            false,
        )
        .unwrap()
}
fn tracking_store_input(id: &str, event: &str, key: &str) -> TicketAssignmentTrackingUpdate {
    TicketAssignmentTrackingUpdate {
        assignment_id: id.into(),
        expected_last_event_id: event.into(),
        operation_key: key.into(),
        tracking: TicketAssignmentTracking {
            status: TicketAssignmentTrackingStatus::Done,
            waiting_reason: None,
            progress: None,
            next_action: None,
            result: Some("Returned alternatives".into()),
        },
    }
}

#[tokio::test]
async fn tracking_store_every_replay_fences_current_singleton_and_authenticated_binding_under_write_lock()
 {
    for change in ["replacement", "revoke", "binding-change", "remove"] {
        let (directory, store) = tracking_store_fixture().await;
        tracking_store_assign(&store, "a", "assign-a");
        let source = tracking_store_source();
        let input = tracking_store_input("a", "event-assign-a", "result-a");
        let result = store
            .update_ticket_assignment_tracking("w", "tracking", &input, &source)
            .unwrap();
        assert_eq!(
            store
                .update_ticket_assignment_tracking("w", "tracking", &input, &source)
                .unwrap(),
            result
        );
        // Model the authority change after authenticated source capture, before
        // BEGIN IMMEDIATE on a second connection. Receipt presence cannot bypass it.
        let second =
            SqliteWorkspaceStore::open_for_test(directory.path().join("workspace.db")).unwrap();
        second.with_conn_mut(|conn| {
            let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            match change {
                "replacement" => {tx.execute("UPDATE worker_singleton_owners SET worker_id='replacement',generation=2 WHERE workspace_id='w'",[])?;},
                "revoke" => {tx.execute("UPDATE workspace_runtime_bindings SET revoked_at='revoked' WHERE workspace_id='w'",[])?;},
                "binding-change" => {tx.execute("UPDATE workspace_runtime_bindings SET binding_id='new-binding' WHERE workspace_id='w'",[])?;},
                _ => {tx.execute("DELETE FROM worker_singleton_owners WHERE workspace_id='w'",[])?;},
            }
            tx.commit()?;Ok(())
        }).unwrap();
        assert!(
            matches!(
                store.update_ticket_assignment_tracking("w", "tracking", &input, &source),
                Err(Error::WorkspacePermissionDenied(_))
            ),
            "reject receipt after {change}"
        );
        let next = tracking_store_input(
            "a",
            result.last_event_id.as_deref().unwrap(),
            "fresh-after-change",
        );
        assert!(
            matches!(
                store.update_ticket_assignment_tracking("w", "tracking", &next, &source),
                Err(Error::WorkspacePermissionDenied(_))
            ),
            "reject fresh input after {change}"
        );
        assert_eq!(
            store
                .list_ticket_assignment_tracking_history("w", "tracking", "a")
                .unwrap()
                .len(),
            2
        );
    }
}

#[tokio::test]
async fn tracking_store_same_principal_reissue_and_exact_retries_keep_original_result_and_release_audit()
 {
    let (directory, store) = tracking_store_fixture().await;
    let first = tracking_store_assign(&store, "a", "assign-a");
    let input = tracking_store_input("a", "event-assign-a", "result-a");
    let source = tracking_store_source();
    let result = store
        .update_ticket_assignment_tracking("w", "tracking", &input, &source)
        .unwrap();
    let second = tracking_store_assign(&store, "b", "assign-b");
    assert_eq!(
        store
            .set_current_ticket_role_assignment(
                &second,
                None,
                "ignored-retry-event",
                "assign-b",
                false
            )
            .unwrap(),
        second
    );
    assert_eq!(
        store
            .set_current_ticket_role_assignment(
                &first,
                None,
                "ignored-original-retry",
                "assign-a",
                false
            )
            .unwrap(),
        first
    );
    let old = store
        .get_ticket_assignment_tracking("w", "tracking", "a")
        .unwrap()
        .unwrap();
    assert!(!old.current);
    assert_eq!(old.tracking, result.tracking);
    assert_eq!(old.assigned_by, first.assigned_by);
    assert_eq!(old.assigned_at, first.assigned_at);
    assert_eq!(old.role, "orchestrator");
    let release = old.release.as_ref().unwrap();
    assert_eq!(
        release.action,
        ticket::TicketAssignmentReleaseAction::Replaced
    );
    assert_eq!(release.replacement_assignment_id.as_deref(), Some("b"));
    assert_eq!(release.operation_key.as_deref(), Some("assign-b"));
    assert_eq!(release.last_tracking, old.tracking);
    assert_eq!(
        store
            .update_ticket_assignment_tracking("w", "tracking", &input, &source)
            .unwrap(),
        result
    );
    assert!(
        store
            .update_ticket_assignment_tracking(
                "w",
                "tracking",
                &tracking_store_input("a", result.last_event_id.as_deref().unwrap(), "stale-new"),
                &source
            )
            .is_err()
    );
    let current = store
        .get_ticket_assignment_tracking("w", "tracking", "b")
        .unwrap()
        .unwrap();
    assert_eq!(
        current.tracking.as_ref().unwrap().status,
        TicketAssignmentTrackingStatus::Waiting
    );
    assert!(
        store
            .clear_current_ticket_role_assignment(
                "w",
                "tracking",
                TicketAssignmentRole::Orchestrator,
                "b",
                "unassign-event",
                "unassign-op",
                "human",
                "3",
                Some("Returned to human")
            )
            .unwrap()
    );
    let removed = store
        .get_ticket_assignment_tracking("w", "tracking", "b")
        .unwrap()
        .unwrap();
    assert_eq!(
        removed.release.as_ref().unwrap().action,
        ticket::TicketAssignmentReleaseAction::Unassigned
    );
    assert_eq!(
        removed.release.as_ref().unwrap().reason.as_deref(),
        Some("Returned to human")
    );
    assert_eq!(removed.tracking, current.tracking);
    assert!(
        store
            .update_ticket_assignment_tracking(
                "w",
                "tracking",
                &tracking_store_input("b", "event-assign-b", "removed-new"),
                &source
            )
            .is_err()
    );
    drop(store);
    let reopened =
        SqliteWorkspaceStore::open_for_test(directory.path().join("workspace.db")).unwrap();
    assert_eq!(
        reopened
            .get_ticket_assignment_tracking("w", "tracking", "a")
            .unwrap()
            .unwrap(),
        old
    );
    reopened
        .with_conn(|conn| {
            assert_eq!(
                conn.query_row("SELECT COUNT(*) FROM ticket_worker_assignments", [], |r| {
                    r.get::<_, i64>(0)
                })?,
                2
            );
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM ticket_assignment_operations",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                3
            );
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn tracking_store_close_reopen_and_tracking_done_never_restore_grants_or_work_release() {
    let (_directory, store) = tracking_store_fixture().await;
    tracking_store_assign(&store, "a", "assign-a");
    let source = tracking_store_source();
    store.with_conn(|conn| {conn.execute("UPDATE typed_tickets SET workflow_state='closed' WHERE workspace_id='w' AND ticket_id='tracking'",[])?;Ok(())}).unwrap();
    let input = tracking_store_input("a", "event-assign-a", "closed-result");
    let done = store
        .update_ticket_assignment_tracking("w", "tracking", &input, &source)
        .unwrap();
    store.with_conn(|conn| {assert_eq!(conn.query_row("SELECT workflow_state FROM typed_tickets WHERE ticket_id='tracking'",[],|r|r.get::<_,String>(0))?,"closed");conn.execute("UPDATE typed_tickets SET workflow_state='open' WHERE workspace_id='w' AND ticket_id='tracking'",[])?;assert_eq!(conn.query_row("SELECT COUNT(*) FROM ticket_assignment_work_releases WHERE assignment_id='a'",[],|r|r.get::<_,i64>(0))?,1);assert_eq!(conn.query_row("SELECT COUNT(*) FROM worker_control_grants",[],|r|r.get::<_,i64>(0))?,0);Ok(())}).unwrap();
    assert!(
        store
            .get_active_ticket_role_assignment("w", "tracking", TicketAssignmentRole::Orchestrator)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store
            .get_ticket_assignment_tracking("w", "tracking", "a")
            .unwrap()
            .unwrap(),
        done
    );
    assert_eq!(
        store
            .update_ticket_assignment_tracking("w", "tracking", &input, &source)
            .unwrap(),
        done
    );
}

fn tracking_schema91_fixture(path: &Path) -> Connection {
    let conn = Connection::open(path).unwrap();
    configure_sqlite(&conn).unwrap();
    // Genuine retained component-10 DDL, before assignment tracking existed.
    // The host structure is unchanged from Workspace 91: no tracking columns,
    // tracking tables/triggers, or component-11 marker are created here.
    conn.execute_batch(include_str!("frozen_ticket_schema_v10.sql"))
        .unwrap();
    conn.execute_batch("CREATE TABLE ticket_schema_migrations(version INTEGER PRIMARY KEY,name TEXT NOT NULL,applied_at TEXT NOT NULL); INSERT INTO ticket_schema_migrations VALUES(10,'ticket schema baseline','original-component10-time'); PRAGMA foreign_keys=OFF; PRAGMA legacy_alter_table=ON;").unwrap();
    create_workspace_schema_structure_from_sql(&conn, include_str!("frozen_schema_v93.sql"), false)
        .unwrap();
    conn.execute(
        "INSERT INTO __yoi_schema_migrations(version,name) VALUES(91,'workspace schema baseline')",
        [],
    )
    .unwrap();
    conn.execute_batch("PRAGMA legacy_alter_table=OFF; PRAGMA foreign_keys=ON;")
        .unwrap();
    conn.execute_batch("INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at) VALUES('tracking-owner','user','tracking-owner','Owner','1','1'); INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at) VALUES('w','tracking-owner','Workspace','active','1','1'); INSERT INTO worker_registry(workspace_id,runtime_id,worker_id,display_name,retention_state,created_at,updated_at) VALUES('w','r','worker','Worker','normal','1','1');").unwrap();
    for (ticket, state, snapshot, fence) in [
        ("pending", "open", Some("queued"), Some("a-pending")),
        ("progress", "open", Some("inprogress"), Some("a-progress")),
        (
            "invalid-fence",
            "open",
            Some("queued"),
            Some("not-the-current-identity"),
        ),
        ("no-snapshot", "open", None, Some("a-no-snapshot")),
        (
            "closed-reopen",
            "open",
            Some("queued"),
            Some("a-closed-reopen"),
        ),
        ("replaced", "open", Some("queued"), Some("a-replaced")),
        (
            "historical-done",
            "closed",
            Some("done"),
            Some("a-historical-done"),
        ),
    ] {
        conn.execute("INSERT INTO typed_tickets(workspace_id,ticket_id,slug,title,status,kind,priority,body,workflow_state,workflow_state_explicit) VALUES('w',?1,?1,'Title',?2,'task','P2','Original body',?2,1)",params![ticket,state]).unwrap();
        let assignment = format!("a-{ticket}");
        conn.execute("INSERT INTO ticket_worker_assignments(workspace_id,ticket_id,assignment_id,role,principal_kind,principal_id,assigned_by,assigned_at) VALUES('w',?1,?2,'orchestrator','workspace_agent','workspace-orchestrator','original-assign-actor','2026-10-10T00:00:00Z')",params![ticket,assignment]).unwrap();
        conn.execute("INSERT INTO ticket_current_worker_assignments(workspace_id,ticket_id,assignment_id,role,principal_kind,principal_id,updated_at) VALUES('w',?1,?2,'orchestrator','workspace_agent','workspace-orchestrator','2026-10-10T00:00:00Z')",params![ticket,assignment]).unwrap();
        conn.execute("INSERT INTO ticket_worker_assignment_events(workspace_id,ticket_id,role,event_id,action,assignment_id,actor,created_at,operation_id) VALUES('w',?1,'orchestrator',?2,'assigned',?3,'original-assign-actor','2026-10-10T00:00:00Z',?4)",params![ticket,format!("assigned-{ticket}"),assignment,format!("original-op-{ticket}")]).unwrap();
        conn.execute("INSERT INTO ticket_assignment_operations(workspace_id,operation_id,action,ticket_id,role,principal_kind,principal_id,assignment_id,created_at,request_fingerprint) VALUES('w',?1,'assign',?2,'orchestrator','workspace_agent','workspace-orchestrator',?3,'2026-10-10T00:00:00Z','original-operation-fingerprint')",params![format!("original-op-{ticket}"),ticket,assignment]).unwrap();
        if let Some(state) = snapshot {
            let original = ticket::TicketLegacyWorkflowSnapshot {
                workflow_state: state.into(),
                workflow_state_explicit: true,
                status: state.into(),
                queued_by: Some("original-queue-actor".into()),
                queued_at: Some("2026-10-10T00:01:00Z".into()),
                resolution: None,
            };
            conn.execute(
                "INSERT INTO typed_ticket_legacy_workflow_snapshot VALUES('w',?1,?2)",
                params![ticket, serde_json::to_string(&original).unwrap()],
            )
            .unwrap();
        }
        conn.execute("INSERT INTO typed_ticket_events(workspace_id,ticket_id,event_index,kind,author,at,from_state,to_state,body) VALUES('w',?1,0,'state_changed','original-queue-actor','2026-10-10T00:01:00Z','ready','queued','Original Queue intent')",[ticket]).unwrap();
        if let Some(fence) = fence {
            conn.execute("INSERT INTO typed_ticket_event_attributes VALUES('w',?1,0,'orchestrator_assignment_id',?2)",params![ticket,fence]).unwrap();
        }
    }
    // A pending worker startup reservation is operational data, not tracking.
    conn.execute("INSERT INTO ticket_assignment_operations(workspace_id,operation_id,action,ticket_id,role,principal_kind,runtime_id,worker_id,created_at,request_fingerprint,claim_state,binding_recovery_json) VALUES('w','pending-startup-claim','assign','pending','worker','worker','r','worker','original-claim-time','pending-claim-fingerprint','pending','{}')",[]).unwrap();
    for (ordinal, action) in [
        (
            0,
            ticket::TicketCaseAction::Close {
                reason: ticket::TicketClosureReason::Unknown,
            },
        ),
        (1, ticket::TicketCaseAction::Reopen),
    ] {
        let key = format!("original-case-{ordinal}");
        let history = ticket::TicketCaseHistoryEntry {
            operation_key: key.clone(),
            previous_closure_operation_key: if ordinal == 1 {
                Some("original-case-0".into())
            } else {
                None
            },
            action,
            explanation: "Original later judgment".into(),
            references: vec![],
            author: "original-case-actor".into(),
            at: format!("2026-10-10T00:0{}:00Z", ordinal + 2),
        };
        conn.execute("INSERT INTO typed_ticket_case_history VALUES('w','closed-reopen',?1,?2,'original-case-fingerprint',?3)",params![ordinal,key,serde_json::to_string(&history).unwrap()]).unwrap();
    }
    conn.execute("INSERT INTO ticket_worker_assignments(workspace_id,ticket_id,assignment_id,role,principal_kind,principal_id,assigned_by,assigned_at) VALUES('w','replaced','b-replaced','orchestrator','workspace_agent','workspace-orchestrator','original-reassign-actor','2026-10-10T00:05:00Z')",[]).unwrap();
    conn.execute("DELETE FROM ticket_current_worker_assignments WHERE workspace_id='w' AND ticket_id='replaced'",[]).unwrap();
    conn.execute("INSERT INTO ticket_current_worker_assignments(workspace_id,ticket_id,assignment_id,role,principal_kind,principal_id,updated_at) VALUES('w','replaced','b-replaced','orchestrator','workspace_agent','workspace-orchestrator','2026-10-10T00:05:00Z')",[]).unwrap();
    conn.execute("INSERT INTO ticket_worker_assignment_events(workspace_id,ticket_id,role,event_id,action,assignment_id,previous_assignment_id,actor,created_at,operation_id) VALUES('w','replaced','orchestrator','original-reassign-event','reassigned','b-replaced','a-replaced','original-reassign-actor','2026-10-10T00:05:00Z','original-reassign-op')",[]).unwrap();
    assert!(!table_exists(&conn, "typed_ticket_assignment_tracking_history").unwrap());
    assert_eq!(
        conn.query_row(
            "SELECT MAX(version) FROM ticket_schema_migrations",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        10
    );
    assert_eq!(current_schema_version(&conn).unwrap(), 91);
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| r
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        0
    );
    conn
}

#[test]
fn tracking_store_real_schema91_to92_retains_queue_cases_principals_audit_receipts_and_claims() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("retained91.db");
    let conn = tracking_schema91_fixture(&path);
    let retained = [
        "typed_ticket_events",
        "typed_ticket_event_attributes",
        "typed_ticket_legacy_workflow_snapshot",
        "typed_ticket_case_history",
        "ticket_worker_assignments",
        "ticket_current_worker_assignments",
        "ticket_worker_assignment_events",
        "ticket_assignment_operations",
    ];
    let original = retained.map(|table| stored_fixture_rows(&conn, table));
    drop(conn);
    let plan = SqliteWorkspaceStore::migration_plan(&path).unwrap();
    assert_eq!(plan.current_schema_version, 91);
    assert_eq!(
        plan.migrations
            .iter()
            .map(|m| m.version)
            .collect::<Vec<_>>(),
        (92..=LATEST_SCHEMA_VERSION).collect::<Vec<_>>()
    );
    let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    store
        .with_conn(|conn| {
            assert_eq!(current_schema_version(conn)?, LATEST_SCHEMA_VERSION);
            assert_eq!(
                conn.query_row(
                    "SELECT applied_at FROM ticket_schema_migrations WHERE version=10",
                    [],
                    |r| r.get::<_, String>(0)
                )?,
                "original-component10-time"
            );
            assert_eq!(
                workspace_schema_migration_history(conn)?,
                vec![
                    (91, "workspace schema baseline".into()),
                    (92, "Ticket-owned assignment tracking".into()),
                    (
                        93,
                        "Durable assignment deliveries and source-bound startup fences".into()
                    ),
                    (94, "Canonical Ticket posts".into()),
                    (
                        95,
                        "Common notification routing and durable Worker delivery".into()
                    ),
                    (96,"Backend Job common notification routing".into()),
                    (97, "MR notifications and authenticated implementation results".into()),
                    (98, "Assignment-independent implementation reports".into()),
                    (99, "Explicit hosting administrators and self-registration policy".into()),
                    (100, "Provisional users and scoped passkey activation".into()),
                    (101, "Durable explicit Subject body release".into()),
                    (102, "Durable failed Subject creation recovery".into()),
                    (103, "Passkey metadata and atomic addition results".into()),
                    (104, "Converge Passkey results and Job input retention".into()),
                    (105, "Durable Subject deletion snapshots and Job admission fences".into()),
                    (106, crate::plugin_catalog::MIGRATION_NAME.into()),
                    (107, "Immutable Worker Plugin plans and retained pins".into())
                ]
            );
            for (table, rows) in retained.iter().zip(original.iter()) {
                assert_eq!(
                    &stored_fixture_rows(conn, table),
                    rows,
                    "preserve original {table}"
                );
            }
            assert_eq!(
                conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| r
                    .get::<_, i64>(
                    0
                ))?,
                0
            );
            Ok(())
        })
        .unwrap();
    for (ticket, status) in [
        ("pending", Some(TicketAssignmentTrackingStatus::Waiting)),
        ("progress", Some(TicketAssignmentTrackingStatus::Inprogress)),
        ("invalid-fence", None),
        ("no-snapshot", None),
        ("closed-reopen", None),
        ("replaced", None),
        ("historical-done", None),
    ] {
        let value = store
            .get_ticket_assignment_tracking("w", ticket, &format!("a-{ticket}"))
            .unwrap()
            .unwrap();
        assert_eq!(
            value.tracking.as_ref().map(|t| t.status),
            status,
            "{ticket}"
        );
        assert_eq!(value.assigned_by, "original-assign-actor");
        assert_eq!(value.assigned_at, "2026-10-10T00:00:00Z");
        assert!(value.migration_diagnostic.is_some());
    }
    let replaced = store
        .get_ticket_assignment_tracking("w", "replaced", "a-replaced")
        .unwrap()
        .unwrap();
    assert_eq!(
        replaced.release.as_ref().unwrap().event_id,
        "original-reassign-event"
    );
    assert!(replaced.release.as_ref().unwrap().last_tracking.is_none());
    let pending = store
        .get_ticket_assignment_operation("w", "pending-startup-claim")
        .unwrap()
        .unwrap();
    assert_eq!(pending.claim_state, "pending");
    assert_eq!(
        pending.request_fingerprint.as_deref(),
        Some("pending-claim-fingerprint")
    );
    let history = store
        .list_ticket_assignment_tracking_history("w", "pending", "a-pending")
        .unwrap();
    drop(store);
    let reopened = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    assert_eq!(
        reopened
            .list_ticket_assignment_tracking_history("w", "pending", "a-pending")
            .unwrap(),
        history
    );
}

#[test]
fn tracking_store_schema91_closed_unknown_assignment_uses_assign_audit_for_first_self_report() {
    use ticket::TicketBackend;

    for report_after_reopen in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("closed-unknown91.db");
        let conn = tracking_schema91_fixture(&path);
        conn.execute("INSERT INTO workspace_runtime_bindings(workspace_id,runtime_id,display_name,base_url,public_key,public_key_fingerprint,binding_id,authentication_mode,created_at,updated_at) VALUES('w','r','Runtime','https://runtime.invalid','verified-fixture-key','verified-fixture-fingerprint','tracking-binding','workspace_identity','1','1')",[]).unwrap();
        conn.execute("INSERT INTO worker_singleton_owners(workspace_id,singleton_key,runtime_id,worker_id,generation,created_at,updated_at) VALUES('w',?1,'r','worker',1,'1','1')",[crate::hosts::WORKSPACE_ORCHESTRATOR_SINGLETON_KEY]).unwrap();
        conn.execute("INSERT INTO worker_control_grants(workspace_id,grant_id,controller_runtime_id,controller_worker_id,subject_runtime_id,subject_worker_id,relation,origin,permissions_json,operation_id,created_at,revoked_at) VALUES('w','old-revoked-grant','r','worker','r','worker','ticket','user','[\"observe\"]','old-grant-op','1','original-revocation')",[]).unwrap();
        let audit_before = stored_fixture_rows(&conn, "ticket_worker_assignment_events");
        let grants_before = stored_fixture_rows(&conn, "worker_control_grants");
        let releases_before = stored_fixture_rows(&conn, "ticket_assignment_work_releases");
        drop(conn);

        let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
        let backend = ticket::SqliteTicketBackend::open_verified(&path, "w").unwrap();
        let reopen = || {
            let observed = backend.show("historical-done".into()).unwrap();
            backend
                .update_case(
                    "historical-done",
                    ticket::TicketCaseUpdate {
                        expected_content_digest: ticket::ticket_content_digest(&observed),
                        expected_state: ticket::TicketState::Closed,
                        expected_closure_operation_key: observed.closure_operation_key.clone(),
                        operation_key: "explicit-reopen".into(),
                        action: ticket::TicketCaseAction::Reopen,
                        explanation: "Reopen without redelegation or restoring old work".into(),
                        references: vec![],
                        author: Some("account-author".into()),
                    },
                )
                .unwrap();
        };
        let unknown = store
            .get_ticket_assignment_tracking("w", "historical-done", "a-historical-done")
            .unwrap()
            .unwrap();
        assert!(unknown.current);
        assert!(unknown.tracking.is_none());
        assert!(unknown.updated_at.is_none());
        assert!(unknown.migration_diagnostic.is_some());
        assert_eq!(
            unknown.last_event_id.as_deref(),
            Some("assigned-historical-done")
        );
        assert_eq!(
            store
                .list_current_ticket_assignment_tracking("w", "historical-done")
                .unwrap(),
            vec![unknown.clone()]
        );
        assert!(
            store
                .list_ticket_assignment_tracking_history(
                    "w",
                    "historical-done",
                    "a-historical-done"
                )
                .unwrap()
                .is_empty()
        );
        store.with_conn(|conn| {
            assert_eq!(current_schema_version(conn)?, LATEST_SCHEMA_VERSION);
            assert_eq!(conn.query_row("SELECT MAX(version) FROM ticket_schema_migrations", [], |row| row.get::<_,i64>(0))?, 13);
            assert_eq!(conn.query_row("SELECT workflow_state FROM typed_tickets WHERE ticket_id='historical-done'", [], |row| row.get::<_,String>(0))?, "closed");
            let stored: (Option<String>, Option<String>) = conn.query_row("SELECT tracking_json,last_tracking_event_id FROM ticket_worker_assignments WHERE assignment_id='a-historical-done'", [], |row| Ok((row.get(0)?,row.get(1)?)))?;
            assert_eq!(stored, (None, None), "read baseline must not manufacture tracking storage");
            assert_eq!(stored_fixture_rows(conn, "ticket_worker_assignment_events"), audit_before);
            Ok(())
        }).unwrap();
        if report_after_reopen {
            reopen();
        }
        let source = tracking_store_source();
        let input = tracking_store_input(
            "a-historical-done",
            unknown.last_event_id.as_deref().unwrap(),
            "first-explicit-result",
        );
        let mut wrong_source = source.clone();
        wrong_source.worker_id = "not-current-orchestrator".into();
        assert!(matches!(
            store.update_ticket_assignment_tracking("w", "historical-done", &input, &wrong_source),
            Err(Error::WorkspacePermissionDenied(_))
        ));
        for (workspace, ticket) in [("other-workspace", "historical-done"), ("w", "pending")] {
            assert!(
                store
                    .update_ticket_assignment_tracking(workspace, ticket, &input, &source)
                    .is_err()
            );
        }
        assert!(
            store
                .update_ticket_assignment_tracking(
                    "w",
                    "replaced",
                    &tracking_store_input(
                        "a-replaced",
                        "assigned-replaced",
                        "wrong-old-assignment"
                    ),
                    &source
                )
                .is_err()
        );
        // A second observer holds the same Assign baseline. Only the first
        // explicit writer may consume it; BEGIN IMMEDIATE serializes the CAS.
        let second = SqliteWorkspaceStore::open_for_test(&path).unwrap();
        assert_eq!(
            second
                .get_ticket_assignment_tracking("w", "historical-done", "a-historical-done")
                .unwrap()
                .unwrap(),
            unknown
        );
        let done = store
            .update_ticket_assignment_tracking("w", "historical-done", &input, &source)
            .unwrap();
        assert_eq!(
            done.tracking.as_ref().unwrap().status,
            TicketAssignmentTrackingStatus::Done
        );
        assert_eq!(done.assignment_id, unknown.assignment_id);
        assert_ne!(done.last_event_id, unknown.last_event_id);
        assert_eq!(
            store
                .update_ticket_assignment_tracking("w", "historical-done", &input, &source)
                .unwrap(),
            done
        );
        assert!(matches!(
            second.update_ticket_assignment_tracking(
                "w",
                "historical-done",
                &tracking_store_input(
                    "a-historical-done",
                    "assigned-historical-done",
                    "stale-second-observer"
                ),
                &source
            ),
            Err(Error::TicketAssignmentConflict(_))
        ));
        assert_eq!(
            backend
                .show("historical-done".into())
                .unwrap()
                .meta
                .workflow_state,
            if report_after_reopen {
                ticket::TicketState::Open
            } else {
                ticket::TicketState::Closed
            }
        );
        if !report_after_reopen {
            reopen();
        }
        let after_reopen = store
            .update_ticket_assignment_tracking(
                "w",
                "historical-done",
                &tracking_store_input(
                    "a-historical-done",
                    done.last_event_id.as_deref().unwrap(),
                    "explicit-result-after-reopen",
                ),
                &source,
            )
            .unwrap();
        let history = store
            .list_ticket_assignment_tracking_history("w", "historical-done", "a-historical-done")
            .unwrap();
        assert_eq!(
            history.len(),
            2,
            "only explicit self-reports create handling events"
        );
        assert_eq!(history[0].event_id, done.last_event_id.clone().unwrap());
        assert_eq!(
            history[1].event_id,
            after_reopen.last_event_id.clone().unwrap()
        );
        assert!(
            store
                .get_active_ticket_role_assignment(
                    "w",
                    "historical-done",
                    TicketAssignmentRole::Orchestrator
                )
                .unwrap()
                .is_none()
        );
        store.with_conn(|conn| {
            assert_eq!(conn.query_row("SELECT COUNT(*) FROM ticket_worker_assignments WHERE workspace_id='w' AND ticket_id='historical-done'", [], |row| row.get::<_,i64>(0))?, 1);
            assert_eq!(stored_fixture_rows(conn, "ticket_worker_assignment_events"), audit_before);
            assert_eq!(stored_fixture_rows(conn, "worker_control_grants"), grants_before);
            assert_eq!(stored_fixture_rows(conn, "ticket_assignment_work_releases"), releases_before);
            Ok(())
        }).unwrap();
        drop(second);
        drop(store);
        let reopened_store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
        assert_eq!(
            reopened_store
                .get_ticket_assignment_tracking("w", "historical-done", "a-historical-done")
                .unwrap()
                .unwrap(),
            after_reopen
        );
        assert!(
            reopened_store
                .clear_current_ticket_orchestrator_assignment(
                    "w",
                    "historical-done",
                    "a-historical-done",
                    "explicit-release-event",
                    "explicit-release",
                    "human",
                    "3",
                    Some("Return responsibility"),
                    &orchestrator_mutation_runtime_source()
                )
                .unwrap()
        );
        assert!(matches!(
            reopened_store.update_ticket_assignment_tracking(
                "w",
                "historical-done",
                &tracking_store_input(
                    "a-historical-done",
                    after_reopen.last_event_id.as_deref().unwrap(),
                    "new-report-after-release"
                ),
                &source
            ),
            Err(Error::TicketAssignmentConflict(_))
        ));
        let released = reopened_store
            .get_ticket_assignment_tracking("w", "historical-done", "a-historical-done")
            .unwrap()
            .unwrap();
        assert!(!released.current);
        assert_eq!(released.tracking, after_reopen.tracking);
    }
}

#[test]
fn tracking_store_schema91_missing_assign_audit_remains_unknown_without_cas_baseline() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("missing-audit91.db");
    let conn = tracking_schema91_fixture(&path);
    conn.execute(
        "DELETE FROM ticket_worker_assignment_events WHERE event_id='assigned-historical-done'",
        [],
    )
    .unwrap();
    drop(conn);
    let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    let unknown = store
        .get_ticket_assignment_tracking("w", "historical-done", "a-historical-done")
        .unwrap()
        .unwrap();
    assert!(unknown.current);
    assert!(unknown.tracking.is_none());
    assert!(unknown.last_event_id.is_none());
    assert!(unknown.updated_at.is_none());
    assert!(unknown.migration_diagnostic.is_some());
    assert!(
        store
            .list_ticket_assignment_tracking_history("w", "historical-done", "a-historical-done")
            .unwrap()
            .is_empty()
    );
    // Another assignment's real audit is not a substitute for missing evidence.
    store
        .with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            assert!(
                ticket::sqlite_update_ticket_assignment_tracking_in_transaction(
                    &tx,
                    "w",
                    "historical-done",
                    &tracking_store_input(
                        "a-historical-done",
                        "assigned-pending",
                        "not-a-baseline"
                    ),
                    &tracking_store_source()
                )
                .is_err()
            );
            Ok(())
        })
        .unwrap();
}

fn orchestrator_mutation_runtime_source() -> TicketOrchestratorAssignmentSourceContext {
    TicketOrchestratorAssignmentSourceContext::Runtime {
        runtime_id: "r".into(),
        worker_id: "worker".into(),
        agent_key: "workspace-orchestrator".into(),
        authentication_binding_id: "tracking-binding".into(),
    }
}
fn orchestrator_mutation_record(id: &str) -> TicketRoleAssignmentRecord {
    TicketRoleAssignmentRecord {
        assignment_id: id.into(),
        role: TicketAssignmentRole::Orchestrator,
        principal: TicketAssignmentPrincipal::WorkspaceAgent {
            agent_key: "workspace-orchestrator".into(),
        },
        assigned_by: "readable Backend attribution".into(),
        ..manual_worker_assignment("tracking", id)
    }
}

#[tokio::test]
async fn orchestrator_assignment_receipts_bind_verified_source_even_when_readable_actor_is_unchanged()
 {
    let (_directory, store) = tracking_store_fixture().await;
    let source = orchestrator_mutation_runtime_source();
    let record = orchestrator_mutation_record("source-a");
    let assigned = store
        .set_current_ticket_orchestrator_assignment(
            &record,
            None,
            "source-event",
            "source-assign",
            false,
            &source,
        )
        .unwrap();
    assert_eq!(
        store
            .set_current_ticket_orchestrator_assignment(
                &orchestrator_mutation_record("ignored-result-id"),
                None,
                "ignored-event",
                "source-assign",
                false,
                &source
            )
            .unwrap(),
        assigned
    );
    assert!(
        store
            .clear_current_ticket_orchestrator_assignment(
                "w",
                "tracking",
                "source-a",
                "source-clear-event",
                "source-clear",
                "readable clear actor",
                "3",
                Some("Returned"),
                &source
            )
            .unwrap()
    );
    let second = orchestrator_mutation_record("source-b");
    store
        .set_current_ticket_orchestrator_assignment(
            &second,
            None,
            "second-event",
            "second-assign",
            false,
            &source,
        )
        .unwrap();
    // Exact old operations preserve original results after assignment movement.
    assert_eq!(
        store
            .set_current_ticket_orchestrator_assignment(
                &record,
                None,
                "ignored-event",
                "source-assign",
                false,
                &source
            )
            .unwrap(),
        assigned
    );
    assert!(
        store
            .clear_current_ticket_orchestrator_assignment(
                "w",
                "tracking",
                "source-a",
                "ignored-clear-event",
                "source-clear",
                "readable clear actor",
                "later-time",
                Some("Returned"),
                &source
            )
            .unwrap()
    );
    assert!(
        !store
            .clear_current_ticket_orchestrator_assignment(
                "w",
                "tracking",
                "source-a",
                "new-stale-event",
                "new-stale-clear",
                "readable clear actor",
                "later-time",
                Some("Returned"),
                &source
            )
            .unwrap()
    );
    assert!(
        store
            .get_ticket_assignment_operation("w", "new-stale-clear")
            .unwrap()
            .is_none()
    );
    store.with_conn(|conn| {conn.execute("UPDATE workspace_runtime_bindings SET binding_id='replacement-binding' WHERE workspace_id='w' AND runtime_id='r'",[])?;Ok(())}).unwrap();
    let new_source = TicketOrchestratorAssignmentSourceContext::Runtime {
        authentication_binding_id: "replacement-binding".into(),
        runtime_id: "r".into(),
        worker_id: "worker".into(),
        agent_key: "workspace-orchestrator".into(),
    };
    assert!(matches!(
        store.set_current_ticket_orchestrator_assignment(
            &record,
            None,
            "ignored-event",
            "source-assign",
            false,
            &new_source
        ),
        Err(Error::TicketAssignmentConflict(_))
    ));
    assert!(matches!(
        store.clear_current_ticket_orchestrator_assignment(
            "w",
            "tracking",
            "source-a",
            "ignored-event",
            "source-clear",
            "readable clear actor",
            "later-time",
            Some("Returned"),
            &new_source
        ),
        Err(Error::TicketAssignmentConflict(_))
    ));
    assert!(matches!(
        store.set_current_ticket_orchestrator_assignment(
            &record,
            None,
            "ignored-event",
            "source-assign",
            false,
            &source
        ),
        Err(Error::WorkspacePermissionDenied(_))
    ));
    assert!(matches!(
        store.clear_current_ticket_orchestrator_assignment(
            "w",
            "tracking",
            "source-a",
            "ignored-event",
            "source-clear",
            "readable clear actor",
            "later-time",
            Some("Returned"),
            &source
        ),
        Err(Error::WorkspacePermissionDenied(_))
    ));
    store
        .with_conn(|conn| {
            assert_eq!(
                conn.query_row("SELECT COUNT(*) FROM ticket_worker_assignments", [], |r| {
                    r.get::<_, i64>(0)
                })?,
                2
            );
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM ticket_worker_assignment_events",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                3
            );
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn orchestrator_assignment_all_runtime_retries_fence_singleton_and_binding_in_write_transaction()
 {
    for change in ["replace", "revoke", "remove"] {
        let (directory, store) = tracking_store_fixture().await;
        let source = orchestrator_mutation_runtime_source();
        let record = orchestrator_mutation_record("source-a");
        store
            .set_current_ticket_orchestrator_assignment(
                &record,
                None,
                "assigned-event",
                "assigned-operation",
                false,
                &source,
            )
            .unwrap();
        store
            .clear_current_ticket_orchestrator_assignment(
                "w",
                "tracking",
                "source-a",
                "clear-event",
                "clear-operation",
                "human-readable actor",
                "3",
                None,
                &source,
            )
            .unwrap();
        let second =
            SqliteWorkspaceStore::open_for_test(directory.path().join("workspace.db")).unwrap();
        second.with_conn_mut(|conn| {let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;match change {
            "replace" => {tx.execute("UPDATE worker_singleton_owners SET worker_id='replacement',generation=2 WHERE workspace_id='w'",[])?;},
            "revoke" => {tx.execute("UPDATE workspace_runtime_bindings SET revoked_at='revoked' WHERE workspace_id='w'",[])?;},
            _ => {tx.execute("DELETE FROM worker_singleton_owners WHERE workspace_id='w'",[])?;},
        }tx.commit()?;Ok(())}).unwrap();
        assert!(
            matches!(
                store.set_current_ticket_orchestrator_assignment(
                    &record,
                    None,
                    "retry-event",
                    "assigned-operation",
                    false,
                    &source
                ),
                Err(Error::WorkspacePermissionDenied(_))
            ),
            "assign after {change}"
        );
        assert!(
            matches!(
                store.clear_current_ticket_orchestrator_assignment(
                    "w",
                    "tracking",
                    "source-a",
                    "retry-clear-event",
                    "clear-operation",
                    "human-readable actor",
                    "4",
                    None,
                    &source
                ),
                Err(Error::WorkspacePermissionDenied(_))
            ),
            "clear after {change}"
        );
    }
}

#[tokio::test]
async fn orchestrator_assignment_account_authoring_is_trusted_source_bound_and_never_requires_a_runtime()
 {
    let (_directory, store) = tracking_store_fixture().await;
    store
        .with_conn(|conn| {
            conn.execute("DELETE FROM worker_singleton_owners", [])?;
            conn.execute(
                "UPDATE workspace_runtime_bindings SET revoked_at='revoked'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    let source = TicketOrchestratorAssignmentSourceContext::Account {
        account_id: "owner-account".into(),
        user_id: "authenticated-user".into(),
    };
    let record = orchestrator_mutation_record("account-a");
    let assigned = store
        .set_current_ticket_orchestrator_assignment(
            &record,
            None,
            "account-event",
            "account-assign",
            false,
            &source,
        )
        .unwrap();
    assert_eq!(assigned.assigned_by, "readable Backend attribution");
    assert_eq!(
        store
            .set_current_ticket_orchestrator_assignment(
                &record,
                None,
                "ignored-event",
                "account-assign",
                false,
                &source
            )
            .unwrap(),
        assigned
    );
    let different = TicketOrchestratorAssignmentSourceContext::Account {
        account_id: "owner-account".into(),
        user_id: "different-authenticated-user".into(),
    };
    assert!(matches!(
        store.set_current_ticket_orchestrator_assignment(
            &record,
            None,
            "ignored-event",
            "account-assign",
            false,
            &different
        ),
        Err(Error::TicketAssignmentConflict(_))
    ));
    assert!(
        store
            .clear_current_ticket_orchestrator_assignment(
                "w",
                "tracking",
                "account-a",
                "account-clear-event",
                "account-clear",
                "account display actor",
                "3",
                None,
                &source
            )
            .unwrap()
    );
    assert!(matches!(
        store.clear_current_ticket_orchestrator_assignment(
            "w",
            "tracking",
            "account-a",
            "ignored-event",
            "account-clear",
            "account display actor",
            "4",
            None,
            &different
        ),
        Err(Error::TicketAssignmentConflict(_))
    ));
}

#[tokio::test]
async fn tracking_history_pages_are_append_ordered_exact_scope_cursors_and_bounded_limits() {
    let (_directory, store) = tracking_store_fixture().await;
    tracking_store_assign(&store, "history-a", "history-assign-a");
    let mut expected = vec!["event-history-assign-a".to_owned()];
    store.with_conn_mut(|conn| {let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for index in 0..204 {
            let event=format!("reverse-event-{:03}",204-index);expected.push(event.clone());
            tx.execute("INSERT INTO typed_ticket_assignment_tracking_history VALUES('w','tracking','history-a',?1,?2,'{\"status\":\"waiting\"}','{\"kind\":\"fixture\"}','same-at')",params![event,format!("fixture-operation-{index}")])?;
        }tx.commit()?;Ok(())
    }).unwrap();
    let first = store
        .page_ticket_assignment_tracking_history("w", "tracking", "history-a", None, 100)
        .unwrap();
    assert_eq!(first.items.len(), 100);
    assert_eq!(
        first
            .items
            .iter()
            .map(|e| e.event_id.clone())
            .collect::<Vec<_>>(),
        expected[..100]
    );
    assert_eq!(first.next_event_id.as_deref(), Some(expected[99].as_str()));
    let second = store
        .page_ticket_assignment_tracking_history(
            "w",
            "tracking",
            "history-a",
            first.next_event_id.as_deref(),
            100,
        )
        .unwrap();
    assert_eq!(second.items.len(), 100);
    assert_eq!(
        second
            .items
            .iter()
            .map(|e| e.event_id.clone())
            .collect::<Vec<_>>(),
        expected[100..200]
    );
    let third = store
        .page_ticket_assignment_tracking_history(
            "w",
            "tracking",
            "history-a",
            second.next_event_id.as_deref(),
            100,
        )
        .unwrap();
    assert_eq!(
        third
            .items
            .iter()
            .map(|e| e.event_id.clone())
            .collect::<Vec<_>>(),
        expected[200..]
    );
    assert!(third.next_event_id.is_none());
    let after_last = store
        .page_ticket_assignment_tracking_history(
            "w",
            "tracking",
            "history-a",
            expected.last().map(String::as_str),
            1,
        )
        .unwrap();
    assert!(after_last.items.is_empty());
    assert!(after_last.next_event_id.is_none());
    let one = store
        .page_ticket_assignment_tracking_history("w", "tracking", "history-a", None, 1)
        .unwrap();
    assert_eq!(one.items.len(), 1);
    assert_eq!(one.next_event_id.as_deref(), Some(expected[0].as_str()));
    for limit in [0, 101, usize::MAX] {
        assert!(matches!(
            store.page_ticket_assignment_tracking_history(
                "w",
                "tracking",
                "history-a",
                None,
                limit
            ),
            Err(Error::InvalidInput(_))
        ));
    }
    assert!(matches!(
        store.page_ticket_assignment_tracking_history(
            "w",
            "tracking",
            "history-a",
            Some("missing"),
            10
        ),
        Err(Error::InvalidInput(_))
    ));
    tracking_store_assign(&store, "history-b", "history-assign-b");
    assert!(matches!(
        store.page_ticket_assignment_tracking_history(
            "w",
            "tracking",
            "history-a",
            Some("event-history-assign-b"),
            10
        ),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        store.page_ticket_assignment_tracking_history(
            "other",
            "tracking",
            "history-a",
            Some(&expected[0]),
            10
        ),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        store.page_ticket_assignment_tracking_history(
            "w",
            "wrong-ticket",
            "history-a",
            Some(&expected[0]),
            10
        ),
        Err(Error::InvalidInput(_))
    ));
    seed_manual_start_ticket(&store, "another-ticket", "open");
    let other_ticket = TicketRoleAssignmentRecord {
        role: TicketAssignmentRole::Orchestrator,
        principal: TicketAssignmentPrincipal::WorkspaceAgent {
            agent_key: "workspace-orchestrator".into(),
        },
        ..manual_worker_assignment("another-ticket", "other-ticket-assignment")
    };
    store
        .set_current_ticket_role_assignment(
            &other_ticket,
            None,
            "other-ticket-cursor",
            "other-ticket-operation",
            false,
        )
        .unwrap();
    assert!(matches!(
        store.page_ticket_assignment_tracking_history(
            "w",
            "tracking",
            "history-a",
            Some("other-ticket-cursor"),
            10
        ),
        Err(Error::InvalidInput(_))
    ));
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
    store.with_conn(|conn| {conn.execute("INSERT INTO typed_tickets(workspace_id,ticket_id,slug,title,status,kind,priority,workflow_state,workflow_state_explicit) VALUES('other','tracking','tracking','Other','open','task','P2','open',1)",[])?;conn.execute("INSERT INTO typed_ticket_events(workspace_id,ticket_id,event_index,event_ref,kind,body) VALUES('other','tracking',-1,'tracking:-1','comment','')",[])?;Ok(())}).unwrap();
    let other_workspace = TicketRoleAssignmentRecord {
        workspace_id: "other".into(),
        ..orchestrator_mutation_record("history-a")
    };
    store
        .set_current_ticket_role_assignment(
            &other_workspace,
            None,
            "other-workspace-cursor",
            "other-workspace-operation",
            false,
        )
        .unwrap();
    assert!(matches!(
        store.page_ticket_assignment_tracking_history(
            "w",
            "tracking",
            "history-a",
            Some("other-workspace-cursor"),
            10
        ),
        Err(Error::InvalidInput(_))
    ));
    // Retained history can still be read after assignment replacement.
    assert_eq!(
        store
            .page_ticket_assignment_tracking_history("w", "tracking", "history-a", None, 100)
            .unwrap(),
        first
    );
}

#[tokio::test]
async fn tracking_history_sql_limit_does_not_deserialize_unrequested_lifetime_rows() {
    let (_directory, store) = tracking_store_fixture().await;
    tracking_store_assign(&store, "history-a", "history-assign-a");
    store.with_conn(|conn| {
        conn.execute("INSERT INTO typed_ticket_assignment_tracking_history VALUES('w','tracking','history-a','safe-lookahead','safe-operation','{\"status\":\"waiting\"}','{}','same-at')",[])?;
        // Simulate a later imported/corrupt domain payload: valid SQLite JSON,
        // but not a tracking DTO. It must not poison an earlier bounded page.
        conn.execute("INSERT INTO typed_ticket_assignment_tracking_history VALUES('w','tracking','history-a','later-corrupt-event','later-corrupt-operation','{\"status\":\"not-a-tracking-status\"}','{}','same-at')",[])?;Ok(())
    }).unwrap();
    let page = store
        .page_ticket_assignment_tracking_history("w", "tracking", "history-a", None, 1)
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].event_id, "event-history-assign-a");
    assert_eq!(
        page.next_event_id.as_deref(),
        Some("event-history-assign-a")
    );
    assert!(
        store
            .page_ticket_assignment_tracking_history(
                "w",
                "tracking",
                "history-a",
                Some("safe-lookahead"),
                1
            )
            .is_err()
    );
}

#[tokio::test]
async fn orchestrator_clear_receipt_compares_nullable_reason_as_full_payload_not_only_readable_text()
 {
    let (_directory, store) = tracking_store_fixture().await;
    let source = orchestrator_mutation_runtime_source();
    let record = orchestrator_mutation_record("reason-a");
    store
        .set_current_ticket_orchestrator_assignment(
            &record,
            None,
            "reason-assign-event",
            "reason-assign",
            false,
            &source,
        )
        .unwrap();
    store
        .clear_current_ticket_orchestrator_assignment(
            "w",
            "tracking",
            "reason-a",
            "reason-clear-event",
            "reason-clear",
            "actor",
            "3",
            None,
            &source,
        )
        .unwrap();
    assert!(matches!(
        store.clear_current_ticket_orchestrator_assignment(
            "w",
            "tracking",
            "reason-a",
            "ignored-event",
            "reason-clear",
            "actor",
            "4",
            Some(""),
            &source
        ),
        Err(Error::TicketAssignmentConflict(_))
    ));
    assert!(
        store
            .clear_current_ticket_orchestrator_assignment(
                "w",
                "tracking",
                "reason-a",
                "ignored-event",
                "reason-clear",
                "actor",
                "4",
                None,
                &source
            )
            .unwrap()
    );
}

fn assignment_answer_input(key: &str) -> ticket::TicketAssignmentAnswer {
    ticket::TicketAssignmentAnswer {
        assignment_id: "answer-a".into(),
        expected_last_event_id: "event-answer-assign".into(),
        operation_key: key.into(),
        body: "Proceed with option B".into(),
    }
}
fn assignment_answer_source() -> TicketOrchestratorAssignmentSourceContext {
    TicketOrchestratorAssignmentSourceContext::Runtime {
        runtime_id: "r".into(),
        worker_id: "worker".into(),
        agent_key: "workspace-orchestrator".into(),
        authentication_binding_id: "tracking-binding".into(),
    }
}

#[tokio::test]
async fn assignment_answer_replay_returns_saved_event_without_new_assignment_or_status_change() {
    let (dir, store) = tracking_store_fixture().await;
    tracking_store_assign(&store, "answer-a", "answer-assign");
    let input = assignment_answer_input("answer-key");
    let before = store
        .get_ticket_assignment_tracking("w", "tracking", "answer-a")
        .unwrap()
        .unwrap();
    let saved = store
        .answer_ticket_assignment(
            "w",
            "tracking",
            &input,
            &assignment_answer_source(),
            "authenticated-worker:r/worker",
        )
        .unwrap();
    drop(store);
    let store = SqliteWorkspaceStore::open(dir.path().join("workspace.db")).unwrap();
    assert_eq!(
        store
            .answer_ticket_assignment(
                "w",
                "tracking",
                &input,
                &assignment_answer_source(),
                "authenticated-worker:r/worker"
            )
            .unwrap(),
        saved
    );
    assert_eq!(
        store
            .get_ticket_assignment_tracking("w", "tracking", "answer-a")
            .unwrap()
            .unwrap(),
        before
    );
    let mut changed = input.clone();
    changed.body = "Different answer".into();
    assert!(
        store
            .answer_ticket_assignment(
                "w",
                "tracking",
                &changed,
                &assignment_answer_source(),
                "authenticated-worker:r/worker"
            )
            .is_err()
    );
    let pending = store
        .list_pending_ticket_assignment_deliveries("w", 100)
        .unwrap();
    assert_eq!(
        pending.len(),
        1,
        "self-answer preserves the receipt without waking the same Orchestrator"
    );
    assert!(
        pending
            .iter()
            .all(|p| p.input_event_id != saved.input_event_id)
    );
    store.with_conn(|conn| {
        let reason: String = conn.query_row("SELECT d.reason FROM notification_sources s JOIN notification_decisions d ON d.source_sequence=s.sequence WHERE s.workspace_id='w' AND s.assignment_id='answer-a' AND s.input_event_id=?1 AND s.role='orchestrator'", [&saved.input_event_id], |row| row.get(0))?;
        assert_eq!(reason, "self_notification");
        Ok(())
    }).unwrap();
    store
        .with_conn(|conn| {
            assert_eq!(
                conn.query_row(
                    "SELECT count(*) FROM typed_ticket_events WHERE kind='assignment_answer'",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                1
            );
            assert_eq!(
                conn.query_row(
                    "SELECT value FROM typed_ticket_event_attributes WHERE key='operation_key'",
                    [],
                    |r| r.get::<_, String>(0)
                )?,
                "answer-key"
            );
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn assignment_answer_receipt_failure_rolls_back_body_audit_and_ticket_touch() {
    let (_dir, store) = tracking_store_fixture().await;
    tracking_store_assign(&store, "answer-a", "answer-assign");
    store.with_conn(|conn| {
        conn.execute_batch("CREATE TRIGGER reject_answer_receipt BEFORE INSERT ON typed_ticket_assignment_tracking_operations BEGIN SELECT RAISE(ABORT,'receipt failed'); END;")?;
        Ok(())
    }).unwrap();
    let before: String = store
        .with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT updated_at FROM typed_tickets WHERE ticket_id='tracking'",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert!(
        store
            .answer_ticket_assignment(
                "w",
                "tracking",
                &assignment_answer_input("failed-key"),
                &assignment_answer_source(),
                "authenticated-worker:r/worker"
            )
            .is_err()
    );
    store
        .with_conn(|conn| {
            assert_eq!(
                conn.query_row(
                    "SELECT count(*) FROM typed_ticket_events WHERE kind='assignment_answer'",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                0
            );
            assert_eq!(
                conn.query_row(
                    "SELECT count(*) FROM typed_ticket_event_attributes WHERE key='operation_key'",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                0
            );
            assert_eq!(
                conn.query_row(
                    "SELECT updated_at FROM typed_tickets WHERE ticket_id='tracking'",
                    [],
                    |r| r.get::<_, String>(0)
                )?,
                before
            );
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn assignment_answer_new_input_rejects_replaced_or_changed_handling_but_exact_retry_is_saved()
{
    let (_dir, store) = tracking_store_fixture().await;
    tracking_store_assign(&store, "answer-a", "answer-assign");
    let input = assignment_answer_input("saved-answer");
    let saved = store
        .answer_ticket_assignment(
            "w",
            "tracking",
            &input,
            &assignment_answer_source(),
            "actor",
        )
        .unwrap();
    let mut update = tracking_store_input("answer-a", "event-answer-assign", "next-status");
    update.tracking.status = TicketAssignmentTrackingStatus::Waiting;
    store
        .update_ticket_assignment_tracking("w", "tracking", &update, &tracking_store_source())
        .unwrap();
    assert!(
        store
            .answer_ticket_assignment(
                "w",
                "tracking",
                &assignment_answer_input("stale-answer"),
                &assignment_answer_source(),
                "actor"
            )
            .is_err()
    );
    let mut replacement = manual_worker_assignment("tracking", "answer-b");
    replacement.role = TicketAssignmentRole::Orchestrator;
    replacement.principal = TicketAssignmentPrincipal::WorkspaceAgent {
        agent_key: "workspace-orchestrator".into(),
    };
    store
        .set_current_ticket_role_assignment(
            &replacement,
            Some("answer-a"),
            "replace-answer",
            "replace-answer-key",
            true,
        )
        .unwrap();
    assert!(
        store
            .answer_ticket_assignment(
                "w",
                "tracking",
                &assignment_answer_input("replaced-answer"),
                &assignment_answer_source(),
                "actor"
            )
            .is_err()
    );
    assert_eq!(
        store
            .answer_ticket_assignment(
                "w",
                "tracking",
                &input,
                &assignment_answer_source(),
                "actor"
            )
            .unwrap(),
        saved
    );
    store
        .with_conn(|conn| {
            conn.execute(
                "UPDATE worker_singleton_owners SET worker_id='replacement' WHERE workspace_id='w'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    assert!(
        store
            .answer_ticket_assignment(
                "w",
                "tracking",
                &input,
                &assignment_answer_source(),
                "actor"
            )
            .is_err()
    );
}

#[tokio::test]
async fn assignment_identity_pages_keep_release_and_last_status_separate_and_reject_foreign_cursor()
{
    let (_dir, store) = tracking_store_fixture().await;
    tracking_store_assign(&store, "z-first", "first");
    let mut replacement = manual_worker_assignment("tracking", "a-second");
    replacement.role = TicketAssignmentRole::Orchestrator;
    replacement.principal = TicketAssignmentPrincipal::WorkspaceAgent {
        agent_key: "workspace-orchestrator".into(),
    };
    store
        .set_current_ticket_role_assignment(
            &replacement,
            Some("z-first"),
            "replacement-event",
            "replacement-key",
            true,
        )
        .unwrap();
    let page = store
        .page_ticket_assignments("w", "tracking", None, 1)
        .unwrap();
    assert_eq!(page.next_assignment_id.as_deref(), Some("z-first"));
    assert!(!page.items[0].current);
    assert_eq!(
        page.items[0].tracking.as_ref().unwrap().status,
        TicketAssignmentTrackingStatus::Waiting
    );
    assert_eq!(
        page.items[0].release.as_ref().unwrap().action,
        ticket::TicketAssignmentReleaseAction::Replaced
    );
    let next = store
        .page_ticket_assignments("w", "tracking", page.next_assignment_id.as_deref(), 1)
        .unwrap();
    assert_eq!(next.items[0].assignment_id, "a-second");
    assert!(next.items[0].current);
    assert!(next.next_assignment_id.is_none());
    assert!(
        store
            .page_ticket_assignments("other", "tracking", Some("z-first"), 1)
            .is_err()
    );
    assert!(
        store
            .page_ticket_assignments("w", "tracking", None, 101)
            .is_err()
    );
}

#[tokio::test]
async fn assignment_answer_concurrent_exact_retry_on_two_connections_creates_one_event_and_receipt()
{
    let (dir, store) = tracking_store_fixture().await;
    tracking_store_assign(&store, "answer-a", "answer-assign");
    let path = dir.path().join("workspace.db");
    let first = SqliteWorkspaceStore::open(&path).unwrap();
    let second = SqliteWorkspaceStore::open(&path).unwrap();
    let barrier = std::sync::Barrier::new(2);
    let results = std::thread::scope(|scope| {
        let run = |store: &SqliteWorkspaceStore| {
            barrier.wait();
            store
                .answer_ticket_assignment(
                    "w",
                    "tracking",
                    &assignment_answer_input("concurrent-answer"),
                    &assignment_answer_source(),
                    "actor",
                )
                .unwrap()
        };
        let a = scope.spawn(move || run(&first));
        let b = scope.spawn(move || run(&second));
        (a.join().unwrap(), b.join().unwrap())
    });
    assert_eq!(results.0, results.1);
    store.with_conn(|conn| {
        assert_eq!(conn.query_row("SELECT count(*) FROM typed_ticket_events WHERE kind='assignment_answer'",[],|r| r.get::<_,i64>(0))?,1);
        assert_eq!(conn.query_row("SELECT count(*) FROM typed_ticket_assignment_tracking_operations WHERE operation_key='concurrent-answer'",[],|r| r.get::<_,i64>(0))?,1);
        Ok(())
    }).unwrap();
}
