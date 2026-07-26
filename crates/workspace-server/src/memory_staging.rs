use memory::extract::StagingRecord;
use memory::schema::{SourceEvidenceRef, SourceRef};
use serde::{Deserialize, Serialize};

use crate::Result;
use crate::authority::MemoryAuthority;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryStagingBacklogSummary {
    pub candidate_count: usize,
    pub total_bytes: u64,
    pub invalid_count: usize,
}

pub fn list_memory_staging_from_authority<A: MemoryAuthority>(
    authority: &A,
    requested_limit: Option<usize>,
) -> Result<MemoryStagingListResponse> {
    let limit = requested_limit
        .unwrap_or(DEFAULT_MEMORY_STAGING_LIMIT)
        .min(MAX_MEMORY_STAGING_LIMIT);
    let entries = authority.list_memory_staging_records(MAX_MEMORY_STAGING_LIMIT + 1)?;
    let fetched_count = entries.len();
    let mut invalid_count = 0usize;
    let mut total_valid_count = 0usize;
    let mut valid_items = Vec::with_capacity(fetched_count.min(limit));
    for entry in entries {
        let record = match serde_json::from_str::<StagingRecord>(&entry.raw_json) {
            Ok(record) => record,
            Err(_) => {
                invalid_count += 1;
                continue;
            }
        };
        total_valid_count += 1;
        if valid_items.len() < limit {
            valid_items.push(MemoryStagingEntrySummary {
                id: entry.candidate_id,
                byte_len: entry.raw_json.len() as u64,
                record: memory_staging_record_summary(record),
            });
        }
    }
    let returned_count = valid_items.len();
    Ok(MemoryStagingListResponse {
        limit,
        returned_count,
        total_valid_count,
        invalid_count,
        truncated: total_valid_count > returned_count || fetched_count > MAX_MEMORY_STAGING_LIMIT,
        order: "imported_at_desc_candidate_id_asc".to_string(),
        record_authority: "sqlite_workspace_authority.memory_staging".to_string(),
        items: valid_items,
    })
}

pub fn memory_staging_backlog_from_authority<A: MemoryAuthority>(
    authority: &A,
) -> Result<MemoryStagingBacklogSummary> {
    let entries = authority.list_memory_staging_records(i64::MAX as usize)?;
    let mut candidate_count = 0usize;
    let mut total_bytes = 0u64;
    let mut invalid_count = 0usize;
    for entry in entries {
        match serde_json::from_str::<StagingRecord>(&entry.raw_json) {
            Ok(_) => {
                candidate_count += 1;
                total_bytes += entry.raw_json.len() as u64;
            }
            Err(_) => invalid_count += 1,
        }
    }
    Ok(MemoryStagingBacklogSummary {
        candidate_count,
        total_bytes,
        invalid_count,
    })
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
    use crate::authority::{MemoryAuthority, SqliteWorkspaceAuthority};
    use crate::store::{ControlPlaneStore, SqliteWorkspaceStore, WorkspaceRecord};
    use memory::extract::{CandidateKind, ExtractedCandidate, StagingRecord};
    use tempfile::TempDir;

    fn source() -> SourceRef {
        SourceRef {
            segment_id: "segment-1".to_string(),
            range: [0, 10],
        }
    }

    fn record_json(id: &str, claim: &str) -> String {
        let record = StagingRecord::from_candidate(
            id,
            "extract-run-1",
            source(),
            ExtractedCandidate {
                kind: CandidateKind::Decision,
                claim: claim.to_string(),
                why_useful: "useful for future work".to_string(),
                staleness: None,
                evidence_ids: Vec::new(),
            },
            Vec::new(),
            Vec::new(),
        );
        serde_json::to_string(&record).unwrap()
    }

    async fn authority() -> (TempDir, SqliteWorkspaceAuthority) {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("workspace.db");
        let store = SqliteWorkspaceStore::open(&db_path).unwrap();
        store
            .upsert_workspace(&WorkspaceRecord {
                workspace_id: "workspace-test".to_string(),
                owner_account_id: None,
                display_name: "Workspace Test".to_string(),
                state: "active".to_string(),
                created_at: "2026-01-01T00:00:00Z".to_string(),
                updated_at: "2026-01-01T00:00:00Z".to_string(),
            })
            .await
            .unwrap();
        let authority = SqliteWorkspaceAuthority::new(db_path, "workspace-test").unwrap();
        (temp, authority)
    }

    #[tokio::test]
    async fn lists_memory_staging_records_with_cap_from_authority() {
        let (_temp, authority) = authority().await;
        authority
            .upsert_memory_staging_record(
                "00000000000000000000000001",
                &record_json("00000000000000000000000001", "first claim"),
                None,
            )
            .unwrap();
        authority
            .upsert_memory_staging_record(
                "00000000000000000000000002",
                &record_json("00000000000000000000000002", "second claim"),
                None,
            )
            .unwrap();

        let response = list_memory_staging_from_authority(&authority, Some(1)).unwrap();

        assert_eq!(response.limit, 1);
        assert_eq!(response.returned_count, 1);
        assert_eq!(response.total_valid_count, 2);
        assert!(response.truncated);
        assert_eq!(
            response.record_authority,
            "sqlite_workspace_authority.memory_staging"
        );
        assert_eq!(response.items[0].record.kind, "decision");
    }

    #[tokio::test]
    async fn summarizes_memory_staging_backlog_from_authority_records() {
        let (_temp, authority) = authority().await;
        let first = record_json("00000000000000000000000001", "first claim");
        let second = record_json("00000000000000000000000002", "second claim");
        authority
            .upsert_memory_staging_record("00000000000000000000000001", &first, None)
            .unwrap();
        authority
            .upsert_memory_staging_record("00000000000000000000000002", &second, None)
            .unwrap();
        authority
            .upsert_memory_staging_record("invalid-json-shape", r#"{"legacy":"shape"}"#, None)
            .unwrap();

        let backlog = memory_staging_backlog_from_authority(&authority).unwrap();

        assert_eq!(backlog.candidate_count, 2);
        assert_eq!(backlog.total_bytes, (first.len() + second.len()) as u64);
        assert_eq!(backlog.invalid_count, 1);
    }
}
