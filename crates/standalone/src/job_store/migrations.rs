//! One-time stored-schema boundary. Runtime Job operations know only schema 10.
use super::*;

#[cfg(test)]
#[path = "migration_tests.rs"]
pub(super) mod tests;

/// Rebuild under an immediate transaction with FK enforcement disabled outside it.
/// Never rename a parent: extension FKs must retain their canonical targets.
/// A failed check rolls back DDL, removed Jobs, and user_version together.
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
            0 => create_current(&tx)?,
            1..=9 => migrate(&tx, version)?,
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
    // Attempt both restorations even when migration or the first restoration fails.
    let restore_alter = conn.pragma_update(None, "legacy_alter_table", legacy_alter_table);
    let restore_foreign_keys = conn.pragma_update(None, "foreign_keys", foreign_keys);
    result?;
    restore_alter?;
    restore_foreign_keys?;
    Ok(())
}

fn create_current(conn: &Connection) -> Result<(), JobStoreError> {
    conn.execute_batch(super::SCHEMA)?;
    conn.execute_batch(super::failure::SCHEMA)?;
    conn.execute_batch(GRANT_SCHEMA)?;
    conn.execute_batch(RECONCILIATION_SCHEMA)?;
    conn.execute_batch(recovery::SCHEMA)?;
    conn.execute_batch(recovery::START_REJECTION_CURRENT_SCHEMA)?;
    conn.execute_batch(super::reference_retirement::SCHEMA)?;
    Ok(())
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool, JobStoreError> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?1)",
        [table],
        |r| r.get(0),
    )?)
}

fn quoted(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn columns(conn: &Connection, table: &str) -> Result<Vec<String>, JobStoreError> {
    Ok(conn
        .prepare("SELECT name FROM pragma_table_info(?1) ORDER BY cid")?
        .query_map([table], |r| r.get(0))?
        .collect::<Result<Vec<_>, _>>()?)
}

fn migrate(conn: &Connection, version: i64) -> Result<(), JobStoreError> {
    conn.execute_batch("CREATE TEMP TABLE obsolete_jobs(job_id TEXT PRIMARY KEY NOT NULL);")?;
    if version <= 5 {
        // Every pre-v6 Job belongs to the retired protocol. No decoding or
        // rewriting of historical payloads is necessary to remove its graph.
        conn.execute_batch("INSERT INTO obsolete_jobs SELECT job_id FROM job_intents;")?;
    } else {
        conn.execute_batch(
            "INSERT INTO obsolete_jobs SELECT job_id FROM job_intents WHERE input_disposition='invalidated';",
        )?;
        if table_exists(conn, "job_version_stops")? {
            conn.execute_batch(
                "INSERT OR IGNORE INTO obsolete_jobs SELECT job_id FROM job_version_stops;",
            )?;
        }
        if table_exists(conn, "legacy_job_archive")? {
            conn.execute_batch(
                "INSERT OR IGNORE INTO obsolete_jobs SELECT job_id FROM legacy_job_archive;",
            )?;
        }
        classify_stored_jobs(conn)?;
    }

    // Capture the exact removal set before deleting grants or rebuilding the graph.
    // The Host drains this durable cross-database work only after migration commits.
    conn.execute_batch(super::reference_retirement::SCHEMA)?;
    let grants = if table_exists(conn, "job_domain_grants")? {
        "EXISTS(SELECT 1 FROM job_domain_grants g WHERE g.job_id=o.job_id)"
    } else {
        "0"
    };
    conn.execute_batch(&format!(
        "INSERT OR IGNORE INTO job_reference_retirements(job_id,domain_references)
         SELECT o.job_id, {grants} OR CASE WHEN json_valid(i.request_json)
           THEN COALESCE(json_extract(i.request_json,'$.purpose') IN
             ('subjektiv_journal','subjektiv_consolidation'),0) ELSE 1 END
         FROM obsolete_jobs o LEFT JOIN job_intents i ON i.job_id=o.job_id;"
    ))?;

    // Child tables stay in place, including their original rowids and bytes.
    // Suspend ONLY known retention guards; unknown extensions are never dropped
    // or cascaded. Their triggers may refuse removal, and their FKs are checked
    // before committing. Either refusal safely rolls the entire migration back.
    for (table, trigger) in [
        ("job_domain_grants", "job_grant_retained"),
        ("job_domain_reconciliations", "job_reconciliation_retained"),
        ("job_execution_ownership", "job_execution_retained"),
        ("job_start_rejections", "job_start_rejection_retained"),
    ] {
        if !table_exists(conn, table)? {
            continue;
        }
        let guard: Option<String> = conn
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type='trigger' AND name=?1",
                [trigger],
                |r| r.get(0),
            )
            .optional()?;
        if guard.is_some() {
            conn.execute_batch(&format!("DROP TRIGGER {}", quoted(trigger)))?;
        }
        conn.execute_batch(&format!(
            "DELETE FROM {} WHERE job_id IN (SELECT job_id FROM obsolete_jobs)",
            quoted(table),
        ))?;
        if let Some(guard) = guard {
            conn.execute_batch(&guard)?;
        }
    }

    rebuild_core(conn, version)?;
    if version == 1 {
        conn.execute_batch(GRANT_SCHEMA)?;
    }
    if version <= 3 {
        conn.execute_batch(RECONCILIATION_SCHEMA)?;
    }
    if version <= 5 {
        conn.execute_batch(recovery::SCHEMA)?;
    }
    if version <= 6 {
        conn.execute_batch(recovery::START_REJECTION_CURRENT_SCHEMA)?;
    } else {
        // Existing rejection receipts are immutable. Replace only their insertion
        // guard, not the rows, and remove the retired version-stop dependency.
        let schema = recovery::START_REJECTION_CURRENT_SCHEMA;
        let start = schema
            .find("CREATE TRIGGER job_start_rejection_guard")
            .unwrap();
        let end = schema
            .find("CREATE TRIGGER job_start_rejection_immutable")
            .unwrap();
        conn.execute_batch("DROP TRIGGER job_start_rejection_guard;")?;
        conn.execute_batch(&schema[start..end])?;
    }
    for table in [
        "job_version_stops",
        "job_version_history",
        "legacy_job_archive",
    ] {
        conn.execute_batch(&format!("DROP TABLE IF EXISTS {}", quoted(table)))?;
    }
    conn.execute_batch("DROP TABLE obsolete_jobs;")?;
    Ok(())
}

