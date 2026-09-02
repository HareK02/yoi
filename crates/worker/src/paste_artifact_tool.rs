//! Bounded model-facing access to session-owned large paste artifacts.

use std::sync::Arc;

use agen::tool::{Tool, ToolDefinition, ToolError, ToolExecutionContext, ToolMeta, ToolOutput};
use async_trait::async_trait;
use protocol::{PasteArtifactAvailability, PasteArtifactMediaType, PasteArtifactRef};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use session_store::{SessionId, Store, StoreError};

const MAX_QUERY_BYTES: usize = 256;
const DEFAULT_SEARCH_RESULTS: usize = 20;
const MAX_SEARCH_RESULTS: usize = 100;
const MAX_SNIPPET_CHARS: usize = 300;
const DEFAULT_READ_BYTES: usize = 8 * 1024;
const MAX_READ_BYTES: usize = 16 * 1024;

const SEARCH_DESCRIPTION: &str = "Search one large pasted-input artifact owned by the current Worker. Returns bounded matching line snippets; never returns the whole artifact.";
const READ_DESCRIPTION: &str = "Read a bounded UTF-8 byte range from one large pasted-input artifact owned by the current Worker. Use next_offset for repeated calls instead of requesting the whole artifact.";

#[derive(Clone)]
struct ArtifactAccess<St: Store + Clone> {
    store: St,
    session_id: SessionId,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SearchInputArtifactInput {
    /// Opaque artifact id from a large-paste history reference.
    artifact_id: String,
    /// Literal case-sensitive text to find.
    query: String,
    /// Maximum matching lines to return (1..=100).
    max_results: Option<usize>,
}

#[derive(Debug, Serialize)]
struct SearchInputArtifactOutput {
    artifact_id: String,
    matches: Vec<SearchMatch>,
    truncated: bool,
}

#[derive(Debug, Serialize)]
struct SearchMatch {
    line: u64,
    byte_offset: u64,
    snippet: String,
}

struct SearchInputArtifactTool<St: Store + Clone> {
    access: ArtifactAccess<St>,
}

#[async_trait]
impl<St> Tool for SearchInputArtifactTool<St>
where
    St: Store + Clone + Send + Sync + 'static,
{
    async fn execute(
        &self,
        input_json: &str,
        _context: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let input: SearchInputArtifactInput =
            serde_json::from_str(input_json).map_err(|error| {
                ToolError::InvalidArgument(format!("invalid SearchInputArtifact input: {error}"))
            })?;
        if input.query.is_empty() || input.query.len() > MAX_QUERY_BYTES {
            return Err(ToolError::InvalidArgument(
                "query must contain 1..=256 UTF-8 bytes".to_string(),
            ));
        }
        let max_results = input
            .max_results
            .unwrap_or(DEFAULT_SEARCH_RESULTS)
            .clamp(1, MAX_SEARCH_RESULTS);
        let (_, content) =
            read_artifact_text(&self.access, &input.artifact_id).map_err(tool_store_error)?;
        let mut matches = Vec::new();
        let mut truncated = false;
        let mut byte_offset = 0_u64;
        for (index, raw_line) in content.split_inclusive('\n').enumerate() {
            let line = raw_line.strip_suffix('\n').unwrap_or(raw_line);
            if line.contains(&input.query) {
                if matches.len() == max_results {
                    truncated = true;
                    break;
                }
                matches.push(SearchMatch {
                    line: index as u64 + 1,
                    byte_offset,
                    snippet: truncate_chars(line, MAX_SNIPPET_CHARS),
                });
            }
            byte_offset += raw_line.len() as u64;
        }
        json_output(
            format!("Found {} matching pasted-input line(s).", matches.len()),
            &SearchInputArtifactOutput {
                artifact_id: input.artifact_id,
                matches,
                truncated,
            },
        )
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ReadInputArtifactInput {
    /// Opaque artifact id from a large-paste history reference.
    artifact_id: String,
    /// UTF-8 byte offset to start reading. Defaults to 0 and must be a character boundary.
    offset: Option<u64>,
    /// Maximum UTF-8 bytes to return (4..=16384). Defaults to 8192.
    max_bytes: Option<usize>,
}

#[derive(Debug, Serialize)]
struct ReadInputArtifactOutput {
    artifact_id: String,
    offset: u64,
    content: String,
    next_offset: Option<u64>,
    truncated: bool,
}

struct ReadInputArtifactTool<St: Store + Clone> {
    access: ArtifactAccess<St>,
}

#[async_trait]
impl<St> Tool for ReadInputArtifactTool<St>
where
    St: Store + Clone + Send + Sync + 'static,
{
    async fn execute(
        &self,
        input_json: &str,
        _context: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let input: ReadInputArtifactInput = serde_json::from_str(input_json).map_err(|error| {
            ToolError::InvalidArgument(format!("invalid ReadInputArtifact input: {error}"))
        })?;
        let offset = input.offset.unwrap_or(0);
        let max_bytes = input
            .max_bytes
            .unwrap_or(DEFAULT_READ_BYTES)
            .clamp(4, MAX_READ_BYTES);
        let (_, content) =
            read_artifact_text(&self.access, &input.artifact_id).map_err(tool_store_error)?;
        let offset = usize::try_from(offset).map_err(|_| {
            ToolError::InvalidArgument("offset exceeds the artifact size".to_string())
        })?;
        if offset > content.len() || !content.is_char_boundary(offset) {
            return Err(ToolError::InvalidArgument(
                "offset must be a UTF-8 character boundary within the artifact".to_string(),
            ));
        }
        let mut end = offset.saturating_add(max_bytes).min(content.len());
        while end > offset && !content.is_char_boundary(end) {
            end -= 1;
        }
        let output = content[offset..end].to_string();
        let next_offset = (end < content.len()).then_some(end as u64);
        let truncated = next_offset.is_some();

        json_output(
            format!("Read {} pasted-input byte(s).", output.len()),
            &ReadInputArtifactOutput {
                artifact_id: input.artifact_id,
                offset: offset as u64,
                content: output,
                next_offset,
                truncated,
            },
        )
    }
}

pub(crate) fn search_input_artifact_tool<St>(store: St, session_id: SessionId) -> ToolDefinition
where
    St: Store + Clone + Send + Sync + 'static,
{
    Arc::new(move || {
        let schema = serde_json::to_value(schemars::schema_for!(SearchInputArtifactInput))
            .unwrap_or_else(|_| serde_json::json!({}));
        let meta = ToolMeta::new("SearchInputArtifact")
            .description(SEARCH_DESCRIPTION)
            .input_schema(schema);
        let tool: Arc<dyn Tool> = Arc::new(SearchInputArtifactTool {
            access: ArtifactAccess {
                store: store.clone(),
                session_id,
            },
        });
        (meta, tool)
    })
}

pub(crate) fn read_input_artifact_tool<St>(store: St, session_id: SessionId) -> ToolDefinition
where
    St: Store + Clone + Send + Sync + 'static,
{
    Arc::new(move || {
        let schema = serde_json::to_value(schemars::schema_for!(ReadInputArtifactInput))
            .unwrap_or_else(|_| serde_json::json!({}));
        let meta = ToolMeta::new("ReadInputArtifact")
            .description(READ_DESCRIPTION)
            .input_schema(schema);
        let tool: Arc<dyn Tool> = Arc::new(ReadInputArtifactTool {
            access: ArtifactAccess {
                store: store.clone(),
                session_id,
            },
        });
        (meta, tool)
    })
}

fn json_output(summary: String, value: &impl Serialize) -> Result<ToolOutput, ToolError> {
    let content = serde_json::to_string(value)
        .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
    Ok(ToolOutput {
        summary,
        content: Some(content),
        attachments: Vec::new(),
    })
}

fn read_artifact_text<St: Store + Clone>(
    access: &ArtifactAccess<St>,
    artifact_id: &str,
) -> Result<(PasteArtifactRef, String), StoreError> {
    match access
        .store
        .read_paste_artifact(access.session_id, artifact_id)
    {
        Ok(result) => Ok(result),
        Err(paste_error) => {
            let (file, bytes) = match access
                .store
                .read_uploaded_file_by_id(access.session_id, artifact_id)
            {
                Ok(result) => result,
                Err(_) => return Err(paste_error),
            };
            let content =
                String::from_utf8(bytes).map_err(|_| StoreError::ArtifactIntegrityMismatch)?;
            let char_count =
                u64::try_from(content.chars().count()).map_err(|_| StoreError::ArtifactTooLarge)?;
            let line_count =
                u64::try_from(content.lines().count()).map_err(|_| StoreError::ArtifactTooLarge)?;
            Ok((
                PasteArtifactRef {
                    artifact_id: file.artifact_id,
                    created_at_ms: file.created_at_ms,
                    media_type: PasteArtifactMediaType::TextPlainUtf8,
                    availability: PasteArtifactAvailability::Available,
                    byte_len: file.byte_len,
                    char_count,
                    line_count,
                    sha256: file.sha256,
                    source_entry_id: file.source_entry_id.unwrap_or_default(),
                },
                content,
            ))
        }
    }
}

fn tool_store_error(error: StoreError) -> ToolError {
    let message = match error {
        StoreError::PasteArtifactNotFound(_) => "paste artifact not found",
        StoreError::PasteArtifactIntegrity(_) | StoreError::Corrupt { .. } => {
            "paste artifact failed its integrity check"
        }
        StoreError::PasteArtifactUnsupported => "paste artifact storage is unavailable",
        _ => "paste artifact is unavailable",
    };
    ToolError::ExecutionFailed(message.to_string())
}

fn truncate_chars(value: &str, limit: usize) -> String {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(limit).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use agen::tool::ToolExecutionContext;
    use session_store::{FsStore, PasteArtifactLimits, Store, new_session_id};

    use super::*;

    #[tokio::test]
    async fn read_input_artifact_reads_uploaded_text_but_rejects_binary_content() {
        let temp = tempfile::TempDir::new().unwrap();
        let store = FsStore::new(temp.path()).unwrap();
        let owner = new_session_id();
        let text = store
            .write_uploaded_file(
                owner,
                "notes.md",
                "text/markdown",
                b"alpha\nbeta",
                session_store::UploadedFileLimits::default(),
            )
            .unwrap();
        let read = ReadInputArtifactTool {
            access: ArtifactAccess {
                store: store.clone(),
                session_id: owner,
            },
        };
        let output = read
            .execute(
                &serde_json::json!({
                    "artifact_id": text.artifact_id,
                    "offset": 0,
                    "max_bytes": 64
                })
                .to_string(),
                ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        let output: serde_json::Value =
            serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
        assert_eq!(output["content"], "alpha\nbeta");

        let binary = store
            .write_uploaded_file(
                owner,
                "image.png",
                "image/png",
                b"\x89PNG\r\n\x1a\nbody",
                session_store::UploadedFileLimits::default(),
            )
            .unwrap();
        let error = read
            .execute(
                &serde_json::json!({
                    "artifact_id": binary.artifact_id,
                    "offset": 0,
                    "max_bytes": 64
                })
                .to_string(),
                ToolExecutionContext::default(),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("unavailable"));
    }

    #[tokio::test]
    async fn search_and_read_are_bounded_and_owner_scoped() {
        let temp = tempfile::TempDir::new().unwrap();
        let store = FsStore::new(temp.path()).unwrap();
        let owner = new_session_id();
        let other = new_session_id();
        let content = (0..700)
            .map(|index| format!("line {index}: needle {}", "x".repeat(80)))
            .collect::<Vec<_>>()
            .join("\n");
        let artifact = store
            .write_paste_artifact(owner, "entry-1", &content, PasteArtifactLimits::default())
            .unwrap();

        let search = SearchInputArtifactTool {
            access: ArtifactAccess {
                store: store.clone(),
                session_id: owner,
            },
        };
        let searched = search
            .execute(
                &serde_json::json!({
                    "artifact_id": artifact.artifact_id,
                    "query": "needle",
                    "max_results": 3
                })
                .to_string(),
                ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        let searched: serde_json::Value =
            serde_json::from_str(searched.content.as_deref().unwrap()).unwrap();
        assert_eq!(searched["matches"].as_array().unwrap().len(), 3);
        assert_eq!(searched["truncated"], true);

        let read = ReadInputArtifactTool {
            access: ArtifactAccess {
                store: store.clone(),
                session_id: owner,
            },
        };
        let read_output = read
            .execute(
                &serde_json::json!({
                    "artifact_id": artifact.artifact_id,
                    "offset": 2,
                    "max_bytes": 999999
                })
                .to_string(),
                ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        let read_output: serde_json::Value =
            serde_json::from_str(read_output.content.as_deref().unwrap()).unwrap();
        assert!(read_output["content"].as_str().unwrap().len() <= MAX_READ_BYTES);
        assert_eq!(read_output["truncated"], true);
        assert!(read_output["next_offset"].as_u64().is_some());

        let foreign = ReadInputArtifactTool {
            access: ArtifactAccess {
                store,
                session_id: other,
            },
        };
        let error = foreign
            .execute(
                &serde_json::json!({
                    "artifact_id": artifact.artifact_id,
                    "offset": 0,
                    "max_bytes": 1
                })
                .to_string(),
                ToolExecutionContext::default(),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("paste artifact not found"));
    }
}
