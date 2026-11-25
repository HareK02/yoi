//! Real isolated SQLite fixtures compose both published tails from retained 141.
//! The production planner/transaction is exercised; no running database is used.
use super::*;

fn fixture(hare: bool, version: i64) -> (tempfile::TempDir, Connection) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("branch.db");
    super::super::tests::prepare_retained_schema(&path, 141);
    let conn = Connection::open(path).unwrap();
    configure_sqlite(&conn).unwrap();
    if hare {
        passkey_addition_code::migrate(&conn).unwrap();
        if version >= 143 {
            workspace_members::migrate(&conn).unwrap();
        }
    } else {
        feature_initialization::migrate(&conn).unwrap();
        if version >= 143 {
            feature_worker_launch::migrate(&conn).unwrap();
        }
        if version >= 144 {
            memory_retirement::migrate(&conn).unwrap();
        }
    }
    (dir, conn)
}

fn schema(conn: &Connection) -> Vec<(String, String)> {
    conn.prepare("SELECT name,sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY name")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}
fn receipts(conn: &Connection) -> Vec<(i64, String, String)> {
    conn.prepare("SELECT version,name,applied_at FROM __yoi_schema_migrations ORDER BY version")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

#[test]
fn published_branches_converge_preserving_exact_receipts_and_non_target_data() {
    for (hare, version) in [
        (true, 142),
        (true, 143),
        (false, 142),
        (false, 143),
        (false, 144),
    ] {
        let (_dir, conn) = fixture(hare, version);
        conn.execute_batch("INSERT INTO accounts VALUES('branch-owner','user','branch-owner','Owner','created','updated'); INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at) VALUES('branch-ws','branch-owner','Keep','active','created','updated'); INSERT INTO objectives(workspace_id,objective_id,title,state,body_md,created_at,updated_at) VALUES('branch-ws','keep','Keep','active','body bytes','created','updated');").unwrap();
        if hare {
            conn.execute_batch("INSERT INTO users VALUES('branch-user','branch-owner','branch-user','User','created','updated'); INSERT INTO passkey_addition_codes VALUES('branch-user','original-issuance','original-hash','https://a.test','a.test','expiry',2);").unwrap();
        }
        if hare && version == 143 {
            conn.execute_batch("INSERT INTO workspace_members VALUES('branch-ws','branch-owner','original-key'); INSERT INTO workspace_membership_changes VALUES('branch-ws','change-key'); INSERT INTO workspace_membership_operations VALUES('branch-ws','operation','branch-owner','original request','original receipt','original time');").unwrap();
        }
        if hare {
            conn.execute("INSERT INTO worker_create_reservations(workspace_id,allocation_key,worker_id,runtime_id,create_fingerprint,state,created_at,updated_at) VALUES('branch-ws','legacy',?1,'runtime','immutable-fingerprint','reserved','created','updated')",[WorkerId::now_v7().to_string()]).unwrap();
        }
        let before = receipts(&conn);
        let before_schema = schema(&conn);
        let plan = workspace_schema_migration_plan(&conn).unwrap();
        assert_eq!(
            SqliteWorkspaceStore::migration_plan(conn.path().unwrap()).unwrap(),
            plan
        );
        assert_eq!(
            schema(&conn),
            before_schema,
            "public preflight changed source DDL"
        );
        assert_eq!(
            receipts(&conn),
            before,
            "public preflight changed source receipts"
        );
        assert_eq!(
            plan.migrations.last().unwrap().name,
            if hare {
                branch_convergence::HARE_NAME
            } else {
                branch_convergence::DEVELOP_NAME
            }
        );
        if hare {
            assert!(!plan.migrations.iter().any(|m| m.version == 144));
        }
        apply_migrations(&conn).unwrap();
        let after = receipts(&conn);
        assert_eq!(
            after[before.len()..]
                .iter()
                .map(|r| (r.0, r.1.as_str()))
                .collect::<Vec<_>>(),
            plan.migrations
                .iter()
                .map(|m| (m.version, m.name.as_str()))
                .collect::<Vec<_>>()
        );
        assert_eq!(&after[..before.len()], before.as_slice());
        assert_eq!(current_schema_version(&conn).unwrap(), 145);
        assert!(
            !workspace_schema_migration_plan(&conn)
                .unwrap()
                .migration_required()
        );
        assert_eq!(
            conn.query_row(
                "SELECT body_md FROM objectives WHERE objective_id='keep'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "body bytes"
        );
        if hare {
            assert_eq!(
                conn.query_row(
                    "SELECT code_hash FROM passkey_addition_codes WHERE user_id='branch-user'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
                "original-hash"
            );
            let saved:(String,Option<String>)=conn.query_row("SELECT create_fingerprint,feature_initialization_json FROM worker_create_reservations WHERE allocation_key='legacy'",[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
            assert_eq!(saved, ("immutable-fingerprint".into(), None));
            assert_eq!(
                conn.query_row(
                    "SELECT count(*) FROM worker_create_operation_bindings",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                0
            );
        }
        if hare && version == 143 {
            assert_eq!(conn.query_row("SELECT receipt_json FROM workspace_membership_operations WHERE operation_key='operation'",[],|r|r.get::<_,String>(0)).unwrap(),"original receipt");
            assert_eq!(
                conn.query_row("SELECT membership_key FROM workspace_members", [], |r| {
                    r.get::<_, String>(0)
                })
                .unwrap(),
                "original-key"
            );
        }
        assert!(!table_exists(&conn, "workspace_memory_documents").unwrap());
        assert!(table_exists(&conn, "subjektiv_memory_settings").unwrap());
        assert_eq!(
            conn.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r
                .get::<_, i64>(
                0
            ))
            .unwrap(),
            0
        );
        let fresh = SqliteWorkspaceStore::in_memory_for_test().unwrap();
        for table in [
            "workspace_members",
            "workspace_membership_operations",
            "workspace_membership_changes",
            "passkey_addition_codes",
            "worker_create_operation_bindings",
            "subjektiv_memory_settings",
        ] {
            let select = |c: &Connection| {
                storage_formats::convergence_schema_signature(c)
                    .unwrap()
                    .into_iter()
                    .filter(|(_, object)| object.1 == table)
                    .collect::<std::collections::BTreeMap<_, _>>()
            };
            assert_eq!(
                select(&conn),
                fresh.with_conn(|c| Ok(select(c))).unwrap(),
                "{hare} {version} {table}"
            );
        }
    }
}

#[test]
fn hare_convergence_failure_rolls_back_ddl_data_and_receipts_then_retries_once() {
    let (_dir, conn) = fixture(true, 143);
    conn.execute_batch("CREATE TRIGGER fail_convergence BEFORE INSERT ON __yoi_schema_migrations WHEN NEW.version=145 BEGIN SELECT RAISE(ABORT,'injected convergence failure'); END;").unwrap();
    conn.execute_batch("INSERT INTO accounts VALUES('rollback-owner','user','rollback-owner','Owner','1','1'); INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at) VALUES('rollback-ws','rollback-owner','Keep','active','1','1'); INSERT INTO workspace_memory_settings VALUES('rollback-ws','日本語','1','1'); INSERT INTO workspace_memory_documents VALUES('rollback-ws','legacy bytes','1','1');").unwrap();
    let before_schema = schema(&conn);
    let before_receipts = receipts(&conn);
    assert!(branch_convergence::migrate(&conn).is_err());
    assert_eq!(schema(&conn), before_schema);
    assert_eq!(receipts(&conn), before_receipts);
    assert_eq!(
        conn.query_row(
            "SELECT body_md FROM workspace_memory_documents WHERE workspace_id='rollback-ws'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "legacy bytes"
    );
    assert!(table_exists(&conn, "workspace_memory_documents").unwrap());
    conn.execute_batch("DROP TRIGGER fail_convergence").unwrap();
    branch_convergence::migrate(&conn).unwrap();
    assert_eq!(current_schema_version(&conn).unwrap(), 145);
    assert_eq!(receipts(&conn).iter().filter(|r| r.0 == 145).count(), 1);
}

#[test]
fn mixed_missing_and_weakened_published_shapes_fail_before_writes() {
    for attack in [
        "CREATE TABLE passkey_addition_codes(user_id TEXT)",
        "ALTER TABLE worker_create_reservations ADD COLUMN initialization_binding TEXT",
    ] {
        let (_dir, conn) = fixture(false, 142);
        conn.execute_batch(attack).unwrap();
        let before = schema(&conn);
        let history = receipts(&conn);
        assert!(apply_migrations(&conn).is_err());
        assert_eq!(schema(&conn), before);
        assert_eq!(receipts(&conn), history);
    }
    let (_dir, conn) = fixture(true, 143);
    conn.execute_batch("DROP TABLE workspace_membership_operations")
        .unwrap();
    assert!(workspace_schema_migration_plan(&conn).is_err());
}

#[test]
fn fabricated_hare_144_baseline_and_swapped_family_receipt_are_rejected() {
    let fresh = SqliteWorkspaceStore::in_memory_for_test().unwrap();
    fresh
        .with_conn(|conn| {
            conn.execute("UPDATE __yoi_schema_migrations SET version=144", [])?;
            conn.execute(
                "INSERT INTO __yoi_schema_migrations(version,name) VALUES(145,?1)",
                [branch_convergence::HARE_NAME],
            )?;
            assert!(workspace_schema_migration_plan(conn).is_err());
            Ok(())
        })
        .unwrap();
    let (_dir, conn) = fixture(true, 143);
    branch_convergence::migrate(&conn).unwrap();
    conn.execute(
        "UPDATE __yoi_schema_migrations SET name=?1 WHERE version=145",
        [branch_convergence::DEVELOP_NAME],
    )
    .unwrap();
    assert!(workspace_schema_migration_plan(&conn).is_err());
}

#[test]
fn shape_equivalent_branch_local_baselines_keep_their_identity_after_convergence() {
    // Construct empty historical shapes through the retained owner, then model
    // their published single-baseline receipt (not an assertion of old receipts).
    for (hare, version) in [
        (true, 142),
        (true, 143),
        (false, 142),
        (false, 143),
        (false, 144),
    ] {
        let (_dir, conn) = fixture(hare, version);
        conn.execute("DELETE FROM __yoi_schema_migrations", [])
            .unwrap();
        conn.execute("INSERT INTO __yoi_schema_migrations(version,name,applied_at) VALUES(?1,?2,'original baseline time')",params![version,SCHEMA_BASELINE_NAME]).unwrap();
        storage_formats::verify_history(&conn, version).unwrap();
        apply_migrations(&conn).unwrap();
        assert!(
            !workspace_schema_migration_plan(&conn)
                .unwrap()
                .migration_required()
        );
        assert_eq!(
            receipts(&conn)[0],
            (
                version,
                SCHEMA_BASELINE_NAME.into(),
                "original baseline time".into()
            )
        );
        let path = conn.path().unwrap().to_owned();
        drop(conn);
        drop(SqliteWorkspaceStore::open_for_test(path).unwrap());
    }
}

#[test]
fn missing_binding_index_and_weakened_passkey_constraint_are_rejected() {
    let (_dir, conn) = fixture(false, 143);
    conn.execute_batch("DROP INDEX worker_create_operation_allocation")
        .unwrap();
    assert!(workspace_schema_migration_plan(&conn).is_err());
    let (_dir, conn) = fixture(true, 142);
    conn.execute_batch("DROP TABLE passkey_addition_codes; CREATE TABLE passkey_addition_codes(user_id TEXT PRIMARY KEY REFERENCES users(user_id) ON DELETE CASCADE,issuance_id TEXT NOT NULL UNIQUE,code_hash TEXT NOT NULL,target_origin TEXT NOT NULL,rp_id TEXT NOT NULL,expires_at TEXT NOT NULL,failed_attempts INTEGER NOT NULL DEFAULT 0);").unwrap();
    assert!(workspace_schema_migration_plan(&conn).is_err());
}

#[test]
fn unknown_retiring_memory_shape_is_rejected_without_data_loss() {
    let (_dir, conn) = fixture(true, 143);
    conn.execute_batch("ALTER TABLE workspace_memory_documents ADD COLUMN unexpected TEXT")
        .unwrap();
    let before = schema(&conn);
    let history = receipts(&conn);
    assert!(apply_migrations(&conn).is_err());
    assert_eq!(schema(&conn), before);
    assert_eq!(receipts(&conn), history);
}

#[test]
fn develop_convergence_failure_rolls_back_missing_branch_and_preserves_current_data() {
    let (_dir, conn) = fixture(false, 144);
    conn.execute_batch("INSERT INTO accounts VALUES('develop-owner','user','develop-owner','Owner','1','1'); INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at) VALUES('develop-ws','develop-owner','Keep','active','1','1'); INSERT INTO subjektiv_memory_settings VALUES('develop-ws','日本語','1','1'); CREATE TRIGGER fail_develop_convergence BEFORE INSERT ON __yoi_schema_migrations WHEN NEW.version=145 BEGIN SELECT RAISE(ABORT,'injected convergence failure'); END;").unwrap();
    let before_schema = schema(&conn);
    let before_receipts = receipts(&conn);
    assert!(branch_convergence::migrate(&conn).is_err());
    assert_eq!(schema(&conn), before_schema);
    assert_eq!(receipts(&conn), before_receipts);
    assert!(!table_exists(&conn, "workspace_members").unwrap());
    assert_eq!(
        conn.query_row(
            "SELECT language FROM subjektiv_memory_settings WHERE workspace_id='develop-ws'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "日本語"
    );
    conn.execute_batch("DROP TRIGGER fail_develop_convergence")
        .unwrap();
    branch_convergence::migrate(&conn).unwrap();
    assert_eq!(current_schema_version(&conn).unwrap(), 145);
    assert!(
        !workspace_schema_migration_plan(&conn)
            .unwrap()
            .migration_required()
    );
}

#[test]
fn pre_141_runner_rejects_unknown_memory_shape_before_retirement() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pre-retirement.db");
    super::super::tests::prepare_retained_schema(&path, 140);
    let conn = Connection::open(&path).unwrap();
    configure_sqlite(&conn).unwrap();
    conn.execute_batch("ALTER TABLE workspace_memory_documents ADD COLUMN unexpected TEXT")
        .unwrap();
    let error = apply_migrations(&conn).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("retiring Workspace Memory input physical schema mismatch"),
        "{error}"
    );
    assert_eq!(current_schema_version(&conn).unwrap(), 143);
    assert!(column_exists(&conn, "workspace_memory_documents", "unexpected").unwrap());
    assert!(
        !column_exists(
            &conn,
            "worker_create_reservations",
            "initialization_binding"
        )
        .unwrap()
    );
    assert!(!receipts(&conn).iter().any(|row| row.0 >= 144));
}
