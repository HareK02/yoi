//! Backend Job adapter for the common notification ledger. Results, not Worker
//! lifecycle, are the durable source. Fixed Worker recipients never follow leases.
use super::*;
use crate::backend_job::{
    BackendJobAttemptState, BackendJobDeliveryRecord, BackendJobDeliveryState, BackendJobRequest,
    BackendJobState,
};

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

pub(in crate::store) fn install(conn: &Connection) -> Result<()> {
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
        CREATE TABLE notification_job_legacy_receipts (
            workspace_id TEXT NOT NULL, delivery_id TEXT NOT NULL, record_json TEXT NOT NULL CHECK(json_valid(record_json)),
            runtime_accepted INTEGER NOT NULL CHECK(runtime_accepted IN (0,1)),
            PRIMARY KEY(workspace_id,delivery_id),
            FOREIGN KEY(workspace_id,delivery_id) REFERENCES notification_decisions(workspace_id,delivery_id));
        CREATE TRIGGER notification_job_legacy_no_update BEFORE UPDATE ON notification_job_legacy_receipts BEGIN SELECT RAISE(ABORT,'Legacy notification evidence is immutable'); END;
        CREATE TRIGGER notification_job_legacy_no_delete BEFORE DELETE ON notification_job_legacy_receipts BEGIN SELECT RAISE(ABORT,'Legacy notification evidence is immutable'); END;")?;
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
    // Both undecided preparation retries and pinned deliveries use this clock.
    conn.execute_batch(
        "CREATE TABLE notification_source_offers (
        source_sequence INTEGER PRIMARY KEY REFERENCES notification_sources(sequence),
        last_offered INTEGER NOT NULL CHECK(last_offered > 0));",
    )?;
    // Includes no-target results and the crash gap before the former reservation.
    let identities = conn.prepare("SELECT a.workspace_id,a.job_id,a.attempt_id FROM backend_job_attempts a JOIN backend_jobs j USING(workspace_id,job_id) WHERE a.state='completed' AND j.state='completed' AND a.attempt=j.current_attempt")?
        .query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (workspace, job, attempt) in identities {
        capture(conn, &workspace, &job, &attempt)?;
        import_legacy(conn, &workspace, &job, &attempt)?;
    }
    Ok(())
}

struct NotificationResultSnapshot {
    request: BackendJobRequest,
    result: Option<serde_json::Value>,
    result_digest: Option<String>,
}

