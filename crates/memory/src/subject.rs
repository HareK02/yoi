//! Host communication for Subject-bound Memory candidates and resident surfaces.
//!
//! These values carry no execution authority. Hosts must bind operations to an
//! explicit Subject and validate current grants before accessing storage.
//! Availability is separate from body presence: `Ready` without content is a
//! successfully generated empty surface, not an ungenerated or failed surface.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::extract::{ExtractedCandidate, StagingEvidence};
use crate::schema::{SourceEvidenceRef, SourceRef};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryResidentSummaryAvailability {
    Ready,
    Ungenerated,
    Stale,
    Failed,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryResidentSummaryOutput {
    pub availability: MemoryResidentSummaryAvailability,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
pub struct MemoryResidentSummaryOperation {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryStageCandidateOperation {
    pub source: SourceRef,
    pub extract_run_id: String,
    pub candidate: ExtractedCandidate,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<StagingEvidence>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_refs: Vec<SourceEvidenceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemoryConsolidateStagingOperation {
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryConsolidationOutput {
    pub status: String,
    pub summary: String,
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_usize))]
    pub candidate_count: usize,
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub total_bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::{CandidateKind, StagingRecord};
    use serde_json::json;

    #[test]
    fn resident_availability_survives_without_a_body() {
        for (availability, tag) in [
            (MemoryResidentSummaryAvailability::Ready, "ready"),
            (
                MemoryResidentSummaryAvailability::Ungenerated,
                "ungenerated",
            ),
            (MemoryResidentSummaryAvailability::Stale, "stale"),
            (MemoryResidentSummaryAvailability::Failed, "failed"),
            (
                MemoryResidentSummaryAvailability::Unavailable,
                "unavailable",
            ),
        ] {
            let output = MemoryResidentSummaryOutput {
                availability,
                content: None,
            };
            let value = serde_json::to_value(output).unwrap();
            assert_eq!(value, json!({"availability": tag}));
            let restored: MemoryResidentSummaryOutput = serde_json::from_value(value).unwrap();
            assert_eq!(restored.availability, availability);
            assert!(restored.content.is_none());
        }
    }

    #[test]
    fn subject_candidate_retains_evidence_when_expanded_to_a_record() {
        let operation: MemoryStageCandidateOperation = serde_json::from_value(json!({
            "source": {"segment_id": "segment-1", "range": [10, 12]},
            "extract_run_id": "run-1",
            "candidate": {
                "kind": "constraint", "claim": "Keep Subject ownership explicit",
                "why_useful": "Avoid cross-Subject attribution", "evidence_ids": ["E1"]
            },
            "evidence": [{
                "id": "E1", "kind": "message", "entry_range": [10, 12],
                "origin": {"kind": "human_input"}, "excerpt": "Bounded evidence"
            }],
            "source_refs": [{
                "segment_id": "segment-1", "entry_range": [10, 12], "evidence_id": "E1"
            }]
        }))
        .unwrap();
        assert_eq!(operation.candidate.evidence_ids, ["E1"]);
        let record = StagingRecord::from_candidate(
            "candidate-1",
            operation.extract_run_id,
            operation.source,
            operation.candidate,
            operation.evidence,
            operation.source_refs,
        );
        let restored: StagingRecord =
            serde_json::from_value(serde_json::to_value(record).unwrap()).unwrap();
        assert_eq!(restored.kind, CandidateKind::Constraint);
        assert_eq!(restored.extract_run_id, "run-1");
        assert_eq!(restored.source.segment_id, "segment-1");
        assert_eq!(
            restored.evidence[0].excerpt.as_deref(),
            Some("Bounded evidence")
        );
        assert_eq!(restored.source_refs[0].evidence_id.as_deref(), Some("E1"));
    }
}
