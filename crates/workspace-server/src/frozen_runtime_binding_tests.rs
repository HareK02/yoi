use super::*;

// Frozen schema-87 tables: use only the legacy authorities at this cutover.
fn legacy_binding_database() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON;
        CREATE TABLE __yoi_schema_migrations(version INTEGER PRIMARY KEY,name TEXT NOT NULL);
        INSERT INTO __yoi_schema_migrations VALUES(87,'workspace schema baseline');
        CREATE TABLE accounts(account_id TEXT PRIMARY KEY);
        INSERT INTO accounts VALUES('owner');
        CREATE TABLE workspaces(workspace_id TEXT PRIMARY KEY);
        INSERT INTO workspaces VALUES('space');
        CREATE TABLE workspace_config_trees(workspace_id TEXT PRIMARY KEY,content_digest TEXT NOT NULL);
        INSERT INTO workspace_config_trees VALUES('space','sha256:config-content');
        CREATE TABLE workspace_config_tree_history(workspace_id TEXT,content_digest TEXT,toolchain_fingerprint TEXT,projection_digest TEXT,manifest_json TEXT,created_at TEXT,schema_bundle_json TEXT,PRIMARY KEY(workspace_id,content_digest));
        INSERT INTO workspace_config_tree_history VALUES('space','sha256:config-content','toolchain-original','projection-original','{}','created','{}');").unwrap();
    let frozen = include_str!("frozen_schema_v85.sql");
    for table in [
        "workspace_runtime_bindings",
        "workspace_runtime_verifications",
        "workspace_runtime_binding_audit",
        "workspace_signing_identities",
        "workspace_signing_identity_provisioning_operations",
        "workspace_signing_identity_audit",
        "runtime_removal_operations",
        "workspace_worker_retention_policies",
        "workspace_worker_retention_policy_revisions",
        "worker_removal_operations",
        "worker_session_archives",
        "worker_diagnostics_archives",
        "worker_tombstones",
        "workdir_removal_operations",
        "workspace_deletion_operations",
    ] {
        let marker = format!("CREATE TABLE {table} (");
        let start = frozen.find(&marker).unwrap();
        let end = start + frozen[start..].find(");").unwrap() + 2;
        conn.execute_batch(&frozen[start..end]).unwrap();
    }
    for index in [
        "runtime_removal_operations_one_active_runtime",
        "runtime_removal_operations_workspace_state",
        "idx_workdir_removal_operations_recovery",
        "idx_workdir_removal_operations_workdir",
        "workspace_signing_identity_audit_workspace_idx",
    ] {
        let start = frozen
            .find(&format!("CREATE INDEX {index}"))
            .or_else(|| frozen.find(&format!("CREATE UNIQUE INDEX {index}")))
            .unwrap();
        let end = start + frozen[start..].find(';').unwrap() + 1;
        conn.execute_batch(&frozen[start..end]).unwrap();
    }
    conn.execute_batch("INSERT INTO workspace_signing_identities(workspace_id,key_id,algorithm,public_key,public_key_fingerprint,private_material_ref,revision,state,created_at,provisioned_at,updated_at)
        VALUES('space','workspace-key','ed25519','public','sha256:workspace','material',1,'active','created','provisioned','updated');
        INSERT INTO workspace_runtime_bindings(workspace_id,runtime_id,display_name,base_url,public_key,public_key_fingerprint,binding_revision,state,authentication_mode,workspace_key_id,workspace_key_generation,created_at,updated_at)
        VALUES('space','remote','Remote','https://remote.test','public','sha256:runtime',8,'verified','workspace_identity','workspace-key',6,'created','updated');
        INSERT INTO workspace_runtime_binding_audit VALUES('space','remote','owner','created',NULL,'sha256:runtime',8,'created');
        INSERT INTO workspace_runtime_verifications VALUES('space','remote',8,'workspace-key',1,6,'sha256:runtime',2,'challenge','verified','verified','verified-at','checked-at');
        INSERT INTO workspace_worker_retention_policy_revisions VALUES('space','policy',1,'archive','tombstone','forever',NULL,'purge',NULL,'first');
        INSERT INTO workspace_worker_retention_policy_revisions VALUES('space','policy',9,'archive','tombstone','forever',NULL,'purge',NULL,'ninth');
        INSERT INTO workspace_worker_retention_policies VALUES('space','policy',9,'updated');
        INSERT INTO runtime_removal_operations VALUES('removal','space','remote','request',8,4,'pending',NULL,0,NULL,'created','updated',NULL);
        CREATE TRIGGER runtime_binding_cutover_guard BEFORE UPDATE ON workspace_runtime_bindings
        WHEN EXISTS(SELECT 1 FROM runtime_removal_operations WHERE runtime_id=NEW.runtime_id AND state IN ('pending','cleanup_pending'))
        BEGIN SELECT RAISE(ABORT,'runtime_removal_in_progress'); END;").unwrap();
    conn
}

