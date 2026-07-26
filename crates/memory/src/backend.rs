//! Workspace-backend operations for Memory tools.
//!
//! These types are the typed HTTP boundary used by runtime workers that have
//! Workspace authority but no direct local filesystem authority. The local
//! executor intentionally lives in the memory crate so the workspace backend and
//! future non-HTTP hosts share validation and path semantics.

use std::fs;
use std::io;
use std::io::Write;
use std::path::PathBuf;

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
use crate::tool::MemoryToolKind;
use crate::workspace::WorkspaceLayout;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum MemoryBackendOperation {
    Query(MemoryQueryOperation),
    ReadDocument(MemoryDocumentReadOperation),
    UpdateDocument(MemoryDocumentUpdateOperation),
    Read(MemoryReadOperation),
    Write(MemoryWriteOperation),
    Edit(MemoryEditOperation),
    Delete(MemoryDeleteOperation),
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
    pub body_md: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryReadOperation {
    pub kind: MemoryToolKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryWriteOperation {
    pub kind: MemoryToolKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEditOperation {
    pub kind: MemoryToolKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    pub old_string: String,
    pub new_string: String,
    #[serde(default)]
    pub replace_all: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryDeleteOperation {
    pub kind: MemoryToolKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
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
    pub kind: MemoryToolKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    pub operation: MemoryStagingAffectedMemoryOperation,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStagingAffectedMemoryOperation {
    Read,
    Write,
    Edit,
    Delete,
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
        MemoryBackendOperation::Query(operation) => {
            execute_query(layout, operation).map(MemoryBackendOperationResult::ToolOutput)
        }
        MemoryBackendOperation::ReadDocument(_operation) => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Memory document operations require WorkspaceAuthority-backed executor",
        )),
        MemoryBackendOperation::UpdateDocument(_operation) => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Memory document operations require WorkspaceAuthority-backed executor",
        )),
        MemoryBackendOperation::Read(operation) => {
            execute_read(layout, operation).map(MemoryBackendOperationResult::ToolOutput)
        }
        MemoryBackendOperation::Write(operation) => {
            execute_write(layout, operation).map(MemoryBackendOperationResult::ToolOutput)
        }
        MemoryBackendOperation::Edit(operation) => {
            execute_edit(layout, operation).map(MemoryBackendOperationResult::ToolOutput)
        }
        MemoryBackendOperation::Delete(operation) => {
            execute_delete(layout, operation).map(MemoryBackendOperationResult::ToolOutput)
        }
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

fn execute_query(
    layout: &WorkspaceLayout,
    operation: MemoryQueryOperation,
) -> io::Result<MemoryToolOutput> {
    let mut entries = Vec::new();
    for kind in [
        MemoryToolKind::Summary,
        MemoryToolKind::Decision,
        MemoryToolKind::Request,
    ] {
        entries.extend(query_kind(layout, kind, operation.query.as_deref())?);
    }

    if entries.is_empty() {
        let suffix = operation
            .query
            .as_deref()
            .map(|query| format!(" matching {query:?}"))
            .unwrap_or_default();
        return Ok(MemoryToolOutput {
            summary: format!("No memory records found{suffix}"),
            content: None,
        });
    }

    let body = entries.join("\n\n");
    Ok(tool_output_from_string(body))
}

fn query_kind(
    layout: &WorkspaceLayout,
    kind: MemoryToolKind,
    query: Option<&str>,
) -> io::Result<Vec<String>> {
    let mut entries = Vec::new();
    match kind {
        MemoryToolKind::Summary => {
            let path = memory_path(layout, kind, None)?;
            if !path.exists() {
                return Ok(entries);
            }
            let content = fs::read_to_string(&path)?;
            if matches_query(&content, query) {
                entries.push(format!("kind=summary\n{}", excerpt(&content, query)));
            }
        }
        MemoryToolKind::Decision | MemoryToolKind::Request => {
            let dir = record_dir(layout, kind);
            if !dir.exists() {
                return Ok(entries);
            }
            let mut files: Vec<PathBuf> = fs::read_dir(&dir)?
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("md"))
                .collect();
            files.sort();
            for path in files {
                let content = fs::read_to_string(&path)?;
                if matches_query(&content, query) {
                    let slug = path
                        .file_stem()
                        .and_then(|value| value.to_str())
                        .unwrap_or("");
                    entries.push(format!(
                        "kind={} slug={}\n{}",
                        kind.as_str(),
                        slug,
                        excerpt(&content, query)
                    ));
                }
            }
        }
    }
    Ok(entries)
}

fn execute_read(
    layout: &WorkspaceLayout,
    operation: MemoryReadOperation,
) -> io::Result<MemoryToolOutput> {
    let path = memory_path(layout, operation.kind, operation.slug.as_deref())?;
    let content = fs::read_to_string(&path)?;
    let offset = operation.offset.unwrap_or(0);
    let limit = operation.limit.unwrap_or(2000);
    let lines: Vec<&str> = content.lines().collect();
    let selected = lines
        .iter()
        .enumerate()
        .skip(offset)
        .take(limit)
        .map(|(index, line)| format!("{:>6}\t{}", index + 1, line))
        .collect::<Vec<_>>()
        .join("\n");
    let total = lines.len();
    Ok(MemoryToolOutput {
        summary: format!(
            "Read {} lines from {}{}",
            selected.lines().count(),
            operation.kind.as_str(),
            operation
                .slug
                .as_deref()
                .map(|slug| format!("/{slug}"))
                .unwrap_or_default()
        ),
        content: Some(format!(
            "total_lines={total} offset={offset} limit={limit}\n{selected}"
        )),
    })
}

fn execute_write(
    layout: &WorkspaceLayout,
    operation: MemoryWriteOperation,
) -> io::Result<MemoryToolOutput> {
    let path = memory_path(layout, operation.kind, operation.slug.as_deref())?;
    validate_slug_rules(operation.kind, operation.slug.as_deref())?;
    validate_memory_content(
        operation.kind,
        operation.slug.as_deref(),
        &operation.content,
    )?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, operation.content)?;
    Ok(MemoryToolOutput {
        summary: format!("Wrote {} record", operation.kind.as_str()),
        content: Some(path.display().to_string()),
    })
}

