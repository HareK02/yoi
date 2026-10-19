//! Journal admission and existing-Job orchestration. Domain receipts remain in Subjektiv;
//! the Backend owns authenticated source identity and live attempt capabilities.
use super::*;
use crate::backend_job::{
    BackendJobGrants, BackendJobRecord, SubjektivJournalGrant, journal_serialization_key,
};
use subjektiv::journal::recording::{JournalMaterial, select_materials_bounded};
use subjektiv::journal::transport::*;
use subjektiv::journal::{self, *};

pub(super) fn domain(error: JournalError) -> Error {
    match error {
        JournalError::PermissionDenied(s) => Error::WorkspacePermissionDenied(s),
        JournalError::Conflict(s) => Error::RepositoryConflict(s),
        other => Error::Store(other.to_string()),
    }
}

fn store(api: &WorkspaceApi) -> Result<crate::subjektiv::SubjektivStore> {
    crate::subjektiv::SubjektivStore::open(&api.feature_storage, &api.subjektiv_registration)
        .map_err(|e| Error::Store(e.to_string()))
}

pub(super) fn host(
    api: &WorkspaceApi,
    store: &crate::subjektiv::SubjektivStore,
    grant: &SubjektivJournalGrant,
) -> Result<JournalHostContext> {
    let attribution = store
        .session_attribution(&grant.session_id)
        .map_err(|e| Error::Store(e.to_string()))?
        .filter(|a| a.subject_id == grant.subject_id)
        .ok_or_else(|| {
            Error::WorkspacePermissionDenied("Journal Session attribution mismatch".into())
        })?;
    let mut host =
        JournalHostContext::validated_session(store, attribution.clone()).map_err(domain)?;
    if grant.binding_id.is_empty() {
        // Legacy wire/digest remains unchanged. Recovery is anchored exclusively to the
        // immutable original Session's Worker and only while that body is still current.
        let worker = RuntimeWorkerRef::new(&attribution.runtime_id, &attribution.worker_id);
        let lease = api
            .store
            .require_current_worker_singleton_owner(api.workspace_id(), &worker)?
            .ok_or_else(|| {
                Error::WorkspacePermissionDenied(
                    "legacy Journal original body is no longer current".into(),
                )
            })?;
        if lease.key
            != crate::subjektiv::subject_worker_singleton_key(&attribution.subject_id)
                .map_err(|e| Error::InvalidInput(e.to_string()))?
        {
            return Err(Error::WorkspacePermissionDenied(
                "legacy Journal original Subject mismatch".into(),
            ));
        }
        if host.writer_binding_id().is_none() {
            bind_host_session(api, attribution.clone())?;
            host = JournalHostContext::validated_session(store, attribution.clone())
                .map_err(domain)?;
        }
        let binding = JournalWriterBinding {
            subject_id: attribution.subject_id,
            runtime_id: attribution.runtime_id,
            worker_id: attribution.worker_id,
            binding_id: host
                .writer_binding_id()
                .ok_or_else(|| {
                    Error::WorkspacePermissionDenied("legacy original Session writer absent".into())
                })?
                .into(),
        };
        if !store.journal_writer_active(&binding).map_err(domain)? {
            return Err(Error::WorkspacePermissionDenied(
                "legacy original writer was released".into(),
            ));
        }
    } else if host.writer_binding_id() != Some(grant.binding_id.as_str()) {
        return Err(Error::WorkspacePermissionDenied(
            "Journal Job writer generation does not match its frozen grant".into(),
        ));
    }
    Ok(host)
}

