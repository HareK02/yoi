use memory::backend::{
    MemoryBackendAckOutput, MemoryBackendOperation, MemoryBackendOperationResult,
    MemoryDocumentReadOperation, MemoryDocumentUpdateOperation, MemoryQueryOperation,
    MemoryReadOperation, MemoryStagingCloseAction, MemoryStagingCloseOperation,
    MemoryStagingListOperation, MemoryStagingReadOperation, MemoryStagingWriteOutput,
    MemoryToolOutput,
};
use memory::extract::{ExtractedCandidate, StagingEvidence, StagingRecord};
use memory::schema::{SourceEvidenceRef, SourceRef};
use serde_json::json;
use uuid::Uuid;

use crate::authority::MemoryAuthority;
use crate::{Error, Result};

const DEFAULT_READ_LIMIT: usize = 2000;
const MAX_STAGING_LIST_LIMIT: usize = 100;

pub fn execute_memory_backend_operation_with_authority<A: MemoryAuthority>(
    authority: &A,
    operation: MemoryBackendOperation,
) -> Result<MemoryBackendOperationResult> {
    match operation {
        MemoryBackendOperation::Query(operation) => {
            execute_query(authority, operation).map(MemoryBackendOperationResult::ToolOutput)
        }
        MemoryBackendOperation::ReadDocument(operation) => execute_read_document(authority, operation)
            .map(MemoryBackendOperationResult::ToolOutput),
        MemoryBackendOperation::UpdateDocument(operation) => {
            execute_update_document(authority, operation).map(MemoryBackendOperationResult::ToolOutput)
        }
        MemoryBackendOperation::ResidentSummary(_operation) => Ok(
            MemoryBackendOperationResult::ToolOutput(execute_resident_summary(authority)?),
        ),
        MemoryBackendOperation::AppendAudit(_operation) => Ok(
            MemoryBackendOperationResult::Acknowledged(MemoryBackendAckOutput {
                summary: "memory audit event accepted by Workspace authority".to_string(),
            }),
        ),
        MemoryBackendOperation::StageCandidate(operation) => {
            let id = stage_candidate(
                authority,
                operation.source,
                operation.extract_run_id,
                operation.candidate,
                operation.evidence,
                operation.source_refs,
            )?;
            Ok(MemoryBackendOperationResult::StagingWritten(
                MemoryStagingWriteOutput {
                    staging_count: 1,
                    staging_ids: vec![id],
                },
            ))
        }
        MemoryBackendOperation::StageExtracted(operation) => {
            let extract_run_id = Uuid::now_v7().to_string();
            let mut ids = Vec::with_capacity(operation.payload.candidates.len());
            for candidate in operation.payload.candidates {
                ids.push(stage_candidate(
                    authority,
                    operation.source.clone(),
                    extract_run_id.clone(),
                    candidate,
                    Vec::new(),
                    Vec::new(),
                )?);
            }
            Ok(MemoryBackendOperationResult::StagingWritten(
                MemoryStagingWriteOutput {
                    staging_count: ids.len(),
                    staging_ids: ids,
                },
            ))
        }
        MemoryBackendOperation::StagingList(operation) => execute_staging_list(authority, operation)
            .map(MemoryBackendOperationResult::ToolOutput),
        MemoryBackendOperation::StagingRead(operation) => execute_staging_read(authority, operation)
            .map(MemoryBackendOperationResult::ToolOutput),
        MemoryBackendOperation::StagingClose(operation) => execute_staging_close(authority, operation)
            .map(MemoryBackendOperationResult::ToolOutput),
        MemoryBackendOperation::Read(operation) => execute_legacy_read(authority, operation)
            .map(MemoryBackendOperationResult::ToolOutput),
        MemoryBackendOperation::Write(_)
        | MemoryBackendOperation::Edit(_)
        | MemoryBackendOperation::Delete(_) => Err(Error::Store(
            "legacy summary/decision/request Memory mutation operations are not supported by Workspace authority; use MemoryUpdateDocument".to_string(),
        )),
    }
}

fn execute_resident_summary<A: MemoryAuthority>(authority: &A) -> Result<MemoryToolOutput> {
    let document = authority.ensure_memory_document()?;
    if document.body_md.trim().is_empty() {
        Ok(MemoryToolOutput {
            summary: "resident memory summary unavailable".to_string(),
            content: None,
        })
    } else {
        Ok(MemoryToolOutput {
            summary: "resident memory document collected".to_string(),
            content: Some(document.body_md),
        })
    }
}

