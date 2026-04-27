//! `MemoryEdit` tool — partial string replacement on an existing memory record.
//!
//! Reads current content, applies the replacement, runs the Linter on
//! the result, writes only on success. The current-then-write window
//! is single-tool-call narrow; an external tracker is intentionally
//! omitted (memory tools are self-contained, no `tools` crate dep).

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use llm_worker::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use serde::Deserialize;

use crate::linter::{LintReport, Linter, WriteMode};
use crate::workspace::WorkspaceLayout;

const DESCRIPTION: &str = "Replace a substring in an existing memory or knowledge \
record file. By default `old_string` must be unique in the file; set \
`replace_all: true` to replace every occurrence. The resulting content is \
re-validated by the memory linter; failure leaves the file untouched. Path \
must be absolute and lie inside the workspace's `memory/` or `knowledge/` tree.";

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct EditParams {
    /// Absolute path under the workspace's `memory/` or `knowledge/` tree.
    file_path: PathBuf,
    /// String to replace. Must be unique in the file unless `replace_all` is true.
    old_string: String,
    /// Replacement string. Must differ from `old_string`.
    new_string: String,
    /// Replace all occurrences. Defaults to false.
    #[serde(default)]
    replace_all: bool,
}

struct EditTool {
    linter: Linter,
}

#[async_trait]
impl Tool for EditTool {
    async fn execute(&self, input_json: &str) -> Result<ToolOutput, ToolError> {
        let params: EditParams = serde_json::from_str(input_json).map_err(|e| {
            ToolError::InvalidArgument(format!("invalid MemoryEdit input: {e}"))
        })?;

        if !params.file_path.is_absolute() {
            return Err(ToolError::InvalidArgument(format!(
                "file_path must be absolute: {}",
                params.file_path.display()
            )));
        }
        if params.old_string.is_empty() {
            return Err(ToolError::InvalidArgument(
                "old_string must not be empty".into(),
            ));
        }
        if params.old_string == params.new_string {
            return Err(ToolError::InvalidArgument(
                "old_string and new_string are identical".into(),
            ));
        }

        // Path-shape check; the layout::classify also runs inside the
        // linter but we want a crisp error before reading the file.
        if self
            .linter
            .layout()
            .classify(&params.file_path)
            .map_err(|e| ToolError::InvalidArgument(e.to_string()))?
            .is_none()
        {
            return Err(ToolError::InvalidArgument(format!(
                "path is not under the memory tree: {}",
                params.file_path.display()
            )));
        }

        let current_bytes = std::fs::read(&params.file_path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => ToolError::ExecutionFailed(format!(
                "file not found (use MemoryWrite to create): {}",
                params.file_path.display()
            )),
            _ => ToolError::ExecutionFailed(format!(
                "read failed at {}: {e}",
                params.file_path.display()
            )),
        })?;
        let current_text = std::str::from_utf8(&current_bytes).map_err(|_| {
            ToolError::InvalidArgument(format!(
                "file is not valid UTF-8: {}",
                params.file_path.display()
            ))
        })?;

        let count = current_text.matches(&params.old_string).count();
        if count == 0 {
            return Err(ToolError::InvalidArgument(format!(
                "old_string not found in {}",
                params.file_path.display()
            )));
        }
        if !params.replace_all && count > 1 {
            return Err(ToolError::InvalidArgument(format!(
                "old_string occurs {count} times in {}; pass replace_all: true or narrow the snippet",
                params.file_path.display()
            )));
        }

        let new_text = if params.replace_all {
            current_text.replace(&params.old_string, &params.new_string)
        } else {
            current_text.replacen(&params.old_string, &params.new_string, 1)
        };
        let occurrences = if params.replace_all { count } else { 1 };

        let report = self.linter.lint(&params.file_path, &new_text, WriteMode::Update);
        if report.has_errors() {
            return Err(ToolError::InvalidArgument(format_report(&report)));
        }

