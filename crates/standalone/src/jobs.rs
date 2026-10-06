//! Host-provided local Jobs. No Runtime registry, Workspace client or dialog-session state.
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
pub use crate::job_store::{JobAttempt, JobAttemptState, JobSnapshot, JobState, JobStoreError};

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
        let client = client.or_else(|| {
            self.model_client
                .lock()
                .expect("Job model transport poisoned")
                .as_ref()
                .map(|c| c.clone_boxed())
        });
        let snapshot = self.get(job_id)?;
        let timeout = Duration::from_secs(u64::from(snapshot.request.limits.timeout_seconds));
        let deadline = tokio::time::Instant::now() + timeout;
        let fence: Arc<dyn JobAttemptFence> = Arc::new(BoundAttemptFence {
            store: self.store.clone(),
            job_id: job_id.into(),
            attempt_id: snapshot.attempt.attempt_id.clone(),
            input_revision: snapshot.request.input_revision.clone(),
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
        let mut executions = self.executions.lock().expect("Job executions poisoned");
        if executions.closed {
            return Err(JobServiceError::Closed);
        }
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
            input_revision: snapshot.request.input_revision.clone(),
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
            let task = {
                let mut executions = self.executions.lock().expect("Job executions poisoned");
                if executions
                    .jobs
                    .get(job_id)
                    .is_some_and(|run| run.attempt_id == attempt)
                {
                    executions.jobs.remove(job_id)
                } else {
                    None
                }
            };
            if let Some(execution) = task {
                execution
                    .task
                    .await
                    .map_err(|error| JobServiceError::Cleanup(error.to_string()))??;
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

    pub(crate) async fn shutdown(&self) -> Result<(), JobServiceError> {
        self.close();
        let tasks = {
            let mut executions = self.executions.lock().expect("Job executions poisoned");
            std::mem::take(&mut executions.jobs)
        };
        let mut error = None;
        for execution in tasks.into_values() {
            match execution.task.await {
                Ok(Ok(())) => {}
                Ok(Err(failure)) => {
                    error = Some(failure);
                }
                Err(failure) => {
                    error = Some(JobServiceError::Cleanup(failure.to_string()));
                }
            }
        }
        // Close SQLite before the Host releases its lease. Cloned handles may
        // outlive the Host but cannot mutate or retain the live connection.
        self.store.lock().expect("Job store poisoned").take();
        self.domain.lock().expect("Job domain poisoned").take();
        self.model_client
            .lock()
            .expect("Job model transport poisoned")
            .take();
        error.map_or(Ok(()), Err)
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
    input_revision: String,
    deadline: tokio::time::Instant,
}
impl worker::job::JobResultSink for BoundResultSink {
    fn submit(&self, submission: JobResultSubmission) -> Result<(), String> {
        if submission.job_id != self.job_id
            || submission.attempt_id != self.attempt_id
            || submission.input_revision != self.input_revision
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
    input_revision: String,
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
            || snapshot.request.input_revision != self.input_revision
        {
            return Err("Job domain grant is not bound to the current live attempt".into());
        }
        Ok(())
    }
}