fn execute_query<A: MemoryAuthority>(
    authority: &A,
    operation: MemoryQueryOperation,
) -> Result<MemoryToolOutput> {
    let document = authority.ensure_memory_document()?;
    let query = operation
        .query
        .as_deref()
        .map(str::trim)
        .filter(|query| !query.is_empty());
    match query {
        Some(query) => {
            if let Some(body) = excerpt_matching(&document.body_md, query) {
                Ok(MemoryToolOutput {
                    summary: format!("Memory document matched {query:?}"),
                    content: Some(body),
                })
            } else {
                Ok(MemoryToolOutput {
                    summary: format!("No memory document content matched {query:?}"),
                    content: None,
                })
            }
        }
        None => Ok(MemoryToolOutput {
            summary: "Memory document overview".to_string(),
            content: Some(excerpt_head(&document.body_md, 120)),
        }),
    }
}

fn execute_legacy_read<A: MemoryAuthority>(
    authority: &A,
    operation: MemoryReadOperation,
) -> Result<MemoryToolOutput> {
    if operation.kind.as_str() != "summary" || operation.slug.is_some() {
        return Err(Error::Store(
            "legacy kind/slug Memory reads are not supported by Workspace authority; use MemoryReadDocument".to_string(),
        ));
    }
    execute_read_document(
        authority,
        MemoryDocumentReadOperation {
            offset: operation.offset,
            limit: operation.limit,
        },
    )
}

fn execute_read_document<A: MemoryAuthority>(
    authority: &A,
    operation: MemoryDocumentReadOperation,
) -> Result<MemoryToolOutput> {
    let document = authority.ensure_memory_document()?;
    let offset = operation.offset.unwrap_or(0);
    let limit = operation.limit.unwrap_or(DEFAULT_READ_LIMIT);
    let lines: Vec<&str> = document.body_md.lines().collect();
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
            "Read {} lines from Workspace memory document",
            selected.lines().count()
        ),
        content: Some(format!(
            "total_lines={total} offset={offset} limit={limit}\n{selected}"
        )),
    })
}

fn execute_update_document<A: MemoryAuthority>(
    authority: &A,
    operation: MemoryDocumentUpdateOperation,
) -> Result<MemoryToolOutput> {
    if operation.body_md.trim().is_empty() {
        return Err(Error::Store(
            "memory document body_md must not be empty".to_string(),
        ));
    }
    let document = authority.update_memory_document(&operation.body_md)?;
    Ok(MemoryToolOutput {
        summary: format!(
            "Updated Workspace memory document ({} bytes)",
            document.body_md.len()
        ),
        content: Some(format!("updated_at={}", document.updated_at)),
    })
}

fn stage_candidate<A: MemoryAuthority>(
    authority: &A,
    source: SourceRef,
    extract_run_id: String,
    candidate: ExtractedCandidate,
    evidence: Vec<StagingEvidence>,
    source_refs: Vec<SourceEvidenceRef>,
) -> Result<String> {
    let candidate_id = Uuid::now_v7().to_string();
    let record = StagingRecord::from_candidate(
        candidate_id.clone(),
        extract_run_id,
        source,
        candidate,
        evidence,
        source_refs,
    );
    let raw_json = serde_json::to_string_pretty(&record)
        .map_err(|err| Error::Store(format!("serialize memory staging record: {err}")))?;
    authority.upsert_memory_staging_record(&candidate_id, &raw_json, None)?;
    Ok(candidate_id)
}