#[test]
fn binding_cutover_preserves_key_facts_and_indices_but_requires_runtime_issued_trust() {
    let conn = legacy_binding_database();
    migrate_runtime_bindings_v87_to_v88(&conn).unwrap();
    assert_eq!(current_schema_version(&conn).unwrap(), 88);
    let (id,state,trust,key,fingerprint): (String,String,Option<String>,String,String) = conn.query_row(
        "SELECT binding_id,state,workspace_trust_id,workspace_key_id,workspace_public_key_fingerprint FROM workspace_runtime_bindings", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).unwrap();
    assert!(id.starts_with("RB-"));
    assert_eq!(state, "configured");
    assert_eq!(trust, None);
    assert_eq!(key, "workspace-key");
    assert_eq!(fingerprint, "sha256:workspace");
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM workspace_runtime_verifications",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    let (expected, state, digest): (String, String, String) = conn
        .query_row(
            "SELECT expected_binding_id,state,config_digest FROM runtime_removal_operations",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(expected, id);
    assert_eq!(state, "failed");
    assert_eq!(digest, "sha256:config-content");
    let audit: String = conn
        .query_row(
            "SELECT binding_id FROM workspace_runtime_binding_audit",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(audit, id);
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM workspace_worker_retention_policy_snapshots",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    let update = crate::retention::WorkerRetentionPolicyUpdate {
        policy_id: "policy".into(),
        session_disposition: worker_runtime::retention::SessionDisposition::Archive,
        metadata_disposition: crate::retention::MetadataDisposition::Tombstone,
        archive_retention: crate::retention::ArchiveRetention::Forever,
        diagnostics_disposition: worker_runtime::retention::DiagnosticsDisposition::Purge,
        diagnostics_retention_seconds: None,
    };
    let digest: String = conn
        .query_row(
            "SELECT policy_digest FROM workspace_worker_retention_policies",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        digest,
        crate::retention::worker_retention_policy_digest(&update)
    );
    for index in [
        "runtime_removal_operations_one_active_runtime",
        "runtime_removal_operations_workspace_state",
        "idx_workdir_removal_operations_recovery",
        "idx_workdir_removal_operations_workdir",
        "workspace_signing_identity_audit_workspace_idx",
    ] {
        assert!(
            conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='index' AND name=?1)",
                [index],
                |r| r.get::<_, bool>(0)
            )
            .unwrap(),
            "missing index {index}"
        );
    }
    conn.execute(
        "UPDATE runtime_removal_operations SET state='cleanup_pending'",
        [],
    )
    .unwrap();
    assert!(conn.execute("INSERT INTO runtime_removal_operations SELECT 'other',workspace_id,runtime_id,request_fingerprint,expected_binding_id,config_digest,'pending',NULL,0,NULL,created_at,updated_at,NULL FROM runtime_removal_operations", []).is_err());
    assert!(
        conn.execute(
            "UPDATE workspace_runtime_bindings SET display_name='Blocked'",
            []
        )
        .is_err()
    );
    assert_eq!(
        conn.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| r
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        0
    );
}

#[test]
fn same_config_content_keeps_distinct_toolchain_and_projection_history_after_cutover() {
    let conn = legacy_binding_database();
    migrate_runtime_bindings_v87_to_v88(&conn).unwrap();
    conn.execute("INSERT INTO workspace_config_tree_history VALUES('space','sha256:config-content','toolchain-new','projection-new','{}','later','{}')", []).unwrap();
    assert_eq!(conn.query_row("SELECT COUNT(*) FROM workspace_config_tree_history WHERE content_digest='sha256:config-content'", [], |r| r.get::<_, i64>(0)).unwrap(), 2);
    let original: String = conn.query_row("SELECT projection_digest FROM workspace_config_tree_history WHERE toolchain_fingerprint='toolchain-original'", [], |r|r.get(0)).unwrap();
    assert_eq!(original, "projection-original");
}

