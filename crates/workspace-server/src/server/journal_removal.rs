//! A removal intent supplies bounded immutable Jobs until every frozen original
//! has a checkpoint. Job recovery owns attempts; this parent owns only fixed scope
//! and continuation. Waiting returns without retaining a lifecycle/DB lock.

use super::*;
use crate::retention::PreparedWorkerRemoval;
use crate::store::{
    FrozenJournalRemovalMember, JournalRemovalBinding, JournalRemovalCause as Cause,
    JournalRemovalPhase as Phase, JournalRemovalProgress,
};
use worker_runtime::retention::{WorkerRemovalFreeze, WorkerRemovalSessionMember};

fn existing_database(api: &WorkspaceApi) -> Result<crate::FeatureDatabase> {
    let identity = api
        .feature_storage
        .existing_identity(&api.subjektiv_registration)
        .map_err(Error::SubjektivStorage)?;
    api.feature_storage
        .open_existing(&api.subjektiv_registration, &identity)
        .map_err(Error::SubjektivStorage)
}

fn waiting(
    service: &WorkerRemovalService,
    prepared: &PreparedWorkerRemoval,
    progress: &JournalRemovalProgress,
    phase: Phase,
    cause: Cause,
) -> Result<Option<worker::WorkspaceResponse>> {
    let current =
        service
            .store
            .update_journal_removal_progress(prepared, progress, phase, Some(cause))?;
    service.worker_projection.refresh(&current.worker)?;
    tracing::info!(operation_id=%current.operation_id, runtime_id=%current.worker.runtime_id,
        worker_id=%current.worker.worker_id, ?phase, ?cause,
        "Worker removal retained; Journal parent is waiting");
    let observation = progress_observation(&current);
    let response = server_api::WorkerRemoveResponse {
        removed: false,
        runtime_id: current.worker.runtime_id,
        worker_id: current.worker.worker_id,
        operation_id: Some(observation.operation_id),
        state: Some(observation.state),
        phase: Some(observation.phase),
        cause: observation.cause,
    };
    Ok(Some(worker::WorkspaceResponse {
        status: StatusCode::ACCEPTED.as_u16(),
        body: serde_json::to_string(&response).map_err(|error| Error::Store(error.to_string()))?,
    }))
}

/// Shared last-mile decision for ordinary Remove and explicit release. The
/// retained owner resource, not cleanup status or an empty live map, excludes
/// accepted cleanup descendants through the current DB/writer commit.
pub(super) fn release_writer_checked(
    api: &WorkspaceApi,
    database: &crate::FeatureDatabase,
    expected: &feature_storage::DatabaseIdentity,
    store: &crate::subjektiv::SubjektivStore,
    binding: &JournalWriterBinding,
    coverage: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let mut owners = Vec::new();
    for attr in store
        .subject_session_attributions(&binding.subject_id)
        .map_err(subjektiv_store_error)?
        .into_iter()
        .filter(|attr| attr.runtime_id == binding.runtime_id && attr.worker_id == binding.worker_id)
    {
        let mut after = None;
        loop {
            let jobs = api.store.list_backend_journal_jobs(
                api.workspace_id(),
                &attr.session_id,
                after.as_deref(),
                100,
            )?;
            for job in &jobs {
                require_job_effects_ended(api, job)?;
                for attempt in api
                    .store
                    .list_backend_job_attempts(api.workspace_id(), &job.request.job_id)?
                {
                    if attempt.worker.is_some() {
                        if attempt.worker_cleanup_state
                            != Some(BackendJobWorkerCleanupState::Completed)
                        {
                            return Err(Error::RepositoryConflict(
                                "Journal attempt cleanup is not complete".into(),
                            ));
                        }
                        owners.push(
                            crate::backend_job_host_effects::JobHostEffects::try_fence_retained(
                                &api.config.database_path,
                                &attempt,
                            )?,
                        );
                    }
                }
            }
            if jobs.len() < 100 {
                break;
            }
            after = jobs.last().map(|job| job.request.job_id.clone());
        }
    }
    let _storage = database.guard(expected).map_err(Error::SubjektivStorage)?;
    coverage()?;
    store.release_journal_writer(binding).map_err(domain)
}

