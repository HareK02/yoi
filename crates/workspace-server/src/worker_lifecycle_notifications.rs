//! Runtime-owned terminal facts feed the common notification ledger. These records
//! are delivery evidence, never a second authority for Worker execution or stopping.
use super::*;

pub(super) const MIGRATION_NAME: &str = "Worker lifecycle sources in common notification routing";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerLifecycleOutcome {
    Completed,
    Errored,
    Cancelled,
    Limited,
    Stopped,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerLifecycleEvent {
    pub event_id: String,
    pub occurred_at: String,
    pub outcome: WorkerLifecycleOutcome,
    pub session_entry_ref: Option<String>,
    pub run_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerLifecycleNotification {
    pub delivery_id: String,
    pub grant_id: String,
    pub child: RuntimeWorkerRef,
    pub parent: RuntimeWorkerRef,
    pub event: WorkerLifecycleEvent,
}

#[derive(Debug, Clone)]
pub struct WorkerLifecycleScan {
    pub grant: WorkerControlGrantRecord,
    pub run_cursor: Option<String>,
    pub stop_after: Option<String>,
}

pub(super) fn install(conn: &Connection) -> Result<()> {
    // Preserve the allocator even when a Workspace purge removed the highest
    // source. Rebuilding from retained rows alone can otherwise reuse old IDs.
    let sequence: Option<(i64, i64)> = conn
        .query_row(
            "SELECT rowid,seq FROM sqlite_sequence WHERE name='notification_sources'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    // Preserve applied triggers and indexes while admitting a non-Ticket origin.
    let triggers = conn
        .prepare("SELECT name,sql FROM sqlite_schema WHERE type='trigger'")?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let indexes = conn.prepare("SELECT sql FROM sqlite_schema WHERE type='index' AND tbl_name='notification_sources' AND sql IS NOT NULL")?
        .query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    for (name, _) in &triggers {
        conn.execute_batch(&format!("DROP TRIGGER \"{}\"", name.replace('"', "\"\"")))?;
    }
    conn.execute_batch(
        "CREATE TABLE notification_sources_lifecycle_upgrade (
        sequence INTEGER PRIMARY KEY AUTOINCREMENT,
        workspace_id TEXT NOT NULL, ticket_id TEXT, input_event_id TEXT NOT NULL,
        role TEXT NOT NULL, assignment_id TEXT NOT NULL, kind TEXT NOT NULL,
        actor TEXT, event_index INTEGER, facts_json TEXT NOT NULL CHECK(json_valid(facts_json)),
        CHECK(ticket_id IS NOT NULL OR role IN ('backend_job','worker_lifecycle')),
        UNIQUE(workspace_id,ticket_id,input_event_id,role,assignment_id),
        FOREIGN KEY(workspace_id,ticket_id) REFERENCES typed_tickets(workspace_id,ticket_id));
        INSERT INTO notification_sources_lifecycle_upgrade SELECT * FROM notification_sources;
        DROP TABLE notification_sources;
        ALTER TABLE notification_sources_lifecycle_upgrade RENAME TO notification_sources;",
    )?;
    if let Some((rowid, seq)) = sequence {
        let updated = conn.execute(
            "UPDATE sqlite_sequence SET rowid=?1,seq=MAX(seq,?2) WHERE name='notification_sources'",
            params![rowid, seq],
        )?;
        if updated == 0 {
            conn.execute(
                "INSERT INTO sqlite_sequence(rowid,name,seq) VALUES(?1,'notification_sources',?2)",
                params![rowid, seq],
            )?;
        }
    }
    for sql in indexes {
        conn.execute_batch(&sql)?;
    }
    for (_, sql) in triggers {
        conn.execute_batch(&sql)?;
    }
    conn.execute_batch("CREATE UNIQUE INDEX notification_worker_lifecycle_identity
        ON notification_sources(workspace_id,input_event_id) WHERE role='worker_lifecycle';
        CREATE TABLE worker_lifecycle_scan_offers (
            workspace_id TEXT NOT NULL, grant_id TEXT NOT NULL, last_offered INTEGER NOT NULL,
            run_cursor_json TEXT CHECK(run_cursor_json IS NULL OR json_valid(run_cursor_json)), stop_after TEXT,
            PRIMARY KEY(workspace_id,grant_id),
            FOREIGN KEY(workspace_id,grant_id) REFERENCES worker_control_grants(workspace_id,grant_id) ON DELETE CASCADE);")?;
    Ok(())
}

pub(super) fn migrate(conn: &Connection) -> Result<()> {
    if current_schema_version(conn)? != 136 {
        return Err(Error::Store(
            "Worker lifecycle migration requires schema 136".into(),
        ));
    }
    let foreign_keys: bool = conn.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?;
    let legacy_alter: bool = conn.query_row("PRAGMA legacy_alter_table", [], |r| r.get(0))?;
    conn.execute_batch("PRAGMA foreign_keys=OFF; PRAGMA legacy_alter_table=ON;")?;
    let result = (|| {
        let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Exclusive)?;
        install(&tx)?;
        let violations: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
            [],
            |r| r.get(0),
        )?;
        if violations {
            return Err(Error::Store(
                "Worker lifecycle migration foreign-key violation".into(),
            ));
        }
        tx.execute(
            "INSERT INTO __yoi_schema_migrations(version,name) VALUES(137,?1)",
            [MIGRATION_NAME],
        )?;
        tx.commit()?;
        Ok(())
    })();
    let restore = conn
        .execute_batch(&format!(
            "PRAGMA legacy_alter_table={}; PRAGMA foreign_keys={};",
            i32::from(legacy_alter),
            i32::from(foreign_keys)
        ))
        .map_err(Error::from);
    result.and(restore)
}

fn encode<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|e| Error::Store(e.to_string()))
}
fn decode<T: serde::de::DeserializeOwned>(value: &str) -> Result<T> {
    serde_json::from_str(value).map_err(|e| Error::Store(e.to_string()))
}
fn grant(conn: &Connection, workspace: &str, id: &str) -> Result<WorkerControlGrantRecord> {
    conn.query_row("SELECT workspace_id,grant_id,controller_runtime_id,controller_worker_id,subject_runtime_id,subject_worker_id,relation,origin,permissions_json,operation_id,created_at,revoked_at FROM worker_control_grants WHERE workspace_id=?1 AND grant_id=?2 AND relation='spawned' AND origin='worker_spawn'", params![workspace,id], read_worker_control_grant_record).map_err(Error::from)
}

