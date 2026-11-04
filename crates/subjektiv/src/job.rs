//! Shared consolidation Job selection rules and validation against stored results.
//!
//! The Host owns authentication/admission and pins immutable batches/attempts.
//! This module neither dispatches nor reruns Jobs, and grants no execution authority from Profile names or model claims.
//! Validates Memory candidate processing and surface preparation separately; failure of the latter never revokes the former.
use serde::Deserialize;

use crate::api::{OperationError, OperationResult};
use crate::{SubjektivStore, SurfaceAvailability};

pub const CANDIDATE_BATCH_LIMIT: usize = 100;
pub const CONSOLIDATION_THRESHOLD_FILES: usize = 5;
pub const CONSOLIDATION_THRESHOLD_BYTES: u64 = 50_000;

/// Selects an ordered prefix fitting the result budget.
/// Estimates the Subject/surface envelope and JSON-escaped IDs appearing in both
/// batch declarations and validated dispositions. Overflow candidates stay for the next batch; existing batches never expand.
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

/// With no candidates and a ready surface, skips even when forced; ungenerated/stale/failed surfaces need generation regardless of candidate thresholds.
/// This is not a decision to delete committed Memory.
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

/// Validates the exact Subject/batch, immutable dispositions for all candidates, and that Job attempt's stored terminal surface outcome.
/// Rejects unresolved candidates, stale input, generation from another attempt, and fabricated results.
/// Model final text or Idle is not completion evidence; this validation also does not replace Host Job-result acceptance.
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

/// Reconstructs only a stored terminal outcome for this exact Job/attempt.
/// Missing work returns `None`; stale, erased, retired or inconsistent evidence is
/// an error. No candidate decision, generation, model call or finalizer is rerun.
/// Hosts still fence current grants and independently retain execution cleanup.
pub fn lookup_result(
    store: &SubjektivStore,
    subject_id: &str,
    batch: &[String],
    job_id: &str,
    attempt_id: &str,
) -> OperationResult<Option<serde_json::Value>> {
    use crate::SubjektivError;
    use rusqlite::{OptionalExtension, params};

    if batch.len() > CANDIDATE_BATCH_LIMIT {
        return Err(OperationError::InvalidInput(
            "consolidation batch exceeds limit".into(),
        ));
    }
    let result = store.database.try_transaction(|tx| {
        crate::require_active_subject(tx, subject_id)?;
        crate::validate_label("Job id", job_id)?;
        crate::validate_label("attempt id", attempt_id)?;
        let mut complete = true;
        for id in batch {
            let candidate: Option<String> = tx.query_row(
                "SELECT record_json FROM staging_records WHERE subject_id=?1 AND candidate_id=?2",
                params![subject_id, id], |row| row.get(0)
            ).optional()?;
            let candidate = candidate.ok_or_else(|| SubjektivError::CandidateNotFound(id.clone()))?;
            crate::parse_staging(store.workspace_id(), &candidate)?;
            let resolution: Option<String> = tx.query_row(
                "SELECT resolution_json FROM staging_resolutions WHERE subject_id=?1 AND candidate_id=?2",
                params![subject_id, id], |row| row.get(0)
            ).optional()?;
            match resolution {
                Some(raw) => {
                    let saved = crate::parse_resolution(&raw)?;
                    if saved.subject_id != subject_id || saved.candidate_id != *id {
                        return Err(SubjektivError::InvalidRecord("consolidation disposition identity mismatch".into()));
                    }
                }
                None => complete = false,
            }
        }
        // Use the existing immutable generation binding and terminal fingerprint
        // contract, not arrival time or a newly prepared generation. Several
        // preparations for the same input may share a stored outcome.
        let fingerprint = crate::memory_fingerprint_in_connection(tx, subject_id)?;
        let generation: Option<String> = tx.query_row(
            "SELECT generation_id FROM surface_generation_runs
             WHERE subject_id=?1 AND job_id=?2 AND attempt_id=?3 AND memory_fingerprint=?4
             ORDER BY generation_id LIMIT 1",
            params![subject_id, job_id, attempt_id, fingerprint], |row| row.get(0)
        ).optional()?;
        let Some(generation_id) = generation else {
            let prior = tx.prepare(
                "SELECT 1 FROM surface_generation_runs WHERE subject_id=?1 AND job_id=?2 AND attempt_id=?3"
            )?.exists(params![subject_id, job_id, attempt_id])?;
            if prior {
                return Err(SubjektivError::InvalidRecord("consolidation generation input is stale".into()));
            }
            return Ok(None);
        };
        let state: Option<(String, String, Option<String>, Option<String>)> = tx.query_row(
            "SELECT memory_fingerprint,status,snapshot_id,reason_code FROM surface_generation_state WHERE subject_id=?1",
            [subject_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))
        ).optional()?;
        let Some((saved_fingerprint, status, snapshot_id, reason_code)) = state else { return Ok(None) };
        // Memory writes mark this exact input dirty before the producer can
        // publish its terminal receipt. That is unfinished work, not failure.
        if saved_fingerprint == fingerprint && status == "dirty"
            && snapshot_id.is_none() && reason_code.is_none()
        {
            return Ok(None);
        }
        if saved_fingerprint != fingerprint || !matches!(status.as_str(), "ready" | "failed") {
            return Err(SubjektivError::InvalidRecord("consolidation terminal outcome is stale or inconsistent".into()));
        }
        if !complete { return Ok(None) }
        let mut surface = serde_json::json!({
            "availability": status, "generation_id": generation_id, "memory_fingerprint": fingerprint
        });
        if let Some(id) = snapshot_id { surface["snapshot_id"] = id.into(); }
        if let Some(reason) = reason_code { surface["reason_code"] = reason.into(); }
        Ok(Some(serde_json::json!({"subject_id":subject_id, "candidate_ids":batch,"surface":surface})))
    }).map_err(|error: SubjektivError| OperationError::Store(error.to_string()))?;
    let Some(result) = result else {
        return Ok(None);
    };
    validate_result(store, subject_id, batch, job_id, attempt_id, &result)?;
    with_dispositions(store, subject_id, batch, &result).map(Some)
}

/// Adds a compact projection of stored dispositions to a validated result.
/// Reasons, change references, and provenance stay in the receipt store, not duplicated in bounded Job results.
/// Neither reapplies candidates nor creates a new ledger.
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
