//! Persistence for the Server-owned reconciliation of an accepted attempt.
//! This is cause/next-action evidence, not another execution state or a restart
//! authorization. The dispatcher keeps owning Pending until a definitive result.
use super::*;

pub(super) const SCHEMA: &str = r#"
-- An allocation must win Reserved -> Dispatching before crossing the Runtime
-- boundary. This proof is minted only by the competing Reserved -> Failed Tx.
-- Neither migration nor later terminal/cleanup updates may synthesize it.
CREATE TRIGGER backend_job_allocation_precluded_no_insert
BEFORE INSERT ON backend_job_attempts
WHEN NEW.allocation_precluded_at IS NOT NULL
BEGIN SELECT RAISE(ABORT,'allocation preclusion requires the Reserved transition'); END;
CREATE TRIGGER backend_job_allocation_precluded_transition
BEFORE UPDATE ON backend_job_attempts
WHEN NEW.allocation_precluded_at IS NOT OLD.allocation_precluded_at AND (
    OLD.allocation_precluded_at IS NOT NULL
    OR OLD.state != 'reserved' OR NEW.state != 'failed'
    OR OLD.runtime_id IS NOT NULL OR OLD.worker_id IS NOT NULL
    OR OLD.runtime_run_id IS NOT NULL OR OLD.dispatched_at IS NOT NULL
    OR NEW.runtime_id IS NOT NULL OR NEW.worker_id IS NOT NULL
    OR NEW.runtime_run_id IS NOT NULL OR NEW.dispatched_at IS NOT NULL
    OR NEW.allocation_precluded_at IS NOT NEW.completed_at
    OR NEW.failure_category IS NULL OR length(trim(NEW.failure_category))=0
)
BEGIN SELECT RAISE(ABORT,'immutable pre-allocation rejection proof'); END;
CREATE TRIGGER backend_job_allocation_precluded_no_rebind
BEFORE UPDATE ON backend_job_attempts
WHEN OLD.allocation_precluded_at IS NOT NULL AND (
    NEW.state != 'failed' OR NEW.runtime_id IS NOT NULL OR NEW.worker_id IS NOT NULL
    OR NEW.runtime_run_id IS NOT NULL OR NEW.dispatched_at IS NOT NULL
    OR NEW.completed_at IS NOT OLD.completed_at
)
BEGIN SELECT RAISE(ABORT,'allocation was conclusively precluded'); END;
CREATE TABLE backend_job_reconciliation (
    workspace_id TEXT NOT NULL,
    job_id TEXT NOT NULL,
    attempt_id TEXT NOT NULL,
    cause TEXT NOT NULL CHECK(length(cause)>0),
    detail TEXT NOT NULL,
    next_action TEXT NOT NULL CHECK(length(next_action)>0),
    updated_at TEXT NOT NULL,
    PRIMARY KEY(workspace_id,job_id,attempt_id),
    FOREIGN KEY(workspace_id,job_id,attempt_id) REFERENCES backend_job_attempts(workspace_id,job_id,attempt_id) ON DELETE CASCADE
);
CREATE TRIGGER backend_job_reconciliation_no_legacy
BEFORE INSERT ON backend_job_reconciliation
WHEN EXISTS(SELECT 1 FROM backend_job_upgrade_history h WHERE h.workspace_id=NEW.workspace_id AND h.job_id=NEW.job_id)
BEGIN SELECT RAISE(ABORT,'pre-upgrade Backend Job cannot reconcile'); END;
CREATE TABLE backend_job_execution_fences (
    workspace_id TEXT NOT NULL,
    job_id TEXT NOT NULL,
    attempt_id TEXT NOT NULL,
    owner_json TEXT NOT NULL CHECK(json_valid(owner_json)),
    history_json TEXT NOT NULL CHECK(json_valid(history_json)),
    controller_json TEXT NOT NULL CHECK(json_valid(controller_json)),
    host_json TEXT NOT NULL CHECK(json_valid(host_json)),
    recorded_at TEXT NOT NULL,
    PRIMARY KEY(workspace_id,job_id,attempt_id),
    FOREIGN KEY(workspace_id,job_id,attempt_id) REFERENCES backend_job_attempts(workspace_id,job_id,attempt_id) ON DELETE CASCADE
);
CREATE TRIGGER backend_job_execution_fence_immutable
BEFORE UPDATE ON backend_job_execution_fences
BEGIN SELECT RAISE(ABORT,'immutable exact execution fence'); END;
CREATE TABLE backend_job_recovery_probes (
    workspace_id TEXT NOT NULL,
    job_id TEXT NOT NULL,
    attempt_id TEXT NOT NULL,
    successful_writes INTEGER NOT NULL CHECK(successful_writes>0),
    PRIMARY KEY(workspace_id,job_id,attempt_id),
    FOREIGN KEY(workspace_id,job_id,attempt_id) REFERENCES backend_job_attempts(workspace_id,job_id,attempt_id) ON DELETE CASCADE
);
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendJobReconciliation {
    pub cause: String,
    pub detail: String,
    pub next_action: String,
    pub updated_at: String,
}

pub(super) fn require_current_contract(
    conn: &Connection,
    workspace: &str,
    job: &str,
) -> Result<()> {
    let legacy: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM backend_job_upgrade_history WHERE workspace_id=?1 AND job_id=?2)", params![workspace,job], |row|row.get(0))?;
    if legacy {
        return Err(Error::InvalidInput(
            "pre-upgrade Backend Job cannot recover, retry or continue".into(),
        ));
    }
    Ok(())
}

