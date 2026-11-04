//! Durable Journal admission and generation on the existing local Jobs service.
//!
//! Admission never waits for the model. One consumer drains saved oldest prefixes, including
//! input arriving during a generation. Job state, receipts, and the source ledger are the only
//! recovery authority; shutdown does not delete either original Sessions or pending requests.

use super::*;
use crate::jobs::{
    JobAttemptState, JobInputDisposition, JobInputRetirementReason, JobSnapshot, JobState,
};
use session_store::capture::{CaptureDto, CaptureLimits, RecordingPosition};
use subjektiv::api::JobAttemptBinding;
use subjektiv::journal::recording::JournalInput;
use subjektiv::journal::transport::*;
use subjektiv::journal::*;
use worker::subjektiv::{JournalJobFinalizer, JournalJobHost, JournalJobToolsFeature};

#[cfg(test)]
mod tests;

pub(super) const PURPOSE: &str = "subjektiv_journal";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalJournalGrant {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    writer_binding_id: Option<String>,
    binding: StandaloneSubjectBinding,
    session_id: String,
    journal_id: String,
    ranges: Vec<JournalSourceRange>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    origins: Vec<SourceOrigin>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceOrigin {
    pointer: JournalSourcePointer,
    origin: session_store::public_material::PublicMaterialOrigin,
}

impl StandaloneSubjects {
    /// Explicit operator erasure, separate from ordinary Worker removal. Requires the
    /// Subject's exclusive Host lease. The domain tombstone fences delayed admission and
    /// tools before physical source erasure; exact retries keep the same request ID.
    /// No model is started, including when no Journal has yet been generated.
    pub fn erase_session_completely(
        self,
        subject_id: &str,
        session_id: &str,
        request_id: &str,
    ) -> Result<(), SubjectError> {
        let attribution = self
            .store
            .session_attribution(session_id)?
            .filter(|attr| attr.subject_id == subject_id && attr.runtime_id == self.scope_id)
            .ok_or_else(|| {
                SubjectError::InvalidScope(
                    "Session does not belong to the selected Subject/storage scope".into(),
                )
            })?;
        let leases = self.root.join("leases");
        private_dir(&leases, true)?;
        let lease = private_file(&leases.join(format!("{subject_id}.lock")), true)?;
        if !lease.try_lock_exclusive()? {
            return Err(SubjectError::Active(subject_id.into()));
        }
        let context = JournalHostContext::validated_session(&self.store, attribution)?;
        let path = crate::subjektiv_sessions::authorized_sessions_path(
            &self.state_root,
            &self.scope_id,
            subject_id,
            &self.store,
            session_id,
        )
        .map_err(|error| SubjectError::InvalidScope(error.to_string()))?;
        // Erasure is not a logical reconnect. Never mint a new writer lifetime in order
        // to delete a released bodys text; the permanent fence precedes physical deletion.
        self.store.erase_journal_session(&context, request_id)?;
        let jobs = crate::jobs::StandaloneJobs::open(
            &self.root.join("jobs").join(subject_id).join("jobs.sqlite3"),
            self.state_root.clone(),
        )
        .map_err(|error| SubjectError::InvalidScope(error.to_string()))?;
        let mut after = None;
        loop {
            let page = jobs
                .list_session_snapshots(session_id, after.as_deref(), 100)
                .map_err(|e| SubjectError::InvalidScope(e.to_string()))?;
            if page.is_empty() {
                break;
            }
            for snapshot in &page {
                jobs.cancel(&snapshot.request.job_id)
                    .map_err(|e| SubjectError::InvalidScope(e.to_string()))?;
                jobs.reclaim_input(
                    &snapshot.request.job_id,
                    &snapshot.input_digest,
                    JobInputRetirementReason::ExplicitErasure,
                )
                .map_err(|e| SubjectError::InvalidScope(e.to_string()))?;
                if snapshot.state == JobState::Completed && !snapshot.acknowledged {
                    jobs.acknowledge(&snapshot.request.job_id)
                        .map_err(|e| SubjectError::InvalidScope(e.to_string()))?;
                }
            }
            after = page.last().map(|job| job.request.job_id.clone());
        }
        session_store::FsStore::new_with_retention(
            path,
            crate::store::session_retention(&self.state_root),
        )
        .map_err(|e| SubjectError::InvalidScope(e.to_string()))?
        .delete_session_with_intent(
            session_id
                .parse()
                .map_err(|_| SubjectError::InvalidScope("invalid Session".into()))?,
            session_store::retention::SessionDeletionIntent::ErasePermanently,
        )
        .map_err(|e| SubjectError::InvalidScope(e.to_string()))?;
        self.store.release_erased_journal_body(&context)?;
        self.manager.shutdown()?;
        lease.unlock()?;
        Ok(())
    }
}

pub(crate) fn retained_workers_root(state_root: &Path) -> PathBuf {
    state_root.join("subjektiv").join("retained-workers")
}

/// Ordinary removal cannot turn a stopped/unknown body into an unoccupied Subject. The
/// live Host must first release it under the Controller Idle reservation. The saved
/// release fence also prevents newly appended old-body history from being recorded here.
pub(crate) fn prepare_worker_deletion(
    state_root: &Path,
    record: &crate::store::StandaloneWorkerRecord,
) -> Result<(), crate::store::StandaloneStoreError> {
    let Some(binding) = &record.subject else {
        return Ok(());
    };
    let prepare = || -> Result<(), SubjektivHostError> {
        let catalog = StandaloneSubjects::open_existing(state_root)
            .map_err(|error| SubjektivHostError::Unavailable(error.to_string()))?;
        if catalog.scope_id != binding.scope_id {
            return Err(SubjektivHostError::InvalidArgument(
                "deletion Subject scope mismatch".into(),
            ));
        }
        let erased = catalog
            .store
            .session_attribution(&record.active_session_id.to_string())
            .map_err(domain_error)?
            .filter(|attr| {
                attr.subject_id == binding.subject_id
                    && attr.runtime_id == binding.scope_id
                    && attr.worker_id == record.worker_id.to_string()
            })
            .map(|attr| JournalHostContext::validated_session(&catalog.store, attr))
            .transpose()
            .map_err(journal_error)?;
        let destructively_released = match erased.as_ref() {
            Some(context) => catalog
                .store
                .release_erased_journal_body(context)
                .map_err(journal_error)?,
            None => false,
        };
        if !destructively_released
            && !catalog
                .store
                .journal_worker_was_released(
                    &binding.subject_id,
                    &binding.scope_id,
                    &record.worker_id.to_string(),
                )
                .map_err(journal_error)?
        {
            return Err(SubjektivHostError::Conflict { code: "subject_occupied".into(), message: "Subject binding is retained; restore this body and explicitly disconnect while Idle after Journal activity, position and checkpoint complete".into() });
        }
        let workers = crate::store::StandaloneWorkerStore::open_existing(state_root)
            .map_err(|error| SubjektivHostError::Unavailable(error.to_string()))?;
        let source_store = session_store::FsStore::new_with_retention(
            workers.sessions_dir(record.worker_id),
            crate::store::session_retention(state_root),
        )
        .map_err(source_error)?;
        for session in session_store::Store::list_sessions(&source_store).map_err(source_error)? {
            if destructively_released {
                source_store
                    .delete_session_with_intent(
                        session,
                        session_store::retention::SessionDeletionIntent::ErasePermanently,
                    )
                    .map_err(source_error)?;
            } else {
                source_store
                    .release_session_pin(
                        session,
                        session_store::retention::SessionRetentionPin(session),
                    )
                    .map_err(source_error)?;
            }
        }
        Ok(())
    };
    prepare()
        .map_err(|error| crate::store::StandaloneStoreError::JournalRetention(error.to_string()))
}

pub(super) fn journal_error(error: JournalError) -> SubjektivHostError {
    match error {
        JournalError::Conflict(message) => SubjektivHostError::Conflict {
            code: "journal_conflict".into(),
            message,
        },
        JournalError::PermissionDenied(message)
        | JournalError::InvalidInput(message)
        | JournalError::NotFound(message) => SubjektivHostError::InvalidArgument(message),
        other => SubjektivHostError::Unavailable(other.to_string()),
    }
}
fn source_error(error: session_store::StoreError) -> SubjektivHostError {
    SubjektivHostError::Unavailable(error.to_string())
}
fn service_error(error: crate::jobs::JobServiceError) -> SubjektivHostError {
    SubjektivHostError::Unavailable(error.to_string())
}
fn authority(
    host: &LocalSubjectHost,
    session_id: &str,
) -> Result<JournalHostContext, SubjektivHostError> {
    let attribution = host
        .catalog
        .store
        .session_attribution(session_id)
        .map_err(domain_error)?
        .filter(|attr| {
            attr.subject_id == host.binding.subject_id && attr.runtime_id == host.binding.scope_id
        })
        .ok_or_else(|| {
            SubjektivHostError::InvalidArgument("foreign or unattributed Journal Session".into())
        })?;
    JournalHostContext::validated_session(&host.catalog.store, attribution).map_err(journal_error)
}

impl LocalSubjectHost {
    /// The reservation is obtained by this Host from its actual Controller and kept through
    /// final reconciliation and durable revocation. A stopped body cannot supply this proof.
    pub(crate) fn release_for_handoff(
        &self,
        record: &crate::store::StandaloneWorkerRecord,
        _reservation: &worker::SubjectReleaseReservation,
    ) -> Result<(), SubjektivHostError> {
        if !self
            .catalog
            .store
            .journal_writer_active(&self.writer)
            .map_err(journal_error)?
        {
            return Ok(()); // reconcile an earlier committed revocation, never renew it
        }
        self.ensure_live()?;
        let dispatch = self
            .journal_dispatch
            .lock()
            .expect("Journal dispatcher poisoned");
        if *dispatch {
            return Err(SubjektivHostError::Conflict {
                code: "journal_running".into(),
                message: "Journal dispatcher is still running; binding retained".into(),
            });
        }
        let workers = crate::store::StandaloneWorkerStore::open_existing(&self.catalog.state_root)
            .map_err(|e| SubjektivHostError::Unavailable(e.to_string()))?;
        let source_store = session_store::FsStore::new_with_retention(
            workers.sessions_dir(self.worker_id),
            crate::store::session_retention(&self.catalog.state_root),
        )
        .map_err(source_error)?;
        for session in session_store::Store::list_sessions(&source_store).map_err(source_error)? {
            let context = SubjektivHostContext {
                worker_id: self.worker_id.to_string(),
                session_id: session.to_string(),
            };
            self.attribution(&context, session == record.active_session_id)?;
            self.reconcile_journal_session(&context.session_id)?;
            if !self.session_fully_reflected(&context.session_id)? {
                return Err(SubjektivHostError::Conflict {
                    code: "journal_unreflected".into(),
                    message:
                        "Final saved public history has not been fully recorded; binding retained"
                            .into(),
                });
            }
        }
        let jobs = self.jobs()?;
        for result in jobs.unacknowledged_results().map_err(service_error)? {
            if self.journal_grant(&result).is_ok() {
                self.complete_journal_job(&jobs, &result)?;
            }
        }
        let mut job_closure = jobs.reserve_quiescent_closure().map_err(service_error)?;
        for job in jobs
            .pending()
            .map_err(service_error)?
            .into_iter()
            .chain(jobs.unreleased_granted().map_err(service_error)?)
            .chain(jobs.unacknowledged_results().map_err(service_error)?)
        {
            let _ = job;
            return Err(SubjektivHostError::Conflict { code: "journal_outcome_unconfirmed".into(), message: "Subject has queued/running, unreleased or unacknowledged delegated work; binding retained".into() });
        }
        let released = self
            .catalog
            .store
            .release_journal_writer(&self.writer)
            .map_err(journal_error);
        if released.is_ok()
            || !self
                .catalog
                .store
                .journal_writer_active(&self.writer)
                .unwrap_or(false)
        {
            job_closure.commit();
        }
        released
    }

    fn journal_session_erased(&self, session: &str) -> Result<bool, SubjektivHostError> {
        let context = authority(self, session)?;
        self.catalog
            .store
            .journal_session_erased(&context)
            .map_err(journal_error)
    }

    fn prune_source_session(
        &self,
        source_store: &session_store::FsStore,
        session: session_store::SessionId,
    ) -> Result<(), SubjektivHostError> {
        if self.journal_session_erased(&session.to_string())? {
            // Resume the already authorized explicit erasure, including physical deletion
            // that previously failed. Ordinary deletion must not reinterpret its intent.
            source_store
                .delete_session_with_intent(
                    session,
                    session_store::retention::SessionDeletionIntent::ErasePermanently,
                )
                .map(|_| ())
                .map_err(source_error)
        } else {
            source_store
                .release_session_pin(
                    session,
                    session_store::retention::SessionRetentionPin(session),
                )
                .map_err(source_error)?;
            source_store
                .delete_session(session)
                .map(|_| ())
                .map_err(source_error)
        }
    }

    pub(super) fn reserve_journal_session(
        &self,
        context: &SubjektivHostContext,
    ) -> Result<(), SubjektivHostError> {
        self.body(context)?;
        if self.journal_session_erased(&context.session_id)? {
            return Ok(());
        }
        let session: session_store::SessionId = context
            .session_id
            .parse()
            .map_err(|_| SubjektivHostError::InvalidArgument("invalid Session".into()))?;
        let root = self
            .catalog
            .state_root
            .join(self.worker_id.to_string())
            .join("sessions");
        reject_symlink_components(&root)
            .map_err(|error| SubjektivHostError::Unavailable(error.to_string()))?;
        let source_store = session_store::FsStore::new_with_retention(
            &root,
            crate::store::session_retention(&self.catalog.state_root),
        )
        .map_err(source_error)?;
        {
            let _guard = source_store
                .retention()
                .lock_live(session)
                .map_err(source_error)?;
            let path = root.join(session.to_string());
            match fs::create_dir(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let metadata = fs::symlink_metadata(&path)
                        .map_err(|error| SubjektivHostError::Unavailable(error.to_string()))?;
                    if !metadata.is_dir() || metadata.file_type().is_symlink() {
                        return Err(SubjektivHostError::InvalidArgument(
                            "Session retention source is not an owned directory".into(),
                        ));
                    }
                }
                Err(error) => return Err(SubjektivHostError::Unavailable(error.to_string())),
            }
        }
        // Establish retention at attribution, before the first Run can publish bytes. This
        // closes the save-before-notification gap even for ordinary FsStore Session deletion.
        source_store
            .pin_session(
                session,
                session_store::retention::SessionRetentionPin(session),
            )
            .map_err(source_error)
    }

    pub(super) fn admit_journal(
        &self,
        context: &SubjektivHostContext,
    ) -> Result<bool, SubjektivHostError> {
        let attribution = self.attribution(context, false)?;
        self.reconcile_journal_session(&attribution.session_id)
    }

    fn reconcile_journal_session(&self, session: &str) -> Result<bool, SubjektivHostError> {
        self.ensure_live()?;
        let context = authority(self, session)?;
        if self
            .catalog
            .store
            .journal_session_erased(&context)
            .map_err(journal_error)?
        {
            return Ok(false);
        }
        self.pin_journal_session(session)?;
        let sources = self.journal_sources(session)?;
        let existing = self
            .catalog
            .store
            .journal_for_session(&context)
            .map_err(journal_error)?;
        let mut added = false;
        for source in &sources {
            let status = existing.as_deref().map(|id| {
                self.catalog
                    .store
                    .read_journal_source(&context, id, source.source.order)
            });
            match status {
                Some(Ok(status)) => {
                    added |= !status.accepted.iter().any(|range| {
                        range.start <= source.bytes.start && source.bytes.end <= range.end
                    });
                }
                None | Some(Err(JournalError::NotFound(_))) => added = true,
                Some(Err(error)) => return Err(journal_error(error)),
            }
        }
        for batch in sources.chunks(MAX_JOURNAL_RANGES) {
            self.catalog
                .store
                .accept_journal_sources(&context, "Session activity", batch)
                .map_err(journal_error)?;
        }
        Ok(added)
    }

    pub(super) fn recover_journals(&self) {
        let result = (|| {
            self.ensure_live()?;
            self.jobs()?.recover_storage().map_err(service_error)?;
            for attribution in self
                .catalog
                .store
                .subject_session_attributions(&self.binding.subject_id)
                .map_err(domain_error)?
            {
                // Old Workers' retained sources are reconciled as well; possession of the
                // Subject lease alone does not bypass the original Worker binding checks.
                if attribution.runtime_id == self.binding.scope_id
                    && self
                        .catalog
                        .store
                        .journal_session_writer_matches(&self.writer, &attribution.session_id)
                        .map_err(journal_error)?
                    && let Err(error) = self.reconcile_journal_session(&attribution.session_id)
                {
                    eprintln!(
                        "standalone Journal Session {} reconciliation blocked: {error}",
                        attribution.session_id
                    );
                }
            }
            Ok::<_, SubjektivHostError>(())
        })();
        if let Err(error) = result {
            eprintln!("standalone Journal reconciliation blocked: {error}");
        }
        self.wake_journals();
    }

    pub(super) fn wake_journals(&self) {
        let mut running = self
            .journal_dispatch
            .lock()
            .expect("Journal dispatcher poisoned");
        if *running || !self.live.load(Ordering::Acquire) {
            return;
        }
        let Some(host) = self.self_weak.upgrade() else {
            return;
        };
        // Offline admission (for example a deletion preflight) persists intent without
        // assuming a Tokio executor or creating another Journal-specific runner.
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        *running = true;
        runtime.spawn(async move {
            loop {
                let next = {
                    let mut running = host
                        .journal_dispatch
                        .lock()
                        .expect("Journal dispatcher poisoned");
                    match host.next_journal_job() {
                        Ok(Some(job)) => Some(job),
                        Ok(None) => {
                            *running = false;
                            None
                        }
                        Err(error) => {
                            eprintln!("standalone Journal generation remains pending: {error}");
                            *running = false;
                            None
                        }
                    }
                };
                let Some((jobs, id)) = next else {
                    return;
                };
                if let Err(error) = jobs.start(&id).await {
                    eprintln!("standalone Journal Job {id} remains pending: {error}");
                    *host
                        .journal_dispatch
                        .lock()
                        .expect("Journal dispatcher poisoned") = false;
                    return;
                }
                match jobs.wait(&id).await {
                    Ok(snapshot) if snapshot.state == JobState::Completed => {
                        if let Err(error) = host.complete_journal_job(&jobs, &snapshot) {
                            eprintln!("standalone Journal completion remains unconfirmed: {error}");
                            *host
                                .journal_dispatch
                                .lock()
                                .expect("Journal dispatcher poisoned") = false;
                            return;
                        }
                        // Continue even without a further user turn. A later admission has
                        // remained in the ledger, never in this running Job's frozen input.
                    }
                    Ok(snapshot)
                        if snapshot.state == JobState::Failed
                            && snapshot.executions_released()
                            && !snapshot.version_stopped =>
                    {
                        // The next pass reconciles saved operations/checkpoints and resumes
                        // this exact Job/input within its attempt budget, without resampling.
                    }
                    Ok(_) | Err(_) => {
                        // Neither failure nor a missing task proves resource shutdown.
                        *host
                            .journal_dispatch
                            .lock()
                            .expect("Journal dispatcher poisoned") = false;
                        return;
                    }
                }
            }
        });
    }

    fn next_journal_job(
        &self,
    ) -> Result<Option<(crate::jobs::StandaloneJobs, String)>, SubjektivHostError> {
        self.ensure_live()?;
        let jobs = self.jobs()?;
        for saved in jobs.unacknowledged_results().map_err(service_error)? {
            if self.journal_grant_scope(&saved).is_ok() {
                self.complete_journal_job(&jobs, &saved)?;
            }
        }
        for pending in jobs.pending().map_err(service_error)? {
            if pending.request.purpose != PURPOSE {
                continue;
            }
            self.journal_grant(&pending)?;
            return Ok(Some((jobs, pending.request.job_id)));
        }
        let reader =
            JournalHostContext::validated_reader(&self.catalog.store, &self.binding.subject_id)
                .map_err(journal_error)?;
        let mut after = None;
        loop {
            let page = self
                .catalog
                .store
                .list_journals(&reader, after.as_deref(), MAX_JOURNAL_PAGE)
                .map_err(journal_error)?;
            for journal in page.items {
                if !self
                    .catalog
                    .store
                    .journal_session_writer_matches(&self.writer, &journal.source_session_id)
                    .map_err(journal_error)?
                {
                    continue;
                }
                let context = authority(self, &journal.source_session_id)?;
                // Resolve every saved owner before admitting a new prefix. A failed
                // Job is not permission to resample its input into a replacement Job.
                let mut after_job = None;
                loop {
                    let saved_page = jobs
                        .list_session_snapshots(
                            &journal.source_session_id,
                            after_job.as_deref(),
                            100,
                        )
                        .map_err(service_error)?;
                    if saved_page.is_empty() {
                        break;
                    }
                    for saved in &saved_page {
                        if saved.request.purpose != PURPOSE {
                            continue;
                        }
                        let grant = self.journal_grant_scope(saved)?;
                        if grant.journal_id != journal.id {
                            continue;
                        }
                        if saved.version_stopped {
                            // Migration is not normal recovery or Journal completion.
                            // Keep the original ranges/input, even when fully checkpointed.
                            if !self.saved_ranges_reflected(&context, &grant)?
                                || !saved.executions_released()
                            {
                                return Err(SubjektivHostError::Conflict { code: "journal_version_stopped".into(), message: "Version-stopped Journal retains input/checkpoint and unproven execution obligations".into() });
                            }
                            continue;
                        }
                        match saved.state {
                            JobState::Completed => self.complete_journal_job(&jobs, saved)?,
                            JobState::Pending
                                if saved.attempt.state == JobAttemptState::Reserved =>
                            {
                                self.journal_grant(saved)?;
                                return Ok(Some((jobs, saved.request.job_id.clone())));
                            }
                            JobState::Pending | JobState::Failed => {
                                if let Some(resumed) = self.recover_journal_job(&jobs, saved)? {
                                    return Ok(Some((jobs, resumed.request.job_id)));
                                }
                            }
                            JobState::Cancelled => {
                                if !saved.executions_released()
                                    || !self.saved_ranges_reflected(&context, &grant)?
                                {
                                    return Err(SubjektivHostError::Conflict {code:"journal_cancelled_input_retained".into(),message:"Cancelled Journal has unreleased execution or unreflected saved input".into()});
                                }
                            }
                        }
                    }
                    after_job = saved_page.last().map(|job| job.request.job_id.clone());
                }
                let input = self.build_journal_input(&context, &journal.id, None)?;

                if input.selected_ranges.is_empty() {
                    continue;
                }
                let grant = LocalJournalGrant {
                    writer_binding_id: Some(self.writer.binding_id.clone()),
                    binding: self.binding.clone(),
                    session_id: journal.source_session_id,
                    journal_id: journal.id.clone(),
                    ranges: input.selected_ranges.clone(),
                    origins: vec![],
                };
                let request = self.journal_request(&journal.id, &input)?;
                let saved = jobs
                    .request_granted(
                        request,
                        serde_json::to_value(grant)
                            .map_err(|error| SubjektivHostError::Unavailable(error.to_string()))?,
                    )
                    .map_err(service_error)?;
                return Ok(Some((jobs, saved.request.job_id)));
            }
            after = page.next;
            if after.is_none() {
                return Ok(None);
            }
        }
    }

    fn saved_ranges_reflected(
        &self,
        context: &JournalHostContext,
        grant: &LocalJournalGrant,
    ) -> Result<bool, SubjektivHostError> {
        for range in &grant.ranges {
            let status = self
                .catalog
                .store
                .read_journal_source(context, &grant.journal_id, range.source.order)
                .map_err(journal_error)?;
            if status.source != range.source || status.reflected_bytes < range.bytes.end {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn recover_journal_job(
        &self,
        jobs: &crate::jobs::StandaloneJobs,
        saved: &JobSnapshot,
    ) -> Result<Option<JobSnapshot>, SubjektivHostError> {
        let current = jobs.get(&saved.request.job_id).map_err(service_error)?;
        if current.attempt.attempt_id != saved.attempt.attempt_id
            || current.input_digest != saved.input_digest
        {
            return Err(SubjektivHostError::Conflict {
                code: "journal_attempt_changed".into(),
                message: "Journal recovery snapshot no longer names the current attempt/input"
                    .into(),
            });
        }
        let saved = &current;
        if saved.version_stopped {
            return Err(SubjektivHostError::Conflict {
                code: "journal_version_stopped".into(),
                message:
                    "Version-stopped Journal cannot resume or convert checkpoints into completion"
                        .into(),
            });
        }
        self.journal_grant(saved)?; // verifies the immutable body; never fetch live text
        let grant = self.journal_grant_scope(saved)?;
        let context = authority(self, &grant.session_id)?;
        let binding = JobAttemptBinding {
            job_id: saved.request.job_id.clone(),
            attempt_id: saved.attempt.attempt_id.clone(),
        };
        let generation = self
            .catalog
            .store
            .retained_journal_generation(&context, &grant.journal_id, &binding)
            .map_err(journal_error)?;
        if let Some(generation) = generation.as_ref() {
            let recovery = JournalJobContext::validated_recovery(
                &self.catalog.store,
                &context,
                &generation.id,
                binding.clone(),
            )
            .map_err(journal_error)?;
            match subjektiv::journal::service::finish(&self.catalog.store, &recovery) {
                Ok(completion) => {
                    let settled = jobs
                        .accept_reconciled(&job::JobResultSubmission {
                            job_id: binding.job_id.clone(),
                            attempt_id: binding.attempt_id.clone(),
                            input_digest: saved.input_digest.clone(),
                            result: serde_json::to_value(completion)
                                .map_err(|e| SubjektivHostError::Unavailable(e.to_string()))?,
                        })
                        .map_err(service_error)?;
                    self.complete_journal_job(jobs, &settled)?;
                    return Ok(None);
                }
                Err(JournalError::Conflict(_)) => {} // incomplete, retained checkpoint
                Err(error) => return Err(journal_error(error)),
            }
        }
        if !saved.executions_released() {
            return Err(SubjektivHostError::Conflict {code:"journal_execution_unreleased".into(),message:"Saved Journal input/operations retained; preceding execution/cleanup is not proven stopped".into()});
        }
        let receipt = if let Some(generation) = generation {
            self.catalog
                .store
                .close_journal_generation(&context, &generation.id, &binding)
                .map_err(journal_error)?;
            format!("journal-generation-closed:{}", generation.id)
        } else {
            // The authoritative domain lookup found no generation for this exact attempt.
            "journal-no-generation".to_string()
        };
        jobs.reconcile_granted_attempt(&binding.job_id, &binding.attempt_id, &receipt)
            .map_err(service_error)?;
        let resumed = jobs
            .resume_granted(&binding.job_id, &binding.attempt_id)
            .map_err(service_error)?;
        Ok(Some(resumed))
    }

    fn journal_request(
        &self,
        journal_id: &str,
        input: &JournalInput,
    ) -> Result<job::JobRequest, SubjektivHostError> {
        Ok(job::JobRequest {
            job_id: format!("subjektiv-journal:{}", uuid::Uuid::now_v7()), purpose: PURPOSE.into(),
            input_ref: format!("subjektiv://{}/journal/{journal_id}", self.binding.subject_id),
            input: serde_json::to_value(input).map_err(|e| SubjektivHostError::Unavailable(e.to_string()))?,
            instruction: "Input contains the frozen canonical public source bodies, provenance and byte ranges. Use only this saved input, never re-fetch Source. ReadJournalState first and read initial/current Journal position/open text in bounded pages if needed. Reconcile saved operation receipts before new creates. Save sequential Create/Rewrite/Fix and UpdateJournalPosition operations, then CheckpointJournal backed by saved record/no-change receipts for all selected input. Do not fix an ongoing event just because this Job/turn ended. SubmitJobResult with result: {} only after the entire selected prefix is reflected. Never infer tool success or user intent not evidenced by public input.".into(),
            profile: "builtin:subjektiv-journal-recording".into(),
            serialization_key: Some(format!("subjektiv-journal:{journal_id}")),
            limits: job::JobLimits { timeout_seconds: 600, ..Default::default() },
        })
    }

    fn pin_journal_session(&self, session: &str) -> Result<(), SubjektivHostError> {
        let path = crate::subjektiv_sessions::authorized_sessions_path(
            &self.catalog.state_root,
            &self.binding.scope_id,
            &self.binding.subject_id,
            &self.catalog.store,
            session,
        )?;
        let session = session
            .parse()
            .map_err(|_| SubjektivHostError::InvalidArgument("invalid Session".into()))?;
        // A Session owns exactly one Journal; its immutable random ID is the stable pin
        // identity. Pins and erase fences outlive relocation in the Host-wide control root.
        session_store::FsStore::new_with_retention(
            path,
            crate::store::session_retention(&self.catalog.state_root),
        )
        .map_err(source_error)?
        .pin_session(
            session,
            session_store::retention::SessionRetentionPin(session),
        )
        .map_err(source_error)
    }

    fn session_fully_reflected(&self, session: &str) -> Result<bool, SubjektivHostError> {
        if self.journal_session_erased(session)? {
            return Ok(true);
        }
        let sources = self.journal_sources(session)?;
        if sources.is_empty() {
            return Ok(true);
        }
        let context = authority(self, session)?;
        let mut after = None;
        loop {
            let page = self
                .catalog
                .store
                .list_journals(&context, after.as_deref(), MAX_JOURNAL_PAGE)
                .map_err(journal_error)?;
            if let Some(journal) = page
                .items
                .into_iter()
                .find(|journal| journal.source_session_id == session)
            {
                for range in sources {
                    let status = self
                        .catalog
                        .store
                        .read_journal_source(&context, &journal.id, range.source.order)
                        .map_err(journal_error)?;
                    if status.source != range.source
                        || status.reflected_bytes < range.source.total_bytes
                    {
                        return Ok(false);
                    }
                }
                return Ok(true);
            }
            after = page.next;
            if after.is_none() {
                return Ok(false);
            }
        }
    }

    fn prune_retained_workers(&self) -> Result<(), SubjektivHostError> {
        let root = retained_workers_root(&self.catalog.state_root);
        if !root.exists() {
            return Ok(());
        }
        let workers = crate::store::StandaloneWorkerStore::open_existing(&root)
            .map_err(|error| SubjektivHostError::Unavailable(error.to_string()))?;
        for entry in fs::read_dir(&root)
            .map_err(|error| SubjektivHostError::Unavailable(error.to_string()))?
        {
            let entry =
                entry.map_err(|error| SubjektivHostError::Unavailable(error.to_string()))?;
            let Ok(worker) = entry
                .file_name()
                .to_string_lossy()
                .parse::<protocol::WorkerId>()
            else {
                continue;
            };
            let record = workers
                .load(worker)
                .map_err(|error| SubjektivHostError::Unavailable(error.to_string()))?;
            if record.subject.as_ref() != Some(&self.binding) {
                continue;
            }
            let mut all_reflected = true;
            let sessions_path = workers.sessions_dir(worker);
            if !sessions_path.is_dir() {
                return Err(SubjektivHostError::Unavailable(
                    "retained Session storage is missing".into(),
                ));
            }
            let source_store = session_store::FsStore::new_with_retention(
                sessions_path,
                crate::store::session_retention(&self.catalog.state_root),
            )
            .map_err(source_error)?;
            for session in
                session_store::Store::list_sessions(&source_store).map_err(source_error)?
            {
                let attribution = self
                    .catalog
                    .store
                    .session_attribution(&session.to_string())
                    .map_err(domain_error)?
                    .ok_or_else(|| {
                        SubjektivHostError::Unavailable("retained Session is unattributed".into())
                    })?;
                if attribution.worker_id != worker.to_string()
                    || attribution.subject_id != self.binding.subject_id
                    || attribution.runtime_id != self.binding.scope_id
                    || !self.session_fully_reflected(&session.to_string())?
                {
                    all_reflected = false;
                    break;
                }
            }
            if all_reflected {
                for session in
                    session_store::Store::list_sessions(&source_store).map_err(source_error)?
                {
                    self.prune_source_session(&source_store, session)?;
                }
                // These are the relocated originals of an already removed, stopped Worker.
                // No normal restore path addresses this namespace; keep no duplicate history.
                fs::remove_dir_all(entry.path())
                    .map_err(|error| SubjektivHostError::Unavailable(error.to_string()))?;
                File::open(&root)
                    .and_then(|file| file.sync_all())
                    .map_err(|error| SubjektivHostError::Unavailable(error.to_string()))?;
            }
        }
        Ok(())
    }

    fn with_recording_boundary<T>(
        &self,
        session: &str,
        read: impl FnOnce(
            &Path,
            session_store::SessionId,
            session_store::SegmentId,
            &session_store::retention::SessionRetentionGuard,
        ) -> Result<T, SubjektivHostError>,
    ) -> Result<T, SubjektivHostError> {
        let sessions = crate::subjektiv_sessions::authorized_sessions_path(
            &self.catalog.state_root,
            &self.binding.scope_id,
            &self.binding.subject_id,
            &self.catalog.store,
            session,
        )?;
        let attribution = self
            .catalog
            .store
            .session_attribution(session)
            .map_err(domain_error)?
            .ok_or_else(|| {
                SubjektivHostError::InvalidArgument("Session attribution unavailable".into())
            })?;
        let worker_root = sessions.parent().and_then(Path::parent).ok_or_else(|| {
            SubjektivHostError::InvalidArgument("invalid retained Worker path".into())
        })?;
        let workers = crate::store::StandaloneWorkerStore::open_existing(worker_root)
            .map_err(|error| SubjektivHostError::Unavailable(error.to_string()))?;
        let worker = attribution
            .worker_id
            .parse()
            .map_err(|_| SubjektivHostError::InvalidArgument("invalid attributed Worker".into()))?;
        let record = workers
            .load(worker)
            .map_err(|error| SubjektivHostError::Unavailable(error.to_string()))?;
        let metadata_root = sessions
            .parent()
            .ok_or_else(|| SubjektivHostError::InvalidArgument("invalid Session storage".into()))?
            .join("worker")
            .join(&record.storage_key);
        let session_id = session
            .parse()
            .map_err(|_| SubjektivHostError::InvalidArgument("invalid Session".into()))?;
        let retention = crate::store::session_retention(&self.catalog.state_root);
        let guard = retention.lock_live(session_id).map_err(source_error)?;
        // This is the persisted active selector, not the newest filename/mtime or a model claim.
        let metadata =
            session_store::WorkerAggregateStore::open_read_only(metadata_root, &record.storage_key)
                .and_then(|store| store.read_read_only())
                .map_err(|error| SubjektivHostError::Unavailable(error.to_string()))?
                .ok_or_else(|| {
                    SubjektivHostError::Unavailable(
                        "Worker active Session metadata is missing".into(),
                    )
                })?;
        let active = metadata
            .active
            .filter(|active| active.session_id.to_string() == session)
            .or_else(|| {
                (record.status == crate::store::StandaloneWorkerStatus::Stopped
                    && record.active_session_id.to_string() == session)
                    .then_some(session_store::WorkerActiveSegmentRef {
                        session_id: record.active_session_id,
                        segment_id: record.active_segment_id,
                    })
            })
            .and_then(|active| active.segment_id)
            .ok_or_else(|| {
                SubjektivHostError::Unavailable(
                    "no attested active lineage for the retained Session".into(),
                )
            })?;
        read(&sessions, session_id, active, &guard)
    }

    fn recording_capture(&self, session: &str) -> Result<CaptureDto, SubjektivHostError> {
        self.with_recording_boundary(session, |sessions, session_id, active, guard| {
            session_store::capture::capture_recording_fs_guarded(
                sessions,
                session_id,
                active,
                session_store::public_material::PublicMaterialLimits::default(),
                CaptureLimits {
                    max_bytes: 128 * 1024 * 1024,
                    max_entries: 100_000,
                    max_text_bytes: 64 * 1024 * 1024,
                    ..Default::default()
                },
                guard,
            )
            .map_err(|e| SubjektivHostError::Unavailable(e.to_string()))
        })
    }

    fn journal_sources(
        &self,
        session: &str,
    ) -> Result<Vec<JournalSourceRange>, SubjektivHostError> {
        self.with_recording_boundary(session, |sessions, session_id, active, guard| {
            session_store::capture::recording_catalog_fs_guarded(
                sessions,
                session_id,
                active,
                session_store::public_material::PublicMaterialLimits::default(),
                guard,
                |descriptors| {
                    descriptors
                        .iter()
                        .map(|descriptor| JournalSourceRange {
                            source: source_descriptor(session, descriptor),
                            bytes: JournalByteRange {
                                start: 0,
                                end: descriptor.total_bytes,
                            },
                        })
                        .collect()
                },
            )
            .map_err(|e| SubjektivHostError::Unavailable(e.to_string()))
        })
    }

    fn build_journal_input(
        &self,
        context: &JournalHostContext,
        id: &str,
        saved: Option<&CaptureDto>,
    ) -> Result<JournalInput, SubjektivHostError> {
        let session = self
            .catalog
            .store
            .read_journal(context, id)
            .map_err(journal_error)?
            .source_session_id;
        JournalInput::build(&self.catalog.store, context, id, |order, offset, budget| {
            let capture = match saved {
                Some(capture) => capture.clone(),
                None => {
                    let capture = self
                        .with_recording_boundary(&session, |sessions, session_id, active, guard| {
                            session_store::capture::capture_recording_slice_fs_guarded(
                                sessions,
                                session_id,
                                active,
                                session_store::public_material::PublicMaterialLimits::default(),
                                RecordingPosition {
                                    canonical_order: order,
                                    byte_offset: offset,
                                },
                                MAX_JOURNAL_RANGES,
                                budget,
                                guard,
                            )
                            .map(|slice| slice.capture)
                            .map_err(|e| SubjektivHostError::Unavailable(e.to_string()))
                        })
                        .map_err(|e| JournalError::Conflict(e.to_string()))?;
                    // Admission and capture are separate saves. An append between them
                    // may be in replay but has not yet been accepted for this Journal.
                    return self.accepted_capture_prefix(context, id, capture);
                }
            };
            let end = capture
                .entries
                .last()
                .map(|entry| {
                    capture
                        .recording_text(entry.order)
                        .map(|(descriptor, start, text)| {
                            if start + text.len() as u64 == descriptor.total_bytes {
                                RecordingPosition {
                                    canonical_order: entry.order + 1,
                                    byte_offset: 0,
                                }
                            } else {
                                RecordingPosition {
                                    canonical_order: entry.order,
                                    byte_offset: start + text.len() as u64,
                                }
                            }
                        })
                })
                .transpose()
                .map_err(|e| JournalError::Conflict(e.to_string()))?;
            if end.is_none_or(|end| (order, offset) >= (end.canonical_order, end.byte_offset)) {
                return Ok(CaptureDto {
                    entries: vec![],
                    ..capture
                });
            }
            capture
                .recording_slice(
                    RecordingPosition {
                        canonical_order: order,
                        byte_offset: offset,
                    },
                    MAX_JOURNAL_RANGES,
                    budget,
                )
                .map(|slice| slice.capture)
                .map_err(|e| JournalError::Conflict(e.to_string()))
        })
        .map_err(journal_error)
    }

    fn accepted_capture_prefix(
        &self,
        context: &JournalHostContext,
        id: &str,
        mut capture: CaptureDto,
    ) -> JournalResult<CaptureDto> {
        let mut selected = Vec::new();
        for mut entry in capture.entries {
            let status = match self
                .catalog
                .store
                .read_journal_source(context, id, entry.order)
            {
                Ok(status) => status,
                Err(JournalError::NotFound(_)) => break,
                Err(error) => return Err(error),
            };
            let session_store::capture::CaptureBody::CanonicalPublic {
                descriptor,
                byte_offset,
                text,
                ..
            } = &mut entry.body
            else {
                return Err(JournalError::InvalidInput("non-recording capture".into()));
            };
            if source_descriptor(&capture.session_id, descriptor) != status.source {
                return Err(JournalError::Conflict(
                    "captured source differs from admitted identity".into(),
                ));
            }
            let Some(accepted) = status
                .accepted
                .iter()
                .find(|range| range.start <= *byte_offset && *byte_offset < range.end)
            else {
                break;
            };
            let end = status
                .unavailable
                .iter()
                .filter(|range| range.end > *byte_offset)
                .fold(accepted.end, |end, range| {
                    end.min(range.start.max(*byte_offset))
                });
            let mut length = text.len().min(end.saturating_sub(*byte_offset) as usize);
            while !text.is_char_boundary(length) {
                length -= 1;
            }
            if length == 0 {
                break;
            }
            text.truncate(length);
            let partial = *byte_offset + (text.len() as u64) < descriptor.total_bytes;
            selected.push(entry);
            if partial {
                break;
            }
        }
        capture.entries = selected;
        Ok(capture)
    }

    fn journal_grant_scope(
        &self,
        snapshot: &JobSnapshot,
    ) -> Result<LocalJournalGrant, SubjektivHostError> {
        let grant: LocalJournalGrant =
            serde_json::from_value(snapshot.domain_grant.clone().ok_or_else(|| {
                SubjektivHostError::InvalidArgument("missing Journal delegation".into())
            })?)
            .map_err(|error| SubjektivHostError::InvalidArgument(error.to_string()))?;
        if grant.binding != self.binding
            || grant
                .writer_binding_id
                .as_ref()
                .is_some_and(|id| id != &self.writer.binding_id)
            || snapshot.request.purpose != PURPOSE
            || snapshot.request.serialization_key.as_deref()
                != Some(&format!("subjektiv-journal:{}", grant.journal_id))
        {
            return Err(SubjektivHostError::InvalidArgument(
                "Job input/grant does not match this Journal capability".into(),
            ));
        }
        // Pre-binding persisted Jobs lack the new field. They may resume only after the
        // Host has bound their ORIGINAL Session to this lifetime. Session binding is immutable;
        // missing grant metadata never maps an old Session onto a later body/reconnection.
        if !self
            .catalog
            .store
            .journal_session_writer_matches(&self.writer, &grant.session_id)
            .map_err(journal_error)?
            || !self
                .catalog
                .store
                .journal_writer_active(&self.writer)
                .map_err(journal_error)?
        {
            return Err(SubjektivHostError::InvalidArgument(
                "Journal Job's original Session is not bound to this active body lifetime".into(),
            ));
        }
        let context = authority(self, &grant.session_id)?;
        let journal = self
            .catalog
            .store
            .read_journal(&context, &grant.journal_id)
            .map_err(journal_error)?;
        if journal.source_session_id != grant.session_id || grant.ranges.is_empty() {
            return Err(SubjektivHostError::InvalidArgument(
                "Journal input belongs to another Session or is empty".into(),
            ));
        }
        Ok(grant)
    }

    fn journal_grant(
        &self,
        snapshot: &JobSnapshot,
    ) -> Result<LocalJournalGrant, SubjektivHostError> {
        snapshot
            .load_executable_request()
            .map_err(|e| SubjektivHostError::InvalidArgument(e.to_string()))?;
        let grant = self.journal_grant_scope(snapshot)?;
        let input = JournalInput::decode(&snapshot.request.input).map_err(journal_error)?;
        if input.subject_id != self.binding.subject_id
            || input.journal_id != grant.journal_id
            || input.capture.session_id != grant.session_id
            || input.selected_ranges != grant.ranges
        {
            return Err(SubjektivHostError::InvalidArgument(
                "saved Journal body identity/ranges differ from grant".into(),
            ));
        }
        Ok(grant)
    }

    fn complete_journal_job(
        &self,
        jobs: &crate::jobs::StandaloneJobs,
        snapshot: &JobSnapshot,
    ) -> Result<(), SubjektivHostError> {
        let grant = self.journal_grant_scope(snapshot)?;
        let context = authority(self, &grant.session_id)?;
        let completion: JournalJobCompletion =
            serde_json::from_value(snapshot.result.clone().ok_or_else(|| {
                SubjektivHostError::Unavailable("Journal Job has no accepted result".into())
            })?)
            .map_err(|error| SubjektivHostError::Unavailable(error.to_string()))?;
        let generation = self
            .catalog
            .store
            .read_journal_generation(&context, &completion.generation_id)
            .map_err(journal_error)?;
        if generation.journal_id != grant.journal_id
            || generation.job.job_id != snapshot.request.job_id
            || generation.job.attempt_id != snapshot.attempt.attempt_id
        {
            return Err(SubjektivHostError::InvalidArgument(
                "Journal completion binding mismatch".into(),
            ));
        }
        let recovery = JournalJobContext::validated_recovery(
            &self.catalog.store,
            &context,
            &generation.id,
            generation.job.clone(),
        )
        .map_err(journal_error)?;
        let saved = subjektiv::journal::service::finish(&self.catalog.store, &recovery)
            .map_err(journal_error)?;
        if saved != completion {
            return Err(SubjektivHostError::Conflict { code: "journal_completion_mismatch".into(), message: "Job result differs from the saved Journal activity, position and checkpoint completion".into() });
        }
        jobs.acknowledge(&snapshot.request.job_id)
            .map_err(service_error)?;
        if !snapshot.executions_released() {
            return Err(SubjektivHostError::Conflict {code:"journal_cleanup_unconfirmed".into(),message:"Checkpoint/result completed; execution cleanup remains independent and unconfirmed".into()});
        }
        jobs.reclaim_input(
            &snapshot.request.job_id,
            &snapshot.input_digest,
            JobInputRetirementReason::Completed,
        )
        .map_err(service_error)?;
        if let Err(error) = self.prune_retained_workers() {
            // Pruning is independent of a successful checkpoint/result. Keep the originals
            // and continue draining other saved prefixes; a retention error is not model failure.
            eprintln!("standalone reflected Journal source cleanup deferred: {error}");
        }
        Ok(())
    }
}

pub(super) fn grant(
    host: Arc<LocalSubjectHost>,
    snapshot: &JobSnapshot,
    fence: Arc<dyn crate::jobs::JobAttemptFence>,
) -> Result<worker::job::JobFeatureGrant, String> {
    let grant = host
        .journal_grant(snapshot)
        .map_err(|error| error.to_string())?;
    let context = authority(&host, &grant.session_id).map_err(|error| error.to_string())?;
    let input = JournalInput::decode(&snapshot.request.input).map_err(|e| e.to_string())?;
    let input_digest = snapshot.input_digest.clone();
    let generation = host
        .catalog
        .store
        .prepare_journal_input(
            &context,
            &input,
            JobAttemptBinding {
                job_id: snapshot.request.job_id.clone(),
                attempt_id: snapshot.attempt.attempt_id.clone(),
            },
        )
        .map_err(|error| error.to_string())?;
    let job_context = JournalJobContext::validated(
        &host.catalog.store,
        &context,
        &generation.id,
        generation.job.clone(),
    )
    .map_err(|error| error.to_string())?;
    input
        .validate_generation(&generation)
        .map_err(|e| e.to_string())?;
    let adapter = Arc::new(LocalJournalHost {
        host,
        grant,
        context,
        job_context,
        #[cfg(test)]
        generation,
        input_digest,
        fence,
    });
    Ok(worker::job::JobFeatureGrant {
        features: worker::feature::FeatureRegistryBuilder::new()
            .with_module(JournalJobToolsFeature::new(adapter.clone())),
        satisfied_requirements: vec![],
        finalizer: Some(Arc::new(JournalJobFinalizer::new(adapter))),
    })
}

struct LocalJournalHost {
    host: Arc<LocalSubjectHost>,
    grant: LocalJournalGrant,
    context: JournalHostContext,
    job_context: JournalJobContext,
    #[cfg(test)]
    generation: JournalGeneration,
    input_digest: String,
    fence: Arc<dyn crate::jobs::JobAttemptFence>,
}
impl LocalJournalHost {
    fn live(&self) -> Result<JournalJobContext, SubjektivHostError> {
        self.host.ensure_live()?;
        self.fence
            .ensure_live()
            .map_err(SubjektivHostError::Unavailable)?;
        if self
            .host
            .catalog
            .store
            .journal_for_session(&self.context)
            .map_err(journal_error)?
            .as_deref()
            != Some(&self.grant.journal_id)
        {
            return Err(SubjektivHostError::InvalidArgument(
                "Journal Session binding was erased or changed".into(),
            ));
        }
        // The shared service validates active domain authority inside each operation. Reuse
        // this bound context for finish replay, which may legitimately be already closed.
        Ok(self.job_context.clone())
    }
}
impl JournalJobHost for LocalJournalHost {
    fn execute(
        &self,
        request: JournalJobRequest,
    ) -> Result<JournalJobResponse, SubjektivHostError> {
        let _effects = self
            .fence
            .enter()
            .map_err(SubjektivHostError::Unavailable)?;
        let context = self.live()?;
        // The generic runner has supplied this exact immutable input before it can
        // invoke this bound tool. Preparation alone is not evidence of delivery.
        subjektiv::journal::service::attest_input_delivery(
            &self.host.catalog.store,
            &context,
            &self.input_digest,
            &self.grant.ranges,
        )
        .map_err(journal_error)?;
        subjektiv::journal::service::execute_snapshot(
            &self.host.catalog.store,
            &context,
            &self.input_digest,
            request,
        )
        .map_err(journal_error)
    }
    fn finish(&self) -> Result<JournalJobCompletion, SubjektivHostError> {
        let _effects = self
            .fence
            .enter()
            .map_err(SubjektivHostError::Unavailable)?;
        let context = self.live()?;
        subjektiv::journal::service::finish(&self.host.catalog.store, &context)
            .map_err(journal_error)
    }
}

fn source_descriptor(
    session: &str,
    descriptor: &session_store::public_material::PublicMaterialDescriptor,
) -> JournalSource {
    JournalSource {
        session_id: session.into(),
        pointer: JournalSourcePointer {
            segment_id: descriptor.identity.segment_id,
            entry_ref: descriptor.identity.entry_ref.clone(),
        },
        order: descriptor.canonical_order,
        content_sha256: descriptor.sha256.clone(),
        total_bytes: descriptor.total_bytes,
    }
}
