//! Managed owner tests use only temporary catalog and Feature databases.
use super::*;
use crate::{store, subjektiv::SubjectRole};
use feature_storage::FeatureDatabase;
use rusqlite::{params, types::Value};

fn catalog(path: &Path) {
    store::tests::prepare_retained_schema(path, 129);
    let conn = Connection::open(path).unwrap();
    store::configure_sqlite(&conn).unwrap();
    conn.execute_batch("INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at) VALUES('owner','user','owner','Owner','1','1');
        INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at) VALUES('w','owner','Workspace','active','1','1'),('other','owner','Other','active','1','1');").unwrap();
    for (workspace, job, obsolete) in [
        ("w", "old", true),
        ("w", "current", false),
        ("other", "old", false),
    ] {
        let mut request = serde_json::json!({"job_id":job,"purpose":"fixture","input_ref":"saved://input","input":{"body":"saved"},"instruction":"saved instruction","profile":"builtin:backend-job","limits":{"max_concurrent_jobs":1,"timeout_seconds":60,"max_result_bytes":4096,"max_attempts":3}});
        if obsolete {
            request["input_revision"] = "old-token".into();
        }
        conn.execute("INSERT INTO backend_jobs(workspace_id,job_id,purpose,input_digest,input_ref,request_json,intent_fingerprint,state,current_attempt,created_at,updated_at) VALUES(?1,?2,'fixture','digest','saved://input',?3,'intent','pending',1,'1','2')", params![workspace,job,request.to_string()]).unwrap();
    }
}

fn feature(server_path: &Path, workspace: &str) -> (FeatureStorage, FeatureDatabase) {
    let owner = FeatureStorage::for_server_database(server_path).unwrap();
    let scope = owner.scope(workspace).unwrap();
    let registration = SubjektivStore::register(&scope).unwrap();
    let database = scope.open(&registration).unwrap();
    let store = SubjektivStore::from_database(database.clone()).unwrap();
    let subject = store
        .create_subject(SubjectRole::new("owner").unwrap())
        .unwrap();
    database.with_connection(|conn| {
        for id in ["old", "current"] {
            conn.execute("INSERT INTO journals(id,subject_id,source_session_id,title,current_position,state_token,created_at,updated_at) VALUES(?1,?2,?1,'saved title','saved position','saved token','1','2')", params![id,subject.id])?;
            conn.execute("INSERT INTO journal_entries(id,subject_id,journal_id,position,state,title,body,ongoing,created_at,updated_at) VALUES(?1,?2,?1,0,'open','entry','saved body','ongoing','1','2')",params![id,subject.id])?;
            conn.execute("INSERT INTO journal_generations(id,journal_id,job_id,attempt_id,request_json,snapshot_json,active) VALUES(?1,?1,?1,'attempt','{\"saved\":true}','{\"snapshot\":true}',1)",[id])?;
            conn.execute("INSERT INTO journal_operations(journal_id,request_id,generation_id,request_json,receipt_json) VALUES(?1,'operation',?1,'{}','{\"saved\":true}')",[id])?;
        }
        conn.execute("INSERT INTO journal_writer_bindings VALUES('writer',?1,'runtime','worker',1)", [&subject.id])?;
        let tx = conn.unchecked_transaction()?;
        tx.execute("INSERT INTO memory_records(subject_id,memory_id,current_change_id,kind,state,record_json,created_at,updated_at) VALUES(?1,'memory','change','lesson','active','{\"saved\":true}','1','2')", [&subject.id])?;
        tx.execute("INSERT INTO memory_changes(subject_id,memory_id,change_id,kind,state,record_json,created_at) VALUES(?1,'memory','change','lesson','active','{\"saved\":true}','1')", [&subject.id])?;
        tx.commit()?;
        Ok(())
    }).unwrap();
    (owner, database)
}

fn feature_path(server: &Path, workspace: &str) -> std::path::PathBuf {
    server
        .parent()
        .unwrap()
        .join("feature-storage")
        .join(workspace)
        .join("features/subjektiv.sqlite")
}

