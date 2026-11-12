//! Journal admission and existing-Job orchestration. Domain receipts remain in Subjektiv;
//! the Backend owns authenticated source identity and live attempt capabilities.
use super::*;
use crate::backend_job::{
    BackendJobGrants, BackendJobRecord, SubjektivJournalGrant, journal_serialization_key,
};
use subjektiv::journal::recording::JournalInput;
use subjektiv::journal::transport::*;
use subjektiv::journal::{self, *};

#[path = "journal_removal.rs"]
mod removal;
#[path = "journal_retained_source.rs"]
mod retained;
pub(super) use removal::{advance_worker_removal, removal_observation, resume_removals};
pub(super) use retained::RetainedSourceProgress;

pub(super) fn domain(error: JournalError) -> Error {
    match error {
        JournalError::PermissionDenied(s) => Error::WorkspacePermissionDenied(s),
        JournalError::Conflict(s) => Error::RepositoryConflict(s),
        JournalError::Storage(error) => Error::SubjektivStorage(error),
        JournalError::Subject(error) => subjektiv_store_error(error),
        other => Error::Store(other.to_string()),
    }
}

fn store(api: &WorkspaceApi) -> Result<crate::subjektiv::SubjektivStore> {
    crate::subjektiv::SubjektivStore::open(&api.feature_storage, &api.subjektiv_registration)
        .map_err(subjektiv_store_error)
}

pub(super) fn host(
    _api: &WorkspaceApi,
    store: &crate::subjektiv::SubjektivStore,
    grant: &SubjektivJournalGrant,
) -> Result<JournalHostContext> {
    let attribution = store
        .session_attribution(&grant.session_id)
        .map_err(subjektiv_store_error)?
        .filter(|a| a.subject_id == grant.subject_id)
        .ok_or_else(|| {
            Error::WorkspacePermissionDenied("Journal Session attribution mismatch".into())
        })?;
    let host = JournalHostContext::validated_session(store, attribution).map_err(domain)?;
    // This is source-lifetime provenance, not authority borrowed from a current body.
    // Inactive originals are valid; a missing or different lifetime is never inferred.
    if host.writer_binding_id() != Some(grant.binding_id.as_str()) {
        return Err(Error::WorkspacePermissionDenied(
            "Journal source lifetime mismatch".into(),
        ));
    }
    Ok(host)
}

/// Recording authority belongs to the saved Job, not the source body's lease.
/// Cleanup/erasure use the attribution-only context above; they must not issue
/// new recording authority while retiring an execution.
fn recording_host(
    api: &WorkspaceApi,
    store: &crate::subjektiv::SubjektivStore,
    request: &BackendJobRequest,
    attempt: &BackendJobAttemptRecord,
) -> Result<JournalHostContext> {
    let grant = request
        .grants
        .subjektiv_journal
        .as_ref()
        .ok_or_else(|| Error::WorkspacePermissionDenied("Journal Job grant missing".into()))?;
    saved_input(request, grant)?;
    host(api, store, grant)?;
    let attribution = store
        .session_attribution(&grant.session_id)
        .map_err(subjektiv_store_error)?
        .filter(|attr| attr.subject_id == grant.subject_id)
        .ok_or_else(|| {
            Error::WorkspacePermissionDenied("Journal source attribution missing".into())
        })?;
    if request.source_worker.as_ref()
        != Some(&RuntimeWorkerRef::new(
            &attribution.runtime_id,
            &attribution.worker_id,
        ))
        || attempt.workspace_id != api.workspace_id()
        || attempt.job_id != request.job_id
        || attempt.attempt_id != crate::backend_job::attempt_id(&request.job_id, attempt.attempt)
        || attempt.input_digest != request.input_digest()?
    {
        return Err(Error::WorkspacePermissionDenied(
            "Journal saved provenance/attempt mismatch".into(),
        ));
    }
    subjektiv_deletion::require_available(api, &grant.subject_id)?;
    JournalHostContext::validated_recording_job(
        store,
        attribution,
        &grant.journal_id,
        subjektiv::api::JobAttemptBinding {
            job_id: request.job_id.clone(),
            attempt_id: attempt.attempt_id.clone(),
        },
    )
    .map_err(domain)
}

/// Preparing before dispatch is also used by Reserved-attempt restart recovery. A retry
/// with the same attempt and complete selection converges on the original snapshot.
pub(super) fn prepare(api: &WorkspaceApi, reservation: &BackendJobReservation) -> Result<()> {
    let Some(grant) = &reservation.job.request.grants.subjektiv_journal else {
        return Ok(());
    };
    require_job_workspace(api, &reservation.job)?;
    let store = store(api)?;
    let current = api
        .store
        .get_backend_job(api.workspace_id(), &reservation.job.request.job_id)?
        .ok_or_else(|| Error::WorkspacePermissionDenied("Journal Job missing".into()))?;
    let attempt = api
        .store
        .get_backend_job_attempt(
            api.workspace_id(),
            &current.request.job_id,
            &reservation.attempt.attempt_id,
        )?
        .ok_or_else(|| Error::WorkspacePermissionDenied("Journal attempt missing".into()))?;
    if current != reservation.job
        || attempt != reservation.attempt
        || current.state != BackendJobState::Pending
        || current.current_attempt != attempt.attempt
        || attempt.state.terminal()
        || chrono::DateTime::parse_from_rfc3339(&attempt.deadline_at)
            .map_err(|e| Error::Store(e.to_string()))?
            <= Utc::now()
    {
        return Err(Error::WorkspacePermissionDenied(
            "Journal preparation requires the current uncancelled attempt".into(),
        ));
    }
    current.load_executable_request()?;
    subject_authorization::authorize_job_input(api, &current.request)
        .map_err(|error| error.error)?;
    let store = subject_authorization::journal_attempt_effect_store(
        api,
        &store,
        &current,
        &attempt,
        subject_authorization::JournalAttemptEffect::PrepareInput,
    )?;
    let host = recording_host(api, &store, &current.request, &attempt)?;
    let input = saved_input(&reservation.job.request, grant)?;
    let g = store
        .prepare_journal_input(
            &host,
            &input,
            subjektiv::api::JobAttemptBinding {
                job_id: reservation.job.request.job_id.clone(),
                attempt_id: reservation.attempt.attempt_id.clone(),
            },
        )
        .map_err(domain)?;
    input.validate_generation(&g).map_err(domain)?;
    Ok(())
}

fn saved_input(request: &BackendJobRequest, grant: &SubjektivJournalGrant) -> Result<JournalInput> {
    let input = JournalInput::decode(&request.input).map_err(domain)?;
    if input.subject_id != grant.subject_id
        || input.journal_id != grant.journal_id
        || input.capture.session_id != grant.session_id
        || input.selected_ranges != grant.selected_ranges
    {
        return Err(Error::WorkspacePermissionDenied(
            "Journal saved input/capability mismatch".into(),
        ));
    }
    Ok(input)
}

fn live(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
) -> Result<(
    SubjektivJournalGrant,
    BackendJobRecord,
    BackendJobAttemptRecord,
)> {
    let binding = api
        .store
        .worker_registry_projection(api.workspace_id(), worker)?
        .and_then(|p| p.job)
        .ok_or_else(|| {
            Error::WorkspacePermissionDenied("Journal tools require a bound Job Worker".into())
        })?;
    let job = api
        .store
        .get_backend_job(api.workspace_id(), &binding.job_id)?
        .ok_or_else(|| Error::WorkspacePermissionDenied("Journal Job missing".into()))?;
    require_job_workspace(api, &job)?;
    job.load_executable_request()?;
    let attempt = api
        .store
        .get_backend_job_attempt(api.workspace_id(), &binding.job_id, &binding.attempt_id)?
        .ok_or_else(|| Error::WorkspacePermissionDenied("Journal attempt missing".into()))?;
    if job.state != BackendJobState::Pending
        || attempt.state != BackendJobAttemptState::Dispatched
        || attempt.worker.as_ref() != Some(worker)
        || job.current_attempt != attempt.attempt
        || attempt.input_digest != job.request.input_digest()?
        || chrono::DateTime::parse_from_rfc3339(&attempt.deadline_at)
            .map_err(|e| Error::Store(e.to_string()))?
            <= Utc::now()
    {
        return Err(Error::WorkspacePermissionDenied(
            "Journal operation requires the current live uncancelled attempt".into(),
        ));
    }
    let grant = job
        .request
        .grants
        .subjektiv_journal
        .clone()
        .ok_or_else(|| Error::WorkspacePermissionDenied("Job has no Journal capability".into()))?;
    Ok((grant, job, attempt))
}

fn generation(
    store: &crate::subjektiv::SubjektivStore,
    host: &JournalHostContext,
    grant: &SubjektivJournalGrant,
    job: &BackendJobRecord,
    attempt: &BackendJobAttemptRecord,
) -> Result<JournalGeneration> {
    let g = store
        .active_journal_generation(host, &grant.journal_id)
        .map_err(domain)?
        .ok_or_else(|| {
            Error::WorkspacePermissionDenied("Journal generation is no longer active".into())
        })?;
    if g.job.job_id != job.request.job_id
        || g.job.attempt_id != attempt.attempt_id
        || g.selected_ranges != grant.selected_ranges
    {
        return Err(Error::WorkspacePermissionDenied(
            "Journal generation/attempt/range binding mismatch".into(),
        ));
    }
    Ok(g)
}

fn authorize(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
    subject: &str,
    operation: &str,
) -> Result<()> {
    let scope = authorize_subjektiv_operation(api, worker, api.workspace_id(), subject, operation)
        .map_err(|error| error.error)?;
    if scope.is_none() {
        return Err(Error::WorkspacePermissionDenied(
            "Journal requires current Subject operation authority".into(),
        ));
    }
    Ok(())
}

fn authorize_source(
    api: &WorkspaceApi,
    attribution: &crate::subjektiv::SubjectSessionAttribution,
    operation: &str,
) -> Result<()> {
    let worker = RuntimeWorkerRef::new(&attribution.runtime_id, &attribution.worker_id);
    authorize(api, &worker, &attribution.subject_id, operation)
}

fn job_operation(request: &JournalJobRequest) -> &'static str {
    match request {
        JournalJobRequest::Execute { operation } => match &operation.action {
            JournalAction::Create { .. } => "journal.entry.create",
            JournalAction::Rewrite { .. } => "journal.entry.rewrite",
            JournalAction::Fix { .. } => "journal.entry.fix",
            JournalAction::UpdatePosition { .. } => "journal.position.update",
            JournalAction::ConfirmUnchanged => "journal.unchanged.confirm",
            JournalAction::Checkpoint { .. } => "journal.checkpoint",
        },
        JournalJobRequest::ReadState => "journal.state.read",
        JournalJobRequest::QueryOperation { .. } => "journal.operation.query",
        JournalJobRequest::ReadOperationCoverage { .. } => "journal.operation.coverage.read",
        JournalJobRequest::ListOperations { .. } => "journal.operation.list",
        JournalJobRequest::ReadText { .. } => "journal.text.read",
    }
}

enum JournalCaller<'a> {
    Signed(&'a crate::worker_source::VerifiedRuntimeRequestSource),
    #[cfg(test)]
    Internal,
}
impl JournalCaller<'_> {
    fn effect_store(
        &self,
        api: &WorkspaceApi,
        worker: &RuntimeWorkerRef,
        subject: &str,
        operation: &str,
    ) -> Result<crate::subjektiv::SubjektivStore> {
        let store = store(api)?;
        Ok(match self {
            Self::Signed(source) => subject_authorization::authenticated_subject_effect_store(
                api,
                &store,
                worker,
                subject,
                operation,
                &server_api::ServerRuntimeSource {
                    runtime_id: source.runtime_id.clone(),
                    worker_id: source.worker_id.clone(),
                    authentication_binding_id: source.authentication_binding_id.clone(),
                },
            ),
            #[cfg(test)]
            Self::Internal => {
                subject_authorization::subject_effect_store(api, &store, worker, subject, operation)
            }
        })
    }
}

#[cfg(test)]
pub(super) fn execute(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
    request: JournalJobRequest,
) -> Result<JournalJobResponse> {
    execute_for(api, worker, request, JournalCaller::Internal)
}

fn execute_for(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
    request: JournalJobRequest,
    caller: JournalCaller<'_>,
) -> Result<JournalJobResponse> {
    let (grant, _, _) = live(api, worker)?;
    let gate = lifecycle_lock(api, &grant.subject_id)?;
    let _guard = gate
        .lock()
        .map_err(|_| Error::Store("Journal lifecycle lock poisoned".into()))?;
    // Dispatch holds this same gate until matching Runtime acceptance is attested.
    // Revalidate after waiting: erasure, cancellation or replacement may have won.
    let (grant, job, attempt) = live(api, worker)?;
    authorize(api, worker, &grant.subject_id, job_operation(&request))?;
    #[cfg(test)]
    if let Some(observe) = &api.journal_before_effect {
        observe(job_operation(&request));
    }
    let store = caller.effect_store(api, worker, &grant.subject_id, job_operation(&request))?;
    let host = recording_host(api, &store, &job.request, &attempt)?;
    let g = generation(&store, &host, &grant, &job, &attempt)?;
    let input = saved_input(&job.request, &grant)?;
    input.validate_generation(&g).map_err(domain)?;
    let ctx = JournalJobContext::validated(&store, &host, &g.id, g.job.clone()).map_err(domain)?;
    journal::service::execute_snapshot(&store, &ctx, &attempt.input_digest, request).map_err(domain)
}

/// Called only after matching tracked Runtime acceptance of the exact rendered saved input.
/// The durable request and Worker binding alone do not establish body delivery.
pub(super) fn attest_delivery(
    api: &WorkspaceApi,
    reservation: &BackendJobReservation,
) -> Result<()> {
    let Some(grant) = &reservation.job.request.grants.subjektiv_journal else {
        return Ok(());
    };
    let input = saved_input(&reservation.job.request, grant)?;
    // Initial dispatch passes its Reserved snapshot after binding the Worker.
    // Refresh that same owner, never reinterpret a replaced/cancelled attempt.
    let job = api
        .store
        .get_backend_job(api.workspace_id(), &reservation.job.request.job_id)?
        .ok_or_else(|| Error::WorkspacePermissionDenied("Journal delivery Job missing".into()))?;
    let attempt = api
        .store
        .get_backend_job_attempt(
            api.workspace_id(),
            &job.request.job_id,
            &reservation.attempt.attempt_id,
        )?
        .ok_or_else(|| {
            Error::WorkspacePermissionDenied("Journal delivery attempt missing".into())
        })?;
    if job.request != reservation.job.request
        || job.input_digest != reservation.job.input_digest
        || job.intent_fingerprint != reservation.job.intent_fingerprint
        || job.current_attempt != reservation.attempt.attempt
        || attempt.input_digest != reservation.attempt.input_digest
        || attempt.state != BackendJobAttemptState::Dispatched
        || attempt.worker.is_none()
    {
        return Err(Error::WorkspacePermissionDenied(
            "Journal delivery owner changed".into(),
        ));
    }
    let reservation = &BackendJobReservation {
        job,
        attempt,
        replayed: reservation.replayed,
        resource_reused: reservation.resource_reused,
    };
    let store = store(api)?;
    subject_authorization::authorize_job_input(api, &reservation.job.request)
        .map_err(|error| error.error)?;
    let store = subject_authorization::journal_attempt_effect_store(
        api,
        &store,
        &reservation.job,
        &reservation.attempt,
        subject_authorization::JournalAttemptEffect::ConfirmInputDelivery,
    )?;
    let host = recording_host(api, &store, &reservation.job.request, &reservation.attempt)?;
    let g = generation(&store, &host, grant, &reservation.job, &reservation.attempt)?;
    input.validate_generation(&g).map_err(domain)?;
    let ctx = JournalJobContext::validated(&store, &host, &g.id, g.job.clone()).map_err(domain)?;
    journal::service::attest_input_delivery(
        &store,
        &ctx,
        &reservation.attempt.input_digest,
        &input.selected_ranges,
    )
    .map_err(domain)
}

