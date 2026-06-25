//! `MemoryWrite` tool.
//!
//! Creates or overwrites a memory or knowledge record by `(kind, slug)`.
//! Pre-write Linter validates frontmatter, slug uniqueness (Create only),
//! reference integrity, size limits. On any
//! Linter error the tool returns `ToolError::InvalidArgument` with all
//! violations aggregated and the file is **not** written.

use std::sync::Arc;

use async_trait::async_trait;
use llm_engine::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use serde::Deserialize;

use crate::audit::{
    AuditStatus, RecordOperationAudit, append_record_operation, file_hash, hash_bytes,
};
use crate::linter::{LintReport, Linter, WriteMode};
use crate::tool::MemoryToolKind;
use crate::workspace::WorkspaceLayout;

const DESCRIPTION: &str = "Create or overwrite a memory or knowledge record by \
`kind` + `slug`. `kind`: summary | decision | request | knowledge. For `summary` \
omit `slug`. Frontmatter is validated before write; on validation failure no \
write occurs and every violation is returned in the error message.";

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct WriteParams {
    /// Record kind: `summary` | `decision` | `request` | `knowledge`.
    kind: MemoryToolKind,
    /// Slug. Required for everything except `summary`; forbidden for `summary`.
    #[serde(default)]
    slug: Option<String>,
    /// Full file contents (frontmatter + body).
    content: String,
}

struct WriteTool {
    layout: WorkspaceLayout,
    linter: Linter,
}

#[async_trait]
impl Tool for WriteTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: WriteParams = serde_json::from_str(input_json)
            .map_err(|e| ToolError::InvalidArgument(format!("invalid MemoryWrite input: {e}")))?;

        let path = params
            .kind
            .resolve_path(&self.layout, params.slug.as_deref())?;
        let kind = params.kind.to_string();
        let slug = audit_slug(&params.kind, params.slug.as_deref());

        let before_hash = file_hash(&path).ok().flatten();
        let already_exists = before_hash.is_some();
        let mode = if already_exists {
            WriteMode::Update
        } else {
            WriteMode::Create
        };

        let report = self.linter.lint(&path, &params.content, mode);
        if report.has_errors() {
            let reason = format_report(&report);
            let _ = append_record_operation(
                &self.layout,
                RecordOperationAudit {
                    op: "write".to_string(),
                    status: AuditStatus::Failed,
                    kind,
                    slug,
                    path: path.display().to_string(),
                    before_hash,
                    after_hash: None,
                    reason: Some(reason.clone()),
                },
            );
            return Err(ToolError::InvalidArgument(reason));
        }

        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                let reason = format!("failed to create directory {}: {e}", parent.display());
                let _ = append_record_operation(
                    &self.layout,
                    RecordOperationAudit {
                        op: "write".to_string(),
                        status: AuditStatus::Failed,
                        kind,
                        slug,
                        path: path.display().to_string(),
                        before_hash,
                        after_hash: None,
                        reason: Some(reason.clone()),
                    },
                );
                return Err(ToolError::ExecutionFailed(reason));
            }
        }
        if let Err(e) = std::fs::write(&path, params.content.as_bytes()) {
            let reason = format!("failed to write {}: {e}", path.display());
            let _ = append_record_operation(
                &self.layout,
                RecordOperationAudit {
                    op: "write".to_string(),
                    status: AuditStatus::Failed,
                    kind,
                    slug,
                    path: path.display().to_string(),
                    before_hash,
                    after_hash: None,
                    reason: Some(reason.clone()),
                },
            );
            return Err(ToolError::ExecutionFailed(reason));
        }
        let after_hash = Some(hash_bytes(params.content.as_bytes()));
        let _ = append_record_operation(
            &self.layout,
            RecordOperationAudit {
                op: "write".to_string(),
                status: AuditStatus::Success,
                kind,
                slug,
                path: path.display().to_string(),
                before_hash,
                after_hash,
                reason: if report.warnings.is_empty() {
                    None
                } else {
                    Some(format!("{} warning(s)", report.warnings.len()))
                },
            },
        );

        let summary = format!(
            "{} {}{}",
            if already_exists {
                "Overwrote"
            } else {
                "Created"
            },
            path.display(),
            warning_tail(&report),
        );
        Ok(ToolOutput {
            summary,
            content: None,
        })
    }
}

fn audit_slug(kind: &MemoryToolKind, slug: Option<&str>) -> String {
    match kind {
        MemoryToolKind::Summary => "summary".to_string(),
        _ => slug.unwrap_or("<missing>").to_string(),
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
            layout: layout.clone(),
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
        let path = dir.path().join(".yoi/memory/summary.md");
        let content = format!("---\nupdated_at: {n}\n---\nbody\n", n = now());

        let (meta, tool) = write_tool(layout)();
        assert_eq!(meta.name, "MemoryWrite");

        let inp = serde_json::json!({
            "kind": "summary",
            "content": content,
        });
        let out = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap();
        assert!(out.summary.contains("Created"));
        assert!(path.exists());
    }

    #[tokio::test]
    async fn write_aggregates_multiple_errors() {
        let (_dir, layout) = setup();
        // Missing required `status` field for decisions.
        let huge = "x".repeat(8001);
        let content = format!(
            "---\ncreated_at: {n}\nupdated_at: {n}\nsources: []\n---\n{huge}",
            n = now()
        );
        let (_, tool) = write_tool(layout)();
        let inp = serde_json::json!({
            "kind": "decision",
            "slug": "foo",
            "content": content,
        });
        let err = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("status") || msg.contains("missing"), "{msg}");
    }

    #[tokio::test]
    async fn write_update_existing() {
        let (dir, layout) = setup();
        let path = dir.path().join(".yoi/memory/decisions/foo.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let initial = format!(
            "---\ncreated_at: {n}\nupdated_at: {n}\nsources: []\nstatus: open\n---\nold\n",
            n = now()
        );
        std::fs::write(&path, &initial).unwrap();

        let (_, tool) = write_tool(layout.clone())();
        let inp = serde_json::json!({
            "kind": "decision",
            "slug": "foo",
            "content": initial,
        });
        let out = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap();
        assert!(out.summary.contains("Overwrote"));
    }

    #[tokio::test]
    async fn write_decision_requires_slug() {
        let (_dir, layout) = setup();
        let (_, tool) = write_tool(layout)();
        let inp = serde_json::json!({
            "kind": "decision",
            "content": "ignored",
        });
        let err = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn write_does_not_persist_on_lint_failure() {
        let (dir, layout) = setup();
        let path = dir.path().join(".yoi/memory/decisions/foo.md");
        let bad = "no frontmatter at all";
        let (_, tool) = write_tool(layout)();
        let inp = serde_json::json!({
            "kind": "decision",
            "slug": "foo",
            "content": bad,
        });
        assert!(
            tool.execute(&inp.to_string(), Default::default())
                .await
                .is_err()
        );
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn workflow_kind_not_acceptable() {
        // The MemoryToolKind enum doesn't include Workflow, so deserialization fails.
        let (_dir, layout) = setup();
        let (_, tool) = write_tool(layout)();
        let inp = serde_json::json!({
            "kind": "workflow",
            "slug": "wf",
            "content": "---\n---\n",
        });
        let err = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgument(_)));
    }
}
