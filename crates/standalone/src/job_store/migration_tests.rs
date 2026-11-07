//! Real SQLite schema upgrades, without live Host processes or domain output deletion.
use super::super::legacy_migrations as old;
use super::*;
use serde_json::json;

pub(crate) fn schema(conn: &Connection) -> Vec<(String, String, String)> {
    conn.prepare("SELECT type,name,sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY type,name")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

pub(crate) fn rows(
    conn: &Connection,
    table: &str,
    predicate: &str,
) -> Vec<Vec<rusqlite::types::Value>> {
    let mut statement = conn
        .prepare(&format!(
            "SELECT rowid,* FROM {table} WHERE {predicate} ORDER BY rowid"
        ))
        .unwrap();
    let count = statement.column_count();
    statement
        .query_map([], |r| {
            (0..count).map(|i| r.get(i)).collect::<Result<Vec<_>, _>>()
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn request(id: &str) -> JobRequest {
    let mut request = super::super::tests::request(id);
    request.serialization_key = None;
    request
}

fn seed_pre_six(path: &Path, version: i64) {
    let mut conn = Connection::open(path).unwrap();
    feature_storage::configure_connection(&conn).unwrap();
    let tx = conn.transaction().unwrap();
    tx.execute_batch(&old::SCHEMA.replace("input_digest", "input_revision"))
        .unwrap();
    if version >= 2 {
        tx.execute_batch(GRANT_SCHEMA).unwrap();
    }
    for (id, state, attempt_state) in [
        ("pending", "pending", "reserved"),
        ("running", "pending", "dispatched"),
        ("uncertain", "unknown", "unknown"),
        ("failed", "failed", "failed"),
        ("done", "completed", "completed"),
        ("cancelled", "cancelled", "cancelled"),
    ] {
        let mut body = serde_json::to_value(request(id)).unwrap();
        body["input_revision"] = json!("old-binding");
        tx.execute("INSERT INTO job_intents(job_id,request_json,fingerprint,max_concurrent,state,current_attempt,acknowledged) VALUES(?1,?2,'original fingerprint',8,?3,1,?4)",
            params![id, serde_json::to_string_pretty(&body).unwrap(), state, id == "done"]).unwrap();
        tx.execute("INSERT INTO job_attempts(job_id,number,attempt_id,input_revision,state,result_json,result_digest,failure) VALUES(?1,1,?2,'old-binding',?3,?4,?5,'original failure')",
            params![id, job::attempt_id(id, 1), attempt_state, (id == "done").then_some("{\"saved\":true}"), (id == "done").then_some("original-result-digest")]).unwrap();
        if version >= 2 {
            tx.execute(
                "INSERT INTO job_domain_grants VALUES(?1,'{\"original\":true}')",
                [id],
            )
            .unwrap();
        }
    }
    if version >= 3 {
        old::migrate_legacy_jobs(&tx).unwrap();
    }
    if version >= 4 {
        tx.execute_batch(RECONCILIATION_SCHEMA).unwrap();
        tx.execute_batch("INSERT INTO job_domain_reconciliations VALUES('done','done:attempt:1','original domain receipt')").unwrap();
    }
    if version == 5 {
        tx.execute_batch(old::INPUT_RETENTION_SCHEMA).unwrap();
    }
    tx.pragma_update(None, "user_version", version).unwrap();
    tx.commit().unwrap();
}

#[test]
fn every_pre_six_schema_removes_all_old_jobs_and_matches_fresh_schema() {
    for version in 1..=5 {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.sqlite3");
        seed_pre_six(&path, version);
        let mut migrated = JobStore::open(&path).unwrap();
        let fresh = JobStore::open(&dir.path().join("fresh.sqlite3")).unwrap();
        assert_eq!(
            schema(&migrated.conn),
            schema(&fresh.conn),
            "schema {version}"
        );
        for table in [
            "job_intents",
            "job_attempts",
            "job_domain_grants",
            "job_domain_reconciliations",
            "job_execution_ownership",
            "job_start_rejections",
        ] {
            assert!(
                rows(&migrated.conn, table, "1").is_empty(),
                "{version}: {table}"
            );
        }
        for id in [
            "pending",
            "running",
            "uncertain",
            "failed",
            "done",
            "cancelled",
        ] {
            assert!(matches!(migrated.get(id), Err(JobStoreError::NotFound(_))));
            assert!(migrated.retry(id).is_err());
        }
        let current = migrated.reserve(request("new-current")).unwrap();
        drop(migrated);
        let reopened = JobStore::open(&path).unwrap();
        assert_eq!(reopened.get("new-current").unwrap(), current);
        assert_eq!(reopened.pending().unwrap(), vec![current]);
    }
}

fn seed_nine(path: &Path) -> JobStore {
    let conn = Connection::open(path).unwrap();
    feature_storage::configure_connection(&conn).unwrap();
    conn.execute_batch(old::SCHEMA_SIX).unwrap();
    conn.execute_batch(GRANT_SCHEMA).unwrap();
    conn.execute_batch(RECONCILIATION_SCHEMA).unwrap();
    conn.execute_batch(old::RECOVERY_SCHEMA).unwrap();
    conn.execute_batch(old::START_REJECTION_SCHEMA).unwrap();
    conn.execute_batch(old::GRANTED_START_REJECTION_SCHEMA)
        .unwrap();
    conn.execute_batch(super::super::failure::SCHEMA).unwrap();
    conn.pragma_update(None, "user_version", 9).unwrap();
    JobStore {
        conn,
        owner_id: "fixture-owner".into(),
    }
}

fn failed_execution(store: &mut JobStore, id: &str, grant: Option<Value>) -> JobSnapshot {
    let saved = store.reserve_granted(request(id), grant).unwrap();
    let running = store.dispatch(id, &saved.attempt.attempt_id, 32).unwrap();
    store
        .finish(
            id,
            &saved.attempt.attempt_id,
            JobAttemptState::Failed,
            Some("original cause"),
        )
        .unwrap();
    store
        .confirm_execution_cleanup(id, &running.executions[0], "owned-run-joined")
        .unwrap();
    store.get(id).unwrap()
}

#[test]
fn schemas_six_to_nine_remove_only_obsolete_graph_and_preserve_current_rows_and_guards() {
    for version in 6..=9 {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.sqlite3");
        let mut store = seed_nine(&path);
        failed_execution(&mut store, "keep-result", None);
        let retried = store.retry("keep-result").unwrap();
        let running = store
            .dispatch("keep-result", &retried.attempt.attempt_id, 32)
            .unwrap();
        store
            .accept(&JobResultSubmission {
                job_id: "keep-result".into(),
                attempt_id: retried.attempt.attempt_id.clone(),
                input_digest: retried.input_digest.clone(),
                result: json!({"output":"preserve"}),
            })
            .unwrap();
        store
            .confirm_execution_cleanup(
                "keep-result",
                running.executions.last().unwrap(),
                "owned-retry-joined",
            )
            .unwrap();
        store.ack("keep-result").unwrap();
        store
            .reclaim_input(
                "keep-result",
                &retried.input_digest,
                JobInputRetirementReason::Completed,
            )
            .unwrap();
        for id in ["keep-grant", "obsolete-stopped"] {
            let saved = failed_execution(&mut store, id, Some(json!({"frozen":"scope"})));
            if version == 9 && id == "keep-grant" {
                store
                    .record_settlement_failures(
                        id,
                        &saved.attempt.attempt_id,
                        SettlementFailures {
                            primary: Some(JobFailureCause::Worker),
                            cleanup: Some(JobFailureCause::CleanupUnconfirmed),
                            ..Default::default()
                        },
                    )
                    .unwrap();
            }
            store
                .reconcile_granted_attempt(id, &saved.attempt.attempt_id, "domain-receipt")
                .unwrap();
        }
        let reserved = store.reserve(request("keep-running")).unwrap();
        store
            .dispatch("keep-running", &reserved.attempt.attempt_id, 32)
            .unwrap();
        store.reserve(request("keep-reserved")).unwrap();
        for id in ["keep-rejected", "obsolete-rejected"] {
            let saved = store.reserve(request(id)).unwrap();
            store
                .fail_reserved_result(id, &saved.attempt.attempt_id, "configuration refusal")
                .unwrap();
        }
        let erased = store.reserve(request("keep-erased")).unwrap();
        store.cancel("keep-erased").unwrap();
        store
            .reclaim_input(
                "keep-erased",
                &erased.input_digest,
                JobInputRetirementReason::ExplicitErasure,
            )
            .unwrap();
        store.reserve(request("obsolete-invalidated")).unwrap();
        store.conn.execute_batch("UPDATE job_intents SET request_json=json_set(request_json,'$.input',json('null')),input_disposition='invalidated' WHERE job_id='obsolete-invalidated';
            INSERT INTO job_version_stops VALUES('obsolete-stopped');
            INSERT INTO job_version_stops VALUES('obsolete-rejected');
            INSERT INTO job_version_history VALUES('obsolete-stopped','obsolete-stopped:attempt:1','pending','dispatched','old failure');
            CREATE TABLE legacy_job_archive(job_id TEXT PRIMARY KEY,request_json TEXT,fingerprint TEXT,attempts_json TEXT);
            INSERT INTO legacy_job_archive VALUES('obsolete-invalidated','{}','old','[]');").unwrap();
        let mut old_journal = request("obsolete-journal");
        old_journal.purpose = "subjektiv_journal".into();
        store
            .reserve_granted(old_journal, Some(json!({"session_id":"old-session"})))
            .unwrap();
        let guard: String = store
            .conn
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE name='job_input_retirement'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        store.conn.execute_batch("DROP TRIGGER job_input_retirement; UPDATE job_intents SET request_json='  ' || request_json || char(10) WHERE job_id='keep-reserved';").unwrap();
        store.conn.execute_batch(&guard).unwrap();
        let kept = [
            "keep-result",
            "keep-grant",
            "keep-running",
            "keep-reserved",
            "keep-rejected",
            "keep-erased",
        ]
        .map(|id| store.get(id).unwrap());
        // Reconstruct the exact older additive boundaries, not just user_version.
        if version <= 8 {
            store
                .conn
                .execute_batch(
                    "ALTER TABLE job_attempts DROP COLUMN primary_failure_json;
                ALTER TABLE job_attempts DROP COLUMN post_result_failure_json;
                ALTER TABLE job_attempts DROP COLUMN result_receipt_failure_json;
                ALTER TABLE job_attempts DROP COLUMN cleanup_failure_json;",
                )
                .unwrap();
        }
        if version <= 7 {
            store
                .conn
                .execute_batch("DROP TABLE job_start_rejections;")
                .unwrap();
            if version == 7 {
                store
                    .conn
                    .execute_batch(old::START_REJECTION_SCHEMA)
                    .unwrap();
            }
        }
        store
            .conn
            .pragma_update(None, "user_version", version)
            .unwrap();
        let tables = [
            "job_intents",
            "job_attempts",
            "job_domain_grants",
            "job_domain_reconciliations",
            "job_execution_ownership",
        ];
        let original: Vec<_> = tables
            .iter()
            .map(|table| {
                (
                    columns(&store.conn, table).unwrap(),
                    rows(&store.conn, table, "job_id LIKE 'keep-%'"),
                )
            })
            .collect();
        drop(store);
        let mut migrated = JobStore::open(&path).unwrap();
        for (table, (names, original)) in tables.iter().zip(original) {
            let select = names
                .iter()
                .map(|name| quoted(name))
                .collect::<Vec<_>>()
                .join(",");
            let mut statement = migrated
                .conn
                .prepare(&format!(
                    "SELECT rowid,{select} FROM {table} WHERE job_id LIKE 'keep-%' ORDER BY rowid"
                ))
                .unwrap();
            let count = statement.column_count();
            let actual: Vec<Vec<rusqlite::types::Value>> = statement
                .query_map([], |r| {
                    (0..count).map(|i| r.get(i)).collect::<Result<_, _>>()
                })
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            assert_eq!(actual, original, "{version}: {table}");
            assert!(rows(&migrated.conn, table, "job_id LIKE 'obsolete-%'").is_empty());
        }
        for saved in &kept {
            // v6 predates rejection evidence; no receipt is invented by migration.
            if version <= 7 && saved.request.job_id == "keep-rejected" {
                continue;
            }
            assert_eq!(&migrated.get(&saved.request.job_id).unwrap(), saved);
        }
        for id in [
            "obsolete-stopped",
            "obsolete-rejected",
            "obsolete-invalidated",
            "obsolete-journal",
        ] {
            assert!(matches!(migrated.get(id), Err(JobStoreError::NotFound(_))));
        }
        assert!(
            rows(
                &migrated.conn,
                "job_start_rejections",
                "job_id LIKE 'obsolete-%'"
            )
            .is_empty()
        );
        assert!(migrated.retry("keep-running").is_err());
        assert!(
            migrated
                .conn
                .execute_batch("UPDATE job_intents SET fingerprint='changed'")
                .is_err()
        );
        assert!(
            migrated
                .conn
                .execute_batch("UPDATE job_attempts SET input_digest='changed'")
                .is_err()
        );
        assert!(migrated.conn.execute_batch("UPDATE job_attempts SET result_json='{}' WHERE job_id='keep-result' AND number=2").is_err());
        assert!(
            migrated
                .conn
                .execute_batch("DELETE FROM job_execution_ownership")
                .is_err()
        );
        assert!(
            migrated
                .conn
                .execute_batch("DELETE FROM job_domain_grants")
                .is_err()
        );
        assert!(
            migrated
                .conn
                .execute_batch("DELETE FROM job_domain_reconciliations")
                .is_err()
        );
        assert!(migrated.conn.execute_batch("UPDATE job_intents SET input_disposition='invalidated' WHERE job_id='keep-reserved'").is_err());
        let fresh = JobStore::open(&dir.path().join("fresh.sqlite3")).unwrap();
        assert_eq!(
            schema(&migrated.conn),
            schema(&fresh.conn),
            "schema {version}"
        );
        let before_reopen = rows(&migrated.conn, "job_intents", "1");
        drop(migrated);
        let reopened = JobStore::open(&path).unwrap();
        assert_eq!(rows(&reopened.conn, "job_intents", "1"), before_reopen);
    }
}

#[test]
fn foreign_references_abort_removal_and_restore_all_rows_schema_and_connection_settings() {
    for version in [1, 5, 9] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.sqlite3");
        let id = if version == 9 {
            let mut store = seed_nine(&path);
            store.reserve(request("obsolete")).unwrap();
            store
                .conn
                .execute_batch("INSERT INTO job_version_stops VALUES('obsolete')")
                .unwrap();
            "obsolete"
        } else {
            seed_pre_six(&path, version);
            "running"
        };
        let mut conn = Connection::open(&path).unwrap();
        feature_storage::configure_connection(&conn).unwrap();
        conn.execute_batch("CREATE TABLE extension_ref(id TEXT REFERENCES job_intents(job_id));")
            .unwrap();
        conn.execute("INSERT INTO extension_ref VALUES(?1)", [id])
            .unwrap();
        let before = schema(&conn);
        let intents = rows(&conn, "job_intents", "1");
        let attempts = rows(&conn, "job_attempts", "1");
        assert!(matches!(
            initialize(&mut conn),
            Err(JobStoreError::Corrupt(_))
        ));
        assert_eq!(schema(&conn), before);
        assert_eq!(rows(&conn, "job_intents", "1"), intents);
        assert_eq!(rows(&conn, "job_attempts", "1"), attempts);
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            version
        );
        assert!(
            conn.pragma_query_value(None, "foreign_keys", |r| r.get::<_, bool>(0))
                .unwrap()
        );
        assert!(
            !conn
                .pragma_query_value(None, "legacy_alter_table", |r| r.get::<_, bool>(0))
                .unwrap()
        );
        assert!(
            conn.execute_batch("UPDATE job_intents SET fingerprint='changed'")
                .is_err()
        );
        assert!(conn.prepare("SELECT * FROM temp.obsolete_jobs").is_err());
        conn.execute_batch("DROP TABLE extension_ref").unwrap();
        initialize(&mut conn).unwrap();
        assert!(rows(&conn, "job_intents", "1").is_empty());
    }
}

#[test]
fn current_extension_references_and_triggers_survive_parent_rebuild() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("jobs.sqlite3");
    let mut store = seed_nine(&path);
    let saved = store.reserve(request("kept")).unwrap();
    store.conn.execute_batch("CREATE TABLE extension_ref(job_id TEXT REFERENCES job_intents(job_id),attempt_id TEXT REFERENCES job_attempts(attempt_id), body BLOB);
        CREATE TRIGGER extension_guard BEFORE UPDATE ON job_intents WHEN NEW.job_id='kept'
        BEGIN SELECT RAISE(ABORT,'extension retained'); END;").unwrap();
    store
        .conn
        .execute(
            "INSERT INTO extension_ref VALUES('kept',?1,x'00ff80')",
            [&saved.attempt.attempt_id],
        )
        .unwrap();
    let before = rows(&store.conn, "extension_ref", "1");
    drop(store);
    let migrated = JobStore::open(&path).unwrap();
    assert_eq!(migrated.get("kept").unwrap(), saved);
    assert_eq!(rows(&migrated.conn, "extension_ref", "1"), before);
    assert!(
        !migrated
            .conn
            .prepare("PRAGMA foreign_key_check")
            .unwrap()
            .exists([])
            .unwrap()
    );
    let sql: String = migrated
        .conn
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE name='extension_ref'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(sql.contains("REFERENCES job_intents(job_id)"));
    assert!(sql.contains("REFERENCES job_attempts(attempt_id)"));
    assert!(
        migrated
            .conn
            .execute_batch("UPDATE job_intents SET state='failed' WHERE job_id='kept'")
            .is_err()
    );
}

#[test]
fn unknown_columns_and_conflicting_triggers_roll_back_without_discarding_extensions() {
    for extension in ["column", "trigger"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.sqlite3");
        seed_pre_six(&path, 4);
        let mut conn = Connection::open(&path).unwrap();
        feature_storage::configure_connection(&conn).unwrap();
        if extension == "column" {
            conn.execute_batch(
                "ALTER TABLE job_intents ADD COLUMN extension_body BLOB DEFAULT X'00ff';",
            )
            .unwrap();
        } else {
            conn.execute_batch("CREATE TRIGGER job_input_retirement BEFORE UPDATE ON job_attempts BEGIN SELECT RAISE(ABORT,'extension guard'); END;").unwrap();
        }
        let before = schema(&conn);
        let intents = rows(&conn, "job_intents", "1");
        let grants = rows(&conn, "job_domain_grants", "1");
        assert!(matches!(
            initialize(&mut conn),
            Err(JobStoreError::Corrupt(_))
        ));
        assert_eq!(schema(&conn), before);
        assert_eq!(rows(&conn, "job_intents", "1"), intents);
        assert_eq!(rows(&conn, "job_domain_grants", "1"), grants);
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            4
        );
    }
}

#[test]
fn legacy_request_binding_is_removed_but_same_spelling_in_opaque_input_is_preserved() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("jobs.sqlite3");
    let mut store = seed_nine(&path);
    store.reserve(request("obsolete-binding")).unwrap();
    let mut current = request("current-input");
    current.input = json!({"input_revision":"opaque domain value"});
    let kept = store.reserve(current).unwrap();
    let guard: String = store
        .conn
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE name='job_input_retirement'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    store.conn.execute_batch("DROP TRIGGER job_input_retirement;
        UPDATE job_intents SET request_json=json_set(request_json,'$.input_revision','old-binding') WHERE job_id='obsolete-binding';").unwrap();
    store.conn.execute_batch(&guard).unwrap();
    drop(store);
    let reopened = JobStore::open(&path).unwrap();
    assert!(matches!(
        reopened.get("obsolete-binding"),
        Err(JobStoreError::NotFound(_))
    ));
    assert_eq!(reopened.get("current-input").unwrap(), kept);
}
