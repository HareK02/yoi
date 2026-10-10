//! Common consolidation Job policy and verified result projections.
//!
//! Hosts authenticate/admit Jobs and bind their immutable batch and attempt.
//! These helpers do not dispatch, replay, or grant Job execution authority.
use serde::Deserialize;

use crate::api::{OperationError, OperationResult};
use crate::{SubjektivStore, SurfaceAvailability};

pub const CANDIDATE_BATCH_LIMIT: usize = 100;
pub const CONSOLIDATION_THRESHOLD_FILES: usize = 5;
pub const CONSOLIDATION_THRESHOLD_BYTES: u64 = 50_000;

/// Selects an ordered prefix using the T-704 complete-result budget. Reserve
/// 2,048 bytes for the Subject/Host surface envelope, then twice the JSON-escaped
/// ID size plus 128 bytes per candidate for its compact verified disposition.
pub fn bounded_candidate_batch(
    ids: impl IntoIterator<Item = String>,
    max_result_bytes: u32,
) -> OperationResult<Vec<String>> {
    let mut estimated_bytes = 2_048_usize;
    let mut selected = Vec::new();
    for id in ids.into_iter().take(CANDIDATE_BATCH_LIMIT) {
        let id_bytes = serde_json::to_vec(&id)
            .map_err(|e| OperationError::InvalidInput(e.to_string()))?
            .len();
        let additional = 2 * id_bytes + 128;
        if estimated_bytes + additional > max_result_bytes as usize {
            break;
        }
        estimated_bytes += additional;
        selected.push(id);
    }
    Ok(selected)
}

/// Empty ready Subjects need no work, even when forced. Missing/stale/failed
/// surfaces bypass candidate thresholds; confirmed Memory is never discarded.
pub fn consolidation_required(
    candidate_count: usize,
    total_bytes: u64,
    force: bool,
    surface_availability: SurfaceAvailability,
) -> bool {
    let surface_needs_generation = surface_availability != SurfaceAvailability::Ready;
    if candidate_count == 0 {
        return surface_needs_generation;
    }
    force
        || surface_needs_generation
        || candidate_count >= CONSOLIDATION_THRESHOLD_FILES
        || total_bytes >= CONSOLIDATION_THRESHOLD_BYTES
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConsolidationResult {
    subject_id: String,
    candidate_ids: Vec<String>,
    surface: SurfaceResult,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SurfaceResult {
    availability: String,
    generation_id: String,
    memory_fingerprint: String,
    #[serde(default)]
    snapshot_id: Option<String>,
    #[serde(default)]
    reason_code: Option<String>,
}

/// Verifies the exact Subject/batch, every immutable disposition, and the actual
/// terminal surface outcome begun by this Job attempt. Model claims alone never
/// establish completion. Same admission semantics as the Backend T-704 adapter.
pub fn validate_result(
    store: &SubjektivStore,
    subject_id: &str,
    batch: &[String],
    job_id: &str,
    attempt_id: &str,
    value: &serde_json::Value,
) -> OperationResult<()> {
    let result: ConsolidationResult = serde_json::from_value(value.clone())
        .map_err(|e| OperationError::InvalidInput(format!("invalid consolidation result: {e}")))?;
    if result.subject_id != subject_id || result.candidate_ids != batch {
        return Err(OperationError::InvalidInput(
            "consolidation result does not match the immutable Subject/batch grant".into(),
        ));
    }
    for candidate_id in batch {
        if store
            .staging_resolution(subject_id, candidate_id)
            .map_err(|e| OperationError::Store(e.to_string()))?
            .is_none()
        {
            return Err(OperationError::InvalidInput(format!(
                "consolidation candidate `{candidate_id}` is unresolved"
            )));
        }
    }
    store
        .validate_job_surface_outcome(
            subject_id,
            &result.surface.generation_id,
            result.surface.memory_fingerprint,
            &result.surface.availability,
            result.surface.snapshot_id.as_deref(),
            result.surface.reason_code.as_deref(),
            job_id,
            attempt_id,
        )
        .map_err(|e| {
            OperationError::InvalidInput(format!("consolidation surface result mismatch: {e}"))
        })
}

/// Enriches a previously verified result with compact durable dispositions. Full
/// reasons, affected changes and provenance remain in the receipt store. This
/// deterministic projection neither applies candidates nor creates a new ledger.
pub fn with_dispositions(
    store: &SubjektivStore,
    subject_id: &str,
    batch: &[String],
    value: &serde_json::Value,
) -> OperationResult<serde_json::Value> {
    let mut dispositions = Vec::new();
    for id in batch {
        let resolution = store
            .staging_resolution(subject_id, id)
            .map_err(|e| OperationError::Store(e.to_string()))?
            .ok_or_else(|| {
                OperationError::InvalidInput(format!("candidate `{id}` has no durable disposition"))
            })?;
        dispositions.push(serde_json::json!({"candidate_id": id, "action": resolution.action}));
    }
    let mut enriched = value.clone();
    enriched
        .as_object_mut()
        .ok_or_else(|| {
            OperationError::InvalidInput("consolidation result must be an object".into())
        })?
        .insert(
            "candidate_dispositions".into(),
            serde_json::Value::Array(dispositions),
        );
    Ok(enriched)
}

#[cfg(test)]
mod tests;
