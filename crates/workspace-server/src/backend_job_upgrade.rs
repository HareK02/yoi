//! One-shot retirement of Jobs that predate the current execution contract.
//!
//! The Workspace ordered migration owner must validate the retained storage
//! family first, disable foreign keys BEFORE its exclusive transaction, call
//! `install`, and commit the Workspace migration receipt in that same transaction.
//! It must restore foreign keys on both success and failure. This leaf neither
//! owns a version ledger nor runs from ordinary recovery/open after that receipt.
//!
//! A failed Job here means only "do not continue this old request". In particular,
//! no execution/cleanup termination, writer revocation, Journal completion, input
//! disposal or resource release is inferred. Pre-content captures preserve the
//! original cleanup values rather than the legacy cutover's inferred cleanup;
//! external Journal operations/checkpoints are untouched. Consumers must exclude
//! `backend_job_upgrade_history` Jobs from recovery/retry and consult the actual
//! execution/Journal owners before deletion. The guards also reject stale writes.
//! Notification delivery is a separate lifecycle: its saved states, notification
//! IDs and same-ID acceptance reconciliation are deliberately outside this cutover.
use super::{Connection, Error, Result};

/// Final history/guards. Fresh composition calls `install_fresh`, which also
/// installs EMPTY raw evidence; this SQL alone is not the full fresh schema.
pub(super) const MIGRATION_NAME: &str = "Backend Job execution retirement and owned reconciliation";
pub(super) const SCHEMA: &str = include_str!("backend_job_upgrade.sql");
pub(super) const FAILURE_CATEGORY: &str = "version_upgrade_execution_aborted";
pub(super) const FAILURE_DETAIL: &str = "バージョン更新による実行打切り。旧Jobの再開・再試行は行わない。このFailedは実行停止・書込み失効・Journal完了・rollback・資源解放の証明ではない。";

