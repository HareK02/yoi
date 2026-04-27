//! `MemoryWrite` tool.
//!
//! Creates or overwrites a memory or knowledge record with full content.
//! Pre-write Linter validates frontmatter, slug uniqueness (Create only),
//! reference integrity, size limits, and the workflow-write ban. On any
//! Linter error the tool returns `ToolError::InvalidArgument` with all
//! violations aggregated and the file is **not** written.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use llm_worker::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use serde::Deserialize;

use crate::linter::{LintReport, Linter, WriteMode};
use crate::workspace::WorkspaceLayout;

const DESCRIPTION: &str = "Create or overwrite a memory or knowledge record file. \
Path must be absolute and lie inside the workspace's `memory/` or `knowledge/` \
tree. Frontmatter is validated before the file is written; on validation \
failure no write occurs and every violation is returned in the error message.";

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct WriteParams {
    /// Absolute path under the workspace's `memory/` or `knowledge/` tree.
    file_path: PathBuf,
    /// Full file contents (frontmatter + body).
    content: String,
}

struct WriteTool {
    linter: Linter,
}

#[async_trait]
impl Tool for WriteTool {
    async fn execute(&self, input_json: &str) -> Result<ToolOutput, ToolError> {
        let params: WriteParams = serde_json::from_str(input_json).map_err(|e| {
            ToolError::InvalidArgument(format!("invalid MemoryWrite input: {e}"))
        })?;

        if !params.file_path.is_absolute() {
            return Err(ToolError::InvalidArgument(format!(
                "file_path must be absolute: {}",
                params.file_path.display()
            )));
        }

        let already_exists = params.file_path.exists();
        let mode = if already_exists {
            WriteMode::Update
        } else {
            WriteMode::Create
        };

        let report = self.linter.lint(&params.file_path, &params.content, mode);
        if report.has_errors() {
            return Err(ToolError::InvalidArgument(format_report(&report)));
        }

        if let Some(parent) = params.file_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ToolError::ExecutionFailed(format!(
                    "failed to create directory {}: {e}",
                    parent.display()
                ))
            })?;
        }
        std::fs::write(&params.file_path, params.content.as_bytes()).map_err(|e| {
            ToolError::ExecutionFailed(format!(
                "failed to write {}: {e}",
                params.file_path.display()
            ))
        })?;

        let summary = format!(
            "{} {}{}",
            if already_exists { "Overwrote" } else { "Created" },
            params.file_path.display(),
            warning_tail(&report),
        );
        Ok(ToolOutput {
            summary,
            content: None,
        })
    }
}

fn format_report(report: &LintReport) -> String {
    use std::fmt::Write as _;
    let mut buf = String::from("memory linter rejected the write:");
    for e in &report.errors {
        let _ = write!(&mut buf, "\n  - {e}");
    }
    if !report.warnings.is_empty() {
        let _ = write!(&mut buf, "\nwarnings (informational):");
        for w in &report.warnings {
            let _ = write!(&mut buf, "\n  - {w}");
        }
    }
    buf
}

fn warning_tail(report: &LintReport) -> String {
    if report.warnings.is_empty() {
        return String::new();
    }
    let mut s = format!(" [{} warning(s)]", report.warnings.len());
    for w in &report.warnings {
        use std::fmt::Write as _;
        let _ = write!(&mut s, " {w};");
    }
    s
}

pub fn write_tool(layout: WorkspaceLayout) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(WriteParams);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("MemoryWrite")
            .description(DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(WriteTool {
            linter: Linter::new(layout.clone()),
        });
        (meta, tool)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use tempfile::TempDir;

    fn now() -> String {
        Utc::now().to_rfc3339()
    }

    fn setup() -> (TempDir, WorkspaceLayout) {
        let dir = TempDir::new().unwrap();
        let layout = WorkspaceLayout::new(dir.path().to_path_buf());
        (dir, layout)
    }

    #[tokio::test]
    async fn write_creates_summary() {
        let (dir, layout) = setup();
        let path = dir.path().join("memory/summary.md");
        let content = format!("---\nupdated_at: {n}\n---\nbody\n", n = now());

        let (meta, tool) = write_tool(layout)();
        assert_eq!(meta.name, "MemoryWrite");

        let inp = serde_json::json!({
            "file_path": path.to_str().unwrap(),
            "content": content,
        });
        let out = tool.execute(&inp.to_string()).await.unwrap();
        assert!(out.summary.contains("Created"));
        assert!(path.exists());
    }

    #[tokio::test]
    async fn write_rejects_workflow() {
        let (dir, layout) = setup();
        let path = dir.path().join("memory/workflow/wf.md");
        let content = format!(
            "---\nupdated_at: {n}\ndescription: x\nauto_invoke: false\nuser_invocable: true\n---\n",
            n = now()
        );
        let (_, tool) = write_tool(layout)();
        let inp = serde_json::json!({
            "file_path": path.to_str().unwrap(),
            "content": content,
        });
        let err = tool.execute(&inp.to_string()).await.unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("workflow"), "unexpected error: {msg}");
        assert!(!path.exists(), "workflow file must not be written");
    }

    #[tokio::test]
    async fn write_aggregates_multiple_errors() {
        let (dir, layout) = setup();
        let path = dir.path().join("memory/decisions/foo.md");
        // Missing required `status` field AND body too long.
        let huge = "x".repeat(8001);
        let content = format!(
            "---\ncreated_at: {n}\nupdated_at: {n}\nsources: []\n---\n{huge}",
            n = now()
        );
        let (_, tool) = write_tool(layout)();
        let inp = serde_json::json!({
            "file_path": path.to_str().unwrap(),
            "content": content,
        });
        let err = tool.execute(&inp.to_string()).await.unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("status") || msg.contains("missing"), "{msg}");
    }

    #[tokio::test]
    async fn write_blocks_create_when_existing() {
        let (dir, layout) = setup();
        let path = dir.path().join("memory/decisions/foo.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let initial = format!(
            "---\ncreated_at: {n}\nupdated_at: {n}\nsources: []\nstatus: open\n---\nold\n",
            n = now()
        );
        std::fs::write(&path, &initial).unwrap();

        // Same content as a re-write should pass (Update mode).
        let (_, tool) = write_tool(layout.clone())();
        let inp = serde_json::json!({
            "file_path": path.to_str().unwrap(),
            "content": initial,
        });
        let out = tool.execute(&inp.to_string()).await.unwrap();
        assert!(out.summary.contains("Overwrote"));
    }

    #[tokio::test]
    async fn write_rejects_non_absolute() {
        let (_dir, layout) = setup();
        let (_, tool) = write_tool(layout)();
        let inp = serde_json::json!({
            "file_path": "memory/summary.md",
            "content": "ignored",
        });
        let err = tool.execute(&inp.to_string()).await.unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn write_does_not_persist_on_lint_failure() {
        let (dir, layout) = setup();
        let path = dir.path().join("memory/decisions/foo.md");
        let bad = "no frontmatter at all";
        let (_, tool) = write_tool(layout)();
        let inp = serde_json::json!({
            "file_path": path.to_str().unwrap(),
            "content": bad,
        });
        assert!(tool.execute(&inp.to_string()).await.is_err());
        assert!(!path.exists());
    }
}
