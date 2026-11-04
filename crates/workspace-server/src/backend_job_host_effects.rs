//! Live ownership of Host effects admitted for one exact Job execution.
//!
//! This supplements durable allocation/submission/domain receipts; it is not a Job state machine.
//! Create it only when initially admitting an execution, before its first Host callback. Recovery
//! must not manufacture a drain receipt by reconstructing an empty owner after a restart. A lost
//! owner requires reconciliation of the retained domain operations and writer fences instead.
//! Closing admission does not cancel accepted operations. Their existing DbExecutor lifetimes
//! retain the request lease through actual work and discarded-result cleanup, even if the HTTP
//! waiter disappears. Detached work must explicitly retain this same lease or is not covered.
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;
use worker_runtime::identity::RuntimeWorkerRef;

use crate::backend_job::BackendJobAttemptRecord;
use crate::db_executor::DbOperationLifetime;
use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct JobHostOwner {
    pub workspace_id: String,
    pub job_id: String,
    pub attempt_id: String,
    pub input_digest: String,
    pub worker: RuntimeWorkerRef,
    pub runtime_run_id: String,
}

impl TryFrom<&BackendJobAttemptRecord> for JobHostOwner {
    type Error = Error;

    fn try_from(attempt: &BackendJobAttemptRecord) -> Result<Self> {
        Ok(Self {
            workspace_id: attempt.workspace_id.clone(),
            job_id: attempt.job_id.clone(),
            attempt_id: attempt.attempt_id.clone(),
            input_digest: attempt.input_digest.clone(),
            worker: attempt.worker.clone().ok_or_else(|| {
                Error::InvalidInput("Job Host ownership requires an exact Worker binding".into())
            })?,
            runtime_run_id: attempt
                .runtime_run_id
                .clone()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| {
                    Error::InvalidInput(
                        "Job Host ownership requires a tracked Runtime submission".into(),
                    )
                })?,
        })
    }
}

struct Admission {
    open: bool,
    requests: usize,
}

pub(crate) struct JobHostEffects {
    owner: JobHostOwner,
    admission: Mutex<Admission>,
    finished: Notify,
}

impl JobHostEffects {
    pub(crate) fn initially_admit(owner: JobHostOwner) -> Arc<Self> {
        Arc::new(Self {
            owner,
            admission: Mutex::new(Admission {
                open: true,
                requests: 0,
            }),
            finished: Notify::new(),
        })
    }

    pub(crate) fn admit(self: &Arc<Self>, expected: &JobHostOwner) -> Result<JobHostRequestLease> {
        if &self.owner != expected {
            return Err(Error::InvalidInput(
                "Job Host execution owner mismatch".into(),
            ));
        }
        let mut admission = self
            .admission
            .lock()
            .map_err(|_| Error::Store("Job Host admission poisoned".into()))?;
        if !admission.open {
            return Err(Error::InvalidInput(
                "Job Host execution admission is closed".into(),
            ));
        }
        admission.requests = admission
            .requests
            .checked_add(1)
            .ok_or_else(|| Error::Store("Job Host request count overflow".into()))?;
        Ok(JobHostRequestLease {
            request: Arc::new(AdmittedRequest {
                effects: self.clone(),
            }),
        })
    }

    /// The receipt proves only this live Host owner's accepted operations ended. Its constructor
    /// is private so a missing map entry, Job terminal state, or timeout cannot create evidence.
    pub(crate) async fn close_and_drain(
        self: &Arc<Self>,
        expected: &JobHostOwner,
    ) -> Result<JobHostDrain> {
        if &self.owner != expected {
            return Err(Error::InvalidInput("Job Host close owner mismatch".into()));
        }
        self.admission
            .lock()
            .map_err(|_| Error::Store("Job Host admission poisoned".into()))?
            .open = false;
        loop {
            let notified = self.finished.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self
                .admission
                .lock()
                .map_err(|_| Error::Store("Job Host admission poisoned".into()))?
                .requests
                == 0
            {
                return Ok(JobHostDrain {
                    owner: self.owner.clone(),
                });
            }
            notified.await;
        }
    }
}

/// Every clone refers to the same admitted request, including its synchronous child operations.
/// Only the last real owner ends it. Returning/dropping the async waiter alone is insufficient.
#[derive(Clone)]
pub(crate) struct JobHostRequestLease {
    request: Arc<AdmittedRequest>,
}

impl DbOperationLifetime for JobHostRequestLease {
    fn retain(&self) -> Box<dyn Send> {
        Box::new(self.clone())
    }
}

struct AdmittedRequest {
    effects: Arc<JobHostEffects>,
}

impl Drop for AdmittedRequest {
    fn drop(&mut self) {
        let mut admission = self
            .effects
            .admission
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        admission.requests -= 1;
        drop(admission);
        self.effects.finished.notify_waiters();
    }
}

pub(crate) struct JobHostDrain {
    owner: JobHostOwner,
}

impl JobHostDrain {
    pub(crate) fn owner(&self) -> &JobHostOwner {
        &self.owner
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db_executor::{DbExecutor, DbExecutorLimits, with_operation_lifetime};
    use std::{num::NonZeroUsize, time::Duration};

    fn owner(attempt: &str) -> JobHostOwner {
        JobHostOwner {
            workspace_id: "workspace".into(),
            job_id: "job".into(),
            attempt_id: attempt.into(),
            input_digest: "input".into(),
            worker: RuntimeWorkerRef::new("runtime", "worker"),
            runtime_run_id: format!("run:{attempt}"),
        }
    }

    #[tokio::test]
    async fn cancelled_waiter_cannot_release_accepted_host_effect_or_admit_after_close() {
        let expected = owner("attempt-1");
        let effects = JobHostEffects::initially_admit(expected.clone());
        let lease = effects.admit(&expected).unwrap();
        let executor = DbExecutor::new(DbExecutorLimits {
            workers: NonZeroUsize::new(1).unwrap(),
            queued: NonZeroUsize::new(1).unwrap(),
        })
        .unwrap();
        let (entered, started) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let work = executor.clone();
        let waiter: tokio::task::JoinHandle<Result<()>> =
            tokio::spawn(with_operation_lifetime(Arc::new(lease), async move {
                work.run(move || {
                    entered.send(()).unwrap();
                    wait.recv_timeout(Duration::from_secs(5)).unwrap();
                    Ok(())
                })
                .await
            }));
        started.await.unwrap();
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        let draining = effects.clone();
        let binding = expected.clone();
        let drain = tokio::spawn(async move { draining.close_and_drain(&binding).await });
        // An active accepted lease remains an owner even though its waiter is gone.
        assert_eq!(effects.admission.lock().unwrap().requests, 1);
        release.send(()).unwrap();
        let receipt = tokio::time::timeout(Duration::from_secs(2), drain)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(receipt.owner(), &expected);
        assert!(effects.admit(&expected).is_err());
        assert!(effects.admit(&owner("attempt-2")).is_err());
    }

    #[tokio::test]
    async fn stale_close_cannot_revoke_a_successor_owner_or_create_its_drain_receipt() {
        let current = owner("attempt-2");
        let effects = JobHostEffects::initially_admit(current.clone());
        assert!(effects.close_and_drain(&owner("attempt-1")).await.is_err());
        let request = effects.admit(&current).unwrap();
        let child = request.clone();
        drop(request);
        assert_eq!(effects.admission.lock().unwrap().requests, 1);
        drop(child);
        let receipt = effects.close_and_drain(&current).await.unwrap();
        assert_eq!(receipt.owner(), &current);
        assert!(effects.admit(&current).is_err());
    }
}
