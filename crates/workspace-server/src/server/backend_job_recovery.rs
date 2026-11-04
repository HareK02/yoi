//! Recovery belongs to the producer of the accepted attempt, not to callers replaying it.
//! Runtime receipts, domain completion and execution/Host termination are independent facts.
//! Only initial dispatch opens Host admission; a reconstructed empty map proves nothing.
use super::*;
use crate::backend_job::BackendJobRecord;
use crate::backend_job_host_effects::{JobHostEffects, JobHostOwner, JobHostRequestLease};
use feature_storage::FeatureStorageError;
use std::collections::HashSet;
use std::time::Duration;

pub(super) const SETTLEMENT_BACKOFF_MS: [u64; 5] = [100, 200, 400, 800, 1600];

#[derive(Default)]
pub(super) struct RecoveryOwners {
    running: Mutex<HashSet<String>>,
    cleanup_running: Mutex<HashSet<String>>,
    gates: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    hosts: Mutex<HashMap<String, (JobHostOwner, Arc<JobHostEffects>)>>,
    results: Mutex<HashMap<String, (RuntimeWorkerRef, BackendJobResultSubmission)>>,
    submitted: Mutex<HashSet<String>>,
    stopping: std::sync::atomic::AtomicBool,
    changed: tokio::sync::Notify,
    #[cfg(test)]
    pub(super) storage_probe_started: tokio::sync::Notify,
    #[cfg(test)]
    pub(super) domain_storage_settlement: tokio::sync::Notify,
    #[cfg(test)]
    pub(super) nonjournal_lookup_pending: tokio::sync::Notify,
    #[cfg(test)]
    pub(super) cleanup_before_claim: Mutex<Option<Arc<tokio::sync::Semaphore>>>,
}

impl RecoveryOwners {
    #[cfg(test)]
    pub(super) fn resume_after_test_pause(&self) {
        self.stopping.store(false, Ordering::Release);
    }

    #[cfg(test)]
    pub(super) fn cleanup_owner_ids(&self) -> HashSet<String> {
        self.cleanup_running.lock().unwrap().clone()
    }

    async fn pause(&self, duration: Duration) {
        let notified = self.changed.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if self.stopping.load(Ordering::Acquire) {
            return;
        }
        tokio::select! { _ = tokio::time::sleep(duration) => {}, _ = notified => {} }
    }

    pub(super) fn dispatch_gate(&self, attempt: &str) -> Arc<Mutex<()>> {
        self.gates
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(attempt.to_owned())
            .or_default()
            .clone()
    }

    pub(super) fn initially_admit(&self, owner: JobHostOwner) -> Result<()> {
        let mut hosts = self
            .hosts
            .lock()
            .map_err(|_| Error::Store("Job Host owners poisoned".into()))?;
        match hosts.get(&owner.attempt_id) {
            Some((existing, _)) if existing == &owner => Ok(()),
            Some(_) => Err(Error::RepositoryConflict("Job Host owner changed".into())),
            None => {
                hosts.insert(
                    owner.attempt_id.clone(),
                    (owner.clone(), JobHostEffects::initially_admit(owner)),
                );
                Ok(())
            }
        }
    }

    fn initial_submission_pending(&self, owner: &JobHostOwner) -> Result<bool> {
        self.effects(owner)?;
        Ok(!self
            .submitted
            .lock()
            .map_err(|_| Error::Store("Job submission ownership poisoned".into()))?
            .contains(&owner.attempt_id))
    }

    fn claim_initial_submission(&self, owner: &JobHostOwner) -> Result<bool> {
        self.effects(owner)?;
        // Set BEFORE the external call. Once set, even a lost response can only
        // be reconciled by receipt; restoring an empty owner never reaches here.
        Ok(self
            .submitted
            .lock()
            .map_err(|_| Error::Store("Job submission ownership poisoned".into()))?
            .insert(owner.attempt_id.clone()))
    }

    pub(super) fn effects(&self, expected: &JobHostOwner) -> Result<Arc<JobHostEffects>> {
        self.hosts.lock().map_err(|_| Error::Store("Job Host owners poisoned".into()))?
            .get(&expected.attempt_id).filter(|(owner, _)| owner == expected)
            .map(|(_, effects)| effects.clone())
            .ok_or_else(|| Error::RepositoryConflict(
                "Original live Job Host owner unavailable; retained operation/writer fences required".into(),
            ))
    }
}

struct RecoveryLease {
    owners: Arc<RecoveryOwners>,
    attempt: String,
    cleanup: bool,
}
impl Drop for RecoveryLease {
    fn drop(&mut self) {
        let set = if self.cleanup {
            &self.owners.cleanup_running
        } else {
            &self.owners.running
        };
        set.lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&self.attempt);
        self.owners.changed.notify_waiters();
    }
}

fn input_request(attempt: &BackendJobAttemptRecord, content: String) -> WorkerInputRequest {
    WorkerInputRequest {
        source: None,
        kind: WorkerInputKind::User,
        content,
        submission_request_id: attempt.runtime_run_id.clone(),
        segments: None,
    }
}

fn receipt_matches(
    attempt: &BackendJobAttemptRecord,
    result: &crate::hosts::WorkerInputResult,
) -> bool {
    result.state == InternalWorkerOperationState::Accepted
        && result.disposition == WorkerInputDisposition::Accepted
        && attempt.worker.as_ref() == Some(&result.worker)
        && attempt.runtime_run_id.is_some()
        && result.runtime_run_id == attempt.runtime_run_id
}

