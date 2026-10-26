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
//! Timeout/shutdown makes unaccepted attempts Unknown before cancellation. Unconfirmed cleanup
//! keeps Host leases held. Clones cannot reopen the store after service closure or carry
//! authority over to another Host.
#[cfg(test)]
mod tests;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agen::llm_client::LlmClient;
use job::{JobRequest, JobResultSubmission};
use tokio::sync::watch;

use crate::job_store::JobStore;
pub use crate::job_store::{
    JobAttempt, JobAttemptState, JobInputDisposition, JobInputRetirementReason, JobSnapshot,
    JobState, JobStoreError,
};

#[derive(Debug, thiserror::Error)]
pub enum JobServiceError {
    #[error(transparent)]
    Store(#[from] JobStoreError),
    #[error("standalone Job execution requirements are unavailable: {0}")]
    Configuration(String),
    #[error("the standalone Job Host is shutting down")]
    Closed,
    #[error("standalone Job execution capacity is exhausted")]
    Capacity,
    #[error("standalone Job cleanup failed: {0}")]
    Cleanup(String),
    #[error("standalone Job shutdown is incomplete; execution resources are still owned")]
    ShutdownIncomplete,
}

pub(crate) trait JobAttemptFence: Send + Sync {
    fn ensure_live(&self) -> Result<(), String>;
}
pub(crate) trait JobDomainProvider: Send + Sync {
    fn grant(
        &self,
        snapshot: &JobSnapshot,
        fence: Arc<dyn JobAttemptFence>,
    ) -> Result<worker::job::JobFeatureGrant, String>;
}

struct Execution {
    cancel: watch::Sender<bool>,
    complete: watch::Receiver<Option<Result<(), String>>>,
    attempt_id: String,
    resource_key: Option<String>,
    max_concurrent: u16,
    task: tokio::task::JoinHandle<Result<(), JobServiceError>>,
}
struct Executions {
    closed: bool,
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
}

impl StandaloneJobs {
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
        Ok(QuiescentJobClosure { executions })
    }

    pub(crate) fn open(path: &Path, cwd: PathBuf) -> Result<Self, JobServiceError> {
        Ok(Self {
            store: Arc::new(Mutex::new(Some(JobStore::open(path)?))),
            executions: Arc::new(Mutex::new(Executions {
                closed: false,
                jobs: HashMap::new(),
            })),
            cwd,
            capacity: job::DEFAULT_MAX_CONCURRENT_JOBS,
            domain: Arc::default(),
            model_client: Arc::default(),
        })
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
        self.with_store(|store| store.reserve_granted(request, Some(grant)))
    }

    fn with_store<T>(
        &self,
        operation: impl FnOnce(&mut JobStore) -> Result<T, JobStoreError>,
    ) -> Result<T, JobServiceError> {
        let mut slot = self.store.lock().expect("Job store poisoned");
        Ok(operation(slot.as_mut().ok_or(JobServiceError::Closed)?)?)
    }

    /// Persist intent without starting a model. Exact resend returns the same
    /// current attempt, including after acknowledgement or cancellation.
    pub fn request(&self, request: JobRequest) -> Result<JobSnapshot, JobServiceError> {
        let executions = self.executions.lock().expect("Job executions poisoned");
        if executions.closed {
            return Err(JobServiceError::Closed);
        }
        Ok(self.with_store(|store| store.reserve(request))?)
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
        self.with_store(|store| store.reclaim_input(job_id, expected_input_digest, reason))
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

    pub(crate) fn unknown_granted(&self) -> Result<Vec<JobSnapshot>, JobServiceError> {
        self.with_store(|store| store.unknown_granted())
    }

    /// Release only an Unknown attempt's serialization reservation after the bound domain
    /// has reconciled its receipts and durably fenced further effects. Never a generic retry.
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
        if executions.jobs.contains_key(job_id) {
            return Err(JobServiceError::Cleanup(
                "domain reconciliation requires observed execution cleanup".into(),
            ));
        }
        self.with_store(|store| store.reconcile_granted_attempt(job_id, attempt_id, domain_receipt))
    }