pub(in crate::server) fn removal_observation(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
) -> Result<Option<server_api::WorkerRemovalObservation>> {
    let Some(prepared) = api
        .config_store
        .recover_worker_removal_execution(api.workspace_id(), worker)
        .map_err(|error| Error::Store(error.to_string()))?
    else {
        return Ok(None);
    };
    let Some(progress) = api.store.get_journal_removal(&prepared)? else {
        return Ok(None);
    };
    if progress.metadata.phase == Phase::Completed {
        return Ok(None);
    }
    Ok(Some(progress_observation(&progress)))
}

fn progress_observation(progress: &JournalRemovalProgress) -> server_api::WorkerRemovalObservation {
    let cause = progress.metadata.cause.map(|cause| match cause {
        Cause::RecordingDisabled => server_api::WorkerRemovalCause::RecordingDisabled,
        Cause::SourceUnavailable => server_api::WorkerRemovalCause::SourceUnavailable,
        Cause::OwnershipUnknown => server_api::WorkerRemovalCause::OwnershipUnknown,
        Cause::JobPending => server_api::WorkerRemovalCause::JobPending,
        Cause::JobFailed => server_api::WorkerRemovalCause::JobFailed,
        Cause::ExecutionUnconfirmed => server_api::WorkerRemovalCause::ExecutionUnconfirmed,
        Cause::StorageUnavailable => server_api::WorkerRemovalCause::StorageUnavailable,
    });
    server_api::WorkerRemovalObservation {
        operation_id: progress.operation_id.clone(),
        state: if matches!(
            progress.metadata.cause,
            Some(
                Cause::RecordingDisabled
                    | Cause::SourceUnavailable
                    | Cause::OwnershipUnknown
                    | Cause::JobFailed
                    | Cause::ExecutionUnconfirmed
                    | Cause::StorageUnavailable
            )
        ) {
            server_api::WorkerRemovalState::Held
        } else {
            server_api::WorkerRemovalState::Waiting
        },
        phase: match progress.metadata.phase {
            Phase::AwaitingQuiescence => server_api::WorkerRemovalPhase::AwaitingQuiescence,
            Phase::Recording => server_api::WorkerRemovalPhase::Recording,
            Phase::AwaitingEffectsEnd => server_api::WorkerRemovalPhase::AwaitingEffectsEnd,
            Phase::Released => server_api::WorkerRemovalPhase::Released,
            Phase::Completed => server_api::WorkerRemovalPhase::Completed,
        },
        cause,
    }
}

fn member_attribution(
    store: &crate::subjektiv::SubjektivStore,
    prepared: &PreparedWorkerRemoval,
    subject: &str,
    member: &WorkerRemovalSessionMember,
) -> Result<crate::subjektiv::SubjectSessionAttribution> {
    store
        .session_attribution(&member.session_id.to_string())
        .map_err(subjektiv_store_error)?
        .filter(|attr| {
            attr.subject_id == subject
                && attr.runtime_id == prepared.plan.worker.runtime_id
                && attr.worker_id == prepared.plan.worker.worker_id
        })
        .ok_or_else(|| {
            Error::RepositoryConflict(
                "Frozen Session ownership is absent or differs from the Journal body".into(),
            )
        })
}

