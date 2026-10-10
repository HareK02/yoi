use super::*;
use server_api::{
    SubjektivMemoryBackendOperation as Op, SubjektivMemoryBackendResponse as Response,
};

/// Executes the same domain operations and bounded projections for every Host.
/// The caller authenticates and validates evidence before issuing `context`.
pub fn execute(
    store: &crate::SubjektivStore,
    context: &HostOperationContext,
    operation: Op,
) -> OperationResult<Response> {
    context.require_store(store)?;
    let subject_id = context.subject_id();
    Ok(match operation {
        Op::ResidentSummary(_) => {
            context.body()?;
            Response::ResidentSummary(resident_summary_output(
                store
                    .resident_surface(subject_id)
                    .map_err(subjektiv_store_error)?,
            ))
        }
        Op::ResidentContext(_) => {
            context.body()?;
            let resident = store
                .resident_context(subject_id)
                .map_err(subjektiv_store_error)?;
            Response::ResidentContext(server_api::SubjektivResidentContextOutput {
                behavior_md: resident.subject.behavior_md,
                memory_surface: resident_summary_output(resident.surface),
            })
        }
        Op::Query(input) => Response::Query(subjektiv_memory_query(store, subject_id, input)?),
        Op::Read(input) => Response::Read(subjektiv_memory_read(store, subject_id, input)?),
        Op::ListChanges(input) => {
            Response::ListChanges(subjektiv_memory_list_changes(store, subject_id, input)?)
        }
        Op::ValidateProposal(input) => {
            context.body()?;
            Response::ProposalValidated(subjektiv_memory_validate_proposal(
                store, subject_id, input,
            )?)
        }
        Op::ReceiptStatus(input) => {
            context.body()?;
            validate_subjektiv_receipt_id(&input.receipt_id)?;
            let candidate = store
                .staging_candidate(subject_id, &input.receipt_id)
                .map_err(subjektiv_store_error)?;
            Response::ReceiptStatus(server_api::SubjektivMemoryReceiptStatusResponse {
                receipt_id: input.receipt_id,
                status: if candidate.is_some() {
                    server_api::SubjektivMemoryReceiptStatus::Staged
                } else {
                    server_api::SubjektivMemoryReceiptStatus::Missing
                },
                candidate_id: candidate.map(|record| record.id),
            })
        }
        Op::StageExplicit(input) => {
            let attribution = context.body()?.ok_or_else(|| {
                OperationError::PermissionDenied(
                    "explicit staging requires validated Session attribution".into(),
                )
            })?;
            if input.session_id != attribution.session_id {
                return Err(OperationError::PermissionDenied(
                    "explicit staging Session does not match Host attribution".into(),
                ));
            }
            Response::Staged(subjektiv_memory_stage_explicit(
                store,
                input,
                attribution.clone(),
            )?)
        }
        Op::ListCandidates(input) => {
            let (ids, _) = context.consolidation()?;
            let limit = bounded_subjektiv_limit(input.limit, SUBJEKTIV_QUERY_DEFAULT_LIMIT)?;
            let mut candidates = if let Some(ids) = ids {
                let mut pending = Vec::new();
                for id in ids {
                    if store
                        .staging_resolution(subject_id, id)
                        .map_err(subjektiv_store_error)?
                        .is_none()
                    {
                        if let Some(record) = store
                            .staging_candidate(subject_id, id)
                            .map_err(subjektiv_store_error)?
                        {
                            pending.push(record);
                        }
                    }
                }
                pending
            } else {
                store
                    .pending_staging_candidates(subject_id, limit.saturating_add(1))
                    .map_err(subjektiv_store_error)?
            };
            let has_more = candidates.len() > limit;
            candidates.truncate(limit);
            Response::Candidates(server_api::SubjektivMemoryCandidateListResponse {
                items: candidates
                    .into_iter()
                    .map(subjektiv_candidate_summary)
                    .collect(),
                has_more,
            })
        }
        Op::ReadCandidate(input) => {
            context.candidate(&input.candidate_id)?;
            let candidate = store
                .staging_candidate(subject_id, &input.candidate_id)
                .map_err(subjektiv_store_error)?
                .ok_or_else(|| {
                    OperationError::InvalidInput(format!(
                        "candidate_not_found: pending candidate `{}` was not found",
                        input.candidate_id
                    ))
                })?;
            if store
                .staging_resolution(subject_id, &input.candidate_id)
                .map_err(subjektiv_store_error)?
                .is_some()
            {
                return Err(OperationError::Conflict(format!(
                    "candidate_resolved: candidate `{}` is already resolved",
                    input.candidate_id
                )));
            }
            Response::Candidate(subjektiv_candidate(candidate))
        }
        Op::DecideCandidate(input) => {
            context.candidate(&input.candidate_id)?;
            let receipt = store
                .decide_candidate(subject_id, subjektiv_candidate_decision(input)?)
                .map_err(subjektiv_store_error)?;
            Response::CandidateDecided(subjektiv_candidate_decision_response(receipt))
        }
        Op::PrepareSurface(_) => {
            let (ids, job) = context.consolidation()?;
            if let Some(ids) = ids {
                for id in ids {
                    if store
                        .staging_resolution(subject_id, id)
                        .map_err(subjektiv_store_error)?
                        .is_none()
                    {
                        return Err(OperationError::InvalidInput(format!(
                            "surface generation requires a durable disposition for batch candidate `{id}`"
                        )));
                    }
                }
            }
            let generation = store
                .prepare_job_surface_generation(
                    subject_id,
                    job.map(|b| (b.job_id.as_str(), b.attempt_id.as_str())),
                )
                .map_err(subjektiv_store_error)?;
            let resident = store
                .resident_surface(subject_id)
                .map_err(subjektiv_store_error)?;
            let mut response = subjektiv_surface_prepare_response(generation);
            if resident.availability == crate::SurfaceAvailability::Ready {
                response.current_snapshot_id = resident
                    .snapshot
                    .filter(|s| s.built_from_memory_fingerprint == response.memory_fingerprint)
                    .map(|s| s.id);
            }
            Response::SurfacePrepared(response)
        }
        Op::PublishSurface(input) => {
            let (_, job) = context.consolidation()?;
            if let Some(job) = job {
                store
                    .require_job_surface_generation(
                        subject_id,
                        &input.generation_id,
                        &job.job_id,
                        &job.attempt_id,
                    )
                    .map_err(subjektiv_store_error)?;
            }
            let points = input
                .points
                .into_iter()
                .map(|point| crate::SurfacePoint {
                    body_md: point.body_md,
                    memory_refs: point
                        .memory_refs
                        .into_iter()
                        .map(|r| crate::MemoryChangeRef {
                            memory_id: r.memory_id,
                            change_id: r.change_id,
                        })
                        .collect(),
                })
                .collect();
            let snapshot = store
                .publish_surface_generation(subject_id, &input.generation_id, points)
                .map_err(subjektiv_store_error)?;
            Response::SurfacePublished(server_api::SubjektivSurfacePublishResponse {
                snapshot_id: snapshot.id,
                built_from_memory_fingerprint: snapshot.built_from_memory_fingerprint,
                empty: snapshot.body_md.is_empty(),
            })
        }
        Op::FailSurface(input) => {
            let (_, job) = context.consolidation()?;
            if let Some(job) = job {
                store
                    .require_job_surface_generation(
                        subject_id,
                        &input.generation_id,
                        &job.job_id,
                        &job.attempt_id,
                    )
                    .map_err(subjektiv_store_error)?;
            }
            let memory_fingerprint = store
                .fail_surface_generation(subject_id, &input.generation_id, &input.reason_code)
                .map_err(subjektiv_store_error)?;
            let status = if let Some(job) = job {
                store
                    .validate_job_surface_outcome(
                        subject_id,
                        &input.generation_id,
                        memory_fingerprint.clone(),
                        "failed",
                        None,
                        Some(&input.reason_code),
                        &job.job_id,
                        &job.attempt_id,
                    )
                    .map_err(|e| {
                        OperationError::Conflict(format!(
                            "surface failure was not recorded for this attempt: {e}"
                        ))
                    })?;
                "failed_confirmed"
            } else {
                "failed"
            };
            Response::SurfaceFailed(server_api::SubjektivSurfaceFailureResponse {
                memory_fingerprint,
                status: status.into(),
            })
        }
    })
}
