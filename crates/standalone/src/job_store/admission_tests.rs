//! Actual v7 guard upgrade: rollback/retry, no cutover, exact values and fresh
//! schema equality. Temporary SQLite only; this is not an operating DB migration.
use super::*;

#[test]
fn granted_admission_guard_upgrade_rolls_back_retries_preserves_reservations_and_reopens() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("jobs.sqlite3");
    let mut store = JobStore::open(&path).unwrap();
    let mut request = super::super::tests::request("granted-refusal");
    request.serialization_key = None;
    let granted = store
        .reserve_granted(request.clone(), Some(serde_json::json!({"exact":true})))
        .unwrap();
    request.job_id = "prior-execution".into();
    let pending = store.reserve(request).unwrap();
    let dispatched = store
        .dispatch(&pending.request.job_id, &pending.attempt.attempt_id, 8)
        .unwrap();
    drop(store);
    let mut conn = Connection::open(&path).unwrap();
    feature_storage::configure_connection(&conn).unwrap();
    // v8 has no table/row changes. Restore the actual immutable v7 guard text.
    let old_guard = &START_REJECTION_SCHEMA[START_REJECTION_SCHEMA
        .find("CREATE TRIGGER job_start_rejection_guard")
        .unwrap()
        ..START_REJECTION_SCHEMA
            .find("CREATE TRIGGER job_start_rejection_immutable")
            .unwrap()];
    conn.execute_batch("DROP TRIGGER job_start_rejection_guard")
        .unwrap();
    conn.execute_batch(old_guard).unwrap();
    conn.pragma_update(None, "user_version", 7).unwrap();
    conn.execute_batch("CREATE TABLE migration_fault(value TEXT REFERENCES job_intents(job_id))")
        .unwrap();
    conn.pragma_update(None, "foreign_keys", false).unwrap();
    conn.execute("INSERT INTO migration_fault VALUES('missing')", [])
        .unwrap();
    conn.pragma_update(None, "foreign_keys", true).unwrap();
    assert!(
        initialize(&mut conn).is_err(),
        "actual FK check after DDL must abort the transaction"
    );
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        7
    );
    let guard: String = conn
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE name='job_start_rejection_guard'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        guard.contains("result-only"),
        "failed migration must restore old guard"
    );
    assert_eq!(
        require_snapshot(&conn, &granted.request.job_id).unwrap(),
        granted
    );
    assert_eq!(
        require_snapshot(&conn, &dispatched.request.job_id).unwrap(),
        dispatched
    );
    conn.execute_batch("DROP TABLE migration_fault").unwrap();
    initialize(&mut conn).unwrap();
    assert_eq!(
        require_snapshot(&conn, &granted.request.job_id).unwrap(),
        granted
    );
    assert_eq!(
        require_snapshot(&conn, &dispatched.request.job_id).unwrap(),
        dispatched
    );
    assert_eq!(
        conn.query_row("SELECT count(*) FROM job_version_stops", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    let schema = |conn: &Connection| {
        conn.prepare(
            "SELECT type,name,sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY type,name",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
    };
    let fresh = JobStore::open(&temp.path().join("fresh.sqlite3")).unwrap();
    assert_eq!(schema(&conn), schema(&fresh.conn));
    drop(conn);
    let mut reopened = JobStore::open(&path).unwrap();
    reopened
        .fail_reserved_granted(&granted, "permanent configuration refusal")
        .unwrap();
    let failed = reopened.get(&granted.request.job_id).unwrap();
    assert_eq!(
        failed.attempt.failure.as_deref(),
        Some("permanent configuration refusal")
    );
    assert_eq!(failed.request, granted.request);
    assert_eq!(failed.domain_grant, granted.domain_grant);
    assert!(failed.start_rejected_by.is_some());
    assert!(failed.executions.is_empty());
    assert_eq!(
        reopened.get(&dispatched.request.job_id).unwrap(),
        dispatched
    );
    assert!(!dispatched.executions_released());
    assert!(reopened.conn.execute("INSERT INTO job_start_rejections(job_id,attempt_id,owner_id) VALUES(?1,?2,'not-an-owner')",
        params![dispatched.request.job_id, dispatched.attempt.attempt_id]).is_err());
    drop(reopened);
    let reopened = JobStore::open(&path).unwrap();
    assert_eq!(reopened.get(&granted.request.job_id).unwrap(), failed);
    assert_eq!(
        reopened.get(&dispatched.request.job_id).unwrap(),
        dispatched
    );
}
