//! subjektiv is a Job client, not a second runner or Worker registry.
use super::*;
use crate::backend_job::{BackendJobGrants, SubjektivConsolidationGrant};

const PURPOSE: &str = "subjektiv_consolidation";
const CANDIDATE_BATCH_LIMIT: usize = 100;

/// Load Host authority from the authenticated Worker registry and current attempt.
/// Model input, Profile tags, display names and a subject-body lease are not grants.
pub(super) fn active_grant(
    api: &WorkspaceApi,
    workspace_id: &str,
    worker: &RuntimeWorkerRef,
) -> ApiResult<Option<SubjektivConsolidationGrant>> {
    let Some(binding) = api
        .store
        .worker_registry_projection(workspace_id, worker)?
        .and_then(|p| p.job)
    else {
        return Ok(None);
    };
    let job = api
        .store
        .get_backend_job(workspace_id, &binding.job_id)?
        .ok_or_else(|| Error::WorkspacePermissionDenied("Job binding has no intent".into()))?;
    let attempt = api
        .store
        .get_backend_job_attempt(workspace_id, &binding.job_id, &binding.attempt_id)?
        .ok_or_else(|| Error::WorkspacePermissionDenied("Job binding has no attempt".into()))?;
    if job.state != BackendJobState::Pending
        || attempt.state != BackendJobAttemptState::Dispatched
        || attempt.worker.as_ref() != Some(worker)
        || job.current_attempt != attempt.attempt
        || attempt.input_digest != job.request.input_digest()?
        || chrono::DateTime::parse_from_rfc3339(&attempt.deadline_at)
            .map_err(|e| Error::Store(format!("invalid Job deadline: {e}")))?
            <= Utc::now()
    {
        return Err(Error::WorkspacePermissionDenied(
            "subjektiv grant requires the current live Job attempt".into(),
        )
        .into());
    }
    Ok(job.request.grants.subjektiv_consolidation)
}

pub(super) fn require_candidate_grant(
    api: &WorkspaceApi,
    workspace_id: &str,
    worker: &RuntimeWorkerRef,
    candidate_id: &str,
) -> ApiResult<()> {
    if let Some(grant) = active_grant(api, workspace_id, worker)? {
        if !grant.candidate_ids.iter().any(|id| id == candidate_id) {
            return Err(Error::WorkspacePermissionDenied(
                "candidate is outside this Job's immutable batch".into(),
            )
            .into());
        }
    }
    Ok(()) // legacy owned consolidators may finish before the cutover fence
}

/// Retire only the persisted old consolidation owner. Never interrupt a foreground
/// candidate decision. Once Idle is confirmed, stop cancels any legacy surface
/// background work; the new Job regenerates it and does not infer surface success.
pub(super) async fn retire_legacy_worker(api: &WorkspaceApi, subject_id: &str) -> ApiResult<bool> {
    let key = format!("{SUBJEKTIV_CONSOLIDATION_SINGLETON_PREFIX}{subject_id}");
    let Some(owner) = api
        .store
        .current_worker_singleton_owner(api.workspace_id(), &key)?
    else {
        return Ok(true);
    };
    let mut summary = api
        .runtime
        .worker(&owner.worker)
        .map_err(|e| e.into_error())?;
    summary.singleton_key = Some(owner.key.clone());
    require_dedicated_subjektiv_consolidation_worker(&summary, api.workspace_id(), &key)?;
    let stopped = summary.state.eq_ignore_ascii_case("stopped");
    let idle = summary.state.eq_ignore_ascii_case("idle")
        && summary
            .worker_state
            .as_ref()
            .is_some_and(|s| s.state == protocol::WorkerState::Idle);
    if !stopped && !idle {
        return Ok(false);
    }
    if api
        .store
        .current_worker_singleton_owner(api.workspace_id(), &key)?
        .as_ref()
        .map(|o| &o.worker)
        != Some(&owner.worker)
    {
        return Err(Error::RepositoryConflict(
            "legacy consolidation owner moved during cutover".into(),
        )
        .into());
    }
    if !summary.state.eq_ignore_ascii_case("stopped") {
        let stopped = api
            .runtime
            .stop_worker(
                &owner.worker,
                WorkerLifecycleRequest {
                    reason: Some(
                        "Retire legacy subjektiv consolidator after foreground completion".into(),
                    ),
                    ticket_assignment: None,
                },
            )
            .map_err(|e| e.into_error())?;
        if stopped.state != InternalWorkerOperationState::Accepted {
            return Ok(false);
        }
    }
    if !api
        .runtime
        .worker(&owner.worker)
        .map_err(|e| e.into_error())?
        .state
        .eq_ignore_ascii_case("stopped")
    {
        return Ok(false);
    }
    let removal = WorkerRemovalService::new(api)
        .execute_target_removal(
            api.runtime.as_ref(),
            &owner.worker,
            "subjektiv consolidation Job cutover",
            None,
        )
        .await
        .map_err(Error::Store)?;
    if removal.status != StatusCode::OK.as_u16() {
        return Err(Error::Store(format!(
            "legacy consolidation cleanup rejected: {}",
            removal.body
        ))
        .into());
    }
    Ok(api
        .store
        .current_worker_singleton_owner(api.workspace_id(), &key)?
        .is_none())
}

fn bounded_candidate_batch(
    ids: impl IntoIterator<Item = String>,
    max_result_bytes: u32,
) -> Result<Vec<String>> {
    subjektiv::job::bounded_candidate_batch(ids, max_result_bytes).map_err(Error::from)
}