pub(super) fn history(
    conn: &Connection,
    workspace: &str,
    job: &str,
) -> Result<Vec<BackendJobAttemptRecord>> {
    let ids = conn.prepare("SELECT attempt_id FROM backend_job_attempts WHERE workspace_id=?1 AND job_id=?2 ORDER BY attempt,attempt_id")?
        .query_map(params![workspace,job], |row|row.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    ids.into_iter()
        .map(|id| {
            read_backend_job_attempt(conn, workspace, job, &id)?
                .ok_or_else(|| Error::Store("Backend Job history disappeared".into()))
        })
        .collect()
}

pub(super) fn require_history(
    conn: &Connection,
    workspace: &str,
    expected: &[BackendJobAttemptRecord],
    owner: &BackendJobAttemptRecord,
) -> Result<()> {
    if owner.workspace_id != workspace
        || !expected.contains(owner)
        || history(conn, workspace, &owner.job_id)? != expected
    {
        return Err(Error::InvalidInput(
            "Backend Job full attempt history or exact execution owner changed".into(),
        ));
    }
    Ok(())
}

pub(super) fn record(
    store: &SqliteWorkspaceStore,
    workspace: &str,
    expected: &BackendJobAttemptRecord,
    cause: &str,
    detail: &str,
    next_action: &str,
    now: &str,
) -> Result<BackendJobAttemptRecord> {
    if cause.trim().is_empty()
        || next_action.trim().is_empty()
        || cause.len() > 128
        || next_action.len() > 512
    {
        return Err(Error::InvalidInput(
            "reconciliation requires a bounded cause and next action".into(),
        ));
    }
    store.with_conn_mut(|conn| {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_current_contract(&tx,workspace,&expected.job_id)?;
        let job = read_backend_job(&tx,workspace,&expected.job_id)?.ok_or_else(||Error::InvalidInput("Backend Job missing".into()))?;
        let current = read_backend_job_attempt(&tx,workspace,&expected.job_id,&expected.attempt_id)?.ok_or_else(||Error::InvalidInput("Backend Job attempt missing".into()))?;
        if workspace != expected.workspace_id || current != *expected || job.current_attempt != expected.attempt || job.input_digest != expected.input_digest || job.state != BackendJobState::Pending || expected.state.terminal() {
            return Err(Error::InvalidInput("Backend Job reconciliation owner changed".into()));
        }
        subject_deletion_store::require_job_admission(&tx,workspace,&job.request)?;
        let detail = bounded_failure_detail(detail);
        tx.execute("INSERT INTO backend_job_reconciliation(workspace_id,job_id,attempt_id,cause,detail,next_action,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(workspace_id,job_id,attempt_id) DO UPDATE SET cause=excluded.cause,detail=excluded.detail,next_action=excluded.next_action,updated_at=excluded.updated_at", params![workspace,expected.job_id,expected.attempt_id,cause,detail,next_action,now])?;
        // Do not change the attempt's last confirmed admission/dispatch state.
        tx.execute("UPDATE backend_job_attempts SET failure_category=?4,failure_detail=?5,updated_at=?6 WHERE workspace_id=?1 AND job_id=?2 AND attempt_id=?3",params![workspace,expected.job_id,expected.attempt_id,cause,detail,now])?;
        tx.execute("UPDATE backend_jobs SET failure_category=?3,failure_detail=?4,updated_at=?5 WHERE workspace_id=?1 AND job_id=?2",params![workspace,expected.job_id,cause,detail,now])?;
        let result = read_backend_job_attempt(&tx,workspace,&expected.job_id,&expected.attempt_id)?.ok_or_else(||Error::Store("Backend Job reconciliation attempt disappeared".into()))?;
        tx.commit()?;
        Ok(result)
    })
}

pub(super) fn claim_cleanup_owned(
    store: &SqliteWorkspaceStore,
    workspace: &str,
    expected_history: &[BackendJobAttemptRecord],
    expected: &BackendJobAttemptRecord,
    now: &str,
) -> Result<(BackendJobAttemptRecord, bool)> {
    store.with_conn_mut(|conn| {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_current_contract(&tx,workspace,&expected.job_id)?;
        require_history(&tx,workspace,expected_history,expected)?;
        if expected.worker.is_none() || expected.runtime_run_id.is_none() {
            return Err(Error::InvalidInput("cleanup requires the exact allocated Worker/run owner".into()));
        }
        if matches!(expected.worker_cleanup_state,Some(BackendJobWorkerCleanupState::Executing | BackendJobWorkerCleanupState::Completed)) {
            tx.commit()?;
            return Ok((expected.clone(),false));
        }
        tx.execute("UPDATE backend_job_attempts SET worker_cleanup_state='executing',worker_cleanup_failure_category=NULL,worker_cleanup_failure_detail=NULL,worker_cleanup_updated_at=?4 WHERE workspace_id=?1 AND job_id=?2 AND attempt_id=?3",params![workspace,expected.job_id,expected.attempt_id,now])?;
        let result=read_backend_job_attempt(&tx,workspace,&expected.job_id,&expected.attempt_id)?.ok_or_else(||Error::Store("cleanup owner disappeared".into()))?;
        tx.commit()?;
        Ok((result,true))
    })
}

pub(super) fn finish_cleanup_owned(
    store: &SqliteWorkspaceStore,
    workspace: &str,
    expected_history: &[BackendJobAttemptRecord],
    expected: &BackendJobAttemptRecord,
    state: BackendJobWorkerCleanupState,
    cause: Option<&str>,
    detail: Option<&str>,
    now: &str,
) -> Result<BackendJobAttemptRecord> {
    if !matches!(
        state,
        BackendJobWorkerCleanupState::Completed | BackendJobWorkerCleanupState::Failed
    ) {
        return Err(Error::InvalidInput(
            "cleanup result must be definitive".into(),
        ));
    }
    store.with_conn_mut(|conn| {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_current_contract(&tx,workspace,&expected.job_id)?;
        require_history(&tx,workspace,expected_history,expected)?;
        if expected.worker.is_none() || expected.runtime_run_id.is_none() || expected.worker_cleanup_state != Some(BackendJobWorkerCleanupState::Executing) {
            return Err(Error::InvalidInput("cleanup requires an executing exact Worker/run owner".into()));
        }
        // Caller supplies evidence from the execution owner. A terminal Job or
        // missing live map alone must never call this completion boundary.
        tx.execute("UPDATE backend_job_attempts SET worker_cleanup_state=?4,worker_cleanup_failure_category=?5,worker_cleanup_failure_detail=?6,worker_cleanup_updated_at=?7,worker_cleanup_completed_at=?8 WHERE workspace_id=?1 AND job_id=?2 AND attempt_id=?3",params![workspace,expected.job_id,expected.attempt_id,state.as_str(),cause,detail.map(bounded_failure_detail),now,(state==BackendJobWorkerCleanupState::Completed).then_some(now)])?;
        let result=read_backend_job_attempt(&tx,workspace,&expected.job_id,&expected.attempt_id)?.ok_or_else(||Error::Store("cleanup owner disappeared".into()))?;
        tx.commit()?;
        Ok(result)
    })
}

/// Snapshot outside the write transaction, then compare every attempt under the
/// SQLite writer lock. No diagnosis/result is rewritten merely to test recovery.
pub(super) fn probe(
    store: &SqliteWorkspaceStore,
    workspace: &str,
    expected: &BackendJobAttemptRecord,
) -> Result<bool> {
    let observed = store.with_conn(|conn| history(conn, workspace, &expected.job_id))?;
    store.with_conn_mut(|conn| {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_current_contract(&tx, workspace, &expected.job_id)?;
        require_history(&tx, workspace, &observed, expected)?;
        require_retained_owner(&tx, workspace, expected)?;
        tx.execute("INSERT INTO backend_job_recovery_probes(workspace_id,job_id,attempt_id,successful_writes) VALUES(?1,?2,?3,1) ON CONFLICT(workspace_id,job_id,attempt_id) DO UPDATE SET successful_writes=successful_writes+1",params![workspace,expected.job_id,expected.attempt_id])?;
        tx.commit()?;
        Ok(true)
    })
}

fn require_retained_owner(
    conn: &Connection,
    workspace: &str,
    expected: &BackendJobAttemptRecord,
) -> Result<BackendJobRecord> {
    let job = read_backend_job(conn, workspace, &expected.job_id)?
        .ok_or_else(|| Error::InvalidInput("Backend Job missing".into()))?;
    let attempt =
        read_backend_job_attempt(conn, workspace, &expected.job_id, &expected.attempt_id)?;
    if workspace != expected.workspace_id
        || attempt.as_ref() != Some(expected)
        || job.input_digest != expected.input_digest
    {
        return Err(Error::RepositoryConflict(
            "Backend Job exact retained owner changed".into(),
        ));
    }
    Ok(job)
}

fn require_current_owner(
    conn: &Connection,
    workspace: &str,
    expected: &BackendJobAttemptRecord,
) -> Result<BackendJobRecord> {
    let job = require_retained_owner(conn, workspace, expected)?;
    if job.current_attempt != expected.attempt {
        return Err(Error::RepositoryConflict(
            "Backend Job current owner changed".into(),
        ));
    }
    job.load_executable_request()?;
    Ok(job)
}

fn validate_fence(
    expected: &BackendJobAttemptRecord,
    controller: &str,
    host: &str,
) -> Result<crate::backend_job_host_effects::JobHostOwner> {
    use crate::backend_job_host_effects::JobHostOwner;
    use worker_runtime::execution::{WorkerExecutionTermination, WorkerExecutionTerminationScope};
    if controller.len() > 65536 || host.len() > 8192 {
        return Err(Error::InvalidInput(
            "execution evidence exceeds bound".into(),
        ));
    }
    let owner = JobHostOwner::try_from(expected)?;
    let host: JobHostOwner = serde_json::from_str(host)
        .map_err(|_| Error::InvalidInput("typed Host drain evidence required".into()))?;
    let ended: WorkerExecutionTermination = serde_json::from_str(controller).map_err(|_| {
        Error::InvalidInput("typed Controller termination evidence required".into())
    })?;
    if host != owner
        || ended.scope != WorkerExecutionTerminationScope::Controller
        || !ended.feature_failures.is_empty()
        || ended.execution_id.is_nil()
        || ended.shutdown_command_id.is_none()
        || ended.worker_ref.worker_id.to_string() != owner.worker.worker_id
        || !ended
            .submissions
            .iter()
            .any(|submission| submission.submission_request_id == owner.runtime_run_id)
    {
        return Err(Error::InvalidInput("Controller/Feature/background and Host drain must confirm the exact owner independently".into()));
    }
    Ok(owner)
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct ExecutionBinding {
    attempt_id: String,
    attempt: u8,
    input_digest: String,
    worker: Option<RuntimeWorkerRef>,
    runtime_run_id: Option<String>,
}

impl From<&BackendJobAttemptRecord> for ExecutionBinding {
    fn from(value: &BackendJobAttemptRecord) -> Self {
        Self {
            attempt_id: value.attempt_id.clone(),
            attempt: value.attempt,
            input_digest: value.input_digest.clone(),
            worker: value.worker.clone(),
            runtime_run_id: value.runtime_run_id.clone(),
        }
    }
}

pub(super) fn record_fence(
    store: &SqliteWorkspaceStore,
    workspace: &str,
    expected: &BackendJobAttemptRecord,
    controller: &str,
    host: &str,
    now: &str,
) -> Result<BackendJobAttemptRecord> {
    let owner = validate_fence(expected, controller, host)?;
    let observed = store.with_conn(|conn| history(conn, workspace, &expected.job_id))?;
    store.with_conn_mut(|conn| {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_current_contract(&tx, workspace, &expected.job_id)?;
        require_history(&tx, workspace, &observed, expected)?;
        require_retained_owner(&tx, workspace, expected)?;
        // Strings are transport, not truthy markers. Only the Host's private
        // drain constructor may supply host evidence; this checks its binding.
        let saved: Option<String> = tx.query_row("SELECT owner_json FROM backend_job_execution_fences WHERE workspace_id=?1 AND job_id=?2 AND attempt_id=?3",params![workspace,expected.job_id,expected.attempt_id],|row|row.get(0)).optional()?;
        let owner_json = serde_json::to_string(&owner).map_err(|error|Error::Store(error.to_string()))?;
        if let Some(saved) = saved {
            if saved != owner_json { return Err(Error::RepositoryConflict("execution fence owner changed".into())); }
        } else {
            // Persist bindings, not a second copy of domain results/causes. The
            // full observed rows were compared under the same writer lock above.
            let bindings: Vec<_> = observed.iter().map(ExecutionBinding::from).collect();
            let history_json = serde_json::to_string(&bindings).map_err(|error|Error::Store(error.to_string()))?;
            tx.execute("INSERT INTO backend_job_execution_fences(workspace_id,job_id,attempt_id,owner_json,history_json,controller_json,host_json,recorded_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![workspace,expected.job_id,expected.attempt_id,owner_json,history_json,controller,host,now])?;
        }
        tx.commit()?;
        Ok(expected.clone())
    })
}

pub(super) fn has_fence(
    conn: &Connection,
    workspace: &str,
    expected: &BackendJobAttemptRecord,
) -> Result<bool> {
    require_current_contract(conn, workspace, &expected.job_id)?;
    let current =
        read_backend_job_attempt(conn, workspace, &expected.job_id, &expected.attempt_id)?;
    if workspace != expected.workspace_id || current.as_ref() != Some(expected) {
        return Ok(false);
    }
    // This is a different fact from an execution termination receipt. The same
    // SQLite transaction conclusively rejected admission before allocation; the
    // schema guards forbid later rebinding or backfilling it from a terminal row.
    if expected.allocation_precluded_at.is_some() {
        if expected.state != BackendJobAttemptState::Failed
            || expected.worker.is_some()
            || expected.runtime_run_id.is_some()
            || expected.dispatched_at.is_some()
            || expected.allocation_precluded_at != expected.completed_at
        {
            return Err(Error::Store(
                "invalid pre-allocation rejection proof".into(),
            ));
        }
        return Ok(true);
    }
    let saved: Option<(String,String,String)> = conn.query_row("SELECT controller_json,host_json,history_json FROM backend_job_execution_fences WHERE workspace_id=?1 AND job_id=?2 AND attempt_id=?3",params![workspace,expected.job_id,expected.attempt_id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
    let Some((controller, host, bindings)) = saved else {
        return Ok(false);
    };
    let bindings: Vec<ExecutionBinding> = serde_json::from_str(&bindings)
        .map_err(|_| Error::Store("invalid execution fence history".into()))?;
    let current = history(conn, workspace, &expected.job_id)?;
    // Later owners do not inherit this fence. Earlier binding changes invalidate
    // its full-history evidence, while ordinary result/cleanup progress does not.
    if bindings.is_empty()
        || !bindings.iter().all(|binding| {
            current
                .iter()
                .any(|attempt| ExecutionBinding::from(attempt) == *binding)
        })
    {
        return Ok(false);
    }
    validate_fence(expected, &controller, &host).map(|_| true)
}

pub(super) fn reconcile_domain_result(
    store: &SqliteWorkspaceStore,
    workspace: &str,
    expected: &BackendJobAttemptRecord,
    submission: &BackendJobResultSubmission,
    now: &str,
) -> Result<BackendJobAttemptRecord> {
    let observed = store.with_conn(|conn| history(conn, workspace, &expected.job_id))?;
    store.with_conn_mut(|conn| {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_current_contract(&tx, workspace, &expected.job_id)?;
        require_history(&tx, workspace, &observed, expected)?;
        let job = require_current_owner(&tx, workspace, expected)?;
        subject_deletion_store::require_job_admission(&tx,workspace,&job.request)?;
        if job.request.grants.subjektiv_journal.is_some() || job.state != BackendJobState::Pending
            || expected.state.terminal() || expected.worker.is_none() || expected.runtime_run_id.is_none()
            || submission.job_id != expected.job_id || submission.attempt_id != expected.attempt_id
            || submission.input_digest != expected.input_digest {
            return Err(Error::InvalidInput("saved non-Journal domain result requires a nonterminal exact current execution owner".into()));
        }
        let (digest,json) = result_digest(&submission.result,job.request.limits.max_result_bytes)?;
        tx.execute("UPDATE backend_job_attempts SET state='completed',result_json=?4,result_digest=?5,failure_category=NULL,failure_detail=NULL,worker_cleanup_state=COALESCE(worker_cleanup_state,'pending'),worker_cleanup_updated_at=CASE WHEN worker_cleanup_state IS NULL THEN ?6 ELSE worker_cleanup_updated_at END,updated_at=?6,completed_at=?6 WHERE workspace_id=?1 AND job_id=?2 AND attempt_id=?3",params![workspace,expected.job_id,expected.attempt_id,json,digest,now])?;
        tx.execute("UPDATE backend_jobs SET state='completed',result_json=?3,result_digest=?4,failure_category=NULL,failure_detail=NULL,updated_at=?5,completed_at=?5 WHERE workspace_id=?1 AND job_id=?2",params![workspace,expected.job_id,json,digest,now])?;
        notification_routing::jobs::capture(&tx,workspace,&expected.job_id,&expected.attempt_id)?;
        let result = read_backend_job_attempt(&tx,workspace,&expected.job_id,&expected.attempt_id)?.ok_or_else(||Error::Store("reconciled owner disappeared".into()))?;
        tx.commit()?;
        Ok(result)
    })
}

#[cfg(test)]
mod tests {
    //! Real Store/SQLite component tests. Only Runtime allocation/termination is
    //! outside this boundary: saved owner bindings are fixture rows, not proof
    //! that a process, Controller, background action or Host operation has ended.
    use super::*;

    const NOW: &str = "2026-11-04T00:00:00Z";

    fn fixture() -> (
        tempfile::TempDir,
        SqliteWorkspaceStore,
        BackendJobReservation,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let store =
            SqliteWorkspaceStore::open_for_test(&dir.path().join("recovery.sqlite")).unwrap();
        store.with_conn(|conn| {
            conn.execute_batch("INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at) VALUES('owner','user','owner','Owner','1','1'); INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at) VALUES('workspace','owner','Workspace','active','1','1');")?;
            conn.execute("INSERT INTO workspace_memory_settings(workspace_id,language,created_at,updated_at) VALUES('workspace','en',?1,?1)",params![NOW])?;
            Ok(())
        }).unwrap();
        let request = BackendJobRequest {
            job_id: "accepted".into(),
            purpose: "recovery-test".into(),
            input_ref: "saved-input".into(),
            input: serde_json::json!({"body":"frozen"}),
            instruction: "Use saved input".into(),
            profile: "builtin:backend-job".into(),
            grants: Default::default(),
            serialization_key: None,
            source_worker: None,
            notification_target: None,
            limits: crate::backend_job::BackendJobLimits {
                max_attempts: 3,
                ..Default::default()
            },
        };
        let reserved = store
            .reserve_backend_job("workspace", &request, NOW)
            .unwrap();
        (dir, store, reserved)
    }

    #[test]
    fn preallocation_rejection_commits_with_failure_and_cannot_be_backfilled_or_rebound() {
        let (_dir, store, reserved) = fixture();
        let id = &reserved.attempt.attempt_id;
        store.with_conn(|conn| {
            conn.execute_batch("CREATE TRIGGER refuse_failure BEFORE UPDATE ON backend_jobs WHEN NEW.state='failed' BEGIN SELECT RAISE(ABORT,'injected failure commit'); END;")?;
            Ok(())
        }).unwrap();
        assert!(
            store
                .finish_backend_job_attempt(
                    "workspace",
                    "accepted",
                    id,
                    BackendJobAttemptState::Failed,
                    "cancelled_before_dispatch",
                    "No execution admitted",
                    NOW
                )
                .is_err()
        );
        let rolled_back = store
            .get_backend_job_attempt("workspace", "accepted", id)
            .unwrap()
            .unwrap();
        assert_eq!(rolled_back, reserved.attempt);
        assert!(
            !store
                .has_backend_job_execution_fence("workspace", &rolled_back)
                .unwrap()
        );
        store
            .with_conn(|conn| {
                conn.execute_batch("DROP TRIGGER refuse_failure")?;
                Ok(())
            })
            .unwrap();
        let failed = store
            .finish_backend_job_attempt(
                "workspace",
                "accepted",
                id,
                BackendJobAttemptState::Failed,
                "cancelled_before_dispatch",
                "No execution admitted",
                NOW,
            )
            .unwrap();
        assert_eq!(failed.allocation_precluded_at.as_deref(), Some(NOW));
        assert!(
            store
                .has_backend_job_execution_fence("workspace", &failed)
                .unwrap()
        );
        let (replay, acquired) = store
            .claim_backend_job_attempt_dispatch("workspace", "accepted", id, NOW)
            .unwrap();
        assert!(!acquired);
        assert_eq!(replay.attempt, failed);
        store.with_conn(|conn| {
            assert!(conn.execute("UPDATE backend_job_attempts SET state='dispatching',worker_id='foreign',runtime_id='foreign' WHERE attempt_id=?1", [id]).is_err());
            assert!(conn.execute("UPDATE backend_job_attempts SET allocation_precluded_at=NULL WHERE attempt_id=?1", [id]).is_err());
            Ok(())
        }).unwrap();
        let retry = store
            .reserve_backend_job_retry("workspace", "accepted", NOW)
            .unwrap();
        assert!(retry.attempt.allocation_precluded_at.is_none());
        assert!(
            !store
                .has_backend_job_execution_fence("workspace", &retry.attempt)
                .unwrap()
        );
        assert!(
            store
                .has_backend_job_execution_fence("workspace", &failed)
                .unwrap()
        );

        // Dispatch claimed allocation admission, even though no Worker is bound yet.
        let (claimed, acquired) = store
            .claim_backend_job_attempt_dispatch(
                "workspace",
                "accepted",
                &retry.attempt.attempt_id,
                NOW,
            )
            .unwrap();
        assert!(acquired);
        let uncertain = store
            .finish_backend_job_attempt(
                "workspace",
                "accepted",
                &claimed.attempt.attempt_id,
                BackendJobAttemptState::Failed,
                "allocation_response_lost",
                "Must reconcile the original allocation",
                NOW,
            )
            .unwrap();
        assert!(uncertain.worker.is_none());
        assert!(uncertain.allocation_precluded_at.is_none());
        assert!(
            !store
                .has_backend_job_execution_fence("workspace", &uncertain)
                .unwrap()
        );
        store.with_conn(|conn| {
            assert!(conn.execute("UPDATE backend_job_attempts SET allocation_precluded_at=completed_at WHERE attempt_id=?1", [&uncertain.attempt_id]).is_err());
            Ok(())
        }).unwrap();
    }

    #[test]
    fn concurrent_dispatch_claim_and_preallocation_rejection_serialize_on_the_same_attempt() {
        let (dir, store, reserved) = fixture();
        let other =
            SqliteWorkspaceStore::open_for_test(&dir.path().join("recovery.sqlite")).unwrap();
        let start = std::sync::Barrier::new(2);
        let id = &reserved.attempt.attempt_id;
        let (claim, rejected) = std::thread::scope(|scope| {
            let dispatch = scope.spawn(|| {
                start.wait();
                store
                    .claim_backend_job_attempt_dispatch("workspace", "accepted", id, NOW)
                    .unwrap()
            });
            let reject = scope.spawn(|| {
                start.wait();
                other
                    .finish_backend_job_attempt(
                        "workspace",
                        "accepted",
                        id,
                        BackendJobAttemptState::Failed,
                        "cancelled",
                        "Concurrent admission rejection",
                        NOW,
                    )
                    .unwrap()
            });
            (dispatch.join().unwrap(), reject.join().unwrap())
        });
        let saved = store
            .get_backend_job_attempt("workspace", "accepted", id)
            .unwrap()
            .unwrap();
        assert_eq!(saved, rejected);
        assert_eq!(saved.state, BackendJobAttemptState::Failed);
        assert_eq!(saved.allocation_precluded_at.is_some(), !claim.1);
        assert_eq!(
            store
                .has_backend_job_execution_fence("workspace", &saved)
                .unwrap(),
            !claim.1
        );
        if claim.1 {
            assert_eq!(claim.0.attempt.state, BackendJobAttemptState::Dispatching);
        } else {
            assert_eq!(claim.0.attempt, rejected);
        }
    }

    #[test]
    fn allocation_lookup_is_read_only_matches_create_fingerprint_and_never_resurrects_removed_worker()
     {
        let (_dir, store, reserved) = fixture();
        let settings = WorkspaceMemorySettingsRecord {
            workspace_id: "workspace".into(),
            language: "en".into(),
            created_at: NOW.into(),
            updated_at: NOW.into(),
        };
        let key = crate::backend_job::allocation_key(&reserved.attempt.attempt_id);
        assert!(
            store
                .lookup_backend_job_allocation("workspace", &reserved.job, &reserved.attempt)
                .unwrap()
                .is_none()
        );
        let created = store
            .reserve_worker_create(
                "workspace",
                "runtime",
                &key,
                "sha256:create-input",
                None,
                &settings,
            )
            .unwrap();
        let found = store
            .lookup_backend_job_allocation("workspace", &reserved.job, &reserved.attempt)
            .unwrap()
            .unwrap();
        assert_eq!(found, (created.clone(), "runtime".to_string()));
        store.with_conn(|conn| {
            conn.execute("UPDATE worker_create_reservations SET create_fingerprint='corrupt' WHERE allocation_key=?1", params![key])?;
            Ok(())
        }).unwrap();
        assert!(
            store
                .lookup_backend_job_allocation("workspace", &reserved.job, &reserved.attempt)
                .is_err()
        );
        store.with_conn(|conn| {
            conn.execute("UPDATE worker_create_reservations SET create_fingerprint=?2 WHERE allocation_key=?1", params![key,created.create_fingerprint])?;
            Ok(())
        }).unwrap();
        let mut wrong = reserved.job.clone();
        wrong.input_digest = "different-input".into();
        assert!(
            store
                .lookup_backend_job_allocation("workspace", &wrong, &reserved.attempt)
                .is_err()
        );
        store
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE worker_create_reservations SET state='removed' WHERE allocation_key=?1",
                    params![key],
                )?;
                Ok(())
            })
            .unwrap();
        assert!(
            store
                .lookup_backend_job_allocation("workspace", &reserved.job, &reserved.attempt)
                .is_err()
        );
        store
            .with_conn(|conn| {
                assert_eq!(
                    conn.query_row(
                        "SELECT count(*) FROM worker_create_reservations",
                        [],
                        |row| row.get::<_, i64>(0)
                    )?,
                    1
                );
                let state: String = conn.query_row(
                    "SELECT state FROM worker_create_reservations WHERE allocation_key=?1",
                    params![key],
                    |row| row.get(0),
                )?;
                assert_eq!(state, "removed");
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn terminal_executing_cleanup_probe_commits_without_changing_failure_and_rejects_stale_owner() {
        let (dir, store, reserved) = fixture();
        let owner = bind_fixture_owner(&store, &reserved);
        let failed = store
            .finish_backend_job_attempt(
                "workspace",
                "accepted",
                &owner.attempt_id,
                BackendJobAttemptState::Failed,
                "execution_failed",
                "original cause",
                NOW,
            )
            .unwrap();
        let history = store
            .list_backend_job_attempts("workspace", "accepted")
            .unwrap();
        let (executing, claimed) = store
            .claim_backend_job_worker_cleanup_owned("workspace", &history, &failed, NOW)
            .unwrap();
        assert!(claimed);
        let job = store
            .get_backend_job("workspace", "accepted")
            .unwrap()
            .unwrap();
        assert!(
            store
                .probe_backend_job_recovery("workspace", &failed)
                .is_err()
        );
        store.with_conn(|conn| {
            conn.execute_batch("CREATE TEMP TRIGGER probe_fault BEFORE INSERT ON main.backend_job_recovery_probes BEGIN SELECT RAISE(ABORT,'same SQLite fault'); END;")?;
            Ok(())
        }).unwrap();
        assert!(
            store
                .probe_backend_job_recovery("workspace", &executing)
                .is_err()
        );
        store
            .with_conn(|conn| {
                conn.execute_batch("DROP TRIGGER probe_fault")?;
                Ok(())
            })
            .unwrap();
        assert!(
            store
                .probe_backend_job_recovery("workspace", &executing)
                .unwrap()
        );
        assert!(
            store
                .probe_backend_job_recovery("workspace", &executing)
                .unwrap()
        );
        assert_eq!(
            store
                .get_backend_job("workspace", "accepted")
                .unwrap()
                .unwrap(),
            job
        );
        assert_eq!(
            store
                .get_backend_job_attempt("workspace", "accepted", &executing.attempt_id)
                .unwrap()
                .unwrap(),
            executing
        );
        drop(store);
        let store =
            SqliteWorkspaceStore::open_for_test(&dir.path().join("recovery.sqlite")).unwrap();
        store
            .with_conn(|conn| {
                let writes: i64 = conn.query_row(
                    "SELECT successful_writes FROM backend_job_recovery_probes",
                    [],
                    |row| row.get(0),
                )?;
                assert_eq!(writes, 2);
                Ok(())
            })
            .unwrap();
        assert!(
            !store
                .has_backend_job_execution_fence("workspace", &executing)
                .unwrap()
        );
    }

    #[test]
    fn historical_probe_and_fence_do_not_reset_cleanup_or_release_a_successor() {
        let (_dir, store, reserved) = fixture();
        let bound = bind_fixture_owner(&store, &reserved);
        store.with_conn(|conn| {
            conn.execute("UPDATE backend_job_attempts SET worker_id='019a4ea8-0000-7000-8000-000000000003' WHERE attempt_id=?1",params![bound.attempt_id])?;
            Ok(())
        }).unwrap();
        let failed = store
            .finish_backend_job_attempt(
                "workspace",
                "accepted",
                &bound.attempt_id,
                BackendJobAttemptState::Failed,
                "execution_failed",
                "saved original failure",
                NOW,
            )
            .unwrap();
        let history = store
            .list_backend_job_attempts("workspace", "accepted")
            .unwrap();
        let (executing, acquired) = store
            .claim_backend_job_worker_cleanup_owned("workspace", &history, &failed, NOW)
            .unwrap();
        assert!(acquired);
        assert_eq!(
            store
                .list_backend_job_worker_cleanups("workspace", None, 100)
                .unwrap(),
            vec![executing.clone()]
        );
        assert_eq!(
            store
                .get_backend_job_attempt("workspace", "accepted", &executing.attempt_id)
                .unwrap()
                .unwrap(),
            executing
        );
        assert!(
            store
                .reserve_backend_job_retry("workspace", "accepted", NOW)
                .is_err()
        );
        // The fixture substitutes only the external cleanup receipt; this does
        // not claim actual Controller/background/Host/process termination.
        let history = store
            .list_backend_job_attempts("workspace", "accepted")
            .unwrap();
        let historical = store
            .finish_backend_job_worker_cleanup_owned(
                "workspace",
                &history,
                &executing,
                BackendJobWorkerCleanupState::Completed,
                None,
                None,
                NOW,
            )
            .unwrap();
        let next = store
            .reserve_backend_job_retry("workspace", "accepted", NOW)
            .unwrap();
        let successor = bind_fixture_owner(&store, &next);
        let next_job = store
            .get_backend_job("workspace", "accepted")
            .unwrap()
            .unwrap();
        assert!(
            store
                .probe_backend_job_recovery("workspace", &historical)
                .unwrap()
        );
        let (controller, host) = evidence(&historical);
        store
            .record_backend_job_execution_fence("workspace", &historical, &controller, &host, NOW)
            .unwrap();
        assert!(
            store
                .has_backend_job_execution_fence("workspace", &historical)
                .unwrap()
        );
        assert!(
            !store
                .has_backend_job_execution_fence("workspace", &successor)
                .unwrap()
        );
        assert_eq!(
            store
                .get_backend_job_attempt("workspace", "accepted", &successor.attempt_id)
                .unwrap()
                .unwrap(),
            successor
        );
        assert_eq!(
            store
                .get_backend_job("workspace", "accepted")
                .unwrap()
                .unwrap(),
            next_job
        );
        let mut foreign = historical.clone();
        foreign.worker = successor.worker.clone();
        assert!(
            store
                .probe_backend_job_recovery("workspace", &foreign)
                .is_err()
        );
        assert!(
            store
                .record_backend_job_execution_fence("workspace", &foreign, &controller, &host, NOW)
                .is_err()
        );
    }

    #[test]
    fn completed_journal_with_available_input_is_rediscovered_without_claiming_recording_complete()
    {
        let (_dir, store, reserved) = fixture();
        let owner = bind_fixture_owner(&store, &reserved);
        // Selection boundary only; SQL cleanup/result rows are not external
        // execution, Journal checkpoint, or Purge receipts.
        store.with_conn(|conn| {
            conn.execute("UPDATE backend_job_attempts SET state='completed',result_json='{}',result_digest='fixture-result',worker_cleanup_state='completed' WHERE attempt_id=?1", params![owner.attempt_id])?;
            conn.execute("UPDATE backend_jobs SET state='completed',result_json='{}',result_digest='fixture-result' WHERE job_id='accepted'", [])?;
            Ok(())
        }).unwrap();
        assert!(
            store
                .list_backend_job_worker_cleanups("workspace", None, 100)
                .unwrap()
                .is_empty()
        );
        let mut request = reserved.job.request.clone();
        request.job_id = "journal-retirement".into();
        request.serialization_key = Some(crate::backend_job::journal_serialization_key("journal"));
        request.grants.subjektiv_journal = Some(crate::backend_job::SubjektivJournalGrant {
            binding_id: "binding".into(), subject_id: "subject".into(), session_id: "session".into(), journal_id: "journal".into(),
            selected_ranges: vec![serde_json::from_value(serde_json::json!({
                "source":{"session_id":"session", "pointer":{"segment_id":"019a4ea8-0000-7000-8000-000000000001", "entry_ref":"Eentry"},
                    "order":0,"content_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "total_bytes":1},
                "bytes":{"start":0,"end":1}
            })).unwrap()],
        });
        let journal = store
            .reserve_backend_job("workspace", &request, NOW)
            .unwrap();
        store.with_conn(|conn| {
            conn.execute("UPDATE backend_job_attempts SET state='completed',runtime_id='runtime',worker_id='journal-worker',runtime_run_id='run',result_json='{}',result_digest='fixture-result',worker_cleanup_state='completed' WHERE attempt_id=?1", params![journal.attempt.attempt_id])?;
            conn.execute("UPDATE backend_jobs SET state='completed',result_json='{}',result_digest='fixture-result' WHERE job_id=?1", params![request.job_id])?;
            Ok(())
        }).unwrap();
        let selected = store
            .list_backend_job_worker_cleanups("workspace", None, 100)
            .unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].attempt_id, journal.attempt.attempt_id);
        assert_eq!(
            selected[0].worker_cleanup_state,
            Some(BackendJobWorkerCleanupState::Completed)
        );
        assert!(
            !store
                .has_backend_job_execution_fence("workspace", &selected[0])
                .unwrap()
        );
        store
            .reclaim_backend_job_input(
                "workspace",
                &request.job_id,
                &journal.job.input_digest,
                crate::backend_job::JobInputRetirementReason::ExplicitErasure,
            )
            .unwrap();
        assert!(
            store
                .list_backend_job_worker_cleanups("workspace", None, 100)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn saved_nonjournal_domain_completion_full_cas_preserves_independent_cleanup() {
        let (_dir, store, reserved) = fixture();
        let owner = bind_fixture_owner(&store, &reserved);
        let submission = BackendJobResultSubmission {
            job_id: owner.job_id.clone(),
            attempt_id: owner.attempt_id.clone(),
            input_digest: owner.input_digest.clone(),
            result: serde_json::json!({"domain":"saved-disposition"}),
        };
        let mut wrong = submission.clone();
        wrong.input_digest = "different-input".into();
        assert!(
            store
                .reconcile_backend_job_domain_result("workspace", &owner, &wrong, NOW)
                .is_err()
        );
        let mut oversized = submission.clone();
        oversized.result = serde_json::Value::String("x".repeat(1024 * 1024));
        assert!(
            store
                .reconcile_backend_job_domain_result("workspace", &owner, &oversized, NOW)
                .is_err()
        );
        let done = store
            .reconcile_backend_job_domain_result("workspace", &owner, &submission, NOW)
            .unwrap();
        assert_eq!(done.state, BackendJobAttemptState::Completed);
        assert_eq!(done.result, Some(submission.result.clone()));
        assert_eq!(
            done.worker_cleanup_state,
            Some(
                owner
                    .worker_cleanup_state
                    .unwrap_or(BackendJobWorkerCleanupState::Pending)
            )
        );
        assert_eq!(
            done.worker_cleanup_completed_at,
            owner.worker_cleanup_completed_at
        );
        assert!(
            !store
                .has_backend_job_execution_fence("workspace", &done)
                .unwrap()
        );
        assert!(
            store
                .reconcile_backend_job_domain_result("workspace", &owner, &submission, NOW)
                .is_err()
        );
        assert!(
            store
                .reconcile_backend_job_domain_result("workspace", &done, &submission, NOW)
                .is_err()
        );
    }

    fn evidence(owner: &BackendJobAttemptRecord) -> (String, String) {
        let host = crate::backend_job_host_effects::JobHostOwner::try_from(owner).unwrap();
        let controller = serde_json::json!({
            "scope":"controller", "worker_ref":{"worker_id":owner.worker.as_ref().unwrap().worker_id},
            "stop_operation_id":"019a4ea8-0000-7000-8000-000000000001",
            "execution_id":"019a4ea8-0000-7000-8000-000000000002",
            "shutdown_command_id":1,"submissions":[{
                "submission_request_id":owner.runtime_run_id,
                "submission_id":"accepted-submission",
                "disposition":"started"
            }],"feature_failures":[]
        });
        (
            controller.to_string(),
            serde_json::to_string(&host).unwrap(),
        )
    }

    #[test]
    fn fence_requires_typed_independent_exact_owner_evidence_and_survives_only_its_binding() {
        let (dir, store, reserved) = fixture();
        let owner = bind_fixture_owner(&store, &reserved);
        store.with_conn(|conn| {conn.execute("UPDATE backend_job_attempts SET worker_id='019a4ea8-0000-7000-8000-000000000003' WHERE attempt_id=?1",params![owner.attempt_id])?;Ok(())}).unwrap();
        let owner = store
            .get_backend_job_attempt("workspace", "accepted", &owner.attempt_id)
            .unwrap()
            .unwrap();
        let (controller, host) = evidence(&owner);
        assert!(
            store
                .record_backend_job_execution_fence("workspace", &owner, "true", "true", NOW)
                .is_err()
        );
        let wrong = host.replace(
            "\"runtime_run_id\":\"run\"",
            "\"runtime_run_id\":\"successor-run\"",
        );
        assert_ne!(wrong, host);
        assert!(
            store
                .record_backend_job_execution_fence("workspace", &owner, &controller, &wrong, NOW)
                .is_err()
        );
        let mut wrong_run: serde_json::Value = serde_json::from_str(&controller).unwrap();
        wrong_run["submissions"][0]["submission_request_id"] = "successor-run".into();
        assert!(
            store
                .record_backend_job_execution_fence(
                    "workspace",
                    &owner,
                    &wrong_run.to_string(),
                    &host,
                    NOW
                )
                .is_err()
        );
        wrong_run["submissions"] = serde_json::json!([]);
        assert!(
            store
                .record_backend_job_execution_fence(
                    "workspace",
                    &owner,
                    &wrong_run.to_string(),
                    &host,
                    NOW
                )
                .is_err()
        );
        let bad = controller.replace("controller", "unpublished_startup_candidate");
        assert!(
            store
                .record_backend_job_execution_fence("workspace", &owner, &bad, &host, NOW)
                .is_err()
        );
        store
            .record_backend_job_execution_fence("workspace", &owner, &controller, &host, NOW)
            .unwrap();
        assert!(
            store
                .has_backend_job_execution_fence("workspace", &owner)
                .unwrap()
        );
        // A fence is not domain completion or canonical removal.
        assert_eq!(
            store
                .get_backend_job("workspace", "accepted")
                .unwrap()
                .unwrap()
                .state,
            BackendJobState::Pending
        );
        assert_eq!(
            store
                .get_backend_job_attempt("workspace", "accepted", &owner.attempt_id)
                .unwrap()
                .unwrap(),
            owner
        );
        drop(store);
        let store =
            SqliteWorkspaceStore::open_for_test(&dir.path().join("recovery.sqlite")).unwrap();
        assert!(
            store
                .has_backend_job_execution_fence("workspace", &owner)
                .unwrap()
        );
        store.with_conn(|conn| {conn.execute("UPDATE backend_job_attempts SET runtime_run_id='successor-run' WHERE attempt_id=?1",params![owner.attempt_id])?;Ok(())}).unwrap();
        let successor = store
            .get_backend_job_attempt("workspace", "accepted", &owner.attempt_id)
            .unwrap()
            .unwrap();
        assert!(
            !store
                .has_backend_job_execution_fence("workspace", &owner)
                .unwrap()
        );
        assert!(
            !store
                .has_backend_job_execution_fence("workspace", &successor)
                .unwrap()
        );
        assert!(
            store
                .record_backend_job_execution_fence("workspace", &owner, &controller, &host, NOW)
                .is_err()
        );
    }

    #[test]
    fn reconciliation_keeps_the_admission_fact_and_saved_next_action_across_reopen() {
        let (dir, store, reserved) = fixture();
        let (claimed, was_claimed) = store
            .claim_backend_job_attempt_dispatch(
                "workspace",
                "accepted",
                &reserved.attempt.attempt_id,
                NOW,
            )
            .unwrap();
        assert!(was_claimed);
        let pending = store
            .record_backend_job_reconciliation(
                "workspace",
                &claimed.attempt,
                "receipt_read_failed",
                "temporary Runtime lookup failure",
                "read_same_allocation_receipt",
                NOW,
            )
            .unwrap();
        assert_eq!(pending.state, BackendJobAttemptState::Dispatching);
        assert!(
            store
                .reserve_backend_job_retry("workspace", "accepted", NOW)
                .is_err()
        );
        // Stale diagnosis cannot overwrite a later saved owner observation.
        assert!(
            store
                .record_backend_job_reconciliation(
                    "workspace",
                    &claimed.attempt,
                    "late",
                    "stale lookup",
                    "do_not_reallocate",
                    NOW
                )
                .is_err()
        );
        drop(store);
        let store =
            SqliteWorkspaceStore::open_for_test(&dir.path().join("recovery.sqlite")).unwrap();
        let recovery = store
            .list_recoverable_backend_job_attempts("workspace", 100)
            .unwrap();
        assert_eq!(recovery, vec![pending]);
        assert_eq!(
            store
                .get_backend_job("workspace", "accepted")
                .unwrap()
                .unwrap()
                .state,
            BackendJobState::Pending
        );
        let diagnosis = store
            .get_backend_job_reconciliation("workspace", "accepted", &reserved.attempt.attempt_id)
            .unwrap()
            .unwrap();
        assert_eq!(diagnosis.cause, "receipt_read_failed");
        assert_eq!(diagnosis.next_action, "read_same_allocation_receipt");
        // Restart did not clear the dispatch claim for blind retransmission.
        assert!(
            !store
                .claim_backend_job_attempt_dispatch(
                    "workspace",
                    "accepted",
                    &reserved.attempt.attempt_id,
                    NOW
                )
                .unwrap()
                .1
        );
    }

    fn bind_fixture_owner(
        store: &SqliteWorkspaceStore,
        reservation: &BackendJobReservation,
    ) -> BackendJobAttemptRecord {
        store.with_conn(|conn| {
            conn.execute("UPDATE backend_job_attempts SET state='dispatched',runtime_id='runtime',worker_id='worker',runtime_run_id='run',dispatched_at=?2 WHERE attempt_id=?1",params![reservation.attempt.attempt_id,NOW])?;
            Ok(())
        }).unwrap();
        store
            .get_backend_job_attempt("workspace", "accepted", &reservation.attempt.attempt_id)
            .unwrap()
            .unwrap()
    }

    #[test]
    fn cleanup_cas_rejects_changed_history_and_wrong_worker_run_without_releasing_owner() {
        let (_dir, store, reserved) = fixture();
        let owner = bind_fixture_owner(&store, &reserved);
        let history = store
            .list_backend_job_attempts("workspace", "accepted")
            .unwrap();
        let mut wrong = owner.clone();
        wrong.runtime_run_id = Some("later-run".into());
        assert!(
            store
                .claim_backend_job_worker_cleanup_owned("workspace", &history, &wrong, NOW)
                .is_err()
        );
        assert!(
            store
                .claim_backend_job_worker_cleanup_owned("workspace", &[], &owner, NOW)
                .is_err()
        );
        let (executing, claimed) = store
            .claim_backend_job_worker_cleanup_owned("workspace", &history, &owner, NOW)
            .unwrap();
        assert!(claimed);
        // Even identical Job/attempt IDs cannot erase an intervening cleanup fact.
        assert!(
            store
                .finish_backend_job_worker_cleanup_owned(
                    "workspace",
                    &history,
                    &owner,
                    BackendJobWorkerCleanupState::Completed,
                    None,
                    None,
                    NOW
                )
                .is_err()
        );
        let history = store
            .list_backend_job_attempts("workspace", "accepted")
            .unwrap();
        let done = store
            .finish_backend_job_worker_cleanup_owned(
                "workspace",
                &history,
                &executing,
                BackendJobWorkerCleanupState::Completed,
                None,
                None,
                NOW,
            )
            .unwrap();
        assert_eq!(
            done.worker_cleanup_state,
            Some(BackendJobWorkerCleanupState::Completed)
        );
        // Cleanup and domain completion remain independent facts.
        assert_eq!(
            store
                .get_backend_job("workspace", "accepted")
                .unwrap()
                .unwrap()
                .state,
            BackendJobState::Pending
        );
        assert!(
            store
                .reserve_backend_job_retry("workspace", "accepted", NOW)
                .is_err()
        );
    }

    #[test]
    fn diagnosis_and_failure_do_not_turn_an_unbound_dispatch_into_execution_end() {
        let (_dir, store, reserved) = fixture();
        store
            .claim_backend_job_attempt_dispatch(
                "workspace",
                "accepted",
                &reserved.attempt.attempt_id,
                NOW,
            )
            .unwrap();
        let failed = store
            .finish_backend_job_attempt(
                "workspace",
                "accepted",
                &reserved.attempt.attempt_id,
                BackendJobAttemptState::Failed,
                "dispatch_refused",
                "definitive provider refusal",
                NOW,
            )
            .unwrap();
        assert_eq!(
            failed.worker_cleanup_state,
            Some(BackendJobWorkerCleanupState::Pending)
        );
        assert!(failed.worker_cleanup_completed_at.is_none());
        assert!(
            store
                .reserve_backend_job_retry("workspace", "accepted", NOW)
                .is_err()
        );
    }
}
