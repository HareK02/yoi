//! Physical removal retains frozen input only until finite Host Job admission.
//! Legacy removal/release JSON is imported once; it never drains checkpoints,
//! follows child completion or manufactures subsequent recording work.

use super::*;
use crate::retention::PreparedWorkerRemoval;
use crate::store::{
    InputHandoffStatus, WorkerInputHandoffBinding, WorkerInputHandoffCause as Cause,
    WorkerInputHandoffMember, WorkerInputHandoffPhase as Phase, WorkerInputHandoffProgress,
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
    progress: &WorkerInputHandoffProgress,
    phase: Phase,
    cause: Cause,
) -> Result<Option<worker::WorkspaceResponse>> {
    let current = service.store.update_worker_input_handoff_progress(
        prepared,
        progress,
        phase,
        Some(cause),
    )?;
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
        content_type: None,
        body: serde_json::to_string(&response).map_err(|error| Error::Store(error.to_string()))?,
    }))
}

pub(in crate::server) fn removal_observation(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
) -> Result<Option<server_api::WorkerRemovalObservation>> {
    let Some(prepared) = api
        .config_store
        .recover_worker_removal_plan(api.workspace_id(), worker)
        .map_err(|error| Error::Store(error.to_string()))?
    else {
        return Ok(None);
    };
    let Some(progress) = api.store.get_worker_input_handoff_for_plan(&prepared)? else {
        return Ok(None);
    };
    if progress.metadata.phase == Phase::Completed {
        return Ok(None);
    }
    Ok(Some(progress_observation(&progress)))
}

