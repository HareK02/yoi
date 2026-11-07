//! Backend Job adapter for the common notification ledger. Results, not Worker
//! lifecycle, are the durable source. Fixed Worker recipients never follow leases.
use super::*;
use crate::backend_job::{BackendJobAttemptState, BackendJobState};

#[path = "job_notification_legacy.rs"]
mod legacy;
// Schema-boundary entry points only; no runtime path calls the historical importer.
pub(in crate::store) use legacy::{install, verify_schema96};

// Fresh schema construction: no old receipt tables or historical row import.
pub(in crate::store) fn install_current(conn: &Connection) -> Result<()> {
    // Generalize the existing source parent without changing any sequence, request,
    // or receipt. The migration caller disables FK enforcement for this rebuild.
    let triggers = conn
        .prepare("SELECT name,sql FROM sqlite_schema WHERE type='trigger'")?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let indexes = conn.prepare("SELECT sql FROM sqlite_schema WHERE type='index' AND tbl_name='notification_sources' AND sql IS NOT NULL")?
        .query_map([], |r| r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    for (name, _) in &triggers {
        conn.execute_batch(&format!("DROP TRIGGER \"{}\"", name.replace('"', "\"\"")))?;
    }
    conn.execute_batch(
        "CREATE TABLE notification_sources_job_upgrade (
        sequence INTEGER PRIMARY KEY AUTOINCREMENT,
        workspace_id TEXT NOT NULL, ticket_id TEXT, input_event_id TEXT NOT NULL,
        role TEXT NOT NULL, assignment_id TEXT NOT NULL, kind TEXT NOT NULL,
        actor TEXT, event_index INTEGER, facts_json TEXT NOT NULL CHECK(json_valid(facts_json)),
        CHECK(ticket_id IS NOT NULL OR role='backend_job'),
        UNIQUE(workspace_id,ticket_id,input_event_id,role,assignment_id),
        FOREIGN KEY(workspace_id,ticket_id) REFERENCES typed_tickets(workspace_id,ticket_id));
        INSERT INTO notification_sources_job_upgrade SELECT * FROM notification_sources;
        DROP TABLE notification_sources;
        ALTER TABLE notification_sources_job_upgrade RENAME TO notification_sources;",
    )?;
    for sql in indexes {
        conn.execute_batch(&sql)?;
    }
    for (_, sql) in triggers {
        conn.execute_batch(&sql)?;
    }
    conn.execute_batch("CREATE TRIGGER notification_job_source_guard BEFORE INSERT ON notification_sources WHEN NEW.role='backend_job' BEGIN
        SELECT CASE WHEN NOT EXISTS(SELECT 1 FROM backend_jobs j JOIN backend_job_attempts a USING(workspace_id,job_id)
        WHERE j.workspace_id=NEW.workspace_id AND j.job_id=json_extract(NEW.facts_json,'$.backend_job.job_id') AND a.attempt_id=json_extract(NEW.facts_json,'$.backend_job.attempt_id')
        AND j.state='completed' AND a.state='completed' AND a.attempt=j.current_attempt AND j.result_digest=json_extract(NEW.facts_json,'$.result_digest'))
        THEN RAISE(ABORT,'Job notification requires a committed current result') END;
        END;
        CREATE UNIQUE INDEX notification_job_identity ON notification_sources(workspace_id,input_event_id) WHERE role='backend_job';
        CREATE TABLE notification_job_targets (
            workspace_id TEXT NOT NULL, delivery_id TEXT NOT NULL, target_json TEXT NOT NULL CHECK(json_valid(target_json)),
            PRIMARY KEY(workspace_id,delivery_id),
            FOREIGN KEY(workspace_id,delivery_id) REFERENCES notification_requests(workspace_id,delivery_id));
        CREATE TRIGGER notification_job_targets_no_update BEFORE UPDATE ON notification_job_targets BEGIN SELECT RAISE(ABORT,'Job notification target is immutable'); END;
        CREATE TRIGGER notification_job_targets_no_delete BEFORE DELETE ON notification_job_targets BEGIN SELECT RAISE(ABORT,'Job notification target is immutable'); END;
" )?;
    // Preparation may fail before a routing decision exists. Keep observations
    // against the common source, never invent a terminal decision to hold them.
    conn.execute_batch("CREATE TABLE notification_source_preparation_failures (
        sequence INTEGER PRIMARY KEY AUTOINCREMENT,
        source_sequence INTEGER NOT NULL REFERENCES notification_sources(sequence),
        code TEXT NOT NULL,
        recorded_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')));
        CREATE INDEX notification_source_preparation_failures_source ON notification_source_preparation_failures(source_sequence,sequence);
        CREATE TRIGGER notification_source_preparation_failures_no_update BEFORE UPDATE ON notification_source_preparation_failures BEGIN SELECT RAISE(ABORT,'Source preparation evidence is immutable'); END;
        CREATE TRIGGER notification_source_preparation_failures_no_delete BEFORE DELETE ON notification_source_preparation_failures BEGIN SELECT RAISE(ABORT,'Source preparation evidence is immutable'); END;")?;
    // Mutable scheduling only, independent of immutable source/decision evidence.
    conn.execute_batch(
        "CREATE TABLE notification_source_offers (
        source_sequence INTEGER PRIMARY KEY REFERENCES notification_sources(sequence),
        last_offered INTEGER NOT NULL CHECK(last_offered > 0));",
    )?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobNotificationSource {
    pub job_id: String,
    pub attempt_id: String,
    pub delivery_id: String,
}

/// Domain facts, not a second notification policy. Rendering is performed by the
/// domain adapter; only this routing module chooses deliver/suppress/defer.
#[derive(Debug, Clone)]
pub enum JobNotificationFacts {
    Result(String),
    NoFindings,
    StaleContent,
    /// SourceLookupFailed leaves routing undecided; other failures are terminal.
    Failed(NotificationFailureCode),
}

pub(in crate::store) fn capture(
    conn: &Connection,
    workspace: &str,
    job: &str,
    attempt: &str,
) -> Result<()> {
    let id = crate::backend_job::delivery_id(job, attempt);
    let record = read_backend_job(conn, workspace, job)?
        .ok_or_else(|| Error::InvalidInput("Job notification source disappeared".into()))?;
    capture_result(
        conn,
        workspace,
        job,
        attempt,
        &id,
        &record.request.purpose,
        &record.request.notification_target,
        record.result_digest.as_deref(),
    )
}

// Shared ledger encoding, not legacy lookup or routing. Historical migration
// supplies its own frozen snapshot; normal capture reads the current Job record.
fn capture_result(
    conn: &Connection,
    workspace: &str,
    job: &str,
    attempt: &str,
    id: &str,
    purpose: &str,
    target: &Option<RuntimeWorkerRef>,
    result_digest: Option<&str>,
) -> Result<()> {
    let source = JobNotificationSource {
        job_id: job.into(),
        attempt_id: attempt.into(),
        delivery_id: id.into(),
    };
    let facts = serde_json::json!({"principal_kind":if target.is_some(){Some("worker")}else{None}, "principal_id":null,"runtime_id":target.as_ref().map(|w|&w.runtime_id),"worker_id":target.as_ref().map(|w|&w.worker_id),"status":null,"case_state":"open","released":false,"backend_job":source,"recipient_kind":"fixed_worker","job_purpose":purpose,"result_digest":result_digest});
    conn.execute("INSERT OR IGNORE INTO notification_sources(workspace_id,ticket_id,input_event_id,role,assignment_id,kind,facts_json) VALUES(?1,NULL,?2,'backend_job','','backend_job_result',?3)",params![workspace,id,json(&facts)?])?;
    Ok(())
}

pub(in crate::store) fn pending(
    store: &SqliteWorkspaceStore,
    workspace: &str,
    limit: usize,
) -> Result<Vec<JobNotificationSource>> {
    store.with_conn_mut(|conn| {
        let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let rows = tx.prepare("SELECT s.facts_json,s.sequence FROM notification_sources s LEFT JOIN notification_decisions d ON d.source_sequence=s.sequence LEFT JOIN notification_source_offers o ON o.source_sequence=s.sequence WHERE s.workspace_id=?1 AND s.role='backend_job' AND (d.source_sequence IS NULL OR d.disposition='deliver') AND NOT EXISTS(SELECT 1 FROM notification_acknowledgements a WHERE a.workspace_id=?1 AND a.delivery_id=s.input_event_id) ORDER BY COALESCE(o.last_offered,0),s.sequence LIMIT ?2")?
            .query_map(params![workspace,limit.clamp(1,100) as i64],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        // Advance every offered source under the scan's write lock, even if no
        // routing decision exists yet. Failure observation IDs are not a clock.
        let tick:i64=tx.query_row("SELECT COALESCE(MAX(o.last_offered),0)+1 FROM notification_source_offers o JOIN notification_sources s ON s.sequence=o.source_sequence WHERE s.workspace_id=?1 AND s.role='backend_job'",[workspace],|r|r.get(0))?;
        let mut sources=Vec::new();
        for (facts,sequence) in rows {
            let facts:serde_json::Value=decode(&facts)?;
            sources.push(serde_json::from_value(facts["backend_job"].clone()).map_err(|e|Error::Store(e.to_string()))?);
            tx.execute("INSERT INTO notification_source_offers(source_sequence,last_offered) VALUES(?1,?2) ON CONFLICT(source_sequence) DO UPDATE SET last_offered=excluded.last_offered",params![sequence,tick])?;
        }
        tx.commit()?; Ok(sources)
    })
}

fn policy(target: bool, facts: &JobNotificationFacts) -> (&'static str, NotificationReason) {
    if !target {
        return ("suppress", NotificationReason::NoRecipient);
    }
    match facts {
        JobNotificationFacts::Result(_) => ("deliver", NotificationReason::Result),
        JobNotificationFacts::NoFindings => ("suppress", NotificationReason::NoFindings),
        JobNotificationFacts::StaleContent => ("suppress", NotificationReason::StaleContent),
        JobNotificationFacts::Failed(_) => ("defer", NotificationReason::SourceUnavailable),
    }
}

pub(in crate::store) fn route(
    store: &SqliteWorkspaceStore,
    workspace: &str,
    source: &JobNotificationSource,
    facts: &JobNotificationFacts,
) -> Result<bool> {
    let invalid;
    let facts = match facts {
        JobNotificationFacts::Result(content)
            if content.len() > crate::backend_job::MAX_JOB_DELIVERY_BYTES =>
        {
            invalid = JobNotificationFacts::Failed(NotificationFailureCode::ContentTooLarge);
            &invalid
        }
        JobNotificationFacts::Result(content) if content.is_empty() || content.contains('\0') => {
            invalid = JobNotificationFacts::Failed(NotificationFailureCode::RenderingFailed);
            &invalid
        }
        other => other,
    };
    store.with_conn_mut(|conn| {
        let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let job=read_backend_job(&tx,workspace,&source.job_id)?.ok_or_else(||Error::InvalidInput("Job notification source disappeared".into()))?;
        let attempt=read_backend_job_attempt(&tx,workspace,&source.job_id,&source.attempt_id)?.ok_or_else(||Error::InvalidInput("Job notification attempt disappeared".into()))?;
        if job.state!=BackendJobState::Completed || attempt.state!=BackendJobAttemptState::Completed || attempt.attempt!=job.current_attempt || source.delivery_id!=crate::backend_job::delivery_id(&source.job_id,&source.attempt_id) {
            return Err(Error::InvalidInput("Job notification requires its current committed result".into()));
        }
        capture(&tx,workspace,&source.job_id,&source.attempt_id)?;
        let existing:Option<String>=tx.query_row("SELECT disposition FROM notification_decisions WHERE workspace_id=?1 AND delivery_id=?2",params![workspace,source.delivery_id],|r|r.get(0)).optional()?;
        if let Some(disposition)=existing {
            let ack:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM notification_acknowledgements WHERE workspace_id=?1 AND delivery_id=?2)",params![workspace,source.delivery_id],|r|r.get(0))?;
            tx.commit()?; return Ok(disposition=="deliver" && !ack);
        }
        if job.request.notification_target.is_some() && matches!(facts, JobNotificationFacts::Failed(NotificationFailureCode::SourceLookupFailed)) {
            // A transient checker read is not suppression or an immutable defer.
            // With no decision, the existing pending scan retries the same source.
            // Check existing decisions first: never replace pinned request evidence.
            tx.execute("INSERT INTO notification_source_preparation_failures(source_sequence,code) SELECT sequence,?3 FROM notification_sources WHERE workspace_id=?1 AND input_event_id=?2 AND role='backend_job'",params![workspace,source.delivery_id,code(NotificationFailureCode::SourceLookupFailed)?])?;
            tx.commit()?; return Ok(false);
        }
        let (disposition,reason)=policy(job.request.notification_target.is_some(),facts);
        save_decision(&tx, workspace, &source.delivery_id, disposition, reason)?;
        if let JobNotificationFacts::Failed(failure)=facts { save_failure(&tx,workspace,&source.delivery_id,*failure)?; }
        // Pin bytes with the decision, before any network call. A prepared body is
        // never rerendered when content changes or an acknowledgement is lost.
        if disposition=="deliver" {
            if let (Some(target),JobNotificationFacts::Result(content))=(&job.request.notification_target,facts) {
                tx.execute("INSERT OR IGNORE INTO notification_requests VALUES(?1,?2,?3,?4)",params![workspace,source.delivery_id,content,json(target)?])?;
            }
        }
        tx.commit()?; Ok(disposition=="deliver")
    })
}

pub(in crate::store) fn request(
    store: &SqliteWorkspaceStore,
    workspace: &str,
    id: &str,
) -> Result<Option<(String, RuntimeWorkerRef)>> {
    store.with_conn(|conn| {
        let row:Option<(String,String)>=conn.query_row("SELECT content,target_json FROM notification_requests WHERE workspace_id=?1 AND delivery_id=?2",params![workspace,id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        row.map(|(content,target)|Ok((content,decode(&target)?))).transpose()
    })
}

pub(in crate::store) fn ack(
    store: &SqliteWorkspaceStore,
    workspace: &str,
    id: &str,
    target: &RuntimeWorkerRef,
) -> Result<()> {
    store.with_conn_mut(|conn| {
        let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let saved:String=tx.query_row("SELECT target_json FROM notification_requests WHERE workspace_id=?1 AND delivery_id=?2",params![workspace,id],|r|r.get(0))?;
        if saved!=json(target)? {return Err(Error::InvalidInput("Job notification target changed".into()));}
        let authorized:String=tx.query_row("SELECT target_json FROM notification_job_targets WHERE workspace_id=?1 AND delivery_id=?2",params![workspace,id],|r|r.get(0))?;
        let authorized_target:TicketWorkerNotificationTarget=decode(&authorized)?;
        if authorized_target.worker!=*target {return Err(Error::InvalidInput("Job acknowledgement target differs from authorized send".into()));}
        tx.execute("INSERT OR IGNORE INTO notification_acknowledgements VALUES(?1,?2,?3)",params![workspace,id,authorized])?;
        tx.commit()?; Ok(())
    })
}

fn save_decision(
    conn: &Connection,
    workspace: &str,
    id: &str,
    disposition: &str,
    reason: NotificationReason,
) -> Result<()> {
    conn.execute("INSERT INTO notification_decisions(source_sequence,workspace_id,delivery_id,disposition,reason,purpose) SELECT sequence,workspace_id,?2,?3,?4,'result' FROM notification_sources WHERE workspace_id=?1 AND input_event_id=?2 AND role='backend_job'",params![workspace,id,disposition,code(reason)?])?;
    Ok(())
}

/// None authorizes this exact send target; a typed denial is not a DB failure.
pub(in crate::store) fn authorize(
    store: &SqliteWorkspaceStore,
    workspace: &str,
    id: &str,
    target: &TicketWorkerNotificationTarget,
) -> Result<Option<NotificationFailureCode>> {
    store.with_conn_mut(|conn| {
        let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let recipient:String=tx.query_row("SELECT target_json FROM notification_requests WHERE workspace_id=?1 AND delivery_id=?2",params![workspace,id],|r|r.get(0))?;
        if decode::<RuntimeWorkerRef>(&recipient)?!=target.worker {return Ok(Some(NotificationFailureCode::PreparationFailed));}
        let present:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM worker_registry w WHERE w.workspace_id=?1 AND w.runtime_id=?2 AND w.worker_id=?3 AND NOT EXISTS(SELECT 1 FROM worker_removal_operations r WHERE r.workspace_id=w.workspace_id AND r.runtime_id=w.runtime_id AND r.worker_id=w.worker_id AND r.state IN ('executing','failed','succeeded')))",params![workspace,target.worker.runtime_id,target.worker.worker_id],|r|r.get(0))?;
        if !present {return Ok(Some(NotificationFailureCode::TargetUnavailable));}
        let binding:Option<(String,Option<String>)>=tx.query_row("SELECT binding_id,revoked_at FROM workspace_runtime_bindings WHERE workspace_id=?1 AND runtime_id=?2",params![workspace,target.worker.runtime_id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        let Some((binding,revoked))=binding else {return Ok(Some(NotificationFailureCode::TargetUnavailable));};
        if revoked.is_some() || binding!=target.authentication_binding_id {return Ok(Some(NotificationFailureCode::BindingRevoked));}
        let saved:Option<String>=tx.query_row("SELECT target_json FROM notification_job_targets WHERE workspace_id=?1 AND delivery_id=?2",params![workspace,id],|r|r.get(0)).optional()?;
        let serialized=json(target)?;
        if saved.as_ref().is_some_and(|saved|*saved!=serialized) {return Ok(Some(NotificationFailureCode::BindingRevoked));}
        tx.execute("INSERT OR IGNORE INTO notification_job_targets VALUES(?1,?2,?3)",params![workspace,id,serialized])?;
        tx.commit()?; Ok(None)
    })
}

pub(in crate::store) fn verify(conn: &Connection) -> Result<()> {
    for name in [
        "notification_source_offers",
        "notification_source_preparation_failures",
        "notification_source_preparation_failures_source",
        "notification_source_preparation_failures_no_update",
        "notification_source_preparation_failures_no_delete",
        "notification_job_identity",
        "notification_job_source_guard",
        "notification_job_targets",
        "notification_job_targets_no_update",
        "notification_job_targets_no_delete",
    ] {
        let present: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name=?1)",
            [name],
            |r| r.get(0),
        )?;
        if !present {
            return Err(Error::Store(format!(
                "Job notification constraint {name} is missing"
            )));
        }
    }
    Ok(())
}

pub(in crate::store) fn source(
    store: &SqliteWorkspaceStore,
    workspace: &str,
    job: &str,
    attempt: &str,
) -> Result<JobNotificationSource> {
    store.with_conn(|conn| {
        let facts:String=conn.query_row("SELECT json_extract(facts_json,'$.backend_job') FROM notification_sources WHERE workspace_id=?1 AND role='backend_job' AND json_extract(facts_json,'$.backend_job.job_id')=?2 AND json_extract(facts_json,'$.backend_job.attempt_id')=?3",params![workspace,job,attempt],|r|r.get(0))?;
        decode(&facts)
    })
}
