// Frozen schema-85 cutover. Legacy ordinal/wire names belong only here.
fn migrate_content_state_v85_to_v86(conn: &Connection) -> Result<()> {
    if current_schema_version(conn)? != 85 {
        return Err(Error::Store(
            "content state migration requires schema 85".into(),
        ));
    }
    if !conn.is_autocommit() {
        return Err(Error::Store(
            "content state migration requires an autocommit connection".into(),
        ));
    }
    let foreign_keys: bool = conn.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?;
    let legacy_alter: bool = conn.query_row("PRAGMA legacy_alter_table", [], |r| r.get(0))?;
    // Replacing a parent table must not cascade-delete its referencing rows or
    // rewrite their FK targets. Restore both connection settings on every exit.
    conn.execute_batch("PRAGMA foreign_keys=OFF; PRAGMA legacy_alter_table=ON;")?;
    let result = (|| -> Result<()> {
        let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
        if table_exists(&tx, "workspace_config_tree_revisions")? {
            tx.execute_batch(
            r#"
            CREATE TABLE workspace_config_tree_history (
                workspace_id TEXT NOT NULL,
                content_digest TEXT NOT NULL,
                toolchain_fingerprint TEXT NOT NULL,
                projection_digest TEXT NOT NULL,
                manifest_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                schema_bundle_json TEXT NOT NULL DEFAULT '{"contributions":[],"source":"{}","fingerprint":""}',
                PRIMARY KEY (workspace_id, content_digest, toolchain_fingerprint, projection_digest),
                FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
            );
        "#,
        )?;
            let history = {
                let mut stmt = tx.prepare("SELECT workspace_id, tree_digest, toolchain_fingerprint, projection_digest, manifest_json, created_at, schema_bundle_json FROM workspace_config_tree_revisions ORDER BY revision")?;
                stmt.query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, String>(5)?,
                        r.get::<_, String>(6)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
            };
            for (workspace, digest, toolchain, projection, manifest, created, schema) in history {
                let entries: std::collections::BTreeMap<
                    config_source::VirtualPath,
                    config_source::ConfigEntry,
                > = serde_json::from_str(&manifest)
                    .map_err(|e| Error::Store(format!("invalid legacy config manifest: {e}")))?;
                if entries.iter().any(|(path, entry)| path != &entry.path) {
                    return Err(Error::Store("legacy config manifest path mismatch".into()));
                }
                let snapshot =
                    config_source::ConfigTreeSnapshot::from_entries(entries.into_values())
                        .map_err(|e| Error::Store(format!("invalid legacy config content: {e}")))?;
                if snapshot.digest != digest {
                    return Err(Error::Store("legacy config content digest mismatch".into()));
                }
                tx.execute("INSERT OR IGNORE INTO workspace_config_tree_history(workspace_id,content_digest,toolchain_fingerprint,projection_digest,manifest_json,created_at,schema_bundle_json) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![workspace,digest,toolchain,projection,manifest,created,schema])?;
            }
            tx.execute_batch(
                "DROP TABLE workspace_config_tree_revisions;
            ALTER TABLE workspace_config_trees DROP COLUMN revision;
            ALTER TABLE workspace_config_trees RENAME COLUMN tree_digest TO content_digest;",
            )?;
        }
        let missing_current: i64 = tx.query_row("SELECT COUNT(*) FROM workspace_config_trees t WHERE NOT EXISTS (
        SELECT 1 FROM workspace_config_tree_history h WHERE h.workspace_id=t.workspace_id AND h.content_digest=t.content_digest)", [], |r| r.get(0))?;
        if missing_current != 0 {
            return Err(Error::Store(
                "config cutover found missing current content".into(),
            ));
        }
        if column_exists(&tx, "workspace_memory_settings", "settings_revision")? {
            tx.execute_batch(
                "ALTER TABLE workspace_memory_settings DROP COLUMN settings_revision",
            )?;
        }
        if column_exists(
            &tx,
            "worker_create_reservations",
            "memory_settings_revision",
        )? {
            remove_legacy_reservation_memory_counter(&tx)?;
        }
        // Older retained chains rebuilt this table without the lookup index.
        // Converge on the canonical schema for both legacy creation paths.
        tx.execute_batch("CREATE INDEX IF NOT EXISTS worker_create_reservations_worker ON worker_create_reservations(workspace_id, worker_id);")?;
        if column_exists(&tx, "backend_jobs", "input_revision")? {
            let jobs = {
                let mut stmt =
                    tx.prepare("SELECT workspace_id,job_id,request_json FROM backend_jobs")?;
                stmt.query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
            };
            tx.execute_batch(
                "ALTER TABLE backend_jobs RENAME COLUMN input_revision TO input_digest;
            ALTER TABLE backend_job_attempts RENAME COLUMN input_revision TO input_digest;",
            )?;
            for (workspace, id, encoded) in jobs {
                let mut value: serde_json::Value = serde_json::from_str(&encoded)
                    .map_err(|e| Error::Store(format!("invalid legacy Job intent: {e}")))?;
                value
                    .as_object_mut()
                    .ok_or_else(|| Error::Store("legacy Job intent is not an object".into()))?
                    .remove("input_revision");
                let request: BackendJobRequest = serde_json::from_value(value)
                    .map_err(|e| Error::Store(format!("invalid legacy Job intent: {e}")))?;
                let digest = request.input_digest()?;
                let encoded =
                    serde_json::to_string(&request).map_err(|e| Error::Store(e.to_string()))?;
                // A Reserved-only ledger proves no dispatch was admitted. Fail it
                // definitively and record vacuous cleanup so its resource is released.
                let reserved_only: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM backend_job_attempts WHERE workspace_id=?1 AND job_id=?2)
                 AND NOT EXISTS(SELECT 1 FROM backend_job_attempts WHERE workspace_id=?1 AND job_id=?2
                    AND (state != 'reserved' OR runtime_id IS NOT NULL OR worker_cleanup_state IS NOT NULL))",
                params![workspace, id], |r| r.get(0),
            )?;
                // Outstanding old capabilities must not gain authority under a new input fence.
                // Preserve completed results and cleanup receipts; uncertain execution is not retryable.
                tx.execute("UPDATE backend_jobs SET input_digest=?3,request_json=?4,intent_fingerprint=?5,
                state=CASE WHEN state='pending' THEN ?6 ELSE state END,
                failure_category=CASE WHEN state='pending' THEN 'content_contract_cutover' ELSE failure_category END,
                completed_at=CASE WHEN state='pending' THEN COALESCE(completed_at,updated_at) ELSE completed_at END
                WHERE workspace_id=?1 AND job_id=?2", params![workspace,id,digest,encoded,request.fingerprint()?, if reserved_only { "failed" } else { "unknown" }])?;
                tx.execute("UPDATE backend_job_attempts SET input_digest=?3,
                state=CASE WHEN state='reserved' AND ?4 THEN 'failed' WHEN state IN ('reserved','dispatching','dispatched') THEN 'unknown' ELSE state END,
                failure_category=CASE WHEN state IN ('reserved','dispatching','dispatched') THEN 'content_contract_cutover' ELSE failure_category END,
                worker_cleanup_state=CASE WHEN state='reserved' AND ?4 THEN 'completed' WHEN runtime_id IS NOT NULL AND state IN ('dispatching','dispatched') AND worker_cleanup_state IS NULL THEN 'pending' ELSE worker_cleanup_state END,
                completed_at=CASE WHEN state IN ('reserved','dispatching','dispatched') THEN COALESCE(completed_at,updated_at) ELSE completed_at END
                WHERE workspace_id=?1 AND job_id=?2", params![workspace,id,digest,reserved_only])?;
                tx.execute("UPDATE backend_job_deliveries SET state='unknown', failure_category='content_contract_cutover'
                WHERE workspace_id=?1 AND job_id=?2 AND state IN ('pending','sending')", params![workspace,id])?;
            }
        }
        // A legacy pending manual claim cannot be admitted against a newly inferred digest.
        tx.execute("UPDATE ticket_assignment_operations SET claim_state='failed',failure_reason='content_contract_cutover'
        WHERE claim_state='pending' AND binding_recovery_json IS NOT NULL AND assignment_id IS NULL", [])?;
        let broken: i64 =
            tx.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| {
                r.get(0)
            })?;
        if broken != 0 {
            return Err(Error::Store(
                "content state migration foreign-key verification failed".into(),
            ));
        }
        tx.execute("INSERT INTO __yoi_schema_migrations(version,name) VALUES(86,'content-fenced config, Memory settings and Jobs')", [])?;
        tx.commit()?;
        Ok(())
    })();
    conn.pragma_update(None, "legacy_alter_table", legacy_alter)?;
    conn.pragma_update(None, "foreign_keys", foreign_keys)?;
    result
}

fn remove_legacy_reservation_memory_counter(conn: &Connection) -> Result<()> {
    // This is the exact table CHECK introduced by the frozen v52 -> v53
    // migration. Fresh v85 databases do not have it. Do not rewrite arbitrary
    // constraints: SQLite's DROP COLUMN must still reject unknown dependencies.
    const LEGACY_CHECK: &str = ",\n            CHECK (\n                (request_fingerprint IS NULL AND memory_settings_revision IS NULL AND memory_language IS NULL)\n                OR\n                (request_fingerprint IS NOT NULL AND memory_settings_revision IS NOT NULL AND memory_settings_revision > 0 AND memory_language IS NOT NULL AND length(trim(memory_language)) > 0)\n            )";
    let ddl: String = conn.query_row(
        "SELECT sql FROM sqlite_schema WHERE type='table' AND name='worker_create_reservations'",
        [],
        |r| r.get(0),
    )?;
    if !ddl.contains(LEGACY_CHECK) {
        conn.execute_batch(
            "ALTER TABLE worker_create_reservations DROP COLUMN memory_settings_revision",
        )?;
        return Ok(());
    }
    let ddl = ddl.replacen(LEGACY_CHECK, "", 1);
    let definition = ddl
        .find('(')
        .ok_or_else(|| Error::Store("invalid legacy reservation DDL".into()))?;
    // Retain the actual table definition, not a baseline copy: historical
    // column order, additional constraints and outbound FKs must survive.
    conn.execute_batch(&format!(
        "CREATE TABLE worker_create_reservations_v86 {}",
        &ddl[definition..]
    ))?;
    conn.execute_batch(
        "ALTER TABLE worker_create_reservations_v86 DROP COLUMN memory_settings_revision",
    )?;
    let columns = {
        let mut stmt = conn.prepare(
            "SELECT name FROM pragma_table_info('worker_create_reservations_v86') ORDER BY cid",
        )?;
        stmt.query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let columns = columns
        .into_iter()
        .map(|name| format!("\"{}\"", name.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(",");
    let indexes = {
        let mut stmt = conn.prepare("SELECT sql FROM sqlite_schema WHERE type='index' AND tbl_name='worker_create_reservations' AND sql IS NOT NULL")?;
        stmt.query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    // Cross-table triggers can also reference this table during replacement.
    let triggers = {
        let mut stmt = conn.prepare("SELECT name,sql FROM sqlite_schema WHERE type='trigger'")?;
        stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (name, _) in &triggers {
        conn.execute_batch(&format!("DROP TRIGGER \"{}\"", name.replace('"', "\"\"")))?;
    }
    // No state, fingerprint, language, lease, timestamp or uncertain admission
    // is inferred from the removed ordinal. Copy every remaining column as-is.
    conn.execute_batch(&format!(
        "INSERT INTO worker_create_reservations_v86 ({columns}) SELECT {columns} FROM worker_create_reservations;
         DROP TABLE worker_create_reservations;
         ALTER TABLE worker_create_reservations_v86 RENAME TO worker_create_reservations;"
    ))?;
    for sql in indexes {
        conn.execute_batch(&sql)?;
    }
    for (_, sql) in triggers {
        conn.execute_batch(&sql)?;
    }
    Ok(())
}