fn execute_staging_list<A: MemoryAuthority>(
    authority: &A,
    operation: MemoryStagingListOperation,
) -> Result<MemoryToolOutput> {
    let limit = operation.limit.unwrap_or(20).min(MAX_STAGING_LIST_LIMIT);
    let entries = authority.list_memory_staging_records(limit)?;
    let records = entries
        .iter()
        .map(|entry| {
            staging_summary_json(
                &entry.candidate_id,
                &entry.raw_json,
                entry.source_path.as_deref(),
            )
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(MemoryToolOutput {
        summary: format!(
            "Listed {} memory staging candidate(s) from Workspace authority",
            records.len()
        ),
        content: Some(
            serde_json::to_string_pretty(&records)
                .map_err(|err| Error::Store(format!("serialize memory staging list: {err}")))?,
        ),
    })
}

fn execute_staging_read<A: MemoryAuthority>(
    authority: &A,
    operation: MemoryStagingReadOperation,
) -> Result<MemoryToolOutput> {
    let entry = authority.memory_staging_record(&operation.candidate_id)?;
    let raw: serde_json::Value = serde_json::from_str(&entry.raw_json)
        .map_err(|err| Error::Store(format!("decode memory staging record: {err}")))?;
    Ok(MemoryToolOutput {
        summary: format!("Read memory staging candidate {}", entry.candidate_id),
        content: Some(
            serde_json::to_string_pretty(&raw)
                .map_err(|err| Error::Store(format!("serialize memory staging record: {err}")))?,
        ),
    })
}

fn execute_staging_close<A: MemoryAuthority>(
    authority: &A,
    operation: MemoryStagingCloseOperation,
) -> Result<MemoryToolOutput> {
    if operation.reason.trim().is_empty() {
        return Err(Error::Store("reason is required".to_string()));
    }
    let affected_refs_json = serde_json::to_string(&operation.affected_memory)
        .map_err(|err| Error::Store(format!("serialize affected memory refs: {err}")))?;
    let action = staging_close_action_name(&operation.action);
    let resolution = authority.close_memory_staging_record(
        &operation.candidate_id,
        action,
        &operation.reason,
        &affected_refs_json,
    )?;
    Ok(MemoryToolOutput {
        summary: format!("Closed memory staging candidate {}", resolution.candidate_id),
        content: Some(
            serde_json::to_string_pretty(&json!({
                "candidate_id": resolution.candidate_id,
                "action": resolution.action,
                "reason": resolution.reason,
                "affected_refs": serde_json::from_str::<serde_json::Value>(&resolution.affected_refs_json).unwrap_or_else(|_| json!([])),
                "resolved_at": resolution.resolved_at,
                "record_authority": resolution.record_source,
            }))
            .map_err(|err| Error::Store(format!("serialize memory staging resolution: {err}")))?,
        ),
    })
}

fn staging_close_action_name(action: &MemoryStagingCloseAction) -> &'static str {
    match action {
        MemoryStagingCloseAction::Applied => "applied",
        MemoryStagingCloseAction::Discarded => "discarded",
        MemoryStagingCloseAction::Invalid => "invalid",
        MemoryStagingCloseAction::Duplicate => "duplicate",
        MemoryStagingCloseAction::AlreadyCovered => "already_covered",
    }
}

fn staging_summary_json(
    candidate_id: &str,
    raw_json: &str,
    source_path: Option<&str>,
) -> Result<serde_json::Value> {
    let raw: serde_json::Value = serde_json::from_str(raw_json).map_err(|err| {
        Error::Store(format!(
            "decode memory staging record {candidate_id}: {err}"
        ))
    })?;
    Ok(json!({
        "candidate_id": candidate_id,
        "bytes": raw_json.len(),
        "source_path": source_path,
        "source": raw.get("source").cloned().unwrap_or(serde_json::Value::Null),
        "kind": raw.get("kind").cloned().unwrap_or(serde_json::Value::Null),
        "claim": raw.get("claim").cloned().unwrap_or(serde_json::Value::Null),
        "why_useful": raw.get("why_useful").cloned().unwrap_or(serde_json::Value::Null),
        "staleness": raw.get("staleness").cloned().unwrap_or(serde_json::Value::Null),
        "evidence_count": raw.get("evidence").and_then(|value| value.as_array()).map_or(0, Vec::len),
        "source_ref_count": raw.get("source_refs").and_then(|value| value.as_array()).map_or(0, Vec::len),
    }))
}

fn excerpt_head(body: &str, max_lines: usize) -> String {
    let mut lines = body.lines().take(max_lines).collect::<Vec<_>>().join("\n");
    if body.lines().count() > max_lines {
        lines.push_str("\n... [truncated]");
    }
    lines
}

fn excerpt_matching(body: &str, query: &str) -> Option<String> {
    let query_lower = query.to_lowercase();
    let lines = body.lines().collect::<Vec<_>>();
    let index = lines
        .iter()
        .position(|line| line.to_lowercase().contains(&query_lower))?;
    let start = index.saturating_sub(3);
    let end = (index + 4).min(lines.len());
    Some(
        lines[start..end]
            .iter()
            .enumerate()
            .map(|(offset, line)| format!("{:>6}\t{}", start + offset + 1, line))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority::SqliteWorkspaceAuthority;
    use memory::backend::{
        MemoryDocumentReadOperation, MemoryDocumentUpdateOperation, MemoryStagingCloseAction,
        MemoryStagingCloseOperation, MemoryStagingListOperation,
    };
    use memory::extract::{CandidateKind, ExtractedCandidate};
    use memory::schema::SourceRef;
    use tempfile::TempDir;

    fn authority(temp: &TempDir) -> SqliteWorkspaceAuthority {
        let db_path = temp.path().join("workspace.sqlite3");
        let authority = SqliteWorkspaceAuthority::new(&db_path, "workspace").unwrap();
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "INSERT INTO workspaces (
                workspace_id, owner_account_id, display_name, state, created_at, updated_at
            ) VALUES (?1, NULL, ?2, ?3, ?4, ?4)",
            rusqlite::params!["workspace", "Workspace", "active", "2026-01-01T00:00:00Z"],
        )
        .unwrap();
        authority
    }

    fn source() -> SourceRef {
        SourceRef {
            segment_id: "segment-1".into(),
            range: [1, 2],
        }
    }

    fn candidate() -> ExtractedCandidate {
        ExtractedCandidate {
            kind: CandidateKind::Decision,
            claim: "Use SQLite memory authority".into(),
            why_useful: "Keeps Worker tools away from .yoi/memory filesystem authority".into(),
            staleness: None,
            evidence_ids: Vec::new(),
        }
    }

    #[test]
    fn document_operations_use_workspace_authority() {
        let temp = TempDir::new().unwrap();
        let authority = authority(&temp);
        execute_memory_backend_operation_with_authority(
            &authority,
            MemoryBackendOperation::UpdateDocument(MemoryDocumentUpdateOperation {
                body_md: "# Memory\n\nSQLite backed body".into(),
            }),
        )
        .unwrap();

        let result = execute_memory_backend_operation_with_authority(
            &authority,
            MemoryBackendOperation::ReadDocument(MemoryDocumentReadOperation {
                offset: None,
                limit: None,
            }),
        )
        .unwrap();

        let MemoryBackendOperationResult::ToolOutput(output) = result else {
            panic!("expected tool output")
        };
        assert!(output.content.unwrap().contains("SQLite backed body"));
    }

    #[test]
    fn staging_close_moves_record_to_workspace_resolution() {
        let temp = TempDir::new().unwrap();
        let authority = authority(&temp);
        let result = execute_memory_backend_operation_with_authority(
            &authority,
            MemoryBackendOperation::StageCandidate(
                memory::backend::MemoryStageCandidateOperation {
                    source: source(),
                    extract_run_id: "run-1".into(),
                    candidate: candidate(),
                    evidence: Vec::new(),
                    source_refs: Vec::new(),
                },
            ),
        )
        .unwrap();
        let MemoryBackendOperationResult::StagingWritten(written) = result else {
            panic!("expected staging write")
        };
        assert_eq!(written.staging_count, 1);

        let list = execute_memory_backend_operation_with_authority(
            &authority,
            MemoryBackendOperation::StagingList(MemoryStagingListOperation { limit: None }),
        )
        .unwrap();
        let MemoryBackendOperationResult::ToolOutput(list) = list else {
            panic!("expected list")
        };
        assert!(
            list.content
                .unwrap()
                .contains("Use SQLite memory authority")
        );

        execute_memory_backend_operation_with_authority(
            &authority,
            MemoryBackendOperation::StagingClose(MemoryStagingCloseOperation {
                candidate_id: written.staging_ids[0].clone(),
                action: MemoryStagingCloseAction::Applied,
                reason: "merged into document".into(),
                affected_memory: Vec::new(),
            }),
        )
        .unwrap();

        assert_eq!(authority.list_memory_staging_records(10).unwrap().len(), 0);
        assert_eq!(
            authority.list_memory_staging_resolutions(10).unwrap().len(),
            1
        );
    }
}