// The schema-96 import runs before later Job lifecycle migrations. Read only
// the retained result envelope, not the executable Job's current-schema columns.
fn read_result_snapshot(
    conn: &Connection,
    workspace: &str,
    job: &str,
) -> Result<NotificationResultSnapshot> {
    let (request, result, result_digest): (String, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT request_json, result_json, result_digest FROM backend_jobs WHERE workspace_id=?1 AND job_id=?2",
            params![workspace, job],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
    Ok(NotificationResultSnapshot {
        request: decode(&request)?,
        result: result.map(|value| decode(&value)).transpose()?,
        result_digest,
    })
}

pub(in crate::store) fn capture(
    conn: &Connection,
    workspace: &str,
    job: &str,
    attempt: &str,
) -> Result<()> {
    let id = retained_delivery_id(conn, workspace, job, attempt)?;
    let source = JobNotificationSource {
        job_id: job.into(),
        attempt_id: attempt.into(),
        delivery_id: id.clone(),
    };
    let job = read_result_snapshot(conn, workspace, job)?;
    let target = &job.request.notification_target;
    let facts = serde_json::json!({"principal_kind":if target.is_some(){Some("worker")}else{None}, "principal_id":null,"runtime_id":target.as_ref().map(|w|&w.runtime_id),"worker_id":target.as_ref().map(|w|&w.worker_id),"status":null,"case_state":"open","released":false,"backend_job":source,"recipient_kind":"fixed_worker","job_purpose":job.request.purpose,"result_digest":job.result_digest});
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
        let rows = tx.prepare("SELECT s.facts_json,s.sequence FROM notification_sources s LEFT JOIN notification_decisions d ON d.source_sequence=s.sequence LEFT JOIN notification_source_offers o ON o.source_sequence=s.sequence WHERE s.workspace_id=?1 AND s.role='backend_job' AND (d.source_sequence IS NULL OR d.disposition='deliver') AND NOT EXISTS(SELECT 1 FROM notification_acknowledgements a WHERE a.workspace_id=?1 AND a.delivery_id=s.input_event_id) AND NOT EXISTS(SELECT 1 FROM notification_job_legacy_receipts l WHERE l.workspace_id=?1 AND l.delivery_id=s.input_event_id AND l.runtime_accepted=1) ORDER BY COALESCE(o.last_offered,0),s.sequence LIMIT ?2")?
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
        if job.state!=BackendJobState::Completed || attempt.state!=BackendJobAttemptState::Completed || attempt.attempt!=job.current_attempt || source.delivery_id!=retained_delivery_id(&tx,workspace,&source.job_id,&source.attempt_id)? {
            return Err(Error::InvalidInput("Job notification requires its current committed result".into()));
        }
        capture(&tx,workspace,&source.job_id,&source.attempt_id)?;
        import_legacy(&tx, workspace, &source.job_id, &source.attempt_id)?;
        let existing:Option<String>=tx.query_row("SELECT disposition FROM notification_decisions WHERE workspace_id=?1 AND delivery_id=?2",params![workspace,source.delivery_id],|r|r.get(0)).optional()?;
        if let Some(disposition)=existing {
            let ack:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM notification_acknowledgements WHERE workspace_id=?1 AND delivery_id=?2) OR EXISTS(SELECT 1 FROM notification_job_legacy_receipts WHERE workspace_id=?1 AND delivery_id=?2 AND runtime_accepted=1)",params![workspace,source.delivery_id],|r|r.get(0))?;
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

// Old rows did not store request bytes. Only the original deterministic renderers
// may reconstruct an interrupted request; no current Ticket contents participate.
// Accepted legacy rows keep their actual evidence, never a fabricated new request.
fn import_legacy(conn: &Connection, workspace: &str, job_id: &str, attempt_id: &str) -> Result<()> {
    let id = retained_delivery_id(conn, workspace, job_id, attempt_id)?;
    let exists:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM notification_decisions WHERE workspace_id=?1 AND delivery_id=?2)",params![workspace,id],|r|r.get(0))?;
    if exists {
        return Ok(());
    }
    let Some(old) = read_backend_job_delivery(conn, workspace, &id)? else {
        return Ok(());
    };
    let accepted =
        old.state == BackendJobDeliveryState::Completed && old.failure_category.is_none();
    let count:i64=conn.query_row("SELECT COUNT(*) FROM backend_job_deliveries WHERE workspace_id=?1 AND job_id=?2 AND attempt_id=?3",params![workspace,job_id,attempt_id],|r|r.get(0))?;
    let mut content = None;
    let mut failure = legacy_failure(&old);
    let (disposition, reason) = match old.failure_category.as_deref() {
        _ if accepted => ("deliver", NotificationReason::Result),
        _ if count > 1 => {
            failure = Some(NotificationFailureCode::PreparationFailed);
            ("defer", NotificationReason::SourceUnavailable)
        }
        Some("notification_suppressed_no_findings") => ("suppress", NotificationReason::NoFindings),
        Some("notification_suppressed_stale_ticket_content") => {
            ("suppress", NotificationReason::StaleContent)
        }
        _ if old.state == BackendJobDeliveryState::Completed => {
            ("defer", NotificationReason::SourceUnavailable)
        }
        Some(
            "invalid_checker_input"
            | "invalid_checker_result"
            | "current_ticket_content_unavailable"
            | "notification_too_large",
        ) => ("defer", NotificationReason::SourceUnavailable),
        _ => {
            let snapshot = read_result_snapshot(conn, workspace, job_id)?;
            let facts = if snapshot.request.notification_target.as_ref() != Some(&old.target) {
                JobNotificationFacts::Failed(NotificationFailureCode::PreparationFailed)
            } else {
                legacy_result_facts(&snapshot, job_id, attempt_id)
            };
            let decision = policy(snapshot.request.notification_target.is_some(), &facts);
            match facts {
                JobNotificationFacts::Result(body) => content = Some(body),
                JobNotificationFacts::Failed(code) => failure = Some(code),
                // A valid empty result could not have passed the old sender's
                // findings gate even if it crashed before recording suppression.
                JobNotificationFacts::NoFindings => failure = None,
                JobNotificationFacts::StaleContent => {}
            }
            decision
        }
    };
    save_decision(conn, workspace, &id, disposition, reason)?;
    conn.execute(
        "INSERT INTO notification_job_legacy_receipts VALUES(?1,?2,?3,?4)",
        params![workspace, id, json(&old)?, accepted],
    )?;
    if !accepted && disposition != "suppress" {
        if let Some(failure) = failure {
            save_failure(conn, workspace, &id, failure)?;
        }
    }
    if disposition == "deliver" {
        if let Some(content) = content {
            conn.execute(
                "INSERT INTO notification_requests VALUES(?1,?2,?3,?4)",
                params![workspace, id, content, json(&old.target)?],
            )?;
        }
    }
    Ok(())
}

fn legacy_result_facts(
    snapshot: &NotificationResultSnapshot,
    job_id: &str,
    attempt_id: &str,
) -> JobNotificationFacts {
    let content = if snapshot.request.purpose == crate::ticket_item_checker::PURPOSE {
        let Some(result) = &snapshot.result else {
            return JobNotificationFacts::Failed(NotificationFailureCode::InvalidResult);
        };
        match crate::ticket_item_checker::validate_result(&snapshot.request.input, result) {
            Ok(result) if result.findings.is_empty() => return JobNotificationFacts::NoFindings,
            Ok(_) => {}
            Err(_) => return JobNotificationFacts::Failed(NotificationFailureCode::InvalidResult),
        }
        crate::ticket_item_checker::render_result(&snapshot.request.input, result)
    } else {
        Ok(format!(
            "Backend Job `{job_id}` completed attempt `{attempt_id}` for `{}`. Durable result digest: {}.",
            snapshot.request.purpose,
            snapshot.result_digest.as_deref().unwrap_or("unavailable")
        ))
    };
    match content {
        Ok(content) if content.len() > crate::backend_job::MAX_JOB_DELIVERY_BYTES => {
            JobNotificationFacts::Failed(NotificationFailureCode::ContentTooLarge)
        }
        Ok(content) if !content.is_empty() && !content.contains('\0') => {
            JobNotificationFacts::Result(content)
        }
        _ => JobNotificationFacts::Failed(NotificationFailureCode::RenderingFailed),
    }
}

fn legacy_failure(old: &BackendJobDeliveryRecord) -> Option<NotificationFailureCode> {
    use NotificationFailureCode as F;
    match old.failure_category.as_deref() {
        Some("notification_outcome_unknown") => Some(F::AcceptanceUnknown),
        Some("notification_rejected") => Some(F::RuntimeRejected),
        Some("notification_too_large") => Some(F::ContentTooLarge),
        Some("invalid_checker_input" | "invalid_checker_result") => Some(F::InvalidResult),
        Some("current_ticket_content_unavailable") => Some(F::SourceLookupFailed),
        _ if matches!(
            old.state,
            BackendJobDeliveryState::Sending
                | BackendJobDeliveryState::Unknown
                | BackendJobDeliveryState::Completed
        ) =>
        {
            Some(F::AcceptanceUnknown)
        }
        _ => None,
    }
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
        "notification_job_legacy_receipts",
        "notification_job_legacy_no_update",
        "notification_job_legacy_no_delete",
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

fn retained_delivery_id(
    conn: &Connection,
    workspace: &str,
    job: &str,
    attempt: &str,
) -> Result<String> {
    // The production legacy sender used the derived ID, but retained receipts are
    // authoritative even when imported history contains a non-derived identity.
    // If history contains multiple identities, an actual ACK wins; otherwise the
    // importer defers the ambiguity instead of manufacturing another send.
    let saved:Option<String>=conn.query_row("SELECT delivery_id FROM backend_job_deliveries WHERE workspace_id=?1 AND job_id=?2 AND attempt_id=?3 ORDER BY (state='completed' AND failure_category IS NULL) DESC,created_at,delivery_id LIMIT 1",params![workspace,job,attempt],|r|r.get(0)).optional()?;
    Ok(saved.unwrap_or_else(|| crate::backend_job::delivery_id(job, attempt)))
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