fn rows(conn: &Connection, sql: &str) -> Vec<Vec<Value>> {
    let mut query = conn.prepare(sql).unwrap();
    let count = query.column_count();
    query
        .query_map([], |row| {
            (0..count)
                .map(|i| row.get(i))
                .collect::<rusqlite::Result<Vec<Value>>>()
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

// Compare all persisted domain bytes, except the one intentionally retired bit.
fn contents(database: &FeatureDatabase) -> Vec<(String, Vec<Vec<Value>>)> {
    database
        .with_connection(|conn| {
            let tables = conn
                .prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")?
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(tables
                .into_iter()
                .filter(|table| table != "__yoi_storage_identity")
                .map(|table| {
                    let columns = if table == "journal_generations" {
                        "id,journal_id,job_id,attempt_id,request_json,snapshot_json,completion_json"
                    } else {
                        "*"
                    };
                    let mut saved = rows(conn, &format!("SELECT {columns} FROM \"{table}\""));
                    saved.sort_by_key(|row| format!("{row:?}"));
                    (table, saved)
                })
                .collect())
        })
        .unwrap()
}

fn active(database: &FeatureDatabase) -> Vec<Vec<Value>> {
    database
        .with_connection(|conn| {
            Ok(rows(
                conn,
                "SELECT id,active FROM journal_generations ORDER BY id",
            ))
        })
        .unwrap()
}

fn expected_active(old: bool) -> Vec<Vec<Value>> {
    vec![
        vec![Value::Text("current".into()), Value::Integer(1)],
        vec![Value::Text("old".into()), Value::Integer(i64::from(old))],
    ]
}

fn pending(path: &Path) -> Vec<Vec<Value>> {
    rows(
        &Connection::open(path).unwrap(),
        "SELECT workspace_id,job_id FROM backend_job_reference_migration_jobs ORDER BY 1,2",
    )
}

#[test]
fn managed_open_drains_before_return_and_preserves_current_jobs_and_all_other_content() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.sqlite");
    catalog(&path);
    let (_owner, database) = feature(&path, "w");
    let (_other_owner, other) = feature(&path, "other");
    let before = contents(&database);
    let other_before = contents(&other);
    let current = rows(
        &Connection::open(&path).unwrap(),
        "SELECT * FROM backend_jobs WHERE NOT(workspace_id='w' AND job_id='old') ORDER BY 1,2",
    );
    let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    assert!(pending(&path).is_empty());
    assert_eq!(active(&database), expected_active(false));
    assert_eq!(active(&other), expected_active(true));
    assert_eq!(contents(&database), before);
    assert_eq!(contents(&other), other_before);
    store
        .with_conn(|conn| {
            assert_eq!(
                rows(conn, "SELECT * FROM backend_jobs ORDER BY 1,2"),
                current
            );
            Ok(())
        })
        .unwrap();
    drop(store);
    drop(SqliteWorkspaceStore::open_for_test(&path).unwrap());
    assert_eq!(contents(&database), before);
    assert_eq!(active(&database), expected_active(false));
}

#[test]
fn multiple_batches_and_workspaces_share_one_feature_registration() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.sqlite");
    catalog(&path);
    let (_owner, database) = feature(&path, "w");
    let (_other_owner, other) = feature(&path, "other");
    let conn = Connection::open(&path).unwrap();
    for index in 0..130 {
        for workspace in ["w", "other"] {
            let id = format!("obsolete-{index}");
            conn.execute("INSERT INTO backend_jobs(workspace_id,job_id,purpose,input_digest,input_ref,request_json,intent_fingerprint,state,current_attempt,created_at,updated_at)
                SELECT ?1,?2,purpose,input_digest,input_ref,json_set(request_json,'$.job_id',?2),intent_fingerprint,state,current_attempt,created_at,updated_at
                FROM backend_jobs WHERE workspace_id='w' AND job_id='old'", params![workspace,id]).unwrap();
        }
    }
    drop(conn);
    let before = contents(&database);
    let other_before = contents(&other);
    drop(SqliteWorkspaceStore::open_for_test(&path).unwrap());
    assert!(pending(&path).is_empty());
    assert_eq!(active(&database), expected_active(false));
    assert_eq!(active(&other), expected_active(true));
    assert_eq!(contents(&database), before);
    assert_eq!(contents(&other), other_before);
    drop(SqliteWorkspaceStore::open_for_test(&path).unwrap());
}

#[test]
fn interruption_after_feature_commit_keeps_pinned_work_and_retries_idempotently() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.sqlite");
    catalog(&path);
    let (_owner, database) = feature(&path, "w");
    let before = contents(&database);
    INTERRUPT_AFTER_FEATURE_COMMIT.with(|flag| flag.set(true));
    let error = SqliteWorkspaceStore::open_for_test(&path).err().unwrap();
    assert!(
        error.to_string().contains("injected interruption"),
        "{error}"
    );
    assert_eq!(
        pending(&path),
        vec![vec![Value::Text("w".into()), Value::Text("old".into())]]
    );
    assert_eq!(
        rows(
            &Connection::open(&path).unwrap(),
            "SELECT physical_identity FROM backend_job_reference_migration_targets"
        ),
        vec![vec![Value::Text(database.identity().physical.clone())]]
    );
    assert_eq!(active(&database), expected_active(false));
    assert_eq!(contents(&database), before);
    drop(SqliteWorkspaceStore::open_for_test(&path).unwrap());
    assert!(pending(&path).is_empty());
    assert_eq!(contents(&database), before);
    assert_eq!(active(&database), expected_active(false));
}

#[test]
fn pinned_missing_or_replaced_feature_refuses_open_without_completing_work() {
    for replacement in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.sqlite");
        catalog(&path);
        let (_owner, database) = feature(&path, "w");
        INTERRUPT_AFTER_FEATURE_COMMIT.with(|flag| flag.set(true));
        assert!(SqliteWorkspaceStore::open_for_test(&path).is_err());
        drop(database);
        let feature_file = feature_path(&path, "w");
        std::fs::rename(&feature_file, feature_file.with_extension("retained")).unwrap();
        if replacement {
            let source = dir.path().join("replacement/server.sqlite");
            let (_replacement_owner, replacement_db) = feature(&source, "w");
            replacement_db
                .with_connection(|conn| {
                    let mut copy = Connection::open(&feature_file)?;
                    rusqlite::backup::Backup::new(conn, &mut copy)?.run_to_completion(
                        128,
                        std::time::Duration::from_millis(1),
                        None,
                    )?;
                    Ok(())
                })
                .unwrap();
        }
        let saved = pending(&path);
        assert!(SqliteWorkspaceStore::open_for_test(&path).is_err());
        assert_eq!(pending(&path), saved);
        assert_eq!(
            feature_file.exists(),
            replacement,
            "missing Feature must not be recreated"
        );
        if replacement {
            assert_eq!(
                rows(
                    &Connection::open(feature_file).unwrap(),
                    "SELECT id,active FROM journal_generations ORDER BY id"
                ),
                expected_active(true)
            );
        }
    }
}

