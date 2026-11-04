//! Component tests use retained schema-113 DDL and real SQLite transactions.
//! Runner tests use the store's actual plan/apply/open paths, not a second ledger.
//! They run in the ordinary workspace-server lib target once the root registers
//! this module AND the migration. No Runtime/process termination or external
//! Journal checkpoint completion is simulated or claimed by these tests.
use super::*;
use crate::backend_job::BackendJobRequest;
use crate::store::{self as store_owner, ControlPlaneStore};
use rusqlite::{TransactionBehavior, params, types::Value};
use std::path::Path;

const JOB_VALUES: &str = "workspace_id,job_id,purpose,input_digest,input_ref,request_json,intent_fingerprint,current_attempt,result_json,result_digest,created_at,updated_at,completed_at";
const ATTEMPT_VALUES: &str = "workspace_id,job_id,attempt_id,attempt,input_digest,runtime_id,worker_id,runtime_run_id,dispatched_at,deadline_at,result_json,result_digest,worker_cleanup_state,worker_cleanup_failure_category,worker_cleanup_failure_detail,worker_cleanup_updated_at,worker_cleanup_completed_at,created_at,updated_at,completed_at";

fn rows(conn: &Connection, sql: &str) -> Vec<Vec<Value>> {
    let mut statement = conn.prepare(sql).unwrap();
    let count = statement.column_count();
    statement
        .query_map([], |row| {
            (0..count)
                .map(|i| row.get(i))
                .collect::<rusqlite::Result<Vec<Value>>>()
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

fn request(id: &str) -> BackendJobRequest {
    BackendJobRequest {
        job_id: id.into(),
        purpose: "upgrade_fixture".into(),
        input_ref: format!("saved://{id}"),
        input: serde_json::json!({"body":"immutable input","selection":[1,2]}),
        instruction: "Use only the saved input".into(),
        profile: "builtin:backend-job".into(),
        grants: Default::default(),
        serialization_key: None,
        source_worker: None,
        notification_target: None,
        limits: crate::backend_job::BackendJobLimits {
            max_attempts: 3,
            ..Default::default()
        },
    }
}

fn seed_workspace(conn: &Connection) {
    conn.execute_batch("INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at)
        VALUES('upgrade-owner','user','upgrade-owner','Owner','1','1');
        INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at)
        VALUES('upgrade-workspace','upgrade-owner','Workspace','active','1','1');").unwrap();
}

fn insert_job(conn: &Connection, id: &str, state: &str) {
    let request = request(id);
    let legacy = store_owner::column_exists(conn, "backend_jobs", "input_revision").unwrap();
    let input_column = if legacy {
        "input_revision"
    } else {
        "input_digest"
    };
    let mut encoded = serde_json::to_value(&request).unwrap();
    let input_identity = if legacy {
        let legacy_token = format!("legacy-token:{id}");
        encoded["input_revision"] = serde_json::Value::String(legacy_token.clone());
        legacy_token
    } else {
        request.input_digest().unwrap()
    };
    let fingerprint = if legacy {
        format!("legacy-fingerprint:{id}")
    } else {
        request.fingerprint().unwrap()
    };
    // Retain noncanonical whitespace too, not merely a reserialized body value.
    let encoded = if legacy {
        serde_json::to_string_pretty(&encoded).unwrap()
    } else {
        serde_json::to_string(&request).unwrap()
    };
    conn.execute(&format!("INSERT INTO backend_jobs(workspace_id,job_id,purpose,{input_column},input_ref,request_json,intent_fingerprint,state,current_attempt,result_json,result_digest,failure_category,failure_detail,created_at,updated_at,completed_at)
        VALUES('upgrade-workspace',?1,?2,?3,?4,?5,?6,?7,1,?8,?9,'old-cause','old detail','created','updated',?10)"),
        params![id, request.purpose, input_identity, request.input_ref,
            encoded, fingerprint, state,
            (state=="completed").then_some("{\"saved\":true}"),
            (state=="completed").then_some("result-digest"),
            (state=="completed" || state=="failed" || state=="unknown").then_some("old-end")]).unwrap();
}

fn insert_attempt(conn: &Connection, job: &str, id: &str, number: i64, state: &str) {
    let legacy =
        store_owner::column_exists(conn, "backend_job_attempts", "input_revision").unwrap();
    let input_column = if legacy {
        "input_revision"
    } else {
        "input_digest"
    };
    let input_identity = if legacy {
        format!("legacy-token:{job}")
    } else {
        "attempt-digest".into()
    };
    let ddl: String = conn
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE name='backend_job_attempts'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let state = if state == "dispatching" && !ddl.contains("'dispatching'") {
        "dispatched"
    } else {
        state
    };
    conn.execute(&format!("INSERT INTO backend_job_attempts(workspace_id,job_id,attempt_id,attempt,{input_column},state,runtime_id,worker_id,runtime_run_id,dispatched_at,deadline_at,result_json,result_digest,failure_category,failure_detail,created_at,updated_at,completed_at)
        VALUES('upgrade-workspace',?1,?2,?3,?12,?4,?5,?6,?7,?8,'deadline',?9,?10,'old-attempt-cause','old attempt detail','created','updated',?11)"),
        params![job,id,number,state,
            (state!="reserved").then_some(format!("runtime-{id}")),
            (state!="reserved").then_some(format!("worker-{id}")),
            (state=="dispatched").then_some(format!("run-{id}")),
            (state=="dispatched").then_some("dispatched-at"),
            (state=="completed").then_some("{\"saved_attempt\":true}"),
            (state=="completed").then_some("attempt-result-digest"),
            (state=="completed" || state=="failed" || state=="unknown").then_some("old-end"),input_identity]).unwrap();
}

fn seed_boundary(conn: &Connection) {
    seed_workspace(conn);
    for (job, state, attempt_states) in [
        ("unstarted", "pending", vec!["reserved"]),
        ("running", "pending", vec!["dispatched", "dispatching"]),
        ("uncertain", "unknown", vec!["unknown"]),
        (
            "partial-journal",
            "pending",
            vec!["completed", "failed", "reserved"],
        ),
        ("checkpointed-journal", "pending", vec!["dispatched"]),
        // Even inconsistent unfinished historical attempts under terminal Jobs
        // are retired; a current_attempt-only sweep would miss these rows.
        ("completed", "completed", vec!["completed", "reserved"]),
        ("failed", "failed", vec!["failed", "dispatched"]),
    ] {
        insert_job(conn, job, state);
        for (index, attempt_state) in attempt_states.iter().enumerate() {
            insert_attempt(
                conn,
                job,
                &format!("{job}-{index}"),
                index as i64 + 1,
                attempt_state,
            );
        }
    }
    conn.execute_batch("UPDATE backend_jobs SET current_attempt=3 WHERE job_id='partial-journal';")
        .unwrap();
    if store_owner::column_exists(conn, "backend_job_attempts", "worker_cleanup_state").unwrap() {
        conn.execute_batch("UPDATE backend_job_attempts SET worker_cleanup_state='executing',worker_cleanup_failure_category='saved-cleanup-cause',worker_cleanup_failure_detail='saved cleanup detail',worker_cleanup_updated_at='cleanup-updated' WHERE attempt_id='running-0';").unwrap();
    }
    let delivery_ddl: String = conn
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE name='backend_job_deliveries'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    for (index, state) in ["pending", "sending", "completed", "failed", "unknown"]
        .iter()
        .enumerate()
    {
        if !delivery_ddl.contains(&format!("'{state}'")) {
            continue;
        }
        conn.execute("INSERT INTO backend_job_deliveries(workspace_id,delivery_id,job_id,attempt_id,target_runtime_id,target_worker_id,state,failure_category,failure_detail,created_at,updated_at,delivered_at)
            VALUES('upgrade-workspace',?1,'running','running-0','target-runtime','target-worker',?2,'old-delivery-cause','old delivery detail','created','updated',?3)",
            params![format!("delivery-{index}"),state,(*state=="completed").then_some("delivered")]).unwrap();
    }
    // Preserve explicit erasure without copying the old body into history.
    if store_owner::column_exists(conn, "backend_jobs", "input_disposition").unwrap() {
        conn.execute_batch("UPDATE backend_jobs SET input_disposition='erased',request_json=json_set(request_json,'$.input',json('null')) WHERE job_id='failed';").unwrap();
    } else {
        // A legacy body already absent before capture must never regenerate.
        conn.execute_batch("UPDATE backend_jobs SET request_json=json_set(request_json,'$.input',json('null')) WHERE job_id='failed';").unwrap();
    }
}

const PRE_CONTENT_JOB_VALUES: &str = "workspace_id,job_id,purpose,input_revision,input_ref,resource_key,request_json,intent_fingerprint,current_attempt,state,result_digest,failure_category,failure_detail,updated_at,completed_at";
const PRE_CONTENT_ATTEMPT_VALUES: &str = "workspace_id,job_id,attempt_id,attempt,input_revision,state,runtime_id,worker_id,runtime_run_id,dispatched_at,deadline_at,result_digest,failure_category,failure_detail,worker_cleanup_state,worker_cleanup_failure_category,worker_cleanup_failure_detail,worker_cleanup_updated_at,worker_cleanup_completed_at,updated_at,completed_at";
const PRE_CONTENT_DELIVERY_VALUES: &str = "workspace_id,delivery_id,job_id,attempt_id,target_runtime_id,target_worker_id,state,failure_category,failure_detail,updated_at,delivered_at";

fn legacy_fixture_to_85(path: &Path, baseline: i64) -> Connection {
    if baseline <= 51 {
        store_owner::tests::prepare_schema_v50(path, None);
    } else {
        store_owner::tests::prepare_retained_schema(path, baseline);
    }
    let conn = Connection::open(path).unwrap();
    store_owner::configure_sqlite(&conn).unwrap();
    let current = store_owner::current_schema_version(&conn).unwrap();
    // The oldest baseline has no Job tables. Admit rows at the first REAL Job
    // step (70), keeping the original 50/51/... receipt prefix intact.
    for step in store_owner::MIGRATIONS
        .iter()
        .filter(|step| step.version > current && step.version <= 70)
    {
        (step.apply)(&conn).unwrap();
    }
    seed_boundary(&conn);
    let current = store_owner::current_schema_version(&conn).unwrap();
    for step in store_owner::MIGRATIONS
        .iter()
        .filter(|step| step.version > current && step.version <= 85)
    {
        (step.apply)(&conn).unwrap();
    }
    assert_eq!(store_owner::current_schema_version(&conn).unwrap(), 85);
    conn
}

fn pre_content_values(conn: &Connection, raw: bool) -> Vec<Vec<Vec<Value>>> {
    [
        (
            "backend_jobs",
            "backend_job_pre_content_history",
            PRE_CONTENT_JOB_VALUES,
            "job_id",
        ),
        (
            "backend_job_attempts",
            "backend_job_attempt_pre_content_history",
            PRE_CONTENT_ATTEMPT_VALUES,
            "attempt_id",
        ),
        (
            "backend_job_deliveries",
            "backend_job_delivery_pre_content_history",
            PRE_CONTENT_DELIVERY_VALUES,
            "delivery_id",
        ),
    ]
    .into_iter()
    .map(|(live, history, columns, key)| {
        rows(
            conn,
            &format!(
                "SELECT {columns} FROM {} ORDER BY {key}",
                if raw { history } else { live }
            ),
        )
    })
    .collect()
}

#[test]
fn explicit_capture_is_transactional_single_call_and_retirement_scrubs_raw_body() {
    let dir = tempfile::tempdir().unwrap();
    let conn = legacy_fixture_to_85(&dir.path().join("capture.sqlite"), 85);
    let original = pre_content_values(&conn, false);
    assert!(capture_before_content_cutover(&conn).is_err());
    {
        let tx = conn.unchecked_transaction().unwrap();
        capture_before_content_cutover(&tx).unwrap();
        assert_eq!(original, pre_content_values(&tx, true));
        let live = rows(
            &tx,
            "SELECT request_json FROM backend_jobs WHERE job_id='unstarted'",
        );
        assert!(tx.execute_batch("UPDATE backend_job_pre_content_history SET request_json=json_set(request_json,'$.input',json('null')),input_retired=1 WHERE job_id='unstarted'").is_err());
        tx.execute_batch("UPDATE backend_jobs SET request_json=json_set(request_json,'$.input',json('null')) WHERE job_id='unstarted'").unwrap();
        assert_eq!(
            rows(
                &tx,
                "SELECT input_retired,json_type(request_json,'$.input') FROM backend_job_pre_content_history WHERE job_id='unstarted'"
            ),
            vec![vec![Value::Integer(1), Value::Text("null".into())]]
        );
        let Value::Text(original_request) = &live[0][0] else {
            unreachable!()
        };
        assert!(
            tx.execute(
                "UPDATE backend_jobs SET request_json=?1 WHERE job_id='unstarted'",
                [original_request]
            )
            .is_err()
        );
        assert!(
            tx.execute_batch(
                "UPDATE backend_job_pre_content_history SET failure_category='rewritten'"
            )
            .is_err()
        );
        assert!(
            tx.execute_batch(
                "UPDATE backend_job_delivery_pre_content_history SET state='completed'"
            )
            .is_err()
        );
        assert!(capture_before_content_cutover(&tx).is_err());
        // Owner rolls back the WHOLE step after any failure, not just a subcall.
    }
    assert!(!store_owner::table_exists(&conn, "backend_job_pre_content_history").unwrap());
    assert_eq!(original, pre_content_values(&conn, false));
}

#[test]
fn intervening_cleanup_evidence_is_not_overwritten_by_original_value_restoration() {
    let dir = tempfile::tempdir().unwrap();
    let conn = legacy_fixture_to_85(&dir.path().join("intervening-evidence.sqlite"), 85);
    store_owner::migrate_content_state_v85_to_v86(&conn).unwrap();
    // Simulate a violation of the caller's migration-only ownership window.
    // The leaf must fail closed, preserving an actual later observation instead
    // of replacing it with NULL from the old capture.
    conn.execute_batch("UPDATE backend_job_attempts SET worker_cleanup_state='completed',worker_cleanup_completed_at='later-owner-evidence' WHERE attempt_id='running-0'").unwrap();
    let later = rows(
        &conn,
        "SELECT * FROM backend_job_attempts ORDER BY attempt_id",
    );
    assert!(apply_leaf(&conn).is_err());
    assert_eq!(
        later,
        rows(
            &conn,
            "SELECT * FROM backend_job_attempts ORDER BY attempt_id"
        )
    );
    assert!(!store_owner::table_exists(&conn, "backend_job_upgrade_history").unwrap());
}

#[test]
fn real_content_step_receipt_failure_rolls_back_raw_capture_then_retries_into_final_cutover() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy-receipt.sqlite");
    let conn = legacy_fixture_to_85(&path, 85);
    let original = pre_content_values(&conn, false);
    let jobs = rows(&conn, "SELECT * FROM backend_jobs ORDER BY job_id");
    let attempts = rows(
        &conn,
        "SELECT * FROM backend_job_attempts ORDER BY attempt_id",
    );
    let deliveries = rows(
        &conn,
        "SELECT * FROM backend_job_deliveries ORDER BY delivery_id",
    );
    conn.execute_batch("CREATE TRIGGER injected_content_receipt BEFORE INSERT ON __yoi_schema_migrations WHEN NEW.version=86 BEGIN SELECT RAISE(ABORT,'injected content receipt'); END;").unwrap();
    assert!(store_owner::migrate_content_state_v85_to_v86(&conn).is_err());
    assert_eq!(store_owner::current_schema_version(&conn).unwrap(), 85);
    assert!(!store_owner::table_exists(&conn, "backend_job_pre_content_history").unwrap());
    assert_eq!(
        jobs,
        rows(&conn, "SELECT * FROM backend_jobs ORDER BY job_id")
    );
    assert_eq!(
        attempts,
        rows(
            &conn,
            "SELECT * FROM backend_job_attempts ORDER BY attempt_id"
        )
    );
    assert_eq!(
        deliveries,
        rows(
            &conn,
            "SELECT * FROM backend_job_deliveries ORDER BY delivery_id"
        )
    );
    conn.execute_batch("DROP TRIGGER injected_content_receipt")
        .unwrap();
    store_owner::migrate_content_state_v85_to_v86(&conn).unwrap();
    assert_eq!(original, pre_content_values(&conn, true));
    // This is precisely the legacy vacuous failure that a final-state-only
    // sweep used to miss. Original cleanup/completed_at were both NULL.
    assert_eq!(
        rows(
            &conn,
            "SELECT state,failure_category FROM backend_jobs WHERE job_id='unstarted'"
        ),
        vec![vec![
            Value::Text("failed".into()),
            Value::Text("content_contract_cutover".into())
        ]]
    );
    let delivery_after_content = rows(
        &conn,
        "SELECT * FROM backend_job_deliveries ORDER BY delivery_id",
    );
    conn.execute_batch("CREATE TEMP TRIGGER injected_final_receipt BEFORE INSERT ON main.__yoi_schema_migrations WHEN EXISTS(SELECT 1 FROM sqlite_schema WHERE name='backend_job_upgrade_history') BEGIN SELECT RAISE(ABORT,'injected final receipt'); END;").unwrap();
    assert!(store_owner::apply_migrations(&conn).is_err());
    assert!(!store_owner::table_exists(&conn, "backend_job_upgrade_history").unwrap());
    assert_eq!(original, pre_content_values(&conn, true));
    conn.execute_batch("DROP TRIGGER injected_final_receipt")
        .unwrap();
    store_owner::apply_migrations(&conn).unwrap();
    assert_captured_cutover(&conn, &original, &delivery_after_content);
    drop(conn);
    drop(store_owner::SqliteWorkspaceStore::open_for_test(&path).unwrap());
    let conn = Connection::open(path).unwrap();
    assert_captured_cutover(&conn, &original, &delivery_after_content);
}

fn assert_captured_cutover(
    conn: &Connection,
    original: &[Vec<Vec<Value>>],
    deliveries: &[Vec<Value>],
) {
    verify(conn).unwrap();
    assert_eq!(original, pre_content_values(conn, true));
    assert_eq!(
        deliveries,
        rows(
            conn,
            "SELECT * FROM backend_job_deliveries ORDER BY delivery_id"
        )
    );
    assert_eq!(
        rows(
            conn,
            "SELECT state,failure_category,completed_at FROM backend_jobs WHERE job_id='unstarted'"
        ),
        vec![vec![
            Value::Text("failed".into()),
            Value::Text(FAILURE_CATEGORY.into()),
            Value::Null
        ]]
    );
    assert_eq!(
        rows(
            conn,
            "SELECT state,failure_category,completed_at,worker_cleanup_state,worker_cleanup_completed_at FROM backend_job_attempts WHERE attempt_id='unstarted-0'"
        ),
        vec![vec![
            Value::Text("failed".into()),
            Value::Text(FAILURE_CATEGORY.into()),
            Value::Null,
            Value::Null,
            Value::Null
        ]]
    );
    assert_eq!(
        rows(
            conn,
            "SELECT result_json,result_digest FROM backend_jobs WHERE job_id='completed'"
        ),
        vec![vec![
            Value::Text("{\"saved\":true}".into()),
            Value::Text("result-digest".into())
        ]]
    );
    assert_eq!(
        rows(
            conn,
            "SELECT result_json,result_digest FROM backend_job_attempts WHERE attempt_id='completed-0'"
        ),
        vec![vec![
            Value::Text("{\"saved_attempt\":true}".into()),
            Value::Text("attempt-result-digest".into())
        ]]
    );
    for table in ["backend_jobs", "backend_job_attempts"] {
        let invalid: bool = conn.query_row(&format!("SELECT EXISTS(SELECT 1 FROM {table} a JOIN {} r ON a.workspace_id=r.workspace_id AND a.job_id=r.job_id {} WHERE r.state IN ('completed','failed') AND (a.state IS NOT r.state OR a.failure_category IS NOT r.failure_category OR a.failure_detail IS NOT r.failure_detail OR a.result_digest IS NOT r.result_digest))",
            if table=="backend_jobs" {"backend_job_pre_content_history"} else {"backend_job_attempt_pre_content_history"},
            if table=="backend_job_attempts" {"AND a.attempt_id=r.attempt_id"} else {""}),[],|row|row.get(0)).unwrap();
        assert!(!invalid);
    }
}

#[test]
fn oldest_retained_row_fixtures_cross_the_real_content_hook_and_reopen_without_sweeping_new_jobs() {
    for baseline in 50..=85 {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oldest-with-rows.sqlite");
        let conn = legacy_fixture_to_85(&path, baseline);
        let original = pre_content_values(&conn, false);
        // Actual production step must call capture; this test does not call it.
        store_owner::migrate_content_state_v85_to_v86(&conn).unwrap();
        assert_eq!(original, pre_content_values(&conn, true));
        let deliveries = rows(
            &conn,
            "SELECT * FROM backend_job_deliveries ORDER BY delivery_id",
        );
        drop(conn);
        drop(store_owner::SqliteWorkspaceStore::open_for_test(&path).unwrap());
        let conn = Connection::open(&path).unwrap();
        store_owner::configure_sqlite(&conn).unwrap();
        assert_captured_cutover(&conn, &original, &deliveries);
        insert_job(&conn, "post-cutover", "pending");
        insert_attempt(&conn, "post-cutover", "post-cutover-attempt", 1, "reserved");
        let history = rows(
            &conn,
            "SELECT * FROM backend_job_upgrade_history ORDER BY job_id",
        );
        drop(conn);
        drop(store_owner::SqliteWorkspaceStore::open_for_test(&path).unwrap());
        let conn = Connection::open(&path).unwrap();
        assert_eq!(
            history,
            rows(
                &conn,
                "SELECT * FROM backend_job_upgrade_history ORDER BY job_id"
            )
        );
        assert_eq!(
            rows(
                &conn,
                "SELECT state FROM backend_jobs WHERE job_id='post-cutover'"
            ),
            vec![vec![Value::Text("pending".into())]]
        );
    }
}

#[test]
fn real_input_retirement_atomically_scrubs_raw_evidence_and_retries_after_failure() {
    use crate::backend_job::JobInputRetirementReason;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("retire-raw.sqlite");
    let conn = legacy_fixture_to_85(&path, 85);
    conn.execute_batch("UPDATE backend_jobs SET request_json=json_set(request_json,'$.input.body','unique-pre-content-body-to-erase') WHERE job_id='unstarted'").unwrap();
    store_owner::migrate_content_state_v85_to_v86(&conn).unwrap();
    drop(conn);
    let store = store_owner::SqliteWorkspaceStore::open_for_test(&path).unwrap();
    let job = store
        .get_backend_job("upgrade-workspace", "unstarted")
        .unwrap()
        .unwrap();
    let conn = Connection::open(&path).unwrap();
    store_owner::configure_sqlite(&conn).unwrap();
    let bodies = rows(
        &conn,
        "SELECT j.request_json,r.request_json FROM backend_jobs j JOIN backend_job_pre_content_history r USING(workspace_id,job_id) WHERE j.job_id='unstarted'",
    );
    conn.execute_batch("CREATE TRIGGER injected_raw_retirement BEFORE UPDATE OF request_json ON backend_job_pre_content_history WHEN OLD.job_id='unstarted' BEGIN SELECT RAISE(ABORT,'injected raw retirement'); END;").unwrap();
    assert!(
        store
            .reclaim_backend_job_input(
                "upgrade-workspace",
                "unstarted",
                &job.input_digest,
                JobInputRetirementReason::ExplicitErasure
            )
            .is_err()
    );
    assert_eq!(
        bodies,
        rows(
            &conn,
            "SELECT j.request_json,r.request_json FROM backend_jobs j JOIN backend_job_pre_content_history r USING(workspace_id,job_id) WHERE j.job_id='unstarted'"
        )
    );
    conn.execute_batch("DROP TRIGGER injected_raw_retirement")
        .unwrap();
    store
        .reclaim_backend_job_input(
            "upgrade-workspace",
            "unstarted",
            &job.input_digest,
            JobInputRetirementReason::ExplicitErasure,
        )
        .unwrap();
    assert_eq!(
        rows(
            &conn,
            "SELECT j.input_disposition,json_type(j.request_json,'$.input'),r.input_retired,json_type(r.request_json,'$.input') FROM backend_jobs j JOIN backend_job_pre_content_history r USING(workspace_id,job_id) WHERE j.job_id='unstarted'"
        ),
        vec![vec![
            Value::Text("erased".into()),
            Value::Text("null".into()),
            Value::Integer(1),
            Value::Text("null".into())
        ]]
    );
    verify(&conn).unwrap();
    drop(conn);
    drop(store);
    // The existing retirement owner performs secure_delete/VACUUM/checkpoint for
    // the WHOLE database, including raw evidence, not only the live Job table.
    let bytes = std::fs::read(&path).unwrap();
    assert!(
        !bytes
            .windows(b"unique-pre-content-body-to-erase".len())
            .any(|window| window == b"unique-pre-content-body-to-erase")
    );
    drop(store_owner::SqliteWorkspaceStore::open_for_test(&path).unwrap());
    let conn = Connection::open(&path).unwrap();
    assert_eq!(
        rows(
            &conn,
            "SELECT input_retired,json_type(request_json,'$.input') FROM backend_job_pre_content_history WHERE job_id='unstarted'"
        ),
        vec![vec![Value::Integer(1), Value::Text("null".into())]]
    );
}

fn retained_fixture(path: &Path) -> Connection {
    store_owner::tests::prepare_retained_schema(path, 113);
    let conn = Connection::open(path).unwrap();
    store_owner::configure_sqlite(&conn).unwrap();
    seed_boundary(&conn);
    conn
}

fn apply_leaf(conn: &Connection) -> Result<()> {
    conn.execute_batch("PRAGMA foreign_keys=OFF;")?;
    let result = (|| {
        let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Exclusive)?;
        install(&tx)?;
        tx.commit()?;
        Ok(())
    })();
    conn.execute_batch("PRAGMA foreign_keys=ON;")?;
    result
}

fn assert_retired(conn: &Connection) {
    verify(conn).unwrap();
    assert_eq!(
        rows(conn, "SELECT count(*) FROM backend_job_upgrade_history"),
        vec![vec![Value::Integer(7)]]
    );
    for (table, history, key) in [
        (
            "backend_jobs",
            "backend_job_upgrade_history",
            "a.job_id=h.job_id",
        ),
        (
            "backend_job_attempts",
            "backend_job_attempt_upgrade_history",
            "a.job_id=h.job_id AND a.attempt_id=h.attempt_id",
        ),
    ] {
        let violations: i64 = conn.query_row(&format!("SELECT count(*) FROM {table} a JOIN {history} h ON a.workspace_id=h.workspace_id AND {key}
            WHERE (h.state NOT IN ('completed','failed') AND (a.state!='failed' OR a.failure_category IS NOT ?1 OR a.failure_detail IS NOT ?2))
               OR (h.state IN ('completed','failed') AND (a.state!=h.state OR a.failure_category IS NOT h.failure_category OR a.failure_detail IS NOT h.failure_detail))"),
            [FAILURE_CATEGORY,FAILURE_DETAIL], |row| row.get(0)).unwrap();
        assert_eq!(violations, 0, "{table}");
    }
    assert_eq!(
        rows(
            conn,
            "SELECT state,failure_category,failure_detail,completed_at FROM backend_job_attempt_upgrade_history WHERE attempt_id='uncertain-0'"
        ),
        vec![vec![
            Value::Text("unknown".into()),
            Value::Text("old-attempt-cause".into()),
            Value::Text("old attempt detail".into()),
            Value::Text("old-end".into())
        ]]
    );
    // Failure did not manufacture execution/cleanup end or Journal completion.
    assert_eq!(
        rows(
            conn,
            "SELECT completed_at,worker_cleanup_state,worker_cleanup_completed_at FROM backend_job_attempts WHERE attempt_id='running-0'"
        ),
        vec![vec![
            Value::Null,
            Value::Text("executing".into()),
            Value::Null
        ]]
    );
}

#[test]
fn all_old_attempts_are_retired_without_losing_terminal_results_inputs_or_execution_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let conn = retained_fixture(&dir.path().join("jobs.sqlite"));
    let jobs = rows(
        &conn,
        &format!("SELECT {JOB_VALUES} FROM backend_jobs ORDER BY job_id"),
    );
    let attempts = rows(
        &conn,
        &format!("SELECT {ATTEMPT_VALUES} FROM backend_job_attempts ORDER BY attempt_id"),
    );
    let deliveries = rows(
        &conn,
        "SELECT * FROM backend_job_deliveries ORDER BY delivery_id",
    );
    apply_leaf(&conn).unwrap();
    assert_retired(&conn);
    assert_eq!(
        jobs,
        rows(
            &conn,
            &format!("SELECT {JOB_VALUES} FROM backend_jobs ORDER BY job_id")
        )
    );
    assert_eq!(
        attempts,
        rows(
            &conn,
            &format!("SELECT {ATTEMPT_VALUES} FROM backend_job_attempts ORDER BY attempt_id")
        )
    );
    assert_eq!(
        deliveries,
        rows(
            &conn,
            "SELECT * FROM backend_job_deliveries ORDER BY delivery_id"
        )
    );
    assert_eq!(
        rows(
            &conn,
            "SELECT input_disposition,json_type(request_json,'$.input') FROM backend_jobs WHERE job_id='failed'"
        ),
        vec![vec![
            Value::Text("erased".into()),
            Value::Text("null".into())
        ]]
    );
    assert!(
        conn.execute_batch("UPDATE backend_job_upgrade_history SET state='pending'")
            .is_err()
    );
}

#[test]
fn delivery_states_and_same_id_reconciliation_survive_job_cutover() {
    let dir = tempfile::tempdir().unwrap();
    let conn = retained_fixture(&dir.path().join("delivery.sqlite"));
    let deliveries = rows(
        &conn,
        "SELECT * FROM backend_job_deliveries ORDER BY delivery_id",
    );
    let ddl = rows(
        &conn,
        "SELECT type,name,sql FROM sqlite_schema WHERE tbl_name='backend_job_deliveries' ORDER BY type,name",
    );
    apply_leaf(&conn).unwrap();
    assert_eq!(
        deliveries,
        rows(
            &conn,
            "SELECT * FROM backend_job_deliveries ORDER BY delivery_id"
        )
    );
    assert_eq!(
        ddl,
        rows(
            &conn,
            "SELECT type,name,sql FROM sqlite_schema WHERE tbl_name='backend_job_deliveries' ORDER BY type,name"
        )
    );
    // Same-ID notification acceptance recovery is not Job execution recovery.
    conn.execute_batch("UPDATE backend_job_deliveries SET state='pending',failure_category=NULL,failure_detail=NULL,updated_at='reconcile' WHERE state IN ('sending','unknown');
        UPDATE backend_job_deliveries SET state='sending',updated_at='claim' WHERE delivery_id='delivery-4' AND state='pending';
        UPDATE backend_job_deliveries SET state='completed',delivered_at='accepted',updated_at='accepted' WHERE delivery_id='delivery-4' AND state='sending';").unwrap();
    assert_eq!(
        rows(
            &conn,
            "SELECT delivery_id,job_id,attempt_id,state FROM backend_job_deliveries WHERE delivery_id='delivery-4'"
        ),
        vec![vec![
            Value::Text("delivery-4".into()),
            Value::Text("running".into()),
            Value::Text("running-0".into()),
            Value::Text("completed".into())
        ]]
    );
    // Retained completed results can still acquire their first notification row.
    conn.execute_batch("INSERT INTO backend_job_deliveries(workspace_id,delivery_id,job_id,attempt_id,target_runtime_id,target_worker_id,state,created_at,updated_at)
        VALUES('upgrade-workspace','later-delivery','completed','completed-0','target-runtime','target-worker','unknown','1','1');").unwrap();
    assert_retired(&conn);
}

#[test]
fn terminal_receipts_cleanup_evidence_and_input_retirement_remain_independent() {
    let dir = tempfile::tempdir().unwrap();
    let conn = retained_fixture(&dir.path().join("terminal.sqlite"));
    for state in ["completed", "failed"] {
        let id = format!("terminal-{state}");
        insert_job(&conn, &id, state);
        insert_attempt(&conn, &id, &id, 1, state);
    }
    let receipts_query = "SELECT job_id,state,result_json,result_digest,failure_category,failure_detail,completed_at FROM backend_jobs WHERE job_id LIKE 'terminal-%' ORDER BY job_id";
    let attempts_query = "SELECT attempt_id,state,result_json,result_digest,failure_category,failure_detail,completed_at FROM backend_job_attempts WHERE job_id LIKE 'terminal-%' ORDER BY attempt_id";
    let receipts = rows(&conn, receipts_query);
    let attempts = rows(&conn, attempts_query);
    apply_leaf(&conn).unwrap();
    conn.execute_batch("UPDATE backend_jobs SET result_json=result_json,result_digest=result_digest,failure_category=failure_category,failure_detail=failure_detail,completed_at=completed_at WHERE job_id LIKE 'terminal-%';
        UPDATE backend_job_attempts SET state=state,result_json=result_json,result_digest=result_digest,failure_category=failure_category,failure_detail=failure_detail WHERE job_id LIKE 'terminal-%';
        UPDATE backend_job_attempts SET worker_cleanup_state='executing',worker_cleanup_updated_at='cleanup-claim' WHERE job_id LIKE 'terminal-%';
        UPDATE backend_job_attempts SET worker_cleanup_state='failed',worker_cleanup_failure_category='independent-cleanup-failure',worker_cleanup_failure_detail='independent evidence',worker_cleanup_updated_at='cleanup-failed' WHERE job_id LIKE 'terminal-%';
        UPDATE backend_job_attempts SET worker_cleanup_state='executing',worker_cleanup_updated_at='cleanup-reclaim' WHERE job_id LIKE 'terminal-%';
        UPDATE backend_job_attempts SET worker_cleanup_state='completed',worker_cleanup_completed_at='independently-supplied-end',worker_cleanup_updated_at='cleanup-end' WHERE job_id LIKE 'terminal-%';
        UPDATE backend_jobs SET input_disposition='reclaimed',request_json=json_set(request_json,'$.input',json('null')) WHERE job_id='terminal-completed';
        UPDATE backend_jobs SET input_disposition='invalidated',request_json=json_set(request_json,'$.input',json('null')) WHERE job_id='terminal-failed';
        UPDATE backend_jobs SET input_disposition='erased' WHERE job_id LIKE 'terminal-%';").unwrap();
    assert_eq!(receipts, rows(&conn, receipts_query));
    assert_eq!(attempts, rows(&conn, attempts_query));
    assert_eq!(
        rows(
            &conn,
            "SELECT DISTINCT worker_cleanup_state,worker_cleanup_failure_category FROM backend_job_attempts WHERE job_id LIKE 'terminal-%'"
        ),
        vec![vec![
            Value::Text("completed".into()),
            Value::Text("independent-cleanup-failure".into())
        ]]
    );
    assert_eq!(
        rows(
            &conn,
            "SELECT DISTINCT input_disposition,json_type(request_json,'$.input') FROM backend_jobs WHERE job_id LIKE 'terminal-%'"
        ),
        vec![vec![
            Value::Text("erased".into()),
            Value::Text("null".into())
        ]]
    );
    verify(&conn).unwrap();
    // This fixture supplies cleanup facts explicitly; migration did not derive
    // them from Failed. It is not a process-termination proof or cleanup runner.
}

#[test]
fn old_jobs_cannot_resume_retry_accept_late_results_or_allocate_continuation_attempts() {
    let dir = tempfile::tempdir().unwrap();
    let conn = retained_fixture(&dir.path().join("jobs.sqlite"));
    apply_leaf(&conn).unwrap();
    for sql in [
        "UPDATE backend_jobs SET state='pending' WHERE job_id='running'",
        "UPDATE backend_jobs SET current_attempt=2 WHERE job_id='running'",
        "UPDATE backend_jobs SET state='completed',result_json='{}',result_digest='late' WHERE job_id='running'",
        "UPDATE backend_job_attempts SET state='dispatching' WHERE attempt_id='running-0'",
        "UPDATE backend_job_attempts SET runtime_run_id='late-run' WHERE attempt_id='running-0'",
        "INSERT INTO backend_job_attempts(workspace_id,job_id,attempt_id,attempt,input_digest,state,deadline_at,created_at,updated_at) VALUES('upgrade-workspace','running','continuation',3,'digest','reserved','deadline','1','1')",
    ] {
        assert!(conn.execute_batch(sql).is_err(), "accepted {sql}");
    }
    // Input retirement remains its own explicit operation and does not grant retry.
    conn.execute_batch("UPDATE backend_jobs SET input_disposition='erased',request_json=json_set(request_json,'$.input',json('null')) WHERE job_id='running';").unwrap();
    assert_retired(&conn);
}

#[test]
fn failure_after_job_conversion_rolls_back_history_ddl_and_states_then_retry_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let conn = retained_fixture(&dir.path().join("jobs.sqlite"));
    conn.execute_batch("CREATE TRIGGER injected_attempt_failure BEFORE UPDATE OF state ON backend_job_attempts WHEN NEW.state='failed' BEGIN SELECT RAISE(ABORT,'injected migration failure'); END;").unwrap();
    let schema = rows(
        &conn,
        "SELECT type,name,sql FROM sqlite_schema ORDER BY type,name",
    );
    let jobs = rows(&conn, "SELECT * FROM backend_jobs ORDER BY job_id");
    let attempts = rows(
        &conn,
        "SELECT * FROM backend_job_attempts ORDER BY attempt_id",
    );
    assert!(apply_leaf(&conn).is_err());
    assert_eq!(
        schema,
        rows(
            &conn,
            "SELECT type,name,sql FROM sqlite_schema ORDER BY type,name"
        )
    );
    assert_eq!(
        jobs,
        rows(&conn, "SELECT * FROM backend_jobs ORDER BY job_id")
    );
    assert_eq!(
        attempts,
        rows(
            &conn,
            "SELECT * FROM backend_job_attempts ORDER BY attempt_id"
        )
    );
    let enabled: bool = conn
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .unwrap();
    assert!(enabled);
    conn.execute_batch("DROP TRIGGER injected_attempt_failure")
        .unwrap();
    apply_leaf(&conn).unwrap();
    assert_retired(&conn);
}

#[test]
fn failure_after_rebuild_rolls_back_constraints_and_retries_without_temp_table_leaks() {
    let dir = tempfile::tempdir().unwrap();
    let conn = retained_fixture(&dir.path().join("jobs.sqlite"));
    conn.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
    let before = rows(
        &conn,
        "SELECT type,name,sql FROM sqlite_schema ORDER BY type,name",
    );
    {
        let tx = conn.unchecked_transaction().unwrap();
        install(&tx).unwrap();
        // Represent failure persisting the owning migration receipt after DDL.
        assert!(
            tx.execute_batch("INSERT INTO missing_migration_receipt VALUES(1)")
                .is_err()
        );
    }
    assert_eq!(
        before,
        rows(
            &conn,
            "SELECT type,name,sql FROM sqlite_schema ORDER BY type,name"
        )
    );
    assert!(
        rows(
            &conn,
            "SELECT name FROM sqlite_temp_schema WHERE name LIKE '__yoi_upgrade_%'"
        )
        .is_empty()
    );
    conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    apply_leaf(&conn).unwrap();
    assert_retired(&conn);
}

#[test]
fn ordinary_reopen_and_accidental_reapply_do_not_sweep_new_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("jobs.sqlite");
    let conn = retained_fixture(&path);
    apply_leaf(&conn).unwrap();
    insert_job(&conn, "post-upgrade", "pending");
    insert_attempt(&conn, "post-upgrade", "new-attempt", 1, "reserved");
    let history = rows(
        &conn,
        "SELECT * FROM backend_job_upgrade_history ORDER BY job_id",
    );
    drop(conn);
    let conn = Connection::open(path).unwrap();
    store_owner::configure_sqlite(&conn).unwrap();
    verify(&conn).unwrap();
    assert!(apply_leaf(&conn).is_err());
    assert_eq!(
        history,
        rows(
            &conn,
            "SELECT * FROM backend_job_upgrade_history ORDER BY job_id"
        )
    );
    assert_eq!(
        rows(
            &conn,
            "SELECT state FROM backend_jobs WHERE job_id='post-upgrade'"
        ),
        vec![vec![Value::Text("pending".into())]]
    );
    conn.execute_batch(
        "UPDATE backend_job_attempts SET state='dispatching' WHERE attempt_id='new-attempt'",
    )
    .unwrap();
    assert!(conn.execute_batch("UPDATE backend_job_attempts SET job_id='unstarted',attempt=2 WHERE attempt_id='new-attempt'").is_err());
    assert!(
        conn.execute_batch("UPDATE backend_jobs SET state='unknown' WHERE job_id='post-upgrade'")
            .is_err()
    );
}

#[test]
fn install_requires_an_explicit_migration_transaction_and_pretransaction_fk_window() {
    let dir = tempfile::tempdir().unwrap();
    let conn = retained_fixture(&dir.path().join("jobs.sqlite"));
    assert!(install(&conn).is_err());
    let tx = conn.unchecked_transaction().unwrap();
    assert!(install(&tx).is_err());
    assert!(!store_owner::table_exists(&tx, "backend_job_upgrade_history").unwrap());
}

// These tests intentionally fail if only module registration is added: the
// production ordered runner, fresh creation and physical recognizer must also
// be connected. No ignored feature or duplicate test-only migration is used.
#[test]
fn retained_baselines_reach_upgrade_via_the_real_runner_and_reopen_keeps_new_jobs_live() {
    for baseline in 50..=113 {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("retained.sqlite");
        if baseline <= 51 {
            store_owner::tests::prepare_schema_v50(&path, None);
            if baseline == 51 {
                let conn = Connection::open(&path).unwrap();
                store_owner::migrate_workspace_runtime_bindings_v50_to_v51(&conn).unwrap();
            }
        } else {
            store_owner::tests::prepare_retained_schema(&path, baseline);
        }
        if baseline >= 86 {
            let conn = Connection::open(&path).unwrap();
            store_owner::configure_sqlite(&conn).unwrap();
            seed_boundary(&conn);
        }
        let store = store_owner::SqliteWorkspaceStore::open_for_test(&path).unwrap();
        drop(store);
        let conn = Connection::open(&path).unwrap();
        store_owner::configure_sqlite(&conn).unwrap();
        verify(&conn).unwrap();
        if baseline >= 86 {
            assert_retired(&conn);
            // Already-converted source rows have no recoverable pre-86 wire.
            assert_eq!(
                rows(
                    &conn,
                    "SELECT count(*) FROM backend_job_pre_content_history"
                ),
                vec![vec![Value::Integer(0)]]
            );
        } else {
            seed_workspace(&conn);
        }
        insert_job(&conn, "post-upgrade", "pending");
        insert_attempt(&conn, "post-upgrade", "post-upgrade-attempt", 1, "reserved");
        let history = rows(
            &conn,
            "SELECT * FROM backend_job_upgrade_history ORDER BY job_id",
        );
        let receipts = rows(
            &conn,
            "SELECT * FROM __yoi_schema_migrations ORDER BY version",
        );
        drop(conn);
        drop(store_owner::SqliteWorkspaceStore::open_for_test(&path).unwrap());
        let conn = Connection::open(&path).unwrap();
        assert_eq!(
            history,
            rows(
                &conn,
                "SELECT * FROM backend_job_upgrade_history ORDER BY job_id"
            )
        );
        assert_eq!(
            receipts,
            rows(
                &conn,
                "SELECT * FROM __yoi_schema_migrations ORDER BY version"
            )
        );
        assert_eq!(
            rows(
                &conn,
                "SELECT state FROM backend_jobs WHERE job_id='post-upgrade'"
            ),
            vec![vec![Value::Text("pending".into())]]
        );
    }
}

#[test]
fn store_retry_and_recovery_exclude_migrated_jobs_but_admit_a_new_request() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("api.sqlite");
    store_owner::tests::prepare_retained_schema(&path, 113);
    let conn = Connection::open(&path).unwrap();
    store_owner::configure_sqlite(&conn).unwrap();
    seed_workspace(&conn);
    insert_job(&conn, "legacy-retry", "pending");
    let old_attempt = crate::backend_job::attempt_id("legacy-retry", 1);
    insert_attempt(&conn, "legacy-retry", &old_attempt, 1, "reserved");
    conn.execute(
        "UPDATE backend_job_attempts SET input_digest=?1 WHERE attempt_id=?2",
        params![request("legacy-retry").input_digest().unwrap(), old_attempt],
    )
    .unwrap();
    drop(conn);
    let store = store_owner::SqliteWorkspaceStore::open_for_test(&path).unwrap();
    let old = store
        .get_backend_job("upgrade-workspace", "legacy-retry")
        .unwrap()
        .unwrap();
    assert_eq!(old.failure_category.as_deref(), Some(FAILURE_CATEGORY));
    let retired_attempt = store
        .get_backend_job_attempt("upgrade-workspace", "legacy-retry", &old_attempt)
        .unwrap()
        .unwrap();
    assert!(
        store
            .probe_backend_job_recovery("upgrade-workspace", &retired_attempt)
            .is_err()
    );
    let saved_domain_result = crate::backend_job::BackendJobResultSubmission {
        job_id: "legacy-retry".into(),
        attempt_id: old_attempt.clone(),
        input_digest: retired_attempt.input_digest.clone(),
        result: serde_json::json!({"saved":true}),
    };
    assert!(
        store
            .reconcile_backend_job_domain_result(
                "upgrade-workspace",
                &retired_attempt,
                &saved_domain_result,
                "2026-11-04T00:00:00Z"
            )
            .is_err()
    );
    // The old attempt has no Worker and no cleanup debt. Retry must still be
    // rejected, not merely hidden by an unfinished-cleanup or attempt-budget gate.
    assert!(
        store
            .reserve_backend_job_retry("upgrade-workspace", "legacy-retry", "2026-11-04T00:00:00Z")
            .is_err()
    );
    assert!(
        store
            .list_recoverable_backend_job_attempts("upgrade-workspace", 100)
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .list_reserved_backend_job_attempts("upgrade-workspace", 100)
            .unwrap()
            .is_empty()
    );
    let next = store
        .reserve_backend_job(
            "upgrade-workspace",
            &request("new-live"),
            "2026-11-04T00:00:00Z",
        )
        .unwrap();
    assert_eq!(next.job.state, crate::backend_job::BackendJobState::Pending);
    drop(store);
    let reopened = store_owner::SqliteWorkspaceStore::open_for_test(&path).unwrap();
    let recoverable = reopened
        .list_recoverable_backend_job_attempts("upgrade-workspace", 100)
        .unwrap();
    assert_eq!(recoverable.len(), 1);
    assert_eq!(recoverable[0].job_id, "new-live");
    assert_eq!(
        reopened
            .get_backend_job("upgrade-workspace", "legacy-retry")
            .unwrap()
            .unwrap(),
        old
    );
}

#[test]
fn store_delivery_reconciliation_keeps_the_original_delivery_id_after_cutover() {
    use crate::backend_job::BackendJobDeliveryState;
    use worker_runtime::identity::RuntimeWorkerRef;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("delivery-api.sqlite");
    drop(retained_fixture(&path));
    let store = store_owner::SqliteWorkspaceStore::open_for_test(&path).unwrap();
    let target = RuntimeWorkerRef::new("target-runtime", "target-worker");
    let recovered = store
        .recover_backend_job_deliveries("upgrade-workspace", "2026-11-04T00:00:00Z")
        .unwrap();
    assert!(
        recovered
            .iter()
            .any(|record| record.delivery_id == "delivery-4"
                && record.state == BackendJobDeliveryState::Pending)
    );
    let (claimed, was_claimed) = store
        .reserve_backend_job_delivery(
            "upgrade-workspace",
            "delivery-4",
            "running",
            &target,
            "2026-11-04T00:00:01Z",
        )
        .unwrap();
    assert!(was_claimed);
    assert_eq!(claimed.delivery_id, "delivery-4");
    let finished = store
        .finish_backend_job_delivery(
            "upgrade-workspace",
            "delivery-4",
            BackendJobDeliveryState::Completed,
            None,
            None,
            "2026-11-04T00:00:02Z",
        )
        .unwrap();
    let replay = store
        .finish_backend_job_delivery(
            "upgrade-workspace",
            "delivery-4",
            BackendJobDeliveryState::Completed,
            None,
            None,
            "2026-11-04T00:00:03Z",
        )
        .unwrap();
    assert_eq!(finished, replay);
    drop(store);
    let conn = Connection::open(path).unwrap();
    assert_eq!(
        rows(&conn, "SELECT count(*) FROM backend_job_deliveries"),
        vec![vec![Value::Integer(5)]]
    );
    assert_retired(&conn);
}

#[test]
fn store_terminal_replays_and_retirement_preserve_old_causes_and_results() {
    use crate::backend_job::{
        BackendJobAttemptState, BackendJobResultSubmission, JobInputRetirementReason,
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("terminal-api.sqlite");
    store_owner::tests::prepare_retained_schema(&path, 113);
    let conn = Connection::open(&path).unwrap();
    store_owner::configure_sqlite(&conn).unwrap();
    seed_workspace(&conn);
    let value = serde_json::json!({"already":"accepted"});
    let (result_digest, result_json) =
        job::result_digest(&value, request("x").limits.max_result_bytes).unwrap();
    for state in ["completed", "failed"] {
        let id = format!("terminal-api-{state}");
        insert_job(&conn, &id, state);
        let attempt = crate::backend_job::attempt_id(&id, 1);
        insert_attempt(&conn, &id, &attempt, 1, state);
        conn.execute("UPDATE backend_job_attempts SET input_digest=?1,runtime_run_id='retained-run',worker_cleanup_state='completed',worker_cleanup_completed_at='retained-cleanup-end' WHERE attempt_id=?2",
            params![request(&id).input_digest().unwrap(),attempt]).unwrap();
        if state == "completed" {
            conn.execute(
                "UPDATE backend_jobs SET result_json=?1,result_digest=?2 WHERE job_id=?3",
                params![result_json, result_digest, id],
            )
            .unwrap();
            conn.execute(
                "UPDATE backend_job_attempts SET result_json=?1,result_digest=?2 WHERE job_id=?3",
                params![result_json, result_digest, id],
            )
            .unwrap();
        }
    }
    let query = "SELECT job_id,state,result_json,result_digest,failure_category,failure_detail,completed_at FROM backend_jobs ORDER BY job_id";
    let before = rows(&conn, query);
    drop(conn);
    let store = store_owner::SqliteWorkspaceStore::open_for_test(&path).unwrap();
    let id = "terminal-api-completed";
    let attempt_id = crate::backend_job::attempt_id(id, 1);
    let attempt = store
        .get_backend_job_attempt("upgrade-workspace", id, &attempt_id)
        .unwrap()
        .unwrap();
    let replay = store
        .accept_backend_job_result(
            "upgrade-workspace",
            attempt.worker.as_ref().unwrap(),
            &BackendJobResultSubmission {
                job_id: id.into(),
                attempt_id,
                input_digest: request(id).input_digest().unwrap(),
                result: value,
            },
            "2026-11-04T00:00:00Z",
        )
        .unwrap();
    assert!(replay.replayed);
    let failed_id = "terminal-api-failed";
    let failed_attempt = crate::backend_job::attempt_id(failed_id, 1);
    let failed = store
        .finish_backend_job_attempt(
            "upgrade-workspace",
            failed_id,
            &failed_attempt,
            BackendJobAttemptState::Failed,
            "old-attempt-cause",
            "old attempt detail",
            "2026-11-04T00:00:01Z",
        )
        .unwrap();
    assert_eq!(
        failed.failure_category.as_deref(),
        Some("old-attempt-cause")
    );
    for (id, reason) in [
        (id, JobInputRetirementReason::Completed),
        (failed_id, JobInputRetirementReason::LegacyInvalidation),
    ] {
        store
            .reclaim_backend_job_input(
                "upgrade-workspace",
                id,
                &request(id).input_digest().unwrap(),
                reason,
            )
            .unwrap();
        store
            .reclaim_backend_job_input(
                "upgrade-workspace",
                id,
                &request(id).input_digest().unwrap(),
                JobInputRetirementReason::ExplicitErasure,
            )
            .unwrap();
    }
    drop(store);
    let conn = Connection::open(path).unwrap();
    assert_eq!(before, rows(&conn, query));
    assert_eq!(
        rows(
            &conn,
            "SELECT DISTINCT input_disposition,json_type(request_json,'$.input') FROM backend_jobs"
        ),
        vec![vec![
            Value::Text("erased".into()),
            Value::Text("null".into())
        ]]
    );
    verify(&conn).unwrap();
}

#[test]
fn owning_runner_receipt_failure_rolls_back_conversion_and_retry_commits_once() {
    let dir = tempfile::tempdir().unwrap();
    let conn = retained_fixture(&dir.path().join("receipt.sqlite"));
    let jobs = rows(&conn, "SELECT * FROM backend_jobs ORDER BY job_id");
    let attempts = rows(
        &conn,
        "SELECT * FROM backend_job_attempts ORDER BY attempt_id",
    );
    conn.execute_batch("CREATE TEMP TRIGGER injected_upgrade_receipt_failure BEFORE INSERT ON main.__yoi_schema_migrations
        WHEN EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='backend_job_upgrade_history')
        BEGIN SELECT RAISE(ABORT,'injected owning upgrade receipt failure'); END;").unwrap();
    assert!(store_owner::apply_migrations(&conn).is_err());
    assert!(!store_owner::table_exists(&conn, "backend_job_upgrade_history").unwrap());
    assert_eq!(
        jobs,
        rows(&conn, "SELECT * FROM backend_jobs ORDER BY job_id")
    );
    assert_eq!(
        attempts,
        rows(
            &conn,
            "SELECT * FROM backend_job_attempts ORDER BY attempt_id"
        )
    );
    let enabled: bool = conn
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .unwrap();
    assert!(enabled);
    conn.execute_batch("DROP TRIGGER injected_upgrade_receipt_failure")
        .unwrap();
    store_owner::apply_migrations(&conn).unwrap();
    assert_retired(&conn);
    let receipts = rows(
        &conn,
        "SELECT * FROM __yoi_schema_migrations ORDER BY version",
    );
    store_owner::apply_migrations(&conn).unwrap();
    assert_eq!(
        receipts,
        rows(
            &conn,
            "SELECT * FROM __yoi_schema_migrations ORDER BY version"
        )
    );
}

#[test]
fn both_103_receipts_and_baseline_only_identities_use_the_real_upgrade_runner() {
    for job_variant in [false, true] {
        for baseline_only in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("branch.sqlite");
            store_owner::tests::prepare_retained_schema(&path, 102);
            let conn = Connection::open(&path).unwrap();
            store_owner::configure_sqlite(&conn).unwrap();
            if job_variant {
                (store_owner::JOB_INPUT_RETENTION_SCHEMA_103.apply)(&conn).unwrap();
            } else {
                store_owner::migrate_passkey_results_v102_to_v103(&conn).unwrap();
            }
            if baseline_only {
                conn.execute_batch("DELETE FROM __yoi_schema_migrations; INSERT INTO __yoi_schema_migrations(version,name) VALUES(103,'workspace schema baseline');").unwrap();
            }
            seed_boundary(&conn);
            let before = rows(
                &conn,
                "SELECT * FROM __yoi_schema_migrations WHERE version<=103 ORDER BY version",
            );
            drop(conn);
            drop(store_owner::SqliteWorkspaceStore::open_for_test(&path).unwrap());
            let conn = Connection::open(path).unwrap();
            assert_retired(&conn);
            assert_eq!(
                before,
                rows(
                    &conn,
                    "SELECT * FROM __yoi_schema_migrations WHERE version<=103 ORDER BY version"
                )
            );
        }
    }
}

#[test]
fn fresh_creation_and_empty_retained_upgrade_have_identical_job_ddl() {
    let dir = tempfile::tempdir().unwrap();
    // Each catalog owns its sibling feature-storage identity directory. Sharing
    // a parent would ask the same owner to replace an already-bound catalog.
    let migrated = dir.path().join("migrated/server.sqlite");
    let fresh = dir.path().join("fresh/server.sqlite");
    std::fs::create_dir_all(migrated.parent().unwrap()).unwrap();
    std::fs::create_dir_all(fresh.parent().unwrap()).unwrap();
    store_owner::tests::prepare_retained_schema(&migrated, 113);
    drop(store_owner::SqliteWorkspaceStore::open_for_test(&migrated).unwrap());
    drop(store_owner::SqliteWorkspaceStore::open_for_test(&fresh).unwrap());
    // Column declaration order can differ after retained ALTERs. Compare values,
    // keys, FK actions and index columns, not SQLite's source formatting.
    fn objects(path: &Path) -> Vec<Vec<Value>> {
        let conn = Connection::open(path).unwrap();
        let tables = rows(
            &conn,
            "SELECT name FROM sqlite_schema WHERE type='table' AND name LIKE 'backend_job%' ORDER BY name",
        );
        let mut signature = tables.clone();
        for table in tables {
            let Value::Text(name) = &table[0] else {
                unreachable!()
            };
            let name = format!("'{}'", name.replace('\'', "''"));
            for query in [
                format!(
                    "SELECT name,type,\"notnull\",dflt_value,pk FROM pragma_table_info({name}) ORDER BY name"
                ),
                format!(
                    "SELECT \"table\",\"from\",\"to\",on_update,on_delete,match,seq FROM pragma_foreign_key_list({name}) ORDER BY \"table\",\"from\",\"to\",seq"
                ),
                format!(
                    "SELECT i.\"unique\",i.partial,(SELECT group_concat(name,',') FROM (SELECT name FROM pragma_index_info(i.name) ORDER BY seqno)) FROM pragma_index_list({name}) i ORDER BY 1,2,3"
                ),
            ] {
                signature.extend(rows(&conn, &query));
            }
        }
        signature.extend(rows(&conn,"SELECT type,name,replace(replace(replace(sql,char(10),''),char(13),''),' ','') FROM sqlite_schema WHERE name LIKE 'backend_job%' AND type IN ('trigger','index') AND sql IS NOT NULL ORDER BY type,name"));
        verify(&conn).unwrap();
        signature
    }
    assert_eq!(objects(&migrated), objects(&fresh));
}

// The storage-format fixture owner can call this for EVERY real published stage
// and convergence predecessor. It snapshots all rows, so policy/root/erasure and
// existing T820 history/cleanup invocation evidence must survive byte-for-byte.
pub(super) fn assert_published_fixture(conn: &Connection) {
    let tables = conn.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name").unwrap()
        .query_map([],|row| row.get::<_,String>(0)).unwrap()
        .collect::<rusqlite::Result<Vec<_>>>().unwrap();
    let saved = tables
        .iter()
        .map(|name| {
            let columns = store_owner::table_columns(conn, name)
                .unwrap()
                .into_iter()
                .filter(|column| {
                    !["backend_jobs", "backend_job_attempts"].contains(&name.as_str())
                        || !["state", "failure_category", "failure_detail"]
                            .contains(&column.as_str())
                })
                .map(|column| quoted(&column))
                .collect::<Vec<_>>()
                .join(",");
            let query = format!("SELECT {columns} FROM {} ORDER BY {columns}", quoted(name));
            let before = rows(conn, &query);
            (name.clone(), query, before)
        })
        .collect::<Vec<_>>();
    apply_leaf(conn).unwrap();
    verify(conn).unwrap();
    for (name, query, before) in saved {
        assert_eq!(before, rows(conn, &query), "changed {name}");
    }
}

#[test]
fn captured_history_is_accepted_at_each_real_retained_prefix_and_cannot_be_forged_later() {
    let dir = tempfile::tempdir().unwrap();
    let conn = legacy_fixture_to_85(&dir.path().join("prefixes.sqlite"), 85);
    let original = pre_content_values(&conn, false);
    for migration in store_owner::MIGRATIONS
        .iter()
        .filter(|migration| migration.version >= 86 && migration.version <= 122)
    {
        (migration.apply)(&conn).unwrap();
        // In particular 103 keeps its original branch name and all later
        // prefixes recognize genuine raw capture without relabeling receipts.
        super::super::storage_formats::verify_history(&conn, migration.version).unwrap();
        assert_eq!(original, pre_content_values(&conn, true));
        let error=conn.execute_batch("INSERT INTO backend_job_pre_content_history SELECT * FROM backend_job_pre_content_history LIMIT 1").unwrap_err();
        assert!(
            error.to_string().contains("schema-85 transaction"),
            "{error}"
        );
    }
    store_owner::apply_migrations(&conn).unwrap();
    let receipts = rows(
        &conn,
        "SELECT version,name FROM __yoi_schema_migrations ORDER BY version",
    );
    assert_eq!(
        receipts.last().unwrap(),
        &vec![Value::Integer(123), Value::Text(MIGRATION_NAME.into())]
    );
    assert_eq!(original, pre_content_values(&conn, true));
    assert!(conn.execute_batch("INSERT INTO backend_job_pre_content_history SELECT * FROM backend_job_pre_content_history LIMIT 1").is_err());
}

#[test]
fn raw_capture_shape_drift_is_rejected_before_the_runner_writes_or_retires_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let conn = legacy_fixture_to_85(&dir.path().join("raw-drift.sqlite"), 85);
    store_owner::migrate_content_state_v85_to_v86(&conn).unwrap();
    conn.execute_batch("CREATE INDEX forged_raw_index ON backend_job_pre_content_history(job_id)")
        .unwrap();
    let jobs = rows(&conn, "SELECT * FROM backend_jobs ORDER BY job_id");
    let receipts = rows(
        &conn,
        "SELECT * FROM __yoi_schema_migrations ORDER BY version",
    );
    assert!(store_owner::apply_migrations(&conn).is_err());
    assert_eq!(
        jobs,
        rows(&conn, "SELECT * FROM backend_jobs ORDER BY job_id")
    );
    assert_eq!(
        receipts,
        rows(
            &conn,
            "SELECT * FROM __yoi_schema_migrations ORDER BY version"
        )
    );
    assert!(!store_owner::table_exists(&conn, "backend_job_upgrade_history").unwrap());
    conn.execute_batch("DROP INDEX forged_raw_index").unwrap();
    store_owner::apply_migrations(&conn).unwrap();
    verify(&conn).unwrap();
}

#[test]
fn fresh_install_has_empty_histories_and_rejects_legacy_live_state_at_the_sql_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fresh-live.sqlite");
    drop(store_owner::SqliteWorkspaceStore::open_for_test(&path).unwrap());
    let conn = Connection::open(path).unwrap();
    store_owner::configure_sqlite(&conn).unwrap();
    for table in [
        "backend_job_pre_content_history",
        "backend_job_attempt_pre_content_history",
        "backend_job_delivery_pre_content_history",
        "backend_job_upgrade_history",
        "backend_job_attempt_upgrade_history",
    ] {
        assert_eq!(
            rows(&conn, &format!("SELECT count(*) FROM {table}")),
            vec![vec![Value::Integer(0)]]
        );
    }
    seed_workspace(&conn);
    insert_job(&conn, "new-live", "pending");
    insert_attempt(&conn, "new-live", "new-live-attempt", 1, "reserved");
    assert!(
        conn.execute_batch("UPDATE backend_jobs SET state='unknown' WHERE job_id='new-live'")
            .is_err()
    );
    assert!(
        conn.execute_batch(
            "UPDATE backend_job_attempts SET state='unknown' WHERE job_id='new-live'"
        )
        .is_err()
    );
    assert_eq!(
        rows(
            &conn,
            "SELECT state FROM backend_jobs WHERE job_id='new-live'"
        ),
        vec![vec![Value::Text("pending".into())]]
    );
}

#[test]
fn actual_runner_conversion_fault_restores_pragma_history_and_rows_then_reopens_new_job() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("actual-runner-fault.sqlite");
    let conn = retained_fixture(&path);
    for migration in store_owner::MIGRATIONS
        .iter()
        .filter(|m| m.version > 113 && m.version <= 122)
    {
        (migration.apply)(&conn).unwrap();
    }
    let jobs = rows(&conn, "SELECT * FROM backend_jobs ORDER BY job_id");
    let attempts = rows(
        &conn,
        "SELECT * FROM backend_job_attempts ORDER BY job_id,attempt_id",
    );
    let receipts = rows(
        &conn,
        "SELECT * FROM __yoi_schema_migrations ORDER BY version",
    );
    // A connection-local fault is not a forged physical storage family. It fires
    // after Job conversion, in the actual ordered runner's attempt update.
    conn.execute_batch("CREATE TEMP TRIGGER actual_attempt_fault BEFORE UPDATE OF state ON main.backend_job_attempts WHEN NEW.state='failed' AND OLD.state NOT IN ('completed','failed') BEGIN SELECT RAISE(ABORT,'actual attempt conversion fault'); END;").unwrap();
    let error = store_owner::apply_migrations(&conn).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("actual attempt conversion fault"),
        "{error}"
    );
    assert_eq!(
        jobs,
        rows(&conn, "SELECT * FROM backend_jobs ORDER BY job_id")
    );
    assert_eq!(
        attempts,
        rows(
            &conn,
            "SELECT * FROM backend_job_attempts ORDER BY job_id,attempt_id"
        )
    );
    assert_eq!(
        receipts,
        rows(
            &conn,
            "SELECT * FROM __yoi_schema_migrations ORDER BY version"
        )
    );
    assert!(!store_owner::table_exists(&conn, "backend_job_upgrade_history").unwrap());
    assert!(
        conn.query_row("PRAGMA foreign_keys", [], |row| row.get::<_, bool>(0))
            .unwrap()
    );
    conn.execute_batch("DROP TRIGGER actual_attempt_fault")
        .unwrap();
    store_owner::apply_migrations(&conn).unwrap();
    assert_retired(&conn); // includes unchanged Completed/Failed and all historical attempts
    let saved = rows(
        &conn,
        "SELECT * FROM backend_job_attempt_upgrade_history ORDER BY job_id,attempt_id",
    );
    drop(conn);
    let store = store_owner::SqliteWorkspaceStore::open_for_test(&path).unwrap();
    let fresh = store
        .reserve_backend_job(
            "upgrade-workspace",
            &request("after-fault-retry"),
            "2026-11-04T00:00:00Z",
        )
        .unwrap();
    drop(store);
    let store = store_owner::SqliteWorkspaceStore::open_for_test(&path).unwrap();
    assert_eq!(
        store
            .get_backend_job_attempt(
                "upgrade-workspace",
                "after-fault-retry",
                &fresh.attempt.attempt_id
            )
            .unwrap()
            .unwrap(),
        fresh.attempt
    );
    drop(store);
    let conn = Connection::open(path).unwrap();
    assert_eq!(
        saved,
        rows(
            &conn,
            "SELECT * FROM backend_job_attempt_upgrade_history ORDER BY job_id,attempt_id"
        )
    );
    assert_retired(&conn);
}