fn transient_subjektiv_storage(error: &Error) -> bool {
    match error {
        Error::SubjektivStorage(FeatureStorageError::Sqlite(rusqlite::Error::SqliteFailure(
            error,
            _,
        ))) => matches!(
            error.code,
            rusqlite::ErrorCode::DatabaseBusy
                | rusqlite::ErrorCode::DatabaseLocked
                | rusqlite::ErrorCode::SystemIoFailure
                | rusqlite::ErrorCode::DiskFull
                | rusqlite::ErrorCode::CannotOpen
        ),
        // Feature I/O is bounded by the same operation's deadline, not a health flag.
        Error::SubjektivStorage(FeatureStorageError::Io(_)) => true,
        Error::SubjektivStorage(FeatureStorageError::OutcomeUnknown { .. }) => true,
        _ => false,
    }
}

fn transient_storage(error: &Error) -> bool {
    transient_subjektiv_storage(error) || workspace_storage_failure(error)
}

fn workspace_storage_failure(error: &Error) -> bool {
    matches!(error, Error::Sqlite(rusqlite::Error::SqliteFailure(error, _))
        if matches!(error.code, rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
            | rusqlite::ErrorCode::SystemIoFailure | rusqlite::ErrorCode::DiskFull
            | rusqlite::ErrorCode::CannotOpen))
        // Lost DB responses are not definitive execution failures. Reconcile the
        // same owner through a serialized storage probe before refilling budget.
        || matches!(error, Error::DbCapacityExceeded | Error::DbOperationOutcomeUnknown)
}

impl WorkspaceApi {
    pub(super) async fn stop_backend_job_recovery(&self) {
        self.backend_job_recovery
            .stopping
            .store(true, Ordering::Release);
        self.backend_job_recovery.changed.notify_waiters();
        loop {
            let notified = self.backend_job_recovery.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let executions_empty = self
                .backend_job_recovery
                .running
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .is_empty();
            let cleanups_empty = self
                .backend_job_recovery
                .cleanup_running
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .is_empty();
            if executions_empty && cleanups_empty {
                return;
            }
            // Already-admitted Host/DB closures are awaited, never aborted/dropped
            // into a fabricated termination receipt. Stop only future producer steps.
            notified.await;
        }
    }

