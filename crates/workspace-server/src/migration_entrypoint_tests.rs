// Public migration entrypoints must preserve the old authority on refusal and
// validate encrypted data using the original database's secret context.
fn seed_migration_workspace(conn: &Connection) {
    conn.execute_batch("INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at)
        VALUES('owner','user','owner','Owner','1','1');
        INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at)
        VALUES('space','owner','Space','active','1','1');").unwrap();
}

#[test]
fn opening_schema_84_with_uncertain_removal_preserves_old_server_recovery() {
    for state in ["executing", "failed"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.db");
        prepare_retained_schema(&path, 84);
        let conn = Connection::open(&path).unwrap();
        configure_sqlite(&conn).unwrap();
        seed_migration_workspace(&conn);
        conn.execute_batch("INSERT INTO workspace_worker_retention_policy_revisions VALUES('space','policy',1,'archive','tombstone','forever',NULL,'retain',3600,'created');
            UPDATE workspace_worker_retention_policies SET policy_id='policy',revision=1,updated_at='updated' WHERE workspace_id='space';").unwrap();
        conn.execute("INSERT INTO worker_removal_operations
            (operation_id,plan_id,input_fingerprint,workspace_id,runtime_id,worker_id,worker_revision,
             policy_id,policy_revision,session_disposition,metadata_disposition,archive_retention_kind,
             archive_retention_seconds,diagnostics_disposition,diagnostics_retention_seconds,archive_id,
             blockers_json,state,reason,failure_category,created_at,updated_at)
            VALUES('removal','plan','legacy-receipt','space','runtime','worker','updated',
             'policy',1,'archive','tombstone','forever',NULL,'retain',3600,'archive',
             '[]',?1,'cleanup','uncertain','created','updated')",[state]).unwrap();
        let schema_before: String = conn.query_row(
            "SELECT group_concat(sql, char(10)) FROM (SELECT sql FROM sqlite_schema ORDER BY type,name)",
            [], |row| row.get(0)).unwrap();
        drop(conn);
        for attempt in 0..3 {
            let error = match attempt {
                0 => SqliteWorkspaceStore::open_for_test(&path).err().unwrap(),
                1 => SqliteWorkspaceStore::migration_plan(&path).unwrap_err(),
                _ => SqliteWorkspaceStore::migrate_database(&path).unwrap_err(),
            };
            assert!(
                error.to_string().contains("receipt reconciliation"),
                "{error}"
            );
            let conn = Connection::open(&path).unwrap();
            assert_eq!(current_schema_version(&conn).unwrap(), 84);
            let schema_after: String = conn.query_row(
                "SELECT group_concat(sql, char(10)) FROM (SELECT sql FROM sqlite_schema ORDER BY type,name)",
                [], |row| row.get(0)).unwrap();
            assert_eq!(schema_after, schema_before);
            let receipt: (String,String,String) = conn.query_row(
                "SELECT input_fingerprint,archive_id,state FROM worker_removal_operations WHERE operation_id='removal'",
                [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
            assert_eq!(
                receipt,
                ("legacy-receipt".into(), "archive".into(), state.into())
            );
        }
        // Simulate the old Server settling its receipt, then upgrade normally.
        let conn = Connection::open(&path).unwrap();
        conn.execute(
            "UPDATE worker_removal_operations SET state='succeeded' WHERE operation_id='removal'",
            [],
        )
        .unwrap();
        drop(conn);
        let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
        store.with_conn(|conn| {
            assert_eq!(current_schema_version(conn)?, LATEST_SCHEMA_VERSION);
            assert_eq!(conn.query_row("SELECT input_fingerprint FROM worker_removal_operations WHERE operation_id='removal'",[],|r|r.get::<_,String>(0))?,"legacy-receipt");
            Ok(())
        }).unwrap();
    }
}

#[test]
fn migration_dry_run_and_apply_share_the_original_repository_secret_authority() {
    use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.db");
    prepare_retained_schema(&path, 84);
    let conn = Connection::open(&path).unwrap();
    configure_sqlite(&conn).unwrap();
    seed_migration_workspace(&conn);
    conn.execute_batch("INSERT INTO repository_secret_operations VALUES('space','create-key','intent','credential','key',7,'2026-01-01T00:00:00.000Z');
        INSERT INTO repository_ssh_credentials VALUES('space','key','Key','ssh-ed25519','fingerprint',7,'active','2026-01-01T00:00:00.000Z',NULL);
        INSERT INTO repository_ssh_credential_revisions VALUES('space','key',7,'ssh-ed25519','fingerprint','2026-01-01T00:00:00.000Z');
        INSERT INTO repository_secret_audit_events VALUES('space','audit','credential_created','key',7,'owner','2026-01-01T00:00:00.000Z');").unwrap();
    let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &[42; 32]).unwrap());
    let mut ciphertext = b"private bytes".to_vec();
    key.seal_in_place_append_tag(
        Nonce::assume_unique_for_key([1; 12]),
        Aad::from(b"yoi/repository-secret/v1/space/key/7/private_key".as_slice()),
        &mut ciphertext,
    )
    .unwrap();
    conn.execute("INSERT INTO server_secret_versions VALUES('space','key',7,'private_key','aes-256-gcm-v1',?1,?2,'2026-01-01T00:00:00.000Z')",params![[1u8;12].as_slice(),ciphertext]).unwrap();
    drop(conn);
    let master_key = dir.path().join("repository-secrets.master-key");
    std::fs::write(&master_key, [42; 32]).unwrap();
    let plan = SqliteWorkspaceStore::migration_plan(&path).unwrap();
    assert_eq!(plan.current_schema_version, 84);
    let conn = Connection::open(&path).unwrap();
    assert_eq!(current_schema_version(&conn).unwrap(), 84);
    let unchanged: Vec<u8> = conn
        .query_row("SELECT ciphertext FROM server_secret_versions", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(unchanged, ciphertext);
    assert!(!table_exists(&conn, "server_secret_objects").unwrap());
    drop(conn);
    assert_eq!(SqliteWorkspaceStore::migrate_database(&path).unwrap(), plan);
    let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    store.with_conn(|conn| {
        assert_eq!(current_schema_version(conn)?,LATEST_SCHEMA_VERSION);
        let (nonce, mut sealed): (Vec<u8>,Vec<u8>) = conn.query_row(
            "SELECT nonce,ciphertext FROM server_secret_objects WHERE workspace_id='space' AND secret_id='key' AND operation_id='create-key'",
            [],|r|Ok((r.get(0)?,r.get(1)?)))?;
        let aad = serde_json::to_string(&("yoi/repository-secret/operation","space","key","create-key","private_key")).unwrap();
        let plaintext = key.open_in_place(Nonce::assume_unique_for_key(nonce.try_into().unwrap()),Aad::from(aad.as_bytes()),&mut sealed).unwrap();
        assert_eq!(plaintext,b"private bytes");
        Ok(())
    }).unwrap();
    assert_eq!(std::fs::read(master_key).unwrap(), [42; 32]);
}

#[test]
fn launch_policy_migration_preserves_adopted_configuration_and_rolls_back() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.db");
    prepare_retained_schema(&path, 130);
    let conn = Connection::open(&path).unwrap();
    configure_sqlite(&conn).unwrap();
    seed_migration_workspace(&conn);
    let policy = crate::policy_config::AdoptedSubjectPolicy {
        reference_encoding: server_api::authorization::PermissionReferenceEncoding::FeatureSetsV2,
        source: crate::policy_config::SubjectAssignmentSource {
            subject_id: "subject".into(),
            execution: Default::default(),
            delegation: Default::default(),
        },
        change_id: "policy-change".into(),
        legacy_constraints: None,
    };
    conn.execute_batch(
        "INSERT INTO workspace_configuration_receipts
            VALUES('space','adopt','fingerprint','{}');",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO workspace_configuration_policies VALUES('space','subject',?1)",
        [serde_json::to_string(&policy).unwrap()],
    )
    .unwrap();
    let adoption_records = |conn: &Connection| -> Result<_> {
        Ok(conn.query_row(
            "SELECT policy_json,request_fingerprint,result_json
             FROM workspace_configuration_policies
             JOIN workspace_configuration_receipts USING(workspace_id)
             WHERE workspace_id='space' AND subject_id='subject' AND operation_id='adopt'",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )?)
    };
    let before = adoption_records(&conn).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER interrupt_launch_policy_migration
         BEFORE INSERT ON __yoi_schema_migrations WHEN NEW.version=131
         BEGIN SELECT RAISE(ABORT,'injected launch policy migration interruption'); END;",
    )
    .unwrap();
    let error = migrate_worker_launch_policies_to_v131(&conn).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("injected launch policy migration interruption"),
        "{error}"
    );
    assert_eq!(current_schema_version(&conn).unwrap(), 130);
    for table in [
        "worker_launch_policies",
        "worker_launch_policy_history",
        "worker_launch_policy_operations",
        "core_worker_execution_grants",
        "core_worker_grant_revocations",
        "core_worker_launch_bindings",
    ] {
        assert!(!table_exists(&conn, table).unwrap(), "{table}");
    }
    assert_eq!(adoption_records(&conn).unwrap(), before);
    conn.execute_batch("DROP TRIGGER interrupt_launch_policy_migration")
        .unwrap();
    drop(conn);

    let plan = SqliteWorkspaceStore::migration_plan(&path).unwrap();
    assert_eq!(plan.current_schema_version, 130);
    assert!(plan.migration_required());
    let conn = Connection::open(&path).unwrap();
    assert_eq!(current_schema_version(&conn).unwrap(), 130);
    assert!(!table_exists(&conn, "worker_launch_policies").unwrap());
    assert_eq!(adoption_records(&conn).unwrap(), before);
    drop(conn);

    assert_eq!(SqliteWorkspaceStore::migrate_database(&path).unwrap(), plan);
    let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    assert_eq!(store.with_conn(adoption_records).unwrap(), before);
    store
        .with_conn(|conn| {
            assert!(
                conn.execute("DELETE FROM workspace_configuration_receipts", [])
                    .is_err()
            );
            assert_eq!(
                conn.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
                    row.get::<_, i64>(0)
                })?,
                0
            );
            Ok(())
        })
        .unwrap();
    let schema = |conn: &Connection| -> Result<Vec<(String, String)>> {
        let mut statement = conn.prepare(
            "SELECT name,sql FROM sqlite_schema WHERE sql IS NOT NULL AND tbl_name IN
             ('worker_launch_policies','worker_launch_policy_history',
              'worker_launch_policy_operations','core_worker_execution_grants',
              'core_worker_grant_revocations','core_worker_launch_bindings') ORDER BY name",
        )?;
        Ok(statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    };
    let fresh = SqliteWorkspaceStore::in_memory_for_test().unwrap();
    let expected = fresh.with_conn(schema).unwrap();
    assert!(!expected.is_empty());
    assert_eq!(store.with_conn(schema).unwrap(), expected);
}