/// The exact Runtime receipt can race a normal finish before its Backend POST.
/// Reconcile that completion and active delivery under one lifecycle gate. The
/// saved completion is an outcome, never execution-end or writer-release proof.
pub(super) fn recover_delivery(
    api: &WorkspaceApi,
    reservation: &BackendJobReservation,
) -> Result<()> {
    use crate::backend_job_host_effects::JobHostOwner;
    let Some(grant) = &reservation.job.request.grants.subjektiv_journal else {
        return Ok(());
    };
    let gate = lifecycle_lock(api, &grant.subject_id)?;
    let guard = gate
        .lock()
        .map_err(|_| Error::Store("Journal lifecycle lock poisoned".into()))?;
    let current_job = api
        .store
        .get_backend_job(api.workspace_id(), &reservation.job.request.job_id)?
        .ok_or_else(|| Error::Store("Journal delivery Job missing".into()))?;
    let current_attempt = api
        .store
        .get_backend_job_attempt(
            api.workspace_id(),
            &current_job.request.job_id,
            &reservation.attempt.attempt_id,
        )?
        .ok_or_else(|| Error::Store("Journal delivery attempt missing".into()))?;
    if current_job.current_attempt != reservation.attempt.attempt
        || current_job.input_digest != reservation.job.input_digest
        || current_job.intent_fingerprint != reservation.job.intent_fingerprint
        || JobHostOwner::try_from(&current_attempt)?
            != JobHostOwner::try_from(&reservation.attempt)?
    {
        return Err(Error::WorkspacePermissionDenied(
            "Journal delivery receipt owner changed".into(),
        ));
    }
    if current_attempt.state.terminal() {
        return Ok(());
    }
    if current_attempt.state != BackendJobAttemptState::Dispatched
        || current_job.request != reservation.job.request
    {
        return Err(Error::WorkspacePermissionDenied(
            "Journal delivery input or dispatch binding changed".into(),
        ));
    }
    reconcile_attempt_under_gate(api, &current_job, &current_attempt, true, guard)
}

#[cfg(test)]
pub(super) fn finish(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
) -> Result<JournalJobCompletion> {
    finish_for(api, worker, JournalCaller::Internal)
}

fn finish_for(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
    caller: JournalCaller<'_>,
) -> Result<JournalJobCompletion> {
    let (grant, _, _) = live(api, worker)?;
    let gate = lifecycle_lock(api, &grant.subject_id)?;
    let _guard = gate
        .lock()
        .map_err(|_| Error::Store("Journal lifecycle lock poisoned".into()))?;
    // Dispatch holds this same gate until matching Runtime acceptance is attested.
    // Revalidate after waiting: erasure, cancellation or replacement may have won.
    let (grant, job, attempt) = live(api, worker)?;
    authorize(api, worker, &grant.subject_id, "journal.finish")?;
    #[cfg(test)]
    if let Some(observe) = &api.journal_before_effect {
        observe("journal.finish");
    }
    let store = caller.effect_store(api, worker, &grant.subject_id, "journal.finish")?;
    let host = recording_host(api, &store, &job.request, &attempt)?;
    let input = saved_input(&job.request, &grant)?;
    // prepare is an exact binding lookup too, including the saved completion of a closed generation.
    let g = store
        .prepare_journal_input(
            &host,
            &input,
            subjektiv::api::JobAttemptBinding {
                job_id: job.request.job_id,
                attempt_id: attempt.attempt_id,
            },
        )
        .map_err(domain)?;
    input.validate_generation(&g).map_err(domain)?;
    let ctx = JournalJobContext::validated_recovery(&store, &host, &g.id, g.job.clone())
        .map_err(domain)?;
    journal::service::finish(&store, &ctx).map_err(domain)
}

/// Final result acknowledges an actual saved checkpoint only. No aggregate Journal write.
pub(super) fn validate_result(
    api: &WorkspaceApi,
    request: &BackendJobRequest,
    attempt: &BackendJobAttemptRecord,
    value: &serde_json::Value,
) -> Result<()> {
    let Some(grant) = &request.grants.subjektiv_journal else {
        return Ok(());
    };
    let completion: JournalJobCompletion =
        serde_json::from_value(value.clone()).map_err(|e| Error::InvalidInput(e.to_string()))?;
    let store = store(api)?;
    let host = host(api, &store, grant)?;
    // The immutable Job/attempt and original Session bind this retained generation.
    // Do not prepare here: preparation requires live writer authority and can mint a
    // generation, whereas a saved final result must remain recoverable after release.
    let ctx = JournalJobContext::validated_recovery(
        &store,
        &host,
        &completion.generation_id,
        subjektiv::api::JobAttemptBinding {
            job_id: request.job_id.clone(),
            attempt_id: attempt.attempt_id.clone(),
        },
    )
    .map_err(domain)?;
    let saved = journal::service::finish(&store, &ctx).map_err(domain)?;
    if completion != saved
        || *value != serde_json::to_value(&saved).map_err(|e| Error::Store(e.to_string()))?
    {
        return Err(Error::InvalidInput(
            "Journal result differs from saved checkpoint completion".into(),
        ));
    }
    Ok(())
}

pub(super) fn close(
    api: &WorkspaceApi,
    request: &BackendJobRequest,
    attempt: &BackendJobAttemptRecord,
) -> Result<()> {
    let Some(grant) = &request.grants.subjektiv_journal else {
        return Ok(());
    };
    let saved = api
        .store
        .get_backend_job(api.workspace_id(), &request.job_id)?
        .ok_or_else(|| Error::WorkspacePermissionDenied("Journal cleanup Job missing".into()))?;
    let saved_attempt = api
        .store
        .get_backend_job_attempt(api.workspace_id(), &request.job_id, &attempt.attempt_id)?
        .ok_or_else(|| {
            Error::WorkspacePermissionDenied("Journal cleanup attempt missing".into())
        })?;
    if saved.workspace_id != api.workspace_id()
        || saved.request.grants != request.grants
        || saved.request.source_worker != request.source_worker
        || saved_attempt != *attempt
        || attempt.input_digest != saved.input_digest
        || attempt.job_id != request.job_id
    {
        return Err(Error::WorkspacePermissionDenied(
            "Journal cleanup owner mismatch".into(),
        ));
    }
    let store = store(api)?;
    let host = host(api, &store, grant)?;
    if store.journal_session_erased(&host).map_err(domain)? {
        return Ok(());
    }
    let binding = subjektiv::api::JobAttemptBinding {
        job_id: request.job_id.clone(),
        attempt_id: attempt.attempt_id.clone(),
    };
    if let Some(g) = store
        .retained_journal_generation(&host, &grant.journal_id, &binding)
        .map_err(domain)?
    {
        let attr = store
            .session_attribution(&grant.session_id)
            .map_err(subjektiv_store_error)?
            .filter(|attr| {
                attr.subject_id == grant.subject_id
                    && request.source_worker.as_ref()
                        == Some(&RuntimeWorkerRef::new(&attr.runtime_id, &attr.worker_id))
            })
            .ok_or_else(|| {
                Error::WorkspacePermissionDenied("Journal cleanup provenance mismatch".into())
            })?;
        let host = JournalHostContext::validated_recording_job(
            &store,
            attr,
            &grant.journal_id,
            binding.clone(),
        )
        .map_err(domain)?;
        store
            .close_journal_generation(&host, &g.id, &binding)
            .map_err(domain)?;
    }
    Ok(())
}

pub(super) fn bind_host_session(
    api: &WorkspaceApi,
    attr: crate::subjektiv::SubjectSessionAttribution,
) -> Result<()> {
    bind_host_session_for_registration(api, attr, SessionRegistrationCaller::InternalRecovery)
        .map_err(|failure| failure.error)
}

pub(super) enum SessionRegistrationCaller<'a> {
    Signed(&'a server_api::ServerRuntimeSource),
    InternalRecovery,
}

pub(super) fn bind_host_session_for_registration(
    api: &WorkspaceApi,
    attr: crate::subjektiv::SubjectSessionAttribution,
    caller: SessionRegistrationCaller<'_>,
) -> ApiResult<()> {
    let gate = lifecycle_lock(api, &attr.subject_id)?;
    let _guard = gate
        .lock()
        .map_err(|_| Error::Store("Journal lifecycle lock poisoned".into()))?;
    let worker = RuntimeWorkerRef::new(&attr.runtime_id, &attr.worker_id);
    api.store
        .require_current_worker_singleton_owner(api.workspace_id(), &worker)?
        .ok_or_else(|| {
            Error::WorkspacePermissionDenied("Subject body singleton is missing".into())
        })?;
    if api
        .config_store
        .subject_release_pending(api.workspace_id(), &worker)?
    {
        // The current body is explicitly fenced from publication, not an
        // unobserved callback outcome. Keep the conflict separate from its cause.
        return Err(session_registration::Judgment::SessionRecord.error(
            Error::RepositoryConflict(
                "Subject release intent fences Controller publication".into(),
            ),
            protocol::session_failure::SessionRegistrationCause::binding_mismatch(),
        ));
    }
    let store = store(api)?;
    let store = match caller {
        SessionRegistrationCaller::Signed(source) => {
            subject_authorization::authenticated_subject_effect_store(
                api,
                &store,
                &worker,
                &attr.subject_id,
                "session.record",
                source,
            )
        }
        SessionRegistrationCaller::InternalRecovery => store,
    };
    if store
        .journal_worker_was_released(&attr.subject_id, &attr.runtime_id, &attr.worker_id)
        .map_err(session_registration::journal_error)?
    {
        return Err(Error::WorkspacePermissionDenied(
            "released body cannot reacquire Journal authority".into(),
        )
        .into());
    }
    // A released Backend body is never rebound by a delayed create/restore callback. New
    // Backend bodies have new Worker IDs; their first callback has no historical generation.
    for prior in store
        .subject_session_attributions(&attr.subject_id)
        .map_err(|e| Error::Store(e.to_string()))?
        .into_iter()
        .filter(|a| a.runtime_id == attr.runtime_id && a.worker_id == attr.worker_id)
    {
        let host = JournalHostContext::validated_session(&store, prior)
            .map_err(session_registration::journal_error)?;
        if let Some(id) = host.writer_binding_id() {
            let binding = JournalWriterBinding {
                subject_id: attr.subject_id.clone(),
                runtime_id: attr.runtime_id.clone(),
                worker_id: attr.worker_id.clone(),
                binding_id: id.into(),
            };
            if !store
                .journal_writer_active(&binding)
                .map_err(session_registration::journal_error)?
            {
                return Err(Error::WorkspacePermissionDenied(
                    "released body Session cannot reacquire Journal authority".into(),
                )
                .into());
            }
        }
    }
    // Publication (including attribution) shares the recovery gate. A callback
    // authenticated before a recovery intent must not leave a late attribution.
    let attr = store
        .record_session_attribution(attr)
        .map_err(session_registration::domain_error)?;
    let writer = store
        .claim_journal_writer(&attr.subject_id, &attr.runtime_id, &attr.worker_id)
        .map_err(session_registration::journal_error)?;
    store
        .bind_journal_session(&writer, attr)
        .map_err(session_registration::journal_error)?;
    Ok(())
}

/// Runs under the same Subject gate as final revoke/singleton release. A failed create or
/// legacy removal may have released its Backend reservation but cannot hide a durable writer.
pub(super) fn check_start_allowed(api: &WorkspaceApi, subject: &str) -> Result<()> {
    subjektiv_deletion::require_available(api, subject)?;
    let store = store(api)?;
    let key = crate::subjektiv::subject_worker_singleton_key(subject)
        .map_err(|e| Error::InvalidInput(e.to_string()))?;
    let owner = api
        .store
        .current_worker_singleton_owner(api.workspace_id(), &key)?;
    for attr in store
        .subject_session_attributions(subject)
        .map_err(|e| Error::Store(e.to_string()))?
    {
        let host = JournalHostContext::validated_session(&store, attr.clone()).map_err(domain)?;
        if let Some(id) = host.writer_binding_id() {
            let binding = JournalWriterBinding {
                subject_id: subject.into(),
                runtime_id: attr.runtime_id.clone(),
                worker_id: attr.worker_id.clone(),
                binding_id: id.into(),
            };
            if store.journal_writer_active(&binding).map_err(domain)?
                && owner.as_ref().is_none_or(|o| {
                    o.worker.runtime_id != attr.runtime_id || o.worker.worker_id != attr.worker_id
                })
            {
                return Err(Error::RepositoryConflict("Subject has an unreleased durable Journal writer; replacement start is forbidden".into()));
            }
        }
    }
    Ok(())
}

/// Read-only eligibility, not proof of Runtime absence or permission to replace a body.
pub(super) fn failed_subject_create_is_unpublished(
    api: &WorkspaceApi,
    subject: &str,
    lease: &crate::store::WorkerSingletonLease,
) -> Result<bool> {
    Ok(api
        .config_store
        .subject_create_recovery_candidate(api.workspace_id(), lease)?
        .is_some()
        && store(api)?
            .journal_body_is_unpublished(subject, &lease.worker.runtime_id, &lease.worker.worker_id)
            .map_err(domain)?)
}

/// Explicit creation can reconcile an earlier failed, never-published body.
/// No missing catalog/status observation is treated as a Runtime fencing receipt.
pub(super) fn recover_failed_subject_create(api: &WorkspaceApi, subject: &str) -> Result<()> {
    let key = crate::subjektiv::subject_worker_singleton_key(subject)
        .map_err(|e| Error::InvalidInput(e.to_string()))?;
    if let Some(lease) = api
        .config_store
        .current_worker_singleton_owner(api.workspace_id(), &key)?
        && failed_subject_create_is_unpublished(api, subject, &lease)?
    {
        recover_subject_create_reservation(api, subject, &lease)?;
    }
    Ok(())
}

/// Compensation and explicit recovery fence only the observed reservation, never a successor.
pub(super) fn recover_subject_create_reservation(
    api: &WorkspaceApi,
    subject: &str,
    expected: &crate::store::WorkerSingletonLease,
) -> Result<()> {
    let continuation = lifecycle_lock(api, &format!("release-continuation:{subject}"))?;
    let _continuation = continuation
        .lock()
        .map_err(|_| Error::Store("release continuation poisoned".into()))?;
    let gate = lifecycle_lock(api, subject)?;
    let subject_key = crate::subjektiv::subject_worker_singleton_key(subject)
        .map_err(|e| Error::InvalidInput(e.to_string()))?;
    let store = store(api)?;
    let (lease, request) = {
        let _guard = gate
            .lock()
            .map_err(|_| Error::Store("Journal lifecycle lock poisoned".into()))?;
        let lease = api
            .config_store
            .current_worker_singleton_owner(api.workspace_id(), &subject_key)?
            .filter(|lease| lease == expected)
            .ok_or(Error::SubjectBodySuperseded)?;
        if !failed_subject_create_is_unpublished(api, subject, &lease)? {
            return Err(Error::RepositoryConflict(
                "Only a failed, unpublished Subject creation can be recovered; retain occupancy"
                    .into(),
            ));
        }
        let operation_id = format!(
            "subject-create-recovery:{}:{}",
            lease.worker.worker_id, lease.generation
        );
        let fingerprint = api.config_store.prepare_subject_create_recovery(
            api.workspace_id(),
            &lease,
            &operation_id,
        )?;
        let request = runtime_api::SubjectCreateRecoveryRequest {
            workspace_id: api.workspace_id().into(),
            worker_id: worker_runtime::identity::WorkerId::parse(&lease.worker.worker_id)
                .ok_or_else(|| Error::Store("invalid retained Worker ID".into()))?,
            operation_id,
            create_fingerprint: fingerprint,
        };
        (lease, request)
    };
    // Never hold the callback lane/gate across Runtime I/O. Unknown outcome
    // leaves the durable intent in place and reuses the exact identity next time.
    let response = api
        .runtime
        .recover_subject_create(&lease.worker.runtime_id, request.clone())
        .map_err(RuntimeRegistryError::into_error)?;
    if response.workspace_id != request.workspace_id
        || response.worker_id != request.worker_id
        || response.operation_id != request.operation_id
        || response.create_fingerprint != request.create_fingerprint
    {
        return Err(Error::RepositoryConflict(
            "Subject create recovery acknowledgment mismatch; retain occupancy".into(),
        ));
    }
    let _guard = gate
        .lock()
        .map_err(|_| Error::Store("Journal lifecycle lock poisoned".into()))?;
    if !store
        .journal_body_is_unpublished(subject, &lease.worker.runtime_id, &lease.worker.worker_id)
        .map_err(domain)?
    {
        return Err(Error::RepositoryConflict(
            "Subject publication appeared during recovery; retain occupancy".into(),
        ));
    }
    api.config_store.complete_subject_create_recovery(
        api.workspace_id(),
        &lease,
        &request.operation_id,
        &request.create_fingerprint,
    )
}

pub(super) fn lifecycle_lock(api: &WorkspaceApi, subject: &str) -> Result<Arc<Mutex<()>>> {
    let mut locks = api
        .journal_lifecycle
        .lock()
        .map_err(|_| Error::Store("Journal lifecycle lock poisoned".into()))?;
    Ok(locks
        .entry(subject.into())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone())
}

