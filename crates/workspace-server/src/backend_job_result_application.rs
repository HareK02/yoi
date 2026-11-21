//! Host-owned domain application settlement, not an execution/cleanup transition.
use super::*;

pub(super) const MIGRATION_NAME: &str = "Backend Job attempt result application disposition";

/// Additive and reader-first: historical attempts remain explicitly unsettled.
/// The same DDL is used by migration and the fresh installer.
pub(super) const SCHEMA: &str = r#"
ALTER TABLE backend_job_attempts ADD COLUMN result_application_disposition TEXT
    CHECK(result_application_disposition IS NULL OR result_application_disposition IN ('applied','not_required'));
ALTER TABLE backend_job_attempts ADD COLUMN result_application_recorded_at TEXT
    CHECK((result_application_disposition IS NULL AND result_application_recorded_at IS NULL)
       OR (result_application_disposition IS NOT NULL AND result_application_recorded_at IS NOT NULL
           AND length(trim(result_application_recorded_at))>0));
CREATE TRIGGER backend_job_result_application_immutable
BEFORE UPDATE OF result_application_disposition,result_application_recorded_at ON backend_job_attempts
WHEN OLD.result_application_disposition IS NOT NULL AND (
    NEW.result_application_disposition IS NOT OLD.result_application_disposition
    OR NEW.result_application_recorded_at IS NOT OLD.result_application_recorded_at
)
BEGIN SELECT RAISE(ABORT,'immutable Backend Job result application disposition'); END;
"#;

pub(super) fn migrate(conn: &Connection) -> Result<()> {
    if !conn.is_autocommit() || current_schema_version(conn)? != 138 {
        return Err(Error::Store(
            "Job result application migration requires schema 138".into(),
        ));
    }
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(SCHEMA)?;
    tx.execute(
        "INSERT INTO __yoi_schema_migrations(version,name) VALUES(139,?1)",
        [MIGRATION_NAME],
    )?;
    tx.commit()?;
    Ok(())
}

pub(super) fn record(
    store: &SqliteWorkspaceStore,
    workspace_id: &str,
    expected: &BackendJobAttemptRecord,
    expected_intent_fingerprint: &str,
    disposition: JobResultApplicationDisposition,
    now: &str,
) -> Result<BackendJobResultApplicationReceipt> {
    if now.trim().is_empty() {
        return Err(Error::InvalidInput(
            "Job result application requires a timestamp".into(),
        ));
    }
    store.with_conn_mut(|conn| {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let conflict = || Error::RepositoryConflict("Backend Job result application binding or settlement changed".into());
        let job = read_backend_job(&tx, workspace_id, &expected.job_id)?.ok_or_else(conflict)?;
        let attempt = read_backend_job_attempt(&tx, workspace_id, &expected.job_id, &expected.attempt_id)?
            .ok_or_else(conflict)?;
        // Compare immutable identity, not a lifecycle snapshot: an old owner must
        // never settle or replay a superseded attempt, even if it has a receipt.
        if expected.workspace_id != workspace_id
            || attempt.attempt != expected.attempt
            || attempt.attempt != job.current_attempt
            || attempt.attempt_id != attempt_id(&job.request.job_id, job.current_attempt)
            || attempt.input_digest != expected.input_digest
            || attempt.input_digest != job.input_digest
            || job.intent_fingerprint != expected_intent_fingerprint
            || attempt.worker != expected.worker
            || attempt.runtime_run_id != expected.runtime_run_id
        {
            return Err(conflict());
        }
        if disposition == JobResultApplicationDisposition::Applied
            && (attempt.state != BackendJobAttemptState::Completed
                || !matches!(job.state, BackendJobState::Finalizing | BackendJobState::Completed)
                || attempt.result.is_none()
                || attempt.result_digest.is_none()
                || attempt.result != job.result
                || attempt.result_digest != job.result_digest
                || attempt.result_digest != expected.result_digest)
        {
            return Err(Error::InvalidInput("Applied requires the accepted Backend Job result".into()));
        }
        let replayed = attempt.result_application_disposition.is_some();
        let recorded_at = if let Some(saved) = attempt.result_application_disposition {
            if saved != disposition {
                return Err(conflict());
            }
            attempt.result_application_recorded_at.clone()
                .ok_or_else(|| Error::Store("Job application settlement timestamp missing".into()))?
        } else {
            // This projects an already accepted historical result or a consumer's
            // canonical deletion fence. Neither case issues new domain authority.
            // Do not touch updated_at or any execution/result/failure/cleanup/
            // input state: application settlement is an independent fact.
            let changed = tx.execute(
                "UPDATE backend_job_attempts SET result_application_disposition=?4,result_application_recorded_at=?5 WHERE workspace_id=?1 AND job_id=?2 AND attempt_id=?3 AND result_application_disposition IS NULL",
                params![workspace_id, expected.job_id, expected.attempt_id, disposition.as_str(), now],
            )?;
            if changed != 1 { return Err(conflict()); }
            now.to_string()
        };
        let receipt = BackendJobResultApplicationReceipt {
            workspace_id: workspace_id.into(),
            job_id: attempt.job_id,
            attempt_id: attempt.attempt_id,
            attempt: attempt.attempt,
            input_digest: attempt.input_digest,
            intent_fingerprint: job.intent_fingerprint,
            worker: attempt.worker,
            runtime_run_id: attempt.runtime_run_id,
            disposition,
            recorded_at,
            replayed,
        };
        tx.commit()?;
        Ok(receipt)
    })
}
