//! `MemoryRead` tool.
//!
//! Reads a memory or knowledge record by `(kind, slug)`. Returns
//! line-numbered content (1-based), like the generic Read tool. The
//! agent never names a path — `Search` returns `{kind, slug, ...}`
//! and that pair feeds straight into Read.

use std::sync::Arc;

use async_trait::async_trait;
use llm_worker::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use serde::Deserialize;

use crate::tool::MemoryToolKind;
use crate::usage::{self, UsageSource};
use crate::workspace::WorkspaceLayout;

const DESCRIPTION: &str = "Read a memory or knowledge record by `kind` + `slug`. \
`kind` is one of: summary, decision, request, knowledge. \
For `summary` omit `slug`; for the others `slug` is required. \
Returns line-numbered output (1-based).";

const DEFAULT_LIMIT: usize = 2000;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ReadParams {
    /// Record kind: `summary` | `decision` | `request` | `knowledge`.
    kind: MemoryToolKind,
    /// Slug. Required for everything except `summary`; forbidden for `summary`.
    #[serde(default)]
    slug: Option<String>,
    /// 0-based line offset from the start. Defaults to 0.
    #[serde(default)]
    offset: Option<usize>,
    /// Maximum number of lines to return. Defaults to 2000.
    #[serde(default)]
    limit: Option<usize>,
}

struct ReadTool {
    layout: WorkspaceLayout,
    usage_session_id: Option<String>,
}

#[async_trait]
impl Tool for ReadTool {
    async fn execute(&self, input_json: &str) -> Result<ToolOutput, ToolError> {
        let params: ReadParams = serde_json::from_str(input_json)
            .map_err(|e| ToolError::InvalidArgument(format!("invalid MemoryRead input: {e}")))?;

        let path = params
            .kind
            .resolve_path(&self.layout, params.slug.as_deref())?;

        let bytes = std::fs::read(&path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                ToolError::ExecutionFailed(format!("record not found: {}", path.display()))
            }
            _ => ToolError::ExecutionFailed(format!("read failed at {}: {e}", path.display())),
        })?;

        let text = String::from_utf8_lossy(&bytes).into_owned();
        if let Some(segment_id) = self.usage_session_id.as_deref() {
            let usage_slug = params.slug.as_deref().unwrap_or("summary");
            let snapshot = usage::snapshot_record_from_bytes(
                params.kind.record_kind(),
                usage_slug.to_string(),
                &bytes,
            );
            if let Err(err) = usage::append_use_event(
                &self.layout,
                segment_id.to_string(),
                UsageSource::MemoryRead,
                vec![snapshot],
            ) {
                tracing::warn!(error = %err, "failed to append MemoryRead usage event");
            }
        }
        let offset = params.offset.unwrap_or(0);
        let limit = params.limit.unwrap_or(DEFAULT_LIMIT).max(1);
        let rendered = render_numbered(&text, offset, limit);

        let summary = if rendered.truncated {
            format!(
                "Read {} line(s) [{}..{}] of {} from {}",
                rendered.line_count,
                offset + 1,
                offset + rendered.line_count,
                rendered.total_lines,
                path.display()
            )
        } else {
            format!(
                "Read {} line(s) from {}",
                rendered.line_count,
                path.display()
            )
        };

        Ok(ToolOutput {
            summary,
            content: Some(rendered.body),
        })
    }
}

struct Rendered {
    body: String,
    line_count: usize,
    total_lines: usize,
    truncated: bool,
}

fn render_numbered(text: &str, offset: usize, limit: usize) -> Rendered {
    let all_lines: Vec<&str> = text.lines().collect();
    let total_lines = all_lines.len();
    let start = offset.min(total_lines);
    let end = start.saturating_add(limit).min(total_lines);
    let slice = &all_lines[start..end];
    let line_count = slice.len();

    use std::fmt::Write as _;
    let mut body = String::with_capacity(text.len().saturating_add(line_count * 8));
    for (i, line) in slice.iter().enumerate() {
        let lineno = start + i + 1;
        let _ = writeln!(&mut body, "{:>6}\t{}", lineno, line);
    }

    Rendered {
        body,
        line_count,
        total_lines,
        truncated: start > 0 || end < total_lines,
    }
}