#[test]
fn dry_run_and_catalog_only_preparation_do_not_touch_feature_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.sqlite");
    catalog(&path);
    let (_owner, database) = feature(&path, "w");
    let before = contents(&database);
    let plan = SqliteWorkspaceStore::migration_plan(&path).unwrap();
    assert_eq!(plan.current_schema_version, 129);
    assert_eq!(contents(&database), before);
    assert_eq!(active(&database), expected_active(true));
    assert_eq!(
        store::current_schema_version(&Connection::open(&path).unwrap()).unwrap(),
        129
    );
    let mut copy = Connection::open_in_memory().unwrap();
    rusqlite::backup::Backup::new(&Connection::open(&path).unwrap(), &mut copy)
        .unwrap()
        .run_to_completion(128, std::time::Duration::from_millis(1), None)
        .unwrap();
    let fixture = SqliteWorkspaceStore::from_connection_for_test(copy).unwrap();
    fixture
        .with_conn(|conn| {
            assert_eq!(
                rows(
                    conn,
                    "SELECT workspace_id,job_id FROM backend_job_reference_migration_jobs"
                ),
                vec![vec![Value::Text("w".into()), Value::Text("old".into())]]
            );
            Ok(())
        })
        .unwrap();
    assert_eq!(active(&database), expected_active(true));
    assert_eq!(contents(&database), before);
}