fn classify_stored_jobs(conn: &Connection) -> Result<(), JobStoreError> {
    let rows = conn
        .prepare(
            "SELECT i.job_id,i.request_json,i.input_disposition,g.grant_json
         FROM job_intents i LEFT JOIN job_domain_grants g ON g.job_id=i.job_id
         WHERE i.job_id NOT IN (SELECT job_id FROM obsolete_jobs)",
        )?
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (id, body, disposition, grant) in rows {
        // Malformed JSON aborts safely. Only the retired top-level binding or
        // the exact Journal purpose identifies an obsolete stored contract;
        // opaque input and result objects are never searched for legacy fields.
        let value: Value = serde_json::from_str(&body)?;
        if value.get("input_revision").is_some() {
            conn.execute("INSERT INTO obsolete_jobs VALUES(?1)", [&id])?;
            continue;
        }
        if value.get("purpose").and_then(Value::as_str) != Some("subjektiv_journal") {
            continue;
        }
        let request = serde_json::from_value::<JobRequest>(value);
        let grant = grant
            .as_deref()
            .map(serde_json::from_str::<Value>)
            .transpose();
        let valid = match (request, grant) {
            (Ok(request), Ok(grant)) => {
                crate::subjektiv::LocalSubjectHost::validate_stored_journal_job(
                    &request,
                    &grant.unwrap_or(Value::Null),
                    disposition == "available",
                )
                .is_ok()
            }
            _ => false,
        };
        if !valid {
            conn.execute("INSERT INTO obsolete_jobs VALUES(?1)", [&id])?;
        }
    }
    Ok(())
}

fn rebuild_core(conn: &Connection, version: i64) -> Result<(), JobStoreError> {
    let objects = conn
        .prepare(
            "SELECT name,sql,tbl_name FROM sqlite_schema
         WHERE tbl_name IN ('job_intents','job_attempts')
           AND type IN ('index','trigger') AND sql IS NOT NULL ORDER BY type,name",
        )?
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let intents = columns(conn, "job_intents")?;
    let attempts = columns(conn, "job_attempts")?;
    // SQL copies preserve exact TEXT/BLOB values, fingerprints, result receipts,
    // typed failure attachments, historical attempts, and ordering rowids. No
    // serde roundtrip or runtime admission trigger runs over surviving records.
    conn.execute_batch(
        "CREATE TEMP TABLE migration_intents AS
             SELECT rowid AS migration_rowid,* FROM job_intents
             WHERE job_id NOT IN (SELECT job_id FROM obsolete_jobs);
         CREATE TEMP TABLE migration_attempts AS
             SELECT rowid AS migration_rowid,* FROM job_attempts
             WHERE job_id NOT IN (SELECT job_id FROM obsolete_jobs);
         DROP TABLE job_attempts;
         DROP TABLE job_intents;",
    )?;
    conn.execute_batch(super::SCHEMA)?;
    conn.execute_batch(super::failure::SCHEMA)?;
    for (table, saved, old_columns) in [
        ("job_intents", "migration_intents", intents),
        ("job_attempts", "migration_attempts", attempts),
    ] {
        let current_columns = columns(conn, table)?;
        if old_columns.iter().any(|column| {
            !current_columns.contains(column)
                && !(version <= 2 && table == "job_attempts" && column == "input_revision")
        }) {
            return Err(JobStoreError::Corrupt(format!(
                "cannot discard unknown columns on {table} during Job migration",
            )));
        }
        if version >= 6 {
            let names = old_columns
                .iter()
                .map(|s| quoted(s))
                .collect::<Vec<_>>()
                .join(",");
            conn.execute_batch(&format!(
                "INSERT INTO {table}(rowid,{names}) SELECT migration_rowid,{names} FROM {saved};",
            ))?;
        }
    }
    for (name, sql, table) in objects {
        let current_table: Option<String> = conn
            .query_row(
                "SELECT tbl_name FROM sqlite_schema WHERE name=?1",
                [&name],
                |r| r.get(0),
            )
            .optional()?;
        match current_table {
            None => conn.execute_batch(&sql)?,
            Some(current) if current == table => {}
            Some(_) => {
                return Err(JobStoreError::Corrupt(format!(
                    "Job schema object {name} collides with an extension on {table}",
                )));
            }
        }
    }
    conn.execute_batch("DROP TABLE migration_intents; DROP TABLE migration_attempts;")?;
    Ok(())
}
