use super::*;

/// Frozen schema-1/2 adapter. Preserve original intent and attempt bindings
/// before replacing counters with identities derived from immutable input.
pub(super) fn migrate_legacy_jobs(conn: &Connection) -> Result<(), JobStoreError> {
    conn.execute_batch(
        "CREATE TABLE legacy_job_archive (
            job_id TEXT PRIMARY KEY NOT NULL,
            request_json TEXT NOT NULL,
            fingerprint TEXT NOT NULL,
            attempts_json TEXT NOT NULL
        );
        DROP TRIGGER job_intent_immutable;
        DROP TRIGGER job_attempt_immutable;",
    )?;
    let intents = conn
        .prepare("SELECT job_id, request_json, fingerprint FROM job_intents")?
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (id, original, fingerprint) in intents {
        let attempts = conn.prepare(
            "SELECT number, attempt_id, input_revision, state, result_json, result_digest, failure
             FROM job_attempts WHERE job_id = ?1 ORDER BY number",
        )?.query_map([&id], |r| Ok(serde_json::json!({
            "number":r.get::<_, u8>(0)?, "attempt_id":r.get::<_, String>(1)?,
            "input_revision":r.get::<_, String>(2)?, "state":r.get::<_, String>(3)?,
            "result_json":r.get::<_, Option<String>>(4)?, "result_digest":r.get::<_, Option<String>>(5)?,
            "failure":r.get::<_, Option<String>>(6)?
        })))?.collect::<Result<Vec<_>, _>>()?;
        conn.execute(
            "INSERT INTO legacy_job_archive VALUES (?1, ?2, ?3, ?4)",
            params![id, original, fingerprint, serde_json::to_string(&attempts)?],
        )?;
        let mut value: Value = serde_json::from_str(&original)?;
        value
            .as_object_mut()
            .ok_or_else(|| JobStoreError::Corrupt("legacy request is not an object".into()))?
            .remove("input_revision");
        let request: JobRequest = serde_json::from_value(value)?;
        if request.job_id != id {
            return Err(JobStoreError::Corrupt(
                "legacy request Job id mismatch".into(),
            ));
        }
        conn.execute(
            "UPDATE job_intents SET request_json = ?2, fingerprint = ?3 WHERE job_id = ?1",
            params![id, serde_json::to_string(&request)?, request.fingerprint()?],
        )?;
        conn.execute(
            "UPDATE job_attempts SET input_revision = ?2 WHERE job_id = ?1",
            params![id, request.input_digest()?],
        )?;
    }
    conn.execute_batch(
        "ALTER TABLE job_attempts RENAME COLUMN input_revision TO input_digest;
        CREATE TRIGGER job_intent_immutable BEFORE UPDATE OF
            job_id, request_json, fingerprint, serialization_key, max_concurrent ON job_intents
        BEGIN SELECT RAISE(ABORT, 'immutable Job intent'); END;
        CREATE TRIGGER job_attempt_immutable BEFORE UPDATE OF
            job_id, number, attempt_id, input_digest ON job_attempts
        BEGIN SELECT RAISE(ABORT, 'immutable Job attempt identity'); END;
        CREATE TRIGGER legacy_job_archive_immutable BEFORE UPDATE ON legacy_job_archive
        BEGIN SELECT RAISE(ABORT, 'immutable legacy Job archive'); END;
        CREATE TRIGGER legacy_job_archive_retained BEFORE DELETE ON legacy_job_archive
        BEGIN SELECT RAISE(ABORT, 'retained legacy Job archive'); END;",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request(id: &str) -> JobRequest {
        JobRequest {
            job_id: id.into(),
            purpose: "migration".into(),
            input_ref: "fixture:input".into(),
            input: json!({"immutable":[1,2,3]}),
            instruction: "Return a result".into(),
            profile: "builtin:job".into(),
            serialization_key: None,
            limits: Default::default(),
        }
    }

    fn seed(path: &Path, version: i64) -> (String, String, String) {
        let mut conn = Connection::open(path).unwrap();
        feature_storage::configure_connection(&conn).unwrap();
        let tx = conn.transaction().unwrap();
        tx.execute_batch(&SCHEMA.replace("input_digest", "input_revision"))
            .unwrap();
        if version == 2 {
            tx.execute_batch(GRANT_SCHEMA).unwrap();
        }
        let mut original = serde_json::to_value(request("done")).unwrap();
        original["input_revision"] = json!("old-token");
        let original = serde_json::to_string_pretty(&original).unwrap();
        let receipt = "{\"accepted\":true,\"input_revision\":\"receipt-domain-data\"}".to_string();
        let digest = job::result_digest(
            &serde_json::from_str(&receipt).unwrap(),
            job::ABSOLUTE_MAX_RESULT_BYTES,
        )
        .unwrap()
        .0;
        tx.execute("INSERT INTO job_intents (job_id,request_json,fingerprint,max_concurrent,state,current_attempt,acknowledged)
                    VALUES ('done',?1,'legacy-fingerprint',8,'completed',2,1)", [&original]).unwrap();
        tx.execute(
            "INSERT INTO job_attempts (job_id,number,attempt_id,input_revision,state,failure)
                    VALUES ('done',1,'done:attempt:1','old-token','failed','timeout')",
            [],
        )
        .unwrap();
        tx.execute("INSERT INTO job_attempts (job_id,number,attempt_id,input_revision,state,result_json,result_digest)
                    VALUES ('done',2,'done:attempt:2','old-token','completed',?1,?2)", params![receipt,digest]).unwrap();
        let grant = "{\"subject_id\":\"local-subject\",\"candidate_ids\":[\"c1\"]}".to_string();
        if version == 2 {
            tx.execute("INSERT INTO job_domain_grants VALUES ('done',?1)", [&grant])
                .unwrap();
        }
        for (id, state) in [("pending", "reserved"), ("running", "dispatched")] {
            let mut value = serde_json::to_value(request(id)).unwrap();
            value["input_revision"] = json!("old-token");
            tx.execute("INSERT INTO job_intents (job_id,request_json,fingerprint,max_concurrent,state,current_attempt)
                        VALUES (?1,?2,'legacy-fingerprint',8,'pending',1)", params![id,serde_json::to_string(&value).unwrap()]).unwrap();
            tx.execute(
                "INSERT INTO job_attempts (job_id,number,attempt_id,input_revision,state)
                        VALUES (?1,1,?2,'old-token',?3)",
                params![id, job::attempt_id(id, 1), state],
            )
            .unwrap();
        }
        tx.pragma_update(None, "user_version", version).unwrap();
        tx.commit().unwrap();
        (original, receipt, grant)
    }

    #[test]
    fn legacy_versions_preserve_intent_receipts_attempts_grants_and_replay() {
        for version in [1, 2] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("jobs.sqlite3");
            let (original, receipt, grant) = seed(&path, version);
            let mut store = JobStore::open(&path).unwrap();
            let grant_value =
                (version == 2).then(|| serde_json::from_str::<Value>(&grant).unwrap());
            let snapshot = store
                .reserve_granted(request("done"), grant_value.clone())
                .unwrap();
            assert!(snapshot.acknowledged);
            assert_eq!(snapshot.state, JobState::Completed);
            assert_eq!(snapshot.attempt.number, 2);
            assert_eq!(
                snapshot.result,
                Some(serde_json::from_str(&receipt).unwrap())
            );
            let replay = JobResultSubmission {
                job_id: "done".into(),
                attempt_id: snapshot.attempt.attempt_id.clone(),
                input_digest: request("done").input_digest().unwrap(),
                result: snapshot.result.clone().unwrap(),
            };
            assert_eq!(store.accept(&replay).unwrap(), snapshot);
            let mut stale = replay.clone();
            stale.attempt_id = "done:attempt:1".into();
            assert!(matches!(
                store.accept(&stale),
                Err(JobStoreError::AttemptMismatch)
            ));
            let mut changed = request("done");
            changed.input = json!({"different":true});
            assert!(matches!(
                store.reserve_granted(changed, grant_value.clone()),
                Err(JobStoreError::IntentConflict(_))
            ));
            let archived: (String, String, String) = store.conn.query_row(
                "SELECT request_json,fingerprint,attempts_json FROM legacy_job_archive WHERE job_id='done'", [],
                |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
            assert_eq!(archived.0, original);
            assert_eq!(archived.1, "legacy-fingerprint");
            let attempts: Value = serde_json::from_str(&archived.2).unwrap();
            assert_eq!(attempts[0]["failure"], "timeout");
            assert_eq!(attempts[1]["input_revision"], "old-token");
            assert_eq!(attempts[1]["result_json"], receipt);
            if version == 2 {
                let persisted: String = store
                    .conn
                    .query_row("SELECT grant_json FROM job_domain_grants", [], |r| r.get(0))
                    .unwrap();
                assert_eq!(persisted, grant);
            }
            let digests: Vec<String> = store
                .conn
                .prepare(
                    "SELECT input_digest FROM job_attempts WHERE job_id='done' ORDER BY number",
                )
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            assert_eq!(digests, vec![request("done").input_digest().unwrap(); 2]);
            assert_eq!(store.get("running").unwrap().state, JobState::Unknown);
            assert_eq!(
                store.reserve(request("pending")).unwrap().attempt.state,
                JobAttemptState::Reserved
            );
            assert!(
                store
                    .conn
                    .execute_batch("DELETE FROM legacy_job_archive")
                    .is_err()
            );
            assert!(
                store
                    .conn
                    .execute_batch("UPDATE legacy_job_archive SET fingerprint='changed'")
                    .is_err()
            );
            assert!(
                store
                    .conn
                    .execute_batch("UPDATE job_intents SET fingerprint='changed'")
                    .is_err()
            );
            assert!(
                store
                    .conn
                    .execute_batch("UPDATE job_attempts SET input_digest='changed'")
                    .is_err()
            );
            assert!(
                !store
                    .conn
                    .prepare("PRAGMA foreign_key_check")
                    .unwrap()
                    .exists([])
                    .unwrap()
            );
            drop(store);
            let mut store = JobStore::open(&path).unwrap();
            assert_eq!(
                store.reserve_granted(request("done"), grant_value).unwrap(),
                snapshot
            );
        }
    }

    #[test]
    fn failed_migration_rolls_back_schema_archive_and_immutable_triggers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.sqlite3");
        seed(&path, 2);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "DROP TRIGGER job_intent_immutable;
            UPDATE job_intents SET request_json='{}' WHERE job_id='running';
            CREATE TRIGGER job_intent_immutable BEFORE UPDATE OF request_json ON job_intents
            BEGIN SELECT RAISE(ABORT,'immutable'); END;",
        )
        .unwrap();
        drop(conn);
        assert!(JobStore::open(&path).is_err());
        let conn = Connection::open(&path).unwrap();
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert!(conn.prepare("SELECT * FROM legacy_job_archive").is_err());
        let binding: String = conn
            .query_row(
                "SELECT input_revision FROM job_attempts WHERE job_id='done' AND number=2",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(binding, "old-token");
        assert!(
            conn.execute_batch("UPDATE job_intents SET request_json='{}' WHERE job_id='done'")
                .is_err()
        );
        assert!(
            conn.execute_batch("UPDATE job_attempts SET input_revision='changed'")
                .is_err()
        );
    }
}