/// The same checkpoint/attempt test is used for explicit release and Remove.
/// An admitted prefix is not a fixed endpoint, and a terminal Job is not a checkpoint.
fn require_member_complete(
    api: &WorkspaceApi,
    store: &crate::subjektiv::SubjektivStore,
    attr: &crate::subjektiv::SubjectSessionAttribution,
    member: &WorkerRemovalSessionMember,
) -> Result<()> {
    require_session_job_effects_ended(api, &attr.session_id)?;
    let host = JournalHostContext::validated_session(store, attr.clone()).map_err(domain)?;
    if store.journal_session_erased(&host).map_err(domain)? {
        return Err(Error::RepositoryConflict(
            "Session erasure must use its separate removal path".into(),
        ));
    }
    let id = store.journal_for_session(&host).map_err(domain)?;
    if !member.descriptors.is_empty() {
        let id = id.as_deref().ok_or_else(|| {
            Error::RepositoryConflict("Frozen original history has not been admitted".into())
        })?;
        let journal = store.read_journal(&host, id).map_err(domain)?;
        if (journal.process_order, journal.process_byte)
            != (member.end.canonical_order, member.end.byte_offset)
        {
            return Err(Error::RepositoryConflict(
                "Journal checkpoint has not reached the frozen end".into(),
            ));
        }
        for descriptor in &member.descriptors {
            let source = store
                .read_journal_source(&host, id, descriptor.canonical_order)
                .map_err(domain)?;
            if source.source.pointer.segment_id != descriptor.identity.segment_id
                || source.source.pointer.entry_ref != descriptor.identity.entry_ref
                || source.source.content_sha256 != descriptor.sha256
                || source.source.total_bytes != descriptor.total_bytes
                || source.reflected_bytes != descriptor.total_bytes
                || !source.unavailable.is_empty()
            {
                return Err(Error::RepositoryConflict(
                    "Frozen original has no matching full checkpoint".into(),
                ));
            }
        }
    } else if member.end.canonical_order != 0 || member.end.byte_offset != 0 {
        return Err(Error::RepositoryConflict(
            "Frozen empty scope has a nonempty endpoint".into(),
        ));
    }
    if store.journal_retention_required(&host).map_err(domain)? {
        return Err(Error::RepositoryConflict(
            "Journal still retains unreflected input or a generation".into(),
        ));
    }
    Ok(())
}

pub(super) fn require_archive_scope(
    progress: &JournalRemovalProgress,
    archive: &worker_runtime::retention::WorkerSessionArchiveManifest,
) -> Result<()> {
    let members =
        progress.metadata.members.as_ref().ok_or_else(|| {
            Error::RepositoryConflict("Archive has no frozen recording scope".into())
        })?;
    let mut archived = std::collections::HashMap::new();
    archived.insert(
        archive.source_session_id.clone(),
        archive.source_active_segment_id,
    );
    for binding in &archive.retained_sessions {
        if binding.owner.workspace_id != progress.workspace_id
            || binding.owner.worker_id != progress.worker.worker_id
            || archived
                .insert(
                    binding.session_id.to_string(),
                    Some(binding.active_segment_id),
                )
                .is_some()
        {
            return Err(Error::RepositoryConflict(
                "Archive retained Session ownership is inconsistent".into(),
            ));
        }
    }
    if members.is_empty()
        || members.len() != archived.len()
        || members
            .iter()
            .any(|member| archived.get(&member.session_id) != Some(&member.active_segment_id))
    {
        return Err(Error::RepositoryConflict(
            "Archive does not preserve the frozen recording members".into(),
        ));
    }
    Ok(())
}

pub(super) fn require_scope_complete(
    api: &WorkspaceApi,
    store: &crate::subjektiv::SubjektivStore,
    worker: &RuntimeWorkerRef,
    subject: &str,
    members: &[WorkerRemovalSessionMember],
) -> Result<()> {
    verify_members(members)?;
    let attributed: std::collections::HashSet<_> = store
        .subject_session_attributions(subject)
        .map_err(subjektiv_store_error)?
        .into_iter()
        .filter(|attr| attr.runtime_id == worker.runtime_id && attr.worker_id == worker.worker_id)
        .map(|attr| attr.session_id)
        .collect();
    let frozen: std::collections::HashSet<_> = members
        .iter()
        .map(|member| member.session_id.to_string())
        .collect();
    if members.is_empty() || frozen.len() != members.len() || attributed != frozen {
        return Err(Error::RepositoryConflict(
            "Journal recording scope ownership is absent or changed".into(),
        ));
    }
    for member in members {
        let attr = store
            .session_attribution(&member.session_id.to_string())
            .map_err(subjektiv_store_error)?
            .filter(|attr| {
                attr.subject_id == subject
                    && attr.runtime_id == worker.runtime_id
                    && attr.worker_id == worker.worker_id
            })
            .ok_or_else(|| {
                Error::RepositoryConflict("Journal recording member ownership missing".into())
            })?;
        require_member_complete(api, store, &attr, member)?;
    }
    Ok(())
}

