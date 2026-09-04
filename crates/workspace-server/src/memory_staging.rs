use memory::extract::{CandidateKind, StagingRecord};
use memory::schema::{EvidenceOrigin, EvidenceOriginKind, SourceEvidenceRef, SourceRef};
use workspace_api::{
    Diagnostic, DiagnosticSeverity, MemoryCandidateKind, MemoryEvidenceOrigin,
    MemoryEvidenceOriginKind, MemorySourceEvidenceRef, MemorySourceRef, MemoryStagingEntry,
    MemoryStagingEvidence, MemoryStagingListResponse, MemoryStagingRecord,
};

use crate::Result;
use crate::authority::MemoryAuthority;

const DEFAULT_MEMORY_STAGING_LIMIT: usize = 100;
const MAX_MEMORY_STAGING_LIMIT: usize = 500;

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
            valid_items.push(MemoryStagingEntry {
                id: entry.candidate_id,
                byte_len: entry.raw_json.len() as u64,
                record: memory_staging_record_projection(record),
            });
        }
    }
    let returned_count = valid_items.len();
    let diagnostics = (invalid_count > 0)
        .then(|| Diagnostic {
            code: "memory_staging_record_invalid".to_string(),
            message: format!(
                "{invalid_count} Memory staging record(s) were excluded because they did not match the current schema."
            ),
            severity: DiagnosticSeverity::Error,
        })
        .into_iter()
        .collect();
    Ok(MemoryStagingListResponse {
        limit,
        returned_count,
        total_valid_count,
        invalid_count,
        truncated: total_valid_count > returned_count || fetched_count > MAX_MEMORY_STAGING_LIMIT,
        order: "imported_at_desc_candidate_id_asc".to_string(),
        record_authority: "sqlite_workspace_authority.memory_staging".to_string(),
        items: valid_items,
        diagnostics,
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

fn memory_staging_record_projection(record: StagingRecord) -> MemoryStagingRecord {
    MemoryStagingRecord {
        schema_version: record.schema_version,
        id: record.id,
        extract_run_id: record.extract_run_id,
        source: memory_source_ref_projection(record.source),
        kind: memory_candidate_kind_projection(record.kind),
        claim: record.claim,
        why_useful: record.why_useful,
        staleness: record.staleness,
        evidence: record
            .evidence
            .into_iter()
            .map(|evidence| MemoryStagingEvidence {
                id: evidence.id,
                kind: evidence.kind.as_str().to_string(),
                entry_range: evidence.entry_range,
                origin: evidence.origin.map(memory_evidence_origin_projection),
                excerpt: evidence.excerpt,
                summary: evidence.summary,
            })
            .collect(),
        source_refs: record
            .source_refs
            .into_iter()
            .map(memory_source_evidence_ref_projection)
            .collect(),
    }
}

fn memory_source_ref_projection(source_ref: SourceRef) -> MemorySourceRef {
    MemorySourceRef {
        segment_id: source_ref.segment_id,
        range: source_ref.range,
    }
}

fn memory_candidate_kind_projection(kind: CandidateKind) -> MemoryCandidateKind {
    match kind {
        CandidateKind::Preference => MemoryCandidateKind::Preference,
        CandidateKind::WorkingAssumption => MemoryCandidateKind::WorkingAssumption,
        CandidateKind::Constraint => MemoryCandidateKind::Constraint,
        CandidateKind::Decision => MemoryCandidateKind::Decision,
        CandidateKind::OpenQuestion => MemoryCandidateKind::OpenQuestion,
        CandidateKind::Lesson => MemoryCandidateKind::Lesson,
    }
}

fn memory_source_evidence_ref_projection(source_ref: SourceEvidenceRef) -> MemorySourceEvidenceRef {
    MemorySourceEvidenceRef {
        session_id: source_ref.session_id,
        segment_id: source_ref.segment_id,
        entry_range: source_ref.entry_range,
        evidence_id: source_ref.evidence_id,
        origin: source_ref.origin.map(memory_evidence_origin_projection),
        evidence_kind: source_ref
            .evidence_kind
            .map(|evidence_kind| evidence_kind.as_str().to_string()),
        label: source_ref.label,
        summary: source_ref.summary,
    }
}

fn memory_evidence_origin_projection(origin: EvidenceOrigin) -> MemoryEvidenceOrigin {
    MemoryEvidenceOrigin {
        kind: match origin.kind {
            EvidenceOriginKind::HumanInput => MemoryEvidenceOriginKind::HumanInput,
            EvidenceOriginKind::WorkerInput => MemoryEvidenceOriginKind::WorkerInput,
            EvidenceOriginKind::FlowInstruction => MemoryEvidenceOriginKind::FlowInstruction,
            EvidenceOriginKind::BackendInstruction => MemoryEvidenceOriginKind::BackendInstruction,
            EvidenceOriginKind::ModelOutput => MemoryEvidenceOriginKind::ModelOutput,
            EvidenceOriginKind::ToolOutput => MemoryEvidenceOriginKind::ToolOutput,
            EvidenceOriginKind::DerivedSummary => MemoryEvidenceOriginKind::DerivedSummary,
            EvidenceOriginKind::LegacyUnknown => MemoryEvidenceOriginKind::LegacyUnknown,
        },
        account_id: origin.account_id,
        workspace_id: origin.workspace_id,
        runtime_id: origin.runtime_id,
        worker_id: origin.worker_id,
        flow_selector: origin.flow_selector,
        flow_definition_id: origin.flow_definition_id,
        flow_definition_revision: origin.flow_definition_revision,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority::{MemoryAuthority, SqliteWorkspaceAuthority};
    use crate::store::{ControlPlaneStore, SqliteWorkspaceStore, WorkspaceRecord};
    use memory::extract::{CandidateKind, ExtractedCandidate, StagingRecord};
    use memory::schema::{EvidenceOrigin, EvidenceOriginKind, SourceEvidenceRef};
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
                owner_account_id: "owner-account".to_string(),
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
        assert_eq!(response.items[0].record.kind, MemoryCandidateKind::Decision);
        assert!(response.diagnostics.is_empty());
    }

    #[test]
    fn projects_every_typed_evidence_origin_without_flattening() {
        let cases = [
            (
                EvidenceOriginKind::HumanInput,
                MemoryEvidenceOriginKind::HumanInput,
            ),
            (
                EvidenceOriginKind::WorkerInput,
                MemoryEvidenceOriginKind::WorkerInput,
            ),
            (
                EvidenceOriginKind::FlowInstruction,
                MemoryEvidenceOriginKind::FlowInstruction,
            ),
            (
                EvidenceOriginKind::BackendInstruction,
                MemoryEvidenceOriginKind::BackendInstruction,
            ),
            (
                EvidenceOriginKind::ModelOutput,
                MemoryEvidenceOriginKind::ModelOutput,
            ),
            (
                EvidenceOriginKind::ToolOutput,
                MemoryEvidenceOriginKind::ToolOutput,
            ),
            (
                EvidenceOriginKind::DerivedSummary,
                MemoryEvidenceOriginKind::DerivedSummary,
            ),
            (
                EvidenceOriginKind::LegacyUnknown,
                MemoryEvidenceOriginKind::LegacyUnknown,
            ),
        ];

        for (domain_kind, api_kind) in cases {
            let projected = memory_source_evidence_ref_projection(SourceEvidenceRef {
                session_id: Some("session-1".to_string()),
                origin: Some(EvidenceOrigin {
                    kind: domain_kind,
                    account_id: Some("account-1".to_string()),
                    workspace_id: Some("workspace-test".to_string()),
                    runtime_id: Some("runtime-1".to_string()),
                    worker_id: Some("worker-1".to_string()),
                    flow_selector: Some("builtin:coder-review".to_string()),
                    flow_definition_id: Some("flow-1".to_string()),
                    flow_definition_revision: Some(7),
                }),
                ..SourceEvidenceRef::default()
            });
            let origin = projected.origin.unwrap();
            assert_eq!(origin.kind, api_kind);
            assert_eq!(origin.account_id.as_deref(), Some("account-1"));
            assert_eq!(origin.workspace_id.as_deref(), Some("workspace-test"));
            assert_eq!(origin.runtime_id.as_deref(), Some("runtime-1"));
            assert_eq!(origin.worker_id.as_deref(), Some("worker-1"));
            assert_eq!(
                origin.flow_selector.as_deref(),
                Some("builtin:coder-review")
            );
            assert_eq!(origin.flow_definition_id.as_deref(), Some("flow-1"));
            assert_eq!(origin.flow_definition_revision, Some(7));
        }
    }

    #[tokio::test]
    async fn invalid_or_newer_origin_shapes_are_excluded_with_bounded_diagnostic() {
        let (_temp, authority) = authority().await;
        for (id, origin) in [
            (
                "unknown-origin-kind",
                serde_json::json!({"kind": "future_origin_kind"}),
            ),
            (
                "newer-origin-shape",
                serde_json::json!({"kind": "human_input", "future_field": "do not echo me"}),
            ),
        ] {
            let mut record: serde_json::Value =
                serde_json::from_str(&record_json(id, "claim")).unwrap();
            record["source_refs"] = serde_json::json!([{"origin": origin}]);
            authority
                .upsert_memory_staging_record(id, &serde_json::to_string(&record).unwrap(), None)
                .unwrap();
        }

        let response = list_memory_staging_from_authority(&authority, None).unwrap();

        assert_eq!(response.returned_count, 0);
        assert_eq!(response.invalid_count, 2);
        assert_eq!(response.diagnostics.len(), 1);
        assert_eq!(
            response.diagnostics[0].code,
            "memory_staging_record_invalid"
        );
        assert_eq!(response.diagnostics[0].severity, DiagnosticSeverity::Error);
        assert!(
            !response.diagnostics[0]
                .message
                .contains("future_origin_kind")
        );
        assert!(!response.diagnostics[0].message.contains("do not echo me"));
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
