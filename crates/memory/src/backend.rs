//! Workspace-backend operations for Memory tools.
//!
//! These types are the typed HTTP boundary used by runtime workers that have
//! Workspace authority but no direct local filesystem authority. The local
//! executor intentionally lives in the memory crate so the workspace backend and
//! future non-HTTP hosts share validation and path semantics.

use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::extract::{ExtractedPayload, write_staging};
use crate::schema::SourceRef;
use crate::tool::MemoryToolKind;
use crate::workspace::WorkspaceLayout;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum MemoryBackendOperation {
    Query(MemoryQueryOperation),
    Read(MemoryReadOperation),
    Write(MemoryWriteOperation),
    Edit(MemoryEditOperation),
    Delete(MemoryDeleteOperation),
    StageExtracted(MemoryStageExtractedOperation),
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStageExtractedOperation {
    pub source: SourceRef,
    pub payload: ExtractedPayload,
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
        MemoryBackendOperation::StageExtracted(operation) => {
            let written = write_staging(layout, operation.source, operation.payload)?;
            Ok(MemoryBackendOperationResult::StagingWritten(
                MemoryStagingWriteOutput {
                    staging_count: written.len(),
                    staging_ids: written.iter().map(|item| item.id.to_string()).collect(),
                },
            ))
        }
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
