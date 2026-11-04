use super::*;

pub(super) const UPDATE_INTERRUPTION: &str = "バージョン更新による実行打切り";

impl JobStore {
    /// The Host supplies the identity from its revalidated exclusive Worker lease.
    /// This binds admission, results and restart reconciliation to the same owner,
    /// rather than a fresh connection UUID that could impersonate a restart.
    pub(crate) fn bind_host_lease(&mut self, lease_id: &str) -> Result<(), JobStoreError> {
        job::validate_identifier("Worker lease", lease_id, 128)?;
        self.owner_id = format!("worker-lease:{lease_id}");
        Ok(())
    }

    /// A Host recovery event probes the exact retained attempt using an actual
    /// write transaction without changing its cause, result or ownership proof.
    pub(crate) fn probe_settlement_write(
        &mut self,
        job_id: &str,
        attempt_id: &str,
    ) -> Result<(), JobStoreError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let saved = require_snapshot(&tx, job_id)?;
        check_attempt(&saved, attempt_id)?;
        if saved.version_stopped
            || !saved.executions.iter().any(|execution| {
                execution.attempt_id == attempt_id && execution.owner_id == self.owner_id
            })
        {
            return Err(JobStoreError::AttemptMismatch);
        }
        tx.execute(
            "UPDATE job_attempts SET failure=failure WHERE job_id=?1 AND attempt_id=?2",
            params![job_id, attempt_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Normal restart only: the caller rechecks the acquired Host lease immediately
    /// before this transaction. Failure fences subsequent callbacks/results but is
    /// NOT a cleanup receipt, rollback, or permission to repeat domain side effects.
    pub(crate) fn reconcile_host_restart(&mut self) -> Result<Vec<JobSnapshot>, JobStoreError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ids = tx
            .prepare(
                "SELECT i.job_id FROM job_intents i JOIN job_attempts a
             ON a.job_id=i.job_id AND a.number=i.current_attempt
             WHERE i.state='pending' AND a.state='dispatched'
             AND NOT EXISTS(SELECT 1 FROM job_version_stops s WHERE s.job_id=i.job_id)
             ORDER BY i.rowid",
            )?
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        let snapshots = ids
            .iter()
            .map(|id| require_snapshot(&tx, id))
            .collect::<Result<Vec<_>, _>>()?;
        // A second connection under the SAME live lease is not a replacement Host.
        // Reject the whole transaction before changing even an unrelated attempt.
        if snapshots.iter().any(|saved| {
            saved.executions.iter().any(|execution| {
                execution.attempt_id == saved.attempt.attempt_id
                    && execution.owner_id == self.owner_id
            })
        }) {
            return Err(JobStoreError::InvalidTransition(
                "the current Worker lease still owns a dispatched execution",
            ));
        }
        for saved in snapshots {
            let bound = saved
                .executions
                .iter()
                .find(|execution| execution.attempt_id == saved.attempt.attempt_id);
            let cause = match bound {
                Some(_) if saved.executions_released() => "host_restart_result_receipt_missing: execution cleanup is confirmed but no structured result was accepted; reconcile same-attempt domain receipts; do not repeat non-idempotent effects".into(),
                Some(execution) => format!(
                    "host_restart_execution_unconfirmed: prior owner={} run={}; no authoritative execution/Host callback drain proof; reconcile same-attempt domain/result receipts, retain cleanup ownership; do not redispatch",
                    execution.owner_id, execution.run_id),
                None => "host_restart_acceptance_evidence_missing: dispatched attempt has no saved execution binding; cannot reconcile an owner or redispatch; retain immutable input and domain receipts".into(),
            };
            tx.execute("UPDATE job_attempts SET state='failed',failure=?3 WHERE job_id=?1 AND attempt_id=?2 AND state='dispatched'",
                params![saved.request.job_id,saved.attempt.attempt_id,cause])?;
            tx.execute(
                "UPDATE job_intents SET state='failed' WHERE job_id=?1 AND state='pending'",
                [&saved.request.job_id],
            )?;
        }
        let settled = ids
            .iter()
            .map(|id| require_snapshot(&tx, id))
            .collect::<Result<Vec<_>, _>>()?;
        tx.commit()?;
        Ok(settled)
    }
}

/// Rebuild with FK enforcement disabled outside the transaction: DROP must not
/// delete children or rewrite their canonical references. Check the entire graph
/// before commit, and restore connection settings even after a failed migration.
pub(super) fn initialize(conn: &mut Connection) -> Result<(), JobStoreError> {
    if !conn.is_autocommit() {
        return Err(JobStoreError::InvalidTransition(
            "Job initialization requires an idle connection",
        ));
    }
    let foreign_keys: bool = conn.pragma_query_value(None, "foreign_keys", |r| r.get(0))?;
    let legacy_alter_table: bool =
        conn.pragma_query_value(None, "legacy_alter_table", |r| r.get(0))?;
    let result = (|| {
        conn.pragma_update(None, "foreign_keys", false)?;
        conn.pragma_update(None, "legacy_alter_table", true)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        match version {
            0 => {
                tx.execute_batch(super::SCHEMA)?;
                tx.execute_batch(GRANT_SCHEMA)?;
                tx.execute_batch(RECONCILIATION_SCHEMA)?;
                tx.execute_batch(SCHEMA)?;
            }
            1..=5 => {
                if version == 1 {
                    tx.execute_batch(GRANT_SCHEMA)?;
                }
                if version <= 2 {
                    legacy_migrations::migrate_legacy_jobs(&tx)?;
                }
                if version <= 3 {
                    tx.execute_batch(RECONCILIATION_SCHEMA)?;
                }
                if version < 5 {
                    tx.execute_batch(INPUT_RETENTION_SCHEMA)?;
                    let has_legacy_archive: bool = tx.query_row(
                        "SELECT EXISTS(SELECT 1 FROM sqlite_schema
                         WHERE type='table' AND name='legacy_job_archive')",
                        [],
                        |r| r.get(0),
                    )?;
                    if has_legacy_archive {
                        tx.execute_batch(
                            "DROP TRIGGER legacy_job_archive_immutable;
                             CREATE TRIGGER legacy_job_archive_immutable BEFORE UPDATE OF job_id,fingerprint,attempts_json ON legacy_job_archive
                             BEGIN SELECT RAISE(ABORT, 'immutable legacy Job archive'); END;
                             CREATE TRIGGER legacy_job_archive_input_retirement BEFORE UPDATE OF request_json ON legacy_job_archive
                             WHEN NOT EXISTS (SELECT 1 FROM job_intents WHERE job_id=OLD.job_id AND input_disposition<>'available')
                               OR (json_remove(NEW.request_json,'$.input') = json_remove(OLD.request_json,'$.input') AND json_type(NEW.request_json,'$.input')='null') IS NOT TRUE
                             BEGIN SELECT RAISE(ABORT, 'immutable legacy Job input'); END;",
                        )?;
                    }
                }
                migrate(&tx)?;
            }
            SCHEMA_VERSION => {}
            other => return Err(JobStoreError::UnsupportedSchema(other)),
        }
        if tx.prepare("PRAGMA foreign_key_check")?.exists([])? {
            return Err(JobStoreError::Corrupt(
                "Job migration foreign key check failed".into(),
            ));
        }
        tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        tx.commit()?;
        Ok(())
    })();
    // Attempt both restorations; an error in the first must not skip the second.
    let restore_alter = conn.pragma_update(None, "legacy_alter_table", legacy_alter_table);
    let restore_foreign_keys = conn.pragma_update(None, "foreign_keys", foreign_keys);
    result?;
    restore_alter?;
    restore_foreign_keys?;
    Ok(())
}

