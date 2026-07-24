use memory::WorkspaceLayout;
use memory::consolidate::{StagingEntry, list_staging_entries_snapshot};
use memory::extract::StagingRecord;
use memory::schema::{SourceEvidenceRef, SourceRef};
use serde::{Deserialize, Serialize};

const DEFAULT_MEMORY_STAGING_LIMIT: usize = 100;
const MAX_MEMORY_STAGING_LIMIT: usize = 500;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryStagingListResponse {
    pub limit: usize,
    pub returned_count: usize,
    pub total_valid_count: usize,
    pub invalid_count: usize,
    pub truncated: bool,
    pub order: String,
    pub record_authority: String,
    pub items: Vec<MemoryStagingEntrySummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryStagingEntrySummary {
    pub id: String,
    pub byte_len: u64,
    pub record: MemoryStagingRecordSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryStagingRecordSummary {
    pub schema_version: u32,
    pub id: String,
    pub extract_run_id: String,
    pub source: SourceRef,
    pub kind: String,
    pub claim: String,
    pub why_useful: String,
    pub staleness: Option<String>,
    pub evidence: Vec<MemoryStagingEvidenceSummary>,
    pub source_refs: Vec<MemorySourceEvidenceRefSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryStagingEvidenceSummary {
    pub id: String,
    pub kind: String,
    pub entry_range: Option<[u64; 2]>,
    pub excerpt: Option<String>,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemorySourceEvidenceRefSummary {
    pub session_id: Option<String>,
    pub segment_id: Option<String>,
    pub entry_range: Option<[u64; 2]>,
    pub evidence_id: Option<String>,
    pub evidence_kind: Option<String>,
    pub label: Option<String>,
    pub summary: Option<String>,
}

pub fn list_memory_staging(
    layout: &WorkspaceLayout,
    requested_limit: Option<usize>,
) -> MemoryStagingListResponse {
    let limit = requested_limit
        .unwrap_or(DEFAULT_MEMORY_STAGING_LIMIT)
        .min(MAX_MEMORY_STAGING_LIMIT);
    let snapshot = list_staging_entries_snapshot(layout);
    let total_valid_count = snapshot.entries.len();
    let items = snapshot
        .entries
        .into_iter()
        .take(limit)
        .map(memory_staging_entry_summary)
        .collect::<Vec<_>>();
    let returned_count = items.len();
    MemoryStagingListResponse {
        limit,
        returned_count,
        total_valid_count,
        invalid_count: snapshot.invalid_count,
        truncated: total_valid_count > returned_count,
        order: "uuidv7_ascending".to_string(),
        record_authority: "workspace_memory_staging".to_string(),
        items,
    }
}

fn memory_staging_entry_summary(entry: StagingEntry) -> MemoryStagingEntrySummary {
    MemoryStagingEntrySummary {
        id: entry.id.to_string(),
        byte_len: entry.bytes,
        record: memory_staging_record_summary(entry.record),
    }
}

fn memory_staging_record_summary(record: StagingRecord) -> MemoryStagingRecordSummary {
    MemoryStagingRecordSummary {
        schema_version: record.schema_version,
        id: record.id,
        extract_run_id: record.extract_run_id,
        source: record.source,
        kind: record.kind.as_str().to_string(),
        claim: record.claim,
        why_useful: record.why_useful,
        staleness: record.staleness,
        evidence: record
            .evidence
            .into_iter()
            .map(|evidence| MemoryStagingEvidenceSummary {
                id: evidence.id,
                kind: evidence.kind.as_str().to_string(),
                entry_range: evidence.entry_range,
                excerpt: evidence.excerpt,
                summary: evidence.summary,
            })
            .collect(),
        source_refs: record
            .source_refs
            .into_iter()
            .map(memory_source_evidence_ref_summary)
            .collect(),
    }
}

fn memory_source_evidence_ref_summary(
    source_ref: SourceEvidenceRef,
) -> MemorySourceEvidenceRefSummary {
    MemorySourceEvidenceRefSummary {
        session_id: source_ref.session_id,
        segment_id: source_ref.segment_id,
        entry_range: source_ref.entry_range,
        evidence_id: source_ref.evidence_id,
        evidence_kind: source_ref
            .evidence_kind
            .map(|evidence_kind| evidence_kind.as_str().to_string()),
        label: source_ref.label,
        summary: source_ref.summary,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use memory::extract::{CandidateKind, ExtractedCandidate, ExtractedPayload, write_staging};
    use tempfile::TempDir;

    fn source() -> SourceRef {
        SourceRef {
            segment_id: "segment-1".to_string(),
            range: [0, 10],
        }
    }

    fn payload(claim: &str) -> ExtractedPayload {
        ExtractedPayload {
            candidates: vec![ExtractedCandidate {
                kind: CandidateKind::Decision,
                claim: claim.to_string(),
                why_useful: "useful for future work".to_string(),
                staleness: None,
                evidence_ids: Vec::new(),
            }],
        }
    }

    #[test]
    fn lists_memory_staging_records_with_cap() {
        let temp = TempDir::new().unwrap();
        let layout = WorkspaceLayout::new(temp.path().to_path_buf());
        write_staging(&layout, source(), payload("first claim")).unwrap();
        write_staging(&layout, source(), payload("second claim")).unwrap();

        let response = list_memory_staging(&layout, Some(1));

        assert_eq!(response.limit, 1);
        assert_eq!(response.returned_count, 1);
        assert_eq!(response.total_valid_count, 2);
        assert!(response.truncated);
        assert_eq!(response.record_authority, "workspace_memory_staging");
        assert_eq!(response.items[0].record.kind, "decision");
        assert_eq!(response.items[0].record.claim, "first claim");
    }
}