pub(super) fn dispatch(
    api: &WorkspaceApi,
    subject_id: &str,
    candidate_count: usize,
    total_bytes: u64,
) -> ApiResult<MemoryConsolidationOutput> {
    let store = open_subjektiv_store(api)?;
    store
        .subject(subject_id)
        .map_err(subjektiv_store_error)?
        .ok_or_else(|| Error::SubjektivSubjectNotFound(subject_id.into()))?;
    let candidate_ids = bounded_candidate_batch(
        store
            .pending_staging_candidates(subject_id, CANDIDATE_BATCH_LIMIT)
            .map_err(subjektiv_store_error)?
            .into_iter()
            .map(|c| c.id),
        crate::backend_job::DEFAULT_MAX_RESULT_BYTES,
    )?;
    let batch_len = candidate_ids.len();
    let request = BackendJobRequest {
        job_id: format!("subjektiv-consolidation:{}", Uuid::new_v4()),
        purpose: PURPOSE.into(),
        input_ref: format!("subjektiv://{subject_id}/consolidation"),
        input: serde_json::json!({"subject_id": subject_id, "candidate_ids": candidate_ids}),
        instruction: "Resolve ONLY the immutable candidate_ids batch using MemoryStagingList/Read, confirmed Memory reads and MemoryApplyCandidate. Already resolved candidates are retained receipts, not work to repeat. Leave newly arriving candidates to a subsequent Job. When every batch candidate has a durable disposition, call SubmitBackendJobResult with {subject_id, candidate_ids}; its Host capability awaits bounded clean-context surface generation and adds the actual surface outcome before submission. Final prose is not completion.".into(),
        // Selection belongs here, never in the common runner's purpose switch.
        profile: format!("builtin:{SUBJEKTIV_MEMORY_CONSOLIDATION_PROFILE}"),
        grants: BackendJobGrants { subjektiv_consolidation: Some(SubjektivConsolidationGrant { subject_id: subject_id.into(), candidate_ids }) },
        serialization_key: None,
        source_worker: None,
        notification_target: None,
        limits: crate::backend_job::BackendJobLimits { timeout_seconds: 600, ..Default::default() },
    };
    let reservation = api.dispatch_backend_job(&request)?;
    Ok(MemoryConsolidationOutput {
        status: if reservation.resource_reused {
            "skipped_existing_job"
        } else {
            "started"
        }
        .into(),
        summary: format!(
            "Subject consolidation Job '{}' ({:?}) owns an immutable batch of {} candidate(s); remaining/new candidates stay pending for the next request. No persistent subject consolidator is reused.",
            reservation.job.request.job_id,
            reservation.attempt.state,
            if reservation.resource_reused {
                reservation
                    .job
                    .request
                    .grants
                    .subjektiv_consolidation
                    .as_ref()
                    .map_or(0, |g| g.candidate_ids.len())
            } else {
                batch_len
            }
        ),
        candidate_count,
        total_bytes,
    })
}

pub(super) fn validate_result(
    api: &WorkspaceApi,
    request: &BackendJobRequest,
    attempt: &BackendJobAttemptRecord,
    value: &serde_json::Value,
) -> Result<()> {
    let Some(grant) = &request.grants.subjektiv_consolidation else {
        return Ok(());
    };
    let store =
        crate::subjektiv::SubjektivStore::open(&api.feature_storage, &api.subjektiv_registration)
            .map_err(|e| Error::Store(e.to_string()))?;
    subjektiv::job::validate_result(
        &store,
        &grant.subject_id,
        &grant.candidate_ids,
        &request.job_id,
        &attempt.attempt_id,
        value,
    )
    .map_err(Error::from)
}

/// Persist actual candidate dispositions alongside the separate surface outcome.
/// Dispositions are immutable existing records, not model assertions or another
/// apply ledger. This enrichment is deterministic on exact result replay.
pub(super) fn with_dispositions(
    api: &WorkspaceApi,
    request: &BackendJobRequest,
    value: &serde_json::Value,
) -> Result<serde_json::Value> {
    let grant = request
        .grants
        .subjektiv_consolidation
        .as_ref()
        .ok_or_else(|| Error::InvalidInput("missing consolidation grant".into()))?;
    let store =
        crate::subjektiv::SubjektivStore::open(&api.feature_storage, &api.subjektiv_registration)
            .map_err(|e| Error::Store(e.to_string()))?;
    subjektiv::job::with_dispositions(&store, &grant.subject_id, &grant.candidate_ids, value)
        .map_err(Error::from)
}

#[cfg(test)]
mod result_budget_tests {
    use super::*;

    #[test]
    fn escaped_maximum_ids_leave_a_bounded_complete_result() {
        let ids: Vec<_> = (0..100)
            .map(|i| format!("{i:03}{}", "\\\"".repeat(126)))
            .collect();
        let batch =
            bounded_candidate_batch(ids.clone(), crate::backend_job::DEFAULT_MAX_RESULT_BYTES)
                .unwrap();
        assert!(!batch.is_empty());
        assert!(batch.len() < ids.len());
        assert_eq!(batch, ids[..batch.len()]);
        let dispositions: Vec<_> = batch
            .iter()
            .map(|id| serde_json::json!({"candidate_id":id,"action":"already_covered"}))
            .collect();
        let result = serde_json::json!({
            "subject_id":"\\\"".repeat(128),"candidate_ids":batch,
            "candidate_dispositions":dispositions,
            "surface":{"availability":"failed","generation_id":"g".repeat(100),"store_revision":u64::MAX,"reason_code":"\\\"".repeat(128)},
        });
        assert!(
            serde_json::to_vec(&result).unwrap().len()
                <= crate::backend_job::DEFAULT_MAX_RESULT_BYTES as usize
        );
    }
}