    /// Retain a bounded result on its original live producer when storage fails.
    /// Reconciliation re-enters all domain/source/CAS checks; memory is not acceptance.
    pub fn accept_backend_job_result(
        &self,
        source: &RuntimeWorkerRef,
        submission: &BackendJobResultSubmission,
    ) -> Result<BackendJobResultAcceptance> {
        let result = self.accept_backend_job_result_inner(source, submission);
        if result.as_ref().is_err_and(|error| {
            transient_storage(error) || matches!(error, Error::DbOperationOutcomeUnknown)
        }) {
            let bytes = serde_json::to_vec(submission)
                .map_err(|error| Error::InvalidInput(error.to_string()))?;
            if bytes.len()
                <= usize::try_from(crate::backend_job::ABSOLUTE_MAX_RESULT_BYTES)
                    .unwrap_or(usize::MAX)
                    + 4096
            {
                let valid = self
                    .backend_job_recovery
                    .hosts
                    .lock()
                    .map_err(|_| Error::Store("Job Host owners poisoned".into()))?
                    .get(&submission.attempt_id)
                    .is_some_and(|(owner, _)| {
                        &owner.worker == source
                            && owner.job_id == submission.job_id
                            && owner.input_digest == submission.input_digest
                    });
                if valid {
                    let mut results = self
                        .backend_job_recovery
                        .results
                        .lock()
                        .map_err(|_| Error::Store("Job retained results poisoned".into()))?;
                    if let Some((saved_source, saved)) = results.get(&submission.attempt_id) {
                        if saved_source != source
                            || serde_json::to_value(saved).ok()
                                != serde_json::to_value(submission).ok()
                        {
                            return Err(Error::RepositoryConflict(
                                "Different result already retained for this execution".into(),
                            ));
                        }
                    } else {
                        results.insert(
                            submission.attempt_id.clone(),
                            (source.clone(), submission.clone()),
                        );
                    }
                }
            }
        }
        if result.is_ok() {
            self.backend_job_recovery
                .results
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&submission.attempt_id);
        }
        result
    }

    pub(super) fn retain_backend_job_recovery(
        &self,
        attempt: &BackendJobAttemptRecord,
        category: &str,
        detail: &str,
        next: &str,
    ) -> Result<BackendJobAttemptRecord> {
        self.store.record_backend_job_reconciliation(
            self.workspace_id(),
            attempt,
            category,
            detail,
            next,
            &Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true),
        )
    }

    pub(super) fn submit_initial_backend_job_input(
        &self,
        job: &BackendJobRecord,
        attempt: &BackendJobAttemptRecord,
        content: String,
    ) -> Result<()> {
        let worker = attempt
            .worker
            .as_ref()
            .ok_or_else(|| Error::Store("Job input missing Worker".into()))?;
        let request = input_request(attempt, content);
        if !self
            .backend_job_recovery
            .claim_initial_submission(&JobHostOwner::try_from(attempt)?)?
        {
            let receipt = self
                .runtime
                .reconcile_worker_input(worker, request)
                .map_err(RuntimeRegistryError::into_error)?;
            return if receipt_matches(attempt, &receipt) {
                Ok(())
            } else {
                Err(Error::RepositoryConflict(
                    "Previously attempted Job input requires exact receipt reconciliation".into(),
                ))
            };
        }
        let dispatched = self.runtime.send_input(worker, request.clone());
        if let Ok(result) = &dispatched {
            if receipt_matches(attempt, result) {
                return Ok(());
            }
            if result.worker == *worker && result.disposition == WorkerInputDisposition::Rejected {
                self.finish_backend_job_attempt(
                    &attempt.job_id,
                    &attempt.attempt_id,
                    BackendJobAttemptState::Failed,
                    "input_rejected",
                    &runtime_diagnostics_message(&result.diagnostics),
                )?;
                return Err(Error::RuntimeOperationFailed {
                    runtime_id: worker.runtime_id.clone(),
                    code: "backend_job_input_rejected".into(),
                    message: "Runtime definitively rejected Job input".into(),
                });
            }
        }
        // Lost/mismatched acknowledgements require read-only receipt lookup, never Submit replay.
        let receipt = self.runtime.reconcile_worker_input(worker, request);
        if receipt
            .as_ref()
            .is_ok_and(|result| receipt_matches(attempt, result))
        {
            return Ok(());
        }
        let detail = match dispatched {
            Err(error) => error.message(),
            Ok(result) => runtime_diagnostics_message(&result.diagnostics),
        };
        let current = self
            .store
            .get_backend_job_attempt(
                self.workspace_id(),
                &job.request.job_id,
                &attempt.attempt_id,
            )?
            .ok_or_else(|| Error::Store("Job input owner disappeared".into()))?;
        if !current.state.terminal() {
            self.retain_backend_job_recovery(&current, "submission_receipt_unconfirmed", &detail,
                "Read exact Runtime submission and domain receipts; never resubmit non-idempotent input")?;
        }
        Err(Error::RuntimeOperationFailed {
            runtime_id: worker.runtime_id.clone(),
            code: "backend_job_submission_receipt_unconfirmed".into(),
            message: detail,
        })
    }

    pub(super) fn own_backend_job_recovery(&self, attempt: &BackendJobAttemptRecord) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let mut running = self
            .backend_job_recovery
            .running
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if self.backend_job_recovery.stopping.load(Ordering::Acquire)
            || !running.insert(attempt.attempt_id.clone())
        {
            return;
        }
        drop(running);
        let lease = RecoveryLease {
            owners: self.backend_job_recovery.clone(),
            attempt: attempt.attempt_id.clone(),
            cleanup: false,
        };
        let api = self.clone();
        let expected = attempt.clone();
        handle.spawn(async move {
            let _lease = lease;
            if let Err(error) = api.run_backend_job_recovery(expected.clone()).await {
                tracing::error!(job_id=%expected.job_id, attempt_id=%expected.attempt_id, %error,
                    "Backend Job recovery stopped with retained execution/input; no replacement dispatched");
            }
        });
    }

    pub(super) fn own_backend_job_cleanup(&self, attempt: &BackendJobAttemptRecord) -> bool {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return false;
        };
        let mut running = self
            .backend_job_recovery
            .cleanup_running
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if self.backend_job_recovery.stopping.load(Ordering::Acquire)
            || !running.insert(attempt.attempt_id.clone())
        {
            return false;
        }
        drop(running);
        let lease = RecoveryLease {
            owners: self.backend_job_recovery.clone(),
            attempt: attempt.attempt_id.clone(),
            cleanup: true,
        };
        let api = self.clone();
        let expected = attempt.clone();
        // Tokio tasks do not inherit OPERATION_LIFETIME. Admitting cleanup from
        // inside the result's Job lease would make close_and_drain wait on itself.
        runtime.spawn(async move {
            let _lease = lease;
            #[cfg(test)]
            {
                let gate = api
                    .backend_job_recovery
                    .cleanup_before_claim
                    .lock()
                    .unwrap()
                    .clone();
                if let Some(gate) = gate {
                    let Ok(permit) = gate.acquire().await else {
                        return;
                    };
                    drop(permit);
                }
            }
            if let Err(error) = api.run_backend_job_cleanup(expected.clone()).await {
                tracing::warn!(job_id=%expected.job_id, attempt_id=%expected.attempt_id, %error,
                    "Job cleanup stopped; execution/input/resource fences remain retained");
            }
        });
        true
    }

    pub(super) async fn run_backend_job_cleanup(
        &self,
        expected: BackendJobAttemptRecord,
    ) -> Result<()> {
        let claimed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        // Pin the observed canonical operation across this owner's storage retries.
        let removal_owner = Arc::new(Mutex::new(None::<(String, String)>));
        let mut failures = 0;
        loop {
            if self.backend_job_recovery.stopping.load(Ordering::Acquire) {
                return Ok(());
            }
            let api = self.clone();
            let owner = expected.clone();
            let mine = claimed.clone();
            let removal_owner = removal_owner.clone();
            let runtime = tokio::runtime::Handle::current();
            let result = self
                .host_executors
                .cleanup()?
                .run(move || {
                    let current = api
                        .store
                        .get_backend_job_attempt(
                            api.workspace_id(),
                            &owner.job_id,
                            &owner.attempt_id,
                        )?
                        .ok_or_else(|| Error::Store("Cleanup owner disappeared".into()))?;
                    if current.worker != owner.worker
                        || current.runtime_run_id != owner.runtime_run_id
                        || current.input_digest != owner.input_digest
                    {
                        return Err(Error::RepositoryConflict(
                            "Cleanup execution owner changed".into(),
                        ));
                    }
                    if current.worker_cleanup_state == Some(BackendJobWorkerCleanupState::Completed)
                    {
                        return api.finish_backend_job_cleanup_retirement(
                            &owner.job_id,
                            &owner.attempt_id,
                        );
                    }
                    let attempt = if current.worker_cleanup_state
                        == Some(BackendJobWorkerCleanupState::Executing)
                    {
                        if !mine.load(Ordering::Acquire) {
                            // Executing is not a lease we can steal after restart.
                            // First require the exact durable execution/Host fence,
                            // then recover (never plan) its canonical removal owner.
                            api.require_backend_job_attempt_effects_ended(&current)?;
                            let worker = current.worker.as_ref().ok_or_else(||
                                Error::RepositoryConflict("Executing cleanup Worker missing".into()))?;
                            let saved = api.config_store.recover_worker_removal_execution(
                                api.workspace_id(), worker,
                            ).map_err(|error| Error::Store(error.to_string()))?
                                .ok_or_else(|| Error::RepositoryConflict(
                                    "Executing cleanup has no retained canonical removal operation; ownership cannot be reconstructed".into()))?;
                            if saved.plan.workspace_id != api.workspace_id()
                                || saved.plan.worker != *worker
                                || saved.plan.operation_id.is_empty()
                                || saved.plan.input_fingerprint.is_empty()
                                || saved.runtime_request.workspace_id != api.workspace_id()
                                || saved.runtime_request.source_runtime_id != worker.runtime_id
                                || saved.runtime_request.worker_id.to_string() != worker.worker_id
                                || saved.runtime_request.operation_id != saved.plan.operation_id
                                || saved.runtime_request.input_fingerprint != saved.plan.input_fingerprint
                            {
                                return Err(Error::RepositoryConflict(
                                    "Executing cleanup canonical removal owner mismatch".into()));
                            }
                            let expected = (saved.plan.operation_id, saved.plan.input_fingerprint);
                            let mut pinned = removal_owner.lock().map_err(|_| Error::Store(
                                "Cleanup removal owner poisoned".into()))?;
                            if pinned.as_ref().is_some_and(|prior| prior != &expected) {
                                return Err(Error::RepositoryConflict(
                                    "Executing cleanup canonical removal owner changed".into()));
                            }
                            *pinned = Some(expected);
                        }
                        current
                    } else {
                        let history = api
                            .store
                            .list_backend_job_attempts(api.workspace_id(), &owner.job_id)?;
                        let (attempt, acquired) =
                            api.store.claim_backend_job_worker_cleanup_owned(
                                api.workspace_id(),
                                &history,
                                &current,
                                &Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true),
                            )?;
                        if !acquired {
                            return Err(Error::RepositoryConflict(
                                "Cleanup belongs to another admitted owner".into(),
                            ));
                        }
                        mine.store(true, Ordering::Release);
                        attempt
                    };
                    let retained_removal = removal_owner.lock().map_err(|_| Error::Store(
                        "Cleanup removal owner poisoned".into()))?.clone();
                    runtime.block_on(async {
                        if attempt.state == BackendJobAttemptState::Completed
                            || attempt.failure_category.as_deref() == Some("invalid_result")
                        {
                            api.wait_for_backend_job_worker_run_teardown(&attempt)
                                .await?;
                        }
                        api.cleanup_backend_job_attempt_worker(attempt, retained_removal).await
                    })
                })
                .await;
            match result {
                Ok(()) => return Ok(()),
                Err(error)
                    if failures < 5
                        && (transient_storage(&error)
                            || matches!(error, Error::RuntimeOperationFailed { .. })) =>
                {
                    self.backend_job_recovery
                        .pause(Duration::from_millis(100 << failures))
                        .await;
                    failures += 1;
                }
                Err(error) if workspace_storage_failure(&error) => {
                    tracing::warn!(attempt_id=%expected.attempt_id, %error, "Cleanup settlement budget exhausted; exact-owner storage probe only");
                    let mut delay = Duration::from_secs(1);
                    loop {
                        if self.backend_job_recovery.stopping.load(Ordering::Acquire) {
                            return Ok(());
                        }
                        self.backend_job_recovery.pause(delay).await;
                        let api = self.clone();
                        let owner = expected.clone();
                        let probe = self
                            .host_executors
                            .cleanup()?
                            .run(move || {
                                let current = api
                                    .store
                                    .get_backend_job_attempt(
                                        api.workspace_id(),
                                        &owner.job_id,
                                        &owner.attempt_id,
                                    )?
                                    .ok_or_else(|| {
                                        Error::Store("Cleanup probe owner disappeared".into())
                                    })?;
                                api.store
                                    .probe_backend_job_recovery(api.workspace_id(), &current)
                            })
                            .await;
                        match probe {
                            Ok(true) => {
                                failures = 0;
                                break;
                            }
                            Ok(false) => {
                                return Err(Error::RepositoryConflict(
                                    "Cleanup probe owner changed".into(),
                                ));
                            }
                            Err(error) if transient_storage(&error) => {
                                delay = (delay * 2).min(Duration::from_secs(30));
                            }
                            Err(error) => return Err(error),
                        }
                    }
                }
                Err(error) => return Err(error),
            }
        }
    }

    async fn run_backend_job_recovery(&self, expected: BackendJobAttemptRecord) -> Result<()> {
        // Workspace write probes never refill a Feature operation's settlement budget.
        // Feature failures reconcile the original operation until its original deadline.
        let mut storage_failures = 0;
        let mut domain_failures = 0usize;
        let deadline = chrono::DateTime::parse_from_rfc3339(&expected.deadline_at)
            .map_err(|error| Error::Store(error.to_string()))?;
        let mut definitive_failure: Option<String> = None;
        let mut domain_failure: Option<String> = None;
        loop {
            if self.backend_job_recovery.stopping.load(Ordering::Acquire) {
                return Ok(());
            }
            if Utc::now() >= deadline
                && let Some(error) = domain_failure.take()
            {
                definitive_failure = Some(format!(
                    "Original attempt deadline exceeded during exact Feature operation reconciliation: {error}; execution and Host termination remain unconfirmed"
                ));
            }
            let api = self.clone();
            let owner = expected.clone();
            let failure = definitive_failure.clone();
            let step = self
                .host_executors
                .host()?
                .run(move || {
                    if let Some(detail) = failure {
                        api.fail_backend_job_recovery(&owner, &detail)
                    } else {
                        let result = api.recover_backend_job_step(&owner);
                        if let Err(error) = &result
                            && transient_subjektiv_storage(error)
                        {
                            let current = api.store.get_backend_job_attempt(api.workspace_id(),
                                &owner.job_id, &owner.attempt_id)?
                                .ok_or_else(|| Error::Store("Domain reconciliation owner disappeared".into()))?;
                            api.retain_backend_job_recovery(&current, "domain_storage_unavailable",
                                &error.to_string(),
                                "Reconcile the same Feature operation and receipt until the original attempt deadline; do not resubmit or probe another database")?;
                        }
                        result
                    }
                })
                .await;
            match step {
                Ok(true) => return Ok(()),
                Ok(false) => {
                    domain_failure = None;
                    self.backend_job_recovery
                        .pause(Duration::from_millis(250))
                        .await;
                }
                Err(error) if transient_subjektiv_storage(&error) => {
                    domain_failure = Some(error.to_string());
                    if Utc::now() >= deadline {
                        continue;
                    }
                    if domain_failures >= SETTLEMENT_BACKOFF_MS.len() {
                        #[cfg(test)]
                        self.backend_job_recovery
                            .domain_storage_settlement
                            .notify_one();
                    }
                    let delay = SETTLEMENT_BACKOFF_MS
                        .get(domain_failures)
                        .copied()
                        .map(Duration::from_millis)
                        .unwrap_or_else(|| {
                            Duration::from_secs(
                                (1u64 << domain_failures.saturating_sub(5).min(5)).min(30),
                            )
                        });
                    domain_failures = domain_failures.saturating_add(1);
                    let remaining = (deadline.with_timezone(&Utc) - Utc::now())
                        .to_std()
                        .unwrap_or_default();
                    self.backend_job_recovery.pause(delay.min(remaining)).await;
                }
                Err(error) if workspace_storage_failure(&error) => {
                    if let Some(delay) = SETTLEMENT_BACKOFF_MS.get(storage_failures) {
                        storage_failures += 1;
                        self.backend_job_recovery
                            .pause(Duration::from_millis(*delay))
                            .await;
                        continue;
                    }
                    tracing::warn!(job_id=%expected.job_id, attempt_id=%expected.attempt_id, %error,
                        "Job settlement budget exhausted; owner now probes exact-attempt storage only");
                    let mut delay = Duration::from_secs(1);
                    loop {
                        if self.backend_job_recovery.stopping.load(Ordering::Acquire) {
                            return Ok(());
                        }
                        self.backend_job_recovery.pause(delay).await;
                        let api = self.clone();
                        let owner = expected.clone();
                        match self
                            .host_executors
                            .host()?
                            .run(move || api.probe_backend_job_storage(&owner))
                            .await
                        {
                            Ok(()) => {
                                storage_failures = 0;
                                break;
                            }
                            Err(error) if transient_storage(&error) => {
                                delay = (delay * 2).min(Duration::from_secs(30));
                            }
                            Err(error) => return Err(error),
                        }
                    }
                }
                Err(error) if definitive_failure.is_none() => {
                    definitive_failure = Some(error.to_string());
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn fail_backend_job_recovery(
        &self,
        owner: &BackendJobAttemptRecord,
        detail: &str,
    ) -> Result<bool> {
        let current = self
            .store
            .get_backend_job_attempt(self.workspace_id(), &owner.job_id, &owner.attempt_id)?
            .ok_or_else(|| Error::Store("Failed recovery attempt disappeared".into()))?;
        let job = self
            .store
            .get_backend_job(self.workspace_id(), &owner.job_id)?
            .ok_or_else(|| Error::Store("Failed recovery Job disappeared".into()))?;
        if job.current_attempt != owner.attempt
            || job.input_digest != owner.input_digest
            || self
                .store
                .backend_job_was_migrated(self.workspace_id(), &owner.job_id)?
        {
            return Ok(true);
        }
        if !current.state.terminal() {
            self.finish_backend_job_attempt(
                &owner.job_id,
                &owner.attempt_id,
                BackendJobAttemptState::Failed,
                "recovery_cannot_continue",
                detail,
            )?;
        }
        self.schedule_backend_job_worker_cleanup(&current);
        Ok(true)
    }

    fn probe_backend_job_storage(&self, owner: &BackendJobAttemptRecord) -> Result<()> {
        #[cfg(test)]
        self.backend_job_recovery.storage_probe_started.notify_one();
        let current = self
            .store
            .get_backend_job_attempt(self.workspace_id(), &owner.job_id, &owner.attempt_id)?
            .ok_or_else(|| Error::Store("Job storage probe owner disappeared".into()))?;
        // A read or another Job's commit cannot declare this accepted operation
        // recoverable. The Store commits an exact-owner probe without rewriting
        // the saved result or original cause, including terminal cleanup owners.
        if self
            .store
            .probe_backend_job_recovery(self.workspace_id(), &current)?
        {
            Ok(())
        } else {
            Err(Error::RepositoryConflict(
                "Job storage probe owner changed".into(),
            ))
        }
    }

    fn recover_backend_job_step(&self, expected: &BackendJobAttemptRecord) -> Result<bool> {
        let Some(attempt) = self.store.get_backend_job_attempt(
            self.workspace_id(),
            &expected.job_id,
            &expected.attempt_id,
        )?
        else {
            return Err(Error::Store("Accepted Job attempt missing".into()));
        };
        let job = self
            .store
            .get_backend_job(self.workspace_id(), &attempt.job_id)?
            .ok_or_else(|| Error::Store("Accepted Job missing".into()))?;
        if job.current_attempt != attempt.attempt || job.input_digest != expected.input_digest {
            return Err(Error::RepositoryConflict(
                "Recovery owner no longer current".into(),
            ));
        }
        let retained = self
            .backend_job_recovery
            .results
            .lock()
            .map_err(|_| Error::Store("Job retained results poisoned".into()))?
            .get(&attempt.attempt_id)
            .cloned();
        if let Some((source, submission)) = retained {
            match self.accept_backend_job_result(&source, &submission) {
                Ok(_) => return Ok(false),
                Err(error) if transient_storage(&error) => return Err(error),
                Err(error @ Error::DbOperationOutcomeUnknown) => return Err(error),
                Err(error) => {
                    self.backend_job_recovery
                        .results
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .remove(&attempt.attempt_id);
                    if !attempt.state.terminal() {
                        self.finish_backend_job_attempt(
                            &attempt.job_id,
                            &attempt.attempt_id,
                            BackendJobAttemptState::Failed,
                            "retained_result_rejected",
                            &error.to_string(),
                        )?;
                    }
                    return Ok(false);
                }
            }
        }
        if attempt.state.terminal() {
            self.schedule_backend_job_worker_cleanup(&attempt);
            self.drain_reserved_backend_jobs();
            return Ok(true);
        }
        if self
            .store
            .backend_job_was_migrated(self.workspace_id(), &attempt.job_id)?
        {
            return Ok(true);
        }
        if attempt.state == BackendJobAttemptState::Reserved {
            let deadline = chrono::DateTime::parse_from_rfc3339(&attempt.deadline_at)
                .map_err(|error| Error::Store(error.to_string()))?;
            if Utc::now() >= deadline {
                self.finish_backend_job_attempt(&attempt.job_id, &attempt.attempt_id,
                    BackendJobAttemptState::Failed, "timeout",
                    "Original attempt deadline exceeded before dispatch; no execution termination is inferred")?;
                return Ok(false);
            }
            self.dispatch_backend_job_reservation(BackendJobReservation {
                job,
                attempt,
                replayed: true,
                resource_reused: false,
            })?;
            return Ok(false);
        }
        let gate = self.backend_job_recovery.dispatch_gate(&attempt.attempt_id);
        let _guard = gate
            .lock()
            .map_err(|_| Error::Store("Job recovery dispatch gate poisoned".into()))?;
        // Reread after acquiring the producer gate: initial dispatch might have bound/finished it.
        let mut attempt = self
            .store
            .get_backend_job_attempt(self.workspace_id(), &attempt.job_id, &attempt.attempt_id)?
            .ok_or_else(|| Error::Store("Job receipt owner missing".into()))?;
        if attempt.state.terminal() {
            return Ok(false);
        }
        if job.request.grants.subjektiv_journal.is_some() {
            if let Err(error) = subjektiv_journal::reconcile_attempt(self, &job, &attempt) {
                if transient_storage(&error) {
                    return Err(error);
                }
                // Incomplete generation/missing termination evidence is not a
                // rejected dispatch or permission to reselect Journal input.
                self.retain_backend_job_recovery(&attempt, "journal_receipt_reconciliation",
                    &error.to_string(), "Read retained generation/checkpoints and exact execution fences until deadline")?;
            }
            attempt = self
                .store
                .get_backend_job_attempt(self.workspace_id(), &attempt.job_id, &attempt.attempt_id)?
                .ok_or_else(|| Error::Store("Journal receipt owner missing".into()))?;
            if attempt.state.terminal() {
                return Ok(false);
            }
        }
        if job.request.grants.subjektiv_consolidation.is_some()
            && let Some(result) = subjektiv_job::lookup_result(self, &job, &attempt)?
        {
            let current = self.store.reconcile_backend_job_domain_result(
                self.workspace_id(),
                &attempt,
                &BackendJobResultSubmission {
                    job_id: attempt.job_id.clone(),
                    attempt_id: attempt.attempt_id.clone(),
                    input_digest: attempt.input_digest.clone(),
                    result,
                },
                &Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true),
            )?;
            let saved = self
                .store
                .get_backend_job(self.workspace_id(), &attempt.job_id)?
                .ok_or_else(|| Error::Store("Reconciled consolidation Job missing".into()))?;
            notification_delivery::deliver_job_result(
                self,
                &BackendJobResultAcceptance {
                    job: saved,
                    attempt: current.clone(),
                    replayed: false,
                },
            );
            self.schedule_backend_job_worker_cleanup(&current);
            return Ok(false);
        }
        #[cfg(test)]
        if job.request.grants.subjektiv_consolidation.is_some() {
            self.backend_job_recovery
                .nonjournal_lookup_pending
                .notify_waiters();
        }
        let deadline = chrono::DateTime::parse_from_rfc3339(&attempt.deadline_at)
            .map_err(|error| Error::Store(error.to_string()))?;
        if Utc::now() >= deadline {
            self.finish_backend_job_attempt(&attempt.job_id, &attempt.attempt_id,
                BackendJobAttemptState::Failed, "timeout",
                "Attempt deadline exceeded while reconciling exact Runtime/domain receipts; termination and Host cleanup remain independent")?;
            return Ok(false);
        }
        match attempt.state {
            BackendJobAttemptState::Dispatching => {
                self.reconcile_backend_job_allocation(&job, &attempt)?
            }
            BackendJobAttemptState::Dispatched => {
                let owner = JobHostOwner::try_from(&attempt)?;
                if self
                    .backend_job_recovery
                    .initial_submission_pending(&owner)
                    .unwrap_or(false)
                {
                    // A failed binding save can precede the first send. Only the
                    // original live producer's never-sent claim permits that send;
                    // restored/reconciled allocations cannot create this authority.
                    if let Err(error) = self.submit_initial_backend_job_input(
                        &job,
                        &attempt,
                        job.load_executable_request()?
                            .worker_input(&attempt.attempt_id)?,
                    ) {
                        if transient_storage(&error) {
                            return Err(error);
                        }
                    }
                    return Ok(false);
                }
                let worker = attempt
                    .worker
                    .as_ref()
                    .ok_or_else(|| Error::Store("Dispatched Job Worker missing".into()))?;
                let receipt = self.runtime.reconcile_worker_input(
                    worker,
                    input_request(
                        &attempt,
                        job.load_executable_request()?
                            .worker_input(&attempt.attempt_id)?,
                    ),
                );
                match receipt {
                    Ok(result) if receipt_matches(&attempt, &result) => {
                        subjektiv_journal::attest_delivery(
                            self,
                            &BackendJobReservation {
                                job,
                                attempt,
                                replayed: true,
                                resource_reused: false,
                            },
                        )?;
                    }
                    Ok(result) => {
                        self.retain_backend_job_recovery(&attempt, "submission_receipt_unconfirmed",
                            &runtime_diagnostics_message(&result.diagnostics),
                            "Read same submission/domain receipts until deadline; never blindly repeat input")?;
                    }
                    Err(error) => {
                        self.retain_backend_job_recovery(&attempt, "submission_receipt_unavailable", &error.message(),
                            "Read same submission/domain receipts until deadline; keep execution and input owned")?;
                    }
                }
            }
            _ => {}
        }
        Ok(false)
    }

    /// Only initial dispatch resolves the live Profile and constructs create intent.
    /// Accepted-allocation receipt recovery reads the immutable saved reservation instead.
    pub(super) fn backend_job_spawn_request(
        &self,
        job: &BackendJobRecord,
        attempt: &BackendJobAttemptRecord,
    ) -> Result<WorkerSpawnRequest> {
        let request = job.load_executable_request()?;
        let (profile, config_bundle) = self.resolve_backend_job_profile(&request)?;
        Ok(WorkerSpawnRequest {
            requested_worker_name: Some(format!("job:{}", request.purpose)),
            singleton_key: None,
            intent: WorkerSpawnIntent::BackendJob {
                job_id: request.job_id.clone(),
                attempt_id: attempt.attempt_id.clone(),
                purpose: request.purpose.clone(),
                input_digest: request.input_digest()?,
                subjektiv_consolidation: request.grants.subjektiv_consolidation.is_some(),
                subjektiv_journal: request.grants.subjektiv_journal.is_some(),
            },
            acceptance: WorkerSpawnAcceptanceRequirement::RunAccepted {
                expected_segments: 0,
            },
            profile,
            ticket_assignment: None,
            initial_submit: Vec::new(),
            workdir_attachment_requests: Vec::new(),
            resolved_workdir_attachment_requests: Vec::new(),
            resolved_workdir_attachments: Vec::new(),
            resolved_config_bundle: Some(config_bundle),
            resolved_worker_observation_enabled: false,
            resolved_worker_observation_grants: Vec::new(),
            resolved_workspace_api: None,
            resolved_memory_settings: None,
            // Journal Jobs use their independent frozen Job Host grant, not the
            // main-Subject/Memory attachment or its resident/extraction policy.
            resolved_subjektiv_attached: request.grants.subjektiv_consolidation.is_some(),
            resolved_control_operation: Some(WorkerControlOperation {
                operation_id: allocation_key(&attempt.attempt_id),
                input_fingerprint: job.intent_fingerprint.clone(),
            }),
            resolved_execution_grant: None,
        })
    }

    fn reconcile_backend_job_allocation(
        &self,
        job: &BackendJobRecord,
        attempt: &BackendJobAttemptRecord,
    ) -> Result<()> {
        let allocation =
            self.config_store
                .lookup_backend_job_allocation(self.workspace_id(), job, attempt)?;
        let Some((allocation, runtime_id)) = allocation else {
            self.retain_backend_job_recovery(
                attempt,
                "allocation_receipt_unconfirmed",
                "Original create reservation is not yet available",
                "Read the original reservation until deadline; never allocate a replacement",
            )?;
            return Ok(());
        };
        let worker = RuntimeWorkerRef::new(runtime_id, allocation.worker_id.to_string());
        // Removed catalog entries and foreign bindings cannot be resurrected from
        // Runtime metadata. Before attempt binding, a Job projection may be absent.
        if let Some(projection) = self
            .store
            .worker_registry_projection(self.workspace_id(), &worker)?
        {
            if projection.registry.retention_state == "removed"
                || projection.job.as_ref().is_some_and(|bound| {
                    bound.job_id != attempt.job_id || bound.attempt_id != attempt.attempt_id
                })
            {
                return Err(Error::RepositoryConflict(
                    "Original Job allocation was removed or rebound".into(),
                ));
            }
        }
        let request = job.load_executable_request()?;
        let binding = worker_runtime::catalog::BackendJobExecutionBinding {
            job_id: attempt.job_id.clone(),
            attempt_id: attempt.attempt_id.clone(),
            input_digest: Some(attempt.input_digest.clone()),
            subjektiv_consolidation: request.grants.subjektiv_consolidation.is_some(),
            subjektiv_journal: request.grants.subjektiv_journal.is_some(),
        };
        match self.runtime.lookup_job_worker_creation(
            &worker,
            &allocation.create_fingerprint,
            &binding,
        ) {
            Ok(Some(receipt)) if receipt.worker == worker => {
                // This read never activates/restores the Worker, submits input, or
                // creates a Host owner from a reconstructed empty effects map.
                self.worker_projection.publish_ordered(|| {
                    self.store
                        .bind_backend_job_attempt_worker(
                            self.workspace_id(),
                            &attempt.job_id,
                            &attempt.attempt_id,
                            &worker,
                            Some(&crate::backend_job::runtime_run_id(&attempt.attempt_id)),
                            &Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true),
                        )
                        .map(|(commit, bound)| (bound, commit))
                })?;
            }
            outcome => {
                let detail = match outcome {
                    Err(error) => error.message(),
                    _ => "No exact Runtime Job creation receipt available".into(),
                };
                self.retain_backend_job_recovery(attempt, "allocation_receipt_unconfirmed", &detail,
                    "Read original allocation receipt until attempt deadline; do not allocate a replacement")?;
            }
        }
        Ok(())
    }

    pub(super) fn finish_backend_job_cleanup_retirement(
        &self,
        job_id: &str,
        attempt_id: &str,
    ) -> Result<()> {
        let current = self
            .store
            .get_backend_job_attempt(self.workspace_id(), job_id, attempt_id)?
            .ok_or_else(|| Error::Store("Cleaned Job attempt disappeared".into()))?;
        self.require_backend_job_attempt_effects_ended(&current)?;
        if current.worker_cleanup_state != Some(BackendJobWorkerCleanupState::Completed) {
            return Err(Error::RepositoryConflict(
                "Canonical Job cleanup not completed".into(),
            ));
        }
        let job = self
            .store
            .get_backend_job(self.workspace_id(), job_id)?
            .ok_or_else(|| Error::Store("Cleaned Job disappeared".into()))?;
        if job.request.grants.subjektiv_journal.is_some()
            && job.state == BackendJobState::Completed
            && job.current_attempt == current.attempt
            && current.state == BackendJobAttemptState::Completed
            && !matches!(
                job.input_disposition,
                crate::backend_job::JobInputDisposition::Erased
                    | crate::backend_job::JobInputDisposition::Invalidated
            )
        {
            // Input retirement is later than execution termination and cleanup.
            // Check every historical owner, not just the latest terminal result.
            for prior in self
                .store
                .list_backend_job_attempts(self.workspace_id(), job_id)?
            {
                self.require_backend_job_attempt_effects_ended(&prior)?;
                if prior.worker_cleanup_state != Some(BackendJobWorkerCleanupState::Completed) {
                    return Err(Error::RepositoryConflict(
                        "Historical Job cleanup not completed".into(),
                    ));
                }
            }
            if job.input_disposition == crate::backend_job::JobInputDisposition::Available {
                self.store.reclaim_backend_job_input(
                    self.workspace_id(),
                    job_id,
                    &job.input_digest,
                    crate::backend_job::JobInputRetirementReason::Completed,
                )?;
            }
            subjektiv_journal::drain_after_completion(self, &job.request);
        }
        // Only after durable fences/retirement may live bookkeeping be forgotten.
        self.backend_job_recovery
            .hosts
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(attempt_id);
        self.backend_job_recovery
            .submitted
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(attempt_id);
        self.backend_job_recovery
            .gates
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(attempt_id);
        Ok(())
    }

    /// Nonblocking consumer boundary; terminal status/cleanup markers are not proof.
    pub(super) fn require_backend_job_attempt_effects_ended(
        &self,
        attempt: &BackendJobAttemptRecord,
    ) -> Result<()> {
        if self
            .store
            .has_backend_job_execution_fence(self.workspace_id(), attempt)?
        {
            return Ok(());
        }
        Err(Error::RepositoryConflict(
            "Exact Controller/Feature/background and Host writer/drain evidence missing".into(),
        ))
    }

    pub(super) async fn fence_backend_job_effects(
        &self,
        attempt: &BackendJobAttemptRecord,
    ) -> Result<()> {
        if self
            .store
            .has_backend_job_execution_fence(self.workspace_id(), attempt)?
        {
            return Ok(());
        }
        let worker = attempt.worker.as_ref().ok_or_else(|| {
            Error::RepositoryConflict("Unbound allocation cannot prove execution ended".into())
        })?;
        let termination = self
            .runtime
            .backend_job_termination(worker)
            .map_err(RuntimeRegistryError::into_error)?
            .ok_or_else(|| {
                Error::RepositoryConflict("Actual Runtime scope termination receipt missing".into())
            })?;
        if !termination.feature_failures.is_empty()
            || termination.scope
                != worker_runtime::execution::WorkerExecutionTerminationScope::Controller
            || termination.worker_ref.worker_id.to_string() != worker.worker_id
        {
            return Err(Error::RepositoryConflict(
                "Runtime receipt does not prove exact Controller/Feature/background termination"
                    .into(),
            ));
        }
        let owner = JobHostOwner::try_from(attempt)?;
        let effects = self.backend_job_recovery.effects(&owner)?;
        let drain = effects.close_and_drain(&owner).await?;
        self.store.record_backend_job_execution_fence(
            self.workspace_id(),
            attempt,
            &serde_json::to_string(&termination)
                .map_err(|error| Error::Store(error.to_string()))?,
            &serde_json::to_string(drain.owner())
                .map_err(|error| Error::Store(error.to_string()))?,
            &Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true),
        )?;
        Ok(())
    }
}