/// This cutover is not startup recovery. Preserve original states and causes
/// without copying input bodies into another persistent erasure domain. An old
/// execution has no stop/cleanup proof, even if its outcome was already terminal.
fn migrate(conn: &Connection) -> Result<(), JobStoreError> {
    conn.execute_batch(SCHEMA)?;
    conn.execute_batch(
        "INSERT INTO job_version_history(job_id,attempt_id,job_state,attempt_state,failure)
         SELECT i.job_id,a.attempt_id,i.state,a.state,a.failure
         FROM job_intents i JOIN job_attempts a ON a.job_id=i.job_id;
         INSERT INTO job_version_stops(job_id) SELECT job_id FROM job_intents;
         INSERT INTO job_execution_ownership(job_id,attempt_id,owner_id,run_id,writes_revoked)
         SELECT job_id,attempt_id,'schema-before-6',attempt_id,0 FROM job_attempts
         WHERE state <> 'reserved';",
    )?;
    let objects = conn
        .prepare(
            "SELECT name,sql FROM sqlite_schema
             WHERE tbl_name IN ('job_intents','job_attempts')
               AND type IN ('index','trigger') AND sql IS NOT NULL
             ORDER BY type,name",
        )?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    // Do not rename the parents: child FKs (including extension tables) must
    // continue to name job_intents/job_attempts. Copy without running old update
    // triggers, and restore extension triggers only after the rows are reloaded.
    conn.execute_batch(
        "CREATE TEMP TABLE cutover_intents AS SELECT * FROM job_intents;
         CREATE TEMP TABLE cutover_attempts AS SELECT * FROM job_attempts;
         DROP TABLE job_attempts;
         DROP TABLE job_intents;",
    )?;
    conn.execute_batch(super::SCHEMA)?;
    conn.execute_batch(
        "INSERT INTO job_intents
             (job_id,request_json,fingerprint,serialization_key,max_concurrent,
              state,current_attempt,acknowledged,input_disposition)
         SELECT job_id,request_json,fingerprint,serialization_key,max_concurrent,
                CASE WHEN state IN ('pending','unknown') THEN 'failed' ELSE state END,
                current_attempt,acknowledged,input_disposition FROM cutover_intents;",
    )?;
    conn.execute(
        "INSERT INTO job_attempts
             (job_id,number,attempt_id,input_digest,state,result_json,result_digest,failure)
         SELECT job_id,number,attempt_id,input_digest,
                CASE WHEN state IN ('reserved','dispatched','unknown') THEN 'failed' ELSE state END,
                result_json,result_digest,
                CASE WHEN state IN ('reserved','dispatched','unknown') THEN ?1 ELSE failure END
         FROM cutover_attempts",
        [UPDATE_INTERRUPTION],
    )?;
    conn.execute_batch("DROP TABLE cutover_intents; DROP TABLE cutover_attempts;")?;
    for (name, sql) in objects {
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name=?1)",
            [&name],
            |r| r.get(0),
        )?;
        if !exists {
            conn.execute_batch(&sql)?;
        }
    }
    Ok(())
}

