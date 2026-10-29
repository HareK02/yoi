// Included in store::tests; requires the production module as a store child.
// Component14 owns principal migration and its ledger; these tests exercise the
// real owner API through host composition, not a copied table/guard installer.
// Run: cargo test -p yoi-workspace-server --lib subject_assignment_migration
mod subject_assignment_migration_tests {
    use super::*;
    use crate::store::subject_assignment_migration::{LEGACY_DIAGNOSTIC, install, verify};

    fn fixture(baseline_only: bool) -> (tempfile::TempDir, std::path::PathBuf, Connection) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("subject-assignment.sqlite");
        // Real retained migrations, not latest DDL with its version relabelled.
        prepare_retained_schema(&path, 106);
        let conn = Connection::open(&path).unwrap();
        configure_sqlite(&conn).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT max(version) FROM ticket_schema_migrations",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            13,
            "retained106 must be a real pre-Subject component13 fixture; historical host migrations must not consume component14 early"
        );
        if baseline_only {
            conn.execute_batch("DELETE FROM __yoi_schema_migrations;
                INSERT INTO __yoi_schema_migrations(version,name) VALUES(106,'workspace schema baseline');").unwrap();
        }
        seed_legacy_evidence(&conn);
        (directory, path, conn)
    }

    fn seed_legacy_evidence(conn: &Connection) {
        seed_workspace(conn);
        for status in ["waiting", "inprogress", "done", "unknown", "released"] {
            seed_ticket(conn, status);
            conn.execute("INSERT INTO ticket_worker_assignments(workspace_id,ticket_id,assignment_id,role,principal_kind,principal_id,assigned_by,assigned_at)
                VALUES('w',?1,?1,'orchestrator','workspace_agent','workspace-orchestrator','original actor','original time')",[status]).unwrap();
            if status == "unknown" {
                conn.execute("UPDATE ticket_worker_assignments SET tracking_json=NULL,tracking_migration_diagnostic='Existing ambiguity' WHERE assignment_id=?1",[status]).unwrap();
            } else if status != "released" {
                conn.execute(
                    "UPDATE ticket_worker_assignments SET tracking_json=?2 WHERE assignment_id=?1",
                    params![
                        status,
                        format!(
                            "{{\"status\":\"{status}\", \"result\":\"original result bytes\"}}"
                        )
                    ],
                )
                .unwrap();
            }
            conn.execute("INSERT INTO ticket_current_worker_assignments(workspace_id,ticket_id,role,assignment_id,principal_kind,principal_id,updated_at)
                VALUES('w',?1,'orchestrator',?1,'workspace_agent','workspace-orchestrator','original time')",[status]).unwrap();
            conn.execute("INSERT INTO ticket_worker_assignment_events(workspace_id,ticket_id,role,event_id,action,assignment_id,actor,created_at,operation_id)
                VALUES('w',?1,'orchestrator',?1,'assigned',?1,'original actor','original time',?1)", [status]).unwrap();
            conn.execute("INSERT INTO ticket_assignment_operations(workspace_id,operation_id,action,ticket_id,role,principal_kind,principal_id,assignment_id,created_at,request_fingerprint,binding_recovery_json)
                VALUES('w',?1,'assign',?1,'orchestrator','workspace_agent','workspace-orchestrator',?1,'original time','unchanged-fingerprint','{ \"original\" : true }')",[status]).unwrap();
        }
        conn.execute_batch("INSERT INTO ticket_worker_assignment_events(workspace_id,ticket_id,role,event_id,action,assignment_id,actor,created_at,operation_id)
            VALUES('w','released','orchestrator','release-event','unassigned','released','original releaser','release time','clear-op');
            DELETE FROM ticket_current_worker_assignments WHERE assignment_id='released';
            INSERT INTO typed_ticket_assignment_tracking_operations
            VALUES('w','saved-answer','waiting','{ \"old_input\" : true }','{ \"old_source\" : true }','{ \"old_result\" : true }');
            INSERT INTO ticket_assignment_operations(workspace_id,operation_id,action,ticket_id,role,principal_kind,runtime_id,worker_id,created_at,request_fingerprint,claim_state,binding_recovery_json)
            VALUES('w','pending-start','assign','waiting','worker','worker','r','worker','claim time','pending fingerprint','pending','{ \"reservation\" : true }');
            INSERT INTO ticket_assignment_operation_fences
            VALUES('w','pending-start','waiting','r','worker','workspace-orchestrator','old-binding','{ \"saved_fence\" : true }');
            INSERT INTO ticket_assignment_deliveries VALUES('w','waiting','pending-input','pending-delivery','{ \"payload\" : true }','{ \"source\" : true }');
            INSERT INTO ticket_assignment_deliveries VALUES('w','waiting','accepted-input','accepted-delivery','{ \"payload\" : false }','{ \"source\" : false }');
            INSERT INTO ticket_assignment_delivery_requests VALUES('w','accepted-delivery','original rendered request');
            INSERT INTO ticket_assignment_delivery_acknowledgements VALUES('w','accepted-delivery','{ \"accepted\" : true }');
            INSERT INTO ticket_assignment_content_inputs(workspace_id,ticket_id,assignment_id,after_ticket_event_index,actor,kind)
            VALUES('w','waiting','waiting',-1,'original editor','content_edit');
            INSERT INTO ticket_worker_assignments(workspace_id,ticket_id,assignment_id,role,principal_kind,runtime_id,worker_id,assigned_by,assigned_at)
            VALUES('w','waiting','implementation','worker','worker','r','worker','original assigner','original time');
            INSERT INTO ticket_current_worker_assignments(workspace_id,ticket_id,role,assignment_id,principal_kind,runtime_id,worker_id,updated_at)
            VALUES('w','waiting','worker','implementation','worker','r','worker','original time');
            INSERT INTO notification_assignment_targets VALUES('w','accepted-delivery','r','worker','workspace-orchestrator','old-binding');").unwrap();
        if table_exists(conn, "ticket_implementation_results").unwrap() {
            conn.execute_batch("            INSERT INTO ticket_implementation_delegations VALUES('w','implementation','waiting');
            INSERT INTO ticket_implementation_results VALUES('w','waiting','result-op','saved-result','r','worker','{ \"body\" : \"original result\", \"kind\" : \"result\" }','result time');
            INSERT INTO notification_decisions(source_sequence,workspace_id,delivery_id,disposition,reason,purpose)
            SELECT sequence,'w','saved-notification','deliver','original reason','attention' FROM notification_sources WHERE input_event_id='implementation-result:saved-result';
            INSERT INTO notification_requests VALUES('w','saved-notification','original common request','{ \"target\" : \"old body\" }');
            INSERT INTO notification_acknowledgements VALUES('w','saved-notification','{ \"accepted\" : \"old body\" }');
            INSERT INTO notification_failures(workspace_id,delivery_id,code,recorded_at) VALUES('w','saved-notification','old-unknown-outcome','original time');").unwrap();
        }
        // A live body candidate and the old singleton exist simultaneously. A
        // migration must not infer stable-agent responsibility from either one.
        conn.execute_batch("INSERT INTO worker_singleton_owners(workspace_id,singleton_key,runtime_id,worker_id,generation,created_at,updated_at)
            VALUES('w','workspace-orchestrator','r','worker',1,'1','2');
            INSERT INTO worker_registry(workspace_id,runtime_id,worker_id,display_name,retention_state,created_at,updated_at)
            VALUES('w','r','body','Body','normal','1','2');
            INSERT INTO worker_singleton_owners(workspace_id,singleton_key,runtime_id,worker_id,generation,created_at,updated_at)
            VALUES('w','subjektiv:tempting-subject','r','body',1,'1','2');").unwrap();
    }

    fn seed_workspace(conn: &Connection) {
        conn.execute_batch("INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at)
            VALUES('owner','user','owner','Owner','1','2');
            INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at)
            VALUES('w','owner','Workspace','active','1','2');
            INSERT INTO worker_registry(workspace_id,runtime_id,worker_id,display_name,retention_state,created_at,updated_at)
            VALUES('w','r','worker','Worker','normal','1','2');").unwrap();
    }

    fn seed_ticket(conn: &Connection, id: &str) {
        conn.execute("INSERT INTO typed_tickets(workspace_id,ticket_id,slug,title,status,kind,priority,workflow_state,workflow_state_explicit)
            VALUES('w',?1,?1,'Title','open','task','P2','open',1)",[id]).unwrap();
        conn.execute("INSERT INTO typed_ticket_events(workspace_id,ticket_id,event_index,event_ref,kind,body)
            VALUES('w',?1,-1,?2,'comment','Original body')",params![id,format!("{id}:-1")]).unwrap();
    }

    fn migrate(conn: &Connection) {
        let tx = conn.unchecked_transaction().unwrap();
        install(&tx).unwrap();
        tx.commit().unwrap();
    }

    // Rowids matter: history pagination uses append ordering. Include all old
    // bytes except the one explicitly enriched diagnostic column.
    fn evidence(conn: &Connection) -> Vec<Vec<Vec<rusqlite::types::Value>>> {
        let mut rows = Vec::new();
        rows.push(fixture_query_rows(
            conn,
            "SELECT rowid,* FROM ticket_schema_migrations WHERE version<=13 ORDER BY version",
        ));
        rows.push(fixture_query_rows(conn,"SELECT rowid,workspace_id,ticket_id,assignment_id,role,principal_kind,principal_id,runtime_id,worker_id,assigned_by,assigned_at,tracking_json,last_tracking_event_id,tracking_updated_at FROM ticket_worker_assignments ORDER BY rowid"));
        for table in [
            "ticket_current_worker_assignments",
            "ticket_worker_assignment_events",
            "ticket_assignment_operations",
            "typed_ticket_assignment_tracking_history",
            "typed_ticket_assignment_tracking_operations",
            "ticket_assignment_delivery_origins",
            "ticket_assignment_deliveries",
            "ticket_assignment_delivery_requests",
            "ticket_assignment_delivery_acknowledgements",
            "ticket_assignment_operation_fences",
            "ticket_assignment_content_inputs",
            "ticket_assignment_work_releases",
            "ticket_implementation_delegations",
            "ticket_implementation_results",
            "notification_sources",
            "notification_decisions",
            "notification_requests",
            "notification_acknowledgements",
            "notification_failures",
            "notification_assignment_targets",
            "worker_registry",
            "worker_singleton_owners",
            "worker_create_reservations",
            "__yoi_schema_migrations",
            // Ticket's owning ledger legitimately appends component14; assert
            // its preserved prefix and new receipt separately below.
        ] {
            rows.push(fixture_query_rows(
                conn,
                &format!("SELECT rowid,* FROM {table} ORDER BY rowid"),
            ));
        }
        rows
    }

    fn component_history(conn: &Connection) -> Vec<Vec<rusqlite::types::Value>> {
        fixture_query_rows(
            conn,
            "SELECT rowid,* FROM ticket_schema_migrations ORDER BY version",
        )
    }

    fn assert_component13(conn: &Connection) {
        assert_eq!(
            conn.query_row(
                "SELECT max(version) FROM ticket_schema_migrations",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            13,
            "failed owning/host migration must not retain a component14 receipt"
        );
    }

    fn assert_component14_append(conn: &Connection, before: &[Vec<rusqlite::types::Value>]) {
        let history = component_history(conn);
        assert_eq!(history.len(), before.len() + 1);
        assert_eq!(
            &history[..before.len()],
            before,
            "owning migration preserves all old component receipt bytes/rowids"
        );
        let latest: (i64,String,String) = conn.query_row("SELECT version,name,applied_at FROM ticket_schema_migrations ORDER BY version DESC LIMIT 1", [],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap();
        assert_eq!(latest.0, 14);
        assert_eq!(latest.1, "Subject Ticket responsibility");
        assert!(!latest.2.is_empty());
    }

    fn schema(conn: &Connection) -> Vec<Vec<rusqlite::types::Value>> {
        fixture_query_rows(
            conn,
            "SELECT type,name,tbl_name,rootpage,sql FROM sqlite_schema ORDER BY type,name",
        )
    }

    fn preserved_schema_objects(conn: &Connection) -> Vec<Vec<rusqlite::types::Value>> {
        fixture_query_rows(conn, "SELECT type,name,tbl_name,rootpage,sql FROM sqlite_schema
            WHERE name NOT IN ('ticket_assignment_operations','ticket_current_worker_assignments','ticket_worker_assignments',
                'typed_ticket_assignment_orchestrator_principal_insert','typed_ticket_assignment_role_principal_insert')
            AND name NOT LIKE 'ticket_subject_%' AND name NOT LIKE 'ticket_legacy_orchestrator_%'
            ORDER BY type,name")
    }

    fn principal_table_contracts(conn: &Connection) -> Vec<Vec<Vec<rusqlite::types::Value>>> {
        let mut contracts = Vec::new();
        for table in [
            "ticket_assignment_operations",
            "ticket_current_worker_assignments",
            "ticket_worker_assignments",
        ] {
            contracts.push(fixture_query_rows(
                conn,
                &format!("SELECT * FROM pragma_table_info('{table}') ORDER BY cid"),
            ));
            contracts.push(fixture_query_rows(
                conn,
                &format!("SELECT * FROM pragma_foreign_key_list('{table}') ORDER BY id,seq"),
            ));
            contracts.push(fixture_query_rows(
                conn,
                &format!("SELECT * FROM pragma_index_list('{table}') ORDER BY name"),
            ));
        }
        contracts
    }

    fn cookie(conn: &Connection) -> i64 {
        conn.pragma_query_value(None, "schema_version", |row| row.get(0))
            .unwrap()
    }

    fn assert_closed_schema_writes(conn: &Connection) {
        assert!(
            !conn
                .pragma_query_value(None, "writable_schema", |row| row.get::<_, bool>(0))
                .unwrap()
        );
        assert!(
            !conn
                .pragma_query_value(None, "ignore_check_constraints", |row| row
                    .get::<_, bool>(0))
                .unwrap()
        );
        assert!(
            conn.pragma_query_value(None, "foreign_keys", |row| row.get::<_, bool>(0))
                .unwrap()
        );
    }

    #[test]
    fn subject_assignment_migration_preserves_legacy_bytes_rowids_receipts_and_pending_inputs() {
        for baseline_only in [false, true] {
            let (_directory, path, conn) = fixture(baseline_only);
            let before = evidence(&conn);
            let component_before = component_history(&conn);
            let preserved_schema = preserved_schema_objects(&conn);
            let contracts = principal_table_contracts(&conn);
            let table_sql_query = "SELECT name,sql FROM sqlite_schema WHERE type='table' AND name IN ('ticket_assignment_operations','ticket_current_worker_assignments','ticket_worker_assignments') ORDER BY name";
            let mut table_sql = fixture_query_rows(&conn, table_sql_query);
            for row in &mut table_sql {
                if let rusqlite::types::Value::Text(sql) = &mut row[1] {
                    *sql = sql.replace(
                        "CHECK(principal_kind IN ('user', 'worker', 'workspace_agent'))",
                        "CHECK(principal_kind IN ('user', 'worker', 'workspace_agent', 'subject'))",
                    );
                } else {
                    panic!("table SQL must be text");
                }
            }
            let rootpages = fixture_query_rows(
                &conn,
                "SELECT name,rootpage FROM sqlite_schema WHERE type='table' ORDER BY name",
            );
            let content_capture = fixture_query_rows(
                &conn,
                "SELECT name,sql FROM sqlite_schema WHERE name IN ('ticket_assignment_title_input','ticket_assignment_initial_post_input') ORDER BY name",
            );
            migrate(&conn);
            assert_component14_append(&conn, &component_before);
            verify(&conn).unwrap();
            assert_eq!(preserved_schema_objects(&conn), preserved_schema);
            assert_eq!(principal_table_contracts(&conn), contracts);
            assert_eq!(fixture_query_rows(&conn, table_sql_query), table_sql);
            assert_eq!(evidence(&conn), before);
            assert_eq!(
                fixture_query_rows(
                    &conn,
                    "SELECT name,rootpage FROM sqlite_schema WHERE type='table' ORDER BY name"
                ),
                rootpages
            );
            assert_eq!(
                fixture_query_rows(
                    &conn,
                    "SELECT name,sql FROM sqlite_schema WHERE name IN ('ticket_assignment_title_input','ticket_assignment_initial_post_input') ORDER BY name"
                ),
                content_capture
            );
            let diagnostics: Vec<(String,String)> = conn.prepare("SELECT assignment_id,tracking_migration_diagnostic FROM ticket_worker_assignments WHERE role='orchestrator' ORDER BY assignment_id").unwrap()
                .query_map([],|row|Ok((row.get(0)?,row.get(1)?))).unwrap().collect::<rusqlite::Result<_>>().unwrap();
            assert_eq!(diagnostics.len(), 5);
            for (id, diagnostic) in diagnostics {
                if id == "unknown" {
                    assert_eq!(
                        diagnostic,
                        format!("Existing ambiguity\n{LEGACY_DIAGNOSTIC}")
                    );
                } else {
                    assert_eq!(diagnostic, LEGACY_DIAGNOSTIC);
                }
            }
            assert_eq!(
                conn.query_row(
                    "SELECT count(*) FROM ticket_worker_assignments WHERE principal_kind='subject'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
                0
            );
            assert_closed_schema_writes(&conn);
            drop(conn);
            let reopened = Connection::open(path).unwrap();
            configure_sqlite(&reopened).unwrap();
            assert_eq!(evidence(&reopened), before);
            assert_component14_append(&reopened, &component_before);
            assert_eq!(preserved_schema_objects(&reopened), preserved_schema);
            assert_eq!(principal_table_contracts(&reopened), contracts);
            verify(&reopened).unwrap();
            assert_closed_schema_writes(&reopened);
        }
    }

    fn assert_subject_contract(conn: &Connection) {
        seed_ticket(conn, "new-ticket");
        conn.execute_batch("INSERT INTO ticket_assignment_operations(workspace_id,operation_id,action,ticket_id,role,principal_kind,principal_id,assignment_id,created_at,request_fingerprint)
            VALUES('w','new-op','assign','new-ticket','orchestrator','subject','subject-a','new-assignment','3','subject fingerprint');
            INSERT INTO ticket_worker_assignments(workspace_id,ticket_id,assignment_id,role,principal_kind,principal_id,assigned_by,assigned_at)
            VALUES('w','new-ticket','new-assignment','orchestrator','subject','subject-a','authorized actor','3');
            INSERT INTO ticket_current_worker_assignments(workspace_id,ticket_id,role,assignment_id,principal_kind,principal_id,updated_at)
            VALUES('w','new-ticket','orchestrator','new-assignment','subject','subject-a','3');
            INSERT INTO ticket_worker_assignment_events(workspace_id,ticket_id,role,event_id,action,assignment_id,actor,created_at)
            VALUES('w','new-ticket','orchestrator','new-event','assigned','new-assignment','authorized actor','3');").unwrap();
        assert_eq!(conn.query_row("SELECT principal_id FROM ticket_current_worker_assignments WHERE assignment_id='new-assignment'",[],|row|row.get::<_,String>(0)).unwrap(),"subject-a");
        assert_eq!(conn.query_row("SELECT json_extract(tracking_json,'$.status') FROM ticket_worker_assignments WHERE assignment_id='new-assignment'",[],|row|row.get::<_,String>(0)).unwrap(),"waiting");
        for (role, kind, id, runtime, worker) in [
            (
                "orchestrator",
                "workspace_agent",
                Some("workspace-orchestrator"),
                None,
                None,
            ),
            ("orchestrator", "subject", Some("  "), None, None),
            ("orchestrator", "subject", None, None, None),
            (
                "orchestrator",
                "subject",
                Some("subject-a"),
                Some("r"),
                Some("worker"),
            ),
            ("orchestrator", "unsupported", Some("subject-a"), None, None),
            ("worker", "subject", Some("subject-a"), None, None),
            ("owner", "subject", Some("subject-a"), None, None),
            ("contributor", "subject", Some("subject-a"), None, None),
        ] {
            assert!(conn.execute("INSERT INTO ticket_worker_assignments(workspace_id,ticket_id,assignment_id,role,principal_kind,principal_id,runtime_id,worker_id,assigned_by,assigned_at)
                VALUES('w','new-ticket','bad',?1,?2,?3,?4,?5,'actor','4')",params![role,kind,id,runtime,worker]).is_err(),"{role}/{kind}/{id:?}");
        }
        assert!(conn.execute("UPDATE ticket_worker_assignments SET principal_id='other-subject' WHERE assignment_id='new-assignment'",[]).is_err());
        assert!(conn.execute("UPDATE ticket_current_worker_assignments SET principal_id='other-subject' WHERE assignment_id='new-assignment'",[]).is_err());
        assert!(conn.execute("UPDATE ticket_assignment_operations SET request_fingerprint='changed' WHERE operation_id='new-op'",[]).is_err());
        assert_eq!(
            conn.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            0
        );
    }

    #[test]
    fn subject_assignment_migration_baseline_and_retained_match_fresh_constraints() {
        let fresh = SqliteWorkspaceStore::in_memory_for_test().unwrap();
        for baseline_only in [false, true] {
            let (_directory, _path, conn) = fixture(baseline_only);
            migrate(&conn);
            fresh.with_conn(|new| {
                assert!(new.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='ticket_subject_current_insert')",[],|row|row.get::<_,bool>(0))?, "fresh composition must install the Subject assignment contract");
                for table in ["ticket_assignment_operations","ticket_worker_assignments","ticket_current_worker_assignments"] {
                    // Fresh106 installed Ticket core before workspaces existed;
                    // retained83 has two additional direct Workspace parent FKs.
                    // Verify both complete known targets, preserve their original
                    // FKs, and compare all other contract signatures/behavior.
                    verify(&conn)?;
                    verify(new)?;
                    let workspace_parents = |db: &Connection| -> Result<i64> {
                        db.query_row(&format!("SELECT count(*) FROM pragma_foreign_key_list('{table}') WHERE \"table\"='workspaces'"), [], |row| row.get(0)).map_err(Error::from)
                    };
                    assert_eq!(workspace_parents(new)?, 0, "fresh standalone-first preimage: {table}");
                    assert_eq!(workspace_parents(&conn)?, if table=="ticket_current_worker_assignments" {0} else {1}, "retained Workspace preimage: {table}");
                    let contract = |db: &Connection| state_contract_schema_signature(db,table);
                    assert_eq!(contract(&conn),contract(new),"{table}");
                    for db in [&conn,new] {
                        let ddl:String=db.query_row("SELECT sql FROM sqlite_schema WHERE type='table' AND name=?1",[table],|row|row.get(0))?;
                        assert!(ddl.contains("CHECK(principal_kind IN ('user', 'worker', 'workspace_agent', 'subject'))"));
                    }
                }
                Ok(())
            }).unwrap();
            assert_subject_contract(&conn);
        }
        fresh
            .with_conn(|conn| {
                seed_workspace(conn);
                assert_subject_contract(conn);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn subject_assignment_migration_legacy_is_readable_but_never_new_authority() {
        let (_directory, _path, conn) = fixture(false);
        migrate(&conn);
        for sql in [
            "UPDATE ticket_worker_assignments SET tracking_json='{\"status\":\"inprogress\"}' WHERE assignment_id='waiting'",
            "UPDATE ticket_worker_assignments SET last_tracking_event_id='new' WHERE assignment_id='waiting'",
            "INSERT INTO ticket_current_worker_assignments(workspace_id,ticket_id,role,assignment_id,principal_kind,principal_id,updated_at) VALUES('w','released','orchestrator','released','workspace_agent','workspace-orchestrator','new')",
            "INSERT INTO typed_ticket_assignment_tracking_history VALUES('w','waiting','waiting','forged','forged','{\"status\":\"waiting\"}','{}','new')",
            "INSERT INTO ticket_assignment_deliveries VALUES('w','waiting','fresh-input','fresh-delivery','{}','{}')",
            "INSERT INTO ticket_assignment_operation_fences VALUES('w','waiting','waiting','r','worker','workspace-orchestrator','old-binding','{}')",
            "UPDATE ticket_assignment_operations SET request_fingerprint='changed' WHERE operation_id='waiting'",
            "UPDATE typed_ticket_assignment_tracking_operations SET result_json='{}' WHERE operation_key='saved-answer'",
        ] {
            assert!(conn.execute(sql, []).is_err(), "{sql}");
        }
        // Explicit clear may preserve a legacy principal in its new receipt; it
        // ends responsibility and does not resurrect an assignment or Worker.
        conn.execute("INSERT INTO ticket_assignment_operations(workspace_id,operation_id,action,ticket_id,role,principal_kind,principal_id,assignment_id,created_at)
            VALUES('w','explicit-clear','unassign','waiting','orchestrator','workspace_agent','workspace-orchestrator','waiting','new')",[]).unwrap();
        assert_subject_contract(&conn);
    }

    #[test]
    fn subject_assignment_migration_failure_rolls_back_schema_cookie_rows_and_allows_retry() {
        let (_directory, path, conn) = fixture(false);
        conn.execute_batch("CREATE TRIGGER reject_subject_diagnostic BEFORE UPDATE OF tracking_migration_diagnostic ON ticket_worker_assignments BEGIN SELECT RAISE(ABORT,'injected Subject migration failure'); END;").unwrap();
        let before_rows = evidence(&conn);
        let component_before = component_history(&conn);
        let before_schema = schema(&conn);
        let before_cookie = cookie(&conn);
        {
            let tx = conn.unchecked_transaction().unwrap();
            let error = install(&tx).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("injected Subject migration failure"),
                "{error}"
            );
            assert_closed_schema_writes(&tx);
            assert_component13(&tx); // Ticket's savepoint already rolled back.
            // Caller rolls back its whole transaction, including ledger writes.
        }
        assert_eq!(schema(&conn), before_schema);
        assert_eq!(cookie(&conn), before_cookie);
        assert_component13(&conn);
        assert_eq!(evidence(&conn), before_rows);
        assert_closed_schema_writes(&conn);
        drop(conn);
        let conn = Connection::open(path).unwrap();
        configure_sqlite(&conn).unwrap();
        assert_eq!(schema(&conn), before_schema);
        assert_eq!(evidence(&conn), before_rows);
        conn.execute_batch("DROP TRIGGER reject_subject_diagnostic")
            .unwrap();
        migrate(&conn);
        assert_eq!(evidence(&conn), before_rows);
        assert_component14_append(&conn, &component_before);
        assert_subject_contract(&conn);
    }

    #[test]
    fn subject_assignment_migration_rejects_autocommit_and_unknown_schema_without_changes() {
        let (_directory, _path, conn) = fixture(false);
        let before = schema(&conn);
        assert!(
            install(&conn)
                .unwrap_err()
                .to_string()
                .contains("caller-owned transaction")
        );
        assert_eq!(schema(&conn), before);
        conn.execute_batch("ALTER TABLE ticket_assignment_operations RENAME COLUMN principal_kind TO unexpected_kind;").unwrap();
        let before = schema(&conn);
        let before_cookie = cookie(&conn);
        {
            let tx = conn.unchecked_transaction().unwrap();
            let error = install(&tx).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("Ticket Subject component composition failed"),
                "{error}"
            );
        }
        assert_eq!(schema(&conn), before);
        assert_eq!(cookie(&conn), before_cookie);
        assert_component13(&conn);
        assert_closed_schema_writes(&conn);
    }

    #[test]
    fn subject_assignment_migration_rejects_whole_table_drift_and_rolls_back_unknown_schema() {
        for table in [
            "ticket_assignment_operations",
            "ticket_current_worker_assignments",
            "ticket_worker_assignments",
        ] {
            let (_directory, path, conn) = fixture(false);
            // The formerly matched principal CHECK remains exactly present.
            // Unknown layout/default/constraint additions must not be accepted.
            conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN unexpected_metadata TEXT DEFAULT 'not canonical' CHECK(unexpected_metadata IS NOT NULL);")).unwrap();
            let before = schema(&conn);
            let rows = evidence(&conn);
            let before_cookie = cookie(&conn);
            let ddl: String = conn
                .query_row(
                    "SELECT sql FROM sqlite_schema WHERE name=?1",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(ddl.contains("CHECK(principal_kind IN ('user', 'worker', 'workspace_agent'))"));
            {
                let tx = conn.unchecked_transaction().unwrap();
                let error = install(&tx).unwrap_err();
                assert!(
                    error
                        .to_string()
                        .contains("Ticket Subject component composition failed")
                        && error.to_string().contains(table),
                    "{error}"
                );
                assert_closed_schema_writes(&tx);
            }
            assert_eq!(schema(&conn), before);
            assert_eq!(cookie(&conn), before_cookie);
            assert_component13(&conn);
            assert_eq!(evidence(&conn), rows);
            drop(conn);
            let reopened = Connection::open(path).unwrap();
            configure_sqlite(&reopened).unwrap();
            assert_eq!(schema(&reopened), before);
            assert_eq!(evidence(&reopened), rows);
            assert_closed_schema_writes(&reopened);
        }
    }

    #[test]
    fn subject_assignment_migration_rejects_weakened_principal_preimage_without_repair() {
        let (_directory, _path, conn) = fixture(false);
        conn.execute_batch("DROP TRIGGER typed_ticket_assignment_orchestrator_principal_insert;
            CREATE TRIGGER typed_ticket_assignment_orchestrator_principal_insert BEFORE INSERT ON ticket_worker_assignments BEGIN SELECT 1; END;").unwrap();
        let before = schema(&conn);
        let rows = evidence(&conn);
        {
            let tx = conn.unchecked_transaction().unwrap();
            let error = install(&tx).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("typed_ticket_assignment_orchestrator_principal_insert")
                    && error.to_string().contains("canonical"),
                "{error}"
            );
        }
        assert_eq!(schema(&conn), before);
        assert_eq!(evidence(&conn), rows);
        assert_component13(&conn);
        assert_closed_schema_writes(&conn);
    }

    #[test]
    fn subject_assignment_migration_readonly_owner_failure_preserves_settings_and_retries() {
        let (_directory, _path, conn) = fixture(false);
        let before = schema(&conn);
        let rows = evidence(&conn);
        let before_cookie = cookie(&conn);
        {
            let tx = conn.unchecked_transaction().unwrap();
            // Failure at the delegated owning migration boundary must not leak
            // connection settings or install host guards. Ticket owns injection
            // details inside its principal-table migration, not this adapter.
            tx.pragma_update(None, "query_only", true).unwrap();
            let error = install(&tx).unwrap_err();
            assert!(error.to_string().contains("readonly"), "{error}");
            assert_closed_schema_writes(&tx);
            tx.pragma_update(None, "query_only", false).unwrap();
        }
        assert_eq!(schema(&conn), before);
        assert_eq!(cookie(&conn), before_cookie);
        assert_component13(&conn);
        assert_eq!(evidence(&conn), rows);
        assert_closed_schema_writes(&conn);
        migrate(&conn);
        verify(&conn).unwrap();
        assert_subject_contract(&conn);
    }

    #[test]
    fn subject_assignment_migration_verifier_checks_every_complete_trigger_and_table() {
        let (_directory, _path, conn) = fixture(false);
        migrate(&conn);
        verify(&conn).unwrap();
        let guards: Vec<(String, String)> = conn.prepare("SELECT name,tbl_name FROM sqlite_schema
            WHERE type='trigger' AND (name LIKE 'ticket_subject_%' OR name LIKE 'ticket_legacy_orchestrator_%'
                OR name IN ('typed_ticket_assignment_orchestrator_principal_insert','typed_ticket_assignment_role_principal_insert'))
            ORDER BY name").unwrap().query_map([], |row| Ok((row.get(0)?,row.get(1)?))).unwrap().collect::<rusqlite::Result<_>>().unwrap();
        assert_eq!(guards.len(), 10);
        for (name, table) in guards {
            conn.execute_batch("SAVEPOINT weakened_guard").unwrap();
            conn.execute_batch(&format!("DROP TRIGGER {name}; CREATE TRIGGER {name} BEFORE INSERT ON {table} BEGIN SELECT 1; END;")).unwrap();
            let drift = schema(&conn);
            let error = verify(&conn).unwrap_err();
            assert!(
                error.to_string().contains(&name),
                "owning or host verifier must reject {name}: {error}"
            );
            if name == "ticket_legacy_orchestrator_delivery_no_insert"
                || name == "ticket_legacy_orchestrator_start_no_insert"
            {
                assert!(
                    error.to_string().contains("Workspace Subject guard"),
                    "{error}"
                );
            } else {
                assert!(
                    error
                        .to_string()
                        .contains("Ticket Subject schema verification failed"),
                    "{error}"
                );
            }
            assert_eq!(schema(&conn), drift, "verification must not repair a guard");
            conn.execute_batch("ROLLBACK TO weakened_guard; RELEASE weakened_guard")
                .unwrap();
            verify(&conn).unwrap();
        }
        for table in [
            "ticket_assignment_operations",
            "ticket_current_worker_assignments",
            "ticket_worker_assignments",
        ] {
            conn.execute_batch("SAVEPOINT weakened_table").unwrap();
            conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN unexpected TEXT;"))
                .unwrap();
            let error = verify(&conn).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("Ticket Subject schema verification failed")
                    && error.to_string().contains(table),
                "{error}"
            );
            conn.execute_batch("ROLLBACK TO weakened_table; RELEASE weakened_table")
                .unwrap();
        }
        for setting in ["writable_schema", "ignore_check_constraints"] {
            conn.pragma_update(None, setting, true).unwrap();
            let error = verify(&conn).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains(&format!("requires {setting} OFF")),
                "{error}"
            );
            conn.pragma_update(None, setting, false).unwrap();
        }
        verify(&conn).unwrap();
    }

    #[test]
    fn subject_assignment_migration_startup_rejects_guard_drift_without_repair() {
        for guard in [
            "ticket_legacy_orchestrator_delivery_no_insert",
            "ticket_legacy_orchestrator_start_no_insert",
        ] {
            let (_directory, path, conn) = fixture(false);
            apply_migrations(&conn).unwrap();
            conn.execute_batch(&format!("DROP TRIGGER {guard}"))
                .unwrap();
            let before_schema = schema(&conn);
            let before_rows = evidence(&conn);
            let before_cookie = cookie(&conn);
            let before_components = component_history(&conn);
            drop(conn);

            let conn = Connection::open(path).unwrap();
            configure_sqlite(&conn).unwrap();
            let error = apply_migrations(&conn).unwrap_err();
            assert!(error.to_string().contains(guard), "{error}");
            assert_eq!(schema(&conn), before_schema);
            assert_eq!(evidence(&conn), before_rows);
            assert_eq!(cookie(&conn), before_cookie);
            assert_eq!(component_history(&conn), before_components);
            assert_closed_schema_writes(&conn);
        }
    }

    #[test]
    fn subject_assignment_migration_host_failure_rolls_back_owning_component_and_retries() {
        let (_directory, path, conn) = fixture(false);
        // Force failure on the SECOND host guard, after Ticket14 and the first
        // host guard have succeeded. The outer host transaction owns rollback.
        conn.execute_batch(
            "CREATE TRIGGER ticket_legacy_orchestrator_start_no_insert
            BEFORE INSERT ON ticket_assignment_operation_fences BEGIN SELECT 1; END;",
        )
        .unwrap();
        let before_schema = schema(&conn);
        let before_rows = evidence(&conn);
        let component_before = component_history(&conn);
        let before_cookie = cookie(&conn);
        {
            let tx = conn.unchecked_transaction().unwrap();
            let error = install(&tx).unwrap_err();
            assert!(error.to_string().contains("already exists"), "{error}");
            assert_component14_append(&tx, &component_before);
            assert!(tx.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='ticket_legacy_orchestrator_delivery_no_insert')", [], |row| row.get::<_,bool>(0)).unwrap());
            assert!(!tx.is_autocommit());
            assert_closed_schema_writes(&tx);
        }
        assert_eq!(schema(&conn), before_schema);
        assert_eq!(evidence(&conn), before_rows);
        assert_eq!(component_history(&conn), component_before);
        assert_eq!(cookie(&conn), before_cookie);
        assert_component13(&conn);
        drop(conn);
        let conn = Connection::open(path).unwrap();
        configure_sqlite(&conn).unwrap();
        assert_eq!(schema(&conn), before_schema);
        assert_eq!(component_history(&conn), component_before);
        conn.execute_batch("DROP TRIGGER ticket_legacy_orchestrator_start_no_insert")
            .unwrap();
        migrate(&conn);
        assert_component14_append(&conn, &component_before);
        verify(&conn).unwrap();
        assert_subject_contract(&conn);
    }

    #[test]
    fn subject_assignment_migration_precomposed_owner_is_not_reinstalled_or_rediagnosed() {
        let (_directory, _path, conn) = fixture(false);
        let old_components = component_history(&conn);
        {
            let tx = conn.unchecked_transaction().unwrap();
            ticket::migrate_sqlite_ticket_schema_in_transaction(&tx).unwrap();
            tx.commit().unwrap();
        }
        assert_component14_append(&conn, &old_components);
        ticket::verify_sqlite_ticket_schema(&conn).unwrap();
        // The host must not append the owning diagnostic again or issue even a
        // redundant diagnostic UPDATE for any already-diagnosed legacy row.
        conn.execute_batch(
            "CREATE TRIGGER forbid_duplicate_subject_diagnostic
            BEFORE UPDATE OF tracking_migration_diagnostic ON ticket_worker_assignments
            BEGIN SELECT RAISE(ABORT,'host duplicated owning diagnostic'); END;",
        )
        .unwrap();
        let owner_schema = schema(&conn);
        let owner_assignments = fixture_query_rows(
            &conn,
            "SELECT rowid,* FROM ticket_worker_assignments ORDER BY rowid",
        );
        let owner_receipts = component_history(&conn);
        let before_rows = evidence(&conn);
        let before_cookie = cookie(&conn);
        migrate(&conn);
        assert_eq!(component_history(&conn), owner_receipts);
        assert_eq!(
            fixture_query_rows(
                &conn,
                "SELECT rowid,* FROM ticket_worker_assignments ORDER BY rowid"
            ),
            owner_assignments
        );
        assert_eq!(evidence(&conn), before_rows);
        let after_without_host = fixture_query_rows(&conn,"SELECT type,name,tbl_name,rootpage,sql FROM sqlite_schema
            WHERE name NOT IN ('ticket_legacy_orchestrator_delivery_no_insert','ticket_legacy_orchestrator_start_no_insert') ORDER BY type,name");
        assert_eq!(
            after_without_host, owner_schema,
            "host must not rebuild tables or replace any owning guard"
        );
        assert_eq!(
            cookie(&conn),
            before_cookie + 2,
            "only the two host CREATE TRIGGER statements change the schema"
        );
        verify(&conn).unwrap();
    }

    #[test]
    fn subject_assignment_migration_reloads_prepared_constraints_and_reopen_cache() {
        let (_directory, path, conn) = fixture(false);
        let mut cached = conn.prepare_cached("INSERT INTO ticket_assignment_operations(workspace_id,operation_id,action,ticket_id,role,principal_kind,principal_id,assignment_id,created_at,request_fingerprint)
            VALUES('w','cached-subject','assign','waiting','orchestrator','subject','subject-a','cached-assignment','3','cached fingerprint')").unwrap();
        assert!(cached.execute([]).is_err(), "old CHECK rejects Subject");
        {
            let tx = conn.unchecked_transaction().unwrap();
            install(&tx).unwrap();
            // Drop the whole migration transaction; a prepared statement must
            // not retain broadened constraints after rollback.
        }
        assert_component13(&conn);
        assert!(
            cached.execute([]).is_err(),
            "rollback restores old CHECK and guards"
        );
        migrate(&conn);
        cached.execute([]).unwrap();
        drop(cached);
        verify(&conn).unwrap();
        drop(conn);
        let conn = Connection::open(path).unwrap();
        configure_sqlite(&conn).unwrap();
        verify(&conn).unwrap();
        assert_eq!(conn.query_row("SELECT request_fingerprint FROM ticket_assignment_operations WHERE operation_id='cached-subject'", [], |row| row.get::<_, String>(0)).unwrap(), "cached fingerprint");
        assert_subject_contract(&conn);
    }

    mod subject_assignment_final_tail {
        use super::*;
        use crate::plugin_plan::{
            self, PlanPinState, PluginPlanCreateReceipt, WorkerPluginExecutionPlan,
            WorkerPluginPlanInput,
        };

        const PLAN_NAME: &str = "Immutable Worker Plugin plans and retained pins";
        const SUBJECT_NAME: &str =
            "Subject Ticket responsibility and current-body notification routing";
        type Rows = Vec<Vec<rusqlite::types::Value>>;

        fn raw(path: &Path) -> Connection {
            let conn = Connection::open(path).unwrap();
            configure_sqlite(&conn).unwrap();
            conn
        }

        fn retained(version: i64) -> (tempfile::TempDir, std::path::PathBuf, Connection) {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("tail.db");
            prepare_retained_schema(&path, version);
            let conn = raw(&path);
            seed_legacy_evidence(&conn);
            assert_component13(&conn);
            assert!(!table_exists(&conn, "plugin_execution_plans").unwrap());
            assert_eq!(
                table_exists(&conn, "plugin_installations").unwrap(),
                version >= 106
            );
            (dir, path, conn)
        }

        // Explicit data dependencies only. Real 107 DDL is installed by the frozen
        // migration, not copied from a latest store or relabelled after downgrade.
        async fn populated107() -> (
            tempfile::TempDir,
            std::path::PathBuf,
            Connection,
            Vec<(String, PluginPlanCreateReceipt, PlanPinState)>,
        ) {
            let (dir, path, conn) = retained(106);
            migrate_plugin_plans_v106_to_v107(&conn).unwrap();
            let (_donor_dir, donor, receipts) =
                SqliteWorkspaceStore::plugin_plan_migration_donor().await;
            for table in [
                "accounts",
                "workspaces",
                "workspace_config_trees",
                "workspace_config_entries",
                "workspace_config_tree_history",
                "plugin_installations",
                "plugin_artifacts",
                "plugin_uploads",
                "plugin_operations",
                "plugin_execution_plans",
                "plugin_references",
            ] {
                let columns = conn
                    .prepare(&format!("PRAGMA table_info({table})"))
                    .unwrap()
                    .query_map([], |row| row.get::<_, String>(1))
                    .unwrap()
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .unwrap();
                let names = columns.join(",");
                let rows = donor
                    .with_conn(|source| {
                        Ok(fixture_query_rows(
                            source,
                            &format!("SELECT {names} FROM {table} ORDER BY rowid"),
                        ))
                    })
                    .unwrap();
                for row in rows {
                    conn.execute(
                        &format!(
                            "INSERT INTO {table} ({names}) VALUES ({})",
                            vec!["?"; columns.len()].join(",")
                        ),
                        rusqlite::params_from_iter(row),
                    )
                    .unwrap();
                }
            }
            assert_eq!(
                workspace_schema_migration_history(&conn).unwrap().last(),
                Some(&(107, PLAN_NAME.into()))
            );
            assert_pre_subject(&conn, 107);
            crate::plugin_catalog::verify(&conn).unwrap();
            plugin_plan::verify_plan_schema(&conn).unwrap();
            for (_, receipt, state) in &receipts {
                let loaded = plugin_plan::plugin_plan_load(&conn, "a", &receipt.plan_id).unwrap();
                assert_eq!(loaded.plan, receipt.plan);
                assert_eq!(loaded.pin_state, *state);
            }
            assert_health(&conn);
            drop(conn);
            let conn = raw(&path);
            (dir, path, conn, receipts)
        }

        fn assert_pre_subject(conn: &Connection, version: i64) {
            assert_eq!(current_schema_version(conn).unwrap(), version);
            assert_component13(conn);
            assert_eq!(conn.query_row("SELECT count(*) FROM sqlite_schema WHERE name LIKE 'ticket_subject_%' OR name LIKE 'ticket_legacy_orchestrator_%'", [], |r|r.get::<_,i64>(0)).unwrap(), 0);
            let kinds: String = conn
                .query_row(
                    "SELECT sql FROM sqlite_schema WHERE name='ticket_worker_assignments'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(
                kinds.contains("CHECK(principal_kind IN ('user', 'worker', 'workspace_agent'))")
            );
            assert!(!kinds.contains("'subject'"));
            let capture: String = conn
                .query_row(
                    "SELECT sql FROM sqlite_schema WHERE name='notification_assignment_change'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(!capture.contains("'subject'"));
        }

        fn assert_health(conn: &Connection) {
            assert_eq!(
                fixture_query_rows(conn, "PRAGMA integrity_check"),
                vec![vec![rusqlite::types::Value::Text("ok".into())]]
            );
            assert!(fixture_query_rows(conn, "PRAGMA foreign_key_check").is_empty());
            assert_closed_schema_writes(conn);
        }

        fn host_history(conn: &Connection) -> Rows {
            fixture_query_rows(
                conn,
                "SELECT rowid,* FROM __yoi_schema_migrations ORDER BY version",
            )
        }

        // Save the exact source columns, including rowids and original JSON bytes.
        // Only the owning diagnostic enrichment and two append-only ledgers are
        // excluded from the success oracle; failures compare ALL data and schema.
        fn data(conn: &Connection, preserved: bool) -> Vec<(String, Rows)> {
            let tables = conn
                .prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")
                .unwrap()
                .query_map([], |r| r.get::<_, String>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            tables
                .into_iter()
                .filter(|table| !preserved || !table.contains("schema_migrations"))
                .map(|table| {
                    let columns = conn
                        .prepare(&format!("PRAGMA table_info(\"{table}\")"))
                        .unwrap()
                        .query_map([], |r| r.get::<_, String>(1))
                        .unwrap()
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .unwrap()
                        .into_iter()
                        .filter(|column| {
                            !(preserved
                                && table == "ticket_worker_assignments"
                                && column == "tracking_migration_diagnostic")
                        })
                        .map(|column| format!("\"{column}\""))
                        .collect::<Vec<_>>()
                        .join(",");
                    // Later historical migrations may introduce a new sequence.
                    // Existing append identities, particularly notification sources,
                    // must not move; allow only newly introduced sequence names.
                    let sequence_filter = if preserved && table == "sqlite_sequence" {
                        let names = conn
                            .prepare("SELECT quote(name) FROM sqlite_sequence ORDER BY name")
                            .unwrap()
                            .query_map([], |r| r.get::<_, String>(0))
                            .unwrap()
                            .collect::<rusqlite::Result<Vec<_>>>()
                            .unwrap();
                        if names.is_empty() {
                            " WHERE 0".into()
                        } else {
                            format!(" WHERE name IN ({})", names.join(","))
                        }
                    } else {
                        String::new()
                    };
                    let query = format!(
                        "SELECT rowid,{columns} FROM \"{table}\"{sequence_filter} ORDER BY rowid"
                    );
                    let rows = fixture_query_rows(conn, &query);
                    (query, rows)
                })
                .collect()
        }

        fn assert_data(conn: &Connection, before: &[(String, Rows)]) {
            for (query, rows) in before {
                assert_eq!(&fixture_query_rows(conn, query), rows, "{query}");
            }
        }

        fn snapshot(conn: &Connection) -> (i64, Rows, Vec<(String, Rows)>) {
            (cookie(conn), schema(conn), data(conn, false))
        }

        fn assert_final(conn: &Connection, version: i64, host: &Rows, component: &Rows) {
            let history = host_history(conn);
            assert_eq!(&history[..host.len()], host);
            let expected = expected_tail(version);
            assert_eq!(
                workspace_schema_migration_history(conn)
                    .unwrap()
                    .into_iter()
                    .filter(|(v, _)| *v > version)
                    .collect::<Vec<_>>(),
                expected
            );
            assert_eq!(history.len(), host.len() + expected.len());
            assert_component14_append(conn, component);
            crate::plugin_catalog::verify(conn).unwrap();
            plugin_plan::verify_plan_schema(conn).unwrap();
            verify(conn).unwrap();
            assert_health(conn);
            // Reuse the owner's real constraint probe, rolled back so it is not
            // confused with migration writes or preserved notification evidence.
            let tx = conn.unchecked_transaction().unwrap();
            assert_subject_contract(&tx);
            tx.rollback().unwrap();
            for status in ["waiting", "inprogress", "done", "unknown", "released"] {
                let actual: String = conn.query_row("SELECT tracking_migration_diagnostic FROM ticket_worker_assignments WHERE assignment_id=?1", [status], |r|r.get(0)).unwrap();
                assert_eq!(
                    actual,
                    if status == "unknown" {
                        format!("Existing ambiguity\n{LEGACY_DIAGNOSTIC}")
                    } else {
                        LEGACY_DIAGNOSTIC.into()
                    }
                );
            }
        }

        fn expected_tail(version: i64) -> Vec<(i64, String)> {
            let mut tail = match version {
                96 => vec![
                    (
                        97,
                        "MR notifications and authenticated implementation results".into(),
                    ),
                    (98, "Assignment-independent implementation reports".into()),
                    (
                        99,
                        "Explicit hosting administrators and self-registration policy".into(),
                    ),
                    (
                        100,
                        "Provisional users and scoped passkey activation".into(),
                    ),
                    (101, "Durable explicit Subject body release".into()),
                    (102, "Durable failed Subject creation recovery".into()),
                    (103, "Passkey metadata and atomic addition results".into()),
                    (
                        104,
                        "Converge Passkey results and Job input retention".into(),
                    ),
                    (
                        105,
                        "Durable Subject deletion snapshots and Job admission fences".into(),
                    ),
                    (106, crate::plugin_catalog::MIGRATION_NAME.into()),
                    (107, PLAN_NAME.into()),
                    (108, SUBJECT_NAME.into()),
                ],
                106 => vec![(107, PLAN_NAME.into()), (108, SUBJECT_NAME.into())],
                107 => vec![(108, SUBJECT_NAME.into())],
                _ => panic!("unsupported fixture origin"),
            };
            tail.push((109, "Durable Plugin Host invocations and HTTPS effect receipts".into()));
            tail
        }

        fn plan_steps(version: i64) -> Vec<WorkspaceSchemaMigrationStep> {
            expected_tail(version)
                .into_iter()
                .map(|(version, name)| WorkspaceSchemaMigrationStep { version, name })
                .collect()
        }

        #[test]
        fn retained96_and106_preflight_apply_and_reopen_preserve_original_evidence() {
            for version in [96, 106] {
                let (_dir, path, conn) = retained(version);
                assert_pre_subject(&conn, version);
                let before = snapshot(&conn);
                let rows = data(&conn, true);
                let host = host_history(&conn);
                let component = component_history(&conn);
                let plan = SqliteWorkspaceStore::migration_plan(&path).unwrap();
                assert_eq!(plan.current_schema_version, version);
                assert_eq!(plan.migrations, plan_steps(version));
                assert_eq!(
                    snapshot(&conn),
                    before,
                    "read-only preflight changed source"
                );
                drop(conn);
                assert_eq!(SqliteWorkspaceStore::migrate_database(&path).unwrap(), plan);
                let conn = raw(&path);
                assert_data(&conn, &rows);
                assert_final(&conn, version, &host, &component);
                let final_snapshot = snapshot(&conn);
                drop(conn);
                drop(SqliteWorkspaceStore::open_for_test(&path).unwrap());
                let conn = raw(&path);
                assert_eq!(snapshot(&conn), final_snapshot);
                assert!(
                    SqliteWorkspaceStore::migration_plan(&path)
                        .unwrap()
                        .migrations
                        .is_empty()
                );
            }
        }

        fn plugin_schema(conn: &Connection) -> Rows {
            fixture_query_rows(
                conn,
                "SELECT type,name,tbl_name,rootpage,sql FROM sqlite_schema WHERE tbl_name LIKE 'plugin_%' AND tbl_name NOT IN ('plugin_invocations','plugin_effect_receipts') ORDER BY type,name",
            )
        }

        #[tokio::test]
        async fn applied107_preserves_canonical_plans_pins_and_released_replay_through_later_tail() {
            let (_dir, path, conn, receipts) = populated107().await;
            let before = snapshot(&conn);
            let rows = data(&conn, true);
            let plans = plugin_schema(&conn);
            let contracts = principal_table_contracts(&conn);
            let host = host_history(&conn);
            let component = component_history(&conn);
            let plan = SqliteWorkspaceStore::migration_plan(&path).unwrap();
            assert_eq!(plan.current_schema_version, 107);
            assert_eq!(
                plan.migrations,
                plan_steps(107)
            );
            assert_eq!(snapshot(&conn), before);
            drop(conn);
            SqliteWorkspaceStore::migrate_database(&path).unwrap();
            let conn = raw(&path);
            assert_data(&conn, &rows);
            assert_final(&conn, 107, &host, &component);
            assert_eq!(plugin_schema(&conn), plans);
            assert_eq!(principal_table_contracts(&conn), contracts);
            let final_snapshot = snapshot(&conn);
            drop(conn);
            let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
            let conn = raw(&path);
            assert_eq!(snapshot(&conn), final_snapshot);
            for (key, receipt, state) in receipts {
                let saved = store.plugin_plan_load("a", &receipt.plan_id).unwrap();
                assert_eq!(saved.plan, receipt.plan);
                assert_eq!(saved.fingerprint, receipt.fingerprint);
                assert_eq!(saved.pin_state, state);
                assert_eq!(
                    store
                        .plugin_plan_create("actor", &key, &receipt.plan)
                        .unwrap(),
                    receipt
                );
                if state == PlanPinState::Released {
                    assert_eq!(
                        conn.query_row(
                            "SELECT count(*) FROM plugin_references WHERE owner=?1",
                            [&receipt.plan_id],
                            |r| r.get::<_, i64>(0)
                        )
                        .unwrap(),
                        0
                    );
                }
                let changed = WorkerPluginExecutionPlan::canonicalize(WorkerPluginPlanInput {
                    workspace_id: "a".into(),
                    config: receipt.plan.config().clone(),
                    profile: receipt.plan.profile().clone(),
                    execution: crate::plugin_plan::HostExecutionIdentity {
                        worker_id: "changed-worker".into(),
                        ..receipt.plan.execution().clone()
                    },
                    plugins: receipt.plan.plugins().to_vec(),
                })
                .unwrap();
                assert!(matches!(
                    store.plugin_plan_create("actor", &key, &changed),
                    Err(Error::PluginCatalog(
                        crate::plugin_catalog::PluginCatalogError::Conflict
                    ))
                ));
            }
            assert_data(&conn, &rows);
        }

        fn interrupt108(conn: &Connection) {
            conn.execute_batch("CREATE TRIGGER interrupt_subject_tail BEFORE INSERT ON __yoi_schema_migrations WHEN NEW.version=108 BEGIN SELECT RAISE(ABORT,'subject tail marker failure'); END;").unwrap();
        }

        #[tokio::test]
        async fn marker108_failure_rolls_back_all_subject_changes_retains107_and_raw_retries() {
            let (_dir, path, conn, _receipts) = populated107().await;
            interrupt108(&conn);
            let before = snapshot(&conn);
            let host = host_history(&conn);
            let component = component_history(&conn);
            assert!(SqliteWorkspaceStore::migration_plan(&path).is_err());
            assert_eq!(snapshot(&conn), before);
            let error = apply_migrations(&conn).unwrap_err();
            assert!(
                error.to_string().contains("subject tail marker failure"),
                "{error}"
            );
            assert_eq!(snapshot(&conn), before);
            assert_pre_subject(&conn, 107);
            assert_health(&conn);
            drop(conn);
            let conn = raw(&path);
            assert_eq!(snapshot(&conn), before);
            conn.execute_batch("DROP TRIGGER interrupt_subject_tail")
                .unwrap();
            assert_eq!(
                SqliteWorkspaceStore::migration_plan(&path)
                    .unwrap()
                    .migrations,
                plan_steps(107)
            );
            apply_migrations(&conn).unwrap();
            assert_final(&conn, 107, &host, &component);
        }

        #[test]
        fn real106_commits107_before108_failure_but_dryrun_never_changes_source() {
            let (_dir, path, conn) = retained(106);
            interrupt108(&conn);
            let before = snapshot(&conn);
            let host = host_history(&conn);
            let component = component_history(&conn);
            let capture = fixture_query_rows(
                &conn,
                "SELECT name,sql FROM sqlite_schema WHERE name LIKE 'notification_%' AND type='trigger' ORDER BY name",
            );
            assert!(SqliteWorkspaceStore::migration_plan(&path).is_err());
            assert_eq!(snapshot(&conn), before);
            assert!(SqliteWorkspaceStore::migrate_database(&path).is_err());
            assert_eq!(snapshot(&conn), before);
            assert!(
                apply_migrations(&conn)
                    .unwrap_err()
                    .to_string()
                    .contains("subject tail marker failure")
            );
            assert_pre_subject(&conn, 107);
            assert!(table_exists(&conn, "plugin_execution_plans").unwrap());
            plugin_plan::verify_plan_schema(&conn).unwrap();
            let history = host_history(&conn);
            assert_eq!(&history[..host.len()], &host);
            assert_eq!(history.len(), host.len() + 1);
            assert_eq!(
                workspace_schema_migration_history(&conn).unwrap().last(),
                Some(&(107, PLAN_NAME.into()))
            );
            assert_eq!(component_history(&conn), component);
            assert_eq!(
                fixture_query_rows(
                    &conn,
                    "SELECT name,sql FROM sqlite_schema WHERE name LIKE 'notification_%' AND type='trigger' ORDER BY name"
                ),
                capture
            );
            assert_health(&conn);
            let failed = snapshot(&conn);
            drop(conn);
            let conn = raw(&path);
            assert_eq!(snapshot(&conn), failed);
            conn.execute_batch("DROP TRIGGER interrupt_subject_tail")
                .unwrap();
            assert_eq!(
                SqliteWorkspaceStore::migration_plan(&path)
                    .unwrap()
                    .migrations,
                plan_steps(107)
            );
            apply_migrations(&conn).unwrap();
            assert_final(&conn, 106, &host, &component);
        }

        #[tokio::test]
        async fn preexisting_plan_or_catalog_drift_refuses108_without_repair() {
            for trigger in ["plugin_plan_snapshot_immutable", "plugin_reference_live"] {
                let (_dir, path, conn, _receipts) = populated107().await;
                conn.execute_batch(&format!("DROP TRIGGER {trigger}"))
                    .unwrap();
                let before = snapshot(&conn);
                assert!(SqliteWorkspaceStore::migration_plan(&path).is_err());
                assert_eq!(snapshot(&conn), before);
                assert!(apply_migrations(&conn).is_err());
                assert_eq!(snapshot(&conn), before);
                assert_pre_subject(&conn, 107);
                drop(conn);
                assert!(SqliteWorkspaceStore::open_for_test(&path).is_err());
                assert_eq!(snapshot(&raw(&path)), before);
            }
        }

        #[tokio::test]
        async fn current_missing_host_guard_or_plan_drift_reopen_refuses_without_repair() {
            for trigger in [
                "ticket_legacy_orchestrator_delivery_no_insert",
                "ticket_legacy_orchestrator_start_no_insert",
                "plugin_plan_snapshot_immutable",
            ] {
                let (_dir, path, conn, _receipts) = populated107().await;
                apply_migrations(&conn).unwrap();
                conn.execute_batch(&format!("DROP TRIGGER {trigger}"))
                    .unwrap();
                let before = snapshot(&conn);
                drop(conn);
                assert!(SqliteWorkspaceStore::open_for_test(&path).is_err());
                let conn = raw(&path);
                assert_eq!(snapshot(&conn), before);
                assert_health(&conn);
            }
        }
    }
}
