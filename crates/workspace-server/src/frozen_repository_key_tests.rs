use super::*;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use sha2::{Digest, Sha256};

// Frozen v85 contains the same SSH/Workdir tables as v86. Only the tables at
// this migration boundary are needed; unrelated authorities are not fixtures.
fn legacy_repository_keys(path: &std::path::Path) -> Connection {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys=ON;
        CREATE TABLE __yoi_schema_migrations(version INTEGER PRIMARY KEY,name TEXT NOT NULL);
        INSERT INTO __yoi_schema_migrations VALUES(86,'workspace schema baseline');
        CREATE TABLE workspaces(workspace_id TEXT PRIMARY KEY);
        INSERT INTO workspaces VALUES('space');
        CREATE TABLE runtime_removal_operations(runtime_id TEXT,state TEXT);
        CREATE TABLE repositories(repository_id TEXT PRIMARY KEY, source_revision INTEGER NOT NULL, source_fingerprint TEXT NOT NULL);
        INSERT INTO repositories VALUES('repo',9,'source-content');",
    )
    .unwrap();
    let frozen = include_str!("frozen_schema_v85.sql");
    for (start, end) in [
        (
            "CREATE TABLE repository_secret_audit_events",
            "CREATE TABLE ticket_assignment_operations",
        ),
        (
            "CREATE TABLE workdir_create_operations",
            "CREATE TABLE \"workdir_registry\"",
        ),
    ] {
        let start = frozen.find(start).unwrap();
        let end = start + frozen[start..].find(end).unwrap();
        conn.execute_batch(&frozen[start..end]).unwrap();
    }
    conn.execute_batch("INSERT INTO repository_secret_operations VALUES('space','create-key','intent','credential','key',7,'created');
        INSERT INTO repository_ssh_credentials VALUES('space','key','Key','ssh-ed25519','fingerprint',7,'active','created',NULL);
        INSERT INTO repository_ssh_credential_revisions VALUES('space','key',7,'ssh-ed25519','fingerprint','created');
        INSERT INTO repository_secret_audit_events VALUES('space','audit','credential_created','key',7,'actor','created');
        INSERT INTO workdir_create_operations(workspace_id,operation_id,request_fingerprint,repository_id,resolved_runtime_id,config_revision,config_projection_digest,working_directory_id,state,created_at,updated_at,credential_id,credential_revision)
            VALUES('space','create-workdir','intent','repo','runtime',3,'projection','workdir','pending','created','created','key',7);
        INSERT INTO workdir_create_credential_candidates VALUES('space','create-workdir',0,'primary','key',7);
        INSERT INTO workdir_create_credential_revision_retentions VALUES('space','create-workdir',0,'key',7);
        CREATE TRIGGER workdir_create_insert_blocked_by_runtime_removal
        BEFORE INSERT ON workdir_create_operations WHEN EXISTS(SELECT 1 FROM runtime_removal_operations WHERE runtime_id=NEW.resolved_runtime_id AND state IN ('pending','cleanup_pending'))
        BEGIN SELECT RAISE(ABORT,'runtime_removal_in_progress'); END;
        CREATE TRIGGER workdir_create_update_blocked_by_runtime_removal
        BEFORE UPDATE ON workdir_create_operations WHEN EXISTS(SELECT 1 FROM runtime_removal_operations WHERE runtime_id=NEW.resolved_runtime_id AND state IN ('pending','cleanup_pending'))
        BEGIN SELECT RAISE(ABORT,'runtime_removal_in_progress'); END;").unwrap();
    let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &[42; 32]).unwrap());
    for (purpose, nonce, body) in [
        ("private_key", [1; 12], b"private bytes".as_slice()),
        ("passphrase", [2; 12], b"passphrase bytes".as_slice()),
    ] {
        let mut ciphertext = body.to_vec();
        let aad = format!("yoi/repository-secret/v1/space/key/7/{purpose}");
        key.seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(aad.as_bytes()),
            &mut ciphertext,
        )
        .unwrap();
        conn.execute("INSERT INTO server_secret_versions VALUES('space','key',7,?1,'aes-256-gcm-v1',?2,?3,'created')",params![purpose,nonce.as_slice(),ciphertext]).unwrap();
    }
    conn
}