/// Evidence, not a second migration ledger. The existing schema-86 receipt owns
/// populated captures; fresh creation and later baselines install it EMPTY.
/// Raw request bytes remain historical and cannot authorize execution. Their only
/// allowed mutation is body retirement together with the live Job's owner.
pub(super) const RAW_SCHEMA: &str = r#"
CREATE TABLE backend_job_pre_content_history (
    workspace_id TEXT NOT NULL,
    job_id TEXT NOT NULL,
    purpose TEXT NOT NULL,
    input_revision TEXT NOT NULL,
    input_ref TEXT NOT NULL,
    resource_key TEXT,
    request_json TEXT NOT NULL CHECK (json_valid(request_json)),
    input_retired INTEGER NOT NULL CHECK (input_retired IN (0,1)),
    intent_fingerprint TEXT NOT NULL,
    current_attempt INTEGER NOT NULL,
    state TEXT NOT NULL,
    result_digest TEXT,
    failure_category TEXT,
    failure_detail TEXT,
    updated_at TEXT NOT NULL,
    completed_at TEXT,
    captured_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    PRIMARY KEY (workspace_id,job_id),
    FOREIGN KEY (workspace_id,job_id) REFERENCES backend_jobs(workspace_id,job_id) ON DELETE CASCADE
);
CREATE TABLE backend_job_attempt_pre_content_history (
    workspace_id TEXT NOT NULL,
    job_id TEXT NOT NULL,
    attempt_id TEXT NOT NULL,
    attempt INTEGER NOT NULL,
    input_revision TEXT NOT NULL,
    state TEXT NOT NULL,
    runtime_id TEXT,
    worker_id TEXT,
    runtime_run_id TEXT,
    dispatched_at TEXT,
    deadline_at TEXT NOT NULL,
    result_digest TEXT,
    failure_category TEXT,
    failure_detail TEXT,
    worker_cleanup_state TEXT,
    worker_cleanup_failure_category TEXT,
    worker_cleanup_failure_detail TEXT,
    worker_cleanup_updated_at TEXT,
    worker_cleanup_completed_at TEXT,
    updated_at TEXT NOT NULL,
    completed_at TEXT,
    captured_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    PRIMARY KEY (workspace_id,job_id,attempt_id),
    FOREIGN KEY (workspace_id,job_id,attempt_id) REFERENCES backend_job_attempts(workspace_id,job_id,attempt_id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,job_id) REFERENCES backend_job_pre_content_history(workspace_id,job_id) ON DELETE CASCADE
);
-- Delivery is captured because the retained content migration modifies it, not
-- because notification reconciliation participates in the Job execution cutover.
CREATE TABLE backend_job_delivery_pre_content_history (
    workspace_id TEXT NOT NULL,
    delivery_id TEXT NOT NULL,
    job_id TEXT NOT NULL,
    attempt_id TEXT NOT NULL,
    target_runtime_id TEXT NOT NULL,
    target_worker_id TEXT NOT NULL,
    state TEXT NOT NULL,
    failure_category TEXT,
    failure_detail TEXT,
    updated_at TEXT NOT NULL,
    delivered_at TEXT,
    captured_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    PRIMARY KEY (workspace_id,delivery_id),
    FOREIGN KEY (workspace_id,delivery_id) REFERENCES backend_job_deliveries(workspace_id,delivery_id) ON DELETE CASCADE
);
CREATE TRIGGER backend_job_pre_content_capture_boundary
BEFORE INSERT ON backend_job_pre_content_history
WHEN (SELECT max(version) FROM __yoi_schema_migrations) IS NOT 85
BEGIN SELECT RAISE(ABORT,'pre-content evidence requires the schema-85 transaction'); END;
CREATE TRIGGER backend_job_attempt_pre_content_capture_boundary
BEFORE INSERT ON backend_job_attempt_pre_content_history
WHEN (SELECT max(version) FROM __yoi_schema_migrations) IS NOT 85
BEGIN SELECT RAISE(ABORT,'pre-content evidence requires the schema-85 transaction'); END;
CREATE TRIGGER backend_job_delivery_pre_content_capture_boundary
BEFORE INSERT ON backend_job_delivery_pre_content_history
WHEN (SELECT max(version) FROM __yoi_schema_migrations) IS NOT 85
BEGIN SELECT RAISE(ABORT,'pre-content evidence requires the schema-85 transaction'); END;
CREATE TRIGGER backend_job_pre_content_identity_no_update
BEFORE UPDATE OF workspace_id,job_id,purpose,input_revision,input_ref,resource_key,intent_fingerprint,current_attempt,state,result_digest,failure_category,failure_detail,updated_at,completed_at,captured_at ON backend_job_pre_content_history
BEGIN SELECT RAISE(ABORT,'immutable pre-content Job evidence'); END;
CREATE TRIGGER backend_job_pre_content_request_retirement
BEFORE UPDATE OF request_json,input_retired ON backend_job_pre_content_history
WHEN (
    OLD.input_retired=0 AND NEW.input_retired=1
    AND json_type(NEW.request_json,'$.input')='null'
    AND json_remove(NEW.request_json,'$.input')=json_remove(OLD.request_json,'$.input')
    AND EXISTS(SELECT 1 FROM backend_jobs j WHERE j.workspace_id=OLD.workspace_id AND j.job_id=OLD.job_id
        AND COALESCE(json_type(j.request_json,'$.input'),'null')='null')
) IS NOT TRUE
BEGIN SELECT RAISE(ABORT,'pre-content Job body retires only with its owner'); END;
CREATE TRIGGER backend_job_pre_content_retire_with_owner
AFTER UPDATE OF request_json ON backend_jobs
WHEN COALESCE(json_type(NEW.request_json,'$.input'),'null')='null'
BEGIN
    UPDATE backend_job_pre_content_history
    SET request_json=json_set(request_json,'$.input',json('null')),input_retired=1
    WHERE workspace_id=NEW.workspace_id AND job_id=NEW.job_id AND input_retired=0;
END;
CREATE TRIGGER backend_job_pre_content_no_input_restore
BEFORE UPDATE OF request_json ON backend_jobs
WHEN COALESCE(json_type(NEW.request_json,'$.input'),'null')!='null'
 AND EXISTS(SELECT 1 FROM backend_job_pre_content_history h
            WHERE h.workspace_id=OLD.workspace_id AND h.job_id=OLD.job_id AND h.input_retired=1)
