use super::*;

fn legacy_content_database(path: &Path) -> (Connection, String) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(r#"
        PRAGMA foreign_keys=ON;
        CREATE TABLE __yoi_schema_migrations(version INTEGER PRIMARY KEY,name TEXT NOT NULL);
        INSERT INTO __yoi_schema_migrations VALUES(85,'workspace schema baseline');
        CREATE TABLE workspaces(workspace_id TEXT PRIMARY KEY);
        INSERT INTO workspaces VALUES('space');
        CREATE TABLE workspace_config_trees(workspace_id TEXT PRIMARY KEY,revision INTEGER,tree_digest TEXT);
        CREATE TABLE workspace_config_tree_revisions(workspace_id TEXT,revision INTEGER,tree_digest TEXT,
            toolchain_fingerprint TEXT,projection_digest TEXT,manifest_json TEXT,created_at TEXT,schema_bundle_json TEXT,
            PRIMARY KEY(workspace_id,revision));
        CREATE TABLE workspace_memory_settings(workspace_id TEXT PRIMARY KEY,settings_revision INTEGER,
            language TEXT,created_at TEXT,updated_at TEXT);
        INSERT INTO workspace_memory_settings VALUES('space',9,'Japanese','created','updated');
        CREATE TABLE ticket_assignment_operations(claim_state TEXT,failure_reason TEXT,binding_recovery_json TEXT,assignment_id TEXT);
        INSERT INTO ticket_assignment_operations VALUES('pending',NULL,'legacy recovery',NULL);
    "#).unwrap();
    // Reproduce the real upgrade path: v53's table CHECK survives v73's
    // singleton ALTERs, unlike the simplified fresh-v85 baseline definition.
    let legacy = include_str!("frozen_legacy_migrations.rs");
    let start = legacy
        .find("CREATE TABLE worker_create_reservations_v53 (")
        .unwrap();
    let end = start
        + legacy[start..]
            .find("INSERT INTO worker_create_reservations_v53 (")
            .unwrap();
    conn.execute_batch(&legacy[start..end].replace(
        "worker_create_reservations_v53",
        "worker_create_reservations",
    ))
    .unwrap();
    conn.execute_batch("ALTER TABLE worker_create_reservations ADD COLUMN singleton_key TEXT;
        ALTER TABLE worker_create_reservations ADD COLUMN singleton_generation INTEGER
            CHECK (singleton_generation IS NULL OR singleton_generation > 0);
        INSERT INTO worker_create_reservations(workspace_id,allocation_key,worker_id,runtime_id,create_fingerprint,state,
            request_fingerprint,memory_settings_revision,memory_language,created_at,updated_at)
        VALUES('space','create','worker','runtime','legacy-create','reserved','legacy-request',8,'English','created','updated');").unwrap();
    let frozen = include_str!("frozen_schema_v85.sql");
    let start = frozen
        .find("CREATE TRIGGER worker_create_reservation_singleton_lease_insert")
        .unwrap();
    let end = frozen
        .find("CREATE TABLE worker_singleton_owners (")
        .unwrap();
    conn.execute_batch(&frozen[start..end]).unwrap();
    // The frozen Job DDL protects the persisted checks and public replay/resource contract.
    let jobs = frozen.find("CREATE TABLE backend_jobs (").unwrap();
    let end = frozen
        .find("CREATE TABLE worker_registry_observations (")
        .unwrap();
    conn.execute_batch(&frozen[jobs..end]).unwrap();
    let snapshot =
        config_source::ConfigTreeSnapshot::from_entries([config_source::ConfigEntry::new(
            config_source::VirtualPath::parse("main.dcdl").unwrap(),
            config_source::ConfigContentType::Decodal,
            "{} as WorkspaceConfigSchema",
        )
        .unwrap()])
        .unwrap();
    let manifest = serde_json::to_string(&snapshot.entries).unwrap();
    conn.execute(
        "INSERT INTO workspace_config_trees VALUES('space',3,?1)",
        [&snapshot.digest],
    )
    .unwrap();
    for (ordinal, created) in [(1, "first"), (3, "third")] {
        conn.execute("INSERT INTO workspace_config_tree_revisions VALUES('space',?1,?2,'toolchain','projection',?3,?4,'{}')",
            params![ordinal,snapshot.digest,manifest,created]).unwrap();
    }
    let request = BackendJobRequest {
        job_id: "legacy-job".into(),
        purpose: "migration_test".into(),
        input_ref: "test://input".into(),
        input: serde_json::json!({"content":"immutable"}),
        instruction: "Inspect immutable input.".into(),
        profile: "builtin:backend-job".into(),
        grants: Default::default(),
        serialization_key: Some("migration-resource".into()),
        source_worker: None,
        notification_target: None,
        limits: Default::default(),
    };
    let mut value = serde_json::to_value(&request).unwrap();
    value["input_revision"] = "old-number".into();
    conn.execute("INSERT INTO backend_jobs(workspace_id,job_id,purpose,input_ref,resource_key,request_json,input_revision,intent_fingerprint,state,current_attempt,created_at,updated_at)
        VALUES('space','legacy-job','migration_test','test://input',?2,?1,'old-number','old-fingerprint','pending',1,'2026-08-13T00:00:00Z','2026-08-13T00:00:00Z')",
        params![serde_json::to_string(&value).unwrap(),request.resource_key()]).unwrap();
    conn.execute_batch("INSERT INTO backend_job_attempts(workspace_id,job_id,attempt_id,attempt,input_revision,state,runtime_id,worker_id,deadline_at,created_at,updated_at)
        VALUES('space','legacy-job','legacy-job:attempt:1',1,'old-number','dispatched','runtime','worker','2026-08-14T00:00:00Z','2026-08-13T00:00:00Z','2026-08-13T00:00:00Z');
        INSERT INTO backend_job_deliveries(workspace_id,delivery_id,job_id,attempt_id,target_runtime_id,target_worker_id,state,created_at,updated_at)
        VALUES('space','delivery','legacy-job','legacy-job:attempt:1','runtime','target','pending','created','updated');") .unwrap();
    (conn, snapshot.digest)
}

#[test]
fn legacy_reservation_check_cutover_preserves_rows_constraints_indexes_triggers_and_foreign_keys() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("content.db");
    let (conn, _) = legacy_content_database(&path);
    let error = conn
        .execute_batch(
            "ALTER TABLE worker_create_reservations DROP COLUMN memory_settings_revision",
        )
        .unwrap_err();
    assert!(
        error.to_string().contains("memory_settings_revision"),
        "{error}"
    );
    conn.execute_batch(r#"
        CREATE TABLE reservation_owners(owner_id TEXT PRIMARY KEY);
        INSERT INTO reservation_owners VALUES('owner');
        ALTER TABLE worker_create_reservations ADD COLUMN extension_owner TEXT
            REFERENCES reservation_owners(owner_id) ON DELETE RESTRICT;
        UPDATE worker_create_reservations SET extension_owner='owner';
        INSERT INTO worker_create_reservations(workspace_id,allocation_key,worker_id,runtime_id,create_fingerprint,state,
            request_fingerprint,memory_settings_revision,memory_language,created_at,updated_at,singleton_key,singleton_generation)
        VALUES ('space','uncertain','uncertain-worker','runtime','old-uncertain','reserved',NULL,NULL,NULL,'old','old',NULL,NULL),
               ('space','created','created-worker','runtime','old-created','created','request',2,'Japanese','old','new','singleton',3),
               ('space','removed','removed-worker','runtime','old-removed','removed','request',7,'French','old','new',NULL,NULL);
        CREATE INDEX reservation_pending_idx ON worker_create_reservations(workspace_id,updated_at) WHERE state='reserved';
        CREATE TABLE reservation_receipts(workspace_id TEXT,allocation_key TEXT,
            FOREIGN KEY(workspace_id,allocation_key) REFERENCES worker_create_reservations(workspace_id,allocation_key) ON DELETE CASCADE);
        INSERT INTO reservation_receipts VALUES('space','create');
        CREATE TABLE reservation_events(allocation_key TEXT,state TEXT);
        CREATE TRIGGER reservation_state_audit AFTER UPDATE OF state ON worker_create_reservations
            BEGIN INSERT INTO reservation_events VALUES(NEW.allocation_key,NEW.state); END;
        CREATE TRIGGER reservation_receipt_touch AFTER INSERT ON reservation_receipts
            BEGIN UPDATE worker_create_reservations SET updated_at=updated_at
                WHERE workspace_id=NEW.workspace_id AND allocation_key=NEW.allocation_key; END;
    "#).unwrap();
    let schema_objects = |conn: &Connection| -> Vec<(String, String)> {
        conn.prepare("SELECT name,sql FROM sqlite_schema WHERE sql IS NOT NULL AND
            (tbl_name='worker_create_reservations' AND type IN ('index','trigger') OR name IN ('reservation_receipts','reservation_receipt_touch')) ORDER BY name")
            .unwrap().query_map([], |r| Ok((r.get(0)?,r.get(1)?))).unwrap()
            .collect::<rusqlite::Result<_>>().unwrap()
    };
    let original_objects = schema_objects(&conn);
    let columns: Vec<String> = conn.prepare("SELECT name FROM pragma_table_info('worker_create_reservations') WHERE name != 'memory_settings_revision' ORDER BY cid")
        .unwrap().query_map([], |r| r.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap();
    let select = format!(
        "SELECT {} FROM worker_create_reservations ORDER BY allocation_key",
        columns.join(",")
    );
    let rows = |conn: &Connection| -> Vec<Vec<rusqlite::types::Value>> {
        conn.prepare(&select)
            .unwrap()
            .query_map([], |r| {
                (0..columns.len())
                    .map(|i| r.get(i))
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    let original_rows = rows(&conn);
    let foreign_keys = |conn: &Connection| -> Vec<(String, String, String, String)> {
        conn.prepare("SELECT \"table\",\"from\",\"to\",on_delete FROM pragma_foreign_key_list('worker_create_reservations') ORDER BY id,seq")
            .unwrap().query_map([], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap()
            .collect::<rusqlite::Result<_>>().unwrap()
    };
    let original_foreign_keys = foreign_keys(&conn);
    migrate_content_state_v85_to_v86(&conn).unwrap();
    assert!(
        conn.pragma_query_value(None, "foreign_keys", |r| r.get::<_, bool>(0))
            .unwrap()
    );
    assert!(
        !conn
            .pragma_query_value(None, "legacy_alter_table", |r| r.get::<_, bool>(0))
            .unwrap()
    );
    drop(conn);
    let conn = Connection::open(path).unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON").unwrap();
    assert_eq!(current_schema_version(&conn).unwrap(), 86);
    assert!(
        !column_exists(
            &conn,
            "worker_create_reservations",
            "memory_settings_revision"
        )
        .unwrap()
    );
    assert_eq!(rows(&conn), original_rows);
    let mut expected_objects = original_objects;
    expected_objects.push((
        "worker_create_reservations_worker".into(),
        "CREATE INDEX worker_create_reservations_worker ON worker_create_reservations(workspace_id, worker_id)".into(),
    ));
    expected_objects.sort();
    assert_eq!(schema_objects(&conn), expected_objects);
    assert_eq!(foreign_keys(&conn), original_foreign_keys);
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM reservation_receipts", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM reservation_events", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| r
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        0
    );
    for sql in [
        "UPDATE worker_create_reservations SET singleton_generation=NULL WHERE allocation_key='created'",
        "UPDATE worker_create_reservations SET singleton_generation=0 WHERE allocation_key='created'",
        "UPDATE worker_create_reservations SET state='pending' WHERE allocation_key='create'",
        "UPDATE worker_create_reservations SET worker_id='worker' WHERE allocation_key='uncertain'",
        "UPDATE worker_create_reservations SET extension_owner='missing' WHERE allocation_key='create'",
        "UPDATE worker_create_reservations SET workspace_id='missing' WHERE allocation_key='create'",
    ] {
        assert!(conn.execute(sql, []).is_err(), "{sql}");
    }
    conn.execute(
        "UPDATE worker_create_reservations SET state='removed' WHERE allocation_key='create'",
        [],
    )
    .unwrap();
    let audit: (String, String) = conn
        .query_row(
            "SELECT allocation_key,state FROM reservation_events",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(audit, ("create".into(), "removed".into()));
}

#[test]
fn fresh_v85_reservations_cut_over_without_legacy_check_and_restore_connection_settings() {
    for (foreign_keys, legacy_alter) in [(false, false), (false, true), (true, false), (true, true)]
    {
        let dir = tempfile::tempdir().unwrap();
        let (conn, _) = legacy_content_database(&dir.path().join("content.db"));
        conn.execute_batch("DROP TABLE worker_create_reservations")
            .unwrap();
        let frozen = include_str!("frozen_schema_v85.sql");
        let start = frozen
            .find("CREATE TABLE worker_create_reservations (")
            .unwrap();
        let end = frozen
            .find("CREATE TABLE worker_singleton_owners (")
            .unwrap();
        conn.execute_batch(&frozen[start..end]).unwrap();
        conn.execute_batch("INSERT INTO worker_create_reservations(workspace_id,allocation_key,worker_id,runtime_id,
            create_fingerprint,state,created_at,updated_at,memory_settings_revision,memory_language)
            VALUES('space','fresh','worker','runtime','fingerprint','reserved','created','updated',9,'Japanese')").unwrap();
        conn.pragma_update(None, "foreign_keys", foreign_keys)
            .unwrap();
        conn.pragma_update(None, "legacy_alter_table", legacy_alter)
            .unwrap();
        migrate_content_state_v85_to_v86(&conn).unwrap();
        assert_eq!(current_schema_version(&conn).unwrap(), 86);
        assert!(
            !column_exists(
                &conn,
                "worker_create_reservations",
                "memory_settings_revision"
            )
            .unwrap()
        );
        let saved: (String, String) = conn
            .query_row(
                "SELECT state,memory_language FROM worker_create_reservations",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(saved, ("reserved".into(), "Japanese".into()));
        assert_eq!(
            conn.pragma_query_value(None, "foreign_keys", |r| r.get::<_, bool>(0))
                .unwrap(),
            foreign_keys
        );
        assert_eq!(
            conn.pragma_query_value(None, "legacy_alter_table", |r| r.get::<_, bool>(0))
                .unwrap(),
            legacy_alter
        );
    }
}

#[test]
fn content_cutover_preserves_content_and_languages_but_fences_old_job_capabilities() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("content.db");
    let (conn, digest) = legacy_content_database(&path);
    migrate_content_state_v85_to_v86(&conn).unwrap();
    drop(conn);
    let conn = Connection::open(path).unwrap();
    assert_eq!(current_schema_version(&conn).unwrap(), 86);
    assert!(!table_exists(&conn, "workspace_config_tree_revisions").unwrap());
    let history: (i64, String, String) = conn
        .query_row(
            "SELECT COUNT(*),content_digest,created_at FROM workspace_config_tree_history",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(history, (1, digest, "first".into()));
    assert!(!column_exists(&conn, "workspace_memory_settings", "settings_revision").unwrap());
    assert!(
        !column_exists(
            &conn,
            "worker_create_reservations",
            "memory_settings_revision"
        )
        .unwrap()
    );
    let language: String = conn
        .query_row("SELECT language FROM workspace_memory_settings", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(language, "Japanese");
    let captured: String = conn
        .query_row(
            "SELECT memory_language FROM worker_create_reservations",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(captured, "English");
    let (encoded, state): (String, String) = conn
        .query_row("SELECT request_json,state FROM backend_jobs", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    let request: BackendJobRequest = serde_json::from_str(&encoded).unwrap();
    assert_eq!(state, "unknown");
    let (input_digest, state, cleanup): (String, String, String) = conn
        .query_row(
            "SELECT input_digest,state,worker_cleanup_state FROM backend_job_attempts",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(input_digest, request.input_digest().unwrap());
    assert_eq!(state, "unknown");
    assert_eq!(cleanup, "pending");
    let delivery: String = conn
        .query_row("SELECT state FROM backend_job_deliveries", [], |r| r.get(0))
        .unwrap();
    assert_eq!(delivery, "unknown");
    let claim: String = conn
        .query_row(
            "SELECT claim_state FROM ticket_assignment_operations",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(claim, "failed");
}

#[test]
fn content_cutover_keeps_each_evaluation_provenance_and_deduplicates_only_identical_evaluations() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("content.db");
    let (conn, _) = legacy_content_database(&path);
    for (revision, toolchain, projection, schema) in [
        (4, "new-toolchain", "projection", "schema-2"),
        (5, "toolchain", "new-projection", "schema-1"),
    ] {
        conn.execute(
            "INSERT INTO workspace_config_tree_revisions
            SELECT workspace_id,?1,tree_digest,?2,?3,manifest_json,'later',?4
            FROM workspace_config_tree_revisions WHERE revision=1",
            params![revision, toolchain, projection, schema],
        )
        .unwrap();
    }
    let expected: Vec<(String,String,String,String,String,String)> = conn.prepare(
        "SELECT tree_digest,toolchain_fingerprint,projection_digest,manifest_json,created_at,schema_bundle_json
         FROM workspace_config_tree_revisions WHERE revision != 3 ORDER BY toolchain_fingerprint,projection_digest"
    ).unwrap().query_map([], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)))
        .unwrap().collect::<rusqlite::Result<_>>().unwrap();
    migrate_content_state_v85_to_v86(&conn).unwrap();
    drop(conn);
    let conn = Connection::open(path).unwrap();
    assert_eq!(current_schema_version(&conn).unwrap(), 86);
    let saved: Vec<(String,String,String,String,String,String)> = conn.prepare(
        "SELECT content_digest,toolchain_fingerprint,projection_digest,manifest_json,created_at,schema_bundle_json
         FROM workspace_config_tree_history ORDER BY toolchain_fingerprint,projection_digest"
    ).unwrap().query_map([], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)))
        .unwrap().collect::<rusqlite::Result<_>>().unwrap();
    assert_eq!(saved, expected);
    let default: String = conn.query_row(
        "SELECT dflt_value FROM pragma_table_info('workspace_config_tree_history') WHERE name='schema_bundle_json'",
        [], |r| r.get(0),
    ).unwrap();
    assert_eq!(
        default,
        "'{\"contributions\":[],\"source\":\"{}\",\"fingerprint\":\"\"}'"
    );
}

#[test]
fn content_cutover_reserved_only_releases_resource_but_uncertain_dispatch_keeps_it() {
    for (state, expected_job, expected_attempt, expected_cleanup) in [
        (
            "reserved",
            BackendJobState::Failed,
            BackendJobAttemptState::Failed,
            Some(BackendJobWorkerCleanupState::Completed),
        ),
        (
            "dispatching",
            BackendJobState::Unknown,
            BackendJobAttemptState::Unknown,
            Some(BackendJobWorkerCleanupState::Pending),
        ),
        (
            "dispatched",
            BackendJobState::Unknown,
            BackendJobAttemptState::Unknown,
            Some(BackendJobWorkerCleanupState::Pending),
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("content.db");
        let (conn, _) = legacy_content_database(&path);
        conn.execute(
            "UPDATE backend_job_attempts SET state=?1,
            runtime_id=CASE WHEN ?1='reserved' THEN NULL ELSE runtime_id END,
            worker_id=CASE WHEN ?1='reserved' THEN NULL ELSE worker_id END",
            [state],
        )
        .unwrap();
        migrate_content_state_v85_to_v86(&conn).unwrap();
        drop(conn);
        let store = SqliteWorkspaceStore {
            conn: Arc::new(Mutex::new(Connection::open(path).unwrap())),
        };
        let job = store
            .get_backend_job("space", "legacy-job")
            .unwrap()
            .unwrap();
        assert_eq!(job.state, expected_job, "{state}");
        let replay = store
            .reserve_backend_job("space", &job.request, "2026-08-15T00:00:00Z")
            .unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.attempt.state, expected_attempt);
        assert_eq!(replay.attempt.worker_cleanup_state, expected_cleanup);
        let mut next = job.request.clone();
        next.job_id = "next-job".into();
        let next = store
            .reserve_backend_job("space", &next, "2026-08-15T00:00:00Z")
            .unwrap();
        assert_eq!(next.resource_reused, state != "reserved");
        assert_eq!(
            next.job.request.job_id,
            if state == "reserved" {
                "next-job"
            } else {
                "legacy-job"
            }
        );
    }
}

#[test]
fn content_cutover_completed_result_cleanup_and_replay_remain_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("content.db");
    let (conn, _) = legacy_content_database(&path);
    let result = serde_json::json!({"finding":"preserved"});
    let (digest, encoded) = job::result_digest(&result, 4096).unwrap();
    conn.execute("UPDATE backend_jobs SET state='completed',result_json=?1,result_digest=?2,completed_at='finished'",params![encoded,digest]).unwrap();
    conn.execute("UPDATE backend_job_attempts SET state='completed',result_json=?1,result_digest=?2,
        worker_cleanup_state='completed',worker_cleanup_completed_at='cleaned',completed_at='finished'",params![encoded,digest]).unwrap();
    conn.execute(
        "UPDATE backend_job_deliveries SET state='completed',delivered_at='delivered'",
        [],
    )
    .unwrap();
    migrate_content_state_v85_to_v86(&conn).unwrap();
    drop(conn);
    let store = SqliteWorkspaceStore {
        conn: Arc::new(Mutex::new(Connection::open(path).unwrap())),
    };
    let job = store
        .get_backend_job("space", "legacy-job")
        .unwrap()
        .unwrap();
    let replay = store
        .reserve_backend_job("space", &job.request, "2026-08-15T00:00:00Z")
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.job.state, BackendJobState::Completed);
    assert_eq!(replay.job.result, Some(result.clone()));
    assert_eq!(replay.job.result_digest, Some(digest.clone()));
    assert_eq!(replay.job.completed_at.as_deref(), Some("finished"));
    assert_eq!(replay.attempt.state, BackendJobAttemptState::Completed);
    assert_eq!(replay.attempt.result, Some(result));
    assert_eq!(replay.attempt.result_digest, Some(digest));
    assert_eq!(
        replay.attempt.worker_cleanup_state,
        Some(BackendJobWorkerCleanupState::Completed)
    );
    assert_eq!(
        replay.attempt.worker_cleanup_completed_at.as_deref(),
        Some("cleaned")
    );
    let delivery: (String, String) = store
        .with_conn(|conn| {
            conn.query_row(
                "SELECT state,delivered_at FROM backend_job_deliveries",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(Error::from)
        })
        .unwrap();
    assert_eq!(delivery, ("completed".into(), "delivered".into()));
}

#[test]
fn late_job_failure_rolls_back_config_memory_jobs_and_migration_version() {
    let dir = tempfile::tempdir().unwrap();
    let (conn, _) = legacy_content_database(&dir.path().join("content.db"));
    let original_reservation_schema: Vec<(String,String)> = conn.prepare(
        "SELECT name,sql FROM sqlite_schema WHERE tbl_name='worker_create_reservations' AND sql IS NOT NULL ORDER BY name"
    ).unwrap().query_map([],|r| Ok((r.get(0)?,r.get(1)?))).unwrap().collect::<rusqlite::Result<_>>().unwrap();
    conn.execute_batch(
        "CREATE TRIGGER fail_delivery_update BEFORE UPDATE ON backend_job_deliveries
        BEGIN SELECT RAISE(ABORT,'late cutover failure'); END;",
    )
    .unwrap();
    let original: (String, String) = conn
        .query_row(
            "SELECT request_json,intent_fingerprint FROM backend_jobs",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let error = migrate_content_state_v85_to_v86(&conn).unwrap_err();
    assert!(
        error.to_string().contains("late cutover failure"),
        "{error}"
    );
    let restored_reservation_schema: Vec<(String,String)> = conn.prepare(
        "SELECT name,sql FROM sqlite_schema WHERE tbl_name='worker_create_reservations' AND sql IS NOT NULL ORDER BY name"
    ).unwrap().query_map([],|r| Ok((r.get(0)?,r.get(1)?))).unwrap().collect::<rusqlite::Result<_>>().unwrap();
    assert_eq!(restored_reservation_schema, original_reservation_schema);
    assert!(!table_exists(&conn, "worker_create_reservations_v86").unwrap());
    assert!(
        conn.pragma_query_value(None, "foreign_keys", |r| r.get::<_, bool>(0))
            .unwrap()
    );
    assert!(
        !conn
            .pragma_query_value(None, "legacy_alter_table", |r| r.get::<_, bool>(0))
            .unwrap()
    );
    assert_eq!(current_schema_version(&conn).unwrap(), 85);
    assert!(table_exists(&conn, "workspace_config_tree_revisions").unwrap());
    assert!(!table_exists(&conn, "workspace_config_tree_history").unwrap());
    assert!(column_exists(&conn, "workspace_config_trees", "revision").unwrap());
    assert!(column_exists(&conn, "workspace_memory_settings", "settings_revision").unwrap());
    assert!(
        column_exists(
            &conn,
            "worker_create_reservations",
            "memory_settings_revision"
        )
        .unwrap()
    );
    assert!(column_exists(&conn, "backend_jobs", "input_revision").unwrap());
    assert!(column_exists(&conn, "backend_job_attempts", "input_revision").unwrap());
    let saved: (String, String, String) = conn
        .query_row(
            "SELECT request_json,intent_fingerprint,state FROM backend_jobs",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(saved, (original.0, original.1, "pending".into()));
    let attempt: (String, Option<String>) = conn
        .query_row(
            "SELECT state,worker_cleanup_state FROM backend_job_attempts",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(attempt, ("dispatched".into(), None));
    let delivery: String = conn
        .query_row("SELECT state FROM backend_job_deliveries", [], |r| r.get(0))
        .unwrap();
    assert_eq!(delivery, "pending");
    let claim: String = conn
        .query_row(
            "SELECT claim_state FROM ticket_assignment_operations",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(claim, "pending");
}

#[test]
fn corrupt_config_history_rolls_back_entire_content_cutover() {
    let dir = tempfile::tempdir().unwrap();
    let (conn, _) = legacy_content_database(&dir.path().join("content.db"));
    conn.execute(
        "UPDATE workspace_config_tree_revisions SET tree_digest='corrupt'",
        [],
    )
    .unwrap();
    assert!(migrate_content_state_v85_to_v86(&conn).is_err());
    assert_eq!(current_schema_version(&conn).unwrap(), 85);
    assert!(table_exists(&conn, "workspace_config_tree_revisions").unwrap());
    assert!(!table_exists(&conn, "workspace_config_tree_history").unwrap());
    assert!(column_exists(&conn, "backend_jobs", "input_revision").unwrap());
    let state: String = conn
        .query_row("SELECT state FROM backend_jobs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(state, "pending");
}
