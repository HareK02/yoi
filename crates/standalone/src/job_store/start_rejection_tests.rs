//! Admission evidence tests use the production store/initializer and real SQLite.
use super::*;

fn request(id: &str) -> JobRequest {
    JobRequest {
        job_id: id.into(),
        purpose: "rejection fixture".into(),
        input_ref: "fixture:input".into(),
        input: serde_json::json!({"body":"retained"}),
        instruction: "Return result".into(),
        profile: "builtin:job".into(),
        serialization_key: None,
        limits: job::JobLimits::default(),
    }
}

#[test]
fn start_rejection_is_atomic_and_failed_or_missing_execution_never_implies_it() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("jobs.sqlite3");
    let mut store = JobStore::open(&path).unwrap();
    let saved = store.reserve(request("reject")).unwrap();
    let attempt = &saved.attempt.attempt_id;
    store
        .conn
        .execute_batch(
            "CREATE TRIGGER refuse_failure BEFORE UPDATE OF state ON job_attempts
        WHEN NEW.state='failed' BEGIN SELECT RAISE(ABORT,'injected terminal write failure'); END;",
        )
        .unwrap();
    assert!(
        store
            .fail_reserved_result("reject", attempt, "configuration unavailable")
            .is_err()
    );
    assert_eq!(store.get("reject").unwrap(), saved);
    assert_eq!(
        store
            .conn
            .query_row("SELECT COUNT(*) FROM job_start_rejections", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    store
        .conn
        .execute_batch("DROP TRIGGER refuse_failure")
        .unwrap();
    store
        .fail_reserved_result("reject", attempt, "configuration unavailable")
        .unwrap();
    let rejected = store.get("reject").unwrap();
    assert_eq!(
        rejected.attempt.failure.as_deref(),
        Some("configuration unavailable")
    );
    assert_eq!(
        rejected.start_rejected_by.as_deref(),
        Some(store.owner_id.as_str())
    );
    assert!(rejected.executions.is_empty());
    assert!(rejected.executions_released());
    assert!(store.execution_obligations().unwrap().is_empty());
    for sql in [
        "DELETE FROM job_start_rejections",
        "UPDATE job_start_rejections SET owner_id='other'",
    ] {
        assert!(store.conn.execute_batch(sql).is_err());
    }
    let bare = store.reserve(request("bare-failed")).unwrap();
    store
        .finish(
            "bare-failed",
            &bare.attempt.attempt_id,
            JobAttemptState::Failed,
            Some("not admission evidence"),
        )
        .unwrap();
    assert!(!store.get("bare-failed").unwrap().executions_released());
    assert!(
        store
            .conn
            .execute(
                "INSERT INTO job_start_rejections VALUES('bare-failed',?1,'forged')",
                [&bare.attempt.attempt_id]
            )
            .is_err()
    );
    assert!(store.execution_obligations().is_err());
    let running = store.reserve(request("running")).unwrap();
    store
        .dispatch("running", &running.attempt.attempt_id, 8)
        .unwrap();
    assert!(
        store
            .fail_reserved_result("running", &running.attempt.attempt_id, "late failure")
            .is_err()
    );
    let running = store.get("running").unwrap();
    assert!(running.start_rejected_by.is_none());
    assert!(!running.executions_released());
    drop(store);
    let reopened = JobStore::open(&path).unwrap();
    assert_eq!(reopened.get("reject").unwrap(), rejected);
    assert_eq!(reopened.get("running").unwrap(), running);
    assert!(reopened.execution_obligations().is_err());
}

#[test]
fn start_rejection_migration_adds_only_evidence_and_rolls_back_then_reopens_without_cutover() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("jobs.sqlite3");
    let mut conn = Connection::open(&path).unwrap();
    feature_storage::configure_connection(&conn).unwrap();
    conn.execute_batch(SCHEMA).unwrap();
    conn.execute_batch(GRANT_SCHEMA).unwrap();
    conn.execute_batch(RECONCILIATION_SCHEMA).unwrap();
    conn.execute_batch(recovery::SCHEMA).unwrap();
    conn.pragma_update(None, "user_version", 6).unwrap();
    let req = request("already-reserved");
    let tx = conn.transaction().unwrap();
    tx.execute("INSERT INTO job_intents(job_id,request_json,fingerprint,max_concurrent,state,current_attempt,acknowledged)
        VALUES(?1,?2,?3,8,'pending',1,0)", params![req.job_id,serde_json::to_string(&req).unwrap(),req.fingerprint().unwrap()]).unwrap();
    insert_attempt(&tx, &req, 1).unwrap();
    tx.commit().unwrap();
    conn.execute_batch("CREATE TRIGGER job_start_rejection_guard BEFORE UPDATE ON job_intents BEGIN SELECT 1; END;").unwrap();
    assert!(recovery::initialize(&mut conn).is_err());
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        6
    );
    assert!(
        !conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='job_start_rejections')",
                [],
                |r| r.get::<_, bool>(0)
            )
            .unwrap()
    );
    conn.execute_batch("DROP TRIGGER job_start_rejection_guard")
        .unwrap();
    recovery::initialize(&mut conn).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM job_start_rejections", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM job_version_stops", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    drop(conn);
    let mut store = JobStore::open(&path).unwrap();
    let saved = store.get(&req.job_id).unwrap();
    assert_eq!(saved.request, req);
    assert_eq!(saved.state, JobState::Pending);
    assert_eq!(saved.attempt.state, JobAttemptState::Reserved);
    assert!(!saved.version_stopped);
    assert!(saved.start_rejected_by.is_none());
    store
        .fail_reserved_result(
            &req.job_id,
            &saved.attempt.attempt_id,
            "unavailable profile",
        )
        .unwrap();
    let rejected = store.get(&req.job_id).unwrap();
    drop(store);
    let reopened = JobStore::open(&path).unwrap();
    assert_eq!(reopened.get(&req.job_id).unwrap(), rejected);
    assert!(reopened.execution_obligations().unwrap().is_empty());
    let fresh = JobStore::open(&temp.path().join("fresh.sqlite3")).unwrap();
    let schema = |conn: &Connection| {
        conn.prepare(
            "SELECT type,name,sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY type,name",
        )
        .unwrap()
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
    };
    assert_eq!(schema(&reopened.conn), schema(&fresh.conn));
}