BEGIN SELECT RAISE(ABORT,'retired pre-content Job input cannot regenerate'); END;
CREATE TRIGGER backend_job_attempt_pre_content_no_update
BEFORE UPDATE ON backend_job_attempt_pre_content_history
BEGIN SELECT RAISE(ABORT,'immutable pre-content attempt evidence'); END;
CREATE TRIGGER backend_job_delivery_pre_content_no_update
BEFORE UPDATE ON backend_job_delivery_pre_content_history
BEGIN SELECT RAISE(ABORT,'immutable pre-content delivery evidence'); END;
"#;

const RAW_CAPTURE_SQL: &str = r#"
INSERT INTO backend_job_pre_content_history
    (workspace_id,job_id,purpose,input_revision,input_ref,resource_key,request_json,input_retired,intent_fingerprint,current_attempt,state,result_digest,failure_category,failure_detail,updated_at,completed_at)
SELECT workspace_id,job_id,purpose,input_revision,input_ref,resource_key,request_json,
       CASE WHEN COALESCE(json_type(request_json,'$.input'),'null')='null' THEN 1 ELSE 0 END,
       intent_fingerprint,current_attempt,state,result_digest,failure_category,failure_detail,updated_at,completed_at
FROM backend_jobs;
INSERT INTO backend_job_attempt_pre_content_history
    (workspace_id,job_id,attempt_id,attempt,input_revision,state,runtime_id,worker_id,runtime_run_id,dispatched_at,deadline_at,result_digest,failure_category,failure_detail,worker_cleanup_state,worker_cleanup_failure_category,worker_cleanup_failure_detail,worker_cleanup_updated_at,worker_cleanup_completed_at,updated_at,completed_at)
SELECT workspace_id,job_id,attempt_id,attempt,input_revision,state,runtime_id,worker_id,runtime_run_id,dispatched_at,deadline_at,result_digest,failure_category,failure_detail,worker_cleanup_state,worker_cleanup_failure_category,worker_cleanup_failure_detail,worker_cleanup_updated_at,worker_cleanup_completed_at,updated_at,completed_at
FROM backend_job_attempts;
INSERT INTO backend_job_delivery_pre_content_history
    (workspace_id,delivery_id,job_id,attempt_id,target_runtime_id,target_worker_id,state,failure_category,failure_detail,updated_at,delivered_at)
SELECT workspace_id,delivery_id,job_id,attempt_id,target_runtime_id,target_worker_id,state,failure_category,failure_detail,updated_at,delivered_at
FROM backend_job_deliveries;
"#;

/// Called exactly once INSIDE migrate_content_state_v85_to_v86's transaction,
/// before its Job SELECT/renames/updates. On error the owner must roll back the
/// entire schema-86 step. No open/read helper may call this entrypoint. The owner
/// holds the storage lifecycle lock until the final cutover is committed; partial
/// retained prefixes are resumable migration data, never execution admission.
pub(crate) fn capture_before_content_cutover(conn: &Connection) -> Result<()> {
    require_transaction(conn)?;
    if super::current_schema_version(conn)? != 85
        || !super::column_exists(conn, "backend_jobs", "input_revision")?
        || super::column_exists(conn, "backend_jobs", "input_digest")?
    {
        return Err(Error::Store(
            "pre-content Job capture requires the unmodified schema-85 boundary".into(),
        ));
    }
    conn.execute_batch(RAW_SCHEMA)?;
    conn.execute_batch(RAW_CAPTURE_SQL)?;
    verify_pre_content(conn)
}

