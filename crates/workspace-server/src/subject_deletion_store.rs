//! Main-DB half of Subject deletion. Call prepare only after the domain's
//! content-free fence, while holding the Host Subject gate. Reuse the operation
//! after crashes; another operation is a conflict, never a fresh deletion.
//!
//! All historical Job attempts and dispatch allocations are snapshotted, not just
//! the current attempt. Pending/unknown work becomes failed/subject_deleted (not a
//! successful Job). Originals in body/delegated Sessions are archived forever;
//! Successful canonical Worker removals and archives are outside this deletion:
//! prepare pins their exact successful receipt, regardless of its disposition.
//! Unfinished removals instead pin their immutable old execution request; exact
//! reconciliation and (for Jobs) additional physical erasure must complete first.
//! Newly removed Job Sessions/diagnostics need canonical Purge before input,
//! result, notification payloads and live Job/cache references can be erased.
//! This DB has no separate Job content cache. Runtime caches are the canonical
//! Worker removal's responsibility. Notification text already delivered into an
//! ordinary conversation follows that conversation's archive lifecycle.
use super::*;
use worker_runtime::retention::{
    DiagnosticsDisposition, WorkerRetentionExecutionRequest, WorkerRetentionExecutionResult,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SubjectDeletionRemoval {
    pub request: WorkerRetentionExecutionRequest,
    pub completed: bool,
    pub is_job: bool,
}

fn decode_request(json: &str) -> Result<WorkerRetentionExecutionRequest> {
    serde_json::from_str(json).map_err(|e| Error::Store(e.to_string()))
}

// Only source metadata comes from the saved Worker snapshot. Everything owned by
// the canonical plan is rechecked, even after its success/catalog removal.
fn canonical_request(
    conn: &Connection,
    operation: &str,
    source_created_at: &str,
    profile: Option<&str>,
) -> Result<Option<(WorkerRetentionExecutionRequest, String)>> {
    let row: Option<(String, String)> = conn.query_row(
        "SELECT json_object('operation_id',operation_id,'input_fingerprint',input_fingerprint,
          'archive_id',archive_id,'workspace_id',workspace_id,'source_runtime_id',runtime_id,
          'worker_id',worker_id,'source_created_at',?2,'removed_at',created_at,
          'effective_profile',?3,'retention_class',NULL,'policy_id',policy_id,
          'session_disposition',session_disposition,'diagnostics_disposition',diagnostics_disposition),state
         FROM worker_removal_operations WHERE operation_id=?1",
        params![operation,source_created_at,profile], |r| Ok((r.get(0)?,r.get(1)?)),
    ).optional()?;
    row.map(|(json, state)| Ok((decode_request(&json)?, state)))
        .transpose()
}

fn saved_removal(
    conn: &Connection,
    workspace: &str,
    subject: Option<&str>,
    worker: &RuntimeWorkerRef,
) -> Result<Option<SubjectDeletionRemoval>> {
    let rows = conn.prepare("SELECT d.request_json,d.completed,w.is_job FROM subject_deletion_removals d
        JOIN subject_deletion_workers w USING(workspace_id,subject_id,runtime_id,worker_id)
        WHERE d.workspace_id=?1 AND d.runtime_id=?2 AND d.worker_id=?3 AND (?4 IS NULL OR d.subject_id=?4)
        ORDER BY d.subject_id")?
        .query_map(params![workspace,worker.runtime_id,worker.worker_id,subject], |r| Ok((r.get::<_,String>(0)?,r.get::<_,bool>(1)?,r.get::<_,bool>(2)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut saved: Option<SubjectDeletionRemoval> = None;
    for (json, completed, is_job) in rows {
        let request = decode_request(&json)?;
        if let Some(saved) = saved.as_mut() {
            if saved.request != request {
                return Err(Error::RepositoryConflict(
                    "Conflicting Subject deletion removal snapshots".into(),
                ));
            }
            saved.completed &= completed;
            saved.is_job |= is_job;
        } else {
            saved = Some(SubjectDeletionRemoval {
                request,
                completed,
                is_job,
            });
        }
    }
    Ok(saved)
}

pub(crate) fn removal_reconciliation_matches(
    conn: &Connection,
    workspace: &str,
    worker: &RuntimeWorkerRef,
    operation: &str,
    fp: &str,
) -> Result<bool> {
    let Some(saved) = saved_removal(conn, workspace, None, worker)? else {
        return Ok(false);
    };
    let request = &saved.request;
    if request.workspace_id != workspace
        || request.source_runtime_id != worker.runtime_id
        || request.worker_id.to_string() != worker.worker_id
        || request.operation_id != operation
        || request.input_fingerprint != fp
    {
        return Ok(false);
    }
    Ok(canonical_request(
        conn,
        operation,
        &request.source_created_at,
        request.effective_profile.as_deref(),
    )?
    .is_some_and(|(canonical, state)| {
        canonical == *request
            && matches!(
                state.as_str(),
                "planned" | "executing" | "failed" | "succeeded"
            )
    }))
}

pub(super) fn completed_removal_matches(
    conn: &Connection,
    workspace: &str,
    subject: &str,
    worker: &RuntimeWorkerRef,
) -> Result<bool> {
    let Some(saved) = saved_removal(conn, workspace, Some(subject), worker)? else {
        return Ok(false);
    };
    if !saved.completed
        || !removal_reconciliation_matches(
            conn,
            workspace,
            worker,
            &saved.request.operation_id,
            &saved.request.input_fingerprint,
        )?
    {
        return Ok(false);
    }
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM subject_deletion_removals d
         JOIN worker_removal_operations r ON r.operation_id=d.operation_id AND r.input_fingerprint=d.input_fingerprint
         WHERE d.workspace_id=?1 AND d.subject_id=?2 AND d.runtime_id=?3 AND d.worker_id=?4
         AND r.state='succeeded' AND d.completed=1 AND (?5=0 OR (
           json_extract(d.job_purge_receipt_json,'$.workspace_id')=d.workspace_id
           AND json_extract(d.job_purge_receipt_json,'$.worker_id')=d.worker_id
           AND json_extract(d.job_purge_receipt_json,'$.operation_id')=d.operation_id
           AND json_extract(d.job_purge_receipt_json,'$.input_fingerprint')=d.input_fingerprint)))",
        params![workspace,subject,worker.runtime_id,worker.worker_id,saved.is_job],
        |r| r.get(0),
    )?)
}

fn pin_removal(
    conn: &Connection,
    workspace: &str,
    subject: &str,
    worker: &RuntimeWorkerRef,
) -> Result<()> {
    let operation: Option<String> = conn.query_row("SELECT operation_id FROM worker_removal_operations
        WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3 AND state IN ('planned','executing','failed')
        AND (state IN ('executing','failed') OR worker_updated_at=(SELECT updated_at FROM worker_registry WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3))
        ORDER BY created_at DESC,operation_id DESC LIMIT 1",
        params![workspace,worker.runtime_id,worker.worker_id],|r|r.get(0)).optional()?;
    let Some(operation) = operation else {
        return Ok(());
    };
    let source: Option<(String,Option<String>)> = conn.query_row("SELECT created_at,profile FROM worker_registry WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3",
        params![workspace,worker.runtime_id,worker.worker_id], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
    let (created_at, profile) = source.ok_or_else(|| {
        Error::RepositoryConflict(
            "Unfinished Worker removal has no source identity to snapshot".into(),
        )
    })?;
    let (request, _) = canonical_request(conn, &operation, &created_at, profile.as_deref())?
        .ok_or_else(|| {
            Error::RepositoryConflict("Canonical removal disappeared while snapshotting".into())
        })?;
    let json = serde_json::to_string(&request).map_err(|e| Error::Store(e.to_string()))?;
    conn.execute("INSERT INTO subject_deletion_removals(workspace_id,subject_id,runtime_id,worker_id,operation_id,input_fingerprint,request_json)
        VALUES(?1,?2,?3,?4,?5,?6,?7)",params![workspace,subject,worker.runtime_id,worker.worker_id,operation,request.input_fingerprint,json])?;
    Ok(())
}

const MIGRATION_NAME: &str = "Durable Subject deletion snapshots and Job admission fences";

pub(super) fn migrate(conn: &Connection) -> Result<()> {
    if current_schema_version(conn)? != 104 {
        return Err(Error::Store(
            "Subject deletion migration requires schema 104".into(),
        ));
    }
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(include_str!("subject_deletion_schema.sql"))?;
    install_notification_erasure(&tx)?;
    tx.execute(
        "INSERT INTO __yoi_schema_migrations(version,name) VALUES(105,?1)",
        [MIGRATION_NAME],
    )?;
    verify(&tx)?;
    tx.commit()?;
    Ok(())
}

// Immutable notification evidence normally forbids deletion. The only exception
// is a saved Subject deletion snapshot; erasure below still requires physical
// Job Session Purge receipts. Updates remain forbidden, including payload edits.
pub(super) fn install_notification_erasure(conn: &Connection) -> Result<()> {
    for (trigger, table, predicate) in [
        (
            "notification_sources_no_delete",
            "notification_sources",
            "s.sequence=OLD.sequence",
        ),
        (
            "notification_decisions_no_delete",
            "notification_decisions",
            "s.sequence=OLD.source_sequence",
        ),
        (
            "notification_requests_no_delete",
            "notification_requests",
            "s.workspace_id=OLD.workspace_id AND s.input_event_id=OLD.delivery_id",
        ),
        (
            "notification_acknowledgements_no_delete",
            "notification_acknowledgements",
            "s.workspace_id=OLD.workspace_id AND s.input_event_id=OLD.delivery_id",
        ),
        (
            "notification_failures_no_delete",
            "notification_failures",
            "s.workspace_id=OLD.workspace_id AND s.input_event_id=OLD.delivery_id",
        ),
        (
            "notification_job_targets_no_delete",
            "notification_job_targets",
            "s.workspace_id=OLD.workspace_id AND s.input_event_id=OLD.delivery_id",
        ),
        (
            "notification_job_legacy_no_delete",
            "notification_job_legacy_receipts",
            "s.workspace_id=OLD.workspace_id AND s.input_event_id=OLD.delivery_id",
        ),
        (
            "notification_source_preparation_failures_no_delete",
            "notification_source_preparation_failures",
            "s.sequence=OLD.source_sequence",
        ),
    ] {
        conn.execute_batch(&format!("DROP TRIGGER {trigger};
            CREATE TRIGGER {trigger} BEFORE DELETE ON {table}
            WHEN NOT EXISTS(SELECT 1 FROM notification_sources s JOIN subject_deletion_jobs j
              ON j.workspace_id=s.workspace_id AND j.job_id=json_extract(s.facts_json,'$.backend_job.job_id')
              WHERE s.role='backend_job' AND {predicate})
            BEGIN SELECT RAISE(ABORT,'Notification evidence is immutable outside Subject deletion'); END;"))?;
    }
    conn.execute_batch("CREATE TRIGGER subject_deleted_notification_source BEFORE INSERT ON notification_sources
        WHEN NEW.role='backend_job' AND EXISTS(SELECT 1 FROM subject_deletion_jobs j WHERE j.workspace_id=NEW.workspace_id AND j.job_id=json_extract(NEW.facts_json,'$.backend_job.job_id'))
        BEGIN SELECT RAISE(ABORT,'subject_deleted: Job notification source fenced'); END;
        CREATE TRIGGER subject_deleted_notification_payload BEFORE INSERT ON notification_requests
        WHEN EXISTS(SELECT 1 FROM notification_sources s JOIN subject_deletion_jobs j ON j.workspace_id=s.workspace_id AND j.job_id=json_extract(s.facts_json,'$.backend_job.job_id') WHERE s.role='backend_job' AND s.workspace_id=NEW.workspace_id AND s.input_event_id=NEW.delivery_id)
        BEGIN SELECT RAISE(ABORT,'subject_deleted: Job notification payload fenced'); END;
        CREATE TRIGGER subject_deleted_notification_send BEFORE INSERT ON notification_job_targets
        WHEN EXISTS(SELECT 1 FROM notification_sources s JOIN subject_deletion_jobs j ON j.workspace_id=s.workspace_id AND j.job_id=json_extract(s.facts_json,'$.backend_job.job_id') WHERE s.role='backend_job' AND s.workspace_id=NEW.workspace_id AND s.input_event_id=NEW.delivery_id)
        BEGIN SELECT RAISE(ABORT,'subject_deleted: Job notification send fenced'); END;")?;
    Ok(())
}

pub(super) fn verify(conn: &Connection) -> Result<()> {
    for name in [
        "subject_deletions",
        "subject_deletion_workers",
        "subject_deletion_removals",
        "subject_deletion_removal_insert",
        "subject_deletion_removal_identity_immutable",
        "subject_deletion_removal_completion_monotonic",
        "subject_deletion_jobs",
        "subject_deletion_allocations",
        "subject_deleted_job_insert",
        "subject_deleted_job_result",
        "subject_deleted_attempt_insert",
        "subject_deleted_attempt_result",
        "subject_deleted_worker_reservation",
        "subject_deletion_worker_prior_insert",
        "subject_deletion_worker_identity_immutable",
        "subject_deletion_original_absence_monotonic",
        "subject_deleted_notification_source",
        "subject_deleted_notification_payload",
        "subject_deleted_notification_send",
    ] {
        let present: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name=?1)",
            [name],
            |r| r.get(0),
        )?;
        if !present {
            return Err(Error::Store(format!(
                "Subject deletion schema object {name} is missing"
            )));
        }
    }
    if !column_exists(
        conn,
        "subject_deletion_workers",
        "prior_removal_operation_id",
    )? {
        return Err(Error::Store(
            "Subject deletion prior removal snapshot column is missing".into(),
        ));
    }
    Ok(())
}

pub(super) fn require_worker_admission(
    conn: &Connection,
    workspace: &str,
    allocation: &str,
    singleton: Option<&str>,
) -> Result<()> {
    let fenced: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM subject_deletions WHERE workspace_id=?1 AND ?3 IN ('subjektiv:'||subject_id,'subjektiv-consolidation:'||subject_id)) OR EXISTS(SELECT 1 FROM subject_deletion_allocations WHERE workspace_id=?1 AND allocation_key=?2)",params![workspace,allocation,singleton],|r|r.get(0))?;
    if fenced {
        return Err(Error::RepositoryConflict(
            "subject_deleted: Worker allocation fenced".into(),
        ));
    }
    Ok(())
}

fn require_operation(
    conn: &Connection,
    workspace: &str,
    subject: &str,
    operation: &str,
) -> Result<()> {
    let matches: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM subject_deletions WHERE workspace_id=?1 AND subject_id=?2 AND operation_id=?3)", params![workspace,subject,operation], |r|r.get(0))?;
    if !matches {
        return Err(Error::RepositoryConflict(
            "Subject deletion operation does not match saved fence".into(),
        ));
    }
    Ok(())
}

pub(crate) fn worker_kind(
    conn: &Connection,
    workspace: &str,
    worker: &RuntimeWorkerRef,
) -> Result<Option<bool>> {
    Ok(conn.query_row("SELECT max(is_job) FROM subject_deletion_workers WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3", params![workspace,worker.runtime_id,worker.worker_id], |r|r.get(0))?)
}

pub(crate) fn prior_removal_matches(
    conn: &Connection,
    workspace: &str,
    worker: &RuntimeWorkerRef,
    operation: &str,
) -> Result<bool> {
    Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM subject_deletion_workers w JOIN worker_removal_operations r ON r.operation_id=w.prior_removal_operation_id AND r.workspace_id=w.workspace_id AND r.runtime_id=w.runtime_id AND r.worker_id=w.worker_id WHERE w.workspace_id=?1 AND w.runtime_id=?2 AND w.worker_id=?3 AND r.operation_id=?4 AND r.state='succeeded')",params![workspace,worker.runtime_id,worker.worker_id,operation],|r|r.get(0))?)
}

/// Even an idempotent completed-result replay must pass this fence. Otherwise a
/// deleted Subject could reaccept old result bodies before the physical purge.
pub(super) fn require_job_admission(
    conn: &Connection,
    workspace: &str,
    request: &BackendJobRequest,
) -> Result<()> {
    let journal = request
        .grants
        .subjektiv_journal
        .as_ref()
        .map(|g| g.subject_id.as_str());
    let consolidation = request
        .grants
        .subjektiv_consolidation
        .as_ref()
        .map(|g| g.subject_id.as_str());
    let fenced: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM subject_deletions WHERE workspace_id=?1 AND subject_id IN (?2,?3)) OR EXISTS(SELECT 1 FROM subject_deletion_jobs WHERE workspace_id=?1 AND job_id=?4)", params![workspace,journal,consolidation,request.job_id], |r|r.get(0))?;
    if fenced {
        return Err(Error::RepositoryConflict(
            "subject_deleted: Backend Job admission/result fenced".into(),
        ));
    }
    Ok(())
}