/// Preparing before dispatch is also used by Reserved-attempt restart recovery. A retry
/// with the same attempt and complete selection converges on the original snapshot.
pub(super) fn prepare(api: &WorkspaceApi, reservation: &BackendJobReservation) -> Result<()> {
    let Some(grant) = &reservation.job.request.grants.subjektiv_journal else {
        return Ok(());
    };
    let store = store(api)?;
    let host = host(api, &store, grant)?;
    store
        .prepare_journal_generation(
            &host,
            &grant.journal_id,
            subjektiv::api::JobAttemptBinding {
                job_id: reservation.job.request.job_id.clone(),
                attempt_id: reservation.attempt.attempt_id.clone(),
            },
            grant.selected_ranges.clone(),
        )
        .map_err(domain)?;
    Ok(())
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

pub(super) fn execute(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
    request: JournalJobRequest,
) -> Result<JournalJobResponse> {
    let (grant, job, attempt) = live(api, worker)?;
    let store = store(api)?;
    let host = host(api, &store, &grant)?;
    let g = generation(&store, &host, &grant, &job, &attempt)?;
    let ctx = JournalJobContext::validated(&store, &host, &g.id, g.job.clone()).map_err(domain)?;
    journal::service::execute_bounded(&store, &ctx, request, |source, start, limit| {
        read_source(api, &grant, source, start, limit)
            .map_err(|e| JournalError::Conflict(e.to_string()))
    })
    .map_err(domain)
}

pub(super) fn finish(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
) -> Result<JournalJobCompletion> {
    let (grant, job, attempt) = live(api, worker)?;
    let store = store(api)?;
    let host = host(api, &store, &grant)?;
    // prepare is an exact binding lookup too, including the saved completion of a closed generation.
    let g = store
        .prepare_journal_generation(
            &host,
            &grant.journal_id,
            subjektiv::api::JobAttemptBinding {
                job_id: job.request.job_id,
                attempt_id: attempt.attempt_id,
            },
            grant.selected_ranges.clone(),
        )
        .map_err(domain)?;
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
    let store = store(api)?;
    let host = host(api, &store, grant)?;
    if let Some(g) = store
        .active_journal_generation(&host, &grant.journal_id)
        .map_err(domain)?
    {
        if g.job.job_id == request.job_id && g.job.attempt_id == attempt.attempt_id {
            store
                .close_journal_generation(&host, &g.id, &g.job)
                .map_err(domain)?;
        }
    }
    Ok(())
}

fn read_source(
    api: &WorkspaceApi,
    grant: &SubjektivJournalGrant,
    source: &JournalSource,
    start: u64,
    limit: usize,
) -> Result<JournalMaterial> {
    let store = store(api)?;
    let attr = store
        .session_attribution(&grant.session_id)
        .map_err(|e| Error::Store(e.to_string()))?
        .ok_or_else(|| Error::WorkspacePermissionDenied("Session attribution absent".into()))?;
    read_canonical(&material_reader(api, &attr)?, source, start, limit)
}

pub(super) fn bind_host_session(
    api: &WorkspaceApi,
    attr: crate::subjektiv::SubjectSessionAttribution,
) -> Result<()> {
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
        return Err(Error::RepositoryConflict(
            "Subject release intent fences Controller publication".into(),
        ));
    }
    let store = store(api)?;
    if store
        .journal_worker_was_released(&attr.subject_id, &attr.runtime_id, &attr.worker_id)
        .map_err(domain)?
    {
        return Err(Error::WorkspacePermissionDenied(
            "released body cannot reacquire Journal authority".into(),
        ));
    }
    // A released Backend body is never rebound by a delayed create/restore callback. New
    // Backend bodies have new Worker IDs; their first callback has no historical generation.
    for prior in store
        .subject_session_attributions(&attr.subject_id)
        .map_err(|e| Error::Store(e.to_string()))?
        .into_iter()
        .filter(|a| a.runtime_id == attr.runtime_id && a.worker_id == attr.worker_id)
    {
        let host = JournalHostContext::validated_session(&store, prior).map_err(domain)?;
        if let Some(id) = host.writer_binding_id() {
            let binding = JournalWriterBinding {
                subject_id: attr.subject_id.clone(),
                runtime_id: attr.runtime_id.clone(),
                worker_id: attr.worker_id.clone(),
                binding_id: id.into(),
            };
            if !store.journal_writer_active(&binding).map_err(domain)? {
                return Err(Error::WorkspacePermissionDenied(
                    "released body Session cannot reacquire Journal authority".into(),
                ));
            }
        }
    }
    // Publication (including attribution) shares the recovery gate. A callback
    // authenticated before a recovery intent must not leave a late attribution.
    let attr = store
        .record_session_attribution(attr)
        .map_err(|error| Error::InvalidInput(error.to_string()))?;
    let writer = store
        .claim_journal_writer(&attr.subject_id, &attr.runtime_id, &attr.worker_id)
        .map_err(domain)?;
    store.bind_journal_session(&writer, attr).map_err(domain)?;
    Ok(())
}

