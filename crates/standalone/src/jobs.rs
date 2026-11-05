//! Host-owned local Job execution service.
//!
//! Each attempt uses an existing private Internal Worker, separate from interactive Sessions
//! and model-created SubWorkers. Profile names do not grant result/domain authority.
//! The shared runner validates Profile requirements; the Host supplies a sink bound to the
//! Job, attempt, input digest, and deadline, plus limited domain grants when needed.
//! Parent filesystem and Workspace connections are not implicitly inherited.
//!
//! Success rests on the structured result's SQLite commit, not final prose or `wait` completion.
//! Concurrency slots and serialization keys stay held after result acceptance until cleanup.
//! Dropping a wait future does not release Host cancel/cleanup ownership. Consumer acknowledgement
//! is separate from result lifetime; delivery failures never trigger model reexecution.
//!
//! Timeout/shutdown fences unaccepted attempts before cancellation. Only owned runner shutdown
//! plus joined/revoked Host callbacks discharge execution reservations. Clones cannot reopen
//! the store after service closure or carry authority over to another Host.
mod recovery;
#[cfg(test)]
mod tests;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use agen::llm_client::LlmClient;
use job::{JobRequest, JobResultSubmission};
use tokio::sync::{Notify, watch};

use crate::job_store::JobStore;
pub use crate::job_store::{
    JobAttempt, JobAttemptState, JobExecutionOwnership, JobInputDisposition,
    JobInputRetirementReason, JobSnapshot, JobState, JobStoreError,
};

#[derive(Debug, thiserror::Error)]
pub enum JobServiceError {
    #[error(transparent)]
    Store(#[from] JobStoreError),
    #[error("standalone Job execution requirements are unavailable: {0}")]
    Configuration(String),
    #[error(transparent)]
    DomainStorage(#[from] feature_storage::FeatureStorageError),
    #[error("the standalone Job Host is shutting down")]
    Closed,
    #[error("standalone Job execution capacity is exhausted")]
    Capacity,
    #[error("standalone Job cleanup failed: {0}")]
    Cleanup(String),
    #[error("standalone Job shutdown is incomplete; execution resources are still owned")]
    ShutdownIncomplete,
    #[error(
        "Job settlement storage recovery exhausted after {attempts} tries; execution retained; next action: await a successful write/recovery probe in this Host store, then reconcile the same attempt: {source}"
    )]
    SettlementRecoveryExhausted {
        attempts: usize,
        #[source]
        source: Box<JobServiceError>,
    },
}

pub(crate) trait JobEffectGuard {}
impl<T> JobEffectGuard for T {}

pub(crate) trait JobAttemptFence: Send + Sync {
    fn ensure_live(&self) -> Result<(), String>;
    fn enter(&self) -> Result<Box<dyn JobEffectGuard + '_>, String> {
        self.ensure_live()?;
        Ok(Box::new(()))
    }
}
pub(crate) trait JobDomainProvider: Send + Sync {
    /// Opt in only when every external callback holds `JobAttemptFence::enter`
    /// through its complete synchronous effect and no ungated background work exists.
    /// The service joins that gate after runner/finalizer shutdown; an ordinary
    /// liveness check alone cannot provide this guarantee.
    fn callbacks_use_attempt_fence(&self) -> bool {
        false
    }

    fn grant(
        &self,
        snapshot: &JobSnapshot,
        fence: Arc<dyn JobAttemptFence>,
    ) -> Result<worker::job::JobFeatureGrant, JobServiceError>;
}

type OwnedExecutionReport = Arc<
    Mutex<Option<Result<worker::job::OwnedInternalJobExecution, worker::job::JobExecutionError>>>,
>;

struct Execution {
    cancel: watch::Sender<bool>,
    complete: watch::Receiver<Option<Result<(), String>>>,
    attempt_id: String,
    resource_key: Option<String>,
    max_concurrent: u16,
    effect_gate: Arc<Mutex<()>>,
    recovery_issue: Arc<Mutex<Option<JobServiceError>>>,
    task: tokio::task::JoinHandle<Result<(), JobServiceError>>,
    // Retain typed Worker failures and their cleanup resources until the owner can resolve them.
    _owned: OwnedExecutionReport,
}
struct Executions {
    closed: bool,
    // Set only after this service joined owned Tasks, checked durable obligations
    // and dropped its stores/adapters. Empty maps or closed slots are not proof.
    shutdown_confirmed: bool,
    jobs: HashMap<String, Execution>,
}

pub(crate) struct QuiescentJobClosure<'a> {
    executions: std::sync::MutexGuard<'a, Executions>,
}
impl QuiescentJobClosure<'_> {
    pub(crate) fn commit(&mut self) {
        self.executions.closed = true;
    }
}

/// Cloneable Feature-facing access to a Host-owned service. Cloning this handle
/// neither extends the Host's authority nor permits opening/recovering its store.
#[derive(Clone)]
pub struct StandaloneJobs {
    store: Arc<Mutex<Option<JobStore>>>,
    executions: Arc<Mutex<Executions>>,
    cwd: PathBuf,
    capacity: u16,
    domain: Arc<Mutex<Option<Arc<dyn JobDomainProvider>>>>,
    model_client: Arc<Mutex<Option<Box<dyn LlmClient>>>>,
    storage_recovered: Arc<Notify>,
    capacity_changed: Arc<Notify>,
    // Exact restore admissions and typed causes remain Host-owned, even if a
    // dispatcher fails. These are identities, never copies of immutable bodies.
    result_recovery: Arc<Mutex<HashMap<String, Arc<Mutex<Option<JobServiceError>>>>>>,
    // Process-local typed reports survive successful wait/capacity release.
    // They are diagnostics, never durable shutdown or replay authority.
    diagnostics: Arc<Mutex<HashMap<(String, String), OwnedExecutionReport>>>,
}