/// Read the existing retention responsibility without consulting the producer's
/// registry or body lease. Native admission repeats this check under its catalog guard.
pub(super) fn retained_source_progress(
    api: &WorkspaceApi,
    attr: &crate::subjektiv::SubjectSessionAttribution,
) -> Result<RetainedSourceProgress> {
    let mut after = None;
    loop {
        let page = api.store.list_retained_journal_recordings(
            api.workspace_id(),
            after.as_deref(),
            100,
        )?;
        for progress in &page {
            if progress.worker.runtime_id == attr.runtime_id
                && progress.worker.worker_id == attr.worker_id
                && progress.workspace_id == api.workspace_id()
                && progress.metadata.binding.subject_id == attr.subject_id
                && matches!(
                    progress.metadata.phase,
                    crate::store::JournalRemovalPhase::Released
                        | crate::store::JournalRemovalPhase::Completed
                )
                && progress
                    .metadata
                    .retained_recording
                    .as_ref()
                    .is_some_and(|r| !r.completed && !r.archive_id.is_empty())
                && progress.metadata.members.as_ref().is_some_and(|members| {
                    members
                        .iter()
                        .any(|member| member.session_id == attr.session_id)
                })
            {
                return RetainedSourceProgress::removal(progress);
            }
        }
        if page.len() < 100 {
            let worker = RuntimeWorkerRef::new(&attr.runtime_id, &attr.worker_id);
            if let Some(progress) = api
                .config_store
                .get_release_journal_recording(api.workspace_id(), &worker)?
                && progress.released
                && !progress.recording.completed
                && progress.recording.binding.subject_id == attr.subject_id
                && progress
                    .recording
                    .members
                    .iter()
                    .any(|member| member.session_id == attr.session_id)
            {
                return Ok(RetainedSourceProgress::release(&progress));
            }
            return Err(Error::WorkspacePermissionDenied(
                "Session has no released retained recording responsibility".into(),
            ));
        }
        after = page.last().map(|p| p.operation_id.clone());
    }
}

pub(super) fn retained_source_store(
    api: &WorkspaceApi,
    attr: &crate::subjektiv::SubjectSessionAttribution,
) -> Result<crate::subjektiv::SubjektivStore> {
    subjektiv_deletion::require_available(api, &attr.subject_id)?;
    let progress = retained_source_progress(api, attr)?;
    let database = api
        .feature_storage
        .open_existing(&api.subjektiv_registration, &progress.binding.database)
        .map_err(Error::SubjektivStorage)?;
    let store =
        crate::subjektiv::SubjektivStore::from_database(database).map_err(subjektiv_store_error)?;
    JournalHostContext::validated_recording_source(&store, attr.clone()).map_err(domain)?;
    subject_authorization::retained_journal_source_store(api, &store, attr, progress)
        .map_err(|error| error.error)
}

fn dispatch(
    api: &WorkspaceApi,
    attribution: crate::subjektiv::SubjectSessionAttribution,
    journal_id: &str,
) -> Result<()> {
    dispatch_saved_or_new(api, attribution, journal_id, true)
}

fn dispatch_saved_or_new(
    api: &WorkspaceApi,
    attribution: crate::subjektiv::SubjectSessionAttribution,
    journal_id: &str,
    capture_new: bool,
) -> Result<()> {
    dispatch_with_capture(
        api,
        attribution.clone(),
        journal_id,
        capture_new,
        |order, offset, budget| capture_input(api, &attribution, order, offset, budget),
    )
}

enum RecordingAdmission {
    Producer,
    Retained,
}

pub(super) fn dispatch_retained_with_capture(
    api: &WorkspaceApi,
    attribution: crate::subjektiv::SubjectSessionAttribution,
    journal_id: &str,
    mut capture: impl FnMut(u64, u64, usize) -> Result<session_store::capture::CaptureDto>,
) -> Result<()> {
    dispatch_with_admission(
        api,
        attribution,
        journal_id,
        true,
        RecordingAdmission::Retained,
        |order, offset, budget| capture(order, offset, budget),
    )
}

fn dispatch_with_capture(
    api: &WorkspaceApi,
    attribution: crate::subjektiv::SubjectSessionAttribution,
    journal_id: &str,
    capture_new: bool,
    capture: impl FnOnce(u64, u64, usize) -> Result<session_store::capture::CaptureDto>,
) -> Result<()> {
    dispatch_with_admission(
        api,
        attribution,
        journal_id,
        capture_new,
        RecordingAdmission::Producer,
        capture,
    )
}

fn dispatch_with_admission(
    api: &WorkspaceApi,
    attribution: crate::subjektiv::SubjectSessionAttribution,
    journal_id: &str,
    capture_new: bool,
    admission: RecordingAdmission,
    capture: impl FnOnce(u64, u64, usize) -> Result<session_store::capture::CaptureDto>,
) -> Result<()> {
    let gate = lifecycle_lock(api, &attribution.subject_id)?;
    let _gate = gate
        .lock()
        .map_err(|_| Error::Store("Journal lifecycle lock poisoned".into()))?;
    let store = store(api)?;
    let host =
        JournalHostContext::validated_session(&store, attribution.clone()).map_err(domain)?;
    if store.journal_session_erased(&host).map_err(domain)? {
        return Err(Error::WorkspacePermissionDenied(
            "Session was explicitly erased".into(),
        ));
    }
    subjektiv_deletion::require_available(api, &attribution.subject_id)?;
    // Existing Jobs use only their saved grant/input. Source-body authority is
    // needed below solely when admitting newly captured history.
    let mut after = None;
    loop {
        let jobs = api.store.list_backend_journal_jobs(
            api.workspace_id(),
            &attribution.session_id,
            after.as_deref(),
            100,
        )?;
        for job in &jobs {
            if job
                .request
                .grants
                .subjektiv_journal
                .as_ref()
                .is_none_or(|g| g.journal_id != journal_id)
            {
                continue;
            }
            if matches!(
                job.state,
                BackendJobState::Pending | BackendJobState::Finalizing
            ) {
                return Ok(());
            }
            if job.state == BackendJobState::Failed {
                if !can_resume_partial(job) {
                    return Ok(());
                }
                require_job_effects_ended(api, job)?;
                let mut cleanup_returns = Vec::new();
                for attempt in api
                    .store
                    .list_backend_job_attempts(api.workspace_id(), &job.request.job_id)?
                {
                    if attempt.worker.is_some() {
                        if attempt.worker_cleanup_state
                            != Some(BackendJobWorkerCleanupState::Completed)
                        {
                            return Err(Error::RepositoryConflict(
                                "Prior Journal attempt cleanup remains owned".into(),
                            ));
                        }
                        cleanup_returns.push(
                            crate::backend_job_host_effects::JobHostEffects::try_fence_retained(
                                &api.config.database_path,
                                &attempt,
                            )?,
                        );
                    }
                }
                job.load_executable_request()?;
                saved_input(
                    &job.request,
                    job.request
                        .grants
                        .subjektiv_journal
                        .as_ref()
                        .expect("checked Journal grant"),
                )?;
                subject_authorization::authorize_job_input(api, &job.request)
                    .map_err(|error| error.error)?;
                let reservation = api.store.reserve_backend_job_retry(
                    api.workspace_id(),
                    &job.request.job_id,
                    &Utc::now().to_rfc3339(),
                )?;
                return queue_reservation(api, reservation);
            }
        }
        if jobs.len() < 100 {
            break;
        }
        after = jobs.last().map(|j| j.request.job_id.clone());
    }
    if !capture_new {
        return Ok(());
    }
    let (store, host) = match admission {
        RecordingAdmission::Producer => {
            authorize_source(api, &attribution, "journal.recording.request")?;
            (store, host)
        }
        RecordingAdmission::Retained => {
            let store = retained_source_store(api, &attribution)?;
            let host = JournalHostContext::validated_recording_source(&store, attribution.clone())
                .map_err(domain)?;
            (store, host)
        }
    };
    let input = JournalInput::build(&store, &host, journal_id, |order, offset, budget| {
        // Build reads its snapshot directly; recheck native source authority before
        // acquiring protected bytes from the retained archive callback.
        store.journal_resume_state(&host, journal_id)?;
        capture(order, offset, budget).map_err(|e| JournalError::Conflict(e.to_string()))
    })
    .map_err(domain)?;
    if input.selected_ranges.is_empty() {
        return Ok(());
    }
    if matches!(admission, RecordingAdmission::Producer) {
        authorize_source(api, &attribution, "journal.recording.request")?;
    }
    store
        .accept_journal_sources(&host, "Session activity", &input.selected_ranges)
        .map_err(domain)?;
    let request = recording_request(&store, attribution, journal_id, input)?;
    // The intent is saved before generation binding or dispatch. If binding/dispatch fails,
    // Reserved recovery repeats this exact intent, never creates a replacement blindly.
    api.resolve_backend_job_profile(&request)?;
    let reservation = api.store.reserve_backend_job(
        api.workspace_id(),
        &request,
        &Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true),
    )?;
    queue_reservation(api, reservation)
}

fn queue_reservation(api: &WorkspaceApi, reservation: BackendJobReservation) -> Result<()> {
    // The durable intent is already accepted. Host capacity is only an eager-dispatch
    // optimization: its rejection must not leave this same attempt without recovery.
    api.own_backend_job_recovery(&reservation.attempt);
    let queued = api.clone();
    if let Err(error)=api.host_executors.host()?.dispatch(move || {
        if let Err(error)=queued.dispatch_backend_job_reservation(reservation) {
            tracing::warn!(%error,"Journal dispatch failed; reservation and saved operations retained");
        }
    }) {tracing::warn!(%error,"Journal Host capacity unavailable; durable reservation retained");}
    Ok(())
}

fn admit(
    api: &WorkspaceApi,
    attribution: crate::subjektiv::SubjectSessionAttribution,
) -> Result<JournalRecordingResponse> {
    admit_sources(api, attribution, true)
}

fn admit_sources(
    api: &WorkspaceApi,
    attribution: crate::subjektiv::SubjectSessionAttribution,
    dispatch_jobs: bool,
) -> Result<JournalRecordingResponse> {
    admit_sources_in_store(api, attribution, dispatch_jobs, store(api)?)
}

fn admit_sources_in_store(
    api: &WorkspaceApi,
    attribution: crate::subjektiv::SubjectSessionAttribution,
    dispatch_jobs: bool,
    store: crate::subjektiv::SubjektivStore,
) -> Result<JournalRecordingResponse> {
    authorize_source(api, &attribution, "journal.recording.request")?;
    let host =
        JournalHostContext::validated_session(&store, attribution.clone()).map_err(domain)?;
    if store.journal_session_erased(&host).map_err(domain)? {
        return Err(Error::WorkspacePermissionDenied(
            "Session was explicitly erased".into(),
        ));
    }
    let mut id = store.journal_for_session(&host).map_err(domain)?;
    let (order, offset) = if let Some(id) = id.as_deref() {
        let journal = store.read_journal(&host, id).map_err(domain)?;
        (journal.process_order, journal.process_byte)
    } else {
        (0, 0)
    };
    let capture = capture_input(
        api,
        &attribution,
        order,
        offset,
        JournalInput::capture_budget(),
    )?;
    authorize_source(api, &attribution, "journal.recording.request")?;
    retain_source(
        api.runtime.as_ref(),
        api.workspace_id(),
        &attribution,
        material_source(api, &attribution)?,
        capture.segment_id == "not-materialized",
    )?;
    let sources = journal::recording::capture_ranges(&capture).map_err(domain)?;
    for batch in sources.chunks(MAX_JOURNAL_RANGES) {
        authorize_source(api, &attribution, "journal.recording.request")?;
        id = Some(
            store
                .accept_journal_sources(&host, "Session activity", batch)
                .map_err(domain)?,
        );
    }
    let Some(id) = id else {
        return Ok(JournalRecordingResponse::NoNewSources);
    };
    // Failure to start never retracts admission. Capacity preserves a Reserved intent.
    if dispatch_jobs && let Err(error) = dispatch(api, attribution, &id) {
        tracing::warn!(%error,journal_id=%id,"Journal admitted; Job dispatch remains recoverable");
    }
    Ok(JournalRecordingResponse::Accepted)
}

/// Startup reads are store-only: Runtime waits for this callback during Controller creation.
/// Authentication plus the current body singleton selects Subject; no caller-selected Subject
/// or Job write capability can select a different journal.
pub(super) async fn startup_route(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    source: Option<axum::Extension<crate::worker_source::VerifiedRuntimeRequestSource>>,
) -> ApiResult<Json<journal::startup::JournalStartupSnapshot>> {
    let (source, worker) = verified_source(source)?;
    api.store
        .db_executor()
        .run(move || {
            validate_workspace_scope(&api, &path.workspace_id)?;
            let lease = api
                .store
                .require_current_worker_singleton_owner(&path.workspace_id, &worker)?
                .ok_or_else(|| {
                    Error::WorkspacePermissionDenied(
                        "Journal startup requires the current main Subject Worker".into(),
                    )
                })?;
            let (subject, role) = subjektiv_scope_from_singleton_key(&lease.key)?;
            if role != SubjektivWorkerAuthority::Subject {
                return Err(Error::WorkspacePermissionDenied(
                    "Journal startup is not a recording or consolidation Job capability".into(),
                )
                .into());
            }
            authorize_subjektiv_operation(
                &api,
                &worker,
                &path.workspace_id,
                subject,
                "journal.startup.read",
            )?;
            let store = JournalCaller::Signed(&source).effect_store(
                &api,
                &worker,
                subject,
                "journal.startup.read",
            )?;
            let reader = JournalHostContext::validated_reader(&store, subject).map_err(domain)?;
            let mut current_writer = None;
            for attr in store
                .subject_session_attributions(subject)
                .map_err(|e| Error::Store(e.to_string()))?
                .into_iter()
                .filter(|a| a.runtime_id == worker.runtime_id && a.worker_id == worker.worker_id)
            {
                let host = JournalHostContext::validated_session(&store, attr).map_err(domain)?;
                if let Some(id) = host.writer_binding_id() {
                    let binding = JournalWriterBinding {
                        subject_id: subject.into(),
                        runtime_id: worker.runtime_id.clone(),
                        worker_id: worker.worker_id.clone(),
                        binding_id: id.into(),
                    };
                    if store.journal_writer_active(&binding).map_err(domain)? {
                        current_writer = Some(binding);
                        break;
                    }
                }
            }
            let current_writer = current_writer.ok_or_else(|| {
                Error::RepositoryConflict(
                    "startup requires the current Host-bound writer lifetime".into(),
                )
            })?;
            let previous = store
                .journal_predecessor_session(&current_writer)
                .map_err(domain)?;
            store
                .journal_startup_snapshot(&reader, previous.as_deref())
                .map(Json)
                .map_err(domain)
                .map_err(ApiError::from)
        })
        .await
}