/// All authenticated Job callbacks retain the exact live attempt lease through Host/DB
/// work, including accepted synchronous descendants and discarded response cleanup.
/// Identity hints are never admission authority; the outer request proof supplies source.
pub(super) async fn admit_host_request(
    State(api): State<WorkspaceApi>,
    request: Request,
    next: Next,
) -> Response {
    let Some(source) = request
        .extensions()
        .get::<crate::worker_source::VerifiedRuntimeRequestSource>()
        .cloned()
    else {
        return next.run(request).await;
    };
    let Some(worker_id) = source.worker_id else {
        return next.run(request).await;
    };
    let worker = RuntimeWorkerRef::new(source.runtime_id, worker_id);
    let lookup = api.clone();
    let admission = api
        .store
        .db_executor()
        .run(move || -> Result<Option<JobHostRequestLease>> {
            let Some(job) = lookup
                .store
                .get_active_backend_job_for_worker(lookup.workspace_id(), &worker)?
            else {
                // Completed/failed Jobs must not become ordinary Workers and bypass
                // their closed admission fence. The projection retains Job provenance.
                if lookup
                    .store
                    .worker_registry_projection(lookup.workspace_id(), &worker)?
                    .is_some_and(|p| p.job.is_some())
                {
                    return Err(Error::WorkspacePermissionDenied(
                        "Backend Job execution is no longer admitted".into(),
                    ));
                }
                return Ok(None);
            };
            let id = crate::backend_job::attempt_id(&job.request.job_id, job.current_attempt);
            let attempt = lookup
                .store
                .get_backend_job_attempt(lookup.workspace_id(), &job.request.job_id, &id)?
                .ok_or_else(|| Error::Store("Job Host attempt missing".into()))?;
            let owner = JobHostOwner::try_from(&attempt)?;
            Ok(Some(
                lookup.backend_job_recovery.effects(&owner)?.admit(&owner)?,
            ))
        })
        .await;
    match admission {
        Ok(Some(lease)) => {
            crate::db_executor::with_operation_lifetime(Arc::new(lease), next.run(request)).await
        }
        Ok(None) => next.run(request).await,
        Err(error) => server_error_response(error),
    }
}
