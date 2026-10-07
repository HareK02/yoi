//! Shared model-facing Memory operations and bounded projections.
//! Hosts retain authentication, execution leases, Job admission and committed
//! Session evidence validation; this module never derives ambient authority.

mod context;
mod operation;
pub use context::{HostOperationContext, JobAttemptBinding};
pub use operation::execute;

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const SUBJEKTIV_QUERY_DEFAULT_LIMIT: usize = 20;
pub const SUBJEKTIV_QUERY_MAX_LIMIT: usize = 100;
pub const SUBJEKTIV_BODY_DEFAULT_LINES: usize = 200;
pub const SUBJEKTIV_BODY_MAX_LINES: usize = 1_000;
pub const SUBJEKTIV_BODY_MAX_BYTES: usize = 16 * 1024;
pub const SUBJEKTIV_CANDIDATE_ANCHOR_LIMIT: usize = crate::MAX_STAGING_ANCHORS;
pub const SUBJEKTIV_EVIDENCE_ANCHOR_PAGE_SIZE: usize = 2;
pub const SUBJEKTIV_ANCHOR_TEXT_MAX_BYTES: usize = 64;
pub const SUBJEKTIV_NESTED_EVIDENCE_MAX_BYTES: usize = 40 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SubjektivQueryCursor {
    subject_id: String,
    store_revision: u64,
    query: Option<String>,
    kinds: Vec<String>,
    states: Vec<String>,
    offset: usize,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SubjektivRevisionCursor {
    subject_id: String,
    memory_id: String,
    max_revision: u64,
    offset: usize,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SubjektivEvidenceCursor {
    subject_id: String,
    memory_id: String,
    revision: u64,
    /// Candidate/derivation reference offset within the immutable revision.
    offset: usize,
    /// Nested offsets keep compatible historical candidates with more anchors
    /// resumable without admitting new over-limit records.
    #[serde(default)]
    evidence_offset: usize,
    #[serde(default)]
    source_ref_offset: usize,
}

pub fn resident_summary_output(
    resident: crate::ResidentSurface,
) -> memory::backend::MemoryResidentSummaryOutput {
    let (availability, content) = match resident.availability {
        crate::SurfaceAvailability::Ready => {
            let snapshot = resident.snapshot.expect("ready surface has a snapshot");
            (
                memory::backend::MemoryResidentSummaryAvailability::Ready,
                (!snapshot.body_md.is_empty()).then_some(snapshot.body_md),
            )
        }
        crate::SurfaceAvailability::Ungenerated => (
            memory::backend::MemoryResidentSummaryAvailability::Ungenerated,
            None,
        ),
        crate::SurfaceAvailability::Stale => (
            memory::backend::MemoryResidentSummaryAvailability::Stale,
            None,
        ),
        crate::SurfaceAvailability::Failed => (
            memory::backend::MemoryResidentSummaryAvailability::Failed,
            None,
        ),
    };
    memory::backend::MemoryResidentSummaryOutput {
        availability,
        content,
    }
}

pub fn subjektiv_surface_prepare_response(
    generation: crate::SurfaceGeneration,
) -> server_api::SubjektivSurfacePrepareResponse {
    server_api::SubjektivSurfacePrepareResponse {
        generation_id: generation.id,
        current_snapshot_id: None,
        store_revision: generation.store_revision,
        active_memory_count: generation.active_memory_count,
        materials: generation
            .materials
            .into_iter()
            .map(|material| server_api::SubjektivSurfaceMaterial {
                memory_id: material.memory_id,
                revision: material.revision,
                kind: material.kind,
                body_md: material.body_md,
                why_useful: material.why_useful,
                staleness: material.staleness,
            })
            .collect(),
        body_token_budget: crate::SURFACE_BODY_TOKEN_BUDGET,
        input_token_budget: crate::SURFACE_INPUT_TOKEN_BUDGET,
        per_kind_limit: crate::SURFACE_PER_KIND_LIMIT,
        total_material_limit: crate::SURFACE_TOTAL_MATERIAL_LIMIT,
    }
}

pub fn subjektiv_candidate_summary(
    candidate: crate::SubjectStagingRecord,
) -> server_api::SubjektivMemoryCandidateSummary {
    server_api::SubjektivMemoryCandidateSummary {
        candidate_id: candidate.id,
        kind: candidate.kind,
        claim: candidate.claim,
        revision_proposal: candidate.revision_proposal.map(subjektiv_proposal_metadata),
        created_at: candidate.created_at,
    }
}

pub fn subjektiv_candidate(
    candidate: crate::SubjectStagingRecord,
) -> server_api::SubjektivMemoryCandidate {
    server_api::SubjektivMemoryCandidate {
        candidate_id: candidate.id,
        kind: candidate.kind,
        claim: candidate.claim,
        why_useful: candidate.why_useful,
        staleness: candidate.staleness,
        source: candidate.source,
        evidence: candidate.evidence,
        source_refs: candidate.source_refs,
        revision_proposal: candidate.revision_proposal.map(subjektiv_proposal_metadata),
        created_at: candidate.created_at,
    }
}

pub fn subjektiv_proposal_metadata(
    proposal: crate::RevisionProposal,
) -> server_api::SubjektivMemoryRevisionProposalMetadata {
    server_api::SubjektivMemoryRevisionProposalMetadata {
        memory_id: proposal.memory_id,
        expected_revision: proposal.expected_revision,
        intent: match proposal.intent {
            crate::RevisionProposalIntent::Revise => {
                server_api::SubjektivMemoryRevisionIntent::Revise
            }
            crate::RevisionProposalIntent::Resolve => {
                server_api::SubjektivMemoryRevisionIntent::Resolve
            }
            crate::RevisionProposalIntent::Retract => {
                server_api::SubjektivMemoryRevisionIntent::Retract
            }
            crate::RevisionProposalIntent::Reopen => {
                server_api::SubjektivMemoryRevisionIntent::Reopen
            }
        },
        change_reason: proposal.change_reason,
    }
}

pub fn subjektiv_candidate_decision(
    input: server_api::SubjektivMemoryCandidateDecisionRequest,
) -> OperationResult<crate::CandidateDecisionRequest> {
    let decision = match input.decision {
        server_api::SubjektivMemoryCandidateDecision::Apply { target, memory } => {
            let target = match target {
                server_api::SubjektivMemoryApplyTarget::Create => {
                    crate::MemoryRevisionTarget::Create
                }
                server_api::SubjektivMemoryApplyTarget::Revise {
                    memory_id,
                    expected_revision,
                } => crate::MemoryRevisionTarget::Revise {
                    memory_id,
                    expected_revision,
                },
            };
            crate::CandidateDecision::Apply {
                target,
                draft: crate::MemoryDraft {
                    kind: memory.kind,
                    state: domain_memory_state(memory.state),
                    claim: memory.claim,
                    body_md: memory.body_md,
                    why_useful: memory.why_useful,
                    staleness: memory.staleness,
                    source_candidate_ids: Vec::new(),
                    derived_from: memory
                        .derived_from
                        .into_iter()
                        .map(|reference| crate::MemoryRevisionRef {
                            memory_id: reference.memory_id,
                            revision: reference.revision,
                        })
                        .collect(),
                    change_reason: memory.change_reason,
                },
            }
        }
        server_api::SubjektivMemoryCandidateDecision::Close {
            action,
            affected_memory,
        } => crate::CandidateDecision::Close {
            action: match action {
                server_api::SubjektivMemoryCandidateCloseAction::Discarded => {
                    crate::StagingResolutionAction::Discarded
                }
                server_api::SubjektivMemoryCandidateCloseAction::Invalid => {
                    crate::StagingResolutionAction::Invalid
                }
                server_api::SubjektivMemoryCandidateCloseAction::Duplicate => {
                    crate::StagingResolutionAction::Duplicate
                }
                server_api::SubjektivMemoryCandidateCloseAction::AlreadyCovered => {
                    crate::StagingResolutionAction::AlreadyCovered
                }
            },
            affected_memory: affected_memory
                .into_iter()
                .map(|reference| crate::MemoryRevisionRef {
                    memory_id: reference.memory_id,
                    revision: reference.revision,
                })
                .collect(),
        },
    };
    Ok(crate::CandidateDecisionRequest {
        request_id: input.request_id,
        candidate_id: input.candidate_id,
        reason: input.reason,
        decision,
    })
}

pub fn subjektiv_candidate_decision_response(
    receipt: crate::CandidateDecisionReceipt,
) -> server_api::SubjektivMemoryCandidateDecisionResponse {
    let operation = match receipt.operation {
        Some(crate::MemoryDecisionOperation::Create) => {
            server_api::SubjektivMemoryAffectedOperation::Create
        }
        Some(crate::MemoryDecisionOperation::Revise) => {
            server_api::SubjektivMemoryAffectedOperation::Revise
        }
        Some(crate::MemoryDecisionOperation::Reference) | None => {
            server_api::SubjektivMemoryAffectedOperation::Reference
        }
    };
    server_api::SubjektivMemoryCandidateDecisionResponse {
        request_id: receipt.request_id,
        candidate_id: receipt.candidate_id,
        action: match receipt.resolution.action {
            crate::StagingResolutionAction::Applied => {
                server_api::SubjektivMemoryCandidateResolutionAction::Applied
            }
            crate::StagingResolutionAction::Discarded => {
                server_api::SubjektivMemoryCandidateResolutionAction::Discarded
            }
            crate::StagingResolutionAction::Invalid => {
                server_api::SubjektivMemoryCandidateResolutionAction::Invalid
            }
            crate::StagingResolutionAction::Duplicate => {
                server_api::SubjektivMemoryCandidateResolutionAction::Duplicate
            }
            crate::StagingResolutionAction::AlreadyCovered => {
                server_api::SubjektivMemoryCandidateResolutionAction::AlreadyCovered
            }
        },
        reason: receipt.resolution.reason,
        affected_memory: receipt
            .resolution
            .affected_memory
            .into_iter()
            .map(|reference| server_api::SubjektivMemoryAffectedRef {
                memory_id: reference.memory_id,
                revision: reference.revision,
                operation,
            })
            .collect(),
        memory: receipt
            .memory
            .map(|memory| server_api::SubjektivMemoryRevisionRef {
                memory_id: memory.id,
                revision: memory.revision,
            }),
        store_revision: receipt.store_revision,
        surface_dirty: receipt.surface_dirty,
    }
}

pub fn domain_memory_state(state: server_api::SubjektivMemoryState) -> crate::MemoryState {
    match state {
        server_api::SubjektivMemoryState::Active => crate::MemoryState::Active,
        server_api::SubjektivMemoryState::Resolved => crate::MemoryState::Resolved,
        server_api::SubjektivMemoryState::Retracted => crate::MemoryState::Retracted,
    }
}

pub fn subjektiv_memory_query(
    store: &crate::SubjektivStore,
    subject_id: &str,
    input: server_api::SubjektivMemoryQueryRequest,
) -> OperationResult<server_api::SubjektivMemoryQueryResponse> {
    let limit = bounded_subjektiv_limit(input.limit, SUBJEKTIV_QUERY_DEFAULT_LIMIT)?;
    let query = input
        .query
        .map(|value| value.trim().to_lowercase())
        .filter(|value| !value.is_empty());
    let kinds = canonical_subjektiv_kinds(input.kinds)?;
    let states = canonical_subjektiv_states(input.states)?;
    let subject = store
        .subject(subject_id)
        .map_err(subjektiv_store_error)?
        .ok_or_else(|| OperationError::SubjectNotFound(subject_id.to_string()))?;
    let mut offset = 0;
    if let Some(cursor) = input.cursor {
        let cursor: SubjektivQueryCursor = decode_subjektiv_cursor("query", &cursor)?;
        if cursor.subject_id != subject_id
            || cursor.query != query
            || cursor.kinds != kinds
            || cursor.states != states
        {
            return Err(OperationError::InvalidInput(
                "subjektiv query cursor does not match subject or filters".into(),
            )
            .into());
        }
        if cursor.store_revision != subject.store_revision {
            return Err(OperationError::Conflict(
                "stale_cursor: subjektiv query cursor is stale because Memory changed; restart the query".into(),
            )
            .into());
        }
        offset = cursor.offset;
    }

    let kind_filter = kinds.iter().cloned().collect::<HashSet<_>>();
    let state_filter = states.iter().cloned().collect::<HashSet<_>>();
    let records = store
        .list_memories(subject_id)
        .map_err(subjektiv_store_error)?
        .into_iter()
        .filter(|record| {
            kind_filter.contains(record.kind.as_str())
                && state_filter.contains(subjektiv_memory_state_name(record.state))
                && query.as_ref().is_none_or(|query| {
                    record.claim.to_lowercase().contains(query)
                        || record.body_md.to_lowercase().contains(query)
                })
        })
        .collect::<Vec<_>>();
    if offset > records.len() {
        return Err(OperationError::InvalidInput(
            "subjektiv query cursor offset is invalid".into(),
        )
        .into());
    }
    let end = offset.saturating_add(limit).min(records.len());
    let has_more = end < records.len();
    let items = records[offset..end]
        .iter()
        .map(|record| server_api::SubjektivMemoryQueryItem {
            id: record.id.clone(),
            revision: record.revision,
            kind: record.kind.clone(),
            state: api_memory_state(record.state),
            claim: record.claim.clone(),
            excerpt: bounded_memory_excerpt(&record.body_md, 240),
            updated_at: record.updated_at.clone(),
        })
        .collect();
    let next_cursor = has_more
        .then(|| {
            encode_subjektiv_cursor(
                "query",
                &SubjektivQueryCursor {
                    subject_id: subject_id.to_string(),
                    store_revision: subject.store_revision,
                    query,
                    kinds,
                    states,
                    offset: end,
                },
            )
        })
        .transpose()?;
    Ok(server_api::SubjektivMemoryQueryResponse {
        items,
        next_cursor,
        has_more,
    })
}

pub fn subjektiv_memory_read(
    store: &crate::SubjektivStore,
    subject_id: &str,
    input: server_api::SubjektivMemoryReadRequest,
) -> OperationResult<server_api::SubjektivMemoryReadResponse> {
    let current = store
        .scoped_memory(subject_id, &input.memory_id)
        .map_err(subjektiv_store_error)?;
    let record = match input.revision {
        Some(0) => {
            return Err(
                OperationError::InvalidInput("revision must be a positive integer".into()).into(),
            );
        }
        Some(revision) => store
            .scoped_memory_revision(subject_id, &input.memory_id, revision)
            .map_err(subjektiv_store_error)?
            .ok_or_else(|| {
                OperationError::InvalidInput(format!(
                    "memory_revision_not_found: Memory `{}` has no revision {revision}",
                    input.memory_id
                ))
            })?,
        None => current.clone(),
    };
    let body_limit = input.limit.unwrap_or(SUBJEKTIV_BODY_DEFAULT_LINES);
    if body_limit == 0 || body_limit > SUBJEKTIV_BODY_MAX_LINES {
        return Err(OperationError::InvalidInput(format!(
            "body line limit must be within 1..={SUBJEKTIV_BODY_MAX_LINES}"
        ))
        .into());
    }
    let body_offset = input.offset.unwrap_or(0);
    let body_byte_offset = input.byte_offset.unwrap_or(0);
    if (body_offset > 0 || body_byte_offset > 0) && input.revision.is_none() {
        return Err(OperationError::InvalidInput(
            "body continuation requires the exact revision returned by the first read".into(),
        )
        .into());
    }
    let lines = memory_body_lines(&record.body_md);
    if body_offset > lines.len() {
        return Err(OperationError::InvalidInput(
            "body line offset exceeds document length".into(),
        )
        .into());
    }
    if body_offset == lines.len() && body_byte_offset > 0 {
        return Err(OperationError::InvalidInput(
            "body byte offset exceeds document length".into(),
        )
        .into());
    }
    if let Some(line) = lines.get(body_offset)
        && (body_byte_offset > line.len() || !line.is_char_boundary(body_byte_offset))
    {
        return Err(OperationError::InvalidInput(
            "body byte offset must be a UTF-8 boundary within the selected line".into(),
        )
        .into());
    }
    let (body_md, body_next) = bounded_memory_body_page(
        &lines,
        body_offset,
        body_byte_offset,
        body_limit,
        SUBJEKTIV_BODY_MAX_BYTES,
    );
    let body_truncated = body_next.is_some();
    let (body_next_offset, body_next_byte_offset) = body_next
        .map(|(line, byte)| (Some(line), Some(byte)))
        .unwrap_or((None, None));

    let (evidence_offset, nested_evidence_offset, nested_source_ref_offset) =
        if let Some(cursor) = input.evidence_cursor {
            let cursor: SubjektivEvidenceCursor = decode_subjektiv_cursor("evidence", &cursor)?;
            if cursor.subject_id != subject_id
                || cursor.memory_id != record.id
                || cursor.revision != record.revision
            {
                return Err(OperationError::InvalidInput(
                    "subjektiv evidence cursor does not match subject, Memory, or revision".into(),
                )
                .into());
            }
            (
                cursor.offset,
                cursor.evidence_offset,
                cursor.source_ref_offset,
            )
        } else {
            (0, 0, 0)
        };
    let total_evidence = record
        .source_candidate_ids
        .len()
        .saturating_add(record.derived_from.len());
    if evidence_offset > total_evidence {
        return Err(OperationError::InvalidInput(
            "subjektiv evidence cursor offset is invalid".into(),
        )
        .into());
    }

    let mut source_candidate_ids = Vec::new();
    let mut source_candidates = Vec::new();
    let mut derived_from = Vec::new();
    let mut next_evidence_offset = evidence_offset;
    let mut next_nested_evidence_offset = 0;
    let mut next_nested_source_ref_offset = 0;
    if let Some(candidate_id) = record.source_candidate_ids.get(evidence_offset) {
        let candidate = store
            .staging_candidate(subject_id, candidate_id)
            .map_err(subjektiv_store_error)?
            .ok_or_else(|| {
                OperationError::Store(format!(
                    "subjektiv candidate provenance `{candidate_id}` is missing"
                ))
            })?;
        let evidence_total = candidate.evidence.len();
        let source_refs_total = candidate.source_refs.len();
        if nested_evidence_offset > evidence_total || nested_source_ref_offset > source_refs_total {
            return Err(OperationError::InvalidInput(
                "subjektiv nested evidence cursor offset is invalid".into(),
            )
            .into());
        }
        let evidence_end = nested_evidence_offset
            .saturating_add(SUBJEKTIV_EVIDENCE_ANCHOR_PAGE_SIZE)
            .min(evidence_total);
        let source_ref_end = nested_source_ref_offset
            .saturating_add(SUBJEKTIV_EVIDENCE_ANCHOR_PAGE_SIZE)
            .min(source_refs_total);
        source_candidate_ids.push(candidate_id.clone());
        source_candidates.push(server_api::SubjektivMemoryEvidenceCandidate {
            candidate_id: candidate.id,
            evidence: candidate
                .evidence
                .into_iter()
                .skip(nested_evidence_offset)
                .take(evidence_end - nested_evidence_offset)
                .map(bounded_subjektiv_staging_evidence)
                .collect(),
            evidence_total,
            evidence_truncated: nested_evidence_offset > 0 || evidence_end < evidence_total,
            source_refs: candidate
                .source_refs
                .into_iter()
                .skip(nested_source_ref_offset)
                .take(source_ref_end - nested_source_ref_offset)
                .map(bounded_subjektiv_source_ref)
                .collect(),
            source_refs_total,
            source_refs_truncated: nested_source_ref_offset > 0
                || source_ref_end < source_refs_total,
        });
        if evidence_end < evidence_total || source_ref_end < source_refs_total {
            next_nested_evidence_offset = evidence_end;
            next_nested_source_ref_offset = source_ref_end;
        } else {
            next_evidence_offset += 1;
        }
    } else if evidence_offset < total_evidence {
        if nested_evidence_offset != 0 || nested_source_ref_offset != 0 {
            return Err(OperationError::InvalidInput(
                "subjektiv derivation cursor cannot contain nested offsets".into(),
            )
            .into());
        }
        let derived_index = evidence_offset - record.source_candidate_ids.len();
        let reference = &record.derived_from[derived_index];
        derived_from.push(server_api::SubjektivMemoryRevisionRef {
            memory_id: reference.memory_id.clone(),
            revision: reference.revision,
        });
        next_evidence_offset += 1;
    } else if nested_evidence_offset != 0 || nested_source_ref_offset != 0 {
        return Err(OperationError::InvalidInput(
            "subjektiv evidence cursor points beyond the final reference".into(),
        )
        .into());
    }
    bound_subjektiv_nested_evidence(&source_candidates)?;
    let evidence_has_more = next_evidence_offset < total_evidence
        || next_nested_evidence_offset != 0
        || next_nested_source_ref_offset != 0;
    let evidence_next_cursor = evidence_has_more
        .then(|| {
            encode_subjektiv_cursor(
                "evidence",
                &SubjektivEvidenceCursor {
                    subject_id: subject_id.to_string(),
                    memory_id: record.id.clone(),
                    revision: record.revision,
                    offset: next_evidence_offset,
                    evidence_offset: next_nested_evidence_offset,
                    source_ref_offset: next_nested_source_ref_offset,
                },
            )
        })
        .transpose()?;

    let mut response = server_api::SubjektivMemoryReadResponse {
        memory_id: record.id,
        revision: record.revision,
        current_revision: current.revision,
        kind: record.kind,
        state: api_memory_state(record.state),
        claim: record.claim,
        body_md,
        why_useful: record.why_useful,
        staleness: record.staleness,
        change_reason: record.change_reason,
        created_at: record.created_at,
        updated_at: record.updated_at,
        body_offset,
        body_byte_offset,
        body_next_offset,
        body_next_byte_offset,
        body_truncated,
        source_candidate_ids,
        source_candidates,
        derived_from,
        evidence_next_cursor,
        evidence_has_more,
    };
    fit_subjektiv_read_body_to_output_budget(
        &mut response,
        &lines,
        body_offset,
        body_byte_offset,
        body_limit,
    )?;
    Ok(response)
}

pub fn subjektiv_memory_list_revisions(
    store: &crate::SubjektivStore,
    subject_id: &str,
    input: server_api::SubjektivMemoryListRevisionsRequest,
) -> OperationResult<server_api::SubjektivMemoryListRevisionsResponse> {
    let limit = bounded_subjektiv_limit(input.limit, SUBJEKTIV_QUERY_DEFAULT_LIMIT)?;
    let current = store
        .scoped_memory(subject_id, &input.memory_id)
        .map_err(subjektiv_store_error)?;
    let (max_revision, offset) = if let Some(cursor) = input.cursor {
        let cursor: SubjektivRevisionCursor = decode_subjektiv_cursor("revisions", &cursor)?;
        if cursor.subject_id != subject_id || cursor.memory_id != input.memory_id {
            return Err(OperationError::InvalidInput(
                "subjektiv revision cursor does not match subject or Memory".into(),
            )
            .into());
        }
        (cursor.max_revision, cursor.offset)
    } else {
        (current.revision, 0)
    };
    let records = store
        .list_memory_revisions(subject_id, &input.memory_id)
        .map_err(subjektiv_store_error)?
        .into_iter()
        .filter(|record| record.revision <= max_revision)
        .collect::<Vec<_>>();
    if offset > records.len() {
        return Err(OperationError::InvalidInput(
            "subjektiv revision cursor offset is invalid".into(),
        )
        .into());
    }
    let end = offset.saturating_add(limit).min(records.len());
    let has_more = end < records.len();
    let items = records[offset..end]
        .iter()
        .map(|record| server_api::SubjektivMemoryRevisionItem {
            revision: record.revision,
            kind: record.kind.clone(),
            state: api_memory_state(record.state),
            claim: record.claim.clone(),
            change_reason: record.change_reason.clone(),
            updated_at: record.updated_at.clone(),
        })
        .collect();
    let next_cursor = has_more
        .then(|| {
            encode_subjektiv_cursor(
                "revisions",
                &SubjektivRevisionCursor {
                    subject_id: subject_id.to_string(),
                    memory_id: input.memory_id.clone(),
                    max_revision,
                    offset: end,
                },
            )
        })
        .transpose()?;
    Ok(server_api::SubjektivMemoryListRevisionsResponse {
        memory_id: input.memory_id,
        current_revision: current.revision,
        items,
        next_cursor,
        has_more,
    })
}

pub fn subjektiv_memory_validate_proposal(
    store: &crate::SubjektivStore,
    subject_id: &str,
    input: server_api::SubjektivMemoryValidateProposalRequest,
) -> OperationResult<server_api::SubjektivMemoryProposalValidationResponse> {
    if input.expected_revision == 0 {
        return Err(
            OperationError::InvalidInput("expected_revision must be positive".into()).into(),
        );
    }
    let current = store
        .scoped_memory(subject_id, &input.memory_id)
        .map_err(subjektiv_store_error)?;
    if current.revision != input.expected_revision {
        return Err(OperationError::Conflict(format!(
            "revision_conflict: Memory `{}` expected {}, current {}",
            input.memory_id, input.expected_revision, current.revision
        ))
        .into());
    }
    validate_subjektiv_proposal_transition(current.state, input.intent)?;
    Ok(server_api::SubjektivMemoryProposalValidationResponse {
        memory_id: input.memory_id,
        current_revision: current.revision,
        kind: current.kind,
        state: api_memory_state(current.state),
        intent: input.intent,
    })
}

pub fn validate_subjektiv_receipt_id(receipt_id: &str) -> OperationResult<()> {
    let receipt_id = receipt_id.trim();
    if receipt_id.is_empty() || receipt_id.len() > 512 || receipt_id.chars().any(char::is_control) {
        return Err(
            OperationError::InvalidInput("explicit Memory receipt_id is invalid".into()).into(),
        );
    }
    Ok(())
}

pub fn subjektiv_memory_stage_explicit(
    store: &crate::SubjektivStore,
    mut input: server_api::SubjektivMemoryStageExplicitRequest,
    attribution: crate::SubjectSessionAttribution,
) -> OperationResult<server_api::SubjektivMemoryStageExplicitResponse> {
    validate_subjektiv_receipt_id(&input.receipt_id)?;
    let receipt_id = input.receipt_id.trim();
    if input.evidence.is_empty() || input.source_refs.is_empty() {
        return Err(OperationError::InvalidInput(
            "explicit Memory staging requires host-resolved committed evidence".into(),
        )
        .into());
    }
    if input.evidence.len() > SUBJEKTIV_CANDIDATE_ANCHOR_LIMIT
        || input.source_refs.len() > SUBJEKTIV_CANDIDATE_ANCHOR_LIMIT
    {
        return Err(OperationError::InvalidInput(format!(
            "explicit Memory evidence/source_refs are limited to {SUBJEKTIV_CANDIDATE_ANCHOR_LIMIT} items each"
        ))
        .into());
    }
    if matches!(input.kind, memory::extract::CandidateKind::Preference)
        && input.evidence.iter().any(|evidence| {
            !matches!(
                evidence.origin.as_ref().map(|origin| &origin.kind),
                Some(memory::schema::EvidenceOriginKind::HumanInput)
            )
        })
    {
        return Err(OperationError::InvalidInput(
            "preference candidates require exclusively HumanInput evidence".into(),
        )
        .into());
    }
    let mut range_start = u64::MAX;
    let mut range_end = 0;
    let mut segment_id = None;
    for source_ref in &mut input.source_refs {
        if source_ref
            .session_id
            .as_deref()
            .is_some_and(|value| value != attribution.session_id)
        {
            return Err(OperationError::InvalidInput(
                "explicit candidate source belongs to a different Session".into(),
            )
            .into());
        }
        source_ref.session_id = Some(attribution.session_id.clone());
        let source_segment = source_ref.segment_id.as_deref().ok_or_else(|| {
            OperationError::InvalidInput("explicit candidate source is missing segment_id".into())
        })?;
        if segment_id
            .as_deref()
            .is_some_and(|value| value != source_segment)
        {
            return Err(OperationError::InvalidInput(
                "explicit candidate evidence spans multiple Session segments".into(),
            )
            .into());
        }
        segment_id = Some(source_segment.to_string());
        let range = source_ref.entry_range.ok_or_else(|| {
            OperationError::InvalidInput("explicit candidate source is missing entry_range".into())
        })?;
        range_start = range_start.min(range[0]);
        range_end = range_end.max(range[1]);
    }
    let source = memory::schema::SourceRef {
        segment_id: segment_id.ok_or_else(|| {
            OperationError::InvalidInput("explicit candidate source is missing segment_id".into())
        })?,
        range: [range_start, range_end],
    };
    let proposal = input
        .proposal
        .as_ref()
        .map(|proposal| {
            crate::RevisionProposal::new(
                domain_proposal_intent(proposal.intent),
                proposal.memory_id.clone(),
                proposal.expected_revision,
                proposal.change_reason.clone(),
            )
        })
        .transpose()
        .map_err(subjektiv_store_error)?;
    if let Some(proposal) = &proposal {
        let current = store
            .scoped_memory(&attribution.subject_id, &proposal.memory_id)
            .map_err(subjektiv_store_error)?;
        if current.kind != input.kind {
            return Err(OperationError::InvalidInput(
                "revision proposal kind must be derived from its target Memory".into(),
            )
            .into());
        }
    }
    let candidate = memory::extract::ExtractedCandidate {
        kind: input.kind,
        claim: input.claim,
        why_useful: input.why_useful,
        staleness: input.staleness,
        evidence_ids: input.evidence.iter().map(|item| item.id.clone()).collect(),
    };
    let staging = memory::extract::StagingRecord::from_candidate(
        receipt_id,
        format!("explicit:{receipt_id}"),
        source,
        candidate,
        input.evidence,
        input.source_refs,
    );
    let mut staging = crate::SubjectStagingRecord::attach(&attribution.subject_id, staging);
    if let Some(proposal) = proposal.clone() {
        staging
            .attach_revision_proposal(proposal)
            .map_err(subjektiv_store_error)?;
    }
    let (staged, _) = store
        .stage_candidate_with_attribution(staging, attribution)
        .map_err(subjektiv_store_error)?;
    Ok(server_api::SubjektivMemoryStageExplicitResponse {
        candidate_id: staged.id,
        receipt_id: receipt_id.to_string(),
        status: server_api::SubjektivMemoryReceiptStatus::Staged,
        target: proposal
            .as_ref()
            .map(|proposal| server_api::SubjektivMemoryProposalTarget {
                memory_id: proposal.memory_id.clone(),
                expected_revision: proposal.expected_revision,
            }),
        intent: proposal
            .as_ref()
            .map(|proposal| api_proposal_intent(proposal.intent)),
    })
}

pub fn validate_subjektiv_proposal_transition(
    state: crate::MemoryState,
    intent: server_api::SubjektivMemoryRevisionIntent,
) -> OperationResult<()> {
    use crate::MemoryState::{Active, Resolved};
    use server_api::SubjektivMemoryRevisionIntent::{Reopen, Resolve, Retract, Revise};
    let valid = match intent {
        Revise => matches!(state, Active | Resolved),
        Resolve => state == Active,
        Retract => matches!(state, Active | Resolved),
        Reopen => state == Resolved,
    };
    if valid {
        Ok(())
    } else {
        Err(OperationError::InvalidInput(format!(
            "invalid_state_transition: {state:?} cannot accept {intent:?}"
        ))
        .into())
    }
}

pub fn canonical_subjektiv_kinds(
    kinds: Option<Vec<memory::extract::CandidateKind>>,
) -> OperationResult<Vec<String>> {
    let kinds = kinds.unwrap_or_else(|| {
        use memory::extract::CandidateKind::*;
        vec![
            Preference,
            WorkingAssumption,
            Constraint,
            Decision,
            OpenQuestion,
            Lesson,
        ]
    });
    if kinds.is_empty() {
        return Err(OperationError::InvalidInput("kinds must not be an empty array".into()).into());
    }
    let mut names = kinds
        .iter()
        .map(|kind| kind.as_str().to_string())
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    Ok(names)
}

pub fn canonical_subjektiv_states(
    states: Option<Vec<server_api::SubjektivMemoryState>>,
) -> OperationResult<Vec<String>> {
    let states = states.unwrap_or_else(|| vec![server_api::SubjektivMemoryState::Active]);
    if states.is_empty() {
        return Err(
            OperationError::InvalidInput("states must not be an empty array".into()).into(),
        );
    }
    let mut names = states
        .into_iter()
        .map(|state| match state {
            server_api::SubjektivMemoryState::Active => "active".to_string(),
            server_api::SubjektivMemoryState::Resolved => "resolved".to_string(),
            server_api::SubjektivMemoryState::Retracted => "retracted".to_string(),
        })
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    Ok(names)
}

pub fn bounded_subjektiv_limit(limit: Option<usize>, default: usize) -> OperationResult<usize> {
    let limit = limit.unwrap_or(default);
    if limit == 0 || limit > SUBJEKTIV_QUERY_MAX_LIMIT {
        return Err(OperationError::InvalidInput(format!(
            "limit must be within 1..={SUBJEKTIV_QUERY_MAX_LIMIT}"
        ))
        .into());
    }
    Ok(limit)
}

pub fn api_memory_state(state: crate::MemoryState) -> server_api::SubjektivMemoryState {
    match state {
        crate::MemoryState::Active => server_api::SubjektivMemoryState::Active,
        crate::MemoryState::Resolved => server_api::SubjektivMemoryState::Resolved,
        crate::MemoryState::Retracted => server_api::SubjektivMemoryState::Retracted,
    }
}

pub fn subjektiv_memory_state_name(state: crate::MemoryState) -> &'static str {
    match state {
        crate::MemoryState::Active => "active",
        crate::MemoryState::Resolved => "resolved",
        crate::MemoryState::Retracted => "retracted",
    }
}

pub fn domain_proposal_intent(
    intent: server_api::SubjektivMemoryRevisionIntent,
) -> crate::RevisionProposalIntent {
    match intent {
        server_api::SubjektivMemoryRevisionIntent::Revise => crate::RevisionProposalIntent::Revise,
        server_api::SubjektivMemoryRevisionIntent::Resolve => {
            crate::RevisionProposalIntent::Resolve
        }
        server_api::SubjektivMemoryRevisionIntent::Retract => {
            crate::RevisionProposalIntent::Retract
        }
        server_api::SubjektivMemoryRevisionIntent::Reopen => crate::RevisionProposalIntent::Reopen,
    }
}

pub fn api_proposal_intent(
    intent: crate::RevisionProposalIntent,
) -> server_api::SubjektivMemoryRevisionIntent {
    match intent {
        crate::RevisionProposalIntent::Revise => server_api::SubjektivMemoryRevisionIntent::Revise,
        crate::RevisionProposalIntent::Resolve => {
            server_api::SubjektivMemoryRevisionIntent::Resolve
        }
        crate::RevisionProposalIntent::Retract => {
            server_api::SubjektivMemoryRevisionIntent::Retract
        }
        crate::RevisionProposalIntent::Reopen => server_api::SubjektivMemoryRevisionIntent::Reopen,
    }
}

pub fn fit_subjektiv_read_body_to_output_budget(
    response: &mut server_api::SubjektivMemoryReadResponse,
    lines: &[&str],
    line_offset: usize,
    byte_offset: usize,
    line_limit: usize,
) -> OperationResult<()> {
    let serialized_len = |response: &server_api::SubjektivMemoryReadResponse| {
        serde_json::to_vec_pretty(response)
            .map(|value| value.len())
            .map_err(|error| {
                OperationError::Store(format!("encode subjektiv Memory read: {error}"))
            })
    };
    if serialized_len(response)? <= server_api::SUBJEKTIV_MEMORY_READ_MAX_TOOL_CONTENT_BYTES {
        return Ok(());
    }

    let initial_body_len = response.body_md.len();
    let mut lower = 0usize;
    let mut upper = initial_body_len;
    while lower < upper {
        let candidate_limit = lower + (upper - lower).div_ceil(2);
        set_subjektiv_read_body_page(
            response,
            lines,
            line_offset,
            byte_offset,
            line_limit,
            candidate_limit,
        );
        if serialized_len(response)? <= server_api::SUBJEKTIV_MEMORY_READ_MAX_TOOL_CONTENT_BYTES {
            lower = candidate_limit;
        } else {
            upper = candidate_limit - 1;
        }
    }
    set_subjektiv_read_body_page(response, lines, line_offset, byte_offset, line_limit, lower);
    let final_len = serialized_len(response)?;
    if final_len > server_api::SUBJEKTIV_MEMORY_READ_MAX_TOOL_CONTENT_BYTES {
        return Err(OperationError::Store(format!(
            "subjektiv Memory metadata/provenance requires {final_len} bytes and exceeds the {} byte read-output budget",
            server_api::SUBJEKTIV_MEMORY_READ_MAX_TOOL_CONTENT_BYTES
        ))
        .into());
    }
    if initial_body_len > 0 && response.body_md.is_empty() {
        return Err(OperationError::Store(
            "subjektiv Memory metadata/provenance leaves no room for resumable body content".into(),
        )
        .into());
    }
    Ok(())
}

pub fn set_subjektiv_read_body_page(
    response: &mut server_api::SubjektivMemoryReadResponse,
    lines: &[&str],
    line_offset: usize,
    byte_offset: usize,
    line_limit: usize,
    byte_limit: usize,
) {
    let (body_md, body_next) =
        bounded_memory_body_page(lines, line_offset, byte_offset, line_limit, byte_limit);
    response.body_md = body_md;
    response.body_truncated = body_next.is_some();
    (response.body_next_offset, response.body_next_byte_offset) = body_next
        .map(|(line, byte)| (Some(line), Some(byte)))
        .unwrap_or((None, None));
}

pub fn bounded_memory_body_page(
    lines: &[&str],
    line_offset: usize,
    byte_offset: usize,
    line_limit: usize,
    byte_limit: usize,
) -> (String, Option<(usize, usize)>) {
    let line_end = line_offset.saturating_add(line_limit).min(lines.len());
    let mut line_index = line_offset;
    let mut within_line = byte_offset;
    let mut body = String::new();
    while line_index < line_end {
        let line = lines[line_index];
        let remainder = &line[within_line..];
        let available = byte_limit.saturating_sub(body.len());
        if remainder.len() <= available {
            body.push_str(remainder);
            line_index += 1;
            within_line = 0;
            continue;
        }
        let mut take = available.min(remainder.len());
        while take > 0 && !remainder.is_char_boundary(take) {
            take -= 1;
        }
        if take == 0 {
            return (body, Some((line_index, within_line)));
        }
        body.push_str(&remainder[..take]);
        within_line += take;
        return (body, Some((line_index, within_line)));
    }
    if line_index < lines.len() {
        (body, Some((line_index, 0)))
    } else {
        (body, None)
    }
}

pub fn memory_body_lines(body: &str) -> Vec<&str> {
    if body.is_empty() {
        Vec::new()
    } else {
        body.split_inclusive('\n').collect()
    }
}

pub fn bound_subjektiv_nested_evidence(
    candidates: &[server_api::SubjektivMemoryEvidenceCandidate],
) -> OperationResult<()> {
    let serialized = serde_json::to_vec(candidates).map_err(|error| {
        OperationError::Store(format!("encode subjektiv evidence page: {error}"))
    })?;
    if serialized.len() <= SUBJEKTIV_NESTED_EVIDENCE_MAX_BYTES {
        Ok(())
    } else {
        Err(OperationError::Store(
            "subjektiv evidence page exceeds its enforced response byte budget".into(),
        )
        .into())
    }
}

pub fn bounded_subjektiv_staging_evidence(
    mut evidence: memory::extract::StagingEvidence,
) -> memory::extract::StagingEvidence {
    evidence.kind = memory::schema::EvidenceKind::new(bounded_utf8_bytes(
        evidence.kind.as_str().to_string(),
        SUBJEKTIV_ANCHOR_TEXT_MAX_BYTES,
    ));
    evidence.origin = evidence.origin.map(bounded_subjektiv_evidence_origin);
    evidence.excerpt = evidence
        .excerpt
        .map(|value| bounded_utf8_bytes(value, SUBJEKTIV_ANCHOR_TEXT_MAX_BYTES));
    evidence.summary = evidence
        .summary
        .map(|value| bounded_utf8_bytes(value, SUBJEKTIV_ANCHOR_TEXT_MAX_BYTES));
    evidence
}

pub fn bounded_subjektiv_source_ref(
    mut source: memory::schema::SourceEvidenceRef,
) -> memory::schema::SourceEvidenceRef {
    source.origin = source.origin.map(bounded_subjektiv_evidence_origin);
    source.evidence_kind = source.evidence_kind.map(|kind| {
        memory::schema::EvidenceKind::new(bounded_utf8_bytes(
            kind.as_str().to_string(),
            SUBJEKTIV_ANCHOR_TEXT_MAX_BYTES,
        ))
    });
    source.label = source
        .label
        .map(|value| bounded_utf8_bytes(value, SUBJEKTIV_ANCHOR_TEXT_MAX_BYTES));
    source.summary = source
        .summary
        .map(|value| bounded_utf8_bytes(value, SUBJEKTIV_ANCHOR_TEXT_MAX_BYTES));
    source
}

pub fn bounded_subjektiv_evidence_origin(
    mut origin: memory::schema::EvidenceOrigin,
) -> memory::schema::EvidenceOrigin {
    for field in [
        &mut origin.account_id,
        &mut origin.workspace_id,
        &mut origin.runtime_id,
        &mut origin.worker_id,
        &mut origin.flow_selector,
        &mut origin.flow_definition_id,
    ] {
        if let Some(value) = field.take() {
            *field = Some(bounded_utf8_bytes(value, SUBJEKTIV_ANCHOR_TEXT_MAX_BYTES));
        }
    }
    origin
}

pub fn bounded_utf8_bytes(value: String, max_bytes: usize) -> String {
    let value = value
        .chars()
        .map(|character| {
            if character.is_control() || matches!(character, '"' | '\\') {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    if value.len() <= max_bytes {
        return value;
    }
    let marker = "…";
    let mut end = max_bytes.saturating_sub(marker.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{marker}", &value[..end])
}

pub fn bounded_memory_excerpt(body: &str, max_chars: usize) -> String {
    let normalized = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = normalized.chars();
    let excerpt = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{excerpt}…")
    } else {
        excerpt
    }
}

pub fn encode_subjektiv_cursor<T: Serialize>(kind: &str, value: &T) -> OperationResult<String> {
    let raw = serde_json::to_vec(value)
        .map_err(|error| OperationError::Store(format!("encode subjektiv cursor: {error}")))?;
    let encoded = raw
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!("subjektiv.{kind}.{encoded}"))
}

pub fn decode_subjektiv_cursor<T: serde::de::DeserializeOwned>(
    kind: &str,
    cursor: &str,
) -> OperationResult<T> {
    let prefix = format!("subjektiv.{kind}.");
    let encoded = cursor.strip_prefix(&prefix).ok_or_else(|| {
        OperationError::InvalidInput(format!("invalid subjektiv {kind} cursor type"))
    })?;
    if encoded.is_empty()
        || encoded.len() > 16_384
        || encoded.len() % 2 != 0
        || !encoded.is_ascii()
        || !encoded.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(
            OperationError::InvalidInput(format!("invalid subjektiv {kind} cursor")).into(),
        );
    }
    let mut raw = Vec::with_capacity(encoded.len() / 2);
    for index in (0..encoded.len()).step_by(2) {
        raw.push(
            u8::from_str_radix(&encoded[index..index + 2], 16).map_err(|_| {
                OperationError::InvalidInput(format!("invalid subjektiv {kind} cursor encoding"))
            })?,
        );
    }
    serde_json::from_slice(&raw).map_err(|_| {
        OperationError::InvalidInput(format!("invalid subjektiv {kind} cursor payload")).into()
    })
}

/// Transport-independent error classification. Backend maps these to its existing
/// HTTP errors; local Hosts preserve the same conflict/permission distinctions.
#[derive(Debug, thiserror::Error)]
pub enum OperationError {
    #[error("{0}")]
    InvalidInput(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    PermissionDenied(String),
    #[error("subject not found: {0}")]
    SubjectNotFound(String),
    #[error("{0}")]
    Store(String),
}

pub type OperationResult<T> = std::result::Result<T, OperationError>;

pub fn subjektiv_store_error(error: crate::SubjektivError) -> OperationError {
    match error {
        crate::SubjektivError::SubjectScopeMismatch { .. } => {
            OperationError::PermissionDenied(format!("subject_scope_mismatch: {error}"))
        }
        crate::SubjektivError::RevisionConflict { .. }
        | crate::SubjektivError::SubjectBehaviorConflict { .. }
        | crate::SubjektivError::SurfaceGenerationConflict(_) => {
            OperationError::Conflict(format!("revision_conflict: {error}"))
        }
        crate::SubjektivError::CandidateResolved(_)
        | crate::SubjektivError::DecisionRequestConflict(_) => {
            OperationError::Conflict(format!("candidate_decision_conflict: {error}"))
        }
        crate::SubjektivError::SubjectNotFound(id) => OperationError::SubjectNotFound(id),
        crate::SubjektivError::MemoryNotFound(id) => {
            OperationError::InvalidInput(format!("memory_not_found: Memory `{id}` was not found"))
        }
        crate::SubjektivError::Storage(_) => OperationError::Store(error.to_string()),
        _ => OperationError::InvalidInput(error.to_string()),
    }
}

#[cfg(test)]
mod tests;
