//! Storage component tests: real retained migration functions and published SQL
//! on isolated SQLite databases; no authority, Runtime or domain recovery fakes.
//! The normal workspace-server lib test target runs this module. These fixtures
//! attest physical acceptance/data retention, not policy transfer or Job recovery.
use super::*;
use rusqlite::{backup::Backup, params};

fn save_history(
    conn: &Connection,
    stage: Stage,
    baseline: i64,
    predecessor: Option<(Family, usize)>,
) {
    conn.execute("DELETE FROM __yoi_schema_migrations", [])
        .unwrap();
    conn.execute(
        "INSERT INTO __yoi_schema_migrations(version,name) VALUES(?1,?2)",
        params![baseline, super::super::SCHEMA_BASELINE_NAME],
    )
    .unwrap();
    let published = match stage {
        Stage::Published(family, count) => Some((family, count)),
        _ => predecessor,
    };
    if let Some((family, count)) = published {
        for (index, name) in family.names().iter().take(count).enumerate() {
            let version = 114 + index as i64;
            if version > baseline {
                conn.execute(
                    "INSERT INTO __yoi_schema_migrations(version,name) VALUES(?1,?2)",
                    params![version, name],
                )
                .unwrap();
            }
        }
    }
    if let Stage::Converged(current) = stage {
        for (version, name) in [
            (119, CONVERGED),
            (120, PRIOR_RELEASE),
            (121, CLEANUP_COVERAGE),
        ] {
            if version > baseline && version <= current {
                conn.execute(
                    "INSERT INTO __yoi_schema_migrations(version,name) VALUES(?1,?2)",
                    params![version, name],
                )
                .unwrap();
            }
        }
    }
}