fn capture_member(
    api: &WorkspaceApi,
    prepared: &PreparedWorkerRemoval,
    member: &WorkerRemovalSessionMember,
    order: u64,
    offset: u64,
    budget: usize,
) -> Result<session_store::capture::CaptureDto> {
    let slice = api
        .runtime
        .capture_worker_removal_session(
            &prepared.plan.worker,
            runtime_api::WorkerRemovalSessionCaptureRequest {
                workspace_id: api.workspace_id().into(),
                operation_id: prepared.plan.operation_id.clone(),
                session_id: member.session_id,
                active_segment_id: member.active_segment_id,
                start: session_store::capture::RecordingPosition {
                    canonical_order: order,
                    byte_offset: offset,
                },
                max_entries: MAX_JOURNAL_RANGES,
                max_serialized_bytes: budget,
            },
        )
        .map_err(RuntimeRegistryError::into_error)?;
    if slice.capture.session_id != member.session_id.to_string()
        || serde_json::to_vec(&slice)
            .map_err(|error| Error::Store(error.to_string()))?
            .len()
            > budget
    {
        return Err(Error::RepositoryConflict(
            "Frozen capture identity or envelope budget mismatch".into(),
        ));
    }
    let ranges = journal::recording::capture_ranges(&slice.capture).map_err(domain)?;
    for range in ranges {
        let descriptor = member
            .descriptors
            .iter()
            .find(|descriptor| descriptor.canonical_order == range.source.order)
            .ok_or_else(|| {
                Error::RepositoryConflict("Capture is outside frozen membership".into())
            })?;
        if range.source.pointer.segment_id != descriptor.identity.segment_id
            || range.source.pointer.entry_ref != descriptor.identity.entry_ref
            || range.source.content_sha256 != descriptor.sha256
            || range.source.total_bytes != descriptor.total_bytes
            || range.bytes.end > descriptor.total_bytes
        {
            return Err(Error::RepositoryConflict(
                "Frozen capture source identity changed".into(),
            ));
        }
    }
    Ok(slice.capture)
}

fn verify_freeze(prepared: &PreparedWorkerRemoval, freeze: &WorkerRemovalFreeze) -> Result<()> {
    let request = &prepared.runtime_request;
    if freeze.operation_id != request.operation_id
        || freeze.input_fingerprint != request.input_fingerprint
        || freeze.workspace_id != request.workspace_id
        || freeze.source_runtime_id != request.source_runtime_id
        || freeze.worker_id != request.worker_id
        || freeze.members.is_empty()
    {
        return Err(Error::RepositoryConflict(
            "Runtime freeze does not match the canonical removal intent".into(),
        ));
    }
    verify_members(&freeze.members)
}

fn verify_members(members: &[WorkerRemovalSessionMember]) -> Result<()> {
    if members.is_empty() {
        return Err(Error::RepositoryConflict(
            "Recording scope is unavailable, not empty".into(),
        ));
    }
    let mut sessions = std::collections::HashSet::new();
    for member in members {
        if !sessions.insert(member.session_id)
            || member.end.byte_offset != 0
            || member.end.canonical_order != member.descriptors.len() as u64
            || (!member.materialized
                && (!member.descriptors.is_empty() || member.active_segment_id.is_some()))
        {
            return Err(Error::RepositoryConflict(
                "Runtime freeze has inconsistent Session scope".into(),
            ));
        }
        let mut identities = std::collections::HashSet::new();
        for (order, descriptor) in member.descriptors.iter().enumerate() {
            if descriptor.canonical_order != order as u64
                || !identities.insert((
                    descriptor.identity.segment_id,
                    descriptor.identity.entry_ref.clone(),
                ))
            {
                return Err(Error::RepositoryConflict(
                    "Frozen canonical originals are duplicated or discontinuous".into(),
                ));
            }
        }
    }
    Ok(())
}

