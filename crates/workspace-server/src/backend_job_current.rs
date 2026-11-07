//! Schema-130 boundary: discard obsolete Job execution records, not their output.
//!
//! The storage lifecycle owner must quiesce all external Server/Runtime writers
//! before invoking this migration, including writers from older binaries. A local
//! SQLite transaction does not revoke their authority. Removing rows is NOT
//! execution termination, writer revocation, Journal completion or external
//! resource release. This module never
//! contacts a Runtime or Feature store, nor fabricates cleanup/removal receipts.
//! Worker allocation/removal records and Subject erasure fences retain their own
//! owners. An external running resource must be quiesced by that owner before the
//! destructive boundary; a terminal Job/cleanup flag is not proof of quiescence.
use super::{Connection, Error, Result};
use rusqlite::TransactionBehavior;

pub(super) const MIGRATION_NAME: &str =
    "Discard obsolete Backend Jobs and retain only the current contract";

pub(super) const OBSOLETE_TABLES: &[&str] = &[
    "backend_job_deliveries",
    "notification_job_legacy_receipts",
    "backend_job_delivery_pre_content_history",
    "backend_job_attempt_pre_content_history",
    "backend_job_pre_content_history",
    "backend_job_attempt_upgrade_history",
    "backend_job_upgrade_history",
    "backend_job_attempt_legacy_state_history",
    "backend_job_legacy_state_history",
];

pub(super) const OBSOLETE_LIVE_TRIGGERS: &[&str] = &[
    "notification_job_legacy_no_update",
    "notification_job_legacy_no_delete",
    "backend_job_pre_content_retire_with_owner",
    "backend_job_pre_content_no_input_restore",
    "backend_job_upgrade_no_resume",
    "backend_job_upgrade_no_attempt",
    "backend_job_upgrade_no_attempt_reparent",
    "backend_job_attempt_upgrade_no_resume",
    "backend_job_reconciliation_no_legacy",
];

const NOTIFICATION_DELETE_GUARDS: &[&str] = &[
    "notification_sources_no_delete",
    "notification_decisions_no_delete",
    "notification_requests_no_delete",
    "notification_acknowledgements_no_delete",
    "notification_failures_no_delete",
    "notification_job_targets_no_delete",
    "notification_job_legacy_no_delete",
    "notification_source_preparation_failures_no_delete",
];

const OLD_DISPOSITION: &str =
    "CHECK (input_disposition IN ('available','reclaimed','erased','invalidated'))";
const CURRENT_DISPOSITION: &str = "CHECK (input_disposition IN ('available','reclaimed','erased'))";

/// Ordered owner entrypoint, never an ordinary open/recovery repair. The receipt,
/// row removal, DDL and temporarily removed immutable guards commit atomically.
pub(super) fn migrate(conn: &Connection) -> Result<()> {
    if !conn.is_autocommit() || super::current_schema_version(conn)? != 129 {
        return Err(Error::Store(
            "current-only Backend Jobs require the schema-129 boundary outside a transaction"
                .into(),
        ));
    }
    super::storage_formats::verify_history(conn, 129)?;
    require_foreign_keys_clean(conn)?;
    let foreign_keys: bool = conn.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
    conn.execute_batch("PRAGMA foreign_keys=OFF;")?;
    let result = (|| -> Result<()> {
        let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Exclusive)?;
        // Recheck after obtaining the write lock, not only during planning.
        if super::current_schema_version(&tx)? != 129 {
            return Err(Error::Store(
                "Backend Job migration predecessor changed".into(),
            ));
        }
        super::storage_formats::verify_history(&tx, 129)?;
        discard_obsolete(&tx)?;
        tx.execute(
            "INSERT INTO __yoi_schema_migrations(version,name) VALUES(130,?1)",
            [MIGRATION_NAME],
        )?;
        super::storage_formats::verify_history(&tx, 130)?;
        require_foreign_keys_clean(&tx)?;
        tx.commit()?;
        Ok(())
    })();
    let restore = conn
        .execute_batch(if foreign_keys {
            "PRAGMA foreign_keys=ON;"
        } else {
            "PRAGMA foreign_keys=OFF;"
        })
        .map_err(Error::from);
    result.and(restore)
}