pub(super) async fn compaction_route(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    source: Option<axum::Extension<crate::worker_source::VerifiedRuntimeRequestSource>>,
    Json(input): Json<JournalCompactionRequest>,
) -> ApiResult<Json<journal::startup::JournalStartupSnapshot>> {
    let (source, worker) = verified_source(source)?;
    api.store
        .db_executor()
        .run(move || {
            validate_workspace_scope(&api, &path.workspace_id)?;
            let lease = api
                .store
                .require_current_worker_singleton_owner(&path.workspace_id, &worker)?
                .ok_or_else(|| {
                    Error::WorkspacePermissionDenied(
                        "Journal compaction requires the current main Subject Worker".into(),
                    )
                })?;
            let (subject, role) = subjektiv_scope_from_singleton_key(&lease.key)?;
            if role != SubjektivWorkerAuthority::Subject {
                return Err(Error::WorkspacePermissionDenied(
                    "Journal compaction is not a Job capability".into(),
                )
                .into());
            }
            authorize_subjektiv_operation(
                &api,
                &worker,
                &path.workspace_id,
                subject,
                "journal.compaction.read",
            )?;
            let store = JournalCaller::Signed(&source).effect_store(
                &api,
                &worker,
                subject,
                "journal.compaction.read",
            )?;
            let attribution = store
                .session_attribution(&input.session_id)
                .map_err(|error| Error::Store(error.to_string()))?
                .filter(|attr| {
                    attr.subject_id == subject
                        && attr.runtime_id == worker.runtime_id
                        && attr.worker_id == worker.worker_id
                })
                .ok_or_else(|| {
                    Error::WorkspacePermissionDenied(
                        "foreign or unattributed compaction Session".into(),
                    )
                })?;
            let host =
                JournalHostContext::validated_session(&store, attribution).map_err(domain)?;
            store
                .journal_compaction_snapshot(&host)
                .map(Json)
                .map_err(domain)
                .map_err(ApiError::from)
        })
        .await
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SubjectReleaseInput {
    worker: RuntimeWorkerRef,
    /// Optional explicit generation CAS; omission resolves only an unambiguous retained Host binding.
    #[serde(default)]
    binding_id: String,
    operation_id: String,
}

fn confirm_runtime_disconnect(
    api: &WorkspaceApi,
    lease: &crate::store::WorkerSingletonLease,
    mut request: runtime_api::SubjectReleaseRequest,
) -> Result<()> {
    request.action = runtime_api::SubjectReleaseAction::Disconnect;
    let expected_operation = request.operation_id.clone();
    let expected_binding = request.binding_id.clone();
    let disconnected = api
        .runtime
        .subject_release(&lease.worker.runtime_id, request)
        .map_err(RuntimeRegistryError::into_error)?;
    if disconnected.state != runtime_api::SubjectReleaseState::Disconnected
        || disconnected.operation_id != expected_operation
        || disconnected.binding_id != expected_binding
    {
        return Err(Error::RepositoryConflict(
            "Runtime disconnect receipt is unconfirmed; retain Subject occupancy".into(),
        ));
    }
    if let Some(prepared) = api
        .config_store
        .get_release_journal_recording(api.workspace_id(), &lease.worker)?
    {
        let scope = disconnected.recording_scope.as_ref().ok_or_else(|| {
            Error::RepositoryConflict("Disconnected recording scope missing".into())
        })?;
        if prepared.operation_id != expected_operation
            || scope.workspace_id != api.workspace_id()
            || scope.source_runtime_id != lease.worker.runtime_id
            || scope.worker_id.to_string() != lease.worker.worker_id
            || scope
                .members
                .iter()
                .map(crate::store::FrozenJournalRemovalMember::from)
                .collect::<Vec<_>>()
                != prepared.recording.members
        {
            return Err(Error::RepositoryConflict(
                "Disconnected recording scope changed".into(),
            ));
        }
    }
    api.config_store.confirm_subject_release_proof(
        api.workspace_id(),
        lease,
        &expected_binding,
        &expected_operation,
        false,
    )
}

// Caller holds the narrow release-continuation lock, never the callback lane across I/O.
fn cancel_active_release(
    api: &WorkspaceApi,
    store: &crate::subjektiv::SubjektivStore,
    gate: &Arc<Mutex<()>>,
    lease: &crate::store::WorkerSingletonLease,
    binding: &JournalWriterBinding,
    request: &runtime_api::SubjectReleaseRequest,
) -> Result<()> {
    {
        let _guard = gate
            .lock()
            .map_err(|_| Error::Store("Journal lifecycle lock poisoned".into()))?;
        api.config_store.require_subject_release_intent(
            api.workspace_id(),
            lease,
            &binding.binding_id,
            &request.operation_id,
        )?;
        if !store.journal_writer_active(binding).map_err(domain)? {
            return Err(Error::RepositoryConflict(
                "writer already revoked; cancellation forbidden".into(),
            ));
        }
    }
    let mut cancel = request.clone();
    cancel.action = runtime_api::SubjectReleaseAction::Cancel;
    let response = api
        .runtime
        .subject_release(&lease.worker.runtime_id, cancel)
        .map_err(RuntimeRegistryError::into_error)?;
    if response.state != runtime_api::SubjectReleaseState::Cancelled
        || response.binding_id != binding.binding_id
        || response.operation_id != request.operation_id
    {
        return Err(Error::RepositoryConflict(
            "cancellation unconfirmed; retain exclusion".into(),
        ));
    }
    let _guard = gate
        .lock()
        .map_err(|_| Error::Store("Journal lifecycle lock poisoned".into()))?;
    if !store.journal_writer_active(binding).map_err(domain)? {
        return Err(Error::RepositoryConflict(
            "writer revoked before cancellation commit".into(),
        ));
    }
    api.config_store.cancel_subject_release(
        api.workspace_id(),
        lease,
        &binding.binding_id,
        &request.operation_id,
    )
}

/// Reconcile saved originals while the actual Controller cannot accept a new turn or history
/// mutation. Storage revocation is the durable commit point; singleton release happens last.
pub(super) fn release_subject(
    api: &WorkspaceApi,
    mut input: SubjectReleaseInput,
) -> Result<runtime_api::SubjectReleaseResponse> {
    let lease = api
        .config_store
        .retained_worker_singleton_lease(api.workspace_id(), &input.worker)?
        .ok_or_else(|| Error::WorkspacePermissionDenied("Subject lease is absent".into()))?;
    let (subject, role) = subjektiv_scope_from_singleton_key(&lease.key).map_err(|e| e.error)?;
    let identity = api
        .feature_storage
        .existing_identity(&api.subjektiv_registration)
        .map_err(Error::SubjektivStorage)?;
    let database = api
        .feature_storage
        .open_existing(&api.subjektiv_registration, &identity)
        .map_err(Error::SubjektivStorage)?;
    let store = crate::subjektiv::SubjektivStore::from_database(database.clone())
        .map_err(subjektiv_store_error)?;
    if input.binding_id.is_empty() {
        let mut ids = std::collections::BTreeSet::new();
        for attr in store
            .subject_session_attributions(subject)
            .map_err(|e| Error::Store(e.to_string()))?
            .into_iter()
            .filter(|a| {
                a.runtime_id == input.worker.runtime_id && a.worker_id == input.worker.worker_id
            })
        {
            let host = JournalHostContext::validated_session(&store, attr).map_err(domain)?;
            if let Some(id) = host.writer_binding_id() {
                ids.insert(id.to_string());
            }
        }
        if ids.len() != 1 {
            return Err(Error::RepositoryConflict(
                "Subject body writer generation is absent or ambiguous; retain occupancy".into(),
            ));
        }
        input.binding_id = ids.into_iter().next().expect("one binding");
    }
    if role != SubjektivWorkerAuthority::Subject
        || input.binding_id.is_empty()
        || input.operation_id.is_empty()
    {
        return Err(Error::WorkspacePermissionDenied(
            "invalid Subject release binding".into(),
        ));
    }
    let continuation = lifecycle_lock(api, &format!("release-continuation:{subject}"))?;
    let _continuation = continuation
        .lock()
        .map_err(|_| Error::Store("release continuation poisoned".into()))?;
    let gate = lifecycle_lock(api, subject)?;
    let binding = JournalWriterBinding {
        subject_id: subject.into(),
        runtime_id: input.worker.runtime_id.clone(),
        worker_id: input.worker.worker_id.clone(),
        binding_id: input.binding_id.clone(),
    };
    let request = runtime_api::SubjectReleaseRequest {
        workspace_id: api.workspace_id().into(),
        worker_id: worker_runtime::identity::WorkerId::parse(&input.worker.worker_id)
            .ok_or_else(|| Error::InvalidInput("invalid Worker ID".into()))?,
        binding_id: input.binding_id.clone(),
        operation_id: input.operation_id.clone(),
        action: runtime_api::SubjectReleaseAction::Reserve,
    };
    let response = || runtime_api::SubjectReleaseResponse {
        binding_id: input.binding_id.clone(),
        operation_id: input.operation_id.clone(),
        state: runtime_api::SubjectReleaseState::Disconnected,
        recording_scope: None,
    };
    {
        let _guard = gate
            .lock()
            .map_err(|_| Error::Store("Journal lifecycle lock poisoned".into()))?;
        // Callback and release intent share one Subject gate, before any Runtime wait.
        api.config_store.reserve_subject_release(
            api.workspace_id(),
            &lease,
            &input.binding_id,
            &input.operation_id,
        )?;
    }
    if !store.journal_writer_active(&binding).map_err(domain)? {
        // Writer revocation alone does not certify an acknowledged Controller disconnect.
        // Lost Runtime proof is retained as unknown, never reconstructed from Idle/status.
        if !api.config_store.subject_release_proven(
            api.workspace_id(),
            &lease,
            &input.binding_id,
            &input.operation_id,
        )? {
            confirm_runtime_disconnect(api, &lease, request)?;
        }
        let _guard = gate
            .lock()
            .map_err(|_| Error::Store("Journal lifecycle lock poisoned".into()))?;
        api.config_store.release_subject_singleton(
            api.workspace_id(),
            &lease,
            &input.binding_id,
            &input.operation_id,
        )?;
        drop(_guard);
        if let Err(error) = retained::recover_releases(api) {
            tracing::warn!(%error, "Released Journal continuation deferred");
        }
        return Ok(response());
    }
    let current = api
        .store
        .require_current_worker_singleton_owner(api.workspace_id(), &input.worker)?;
    if current.as_ref() != Some(&lease) {
        return Err(Error::RepositoryConflict("Subject owner changed".into()));
    }
    let reserved = api
        .runtime
        .subject_release(&input.worker.runtime_id, request.clone());
    let (recording_scope, disconnected) = match reserved {
        Ok(reserved)
            if matches!(
                reserved.state,
                runtime_api::SubjectReleaseState::Reserved
                    | runtime_api::SubjectReleaseState::Disconnected
            ) && reserved.operation_id == input.operation_id
                && reserved.binding_id == input.binding_id =>
        {
            (
                reserved.recording_scope,
                reserved.state == runtime_api::SubjectReleaseState::Disconnected,
            )
        }
        result => {
            // A rejected/unknown Reserve is not permission to unfreeze. Only the exact
            // confirmed cancellation below clears the intent, while writer remains active.
            let error = match result {
                Err(error) => error.into_error(),
                Ok(_) => Error::RepositoryConflict("actual Idle reservation is not held".into()),
            };
            cancel_active_release(api, &store, &gate, &lease, &binding, &request)?;
            return Err(error);
        }
    };
    // Controller reservation precedes the Host gate: restore can be synchronously waiting
    // for a Session-attribution callback that itself takes this gate.
    let guard = gate
        .lock()
        .map_err(|_| Error::Store("Journal lifecycle lock poisoned".into()))?;
    let mut sessions = Vec::new();
    let checked = (|| {
        let recording_scope = recording_scope
            .as_ref()
            .filter(|scope| {
                scope.workspace_id == api.workspace_id()
                    && scope.source_runtime_id == input.worker.runtime_id
                    && scope.worker_id.to_string() == input.worker.worker_id
            })
            .ok_or_else(|| {
                Error::RepositoryConflict(
                    "Actual Subject reservation has no matching complete recording scope".into(),
                )
            })?;
        api.config_store.require_subject_release_intent(
            api.workspace_id(),
            &lease,
            &input.binding_id,
            &input.operation_id,
        )?;
        if api
            .store
            .require_current_worker_singleton_owner(api.workspace_id(), &input.worker)?
            .as_ref()
            != Some(&lease)
        {
            return Err(Error::RepositoryConflict("Subject owner changed".into()));
        }
        for attr in store
            .subject_session_attributions(subject)
            .map_err(|e| Error::Store(e.to_string()))?
            .into_iter()
            .filter(|a| {
                a.runtime_id == input.worker.runtime_id && a.worker_id == input.worker.worker_id
            })
        {
            if !store
                .journal_session_writer_matches(&binding, &attr.session_id)
                .map_err(domain)?
            {
                return Err(Error::RepositoryConflict(
                    "Session writer binding is missing or different; recovery cannot rebind legacy history".into(),
                ));
            }
            sessions.push(attr);
        }
        if sessions.is_empty() {
            return Err(Error::WorkspacePermissionDenied(
                "writer has no attributed original Sessions".into(),
            ));
        }
        let _storage = database.guard(&identity).map_err(Error::SubjektivStorage)?;
        retained::prepare_release(
            api,
            &store,
            &identity,
            &lease,
            &binding,
            &input.operation_id,
            recording_scope,
        )
    })();
    if let Err(error) = checked {
        drop(guard);
        if disconnected {
            return Err(error);
        }
        cancel_active_release(api, &store, &gate, &lease, &binding, &request)?;
        // Confirmed cancellation unfreezes the still-current body. Dispatch retained work
        // only after releasing our Subject gate. A new explicit release needs a new op ID.
        for attr in sessions {
            let _ = admit(api, attr);
        }
        return Err(error);
    }
    drop(guard);
    // Make reservation terminal before writer revocation. A delayed Cancel or
    // lost reply can no longer reopen original publication after the scope check.
    // Unknown disconnect retains both writer and occupancy; no status inference.
    confirm_runtime_disconnect(api, &lease, request)?;
    let _guard = gate
        .lock()
        .map_err(|_| Error::Store("Journal lifecycle lock poisoned".into()))?;
    let recording_scope = recording_scope
        .as_ref()
        .ok_or_else(|| Error::RepositoryConflict("Release recording scope missing".into()))?;
    let storage_guard = database.guard(&identity).map_err(Error::SubjektivStorage)?;
    api.config_store.require_subject_release_intent(
        api.workspace_id(),
        &lease,
        &input.binding_id,
        &input.operation_id,
    )?;
    // Revalidate the fixed originals after terminal disconnect. Journal attempts
    // keep their own authority; releasing the body does not close their generation.
    retained::prepare_release(
        api,
        &store,
        &identity,
        &lease,
        &binding,
        &input.operation_id,
        recording_scope,
    )?;
    store
        .release_retained_journal_writer(&binding)
        .map_err(domain)?;
    api.config_store.release_subject_singleton(
        api.workspace_id(),
        &lease,
        &input.binding_id,
        &input.operation_id,
    )?;
    drop(storage_guard);
    drop(_guard);
    if let Err(error) = retained::recover_releases(api) {
        tracing::warn!(%error, "Released Journal continuation deferred");
    }
    Ok(response())
}

pub(super) async fn release_route(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    source: Option<axum::Extension<crate::worker_source::VerifiedRuntimeRequestSource>>,
    Json(input): Json<SubjectReleaseInput>,
) -> ApiResult<Json<runtime_api::SubjectReleaseResponse>> {
    if source.is_some() {
        return Err(Error::WorkspacePermissionDenied(
            "Subject release is a user Host lifecycle operation, not a model tool".into(),
        )
        .into());
    }
    api.host_executors
        .host()?
        .run(move || {
            validate_workspace_scope(&api, &path.workspace_id)?;
            release_subject(&api, input)
                .map(Json)
                .map_err(ApiError::from)
        })
        .await
}

pub(super) async fn recording_route(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    source: Option<axum::Extension<crate::worker_source::VerifiedRuntimeRequestSource>>,
    Json(_request): Json<JournalRecordingRequest>,
) -> ApiResult<Json<JournalRecordingResponse>> {
    // RunCommitted/restore may arrive while the parent holds Host. Admission uses the
    // independent lane, never queues recursively behind the waiting Runtime caller.
    let (source, worker) = verified_source(source)?;
    api.store
        .db_executor()
        .run(move || {
            validate_workspace_scope(&api, &path.workspace_id)?;
            let lease = api
                .store
                .require_current_worker_singleton_owner(&path.workspace_id, &worker)?
                .ok_or_else(|| {
                    Error::WorkspacePermissionDenied(
                        "Journal admission requires the main Subject Worker".into(),
                    )
                })?;
            let (subject, role) =
                subjektiv_scope_from_singleton_key(&lease.key).map_err(|e| e.error)?;
            if role != SubjektivWorkerAuthority::Subject {
                return Err(Error::WorkspacePermissionDenied(
                    "Journal admission is not a Memory Job operation".into(),
                )
                .into());
            }
            authorize_subjektiv_operation(
                &api,
                &worker,
                &path.workspace_id,
                subject,
                "journal.recording.request",
            )?;
            require_enabled(&api, &worker)?;
            let store = JournalCaller::Signed(&source).effect_store(
                &api,
                &worker,
                subject,
                "journal.recording.request",
            )?;
            let attributions = store
                .subject_session_attributions(subject)
                .map_err(|e| Error::Store(e.to_string()))?;
            let mut response = JournalRecordingResponse::NoNewSources;
            for attr in attributions
                .into_iter()
                .filter(|a| a.runtime_id == worker.runtime_id && a.worker_id == worker.worker_id)
            {
                response = admit_sources_in_store(&api, attr, true, store.clone())?;
            }
            Ok(Json(response))
        })
        .await
}
fn verified_source(
    source: Option<axum::Extension<crate::worker_source::VerifiedRuntimeRequestSource>>,
) -> ApiResult<(
    crate::worker_source::VerifiedRuntimeRequestSource,
    RuntimeWorkerRef,
)> {
    let source = source
        .ok_or_else(|| {
            Error::WorkspacePermissionDenied(
                "Journal requires signed Runtime request identity".into(),
            )
        })?
        .0;
    let worker = source.worker_id.as_ref().ok_or_else(|| {
        Error::WorkspacePermissionDenied("Journal requires main Worker identity".into())
    })?;
    let worker = RuntimeWorkerRef::new(&source.runtime_id, worker);
    Ok((source, worker))
}

pub(super) async fn job_route(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    source: Option<axum::Extension<crate::worker_source::VerifiedRuntimeRequestSource>>,
    Json(request): Json<JournalJobRequest>,
) -> ApiResult<Json<JournalJobResponse>> {
    let (source, worker) = verified_source(source)?;
    api.store
        .db_executor()
        .run(move || {
            #[cfg(test)]
            if let Some(observe) = &api.journal_before_effect {
                observe("journal.job.admission");
            }
            validate_workspace_scope(&api, &path.workspace_id)?;
            execute_for(&api, &worker, request, JournalCaller::Signed(&source))
                .map(Json)
                .map_err(ApiError::from)
        })
        .await
}

fn require_enabled(api: &WorkspaceApi, worker: &RuntimeWorkerRef) -> Result<()> {
    if !enabled(api, worker)? {
        return Err(Error::WorkspacePermissionDenied(
            "Journal Feature is disabled".into(),
        ));
    }
    Ok(())
}

fn enabled(api: &WorkspaceApi, worker: &RuntimeWorkerRef) -> Result<bool> {
    let projection = api
        .store
        .worker_registry_projection(api.workspace_id(), worker)?
        .ok_or_else(|| {
            Error::WorkspacePermissionDenied("Journal Worker registration missing".into())
        })?;
    let profile =
        projection.registry.profile.as_deref().ok_or_else(|| {
            Error::WorkspacePermissionDenied("Journal Worker profile missing".into())
        })?;
    let state = api
        .config_store
        .load_workspace_config(api.workspace_id())?
        .ok_or_else(|| Error::Config("Journal requires Workspace Profile configuration".into()))?;
    let profiles = crate::profile_settings::project_profiles_from_workspace_config(
        api.workspace_id(),
        &state,
    )?;
    let key = profile;
    let prompts = api
        .prompt_projection_cache
        .resolve(api.workspace_id(), &state)?;
    let bundle =
        crate::profile_settings::build_virtual_profile_config_bundle_with_prompt_projection(
            &profiles,
            &state,
            api.workspace_id(),
            &api.config.workspace_created_at,
            key,
            prompts.as_ref(),
        )?
        .ok_or_else(|| Error::Config("Journal Worker Profile bundle missing".into()))?;
    let manifest =
        crate::profile_settings::resolve_profile_manifest_from_config_bundle(&bundle, key)?;
    Ok(manifest.feature.subjektiv.enabled())
}

pub(super) fn drain_after_completion(api: &WorkspaceApi, request: &BackendJobRequest) {
    if let Some(grant) = &request.grants.subjektiv_journal {
        let result = (|| {
            let store = store(api)?;
            let attr = store
                .session_attribution(&grant.session_id)
                .map_err(|e| Error::Store(e.to_string()))?
                .ok_or_else(|| Error::Store("Journal Session attribution missing".into()))?;
            let target = RuntimeWorkerRef::new(&attr.runtime_id, &attr.worker_id);
            if let Some(prepared) = api
                .config_store
                .recover_worker_removal_execution(api.workspace_id(), &target)
                .map_err(|error| Error::Store(error.to_string()))?
                && api.store.get_journal_removal(&prepared)?.is_some()
            {
                // The removal parent owns selection from its fixed union. Normal
                // live drain must not replace its scope or bypass the saved end.
                return resume_removals(api);
            }
            // The completed immutable prefix has already passed every execution/cleanup
            // fence and input retirement. Admit a new bounded prefix; never replace or
            // reselect input for an unfinished Job during recovery.
            dispatch_saved_or_new(api, attr.clone(), &grant.journal_id, true)?;
            release_completed_source(api, &attr)
        })();
        if let Err(error) = result {
            tracing::warn!(%error,"Journal pending prefix drain failed; saved input retained");
        }
    }
}

fn material_source(
    api: &WorkspaceApi,
    attribution: &crate::subjektiv::SubjectSessionAttribution,
) -> Result<runtime_api::SessionPublicSource> {
    let target = RuntimeWorkerRef::new(&attribution.runtime_id, &attribution.worker_id);
    Ok(
        if let Some(archive) = api.config_store.worker_session_archive(
            api.workspace_id(),
            &target,
            &attribution.session_id,
        )? {
            runtime_api::SessionPublicSource::Archived {
                archive_id: archive.archive_id,
            }
        } else {
            runtime_api::SessionPublicSource::Retained {
                worker_id: worker_runtime::identity::WorkerId::parse(&attribution.worker_id)
                    .ok_or_else(|| Error::Store("Journal Worker identity invalid".into()))?,
            }
        },
    )
}
/// Close the first-run save/notification gap: Runtime can commit an initial turn before
/// the create handler has finalized the Backend registry required by recording callbacks.
/// Reconcile only after creation succeeds, using the ordinary current-owner/Feature checks.
pub(super) fn reconcile_created_worker(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
) -> Result<()> {
    let Some(lease) = api
        .store
        .require_current_worker_singleton_owner(api.workspace_id(), worker)?
    else {
        return Ok(());
    };
    let Some(subject) = lease.key.strip_prefix(SUBJEKTIV_SINGLETON_PREFIX) else {
        return Ok(());
    };
    require_enabled(api, worker)?;
    let store = store(api)?;
    for attr in store
        .subject_session_attributions(subject)
        .map_err(|error| Error::Store(error.to_string()))?
        .into_iter()
        .filter(|attr| attr.runtime_id == worker.runtime_id && attr.worker_id == worker.worker_id)
    {
        admit(api, attr)?;
    }
    Ok(())
}

pub(super) fn recover(api: &WorkspaceApi) -> Result<()> {
    resume_removals(api)?;
    let store = store(api)?;
    let mut after: Option<(String, String)> = None;
    loop {
        let page = store
            .list_subjects_after(
                100,
                after
                    .as_ref()
                    .map(|(time, id): &(String, String)| (time.as_str(), id.as_str())),
            )
            .map_err(|e| Error::Store(e.to_string()))?;
        for subject in &page.items {
            let mut cursor: Option<(String, String)> = None;
            let snapshot = Utc::now().to_rfc3339();
            loop {
                let attrs = store
                    .subject_session_attribution_page(
                        &subject.id,
                        None,
                        &snapshot,
                        cursor.as_ref().map(|(a, b)| (a.as_str(), b.as_str())),
                        100,
                    )
                    .map_err(|e| Error::Store(e.to_string()))?;
                for attr in &attrs.items {
                    let session_host = JournalHostContext::validated_session(&store, attr.clone())
                        .map_err(domain)?;
                    let erased = store
                        .journal_session_erased(&session_host)
                        .map_err(domain)?;
                    if let Err(error) = reconcile_session_jobs(api, attr, erased) {
                        tracing::warn!(%error,session_id=%attr.session_id,"Journal attempt recovery remains unresolved");
                    }
                    if erased {
                        if let Err(error) = material_source(api, attr).and_then(|source| {
                            erase_source(api.runtime.as_ref(), api.workspace_id(), attr, source)
                        }) {
                            tracing::warn!(%error,session_id=%attr.session_id,"Erased Session physical cleanup remains recoverable");
                        }
                        continue;
                    }
                    // Recovery only consumes saved Job input and domain receipts. A missed
                    // admission notification is not permission to reread/reselect live history.
                    if let Ok(Some(id)) = store.journal_for_session(&session_host) {
                        if let Err(error) = dispatch_saved_or_new(api, attr.clone(), &id, false)
                            .and_then(|_| release_completed_source(api, attr))
                        {
                            tracing::warn!(%error,session_id=%attr.session_id,"retained Journal prefix recovery blocked");
                        }
                    }
                }
                if !attrs.has_more {
                    break;
                }
                cursor = attrs
                    .items
                    .last()
                    .map(|a| (a.attributed_at.clone(), a.session_id.clone()));
                if cursor.is_none() {
                    break;
                }
            }
        }
        if !page.has_more {
            break;
        }
        after = page
            .items
            .last()
            .map(|s| (s.created_at.clone(), s.id.clone()));
        if after.is_none() {
            break;
        }
    }
    Ok(())
}

pub(super) fn recording_request(
    store: &crate::subjektiv::SubjektivStore,
    attribution: crate::subjektiv::SubjectSessionAttribution,
    journal_id: &str,
    input: JournalInput,
) -> Result<BackendJobRequest> {
    let source =
        JournalHostContext::validated_session(store, attribution.clone()).map_err(domain)?;
    let binding_id = source
        .writer_binding_id()
        .ok_or_else(|| Error::WorkspacePermissionDenied("Journal source lifetime missing".into()))?
        .to_owned();
    let source_worker = RuntimeWorkerRef::new(&attribution.runtime_id, &attribution.worker_id);
    let grant = SubjektivJournalGrant {
        binding_id,
        subject_id: attribution.subject_id,
        session_id: attribution.session_id,
        journal_id: journal_id.into(),
        selected_ranges: input.selected_ranges.clone(),
    };
    let request = BackendJobRequest {
        job_id: format!("journal-recording:{}",Uuid::new_v4()), purpose: "subjektiv_journal".into(),
        input_ref: format!("subjektiv://{}/journal/{journal_id}",grant.subject_id),
        input: serde_json::to_value(input).map_err(|e| Error::Store(e.to_string()))?,
        instruction: "Record ONLY the immutable body-bearing capture supplied in this input. These exact canonical source bytes, identities and ranges are the whole input for this Job; do not fetch original Sessions or use ReadJournalText Source. First ReadJournalState; initial/current Journal position/open text remain separately paged by ReadJournalText. Sequentially save CreateJournalEntry/RewriteJournalEntry/FixJournalEntry, current position and receipt-backed CheckpointJournal. Input delivery is not understanding or reflection. Never infer event completion from turn/task/job/compaction boundaries. Preserve the exact operation ID/full input after response loss and reconcile its saved receipt. SubmitBackendJobResult only after the complete selected prefix has a saved checkpoint; no aggregate JournalRecordResult.".into(),
        profile: "builtin:subjektiv-journal-recording".into(),
        grants: BackendJobGrants { subjektiv_journal: Some(grant), ..Default::default() },
        serialization_key: Some(journal_serialization_key(journal_id)), source_worker: Some(source_worker), notification_target: None,
        limits: crate::backend_job::BackendJobLimits { timeout_seconds: 600, ..Default::default() },
    };
    saved_input(
        &request,
        request
            .grants
            .subjektiv_journal
            .as_ref()
            .expect("constructed grant"),
    )?;
    request.validate()?;
    Ok(request)
}

pub(super) fn capture_input(
    api: &WorkspaceApi,
    attr: &crate::subjektiv::SubjectSessionAttribution,
    order: u64,
    offset: u64,
    budget: usize,
) -> Result<session_store::capture::CaptureDto> {
    authorize_source(api, attr, "journal.recording.request")?;
    match api
        .runtime
        .session_recording_capture(
            &attr.runtime_id,
            runtime_api::SessionRecordingCaptureRequest {
                workspace_id: api.workspace_id().into(),
                source: material_source(api, attr)?,
                expected_session_id: attr.session_id.clone(),
                start: session_store::capture::RecordingPosition {
                    canonical_order: order,
                    byte_offset: offset,
                },
                max_entries: MAX_JOURNAL_RANGES,
                max_serialized_bytes: budget,
            },
        )
        .map_err(RuntimeRegistryError::into_error)?
    {
        runtime_api::SessionRecordingCaptureAvailability::Captured { slice } => {
            if slice.capture.session_id != attr.session_id
                || serde_json::to_vec(&slice)
                    .map_err(|e| Error::Store(e.to_string()))?
                    .len()
                    > budget
            {
                return Err(Error::RepositoryConflict(
                    "recording capture identity/budget mismatch".into(),
                ));
            }
            Ok(slice.capture)
        }
        runtime_api::SessionRecordingCaptureAvailability::NotMaterialized { session_id }
            if session_id == attr.session_id && order == 0 && offset == 0 =>
        {
            Ok(session_store::capture::CaptureDto {
                version: 1,
                session_id,
                segment_id: "not-materialized".into(),
                view_entry_count: 0,
                entries: vec![],
            })
        }
        runtime_api::SessionRecordingCaptureAvailability::Unavailable { reason, .. } => {
            Err(Error::Store(format!(
                "committed Journal capture unavailable ({reason:?}); no latest-source fallback"
            )))
        }
        _ => Err(Error::Store(
            "committed Journal capture identity mismatch; no latest-source fallback".into(),
        )),
    }
}

fn existing_removal_store(
    service: &WorkerRemovalService,
) -> Result<crate::subjektiv::SubjektivStore> {
    let identity = service
        .journal_storage
        .existing_identity(&service.journal_registration)
        .map_err(Error::SubjektivStorage)?;
    let database = service
        .journal_storage
        .open_existing(&service.journal_registration, &identity)
        .map_err(Error::SubjektivStorage)?;
    crate::subjektiv::SubjektivStore::from_database(database).map_err(subjektiv_store_error)
}

fn removal_attribution(
    service: &WorkerRemovalService,
    target: &RuntimeWorkerRef,
) -> Result<Option<crate::subjektiv::SubjectSessionAttribution>> {
    let Some(lease) = service
        .store
        .retained_worker_singleton_lease(&service.workspace_id, target)?
    else {
        return Ok(None);
    };
    let Some(subject) = lease.key.strip_prefix(SUBJEKTIV_SINGLETON_PREFIX) else {
        return Ok(None);
    };
    let store = existing_removal_store(service)?;
    let mut after: Option<(String, String)> = None;
    let snapshot = Utc::now().to_rfc3339();
    loop {
        let page = store
            .subject_session_attribution_page(
                subject,
                None,
                &snapshot,
                after.as_ref().map(|(a, b)| (a.as_str(), b.as_str())),
                100,
            )
            .map_err(|e| Error::Store(e.to_string()))?;
        if let Some(attr) = page
            .items
            .iter()
            .find(|a| a.runtime_id == target.runtime_id && a.worker_id == target.worker_id)
        {
            return Ok(Some(attr.clone()));
        }
        if !page.has_more {
            return Ok(None);
        }
        after = page
            .items
            .last()
            .map(|a| (a.attributed_at.clone(), a.session_id.clone()));
        if after.is_none() {
            return Err(Error::Store(
                "Session attribution page did not advance".into(),
            ));
        }
    }
}

/// Explicit erasure is not a continuity handoff. The permanent source tombstone is required
/// before revoking this body and recording the separate erase-driven singleton release.
fn finish_erased_release(
    api: &WorkspaceApi,
    attr: &crate::subjektiv::SubjectSessionAttribution,
) -> Result<()> {
    let continuation = lifecycle_lock(api, &format!("release-continuation:{}", attr.subject_id))?;
    let _continuation = continuation
        .lock()
        .map_err(|_| Error::Store("release continuation poisoned".into()))?;
    let gate = lifecycle_lock(api, &attr.subject_id)?;
    let _guard = gate
        .lock()
        .map_err(|_| Error::Store("Journal lifecycle lock poisoned".into()))?;
    let store = store(api)?;
    let host = JournalHostContext::validated_session(&store, attr.clone()).map_err(domain)?;
    if !store.journal_session_erased(&host).map_err(domain)? {
        return Err(Error::WorkspacePermissionDenied(
            "explicit erase tombstone missing".into(),
        ));
    }
    for session in store
        .subject_session_attributions(&attr.subject_id)
        .map_err(|e| Error::Store(e.to_string()))?
        .into_iter()
        .filter(|session| {
            session.runtime_id == attr.runtime_id && session.worker_id == attr.worker_id
        })
    {
        require_erased_job_copies(api, &session.session_id)?;
    }
    // Pre-binding bodies have no write token. This stable receipt label is not a writer
    // grant: the domain erase operation records an inactive lifetime without binding Sessions.
    let erased_body_id = format!("erased-body:{}", attr.worker_id);
    let binding_id = host.writer_binding_id().unwrap_or(&erased_body_id);
    let worker = RuntimeWorkerRef::new(&attr.runtime_id, &attr.worker_id);
    let lease = api
        .config_store
        .retained_worker_singleton_lease(api.workspace_id(), &worker)?
        .ok_or_else(|| Error::RepositoryConflict("erased Subject lease missing".into()))?;
    let operation = api
        .config_store
        .retained_subject_release_operation(api.workspace_id(), &lease, binding_id)?
        .unwrap_or_else(|| format!("subject-erasure:{}", attr.session_id));
    api.config_store
        .reserve_subject_release(api.workspace_id(), &lease, binding_id, &operation)?;
    // Every attributed Session must be tombstoned, including saved-before-admission history
    // absent from journal_sources. Normal continuity completion is not erasure authority.
    if !store.release_erased_journal_body(&host).map_err(domain)? {
        return Err(Error::RepositoryConflict(
            "Subject has an unerased attributed Session; erase-driven release retains occupancy"
                .into(),
        ));
    }
    if !api.config_store.subject_release_proven(
        api.workspace_id(),
        &lease,
        binding_id,
        &operation,
    )? {
        api.config_store.confirm_subject_release_proof(
            api.workspace_id(),
            &lease,
            binding_id,
            &operation,
            true,
        )?;
    }
    api.config_store
        .release_subject_singleton(api.workspace_id(), &lease, binding_id, &operation)
}

/// Ordinary removal may not treat a terminal Job or an archived source as proof of
/// recording or shutdown. Explicit erasure uses its separate tombstone/Purge contract.
pub(super) fn prepare_worker_removal(
    service: &WorkerRemovalService,
    runtime: &RuntimeRegistry,
    target: &RuntimeWorkerRef,
) -> Result<Option<crate::retention::ErasedJournalSession>> {
    if let Some(api) = service.journal_dispatch_api.upgrade()
        && subjektiv_deletion::deleting_worker(&api, target)?
    {
        // Full Subject deletion has a permanent domain fence. It deliberately drops
        // Journal continuity, but retains source originals through its archive policy.
        return Ok(None);
    }
    let Some(attr) = removal_attribution(service, target)? else {
        if service
            .store
            .retained_worker_singleton_lease(&service.workspace_id, target)?
            .is_some_and(|lease| lease.key.starts_with(SUBJEKTIV_SINGLETON_PREFIX))
        {
            return Err(Error::RepositoryConflict("Subject Session evidence is missing; ordinary deletion cannot release its occupancy".into()));
        }
        return Ok(None);
    };
    let store = existing_removal_store(service)?;
    let source = runtime_api::SessionPublicSource::Retained {
        worker_id: worker_runtime::identity::WorkerId::parse(&target.worker_id)
            .ok_or_else(|| Error::Store("Worker identity invalid".into()))?,
    };
    let api = service.journal_dispatch_api.upgrade().ok_or_else(|| {
        Error::Store("Workspace API unavailable for Journal removal Feature gate".into())
    })?;
    if let Some(proof) =
        crate::retention::ErasedJournalSession::validated(&service.workspace_id, &store, &attr)?
    {
        // Erasure is monotonic. Retry the same physical intent, never admission or generation.
        reconcile_session_jobs(&api, &attr, true)?;
        erase_source(runtime, &service.workspace_id, &attr, source)?;
        finish_erased_release(&api, &attr)?;
        return Ok(Some(proof));
    }
    // Ordinary continuity is advanced only after its canonical removal plan has
    // been accepted. The operation binds Runtime ingress fencing and fixed scope.
    Ok(None)
}

/// A successful erase response proves the Runtime's permanent fence. The reserved
/// archive covers transfer-before-Backend-commit crashes too. Never admit or generate.
pub(super) fn erased_archive_removal_proof(
    service: &WorkerRemovalService,
    runtime: &RuntimeRegistry,
    target: &RuntimeWorkerRef,
    prepared: &crate::retention::PreparedWorkerRemoval,
) -> Result<Option<crate::retention::RuntimeJournalErasureAck>> {
    if let Some(api) = service.journal_dispatch_api.upgrade()
        && subjektiv_deletion::deleting_worker(&api, target)?
    {
        // Resume the exact canonical retention request after full Subject fencing.
        // Subject deletion retains originals; it is not per-Session source erasure.
        return Ok(None);
    }
    let Some(attr) = removal_attribution(service, target)? else {
        return Ok(None);
    };
    let store = existing_removal_store(service)?;
    let Some(proof) =
        crate::retention::ErasedJournalSession::validated(&service.workspace_id, &store, &attr)?
    else {
        return Ok(None);
    };
    let api = service.journal_dispatch_api.upgrade().ok_or_else(|| {
        Error::Store("Workspace API unavailable for erased removal recovery".into())
    })?;
    reconcile_session_jobs(&api, &attr, true)?;
    let source = runtime_api::SessionPublicSource::Retained {
        worker_id: prepared.runtime_request.worker_id,
    };
    let ack = match proof.clone().erase_runtime_source(runtime, source) {
        Ok(ack) => ack,
        Err(original) => {
            let Some(archive_id) = prepared.runtime_request.archive_id.clone() else {
                return Err(original);
            };
            proof
                .erase_runtime_source(
                    runtime,
                    runtime_api::SessionPublicSource::Archived { archive_id },
                )
                .map_err(|_| original)?
        }
    };
    finish_erased_release(&api, &attr)?;
    Ok(Some(ack))
}

pub(super) fn validate_removal_retention(
    service: &WorkerRemovalService,
    target: &RuntimeWorkerRef,
    prepared: &crate::retention::PreparedWorkerRemoval,
) -> Result<()> {
    if let Some(api) = service.journal_dispatch_api.upgrade()
        && subjektiv_deletion::deleting_worker(&api, target)?
    {
        // The plan-local retention policy and canonical Runtime receipt remain required.
        // Neither unfinished Journals nor a stopped old Controller are deletion blockers.
        return Ok(());
    }
    let Some(attr) = removal_attribution(service, target)? else {
        if service
            .store
            .retained_worker_singleton_lease(&service.workspace_id, target)?
            .is_some_and(|lease| lease.key.starts_with(SUBJEKTIV_SINGLETON_PREFIX))
        {
            return Err(Error::RepositoryConflict(
                "Subject Session evidence missing during removal recovery".into(),
            ));
        }
        return Ok(());
    };
    let store = existing_removal_store(service)?;
    let host = JournalHostContext::validated_session(&store, attr.clone()).map_err(domain)?;
    if store.journal_session_erased(&host).map_err(domain)? {
        if prepared.runtime_request.session_disposition
            != worker_runtime::retention::SessionDisposition::Purge
            || prepared.runtime_request.diagnostics_disposition
                != worker_runtime::retention::DiagnosticsDisposition::Purge
            || prepared.runtime_request.archive_id.is_some()
        {
            return Err(Error::RepositoryConflict(
                "Erased Journal Session requires source and diagnostics Purge, not an archive"
                    .into(),
            ));
        }
        // Physical erasure precedes the plan. Runtime resumes aggregate cleanup with
        // its retained permanent fence, including recovery after the aggregate is gone.
        return Ok(());
    }
    let source = runtime_api::SessionPublicSource::Retained {
        worker_id: worker_runtime::identity::WorkerId::parse(&target.worker_id)
            .ok_or_else(|| Error::Store("Worker identity invalid".into()))?,
    };
    let runtime = service.runtime.upgrade().ok_or_else(|| {
        Error::Store("Runtime unavailable for source retention confirmation".into())
    })?;
    let retention_required = store.journal_retention_required(&host).map_err(domain)?;
    if prepared.runtime_request.session_disposition
        == worker_runtime::retention::SessionDisposition::Purge
        && prepared.runtime_request.archive_id.is_none()
        && !retention_required
        && store
            .journal_worker_was_released(&attr.subject_id, &attr.runtime_id, &attr.worker_id)
            .map_err(domain)?
    {
        // Compatibility for an already issued, history-complete ordinary Purge
        // plan. New Subject plans always Archive. Keep the old fingerprint and
        // disposition; never turn this into an erasure or allow an active writer.
        // A missing Runtime Worker is NOT success: execute_worker_retention still
        // has to return the exact persisted receipt before anything can commit.
        // A pending receipt may outlive the physical aggregate while Runtime's
        // in-memory catalog still contains the Worker. Pin release is best effort
        // in this historical branch; exact Runtime retention replay still enforces
        // pins if the source exists, and requires a receipt if it is already gone.
        let api = service.journal_dispatch_api.upgrade().ok_or_else(|| {
            Error::Store("Workspace API unavailable for Journal effect reconciliation".into())
        })?;
        require_session_job_effects_ended(&api, &attr.session_id)?;
        if let Err(error) = retain_source(&runtime, &service.workspace_id, &attr, source, true) {
            tracing::debug!(%error, "Historical Purge source unavailable; reconcile exact Runtime receipt");
        }
        return Ok(());
    }
    if prepared.runtime_request.session_disposition
        != worker_runtime::retention::SessionDisposition::Archive
        || prepared.runtime_request.archive_id.is_none()
    {
        return Err(Error::RepositoryConflict(
            "Subject removal retains its original Session; explicit erasure is required for Purge"
                .into(),
        ));
    }
    // Source removal owns the handoff, not the recording Job's completion.
    // Only the retained-recording owner may declare the whole frozen scope
    // complete and release its pins; a saved checkpoint alone is insufficient.
    let release = service
        .store
        .get_journal_removal(prepared)?
        .and_then(|progress| progress.metadata.retained_recording)
        .is_some_and(|recording| recording.completed);
    if let Err(original) = retain_source(&runtime, &service.workspace_id, &attr, source, release) {
        // A lost reply may follow the physical transfer. Reconcile only the exact
        // reserved archive and Session, whether or not an admitted remainder exists.
        let archive = runtime_api::SessionPublicSource::Archived {
            archive_id: prepared
                .runtime_request
                .archive_id
                .clone()
                .expect("checked"),
        };
        retain_source(&runtime, &service.workspace_id, &attr, archive, release)
            .map_err(|_| original)?;
    }
    Ok(())
}

/// Writer release precedes physical removal. The exact Runtime receipt authorizes
/// the catalog commit, which releases occupancy atomically. A crash retains the old
/// singleton and resumes this same receipt, without restoring or generating history.
pub(super) fn finish_worker_removal<T>(
    service: &WorkerRemovalService,
    target: &RuntimeWorkerRef,
    prepared: &crate::retention::PreparedWorkerRemoval,
    result: &worker_runtime::retention::WorkerRetentionExecutionResult,
    commit: impl FnOnce() -> T,
) -> Result<T> {
    if let Some(api) = service.journal_dispatch_api.upgrade()
        && subjektiv_deletion::deleting_worker(&api, target)?
    {
        // Full Subject deletion already fenced the domain. Its canonical retention
        // commit, not ordinary Journal writer validation, owns completion.
        return Ok(commit());
    }
    let Some(attr) = removal_attribution(service, target)? else {
        return Ok(commit());
    };
    let request = &prepared.runtime_request;
    if prepared.plan.worker != *target
        || request.workspace_id != service.workspace_id
        || request.source_runtime_id != target.runtime_id
        || request.worker_id.to_string() != target.worker_id
        || !result.source_removed
        || result.worker_id != request.worker_id
        || result.operation_id != request.operation_id
        || result.input_fingerprint != request.input_fingerprint
        || result.session_disposition != request.session_disposition
        || result.diagnostics_disposition != request.diagnostics_disposition
        || result.diagnostics_retained
            != (request.diagnostics_disposition
                == worker_runtime::retention::DiagnosticsDisposition::Retain)
    {
        return Err(Error::RepositoryConflict(
            "Subject removal receipt does not match its plan".into(),
        ));
    }
    if request.session_disposition == worker_runtime::retention::SessionDisposition::Archive {
        let archive = result.archive.as_ref().ok_or_else(|| {
            Error::RepositoryConflict("Subject removal archive receipt is missing".into())
        })?;
        if Some(&archive.archive_id) != request.archive_id.as_ref()
            || archive.workspace_id != request.workspace_id
            || archive.source_runtime_id != request.source_runtime_id
            || archive.source_worker_id != request.worker_id
            || archive.operation_id != request.operation_id
            || archive.input_fingerprint != request.input_fingerprint
            || archive.policy_id != request.policy_id
        {
            return Err(Error::RepositoryConflict(
                "Subject removal archive identity mismatch".into(),
            ));
        }
    }
    let api = service.journal_dispatch_api.upgrade().ok_or_else(|| {
        Error::Store("Workspace API unavailable for Subject removal completion".into())
    })?;
    let gate = lifecycle_lock(&api, &attr.subject_id)?;
    let _guard = gate
        .lock()
        .map_err(|_| Error::Store("Journal lifecycle lock poisoned".into()))?;
    let progress = service.store.get_journal_removal(prepared)?;
    let identity = match &progress {
        Some(progress) => progress.metadata.binding.database.clone(),
        None => service
            .journal_storage
            .existing_identity(&service.journal_registration)
            .map_err(Error::SubjektivStorage)?,
    };
    let database = service
        .journal_storage
        .open_existing(&service.journal_registration, &identity)
        .map_err(Error::SubjektivStorage)?;
    let _storage = database.guard(&identity).map_err(Error::SubjektivStorage)?;
    let store = crate::subjektiv::SubjektivStore::from_database(database.clone())
        .map_err(subjektiv_store_error)?;
    if let Some(archive) = &result.archive {
        let primary = store
            .session_attribution(&archive.source_session_id)
            .map_err(subjektiv_store_error)?
            .filter(|source| {
                source.subject_id == attr.subject_id
                    && source.runtime_id == target.runtime_id
                    && source.worker_id == target.worker_id
            })
            .ok_or_else(|| {
                Error::RepositoryConflict(
                    "Archive primary Session ownership differs from the removed body".into(),
                )
            })?;
        let _ = JournalHostContext::validated_session(&store, primary).map_err(domain)?;
        if let Some(progress) = &progress {
            removal::require_archive_scope(progress, archive)?;
        }
    }
    let host = JournalHostContext::validated_session(&store, attr.clone()).map_err(domain)?;
    if store.journal_session_erased(&host).map_err(domain)? {
        if request.session_disposition == worker_runtime::retention::SessionDisposition::Archive {
            // Erasure may win after Runtime archive transfer. Do not publish that
            // archive or skip writer revocation: the same removal retry must take
            // erased_archive_removal_proof and its physical-erasure commit path.
            return Err(Error::RepositoryConflict(
                "Session erased during removal; reconcile the same removal operation".into(),
            ));
        }
        return Ok(commit());
    }
    if progress.is_none()
        && request.session_disposition == worker_runtime::retention::SessionDisposition::Purge
        && request.archive_id.is_none()
        && result.archive.is_none()
        && store
            .journal_worker_was_released(&attr.subject_id, &attr.runtime_id, &attr.worker_id)
            .map_err(domain)?
    {
        // Complete an already issued, fully recorded removal from its exact
        // receipt. No unrecorded scope is invented after originals are gone.
        require_body_recording_and_effects_ended(&api, &store, &attr)?;
        return Ok(commit());
    }
    let progress = progress.as_ref().ok_or_else(|| {
        Error::RepositoryConflict("Ordinary removal has no retained recording handoff".into())
    })?;
    let archive = result.archive.as_ref().ok_or_else(|| {
        Error::RepositoryConflict("Ordinary removal must preserve its frozen originals".into())
    })?;
    require_retained_recording_handoff(progress, &attr, archive)?;
    let attributed: std::collections::HashSet<_> = store
        .subject_session_attributions(&attr.subject_id)
        .map_err(subjektiv_store_error)?
        .into_iter()
        .filter(|session| {
            session.runtime_id == attr.runtime_id && session.worker_id == attr.worker_id
        })
        .map(|session| session.session_id)
        .collect();
    let frozen: std::collections::HashSet<_> = progress
        .metadata
        .members
        .as_ref()
        .expect("checked retained scope")
        .iter()
        .map(|member| member.session_id.clone())
        .collect();
    if attributed != frozen {
        return Err(Error::RepositoryConflict(
            "Retained recording scope no longer covers the body".into(),
        ));
    }
    if !store
        .journal_worker_was_released(&attr.subject_id, &attr.runtime_id, &attr.worker_id)
        .map_err(domain)?
    {
        return Err(Error::RepositoryConflict(
            "Runtime removal cannot precede Journal writer release".into(),
        ));
    }
    // Frozen originals and recording responsibility outlive producer removal.
    // Recording Jobs may remain pending or failed; keep erasure and DB identity
    // excluded only through this short catalog commit.
    Ok(commit())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Empty {}

pub(super) async fn finish_route(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    source: Option<axum::Extension<crate::worker_source::VerifiedRuntimeRequestSource>>,
    Json(_request): Json<Empty>,
) -> ApiResult<Json<JournalJobCompletion>> {
    let (source, worker) = verified_source(source)?;
    api.store
        .db_executor()
        .run(move || {
            validate_workspace_scope(&api, &path.workspace_id)?;
            finish_for(&api, &worker, JournalCaller::Signed(&source))
                .map(Json)
                .map_err(ApiError::from)
        })
        .await
}

/// A bounded Runtime transport view, not a second history index or a local-path fallback.
fn erase_source(
    runtime: &RuntimeRegistry,
    workspace: &str,
    attr: &crate::subjektiv::SubjectSessionAttribution,
    source: runtime_api::SessionPublicSource,
) -> Result<()> {
    let response = runtime
        .session_material_erase(
            &attr.runtime_id,
            runtime_api::SessionMaterialEraseRequest {
                workspace_id: workspace.into(),
                source,
                expected_session_id: attr.session_id.clone(),
            },
        )
        .map_err(|e| e.into_error())?;
    if response.session_id != attr.session_id {
        return Err(Error::RepositoryConflict(
            "Session erase receipt differs from the requested identity".into(),
        ));
    }
    Ok(())
}

fn retain_source(
    runtime: &RuntimeRegistry,
    workspace: &str,
    attr: &crate::subjektiv::SubjectSessionAttribution,
    source: runtime_api::SessionPublicSource,
    release: bool,
) -> Result<()> {
    let pin_id =
        subjektiv::journal::source_retention_pin(workspace, &attr.subject_id, &attr.session_id)
            .to_string();
    let request = runtime_api::SessionMaterialPinRequest {
        workspace_id: workspace.into(),
        source,
        expected_session_id: attr.session_id.clone(),
        pin_id: pin_id.clone(),
    };
    let response = if release {
        runtime.session_material_release(&attr.runtime_id, request)
    } else {
        runtime.session_material_pin(&attr.runtime_id, request)
    }
    .map_err(|e| e.into_error())?;
    if response.session_id != attr.session_id || response.pin_id != pin_id {
        return Err(Error::RepositoryConflict(
            "Session source retention receipt differs from the requested identity".into(),
        ));
    }
    Ok(())
}
fn release_completed_source(
    api: &WorkspaceApi,
    attr: &crate::subjektiv::SubjectSessionAttribution,
) -> Result<()> {
    let store = store(api)?;
    let host = JournalHostContext::validated_session(&store, attr.clone()).map_err(domain)?;
    let target = RuntimeWorkerRef::new(&attr.runtime_id, &attr.worker_id);
    if let Some(prepared) = api
        .config_store
        .recover_worker_removal_execution(api.workspace_id(), &target)
        .map_err(|error| Error::Store(error.to_string()))?
        && api
            .store
            .get_journal_removal(&prepared)?
            .is_some_and(|progress| progress.metadata.retained_recording.is_some())
    {
        // The frozen-scope owner releases pins after every original is reflected.
        // An empty/admitted checkpoint says nothing about its unadmitted tail.
        return Ok(());
    }
    if api
        .config_store
        .get_release_journal_recording(api.workspace_id(), &target)?
        .is_some()
    {
        // Even a prepared handoff owns the whole scope, not just admitted ranges.
        return Ok(());
    }
    require_session_job_effects_ended(api, &attr.session_id)?;
    if !store.journal_retention_required(&host).map_err(domain)?
        && api
            .store
            .get_worker_registry(api.workspace_id(), &target)?
            .is_none()
    {
        retain_source(
            api.runtime.as_ref(),
            api.workspace_id(),
            attr,
            material_source(api, attr)?,
            true,
        )?;
    }
    Ok(())
}

pub(super) fn after_worker_removal(
    service: &WorkerRemovalService,
    result: &worker_runtime::retention::WorkerRetentionExecutionResult,
) {
    let Some(archive) = &result.archive else {
        return;
    };
    let Some(api) = service.journal_dispatch_api.upgrade() else {
        return;
    };
    let release = (|| {
        let store = store(&api)?;
        if let Some(attr) = store
            .session_attribution(&archive.source_session_id)
            .map_err(|e| Error::Store(e.to_string()))?
        {
            release_completed_source(&api, &attr)?;
        }
        Ok::<_, Error>(())
    })();
    if let Err(error) = release {
        tracing::warn!(%error,"Worker removed; Journal source pin release remains recoverable");
    }
}

/// The domain tombstone is checked even after cleanup. Erasure never causes generation.
pub(super) fn session_erased(api: &WorkspaceApi, request: &BackendJobRequest) -> Result<bool> {
    let Some(grant) = &request.grants.subjektiv_journal else {
        return Ok(false);
    };
    if subjektiv_deletion::pending(api, &grant.subject_id)? {
        return Ok(true);
    }
    let store = store(api)?;
    store
        .journal_session_erased(&host(api, &store, grant)?)
        .map_err(domain)
}

/// Journal authority is confined to the authenticated Workspace.
fn require_job_workspace(api: &WorkspaceApi, job: &BackendJobRecord) -> Result<()> {
    if job.workspace_id != api.workspace_id() {
        return Err(Error::WorkspacePermissionDenied(
            "Journal Job Workspace mismatch".into(),
        ));
    }
    Ok(())
}

fn can_resume_partial(job: &BackendJobRecord) -> bool {
    job.state == BackendJobState::Failed
        && matches!(
            job.failure_category.as_deref(),
            Some("journal_attempt_fenced" | "journal_execution_terminated")
        )
        && job.input_disposition == crate::backend_job::JobInputDisposition::Available
        && job.current_attempt < job.request.limits.max_attempts
        && job.load_executable_request().is_ok()
        && job
            .request
            .grants
            .subjektiv_journal
            .as_ref()
            .is_some_and(|grant| saved_input(&job.request, grant).is_ok())
}

/// Continue only the frozen partial Job whose actual termination initiated cleanup.
/// Reopen/retry rereads the same canonical Job and still checks all historical fences.
pub(super) fn resume_after_execution_cleanup(
    api: &WorkspaceApi,
    job: &BackendJobRecord,
    attempt: &BackendJobAttemptRecord,
) -> Result<()> {
    if job.failure_category.as_deref() != Some("journal_execution_terminated")
        || !can_resume_partial(job)
        || job.current_attempt != attempt.attempt
    {
        return Ok(());
    }
    let grant = job
        .request
        .grants
        .subjektiv_journal
        .as_ref()
        .ok_or_else(|| Error::Store("Journal continuation grant missing".into()))?;
    let attribution = store(api)?
        .session_attribution(&grant.session_id)
        .map_err(|error| Error::Store(error.to_string()))?
        .ok_or_else(|| Error::Store("Journal continuation attribution missing".into()))?;
    dispatch_saved_or_new(api, attribution, &grant.journal_id, false)
}

/// Outcome is not a shutdown receipt. Inspect every earlier attempt as well: a late
/// Host operation/cleanup from one of them must not affect the next owner. The root
/// checks exact Runtime/Controller, Feature/background, Host-drain and cleanup evidence
/// without waiting under this Subject gate or reconstructing empty live ownership.
pub(super) fn require_job_effects_ended(api: &WorkspaceApi, job: &BackendJobRecord) -> Result<()> {
    if matches!(
        job.state,
        BackendJobState::Pending | BackendJobState::Finalizing
    ) {
        return Err(Error::RepositoryConflict(
            "Journal Job is still pending".into(),
        ));
    }
    // Terminal Job state is published only by common finalization; consumers
    // do not reconstruct cleanup from historical Runtime resources.
    Ok(())
}

fn require_session_job_effects_ended(api: &WorkspaceApi, session: &str) -> Result<()> {
    let mut after = None;
    loop {
        let jobs = api.store.list_backend_journal_jobs(
            api.workspace_id(),
            session,
            after.as_deref(),
            100,
        )?;
        for job in &jobs {
            require_job_effects_ended(api, job)?;
        }
        if jobs.len() < 100 {
            return Ok(());
        }
        after = jobs.last().map(|job| job.request.job_id.clone());
    }
}

fn require_retained_recording_handoff(
    progress: &crate::store::JournalRemovalProgress,
    attr: &crate::subjektiv::SubjectSessionAttribution,
    archive: &worker_runtime::retention::WorkerSessionArchiveManifest,
) -> Result<()> {
    use crate::store::JournalRemovalPhase;
    if !matches!(
        progress.metadata.phase,
        JournalRemovalPhase::Released | JournalRemovalPhase::Completed
    ) || progress.metadata.binding.subject_id != attr.subject_id
        || progress.worker.runtime_id != attr.runtime_id
        || progress.worker.worker_id != attr.worker_id
        || progress.workspace_id != archive.workspace_id
        || progress.operation_id != archive.operation_id
        || progress.input_fingerprint != archive.input_fingerprint
        || progress.worker.runtime_id != archive.source_runtime_id
        || progress.worker.worker_id != archive.source_worker_id.to_string()
        || progress
            .metadata
            .retained_recording
            .as_ref()
            .is_none_or(|retained| retained.archive_id != archive.archive_id)
    {
        return Err(Error::RepositoryConflict(
            "Removal has no matching released recording handoff".into(),
        ));
    }
    removal::require_archive_scope(progress, archive)
}

fn require_body_recording_and_effects_ended(
    api: &WorkspaceApi,
    store: &crate::subjektiv::SubjektivStore,
    attr: &crate::subjektiv::SubjectSessionAttribution,
) -> Result<()> {
    for session in store
        .subject_session_attributions(&attr.subject_id)
        .map_err(subjektiv_store_error)?
        .into_iter()
        .filter(|session| {
            session.runtime_id == attr.runtime_id && session.worker_id == attr.worker_id
        })
    {
        require_session_job_effects_ended(api, &session.session_id)?;
        let host = JournalHostContext::validated_session(store, session).map_err(domain)?;
        if store.journal_retention_required(&host).map_err(domain)? {
            return Err(Error::RepositoryConflict(
                "Consumed source has unfinished Journal recording".into(),
            ));
        }
    }
    Ok(())
}

/// A definitive inability to use the frozen input/authority is an outcome, not a
/// claim that execution ended. Cleanup still owns its independent exact-owner proof.
fn fail_recovery(
    api: &WorkspaceApi,
    job: &BackendJobRecord,
    attempt: &BackendJobAttemptRecord,
    category: &str,
    detail: &str,
) -> Result<()> {
    let failed = api.store.finish_backend_job_attempt(
        api.workspace_id(),
        &job.request.job_id,
        &attempt.attempt_id,
        BackendJobAttemptState::Failed,
        category,
        detail,
        &Utc::now().to_rfc3339(),
    )?;
    api.schedule_backend_job_worker_cleanup(&failed);
    Ok(())
}

/// Reconcile the current accepted attempt using only immutable input and the exact
/// retained generation. A complete checkpoint settles the same Job independently of
/// teardown. Partial continuation requires independent shutdown AND writer fencing.
pub(super) fn reconcile_attempt(
    api: &WorkspaceApi,
    job: &BackendJobRecord,
    attempt: &BackendJobAttemptRecord,
) -> Result<()> {
    let grant = job
        .request
        .grants
        .subjektiv_journal
        .as_ref()
        .ok_or_else(|| Error::InvalidInput("Journal recovery requires a grant".into()))?;
    let gate = lifecycle_lock(api, &grant.subject_id)?;
    let guard = gate
        .lock()
        .map_err(|_| Error::Store("Journal lifecycle lock poisoned".into()))?;
    reconcile_attempt_under_gate(api, job, attempt, false, guard)
}

fn reconcile_attempt_under_gate(
    api: &WorkspaceApi,
    job: &BackendJobRecord,
    attempt: &BackendJobAttemptRecord,
    attest_received_input: bool,
    guard: std::sync::MutexGuard<'_, ()>,
) -> Result<()> {
    let grant = job
        .request
        .grants
        .subjektiv_journal
        .as_ref()
        .ok_or_else(|| Error::InvalidInput("Journal recovery requires a grant".into()))?;
    // Reread rather than letting a delayed callback settle an old owner or resurrect
    // erased/retired input. The Store repeats this full CAS when saving completion.
    let current_job = api
        .store
        .get_backend_job(api.workspace_id(), &job.request.job_id)?
        .ok_or_else(|| Error::Store("Journal recovery Job missing".into()))?;
    let current_attempt = api
        .store
        .get_backend_job_attempt(api.workspace_id(), &job.request.job_id, &attempt.attempt_id)?
        .ok_or_else(|| Error::Store("Journal recovery attempt missing".into()))?;
    if &current_job != job || &current_attempt != attempt {
        return Err(Error::RepositoryConflict(
            "Journal recovery owner changed; reread receipts".into(),
        ));
    }
    if job.state != BackendJobState::Pending || attempt.state.terminal() {
        return Ok(());
    }
    let expected_run = crate::backend_job::runtime_run_id(&attempt.attempt_id);
    if job.workspace_id != api.workspace_id()
        || attempt.workspace_id != job.workspace_id
        || attempt.job_id != job.request.job_id
        || attempt.attempt != job.current_attempt
        || attempt.attempt_id
            != crate::backend_job::attempt_id(&job.request.job_id, attempt.attempt)
        || attempt.input_digest != job.input_digest
        || attempt
            .runtime_run_id
            .as_ref()
            .is_some_and(|run| run != &expected_run)
        || (attempt.state == BackendJobAttemptState::Dispatched
            && (attempt.worker.is_none() || attempt.runtime_run_id.is_none()))
    {
        return Err(Error::WorkspacePermissionDenied(
            "Journal recovery identity mismatch".into(),
        ));
    }
    if job.input_disposition != crate::backend_job::JobInputDisposition::Available {
        return fail_recovery(
            api,
            job,
            attempt,
            "journal_input_unavailable",
            "Retired or erased Journal input cannot be executed or reconstructed from a Session",
        );
    }
    if job.load_executable_request().is_err() {
        return fail_recovery(
            api,
            job,
            attempt,
            "journal_input_invalid",
            "Immutable Journal input or intent fingerprint is invalid; input and checkpoints retained",
        );
    }
    let input = match saved_input(&job.request, grant) {
        Ok(input) => input,
        Err(_) => {
            return fail_recovery(
                api,
                job,
                attempt,
                "journal_input_invalid",
                "Body-bearing Journal snapshot does not match its frozen grant; no live input fallback",
            );
        }
    };
    let store = store(api)?;
    let host = match host(api, &store, grant) {
        Ok(host) => host,
        Err(Error::WorkspacePermissionDenied(_)) => {
            return fail_recovery(
                api,
                job,
                attempt,
                "journal_attribution_unavailable",
                "Saved Journal Session attribution is missing or mismatched",
            );
        }
        Err(error) => return Err(error),
    };
    let attr = store
        .session_attribution(&grant.session_id)
        .map_err(|error| Error::Store(error.to_string()))?
        .ok_or_else(|| Error::WorkspacePermissionDenied("Journal attribution missing".into()))?;
    if store.journal_session_erased(&host).map_err(domain)? {
        return fail_recovery(
            api,
            job,
            attempt,
            "journal_session_erased",
            "Source Session was explicitly erased",
        );
    }
    if subjektiv_deletion::pending(api, &grant.subject_id)? {
        return fail_recovery(
            api,
            job,
            attempt,
            "journal_subject_deleting",
            "Subject deletion fences Journal execution",
        );
    }
    match subject_authorization::authorize_job_input(api, &job.request) {
        Ok(()) => {}
        Err(error) => match error.error {
            Error::WorkspacePermissionDenied(_) => {
                return fail_recovery(
                    api,
                    job,
                    attempt,
                    "journal_authority_unavailable",
                    "Saved Job provenance or current Subject policy denies Journal recovery",
                );
            }
            error => return Err(error),
        },
    }
    let store = subject_authorization::journal_attempt_effect_store(
        api,
        &store,
        job,
        attempt,
        subject_authorization::JournalAttemptEffect::ReconcileCompletion,
    )?;
    let host = recording_host(api, &store, &job.request, attempt)?;
    let binding = subjektiv::api::JobAttemptBinding {
        job_id: job.request.job_id.clone(),
        attempt_id: attempt.attempt_id.clone(),
    };
    if let Some(generation) = store
        .retained_journal_generation(&host, &grant.journal_id, &binding)
        .map_err(domain)?
    {
        input.validate_generation(&generation).map_err(domain)?;
        let context = JournalJobContext::validated_recovery(&store, &host, &generation.id, binding)
            .map_err(domain)?;
        match journal::service::finish(&store, &context) {
            Ok(completion) => {
                let submission = BackendJobResultSubmission {
                    job_id: job.request.job_id.clone(),
                    attempt_id: attempt.attempt_id.clone(),
                    input_digest: attempt.input_digest.clone(),
                    result: serde_json::to_value(completion)
                        .map_err(|e| Error::Store(e.to_string()))?,
                };
                let current = api.store.reconcile_backend_journal_attempt(
                    api.workspace_id(),
                    attempt,
                    Some(&submission),
                    &Utc::now().to_rfc3339(),
                )?;
                let saved = api
                    .store
                    .get_backend_job(api.workspace_id(), &job.request.job_id)?
                    .ok_or_else(|| Error::Store("Reconciled Journal Job missing".into()))?;
                // Delivery reacquires the Subject gate and rechecks deletion. Only the
                // durable reconciliation belongs to this critical section.
                drop(guard);
                notification_delivery::deliver_job_result(
                    api,
                    &BackendJobResultAcceptance {
                        job: saved,
                        attempt: current.clone(),
                        replayed: false,
                    },
                );
                // Completion must not reclaim input or release pins before all necessary
                // attempts/Host effects/cleanup end. The root cleanup rechecks these fences.
                api.schedule_backend_job_worker_cleanup(&current);
                return Ok(());
            }
            Err(JournalError::Conflict(_)) => {}
            Err(error) => return Err(domain(error)),
        }
    }
    if attest_received_input {
        return attest_delivery(
            api,
            &BackendJobReservation {
                job: job.clone(),
                attempt: attempt.clone(),
                replayed: true,
                resource_reused: false,
            },
        );
    }
    if attempt.state == BackendJobAttemptState::Reserved {
        // The generic reserved-attempt owner can still dispatch this exact saved input.
        return Ok(());
    }
    // An actual Runtime receipt can settle the failed execution outcome, but Host
    // callbacks may still be returning. Terminalize first so the existing cleanup
    // owner can close admission and drain the original resource; retry only after it.
    if api
        .require_backend_job_attempt_effects_ended(attempt)
        .is_err()
        && api.backend_job_execution_termination(attempt)?.is_some()
    {
        return fail_recovery(
            api,
            job,
            attempt,
            "journal_execution_terminated",
            "Runtime execution ended before checkpoint completion; Host drain and cleanup remain pending",
        );
    }
    // Missing evidence is neither rejection nor permission to reselect input.
    api.require_backend_job_attempt_effects_ended(attempt)?;
    for number in 1..attempt.attempt {
        let id = crate::backend_job::attempt_id(&job.request.job_id, number);
        let prior = api
            .store
            .get_backend_job_attempt(api.workspace_id(), &job.request.job_id, &id)?
            .ok_or_else(|| Error::Store("Prior Journal attempt evidence missing".into()))?;
        api.require_backend_job_attempt_effects_ended(&prior)?;
    }
    close(api, &job.request, attempt)?;
    api.store.finish_backend_job_attempt(
        api.workspace_id(), &job.request.job_id, &attempt.attempt_id,
        BackendJobAttemptState::Failed, "journal_attempt_fenced",
        "Execution and Host effects ended; generation fenced; retained input/checkpoints permit bounded same-Job continuation",
        &Utc::now().to_rfc3339(),
    )?;
    // Do not call admission here: it can select newer live Session bytes. Existing
    // bounded retry uses only this Job's input and the operation/checkpoint receipts.
    drop(guard);
    dispatch_saved_or_new(api, attr, &grant.journal_id, false)
}

/// Cleanup return is the retry boundary for an interrupted normal Job, not
/// Completed cleanup metadata or an earlier result/Stop callback.
pub(super) fn resume_recording_after_cleanup(
    api: &WorkspaceApi,
    ended: &BackendJobAttemptRecord,
) -> Result<()> {
    let Some(job) = api
        .store
        .get_backend_job(api.workspace_id(), &ended.job_id)?
    else {
        return Ok(());
    };
    if job.current_attempt != ended.attempt || !can_resume_partial(&job) {
        return Ok(());
    }
    let Some(grant) = job.request.grants.subjektiv_journal.as_ref() else {
        return Ok(());
    };
    let store = store(api)?;
    let attr = store
        .session_attribution(&grant.session_id)
        .map_err(subjektiv_store_error)?
        .filter(|attr| attr.subject_id == grant.subject_id)
        .ok_or_else(|| {
            Error::WorkspacePermissionDenied("Journal retry source ownership missing".into())
        })?;
    // Dispatch repeats exact attempt cleanup/effect fencing, current authority,
    // immutable input validation and the saved maximum-attempt budget.
    dispatch_saved_or_new(api, attr, &grant.journal_id, false)
}

fn reconcile_session_jobs(
    api: &WorkspaceApi,
    attr: &crate::subjektiv::SubjectSessionAttribution,
    erased: bool,
) -> Result<()> {
    let mut after = None;
    loop {
        let jobs = api.store.list_backend_journal_jobs(
            api.workspace_id(),
            &attr.session_id,
            after.as_deref(),
            100,
        )?;
        for job in &jobs {
            let id = crate::backend_job::attempt_id(&job.request.job_id, job.current_attempt);
            let attempt = api
                .store
                .get_backend_job_attempt(api.workspace_id(), &job.request.job_id, &id)?
                .ok_or_else(|| Error::Store("Journal current attempt missing".into()))?;
            if erased && !attempt.state.terminal() {
                let (category, detail) = (
                    "journal_session_erased",
                    "Source Session was explicitly erased",
                );
                api.store.finish_backend_job_attempt(
                    api.workspace_id(),
                    &job.request.job_id,
                    &id,
                    BackendJobAttemptState::Failed,
                    category,
                    detail,
                    &Utc::now().to_rfc3339(),
                )?;
            } else if !erased && job.state == BackendJobState::Pending {
                if let Err(error) = reconcile_attempt(api, job, &attempt) {
                    api.retain_backend_job_recovery(
                        &attempt, "journal_receipt_reconciliation", &error.to_string(),
                        "Reconcile the same input/generation and exact Runtime/Host receipts; never reselect Session input",
                    )?;
                }
            }
            if erased {
                api.store.reclaim_backend_job_input(
                    api.workspace_id(),
                    &job.request.job_id,
                    &job.input_digest,
                    crate::backend_job::JobInputRetirementReason::ExplicitErasure,
                )?;
            }
            // Common Job recovery owns finalization. A newly cancelled current
            // attempt can be handed off directly; settled history is never swept.
            if erased && !attempt.state.terminal() {
                if let Some(current) = api.store.get_backend_job_attempt(
                    api.workspace_id(),
                    &job.request.job_id,
                    &id,
                )? {
                    api.schedule_backend_job_worker_cleanup(&current);
                }
            }
        }
        if jobs.len() < 100 {
            return Ok(());
        }
        after = jobs.last().map(|job| job.request.job_id.clone());
    }
}

fn require_erased_job_copies(api: &WorkspaceApi, session: &str) -> Result<()> {
    let mut after = None;
    loop {
        let jobs = api.store.list_backend_journal_jobs(
            api.workspace_id(),
            session,
            after.as_deref(),
            100,
        )?;
        for job in &jobs {
            if job.input_disposition != crate::backend_job::JobInputDisposition::Erased {
                return Err(Error::RepositoryConflict(
                    "Journal input erasure remains incomplete".into(),
                ));
            }
            for number in 1..=job.current_attempt {
                let id = crate::backend_job::attempt_id(&job.request.job_id, number);
                let attempt = api
                    .store
                    .get_backend_job_attempt(api.workspace_id(), &job.request.job_id, &id)?
                    .ok_or_else(|| Error::Store("erased Journal attempt missing".into()))?;
                if attempt.worker.is_some()
                    && attempt.worker_cleanup_state != Some(BackendJobWorkerCleanupState::Completed)
                {
                    return Err(Error::RepositoryConflict("Journal delivered-copy Purge remains pending; retry the same erasure after cleanup".into()));
                }
            }
            // Report the pending canonical cleanup first, but even its Completed
            // marker cannot acknowledge erasure without independent execution fences.
            require_job_effects_ended(api, job)?;
        }
        if jobs.len() < 100 {
            return Ok(());
        }
        after = jobs.last().map(|j| j.request.job_id.clone());
    }
}

impl WorkspaceApi {
    /// Host-only erasure saves the domain fence, cancels Jobs, then erases originals.
    /// Runtime requires a stopped source Worker. Physical failure leaves the domain
    /// tombstone and cancellations committed for exact retry or removal preflight;
    /// terminal Job execution Sessions remain owned by ordinary Job cleanup.
    pub fn erase_subjektiv_journal_session(
        &self,
        session_id: &str,
        request_id: &str,
    ) -> Result<()> {
        let store = store(self)?;
        let attr = store
            .session_attribution(session_id)
            .map_err(|e| Error::Store(e.to_string()))?
            .ok_or_else(|| {
                Error::WorkspacePermissionDenied("Session attribution missing".into())
            })?;
        let gate = lifecycle_lock(self, &attr.subject_id)?;
        let _guard = gate
            .lock()
            .map_err(|_| Error::Store("Journal lifecycle lock poisoned".into()))?;
        let host = JournalHostContext::validated_session(&store, attr.clone()).map_err(domain)?;
        store
            .erase_journal_session(&host, request_id)
            .map_err(domain)?;
        reconcile_session_jobs(self, &attr, true)?;
        erase_source(
            self.runtime.as_ref(),
            self.workspace_id(),
            &attr,
            material_source(self, &attr)?,
        )?;
        require_erased_job_copies(self, &attr.session_id)
    }
}

/// Generic Job recovery also reaches Journal receipts when the original source Worker
/// or its admission notification is unavailable. Errors stay attached to the exact attempt
/// and root's bounded recovery owner, rather than creating a replacement Job/state.
pub(super) fn recover_attempts(api: &WorkspaceApi) -> Result<()> {
    let mut after = None;
    loop {
        let page = api.store.list_backend_journal_pending_attempts(
            api.workspace_id(),
            after.as_deref(),
            100,
        )?;
        for attempt in &page {
            let job = api
                .store
                .get_backend_job(api.workspace_id(), &attempt.job_id)?
                .ok_or_else(|| Error::Store("Retained Journal recovery Job missing".into()))?;
            if let Err(error) = reconcile_attempt(api, &job, attempt) {
                api.retain_backend_job_recovery(
                    attempt, "journal_receipt_reconciliation", &error.to_string(),
                    "Reconcile the same input/generation and exact Runtime/Host receipts; never reselect Session input",
                )?;
            }
        }
        if page.len() < 100 {
            return Ok(());
        }
        after = page.last().map(|attempt| attempt.job_id.clone());
    }
}

#[cfg(test)]
mod recovery_input_tests {
    //! Local wire/policy tests only. Runtime/Host shutdown and persistence-fault recovery
    //! belong to the connected Server suite; these tests do not substitute for that evidence.
    use super::*;
    use session_store::capture::{
        CaptureBody, CaptureDto, CaptureEntryDto, CaptureSource, CaptureUsePolicy,
    };
    use session_store::public_material::{
        PublicMaterialDescriptor, PublicMaterialIdentity, PublicMaterialOrigin,
    };
    use sha2::{Digest, Sha256};

    fn snapshot_request() -> BackendJobRequest {
        let segment = uuid::Uuid::from_u128(1);
        let text = "saved original input";
        let capture = CaptureDto {
            version: 1,
            session_id: "original-session".into(),
            segment_id: segment.to_string(),
            view_entry_count: 1,
            entries: vec![CaptureEntryDto {
                entry_id: "entry-1".into(),
                order: 0,
                source: CaptureSource {
                    segment_id: segment.to_string(),
                    entry_index: 0,
                    provenance: protocol::SessionEntryProvenance::HumanInput,
                    history_origin: Some(
                        session_store::LoggedSessionHistoryOrigin::LocalHumanInput,
                    ),
                    derived_from: vec![],
                },
                policy: CaptureUsePolicy {
                    recording_material: true,
                    memory_evidence: false,
                    observation_readable: false,
                },
                body: CaptureBody::CanonicalPublic {
                    descriptor: PublicMaterialDescriptor {
                        identity: PublicMaterialIdentity {
                            segment_id: segment,
                            entry_ref: "Eentry-1".into(),
                        },
                        canonical_order: 0,
                        origin: PublicMaterialOrigin::History {
                            origin: session_store::LoggedSessionHistoryOrigin::LocalHumanInput,
                        },
                        sha256: Sha256::digest(text.as_bytes())
                            .iter()
                            .map(|byte| format!("{byte:02x}"))
                            .collect(),
                        total_bytes: text.len() as u64,
                    },
                    byte_offset: 0,
                    text: text.into(),
                    boundary: None,
                },
            }],
        };
        let ranges = subjektiv::journal::recording::capture_ranges(&capture).unwrap();
        let input = JournalInput {
            version: 1,
            subject_id: "original-subject".into(),
            journal_id: "original-journal".into(),
            initial_state_token: "saved-state".into(),
            initial_open_id: None,
            previous_fixed_end: None,
            selected_ranges: ranges.clone(),
            capture,
        };
        BackendJobRequest {
            job_id: "original-job".into(),
            purpose: "subjektiv_journal".into(),
            input_ref: "journal://original-journal".into(),
            input: serde_json::to_value(input).unwrap(),
            instruction: "Record only the immutable input".into(),
            profile: "builtin:subjektiv-journal-recording".into(),
            grants: BackendJobGrants {
                subjektiv_journal: Some(SubjektivJournalGrant {
                    binding_id: "original-lifetime".into(),
                    subject_id: "original-subject".into(),
                    session_id: "original-session".into(),
                    journal_id: "original-journal".into(),
                    selected_ranges: ranges,
                }),
                ..Default::default()
            },
            serialization_key: Some(journal_serialization_key("original-journal")),
            source_worker: None,
            notification_target: None,
            limits: Default::default(),
        }
    }

    #[test]
    fn journal_request_requires_source_lifetime_in_the_single_wire_contract() {
        let request = snapshot_request();
        request.validate().unwrap();
        let mut wire = serde_json::to_value(&request).unwrap();
        assert_eq!(
            wire["grants"]["subjektiv_journal"]["binding_id"],
            "original-lifetime"
        );
        let decoded: BackendJobRequest = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(
            decoded.fingerprint().unwrap(),
            request.fingerprint().unwrap()
        );
        wire["grants"]["subjektiv_journal"]
            .as_object_mut()
            .unwrap()
            .remove("binding_id");
        assert!(serde_json::from_value::<BackendJobRequest>(wire).is_err());
        let mut different_lifetime = request.clone();
        different_lifetime
            .grants
            .subjektiv_journal
            .as_mut()
            .unwrap()
            .binding_id = "successor-lifetime".into();
        assert_ne!(
            different_lifetime.fingerprint().unwrap(),
            request.fingerprint().unwrap()
        );
    }

    #[test]
    fn saved_body_rejects_reselection_descriptor_and_corruption() {
        let request = snapshot_request();
        let grant = request.grants.subjektiv_journal.as_ref().unwrap();
        assert!(saved_input(&request, grant).is_ok());
        for field in ["subject", "session", "journal", "selection"] {
            let mut changed = grant.clone();
            match field {
                "subject" => changed.subject_id = "successor-subject".into(),
                "session" => changed.session_id = "successor-session".into(),
                "journal" => changed.journal_id = "successor-journal".into(),
                "selection" => changed.selected_ranges.clear(),
                _ => unreachable!(),
            }
            assert!(saved_input(&request, &changed).is_err(), "{field}");
        }
        let mut descriptor = request.clone();
        descriptor.input = serde_json::json!({
            "journal_id": grant.journal_id,
            "selected_ranges": grant.selected_ranges,
        });
        assert!(saved_input(&descriptor, grant).is_err());
        let mut corrupt = request.clone();
        let mut input: JournalInput = serde_json::from_value(corrupt.input.clone()).unwrap();
        let CaptureBody::CanonicalPublic { text, .. } = &mut input.capture.entries[0].body else {
            unreachable!()
        };
        *text = "substituted new input".into();
        corrupt.input = serde_json::to_value(input).unwrap();
        assert!(saved_input(&corrupt, grant).is_err());
    }

    #[test]
    fn partial_retry_requires_fenced_cause_available_body_and_remaining_budget() {
        let mut request = snapshot_request();
        request.limits.max_attempts = 2;
        let job = BackendJobRecord {
            workspace_id: "workspace".into(),
            input_digest: request.input_digest().unwrap(),
            intent_fingerprint: request.fingerprint().unwrap(),
            request,
            input_disposition: crate::backend_job::JobInputDisposition::Available,
            state: BackendJobState::Failed,
            current_attempt: 1,
            result: None,
            result_digest: None,
            failure_category: Some("journal_attempt_fenced".into()),
            failure_detail: Some("Exact execution and writer fences confirmed".into()),
            failure_cause: None,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            completed_at: Some("2026-01-01T00:00:00Z".into()),
        };
        assert!(can_resume_partial(&job));
        for cause in [
            "unrecognized_failure",
            "journal_body_released",
            "journal_session_erased",
            "execution_failed",
        ] {
            let mut failed = job.clone();
            failed.failure_category = Some(cause.into());
            assert!(!can_resume_partial(&failed), "{cause}");
        }
        for disposition in [
            crate::backend_job::JobInputDisposition::Reclaimed,
            crate::backend_job::JobInputDisposition::Erased,
        ] {
            let mut retired = job.clone();
            retired.input_disposition = disposition;
            assert!(!can_resume_partial(&retired));
        }
        let mut corrupt = job.clone();
        corrupt.request.input["capture"]["entries"][0]["body"]["text"] =
            serde_json::json!("changed body");
        assert!(!can_resume_partial(&corrupt));
        // A consistent generic fingerprint cannot make a descriptor into executable Journal input.
        let mut descriptor = job.clone();
        let grant = descriptor
            .request
            .grants
            .subjektiv_journal
            .as_ref()
            .unwrap();
        descriptor.request.input = serde_json::json!({
            "journal_id": grant.journal_id,
            "selected_ranges": grant.selected_ranges,
        });
        descriptor.input_digest = descriptor.request.input_digest().unwrap();
        descriptor.intent_fingerprint = descriptor.request.fingerprint().unwrap();
        assert!(!can_resume_partial(&descriptor));
        let mut exhausted = job;
        exhausted.current_attempt = exhausted.request.limits.max_attempts;
        assert!(!can_resume_partial(&exhausted));
    }
}