pub(in crate::server) fn advance_worker_removal(
    service: &WorkerRemovalService,
    runtime: &RuntimeRegistry,
    target: &RuntimeWorkerRef,
    prepared: &PreparedWorkerRemoval,
) -> Result<Option<worker::WorkspaceResponse>> {
    let api = service.journal_dispatch_api.upgrade();
    if let Some(api) = &api
        && subjektiv_deletion::deleting_worker(api, target)?
    {
        return Ok(None);
    }
    let saved = service.store.get_journal_removal(prepared)?;
    if saved.is_none()
        && !service
            .store
            .retained_worker_singleton_lease(&service.workspace_id, target)?
            .is_some_and(|lease| lease.key.starts_with(SUBJEKTIV_SINGLETON_PREFIX))
    {
        // Job cleanup and other non-Subject Workers do not need a Journal owner.
        return Ok(None);
    }
    let api = api.ok_or_else(|| Error::Store("Workspace Journal owner unavailable".into()))?;
    let saved_database = if let Some(saved) = &saved {
        match api.feature_storage.open_existing(
            &api.subjektiv_registration,
            &saved.metadata.binding.database,
        ) {
            Ok(database) => Some(database),
            Err(error) => {
                tracing::warn!(operation_id=%prepared.plan.operation_id, %error, "Removal storage identity unavailable");
                return waiting(
                    service,
                    prepared,
                    saved,
                    saved.metadata.phase,
                    Cause::StorageUnavailable,
                );
            }
        }
    } else {
        None
    };
    let Some(attr) = removal_attribution(service, target)? else {
        if saved.is_some()
            || service
                .store
                .retained_worker_singleton_lease(&service.workspace_id, target)?
                .is_some_and(|lease| lease.key.starts_with(SUBJEKTIV_SINGLETON_PREFIX))
        {
            if let Some(saved) = &saved {
                return waiting(
                    service,
                    prepared,
                    saved,
                    saved.metadata.phase,
                    Cause::OwnershipUnknown,
                );
            }
            return Err(Error::RepositoryConflict(
                "Subject Session evidence is missing; retain removal intent".into(),
            ));
        }
        return Ok(None);
    };
    let database = match saved_database {
        Some(database) => database,
        None => existing_database(&api)?,
    };
    let store = crate::subjektiv::SubjektivStore::from_database(database.clone())
        .map_err(subjektiv_store_error)?;
    let host = JournalHostContext::validated_session(&store, attr.clone()).map_err(domain)?;
    if store.journal_session_erased(&host).map_err(domain)? {
        return Ok(None);
    }
    if saved.is_none()
        && prepared.prior_failure_category.as_deref() == Some("metadata_commit_failed")
        && prepared.runtime_request.session_disposition
            == worker_runtime::retention::SessionDisposition::Purge
        && prepared.runtime_request.archive_id.is_none()
        && store
            .journal_worker_was_released(&attr.subject_id, &attr.runtime_id, &attr.worker_id)
            .map_err(domain)?
        && !store.journal_retention_required(&host).map_err(domain)?
    {
        // Pre-upgrade ordinary Purge may already have consumed the originals.
        // Reconcile that issued operation/receipt, not a newly fabricated frozen
        // scope or Journal-completed observation. New Subject plans use Archive.
        let _storage = database
            .guard(database.identity())
            .map_err(Error::SubjektivStorage)?;
        require_body_recording_and_effects_ended(&api, &store, &attr)?;
        return Ok(None);
    }
    let mut progress = service.store.init_journal_removal(
        prepared,
        &JournalRemovalBinding {
            subject_id: attr.subject_id.clone(),
            writer_session_id: attr.session_id.clone(),
            writer_binding_id: host
                .writer_binding_id()
                .ok_or_else(|| Error::RepositoryConflict("Removal writer binding missing".into()))?
                .into(),
            database: database.identity().clone(),
        },
    )?;
    // Release is independently rechecked on every resume. The phase alone is not proof.
    if matches!(progress.metadata.phase, Phase::Released | Phase::Completed) {
        let _storage = database
            .guard(&progress.metadata.binding.database)
            .map_err(Error::SubjektivStorage)?;
        if !store
            .journal_worker_was_released(&attr.subject_id, &attr.runtime_id, &attr.worker_id)
            .map_err(domain)?
        {
            return Err(Error::RepositoryConflict(
                "Saved release observation has no current writer revocation".into(),
            ));
        }
        require_body_recording_and_effects_ended(&api, &store, &attr)?;
        let members = progress
            .metadata
            .members
            .as_ref()
            .ok_or_else(|| Error::Store("Released body has no frozen scope".into()))?
            .iter()
            .map(|member| {
                Ok(WorkerRemovalSessionMember {
                    session_id: member
                        .session_id
                        .parse()
                        .map_err(|_| Error::Store("Frozen Session identity invalid".into()))?,
                    active_segment_id: member.active_segment_id,
                    materialized: member.materialized,
                    end: member.end,
                    descriptors: member.descriptors.clone(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        require_scope_complete(&api, &store, target, &attr.subject_id, &members)?;
        drop(_storage);
        release_body_pins(&api, runtime, prepared, &store, &attr)?;
        return Ok(None);
    }
    let writer_released = store
        .journal_worker_was_released(&attr.subject_id, &attr.runtime_id, &attr.worker_id)
        .map_err(domain)?;
    if !writer_released {
        if !enabled(&api, target)? {
            let phase = progress.metadata.phase;
            return waiting(
                service,
                prepared,
                &progress,
                phase,
                Cause::RecordingDisabled,
            );
        }
        authorize_source(&api, &attr, "journal.recording.request")?;
    }
    let freeze = match runtime.freeze_worker_removal(target, prepared.runtime_request.clone()) {
        Ok(freeze) => freeze,
        Err(error) => {
            tracing::warn!(operation_id=%prepared.plan.operation_id, ?error, "Runtime quiescence/freeze unconfirmed");
            let phase = progress.metadata.phase;
            let cause = match &error {
                RuntimeRegistryError::RuntimeOperationFailed { code, .. }
                    if code == "source_unavailable" =>
                {
                    Cause::SourceUnavailable
                }
                RuntimeRegistryError::RuntimeOperationFailed { code, .. }
                    if code == "ownership_unknown" =>
                {
                    Cause::OwnershipUnknown
                }
                _ => Cause::ExecutionUnconfirmed,
            };
            return waiting(service, prepared, &progress, phase, cause);
        }
    };
    verify_freeze(prepared, &freeze)?;
    let members: Vec<_> = freeze
        .members
        .iter()
        .map(|member| FrozenJournalRemovalMember {
            session_id: member.session_id.to_string(),
            end: member.end,
            active_segment_id: member.active_segment_id,
            materialized: member.materialized,
            descriptors: member.descriptors.clone(),
        })
        .collect();
    progress = service
        .store
        .freeze_journal_removal(prepared, &progress, &members)?;
    // No absent/unmapped retained Session may disappear by filtering the Runtime set.
    let attrs = store
        .subject_session_attributions(&attr.subject_id)
        .map_err(subjektiv_store_error)?;
    let attributed: std::collections::HashSet<_> = attrs
        .iter()
        .filter(|session| {
            session.runtime_id == target.runtime_id && session.worker_id == target.worker_id
        })
        .map(|session| session.session_id.as_str())
        .collect();
    if attributed
        != members
            .iter()
            .map(|member| member.session_id.as_str())
            .collect()
    {
        return waiting(
            service,
            prepared,
            &progress,
            Phase::Recording,
            Cause::OwnershipUnknown,
        );
    }
    let mut complete = true;
    let mut incomplete_cause = Cause::JobPending;
    for member in &freeze.members {
        let session = member_attribution(&store, prepared, &attr.subject_id, member)?;
        let session_host =
            JournalHostContext::validated_session(&store, session.clone()).map_err(domain)?;
        if writer_released {
            // Explicit release already revoked this body. Recheck its complete
            // coverage and effects, but never request new execution authority.
            if require_member_complete(&api, &store, &session, member).is_err() {
                return waiting(
                    service,
                    prepared,
                    &progress,
                    Phase::Recording,
                    Cause::ExecutionUnconfirmed,
                );
            }
            continue;
        }
        authorize_source(&api, &session, "journal.recording.request")?;
        let mut id = store.journal_for_session(&session_host).map_err(domain)?;
        if let Some(journal_id) = &id {
            // T-841 owns an accepted Job's recovery; never reselect its missing prefix.
            if let Err(error) =
                dispatch_with_capture(&api, session.clone(), journal_id, false, |_, _, _| {
                    Err(Error::Store("Saved-only dispatch cannot capture".into()))
                })
            {
                tracing::debug!(operation_id=%prepared.plan.operation_id, %error, "Frozen Job awaits execution-end reconciliation");
            }
        }
        if require_member_complete(&api, &store, &session, member).is_ok() {
            continue;
        }
        complete = false;
        if id.is_none() && !member.descriptors.is_empty() {
            let capture = match capture_member(
                &api,
                prepared,
                member,
                0,
                0,
                JournalInput::capture_budget(),
            ) {
                Ok(capture) => capture,
                Err(error) => {
                    tracing::warn!(operation_id=%prepared.plan.operation_id, %error, "Frozen member capture unavailable");
                    return waiting(
                        service,
                        prepared,
                        &progress,
                        Phase::Recording,
                        Cause::SourceUnavailable,
                    );
                }
            };
            let ranges = journal::recording::capture_ranges(&capture).map_err(domain)?;
            if ranges.is_empty() {
                return waiting(
                    service,
                    prepared,
                    &progress,
                    Phase::Recording,
                    Cause::SourceUnavailable,
                );
            }
            authorize_source(&api, &session, "journal.recording.request")?;
            id = Some(
                store
                    .accept_journal_sources(&session_host, "Session activity", &ranges)
                    .map_err(domain)?,
            );
        }
        let mut after = None;
        loop {
            let jobs = api.store.list_backend_journal_jobs(
                api.workspace_id(),
                &session.session_id,
                after.as_deref(),
                100,
            )?;
            for job in &jobs {
                if job.state == BackendJobState::Failed && !can_resume_partial(job) {
                    incomplete_cause = Cause::JobFailed;
                } else if job.state == BackendJobState::Pending
                    && incomplete_cause != Cause::JobFailed
                {
                    let attempt = api.store.get_backend_job_attempt(
                        api.workspace_id(),
                        &job.request.job_id,
                        &crate::backend_job::attempt_id(&job.request.job_id, job.current_attempt),
                    )?;
                    if attempt.is_none_or(|attempt| {
                        attempt.worker.is_none() && attempt.allocation_precluded_at.is_none()
                    }) {
                        incomplete_cause = Cause::ExecutionUnconfirmed;
                    }
                }
            }
            if jobs.len() < 100 {
                break;
            }
            after = jobs.last().map(|job| job.request.job_id.clone());
        }
        if let Some(id) = id {
            let capture_failed = std::cell::Cell::new(false);
            if let Err(error) =
                dispatch_with_capture(&api, session.clone(), &id, true, |order, offset, budget| {
                    let result = capture_member(&api, prepared, member, order, offset, budget);
                    capture_failed.set(result.is_err());
                    result
                })
            {
                tracing::warn!(operation_id=%prepared.plan.operation_id, %error, "Frozen recording remains held");
                let cause = if capture_failed.get() {
                    Cause::SourceUnavailable
                } else if incomplete_cause == Cause::JobFailed {
                    Cause::JobFailed
                } else {
                    Cause::ExecutionUnconfirmed
                };
                return waiting(service, prepared, &progress, Phase::Recording, cause);
            }
        }
    }
    if !complete {
        return waiting(
            service,
            prepared,
            &progress,
            Phase::Recording,
            incomplete_cause,
        );
    }
    progress = service.store.update_journal_removal_progress(
        prepared,
        &progress,
        Phase::AwaitingEffectsEnd,
        None,
    )?;
    let release = (|| -> Result<()> {
        // Short final exclusion only: no Runtime I/O, Job wait, or transaction
        // crosses this guard. Reopen checked the existing physical DB, never CREATE.
        let gate = lifecycle_lock(&api, &attr.subject_id)?;
        let _gate = gate
            .lock()
            .map_err(|_| Error::Store("Journal lifecycle lock poisoned".into()))?;
        for member in &freeze.members {
            let session = member_attribution(&store, prepared, &attr.subject_id, member)?;
            require_member_complete(&api, &store, &session, member)?;
        }
        let binding = JournalWriterBinding {
            subject_id: attr.subject_id.clone(),
            runtime_id: attr.runtime_id.clone(),
            worker_id: attr.worker_id.clone(),
            binding_id: host
                .writer_binding_id()
                .ok_or_else(|| {
                    Error::RepositoryConflict("Frozen body writer binding missing".into())
                })?
                .into(),
        };
        release_writer_checked(
            &api,
            &database,
            &progress.metadata.binding.database,
            &store,
            &binding,
            || require_scope_complete(&api, &store, target, &attr.subject_id, &freeze.members),
        )?;
        Ok(())
    })();
    if let Err(error) = release {
        tracing::debug!(operation_id=%prepared.plan.operation_id, %error, "Writer release awaits exact cleanup return");
        return waiting(
            service,
            prepared,
            &progress,
            Phase::AwaitingEffectsEnd,
            Cause::ExecutionUnconfirmed,
        );
    }
    service
        .store
        .update_journal_removal_progress(prepared, &progress, Phase::Released, None)?;
    release_body_pins(&api, runtime, prepared, &store, &attr)?;
    Ok(None)
}

fn release_body_pins(
    api: &WorkspaceApi,
    runtime: &RuntimeRegistry,
    prepared: &PreparedWorkerRemoval,
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
        if let Err(original) = retain_source(
            runtime,
            api.workspace_id(),
            &session,
            runtime_api::SessionPublicSource::Retained {
                worker_id: prepared.runtime_request.worker_id,
            },
            true,
        ) {
            let archive_id = prepared
                .runtime_request
                .archive_id
                .clone()
                .ok_or(original)?;
            retain_source(
                runtime,
                api.workspace_id(),
                &session,
                runtime_api::SessionPublicSource::Archived { archive_id },
                true,
            )?;
        }
    }
    Ok(())
}

/// Startup and exact Job-cleanup completion both resume saved parent intentions.
/// No retry timer blindly resubmits failures; the same operation is reconciled.
pub(in crate::server) fn resume_removals(api: &WorkspaceApi) -> Result<()> {
    let handle =
        tokio::runtime::Handle::try_current().map_err(|error| Error::Store(error.to_string()))?;
    let mut after = None;
    loop {
        let page = api.store.list_pending_worker_removal_intents(
            api.workspace_id(),
            after.as_deref(),
            100,
        )?;
        for progress in &page {
            // Recovery runs from the cycle-free dispatch snapshot, whose own
            // journal_dispatch_owner is empty. Retain this invocation's owner
            // until the continuation returns rather than upgrading an absent weak link.
            let dispatch_owner = Arc::new(api.clone());
            let mut service = WorkerRemovalService::new(api);
            service.journal_dispatch_api = Arc::downgrade(&dispatch_owner);
            let runtime = api.runtime.clone();
            let target = progress.plan.worker.clone();
            let operation_id = progress.plan.operation_id.clone();
            let fingerprint = progress.plan.input_fingerprint.clone();
            let executor = service.host_executors.host()?;
            handle.spawn(async move {
                let _dispatch_owner = dispatch_owner;
                let correlation = operation_id.clone();
                let result = run_host_workflow(executor, async move {
                    let owner = Mutex::new(Some((operation_id, fingerprint)));
                    service.execute_target_removal_on_host_checked(
                        &runtime, &target, "Resume saved Journal removal", None, Some(&owner),
                    ).await
                }).await;
                if let Err(error) = result {
                    tracing::warn!(operation_id=%correlation, %error, "Journal removal continuation remains retained");
                }
            });
        }
        if page.len() < 100 {
            return Ok(());
        }
        after = page
            .last()
            .map(|progress| progress.plan.operation_id.clone());
    }
}