/// Reject intervening Job/attempt ownership writes rather than erase real newer
/// evidence while undoing the retained cutover's synthetic values. Delivery
/// reconciliation and input retirement remain independent and are not frozen.
fn validate_content_cutover_before_restore(conn: &Connection) -> Result<()> {
    let drift: bool = conn.query_row(r#"
WITH reserved_jobs AS (
    SELECT j.workspace_id,j.job_id FROM backend_job_pre_content_history j
    WHERE EXISTS(SELECT 1 FROM backend_job_attempt_pre_content_history a WHERE a.workspace_id=j.workspace_id AND a.job_id=j.job_id)
      AND NOT EXISTS(SELECT 1 FROM backend_job_attempt_pre_content_history a WHERE a.workspace_id=j.workspace_id AND a.job_id=j.job_id
                     AND (a.state!='reserved' OR a.runtime_id IS NOT NULL OR a.worker_cleanup_state IS NOT NULL))
)
SELECT EXISTS(
    SELECT 1 FROM backend_jobs j JOIN backend_job_pre_content_history r USING(workspace_id,job_id)
    LEFT JOIN reserved_jobs v USING(workspace_id,job_id)
    WHERE j.state IS NOT (CASE WHEN r.state='pending' THEN CASE WHEN v.job_id IS NOT NULL THEN 'failed' ELSE 'unknown' END ELSE r.state END)
       OR j.failure_category IS NOT (CASE WHEN r.state='pending' THEN 'content_contract_cutover' ELSE r.failure_category END)
       OR j.failure_detail IS NOT r.failure_detail OR j.updated_at IS NOT r.updated_at
       OR j.completed_at IS NOT (CASE WHEN r.state='pending' THEN COALESCE(r.completed_at,r.updated_at) ELSE r.completed_at END)
) OR EXISTS(
    SELECT 1 FROM backend_job_attempts a JOIN backend_job_attempt_pre_content_history r USING(workspace_id,job_id,attempt_id)
    LEFT JOIN reserved_jobs v USING(workspace_id,job_id)
    WHERE a.state IS NOT (CASE WHEN r.state='reserved' AND v.job_id IS NOT NULL THEN 'failed'
                              WHEN r.state IN ('reserved','dispatching','dispatched') THEN 'unknown' ELSE r.state END)
       OR a.failure_category IS NOT (CASE WHEN r.state IN ('reserved','dispatching','dispatched') THEN 'content_contract_cutover' ELSE r.failure_category END)
       OR a.failure_detail IS NOT r.failure_detail OR a.updated_at IS NOT r.updated_at
       OR a.completed_at IS NOT (CASE WHEN r.state IN ('reserved','dispatching','dispatched') THEN COALESCE(r.completed_at,r.updated_at) ELSE r.completed_at END)
       OR a.worker_cleanup_state IS NOT (CASE WHEN r.state='reserved' AND v.job_id IS NOT NULL THEN 'completed'
            WHEN r.runtime_id IS NOT NULL AND r.state IN ('dispatching','dispatched') AND r.worker_cleanup_state IS NULL THEN 'pending' ELSE r.worker_cleanup_state END)
       OR a.worker_cleanup_failure_category IS NOT r.worker_cleanup_failure_category
       OR a.worker_cleanup_failure_detail IS NOT r.worker_cleanup_failure_detail
       OR a.worker_cleanup_updated_at IS NOT r.worker_cleanup_updated_at
       OR a.worker_cleanup_completed_at IS NOT r.worker_cleanup_completed_at
)
    "#,[],|row|row.get(0))?;
    if drift {
        return Err(Error::Store("pre-content Job/attempt changed after its retained cutover; preserve evidence and reconcile before final migration".into()));
    }
    Ok(())
}

fn require_transaction(conn: &Connection) -> Result<()> {
    if conn.is_autocommit() {
        return Err(Error::Store(
            "Backend Job upgrade requires a caller-owned transaction".into(),
        ));
    }
    Ok(())
}

/// Fresh creation installs empty evidence and final guards, never captures or
/// fails Jobs. The fresh canonical state CHECKs are supplied by the owner DDL.
pub(super) fn install_fresh(conn: &Connection) -> Result<()> {
    require_transaction(conn)?;
    conn.execute_batch(RAW_SCHEMA)?;
    install_live_checks(conn)?;
    conn.execute_batch(SCHEMA)?;
    conn.execute_batch(
        "ALTER TABLE backend_job_attempts ADD COLUMN allocation_precluded_at TEXT;",
    )?;
    conn.execute_batch(super::backend_job_recovery_store::SCHEMA)?;
    verify(conn)
}

pub(super) fn install_live_checks(conn: &Connection) -> Result<()> {
    for (table, old, new) in STATE_CHECKS {
        replace_state_check(conn, table, old, new)?;
    }
    Ok(())
}

/// The retained pre-123 reference composition must keep its original CHECKs,
/// even when the fresh owner DDL is updated to the current live contract.
pub(super) fn retained_checks(sql: &str) -> String {
    STATE_CHECKS.iter().fold(
        sql.replace("            allocation_precluded_at TEXT,\n", ""),
        |sql, (_, old, new)| sql.replace(new, old),
    )
}

const SNAPSHOT_SQL: &str = r#"
INSERT INTO backend_job_upgrade_history
    (workspace_id,job_id,state,failure_category,failure_detail,updated_at,completed_at)
SELECT workspace_id,job_id,state,failure_category,failure_detail,updated_at,completed_at FROM backend_jobs;
INSERT INTO backend_job_attempt_upgrade_history
    (workspace_id,job_id,attempt_id,state,failure_category,failure_detail,updated_at,completed_at)
SELECT workspace_id,job_id,attempt_id,state,failure_category,failure_detail,updated_at,completed_at FROM backend_job_attempts;
"#;

// Preserve result/input/identity and every execution timestamp, including old
// completed_at. Migration time belongs to history, not execution-end evidence.
const FAIL_JOBS_SQL: &str = r#"
UPDATE backend_jobs SET state='failed',failure_category=?1,failure_detail=?2
WHERE state NOT IN ('completed','failed');
"#;
const FAIL_ATTEMPTS_SQL: &str = r#"
UPDATE backend_job_attempts SET state='failed',failure_category=?1,failure_detail=?2
WHERE state NOT IN ('completed','failed');
"#;

// Restore only captured values, not inferred execution-end evidence from the
// retained content cutover. Live normalized input/digest/fingerprint remain in
// their current format; their original wire identity is retained in raw evidence.
const APPLY_CAPTURED_JOBS_SQL: &str = r#"
UPDATE backend_jobs AS j SET (state,failure_category,failure_detail,updated_at,completed_at) = (
    SELECT CASE WHEN r.state IN ('completed','failed') THEN r.state ELSE 'failed' END,
           CASE WHEN r.state IN ('completed','failed') THEN r.failure_category ELSE ?1 END,
           CASE WHEN r.state IN ('completed','failed') THEN r.failure_detail ELSE ?2 END,
           r.updated_at,r.completed_at
    FROM backend_job_pre_content_history r WHERE r.workspace_id=j.workspace_id AND r.job_id=j.job_id
) WHERE EXISTS(SELECT 1 FROM backend_job_pre_content_history r WHERE r.workspace_id=j.workspace_id AND r.job_id=j.job_id);
"#;
const APPLY_CAPTURED_ATTEMPTS_SQL: &str = r#"
UPDATE backend_job_attempts AS a SET
    (state,failure_category,failure_detail,updated_at,completed_at,
     worker_cleanup_state,worker_cleanup_failure_category,worker_cleanup_failure_detail,worker_cleanup_updated_at,worker_cleanup_completed_at) = (
    SELECT CASE WHEN r.state IN ('completed','failed') THEN r.state ELSE 'failed' END,
           CASE WHEN r.state IN ('completed','failed') THEN r.failure_category ELSE ?1 END,
           CASE WHEN r.state IN ('completed','failed') THEN r.failure_detail ELSE ?2 END,
           r.updated_at,r.completed_at,
           r.worker_cleanup_state,r.worker_cleanup_failure_category,r.worker_cleanup_failure_detail,r.worker_cleanup_updated_at,r.worker_cleanup_completed_at
    FROM backend_job_attempt_pre_content_history r
    WHERE r.workspace_id=a.workspace_id AND r.job_id=a.job_id AND r.attempt_id=a.attempt_id
) WHERE EXISTS(SELECT 1 FROM backend_job_attempt_pre_content_history r
               WHERE r.workspace_id=a.workspace_id AND r.job_id=a.job_id AND r.attempt_id=a.attempt_id);
"#;

// Exact retained CHECKs only. Published Jobs-family tables may already have the
// new CHECK; the physical recognizer, not this function, owns family acceptance.
const STATE_CHECKS: [(&str, &str, &str); 2] = [
    (
        "backend_jobs",
        "CHECK (state IN ('pending', 'completed', 'failed', 'unknown'))",
        "CHECK (state IN ('pending', 'completed', 'failed'))",
    ),
    (
        "backend_job_attempts",
        "CHECK (state IN ('reserved', 'dispatching', 'dispatched', 'completed', 'failed', 'unknown'))",
        "CHECK (state IN ('reserved', 'dispatching', 'dispatched', 'completed', 'failed'))",
    ),
];

/// Apply once in the caller's validated Workspace migration transaction.
/// On ANY error the caller must roll back, including DDL and its version receipt.
/// A second invocation is an error, not a new sweep of post-upgrade Jobs.
pub(super) fn install(conn: &Connection) -> Result<()> {
    require_transaction(conn)?;
    let foreign_keys: bool = conn.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
    if foreign_keys {
        return Err(Error::Store(
            "Backend Job upgrade requires foreign_keys OFF before the transaction".into(),
        ));
    }
    for table in ["backend_jobs", "backend_job_attempts"] {
        if !super::column_exists(conn, table, "input_digest")?
            || super::column_exists(conn, table, "input_revision")?
        {
            return Err(Error::Store(
                "Backend Job final cutover requires the retained content normalization first"
                    .into(),
            ));
        }
    }
    let raw_tables = [
        "backend_job_pre_content_history",
        "backend_job_attempt_pre_content_history",
        "backend_job_delivery_pre_content_history",
    ];
    let present = raw_tables
        .into_iter()
        .map(|name| super::table_exists(conn, name))
        .collect::<Result<Vec<_>>>()?;
    if present.iter().all(|exists| !exists) {
        // Sources that were already schema 86+ cannot regain lost pre-86 facts.
        conn.execute_batch(RAW_SCHEMA)?;
    } else if !present.iter().all(|exists| *exists) {
        return Err(Error::Store(
            "incomplete pre-content Job evidence schema".into(),
        ));
    }
    verify_pre_content(conn)?;
    validate_content_cutover_before_restore(conn)?;
    let (history_schema, execution_guards) = SCHEMA
        .split_once("-- FINAL_EXECUTION_GUARDS")
        .ok_or_else(|| Error::Store("missing final Backend Job guard boundary".into()))?;
    conn.execute_batch(history_schema)?;
    conn.execute_batch(SNAPSHOT_SQL)?;
    for sql in [
        FAIL_JOBS_SQL,
        FAIL_ATTEMPTS_SQL,
        APPLY_CAPTURED_JOBS_SQL,
        APPLY_CAPTURED_ATTEMPTS_SQL,
    ] {
        conn.execute(sql, [FAILURE_CATEGORY, FAILURE_DETAIL])?;
    }
    for (table, old, new) in STATE_CHECKS {
        replace_state_check(conn, table, old, new)?;
    }
    // A captured originally-unfinished row may already be Failed by schema 86.
    // Install terminal immutability only after replacing that synthetic outcome.
    conn.execute_batch(execution_guards)?;
    // Old Failed/Reserved rows acquire no negative allocation proof from migration.
    conn.execute_batch(
        "ALTER TABLE backend_job_attempts ADD COLUMN allocation_precluded_at TEXT;",
    )?;
    conn.execute_batch(super::backend_job_recovery_store::SCHEMA)?;
    verify(conn)
}

fn quoted(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

// Recreate under the SAME table name, without ALTER RENAME or writable_schema.
// This avoids rewriting FKs/views/triggers attached to other family tables. Copy
// every physical column, including published cleanup ownership/admission fields;
// don't replace family DDL with a baseline or infer any missing termination rows.
fn replace_state_check(conn: &Connection, table: &str, old: &str, new: &str) -> Result<()> {
    let ddl: String = conn.query_row(
        "SELECT sql FROM sqlite_schema WHERE type='table' AND name=?1",
        [table],
        |row| row.get(0),
    )?;
    if ddl.matches(new).count() == 1 && !ddl.contains(old) {
        return Ok(());
    }
    if ddl.matches(old).count() != 1 || ddl.contains(new) {
        return Err(Error::Store(format!(
            "Backend Job upgrade: unrecognized state CHECK in {table}"
        )));
    }
    let programs = conn.prepare(
        "SELECT sql FROM sqlite_schema WHERE tbl_name=?1 AND type IN ('index','trigger') AND sql IS NOT NULL ORDER BY type,name",
    )?.query_map([table], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let columns = conn
        .prepare(&format!("PRAGMA table_xinfo({})", quoted(table)))?
        .query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, i64>(6)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if columns.iter().any(|(_, hidden)| *hidden != 0) {
        return Err(Error::Store(format!(
            "Backend Job upgrade: unsupported generated/hidden column in {table}"
        )));
    }
    let columns = columns
        .iter()
        .map(|(name, _)| quoted(name))
        .collect::<Vec<_>>()
        .join(",");
    let temporary = quoted(&format!("__yoi_upgrade_{table}"));
    let table_name = quoted(table);
    conn.execute_batch(&format!(
        "CREATE TEMP TABLE {temporary} AS SELECT {columns} FROM main.{table_name};
         DROP TABLE main.{table_name};"
    ))?;
    conn.execute_batch(&ddl.replacen(old, new, 1))?;
    conn.execute_batch(&format!(
        "INSERT INTO main.{table_name} ({columns}) SELECT {columns} FROM temp.{temporary};
         DROP TABLE temp.{temporary};"
    ))?;
    for sql in programs {
        conn.execute_batch(&sql)?;
    }
    Ok(())
}

/// Validate ownership and immutable content without normalizing or restoring any
/// body. This also runs before the schema-86 receipt, when input_revision is live.
/// Cleanup/delivery outcomes may legitimately evolve after migration; they are
/// historical evidence, not shadow operational state machines.
pub(super) fn verify_pre_content(conn: &Connection) -> Result<()> {
    let invalid: bool = conn.query_row(r#"
        SELECT EXISTS(
            SELECT 1 FROM backend_job_pre_content_history r JOIN backend_jobs j
              ON j.workspace_id=r.workspace_id AND j.job_id=r.job_id
            WHERE j.purpose IS NOT r.purpose OR j.input_ref IS NOT r.input_ref
               OR j.resource_key IS NOT r.resource_key OR j.current_attempt IS NOT r.current_attempt
               OR j.result_digest IS NOT r.result_digest
               OR (SELECT count(*) FROM backend_job_attempts a WHERE a.workspace_id=r.workspace_id AND a.job_id=r.job_id)
                  != (SELECT count(*) FROM backend_job_attempt_pre_content_history a WHERE a.workspace_id=r.workspace_id AND a.job_id=r.job_id)
        ) OR EXISTS(
            SELECT 1 FROM backend_job_attempt_pre_content_history r JOIN backend_job_attempts a
              ON a.workspace_id=r.workspace_id AND a.job_id=r.job_id AND a.attempt_id=r.attempt_id
            WHERE a.attempt IS NOT r.attempt OR a.runtime_id IS NOT r.runtime_id OR a.worker_id IS NOT r.worker_id
               OR a.runtime_run_id IS NOT r.runtime_run_id OR a.dispatched_at IS NOT r.dispatched_at
               OR a.deadline_at IS NOT r.deadline_at OR a.result_digest IS NOT r.result_digest
        ) OR EXISTS(
            SELECT 1 FROM backend_job_delivery_pre_content_history r JOIN backend_job_deliveries d
              ON d.workspace_id=r.workspace_id AND d.delivery_id=r.delivery_id
            WHERE d.job_id IS NOT r.job_id OR d.attempt_id IS NOT r.attempt_id
               OR d.target_runtime_id IS NOT r.target_runtime_id OR d.target_worker_id IS NOT r.target_worker_id
        )
    "#, [], |row| row.get(0))?;
    if invalid {
        return Err(Error::Store(
            "pre-content Job identity/result/attempt coverage drift".into(),
        ));
    }
    let mut statement = conn.prepare("SELECT r.request_json,j.request_json,r.input_retired FROM backend_job_pre_content_history r JOIN backend_jobs j ON j.workspace_id=r.workspace_id AND j.job_id=r.job_id")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let original: serde_json::Value = serde_json::from_str(&row.get::<_, String>(0)?)
            .map_err(|_| Error::Store("invalid pre-content request evidence".into()))?;
        let current: serde_json::Value = serde_json::from_str(&row.get::<_, String>(1)?)
            .map_err(|_| Error::Store("invalid live request beside pre-content evidence".into()))?;
        let retired: bool = row.get(2)?;
        let original_input = original.get("input");
        let current_input = current.get("input");
        let original_empty = original_input.is_none_or(serde_json::Value::is_null);
        let current_empty = current_input.is_none_or(serde_json::Value::is_null);
        if (retired && (!original_empty || !current_empty))
            || (!retired && (original_empty || original_input != current_input))
        {
            return Err(Error::Store(
                "pre-content immutable input lost or regenerated".into(),
            ));
        }
    }
    Ok(())
}

/// Read-only postcondition; never sweeps or repairs post-migration Jobs.
pub(super) fn verify(conn: &Connection) -> Result<()> {
    verify_pre_content(conn)?;
    for (table, old, new) in STATE_CHECKS {
        let ddl: String = conn.query_row(
            "SELECT sql FROM sqlite_schema WHERE type='table' AND name=?1",
            [table],
            |row| row.get(0),
        )?;
        if ddl.contains(old) || ddl.matches(new).count() != 1 {
            return Err(Error::Store(format!(
                "Backend Job upgrade left a legacy live-state CHECK in {table}"
            )));
        }
    }
    for (table, history, raw_history, identity) in [
        (
            "backend_jobs",
            "backend_job_upgrade_history",
            "backend_job_pre_content_history",
            "a.job_id=h.job_id",
        ),
        (
            "backend_job_attempts",
            "backend_job_attempt_upgrade_history",
            "backend_job_attempt_pre_content_history",
            "a.job_id=h.job_id AND a.attempt_id=h.attempt_id",
        ),
    ] {
        let raw_identity = identity.replace("h.", "r.");
        let basis = "CASE WHEN r.job_id IS NULL THEN h.state ELSE r.state END";
        let category =
            "CASE WHEN r.job_id IS NULL THEN h.failure_category ELSE r.failure_category END";
        let detail = "CASE WHEN r.job_id IS NULL THEN h.failure_detail ELSE r.failure_detail END";
        let completed = "CASE WHEN r.job_id IS NULL THEN h.completed_at ELSE r.completed_at END";
        let invalid: bool = conn.query_row(
            &format!(
                "SELECT EXISTS(SELECT 1 FROM {table} a JOIN {history} h
                 ON a.workspace_id=h.workspace_id AND {identity}
                 LEFT JOIN {raw_history} r ON a.workspace_id=r.workspace_id AND {raw_identity}
                 WHERE a.state NOT IN ('completed','failed') OR a.completed_at IS NOT ({completed})
                    OR (({basis}) IN ('completed','failed')
                        AND (a.state IS NOT ({basis}) OR a.failure_category IS NOT ({category})
                             OR a.failure_detail IS NOT ({detail})))
                    OR (({basis}) NOT IN ('completed','failed')
                        AND (a.state!='failed' OR a.failure_category IS NOT ?1 OR a.failure_detail IS NOT ?2)))"
            ),
            [FAILURE_CATEGORY, FAILURE_DETAIL],
            |row| row.get(0),
        )?;
        if invalid {
            return Err(Error::Store(format!(
                "Backend Job upgrade left an unretired row in {table}"
            )));
        }
    }
    let violations: i64 =
        conn.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })?;
    if violations != 0 {
        return Err(Error::Store(
            "Backend Job upgrade left foreign-key violations".into(),
        ));
    }
    Ok(())
}

/// Reuse the storage owner's exact published-family fixtures in its normal suite.
#[cfg(test)]
pub(super) fn assert_published_fixture_for_test(conn: &Connection) {
    tests::assert_published_fixture(conn);
}

#[cfg(test)]
#[path = "backend_job_upgrade_tests.rs"]
mod tests;
