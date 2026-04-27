//! `MemoryRead` tool.
//!
//! Constrained to `<workspace>/memory/` and `<workspace>/knowledge/`
//! paths. Returns line-numbered content (1-based), like the generic
//! Read tool, but rejects anything outside the memory tree so the
//! agent can't sneak in a non-memory read through this surface.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use llm_worker::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use serde::Deserialize;

use crate::workspace::WorkspaceLayout;

const DESCRIPTION: &str = "Read a memory or knowledge record file under the \
workspace's `memory/` or `knowledge/` tree. Returns line-numbered output \
(1-based). Paths must be absolute and lie inside the memory tree.";

const DEFAULT_LIMIT: usize = 2000;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ReadParams {
    /// Absolute path to a file under the workspace's `memory/` or `knowledge/` tree.
    file_path: PathBuf,
    /// 0-based line offset from the start. Defaults to 0.
    #[serde(default)]
    offset: Option<usize>,
    /// Maximum number of lines to return. Defaults to 2000.
    #[serde(default)]
    limit: Option<usize>,
}

struct ReadTool {
    layout: WorkspaceLayout,
}

#[async_trait]
impl Tool for ReadTool {
    async fn execute(&self, input_json: &str) -> Result<ToolOutput, ToolError> {
        let params: ReadParams = serde_json::from_str(input_json).map_err(|e| {
            ToolError::InvalidArgument(format!("invalid MemoryRead input: {e}"))
        })?;

        if !params.file_path.is_absolute() {
            return Err(ToolError::InvalidArgument(format!(
                "file_path must be absolute: {}",
                params.file_path.display()
            )));
        }
        if self
            .layout
            .classify(&params.file_path)
            .map_err(|e| ToolError::InvalidArgument(e.to_string()))?
            .is_none()
        {
            return Err(ToolError::InvalidArgument(format!(
                "path is not under the memory tree: {}",
                params.file_path.display()
            )));
        }

        let bytes = std::fs::read(&params.file_path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => ToolError::ExecutionFailed(format!(
                "file not found: {}",
                params.file_path.display()
            )),
            _ => ToolError::ExecutionFailed(format!(
                "read failed at {}: {e}",
                params.file_path.display()
            )),
        })?;

        let text = String::from_utf8_lossy(&bytes).into_owned();
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
                params.file_path.display()
            )
        } else {
            format!(
                "Read {} line(s) from {}",
                rendered.line_count,
                params.file_path.display()
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
    Arc::new(move || {
        let schema = schemars::schema_for!(ReadParams);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("MemoryRead")
            .description(DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(ReadTool {
            layout: layout.clone(),
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
    async fn read_returns_numbered_lines() {
        let (dir, layout) = setup();
        let path = dir.path().join("memory/decisions/foo.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "alpha\nbeta\n").unwrap();

        let (_meta, tool) = read_tool(layout)();
        let inp = serde_json::json!({ "file_path": path.to_str().unwrap() });
        let out = tool.execute(&inp.to_string()).await.unwrap();
        let body = out.content.unwrap();
        assert!(body.contains("     1\talpha"));
        assert!(body.contains("     2\tbeta"));
    }

    #[tokio::test]
    async fn rejects_outside_memory_tree() {
        let (dir, layout) = setup();
        let other = dir.path().join("src/main.rs");
        std::fs::create_dir_all(other.parent().unwrap()).unwrap();
        std::fs::write(&other, "fn main() {}").unwrap();

        let (_, tool) = read_tool(layout)();
        let inp = serde_json::json!({ "file_path": other.to_str().unwrap() });
        let err = tool.execute(&inp.to_string()).await.unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn rejects_relative_path() {
        let (_dir, layout) = setup();
        let (_, tool) = read_tool(layout)();
        let inp = serde_json::json!({ "file_path": "memory/summary.md" });
        let err = tool.execute(&inp.to_string()).await.unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgument(_)));
    }
}
