//! Journal admission and existing-Job orchestration. Domain receipts remain in Subjektiv;
//! the Backend owns authenticated source identity and live attempt capabilities.
use super::*;
use crate::backend_job::{
    BackendJobGrants, BackendJobRecord, SubjektivJournalGrant, journal_serialization_key,
};
use subjektiv::journal::recording::{JournalMaterial, select_materials_bounded};
use subjektiv::journal::transport::*;
use subjektiv::journal::{self, *};

fn domain(error: JournalError) -> Error {
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

fn host(
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
    JournalHostContext::validated_session(store, attribution).map_err(domain)
}

/// Preparing before dispatch is also used by Reserved-attempt restart recovery. A retry
/// with the same attempt and complete selection converges on the original snapshot.
pub(super) fn prepare(api: &WorkspaceApi, reservation: &BackendJobReservation) -> Result<()> {
    let Some(grant) = &reservation.job.request.grants.subjektiv_journal else {
        return Ok(());
    };
    let store = store(api)?;
    let host = host(&store, grant)?;
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
    let host = host(&store, &grant)?;
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
    let host = host(&store, &grant)?;
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
    let host = host(&store, grant)?;
    let g = store
        .prepare_journal_generation(
            &host,
            &grant.journal_id,
            subjektiv::api::JobAttemptBinding {
                job_id: request.job_id.clone(),
                attempt_id: attempt.attempt_id.clone(),
            },
            grant.selected_ranges.clone(),
        )
        .map_err(domain)?;
    let ctx = JournalJobContext::validated_recovery(&store, &host, &g.id, g.job.clone())
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
    let host = host(&store, grant)?;
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

fn dispatch(
    api: &WorkspaceApi,
    attribution: crate::subjektiv::SubjectSessionAttribution,
    journal_id: &str,
) -> Result<()> {
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
    let request = recording_request(attribution, journal_id, materials);
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
    if let Err(error) = dispatch(api, attribution, &id) {
        tracing::warn!(%error,journal_id=%id,"Journal admitted; Job dispatch remains recoverable");
    }
    Ok(JournalRecordingResponse::Accepted)
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
    attribution: crate::subjektiv::SubjectSessionAttribution,
    journal_id: &str,
    materials: Vec<journal::recording::JournalMaterial>,
) -> BackendJobRequest {
    let grant = SubjektivJournalGrant {
        subject_id: attribution.subject_id,
        session_id: attribution.session_id,
        journal_id: journal_id.into(),
        selected_ranges: materials.iter().map(|m| m.range.clone()).collect(),
    };
    BackendJobRequest {
        job_id:format!("journal-recording:{}",Uuid::new_v4()),purpose:"subjektiv_journal".into(),
        input_ref:format!("subjektiv://{}/journal/{journal_id}",grant.subject_id),
        input:serde_json::json!({"journal_id":journal_id,"selected_ranges":grant.selected_ranges}),
        instruction:"Record ONLY this frozen original public selected_ranges prefix. The input contains descriptors, NOT source bodies. First ReadJournalState and use bounded ReadJournalText Source until every byte of every selected range is returned; record operations are refused until then. ReadJournalText also exposes initial/current position/open text. Sequentially save CreateJournalEntry/RewriteJournalEntry/FixJournalEntry, update current position and CheckpointJournal using retained record receipts. Never infer event completion from turn/task/job/compaction boundaries. Preserve exact operation ID/full input after unknown outcomes. SubmitBackendJobResult only after the full selected prefix has a saved checkpoint; no aggregate JournalRecordResult.".into(),
        profile:"builtin:subjektiv-journal-recording".into(),
        grants:BackendJobGrants {subjektiv_journal:Some(grant),..Default::default()},
        serialization_key:Some(journal_serialization_key(journal_id)),source_worker:None,notification_target:None,
        limits:crate::backend_job::BackendJobLimits {timeout_seconds:600,..Default::default()},
    }
}

fn removal_attribution(
    service: &WorkerRemovalService,
    target: &RuntimeWorkerRef,
) -> Result<Option<crate::subjektiv::SubjectSessionAttribution>> {
    let Some(lease) = service
        .store
        .require_current_worker_singleton_owner(&service.workspace_id, target)?
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

/// This Host Feature gate runs for stopped Workers too. Admission/Job intent are
/// durable before deletion planning; actual generation is not a deletion prerequisite.
pub(super) fn prepare_worker_removal(
    service: &WorkerRemovalService,
    runtime: &RuntimeRegistry,
    target: &RuntimeWorkerRef,
) -> Result<Option<crate::retention::ErasedJournalSession>> {
    let Some(attr) = removal_attribution(service, target)? else {
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
        return Ok(Some(proof));
    }
    let collect_new_sources = enabled(&api, target)?;
    let mut id = store.journal_for_session(&host).map_err(domain)?;
    // Disablement forbids new admission, not recovery of an already admitted prefix.
    // With no admitted remainder there is no source reader or Job to create at removal.
    if !collect_new_sources && !store.journal_retention_required(&host).map_err(domain)? {
        return Ok(None);
    }
    let reader = MaterialReader::open(
        runtime,
        &service.workspace_id,
        &target.runtime_id,
        source.clone(),
        &attr.session_id,
    )?;
    if reader.not_materialized {
        if store.journal_retention_required(&host).map_err(domain)? {
            return Err(Error::RepositoryConflict(
                "Admitted Journal original Session is not materialized".into(),
            ));
        }
        retain_source(runtime, &service.workspace_id, &attr, source, true)?;
        return Ok(None);
    }
    retain_source(runtime, &service.workspace_id, &attr, source, false)?;
    if collect_new_sources {
        let ranges: Vec<_> = reader
            .descriptors()
            .map(|d| source_from_descriptor(&attr.session_id, d))
            .collect();
        for batch in ranges.chunks(MAX_JOURNAL_RANGES) {
            id = Some(
                store
                    .accept_journal_sources(&host, "Session activity", batch)
                    .map_err(domain)?,
            );
        }
    }
    let Some(id) = id else {
        return Ok(None);
    };
    if !store.journal_retention_required(&host).map_err(domain)? {
        return Ok(None);
    }
    if let Some(g) = store
        .active_journal_generation(&host, &id)
        .map_err(domain)?
    {
        // An unknown dispatch is still ownership, never permission to create another Job.
        if service
            .store
            .get_backend_job(&service.workspace_id, &g.job.job_id)?
            .is_some()
        {
            return Ok(None);
        }
        return Err(Error::Store(
            "Journal generation's existing Job intent is missing".into(),
        ));
    }
    let materials = select_materials_bounded(&store, &host, &id, 96 * 1024, |s, start, limit| {
        read_canonical(&reader, s, start, limit).map_err(|e| JournalError::Conflict(e.to_string()))
    })
    .map_err(domain)?;
    if materials.is_empty() {
        return Err(Error::Store(
            "unreflected Journal sources could not be handed off".into(),
        ));
    }
    let request = recording_request(attr, &id, materials);
    service.store.reserve_backend_job(
        &service.workspace_id,
        &request,
        &Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true),
    )?;
    if let Some(api) = service.journal_dispatch_api.upgrade() {
        // Already on Host: do not submit and await this same occupied lane. Dispatch
        // only starts the existing bounded Job; it never waits for Journal generation.
        api.drain_reserved_backend_jobs();
    }
    Ok(None)
}

pub(super) fn validate_removal_retention(
    service: &WorkerRemovalService,
    target: &RuntimeWorkerRef,
    prepared: &crate::retention::PreparedWorkerRemoval,
) -> Result<()> {
    let Some(attr) = removal_attribution(service, target)? else {
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
        .journal_session_erased(&host(&store, grant)?)
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
    let host = host(&store, grant)?;
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