pub(super) fn scan_targets(
    store: &SqliteWorkspaceStore,
    workspace: &str,
    limit: usize,
) -> Result<Vec<WorkerLifecycleScan>> {
    store.with_conn_mut(|conn| {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ids = tx.prepare("SELECT g.grant_id FROM worker_control_grants g LEFT JOIN worker_lifecycle_scan_offers o ON o.workspace_id=g.workspace_id AND o.grant_id=g.grant_id WHERE g.workspace_id=?1 AND g.relation='spawned' AND g.origin='worker_spawn' AND g.revoked_at IS NULL ORDER BY COALESCE(o.last_offered,0),g.grant_id LIMIT ?2")?
            .query_map(params![workspace,limit.clamp(1,100) as i64], |r| r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let tick: i64 = tx.query_row("SELECT COALESCE(MAX(last_offered),0)+1 FROM worker_lifecycle_scan_offers WHERE workspace_id=?1", [workspace], |r| r.get(0))?;
        let mut targets = Vec::new();
        for id in ids {
            tx.execute("INSERT INTO worker_lifecycle_scan_offers(workspace_id,grant_id,last_offered) VALUES(?1,?2,?3) ON CONFLICT(workspace_id,grant_id) DO UPDATE SET last_offered=excluded.last_offered", params![workspace,id,tick])?;
            let (run_cursor,stop_after) = tx.query_row("SELECT run_cursor_json,stop_after FROM worker_lifecycle_scan_offers WHERE workspace_id=?1 AND grant_id=?2",params![workspace,id],|r|Ok((r.get(0)?,r.get(1)?)))?;
            targets.push(WorkerLifecycleScan {grant:grant(&tx,workspace,&id)?,run_cursor,stop_after});
        }
        tx.commit()?;
        Ok(targets)
    })
}

pub(super) fn checkpoint(
    store: &SqliteWorkspaceStore,
    workspace: &str,
    scan: &WorkerLifecycleScan,
    run_cursor: Option<&str>,
    stop_after: Option<&str>,
) -> Result<()> {
    store.with_conn(|conn| {
        let changed = conn.execute("UPDATE worker_lifecycle_scan_offers SET run_cursor_json=?5,stop_after=?6 WHERE workspace_id=?1 AND grant_id=?2 AND run_cursor_json IS ?3 AND stop_after IS ?4",params![workspace,scan.grant.grant_id,scan.run_cursor,scan.stop_after,run_cursor,stop_after])?;
        if changed != 1 { return Err(Error::Store("Worker activity scan cursor changed concurrently".into())); }
        Ok(())
    })
}

pub(super) fn capture(
    store: &SqliteWorkspaceStore,
    workspace: &str,
    grant_id: &str,
    event: &WorkerLifecycleEvent,
) -> Result<()> {
    for field in [&event.event_id, &event.occurred_at]
        .into_iter()
        .chain(event.session_entry_ref.iter())
        .chain(event.run_id.iter())
    {
        if field.is_empty() || field.len() > 512 || field.chars().any(char::is_control) {
            return Err(Error::InvalidInput(
                "Invalid Worker lifecycle event identity".into(),
            ));
        }
    }
    chrono::DateTime::parse_from_rfc3339(&event.occurred_at)
        .map_err(|_| Error::InvalidInput("Invalid lifecycle event time".into()))?;
    store.with_conn_mut(|conn| {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let origin = grant(&tx,workspace,grant_id)?;
        // A spawned child can finish before its control relation commits. Do not
        // filter its terminal facts by that later timestamp. Assigned/reused
        // Workers are deliberately excluded by the relation lookup above.
        let digest: String = Sha256::digest(encode(&(workspace,grant_id,&event.event_id))?.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
        let id = format!("worker-lifecycle:{digest}");
        let source = WorkerLifecycleNotification { delivery_id:id.clone(),grant_id:grant_id.into(),child:origin.subject,parent:origin.controller,event:event.clone() };
        let facts = encode(&source)?;
        let saved: Option<String> = tx.query_row("SELECT facts_json FROM notification_sources WHERE workspace_id=?1 AND input_event_id=?2 AND role='worker_lifecycle'",params![workspace,id],|r|r.get(0)).optional()?;
        if let Some(saved) = saved {
            if saved != facts { return Err(Error::InvalidInput("Worker lifecycle source identity reused with different facts".into())); }
            tx.commit()?; return Ok(());
        }
        tx.execute("INSERT INTO notification_sources(workspace_id,ticket_id,input_event_id,role,assignment_id,kind,facts_json) VALUES(?1,NULL,?2,'worker_lifecycle','','worker_lifecycle',?3)",params![workspace,id,facts])?;
        let (disposition, reason) = if origin.revoked_at.is_some() { ("suppress","historical_only") } else { ("deliver","attention") };
        tx.execute("INSERT INTO notification_decisions(source_sequence,workspace_id,delivery_id,disposition,reason,purpose) SELECT sequence,workspace_id,input_event_id,?3,?4,'attention' FROM notification_sources WHERE workspace_id=?1 AND input_event_id=?2 AND role='worker_lifecycle'",params![workspace,id,disposition,reason])?;
        tx.commit()?;
        Ok(())
    })
}

pub(super) fn pending(
    store: &SqliteWorkspaceStore,
    workspace: &str,
    limit: usize,
) -> Result<Vec<WorkerLifecycleNotification>> {
    store.with_conn_mut(|conn| {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let rows = tx.prepare("SELECT s.facts_json FROM notification_sources s JOIN notification_decisions d ON d.source_sequence=s.sequence WHERE s.workspace_id=?1 AND s.role='worker_lifecycle' AND d.disposition='deliver' AND NOT EXISTS(SELECT 1 FROM notification_acknowledgements a WHERE a.workspace_id=?1 AND a.delivery_id=d.delivery_id) ORDER BY d.last_offered,s.sequence LIMIT ?2")?
            .query_map(params![workspace,limit.clamp(1,100) as i64],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let tick: i64 = tx.query_row("SELECT COALESCE(MAX(last_offered),0)+1 FROM notification_decisions WHERE workspace_id=?1",[workspace],|r|r.get(0))?;
        let mut results = Vec::new();
        for row in rows {
            let source: WorkerLifecycleNotification = decode(&row)?;
            tx.execute("UPDATE notification_decisions SET last_offered=?3 WHERE workspace_id=?1 AND delivery_id=?2", params![workspace,source.delivery_id,tick])?;
            results.push(source);
        }
        tx.commit()?;
        Ok(results)
    })
}

pub(super) fn prepare(
    store: &SqliteWorkspaceStore,
    workspace: &str,
    source: &WorkerLifecycleNotification,
    content: &str,
    target: &TicketWorkerNotificationTarget,
) -> Result<String> {
    if content.is_empty() || content.len() > 8192 || content.contains('\0') {
        return Err(Error::InvalidInput(
            "Invalid lifecycle notification content".into(),
        ));
    }
    store.with_conn_mut(|conn| {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let saved: String = tx.query_row("SELECT s.facts_json FROM notification_sources s JOIN notification_decisions d ON d.source_sequence=s.sequence WHERE s.workspace_id=?1 AND d.delivery_id=?2 AND s.role='worker_lifecycle' AND d.disposition='deliver'", params![workspace,source.delivery_id],|r|r.get(0))?;
        if saved != encode(source)? { return Err(Error::InvalidInput("Lifecycle notification source mismatch".into())); }
        let origin = grant(&tx,workspace,&source.grant_id)?;
        if origin.revoked_at.is_some() || !origin.permissions.iter().any(|p|p=="observe") {
            return Err(Error::InvalidInput("Lifecycle notification spawn relation revoked".into()));
        }
        let valid: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM worker_registry w JOIN workspace_runtime_bindings b ON b.workspace_id=w.workspace_id AND b.runtime_id=w.runtime_id WHERE w.workspace_id=?1 AND w.runtime_id=?2 AND w.worker_id=?3 AND b.binding_id=?4 AND b.revoked_at IS NULL)",params![workspace,target.worker.runtime_id,target.worker.worker_id,target.authentication_binding_id],|r|r.get(0))?;
        if !valid { return Err(Error::InvalidInput("Lifecycle notification target unavailable".into())); }
        let previous: Option<(String,String)> = tx.query_row("SELECT content,target_json FROM notification_requests WHERE workspace_id=?1 AND delivery_id=?2",params![workspace,source.delivery_id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        let bytes = if let Some((bytes, bound)) = previous {
            // Never reinterpret uncertain acceptance by sending the same fact to
            // a replacement body. The diagnostic remains recoverable/unknown.
            if bound != encode(target)? { return Err(Error::InvalidInput("Lifecycle notification target changed after preparation".into())); }
            bytes
        } else {
            tx.execute("INSERT INTO notification_requests VALUES(?1,?2,?3,?4)",params![workspace,source.delivery_id,content,encode(target)?])?;
            content.into()
        };
        tx.commit()?;
        Ok(bytes)
    })
}

pub(super) fn ack(
    store: &SqliteWorkspaceStore,
    workspace: &str,
    id: &str,
    target: &TicketWorkerNotificationTarget,
) -> Result<()> {
    store.with_conn_mut(|conn| {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let saved: String = tx.query_row("SELECT target_json FROM notification_requests WHERE workspace_id=?1 AND delivery_id=?2",params![workspace,id],|r|r.get(0))?;
        if saved != encode(target)? { return Err(Error::InvalidInput("Lifecycle notification acknowledgement target mismatch".into())); }
        tx.execute("INSERT OR IGNORE INTO notification_acknowledgements VALUES(?1,?2,?3)",params![workspace,id,saved])?;
        tx.commit()?;
        Ok(())
    })
}