fn worker_fence_request(
    conn: &Connection,
    workspace: &str,
    subject: &str,
    operation: &str,
    worker: &RuntimeWorkerRef,
) -> Result<runtime_api::SubjectDeleteWorkerRequest> {
    require_operation(conn, workspace, subject, operation)?;
    let fingerprint: Option<String> = conn.query_row(
        "SELECT r.create_fingerprint FROM worker_create_reservations r JOIN subject_deletion_workers d USING(workspace_id,runtime_id,worker_id) WHERE d.workspace_id=?1 AND d.subject_id=?2 AND d.runtime_id=?3 AND d.worker_id=?4",
        params![workspace, subject, worker.runtime_id, worker.worker_id], |row| row.get(0),
    ).optional()?;
    let digest = sha2::Sha256::digest(
        serde_json::to_vec(&(subject, operation, worker))
            .map_err(|e| Error::Store(e.to_string()))?,
    );
    let child_key = format!(
        "subject-delete-worker:{}",
        digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    Ok(runtime_api::SubjectDeleteWorkerRequest {
        workspace_id: workspace.into(),
        worker_id: worker_runtime::identity::WorkerId::parse(&worker.worker_id)
            .ok_or_else(|| Error::InvalidInput("invalid Worker ID".into()))?,
        create_fingerprint: fingerprint.ok_or_else(|| {
            Error::RepositoryConflict(
                "Subject Worker has no immutable creation identity in the deletion inventory"
                    .into(),
            )
        })?,
        operation_id: child_key,
    })
}

impl SqliteWorkspaceStore {
    pub(crate) fn subject_deletion_removal(
        &self,
        workspace: &str,
        worker: &RuntimeWorkerRef,
    ) -> Result<Option<super::SubjectDeletionRemoval>> {
        self.with_conn(|conn| saved_removal(conn, workspace, None, worker))
    }

    /// A canonical success alone is not proof of additional Job erasure. Keep the
    /// old receipt/dispositions intact and only complete this separate snapshot
    /// after the exact Runtime result and (for Jobs) physical purge response.
    pub(crate) fn confirm_subject_deletion_removal(
        &self,
        workspace: &str,
        worker: &RuntimeWorkerRef,
        request: &WorkerRetentionExecutionRequest,
        result: &WorkerRetentionExecutionResult,
        purge: Option<&runtime_api::SubjectJobRemovalPurgeResponse>,
    ) -> Result<()> {
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let saved = saved_removal(&tx,workspace,None,worker)?
                .ok_or_else(|| Error::RepositoryConflict("No saved Subject deletion removal".into()))?;
            if saved.request != *request
                || !removal_reconciliation_matches(&tx,workspace,worker,&request.operation_id,&request.input_fingerprint)?
                || !tx.query_row("SELECT state='succeeded' FROM worker_removal_operations WHERE operation_id=?1",[&request.operation_id],|r|r.get::<_,bool>(0))?
                || result.operation_id != request.operation_id || result.input_fingerprint != request.input_fingerprint
                || result.worker_id != request.worker_id || result.session_disposition != request.session_disposition
                || result.diagnostics_disposition != request.diagnostics_disposition || !result.source_removed
                || result.diagnostics_retained != (request.diagnostics_disposition == DiagnosticsDisposition::Retain) {
                return Err(Error::RepositoryConflict("Subject deletion removal requires exact saved request and canonical Runtime success".into()));
            }
            let archive_matches = match (&result.archive, request.session_disposition) {
                (Some(archive), worker_runtime::retention::SessionDisposition::Archive) => {
                    request.archive_id.as_deref() == Some(archive.archive_id.as_str())
                        && archive.workspace_id == workspace
                        && archive.source_runtime_id == worker.runtime_id
                        && archive.source_worker_id == request.worker_id
                        && archive.operation_id == request.operation_id
                        && archive.input_fingerprint == request.input_fingerprint
                        && archive.policy_id == request.policy_id
                        && archive.source_created_at == request.source_created_at
                }
                (None, worker_runtime::retention::SessionDisposition::Purge) => true,
                _ => false,
            };
            if !archive_matches {
                return Err(Error::RepositoryConflict("Subject deletion removal archive identity mismatch".into()));
            }
            if saved.is_job && purge.is_none() {
                return Err(Error::RepositoryConflict("Subject Job removal requires an additional physical purge receipt".into()));
            }
            if let Some(purge) = purge {
                if purge.workspace_id != workspace || purge.worker_id != request.worker_id
                    || purge.operation_id != request.operation_id || purge.input_fingerprint != request.input_fingerprint {
                    return Err(Error::RepositoryConflict("Subject Job physical purge receipt identity mismatch".into()));
                }
            }
            let purge_json = purge.map(serde_json::to_string).transpose().map_err(|e| Error::Store(e.to_string()))?;
            tx.execute("UPDATE subject_deletion_removals SET completed=1,job_purge_receipt_json=coalesce(job_purge_receipt_json,?4)
                WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3",params![workspace,worker.runtime_id,worker.worker_id,purge_json])?;
            tx.commit()?;
            Ok(())
        })
    }

    pub(crate) fn subject_deletion_worker_request(
        &self,
        workspace: &str,
        subject: &str,
        operation: &str,
        worker: &RuntimeWorkerRef,
    ) -> Result<runtime_api::SubjectDeleteWorkerRequest> {
        self.with_conn(|conn| worker_fence_request(conn, workspace, subject, operation, worker))
    }

    /// Snapshot once, under the Host gate, after the domain deletion fence.
    /// Delegated identities must come from domain grants, never request JSON.
    pub fn prepare_subject_deletion(
        &self,
        workspace: &str,
        subject: &str,
        operation: &str,
        delegated: &[RuntimeWorkerRef],
    ) -> Result<()> {
        if [workspace, subject, operation]
            .iter()
            .any(|s| s.is_empty() || s.trim() != *s || s.chars().any(char::is_control))
        {
            return Err(Error::InvalidInput(
                "Subject deletion identities must be non-empty and trimmed".into(),
            ));
        }
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let saved: Option<String> = tx.query_row("SELECT operation_id FROM subject_deletions WHERE workspace_id=?1 AND subject_id=?2",params![workspace,subject],|r|r.get(0)).optional()?;
            if let Some(saved) = saved {
                if saved != operation { return Err(Error::RepositoryConflict("Subject deletion operation changed".into())); }
                tx.commit()?;
                return Ok(());
            }
            let reused:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM subject_deletions WHERE workspace_id=?1 AND operation_id=?2)",params![workspace,operation],|r|r.get(0))?;
            if reused {return Err(Error::RepositoryConflict("Subject deletion operation reused for another Subject".into()));}
            tx.execute("INSERT INTO subject_deletions(workspace_id,subject_id,operation_id) VALUES(?1,?2,?3)",params![workspace,subject,operation])?;
            tx.execute("INSERT INTO subject_deletion_jobs(workspace_id,subject_id,job_id)
                SELECT workspace_id,?2,job_id FROM backend_jobs WHERE workspace_id=?1 AND
                (json_extract(request_json,'$.grants.subjektiv_journal.subject_id')=?2 OR
                 json_extract(request_json,'$.grants.subjektiv_consolidation.subject_id')=?2)",params![workspace,subject])?;
            let mut workers = tx.prepare("SELECT runtime_id,worker_id,singleton_key='subjektiv-consolidation:'||?2 FROM worker_singleton_owners WHERE workspace_id=?1 AND singleton_key IN ('subjektiv:'||?2,'subjektiv-consolidation:'||?2)
                UNION SELECT runtime_id,worker_id,singleton_key='subjektiv-consolidation:'||?2 FROM worker_create_reservations WHERE workspace_id=?1 AND singleton_key IN ('subjektiv:'||?2,'subjektiv-consolidation:'||?2) AND state!='removed'")?
                .query_map(params![workspace,subject],|r|Ok((RuntimeWorkerRef::new(r.get::<_,String>(0)?,r.get::<_,String>(1)?),r.get::<_,bool>(2)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            workers.extend(delegated.iter().cloned().map(|w|(w,false)));
            let attempts = tx.prepare("SELECT a.attempt_id,a.runtime_id,a.worker_id FROM backend_job_attempts a JOIN subject_deletion_jobs j USING(workspace_id,job_id) WHERE j.workspace_id=?1 AND j.subject_id=?2")?
                .query_map(params![workspace,subject],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,Option<String>>(2)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for (attempt, runtime, worker) in attempts {
                if let (Some(runtime),Some(worker)) = (runtime,worker) { workers.push((RuntimeWorkerRef::new(runtime,worker),true)); }
                let allocation = crate::backend_job::allocation_key(&attempt);
                tx.execute("INSERT INTO subject_deletion_allocations(workspace_id,subject_id,allocation_key) VALUES(?1,?2,?3)",params![workspace,subject,allocation])?;
                if let Some(worker) = tx.query_row("SELECT runtime_id,worker_id FROM worker_create_reservations WHERE workspace_id=?1 AND allocation_key=?2",params![workspace,allocation],|r|Ok(RuntimeWorkerRef::new(r.get::<_,String>(0)?,r.get::<_,String>(1)?))).optional()? {
                    workers.push((worker,true));
                }
            }
            for (worker,is_job) in workers {
                let other: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM worker_registry WHERE runtime_id=?1 AND worker_id=?2 AND workspace_id!=?3 UNION ALL SELECT 1 FROM worker_create_reservations WHERE runtime_id=?1 AND worker_id=?2 AND workspace_id!=?3)",params![worker.runtime_id,worker.worker_id,workspace],|r|r.get(0))?;
                if other { return Err(Error::RepositoryConflict("Subject deletion Worker belongs to another Workspace".into())); }
                let prior:Option<String>=tx.query_row("SELECT operation_id FROM worker_removal_operations WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3 AND state='succeeded' ORDER BY created_at DESC,operation_id DESC LIMIT 1",params![workspace,worker.runtime_id,worker.worker_id],|r|r.get(0)).optional()?;
                tx.execute("INSERT INTO subject_deletion_workers(workspace_id,subject_id,runtime_id,worker_id,is_job,prior_removal_operation_id) VALUES(?1,?2,?3,?4,?5,?6)
                    ON CONFLICT(workspace_id,subject_id,runtime_id,worker_id) DO UPDATE SET is_job=max(is_job,excluded.is_job)",params![workspace,subject,worker.runtime_id,worker.worker_id,is_job,prior])?;
            }
            // Deduplicate workers before pinning: an identity can be both a
            // delegated Worker and a Job attempt; the final is_job wins.
            let pending = tx.prepare("SELECT runtime_id,worker_id FROM subject_deletion_workers WHERE workspace_id=?1 AND subject_id=?2 AND prior_removal_operation_id IS NULL")?
                .query_map(params![workspace,subject],|r|Ok(RuntimeWorkerRef::new(r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for worker in pending { pin_removal(&tx,workspace,subject,&worker)?; }
            let now = chrono::Utc::now().to_rfc3339();
            tx.execute("UPDATE backend_job_attempts SET state='failed',failure_category='subject_deleted',failure_detail=NULL,completed_at=?3,updated_at=?3,
                worker_cleanup_state=CASE WHEN worker_id IS NULL THEN 'completed' ELSE 'pending' END,
                worker_cleanup_updated_at=?3,worker_cleanup_completed_at=CASE WHEN worker_id IS NULL THEN ?3 ELSE NULL END
                WHERE workspace_id=?1 AND job_id IN (SELECT job_id FROM subject_deletion_jobs WHERE workspace_id=?1 AND subject_id=?2) AND state IN ('reserved','dispatching','dispatched','unknown')",params![workspace,subject,now])?;
            tx.execute("UPDATE backend_jobs SET state='failed',failure_category='subject_deleted',failure_detail=NULL,completed_at=?3,updated_at=?3
                WHERE workspace_id=?1 AND job_id IN (SELECT job_id FROM subject_deletion_jobs WHERE workspace_id=?1 AND subject_id=?2) AND state IN ('pending','unknown')",params![workspace,subject,now])?;
            tx.commit()?;
            Ok(())
        })
    }

    pub fn list_subject_deletion_workers(
        &self,
        workspace: &str,
        subject: &str,
        operation: &str,
    ) -> Result<Vec<(RuntimeWorkerRef, bool)>> {
        self.with_conn(|conn| {
            require_operation(conn,workspace,subject,operation)?;
            Ok(conn.prepare("SELECT runtime_id,worker_id,is_job FROM subject_deletion_workers WHERE workspace_id=?1 AND subject_id=?2 ORDER BY runtime_id,worker_id")?
                .query_map(params![workspace,subject],|r|Ok((RuntimeWorkerRef::new(r.get::<_,String>(0)?,r.get::<_,String>(1)?),r.get(2)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Stop/catalog absence is not erasure. An exact scoped Runtime receipt may
    /// prove no live Worker/Session/diagnostic aggregate exists under its permanent
    /// creation fence; retain that proof and release the reservation atomically. Otherwise require
    /// a canonical original-retention receipt; never manufacture a Purge receipt.
    pub(crate) fn confirm_subject_deletion_absence(
        &self,
        workspace: &str,
        subject: &str,
        operation: &str,
        worker: &RuntimeWorkerRef,
        fence: &runtime_api::SubjectDeleteWorkerResponse,
    ) -> Result<()> {
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let confirm = |conn: &Connection| -> Result<()> {
            let request = worker_fence_request(conn, workspace, subject, operation, worker)?;
            if fence.workspace_id != request.workspace_id || fence.worker_id != request.worker_id
                || fence.create_fingerprint != request.create_fingerprint || fence.operation_id != request.operation_id {
                return Err(Error::RepositoryConflict("Subject deletion absence fence identity mismatch".into()));
            }
            // An in-flight pin cannot be bypassed by UnknownWorker/absence or
            // a different later canonical success, even if that success is Purge.
            if saved_removal(conn,workspace,Some(subject),worker)?.is_some() {
                if completed_removal_matches(conn,workspace,subject,worker)? { return Ok(()); }
                return Err(Error::RepositoryConflict("Subject deletion removal reconciliation is incomplete".into()));
            }
            if fence.originals_absent {
                let registered: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM worker_registry WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3)", params![workspace,worker.runtime_id,worker.worker_id], |r|r.get(0))?;
                if registered { return Err(Error::RepositoryConflict("Registered Worker requires canonical original retention".into())); }
                conn.execute("UPDATE subject_deletion_workers SET originals_absent=1 WHERE workspace_id=?1 AND subject_id=?2 AND runtime_id=?3 AND worker_id=?4", params![workspace,subject,worker.runtime_id,worker.worker_id])?;
                conn.execute("UPDATE worker_create_reservations SET state='removed' WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3 AND create_fingerprint=?4", params![workspace,worker.runtime_id,worker.worker_id,request.create_fingerprint])?;
                return Ok(());
            }
            let is_job: Option<bool> = conn.query_row("SELECT is_job FROM subject_deletion_workers WHERE workspace_id=?1 AND subject_id=?2 AND runtime_id=?3 AND worker_id=?4",params![workspace,subject,worker.runtime_id,worker.worker_id],|r|r.get(0)).optional()?;
            let Some(is_job)=is_job else { return Err(Error::RepositoryConflict("Worker is not in the Subject deletion snapshot".into())); };
            let prior:Option<String>=conn.query_row("SELECT prior_removal_operation_id FROM subject_deletion_workers WHERE workspace_id=?1 AND subject_id=?2 AND runtime_id=?3 AND worker_id=?4",params![workspace,subject,worker.runtime_id,worker.worker_id],|r|r.get(0))?;
            if let Some(prior)=prior {
                if prior_removal_matches(conn,workspace,worker,&prior)? {return Ok(());}
                return Err(Error::RepositoryConflict("Saved prior Worker removal receipt no longer matches canonical success".into()));
            }
            let proof: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM worker_removal_operations WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3 AND state='succeeded' AND ((?4=1 AND session_disposition='purge' AND diagnostics_disposition='purge') OR (?4=0 AND session_disposition='archive' AND archive_retention_kind='forever')))
                OR (?4=0 AND EXISTS(SELECT 1 FROM subject_create_recoveries WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3 AND create_fingerprint=?5 AND completed=1))",params![workspace,worker.runtime_id,worker.worker_id,is_job,fence.create_fingerprint],|r|r.get(0))?;
            if !proof { return Err(Error::RepositoryConflict("Runtime exclusion/UnknownWorker does not prove original absence; canonical create recovery or Worker removal receipt is required".into())); }
            Ok(())
            };
            confirm(&tx)?;
            tx.commit()?;
            Ok(())
        })
    }

    /// Never infer physical erasure from absence, failed/unknown execution, or a
    /// cleanup-state flag. Newly removed Job Workers require canonical Purge;
    /// only exact prior successful removals retain their pre-deletion disposition.
    pub fn erase_subject_deleted_jobs(
        &self,
        workspace: &str,
        subject: &str,
        operation: &str,
    ) -> Result<()> {
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            require_operation(&tx,workspace,subject,operation)?;
            require_worker_receipts(&tx,workspace,subject,true)?;
            // Remove child evidence before parents; deletion exceptions remain
            // scoped to the content-free saved Job identities throughout.
            let source = "SELECT s.sequence FROM notification_sources s JOIN subject_deletion_jobs j ON j.workspace_id=s.workspace_id AND j.job_id=json_extract(s.facts_json,'$.backend_job.job_id') WHERE s.role='backend_job' AND j.workspace_id=?1 AND j.subject_id=?2";
            let delivery = format!("SELECT input_event_id FROM notification_sources WHERE sequence IN ({source})");
            for table in ["notification_acknowledgements","notification_job_targets","notification_job_legacy_receipts","notification_failures","notification_requests"] {
                tx.execute(&format!("DELETE FROM {table} WHERE workspace_id=?1 AND delivery_id IN ({delivery})"),params![workspace,subject])?;
            }
            for table in ["notification_source_preparation_failures","notification_source_offers","notification_decisions"] {
                tx.execute(&format!("DELETE FROM {table} WHERE source_sequence IN ({source})"),params![workspace,subject])?;
            }
            tx.execute(&format!("DELETE FROM notification_sources WHERE sequence IN ({source})"),params![workspace,subject])?;
            tx.execute("DELETE FROM backend_jobs WHERE workspace_id=?1 AND job_id IN (SELECT job_id FROM subject_deletion_jobs WHERE workspace_id=?1 AND subject_id=?2)",params![workspace,subject])?;
            tx.execute("UPDATE subject_deletions SET jobs_erased=1 WHERE workspace_id=?1 AND subject_id=?2",params![workspace,subject])?;
            tx.commit()?;
            Ok(())
        })
    }

    /// Confirm all Backend cleanup before the caller commits authoritative domain
    /// completion. This main-DB receipt is not domain completion; a crash after
    /// this commit resumes the same operation and still finishes the domain.
    /// Retains content-free identities for recovery and late admission rejection.
    pub fn finish_subject_deletion_record(
        &self,
        workspace: &str,
        subject: &str,
        operation: &str,
    ) -> Result<()> {
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            require_operation(&tx, workspace, subject, operation)?;
            require_worker_receipts(&tx, workspace, subject, false)?;
            let erased: bool = tx.query_row(
                "SELECT jobs_erased FROM subject_deletions WHERE workspace_id=?1 AND subject_id=?2",
                params![workspace, subject],
                |r| r.get(0),
            )?;
            if !erased {
                return Err(Error::RepositoryConflict(
                    "Subject deletion Job erasure is incomplete".into(),
                ));
            }
            // Canonical removal sets reservation state=removed, but Subject
            // occupancy requires an exact prior success, new canonical removal,
            // or explicit release/create recovery proof.
            for key in [
                format!("subjektiv:{subject}"),
                format!("subjektiv-consolidation:{subject}"),
            ] {
                if let Some((_, state)) = current_worker_singleton_owner(&tx, workspace, &key)?
                    && state.as_deref() != Some("removed")
                {
                    return Err(Error::RepositoryConflict(
                        "Subject singleton occupancy requires canonical removal/create recovery"
                            .into(),
                    ));
                }
            }
            tx.execute(
                "UPDATE subject_deletions SET completed=1 WHERE workspace_id=?1 AND subject_id=?2",
                params![workspace, subject],
            )?;
            tx.commit()?;
            Ok(())
        })
    }
}

fn require_worker_receipts(
    conn: &Connection,
    workspace: &str,
    subject: &str,
    jobs_only: bool,
) -> Result<()> {
    let pinned = conn.prepare("SELECT d.runtime_id,d.worker_id FROM subject_deletion_removals d JOIN subject_deletion_workers w USING(workspace_id,subject_id,runtime_id,worker_id)
        WHERE d.workspace_id=?1 AND d.subject_id=?2 AND (?3=0 OR w.is_job=1)")?
        .query_map(params![workspace,subject,jobs_only],|r|Ok(RuntimeWorkerRef::new(r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for worker in pinned {
        if !completed_removal_matches(conn, workspace, subject, &worker)? {
            return Err(Error::RepositoryConflict(
                "Subject deletion removal reconciliation receipt is incomplete".into(),
            ));
        }
    }
    let missing: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM subject_deletion_workers w WHERE w.workspace_id=?1 AND w.subject_id=?2 AND (?3=0 OR w.is_job=1) AND w.originals_absent=0
        AND NOT EXISTS(SELECT 1 FROM subject_deletion_removals d WHERE d.workspace_id=w.workspace_id AND d.subject_id=w.subject_id AND d.runtime_id=w.runtime_id AND d.worker_id=w.worker_id)
        AND NOT (
          (w.prior_removal_operation_id IS NOT NULL AND EXISTS(SELECT 1 FROM worker_removal_operations r WHERE r.operation_id=w.prior_removal_operation_id AND r.workspace_id=w.workspace_id AND r.runtime_id=w.runtime_id AND r.worker_id=w.worker_id AND r.state='succeeded'))
          OR (w.prior_removal_operation_id IS NULL AND (
            EXISTS(SELECT 1 FROM worker_removal_operations r WHERE r.workspace_id=w.workspace_id AND r.runtime_id=w.runtime_id AND r.worker_id=w.worker_id AND r.state='succeeded'
              AND r.metadata_disposition='tombstone' AND ((w.is_job=1 AND r.session_disposition='purge' AND r.diagnostics_disposition='purge') OR
              (w.is_job=0 AND r.session_disposition='archive' AND r.archive_retention_kind='forever' AND r.diagnostics_disposition='retain')))
            OR (w.is_job=0 AND EXISTS(SELECT 1 FROM subject_create_recoveries r WHERE r.workspace_id=w.workspace_id AND r.runtime_id=w.runtime_id AND r.worker_id=w.worker_id AND r.completed=1))
          ))
        ))",params![workspace,subject,jobs_only],|r|r.get(0))?;
    if missing {
        return Err(Error::RepositoryConflict("Subject deletion requires canonical Worker removal/create recovery receipts; unknown or stale outcomes cannot complete deletion".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend_job::{BackendJobGrants, SubjektivConsolidationGrant};
    const NOW: &str = "2026-10-28T00:00:00Z";

    fn seed(store: &SqliteWorkspaceStore) {
        store.with_conn(|c| {
            c.execute("INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at) VALUES('owner','user','owner','Owner',?1,?1)",[NOW])?;
            for w in ["w","other"] {
                c.execute("INSERT INTO workspaces(workspace_id,display_name,state,created_at,updated_at,owner_account_id) VALUES(?1,?1,'active',?2,?2,'owner')",params![w,NOW])?;
            }
            Ok(())
        }).unwrap();
    }
    fn setup() -> SqliteWorkspaceStore {
        let store = SqliteWorkspaceStore::in_memory_fixture().unwrap();
        seed(&store);
        store
    }
    fn request(id: &str, subject: &str) -> BackendJobRequest {
        BackendJobRequest {
            job_id: id.into(),
            purpose: "subject_deletion_test".into(),
            input_ref: format!("subject:{subject}"),
            input: serde_json::json!({"private":"Subject source"}),
            instruction: "Read the private input".into(),
            profile: "builtin:backend-job".into(),
            grants: BackendJobGrants {
                subjektiv_consolidation: Some(SubjektivConsolidationGrant {
                    subject_id: subject.into(),
                    candidate_ids: vec!["candidate".into()],
                }),
                ..Default::default()
            },
            serialization_key: None,
            source_worker: None,
            notification_target: None,
            limits: Default::default(),
        }
    }
    fn worker(n: u64) -> RuntimeWorkerRef {
        RuntimeWorkerRef::new("r", WorkerId::from_legacy_u64(n).to_string())
    }
    fn registry(store: &SqliteWorkspaceStore, worker: &RuntimeWorkerRef) {
        store.with_conn(|c| {
            c.execute("INSERT INTO worker_registry(workspace_id,runtime_id,worker_id,display_name,retention_state,created_at,updated_at) VALUES('w',?1,?2,'worker','normal',?3,?3)",params![worker.runtime_id,worker.worker_id,NOW])?;
            Ok(())
        }).unwrap();
    }
    fn reservation(
        store: &SqliteWorkspaceStore,
        worker: &RuntimeWorkerRef,
        allocation: &str,
        singleton: Option<&str>,
    ) {
        store.with_conn(|c| {
            c.execute("INSERT INTO worker_create_reservations(workspace_id,runtime_id,worker_id,allocation_key,create_fingerprint,state,created_at,updated_at,singleton_key,singleton_generation) VALUES('w',?1,?2,?3,'fp','reserved',?4,?4,?5,?6)",params![worker.runtime_id,worker.worker_id,allocation,NOW,singleton,singleton.map(|_|1)])?;
            if let Some(key) = singleton {
                c.execute("INSERT INTO worker_singleton_owners(workspace_id,singleton_key,runtime_id,worker_id,generation,created_at,updated_at) VALUES('w',?1,?2,?3,1,?4,?4)",params![key,worker.runtime_id,worker.worker_id,NOW])?;
            }
            Ok(())
        }).unwrap();
    }
    // Isolated store receipt fixture: production only creates this row through
    // canonical removal's exact Runtime response checks and atomic catalog commit.
    fn receipt(
        store: &SqliteWorkspaceStore,
        worker: &RuntimeWorkerRef,
        state: &str,
        session: &str,
        diagnostics: &str,
    ) {
        store.with_conn(|c| {
            c.execute("INSERT INTO worker_removal_operations(operation_id,plan_id,input_fingerprint,workspace_id,runtime_id,worker_id,worker_updated_at,policy_id,policy_digest,session_disposition,metadata_disposition,archive_retention_kind,diagnostics_disposition,blockers_json,state,reason,created_at,updated_at) VALUES(?1,?1,'fp','w',?2,?3,?4,'test','digest',?5,'tombstone','forever',?6,'[]',?7,'test',?4,?4)",params![format!("receipt-{}-{state}",worker.worker_id),worker.runtime_id,worker.worker_id,NOW,session,diagnostics,state])?;
            Ok(())
        }).unwrap();
    }

    #[test]
    fn snapshot_all_attempts_dispatch_allocations_and_unknowns_is_scoped_and_idempotent() {
        let s = setup();
        let req = request("job", "subject");
        let first = s.reserve_backend_job("w", &req, NOW).unwrap();
        let first_worker = worker(1);
        registry(&s, &first_worker);
        s.claim_backend_job_attempt_dispatch("w", "job", &first.attempt.attempt_id, NOW)
            .unwrap();
        s.bind_backend_job_attempt_worker(
            "w",
            "job",
            &first.attempt.attempt_id,
            &first_worker,
            Some("run"),
            NOW,
        )
        .unwrap();
        s.finish_backend_job_attempt(
            "w",
            "job",
            &first.attempt.attempt_id,
            BackendJobAttemptState::Failed,
            "failed",
            "failure",
            NOW,
        )
        .unwrap();
        s.with_conn(|c| {c.execute("UPDATE backend_job_attempts SET worker_cleanup_state='completed' WHERE workspace_id='w'",[])?;Ok(())}).unwrap();
        let second = s.reserve_backend_job_retry("w", "job", NOW).unwrap();
        s.claim_backend_job_attempt_dispatch("w", "job", &second.attempt.attempt_id, NOW)
            .unwrap();
        let allocating = worker(2);
        reservation(
            &s,
            &allocating,
            &crate::backend_job::allocation_key(&second.attempt.attempt_id),
            None,
        );
        s.finish_backend_job_attempt(
            "w",
            "job",
            &second.attempt.attempt_id,
            BackendJobAttemptState::Unknown,
            "unknown",
            "unknown outcome",
            NOW,
        )
        .unwrap();
        let body = worker(3);
        reservation(&s, &body, "body", Some("subjektiv:subject"));
        let legacy = worker(4);
        reservation(
            &s,
            &legacy,
            "legacy",
            Some("subjektiv-consolidation:subject"),
        );
        let delegate = worker(5);
        let other_job = s.reserve_backend_job("other", &req, NOW).unwrap();
        let other_subject = s
            .reserve_backend_job("w", &request("unrelated", "another"), NOW)
            .unwrap();
        s.prepare_subject_deletion("w", "subject", "delete", std::slice::from_ref(&delegate))
            .unwrap();
        assert_eq!(
            s.list_subject_deletion_workers("w", "subject", "delete")
                .unwrap(),
            vec![
                (first_worker, true),
                (allocating, true),
                (body, false),
                (legacy, true),
                (delegate, false)
            ]
        );
        let saved = s.get_backend_job("w", "job").unwrap().unwrap();
        assert_eq!(saved.state, BackendJobState::Failed);
        assert_eq!(saved.failure_category.as_deref(), Some("subject_deleted"));
        assert_eq!(
            s.get_backend_job("other", "job").unwrap().unwrap(),
            other_job.job
        );
        assert_eq!(
            s.get_backend_job("w", "unrelated").unwrap().unwrap(),
            other_subject.job
        );
        let second = s
            .get_backend_job_attempt("w", "job", &second.attempt.attempt_id)
            .unwrap()
            .unwrap();
        assert_eq!(second.state, BackendJobAttemptState::Failed);
        assert_eq!(second.failure_category.as_deref(), Some("subject_deleted"));
        s.prepare_subject_deletion("w", "subject", "delete", &[])
            .unwrap();
        assert_eq!(
            s.list_subject_deletion_workers("w", "subject", "delete")
                .unwrap()
                .len(),
            5
        );
        assert!(
            s.prepare_subject_deletion("w", "subject", "different", &[])
                .is_err()
        );
        assert!(
            s.prepare_subject_deletion("w", "another", "delete", &[])
                .is_err()
        );
        assert!(
            s.list_subject_deletion_workers("other", "subject", "delete")
                .is_err()
        );
        assert!(
            s.reserve_backend_job_retry("w", "job", NOW)
                .unwrap_err()
                .to_string()
                .contains("subject_deleted")
        );
        assert!(
            s.reserve_backend_job("w", &request("late", "subject"), NOW)
                .is_err()
        );
        s.with_conn(|c| {
            assert!(require_worker_admission(c, "w", "body", Some("subjektiv:subject")).is_err());
            assert!(
                require_worker_admission(
                    c,
                    "w",
                    &crate::backend_job::allocation_key(&second.attempt_id),
                    None
                )
                .is_err()
            );
            assert!(
                require_worker_admission(c, "other", "body", Some("subjektiv:subject")).is_ok()
            );
            Ok(())
        })
        .unwrap();
        assert!(
            s.erase_subject_deleted_jobs("w", "subject", "delete")
                .is_err()
        );
        assert!(
            s.finish_subject_deletion_record("w", "subject", "delete")
                .is_err()
        );
    }

    #[test]
    fn completed_result_replay_is_fenced_and_payloads_wait_for_physical_purge() {
        let s = setup();
        let target = worker(10);
        registry(&s, &target);
        let mut req = request("job", "subject");
        req.notification_target = Some(target.clone());
        let r = s.reserve_backend_job("w", &req, NOW).unwrap();
        s.claim_backend_job_attempt_dispatch("w", "job", &r.attempt.attempt_id, NOW)
            .unwrap();
        let w = worker(11);
        registry(&s, &w);
        s.bind_backend_job_attempt_worker("w", "job", &r.attempt.attempt_id, &w, Some("run"), NOW)
            .unwrap();
        let submission = BackendJobResultSubmission {
            job_id: "job".into(),
            attempt_id: r.attempt.attempt_id.clone(),
            input_digest: req.input_digest().unwrap(),
            result: serde_json::json!({"secret":"result body"}),
        };
        s.accept_backend_job_result("w", &w, &submission, NOW)
            .unwrap();
        let source = s
            .job_notification_source("w", "job", &r.attempt.attempt_id)
            .unwrap();
        s.route_job_notification(
            "w",
            &source,
            &JobNotificationFacts::Result("secret notice body".into()),
        )
        .unwrap();
        assert!(
            s.job_notification_request("w", &source.delivery_id)
                .unwrap()
                .is_some()
        );
        s.prepare_subject_deletion("w", "subject", "delete", &[])
            .unwrap();
        assert!(
            s.route_job_notification(
                "w",
                &source,
                &JobNotificationFacts::Result("late secret".into())
            )
            .is_err()
        );
        assert!(
            s.accept_backend_job_result("w", &w, &submission, NOW)
                .unwrap_err()
                .to_string()
                .contains("subject_deleted")
        );
        assert!(
            s.erase_subject_deleted_jobs("w", "subject", "delete")
                .is_err()
        );
        assert!(
            s.get_backend_job("w", "job")
                .unwrap()
                .unwrap()
                .result
                .is_some()
        );
        receipt(&s, &w, "failed", "purge", "purge");
        assert!(
            s.erase_subject_deleted_jobs("w", "subject", "delete")
                .is_err()
        );
        receipt(&s, &w, "succeeded", "archive", "retain");
        assert!(
            s.erase_subject_deleted_jobs("w", "subject", "delete")
                .is_err()
        );
        s.with_conn(|c| {c.execute("UPDATE worker_removal_operations SET session_disposition='purge',diagnostics_disposition='purge' WHERE state='succeeded'",[])?;Ok(())}).unwrap();
        s.erase_subject_deleted_jobs("w", "subject", "delete")
            .unwrap();
        assert!(s.get_backend_job("w", "job").unwrap().is_none());
        assert!(
            s.get_backend_job_attempt("w", "job", &r.attempt.attempt_id)
                .unwrap()
                .is_none()
        );
        s.with_conn(|c| {
            for table in [
                "notification_sources",
                "notification_decisions",
                "notification_requests",
                "notification_job_targets",
            ] {
                assert_eq!(
                    c.query_row(
                        &format!("SELECT count(*) FROM {table} WHERE workspace_id='w'"),
                        [],
                        |r| r.get::<_, i64>(0)
                    )?,
                    0
                );
            }
            assert_eq!(
                c.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r
                    .get::<_, i64>(
                    0
                ))?,
                0
            );
            Ok(())
        })
        .unwrap();
        s.erase_subject_deleted_jobs("w", "subject", "delete")
            .unwrap();
        s.finish_subject_deletion_record("w", "subject", "delete")
            .unwrap();
        s.finish_subject_deletion_record("w", "subject", "delete")
            .unwrap();
        assert!(
            s.reserve_backend_job("w", &request("job", "different-subject"), NOW)
                .is_err()
        );
        assert!(
            s.finish_subject_deletion_record("w", "subject", "wrong-operation")
                .is_err()
        );
    }

    #[test]
    fn migration_is_atomic_matches_fresh_and_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("main.db");
        crate::store::tests::prepare_retained_schema(&path, 104);
        let c = Connection::open(&path).unwrap();
        configure_sqlite(&c).unwrap();
        c.execute_batch("CREATE TRIGGER interrupt_subject_deletion BEFORE INSERT ON __yoi_schema_migrations WHEN NEW.version=105 BEGIN SELECT RAISE(ABORT,'interrupted'); END;").unwrap();
        assert!(migrate(&c).is_err());
        assert_eq!(current_schema_version(&c).unwrap(), 104);
        assert!(!table_exists(&c, "subject_deletions").unwrap());
        c.execute_batch("DROP TRIGGER interrupt_subject_deletion")
            .unwrap();
        migrate(&c).unwrap();
        assert!(migrate(&c).is_err());
        let schema = |c: &Connection| -> Result<Vec<(String, String)>> {
            Ok(c.prepare("SELECT name,sql FROM sqlite_schema WHERE name LIKE 'subject_delet%' OR name LIKE 'subject_deleted%' OR name IN ('notification_sources_no_delete','notification_requests_no_delete','notification_job_targets_no_delete') ORDER BY name")?
                .query_map([],|r|Ok((r.get(0)?,r.get::<_,String>(1)?.split_whitespace().collect::<Vec<_>>().join(" "))))?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        };
        let fresh = SqliteWorkspaceStore::in_memory_fixture().unwrap();
        assert_eq!(schema(&c).unwrap(), fresh.with_conn(schema).unwrap());
        drop(c);
        let s = SqliteWorkspaceStore::open_for_test(&path).unwrap();
        seed(&s);
        let old_worker = worker(61);
        receipt(&s, &old_worker, "succeeded", "purge", "purge");
        s.prepare_subject_deletion("w", "subject", "delete", std::slice::from_ref(&old_worker))
            .unwrap();
        drop(s);
        let s = SqliteWorkspaceStore::open_for_test(&path).unwrap();
        assert_eq!(
            saved_prior(&s, "subject", &old_worker),
            Some(format!("receipt-{}-succeeded", old_worker.worker_id))
        );
        s.prepare_subject_deletion("w", "subject", "delete", &[])
            .unwrap();
        s.erase_subject_deleted_jobs("w", "subject", "delete")
            .unwrap();
        s.finish_subject_deletion_record("w", "subject", "delete")
            .unwrap();
    }

    #[test]
    fn concurrent_job_reservation_is_either_snapshotted_or_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("main.db");
        let s = SqliteWorkspaceStore::open_for_test(&path).unwrap();
        seed(&s);
        let other = SqliteWorkspaceStore::open_for_test(&path).unwrap();
        let gate = std::sync::Arc::new(std::sync::Barrier::new(2));
        let other_gate = gate.clone();
        let reserve = std::thread::spawn(move || {
            other_gate.wait();
            other.reserve_backend_job("w", &request("race", "subject"), NOW)
        });
        gate.wait();
        s.prepare_subject_deletion("w", "subject", "delete", &[])
            .unwrap();
        let reserved = reserve.join().unwrap().is_ok();
        s.with_conn(|c| {
            let included:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM subject_deletion_jobs WHERE workspace_id='w' AND job_id='race')",[],|r|r.get(0))?;
            assert_eq!(included,reserved);Ok(())
        }).unwrap();
        if reserved {
            assert_eq!(
                s.get_backend_job("w", "race").unwrap().unwrap().state,
                BackendJobState::Failed
            );
        }
        assert!(
            s.reserve_backend_job("w", &request("later", "subject"), NOW)
                .is_err()
        );
    }

    #[test]
    fn singleton_release_waits_for_canonical_removal_and_absence_is_not_success() {
        let s = setup();
        let body = worker(21);
        reservation(&s, &body, "body", Some("subjektiv:subject"));
        s.prepare_subject_deletion("w", "subject", "delete", &[])
            .unwrap();
        let fence = runtime_api::SubjectDeleteWorkerResponse {
            workspace_id: "w".into(),
            worker_id: WorkerId::from_legacy_u64(21),
            create_fingerprint: "fp".into(),
            operation_id: s
                .subject_deletion_worker_request("w", "subject", "delete", &body)
                .unwrap()
                .operation_id,
            originals_absent: false,
        };
        assert!(
            s.confirm_subject_deletion_absence("w", "subject", "delete", &body, &fence)
                .is_err()
        );
        assert!(
            s.current_worker_singleton_owner("w", "subjektiv:subject")
                .unwrap()
                .is_some()
        );
        receipt(&s, &body, "failed", "archive", "retain");
        assert!(
            s.confirm_subject_deletion_absence("w", "subject", "delete", &body, &fence)
                .is_err()
        );
        s.erase_subject_deleted_jobs("w", "subject", "delete")
            .unwrap();
        assert!(
            s.finish_subject_deletion_record("w", "subject", "delete")
                .is_err()
        );
        receipt(&s, &body, "succeeded", "archive", "retain");
        assert!(
            s.finish_subject_deletion_record("w", "subject", "delete")
                .is_err(),
            "reservation is not yet terminal"
        );
        s.with_conn(|c| {
            c.execute(
                "UPDATE worker_create_reservations SET state='removed' WHERE workspace_id='w'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        assert!(
            s.current_worker_singleton_owner("w", "subjektiv:subject")
                .unwrap()
                .is_none()
        );
        s.confirm_subject_deletion_absence("w", "subject", "delete", &body, &fence)
            .unwrap();
        s.finish_subject_deletion_record("w", "subject", "delete")
            .unwrap();
    }

    fn saved_prior(
        s: &SqliteWorkspaceStore,
        subject: &str,
        w: &RuntimeWorkerRef,
    ) -> Option<String> {
        s.with_conn(|c|Ok(c.query_row("SELECT prior_removal_operation_id FROM subject_deletion_workers WHERE workspace_id='w' AND subject_id=?1 AND runtime_id=?2 AND worker_id=?3",params![subject,w.runtime_id,w.worker_id],|r|r.get(0))?)).unwrap()
    }

    #[test]
    fn prior_archived_job_and_purged_body_preserve_original_receipts_and_other_subject() {
        let s = setup();
        let req = request("archived-job", "subject");
        let job = s.reserve_backend_job("w", &req, NOW).unwrap();
        let jw = worker(31);
        registry(&s, &jw);
        s.claim_backend_job_attempt_dispatch("w", &req.job_id, &job.attempt.attempt_id, NOW)
            .unwrap();
        s.bind_backend_job_attempt_worker(
            "w",
            &req.job_id,
            &job.attempt.attempt_id,
            &jw,
            Some("run"),
            NOW,
        )
        .unwrap();
        s.accept_backend_job_result(
            "w",
            &jw,
            &BackendJobResultSubmission {
                job_id: req.job_id.clone(),
                attempt_id: job.attempt.attempt_id.clone(),
                input_digest: req.input_digest().unwrap(),
                result: serde_json::json!({"secret":"old result"}),
            },
            NOW,
        )
        .unwrap();
        receipt(&s, &jw, "succeeded", "archive", "retain");
        let job_receipt = format!("receipt-{}-succeeded", jw.worker_id);
        let body = worker(32);
        reservation(&s, &body, "body", Some("subjektiv:subject"));
        receipt(&s, &body, "succeeded", "purge", "purge");
        let body_receipt = format!("receipt-{}-succeeded", body.worker_id);
        // A different Subject's already-removed body remains occupied until its
        // own deletion/release. Deleting a delegating Subject must not release it.
        let other_body = worker(33);
        reservation(&s, &other_body, "other-body", Some("subjektiv:another"));
        receipt(&s, &other_body, "succeeded", "purge", "purge");
        let other_job = s
            .reserve_backend_job("w", &request("other-job", "another"), NOW)
            .unwrap();
        s.with_conn(|c| {
            c.execute("DELETE FROM worker_registry WHERE workspace_id='w' AND worker_id=?1",[&jw.worker_id])?;
            c.execute("UPDATE worker_create_reservations SET state='removed' WHERE workspace_id='w'",[])?;
            c.execute("INSERT INTO worker_session_archives(archive_id,workspace_id,runtime_id,worker_id,session_id,checksum_sha256,content_bytes,policy_id,policy_digest,operation_id,committed_at,expires_at) VALUES('old-archive','w',?1,?2,'session','checksum',42,'test','digest',?3,?4,?5)",params![jw.runtime_id,jw.worker_id,job_receipt,NOW,"2027-01-01T00:00:00Z"])?;
            // Prior metadata/retention selections need not match the new policy.
            c.execute("UPDATE worker_removal_operations SET metadata_disposition='purge',archive_retention_kind='for_seconds',archive_retention_seconds=300 WHERE operation_id=?1",[&job_receipt])?;
            Ok(())
        }).unwrap();
        let archive_before = s.worker_session_archive("w", &jw, "session").unwrap();
        assert!(archive_before.is_some());
        s.prepare_subject_deletion("w", "subject", "delete", std::slice::from_ref(&other_body))
            .unwrap();
        assert_eq!(saved_prior(&s, "subject", &jw), Some(job_receipt.clone()));
        assert_eq!(
            saved_prior(&s, "subject", &body),
            Some(body_receipt.clone())
        );
        assert_eq!(
            s.recover_worker_removal_execution("w", &jw)
                .unwrap()
                .unwrap()
                .plan
                .operation_id,
            job_receipt
        );
        assert_eq!(
            s.recover_worker_removal_execution("w", &body)
                .unwrap()
                .unwrap()
                .plan
                .operation_id,
            body_receipt
        );
        s.erase_subject_deleted_jobs("w", "subject", "delete")
            .unwrap();
        // Main finish intentionally precedes authoritative domain completion.
        s.finish_subject_deletion_record("w", "subject", "delete")
            .unwrap();
        assert_eq!(
            s.worker_session_archive("w", &jw, "session").unwrap(),
            archive_before
        );
        assert!(s.get_backend_job("w", "archived-job").unwrap().is_none());
        assert_eq!(
            s.get_backend_job("w", "other-job").unwrap().unwrap(),
            other_job.job
        );
        assert!(
            s.current_worker_singleton_owner("w", "subjektiv:subject")
                .unwrap()
                .is_none()
        );
        assert!(
            s.current_worker_singleton_owner("w", "subjektiv:another")
                .unwrap()
                .is_some()
        );
        s.with_conn(|c| {
            let old:(String,String,String)=c.query_row("SELECT state,session_disposition,metadata_disposition FROM worker_removal_operations WHERE operation_id=?1",[&job_receipt],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
            assert_eq!(old,("succeeded".into(),"archive".into(),"purge".into()));
            Ok(())
        }).unwrap();
    }

    #[test]
    fn late_success_and_unknown_removal_cannot_become_a_prior_receipt() {
        let s = setup();
        let job_worker = worker(41);
        let body = worker(42);
        reservation(
            &s,
            &job_worker,
            "job-owner",
            Some("subjektiv-consolidation:subject"),
        );
        registry(&s, &job_worker);
        registry(&s, &body);
        receipt(&s, &job_worker, "executing", "archive", "retain");
        receipt(&s, &body, "failed", "purge", "purge");
        s.prepare_subject_deletion("w", "subject", "delete", std::slice::from_ref(&body))
            .unwrap();
        assert_eq!(saved_prior(&s, "subject", &job_worker), None);
        assert_eq!(saved_prior(&s, "subject", &body), None);
        receipt(&s, &job_worker, "succeeded", "archive", "retain");
        receipt(&s, &body, "succeeded", "purge", "purge");
        s.prepare_subject_deletion("w", "subject", "delete", &[])
            .unwrap();
        assert_eq!(
            saved_prior(&s, "subject", &job_worker),
            None,
            "replay must not bless a later success"
        );
        assert!(
            s.erase_subject_deleted_jobs("w", "subject", "delete")
                .is_err(),
            "new Job Archive is not Purge"
        );
        s.with_conn(|c| {
            assert!(c.execute("UPDATE subject_deletion_workers SET prior_removal_operation_id=?1 WHERE workspace_id='w' AND worker_id=?2",params![format!("receipt-{}-succeeded",job_worker.worker_id),job_worker.worker_id]).is_err());
            c.execute("UPDATE worker_removal_operations SET session_disposition='purge',diagnostics_disposition='purge' WHERE worker_id=?1 AND state='succeeded'",[&job_worker.worker_id])?;
            c.execute("UPDATE worker_create_reservations SET state='removed' WHERE workspace_id='w'",[])?;
            Ok(())
        }).unwrap();
        assert!(
            s.erase_subject_deleted_jobs("w", "subject", "delete")
                .is_err(),
            "replacement canonical Purge cannot bypass the pinned unfinished removal"
        );
        assert!(
            s.finish_subject_deletion_record("w", "subject", "delete")
                .is_err(),
            "new body Purge is not Archive Forever"
        );
    }

    fn in_flight(s: &SqliteWorkspaceStore, w: &RuntimeWorkerRef, state: &str, is_job: bool) {
        registry(s, w);
        reservation(
            s,
            w,
            "pending",
            Some(if is_job {
                "subjektiv-consolidation:subject"
            } else {
                "subjektiv:subject"
            }),
        );
        receipt(s, w, state, "archive", "retain");
        s.with_conn(|c| {
            c.execute("UPDATE worker_registry SET profile='builtin:old-profile' WHERE worker_id=?1",[&w.worker_id])?;
            c.execute("UPDATE worker_removal_operations SET archive_id='old-archive',metadata_disposition='purge',archive_retention_kind='for_seconds',archive_retention_seconds=60 WHERE worker_id=?1",[&w.worker_id])?;
            Ok(())
        }).unwrap();
    }

    fn execution_result(
        request: &WorkerRetentionExecutionRequest,
    ) -> WorkerRetentionExecutionResult {
        WorkerRetentionExecutionResult {
            operation_id: request.operation_id.clone(),
            input_fingerprint: request.input_fingerprint.clone(),
            worker_id: request.worker_id,
            session_disposition: request.session_disposition,
            diagnostics_disposition: request.diagnostics_disposition,
            archive: request.archive_id.as_ref().map(|archive_id| {
                worker_runtime::retention::WorkerSessionArchiveManifest {
                    schema_version: 1,
                    archive_id: archive_id.clone(),
                    workspace_id: request.workspace_id.clone(),
                    source_runtime_id: request.source_runtime_id.clone(),
                    source_worker_id: request.worker_id,
                    source_session_id: "old-session".into(),
                    source_active_segment_id: None,
                    segment_ids: vec![],
                    source_created_at: request.source_created_at.clone(),
                    removed_at: request.removed_at.clone(),
                    archived_at_unix_seconds: 0,
                    effective_profile: request.effective_profile.clone(),
                    retention_class: request.retention_class.clone(),
                    content_checksum_sha256: "fixture-checksum".into(),
                    content_bytes: 0,
                    content_file_count: 0,
                    policy_id: request.policy_id.clone(),
                    operation_id: request.operation_id.clone(),
                    input_fingerprint: request.input_fingerprint.clone(),
                }
            }),
            source_removed: true,
            diagnostics_retained: request.diagnostics_disposition == DiagnosticsDisposition::Retain,
        }
    }

    fn succeed(s: &SqliteWorkspaceStore, request: &WorkerRetentionExecutionRequest) {
        // Isolated store proof fixture. HTTP/Runtime tests exercise canonical
        // commit; here success alone must still leave reconciliation incomplete.
        s.with_conn(|c| {
            c.execute("UPDATE worker_removal_operations SET state='succeeded' WHERE operation_id=?1",[&request.operation_id])?;
            c.execute("DELETE FROM worker_registry WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3",params![request.workspace_id,request.source_runtime_id,request.worker_id.to_string()])?;
            c.execute("UPDATE worker_create_reservations SET state='removed' WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3",params![request.workspace_id,request.source_runtime_id,request.worker_id.to_string()])?;
            Ok(())
        }).unwrap();
    }

    #[test]
    fn unfinished_removal_pins_full_identity_and_requires_exact_success_before_release() {
        let s = setup();
        let w = worker(71);
        in_flight(&s, &w, "executing", false);
        // An executing plan may have an older registry epoch. Its exact Runtime
        // request is still authoritative; do not turn this into permanent stale.
        s.with_conn(|c| {
            c.execute(
                "UPDATE worker_registry SET updated_at='later' WHERE worker_id=?1",
                [&w.worker_id],
            )?;
            Ok(())
        })
        .unwrap();
        s.prepare_subject_deletion("w", "subject", "delete", &[])
            .unwrap();
        let saved = s.subject_deletion_removal("w", &w).unwrap().unwrap();
        let req = &saved.request;
        assert!(!saved.completed);
        assert!(!saved.is_job);
        assert_eq!(req.source_created_at, NOW);
        assert_eq!(req.removed_at, NOW);
        assert_eq!(
            req.effective_profile.as_deref(),
            Some("builtin:old-profile")
        );
        assert_eq!(req.retention_class, None);
        assert_eq!(req.archive_id.as_deref(), Some("old-archive"));
        assert_eq!(req.policy_id, "test");
        assert_eq!(saved_prior(&s, "subject", &w), None);
        s.with_conn(|c| {
            assert!(removal_reconciliation_matches(c,"w",&w,&req.operation_id,&req.input_fingerprint)?);
            assert!(!removal_reconciliation_matches(c,"w",&w,&req.operation_id,"changed")?);
            assert!(!removal_reconciliation_matches(c,"other",&w,&req.operation_id,&req.input_fingerprint)?);
            assert!(c.execute("UPDATE subject_deletion_removals SET request_json=request_json WHERE workspace_id='w'",[]).is_err());
            Ok(())
        }).unwrap();
        let result = execution_result(req);
        assert!(
            s.confirm_subject_deletion_removal("w", &w, req, &result, None)
                .is_err()
        );
        succeed(&s, req);
        assert!(
            s.current_worker_singleton_owner("w", "subjektiv:subject")
                .unwrap()
                .is_some()
        );
        assert!(
            s.finish_subject_deletion_record("w", "subject", "delete")
                .is_err()
        );
        let fence_request = s
            .subject_deletion_worker_request("w", "subject", "delete", &w)
            .unwrap();
        let fence = runtime_api::SubjectDeleteWorkerResponse {
            workspace_id: fence_request.workspace_id,
            worker_id: fence_request.worker_id,
            create_fingerprint: fence_request.create_fingerprint,
            operation_id: fence_request.operation_id,
            originals_absent: true,
        };
        assert!(
            s.confirm_subject_deletion_absence("w", "subject", "delete", &w, &fence)
                .is_err()
        );
        for (field, value) in [
            ("operation_id", serde_json::json!("changed")),
            ("input_fingerprint", serde_json::json!("changed")),
            ("archive_id", serde_json::json!("changed")),
            ("workspace_id", serde_json::json!("other")),
            ("source_runtime_id", serde_json::json!("other-runtime")),
            ("worker_id", serde_json::json!(worker(72).worker_id)),
            ("source_created_at", serde_json::json!("changed")),
            ("removed_at", serde_json::json!("changed")),
            ("effective_profile", serde_json::json!("changed")),
            ("retention_class", serde_json::json!("changed")),
            ("policy_id", serde_json::json!("changed")),
            ("session_disposition", serde_json::json!("purge")),
            ("diagnostics_disposition", serde_json::json!("purge")),
        ] {
            let mut changed = serde_json::to_value(req).unwrap();
            changed[field] = value;
            let changed: WorkerRetentionExecutionRequest = serde_json::from_value(changed).unwrap();
            assert!(
                s.confirm_subject_deletion_removal("w", &w, &changed, &result, None)
                    .is_err(),
                "{field}"
            );
            assert!(
                !s.subject_deletion_removal("w", &w)
                    .unwrap()
                    .unwrap()
                    .completed
            );
        }
        for (field, value) in [
            ("operation_id", serde_json::json!("changed")),
            ("input_fingerprint", serde_json::json!("changed")),
            ("worker_id", serde_json::json!(worker(72).worker_id)),
            ("session_disposition", serde_json::json!("purge")),
            ("diagnostics_disposition", serde_json::json!("purge")),
            ("source_removed", serde_json::json!(false)),
            ("diagnostics_retained", serde_json::json!(false)),
            ("archive", serde_json::Value::Null),
        ] {
            let mut changed = serde_json::to_value(&result).unwrap();
            changed[field] = value;
            let changed = serde_json::from_value(changed).unwrap();
            assert!(
                s.confirm_subject_deletion_removal("w", &w, req, &changed, None)
                    .is_err(),
                "{field}"
            );
        }
        s.confirm_subject_deletion_removal("w", &w, req, &result, None)
            .unwrap();
        assert_eq!(
            saved_prior(&s, "subject", &w),
            None,
            "not a late prior success"
        );
        assert!(
            s.current_worker_singleton_owner("w", "subjektiv:subject")
                .unwrap()
                .is_none()
        );
        s.confirm_subject_deletion_absence("w", "subject", "delete", &w, &fence)
            .unwrap();
        s.erase_subject_deleted_jobs("w", "subject", "delete")
            .unwrap();
        s.finish_subject_deletion_record("w", "subject", "delete")
            .unwrap();
        s.with_conn(|c| {
            assert!(removal_reconciliation_matches(
                c,
                "w",
                &w,
                &req.operation_id,
                &req.input_fingerprint
            )?);
            c.execute(
                "UPDATE worker_removal_operations SET policy_id='changed' WHERE operation_id=?1",
                [&req.operation_id],
            )?;
            assert!(!completed_removal_matches(c, "w", "subject", &w)?);
            Ok(())
        })
        .unwrap();
        assert!(
            s.finish_subject_deletion_record("w", "subject", "delete")
                .is_err()
        );
    }

    #[test]
    fn job_reconciliation_requires_additional_purge_and_survives_reopen_and_retry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.db");
        let s = SqliteWorkspaceStore::open_for_test(&path).unwrap();
        seed(&s);
        let w = worker(81);
        in_flight(&s, &w, "failed", true);
        let other = worker(82);
        registry(&s, &other);
        reservation(&s, &other, "other-allocation", Some("subjektiv:another"));
        receipt(&s, &other, "executing", "archive", "retain");
        s.prepare_subject_deletion("w", "another", "other-delete", &[])
            .unwrap();
        let other_before = s.subject_deletion_removal("w", &other).unwrap();
        s.prepare_subject_deletion("w", "subject", "delete", &[])
            .unwrap();
        let req = s
            .subject_deletion_removal("w", &w)
            .unwrap()
            .unwrap()
            .request;
        drop(s);
        let s = SqliteWorkspaceStore::open_for_test(&path).unwrap();
        s.prepare_subject_deletion("w", "subject", "delete", &[])
            .unwrap();
        assert_eq!(
            s.subject_deletion_removal("w", &w)
                .unwrap()
                .unwrap()
                .request,
            req
        );
        assert!(s.subject_deletion_removal("other", &w).unwrap().is_none());
        let result = execution_result(&req);
        let purge = runtime_api::SubjectJobRemovalPurgeResponse {
            workspace_id: "w".into(),
            worker_id: req.worker_id,
            operation_id: req.operation_id.clone(),
            input_fingerprint: req.input_fingerprint.clone(),
        };
        assert!(
            s.confirm_subject_deletion_removal("w", &w, &req, &result, Some(&purge))
                .is_err(),
            "failed is not success"
        );
        succeed(&s, &req);
        assert!(
            s.current_worker_singleton_owner("w", "subjektiv-consolidation:subject")
                .unwrap()
                .is_some(),
            "canonical success cannot release Job occupancy before physical purge"
        );
        assert!(
            s.confirm_subject_deletion_removal("w", &w, &req, &result, None)
                .is_err()
        );
        assert!(
            s.erase_subject_deleted_jobs("w", "subject", "delete")
                .is_err()
        );
        for (field, value) in [
            ("workspace_id", serde_json::json!("other")),
            ("worker_id", serde_json::json!(worker(82).worker_id)),
            ("operation_id", serde_json::json!("changed")),
            ("input_fingerprint", serde_json::json!("changed")),
        ] {
            let mut changed = serde_json::to_value(&purge).unwrap();
            changed[field] = value;
            let changed = serde_json::from_value(changed).unwrap();
            assert!(
                s.confirm_subject_deletion_removal("w", &w, &req, &result, Some(&changed))
                    .is_err(),
                "{field}"
            );
        }
        assert!(
            !s.subject_deletion_removal("w", &w)
                .unwrap()
                .unwrap()
                .completed
        );
        s.confirm_subject_deletion_removal("w", &w, &req, &result, Some(&purge))
            .unwrap();
        drop(s);
        let s = SqliteWorkspaceStore::open_for_test(&path).unwrap();
        s.confirm_subject_deletion_removal("w", &w, &req, &result, Some(&purge))
            .unwrap();
        assert!(
            s.subject_deletion_removal("w", &w)
                .unwrap()
                .unwrap()
                .completed
        );
        assert_eq!(saved_prior(&s, "subject", &w), None);
        assert!(
            s.current_worker_singleton_owner("w", "subjektiv-consolidation:subject")
                .unwrap()
                .is_none()
        );
        s.with_conn(|c| {
            let old:(String,String,String) = c.query_row("SELECT state,session_disposition,metadata_disposition FROM worker_removal_operations WHERE operation_id=?1",[&req.operation_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
            assert_eq!(old,("succeeded".into(),"archive".into(),"purge".into()));
            assert!(c.execute("UPDATE subject_deletion_removals SET completed=0",[]).is_err());
            assert_eq!(c.query_row("SELECT count(*) FROM pragma_foreign_key_check",[],|r|r.get::<_,i64>(0))?,0);
            Ok(())
        }).unwrap();
        assert_eq!(
            s.subject_deletion_removal("w", &other).unwrap(),
            other_before
        );
        assert!(
            s.current_worker_singleton_owner("w", "subjektiv:another")
                .unwrap()
                .is_some()
        );
        s.erase_subject_deleted_jobs("w", "subject", "delete")
            .unwrap();
        s.finish_subject_deletion_record("w", "subject", "delete")
            .unwrap();
    }

    #[test]
    fn planned_pin_uses_current_epoch_and_never_replaces_prior_success() {
        for (state, current, expected) in [
            ("planned", true, true),
            ("planned", false, false),
            ("failed", false, true),
        ] {
            let s = setup();
            let w = worker(91);
            in_flight(&s, &w, state, false);
            if !current {
                s.with_conn(|c| {
                    c.execute("UPDATE worker_registry SET updated_at='later'", [])?;
                    Ok(())
                })
                .unwrap();
            }
            s.prepare_subject_deletion("w", "subject", "delete", &[])
                .unwrap();
            assert_eq!(
                s.subject_deletion_removal("w", &w).unwrap().is_some(),
                expected,
                "{state}/{current}"
            );
        }
        let s = setup();
        let w = worker(92);
        in_flight(&s, &w, "executing", true);
        receipt(&s, &w, "succeeded", "archive", "retain");
        s.prepare_subject_deletion("w", "subject", "delete", &[])
            .unwrap();
        assert!(saved_prior(&s, "subject", &w).is_some());
        assert!(s.subject_deletion_removal("w", &w).unwrap().is_none());
        s.erase_subject_deleted_jobs("w", "subject", "delete")
            .unwrap();
    }

    #[test]
    fn saved_prior_receipt_must_remain_the_exact_success_not_a_replacement() {
        let s = setup();
        let w = worker(51);
        receipt(&s, &w, "succeeded", "archive", "retain");
        s.prepare_subject_deletion("w", "subject", "delete", std::slice::from_ref(&w))
            .unwrap();
        let old = saved_prior(&s, "subject", &w).unwrap();
        s.with_conn(|c| {
            // Simulate inconsistent retained evidence. A different valid new
            // receipt must not silently replace the immutable snapshotted one.
            c.execute("UPDATE worker_removal_operations SET state='failed' WHERE operation_id=?1",[&old])?;
            c.execute("INSERT INTO worker_removal_operations SELECT 'replacement','replacement',input_fingerprint,workspace_id,runtime_id,worker_id,worker_updated_at,policy_id,policy_digest,session_disposition,metadata_disposition,archive_retention_kind,archive_retention_seconds,diagnostics_disposition,diagnostics_retention_seconds,NULL,blockers_json,'succeeded',reason,failure_category,created_at,updated_at FROM worker_removal_operations WHERE operation_id=?1",[&old])?;
            Ok(())
        }).unwrap();
        s.erase_subject_deleted_jobs("w", "subject", "delete")
            .unwrap();
        assert!(
            s.finish_subject_deletion_record("w", "subject", "delete")
                .is_err()
        );
        s.with_conn(|c| {
            c.execute(
                "UPDATE worker_removal_operations SET state='succeeded' WHERE operation_id=?1",
                [&old],
            )?;
            Ok(())
        })
        .unwrap();
        s.finish_subject_deletion_record("w", "subject", "delete")
            .unwrap();
    }
}
