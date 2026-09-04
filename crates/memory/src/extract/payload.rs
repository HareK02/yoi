//! extract staging schema.
//!
//! Extract produces memory-candidate records, not activity-log batches. During
//! the transitional `write_extracted` path the model submits an
//! [`ExtractedPayload`] containing `candidates[]`; the host expands each
//! candidate into one [`StagingRecord`] and attaches record ids, extract-run ids,
//! record-level source, evidence snippets, and source anchors mechanically.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::schema::{EvidenceKind, EvidenceOrigin, SourceEvidenceRef, SourceRef};

/// Current flat staging schema version.
pub const STAGING_SCHEMA_VERSION: u32 = 2;

/// Candidate kinds that extract is allowed to stage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CandidateKind {
    Preference,
    WorkingAssumption,
    Constraint,
    Decision,
    OpenQuestion,
    Lesson,
}

impl CandidateKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            CandidateKind::Preference => "preference",
            CandidateKind::WorkingAssumption => "working_assumption",
            CandidateKind::Constraint => "constraint",
            CandidateKind::Decision => "decision",
            CandidateKind::OpenQuestion => "open_question",
            CandidateKind::Lesson => "lesson",
        }
    }
}

/// Model-submitted candidate before host-side staging metadata is attached.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExtractedCandidate {
    /// Kind of candidate. This is intentionally a narrow taxonomy, not an
    /// activity-log category.
    pub kind: CandidateKind,
    /// Concise candidate claim.
    pub claim: String,
    /// Why this may be useful for future work or consolidation.
    pub why_useful: String,
    /// Optional invalidation / staleness hint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staleness: Option<String>,
    /// Evidence ids selected by the extract worker. The transitional
    /// `write_extracted` path may leave this empty until `session-explore`
    /// provides host-issued evidence ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_ids: Vec<String>,
}

/// Transitional extract output: zero or more flat candidates.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ExtractedPayload {
    #[serde(default)]
    pub candidates: Vec<ExtractedCandidate>,
}

impl ExtractedPayload {
    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }
}

/// Bounded evidence snippet copied into a flat staging record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StagingEvidence {
    pub id: String,
    pub kind: EvidenceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_range: Option<[u64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<EvidenceOrigin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excerpt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// One flat staging record. One record is one consolidation decision unit.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StagingRecord {
    pub schema_version: u32,
    pub id: String,
    pub extract_run_id: String,
    pub source: SourceRef,
    pub kind: CandidateKind,
    pub claim: String,
    pub why_useful: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staleness: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<StagingEvidence>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_refs: Vec<SourceEvidenceRef>,
}

impl StagingRecord {
    pub fn from_candidate(
        id: impl Into<String>,
        extract_run_id: impl Into<String>,
        source: SourceRef,
        candidate: ExtractedCandidate,
        evidence: Vec<StagingEvidence>,
        source_refs: Vec<SourceEvidenceRef>,
    ) -> Self {
        Self {
            schema_version: STAGING_SCHEMA_VERSION,
            id: id.into(),
            extract_run_id: extract_run_id.into(),
            source,
            kind: candidate.kind,
            claim: candidate.claim,
            why_useful: candidate.why_useful,
            staleness: candidate.staleness,
            evidence,
            source_refs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> SourceRef {
        SourceRef {
            segment_id: "segment-record".into(),
            range: [0, 20],
        }
    }

    #[test]
    fn extracted_payload_empty_when_no_candidates() {
        assert!(ExtractedPayload::default().is_empty());
        let payload = ExtractedPayload {
            candidates: vec![ExtractedCandidate {
                kind: CandidateKind::Decision,
                claim: "Use flat staging records".into(),
                why_useful: "Consolidation can resolve candidates independently".into(),
                staleness: None,
                evidence_ids: Vec::new(),
            }],
        };
        assert!(!payload.is_empty());
    }

    #[test]
    fn flat_staging_record_roundtrips() {
        let evidence = StagingEvidence {
            id: "E001".into(),
            kind: EvidenceKind::new(EvidenceKind::MESSAGE),
            entry_range: Some([10, 12]),
            origin: None,
            excerpt: Some("extract candidate taxonomy".into()),
            summary: Some("User and assistant discussed staging kinds".into()),
        };
        let source_ref = SourceEvidenceRef {
            segment_id: Some("segment-1".into()),
            entry_range: Some([10, 12]),
            evidence_id: Some("E001".into()),
            evidence_kind: Some(EvidenceKind::new(EvidenceKind::MESSAGE)),
            label: Some("design discussion".into()),
            summary: Some("bounded host summary".into()),
            ..Default::default()
        };
        let candidate = ExtractedCandidate {
            kind: CandidateKind::Constraint,
            claim: "Extract stages candidates but does not write Memory".into(),
            why_useful: "Preserves extract/consolidation boundary".into(),
            staleness: Some("Revisit if the execution boundary changes".into()),
            evidence_ids: vec!["E001".into()],
        };
        let record = StagingRecord::from_candidate(
            "stg-1",
            "run-1",
            source(),
            candidate,
            vec![evidence],
            vec![source_ref],
        );

        let json = serde_json::to_string_pretty(&record).unwrap();
        assert!(json.contains("\"schema_version\": 2"));
        assert!(json.contains("\"kind\": \"constraint\""));
        assert!(json.contains("source_refs"));
        assert!(json.contains("evidence"));
        let parsed: StagingRecord = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.schema_version, STAGING_SCHEMA_VERSION);
        assert_eq!(parsed.kind, CandidateKind::Constraint);
        assert_eq!(parsed.evidence[0].id, "E001");
        assert_eq!(parsed.source_refs[0].evidence_id.as_deref(), Some("E001"));
    }

    #[test]
    fn candidate_kind_serializes_as_snake_case() {
        let json = serde_json::to_string(&CandidateKind::WorkingAssumption).unwrap();
        assert_eq!(json, "\"working_assumption\"");
        let parsed: CandidateKind = serde_json::from_str("\"open_question\"").unwrap();
        assert_eq!(parsed, CandidateKind::OpenQuestion);
    }
}
