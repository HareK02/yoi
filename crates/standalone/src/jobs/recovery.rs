//! Restore ownership for result-only reservations. Domain recovery keeps its own contract.
use super::*;

impl StandaloneJobs {
    /// Capture exact Job/attempt identities before exposing the restored Host.
    /// Each admission has its own waiter: capacity, storage or cleanup failure in
    /// one Job must not discard the other accepted reservations.
    pub(crate) async fn resume_reserved_result_jobs(
        &self,
        ids: Vec<(String, String)>,
    ) -> Result<(), JobServiceError> {
        let mut owners = tokio::task::JoinSet::new();
        for (id, attempt) in ids {
            let issue = Arc::new(Mutex::new(None));
            {
                let mut admissions = self.result_recovery.lock().expect("Job recovery poisoned");
                if admissions.contains_key(&id) {
                    continue;
                }
                admissions.insert(id.clone(), issue.clone());
            }
            let jobs = self.clone();
            owners.spawn(async move {
                let outcome = tokio::select! {
                    biased;
                    _ = jobs.recovery_closed() => return Ok(()),
                    outcome = jobs.resume_result_attempt(&id, &attempt, &issue) => outcome,
                };
                match outcome {
                    Ok(()) => {
                        jobs.result_recovery
                            .lock()
                            .expect("Job recovery poisoned")
                            .remove(&id);
                        Ok(())
                    }
                    Err(error) => {
                        let detail = error.to_string();
                        *issue.lock().expect("Job recovery issue poisoned") = Some(error);
                        Err(JobServiceError::Cleanup(format!(
                            "result recovery {id}/{attempt}: {detail}"
                        )))
                    }
                }
            });
        }
        let mut first_error = None;
        while let Some(result) = owners.join_next().await {
            let result =
                result.unwrap_or_else(|error| Err(JobServiceError::Cleanup(error.to_string())));
            if let Err(error) = result {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// Unresolved restore causes are independent of execution/cleanup evidence.
    pub fn result_recovery_diagnostic(&self, job_id: &str) -> Option<String> {
        let admissions = self.result_recovery.lock().expect("Job recovery poisoned");
        let issue = admissions.get(job_id)?;
        let issue = issue.lock().expect("Job recovery issue poisoned");
        Some(issue.as_ref().map_or_else(
            || "accepted result-only reservation retained; awaiting execution capacity or owner settlement".into(),
            ToString::to_string,
        ))
    }

    async fn recovery_closed(&self) {
        loop {
            let changed = self.capacity_changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self
                .executions
                .lock()
                .expect("Job executions poisoned")
                .closed
            {
                return;
            }
            changed.await;
        }
    }

    async fn resume_result_attempt(
        &self,
        id: &str,
        attempt: &str,
        issue: &Mutex<Option<JobServiceError>>,
    ) -> Result<(), JobServiceError> {
        loop {
            // Register before checking capacity, including completed-but-not-yet-
            // returned tasks, so a real release cannot be lost between checks.
            let changed = self.capacity_changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let completed: Vec<_> = self
                .executions
                .lock()
                .expect("Job executions poisoned")
                .jobs
                .iter()
                .filter(|(_, run)| matches!(run.complete.borrow().as_ref(), Some(Ok(()))))
                .map(|(id, _)| id.clone())
                .collect();
            for completed_id in completed {
                // wait joins task return and only then removes a successful slot.
                // A failed read here is not evidence against this reservation.
                let _ = self.wait(&completed_id).await;
            }
            let started = recover_owned_settlement(
                &self.storage_recovered,
                issue,
                || self.start_restored_result_attempt(id, attempt),
                || self.with_store_write(|store| store.probe_reserved_result_write(id, attempt)),
            )
            .await;
            match started {
                Err(
                    error @ (JobServiceError::Capacity
                    | JobServiceError::Store(
                        JobStoreError::ConcurrentLimit | JobStoreError::SerializationConflict(_),
                    )),
                ) => {
                    *issue.lock().expect("Job recovery issue poisoned") = Some(error);
                    changed.await;
                }
                Err(error) => return Err(error),
                Ok(()) => {
                    // Retry only a failed result read, never the already-admitted
                    // start. Cleanup errors retain their execution ownership.
                    let outcome = self.wait(id).await;
                    if let Err(error) = outcome {
                        if !transient_settlement_storage_error(&error) {
                            return Err(error);
                        }
                        recover_owned_settlement(
                            &self.storage_recovered,
                            issue,
                            || self.get(id).map(|_| ()),
                            || {
                                self.with_store_write(|store| {
                                    store.probe_reserved_result_write(id, attempt)
                                })
                            },
                        )
                        .await?;
                    }
                    return Ok(());
                }
            }
        }
    }

    fn start_restored_result_attempt(
        &self,
        id: &str,
        attempt: &str,
    ) -> Result<(), JobServiceError> {
        let saved = self.get(id)?;
        if saved.attempt.attempt_id != attempt
            || saved.domain_grant.is_some()
            || saved.version_stopped
        {
            return Err(JobStoreError::AttemptMismatch.into());
        }
        // A client may have cancelled or started this exact attempt since capture.
        // Never dispatch it twice, and never follow a newly-issued retry attempt.
        if saved.state != JobState::Pending || saved.attempt.state != JobAttemptState::Reserved {
            return Ok(());
        }
        match self.start_with_optional_model_client(id, None) {
            Ok(()) => Ok(()),
            Err(
                error @ (JobServiceError::Capacity
                | JobServiceError::Closed
                | JobServiceError::Store(
                    JobStoreError::ConcurrentLimit | JobStoreError::SerializationConflict(_),
                )),
            ) => Err(error),
            Err(error) if transient_settlement_storage_error(&error) => Err(error),
            Err(error) => {
                // This path has no admitted execution. Persist the individual
                // rejection without forging cleanup for any prior ownership.
                let detail = format!("result_recovery_cannot_start: {error}");
                self.with_store_write(|store| store.fail_reserved_result(id, attempt, &detail))
            }
        }
    }
}