pub fn read_tool(layout: WorkspaceLayout) -> ToolDefinition {
    read_tool_inner(layout, None)
}

pub fn read_tool_with_usage(
    layout: WorkspaceLayout,
    segment_id: impl Into<String>,
) -> ToolDefinition {
    read_tool_inner(layout, Some(segment_id.into()))
}

fn read_tool_inner(layout: WorkspaceLayout, usage_session_id: Option<String>) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(ReadParams);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("MemoryRead")
            .description(DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(ReadTool {
            layout: layout.clone(),
            usage_session_id: usage_session_id.clone(),
        });
        (meta, tool)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup() -> (TempDir, WorkspaceLayout) {
        let dir = TempDir::new().unwrap();
        let layout = WorkspaceLayout::new(dir.path().to_path_buf());
        (dir, layout)
    }

    #[tokio::test]
    async fn read_decision_by_slug() {
        let (dir, layout) = setup();
        let path = dir.path().join(".insomnia/memory/decisions/foo.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "alpha\nbeta\n").unwrap();

        let (_meta, tool) = read_tool(layout)();
        let inp = serde_json::json!({ "kind": "decision", "slug": "foo" });
        let out = tool.execute(&inp.to_string()).await.unwrap();
        let body = out.content.unwrap();
        assert!(body.contains("     1\talpha"));
        assert!(body.contains("     2\tbeta"));
    }

    #[tokio::test]
    async fn read_summary_without_slug() {
        let (dir, layout) = setup();
        let path = dir.path().join(".insomnia/memory/summary.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "summary body\n").unwrap();

        let (_, tool) = read_tool(layout)();
        let inp = serde_json::json!({ "kind": "summary" });
        let out = tool.execute(&inp.to_string()).await.unwrap();
        assert!(out.content.unwrap().contains("summary body"));
    }

    #[tokio::test]
    async fn summary_with_slug_rejected() {
        let (_dir, layout) = setup();
        let (_, tool) = read_tool(layout)();
        let inp = serde_json::json!({ "kind": "summary", "slug": "x" });
        let err = tool.execute(&inp.to_string()).await.unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn decision_without_slug_rejected() {
        let (_dir, layout) = setup();
        let (_, tool) = read_tool(layout)();
        let inp = serde_json::json!({ "kind": "decision" });
        let err = tool.execute(&inp.to_string()).await.unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn invalid_slug_rejected() {
        let (_dir, layout) = setup();
        let (_, tool) = read_tool(layout)();
        let inp = serde_json::json!({ "kind": "decision", "slug": "Bad-Slug" });
        let err = tool.execute(&inp.to_string()).await.unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn knowledge_path_resolution() {
        let (dir, layout) = setup();
        let path = dir.path().join(".insomnia/knowledge/policy.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "k\n").unwrap();

        let (_, tool) = read_tool(layout)();
        let inp = serde_json::json!({ "kind": "knowledge", "slug": "policy" });
        let out = tool.execute(&inp.to_string()).await.unwrap();
        assert!(out.content.unwrap().contains("k"));
    }

    #[tokio::test]
    async fn read_logs_explicit_use_when_usage_session_is_set() {
        let (dir, layout) = setup();
        let path = dir.path().join(".insomnia/memory/decisions/foo.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "alpha\n").unwrap();

        let (_, tool) = read_tool_with_usage(layout.clone(), "session-1")();
        let inp = serde_json::json!({ "kind": "decision", "slug": "foo" });
        tool.execute(&inp.to_string()).await.unwrap();

        let report = usage::build_usage_report(&layout).unwrap();
        assert_eq!(report.records.len(), 1);
        let record = &report.records[0];
        assert_eq!(record.kind, "decision");
        assert_eq!(record.slug, "foo");
        assert_eq!(record.use_count, 1);
        assert_eq!(record.source_breakdown["MemoryRead"], 1);
        assert_eq!(record.resident_exposure_count, 0);
    }

    #[tokio::test]
    async fn missing_file_returns_execution_failed() {
        let (_dir, layout) = setup();
        let (_, tool) = read_tool(layout)();
        let inp = serde_json::json!({ "kind": "decision", "slug": "missing" });
        let err = tool.execute(&inp.to_string()).await.unwrap_err();
        assert!(matches!(err, ToolError::ExecutionFailed(_)));
    }
}
