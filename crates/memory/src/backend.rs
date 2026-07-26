//! Workspace-backend operations for Memory tools.
//!
//! These types are the typed HTTP boundary used by runtime workers that have
//! Workspace authority but no direct local filesystem authority. The local
//! executor intentionally lives in the memory crate so the workspace backend and
//! future non-HTTP hosts share validation and path semantics.

use std::fs;
use std::io;
use std::io::Write;

use chrono::Utc;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::audit::{AuditEvent, append_audit_event};
use crate::consolidate::list_staging_entries_snapshot;
use crate::extract::{
    ExtractedCandidate, ExtractedPayload, StagingEvidence, write_staging, write_staging_candidate,
};
use crate::schema::{SourceEvidenceRef, SourceRef};
use crate::workspace::WorkspaceLayout;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum MemoryBackendOperation {
    Query(MemoryQueryOperation),
    ReadDocument(MemoryDocumentReadOperation),
    UpdateDocument(MemoryDocumentUpdateOperation),
    ResidentSummary(MemoryResidentSummaryOperation),
    AppendAudit(MemoryAppendAuditOperation),
    StageCandidate(MemoryStageCandidateOperation),
    StageExtracted(MemoryStageExtractedOperation),
    StagingList(MemoryStagingListOperation),
    StagingRead(MemoryStagingReadOperation),
    StagingClose(MemoryStagingCloseOperation),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MemoryBackendHttpResponse {
    Ok {
        result: MemoryBackendOperationResult,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MemoryBackendOperationResult {
    ToolOutput(MemoryToolOutput),
    Acknowledged(MemoryBackendAckOutput),
    StagingWritten(MemoryStagingWriteOutput),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryToolOutput {
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryQueryOperation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryDocumentReadOperation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryDocumentUpdateOperation {
    pub old_string: String,
    pub new_string: String,
    #[serde(default)]
    pub replace_all: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MemoryResidentSummaryOperation {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryAppendAuditOperation {
    pub event: AuditEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStageCandidateOperation {
    pub source: SourceRef,
    pub extract_run_id: String,
    pub candidate: ExtractedCandidate,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<StagingEvidence>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_refs: Vec<SourceEvidenceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStageExtractedOperation {
    pub source: SourceRef,
    pub payload: ExtractedPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryStagingListOperation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryStagingReadOperation {
    pub candidate_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryStagingCloseOperation {
    pub candidate_id: String,
    pub action: MemoryStagingCloseAction,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub affected_memory: Vec<MemoryStagingAffectedMemory>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStagingCloseAction {
    Applied,
    Discarded,
    Invalid,
    Duplicate,
    AlreadyCovered,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryStagingAffectedMemory {
    pub operation: MemoryStagingAffectedMemoryOperation,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStagingAffectedMemoryOperation {
    Edit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConsolidateStagingOperation {
    #[serde(default)]
    pub force: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold_files: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConsolidationOutput {
    pub status: String,
    pub summary: String,
    pub candidate_count: usize,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MemoryStagingCloseDispositionRecord {
    schema_version: u32,
    candidate_id: String,
    staging_path: String,
    recorded_at: String,
    action: MemoryStagingCloseAction,
    reason: String,
    affected_memory: Vec<MemoryStagingAffectedMemory>,
}

const STAGING_RESOLUTIONS_FILE: &str = "_resolutions.jsonl";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryBackendAckOutput {
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStagingWriteOutput {
    pub staging_count: usize,
    pub staging_ids: Vec<String>,
}

pub fn execute_memory_backend_operation(
    layout: &WorkspaceLayout,
    operation: MemoryBackendOperation,
) -> io::Result<MemoryBackendOperationResult> {
    match operation {
        MemoryBackendOperation::Query(operation) => Ok(MemoryBackendOperationResult::ToolOutput(
            execute_query(layout, operation),
        )),
        MemoryBackendOperation::ReadDocument(_operation) => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Memory document operations require WorkspaceAuthority-backed executor",
        )),
        MemoryBackendOperation::UpdateDocument(_operation) => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Memory document operations require WorkspaceAuthority-backed executor",
        )),
        MemoryBackendOperation::ResidentSummary(_operation) => Ok(
            MemoryBackendOperationResult::ToolOutput(execute_resident_summary(layout)),
        ),
        MemoryBackendOperation::AppendAudit(operation) => {
            append_audit_event(layout, &operation.event)?;
            Ok(MemoryBackendOperationResult::Acknowledged(
                MemoryBackendAckOutput {
                    summary: "memory audit event appended".to_string(),
                },
            ))
        }
        MemoryBackendOperation::StageCandidate(operation) => {
            let written = write_staging_candidate(
                layout,
                operation.source,
                &operation.extract_run_id,
                operation.candidate,
                operation.evidence,
                operation.source_refs,
            )?;
            Ok(MemoryBackendOperationResult::StagingWritten(
                MemoryStagingWriteOutput {
                    staging_count: 1,
                    staging_ids: vec![written.id.to_string()],
                },
            ))
        }
        MemoryBackendOperation::StageExtracted(operation) => {
            let written = write_staging(layout, operation.source, operation.payload)?;
            Ok(MemoryBackendOperationResult::StagingWritten(
                MemoryStagingWriteOutput {
                    staging_count: written.len(),
                    staging_ids: written.iter().map(|item| item.id.to_string()).collect(),
                },
            ))
        }
        MemoryBackendOperation::StagingList(operation) => {
            execute_staging_list(layout, operation).map(MemoryBackendOperationResult::ToolOutput)
        }
        MemoryBackendOperation::StagingRead(operation) => {
            execute_staging_read(layout, operation).map(MemoryBackendOperationResult::ToolOutput)
        }
        MemoryBackendOperation::StagingClose(operation) => {
            execute_staging_close(layout, operation).map(MemoryBackendOperationResult::ToolOutput)
        }
    }
}

fn execute_resident_summary(layout: &WorkspaceLayout) -> MemoryToolOutput {
    match crate::collect_resident_summary(layout) {
        Some(summary) => MemoryToolOutput {
            summary: "resident memory summary collected".to_string(),
            content: Some(summary),
        },
        None => MemoryToolOutput {
            summary: "resident memory summary unavailable".to_string(),
            content: None,
        },
    }
}

fn execute_query(layout: &WorkspaceLayout, operation: MemoryQueryOperation) -> MemoryToolOutput {
    let Some(document) = crate::collect_resident_summary(layout) else {
        let suffix = operation
            .query
            .as_deref()
            .map(|query| format!(" matching {query:?}"))
            .unwrap_or_default();
        return MemoryToolOutput {
            summary: format!("No memory document content found{suffix}"),
            content: None,
        };
    };
    if !matches_query(&document, operation.query.as_deref()) {
        let suffix = operation
            .query
            .as_deref()
            .map(|query| format!(" matching {query:?}"))
            .unwrap_or_default();
        return MemoryToolOutput {
            summary: format!("No memory document content found{suffix}"),
            content: None,
        };
    }
    tool_output_from_string(excerpt(&document, operation.query.as_deref()))
}

fn execute_staging_list(
    layout: &WorkspaceLayout,
    operation: MemoryStagingListOperation,
) -> io::Result<MemoryToolOutput> {
    let limit = operation.limit.unwrap_or(20).min(100);
    let snapshot = list_staging_entries_snapshot(layout);
    let total = snapshot.entries.len();
    let invalid_count = snapshot.invalid_count;
    let records = snapshot
        .entries
        .into_iter()
        .take(limit)
        .map(|entry| {
            serde_json::json!({
                "candidate_id": entry.id.to_string(),
                "bytes": entry.bytes,
                "path": entry.path.display().to_string(),
                "source": entry.record.source,
                "kind": entry.record.kind,
                "claim": entry.record.claim,
                "why_useful": entry.record.why_useful,
                "staleness": entry.record.staleness,
                "evidence_count": entry.record.evidence.len(),
                "source_ref_count": entry.record.source_refs.len(),
            })
        })
        .collect::<Vec<_>>();
    Ok(MemoryToolOutput {
        summary: format!(
            "Listed {} of {total} staging candidate(s); invalid_count={invalid_count}",
            records.len()
        ),
        content: Some(serde_json::to_string_pretty(&records).map_err(io::Error::other)?),
    })
}

fn execute_staging_read(
    layout: &WorkspaceLayout,
    operation: MemoryStagingReadOperation,
) -> io::Result<MemoryToolOutput> {
    let entry = find_staging_entry(layout, &operation.candidate_id)?;
    Ok(MemoryToolOutput {
        summary: format!("Read staging candidate {}", entry.id),
        content: Some(serde_json::to_string_pretty(&entry.record).map_err(io::Error::other)?),
    })
}

fn execute_staging_close(
    layout: &WorkspaceLayout,
    operation: MemoryStagingCloseOperation,
) -> io::Result<MemoryToolOutput> {
    if operation.reason.trim().is_empty() {
        return Err(invalid_input("reason is required"));
    }
    let entry = find_staging_entry(layout, &operation.candidate_id)?;
    let disposition = MemoryStagingCloseDispositionRecord {
        schema_version: 1,
        candidate_id: entry.id.to_string(),
        staging_path: entry.path.display().to_string(),
        recorded_at: Utc::now().to_rfc3339(),
        action: operation.action,
        reason: operation.reason,
        affected_memory: operation.affected_memory,
    };
    let resolutions_path = layout.memory_dir().join(STAGING_RESOLUTIONS_FILE);
    if let Some(parent) = resolutions_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut line = serde_json::to_string(&disposition).map_err(io::Error::other)?;
    line.push('\n');
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&resolutions_path)?
        .write_all(line.as_bytes())?;
    fs::remove_file(&entry.path)?;
    Ok(MemoryToolOutput {
        summary: format!("Closed staging candidate {}", entry.id),
        content: Some(serde_json::to_string_pretty(&disposition).map_err(io::Error::other)?),
    })
}

fn find_staging_entry(
    layout: &WorkspaceLayout,
    candidate_id: &str,
) -> io::Result<crate::consolidate::StagingEntry> {
    let candidate_id = Uuid::parse_str(candidate_id)
        .map_err(|err| invalid_input(format!("invalid candidate_id: {err}")))?;
    list_staging_entries_snapshot(layout)
        .entries
        .into_iter()
        .find(|entry| entry.id == candidate_id)
        .ok_or_else(|| invalid_input(format!("staging candidate not found: {candidate_id}")))
}

fn matches_query(content: &str, query: Option<&str>) -> bool {
    match query.map(str::trim).filter(|query| !query.is_empty()) {
        Some(query) => content.to_lowercase().contains(&query.to_lowercase()),
        None => true,
    }
}

fn excerpt(content: &str, query: Option<&str>) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let Some(query) = query.map(str::trim).filter(|query| !query.is_empty()) else {
        return lines
            .iter()
            .take(20)
            .copied()
            .collect::<Vec<_>>()
            .join("\n");
    };
    let query_lower = query.to_lowercase();
    let hit = lines
        .iter()
        .position(|line| line.to_lowercase().contains(&query_lower))
        .unwrap_or(0);
    let start = hit.saturating_sub(2);
    let end = (hit + 3).min(lines.len());
    lines[start..end].join("\n")
}

fn tool_output_from_string(value: String) -> MemoryToolOutput {
    const SUMMARY_THRESHOLD: usize = 200;
    if value.len() <= SUMMARY_THRESHOLD {
        MemoryToolOutput {
            summary: value,
            content: None,
        }
    } else {
        let lines = value.lines().count();
        let first_line: String = value
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(80)
            .collect();
        MemoryToolOutput {
            summary: format!("{lines} lines | {first_line}…"),
            content: Some(value),
        }
    }
}

fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::{CandidateKind, ExtractedCandidate};

    #[test]
    fn staging_list_read_close_records_reason_and_deletes_candidate() {
        let temp = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::resolve(&manifest::MemoryConfig::default(), temp.path());
        let source = SourceRef {
            segment_id: "segment-1".into(),
            range: [0, 1],
        };
        let payload = ExtractedPayload {
            candidates: vec![ExtractedCandidate {
                kind: CandidateKind::Preference,
                claim: "User prefers short reviews".into(),
                why_useful: "Review style preference".into(),
                staleness: None,
                evidence_ids: Vec::new(),
            }],
        };
        let result = execute_memory_backend_operation(
            &layout,
            MemoryBackendOperation::StageExtracted(MemoryStageExtractedOperation {
                source,
                payload,
            }),
        )
        .unwrap();
        let candidate_id = match result {
            MemoryBackendOperationResult::StagingWritten(output) => output.staging_ids[0].clone(),
            _ => panic!("expected staging write output"),
        };

        let list = execute_memory_backend_operation(
            &layout,
            MemoryBackendOperation::StagingList(MemoryStagingListOperation { limit: Some(10) }),
        )
        .unwrap();
        let MemoryBackendOperationResult::ToolOutput(list) = list else {
            panic!("expected list tool output")
        };
        assert!(list.content.unwrap().contains(&candidate_id));

        let read = execute_memory_backend_operation(
            &layout,
            MemoryBackendOperation::StagingRead(MemoryStagingReadOperation {
                candidate_id: candidate_id.clone(),
            }),
        )
        .unwrap();
        let MemoryBackendOperationResult::ToolOutput(read) = read else {
            panic!("expected read tool output")
        };
        assert!(read.content.unwrap().contains("short reviews"));

        execute_memory_backend_operation(
            &layout,
            MemoryBackendOperation::StagingClose(MemoryStagingCloseOperation {
                candidate_id: candidate_id.clone(),
                action: MemoryStagingCloseAction::Applied,
                reason: "Merged into the Memory document.".into(),
                affected_memory: vec![MemoryStagingAffectedMemory {
                    operation: MemoryStagingAffectedMemoryOperation::Edit,
                }],
            }),
        )
        .unwrap();

        let snapshot = list_staging_entries_snapshot(&layout);
        assert!(snapshot.entries.is_empty());
        let resolutions =
            fs::read_to_string(layout.memory_dir().join(STAGING_RESOLUTIONS_FILE)).unwrap();
        assert!(resolutions.contains(&candidate_id));
        assert!(resolutions.contains("Merged into the Memory document."));
    }
}
