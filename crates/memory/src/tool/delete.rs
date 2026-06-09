//! `MemoryDelete` tool for removing memory / knowledge records with audit logging.

use std::sync::Arc;

use async_trait::async_trait;
use llm_worker::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use serde::Deserialize;

use crate::audit::{AuditStatus, RecordOperationAudit, append_record_operation, file_hash};
use crate::tool::MemoryToolKind;
use crate::workspace::WorkspaceLayout;

const DESCRIPTION: &str = "Delete an existing memory or knowledge record selected by `kind` + `slug`. \
For `summary` omit `slug`; for the others `slug` is required. The delete is audited and cannot target \
workflow or staging/log files.";

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct DeleteParams {
    /// Kind of record to delete.
    kind: MemoryToolKind,
    /// Slug. Required for everything except `summary`; forbidden for `summary`.
    #[serde(default)]
    slug: Option<String>,
}

struct MemoryDeleteTool {
    layout: WorkspaceLayout,
}

#[async_trait]
impl Tool for MemoryDeleteTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_worker::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: DeleteParams = serde_json::from_str(input_json)
            .map_err(|e| ToolError::InvalidArgument(format!("invalid MemoryDelete input: {e}")))?;
        let path = params
            .kind
            .resolve_path(&self.layout, params.slug.as_deref())?;
        let kind = params.kind.to_string();
        let slug = audit_slug(&params.kind, params.slug.as_deref());
        let before_hash = file_hash(&path).ok().flatten();
        if before_hash.is_none() {
            let reason = format!("record not found: {}", path.display());
            let _ = append_record_operation(
                &self.layout,
                RecordOperationAudit {
                    op: "delete".to_string(),
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

        if let Err(err) = std::fs::remove_file(&path) {
            let reason = format!("failed to delete {}: {err}", path.display());
            let _ = append_record_operation(
                &self.layout,
                RecordOperationAudit {
                    op: "delete".to_string(),
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

        let _ = append_record_operation(
            &self.layout,
            RecordOperationAudit {
                op: "delete".to_string(),
                status: AuditStatus::Success,
                kind,
                slug,
                path: path.display().to_string(),
                before_hash,
                after_hash: None,
                reason: None,
            },
        );

        Ok(ToolOutput {
            summary: format!("Deleted {}", path.display()),
            content: None,
        })
    }
}

pub fn delete_tool(layout: WorkspaceLayout) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(DeleteParams);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("MemoryDelete")
            .description(DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(MemoryDeleteTool {
            layout: layout.clone(),
        });
        (meta, tool)
    })
}

fn audit_slug(kind: &MemoryToolKind, slug: Option<&str>) -> String {
    match kind {
        MemoryToolKind::Summary => "summary".to_string(),
        _ => slug.unwrap_or("<missing>").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use tempfile::TempDir;

    #[tokio::test]
    async fn delete_removes_file_and_audits() {
        let dir = TempDir::new().unwrap();
        let layout = WorkspaceLayout::new(dir.path().to_path_buf());
        std::fs::create_dir_all(layout.decisions_dir()).unwrap();
        let path = layout.decisions_dir().join("obsolete.md");
        let now = Utc::now().to_rfc3339();
        std::fs::write(
            &path,
            format!(
                "---\ncreated_at: {now}\nupdated_at: {now}\nsources: []\nstatus: open\n---\nold"
            ),
        )
        .unwrap();

        let (_, tool) = delete_tool(layout.clone())();
        let out = tool
            .execute(
                r#"{"kind":"decision","slug":"obsolete"}"#,
                Default::default(),
            )
            .await
            .unwrap();
        assert!(out.summary.contains("Deleted"));
        assert!(!path.exists());
        let log = std::fs::read_to_string(layout.audit_current_log_path()).unwrap();
        assert!(log.contains(r#""event":"record_operation""#));
        assert!(log.contains(r#""op":"delete""#));
        assert!(log.contains(r#""status":"success""#));
    }
}
