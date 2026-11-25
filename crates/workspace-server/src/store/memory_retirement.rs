//! Retires Workspace Memory without rewriting immutable Worker create receipts.
//! The replacement binding seals generic reservation evidence, not Memory data.
use super::*;

pub(super) const MIGRATION_NAME: &str = "Retire Workspace Memory storage and launch settings";

pub(super) fn migrate(conn: &Connection) -> Result<()> {
    if !conn.is_autocommit() || current_schema_version(conn)? != 143 {
        return Err(Error::Store("Memory retirement requires schema 143".into()));
    }
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    // The owner runner may have started before the legacy shape checks existed.
    // Recheck the exact pre-retirement state before destructive DDL in this transaction.
    storage_formats::verify_history(&tx, 143)?;
    install(&tx)?;
    tx.execute(
        "INSERT INTO __yoi_schema_migrations(version,name) VALUES(144,?1)",
        [MIGRATION_NAME],
    )?;
    tx.commit()?;
    Ok(())
}

// Shared only by the ordered owner inside its existing transaction.
pub(super) fn install(conn: &Connection) -> Result<()> {
    if conn.is_autocommit() {
        return Err(Error::Store(
            "Memory retirement installation requires its owner transaction".into(),
        ));
    }
    // Historical removed reservations remain evidence even while Runtime removal
    // is unfinished. Suspend only their live-update guard inside this write
    // transaction, and restore its exact program before leaving the backfill.
    let removal_guard: String = conn.query_row(
        "SELECT sql FROM sqlite_schema WHERE type='trigger' AND name='worker_create_update_blocked_by_runtime_removal'",
        [], |row| row.get(0),
    )?;
    conn.execute_batch("DROP TRIGGER worker_create_update_blocked_by_runtime_removal;")?;
    conn.execute_batch(
        "ALTER TABLE worker_create_reservations ADD COLUMN initialization_binding TEXT;
        ALTER TABLE worker_create_reservations ADD COLUMN subjektiv_memory_language TEXT;",
    )?;
    let reservations = {
        let mut stmt = conn.prepare("SELECT workspace_id,runtime_id,worker_id,request_fingerprint,create_fingerprint,memory_language,feature_initialization_json FROM worker_create_reservations")?;
        stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<String>>(6)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (workspace, runtime, worker, request, create, language, initialization) in reservations {
        // A missing pre-v142 resolution stays unavailable. Do not reconstruct a
        // Profile, issue a grant or reinterpret the opaque create receipt.
        if let Some(initialization) = initialization {
            let request = request
                .ok_or_else(|| Error::Store("saved create request fingerprint missing".into()))?;
            let language = language
                .ok_or_else(|| Error::Store("saved create Memory snapshot missing".into()))?;
            let parsed: FeatureInitializations = serde_json::from_str(&initialization)
                .map_err(|_| Error::Store("saved create Feature initialization invalid".into()))?;
            let canonical =
                serde_json::to_string(&parsed).map_err(|e| Error::Store(e.to_string()))?;
            let mut digest = Sha256::new();
            digest.update(b"workspace-worker-create-v4\0");
            for part in [&request, &workspace, &language] {
                digest.update(part.as_bytes());
                digest.update(b"\0");
            }
            digest.update(canonical.as_bytes());
            let base = format!(
                "sha256:{}",
                digest
                    .finalize()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            );
            let expected = match crate::plugin_plan::worker_launch_plan(
                conn, &workspace, &runtime, &worker,
            )? {
                Some(plan) => {
                    crate::plugin_plan::worker_launch_fingerprint(&base, &plan.fingerprint)?
                }
                None => base,
            };
            if expected != create {
                return Err(Error::Store(
                    "saved create fingerprint does not bind original Feature initialization".into(),
                ));
            }
            if parsed
                .get(subjektiv_api::worker_initialization::FEATURE_ID)
                .is_some()
            {
                conn.execute("UPDATE worker_create_reservations SET subjektiv_memory_language=?4 WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3", params![workspace,runtime,worker,language])?;
            }
            refresh_worker_create_evidence(
                conn,
                &workspace,
                &RuntimeWorkerRef::new(runtime, worker),
            )?;
        }
    }
    conn.execute_batch(&removal_guard)?;
    conn.execute_batch(
        "ALTER TABLE worker_create_reservations DROP COLUMN memory_language;
        DROP TABLE memory_staging_resolutions;
        DROP TABLE memory_staging_records;
        DROP TABLE workspace_memory_documents;
        ALTER TABLE workspace_memory_settings RENAME TO subjektiv_memory_settings;",
    )?;
    // Legacy AppendAudit never persisted into shared audit_events. Do not
    // classify Subject or other-domain audit evidence by a memory.* prefix.
    conn.execute(
        "DELETE FROM worker_singleton_owners WHERE singleton_key='workspace-memory-consolidation'",
        [],
    )?;
    Ok(())
}

pub(super) fn evidence_fingerprint(
    conn: &Connection,
    workspace: &str,
    worker: &RuntimeWorkerRef,
    request: &str,
    create: &str,
    initialization: &str,
) -> Result<String> {
    let plan = crate::plugin_plan::worker_launch_plan(
        conn,
        workspace,
        &worker.runtime_id,
        &worker.worker_id,
    )?;
    let language: Option<String> = conn.query_row("SELECT subjektiv_memory_language FROM worker_create_reservations WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3",params![workspace,worker.runtime_id,worker.worker_id],|r|r.get(0))?;
    let mut digest = Sha256::new();
    digest.update(b"workspace-worker-evidence-v1\0");
    for part in [
        workspace,
        &worker.runtime_id,
        &worker.worker_id,
        request,
        create,
        initialization,
        language.as_deref().unwrap_or(""),
        plan.as_ref().map(|p| p.fingerprint.as_str()).unwrap_or(""),
    ] {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part.as_bytes());
    }
    Ok(format!(
        "sha256:{}",
        digest
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    ))
}