/// Runs under the same Subject gate as final revoke/singleton release. A failed create or
/// legacy removal may have released its Backend reservation but cannot hide a durable writer.
pub(super) fn check_start_allowed(api: &WorkspaceApi, subject: &str) -> Result<()> {
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

/// Explicit creation can reconcile an earlier failed, never-published body.
/// No missing catalog/status observation is treated as a Runtime fencing receipt.
pub(super) fn recover_failed_subject_create(api: &WorkspaceApi, subject: &str) -> Result<()> {
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
        let Some(lease) = api
            .config_store
            .current_worker_singleton_owner(api.workspace_id(), &subject_key)?
        else {
            return Ok(());
        };
        if api
            .config_store
            .subject_create_recovery_candidate(api.workspace_id(), &lease)?
            .is_none()
        {
            return Ok(());
        }
        if !store
            .journal_body_is_unpublished(subject, &lease.worker.runtime_id, &lease.worker.worker_id)
            .map_err(domain)?
        {
            // Published history must go through the ordinary continuity release.
            return Ok(());
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

fn dispatch(
    api: &WorkspaceApi,
    attribution: crate::subjektiv::SubjectSessionAttribution,
    journal_id: &str,
) -> Result<()> {
    let gate = lifecycle_lock(api, &attribution.subject_id)?;
    let _gate = gate
        .lock()
        .map_err(|_| Error::Store("Journal lifecycle lock poisoned".into()))?;
    let store = store(api)?;
    let host =
        JournalHostContext::validated_session(&store, attribution.clone()).map_err(domain)?;
    if let Some(g) = store
        .active_journal_generation(&host, journal_id)
        .map_err(domain)?
    {
        let job = api
            .store
            .get_backend_job(api.workspace_id(), &g.job.job_id)?
            .ok_or_else(|| Error::Store("active Journal generation has no reserved Job".into()))?;
        let attempt = api
            .store
            .get_backend_job_attempt(api.workspace_id(), &g.job.job_id, &g.job.attempt_id)?
            .ok_or_else(|| {
                Error::Store("active Journal generation has no reserved attempt".into())
            })?;
        if job.state == BackendJobState::Pending || attempt.state == BackendJobAttemptState::Unknown
        {
            return Ok(());
        }
        // Definitive terminal outcomes retain saved receipts; they do not mark any more input read.
        store
            .close_journal_generation(&host, &g.id, &g.job)
            .map_err(domain)?;
    }
    let reader = material_reader(api, &attribution)?;
    let materials = select_materials_bounded(
        &store,
        &host,
        journal_id,
        96 * 1024,
        |source, start, limit| {
            read_canonical(&reader, source, start, limit)
                .map_err(|e| JournalError::Conflict(e.to_string()))
        },
    )
    .map_err(domain)?;
    if materials.is_empty() {
        return Ok(());
    }
    let request = recording_request(&host, attribution, journal_id, materials)?;
    // The intent is saved before generation binding or dispatch. If binding/dispatch fails,
    // Reserved recovery repeats this exact intent, never creates a replacement blindly.
    api.resolve_backend_job_profile(&request)?;
    let reservation = api.store.reserve_backend_job(
        api.workspace_id(),
        &request,
        &Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true),
    )?;
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
    let store = store(api)?;
    let host =
        JournalHostContext::validated_session(&store, attribution.clone()).map_err(domain)?;
    if store.journal_session_erased(&host).map_err(domain)? {
        return Err(Error::WorkspacePermissionDenied(
            "Session was explicitly erased".into(),
        ));
    }
    let mut id = store.journal_for_session(&host).map_err(domain)?;
    let reader = material_reader(api, &attribution)?;
    if reader.not_materialized {
        if store.journal_retention_required(&host).map_err(domain)? {
            return Err(Error::RepositoryConflict(
                "Admitted Journal original Session is not materialized".into(),
            ));
        }
        retain_source(
            api.runtime.as_ref(),
            api.workspace_id(),
            &attribution,
            material_source(api, &attribution)?,
            true,
        )?;
        return Ok(JournalRecordingResponse::NoNewSources);
    }
    retain_source(
        api.runtime.as_ref(),
        api.workspace_id(),
        &attribution,
        material_source(api, &attribution)?,
        false,
    )?;
    let sources: Vec<_> = reader
        .descriptors()
        .map(|d| source_from_descriptor(&attribution.session_id, d))
        .collect();
    for batch in sources.chunks(MAX_JOURNAL_RANGES) {
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
    let worker = verified_worker(source)?;
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
            let store = store(&api)?;
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
    let worker = verified_worker(source)?;
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
            let store = store(&api)?;
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
    let store = store(api)?;
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
    match reserved {
        Ok(reserved)
            if reserved.state == runtime_api::SubjectReleaseState::Reserved
                && reserved.operation_id == input.operation_id
                && reserved.binding_id == input.binding_id => {}
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
    }
    // Controller reservation precedes the Host gate: restore can be synchronously waiting
    // for a Session-attribution callback that itself takes this gate.
    let guard = gate
        .lock()
        .map_err(|_| Error::Store("Journal lifecycle lock poisoned".into()))?;
    let mut sessions = Vec::new();
    let checked = (|| {
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
                // Legacy saved-before-admission Sessions are not optional. Prove canonical
                // source identity first, then bind only this original Host-attributed Session.
                let _canonical = material_reader(api, &attr)?;
                let host =
                    JournalHostContext::validated_session(&store, attr.clone()).map_err(domain)?;
                if host
                    .writer_binding_id()
                    .is_some_and(|id| id != binding.binding_id)
                {
                    return Err(Error::RepositoryConflict(
                        "legacy Session belongs to another writer".into(),
                    ));
                }
                store
                    .bind_journal_session(&binding, attr.clone())
                    .map_err(domain)?;
            }
            sessions.push(attr);
        }
        if sessions.is_empty() {
            return Err(Error::WorkspacePermissionDenied(
                "writer has no attributed original Sessions".into(),
            ));
        }
        for attr in &sessions {
            // Always read canonical saved history, even if the post-save notification was lost.
            admit_sources(api, attr.clone(), false)?;
            let mut after: Option<String> = None;
            loop {
                let jobs = api.store.list_backend_journal_jobs(
                    api.workspace_id(),
                    &attr.session_id,
                    after.as_deref(),
                    100,
                )?;
                for job in &jobs {
                    if matches!(
                        job.state,
                        BackendJobState::Pending | BackendJobState::Unknown
                    ) {
                        return Err(Error::RepositoryConflict(
                            "Journal Job remains pending, running or Unknown".into(),
                        ));
                    }
                    let attempt = api
                        .store
                        .get_backend_job_attempt(
                            api.workspace_id(),
                            &job.request.job_id,
                            &crate::backend_job::attempt_id(
                                &job.request.job_id,
                                job.current_attempt,
                            ),
                        )?
                        .ok_or_else(|| {
                            Error::Store("Journal attempt evidence is missing".into())
                        })?;
                    if !attempt.state.terminal() {
                        return Err(Error::RepositoryConflict(
                            "Journal attempt result remains unconfirmed".into(),
                        ));
                    }
                }
                if jobs.len() < 100 {
                    break;
                }
                after = jobs.last().map(|j| j.request.job_id.clone());
            }
        }
        // Also proves saved position/checkpoint coverage, not just Job terminality.
        store.release_journal_writer(&binding).map_err(domain)
    })();
    if let Err(error) = checked {
        drop(guard);
        cancel_active_release(api, &store, &gate, &lease, &binding, &request)?;
        // Confirmed cancellation unfreezes the still-current body. Dispatch retained work
        // only after releasing our Subject gate. A new explicit release needs a new op ID.
        for attr in sessions {
            let _ = admit(api, attr);
        }
        return Err(error);
    }
    // Do not call Runtime while holding the Host gate after revocation: a restarted Runtime
    // could hold its Worker lock while waiting for a callback that must reject this old token.
    drop(guard);
    // Unknown Runtime completion retains occupancy. Persist exact Controller confirmation before
    // the final singleton SQL so a later restart needs no status inference or Controller restore.
    confirm_runtime_disconnect(api, &lease, request)?;
    let _guard = gate
        .lock()
        .map_err(|_| Error::Store("Journal lifecycle lock poisoned".into()))?;
    api.config_store.release_subject_singleton(
        api.workspace_id(),
        &lease,
        &input.binding_id,
        &input.operation_id,
    )?;
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
    let worker = verified_worker(source)?;
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
            require_enabled(&api, &worker)?;
            let store = store(&api)?;
            let attributions = store
                .subject_session_attributions(subject)
                .map_err(|e| Error::Store(e.to_string()))?;
            let mut response = JournalRecordingResponse::NoNewSources;
            for attr in attributions
                .into_iter()
                .filter(|a| a.runtime_id == worker.runtime_id && a.worker_id == worker.worker_id)
            {
                response = admit(&api, attr)?;
            }
            Ok(Json(response))
        })
        .await
}
fn verified_worker(
    source: Option<axum::Extension<crate::worker_source::VerifiedRuntimeRequestSource>>,
) -> ApiResult<RuntimeWorkerRef> {
    let source = source
        .ok_or_else(|| {
            Error::WorkspacePermissionDenied(
                "Journal requires signed Runtime request identity".into(),
            )
        })?
        .0;
    let worker = source.worker_id.ok_or_else(|| {
        Error::WorkspacePermissionDenied("Journal requires main Worker identity".into())
    })?;
    Ok(RuntimeWorkerRef::new(source.runtime_id, worker))
}