pub(super) const SCHEMA: &str = "
CREATE TABLE job_version_history (
    job_id TEXT NOT NULL,
    attempt_id TEXT NOT NULL PRIMARY KEY,
    job_state TEXT NOT NULL,
    attempt_state TEXT NOT NULL,
    failure TEXT
);
CREATE TRIGGER job_version_history_immutable BEFORE UPDATE ON job_version_history
BEGIN SELECT RAISE(ABORT,'immutable previous Job state'); END;
CREATE TRIGGER job_version_history_retained BEFORE DELETE ON job_version_history
BEGIN SELECT RAISE(ABORT,'retained previous Job state'); END;
CREATE TABLE job_version_stops (job_id TEXT NOT NULL PRIMARY KEY);
CREATE TRIGGER job_version_stop_immutable BEFORE UPDATE ON job_version_stops
BEGIN SELECT RAISE(ABORT,'immutable version stop'); END;
CREATE TRIGGER job_version_stop_retained BEFORE DELETE ON job_version_stops
BEGIN SELECT RAISE(ABORT,'retained version stop'); END;
CREATE TABLE job_execution_ownership (
    job_id TEXT NOT NULL REFERENCES job_intents(job_id),
    attempt_id TEXT NOT NULL PRIMARY KEY REFERENCES job_attempts(attempt_id),
    owner_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    writes_revoked INTEGER NOT NULL DEFAULT 0 CHECK(writes_revoked IN (0,1)),
    cleanup_receipt TEXT,
    CHECK(cleanup_receipt IS NULL OR writes_revoked=1)
);
CREATE TRIGGER job_execution_identity_immutable BEFORE UPDATE OF job_id,attempt_id,owner_id,run_id ON job_execution_ownership
BEGIN SELECT RAISE(ABORT,'immutable Job execution binding'); END;
CREATE TRIGGER job_execution_proof_monotonic BEFORE UPDATE ON job_execution_ownership
WHEN NEW.writes_revoked < OLD.writes_revoked
 OR (OLD.cleanup_receipt IS NOT NULL AND NEW.cleanup_receipt IS NOT OLD.cleanup_receipt)
BEGIN SELECT RAISE(ABORT,'immutable execution proof'); END;
CREATE TRIGGER job_execution_retained BEFORE DELETE ON job_execution_ownership
BEGIN SELECT RAISE(ABORT,'retained Job execution'); END;
";

#[cfg(test)]
mod tests {
    //! Store integration tests use the production initializer and real temporary
    //! SQLite databases, with no service/Worker execution or external effects.
    use super::*;

    fn request(id: &str) -> JobRequest {
        super::super::tests::request(id)
    }