    /// Explicit new attempt, bounded by immutable intent limits. Unknown
    /// outcomes cannot be retried without domain reconciliation/new intent.
    pub fn retry(&self, job_id: &str) -> Result<JobSnapshot, JobServiceError> {
        let executions = self.executions.lock().expect("Job executions poisoned");
        if executions.closed {
            return Err(JobServiceError::Closed);
        }
        if executions
            .jobs
            .get(job_id)
            .is_some_and(|run| !run.task.is_finished())
        {
            return Err(JobServiceError::Capacity);
        }
        Ok(self.with_store(|store| store.retry(job_id))?)
    }

    /// Successful model processing and consumer postprocessing have independent
    /// lifetimes. A failed delivery simply leaves this acknowledgement pending.
    pub fn acknowledge(&self, job_id: &str) -> Result<JobSnapshot, JobServiceError> {
        let executions = self.executions.lock().expect("Job executions poisoned");
        if executions.closed {
            return Err(JobServiceError::Closed);
        }
        Ok(self.with_store(|store| store.ack(job_id))?)
    }

    pub fn cancel(&self, job_id: &str) -> Result<JobSnapshot, JobServiceError> {
        let executions = self.executions.lock().expect("Job executions poisoned");
        if executions.closed {
            return Err(JobServiceError::Closed);
        }
        let snapshot = self.with_store(|store| store.cancel(job_id))?;
        if let Some(execution) = executions.jobs.get(job_id) {
            execution.cancel.send_replace(true);
        }
        Ok(snapshot)
    }

    pub async fn start(&self, job_id: &str) -> Result<(), JobServiceError> {
        self.start_with_optional_model_client(job_id, None).await
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
            .await
    }