fn progress_observation(
    progress: &WorkerInputHandoffProgress,
) -> server_api::WorkerRemovalObservation {
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
            Phase::Recording => server_api::WorkerRemovalPhase::InputHandoff,
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

pub(super) fn require_archive_scope(
    progress: &WorkerInputHandoffProgress,
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

pub(super) fn validate_capture(
    member: &WorkerRemovalSessionMember,
    slice: session_store::capture::RecordingCaptureSlice,
    budget: usize,
) -> Result<session_store::capture::CaptureDto> {
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

pub(super) fn verify_members(members: &[WorkerRemovalSessionMember]) -> Result<()> {
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

fn freeze_failure_cause(error: &RuntimeRegistryError) -> Cause {
    use protocol::control_failure::RuntimeControlCause as C;
    match error {
        RuntimeRegistryError::RuntimeControlFailed {
            cause: C::RecordingSourceUnavailable,
            ..
        } => Cause::SourceUnavailable,
        RuntimeRegistryError::RuntimeControlFailed {
            cause: C::RecordingOwnershipUnknown,
            ..
        } => Cause::OwnershipUnknown,
        RuntimeRegistryError::RuntimeOperationFailed { code, .. } => match code.as_str() {
            "source_unavailable" => Cause::SourceUnavailable,
            "ownership_unknown"
            | "worker_removal_target_mismatch"
            | "worker_removal_freeze_identity_mismatch" => Cause::OwnershipUnknown,
            _ => Cause::ExecutionUnconfirmed,
        },
        _ => Cause::ExecutionUnconfirmed,
    }
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
    let saved = service.store.get_worker_input_handoff(prepared)?;
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
    if saved.as_ref().is_some_and(|progress| {
        matches!(progress.metadata.phase, Phase::Released | Phase::Completed)
            && progress.metadata.released_input_handoff.is_some()
    }) {
        // The handoff is durable. Replaying physical removal must not freeze or
        // republish a producer already removed by this exact operation. The
        // retention receipt and catalog commit still validate the saved scope.
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
    let mut progress = service.store.init_worker_input_handoff(
        prepared,
        &WorkerInputHandoffBinding {
            subject_id: attr.subject_id.clone(),
            writer_session_id: attr.session_id.clone(),
            writer_binding_id: host
                .writer_binding_id()
                .ok_or_else(|| Error::RepositoryConflict("Removal writer binding missing".into()))?
                .into(),
            database: database.identity().clone(),
        },
    )?;
    let freeze = match runtime.freeze_worker_removal(target, prepared.runtime_request.clone()) {
        Ok(freeze) => freeze,
        Err(error) => {
            tracing::warn!(operation_id=%prepared.plan.operation_id, ?error, "Runtime quiescence/freeze unconfirmed");
            return waiting(
                service,
                prepared,
                &progress,
                progress.metadata.phase,
                freeze_failure_cause(&error),
            );
        }
    };
    verify_freeze(prepared, &freeze)?;
    let members: Vec<_> = freeze
        .members
        .iter()
        .map(|member| WorkerInputHandoffMember {
            session_id: member.session_id.to_string(),
            end: member.end,
            active_segment_id: member.active_segment_id,
            materialized: member.materialized,
            descriptors: member.descriptors.clone(),
        })
        .collect();
    progress = service
        .store
        .freeze_worker_input_handoff(prepared, &progress, &members)?;
    let attributed: std::collections::HashSet<_> = store
        .subject_session_attributions(&attr.subject_id)
        .map_err(subjektiv_store_error)?
        .into_iter()
        .filter(|session| {
            session.runtime_id == target.runtime_id && session.worker_id == target.worker_id
        })
        .map(|session| session.session_id)
        .collect();
    if attributed
        != members
            .iter()
            .map(|member| member.session_id.clone())
            .collect()
    {
        return waiting(
            service,
            prepared,
            &progress,
            progress.metadata.phase,
            Cause::OwnershipUnknown,
        );
    }
    for member in &freeze.members {
        if member.materialized {
            let session = member_attribution(&store, prepared, &attr.subject_id, member)?;
            if let Err(original) = retain_source(
                runtime,
                api.workspace_id(),
                &session,
                runtime_api::SessionPublicSource::Retained {
                    worker_id: prepared.runtime_request.worker_id,
                },
                false,
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
                    false,
                )?;
            }
        }
    }
    let archive = match runtime
        .publish_worker_removal_archive(target, prepared.runtime_request.clone())
    {
        Ok(archive) => archive,
        Err(error) => {
            tracing::warn!(operation_id=%prepared.plan.operation_id, ?error, "Frozen original archive publication unconfirmed");
            return waiting(
                service,
                prepared,
                &progress,
                progress.metadata.phase,
                Cause::SourceUnavailable,
            );
        }
    };
    require_archive_scope(&progress, &archive)?;
    progress =
        service
            .store
            .retain_released_input_handoff(prepared, &progress, &archive.archive_id)?;
    // The archive shares the Session pin namespace with the producer. Never
    // release those pins here: successful finite input admission or erasure owns them.
    let gate = lifecycle_lock(&api, &attr.subject_id)?;
    let _gate = gate
        .lock()
        .map_err(|_| Error::Store("Journal lifecycle lock poisoned".into()))?;
    let _storage = database
        .guard(&progress.metadata.binding.database)
        .map_err(Error::SubjektivStorage)?;
    for member in &freeze.members {
        let session = member_attribution(&store, prepared, &attr.subject_id, member)?;
        let host = JournalHostContext::validated_session(&store, session).map_err(domain)?;
        if store.journal_session_erased(&host).map_err(domain)? {
            return Err(Error::RepositoryConflict(
                "Session erased during recording handoff".into(),
            ));
        }
    }
    store
        .release_retained_journal_writer(&JournalWriterBinding {
            subject_id: attr.subject_id.clone(),
            runtime_id: attr.runtime_id.clone(),
            worker_id: attr.worker_id.clone(),
            binding_id: progress.metadata.binding.writer_binding_id.clone(),
        })
        .map_err(domain)?;
    if progress.metadata.phase != Phase::Released {
        service.store.update_worker_input_handoff_progress(
            prepared,
            &progress,
            Phase::Released,
            None,
        )?;
    }
    drop(_storage);
    drop(_gate);
    if let Err(error) = recover_input_handoffs(&api) {
        tracing::warn!(operation_id=%progress.operation_id, %error, "Retained recording continuation deferred");
    }
    Ok(None)
}

/// Import the *whole* frozen view, including already reflected legacy prefixes.
/// Starting at a checkpoint or accepted prefix loses legacy child input ownership
/// and makes crash replay depend on a moving cursor. A stable [0,end) snapshot
/// instead replays the same finite Job admission even after application advances.
pub(super) fn admit_retained_input(
    api: &WorkspaceApi,
    progress: &RetainedSourceProgress,
) -> Result<InputHandoffStatus> {
    let gate = lifecycle_lock(api, &progress.binding.subject_id)?;
    let _guard = gate
        .lock()
        .map_err(|_| Error::Store("Input handoff lifecycle lock poisoned".into()))?;
    // The main-DB fence survives domain destruction. Check it before opening the
    // old database, looking up attribution, capturing bytes or reserving a Job.
    if subjektiv_deletion::pending(api, &progress.binding.subject_id)? {
        return Ok(InputHandoffStatus::SubjectDeleted);
    }
    let database = api
        .feature_storage
        .open_existing(&api.subjektiv_registration, &progress.binding.database)
        .map_err(Error::SubjektivStorage)?;
    let _storage = database
        .guard(&progress.binding.database)
        .map_err(Error::SubjektivStorage)?;
    let store = crate::subjektiv::SubjektivStore::from_database(database.clone())
        .map_err(subjektiv_store_error)?;
    // Domain-first deletion may have fenced the Subject before main-DB inventory
    // was committed. Neither ordering permits recreation or a fresh admission.
    if store
        .subject_deletion(&progress.binding.subject_id)
        .map_err(subjektiv_store_error)?
        .is_some()
    {
        return Ok(InputHandoffStatus::SubjectDeleted);
    }
    if store
        .subject(&progress.binding.subject_id)
        .map_err(subjektiv_store_error)?
        .is_none()
    {
        return Err(Error::SubjektivSubjectNotFound(
            progress.binding.subject_id.clone(),
        ));
    }
    for frozen in &progress.members {
        let attr = store
            .session_attribution(&frozen.session_id)
            .map_err(subjektiv_store_error)?
            .filter(|a| {
                a.subject_id == progress.binding.subject_id
                    && a.runtime_id == progress.worker.runtime_id
                    && a.worker_id == progress.worker.worker_id
            })
            .ok_or_else(|| {
                Error::RepositoryConflict("Input handoff attribution mismatch".into())
            })?;
        let host = JournalHostContext::validated_session(&store, attr.clone()).map_err(domain)?;
        if store.journal_session_erased(&host).map_err(domain)? {
            continue; // The canonical Session erasure path, not a checkpoint.
        }
        let binding = host
            .writer_binding_id()
            .filter(|binding| *binding == progress.binding.writer_binding_id)
            .ok_or_else(|| {
                Error::WorkspacePermissionDenied("Input handoff lifetime changed".into())
            })?;
        let member = WorkerRemovalSessionMember {
            session_id: frozen
                .session_id
                .parse()
                .map_err(|_| Error::Store("Invalid frozen Session identity".into()))?,
            active_segment_id: frozen.active_segment_id,
            materialized: frozen.materialized,
            end: frozen.end,
            descriptors: frozen.descriptors.clone(),
        };
        verify_members(std::slice::from_ref(&member))?;
        let start = session_store::capture::RecordingPosition::default();
        let (end, chunks) = backend_recording_job::capture_scope(
            start,
            Some(frozen.end),
            |order, offset, budget| progress.capture(api, &member, order, offset, budget),
        )?;
        // Empty/unmaterialized Sessions also get a finite owning Job. Legacy
        // completed=true never substitutes for a common Job admission receipt.
        backend_recording_job::admit(
            api,
            crate::store::BackendJobRecordingScope {
                subject_id: attr.subject_id.clone(),
                session_id: attr.session_id.clone(),
                worker: progress.worker.clone(),
                binding_id: binding.into(),
                database: progress.binding.database.clone(),
                start,
                end,
            },
            &chunks,
        )?;
    }
    // Do not unpin here: the caller first commits the terminal handoff receipt.
    // Admission failure (including a partial multi-Session import) retains every
    // original so recovery can replay the same fixed Jobs without skipping bytes.
    Ok(InputHandoffStatus::Admitted)
}

/// Best-effort pin cleanup *after* the terminal admission receipt. Cleanup faults
/// must not re-open legacy recovery or wait for recording/checkpoint completion.
pub(super) fn release_original_pins(
    api: &WorkspaceApi,
    progress: &RetainedSourceProgress,
) -> Result<()> {
    let gate = lifecycle_lock(api, &progress.binding.subject_id)?;
    let _guard = gate
        .lock()
        .map_err(|_| Error::Store("Input handoff lifecycle lock poisoned".into()))?;
    if subjektiv_deletion::pending(api, &progress.binding.subject_id)? {
        return Ok(());
    }
    let database = api
        .feature_storage
        .open_existing(&api.subjektiv_registration, &progress.binding.database)
        .map_err(Error::SubjektivStorage)?;
    let store =
        crate::subjektiv::SubjektivStore::from_database(database).map_err(subjektiv_store_error)?;
    if store
        .subject_deletion(&progress.binding.subject_id)
        .map_err(subjektiv_store_error)?
        .is_some()
    {
        return Ok(());
    }
    for frozen in &progress.members {
        if !frozen.materialized {
            continue;
        }
        let attr = store
            .session_attribution(&frozen.session_id)
            .map_err(subjektiv_store_error)?
            .filter(|a| {
                a.subject_id == progress.binding.subject_id
                    && a.runtime_id == progress.worker.runtime_id
                    && a.worker_id == progress.worker.worker_id
            })
            .ok_or_else(|| {
                Error::RepositoryConflict("Input handoff cleanup attribution mismatch".into())
            })?;
        retain_source(
            api.runtime.as_ref(),
            api.workspace_id(),
            &attr,
            progress.material_source()?,
            true,
        )?;
    }
    Ok(())
}

/// Import only unfinished input handoffs, including legacy completed recordings.
/// This never reopens physical removal, restores a producer or waits for a Job.
pub(super) fn recover_input_handoffs(api: &WorkspaceApi) -> Result<()> {
    let mut after = None;
    loop {
        let page = api.store.list_pending_released_input_handoffs(
            api.workspace_id(),
            after.as_deref(),
            100,
        )?;
        for progress in &page {
            let outcome = subjektiv_deletion::pending(api, &progress.metadata.binding.subject_id)
                .and_then(|deleted| {
                    if deleted {
                        Ok(InputHandoffStatus::SubjectDeleted)
                    } else {
                        RetainedSourceProgress::removal(progress)
                            .and_then(|source| admit_retained_input(api, &source))
                    }
                });
            let (status, cause) = match outcome {
                Ok(status) => (status, None),
                Err(error) => {
                    let cause = match &error {
                        Error::WorkspacePermissionDenied(_) => Cause::OwnershipUnknown,
                        Error::SubjektivStorage(_) => Cause::StorageUnavailable,
                        _ => Cause::SourceUnavailable,
                    };
                    tracing::warn!(operation_id=%progress.operation_id, %error, "Retained Journal recording awaits recovery");
                    (InputHandoffStatus::Pending, Some(cause))
                }
            };
            if let Err(error) = api
                .store
                .update_released_input_handoff(progress, status, cause)
            {
                tracing::debug!(operation_id=%progress.operation_id, %error, "Input handoff changed during import");
            } else if status == InputHandoffStatus::Admitted {
                if let Err(error) = RetainedSourceProgress::removal(progress)
                    .and_then(|source| release_original_pins(api, &source))
                {
                    tracing::warn!(operation_id=%progress.operation_id, %error, "Admitted input pin cleanup deferred; Jobs already own all bytes");
                }
            }
        }
        if page.len() < 100 {
            return retained::recover_release_input_handoffs(api);
        }
        after = page.last().map(|p| p.operation_id.clone());
    }
}

/// Startup and exact Job-cleanup completion both resume saved parent intentions.
/// No retry timer blindly resubmits failures; the same operation is reconciled.
pub(in crate::server) fn resume_removals(api: &WorkspaceApi) -> Result<()> {
    recover_input_handoffs(api)?;
    let handle =
        tokio::runtime::Handle::try_current().map_err(|error| Error::Store(error.to_string()))?;
    let mut after = None;
    loop {
        let page = api.store.list_pending_worker_removal_intents(
            api.workspace_id(),
            after.as_deref(),
            100,
        )?;
        for intent in &page {
            let progress = match &intent.preparation {
                Ok(prepared) => prepared,
                Err(error) => {
                    tracing::warn!(operation_id=%intent.operation_id,
                        runtime_id=%intent.worker.runtime_id, worker_id=%intent.worker.worker_id,
                        %error, "Journal removal intent remains held; continuing independent intents");
                    continue;
                }
            };
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
        after = page.last().map(|intent| intent.operation_id.clone());
    }
}