fn discard_obsolete(conn: &Connection) -> Result<()> {
    // Do not normalize aliases into a new request/fingerprint or regenerate old
    // input. Legacy delivery evidence retires its owner, even if that owner's
    // descriptor looks current: dropping only the ACK would permit a resend.
    conn.execute_batch(
        r#"
        CREATE TEMP TABLE __yoi_obsolete_jobs (
            workspace_id TEXT NOT NULL, job_id TEXT NOT NULL,
            PRIMARY KEY(workspace_id,job_id)
        );
        INSERT INTO __yoi_obsolete_jobs
        SELECT workspace_id,job_id FROM backend_job_upgrade_history
        UNION SELECT workspace_id,job_id FROM backend_job_pre_content_history
        UNION SELECT workspace_id,job_id FROM backend_job_deliveries
        UNION SELECT s.workspace_id,json_extract(s.facts_json,'$.backend_job.job_id')
              FROM notification_job_legacy_receipts r
              JOIN notification_decisions d USING(workspace_id,delivery_id)
              JOIN notification_sources s ON s.sequence=d.source_sequence
              WHERE s.role='backend_job'
                AND json_type(s.facts_json,'$.backend_job.job_id')='text'
        UNION SELECT workspace_id,job_id FROM backend_jobs
              WHERE input_disposition='invalidated'
                 OR CASE WHEN json_valid(request_json)
                         THEN json_type(request_json,'$.input_revision') IS NOT NULL ELSE 0 END;
    "#,
    )?;
    // Structural decoding is classification only, never domain validation or
    // dispatch. In particular, retired input=null is still a current descriptor.
    // deny_unknown_fields and required grant fields (including Journal binding_id)
    // reject old envelopes even when no upgrade marker was ever recorded.
    let requests = conn
        .prepare("SELECT workspace_id,job_id,request_json FROM backend_jobs")?
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (workspace, job, request) in requests {
        if serde_json::from_str::<crate::backend_job::BackendJobRequest>(&request).is_err() {
            conn.execute(
                "INSERT OR IGNORE INTO __yoi_obsolete_jobs VALUES(?1,?2)",
                [&workspace, &job],
            )?;
        }
    }
    for table in [
        "backend_job_legacy_state_history",
        "backend_job_attempt_legacy_state_history",
    ] {
        if super::table_exists(conn, table)? {
            conn.execute_batch(&format!(
                "INSERT OR IGNORE INTO __yoi_obsolete_jobs SELECT workspace_id,job_id FROM {table};"
            ))?;
        }
    }
    // Retain exact removed identities for the managed owner's separate Feature
    // continuation. Capture and destructive catalog changes commit together.
    super::backend_job_reference_migration::capture(conn)?;
    // Source identity, not purpose/text or a guessed delivery-id convention,
    // selects the entire graph, including sources without routing decisions.
    conn.execute_batch(
        r#"
        CREATE TEMP TABLE __yoi_obsolete_sources AS
        SELECT s.sequence,s.workspace_id,s.input_event_id FROM notification_sources s
        JOIN __yoi_obsolete_jobs j ON j.workspace_id=s.workspace_id
          AND j.job_id=json_extract(s.facts_json,'$.backend_job.job_id')
        WHERE s.role='backend_job'
        UNION SELECT s.sequence,s.workspace_id,s.input_event_id
        FROM notification_sources s JOIN notification_decisions d ON d.source_sequence=s.sequence
        JOIN notification_job_legacy_receipts r USING(workspace_id,delivery_id)
        UNION SELECT s.sequence,s.workspace_id,s.input_event_id
        FROM notification_sources s JOIN backend_job_deliveries d
          ON d.workspace_id=s.workspace_id AND d.delivery_id=s.input_event_id
        WHERE s.role='backend_job';
        CREATE TEMP TABLE __yoi_obsolete_deliveries AS
        SELECT d.workspace_id,d.delivery_id FROM notification_decisions d
        JOIN __yoi_obsolete_sources s ON s.sequence=d.source_sequence;
    "#,
    )?;
    let mut guards = Vec::new();
    for name in NOTIFICATION_DELETE_GUARDS {
        let sql: String = conn.query_row(
            "SELECT sql FROM sqlite_schema WHERE type='trigger' AND name=?1",
            [name],
            |row| row.get(0),
        )?;
        guards.push(sql);
        conn.execute_batch(&format!("DROP TRIGGER {name};"))?;
    }
    for table in [
        "notification_acknowledgements",
        "notification_job_targets",
        "notification_job_legacy_receipts",
        "notification_failures",
        "notification_requests",
    ] {
        conn.execute_batch(&format!("DELETE FROM {table} WHERE (workspace_id,delivery_id) IN (SELECT workspace_id,delivery_id FROM __yoi_obsolete_deliveries);"))?;
    }
    for table in [
        "notification_source_preparation_failures",
        "notification_source_offers",
        "notification_decisions",
    ] {
        conn.execute_batch(&format!("DELETE FROM {table} WHERE source_sequence IN (SELECT sequence FROM __yoi_obsolete_sources);"))?;
    }
    conn.execute_batch("DELETE FROM notification_sources WHERE sequence IN (SELECT sequence FROM __yoi_obsolete_sources);")?;
    for sql in guards {
        conn.execute_batch(&sql)?;
    }

    // FK enforcement is off for the same-name CHECK rebuild. Explicitly remove
    // every retained Job-owned child (including optional published-family tables)
    // rather than assume CASCADE has run. Independent removal/Subject records stay.
    for table in [
        "backend_job_delivery_pre_content_history",
        "backend_job_deliveries",
        "backend_job_attempt_pre_content_history",
        "backend_job_attempt_upgrade_history",
        "backend_job_attempt_legacy_state_history",
        "backend_job_reconciliation",
        "backend_job_execution_fences",
        "backend_job_recovery_probes",
        "backend_journal_cleanup_removals",
        "backend_job_cleanup_invocation_ends",
        "backend_job_cleanup_invocation_owners",
        "backend_job_cleanup_history_origins",
        "backend_job_attempts",
        "backend_job_pre_content_history",
        "backend_job_upgrade_history",
        "backend_job_legacy_state_history",
        "backend_jobs",
    ] {
        if super::table_exists(conn, table)? {
            conn.execute_batch(&format!("DELETE FROM {table} WHERE (workspace_id,job_id) IN (SELECT workspace_id,job_id FROM __yoi_obsolete_jobs);"))?;
        }
    }
    for name in OBSOLETE_LIVE_TRIGGERS {
        conn.execute_batch(&format!("DROP TRIGGER IF EXISTS {name};"))?;
    }
    for name in OBSOLETE_TABLES {
        conn.execute_batch(&format!("DROP TABLE IF EXISTS {name};"))?;
    }
    rebuild_job_input_contract(conn)?;
    conn.execute_batch("DROP TABLE temp.__yoi_obsolete_deliveries; DROP TABLE temp.__yoi_obsolete_sources; DROP TABLE temp.__yoi_obsolete_jobs;")?;
    verify(conn)
}