impl StandaloneJobs {
    /// Inspect a retained ordinary Worker failure for one exact attempt in this Host.
    /// The report survives `wait` and shutdown, but is not restart or cleanup evidence.
    pub fn with_execution_diagnostic<T>(
        &self,
        job_id: &str,
        attempt_id: &str,
        inspect: impl FnOnce(&worker::job::OwnedInternalJobExecution) -> T,
    ) -> Option<T> {
        let report = self
            .diagnostics
            .lock()
            .expect("Job diagnostics poisoned")
            .get(&(job_id.to_owned(), attempt_id.to_owned()))
            .cloned()?;
        let owned = report.lock().expect("Job owned execution poisoned");
        owned.as_ref()?.as_ref().ok().map(inspect)
    }

    /// Excludes request/start/result acknowledgement while a Subject Host checks its final
    /// saved obligations and releases ownership. Accepted results alone are insufficient:
    /// every spawned execution must have actually finished successful cleanup.
    pub(crate) fn reserve_quiescent_closure(
        &self,
    ) -> Result<QuiescentJobClosure<'_>, JobServiceError> {
        let executions = self.executions.lock().expect("Job executions poisoned");
        if executions.closed {
            return Err(JobServiceError::Closed);
        }
        if executions.jobs.values().any(|execution| {
            !execution.task.is_finished()
                || !matches!(execution.complete.borrow().as_ref(), Some(Ok(())))
        }) {
            return Err(JobServiceError::Cleanup(
                "delegated Job execution/cleanup remains running or unconfirmed".into(),
            ));
        }
        if !self
            .with_store(|store| store.execution_obligations())?
            .is_empty()
        {
            return Err(JobServiceError::ShutdownIncomplete);
        }
        Ok(QuiescentJobClosure { executions })
    }

    pub(crate) fn open(path: &Path, cwd: PathBuf) -> Result<Self, JobServiceError> {
        Ok(Self {
            store: Arc::new(Mutex::new(Some(JobStore::open(path)?))),
            executions: Arc::new(Mutex::new(Executions {
                closed: false,
                shutdown_confirmed: false,
                jobs: HashMap::new(),
            })),
            cwd,
            capacity: job::DEFAULT_MAX_CONCURRENT_JOBS,
            domain: Arc::default(),
            model_client: Arc::default(),
            storage_recovered: Arc::default(),
            capacity_changed: Arc::default(),
            result_recovery: Arc::default(),
            diagnostics: Arc::default(),
        })
    }

    /// Production Host entry: only a freshly revalidated Worker lease authorizes
    /// restart reconciliation. The lease must remain owned until actual shutdown.
    pub(crate) fn open_for_host(
        path: &Path,
        cwd: PathBuf,
        lease: &crate::store::StandaloneWorkerLease,
    ) -> Result<Self, JobServiceError> {
        let owner = lease
            .verify_current_id()
            .map_err(|error| JobServiceError::Configuration(error.to_string()))?;
        let jobs = Self::open(path, cwd)?;
        jobs.with_store_write(|store| {
            store.bind_host_lease(&owner.to_string())?;
            // Re-read after opening/migration, immediately before mutating attempts.
            let current = lease.verify_current_id().map_err(|_| {
                JobStoreError::InvalidTransition("Worker lease ownership changed during Job open")
            })?;
            if current != owner {
                return Err(JobStoreError::AttemptMismatch);
            }
            store.reconcile_host_restart().map(|_| ())
        })?;
        Ok(jobs)
    }

    pub(crate) fn reserved_result_attempts(
        &self,
    ) -> Result<Vec<(String, String)>, JobServiceError> {
        // Retain IDs, not a second queue of immutable bodies outside the owned
        // execution barrier while awaiting an earlier Job's cleanup.
        Ok(self
            .pending()?
            .into_iter()
            .filter(|saved| {
                saved.domain_grant.is_none()
                    && !saved.version_stopped
                    && saved.attempt.state == JobAttemptState::Reserved
            })
            .map(|saved| (saved.request.job_id, saved.attempt.attempt_id))
            .collect())
    }

    /// Storage exhaustion is owned by the same execution Task, not a new pending
    /// Job state. The diagnostic retains its typed source until a successful Host
    /// store write wakes reconciliation; no polling or model retry is scheduled.
    pub fn settlement_recovery_diagnostic(&self, job_id: &str) -> Option<String> {
        let executions = self.executions.lock().expect("Job executions poisoned");
        let execution = executions.jobs.get(job_id)?;
        let issue = execution
            .recovery_issue
            .lock()
            .expect("Job recovery issue poisoned");
        issue.as_ref().map(|original| {
            let completion = execution.complete.borrow();
            match completion.as_ref() {
                Some(Err(final_error)) => format!("{original}; settlement ended: {final_error}; next action: resolve the retained typed execution/cleanup failure, not another storage retry"),
                _ => original.to_string(),
            }
        })
    }

    /// Called by the existing Host/domain recovery hook when storage is available
    /// again. A failed probe does not schedule another budget; a successful write
    /// wakes only retained settlement Tasks, never starts another execution.
    pub(crate) fn recover_storage(&self) -> Result<(), JobServiceError> {
        let attempts: Vec<_> = {
            let executions = self.executions.lock().expect("Job executions poisoned");
            executions
                .jobs
                .iter()
                .filter_map(|(id, execution)| {
                    if execution.task.is_finished() {
                        return None;
                    }
                    execution
                        .recovery_issue
                        .lock()
                        .expect("Job recovery issue poisoned")
                        .is_some()
                        .then(|| (id.clone(), execution.attempt_id.clone()))
                })
                .collect()
        };
        for (id, attempt) in attempts {
            self.with_store_write(|store| store.probe_settlement_write(&id, &attempt))?;
        }
        Ok(())
    }

    pub(crate) fn bind_model_client(&self, client: Box<dyn LlmClient>) {
        *self
            .model_client
            .lock()
            .expect("Job model transport poisoned") = Some(client);
    }
    pub(crate) fn bind_domain(
        &self,
        provider: Arc<dyn JobDomainProvider>,
    ) -> Result<(), JobServiceError> {
        let mut domain = self.domain.lock().expect("Job domain poisoned");
        if domain.is_some() {
            return Err(JobServiceError::Configuration(
                "Job domain already bound".into(),
            ));
        }
        *domain = Some(provider);
        Ok(())
    }

    pub(crate) fn request_granted(
        &self,
        request: JobRequest,
        grant: serde_json::Value,
    ) -> Result<JobSnapshot, JobServiceError> {
        let executions = self.executions.lock().expect("Job executions poisoned");
        if executions.closed {
            return Err(JobServiceError::Closed);
        }
        self.with_store_write(|store| store.reserve_granted(request, Some(grant)))
    }

    fn with_store<T>(
        &self,
        operation: impl FnOnce(&mut JobStore) -> Result<T, JobStoreError>,
    ) -> Result<T, JobServiceError> {
        let mut slot = self.store.lock().expect("Job store poisoned");
        Ok(operation(slot.as_mut().ok_or(JobServiceError::Closed)?)?)
    }

    fn with_store_write<T>(
        &self,
        operation: impl FnOnce(&mut JobStore) -> Result<T, JobStoreError>,
    ) -> Result<T, JobServiceError> {
        let value = self.with_store(operation)?;
        // A successful commit in this same Host/store is a recovery event, unlike
        // an arbitrary read, UI click, timer, or a newly issued replacement Job.
        self.storage_recovered.notify_waiters();
        Ok(value)
    }

    /// Persist intent without starting a model. Exact resend returns the same
    /// current attempt, including after acknowledgement or cancellation.
    pub fn request(&self, request: JobRequest) -> Result<JobSnapshot, JobServiceError> {
        let executions = self.executions.lock().expect("Job executions poisoned");
        if executions.closed {
            return Err(JobServiceError::Closed);
        }
        Ok(self.with_store_write(|store| store.reserve(request))?)
    }

    /// Enumerate historical domain snapshots for a Session, including retired input receipts.
    pub fn list_session_snapshots(
        &self,
        session_id: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<JobSnapshot>, JobServiceError> {
        self.with_store(|store| store.list_session_snapshots(session_id, after, limit))
    }

    /// Retire body only. The caller fences domain effects/delivered snapshots before erasure.
    /// Normal reclamation also requires that execution cleanup has finished.
    pub fn reclaim_input(
        &self,
        job_id: &str,
        expected_input_digest: &str,
        reason: JobInputRetirementReason,
    ) -> Result<JobSnapshot, JobServiceError> {
        let executions = self.executions.lock().expect("Job executions poisoned");
        if reason == JobInputRetirementReason::Completed
            && executions
                .jobs
                .get(job_id)
                .is_some_and(|execution| !execution.task.is_finished())
        {
            return Err(JobServiceError::Capacity);
        }
        self.with_store_write(|store| store.reclaim_input(job_id, expected_input_digest, reason))
    }

    pub fn get(&self, job_id: &str) -> Result<JobSnapshot, JobServiceError> {
        Ok(self.with_store(|store| store.get(job_id))?)
    }

    /// Pending attempts survive shutdown. The consumer chooses when to resume
    /// them; opening a Host never implicitly replays unknown side effects.
    pub fn pending(&self) -> Result<Vec<JobSnapshot>, JobServiceError> {
        Ok(self.with_store(|store| store.pending())?)
    }

    pub fn unacknowledged_results(&self) -> Result<Vec<JobSnapshot>, JobServiceError> {
        self.with_store(|store| store.unacknowledged())
    }

    pub(crate) fn unreleased_granted(&self) -> Result<Vec<JobSnapshot>, JobServiceError> {
        self.with_store(|store| store.unreleased_granted())
    }

    /// Release a failed granted attempt's domain reservation only after authoritative
    /// reconciliation and independent execution cleanup. Never a generic retry.
    pub(crate) fn reconcile_granted_attempt(
        &self,
        job_id: &str,
        attempt_id: &str,
        domain_receipt: &str,
    ) -> Result<(), JobServiceError> {
        let executions = self.executions.lock().expect("Job executions poisoned");
        if executions.closed {
            return Err(JobServiceError::Closed);
        }
        if executions.jobs.get(job_id).is_some_and(|run| {
            !run.task.is_finished() || !matches!(run.complete.borrow().as_ref(), Some(Ok(())))
        }) {
            return Err(JobServiceError::Cleanup(
                "domain reconciliation requires observed execution cleanup".into(),
            ));
        }
        self.with_store_write(|store| {
            store.reconcile_granted_attempt(job_id, attempt_id, domain_receipt)
        })
    }

    pub(crate) fn resume_granted(
        &self,
        job_id: &str,
        attempt_id: &str,
    ) -> Result<JobSnapshot, JobServiceError> {
        let executions = self.executions.lock().expect("Job executions poisoned");
        if executions.closed {
            return Err(JobServiceError::Closed);
        }
        if executions.jobs.get(job_id).is_some_and(|run| {
            !run.task.is_finished() || !matches!(run.complete.borrow().as_ref(), Some(Ok(())))
        }) {
            return Err(JobServiceError::ShutdownIncomplete);
        }
        self.with_store_write(|store| store.resume_granted(job_id, attempt_id))
    }

    pub(crate) fn accept_reconciled(
        &self,
        submission: &JobResultSubmission,
    ) -> Result<JobSnapshot, JobServiceError> {
        let executions = self.executions.lock().expect("Job executions poisoned");
        if executions.closed {
            return Err(JobServiceError::Closed);
        }
        let _effects = executions
            .jobs
            .get(&submission.job_id)
            .map(|run| run.effect_gate.lock().expect("Job effects poisoned"));
        self.with_store_write(|store| store.accept_reconciled(submission))
    }

    /// Explicit new result-only attempt, bounded by immutable intent limits and
    /// independent shutdown proof. Granted Jobs use domain-owned recovery instead.
    pub fn retry(&self, job_id: &str) -> Result<JobSnapshot, JobServiceError> {
        let executions = self.executions.lock().expect("Job executions poisoned");
        if executions.closed {
            return Err(JobServiceError::Closed);
        }
        if executions.jobs.get(job_id).is_some_and(|run| {
            !run.task.is_finished() || !matches!(run.complete.borrow().as_ref(), Some(Ok(())))
        }) {
            return Err(JobServiceError::Capacity);
        }
        Ok(self.with_store_write(|store| store.retry(job_id))?)
    }

    /// Successful model processing and consumer postprocessing have independent
    /// lifetimes. A failed delivery simply leaves this acknowledgement pending.
    pub fn acknowledge(&self, job_id: &str) -> Result<JobSnapshot, JobServiceError> {
        let executions = self.executions.lock().expect("Job executions poisoned");
        if executions.closed {
            return Err(JobServiceError::Closed);
        }
        Ok(self.with_store_write(|store| store.ack(job_id))?)
    }

    pub fn cancel(&self, job_id: &str) -> Result<JobSnapshot, JobServiceError> {
        let executions = self.executions.lock().expect("Job executions poisoned");
        if executions.closed {
            return Err(JobServiceError::Closed);
        }
        let _effect = executions
            .jobs
            .get(job_id)
            .map(|run| run.effect_gate.lock().expect("Job effects poisoned"));
        let snapshot = self.with_store_write(|store| store.cancel(job_id))?;
        if let Some(execution) = executions.jobs.get(job_id) {
            execution.cancel.send_replace(true);
        }
        Ok(snapshot)
    }

    pub async fn start(&self, job_id: &str) -> Result<(), JobServiceError> {
        self.start_with_optional_model_client(job_id, None)
    }

    /// Injectable model transport for embedders and scripted execution tests.
    /// The requested registry Profile still resolves/validates normally; the
    /// transport grants no filesystem, Feature or result-submission authority.
    pub async fn start_with_model_client<C>(
        &self,
        job_id: &str,
        client: C,
    ) -> Result<(), JobServiceError>
    where
        C: LlmClient + 'static,
    {
        self.start_with_optional_model_client(job_id, Some(Box::new(client)))
    }

    fn start_with_optional_model_client(
        &self,
        job_id: &str,
        client: Option<Box<dyn LlmClient>>,
    ) -> Result<(), JobServiceError> {
        self.start_with_admission(job_id, client, None)
    }

    fn start_with_admission(
        &self,
        job_id: &str,
        client: Option<Box<dyn LlmClient>>,
        admission: Option<(&JobSnapshot, tokio::time::Instant)>,
    ) -> Result<(), JobServiceError> {
        // Preparation owns copies of the immutable input and domain adapter too. Exclude
        // owner shutdown before creating them, not only before publishing the task.
        let mut executions = self.executions.lock().expect("Job executions poisoned");
        if executions.closed {
            return Err(JobServiceError::Closed);
        }
        let client = client.or_else(|| {
            self.model_client
                .lock()
                .expect("Job model transport poisoned")
                .as_ref()
                .map(|c| c.clone_boxed())
        });
        let snapshot = self.get(job_id)?;
        if let Some((expected, _)) = admission {
            if snapshot.attempt.attempt_id != expected.attempt.attempt_id
                || snapshot.input_digest != expected.input_digest
                || snapshot.domain_grant != expected.domain_grant
                || snapshot.domain_grant.is_none()
                || !snapshot.executions_released()
            {
                return Err(JobStoreError::AttemptMismatch.into());
            }
        }
        snapshot.load_executable_request()?;
        if snapshot.version_stopped
            || snapshot.state != JobState::Pending
            || snapshot.attempt.state != JobAttemptState::Reserved
        {
            return Err(JobStoreError::InvalidTransition(
                "start requires an unstarted current attempt",
            )
            .into());
        }
        let effect_gate = Arc::new(Mutex::new(()));
        let revoked = Arc::new(AtomicBool::new(false));
        let deadline = admission.map_or_else(
            || {
                tokio::time::Instant::now()
                    + Duration::from_secs(u64::from(snapshot.request.limits.timeout_seconds))
            },
            |(_, deadline)| deadline,
        );
        let timeout = deadline.saturating_duration_since(tokio::time::Instant::now());
        if timeout.is_zero() {
            return Err(JobServiceError::Configuration(
                "consolidation_admission_deadline_exceeded".into(),
            ));
        }
        let fence: Arc<dyn JobAttemptFence> = Arc::new(BoundAttemptFence {
            revoked: revoked.clone(),
            effect_gate: effect_gate.clone(),
            store: self.store.clone(),
            job_id: job_id.into(),
            attempt_id: snapshot.attempt.attempt_id.clone(),
            input_digest: snapshot.input_digest.clone(),
            deadline,
        });
        let domain_fenced = snapshot.domain_grant.is_none()
            || self
                .domain
                .lock()
                .expect("Job domain poisoned")
                .as_ref()
                .is_some_and(|provider| provider.callbacks_use_attempt_fence());
        let grant = match (
            &snapshot.domain_grant,
            self.domain.lock().expect("Job domain poisoned").as_ref(),
        ) {
            (Some(_), Some(provider)) => provider.grant(&snapshot, fence)?,
            (Some(_), None) => {
                return Err(JobServiceError::Configuration(
                    "persisted Job requires an unavailable explicit domain capability".into(),
                ));
            }
            (None, _) => worker::job::JobFeatureGrant::default(),
        };
        let prepared =
            worker::job::prepare_job_with_grant(&snapshot.request, &self.cwd, client, grant)
                .map_err(|error| JobServiceError::Configuration(error.to_string()))?;
        executions.jobs.retain(|_, run| {
            !(run.task.is_finished() && matches!(run.complete.borrow().as_ref(), Some(Ok(()))))
        });
        // Count live resources, not only result states: acceptance can precede
        // the Internal Worker's cleanup and must not free a concurrency slot.
        let live = || {
            executions.jobs.values().filter(|run| {
                !run.task.is_finished() || !matches!(run.complete.borrow().as_ref(), Some(Ok(())))
            })
        };
        let count = live().count();
        let active_limit = live()
            .map(|run| run.max_concurrent)
            .min()
            .unwrap_or(self.capacity);
        if count >= usize::from(self.capacity.min(active_limit))
            || count >= usize::from(snapshot.request.limits.max_concurrent_jobs)
            || executions
                .jobs
                .get(job_id)
                .is_some_and(|run| !run.task.is_finished())
        {
            return Err(JobServiceError::Capacity);
        }
        let resource_key = snapshot.request.serialization_key.clone();
        if let Some(key) = &resource_key {
            if executions.jobs.values().any(|run| {
                run.resource_key.as_ref() == Some(key)
                    && (!run.task.is_finished()
                        || !matches!(run.complete.borrow().as_ref(), Some(Ok(()))))
            }) {
                return Err(JobServiceError::Capacity);
            }
        }
        // Retain errors from finished tasks until a caller observes them rather
        // than silently swallowing store/cleanup failures.
        if let Some(previous) = executions.jobs.get(job_id) {
            if !previous.task.is_finished()
                || !matches!(previous.complete.borrow().as_ref(), Some(Ok(())))
            {
                return Err(JobServiceError::Capacity);
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(JobServiceError::Configuration(
                "consolidation_admission_deadline_exceeded during preparation".into(),
            ));
        }
        let snapshot = self.with_store_write(|store| {
            store.dispatch(job_id, &snapshot.attempt.attempt_id, self.capacity)
        })?;
        let store = self.store.clone();
        let id = job_id.to_string();
        let attempt_id = snapshot.attempt.attempt_id.clone();
        let sink = Arc::new(BoundResultSink {
            storage_recovered: self.storage_recovered.clone(),
            revoked: revoked.clone(),
            store: store.clone(),
            job_id: id.clone(),
            attempt_id: attempt_id.clone(),
            input_digest: snapshot.input_digest.clone(),
            deadline,
        });
        let (cancel, mut cancellation) = watch::channel(false);
        let (complete_tx, complete) = watch::channel(None);
        let execution_attempt = attempt_id.clone();
        let execution_gate = effect_gate.clone();
        let ownership = snapshot
            .executions
            .iter()
            .find(|run| run.attempt_id == attempt_id)
            .cloned()
            .ok_or(JobStoreError::AttemptMismatch)?;
        let expected_digest = snapshot.input_digest.clone();
        let owned = Arc::new(Mutex::new(None));
        let retained = owned.clone();
        let recovery_issue = Arc::new(Mutex::new(None));
        let task_issue = recovery_issue.clone();
        let storage_recovered = self.storage_recovered.clone();
        let capacity_changed = self.capacity_changed.clone();
        let diagnostics = self.diagnostics.clone();
        let task = tokio::spawn(async move {
            let (sender_tx, mut sender_rx) = tokio::sync::oneshot::channel();
            let run = prepared.run_owned(attempt_id.clone(), sink, move |sender| {
                let _ = sender_tx.send(sender);
            });
            tokio::pin!(run);
            let mut stop_reason = None;
            let result = tokio::select! {
                result = &mut run => result,
                reason = async {
                    tokio::select! {
                        _ = tokio::time::sleep_until(deadline) => "timeout",
                        _ = cancellation.wait_for(|cancelled| *cancelled) => "Host cancellation/shutdown",
                    }
                } => {
                    // The persisted terminal state revokes future domain calls, while the gate
                    // joins any synchronous call already inside the Host boundary.
                    {
                        let _effects = execution_gate.lock().expect("Job effects poisoned");
                        revoked.store(true, Ordering::Release);
                        stop_reason = Some(reason);
                        // A transient store failure does not cancel ownership of settlement.
                        // The local revocation and effect gate still fence callbacks;
                        // retry the exact terminal write after the owned runner returns.
                        let _ = finish_if_pending(&store, &id, &attempt_id, JobAttemptState::Failed, reason);
                    }
                    if let Ok(Ok(sender)) = tokio::time::timeout(Duration::from_secs(1), &mut sender_rx).await {
                        let _ = sender.try_send(());
                    }
                    match tokio::time::timeout(Duration::from_secs(2), &mut run).await {
                        Ok(result) => result,
                        Err(_) => {
                            // Report the overdue cancellation, but never drop the owned future.
                            // A later return can supply real shutdown evidence; until then both
                            // the task and durable execution reservation remain owned.
                            eprintln!("standalone Job {id} cancellation is still running; ownership retained");
                            // Keep waiters attached to this recovery owner. An interim
                            // cleanup error would stop the Journal dispatcher before a
                            // later owned return/storage recovery can settle this Job.
                            run.await
                        }
                    }
                }
            };
            let allow_unconfirmed = stop_reason.is_none() && tokio::time::Instant::now() < deadline;
            *retained.lock().expect("Job owned execution poisoned") = Some(result);
            let outcome = recover_owned_settlement(
                &storage_recovered,
                &task_issue,
                || {
                    let _effects = execution_gate.lock().expect("Job effects poisoned");
                    revoked.store(true, Ordering::Release);
                    let owned = retained.lock().expect("Job owned execution poisoned");
                    settle_owned_execution(
                        &store,
                        &id,
                        &attempt_id,
                        &expected_digest,
                        &ownership,
                        domain_fenced,
                        stop_reason,
                        allow_unconfirmed,
                        owned.as_ref().expect("owned execution retained"),
                    )
                },
                || {
                    let mut slot = store.lock().expect("Job store poisoned");
                    let store = slot.as_mut().ok_or(JobServiceError::Closed)?;
                    store.probe_settlement_write(&id, &attempt_id)?;
                    Ok(())
                },
            )
            .await;
            if outcome.is_ok()
                && retained
                    .lock()
                    .expect("Job owned execution poisoned")
                    .as_ref()
                    .and_then(|result| result.as_ref().ok())
                    .is_some_and(|execution| execution.worker_failure().is_some())
            {
                diagnostics
                    .lock()
                    .expect("Job diagnostics poisoned")
                    .insert((id.clone(), attempt_id.clone()), retained.clone());
            }
            complete_tx.send_replace(Some(
                outcome.as_ref().map(|_| ()).map_err(ToString::to_string),
            ));
            // A wake is only an observation: recovery joins the successful task
            // return before rechecking capacity. An error does not free its slot.
            capacity_changed.notify_waiters();
            outcome
        });
        executions.jobs.insert(
            job_id.to_string(),
            Execution {
                cancel,
                complete,
                task,
                _owned: owned,
                effect_gate,
                recovery_issue,
                attempt_id: execution_attempt,
                resource_key,
                max_concurrent: snapshot.request.limits.max_concurrent_jobs,
            },
        );
        Ok(())
    }

    /// Observe execution/cleanup, not success: read the durable result separately.
    pub async fn wait(&self, job_id: &str) -> Result<JobSnapshot, JobServiceError> {
        let execution = self
            .executions
            .lock()
            .expect("Job executions poisoned")
            .jobs
            .get(job_id)
            .map(|run| (run.attempt_id.clone(), run.complete.clone()));
        if let Some((attempt, mut completion)) = execution {
            let result = completion
                .wait_for(Option::is_some)
                .await
                .map_err(|error| JobServiceError::Cleanup(error.to_string()))?
                .clone()
                .expect("completion");
            result.map_err(JobServiceError::Cleanup)?;
            // Completion is published immediately before the task returns. Do not remove
            // its ownership record until the runtime confirms that return (and future drop).
            while self
                .executions
                .lock()
                .expect("Job executions poisoned")
                .jobs
                .get(job_id)
                .is_some_and(|run| run.attempt_id == attempt && !run.task.is_finished())
            {
                tokio::task::yield_now().await;
            }
            let mut executions = self.executions.lock().expect("Job executions poisoned");
            if executions
                .jobs
                .get(job_id)
                .is_some_and(|run| run.attempt_id == attempt)
            {
                executions.jobs.remove(job_id);
                self.capacity_changed.notify_waiters();
            }
        }
        self.get(job_id)
    }

    pub(crate) fn close(&self) {
        let mut executions = self.executions.lock().expect("Job executions poisoned");
        executions.closed = true;
        self.capacity_changed.notify_waiters();
        for execution in executions.jobs.values() {
            execution.cancel.send_replace(true);
        }
    }

    /// Initiate owner shutdown without blocking a synchronous caller. Success confirms all
    /// local executions have returned, including request/history and Feature cleanup. An
    /// incomplete/error result retains the service resources and must retain the Host lease.
    pub(crate) fn try_shutdown(&self) -> Result<(), JobServiceError> {
        self.close();
        self.recover_storage()?;
        let mut executions = self.executions.lock().expect("Job executions poisoned");
        if executions.shutdown_confirmed {
            return Ok(());
        }
        for execution in executions.jobs.values() {
            if !execution.task.is_finished() {
                return Err(JobServiceError::ShutdownIncomplete);
            }
            match execution.complete.borrow().as_ref() {
                Some(Ok(())) => {}
                Some(Err(error)) => return Err(JobServiceError::Cleanup(error.clone())),
                None => {
                    return Err(JobServiceError::Cleanup(
                        "Internal Worker ended without cleanup confirmation".into(),
                    ));
                }
            }
        }
        if !self
            .with_store(|store| store.execution_obligations())?
            .is_empty()
        {
            return Err(JobServiceError::ShutdownIncomplete);
        }
        executions.jobs.clear();
        // Retained external handles share these slots; they cannot retain a live store,
        // provider or model client after successful owner shutdown.
        self.store.lock().expect("Job store poisoned").take();
        self.domain.lock().expect("Job domain poisoned").take();
        self.model_client
            .lock()
            .expect("Job model transport poisoned")
            .take();
        executions.shutdown_confirmed = true;
        Ok(())
    }

    pub(crate) async fn shutdown(&self) -> Result<(), JobServiceError> {
        self.close();
        self.recover_storage()?;
        let completions: Vec<_> = self
            .executions
            .lock()
            .expect("Job executions poisoned")
            .jobs
            .values()
            .map(|execution| execution.complete.clone())
            .collect();
        // Never take execution ownership out of the shared service across an await: a
        // cancelled shutdown future or a concurrent close must still see the barrier.
        for mut completion in completions {
            completion
                .wait_for(Option::is_some)
                .await
                .map_err(|error| JobServiceError::Cleanup(error.to_string()))?;
        }
        loop {
            match self.try_shutdown() {
                // Only the task's final return follows its completion notification.
                Err(JobServiceError::ShutdownIncomplete) => {
                    let has_running_task = self
                        .executions
                        .lock()
                        .expect("Job executions poisoned")
                        .jobs
                        .values()
                        .any(|execution| !execution.task.is_finished());
                    if !has_running_task {
                        // Saved obligations cannot be discharged by yielding or an absent map.
                        return Err(JobServiceError::ShutdownIncomplete);
                    }
                    tokio::task::yield_now().await;
                }
                result => return result,
            }
        }
    }
}

// Retry only persistence, never dispatch, domain operations, or Worker shutdown.
// The owning execution Task retains the exact typed result throughout this finite
// recovery window, including when all observers drop their wait futures.
pub(crate) async fn recover_settlement(
    mut settle: impl FnMut() -> Result<(), JobServiceError>,
) -> Result<(), JobServiceError> {
    const DELAYS_MS: [u64; 5] = [100, 200, 400, 800, 1600];
    for pass in 0..=DELAYS_MS.len() {
        match settle() {
            Ok(()) => return Ok(()),
            Err(error) if transient_settlement_storage_error(&error) => {
                let Some(delay) = DELAYS_MS.get(pass) else {
                    return Err(JobServiceError::SettlementRecoveryExhausted {
                        attempts: pass + 1,
                        source: Box::new(error),
                    });
                };
                tokio::time::sleep(Duration::from_millis(*delay)).await;
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("finite settlement retry returns on its final pass")
}

pub(crate) async fn recover_owned_settlement(
    storage_recovered: &Notify,
    issue: &Mutex<Option<JobServiceError>>,
    mut settle: impl FnMut() -> Result<(), JobServiceError>,
    mut probe: impl FnMut() -> Result<(), JobServiceError>,
) -> Result<(), JobServiceError> {
    loop {
        match recover_settlement(&mut settle).await {
            Err(error @ JobServiceError::SettlementRecoveryExhausted { .. }) => {
                {
                    let mut saved = issue.lock().expect("Job recovery issue poisoned");
                    if saved.is_none() {
                        *saved = Some(error);
                    }
                }
                // The execution owner also owns recovery detection; no user action,
                // other Job or detached monitor is needed. A timer only schedules an
                // exact-owner storage probe, NEVER another settlement budget or effect.
                // Only a committed probe permits settlement to resume. Permanent
                // errors and stale ownership stop probing with the typed cause intact.
                let mut delay = Duration::from_secs(1);
                loop {
                    tokio::select! {
                        _ = storage_recovered.notified() => {},
                        _ = tokio::time::sleep(delay) => {},
                    }
                    match probe() {
                        Ok(()) => break,
                        Err(error) if transient_settlement_storage_error(&error) => {
                            delay = (delay * 2).min(Duration::from_secs(30));
                        }
                        Err(error) => return Err(error),
                    }
                }
            }
            result => {
                if result.is_ok() {
                    issue.lock().expect("Job recovery issue poisoned").take();
                }
                return result;
            }
        }
    }
}

fn transient_settlement_storage_error(error: &JobServiceError) -> bool {
    matches!(error,
        JobServiceError::Store(JobStoreError::Sqlite(rusqlite::Error::SqliteFailure(error, _)))
        if matches!(error.code,
            rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
            | rusqlite::ErrorCode::SystemIoFailure | rusqlite::ErrorCode::DiskFull
            | rusqlite::ErrorCode::CannotOpen))
}

fn settle_owned_execution(
    store: &Mutex<Option<JobStore>>,
    id: &str,
    attempt: &str,
    input_digest: &str,
    ownership: &JobExecutionOwnership,
    domain_fenced: bool,
    stop_reason: Option<&str>,
    allow_unconfirmed: bool,
    result: &Result<worker::job::OwnedInternalJobExecution, worker::job::JobExecutionError>,
) -> Result<(), JobServiceError> {
    let detail = match result {
        Ok(execution) => execution
            .result
            .as_ref()
            .err()
            .map(ToString::to_string)
            .unwrap_or_else(|| "Worker finished without an accepted structured result".into()),
        Err(error) => error.to_string(),
    };
    // Validate the saved owner before any terminal write. A stale result/cleanup
    // must not finish a later attempt, even if that later attempt is still pending.
    let mut slot = store.lock().expect("Job store poisoned");
    let store = slot.as_mut().ok_or(JobServiceError::Closed)?;
    store.check_execution_binding(id, attempt, input_digest, ownership)?;
    let snapshot = store.get(id)?;
    if let Ok(execution) = result {
        if execution.job_id != id
            || execution.attempt_id != attempt
            || execution.input_digest != input_digest
            || execution.run_id.to_string() != ownership.run_id
        {
            return Err(JobStoreError::AttemptMismatch.into());
        }
        let submission = execution
            .result
            .as_ref()
            .ok()
            .map(|accepted| &accepted.submission)
            .or_else(|| {
                (allow_unconfirmed || snapshot.state == JobState::Completed)
                    .then(|| execution.unconfirmed_submission())
                    .flatten()
            });
        if let Some(submission) = submission {
            if submission.job_id != id
                || submission.attempt_id != attempt
                || submission.input_digest != input_digest
            {
                return Err(JobStoreError::AttemptMismatch.into());
            }
            // Reconcile only the sealed, already-finalized payload. In particular,
            // never rerun a non-idempotent domain finalizer after sink response loss.
            store.accept(submission)?;
        }
    }
    let snapshot = store.get(id)?;
    if snapshot.state == JobState::Pending {
        store.finish(
            id,
            attempt,
            JobAttemptState::Failed,
            Some(stop_reason.unwrap_or(&detail)),
        )?;
    }
    let execution = result
        .as_ref()
        .map_err(|error| JobServiceError::Cleanup(error.to_string()))?;
    if !domain_fenced
        || !execution.feature_shutdown_complete()
        || execution.requires_worker_cleanup_reconciliation()
    {
        return Err(JobServiceError::Cleanup(format!(
            "owned Worker shutdown unconfirmed ({} Feature shutdown failures); typed execution retained",
            execution.feature_shutdown_failures().len()
        )));
    }
    // The caller holds the domain effect gate, run_owned has joined/revoked every
    // result/finalizer clone, and terminal state rejects all future domain entries.
    store.confirm_execution_cleanup(id, ownership, &format!("owned-run:{}", execution.run_id))?;
    Ok(())
}

fn finish_if_pending(
    store: &Mutex<Option<JobStore>>,
    id: &str,
    attempt: &str,
    state: JobAttemptState,
    detail: &str,
) -> Result<(), JobServiceError> {
    let mut slot = store.lock().expect("Job store poisoned");
    let store = slot.as_mut().ok_or(JobServiceError::Closed)?;
    let snapshot = store.get(id)?;
    if snapshot.state == JobState::Pending {
        store.finish(id, attempt, state, Some(detail))?;
    }
    Ok(())
}

struct BoundResultSink {
    storage_recovered: Arc<Notify>,
    revoked: Arc<AtomicBool>,
    store: Arc<Mutex<Option<JobStore>>>,
    job_id: String,
    attempt_id: String,
    input_digest: String,
    deadline: tokio::time::Instant,
}
impl worker::job::JobResultSink for BoundResultSink {
    fn submit(&self, submission: JobResultSubmission) -> Result<(), String> {
        if submission.job_id != self.job_id
            || submission.attempt_id != self.attempt_id
            || submission.input_digest != self.input_digest
        {
            return Err("Job result capability binding mismatch".into());
        }
        let mut slot = self
            .store
            .lock()
            .map_err(|_| "Job store poisoned".to_string())?;
        let store = slot.as_mut().ok_or_else(|| "Job Host closed".to_string())?;
        let snapshot = store.get(&self.job_id).map_err(|error| error.to_string())?;
        if (self.revoked.load(Ordering::Acquire) || tokio::time::Instant::now() >= self.deadline)
            && snapshot.state != JobState::Completed
        {
            return Err("Job result capability revoked or deadline expired".into());
        }
        store
            .accept(&submission)
            .map_err(|error| error.to_string())?;
        self.storage_recovered.notify_waiters();
        Ok(())
    }
}

struct BoundAttemptFence {
    revoked: Arc<AtomicBool>,
    effect_gate: Arc<Mutex<()>>,
    store: Arc<Mutex<Option<JobStore>>>,
    job_id: String,
    attempt_id: String,
    input_digest: String,
    deadline: tokio::time::Instant,
}
impl JobAttemptFence for BoundAttemptFence {
    fn enter(&self) -> Result<Box<dyn JobEffectGuard + '_>, String> {
        let guard = self
            .effect_gate
            .lock()
            .map_err(|_| "Job effects poisoned".to_string())?;
        self.ensure_live()?;
        Ok(Box::new(guard))
    }
    fn ensure_live(&self) -> Result<(), String> {
        if self.revoked.load(Ordering::Acquire) {
            return Err("Job domain capability revoked".into());
        }
        if tokio::time::Instant::now() >= self.deadline {
            return Err("Job domain deadline expired".into());
        }
        let slot = self.store.lock().map_err(|_| "Job store poisoned")?;
        let snapshot = slot
            .as_ref()
            .ok_or("Job Host closed")?
            .get(&self.job_id)
            .map_err(|e| e.to_string())?;
        if snapshot.state != JobState::Pending
            || snapshot.attempt.state != JobAttemptState::Dispatched
            || snapshot.attempt.attempt_id != self.attempt_id
            || snapshot.input_digest != self.input_digest
            || snapshot.input_disposition != JobInputDisposition::Available
        {
            return Err("Job domain grant is not bound to the current live attempt".into());
        }
        Ok(())
    }
}
