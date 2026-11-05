//! Ownership of never-dispatched consolidation admissions. The durable obligation
//! is the original Job reservation, not a new queue or a retry execution attempt.
use super::*;
use crate::jobs::{JobServiceError, JobSnapshot, JobState, JobStoreError, StandaloneJobs};
use std::time::Duration;
use tokio::time::Instant;

pub(super) struct ConsolidationAdmission {
    pub(super) deadline: Instant,
    pub(super) issue: Option<ConsolidationRecoveryError>,
    pub(super) blocked: bool,
}

pub(super) fn schedule_job(
    jobs: StandaloneJobs,
    observed: JobSnapshot,
    host: std::sync::Weak<LocalSubjectHost>,
    deadline: Option<Instant>,
) {
    let Some(host) = host.upgrade() else {
        return;
    };
    let key = (
        observed.request.job_id.clone(),
        observed.attempt.attempt_id.clone(),
        observed.input_digest.clone(),
    );
    let deadline = deadline.unwrap_or_else(|| {
        Instant::now() + Duration::from_secs(u64::from(observed.request.limits.timeout_seconds))
    });
    // An overlapping restore/request must not refill the earlier storage owner's
    // admission budget while that exact operation is waiting to hand off.
    let deadline = host
        .consolidation_recovery
        .lock()
        .expect("consolidation recovery poisoned")
        .get(&key)
        .map_or(deadline, |owner| owner.admission_deadline.min(deadline));
    {
        let mut owners = host
            .consolidation_admissions
            .lock()
            .expect("consolidation admissions poisoned");
        if owners.contains_key(&key) {
            return;
        }
        owners.insert(
            key.clone(),
            ConsolidationAdmission {
                deadline,
                issue: None,
                blocked: false,
            },
        );
    }
    tokio::spawn(async move {
        let outcome = tokio::select! {
            biased;
            _ = jobs.recovery_closed() => Ok(false),
            outcome = host.admit_consolidation(&jobs, &observed, &key) => outcome,
        };
        match outcome {
            Err(error) => {
                eprintln!(
                    "standalone consolidation admission {}/{} blocked: {error}; original input retained",
                    key.0, key.1
                );
                let mut owners = host
                    .consolidation_admissions
                    .lock()
                    .expect("consolidation admissions poisoned");
                let owner = owners.get_mut(&key).expect("owned consolidation admission");
                owner.issue = Some(error);
                owner.blocked = true;
                return;
            }
            Ok(started) => {
                // Dispatch is never retried after this point. Execution/cleanup and
                // settlement remain owned by the existing generic Job service.
                if started {
                    match jobs.wait(&observed.request.job_id).await {
                        Ok(result) if result.state == JobState::Completed => {
                            if host.ensure_consolidation_live().is_ok() {
                                let _ = jobs.acknowledge(&result.request.job_id);
                            }
                        }
                        Ok(result) if result.state == JobState::Failed => {
                            if let Err(error) = host.recover_consolidation_job(&jobs, &result) {
                                host.own_consolidation_recovery(jobs.clone(), result, error);
                            }
                        }
                        Ok(_) => {}
                        Err(error) => eprintln!(
                            "standalone Subject Job {} cleanup/acknowledgement is unconfirmed: {error}",
                            key.0
                        ),
                    }
                }
                host.consolidation_admissions
                    .lock()
                    .expect("consolidation admissions poisoned")
                    .remove(&key);
                host.wake_journals();
            }
        }
    });
}

impl LocalSubjectHost {
    async fn admit_consolidation(
        &self,
        jobs: &StandaloneJobs,
        observed: &JobSnapshot,
        key: &(String, String, String),
    ) -> Result<bool, ConsolidationRecoveryError> {
        let deadline = self
            .consolidation_admissions
            .lock()
            .expect("consolidation admissions poisoned")
            .get(key)
            .expect("owned consolidation admission")
            .deadline;
        let mut delay = Duration::from_millis(100);
        // Once preparation genuinely refuses, retry ONLY persistence of that
        // refusal, not preparation/model execution after an unrelated recovery.
        let mut refusal: Option<String> = None;
        loop {
            let changed = jobs.admission_changed();
            tokio::pin!(changed);
            changed.as_mut().enable(); // BEFORE capacity checks, including task return.
            jobs.join_completed_admissions().await;
            let attempt = (|| {
                self.ensure_consolidation_live()?;
                match self.recover_consolidation_job(jobs, observed)? {
                    ConsolidationRecovery::AdmissionRequired => {}
                    ConsolidationRecovery::Settled | ConsolidationRecovery::Retained => {
                        return Ok(false);
                    }
                }
                if Instant::now() >= deadline {
                    refusal.get_or_insert_with(|| "consolidation_admission_deadline_exceeded: original reservation deadline elapsed before dispatch".into());
                }
                if refusal.is_none() {
                    match jobs.start_reserved_granted(observed, deadline) {
                        Ok(()) => return Ok(true),
                        Err(JobServiceError::Configuration(cause)) => {
                            refusal =
                                Some(format!("consolidation_admission_cannot_start: {cause}"));
                        }
                        Err(error) => return Err(error.into()),
                    }
                }
                // Fresh exact snapshot, real Host binding, original input/grant and
                // ALL cleanup were just checked. This write proves no runner cleanup.
                self.ensure_consolidation_live()?;
                jobs.reject_reserved_granted(
                    observed,
                    refusal.as_deref().expect("admission refusal"),
                )?;
                let failed = jobs.get(&observed.request.job_id)?;
                if let Err(error) = self.recover_consolidation_job(jobs, &failed) {
                    self.own_consolidation_recovery(jobs.clone(), failed, error);
                }
                Ok(false)
            })();
            match attempt {
                Ok(started) => return Ok(started),
                Err(error) => {
                    let capacity = matches!(
                        &error,
                        ConsolidationRecoveryError::Service(
                            JobServiceError::Capacity
                                | JobServiceError::Store(
                                    JobStoreError::ConcurrentLimit
                                        | JobStoreError::SerializationConflict(_)
                                )
                        )
                    );
                    if !capacity && !error.retryable_read_or_storage() {
                        return Err(error);
                    }
                    self.consolidation_admissions
                        .lock()
                        .expect("consolidation admissions poisoned")
                        .get_mut(key)
                        .expect("owned consolidation admission")
                        .issue = Some(error);
                    if capacity {
                        tokio::select! {
                            _ = &mut changed => {}
                            _ = tokio::time::sleep_until(deadline) => {}
                        }
                    } else {
                        // Retry the SAME operation against its actual storage source.
                        // No healthy-other-store probe and no new dispatch attempt.
                        tokio::time::sleep(delay).await;
                        delay = (delay * 2).min(Duration::from_secs(30));
                    }
                }
            }
        }
    }
}