fn fixture(stage: Stage, baseline: i64, predecessor: Option<(Family, usize)>) -> Connection {
    let conn = reference_base().unwrap();
    install_reference(&conn, stage).unwrap();
    save_history(&conn, stage, baseline, predecessor);
    conn.execute_batch("INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at)
        VALUES('owner','user','owner','Owner','1','1');
        INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at)
        VALUES('workspace-a','owner','Saved workspace','active','1','1');
        INSERT INTO backend_jobs(workspace_id,job_id,purpose,input_digest,input_ref,request_json,intent_fingerprint,state,current_attempt,created_at,updated_at,input_disposition)
        VALUES('workspace-a','saved-job','journal','digest','input','{}','fingerprint','pending',1,'1','1','erased');
        INSERT INTO backend_job_attempts(workspace_id,job_id,attempt_id,attempt,input_digest,state,deadline_at,created_at,updated_at)
        VALUES('workspace-a','saved-job','saved-attempt',1,'digest','reserved','2','1','1');
        INSERT INTO worker_removal_operations(operation_id,plan_id,input_fingerprint,workspace_id,runtime_id,worker_id,worker_updated_at,policy_id,policy_digest,session_disposition,metadata_disposition,archive_retention_kind,diagnostics_disposition,blockers_json,state,reason,created_at,updated_at)
        VALUES('saved-removal','saved-plan','saved-fingerprint','workspace-a','runtime','worker','1','policy','digest','erase','erase','none','erase','[]','succeeded','saved','1','1');").unwrap();
    if super::super::table_exists(&conn, "permission_policy_receipts").unwrap() {
        conn.execute_batch("INSERT INTO permission_policy_receipts(workspace_id,operation_id,fingerprint,result_json)
            VALUES('workspace-a','saved-permission','saved-fingerprint','{\"saved\":true}');").unwrap();
    }
    if super::super::table_exists(&conn, "server_feature_root").unwrap() {
        conn.execute_batch("INSERT INTO server_feature_root(root_slot,reservation_id,request_fingerprint,state,accepted_receipt)
            VALUES(1,'saved-reservation','saved-fingerprint','accepted','saved-root-receipt');
            INSERT INTO content_history_targets(workspace,target,head,erased)
            VALUES('workspace-a','workspace-config',NULL,1);").unwrap();
    }
    conn
}

fn saved_rows(conn: &Connection) -> Vec<(String, Vec<String>)> {
    let names = conn.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name").unwrap()
        .query_map([], |row| row.get::<_, String>(0)).unwrap()
        .collect::<rusqlite::Result<Vec<_>>>().unwrap();
    names
        .into_iter()
        .map(|name| {
            let mut statement = conn
                .prepare(&format!("SELECT * FROM \"{}\"", name.replace('"', "\"\"")))
                .unwrap();
            let count = statement.column_count();
            let mut rows = statement
                .query_map([], |row| {
                    (0..count)
                        .map(|index| row.get::<_, rusqlite::types::Value>(index))
                        .collect::<rusqlite::Result<Vec<_>>>()
                })
                .unwrap()
                .map(|row| format!("{:?}", row.unwrap()))
                .collect::<Vec<_>>();
            rows.sort();
            (name, rows)
        })
        .collect()
}

fn assert_accepted_without_domain_changes(conn: &Connection, current: i64) {
    let rows = saved_rows(conn);
    let shape = snapshot(conn).unwrap();
    verify_history(conn, current).unwrap();
    if current >= 114 {
        let plan = published_plan(conn, current).unwrap().unwrap();
        assert_eq!(plan.migrations.len(), 1);
        assert_eq!(plan.migrations[0].version, 122);
        assert_eq!(plan.migrations[0].name, crate::config_adoption::MIGRATION_NAME);
        assert_eq!(plan.target_schema_version, 122);
    }
    assert_eq!(rows, saved_rows(conn));
    assert_eq!(shape, snapshot(conn).unwrap());
}

#[test]
fn each_published_receipt_prefix_and_fresh_branch_baseline_requires_its_real_shape() {
    for family in FAMILIES {
        for count in 1..=family.names().len() {
            let stage = Stage::Published(family, count);
            let current = 113 + count as i64;
            // A baseline can start at any retained branch stage, not just 113.
            for baseline in 113..=current {
                let conn = fixture(stage, baseline, None);
                assert_accepted_without_domain_changes(&conn, current);
            }
        }
    }
}

#[test]
fn converged_shapes_keep_all_real_predecessor_receipts_and_erasure_fences() {
    let origins = std::iter::once(None).chain(
        FAMILIES
            .into_iter()
            .flat_map(|family| (1..=family.names().len()).map(move |count| Some((family, count)))),
    );
    for origin in origins {
        for current in 119..=121 {
            let conn = fixture(Stage::Converged(current), 113, origin);
            assert_accepted_without_domain_changes(&conn, current);
            let erased: i64 = conn
                .query_row("SELECT erased FROM content_history_targets", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(erased, 1);
            // A physical check must not act as the Job owner's new Failed migration.
            let state: String = conn
                .query_row("SELECT state FROM backend_jobs", [], |row| row.get(0))
                .unwrap();
            assert_eq!(state, "pending");
        }
    }
    for current in 119..=121 {
        for baseline in 119..=current {
            assert_accepted_without_domain_changes(
                &fixture(Stage::Converged(current), baseline, None),
                current,
            );
        }
    }
}

#[test]
fn published_database_runs_normal_store_preparation_and_reopens_without_downgrade() {
    for stage in [
        Stage::Published(Family::Permissions, 2),
        Stage::Published(Family::Removal, 4),
        Stage::Published(Family::Jobs, 5),
        Stage::Converged(119),
        Stage::Converged(120),
        Stage::Converged(121),
    ] {
        let current = match stage {
            Stage::Published(_, count) => 113 + count as i64,
            Stage::Converged(version) => version,
            Stage::Base => 113,
        };
        let source = fixture(stage, 113, None);
        let expected = saved_rows(&source);
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("catalog.db");
        let mut destination = Connection::open(&path).unwrap();
        Backup::new(&source, &mut destination)
            .unwrap()
            .run_to_completion(128, std::time::Duration::ZERO, None)
            .unwrap();
        drop(destination);
        for _ in 0..2 {
            let conn = Connection::open(&path).unwrap();
            // Production store owner, not a separate test migration implementation.
            super::super::prepare_connection(&conn).unwrap();
            assert_eq!(super::super::current_schema_version(&conn).unwrap(), 122);
            for (table, rows) in &expected {
                if table != "__yoi_schema_migrations" {
                    assert_eq!(saved_rows(&conn).into_iter().find(|(name, _)| name == table).unwrap().1, *rows, "{table}, from {current}");
                }
            }
            let mut history = super::super::workspace_schema_migration_history(&conn).unwrap();
            assert_eq!(history.pop(), Some((122, crate::config_adoption::MIGRATION_NAME.into())));
            assert_eq!(history, super::super::workspace_schema_migration_history(&source).unwrap());
            verify_history(&conn, 122).unwrap();
        }
    }
}

#[test]
fn unknown_mixed_missing_receipts_and_shapes_are_diagnosed_without_writes() {
    for (sql, current) in [
        (
            "UPDATE __yoi_schema_migrations SET name='unknown receipt' WHERE version=115",
            118,
        ),
        ("DELETE FROM __yoi_schema_migrations WHERE version=116", 118),
        (
            "UPDATE __yoi_schema_migrations SET name='Worker removal expected Feature database identity' WHERE version=115",
            118,
        ),
        ("DROP TRIGGER backend_job_cleanup_owner_immutable", 118),
        (
            "CREATE INDEX unreceipted_job_index ON backend_jobs(job_id)",
            118,
        ),
        (
            "INSERT INTO __yoi_schema_migrations(version,name) VALUES(122,'unknown future')",
            122,
        ),
    ] {
        let conn = fixture(Stage::Published(Family::Jobs, 5), 113, None);
        conn.execute_batch(sql).unwrap();
        let rows = saved_rows(&conn);
        let shape = snapshot(&conn).unwrap();
        assert!(verify_history(&conn, current).is_err(), "{sql}");
        assert_eq!(saved_rows(&conn), rows);
        assert_eq!(snapshot(&conn).unwrap(), shape);
    }
    let conn = fixture(Stage::Published(Family::Permissions, 2), 113, None);
    conn.execute_batch(COLLECTION_SQL).unwrap();
    assert!(
        verify_history(&conn, 115)
            .unwrap_err()
            .to_string()
            .contains("mismatch")
    );
    let conn = fixture(Stage::Converged(121), 113, None);
    conn.execute_batch("DROP TRIGGER backend_job_cleanup_origin_immutable")
        .unwrap();
    assert!(verify_history(&conn, 121).is_err());
}

#[test]
fn missing_or_weakened_constraints_and_literal_changes_are_not_shape_equivalent() {
    let conn = fixture(Stage::Published(Family::Permissions, 1), 113, None);
    conn.execute_batch("DROP TABLE permission_policy_receipts;
        CREATE TABLE permission_policy_receipts(workspace_id TEXT NOT NULL,operation_id TEXT NOT NULL,fingerprint TEXT NOT NULL,result_json TEXT NOT NULL,PRIMARY KEY(workspace_id,operation_id));").unwrap();
    assert!(verify_history(&conn, 114).is_err());
    let conn = fixture(Stage::Published(Family::Jobs, 5), 113, None);
    conn.execute_batch("DROP TRIGGER backend_job_cleanup_owner_immutable;
        CREATE TRIGGER backend_job_cleanup_owner_immutable BEFORE UPDATE ON backend_job_cleanup_invocation_owners
        BEGIN SELECT RAISE(ABORT,'Cleanup invocation ownership is immutable'); END;").unwrap();
    assert!(verify_history(&conn, 118).is_err());
}

fn retained_to(target: i64, alternate_103: bool) -> (tempfile::TempDir, Connection) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("retained.db");
    super::super::tests::prepare_schema_v50(&path, Some("workspace-a"));
    let conn = Connection::open(&path).unwrap();
    super::super::configure_sqlite(&conn).unwrap();
    for migration in super::super::MIGRATIONS
        .iter()
        .filter(|migration| migration.version <= target)
    {
        let current = super::super::current_schema_version(&conn).unwrap();
        verify_history(&conn, current).unwrap();
        let migration = if migration.version == 103 && alternate_103 {
            &super::super::JOB_INPUT_RETENTION_SCHEMA_103
        } else {
            migration
        };
        (migration.apply)(&conn).unwrap();
    }
    verify_history(&conn, target).unwrap();
    (temp, conn)
}

#[test]
fn oldest_data_bearing_schema_and_both_real_103_histories_migrate_without_relabeling() {
    for alternate in [false, true] {
        let (_temp, conn) = retained_to(103, alternate);
        let original = super::super::workspace_schema_migration_history(&conn).unwrap();
        super::super::prepare_connection(&conn).unwrap();
        verify_history(&conn, super::super::LATEST_SCHEMA_VERSION).unwrap();
        let updated = super::super::workspace_schema_migration_history(&conn).unwrap();
        assert_eq!(&updated[..original.len()], &original);
        let name: String = conn
            .query_row(
                "SELECT display_name FROM workspaces WHERE workspace_id='workspace-a'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(name, "Workspace A");
    }
}

#[test]
fn fresh_103_baselines_use_shape_and_never_repair_missing_receipts_or_guards() {
    for alternate in [false, true] {
        let (_temp, conn) = retained_to(103, alternate);
        conn.execute_batch("DELETE FROM __yoi_schema_migrations;
            INSERT INTO __yoi_schema_migrations(version,name) VALUES(103,'workspace schema baseline');").unwrap();
        assert_accepted_without_domain_changes(&conn, 103);
        if alternate {
            conn.execute_batch("DROP TRIGGER backend_job_input_retirement")
                .unwrap();
        } else {
            conn.execute_batch("DROP TABLE passkey_addition_receipts")
                .unwrap();
        }
        let rows = saved_rows(&conn);
        assert!(verify_history(&conn, 103).is_err());
        assert_eq!(saved_rows(&conn), rows);
    }
    let (_temp, conn) = retained_to(102, false);
    conn.execute_batch(
        "INSERT INTO __yoi_schema_migrations(version,name) VALUES(103,'workspace schema baseline')",
    )
    .unwrap();
    assert!(verify_history(&conn, 103).is_err());
}

#[test]
fn later_family_objects_without_receipts_and_half_converged_103_are_rejected() {
    let conn = fixture(Stage::Base, 113, None);
    conn.execute_batch(CLAIMS_SQL).unwrap();
    assert!(verify_history(&conn, 113).is_err());
    let (_temp, conn) = retained_to(103, false);
    conn.execute_batch("ALTER TABLE backend_jobs ADD COLUMN input_disposition TEXT NOT NULL DEFAULT 'available' CHECK(input_disposition IN ('available','reclaimed','erased','invalidated'))").unwrap();
    assert!(verify_history(&conn, 103).is_err());
}

#[test]
fn schema_comparison_preserves_literals_programs_and_composite_key_order() {
    let left = declarations(
        tokens("CREATE TABLE item(a TEXT, b TEXT, PRIMARY KEY(a,b), CHECK(a='Sensitive'))")
            .unwrap(),
    )
    .unwrap();
    let reordered = declarations(
        tokens("CREATE TABLE \"item\"(b TEXT, CHECK(a='Sensitive'), a TEXT, PRIMARY KEY(a,b))")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(left, reordered);
    for sql in [
        "CREATE TABLE item(a TEXT, b TEXT, PRIMARY KEY(b,a), CHECK(a='Sensitive'))",
        "CREATE TABLE item(a TEXT, b TEXT, PRIMARY KEY(a,b), CHECK(a='sensitive'))",
    ] {
        assert_ne!(left, declarations(tokens(sql).unwrap()).unwrap());
    }
}