    async fn start_with_optional_model_client(
        &self,
        job_id: &str,
        client: Option<Box<dyn LlmClient>>,
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
        snapshot.load_executable_request()?;
        let timeout = Duration::from_secs(u64::from(snapshot.request.limits.timeout_seconds));
        let deadline = tokio::time::Instant::now() + timeout;
        let fence: Arc<dyn JobAttemptFence> = Arc::new(BoundAttemptFence {
            store: self.store.clone(),
            job_id: job_id.into(),
            attempt_id: snapshot.attempt.attempt_id.clone(),
            input_digest: snapshot.input_digest.clone(),
            deadline,
        });
        let grant = match (
            &snapshot.domain_grant,
            self.domain.lock().expect("Job domain poisoned").as_ref(),
        ) {
            (Some(_), Some(provider)) => provider
                .grant(&snapshot, fence)
                .map_err(JobServiceError::Configuration)?,
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
            executions
                .jobs
                .values()
                .filter(|run| !run.task.is_finished())
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
            if executions
                .jobs
                .values()
                .any(|run| !run.task.is_finished() && run.resource_key.as_ref() == Some(key))
            {
                return Err(JobServiceError::Capacity);
            }
        }
        // Retain errors from finished tasks until a caller observes them rather
        // than silently swallowing store/cleanup failures.
        if let Some(previous) = executions.jobs.get(job_id) {
            if !previous.task.is_finished() {
                return Err(JobServiceError::Capacity);
            }
        }
        let snapshot = self.with_store(|store| {
            store.dispatch(job_id, &snapshot.attempt.attempt_id, self.capacity)
        })?;
        let store = self.store.clone();
        let id = job_id.to_string();
        let attempt_id = snapshot.attempt.attempt_id.clone();
        let sink = Arc::new(BoundResultSink {
            store: store.clone(),
            job_id: id.clone(),
            attempt_id: attempt_id.clone(),
            input_digest: snapshot.input_digest.clone(),
            deadline,
        });
        let (cancel, mut cancellation) = watch::channel(false);
        let (complete_tx, complete) = watch::channel(None);
        let execution_attempt = attempt_id.clone();
        let task = tokio::spawn(async move {
            let outcome: Result<(), JobServiceError> = async {
            let (sender_tx, mut sender_rx) = tokio::sync::oneshot::channel();
            let run = prepared.run(attempt_id.clone(), sink, move |sender| { let _ = sender_tx.send(sender); });
            tokio::pin!(run);
            let reason = tokio::select! {
                result = &mut run => {
                    let (state, detail) = match result {
                        Ok(_) => (JobAttemptState::Failed, "Worker finished without an accepted structured result".to_string()),
                        Err(error @ worker::job::JobExecutionError::ResultUnconfirmed(_)) => (JobAttemptState::Unknown, error.to_string()),
                        Err(error) => (JobAttemptState::Failed, error.to_string()),
                    };
                    finish_if_pending(&store, &id, &attempt_id, state, &detail)?;
                    return Ok(());
                }
                _ = tokio::time::sleep(timeout) => "timeout",
                _ = cancellation.wait_for(|cancelled| *cancelled) => "Host cancellation/shutdown",
            };
            // Fence timeout/shutdown before cancelling so a late tool call can
            // never turn an interrupted unaccepted execution into success.
            let fence_result = finish_if_pending(&store, &id, &attempt_id, JobAttemptState::Unknown, reason);
            // Cancellation is routed through Engine; keep the future alive to
            // allow normal Feature cleanup. Hard stop is a fenced unknown
            // outcome, never an automatic retry. Result acceptance wins races.
            if let Ok(sender) = sender_rx.try_recv() {
                let _ = sender.try_send(());
            } else if let Ok(Ok(sender)) = tokio::time::timeout(Duration::from_secs(1), &mut sender_rx).await {
                let _ = sender.try_send(());
            }
            let cleanup = tokio::time::timeout(Duration::from_secs(2), &mut run).await;
            fence_result?;
            if cleanup.is_err() { return Err(JobServiceError::Cleanup("Internal Worker cancellation deadline exceeded; outcome fenced unknown".into())); }
            Ok(())
            }.await;
            complete_tx.send_replace(Some(
                outcome.as_ref().map(|_| ()).map_err(ToString::to_string),
            ));
            outcome
        });
        executions.jobs.insert(
            job_id.to_string(),
            Execution {
                cancel,
                complete,
                task,
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
            }
        }
        self.get(job_id)
    }

    pub(crate) fn close(&self) {
        let mut executions = self.executions.lock().expect("Job executions poisoned");
        executions.closed = true;
        for execution in executions.jobs.values() {
            execution.cancel.send_replace(true);
        }
    }

    /// Initiate owner shutdown without blocking a synchronous caller. Success confirms all
    /// local executions have returned, including request/history and Feature cleanup. An
    /// incomplete/error result retains the service resources and must retain the Host lease.
    pub(crate) fn try_shutdown(&self) -> Result<(), JobServiceError> {
        self.close();
        let mut executions = self.executions.lock().expect("Job executions poisoned");
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
        executions.jobs.clear();
        // Retained external handles share these slots; they cannot retain a live store,
        // provider or model client after successful owner shutdown.
        self.store.lock().expect("Job store poisoned").take();
        self.domain.lock().expect("Job domain poisoned").take();
        self.model_client
            .lock()
            .expect("Job model transport poisoned")
            .take();
        Ok(())
    }

    pub(crate) async fn shutdown(&self) -> Result<(), JobServiceError> {
        self.close();
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
                Err(JobServiceError::ShutdownIncomplete) => tokio::task::yield_now().await,
                result => return result,
            }
        }
    }
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
        if tokio::time::Instant::now() >= self.deadline && snapshot.state != JobState::Completed {
            return Err("Job result deadline expired".into());
        }
        store
            .accept(&submission)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

struct BoundAttemptFence {
    store: Arc<Mutex<Option<JobStore>>>,
    job_id: String,
    attempt_id: String,
    input_digest: String,
    deadline: tokio::time::Instant,
}
impl JobAttemptFence for BoundAttemptFence {
    fn ensure_live(&self) -> Result<(), String> {
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