fn execute_edit(
    layout: &WorkspaceLayout,
    operation: MemoryEditOperation,
) -> io::Result<MemoryToolOutput> {
    validate_slug_rules(operation.kind, operation.slug.as_deref())?;
    if operation.old_string == operation.new_string {
        return Err(invalid_input("old_string and new_string must differ"));
    }
    let path = memory_path(layout, operation.kind, operation.slug.as_deref())?;
    let original = fs::read_to_string(&path)?;
    let count = original.matches(&operation.old_string).count();
    if count == 0 {
        return Err(invalid_input("old_string was not found"));
    }
    if count > 1 && !operation.replace_all {
        return Err(invalid_input(
            "old_string matched more than once; set replace_all=true to replace every occurrence",
        ));
    }
    let updated = if operation.replace_all {
        original.replace(&operation.old_string, &operation.new_string)
    } else {
        original.replacen(&operation.old_string, &operation.new_string, 1)
    };
    validate_memory_content(operation.kind, operation.slug.as_deref(), &updated)?;
    fs::write(&path, updated)?;
    Ok(MemoryToolOutput {
        summary: format!(
            "Edited {} occurrence(s)",
            if operation.replace_all { count } else { 1 }
        ),
        content: Some(path.display().to_string()),
    })
}

fn execute_delete(
    layout: &WorkspaceLayout,
    operation: MemoryDeleteOperation,
) -> io::Result<MemoryToolOutput> {
    validate_slug_rules(operation.kind, operation.slug.as_deref())?;
    let path = memory_path(layout, operation.kind, operation.slug.as_deref())?;
    fs::remove_file(&path)?;
    Ok(MemoryToolOutput {
        summary: format!("Deleted {} record", operation.kind.as_str()),
        content: Some(path.display().to_string()),
    })
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
    validate_affected_memory(&operation.affected_memory)?;
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

fn validate_affected_memory(records: &[MemoryStagingAffectedMemory]) -> io::Result<()> {
    for record in records {
        validate_slug_rules(record.kind, record.slug.as_deref())?;
    }
    Ok(())
}

fn memory_path(
    layout: &WorkspaceLayout,
    kind: MemoryToolKind,
    slug: Option<&str>,
) -> io::Result<PathBuf> {
    match kind {
        MemoryToolKind::Summary => {
            if slug.is_some() {
                return Err(invalid_input("summary records do not accept slug"));
            }
            Ok(layout.memory_dir().join("summary.md"))
        }
        MemoryToolKind::Decision => {
            Ok(record_dir(layout, kind).join(format!("{}.md", required_slug(slug)?)))
        }
        MemoryToolKind::Request => {
            Ok(record_dir(layout, kind).join(format!("{}.md", required_slug(slug)?)))
        }
    }
}

fn record_dir(layout: &WorkspaceLayout, kind: MemoryToolKind) -> PathBuf {
    match kind {
        MemoryToolKind::Summary => layout.memory_dir(),
        MemoryToolKind::Decision => layout.memory_dir().join("decisions"),
        MemoryToolKind::Request => layout.memory_dir().join("requests"),
    }
}

fn required_slug(slug: Option<&str>) -> io::Result<&str> {
    slug.filter(|value| !value.trim().is_empty())
        .ok_or_else(|| invalid_input("slug is required for this memory kind"))
}

fn validate_slug_rules(kind: MemoryToolKind, slug: Option<&str>) -> io::Result<()> {
    match kind {
        MemoryToolKind::Summary => {
            if slug.is_some() {
                return Err(invalid_input("summary records do not accept slug"));
            }
        }
        MemoryToolKind::Decision | MemoryToolKind::Request => {
            validate_slug(required_slug(slug)?)?;
        }
    }
    Ok(())
}

fn validate_slug(slug: &str) -> io::Result<()> {
    if slug.is_empty()
        || slug.contains('/')
        || slug.contains('\\')
        || slug == "."
        || slug == ".."
        || slug.starts_with('.')
        || !slug
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        return Err(invalid_input(
            "slug must use lowercase ASCII letters, digits, and '-' only",
        ));
    }
    Ok(())
}

fn validate_memory_content(
    kind: MemoryToolKind,
    slug: Option<&str>,
    content: &str,
) -> io::Result<()> {
    if content.trim().is_empty() {
        return Err(invalid_input("content must not be empty"));
    }
    match kind {
        MemoryToolKind::Summary => {
            if !content.trim_start().starts_with("# Memory summary") {
                return Err(invalid_input(
                    "summary content must start with '# Memory summary'",
                ));
            }
        }
        MemoryToolKind::Decision | MemoryToolKind::Request => {
            validate_slug_rules(kind, slug)?;
            if !content.trim_start().starts_with("---") {
                return Err(invalid_input(
                    "memory record content must start with YAML frontmatter",
                ));
            }
        }
    }
    Ok(())
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
                reason: "Merged into durable request memory.".into(),
                affected_memory: vec![MemoryStagingAffectedMemory {
                    kind: MemoryToolKind::Request,
                    slug: Some("review-preferences".into()),
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
        assert!(resolutions.contains("Merged into durable request memory."));
    }
}