        std::fs::write(&params.file_path, new_text.as_bytes()).map_err(|e| {
            ToolError::ExecutionFailed(format!(
                "failed to write {}: {e}",
                params.file_path.display()
            ))
        })?;

        let summary = format!(
            "Edited {} ({} replacement{}){}",
            params.file_path.display(),
            occurrences,
            if occurrences == 1 { "" } else { "s" },
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
    let mut buf = String::from("memory linter rejected the edit:");
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

pub fn edit_tool(layout: WorkspaceLayout) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(EditParams);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("MemoryEdit")
            .description(DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(EditTool {
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

    fn setup() -> (TempDir, WorkspaceLayout, PathBuf) {
        let dir = TempDir::new().unwrap();
        let layout = WorkspaceLayout::new(dir.path().to_path_buf());
        let path = dir.path().join("memory/decisions/foo.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let initial = format!(
            "---\ncreated_at: {n}\nupdated_at: {n}\nsources: []\nstatus: open\n---\nbody body\n",
            n = now()
        );
        std::fs::write(&path, &initial).unwrap();
        (dir, layout, path)
    }

    #[tokio::test]
    async fn edit_simple_replace() {
        let (_dir, layout, path) = setup();
        let (meta, tool) = edit_tool(layout)();
        assert_eq!(meta.name, "MemoryEdit");

        let inp = serde_json::json!({
            "file_path": path.to_str().unwrap(),
            "old_string": "body body",
            "new_string": "edited",
        });
        let out = tool.execute(&inp.to_string()).await.unwrap();
        assert!(out.summary.contains("1 replacement"));
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("edited"));
        assert!(!after.contains("body body"));
    }

    #[tokio::test]
    async fn edit_resulting_invalid_frontmatter_rolled_back() {
        let (_dir, layout, path) = setup();
        let (_, tool) = edit_tool(layout)();

        // Drop the `status` field by replacing it with nothing.
        let inp = serde_json::json!({
            "file_path": path.to_str().unwrap(),
            "old_string": "status: open\n",
            "new_string": "",
        });
        let err = tool.execute(&inp.to_string()).await.unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("status") || msg.contains("missing"));

        // File untouched.
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("status: open"));
    }

    #[tokio::test]
    async fn edit_missing_file() {
        let (dir, layout, _) = setup();
        let other = dir.path().join("memory/decisions/ghost.md");
        let (_, tool) = edit_tool(layout)();
        let inp = serde_json::json!({
            "file_path": other.to_str().unwrap(),
            "old_string": "x",
            "new_string": "y",
        });
        let err = tool.execute(&inp.to_string()).await.unwrap_err();
        assert!(matches!(err, ToolError::ExecutionFailed(_)));
    }

    #[tokio::test]
    async fn edit_outside_memory_tree_rejected() {
        let (dir, layout, _) = setup();
        let other = dir.path().join("src/lib.rs");
        std::fs::create_dir_all(other.parent().unwrap()).unwrap();
        std::fs::write(&other, "fn main() {}").unwrap();
        let (_, tool) = edit_tool(layout)();
        let inp = serde_json::json!({
            "file_path": other.to_str().unwrap(),
            "old_string": "fn",
            "new_string": "pub fn",
        });
        let err = tool.execute(&inp.to_string()).await.unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn edit_workflow_path_rejected() {
        let (dir, layout, _) = setup();
        let path = dir.path().join("memory/workflow/wf.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let initial = format!(
            "---\nupdated_at: {n}\ndescription: x\nauto_invoke: false\nuser_invocable: true\n---\nbody\n",
            n = now()
        );
        std::fs::write(&path, &initial).unwrap();

        let (_, tool) = edit_tool(layout)();
        let inp = serde_json::json!({
            "file_path": path.to_str().unwrap(),
            "old_string": "body",
            "new_string": "edited",
        });
        let err = tool.execute(&inp.to_string()).await.unwrap_err();
        let msg = format!("{err}");
        assert!(msg.to_lowercase().contains("workflow"), "{msg}");
        // Original untouched.
        assert!(std::fs::read_to_string(&path).unwrap().contains("body"));
    }
}
