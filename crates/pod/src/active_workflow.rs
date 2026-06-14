//! Durable active workflow invocation state.
//!
//! Workflow bodies are resolved at invocation time and snapshotted here.  The
//! snapshot, not whatever resource version is installed later, is the procedural
//! authority that survives compaction for the currently governed task.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use llm_worker::Item;
use llm_worker::tool::{
    Tool, ToolDefinition, ToolError, ToolExecutionContext, ToolMeta, ToolOutput,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use session_store::{LogEntry, SystemItem, segment_log};

pub const DOMAIN: &str = "pod.active_workflows";
const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveWorkflowSnapshot {
    pub schema_version: u32,
    pub workflows: Vec<ActiveWorkflowRecord>,
}

impl Default for ActiveWorkflowSnapshot {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            workflows: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveWorkflowRecord {
    pub slug: String,
    pub status: ActiveWorkflowStatus,
    pub invocation: WorkflowInvocationInfo,
    pub task_scope: String,
    pub body_snapshot_policy: WorkflowBodySnapshotPolicy,
    pub guidance_snapshot: String,
    pub obligations: Vec<String>,
    pub checkpoints: Vec<WorkflowCheckpoint>,
    pub updated_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion: Option<WorkflowCompletionInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActiveWorkflowStatus {
    Active,
    Completed,
    Cancelled,
}

impl std::fmt::Display for ActiveWorkflowStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Active => "active",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowInvocationInfo {
    pub source: WorkflowInvocationSource,
    pub invoked_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowInvocationSource {
    UserWorkflowInvokeSegment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowBodySnapshotPolicy {
    SnapshottedAtInvocation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowCheckpoint {
    pub label: String,
    pub status: WorkflowCheckpointStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowCheckpointStatus {
    Open,
    Done,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowCompletionInfo {
    pub completed_at_ms: u64,
    pub reason: String,
}

#[derive(Debug, Clone, Default)]
pub struct ActiveWorkflowStore {
    inner: Arc<Mutex<ActiveWorkflowSnapshot>>,
}

impl ActiveWorkflowStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> ActiveWorkflowSnapshot {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn replace_with(&self, snapshot: ActiveWorkflowSnapshot) {
        *self.inner.lock().unwrap_or_else(|e| e.into_inner()) = snapshot;
    }

    pub fn active_records(&self) -> Vec<ActiveWorkflowRecord> {
        self.snapshot()
            .workflows
            .into_iter()
            .filter(|record| record.status == ActiveWorkflowStatus::Active)
            .collect()
    }

    pub fn activate_from_system_items(
        &self,
        items: &[SystemItem],
        task_scope: String,
        invoked_at_ms: u64,
    ) -> bool {
        let mut grouped: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for item in items {
            if let SystemItem::Workflow { slug, body } = item {
                grouped.entry(slug.clone()).or_default().push(body.clone());
            }
        }
        if grouped.is_empty() {
            return false;
        }

        let mut snapshot = self.snapshot();
        snapshot.schema_version = SCHEMA_VERSION;
        for (slug, bodies) in grouped {
            let guidance_snapshot = bodies.join("\n\n---\n\n");
            let obligations = extract_obligations(&guidance_snapshot);
            let checkpoints = obligations
                .iter()
                .take(32)
                .map(|label| WorkflowCheckpoint {
                    label: label.clone(),
                    status: WorkflowCheckpointStatus::Open,
                })
                .collect();
            let record = ActiveWorkflowRecord {
                slug: slug.clone(),
                status: ActiveWorkflowStatus::Active,
                invocation: WorkflowInvocationInfo {
                    source: WorkflowInvocationSource::UserWorkflowInvokeSegment,
                    invoked_at_ms,
                },
                task_scope: truncate_chars(&task_scope, 2_000),
                body_snapshot_policy: WorkflowBodySnapshotPolicy::SnapshottedAtInvocation,
                guidance_snapshot,
                obligations,
                checkpoints,
                updated_at_ms: invoked_at_ms,
                completion: None,
            };
            upsert_record(&mut snapshot.workflows, record);
        }
        self.replace_with(snapshot);
        true
    }

    pub fn set_status(
        &self,
        slug: &str,
        status: ActiveWorkflowStatus,
        reason: String,
        now_ms: u64,
    ) -> Result<ActiveWorkflowRecord, String> {
        let mut snapshot = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let record = snapshot
            .workflows
            .iter_mut()
            .find(|record| record.slug == slug)
            .ok_or_else(|| format!("active workflow `{slug}` not found"))?;
        record.status = status;
        record.updated_at_ms = now_ms;
        record.completion = Some(WorkflowCompletionInfo {
            completed_at_ms: now_ms,
            reason,
        });
        for checkpoint in &mut record.checkpoints {
            checkpoint.status = match status {
                ActiveWorkflowStatus::Active => WorkflowCheckpointStatus::Open,
                ActiveWorkflowStatus::Completed => WorkflowCheckpointStatus::Done,
                ActiveWorkflowStatus::Cancelled => WorkflowCheckpointStatus::Cancelled,
            };
        }
        Ok(record.clone())
    }

    pub fn snapshot_text(&self) -> Option<String> {
        let active = self.active_records();
        (!active.is_empty()).then(|| render_snapshot_text(&active))
    }

    pub fn rehydration_message(&self) -> Option<String> {
        let active = self.active_records();
        (!active.is_empty()).then(|| render_rehydration_message(&active))
    }

    pub fn extension_entry(&self) -> LogEntry {
        LogEntry::Extension {
            ts: segment_log::now_millis(),
            domain: DOMAIN.into(),
            payload: serde_json::to_value(self.snapshot())
                .expect("ActiveWorkflowSnapshot is always JSON-serializable"),
        }
    }

    pub fn restore_from_history_and_extensions(
        &self,
        history: &[Item],
        extensions: &[(String, serde_json::Value)],
    ) {
        let (mut snapshot, diagnostics) = fold_extensions(extensions);
        for diagnostic in diagnostics {
            tracing::warn!(diagnostic, "failed to restore active workflow state");
        }
        replay_history_tools(&mut snapshot, history);
        self.replace_with(snapshot);
    }
}

pub fn fold_extensions(
    extensions: &[(String, serde_json::Value)],
) -> (ActiveWorkflowSnapshot, Vec<String>) {
    let mut latest = None;
    let mut diagnostics = Vec::new();
    for (domain, payload) in extensions {
        if domain != DOMAIN {
            continue;
        }
        match serde_json::from_value::<ActiveWorkflowSnapshot>(payload.clone()) {
            Ok(snapshot) if snapshot.schema_version == SCHEMA_VERSION => latest = Some(snapshot),
            Ok(snapshot) => {
                latest = None;
                diagnostics.push(format!(
                    "unsupported active workflow schema_version {}",
                    snapshot.schema_version
                ));
            }
            Err(err) => {
                latest = None;
                diagnostics.push(format!("corrupt active workflow payload: {err}"));
            }
        }
    }
    (latest.unwrap_or_default(), diagnostics)
}

fn replay_history_tools(snapshot: &mut ActiveWorkflowSnapshot, history: &[Item]) {
    for item in history {
        let Item::ToolCall {
            name, arguments, ..
        } = item
        else {
            continue;
        };
        let status = match name.as_str() {
            "ActiveWorkflowComplete" => ActiveWorkflowStatus::Completed,
            "ActiveWorkflowCancel" => ActiveWorkflowStatus::Cancelled,
            _ => continue,
        };
        if let Ok(params) = serde_json::from_str::<WorkflowStatusParams>(arguments) {
            if let Some(record) = snapshot
                .workflows
                .iter_mut()
                .find(|record| record.slug == params.slug)
            {
                let reason = params.reason.unwrap_or_else(|| status.to_string());
                record.status = status;
                record.updated_at_ms = record.updated_at_ms.saturating_add(1);
                record.completion = Some(WorkflowCompletionInfo {
                    completed_at_ms: record.updated_at_ms,
                    reason,
                });
                for checkpoint in &mut record.checkpoints {
                    checkpoint.status = match status {
                        ActiveWorkflowStatus::Active => WorkflowCheckpointStatus::Open,
                        ActiveWorkflowStatus::Completed => WorkflowCheckpointStatus::Done,
                        ActiveWorkflowStatus::Cancelled => WorkflowCheckpointStatus::Cancelled,
                    };
                }
            }
        }
    }
}

pub fn active_workflow_tools(store: ActiveWorkflowStore) -> Vec<ToolDefinition> {
    vec![
        list_tool(store.clone()),
        status_tool(store.clone(), ActiveWorkflowStatus::Completed),
        status_tool(store, ActiveWorkflowStatus::Cancelled),
    ]
}

fn list_tool(store: ActiveWorkflowStore) -> ToolDefinition {
    Arc::new(move || {
        (
            ToolMeta::new("ActiveWorkflowList")
                .description("List durable active workflow invocations and their status")
                .input_schema(
                    json!({"type":"object","properties":{},"additionalProperties":false}),
                ),
            Arc::new(ActiveWorkflowListTool {
                store: store.clone(),
            }) as Arc<dyn Tool>,
        )
    })
}

fn status_tool(store: ActiveWorkflowStore, status: ActiveWorkflowStatus) -> ToolDefinition {
    let name = match status {
        ActiveWorkflowStatus::Completed => "ActiveWorkflowComplete",
        ActiveWorkflowStatus::Cancelled => "ActiveWorkflowCancel",
        ActiveWorkflowStatus::Active => unreachable!("active status tool is not exposed"),
    };
    let description = match status {
        ActiveWorkflowStatus::Completed => {
            "Mark an active workflow as completed when its governed task is finished"
        }
        ActiveWorkflowStatus::Cancelled => {
            "Cancel an active workflow when the governed task is explicitly abandoned"
        }
        ActiveWorkflowStatus::Active => unreachable!("active status tool is not exposed"),
    };
    let store_for_tool = store.clone();
    Arc::new(move || {
        (
            ToolMeta::new(name)
                .description(description)
                .input_schema(json!({
                    "type":"object",
                    "properties":{
                        "slug":{"type":"string","description":"Workflow slug to update"},
                        "reason":{"type":"string","description":"Brief completion/cancellation reason"}
                    },
                    "required":["slug"],
                    "additionalProperties":false
                })),
            Arc::new(ActiveWorkflowStatusTool {
                store: store_for_tool.clone(),
                status,
            }) as Arc<dyn Tool>,
        )
    })
}

struct ActiveWorkflowListTool {
    store: ActiveWorkflowStore,
}

#[async_trait]
impl Tool for ActiveWorkflowListTool {
    async fn execute(
        &self,
        _input_json: &str,
        _ctx: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let snapshot = self.store.snapshot();
        let content = serde_json::to_string_pretty(&snapshot)
            .map_err(|err| ToolError::Internal(err.to_string()))?;
        let active = snapshot
            .workflows
            .iter()
            .filter(|record| record.status == ActiveWorkflowStatus::Active)
            .count();
        Ok(ToolOutput {
            summary: format!(
                "ActiveWorkflowStore: {} workflow(s), {active} active",
                snapshot.workflows.len()
            ),
            content: Some(content),
        })
    }
}

struct ActiveWorkflowStatusTool {
    store: ActiveWorkflowStore,
    status: ActiveWorkflowStatus,
}

#[async_trait]
impl Tool for ActiveWorkflowStatusTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: WorkflowStatusParams = serde_json::from_str(input_json)
            .map_err(|err| ToolError::InvalidArgument(err.to_string()))?;
        let reason = params.reason.unwrap_or_else(|| self.status.to_string());
        let record = self
            .store
            .set_status(&params.slug, self.status, reason, segment_log::now_millis())
            .map_err(ToolError::InvalidArgument)?;
        let content = serde_json::to_string_pretty(&record)
            .map_err(|err| ToolError::Internal(err.to_string()))?;
        Ok(ToolOutput {
            summary: format!("workflow {} marked {}", record.slug, record.status),
            content: Some(content),
        })
    }
}

#[derive(Debug, Deserialize)]
struct WorkflowStatusParams {
    slug: String,
    #[serde(default)]
    reason: Option<String>,
}

fn upsert_record(records: &mut Vec<ActiveWorkflowRecord>, record: ActiveWorkflowRecord) {
    if let Some(existing) = records
        .iter_mut()
        .find(|existing| existing.slug == record.slug)
    {
        *existing = record;
    } else {
        records.push(record);
    }
}

fn extract_obligations(body: &str) -> Vec<String> {
    let mut obligations = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim();
        let candidate = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| trimmed.strip_prefix("• "))
            .unwrap_or(trimmed);
        let lower = candidate.to_ascii_lowercase();
        let looks_obligating = lower.contains("must")
            || lower.contains("require")
            || lower.contains("obligation")
            || lower.contains("review")
            || lower.contains("merge")
            || lower.contains("close")
            || lower.contains("report")
            || lower.contains("handoff");
        if looks_obligating && !candidate.is_empty() {
            obligations.push(truncate_chars(candidate, 240));
        }
        if obligations.len() >= 32 {
            break;
        }
    }
    if obligations.is_empty() {
        obligations
            .push("Follow the snapshotted workflow body until completion or cancellation".into());
    }
    obligations
}

fn render_snapshot_text(records: &[ActiveWorkflowRecord]) -> String {
    let json = serde_json::to_string_pretty(&ActiveWorkflowSnapshot {
        schema_version: SCHEMA_VERSION,
        workflows: records.to_vec(),
    })
    .unwrap_or_else(|_| String::from("{\"schema_version\":1,\"workflows\":[]}"));
    format!(
        "ActiveWorkflowStore: {} active workflow(s)\n\n```json\n{}\n```",
        records.len(),
        json
    )
}

fn render_rehydration_message(records: &[ActiveWorkflowRecord]) -> String {
    let mut out = String::from(
        "[Active workflow snapshot]\n\n\
         The following workflow invocation state is durable state carried across compaction. \
         Continue to follow each active workflow's snapshotted guidance until the governed task \
         is completed with ActiveWorkflowComplete or explicitly cancelled with ActiveWorkflowCancel. \
         Missing or obsolete workflow resources must not replace these invocation snapshots.\n",
    );
    for record in records {
        out.push_str(&format!(
            "\n## /{} ({})\n- invoked_at_ms: {}\n- invocation_source: {:?}\n- body_snapshot_policy: {:?}\n- task_scope: {}\n\n### Current obligations/checkpoints\n",
            record.slug,
            record.status,
            record.invocation.invoked_at_ms,
            record.invocation.source,
            record.body_snapshot_policy,
            record.task_scope.replace('\n', " "),
        ));
        for checkpoint in &record.checkpoints {
            out.push_str(&format!(
                "- [{}] {}\n",
                checkpoint.status_label(),
                checkpoint.label
            ));
        }
        out.push_str("\n### Snapshotted workflow guidance\n");
        out.push_str(record.guidance_snapshot.trim_end());
        out.push_str("\n");
    }
    out
}

impl WorkflowCheckpoint {
    fn status_label(&self) -> &'static str {
        match self.status {
            WorkflowCheckpointStatus::Open => "open",
            WorkflowCheckpointStatus::Done => "done",
            WorkflowCheckpointStatus::Cancelled => "cancelled",
        }
    }
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (idx, ch) in text.chars().enumerate() {
        if idx >= max_chars {
            out.push('…');
            return out;
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_workflow_guidance_carries_merge_close_obligations() {
        let store = ActiveWorkflowStore::new();
        let items = vec![SystemItem::Workflow {
            slug: "multi-agent-workflow".into(),
            body: "# Multi-agent workflow\n- Delegate implementation to coder.\n- Require external review before merge.\n- Close the Ticket after merge and report evidence.\n".into(),
        }];

        assert!(store.activate_from_system_items(
            &items,
            "/multi-agent-workflow implement ticket".into(),
            42,
        ));
        let msg = store.rehydration_message().unwrap();

        assert!(msg.contains("multi-agent-workflow"));
        assert!(msg.contains("external review before merge"));
        assert!(msg.contains("Close the Ticket after merge"));
        assert!(msg.contains("Snapshotted workflow guidance"));
    }

    #[test]
    fn corrupt_extension_fails_closed_with_diagnostic() {
        let entries = vec![(DOMAIN.to_string(), json!({"schema_version":"bad"}))];

        let (snapshot, diagnostics) = fold_extensions(&entries);

        assert!(snapshot.workflows.is_empty());
        assert_eq!(diagnostics.len(), 1);
    }
}