#[test]
fn legacy_adoption_checks_attribution_and_dry_run_never_adopts() {
    for attribution in ["w", "other"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.sqlite");
        catalog(&path);
        let (_source_owner, source) =
            feature(&dir.path().join("source/server.sqlite"), attribution);
        let destination = feature_path(&path, "w");
        std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
        source
            .with_connection(|conn| {
                let mut copy = Connection::open(&destination)?;
                rusqlite::backup::Backup::new(conn, &mut copy)?.run_to_completion(
                    128,
                    std::time::Duration::from_millis(1),
                    None,
                )?;
                copy.execute_batch("DROP TABLE __yoi_storage_identity")?;
                Ok(())
            })
            .unwrap();
        let identity_count = || {
            rows(
                &Connection::open(&destination).unwrap(),
                "SELECT count(*) FROM sqlite_schema WHERE name='__yoi_storage_identity'",
            )
        };
        SqliteWorkspaceStore::migration_plan(&path).unwrap();
        assert_eq!(identity_count(), vec![vec![Value::Integer(0)]]);
        assert_eq!(
            rows(
                &Connection::open(&destination).unwrap(),
                "SELECT id,active FROM journal_generations ORDER BY id"
            ),
            expected_active(true)
        );
        let opened = SqliteWorkspaceStore::open_for_test(&path);
        if attribution == "w" {
            drop(opened.unwrap());
            assert_eq!(identity_count(), vec![vec![Value::Integer(1)]]);
            assert!(pending(&path).is_empty());
            let owner = FeatureStorage::for_server_database(&path).unwrap();
            let scope = owner.scope("w").unwrap();
            let registration = SubjektivStore::register(&scope).unwrap();
            let identity = scope.existing_identity(&registration).unwrap();
            let adopted = scope.open_existing(&registration, &identity).unwrap();
            assert_eq!(contents(&adopted), contents(&source));
            assert_eq!(active(&adopted), expected_active(false));
        } else {
            assert!(
                opened
                    .err()
                    .unwrap()
                    .to_string()
                    .contains("attribution differs")
            );
            assert!(!pending(&path).is_empty());
            assert_eq!(identity_count(), vec![vec![Value::Integer(0)]]);
            assert_eq!(
                rows(
                    &Connection::open(&destination).unwrap(),
                    "SELECT id,active FROM journal_generations ORDER BY id"
                ),
                expected_active(true)
            );
        }
    }
}

#[test]
fn saved_identity_mismatch_is_rejected_before_feature_effect() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.sqlite");
    catalog(&path);
    let (_owner, database) = feature(&path, "w");
    let conn = Connection::open(&path).unwrap();
    store::backend_job_current::migrate(&conn).unwrap();
    conn.execute(
        "UPDATE backend_job_reference_migration_targets SET physical_identity='different-database'",
        [],
    )
    .unwrap();
    drop(conn);
    let saved = contents(&database);
    let error = SqliteWorkspaceStore::open_for_test(&path).err().unwrap();
    assert!(
        error.to_string().contains("physical identity changed"),
        "{error}"
    );
    assert!(!pending(&path).is_empty());
    assert_eq!(active(&database), expected_active(true));
    assert_eq!(contents(&database), saved);
}

#[test]
fn managed_open_drains_multiple_bounded_batches_before_return() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.sqlite");
    catalog(&path);
    let (_owner, database) = feature(&path, "w");
    let conn = Connection::open(&path).unwrap();
    for i in 0..130 {
        let id = format!("batch-{i:03}");
        conn.execute("INSERT INTO backend_jobs(workspace_id,job_id,purpose,input_digest,input_ref,request_json,intent_fingerprint,state,current_attempt,created_at,updated_at) VALUES('w',?1,'fixture','digest','saved://input','{\"input_revision\":\"obsolete\",\"input\":null}','intent','pending',1,'1','2')", [&id]).unwrap();
        database.with_connection(|feature| {
            feature.execute("INSERT INTO journal_generations(id,journal_id,job_id,attempt_id,request_json,snapshot_json,active) VALUES(?1,'old',?1,'attempt','{}','{}',0)", [&id])?;
            Ok(())
        }).unwrap();
    }
    drop(conn);
    let before = contents(&database);
    INTERRUPT_AFTER_FEATURE_COMMIT.with(|flag| flag.set(true));
    assert!(SqliteWorkspaceStore::open_for_test(&path).is_err());
    assert_eq!(pending(&path).len(), 131);
    // 'old' sorts after the first batch. Interrupted open has not exposed a
    // usable store while this original active reference still remains.
    assert_eq!(
        database
            .with_connection(|conn| conn
                .query_row(
                    "SELECT active FROM journal_generations WHERE id='old'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .map_err(Into::into))
            .unwrap(),
        1
    );
    drop(SqliteWorkspaceStore::open_for_test(&path).unwrap());
    assert!(pending(&path).is_empty());
    assert_eq!(
        database
            .with_connection(|conn| conn
                .query_row(
                    "SELECT active FROM journal_generations WHERE id='old'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .map_err(Into::into))
            .unwrap(),
        0
    );
    assert_eq!(contents(&database), before);
}

#[test]
fn unprovisioned_optional_feature_is_not_created() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.sqlite");
    catalog(&path);
    drop(SqliteWorkspaceStore::open_for_test(&path).unwrap());
    assert!(pending(&path).is_empty());
    assert!(!feature_path(&path, "w").exists());
}
