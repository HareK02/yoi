// Host/component integration: production dry-run, ordered migration, startup and
// reopen on isolated SQLite files. Component tests own exhaustive old-row field
// preservation/rollback; here the host must not retire Plans before step 128.
mod ticket_plan_retirement {
    use super::*;

    const PRESERVED: &[&str] = &[
        "typed_tickets",
        "typed_ticket_relations",
        "ticket_worker_assignments",
        "ticket_current_worker_assignments",
        "ticket_worker_assignment_events",
        "typed_ticket_assignment_tracking_history",
        "typed_ticket_assignment_tracking_operations",
        "ticket_assignment_content_inputs",
        "typed_ticket_content_changes",
        "typed_ticket_service_outbox",
    ];

    fn saved(conn: &Connection) -> Vec<Vec<Vec<rusqlite::types::Value>>> {
        PRESERVED
            .iter()
            .map(|table| stored_fixture_rows(conn, table))
            .collect()
    }

    #[test]
    fn host128_retires_populated_and_empty_plan_storage_without_changing_authority() {
        use ticket::TicketBackend;
        for version in [112, 127] {
            for populated in [false, true] {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("old.db");
                prepare_retained_schema(&path, version);
                let conn = Connection::open(&path).unwrap();
                configure_sqlite(&conn).unwrap();
                seed_migration_workspace(&conn);
                conn.execute_batch("INSERT INTO typed_tickets(workspace_id,ticket_id,slug,title,status,kind,priority,workflow_state,workflow_state_explicit)
                    VALUES('space','memo','memo','Original requirements','open','task','P2','open',1),
                          ('space','related','related','Other Ticket','open','task','P2','open',1);
                    INSERT INTO typed_ticket_events(workspace_id,ticket_id,event_index,event_ref,kind,author,body)
                    VALUES('space','memo',-1,'memo:-1','comment','owner','Original requirements'),
                          ('space','memo',3,'memo:3','comment','owner','Original discussion'),
                          ('space','related',-1,'related:-1','comment','owner','Other requirements');
                    INSERT INTO typed_ticket_relations VALUES('space','memo','related','related','Original relation','owner','original-time');
                    INSERT INTO ticket_worker_assignments(workspace_id,ticket_id,assignment_id,role,principal_kind,principal_id,assigned_by,assigned_at)
                    VALUES('space','memo','original-assignment','orchestrator','subject','subject-a','owner','original-time');
                    INSERT INTO ticket_current_worker_assignments(workspace_id,ticket_id,assignment_id,role,principal_kind,principal_id,updated_at)
                    VALUES('space','memo','original-assignment','orchestrator','subject','subject-a','original-time');").unwrap();
                if populated {
                    conn.execute_batch("INSERT INTO typed_ticket_orchestration_plans VALUES
                        ('space','memo','old-1','accepted_plan','T-related','Unique note','Original summary','old-branch','old-worktree','old-role','original-author','old-time'),
                        ('space','memo','old-2','unknown-future-kind',NULL,NULL,NULL,NULL,NULL,NULL,'another-author','other-time');").unwrap();
                }
                let before = saved(&conn);
                let events_before = stored_fixture_rows(&conn, "typed_ticket_events");
                let plans_before = stored_fixture_rows(&conn, "typed_ticket_orchestration_plans");
                let component_before = stored_fixture_rows(&conn, "ticket_schema_migrations");
                let host_before = workspace_schema_migration_history(&conn).unwrap();
                drop(conn);
                let plan = SqliteWorkspaceStore::migration_plan(&path).unwrap();
                assert_eq!(plan.target_schema_version, 128);
                assert_eq!(plan.migrations.last().unwrap().version, 128);
                let conn = Connection::open(&path).unwrap();
                assert_eq!(saved(&conn), before, "dry-run preserves authority");
                assert_eq!(
                    stored_fixture_rows(&conn, "typed_ticket_events"),
                    events_before
                );
                assert_eq!(
                    stored_fixture_rows(&conn, "typed_ticket_orchestration_plans"),
                    plans_before
                );
                assert_eq!(
                    workspace_schema_migration_history(&conn).unwrap(),
                    host_before
                );
                drop(conn);
                assert_eq!(SqliteWorkspaceStore::migrate_database(&path).unwrap(), plan);
                for _ in 0..2 {
                    let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
                    store
                        .with_conn(|conn| {
                            assert_eq!(current_schema_version(conn)?, 128);
                            assert!(!table_exists(conn, "typed_ticket_orchestration_plans")?);
                            assert_eq!(
                                saved(conn),
                                before,
                                "historical memos do not become authority"
                            );
                            let component = stored_fixture_rows(conn, "ticket_schema_migrations");
                            assert_eq!(
                                &component[..component_before.len()],
                                component_before.as_slice()
                            );
                            assert_eq!(
                                conn.query_row(
                                    "SELECT MAX(version) FROM ticket_schema_migrations",
                                    [],
                                    |r| r.get::<_, i64>(0)
                                )?,
                                17
                            );
                            let events = stored_fixture_rows(conn, "typed_ticket_events");
                            assert_eq!(&events[..events_before.len()], events_before.as_slice());
                            let count = if populated { 2 } else { 0 };
                            assert_eq!(events.len(), events_before.len() + count);
                            assert_eq!(
                                conn.query_row(
                                    "SELECT COUNT(*) FROM pragma_foreign_key_check",
                                    [],
                                    |r| r.get::<_, i64>(0)
                                )?,
                                0
                            );
                            Ok(())
                        })
                        .unwrap();
                    let backend =
                        ticket::SqliteTicketBackend::open_verified(&path, "space").unwrap();
                    let ticket = backend.show("memo".into()).unwrap();
                    assert_eq!(ticket.events.len(), if populated { 4 } else { 2 });
                    if populated {
                        for memo in &ticket.events[2..] {
                            assert_eq!(memo.kind, ticket::TicketEventKind::Comment);
                            assert!(memo.body.0.contains("not a current assignment"));
                            let json = memo
                                .body
                                .0
                                .split("```json\n")
                                .nth(1)
                                .unwrap()
                                .split("\n```")
                                .next()
                                .unwrap();
                            let original: serde_json::Value = serde_json::from_str(json).unwrap();
                            assert_eq!(original.as_object().unwrap().len(), 12);
                            assert!(
                                original["record_id"] == "old-1"
                                    || original["record_id"] == "old-2"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn fresh_and_host128_upgraded_ticket_schemas_converge_and_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old.db");
        let fresh_dir = tempfile::tempdir().unwrap();
        let new = fresh_dir.path().join("new.db");
        prepare_retained_schema(&old, 127);
        let upgraded = SqliteWorkspaceStore::open_for_test(&old).unwrap();
        let fresh = SqliteWorkspaceStore::open_for_test(&new).unwrap();
        for table in [
            "typed_tickets",
            "typed_ticket_events",
            "typed_ticket_relations",
            "typed_ticket_parent_links",
            "typed_ticket_assignment_tracking_history",
        ] {
            let actual = upgraded
                .with_conn(|conn| Ok(state_contract_schema_signature(conn, table)))
                .unwrap();
            let expected = fresh
                .with_conn(|conn| Ok(state_contract_schema_signature(conn, table)))
                .unwrap();
            assert_eq!(actual, expected, "{table}");
        }
        drop((upgraded, fresh));
        for path in [old, new] {
            let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
            store
                .with_conn(|conn| {
                    assert!(!table_exists(conn, "typed_ticket_orchestration_plans")?);
                    ticket::verify_sqlite_ticket_schema(conn).unwrap();
                    storage_formats::verify_history(conn, 128)?;
                    Ok(())
                })
                .unwrap();
        }
    }
}