pub(super) async fn job_route(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    source: Option<axum::Extension<crate::worker_source::VerifiedRuntimeRequestSource>>,
    Json(request): Json<JournalJobRequest>,
) -> ApiResult<Json<JournalJobResponse>> {
    let worker = verified_worker(source)?;
    api.store
        .db_executor()
        .run(move || {
            validate_workspace_scope(&api, &path.workspace_id)?;
            execute(&api, &worker, request)
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
            dispatch(api, attr.clone(), &grant.journal_id)?;
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
fn material_reader<'a>(
    api: &'a WorkspaceApi,
    attribution: &crate::subjektiv::SubjectSessionAttribution,
) -> Result<MaterialReader<'a>> {
    MaterialReader::open(
        api.runtime.as_ref(),
        api.workspace_id(),
        &attribution.runtime_id,
        material_source(api, attribution)?,
        &attribution.session_id,
    )
}
fn source_from_descriptor(
    session: &str,
    d: &session_store::public_material::PublicMaterialDescriptor,
) -> JournalSourceRange {
    JournalSourceRange {
        source: JournalSource {
            session_id: session.into(),
            pointer: JournalSourcePointer {
                segment_id: d.identity.segment_id,
                entry_ref: d.identity.entry_ref.clone(),
            },
            order: d.canonical_order,
            content_sha256: d.sha256.clone(),
            total_bytes: d.total_bytes,
        },
        bytes: JournalByteRange {
            start: 0,
            end: d.total_bytes,
        },
    }
}
fn read_canonical(
    reader: &MaterialReader<'_>,
    source: &JournalSource,
    start: u64,
    limit: usize,
) -> Result<JournalMaterial> {
    if reader.session_id != source.session_id {
        return Err(Error::WorkspacePermissionDenied(
            "Journal source Session mismatch".into(),
        ));
    }
    let descriptor = reader
        .descriptors()
        .find(|d| source_from_descriptor(&source.session_id, d).source == *source)
        .ok_or_else(|| {
            Error::Store("Journal original public source changed or unavailable".into())
        })?;
    // The reader streams the complete canonical digest while returning only this bounded window.
    let page = reader
        .read_content(descriptor, start, limit)
        .map_err(|e| Error::Store(e.to_string()))?;
    if page.sha256 != source.content_sha256
        || page.total_bytes != source.total_bytes
        || page.next_offset != start.saturating_add(page.content.len() as u64)
    {
        return Err(Error::RepositoryConflict(
            "Journal canonical source identity changed".into(),
        ));
    }
    Ok(JournalMaterial {
        range: JournalSourceRange {
            source: source.clone(),
            bytes: JournalByteRange {
                start,
                end: page.next_offset,
            },
        },
        text: page.content,
    })
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
                    let source = RuntimeWorkerRef::new(&attr.runtime_id, &attr.worker_id);
                    if require_enabled(api, &source).is_err() {
                        // Disablement or removal does not erase previously admitted work, but
                        // also does not authorize collecting a new disabled Worker's history.
                        let host = JournalHostContext::validated_session(&store, attr.clone())
                            .map_err(domain)?;
                        if let Ok(Some(id)) = store.journal_for_session(&host) {
                            if let Err(error) = dispatch(api, attr.clone(), &id)
                                .and_then(|_| release_completed_source(api, attr))
                            {
                                tracing::warn!(%error,session_id=%attr.session_id,"retained Journal prefix recovery blocked");
                            }
                        }
                        continue;
                    }
                    if let Err(error) = admit(api, attr.clone()) {
                        tracing::warn!(%error,session_id=%attr.session_id,"Journal admission recovery blocked; original input not marked processed");
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
    host: &JournalHostContext,
    attribution: crate::subjektiv::SubjectSessionAttribution,
    journal_id: &str,
    materials: Vec<journal::recording::JournalMaterial>,
) -> Result<BackendJobRequest> {
    let binding_id = host
        .writer_binding_id()
        .ok_or_else(|| {
            Error::WorkspacePermissionDenied("Journal Session has no Host writer binding".into())
        })?
        .to_string();
    let grant = SubjektivJournalGrant {
        binding_id,
        subject_id: attribution.subject_id,
        session_id: attribution.session_id,
        journal_id: journal_id.into(),
        selected_ranges: materials.iter().map(|m| m.range.clone()).collect(),
    };
    Ok(BackendJobRequest {
        job_id:format!("journal-recording:{}",Uuid::new_v4()),purpose:"subjektiv_journal".into(),
        input_ref:format!("subjektiv://{}/journal/{journal_id}",grant.subject_id),
        input:serde_json::json!({"journal_id":journal_id,"selected_ranges":grant.selected_ranges}),
        instruction:"Record ONLY this frozen original public selected_ranges prefix. The input contains descriptors, NOT source bodies. First ReadJournalState and use bounded ReadJournalText Source until every byte of every selected range is returned; record operations are refused until then. ReadJournalText also exposes initial/current position/open text. Sequentially save CreateJournalEntry/RewriteJournalEntry/FixJournalEntry, update current position and CheckpointJournal using retained record receipts. Never infer event completion from turn/task/job/compaction boundaries. Preserve exact operation ID/full input after unknown outcomes. SubmitBackendJobResult only after the full selected prefix has a saved checkpoint; no aggregate JournalRecordResult.".into(),
        profile:"builtin:subjektiv-journal-recording".into(),
        grants:BackendJobGrants {subjektiv_journal:Some(grant),..Default::default()},
        serialization_key:Some(journal_serialization_key(journal_id)),source_worker:None,notification_target:None,
        limits:crate::backend_job::BackendJobLimits {timeout_seconds:600,..Default::default()},
    })
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
    let store = crate::subjektiv::SubjektivStore::open(
        &service.journal_storage,
        &service.journal_registration,
    )
    .map_err(|e| Error::Store(e.to_string()))?;
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

/// Normal deletion cannot substitute for Subject release, even for a stopped or disabled body.
/// Explicit erasure remains a separate monotonic source/diagnostics destruction path.
pub(super) fn prepare_worker_removal(
    service: &WorkerRemovalService,
    runtime: &RuntimeRegistry,
    target: &RuntimeWorkerRef,
) -> Result<Option<crate::retention::ErasedJournalSession>> {
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
    let store = crate::subjektiv::SubjektivStore::open(
        &service.journal_storage,
        &service.journal_registration,
    )
    .map_err(|e| Error::Store(e.to_string()))?;
    let host = JournalHostContext::validated_session(&store, attr.clone()).map_err(domain)?;
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
    let Some(binding_id) = host.writer_binding_id() else {
        return Err(Error::RepositoryConflict(
            "Subject writer evidence is missing; ordinary deletion cannot bypass release".into(),
        ));
    };
    let writer = JournalWriterBinding {
        subject_id: attr.subject_id.clone(),
        runtime_id: attr.runtime_id.clone(),
        worker_id: attr.worker_id.clone(),
        binding_id: binding_id.into(),
    };
    if store.journal_writer_active(&writer).map_err(domain)? {
        return Err(Error::RepositoryConflict("Subject remains occupied: restore to actual Idle, finish Journal recording, then explicitly release before ordinary deletion".into()));
    }
    if store.journal_retention_required(&host).map_err(domain)? {
        return Err(Error::RepositoryConflict(
            "Released Subject has unresolved Journal material; retain occupancy and source".into(),
        ));
    }
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
    let Some(attr) = removal_attribution(service, target)? else {
        return Ok(None);
    };
    let store = crate::subjektiv::SubjektivStore::open(
        &service.journal_storage,
        &service.journal_registration,
    )
    .map_err(|e| Error::Store(e.to_string()))?;
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
    let store = crate::subjektiv::SubjektivStore::open(
        &service.journal_storage,
        &service.journal_registration,
    )
    .map_err(|e| Error::Store(e.to_string()))?;
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
    let binding_id = host.writer_binding_id().ok_or_else(|| {
        Error::RepositoryConflict("Subject writer evidence missing during removal recovery".into())
    })?;
    let binding = JournalWriterBinding {
        subject_id: attr.subject_id.clone(),
        runtime_id: attr.runtime_id.clone(),
        worker_id: attr.worker_id.clone(),
        binding_id: binding_id.into(),
    };
    if store.journal_writer_active(&binding).map_err(domain)? {
        return Err(Error::RepositoryConflict(
            "Prepared removal cannot bypass explicit Subject release".into(),
        ));
    }
    let source = runtime_api::SessionPublicSource::Retained {
        worker_id: worker_runtime::identity::WorkerId::parse(&target.worker_id)
            .ok_or_else(|| Error::Store("Worker identity invalid".into()))?,
    };
    let runtime = service.runtime.upgrade().ok_or_else(|| {
        Error::Store("Runtime unavailable for source retention confirmation".into())
    })?;
    if store.journal_retention_required(&host).map_err(domain)? {
        if prepared.runtime_request.session_disposition
            != worker_runtime::retention::SessionDisposition::Archive
            || prepared.runtime_request.archive_id.is_none()
        {
            return Err(Error::RepositoryConflict(
                "unreflected Journal requires a retained original Session archive".into(),
            ));
        }
        if let Err(original) = retain_source(&runtime, &service.workspace_id, &attr, source, false)
        {
            // Recovery may already have transferred the physical source before its Backend
            // catalog commit; authenticate the exact reserved archive, never another Session.
            let archive = runtime_api::SessionPublicSource::Archived {
                archive_id: prepared
                    .runtime_request
                    .archive_id
                    .clone()
                    .expect("checked"),
            };
            retain_source(&runtime, &service.workspace_id, &attr, archive, false)
                .map_err(|_| original)?;
        }
    } else {
        // This stopped/removal-fenced Session has no admitted remainder; purge need not
        // force a generation. A failed release conservatively blocks ordinary deletion.
        retain_source(&runtime, &service.workspace_id, &attr, source, true)?;
    }
    Ok(())
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
    let worker = verified_worker(source)?;
    api.store
        .db_executor()
        .run(move || {
            validate_workspace_scope(&api, &path.workspace_id)?;
            finish(&api, &worker).map(Json).map_err(ApiError::from)
        })
        .await
}

/// A bounded Runtime transport view, not a second history index or a local-path fallback.
/// Reopening it observes a new catalog; each generation still compares its frozen source identity.
struct MaterialReader<'a> {
    runtime: &'a RuntimeRegistry,
    workspace_id: &'a str,
    runtime_id: String,
    source: runtime_api::SessionPublicSource,
    session_id: String,
    not_materialized: bool,
    descriptors: Vec<session_store::public_material::PublicMaterialDescriptor>,
}
fn material_descriptor(
    d: runtime_api::SessionMaterialDescriptor,
) -> Result<session_store::public_material::PublicMaterialDescriptor> {
    Ok(session_store::public_material::PublicMaterialDescriptor {
        identity: session_store::public_material::PublicMaterialIdentity {
            segment_id: Uuid::parse_str(&d.segment_id).map_err(|e| Error::Store(e.to_string()))?,
            entry_ref: d.entry_ref,
        },
        canonical_order: d.canonical_order,
        origin: serde_json::from_str(&d.origin_json).map_err(|e| Error::Store(e.to_string()))?,
        sha256: d.sha256,
        total_bytes: d.total_bytes,
    })
}
impl<'a> MaterialReader<'a> {
    fn open(
        runtime: &'a RuntimeRegistry,
        workspace_id: &'a str,
        runtime_id: &str,
        source: runtime_api::SessionPublicSource,
        session_id: &str,
    ) -> Result<Self> {
        let mut descriptors = Vec::new();
        let mut offset = 0;
        let mut condition = None;
        loop {
            let page = match runtime
                .session_material_catalog(
                    runtime_id,
                    runtime_api::SessionMaterialCatalogRequest {
                        workspace_id: workspace_id.into(),
                        source: source.clone(),
                        expected_session_id: session_id.into(),
                        expected_catalog_sha256: condition.clone(),
                        offset,
                        limit: 100,
                    },
                )
                .map_err(|e| e.into_error())?
            {
                runtime_api::SessionMaterialCatalogAvailability::NotMaterialized {
                    session_id: observed,
                } => {
                    if observed != session_id || offset != 0 || condition.is_some() {
                        return Err(Error::RepositoryConflict(
                            "Journal Session materialization proof changed during catalog read"
                                .into(),
                        ));
                    }
                    return Ok(Self {
                        runtime,
                        workspace_id,
                        runtime_id: runtime_id.into(),
                        source,
                        session_id: session_id.into(),
                        not_materialized: true,
                        descriptors,
                    });
                }
                runtime_api::SessionMaterialCatalogAvailability::Page { page } => page,
                runtime_api::SessionMaterialCatalogAvailability::Unavailable {
                    reason,
                    message,
                } => {
                    return Err(Error::Store(format!(
                        "Journal original material unavailable ({reason:?}): {message}"
                    )));
                }
            };
            if page.session_id != session_id
                || condition
                    .as_ref()
                    .is_some_and(|c| c != &page.catalog_sha256)
                || page.descriptors.len() > 100
                || descriptors.len().saturating_add(page.descriptors.len()) > 1_000_000
            {
                return Err(Error::RepositoryConflict(
                    "Journal canonical Session catalog changed or exceeded its bound".into(),
                ));
            }
            condition = Some(page.catalog_sha256);
            for d in page.descriptors {
                let d = material_descriptor(d)?;
                if d.canonical_order != descriptors.len() as u64 {
                    return Err(Error::RepositoryConflict(
                        "Journal canonical catalog order is discontinuous".into(),
                    ));
                }
                descriptors.push(d);
            }
            match page.next_offset {
                Some(next) if next > offset && next == descriptors.len() as u64 => offset = next,
                None => break,
                _ => {
                    return Err(Error::RepositoryConflict(
                        "Journal catalog continuation did not advance".into(),
                    ));
                }
            }
        }
        Ok(Self {
            runtime,
            workspace_id,
            runtime_id: runtime_id.into(),
            source,
            session_id: session_id.into(),
            not_materialized: false,
            descriptors,
        })
    }
    fn descriptors(
        &self,
    ) -> impl Iterator<Item = &session_store::public_material::PublicMaterialDescriptor> {
        self.descriptors.iter()
    }
    fn read_content(
        &self,
        descriptor: &session_store::public_material::PublicMaterialDescriptor,
        offset: u64,
        limit: usize,
    ) -> Result<session_store::public_material::PublicMaterialContent> {
        let page = match self
            .runtime
            .session_material_read(
                &self.runtime_id,
                runtime_api::SessionMaterialReadRequest {
                    workspace_id: self.workspace_id.into(),
                    source: self.source.clone(),
                    expected_session_id: self.session_id.clone(),
                    expected_descriptor: runtime_api::SessionMaterialDescriptor {
                        segment_id: descriptor.identity.segment_id.to_string(),
                        entry_ref: descriptor.identity.entry_ref.clone(),
                        canonical_order: descriptor.canonical_order,
                        origin_json: serde_json::to_string(&descriptor.origin)
                            .map_err(|e| Error::Store(e.to_string()))?,
                        sha256: descriptor.sha256.clone(),
                        total_bytes: descriptor.total_bytes,
                    },
                    byte_offset: offset,
                    max_content_bytes: limit.clamp(4, 16 * 1024),
                },
            )
            .map_err(|e| e.into_error())?
        {
            runtime_api::SessionMaterialReadAvailability::Page { page } => page,
            runtime_api::SessionMaterialReadAvailability::Unavailable { reason, message } => {
                return Err(Error::Store(format!(
                    "Journal original material read unavailable ({reason:?}): {message}"
                )));
            }
        };
        if material_descriptor(page.descriptor)? != *descriptor
            || page.content.len() > limit.clamp(4, 16 * 1024)
        {
            return Err(Error::RepositoryConflict(
                "Journal canonical material identity changed".into(),
            ));
        }
        Ok(session_store::public_material::PublicMaterialContent {
            content: page.content,
            next_offset: page.next_offset,
            total_bytes: descriptor.total_bytes,
            sha256: descriptor.sha256.clone(),
        })
    }
}

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
    let store = store(api)?;
    store
        .journal_session_erased(&host(api, &store, grant)?)
        .map_err(domain)
}

/// Unknown remains owned by the original Job until domain completion is proven or
/// canonical stop/Purge cleanup AND the generation fence have both committed.
pub(super) fn reconcile_unknown(
    api: &WorkspaceApi,
    job: &BackendJobRecord,
    attempt: &BackendJobAttemptRecord,
) -> Result<()> {
    if attempt.state != BackendJobAttemptState::Unknown {
        return Ok(());
    }
    let grant = job
        .request
        .grants
        .subjektiv_journal
        .as_ref()
        .ok_or_else(|| Error::InvalidInput("Journal recovery requires a grant".into()))?;
    let store = store(api)?;
    let host = host(api, &store, grant)?;
    let erased = store.journal_session_erased(&host).map_err(domain)?;
    if !erased {
        let g = store
            .prepare_journal_generation(
                &host,
                &grant.journal_id,
                subjektiv::api::JobAttemptBinding {
                    job_id: job.request.job_id.clone(),
                    attempt_id: attempt.attempt_id.clone(),
                },
                grant.selected_ranges.clone(),
            )
            .map_err(domain)?;
        let ctx = JournalJobContext::validated_recovery(&store, &host, &g.id, g.job.clone())
            .map_err(domain)?;
        match journal::service::finish(&store, &ctx) {
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
                    .ok_or_else(|| Error::Store("reconciled Job missing".into()))?;
                notification_delivery::deliver_job_result(
                    api,
                    &BackendJobResultAcceptance {
                        job: saved,
                        attempt: current.clone(),
                        replayed: false,
                    },
                );
                if current.worker_cleanup_state == Some(BackendJobWorkerCleanupState::Completed) {
                    drain_after_completion(api, &job.request);
                } else {
                    api.schedule_backend_job_worker_cleanup(&current);
                }
                return Ok(());
            }
            Err(JournalError::Conflict(_) | JournalError::PermissionDenied(_)) => {}
            Err(error) => return Err(domain(error)),
        }
    }
    if attempt.worker_cleanup_state != Some(BackendJobWorkerCleanupState::Completed) {
        return Ok(());
    }
    let confirmed_purge = if let Some(worker) = &attempt.worker {
        api.config_store
            .recover_worker_removal_execution(api.workspace_id(), worker)
            .map_err(|e| Error::Store(e.to_string()))?
            .is_some_and(|p| {
                p.plan.state == crate::retention::WorkerRemovalPlanState::Succeeded
                    && p.runtime_request.session_disposition
                        == worker_runtime::retention::SessionDisposition::Purge
            })
    } else {
        false
    };
    if !confirmed_purge {
        return Err(Error::RepositoryConflict(
            "Unknown Journal attempt has no committed stop/Purge fence; not retried".into(),
        ));
    }
    if !erased {
        close(api, &job.request, attempt)?;
    }
    api.store.reconcile_backend_journal_attempt(
        api.workspace_id(),
        attempt,
        None,
        &Utc::now().to_rfc3339(),
    )?;
    if !erased {
        // The old immutable input may include reflected bytes. Select the remaining
        // prefix afresh only AFTER definitive lifecycle/domain fences; never replace Unknown.
        let attr = store
            .session_attribution(&grant.session_id)
            .map_err(|e| Error::Store(e.to_string()))?
            .ok_or_else(|| Error::Store("Journal attribution missing".into()))?;
        dispatch(api, attr, &grant.journal_id)?;
    }
    Ok(())
}