#[test]
fn repository_key_cutover_reseals_secrets_and_preserves_retention_and_audit() {
    let dir = tempfile::tempdir().unwrap();
    let conn = legacy_repository_keys(&dir.path().join("server.db"));
    std::fs::write(dir.path().join("repository-secrets.master-key"), [42; 32]).unwrap();
    migrate_repository_keys_v86_to_v87(&conn).unwrap();
    assert_eq!(current_schema_version(&conn).unwrap(), 87);
    assert!(
        conn.prepare("SELECT source_revision FROM repositories")
            .is_err()
    );
    assert_eq!(
        conn.query_row("SELECT source_fingerprint FROM repositories", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        "source-content"
    );
    let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &[42; 32]).unwrap());
    for (purpose, body) in [
        ("private_key", b"private bytes".as_slice()),
        ("passphrase", b"passphrase bytes".as_slice()),
    ] {
        let (nonce,mut ciphertext):(Vec<u8>,Vec<u8>)=conn.query_row("SELECT nonce,ciphertext FROM server_secret_objects WHERE workspace_id='space' AND secret_id='key' AND operation_id='create-key' AND purpose=?1",[purpose],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        let aad = serde_json::to_string(&(
            "yoi/repository-secret/operation",
            "space",
            "key",
            "create-key",
            purpose,
        ))
        .unwrap();
        let plaintext = key
            .open_in_place(
                Nonce::assume_unique_for_key(nonce.try_into().unwrap()),
                Aad::from(aad.as_bytes()),
                &mut ciphertext,
            )
            .unwrap();
        assert_eq!(plaintext, body);
    }
    let audit: (String, String) = conn
        .query_row(
            "SELECT operation_id,actor_account_id FROM repository_secret_audit_events",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(audit, ("create-key".into(), "actor".into()));
    let fingerprint: String = conn
        .query_row(
            "SELECT credential_fingerprint FROM workdir_create_credential_retentions",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(fingerprint, "fingerprint");
    assert!(
        conn.execute(
            "UPDATE workdir_create_credential_retentions SET credential_fingerprint='different'",
            []
        )
        .is_err()
    );
    assert!(
        conn.execute(
            "UPDATE workdir_create_credential_candidates SET credential_fingerprint='different'",
            []
        )
        .is_err()
    );
    assert!(
        conn.execute(
            "UPDATE repository_ssh_credential_keys SET public_key_fingerprint='different'",
            []
        )
        .is_err()
    );
    assert!(
        conn.execute("DELETE FROM repository_ssh_credentials", [])
            .is_err()
    );
    conn.execute_batch("INSERT INTO runtime_removal_operations VALUES('runtime','pending')")
        .unwrap();
    assert!(
        conn.execute(
            "UPDATE workdir_create_operations SET updated_at='later'",
            []
        )
        .is_err()
    );
    assert!(conn.execute("INSERT INTO workdir_create_operations(workspace_id,operation_id,request_fingerprint,repository_id,resolved_runtime_id,config_projection_digest,working_directory_id,state,created_at,updated_at) VALUES('space','blocked','intent','repo','runtime','projection','other-workdir','pending','created','created')", []).is_err());
    assert!(
        !conn
            .prepare("PRAGMA foreign_key_check")
            .unwrap()
            .exists([])
            .unwrap()
    );
}

#[test]
fn migrated_workdir_request_replays_and_retries_without_accepting_changed_display_name() {
    let dir = tempfile::tempdir().unwrap();
    let conn = legacy_repository_keys(&dir.path().join("server.db"));
    std::fs::write(dir.path().join("repository-secrets.master-key"), [42; 32]).unwrap();
    let old_digest = "sha256:db15419814117c33c089a36c5348bda85d01a53fccd8d4d427685daf5983353b";
    conn.execute("UPDATE workdir_create_operations SET request_fingerprint=?1, selector='main', source_kind='local_path',source_uri='/tmp/repo',source_fingerprint='source',source_revision=9", [old_digest]).unwrap();
    conn.execute_batch(
        "CREATE TABLE workdir_removal_operations(workspace_id TEXT,workdir_id TEXT,state TEXT)",
    )
    .unwrap();
    migrate_repository_keys_v86_to_v87(&conn).unwrap();
    let store = SqliteWorkspaceStore {
        conn: Arc::new(Mutex::new(conn)),
    };
    let migrated = store
        .load_workdir_create_operation("space", "create-workdir")
        .unwrap()
        .unwrap();
    let replay = crate::workdir_create_operations::request_fingerprint_for_replay(
        "repo",
        Some("main"),
        None,
        Some("Name"),
        "source",
        Some(&migrated.request_fingerprint),
    )
    .unwrap();
    assert_eq!(replay, format!("workdir-create-v86:9:{old_digest}"));
    assert_eq!(
        store.reserve_workdir_create_operation(&migrated).unwrap(),
        migrated
    );
    assert!(
        crate::workdir_create_operations::request_fingerprint_for_replay(
            "repo",
            Some("main"),
            None,
            Some("Changed"),
            "source",
            Some(&migrated.request_fingerprint)
        )
        .is_err()
    );
    let failed = store
        .finish_workdir_create_operation(
            "space",
            "create-workdir",
            &replay,
            false,
            Some("provider failed"),
            "failed",
        )
        .unwrap();
    let retried = store
        .begin_failed_workdir_create_retry("space", "create-workdir", &replay, "retried")
        .unwrap();
    assert_eq!(failed.state, "failed");
    assert_eq!(retried.state, "pending");
    assert_eq!(retried.request_fingerprint, replay);
    assert_eq!(
        retried.credential_candidates,
        migrated.credential_candidates
    );
    let succeeded = store
        .finish_workdir_create_operation(
            "space",
            "create-workdir",
            &replay,
            true,
            None,
            "succeeded",
        )
        .unwrap();
    assert_eq!(
        store.reserve_workdir_create_operation(&migrated).unwrap(),
        succeeded
    );
}

#[test]
fn repository_key_cutover_rejects_active_transaction_without_changing_pragmas() {
    let dir = tempfile::tempdir().unwrap();
    let conn = legacy_repository_keys(&dir.path().join("server.db"));
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert!(migrate_repository_keys_v86_to_v87(&conn).is_err());
    assert!(!conn.is_autocommit());
    assert_eq!(
        conn.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        conn.query_row("PRAGMA legacy_alter_table", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    conn.execute_batch("ROLLBACK").unwrap();
}

#[test]
fn repository_key_cutover_preserves_host_endpoints_deleted_audit_and_nondefault_pragmas() {
    for foreign_keys in [0, 1] {
        for legacy_alter in [0, 1] {
            let dir = tempfile::tempdir().unwrap();
            let conn = legacy_repository_keys(&dir.path().join("server.db"));
            std::fs::write(dir.path().join("repository-secrets.master-key"), [42; 32]).unwrap();
            conn.execute_batch("INSERT INTO repository_secret_operations VALUES('space','host-create','intent','host_trust','host',3,'created');
                INSERT INTO repository_secret_operations VALUES('space','host-move','intent','host_trust','host',4,'updated');
                INSERT INTO repository_ssh_host_trusts VALUES('space','host','new.example.test',2222,'ssh-ed25519','host-key','host-fingerprint',4,'created','updated');
                INSERT INTO repository_ssh_host_trust_revisions VALUES('space','host',3,'old.example.test',22,'ssh-ed25519','host-key','host-fingerprint','created');
                INSERT INTO repository_ssh_host_trust_revisions VALUES('space','host',4,'new.example.test',2222,'ssh-ed25519','host-key','host-fingerprint','updated');
                INSERT INTO repository_secret_audit_events VALUES('space','host-audit','host_trust_rotated','host',4,'host-actor','updated');
                INSERT INTO repository_secret_operations VALUES('space','delete-gone','intent','credential','gone',5,'deleted');
                INSERT INTO repository_secret_audit_events VALUES('space','delete-audit','credential_deleted','gone',5,'delete-actor','deleted');
                UPDATE workdir_create_operations SET host_trust_id='host',host_trust_revision=3;").unwrap();
            conn.execute_batch(&format!(
                "PRAGMA foreign_keys={foreign_keys}; PRAGMA legacy_alter_table={legacy_alter};"
            ))
            .unwrap();
            migrate_repository_keys_v86_to_v87(&conn).unwrap();
            assert_eq!(current_schema_version(&conn).unwrap(), 87);
            assert_eq!(
                conn.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                foreign_keys
            );
            assert_eq!(
                conn.query_row("PRAGMA legacy_alter_table", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                legacy_alter
            );
            let old: (String, i64) = conn.query_row("SELECT hostname,port FROM repository_ssh_host_trust_keys WHERE operation_id='host-create'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
            assert_eq!(old, ("old.example.test".into(), 22));
            let current: (String, i64, String) = conn
                .query_row(
                    "SELECT hostname,port,current_operation_id FROM repository_ssh_host_trusts",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap();
            assert_eq!(
                current,
                ("new.example.test".into(), 2222, "host-move".into())
            );
            let deleted: (String, String) = conn.query_row("SELECT operation_id,actor_account_id FROM repository_secret_audit_events WHERE event_id='delete-audit'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
            assert_eq!(deleted, ("delete-gone".into(), "delete-actor".into()));
            assert_eq!(
                conn.query_row(
                    "SELECT host_trust_fingerprint FROM workdir_create_operations",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
                "host-fingerprint"
            );
            assert!(
                !conn
                    .prepare("PRAGMA foreign_key_check")
                    .unwrap()
                    .exists([])
                    .unwrap()
            );
        }
    }
}

#[test]
fn deleted_successful_workdir_archives_verbatim_history_without_authorizing_retry_or_new_key() {
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("server.db");
    let conn = legacy_repository_keys(&database);
    let digest = "sha256:db15419814117c33c089a36c5348bda85d01a53fccd8d4d427685daf5983353b";
    conn.execute("UPDATE workdir_create_operations SET state='succeeded',request_fingerprint=?1,selector='main',source_kind='local_path',source_uri='/tmp/repo',source_revision=9,source_fingerprint='source'",[digest]).unwrap();
    conn.execute_batch("DELETE FROM workdir_create_credential_revision_retentions;
        INSERT INTO repository_secret_operations VALUES('space','delete-key','delete-intent','credential','key',7,'deleted');
        INSERT INTO repository_secret_audit_events VALUES('space','delete-audit','credential_deleted','key',7,'deleting-actor','deleted');
        DELETE FROM repository_ssh_credentials;").unwrap();
    let original =
        frozen_repository_json_rows(&conn, "SELECT * FROM workdir_create_operations", &[])
            .unwrap()
            .pop()
            .unwrap();
    let candidates = serde_json::json!([{"workspace_id":"space","operation_id":"create-workdir","ordinal":0,"role":"primary","credential_id":"key","credential_revision":7}]);
    migrate_repository_keys_v86_to_v87(&conn).unwrap();
    assert_eq!(current_schema_version(&conn).unwrap(), 87);
    assert!(!dir.path().join("repository-secrets.master-key").exists());
    assert_eq!(
        conn.query_row("SELECT count(*) FROM server_secret_objects", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    let store = Arc::new(SqliteWorkspaceStore {
        conn: Arc::new(Mutex::new(conn)),
    });
    let archived = store
        .load_workdir_create_legacy_ssh_archive("space", "create-workdir")
        .unwrap()
        .unwrap();
    assert_eq!(archived.0, original);
    assert_eq!(archived.1, candidates);
    assert_eq!(archived.0["config_revision"], 3);
    assert_eq!(archived.0["credential_revision"], 7);
    let result = store
        .load_workdir_create_operation("space", "create-workdir")
        .unwrap()
        .unwrap();
    assert_eq!(result.state, "succeeded");
    assert_eq!(result.working_directory_id, "workdir");
    assert!(result.credential_id.is_none());
    assert!(result.credential_fingerprint.is_none());
    assert!(result.credential_candidates.is_empty());
    let replay = crate::workdir_create_operations::request_fingerprint_for_replay(
        "repo",
        Some("main"),
        None,
        Some("Name"),
        "source",
        Some(&result.request_fingerprint),
    )
    .unwrap();
    assert_eq!(
        store.reserve_workdir_create_operation(&result).unwrap(),
        result
    );
    assert!(
        store
            .begin_failed_workdir_create_retry("space", "create-workdir", &replay, "retry")
            .is_err()
    );
    assert!(
        store
            .finish_workdir_create_operation(
                "space",
                "create-workdir",
                &replay,
                false,
                Some("failed"),
                "later"
            )
            .is_err()
    );
    // An explicit new mutation may recreate the same ID. The archived ordinal
    // must neither become its fingerprint nor be rebound to the completed create.
    let service =
        crate::repository_access::RepositorySecretService::open(store.clone(), &database).unwrap();
    let key = ssh_key::PrivateKey::from(ssh_key::private::Ed25519Keypair::from_seed(&[61; 32]));
    let recreated = service
        .create_credential(
            "space",
            server_api::CreateRepositorySshCredentialRequest {
                operation_id: "new-key".into(),
                credential_id: "key".into(),
                name: "New key".into(),
                private_key: key.to_openssh(ssh_key::LineEnding::LF).unwrap().to_string(),
                passphrase: None,
            },
            "actor",
        )
        .unwrap();
    let host = service
        .put_host_trust(
            "space",
            server_api::PutRepositorySshHostTrustRequest {
                operation_id: "new-host".into(),
                host_trust_id: "host".into(),
                hostname: "example.test".into(),
                port: 22,
                host_key: key.public_key().to_openssh().unwrap(),
                expected_fingerprint: None,
            },
            "actor",
        )
        .unwrap();
    let new_candidate = crate::store::WorkdirCreateCredentialCandidate {
        role: crate::store::WorkdirCreateCredentialCandidateRole::Primary,
        credential_id: "key".into(),
        credential_fingerprint: recreated.public_key_fingerprint.clone(),
    };
    assert!(
        store
            .bind_workdir_create_repository_access(
                "space",
                "create-workdir",
                &replay,
                "key",
                &recreated.public_key_fingerprint,
                "host",
                &host.fingerprint,
                "read_only",
                &[new_candidate],
                "later"
            )
            .is_err()
    );
    assert!(
        service
            .lease_ssh_materialization_access_fingerprints(
                "space",
                "key",
                "7",
                "host",
                &host.fingerprint
            )
            .is_err()
    );
    assert!(
        service
            .lease_ssh_materialization_access_fingerprints(
                "space",
                "key",
                "fingerprint",
                "host",
                &host.fingerprint
            )
            .is_err()
    );
    store.with_conn(|conn| {
        assert!(conn.execute("UPDATE workdir_create_operations SET state='failed'",[]).is_err());
        assert!(conn.execute("INSERT INTO workdir_create_credential_candidates VALUES('space','create-workdir',0,'primary','key',?1)",[&recreated.public_key_fingerprint]).is_err());
        assert!(!conn.prepare("PRAGMA foreign_key_check")?.exists([])?); Ok(())
    }).unwrap();
    let mut unrelated = result.clone();
    unrelated.operation_id = "unrelated-create".into();
    unrelated.working_directory_id = "unrelated-workdir".into();
    unrelated.state = "pending".into();
    store.reserve_workdir_create_operation(&unrelated).unwrap();
    store.with_conn(|conn| {
        conn.execute("INSERT INTO workdir_create_credential_candidates VALUES('space','unrelated-create',0,'primary','key',?1)",[&recreated.public_key_fingerprint])?;
        let error = conn.execute("UPDATE workdir_create_credential_candidates SET operation_id='create-workdir' WHERE operation_id='unrelated-create'",[]).unwrap_err();
        assert!(error.to_string().contains("archived_workdir_create_cannot_authorize"),"{error}");
        Ok(())
    }).unwrap();
    assert_eq!(
        store
            .load_workdir_create_operation("space", "create-workdir")
            .unwrap()
            .unwrap(),
        result
    );
    assert_eq!(
        store
            .load_workdir_create_legacy_ssh_archive("space", "create-workdir")
            .unwrap()
            .unwrap(),
        archived
    );
    let status = service
        .operation_status("space", "delete-key")
        .unwrap()
        .unwrap();
    assert_eq!(status.state, "committed");
    assert_eq!(status.mutation_kind.as_deref(), Some("credential_deleted"));
    assert_eq!(status.legacy_precondition_proven, Some(false));
    assert!(
        service
            .operation_status("other-space", "delete-key")
            .unwrap()
            .is_none()
    );
    let deleted_request = server_api::DeleteRepositorySshCredentialRequest {
        operation_id: "delete-key".into(),
        expected_public_key_fingerprint: recreated.public_key_fingerprint.clone(),
    };
    let projection = server_api::RepositoryAccessProjection {
        workspace_id: "space".into(),
        projection_digest: "empty".into(),
        bindings: Vec::new(),
    };
    assert!(
        service
            .delete_credential("space", "key", deleted_request, "actor", &projection)
            .unwrap_err()
            .to_string()
            .contains("operation_status")
    );
    assert!(
        service
            .get_credential("space", "key", &[])
            .unwrap()
            .is_some()
    );
}

#[test]
fn missing_retryable_or_retained_workdir_key_rolls_back_instead_of_archiving() {
    for state in ["pending", "failed", "succeeded"] {
        let dir = tempfile::tempdir().unwrap();
        let conn = legacy_repository_keys(&dir.path().join("server.db"));
        conn.execute("UPDATE workdir_create_operations SET state=?1", [state])
            .unwrap();
        if state != "succeeded" {
            conn.execute(
                "DELETE FROM workdir_create_credential_revision_retentions",
                [],
            )
            .unwrap();
        }
        conn.execute_batch("PRAGMA foreign_keys=OFF; DELETE FROM repository_ssh_credentials; PRAGMA foreign_keys=ON;").unwrap();
        assert!(
            migrate_repository_keys_v86_to_v87(&conn).is_err(),
            "{state}"
        );
        assert_eq!(current_schema_version(&conn).unwrap(), 86);
        assert!(!table_exists(&conn, "workdir_create_legacy_ssh_archives").unwrap());
        assert_eq!(
            conn.query_row("SELECT state FROM workdir_create_operations", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
            state
        );
        assert_eq!(
            conn.query_row("SELECT count(*) FROM repository_ssh_credentials", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}

// This fixture uses the schema-86 wire digest, not the new replay implementation.
fn frozen_credential_request_digest(kind: &str, name: &str, counter: u64, private: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"yoi repository credential operation v1");
    hasher.update(kind.as_bytes());
    hasher.update(b"key");
    hasher.update(name.as_bytes());
    hasher.update(counter.to_be_bytes());
    hasher.update(Sha256::digest(private.as_bytes()));
    let hex: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("sha256:{hex}")
}

fn frozen_host_request_digest(hostname: &str, counter: u64, key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"yoi repository host trust operation v1");
    hasher.update(b"host");
    hasher.update(hostname.as_bytes());
    hasher.update(22u16.to_be_bytes());
    hasher.update(key.as_bytes());
    hasher.update(counter.to_be_bytes());
    let hex: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("sha256:{hex}")
}

#[test]
fn migrated_ssh_mutations_replay_complete_old_input_without_reexecution_or_receipt_rewrite() {
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("server.db");
    let conn = legacy_repository_keys(&database);
    std::fs::write(dir.path().join("repository-secrets.master-key"), [42; 32]).unwrap();
    conn.execute_batch(
        "DELETE FROM workdir_create_operations; DELETE FROM repository_ssh_credentials;
        DELETE FROM repository_secret_operations; DELETE FROM repository_secret_audit_events;",
    )
    .unwrap();
    let old_key = ssh_key::PrivateKey::from(ssh_key::private::Ed25519Keypair::from_seed(&[62; 32]));
    let next_key =
        ssh_key::PrivateKey::from(ssh_key::private::Ed25519Keypair::from_seed(&[63; 32]));
    let old_private = old_key
        .to_openssh(ssh_key::LineEnding::LF)
        .unwrap()
        .to_string();
    let next_private = next_key
        .to_openssh(ssh_key::LineEnding::LF)
        .unwrap()
        .to_string();
    let old_fp = old_key
        .public_key()
        .fingerprint(ssh_key::HashAlg::Sha256)
        .to_string();
    let next_fp = next_key
        .public_key()
        .fingerprint(ssh_key::HashAlg::Sha256)
        .to_string();
    let public = old_key.public_key().to_openssh().unwrap();
    let created_digest = frozen_credential_request_digest("create", "Key", 0, &old_private);
    let rotated_digest = frozen_credential_request_digest("rotate", "", 1, &next_private);
    let host_created_digest = frozen_host_request_digest("old.example.test", 0, &old_fp);
    let host_rotated_digest = frozen_host_request_digest("new.example.test", 1, &old_fp);
    for (operation, digest, kind, resource, revision, timestamp, audit) in [
        (
            "create-key",
            &created_digest,
            "credential",
            "key",
            1,
            "2026-01-01",
            "credential_created",
        ),
        (
            "rotate-key",
            &rotated_digest,
            "credential",
            "key",
            2,
            "2026-01-02",
            "credential_rotated",
        ),
        (
            "create-host",
            &host_created_digest,
            "host_trust",
            "host",
            1,
            "2026-01-01",
            "host_trust_created",
        ),
        (
            "rotate-host",
            &host_rotated_digest,
            "host_trust",
            "host",
            2,
            "2026-01-02",
            "host_trust_rotated",
        ),
    ] {
        conn.execute(
            "INSERT INTO repository_secret_operations VALUES('space',?1,?2,?3,?4,?5,?6)",
            params![operation, digest, kind, resource, revision, timestamp],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO repository_secret_audit_events VALUES('space',?1,?2,?3,?4,'actor',?5)",
            params![operation, audit, resource, revision, timestamp],
        )
        .unwrap();
    }
    conn.execute("INSERT INTO repository_ssh_credentials VALUES('space','key','Key','ssh-ed25519',?1,2,'active','2026-01-01','2026-01-02')",[&next_fp]).unwrap();
    let encryption_key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &[42; 32]).unwrap());
    for (revision, fingerprint, private, timestamp) in [
        (1u64, &old_fp, &old_private, "2026-01-01"),
        (2, &next_fp, &next_private, "2026-01-02"),
    ] {
        conn.execute("INSERT INTO repository_ssh_credential_revisions VALUES('space','key',?1,'ssh-ed25519',?2,?3)",params![revision,fingerprint,timestamp]).unwrap();
        let nonce = [revision as u8; 12];
        let mut ciphertext = private.as_bytes().to_vec();
        let aad = format!("yoi/repository-secret/v1/space/key/{revision}/private_key");
        encryption_key
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(aad.as_bytes()),
                &mut ciphertext,
            )
            .unwrap();
        conn.execute("INSERT INTO server_secret_versions VALUES('space','key',?1,'private_key','aes-256-gcm-v1',?2,?3,?4)",params![revision,nonce.as_slice(),ciphertext,timestamp]).unwrap();
    }
    conn.execute("INSERT INTO repository_ssh_host_trusts VALUES('space','host','new.example.test',22,'ssh-ed25519',?1,?2,2,'2026-01-01','2026-01-02')",params![public,old_fp]).unwrap();
    for (revision, hostname, timestamp) in [
        (1, "old.example.test", "2026-01-01"),
        (2, "new.example.test", "2026-01-02"),
    ] {
        conn.execute("INSERT INTO repository_ssh_host_trust_revisions VALUES('space','host',?1,?2,22,'ssh-ed25519',?3,?4,?5)",params![revision,hostname,public,old_fp,timestamp]).unwrap();
    }
    migrate_repository_keys_v86_to_v87(&conn).unwrap();
    let store = Arc::new(SqliteWorkspaceStore {
        conn: Arc::new(Mutex::new(conn)),
    });
    let service =
        crate::repository_access::RepositorySecretService::open(store.clone(), &database).unwrap();
    let create = server_api::CreateRepositorySshCredentialRequest {
        operation_id: "create-key".into(),
        credential_id: "key".into(),
        name: "Key".into(),
        private_key: old_private,
        passphrase: None,
    };
    let rotate = server_api::RotateRepositorySshCredentialRequest {
        operation_id: "rotate-key".into(),
        expected_public_key_fingerprint: old_fp.clone(),
        private_key: next_private,
        passphrase: None,
    };
    let current = service
        .get_credential("space", "key", &[])
        .unwrap()
        .unwrap();
    assert_eq!(
        service
            .create_credential("space", create.clone(), "another-actor")
            .unwrap(),
        current
    );
    assert_eq!(
        service
            .rotate_credential("space", "key", rotate.clone(), "another-actor")
            .unwrap(),
        current
    );
    let mut changed = create.clone();
    changed.name = "Changed".into();
    assert!(
        service
            .create_credential("space", changed, "actor")
            .is_err()
    );
    let mut changed = rotate.clone();
    changed.expected_public_key_fingerprint = next_fp.clone();
    assert!(
        service
            .rotate_credential("space", "key", changed, "actor")
            .is_err()
    );
    let mut changed = rotate.clone();
    changed.private_key = create.private_key.clone();
    assert!(
        service
            .rotate_credential("space", "key", changed, "actor")
            .is_err()
    );
    for (operation, hostname, expected) in [
        ("create-host", "old.example.test", None),
        ("rotate-host", "new.example.test", Some(old_fp.clone())),
    ] {
        let request = server_api::PutRepositorySshHostTrustRequest {
            operation_id: operation.into(),
            host_trust_id: "host".into(),
            hostname: hostname.into(),
            port: 22,
            host_key: public.clone(),
            expected_fingerprint: expected,
        };
        assert_eq!(
            service
                .put_host_trust("space", request.clone(), "actor")
                .unwrap()
                .hostname,
            "new.example.test"
        );
        let mut changed = request;
        changed.port = 2222;
        assert!(service.put_host_trust("space", changed, "actor").is_err());
    }
    // Same host key at different historical endpoints is not fingerprint-only lease authority.
    assert!(
        service
            .lease_ssh_materialization_access_fingerprints(
                "space", "key", &next_fp, "host", &old_fp
            )
            .is_err()
    );
    let live = service
        .lease_ssh_materialization_access(
            "space",
            &server_api::RepositorySshAccessBinding {
                repository_key: "repo".into(),
                credential_id: "key".into(),
                host_trust_id: "host".into(),
                access: server_api::RepositoryAccessMode::ReadOnly,
            },
        )
        .unwrap();
    assert!(live.known_hosts_entry.starts_with("new.example.test "));
    store.with_conn(|conn| {
        assert_eq!(conn.query_row("SELECT count(*) FROM repository_secret_audit_events",[],|r|r.get::<_,i64>(0))?,4);
        for (operation,digest) in [("create-key",&created_digest),("rotate-key",&rotated_digest),("create-host",&host_created_digest),("rotate-host",&host_rotated_digest)] {
            assert_eq!(&conn.query_row("SELECT request_fingerprint FROM repository_secret_operations WHERE operation_id=?1",[operation],|r|r.get::<_,String>(0))?,digest);
        }
        Ok(())
    }).unwrap();
    let newer = service
        .rotate_credential(
            "space",
            "key",
            server_api::RotateRepositorySshCredentialRequest {
                operation_id: "new-rotation".into(),
                expected_public_key_fingerprint: next_fp,
                private_key: create.private_key.clone(),
                passphrase: None,
            },
            "actor",
        )
        .unwrap();
    assert_eq!(
        service
            .rotate_credential("space", "key", rotate, "actor")
            .unwrap(),
        newer
    );
    assert_eq!(
        service.create_credential("space", create, "actor").unwrap(),
        newer
    );
    assert_eq!(
        service
            .operation_status("space", "rotate-key")
            .unwrap()
            .unwrap()
            .legacy_precondition_proven,
        Some(true)
    );
}

#[test]
fn deleted_legacy_receipt_does_not_bind_to_later_reused_key_ordinal() {
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("server.db");
    let conn = legacy_repository_keys(&database);
    std::fs::write(dir.path().join("repository-secrets.master-key"), [42; 32]).unwrap();
    // The old deleted ID has been recreated using the same ordinal. Its earlier
    // delete receipt must remain history, not acquire proof from this new key.
    conn.execute_batch("DELETE FROM workdir_create_operations;
        INSERT INTO repository_secret_operations VALUES('space','old-delete','intent','credential','key',7,'2025-01-01');
        INSERT INTO repository_secret_audit_events VALUES('space','old-delete-audit','credential_deleted','key',7,'actor','2025-01-01');").unwrap();
    migrate_repository_keys_v86_to_v87(&conn).unwrap();
    let store = Arc::new(SqliteWorkspaceStore {
        conn: Arc::new(Mutex::new(conn)),
    });
    let service =
        crate::repository_access::RepositorySecretService::open(store.clone(), &database).unwrap();
    let status = service
        .operation_status("space", "old-delete")
        .unwrap()
        .unwrap();
    assert_eq!(status.legacy_precondition_proven, Some(false));
    store.with_conn(|conn| {
        let evidence:(Option<String>,Option<String>) = conn.query_row("SELECT expected_operation_id,expected_key_fingerprint FROM repository_secret_legacy_receipts WHERE operation_id='old-delete'",[],|r|Ok((r.get(0)?,r.get(1)?)))?;
        assert_eq!(evidence,(None,None));
        assert_eq!(conn.query_row("SELECT count(*) FROM repository_ssh_credentials",[],|r|r.get::<_,i64>(0))?,1);
        Ok(())
    }).unwrap();
}

#[test]
fn repository_key_cutover_failure_preserves_schema_ciphertext_and_connection_pragmas() {
    for failure in [
        "missing_master",
        "wrong_master",
        "corrupt_ciphertext",
        "missing_receipt",
        "ambiguous_receipt",
        "missing_private_key",
        "inconsistent_current_key",
        "inconsistent_retention",
        "missing_audit_receipt",
        "missing_host_receipt",
        "foreign_key_violation",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let conn = legacy_repository_keys(&dir.path().join("server.db"));
        if failure != "missing_master" {
            std::fs::write(
                dir.path().join("repository-secrets.master-key"),
                if failure == "wrong_master" {
                    [43; 32]
                } else {
                    [42; 32]
                },
            )
            .unwrap();
        }
        match failure {
            "corrupt_ciphertext" => {
                conn.execute(
                    "UPDATE server_secret_versions SET ciphertext=x'00' WHERE purpose='passphrase'",
                    [],
                )
                .unwrap();
            }
            "missing_receipt" => {
                conn.execute("DELETE FROM repository_secret_operations", [])
                    .unwrap();
            }
            "ambiguous_receipt" => {
                conn.execute("INSERT INTO repository_secret_operations SELECT workspace_id,'another',request_fingerprint,resource_kind,resource_id,result_revision,created_at FROM repository_secret_operations",[]).unwrap();
            }
            "missing_private_key" => {
                conn.execute(
                    "DELETE FROM server_secret_versions WHERE purpose='private_key'",
                    [],
                )
                .unwrap();
            }
            "inconsistent_current_key" => {
                conn.execute(
                    "UPDATE repository_ssh_credentials SET public_key_fingerprint='different'",
                    [],
                )
                .unwrap();
            }
            "inconsistent_retention" => {
                // A different valid retained key passes the ordinal-only candidate FK.
                conn.execute_batch("INSERT INTO repository_secret_operations VALUES('space','another-key','intent','credential','key',8,'later');
                    INSERT INTO repository_ssh_credential_revisions VALUES('space','key',8,'ssh-ed25519','other','later');
                    UPDATE workdir_create_credential_revision_retentions SET credential_revision=8;").unwrap();
            }
            "missing_audit_receipt" => {
                conn.execute(
                    "UPDATE repository_secret_audit_events SET created_at='other'",
                    [],
                )
                .unwrap();
            }
            "missing_host_receipt" => {
                conn.execute_batch("INSERT INTO repository_ssh_host_trusts VALUES('space','host','example.test',22,'ssh-ed25519','key','host-fingerprint',4,'created','created');
                INSERT INTO repository_ssh_host_trust_revisions VALUES('space','host',4,'example.test',22,'ssh-ed25519','key','host-fingerprint','created');").unwrap();
            }
            "foreign_key_violation" => {
                conn.execute_batch("PRAGMA foreign_keys=OFF;
                INSERT INTO repository_secret_operations VALUES('missing-space','orphan','intent','credential','deleted',1,'created');
                PRAGMA foreign_keys=ON;").unwrap();
            }
            _ => {}
        }
        let before: Vec<u8> = conn
            .query_row(
                "SELECT ciphertext FROM server_secret_versions WHERE purpose='passphrase'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let error = migrate_repository_keys_v86_to_v87(&conn).unwrap_err();
        if matches!(
            failure,
            "inconsistent_current_key" | "inconsistent_retention"
        ) {
            assert!(
                error
                    .to_string()
                    .contains("inconsistent current or retained key evidence"),
                "{failure}: {error}"
            );
        }
        assert_eq!(current_schema_version(&conn).unwrap(), 86, "{failure}");
        let after: Vec<u8> = conn
            .query_row(
                "SELECT ciphertext FROM server_secret_versions WHERE purpose='passphrase'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(after, before, "{failure}");
        assert_eq!(
            conn.query_row(
                "SELECT request_fingerprint FROM workdir_create_operations",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "intent",
            "{failure}"
        );
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM sqlite_temp_master WHERE name='repository_key_cutover'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0,
            "{failure}"
        );
        let triggers: i64 = conn.query_row("SELECT count(*) FROM sqlite_master WHERE type='trigger' AND tbl_name='workdir_create_operations'", [], |r| r.get(0)).unwrap();
        assert_eq!(triggers, 2, "{failure}");
        assert!(
            table_exists(&conn, "repository_ssh_credential_revisions").unwrap(),
            "{failure}"
        );
        assert!(
            !table_exists(&conn, "repository_ssh_credential_keys").unwrap(),
            "{failure}"
        );
        assert_eq!(
            conn.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row("PRAGMA legacy_alter_table", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        if failure == "missing_master" {
            assert!(!dir.path().join("repository-secrets.master-key").exists());
        }
    }
}