    fn seed(path: &Path, version: i64) -> Connection {
        let mut conn = Connection::open(path).unwrap();
        feature_storage::configure_connection(&conn).unwrap();
        let tx = conn.transaction().unwrap();
        let baseline = if version <= 2 {
            legacy_migrations::SCHEMA.replace("input_digest", "input_revision")
        } else {
            legacy_migrations::SCHEMA.to_owned()
        };
        tx.execute_batch(&baseline).unwrap();
        if version >= 2 {
            tx.execute_batch(GRANT_SCHEMA).unwrap();
        }
        if version >= 4 {
            tx.execute_batch(RECONCILIATION_SCHEMA).unwrap();
        }
        if version >= 5 {
            tx.execute_batch(INPUT_RETENTION_SCHEMA).unwrap();
        }
        for (id, state, attempt) in [
            ("reserved", "pending", "reserved"),
            ("running", "pending", "dispatched"),
            ("uncertain", "unknown", "unknown"),
            ("done", "completed", "completed"),
            ("failed", "failed", "failed"),
            ("cancelled", "cancelled", "cancelled"),
        ] {
            let req = request(id);
            let mut body = serde_json::to_value(&req).unwrap();
            if version <= 2 {
                body["input_revision"] = Value::String("old-token".into());
            }
            tx.execute(
                "INSERT INTO job_intents(job_id,request_json,fingerprint,max_concurrent,state,current_attempt,acknowledged)
                 VALUES(?1,?2,?3,8,?4,1,?5)",
                params![id, serde_json::to_string(&body).unwrap(), req.fingerprint().unwrap(), state, id == "done"],
            ).unwrap();
            tx.execute(
                &format!("INSERT INTO job_attempts(job_id,number,attempt_id,{},state,result_json,result_digest,failure)
                          VALUES(?1,1,?2,?3,?4,?5,?6,'old cause')",
                         if version <= 2 { "input_revision" } else { "input_digest" }),
                params![id, job::attempt_id(id, 1), req.input_digest().unwrap(), attempt,
                        (id == "done").then_some("{\"receipt\":true}"), (id == "done").then_some("digest")],
            ).unwrap();
        }
        // A previous failed attempt must remain distinct from the current one.
        tx.execute(
            &format!("INSERT INTO job_attempts(job_id,number,attempt_id,{},state,failure)
                      VALUES('done',2,'historic-attempt','historic-input','failed','historic cause')",
                     if version <= 2 { "input_revision" } else { "input_digest" }),
            [],
        ).unwrap();
        if version >= 2 {
            tx.execute_batch(
                "INSERT INTO job_domain_grants VALUES('done','{\"domain\":\"fixture\"}');",
            )
            .unwrap();
        }
        if version >= 4 {
            tx.execute_batch("INSERT INTO job_domain_reconciliations VALUES('done','historic-attempt','checkpoint');").unwrap();
        }
        if version == 5 {
            tx.execute_batch("UPDATE job_intents SET request_json=json_set(request_json,'$.input',NULL),input_disposition='erased' WHERE job_id='cancelled';").unwrap();
        }
        tx.execute_batch(
            "CREATE TABLE fixture_children (
                 job_id TEXT REFERENCES job_intents(job_id),
                 attempt_id TEXT REFERENCES job_attempts(attempt_id),
                 checkpoint TEXT NOT NULL);
             INSERT INTO fixture_children VALUES('done','historic-attempt','child checkpoint');
             CREATE INDEX fixture_intent_index ON job_intents(fingerprint);
             CREATE INDEX fixture_attempt_index ON job_attempts(failure);
             CREATE TRIGGER fixture_intent_retained BEFORE DELETE ON job_intents
             BEGIN SELECT RAISE(ABORT,'retained fixture intent'); END;
             CREATE TRIGGER fixture_attempt_retained BEFORE DELETE ON job_attempts
             BEGIN SELECT RAISE(ABORT,'retained fixture attempt'); END;",
        )
        .unwrap();
        tx.pragma_update(None, "user_version", version).unwrap();
        tx.commit().unwrap();
        conn
    }

    fn settings(conn: &Connection) -> (bool, bool) {
        (
            conn.pragma_query_value(None, "foreign_keys", |r| r.get(0))
                .unwrap(),
            conn.pragma_query_value(None, "legacy_alter_table", |r| r.get(0))
                .unwrap(),
        )
    }

    fn schema(conn: &Connection) -> Vec<(String, String, String)> {
        conn.prepare(
            "SELECT type,name,sql FROM sqlite_schema WHERE sql IS NOT NULL
             AND name NOT LIKE 'fixture_%' AND name NOT LIKE 'legacy_job_archive%'
             ORDER BY type,name",
        )
        .unwrap()
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get::<_, String>(2)?
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" "),
            ))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
    }

    fn assert_integrity(conn: &Connection) {
        assert!(
            !conn
                .prepare("PRAGMA foreign_key_check")
                .unwrap()
                .exists([])
                .unwrap()
        );
        let targets = conn.prepare("SELECT \"table\" FROM pragma_foreign_key_list('fixture_children') ORDER BY \"table\"")
            .unwrap().query_map([], |r| r.get::<_, String>(0)).unwrap()
            .collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(targets, ["job_attempts", "job_intents"]);
        let checkpoint: String = conn
            .query_row("SELECT checkpoint FROM fixture_children", [], |r| r.get(0))
            .unwrap();
        assert_eq!(checkpoint, "child checkpoint");
        for table in [
            "job_domain_grants",
            "job_domain_reconciliations",
            "job_execution_ownership",
        ] {
            let invalid: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_list(?1) WHERE \"table\" NOT IN ('job_intents','job_attempts'))",
                [table], |r| r.get(0),
            ).unwrap();
            assert!(!invalid, "noncanonical FK in {table}");
        }
        let extension_objects: i64 = conn.query_row("SELECT count(*) FROM sqlite_schema WHERE name IN ('fixture_intent_index','fixture_attempt_index','fixture_intent_retained','fixture_attempt_retained')", [], |r| r.get(0)).unwrap();
        assert_eq!(extension_objects, 4);
    }

    #[test]
    fn schemas_one_through_five_preserve_receipts_history_checkpoints_and_constraints() {
        let mut fresh = Connection::open_in_memory().unwrap();
        initialize(&mut fresh).unwrap();
        let canonical = schema(&fresh);
        for version in 1..=5 {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("jobs.sqlite3");
            let mut conn = seed(&path, version);
            let before_settings = settings(&conn);
            initialize(&mut conn).unwrap();
            assert_eq!(settings(&conn), before_settings);
            assert_eq!(schema(&conn), canonical, "schema {version}");
            assert_integrity(&conn);
            assert_eq!(
                conn.query_row("SELECT count(*) FROM job_version_stops", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                6
            );
            let proofs: i64 = conn.query_row("SELECT count(*) FROM job_execution_ownership WHERE writes_revoked<>0 OR cleanup_receipt IS NOT NULL", [], |r| r.get(0)).unwrap();
            assert_eq!(proofs, 0);
            let executions: i64 = conn
                .query_row("SELECT count(*) FROM job_execution_ownership", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(executions, 6);
            let historical: (String, String) = conn.query_row("SELECT attempt_state,failure FROM job_version_history WHERE attempt_id='historic-attempt'", [], |r| Ok((r.get(0)?,r.get(1)?))).unwrap();
            assert_eq!(historical, ("failed".into(), "historic cause".into()));
            if version >= 4 {
                assert_eq!(
                    conn.query_row(
                        "SELECT domain_receipt FROM job_domain_reconciliations",
                        [],
                        |r| r.get::<_, String>(0)
                    )
                    .unwrap(),
                    "checkpoint"
                );
            }
            if version <= 2 {
                let attempts: String = conn
                    .query_row(
                        "SELECT attempts_json FROM legacy_job_archive WHERE job_id='done'",
                        [],
                        |r| r.get(0),
                    )
                    .unwrap();
                let attempts: Value = serde_json::from_str(&attempts).unwrap();
                assert_eq!(attempts[1]["failure"], "historic cause");
            }
            drop(conn);
            let mut store = JobStore::open(&path).unwrap();
            for (id, old_state) in [
                ("reserved", "reserved"),
                ("running", "dispatched"),
                ("uncertain", "unknown"),
            ] {
                let saved = store.get(id).unwrap();
                assert_eq!(saved.state, JobState::Failed);
                assert_eq!(saved.attempt.state, JobAttemptState::Failed);
                assert_eq!(saved.attempt.failure.as_deref(), Some(UPDATE_INTERRUPTION));
                assert_eq!(saved.request, request(id));
                let history: (String, String) = store
                    .conn
                    .query_row(
                        "SELECT attempt_state,failure FROM job_version_history WHERE job_id=?1",
                        [id],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .unwrap();
                assert_eq!(history, (old_state.into(), "old cause".into()));
            }
            assert_eq!(
                store.get("done").unwrap().result,
                Some(serde_json::json!({"receipt":true}))
            );
            assert!(store.get("done").unwrap().acknowledged);
            assert_eq!(
                store.get("failed").unwrap().attempt.failure.as_deref(),
                Some("old cause")
            );
            assert_eq!(store.get("cancelled").unwrap().state, JobState::Cancelled);
            if version == 5 {
                let retired = store.get("cancelled").unwrap();
                assert_eq!(retired.input_disposition, JobInputDisposition::Erased);
                assert!(retired.request.input.is_null());
                assert_eq!(
                    retired.intent_fingerprint,
                    request("cancelled").fingerprint().unwrap()
                );
            }
            for id in [
                "reserved",
                "running",
                "uncertain",
                "done",
                "failed",
                "cancelled",
            ] {
                let saved = store.get(id).unwrap();
                assert!(saved.version_stopped);
                if id != "reserved" {
                    assert!(!saved.executions_released());
                    assert!(
                        saved
                            .executions
                            .iter()
                            .all(|e| e.owner_id == "schema-before-6"
                                && !e.writes_revoked
                                && e.cleanup_receipt.is_none())
                    );
                }
                assert!(store.retry(id).is_err());
                assert!(store.dispatch(id, &saved.attempt.attempt_id, 8).is_err());
            }
            for sql in [
                "UPDATE job_intents SET fingerprint='changed' WHERE job_id='done'",
                "UPDATE job_attempts SET input_digest='changed' WHERE job_id='done'",
                "UPDATE job_attempts SET result_json='{}' WHERE job_id='done' AND number=1",
                "UPDATE job_intents SET request_json='{}' WHERE job_id='done'",
                "UPDATE job_attempts SET state='unknown' WHERE job_id='failed'",
                "UPDATE job_intents SET state='unknown' WHERE job_id='failed'",
                "DELETE FROM job_version_stops",
                "UPDATE job_version_history SET failure='changed'",
                "DELETE FROM job_execution_ownership",
                "DELETE FROM job_intents WHERE job_id='done'",
                "DELETE FROM job_attempts WHERE job_id='done'",
            ] {
                assert!(store.conn.execute_batch(sql).is_err(), "{sql}");
            }
            let fresh = store.reserve(request("new")).unwrap();
            assert!(!fresh.version_stopped);
            drop(store);
            let reopened = JobStore::open(&path).unwrap();
            assert_eq!(reopened.get("new").unwrap(), fresh);
            assert_integrity(&reopened.conn);
        }
    }

    #[test]
    fn failed_foreign_key_check_rolls_back_rebuild_and_allows_same_connection_retry() {
        for version in 1..=5 {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("jobs.sqlite3");
            let mut conn = seed(&path, version);
            conn.pragma_update(None, "foreign_keys", false).unwrap();
            conn.execute(
                "INSERT INTO fixture_children VALUES('missing','missing','bad checkpoint')",
                [],
            )
            .unwrap();
            conn.pragma_update(None, "foreign_keys", true).unwrap();
            let before_schema = schema(&conn);
            let before_request: String = conn
                .query_row(
                    "SELECT request_json FROM job_intents WHERE job_id='running'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            let before_settings = settings(&conn);
            assert!(matches!(
                initialize(&mut conn),
                Err(JobStoreError::Corrupt(_))
            ));
            assert_eq!(settings(&conn), before_settings);
            assert!(conn.is_autocommit());
            assert_eq!(schema(&conn), before_schema);
            assert_eq!(
                conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                    .unwrap(),
                version
            );
            assert_eq!(
                conn.query_row(
                    "SELECT request_json FROM job_intents WHERE job_id='running'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
                before_request
            );
            assert_eq!(
                conn.query_row(
                    "SELECT state FROM job_attempts WHERE job_id='running'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
                "dispatched"
            );
            assert!(
                !conn
                    .prepare("SELECT 1 FROM sqlite_temp_schema WHERE name LIKE 'cutover_%'")
                    .unwrap()
                    .exists([])
                    .unwrap()
            );
            assert!(
                conn.execute(
                    "UPDATE job_intents SET fingerprint='changed' WHERE job_id='done'",
                    []
                )
                .is_err()
            );
            conn.execute("DELETE FROM fixture_children WHERE job_id='missing'", [])
                .unwrap();
            initialize(&mut conn).unwrap();
            assert_integrity(&conn);
            let after_schema = schema(&conn);
            initialize(&mut conn).unwrap();
            assert_eq!(schema(&conn), after_schema);
            assert_eq!(settings(&conn), before_settings);
            drop(conn);
            let reopened = JobStore::open(&path).unwrap();
            assert!(reopened.get("failed").unwrap().version_stopped);
            assert_integrity(&reopened.conn);
        }
    }

    #[test]
    fn initialization_restores_both_pragma_values_on_success_and_failure() {
        for foreign_keys in [false, true] {
            for legacy_alter_table in [false, true] {
                let mut conn = Connection::open_in_memory().unwrap();
                conn.pragma_update(None, "foreign_keys", foreign_keys)
                    .unwrap();
                conn.pragma_update(None, "legacy_alter_table", legacy_alter_table)
                    .unwrap();
                initialize(&mut conn).unwrap();
                assert_eq!(settings(&conn), (foreign_keys, legacy_alter_table));
                conn.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
                    .unwrap();
                assert!(matches!(
                    initialize(&mut conn),
                    Err(JobStoreError::UnsupportedSchema(_))
                ));
                assert_eq!(settings(&conn), (foreign_keys, legacy_alter_table));
                assert!(conn.is_autocommit());
            }
        }
    }

    #[tokio::test]
    async fn settlement_recovers_sqlite_contention_for_same_attempt_without_dispatch() {
        // Normal-operation storage fault, not migration or process-exit coverage.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.sqlite3");
        let mut store = JobStore::open(&path).unwrap();
        store.conn.busy_timeout(std::time::Duration::ZERO).unwrap();
        let reserved = store.reserve(request("settlement")).unwrap();
        let dispatched = store
            .dispatch("settlement", &reserved.attempt.attempt_id, 8)
            .unwrap();
        let ownership = dispatched.executions[0].clone();
        let blocker = Connection::open(&path).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
        let mut calls = 0;
        crate::jobs::recover_settlement(|| {
            calls += 1;
            let result = (|| -> Result<(), JobStoreError> {
                store.check_execution_binding(
                    "settlement",
                    &reserved.attempt.attempt_id,
                    &reserved.input_digest,
                    &ownership,
                )?;
                store.finish(
                    "settlement",
                    &reserved.attempt.attempt_id,
                    JobAttemptState::Failed,
                    Some("timeout"),
                )?;
                store.confirm_execution_cleanup(
                    "settlement",
                    &ownership,
                    "fixture-owned-run-joined",
                )
            })();
            if calls == 1 {
                assert!(matches!(result, Err(JobStoreError::Sqlite(_))));
                assert_eq!(store.get("settlement").unwrap(), dispatched);
                blocker.execute_batch("ROLLBACK").unwrap();
            }
            result.map_err(crate::jobs::JobServiceError::from)
        })
        .await
        .unwrap();
        assert_eq!(calls, 2);
        let settled = store.get("settlement").unwrap();
        assert_eq!(settled.attempt.attempt_id, reserved.attempt.attempt_id);
        assert_eq!(settled.attempt.number, 1);
        assert_eq!(settled.attempt.failure.as_deref(), Some("timeout"));
        assert!(settled.executions_released());
        drop(store);
        assert_eq!(
            JobStore::open(&path).unwrap().get("settlement").unwrap(),
            settled
        );
    }

    #[tokio::test]
    async fn settlement_persistent_contention_exhausts_budget_and_retains_obligations() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.sqlite3");
        let mut store = JobStore::open(&path).unwrap();
        store.conn.busy_timeout(std::time::Duration::ZERO).unwrap();
        let reserved = store.reserve(request("blocked")).unwrap();
        let dispatched = store
            .dispatch("blocked", &reserved.attempt.attempt_id, 8)
            .unwrap();
        let blocker = Connection::open(&path).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
        let mut calls = 0;
        let error = crate::jobs::recover_settlement(|| {
            calls += 1;
            store.finish(
                "blocked",
                &reserved.attempt.attempt_id,
                JobAttemptState::Failed,
                Some("timeout"),
            )?;
            Ok(())
        })
        .await
        .unwrap_err();
        assert!(
            matches!(error, crate::jobs::JobServiceError::SettlementRecoveryExhausted { attempts: 6, source }
            if matches!(*source, crate::jobs::JobServiceError::Store(JobStoreError::Sqlite(_))))
        );
        assert_eq!(calls, 6);
        assert_eq!(store.get("blocked").unwrap(), dispatched);
        assert!(!store.execution_obligations().unwrap().is_empty());
        blocker.execute_batch("ROLLBACK").unwrap();
    }

    #[tokio::test]
    async fn exhausted_settlement_detects_sqlite_recovery_without_user_action() {
        use std::sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        };
        use tokio::sync::Notify;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.sqlite3");
        let mut store = JobStore::open(&path).unwrap();
        store.conn.busy_timeout(std::time::Duration::ZERO).unwrap();
        let reserved = store.reserve(request("event-recovery")).unwrap();
        let dispatched = store
            .dispatch("event-recovery", &reserved.attempt.attempt_id, 8)
            .unwrap();
        let ownership = dispatched.executions[0].clone();
        let store = Mutex::new(store);
        let blocker = Connection::open(&path).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
        let calls = AtomicUsize::new(0);
        let exhausted = Notify::new();
        let recovered = Notify::new();
        let issue = Mutex::new(None);
        let settle = crate::jobs::recover_owned_settlement(
            &recovered,
            &issue,
            || {
                let call = calls.fetch_add(1, Ordering::SeqCst) + 1;
                let mut store = store.lock().unwrap();
                let result = store.finish(
                    "event-recovery",
                    &reserved.attempt.attempt_id,
                    JobAttemptState::Failed,
                    Some("original timeout"),
                );
                if call == 6 {
                    exhausted.notify_one();
                }
                result?;
                store.confirm_execution_cleanup(
                    "event-recovery",
                    &ownership,
                    "fixture-owned-run-joined",
                )?;
                Ok(())
            },
            || {
                store
                    .lock()
                    .unwrap()
                    .probe_settlement_write("event-recovery", &reserved.attempt.attempt_id)?;
                Ok(())
            },
        );
        let restore_storage = async {
            exhausted.notified().await;
            assert!(matches!(
                issue.lock().unwrap().as_ref(),
                Some(crate::jobs::JobServiceError::SettlementRecoveryExhausted { attempts: 6, .. })
            ));
            assert!(
                issue
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .to_string()
                    .contains("next action")
            );
            assert_eq!(
                store.lock().unwrap().get("event-recovery").unwrap(),
                dispatched
            );
            blocker.execute_batch("ROLLBACK").unwrap();
            // A restored read is not a write recovery event. Do not signal the
            // Notify or call a Host API: the retained owner must detect recovery.
            assert_eq!(
                store.lock().unwrap().get("event-recovery").unwrap(),
                dispatched
            );
            tokio::task::yield_now().await;
            assert_eq!(calls.load(Ordering::SeqCst), 6);
        };
        let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(8), async {
            tokio::join!(settle, restore_storage)
        })
        .await
        .unwrap();
        result.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 7);
        assert!(issue.lock().unwrap().is_none());
        let saved = store.lock().unwrap().get("event-recovery").unwrap();
        assert_eq!(saved.state, JobState::Failed);
        assert_eq!(saved.attempt.attempt_id, reserved.attempt.attempt_id);
        assert_eq!(saved.attempt.failure.as_deref(), Some("original timeout"));
        assert!(saved.executions_released());
    }

    #[test]
    fn restart_distinguishes_missing_result_from_saved_execution_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.sqlite3");
        let mut store = JobStore::open(&path).unwrap();
        store.bind_host_lease("prior-host").unwrap();
        let reserved = store.reserve(request("receipt-missing")).unwrap();
        let dispatched = store
            .dispatch("receipt-missing", &reserved.attempt.attempt_id, 8)
            .unwrap();
        store
            .confirm_execution_cleanup(
                "receipt-missing",
                &dispatched.executions[0],
                "fixture-owned-run-joined",
            )
            .unwrap();
        let before = store.get("receipt-missing").unwrap();
        drop(store);
        let mut reopened = JobStore::open(&path).unwrap();
        reopened.bind_host_lease("next-host").unwrap();
        let failed = reopened.reconcile_host_restart().unwrap().pop().unwrap();
        assert_eq!(failed.state, JobState::Failed);
        assert_eq!(failed.executions, before.executions);
        assert!(failed.executions_released());
        assert!(
            failed
                .attempt
                .failure
                .as_deref()
                .unwrap()
                .starts_with("host_restart_result_receipt_missing:")
        );
        assert_eq!(failed.attempt.attempt_id, reserved.attempt.attempt_id);
        assert!(failed.result.is_none());
        assert!(reopened.pending().unwrap().is_empty());
    }

    #[test]
    fn restart_missing_dispatch_binding_fails_without_creating_retry_or_cleanup_proof() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.sqlite3");
        let mut store = JobStore::open(&path).unwrap();
        store.bind_host_lease("first-owner").unwrap();
        let reserved = store.reserve(request("missing-binding")).unwrap();
        store
            .conn
            .execute(
                "UPDATE job_attempts SET state='dispatched' WHERE job_id='missing-binding'",
                [],
            )
            .unwrap();
        drop(store);
        let mut reopened = JobStore::open(&path).unwrap();
        reopened.bind_host_lease("next-owner").unwrap();
        let stopped = reopened.reconcile_host_restart().unwrap().pop().unwrap();
        assert_eq!(stopped.state, JobState::Failed);
        assert_eq!(stopped.attempt.attempt_id, reserved.attempt.attempt_id);
        assert!(
            stopped
                .attempt
                .failure
                .as_deref()
                .unwrap()
                .starts_with("host_restart_acceptance_evidence_missing:")
        );
        assert!(stopped.executions.is_empty());
        assert!(!stopped.executions_released());
        assert!(matches!(
            reopened.execution_obligations(),
            Err(JobStoreError::Corrupt(_))
        ));
        assert!(!stopped.version_stopped);
        assert!(reopened.retry("missing-binding").is_err());
        assert!(reopened.pending().unwrap().is_empty());
        assert!(reopened.reconcile_host_restart().unwrap().is_empty());
        assert_eq!(reopened.get("missing-binding").unwrap(), stopped);
    }

    #[test]
    fn normal_restart_never_resumes_or_rewrites_migration_stopped_jobs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.sqlite3");
        let conn = seed(&path, 5);
        drop(conn);
        let mut store = JobStore::open(&path).unwrap();
        let migrated = store.get("running").unwrap();
        let fresh = store.reserve(request("post-upgrade")).unwrap();
        let dispatched = store
            .dispatch("post-upgrade", &fresh.attempt.attempt_id, 8)
            .unwrap();
        drop(store);
        let mut reopened = JobStore::open(&path).unwrap();
        reopened.bind_host_lease("normal-restart").unwrap();
        let changed = reopened.reconcile_host_restart().unwrap();
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].request.job_id, "post-upgrade");
        assert!(!changed[0].version_stopped);
        assert_eq!(changed[0].executions, dispatched.executions);
        assert_eq!(reopened.get("running").unwrap(), migrated);
        assert!(
            reopened
                .resume_granted("running", &migrated.attempt.attempt_id)
                .is_err()
        );
        assert!(reopened.retry("running").is_err());
    }

    #[test]
    fn settlement_binding_rejects_wrong_run_digest_later_attempt_and_reopened_owner() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.sqlite3");
        let mut store = JobStore::open(&path).unwrap();
        let reserved = store.reserve(request("binding")).unwrap();
        let dispatched = store
            .dispatch("binding", &reserved.attempt.attempt_id, 8)
            .unwrap();
        let ownership = dispatched.executions[0].clone();
        let mut wrong = ownership.clone();
        wrong.run_id = "foreign-run".into();
        assert!(
            store
                .check_execution_binding(
                    "binding",
                    &reserved.attempt.attempt_id,
                    &reserved.input_digest,
                    &wrong
                )
                .is_err()
        );
        assert!(
            store
                .check_execution_binding(
                    "binding",
                    &reserved.attempt.attempt_id,
                    "foreign-digest",
                    &ownership
                )
                .is_err()
        );
        assert_eq!(store.get("binding").unwrap(), dispatched);
        store
            .finish(
                "binding",
                &reserved.attempt.attempt_id,
                JobAttemptState::Failed,
                Some("known failure"),
            )
            .unwrap();
        store
            .confirm_execution_cleanup("binding", &ownership, "fixture-owned-run-joined")
            .unwrap();
        let next = store.retry("binding").unwrap();
        assert!(
            store
                .check_execution_binding(
                    "binding",
                    &reserved.attempt.attempt_id,
                    &reserved.input_digest,
                    &ownership
                )
                .is_err()
        );
        assert_eq!(store.get("binding").unwrap(), next);
        let current = store
            .dispatch("binding", &next.attempt.attempt_id, 8)
            .unwrap();
        drop(store);
        let reopened = JobStore::open(&path).unwrap();
        assert!(
            reopened
                .check_execution_binding(
                    "binding",
                    &next.attempt.attempt_id,
                    &next.input_digest,
                    &current.executions[1]
                )
                .is_err()
        );
        assert_eq!(reopened.get("binding").unwrap(), current);
    }

    #[test]
    fn reopening_preserves_current_schema_dispatched_attempt_without_cutover() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.sqlite3");
        let mut store = JobStore::open(&path).unwrap();
        let reserved = store.reserve(request("current")).unwrap();
        let dispatched = store
            .dispatch("current", &reserved.attempt.attempt_id, 8)
            .unwrap();
        drop(store);
        let reopened = JobStore::open(&path).unwrap();
        assert_eq!(reopened.get("current").unwrap(), dispatched);
        assert!(
            !reopened
                .conn
                .prepare("SELECT 1 FROM job_version_history")
                .unwrap()
                .exists([])
                .unwrap()
        );
        assert!(!reopened.get("current").unwrap().version_stopped);
    }
}
