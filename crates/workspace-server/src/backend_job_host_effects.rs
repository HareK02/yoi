//! Exact Backend Job Host admission, independent of Controller and domain completion.
//!
//! Each accepted callback retains a shared admission-file lease through actual DB/Host work,
//! including discarded-response cleanup. After the canonical attempt closes admission, an
//! exclusive lease on that same retained resource fences those callbacks across Server owners;
//! neither an empty counter nor process exit issues a receipt. The original identity file must
//! exist and match. This does not certify detached external work: Backend Job grants permit
//! only result and bounded synchronous Subject operations, not arbitrary external mutations.
//! Canonical notification delivery and Worker removal retain their separate operation owners.
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
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
    admission_path: PathBuf,
    admission: Mutex<Admission>,
    finished: Notify,
}

impl JobHostEffects {
    pub(crate) fn initially_admit(owner: JobHostOwner, database: &Path) -> Result<Arc<Self>> {
        let admission_path = admission_path(database, &owner)?;
        let parent = admission_path.parent().unwrap();
        std::fs::create_dir_all(parent)?;
        // Publish a complete identity before any callback can acquire its lease.
        // The temporary name is not an execution, release or recovery identity.
        let temporary = parent.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
        let publish = (|| -> Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(
                &serde_json::to_vec(&owner).map_err(|error| Error::Store(error.to_string()))?,
            )?;
            file.sync_all()?;
            match std::fs::hard_link(&temporary, &admission_path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    open_admission(&admission_path, &owner)?;
                }
                Err(error) => return Err(error.into()),
            }
            // Resolve a prior publication whose directory sync acknowledgement failed.
            File::open(parent)?.sync_all()?;
            Ok(())
        })();
        let _ = std::fs::remove_file(temporary);
        publish?;
        Ok(Arc::new(Self {
            owner,
            admission_path,
            admission: Mutex::new(Admission {
                open: true,
                requests: 0,
            }),
            finished: Notify::new(),
        }))
    }

    /// The caller must recheck the canonical current terminal attempt while holding
    /// this guard, then persist its fence before releasing it. No resource is created.
    pub(crate) fn fence_retained(
        database: &Path,
        attempt: &BackendJobAttemptRecord,
    ) -> Result<JobHostDrain> {
        if !attempt.state.terminal() {
            return Err(Error::RepositoryConflict(
                "Job Host admission is not canonically closed".into(),
            ));
        }
        let owner = JobHostOwner::try_from(attempt)?;
        let file = open_admission(&admission_path(database, &owner)?, &owner)?;
        file.lock()?;
        Ok(JobHostDrain {
            owner,
            _admission: file,
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
        let file = open_admission(&self.admission_path, expected)?;
        // Never block a DB lane behind cleanup's exclusive guard: cleanup needs
        // that lane to commit its evidence. The canonical terminal row rejects replay.
        file.try_lock_shared().map_err(|error| {
            Error::RepositoryConflict(format!("Job Host admission is fenced: {error}"))
        })?;
        admission.requests = admission
            .requests
            .checked_add(1)
            .ok_or_else(|| Error::Store("Job Host request count overflow".into()))?;
        Ok(JobHostRequestLease {
            request: Arc::new(AdmittedRequest {
                effects: self.clone(),
                _admission: file,
            }),
        })
    }

    pub(crate) fn close_admission(&self, expected: &JobHostOwner) -> Result<()> {
        if &self.owner != expected {
            return Err(Error::InvalidInput("Job Host close owner mismatch".into()));
        }
        self.admission
            .lock()
            .map_err(|_| Error::Store("Job Host admission poisoned".into()))?
            .open = false;
        Ok(())
    }

    #[cfg(test)]
    async fn close_and_drain(self: &Arc<Self>, expected: &JobHostOwner) -> Result<JobHostDrain> {
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
                let file = open_admission(&self.admission_path, expected)?;
                file.lock()?;
                return Ok(JobHostDrain {
                    owner: self.owner.clone(),
                    _admission: file,
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
    _admission: File,
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
    _admission: File,
}

impl JobHostDrain {
    pub(crate) fn owner(&self) -> &JobHostOwner {
        &self.owner
    }
}

fn admission_path(database: &Path, owner: &JobHostOwner) -> Result<PathBuf> {
    let identity = serde_json::to_vec(owner).map_err(|error| Error::Store(error.to_string()))?;
    let key: String = Sha256::digest(identity)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let mut root = database.as_os_str().to_owned();
    root.push(".job-host-admission");
    Ok(PathBuf::from(root).join(key))
}

fn open_admission(path: &Path, expected: &JobHostOwner) -> Result<File> {
    // Recovery must never create a missing predecessor's admission resource.
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| {
            Error::RepositoryConflict(format!(
                "Original Job Host admission resource unavailable: {error}"
            ))
        })?;
    let mut bytes = Vec::new();
    (&mut file).take(8193).read_to_end(&mut bytes)?;
    let saved: JobHostOwner = serde_json::from_slice(&bytes).map_err(|_| {
        Error::RepositoryConflict("Original Job Host admission identity invalid".into())
    })?;
    if bytes.len() > 8192 || &saved != expected {
        return Err(Error::RepositoryConflict(
            "Original Job Host admission identity changed".into(),
        ));
    }
    Ok(file)
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
        let directory = tempfile::tempdir().unwrap();
        let effects = JobHostEffects::initially_admit(
            expected.clone(),
            &directory.path().join("store.sqlite"),
        )
        .unwrap();
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
        let directory = tempfile::tempdir().unwrap();
        let effects = JobHostEffects::initially_admit(
            current.clone(),
            &directory.path().join("store.sqlite"),
        )
        .unwrap();
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