fn rebuild_job_input_contract(conn: &Connection) -> Result<()> {
    let ddl: String = conn.query_row(
        "SELECT sql FROM sqlite_schema WHERE name='backend_jobs' AND type='table'",
        [],
        |row| row.get(0),
    )?;
    if ddl.matches(OLD_DISPOSITION).count() != 1 {
        return Err(Error::Store(
            "unrecognized retained Job input disposition CHECK".into(),
        ));
    }
    let programs = conn.prepare("SELECT name,sql FROM sqlite_schema WHERE tbl_name='backend_jobs' AND type IN ('index','trigger') AND sql IS NOT NULL ORDER BY type,name")?
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    // Same name, all physical columns and bytes, no ALTER RENAME FK rewriting.
    // Copy before restoring INSERT guards: retained Subject erasure fences must
    // not reject their own already-stored rows during a physical rebuild.
    conn.execute_batch("CREATE TEMP TABLE __yoi_current_jobs AS SELECT * FROM backend_jobs; DROP TABLE backend_jobs;")?;
    conn.execute_batch(&ddl.replacen(OLD_DISPOSITION, CURRENT_DISPOSITION, 1))?;
    conn.execute_batch("INSERT INTO backend_jobs SELECT * FROM temp.__yoi_current_jobs; DROP TABLE temp.__yoi_current_jobs;")?;
    for (name, sql) in programs {
        if name != "backend_job_identity_immutable" && name != "backend_job_input_retirement" {
            conn.execute_batch(&sql)?;
        }
    }
    conn.execute_batch(crate::backend_job::INPUT_RETENTION_TRIGGERS)?;
    Ok(())
}

/// Current-schema postcondition; no old-row allow/deny marker or repair sweep.
pub(super) fn verify(conn: &Connection) -> Result<()> {
    for name in OBSOLETE_TABLES.iter().chain(OBSOLETE_LIVE_TRIGGERS) {
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name=?1)",
            [name],
            |row| row.get(0),
        )?;
        if exists {
            return Err(Error::Store(format!(
                "obsolete Job schema object remains: {name}"
            )));
        }
    }
    let ddl: String = conn.query_row(
        "SELECT sql FROM sqlite_schema WHERE name='backend_jobs'",
        [],
        |row| row.get(0),
    )?;
    if ddl.contains(OLD_DISPOSITION) || ddl.matches(CURRENT_DISPOSITION).count() != 1 {
        return Err(Error::Store(
            "current Job input disposition CHECK missing".into(),
        ));
    }
    require_foreign_keys_clean(conn)
}

fn require_foreign_keys_clean(conn: &Connection) -> Result<()> {
    let violations: i64 =
        conn.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })?;
    if violations != 0 {
        return Err(Error::Store(
            "current-only Backend Job migration found foreign-key violations".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "backend_job_current_tests.rs"]
mod tests;