fn reconcile_session_jobs(
    api: &WorkspaceApi,
    attr: &crate::subjektiv::SubjectSessionAttribution,
    erased: bool,
) -> Result<()> {
    let mut after: Option<String> = None;
    loop {
        let jobs = api.store.list_backend_journal_jobs(
            api.workspace_id(),
            &attr.session_id,
            after.as_deref(),
            100,
        )?;
        for job in &jobs {
            let id = crate::backend_job::attempt_id(&job.request.job_id, job.current_attempt);
            let mut attempt = api
                .store
                .get_backend_job_attempt(api.workspace_id(), &job.request.job_id, &id)?
                .ok_or_else(|| Error::Store("Journal current attempt missing".into()))?;
            if erased && !attempt.state.terminal() {
                // The domain tombstone precedes this cancellation and also fences any
                // crash window before it. No erased Job can prepare or use tools again.
                attempt = api.store.finish_backend_job_attempt(
                    api.workspace_id(),
                    &job.request.job_id,
                    &id,
                    BackendJobAttemptState::Failed,
                    "journal_session_erased",
                    "Source Session was explicitly erased",
                    &Utc::now().to_rfc3339(),
                )?;
            }
            if attempt.state == BackendJobAttemptState::Unknown {
                reconcile_unknown(api, job, &attempt)?;
            }
            if attempt.state.terminal() {
                api.schedule_backend_job_worker_cleanup(&attempt);
            }
        }
        if jobs.len() < 100 {
            break;
        }
        after = jobs.last().map(|j| j.request.job_id.clone());
    }
    Ok(())
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
        )
    }
}

/// Called from generic Job restart recovery too: accepted source notifications and
/// source Worker availability are not prerequisites for retained result reconciliation.
pub(super) fn recover_unknowns(api: &WorkspaceApi) -> Result<()> {
    let mut after: Option<String> = None;
    loop {
        let page = api.store.list_backend_journal_unknown_attempts(
            api.workspace_id(),
            after.as_deref(),
            100,
        )?;
        for attempt in &page {
            let job = api
                .store
                .get_backend_job(api.workspace_id(), &attempt.job_id)?
                .ok_or_else(|| Error::Store("retained Unknown Journal Job missing".into()))?;
            if let Err(error) = reconcile_unknown(api, &job, attempt) {
                tracing::warn!(%error,job_id=%attempt.job_id,attempt_id=%attempt.attempt_id,"Journal Unknown is unresolved; no replacement dispatched");
            }
        }
        if page.len() < 100 {
            break;
        }
        after = page.last().map(|attempt| attempt.job_id.clone());
    }
    Ok(())
}