#[test]
fn invalid_legacy_signing_or_policy_reference_rolls_back_all_cutover_changes() {
    for invalid in [
        "UPDATE workspace_signing_identities SET revision=2",
        "PRAGMA foreign_keys=OFF; DELETE FROM workspace_worker_retention_policy_revisions WHERE revision=9; PRAGMA foreign_keys=ON;",
        "DELETE FROM workspace_config_trees",
    ] {
        let conn = legacy_binding_database();
        conn.execute_batch(invalid).unwrap();
        assert!(
            migrate_runtime_bindings_v87_to_v88(&conn).is_err(),
            "accepted {invalid}"
        );
        assert_eq!(current_schema_version(&conn).unwrap(), 87);
        assert!(column_exists(&conn, "workspace_runtime_bindings", "binding_revision").unwrap());
        assert!(!column_exists(&conn, "workspace_runtime_bindings", "binding_id").unwrap());
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM workspace_runtime_verifications",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}

fn seed_legacy_worker_removal(conn: &Connection, state: &str) {
    conn.execute(
        "INSERT INTO worker_removal_operations \
         (operation_id,plan_id,input_fingerprint,workspace_id,runtime_id,worker_id,worker_revision,\
          policy_id,policy_revision,session_disposition,metadata_disposition,archive_retention_kind,\
          archive_retention_seconds,diagnostics_disposition,diagnostics_retention_seconds,archive_id,\
          blockers_json,state,reason,failure_category,created_at,updated_at) \
         VALUES('worker-removal','worker-plan','legacy-receipt-fingerprint','space','remote','worker',\
          'worker-updated','policy',9,'archive','tombstone','forever',NULL,'retain',3600,\
          'archive','[]',?1,'cleanup','legacy-outcome','created','updated')",
        [state],
    ).unwrap();
}

#[test]
fn unresolved_worker_removal_requires_old_version_reconciliation_without_losing_receipt_authority()
{
    for state in ["executing", "failed"] {
        let conn = legacy_binding_database();
        seed_legacy_worker_removal(&conn, state);
        let schema = || {
            conn.prepare("SELECT name,sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY name")
                .unwrap()
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        let schema_before = schema();
        let message = migrate_runtime_bindings_v87_to_v88(&conn)
            .unwrap_err()
            .to_string();
        assert!(message.contains("old-version reconciliation"), "{message}");
        assert!(message.contains("worker-removal"), "{message}");
        assert!(message.contains("schema 87"), "{message}");
        assert_eq!(current_schema_version(&conn).unwrap(), 87);
        let evidence: (String, String, String, i64, String, String, String, String) = conn.query_row(
            "SELECT state,input_fingerprint,worker_revision,policy_revision,archive_id,failure_category,created_at,updated_at FROM worker_removal_operations", [],
            |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?))).unwrap();
        assert_eq!(
            evidence,
            (
                state.into(),
                "legacy-receipt-fingerprint".into(),
                "worker-updated".into(),
                9,
                "archive".into(),
                "legacy-outcome".into(),
                "created".into(),
                "updated".into()
            )
        );
        assert_eq!(schema(), schema_before);
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM workspace_runtime_verifications",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row(
                "SELECT binding_revision FROM workspace_runtime_bindings",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            8
        );
        assert!(
            conn.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, bool>(0))
                .unwrap()
        );
        assert!(
            !conn
                .query_row("PRAGMA legacy_alter_table", [], |r| r.get::<_, bool>(0))
                .unwrap()
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM sqlite_temp_schema", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );

        // Simulate the old Server committing its recovered receipt. The same
        // connection can upgrade without dropping or reissuing the operation.
        conn.execute(
            "UPDATE worker_removal_operations SET state='succeeded',failure_category=NULL",
            [],
        )
        .unwrap();
        migrate_runtime_bindings_v87_to_v88(&conn).unwrap();
        assert_eq!(current_schema_version(&conn).unwrap(), 88);
        assert_eq!(
            conn.query_row(
                "SELECT input_fingerprint FROM worker_removal_operations",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "legacy-receipt-fingerprint"
        );
    }
}

#[test]
fn unexecuted_and_terminal_worker_removal_history_migrates_without_new_execution_authority() {
    for state in ["planned", "blocked", "stale", "succeeded"] {
        let conn = legacy_binding_database();
        seed_legacy_worker_removal(&conn, state);
        if state == "succeeded" {
            conn.execute_batch("INSERT INTO worker_session_archives VALUES('archive','space','remote','worker','session','checksum',42,'policy',9,'worker-removal','committed',NULL);
                INSERT INTO worker_diagnostics_archives VALUES('worker-removal','space','remote','worker','policy',9,'committed','expires');
                INSERT INTO worker_tombstones VALUES('space','remote','worker','Worker','profile','worker-created','removed','archive','policy',9,'worker-removal');").unwrap();
        }
        migrate_runtime_bindings_v87_to_v88(&conn).unwrap();
        assert_eq!(current_schema_version(&conn).unwrap(), 88);
        let (actual_state, category, fingerprint, digest): (String, Option<String>, String, String) = conn.query_row(
            "SELECT state,failure_category,input_fingerprint,policy_digest FROM worker_removal_operations", [],
            |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
        let unexecuted = matches!(state, "planned" | "blocked");
        assert_eq!(actual_state, if unexecuted { "stale" } else { state });
        assert_eq!(
            category.as_deref(),
            Some(if unexecuted {
                "policy_content_cutover"
            } else {
                "legacy-outcome"
            })
        );
        assert_eq!(fingerprint, "legacy-receipt-fingerprint");
        assert_eq!(
            conn.query_row(
                "SELECT policy_digest FROM workspace_worker_retention_policies",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            digest
        );
        if state == "succeeded" {
            for table in [
                "worker_session_archives",
                "worker_diagnostics_archives",
                "worker_tombstones",
            ] {
                let (operation, policy): (String, String) = conn
                    .query_row(
                        &format!("SELECT operation_id,policy_digest FROM {table}"),
                        [],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .unwrap();
                assert_eq!(operation, "worker-removal");
                assert_eq!(policy, digest);
            }
            assert_eq!(
                conn.query_row(
                    "SELECT checksum_sha256,content_bytes FROM worker_session_archives",
                    [],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
                )
                .unwrap(),
                ("checksum".into(), 42)
            );
        }
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| r
                .get::<_, i64>(
                0
            ))
            .unwrap(),
            0
        );
    }
}

#[test]
fn workdir_removal_cutover_preserves_recovery_state_owner_and_materialization_evidence() {
    for (state, attempts, retryable, owner) in [
        ("pending", 0, true, None),
        ("pending", 2, true, Some((1234_i64, 5678_i64))),
        ("failed", 2, true, None),
        ("failed", 2, false, None),
        ("completed", 2, false, None),
    ] {
        let conn = legacy_binding_database();
        conn.execute("INSERT INTO workdir_removal_operations VALUES('space','workdir-removal','workdir-request','workdir','remote','repository','materialization','owner','cleanup',?1,?2,?3,?4,?5,?6,?7,'created','updated',?8)",
            params![state,attempts,retryable,if state == "completed" { Some("removed") } else { None },
                if state == "failed" { Some("provider_cleanup_outcome_unknown") } else { None },
                owner.map(|o|o.0),owner.map(|o|o.1),if state == "completed" { Some("completed") } else { None }]).unwrap();
        migrate_runtime_bindings_v87_to_v88(&conn).unwrap();
        assert_eq!(current_schema_version(&conn).unwrap(), 88);
        let (actual_state,attempt,actual_retryable,pid,marker,request,materialization):
            (String,Option<String>,bool,Option<i64>,Option<i64>,String,String) = conn.query_row(
            "SELECT state,attempt_id,retryable,attempt_owner_pid,attempt_owner_start_marker,request_fingerprint,materialization_fingerprint FROM workdir_removal_operations", [],
            |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).unwrap();
        assert_eq!(actual_state, state);
        assert_eq!(attempt.is_some(), attempts > 0);
        assert_eq!(actual_retryable, retryable);
        assert_eq!(pid.zip(marker), owner);
        assert_eq!(request, "workdir-request");
        assert_eq!(materialization, "materialization");
        let (category, disposition): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT failure_category,disposition FROM workdir_removal_operations",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            category.as_deref(),
            if state == "failed" {
                Some("provider_cleanup_outcome_unknown")
            } else {
                None
            }
        );
        assert_eq!(
            disposition.as_deref(),
            if state == "completed" {
                Some("removed")
            } else {
                None
            }
        );
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM workdir_removal_operations WHERE state='pending' OR (state='failed' AND retryable=1)", [], |r| r.get::<_, i64>(0)).unwrap(), i64::from(state == "pending" || (state == "failed" && retryable)));
    }
}

#[test]
fn runtime_removal_cutover_keeps_post_binding_commit_cleanup_resumable() {
    for state in ["cleanup_pending", "succeeded"] {
        let conn = legacy_binding_database();
        conn.execute("UPDATE runtime_removal_operations SET state=?1,binding_removed=1,runtime_registration_removed=?2,completed_at=?3",
            params![state,if state == "succeeded" { Some(1) } else { None },if state == "succeeded" { Some("completed") } else { None }]).unwrap();
        conn.execute_batch(
            "DELETE FROM workspace_runtime_binding_audit; DELETE FROM workspace_runtime_bindings;",
        )
        .unwrap();
        migrate_runtime_bindings_v87_to_v88(&conn).unwrap();
        assert_eq!(current_schema_version(&conn).unwrap(), 88);
        let (actual_state,binding_removed,registration_removed,completed,fingerprint): (String,bool,Option<bool>,Option<String>,String) = conn.query_row(
            "SELECT state,binding_removed,runtime_registration_removed,completed_at,request_fingerprint FROM runtime_removal_operations", [],
            |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).unwrap();
        assert_eq!(actual_state, state);
        assert!(binding_removed);
        assert_eq!(
            registration_removed,
            if state == "succeeded" {
                Some(true)
            } else {
                None
            }
        );
        assert_eq!(
            completed.as_deref(),
            if state == "succeeded" {
                Some("completed")
            } else {
                None
            }
        );
        assert_eq!(fingerprint, "request");
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM runtime_removal_operations WHERE state IN ('pending','cleanup_pending')", [], |r| r.get::<_, i64>(0)).unwrap(), i64::from(state == "cleanup_pending"));
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM workspace_runtime_bindings", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}

mod existing_workspace_provisioning {
    use super::*;
    use crate::workspace_signing_identity::{
        FsWorkspaceSigningMaterialStore, WorkspaceSigningIdentityService,
        WorkspaceSigningMaterialStore, WorkspaceSigningPrivateMaterial,
    };

    const OLD_OPERATION_KEY: &str = "existing-workspace:space:revision-1";
    // Frozen v1 fingerprint: SHA256(workspace || NUL || key || NUL || 1u64_be).
    const OLD_FINGERPRINT: &str =
        "sha256:913cd2ceeefdd81fcaf0363b0f5db66d39a0b9040440ca564892f3e0c93a9edd";
    const NEW_OPERATION_KEY: &str = "existing-workspace:space:key-workspace-key";

    fn legacy_reserved_database() -> Connection {
        let conn = legacy_binding_database();
        conn.execute_batch("UPDATE workspace_signing_identities SET state='pending_provisioning',public_key=NULL,public_key_fingerprint=NULL,provisioned_at=NULL;").unwrap();
        conn.execute("INSERT INTO workspace_signing_identity_provisioning_operations VALUES(?1,?2,'existing_workspace','space','workspace-key','material',1,'owner','pending','reserved',NULL)",
            params![OLD_OPERATION_KEY, OLD_FINGERPRINT]).unwrap();
        conn
    }

    fn reopen_store(conn: Connection, path: &Path) -> Arc<SqliteWorkspaceStore> {
        let mut persisted = Connection::open(path).unwrap();
        Backup::new(&conn, &mut persisted)
            .unwrap()
            .run_to_completion(128, Duration::from_millis(1), None)
            .unwrap();
        drop(persisted);
        drop(conn);
        let conn = Connection::open(path).unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON").unwrap();
        // This fixture contains only the schema-87 cutover authorities, not the
        // unrelated full Server schema. Exercise the real service/store methods.
        Arc::new(SqliteWorkspaceStore {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    fn install_frozen_material(root: &Path) -> String {
        let material = WorkspaceSigningPrivateMaterial::generate("space", "workspace-key").unwrap();
        let public_key = material
            .validate_and_public_key("space", "workspace-key")
            .unwrap();
        let mut old = serde_json::to_value(material).unwrap();
        old["version"] = 1.into();
        old["revision"] = 1.into();
        std::fs::create_dir_all(root).unwrap();
        std::fs::write(
            root.join("material.json"),
            serde_json::to_vec(&old).unwrap(),
        )
        .unwrap();
        public_key
    }

    fn assert_receipt(store: &SqliteWorkspaceStore, state: &str) {
        store.with_conn(|conn| {
            let receipt: (String,String,String,String,String,String,String,String) = conn.query_row(
                "SELECT operation_key,operation_kind,workspace_id,key_id,private_material_ref,actor_account_id,state,created_at FROM workspace_signing_identity_provisioning_operations", [],
                |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?)))?;
            assert_eq!(receipt, (NEW_OPERATION_KEY.into(),"existing_workspace".into(),"space".into(),"workspace-key".into(),"material".into(),"owner".into(),state.into(),"reserved".into()));
            assert_eq!(conn.query_row("SELECT COUNT(*) FROM workspace_signing_identity_provisioning_operations", [], |r| r.get::<_, i64>(0))?, 1);
            Ok(())
        }).unwrap();
    }

    #[test]
    fn legacy_reserved_workspace_resumes_same_key_and_receipt_after_cutover_and_restart() {
        for material_was_published in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("identities");
            let old_public_key = material_was_published.then(|| install_frozen_material(&root));
            let old_bytes =
                material_was_published.then(|| std::fs::read(root.join("material.json")).unwrap());
            let conn = legacy_reserved_database();
            migrate_runtime_bindings_v87_to_v88(&conn).unwrap();
            assert_eq!(current_schema_version(&conn).unwrap(), 88);
            let store = reopen_store(conn, &temp.path().join("server.db"));
            let materials = Arc::new(FsWorkspaceSigningMaterialStore::new(root.clone()));
            let service = WorkspaceSigningIdentityService::new(store.clone(), materials.clone());
            // A recovery caller need not be the original actor: the reserved
            // actor remains the authority attributed to the activation audit.
            let recovered = service
                .provision_existing("space", "recovery-caller")
                .unwrap();
            assert_eq!(recovered.key_id, "workspace-key");
            assert_eq!(recovered.private_material_ref, "material");
            assert_eq!(recovered.state, "active");
            if let Some(public_key) = old_public_key {
                assert_eq!(recovered.public_key.as_deref(), Some(public_key.as_str()));
                assert_eq!(
                    std::fs::read(root.join("material.json")).unwrap(),
                    old_bytes.unwrap()
                );
            }
            assert_receipt(&store, "completed");
            assert_eq!(
                service
                    .provision_existing("space", "recovery-caller")
                    .unwrap(),
                recovered
            );
            store.with_conn(|conn| {
                assert_eq!(conn.query_row("SELECT COUNT(*) FROM workspace_signing_identity_audit WHERE actor_account_id='owner' AND key_id='workspace-key'", [], |r|r.get::<_, i64>(0))?, 1);
                assert!(conn.query_row("SELECT completed_at FROM workspace_signing_identity_provisioning_operations", [], |r|r.get::<_, Option<String>>(0))?.is_some());
                Ok(())
            }).unwrap();
            assert!(materials.load("material").unwrap().is_some());
        }
    }

    #[test]
    fn legacy_reserved_workspace_activation_failure_rolls_back_and_retries_same_material() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("identities");
        let old_public_key = install_frozen_material(&root);
        let old_bytes = std::fs::read(root.join("material.json")).unwrap();
        let conn = legacy_reserved_database();
        migrate_runtime_bindings_v87_to_v88(&conn).unwrap();
        conn.execute_batch("CREATE TRIGGER fail_provisioning_audit BEFORE INSERT ON workspace_signing_identity_audit BEGIN SELECT RAISE(ABORT,'injected audit failure'); END;").unwrap();
        let path = temp.path().join("server.db");
        let store = reopen_store(conn, &path);
        let service = WorkspaceSigningIdentityService::new(
            store.clone(),
            Arc::new(FsWorkspaceSigningMaterialStore::new(root.clone())),
        );
        assert!(service.provision_existing("space", "owner").is_err());
        assert_receipt(&store, "pending");
        assert_eq!(
            store
                .get_workspace_signing_identity("space")
                .unwrap()
                .unwrap()
                .state,
            "pending_provisioning"
        );
        store.with_conn(|conn| {
            assert_eq!(conn.query_row("SELECT COUNT(*) FROM workspace_signing_identity_audit", [], |r|r.get::<_, i64>(0))?, 0);
            assert_eq!(conn.query_row("SELECT completed_at FROM workspace_signing_identity_provisioning_operations", [], |r|r.get::<_, Option<String>>(0))?, None);
            conn.execute_batch("DROP TRIGGER fail_provisioning_audit")?;
            Ok(())
        }).unwrap();
        drop(service);
        drop(store);
        let store = Arc::new(SqliteWorkspaceStore {
            conn: Arc::new(Mutex::new(Connection::open(&path).unwrap())),
        });
        let service = WorkspaceSigningIdentityService::new(
            store.clone(),
            Arc::new(FsWorkspaceSigningMaterialStore::new(root.clone())),
        );
        let recovered = service.provision_existing("space", "owner").unwrap();
        assert_eq!(
            recovered.public_key.as_deref(),
            Some(old_public_key.as_str())
        );
        assert_eq!(recovered.key_id, "workspace-key");
        assert_receipt(&store, "completed");
        assert_eq!(
            std::fs::read(root.join("material.json")).unwrap(),
            old_bytes
        );
    }

    #[test]
    fn completed_existing_receipt_keeps_completion_evidence_and_workspace_create_contract_stays_opaque()
     {
        let conn = legacy_reserved_database();
        conn.execute_batch("UPDATE workspace_signing_identities SET state='active',public_key='public',public_key_fingerprint='sha256:workspace',provisioned_at='provisioned';
            UPDATE workspace_signing_identity_provisioning_operations SET state='completed',completed_at='completed';
            INSERT INTO workspace_signing_identity_provisioning_operations VALUES('workspace-create:request','create-request-fingerprint','workspace_create','new-space','new-key','new-material',1,'owner','pending','create-reserved',NULL);").unwrap();
        migrate_runtime_bindings_v87_to_v88(&conn).unwrap();
        assert_eq!(current_schema_version(&conn).unwrap(), 88);
        let completed: (String,String,String,String,String,String,String) = conn.query_row(
            "SELECT operation_key,key_id,private_material_ref,actor_account_id,state,created_at,completed_at FROM workspace_signing_identity_provisioning_operations WHERE operation_kind='existing_workspace'", [],
            |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).unwrap();
        assert_eq!(
            completed,
            (
                NEW_OPERATION_KEY.into(),
                "workspace-key".into(),
                "material".into(),
                "owner".into(),
                "completed".into(),
                "reserved".into(),
                "completed".into()
            )
        );
        let create: (String,String,String,String,String,String,String,Option<String>) = conn.query_row(
            "SELECT operation_key,request_fingerprint,workspace_id,key_id,private_material_ref,state,created_at,completed_at FROM workspace_signing_identity_provisioning_operations WHERE operation_kind='workspace_create'", [],
            |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?))).unwrap();
        assert_eq!(
            create,
            (
                "workspace-create:request".into(),
                "create-request-fingerprint".into(),
                "new-space".into(),
                "new-key".into(),
                "new-material".into(),
                "pending".into(),
                "create-reserved".into(),
                None
            )
        );
    }

    #[test]
    fn late_cutover_failure_keeps_old_reservation_contract_for_upgrade_retry() {
        let conn = legacy_reserved_database();
        // The missing config fails the runtime-removal copy after the signing
        // reservation has been converted inside the cutover transaction.
        conn.execute("DELETE FROM workspace_config_trees", [])
            .unwrap();
        assert!(migrate_runtime_bindings_v87_to_v88(&conn).is_err());
        assert_eq!(current_schema_version(&conn).unwrap(), 87);
        let receipt: (String,String,String,String) = conn.query_row(
            "SELECT operation_key,request_fingerprint,key_id,private_material_ref FROM workspace_signing_identity_provisioning_operations", [],
            |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
        assert_eq!(
            receipt,
            (
                OLD_OPERATION_KEY.into(),
                OLD_FINGERPRINT.into(),
                "workspace-key".into(),
                "material".into()
            )
        );
        assert!(
            column_exists(
                &conn,
                "workspace_signing_identity_provisioning_operations",
                "revision"
            )
            .unwrap()
        );
        conn.execute(
            "INSERT INTO workspace_config_trees VALUES('space','sha256:config-content')",
            [],
        )
        .unwrap();
        migrate_runtime_bindings_v87_to_v88(&conn).unwrap();
        assert_eq!(current_schema_version(&conn).unwrap(), 88);
        assert_eq!(
            conn.query_row(
                "SELECT operation_key FROM workspace_signing_identity_provisioning_operations",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            NEW_OPERATION_KEY
        );
    }

    #[test]
    fn mismatched_legacy_provisioning_receipt_rolls_back_cutover_without_replacing_reservation() {
        for invalid in [
            "UPDATE workspace_signing_identity_provisioning_operations SET key_id='different-key'",
            "UPDATE workspace_signing_identity_provisioning_operations SET private_material_ref='different-material'",
            "UPDATE workspace_signing_identity_provisioning_operations SET workspace_id='different-workspace'",
            "UPDATE workspace_signing_identity_provisioning_operations SET operation_key='different-operation'",
            "UPDATE workspace_signing_identity_provisioning_operations SET request_fingerprint='different-input'",
            "UPDATE workspace_signing_identity_provisioning_operations SET state='completed',completed_at='completed'",
        ] {
            let conn = legacy_reserved_database();
            conn.execute_batch(invalid).unwrap();
            let before = || {
                conn.prepare("SELECT operation_key,request_fingerprint,workspace_id,key_id,private_material_ref,state,completed_at FROM workspace_signing_identity_provisioning_operations").unwrap()
                .query_row([], |r| Ok((r.get::<_, String>(0)?,r.get::<_, String>(1)?,r.get::<_, String>(2)?,r.get::<_, String>(3)?,r.get::<_, String>(4)?,r.get::<_, String>(5)?,r.get::<_, Option<String>>(6)?))).unwrap()
            };
            let receipt = before();
            let error = migrate_runtime_bindings_v87_to_v88(&conn)
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("legacy Workspace signing identity provisioning"),
                "{invalid}: {error}"
            );
            assert_eq!(current_schema_version(&conn).unwrap(), 87);
            assert_eq!(before(), receipt);
            assert!(
                column_exists(
                    &conn,
                    "workspace_signing_identity_provisioning_operations",
                    "revision"
                )
                .unwrap()
            );
            assert!(
                column_exists(&conn, "workspace_runtime_bindings", "binding_revision").unwrap()
            );
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM workspace_runtime_verifications",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                1
            );
            assert_eq!(
                conn.query_row("SELECT COUNT(*) FROM sqlite_temp_schema", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                0
            );
            assert!(
                conn.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, bool>(0))
                    .unwrap()
            );
        }
    }
}

mod current_authority {
    use super::*;

    fn binding_store() -> (SqliteWorkspaceStore, WorkspaceRuntimeBinding) {
        let store = SqliteWorkspaceStore::in_memory().unwrap();
        let runtime_key =
            worker_runtime::auth::RuntimeIdentityMaterial::generate("runtime-key").unwrap();
        store.with_conn(|conn| {
            conn.execute_batch("INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at) VALUES('owner','user','owner','Owner','created','updated');
                INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at) VALUES('space','owner','Space','active','created','updated');
                INSERT INTO workspace_signing_identities(workspace_id,key_id,algorithm,public_key,public_key_fingerprint,private_material_ref,state,created_at,provisioned_at,updated_at) VALUES('space','workspace-key','ed25519','public','sha256:workspace','material','active','created','provisioned','updated');")?;
            Ok(())
        }).unwrap();
        let request = WorkspaceRuntimeBinding {
            workspace_id: "space".into(),
            runtime_id: "remote".into(),
            display_name: "Remote".into(),
            base_url: "https://remote.test".into(),
            public_key: runtime_key.public_key,
            public_key_fingerprint: String::new(),
            binding_id: String::new(),
            authentication_mode: WorkspaceRuntimeAuthenticationMode::WorkspaceIdentity,
            created_at: "created".into(),
            updated_at: "updated".into(),
            revoked_at: None,
        };
        let (_, binding) = store
            .put_workspace_runtime_binding_key(request, None, "owner")
            .unwrap();
        (store, binding)
    }

    #[test]
    fn same_key_reactivation_fences_old_config_and_admin_mutations() {
        let (store, binding) = binding_store();
        assert!(store.workspace_runtime_binding_matches(&binding).unwrap());
        let (_, revoked) = store
            .revoke_workspace_runtime_binding_key(
                "space",
                "remote",
                &binding.binding_id,
                "owner",
                "revoked-at",
            )
            .unwrap();
        assert_ne!(binding.binding_id, revoked.binding_id);
        assert!(!store.workspace_runtime_binding_matches(&binding).unwrap());
        assert!(!store.workspace_runtime_binding_matches(&revoked).unwrap());
        let (_, reactivated) = store
            .put_workspace_runtime_binding_key(binding.clone(), Some(&revoked.binding_id), "owner")
            .unwrap();
        assert_eq!(
            binding.public_key_fingerprint,
            reactivated.public_key_fingerprint
        );
        assert_ne!(binding.binding_id, reactivated.binding_id);
        assert!(
            store
                .workspace_runtime_binding_matches(&reactivated)
                .unwrap()
        );
        assert!(!store.workspace_runtime_binding_matches(&binding).unwrap());
        assert!(matches!(
            store.put_workspace_runtime_binding_key(
                reactivated.clone(),
                Some(&binding.binding_id),
                "owner"
            ),
            Err(Error::RuntimeBindingIdConflict { .. })
        ));
        assert!(matches!(
            store.revoke_workspace_runtime_binding_key(
                "space",
                "remote",
                &binding.binding_id,
                "owner",
                "later"
            ),
            Err(Error::RuntimeBindingIdConflict { .. })
        ));
    }

    #[test]
    fn endpoint_aba_fences_old_config_without_copying_workspace_signing_identity() {
        let (store, binding) = binding_store();
        let renamed = store
            .update_workspace_runtime_binding_metadata(
                "space",
                "remote",
                "Renamed",
                &binding.base_url,
                "renamed-at",
            )
            .unwrap();
        assert_eq!(binding.binding_id, renamed.binding_id);
        assert!(!store.workspace_runtime_binding_matches(&binding).unwrap());
        let moved = store
            .update_workspace_runtime_binding_metadata(
                "space",
                "remote",
                "Renamed",
                "https://other.test",
                "changed",
            )
            .unwrap();
        let returned = store
            .update_workspace_runtime_binding_metadata(
                "space",
                "remote",
                "Renamed",
                "https://remote.test",
                "changed-back",
            )
            .unwrap();
        assert_ne!(moved.binding_id, binding.binding_id);
        assert_ne!(returned.binding_id, binding.binding_id);
        assert!(!store.workspace_runtime_binding_matches(&renamed).unwrap());
        store.with_conn(|conn| { conn.execute("UPDATE workspace_signing_identities SET public_key_fingerprint='sha256:replacement' WHERE workspace_id='space'", [])?; Ok(()) }).unwrap();
        assert_eq!(
            store
                .get_workspace_runtime_binding("space", "remote")
                .unwrap(),
            Some(returned.clone())
        );
        assert!(store.workspace_runtime_binding_matches(&returned).unwrap());
    }

    fn rows(conn: &Connection, sql: &str) -> Vec<Vec<rusqlite::types::Value>> {
        let mut stmt = conn.prepare(sql).unwrap();
        let count = stmt.column_count();
        stmt.query_map([], |row| (0..count).map(|i| row.get(i)).collect())
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    }

    fn schema89_bindings() -> Connection {
        let conn = legacy_binding_database();
        migrate_runtime_bindings_v87_to_v88(&conn).unwrap();
        conn.execute(
            "INSERT INTO __yoi_schema_migrations VALUES(89,'descriptive legacy evidence columns')",
            [],
        )
        .unwrap();
        conn.execute_batch("INSERT INTO workspace_runtime_bindings VALUES
            ('space','configured-trust','Configured','https://configured.test','key-c','fp-c','binding-c','configured','workspace_identity','workspace-key','sha256:workspace','trust-c','created-c','updated-c',NULL),
            ('space','verified','Verified','https://verified.test','key-v','fp-v','binding-v','verified','workspace_identity','workspace-key','sha256:workspace','trust-v','created-v','updated-v',NULL),
            ('space','revoked-null-trust','Revoked','https://revoked.test','key-r','fp-r','binding-r','revoked','workspace_identity','workspace-key',NULL,NULL,'created-r','updated-r','revoked-r'),
            ('space','legacy-verified','Legacy','https://legacy.test','key-l','fp-l','binding-l','verified','legacy_server_issuer',NULL,NULL,NULL,'created-l','updated-l',NULL),
            ('space','legacy-revoked','Legacy revoked','https://legacy-r.test','key-lr','fp-lr','binding-lr','revoked','legacy_server_issuer',NULL,NULL,NULL,'created-lr','updated-lr','revoked-lr');
            INSERT INTO workspace_runtime_verifications VALUES
            ('space','verified','binding-v','workspace-key','sha256:workspace','trust-v','fp-v','challenge-v','verified','verified','verified-at','checked-v'),
            ('space','configured-trust','binding-c','workspace-key','sha256:workspace','trust-c','fp-c','challenge-c','pending','challenge_issued',NULL,'checked-c'),
            ('space','revoked-null-trust','binding-r','workspace-key','sha256:workspace','old-trust','fp-r','challenge-r','failed','verification_failed',NULL,'checked-r');").unwrap();
        conn
    }

    #[test]
    fn schema90_preserves_connections_and_archives_all_auth_metadata_without_reenrollment() {
        let conn = schema89_bindings();
        let connections = rows(
            &conn,
            "SELECT workspace_id,runtime_id,display_name,base_url,public_key,public_key_fingerprint,binding_id,authentication_mode,created_at,updated_at,revoked_at FROM workspace_runtime_bindings ORDER BY runtime_id",
        );
        let metadata = rows(
            &conn,
            "SELECT workspace_id,runtime_id,binding_id,state,workspace_key_id,workspace_public_key_fingerprint,workspace_trust_id,updated_at FROM workspace_runtime_bindings ORDER BY runtime_id",
        );
        let proofs = rows(
            &conn,
            "SELECT * FROM workspace_runtime_verifications ORDER BY runtime_id",
        );
        let identities = rows(&conn, "SELECT * FROM workspace_signing_identities");
        let audit = rows(&conn, "SELECT * FROM workspace_runtime_binding_audit");
        let removal = rows(&conn, "SELECT * FROM runtime_removal_operations");
        migrate_runtime_binding_connections_v89_to_v90(&conn).unwrap();
        assert_eq!(current_schema_version(&conn).unwrap(), 90);
        assert_eq!(
            rows(
                &conn,
                "SELECT * FROM workspace_runtime_bindings ORDER BY runtime_id"
            ),
            connections
        );
        assert_eq!(
            rows(
                &conn,
                "SELECT * FROM workspace_runtime_binding_verification_archive ORDER BY runtime_id"
            ),
            metadata
        );
        assert_eq!(
            rows(
                &conn,
                "SELECT * FROM workspace_runtime_verification_archive ORDER BY runtime_id"
            ),
            proofs
        );
        assert_eq!(
            rows(&conn, "SELECT * FROM workspace_signing_identities"),
            identities
        );
        assert_eq!(
            rows(&conn, "SELECT * FROM workspace_runtime_binding_audit"),
            audit
        );
        assert_eq!(
            rows(&conn, "SELECT * FROM runtime_removal_operations"),
            removal
        );
        verify_runtime_binding_connection_schema(&conn).unwrap();
        assert!(!table_exists(&conn, "workspace_runtime_verifications").unwrap());
        assert!(
            conn.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, bool>(0))
                .unwrap()
        );
        assert_eq!(
            rows(&conn, "SELECT * FROM pragma_foreign_key_check"),
            Vec::<Vec<rusqlite::types::Value>>::new()
        );
        conn.execute_batch("UPDATE workspace_runtime_bindings SET base_url='https://moved.test',binding_id='binding-new' WHERE runtime_id='verified';
            UPDATE workspace_runtime_bindings SET revoked_at='admin-revoked' WHERE runtime_id='configured-trust';
            DELETE FROM workspace_runtime_bindings WHERE runtime_id='legacy-revoked';").unwrap();
        assert_eq!(
            rows(
                &conn,
                "SELECT * FROM workspace_runtime_binding_verification_archive ORDER BY runtime_id"
            ),
            metadata
        );
        assert_eq!(
            rows(
                &conn,
                "SELECT * FROM workspace_runtime_verification_archive ORDER BY runtime_id"
            ),
            proofs
        );
        conn.execute(
            "UPDATE runtime_removal_operations SET state='cleanup_pending'",
            [],
        )
        .unwrap();
        assert!(conn.execute("UPDATE workspace_runtime_bindings SET display_name='Blocked' WHERE runtime_id='remote'", []).unwrap_err().to_string().contains("runtime_removal_in_progress"));
    }

    #[test]
    fn schema90_failed_rebuild_rolls_back_connections_archives_and_schema_marker() {
        let conn = schema89_bindings();
        // An incompatible preexisting archive is not permission to discard historical evidence.
        conn.execute_batch("CREATE TABLE workspace_runtime_verification_archive(existing TEXT);")
            .unwrap();
        let before = rows(
            &conn,
            "SELECT * FROM workspace_runtime_bindings ORDER BY runtime_id",
        );
        assert!(migrate_runtime_binding_connections_v89_to_v90(&conn).is_err());
        assert_eq!(current_schema_version(&conn).unwrap(), 89);
        assert_eq!(
            rows(
                &conn,
                "SELECT * FROM workspace_runtime_bindings ORDER BY runtime_id"
            ),
            before
        );
        assert!(!table_exists(&conn, "workspace_runtime_binding_verification_archive").unwrap());
        assert!(table_exists(&conn, "workspace_runtime_verifications").unwrap());
        assert!(
            conn.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, bool>(0))
                .unwrap()
        );
    }
}
