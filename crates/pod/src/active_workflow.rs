//! Durable active workflow invocation state.
//!
//! Workflow bodies are resolved at invocation time and snapshotted here.  The
//! snapshot, not whatever resource version is installed later, is the procedural
//! authority that survives compaction for the currently governed task.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use llm_engine::Item;
use llm_engine::tool::{
    Tool, ToolDefinition, ToolError, ToolExecutionContext, ToolMeta, ToolOutput,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use session_store::{LogEntry, SystemItem, segment_log};

pub const DOMAIN: &str = "pod.active_workflows";
pub const REHYDRATION_MESSAGE_PREFIX: &str = "[Active workflow snapshot]";
pub const INACTIVE_MESSAGE_PREFIX: &str = "[Active workflow state]";
const SCHEMA_VERSION: u32 = 1;

pub type LogEntryCommitter = Arc<dyn Fn(LogEntry) + Send + Sync>;

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

    pub fn sanitize_context(&self, context: &mut Vec<Item>) -> usize {
        let removed = strip_rehydration_messages(context);
        if let Some(message) = self.rehydration_message() {
            context.push(Item::system_message(message));
        } else if removed > 0 || context.iter().any(has_active_workflow_hint) {
            context.push(Item::system_message(inactive_workflow_message()));
        }
        removed
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
        _history: &[Item],
        extensions: &[(String, serde_json::Value)],
    ) {
        let (snapshot, diagnostics) = fold_extensions(extensions);
        for diagnostic in diagnostics {
            tracing::warn!(diagnostic, "failed to restore active workflow state");
        }
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

pub fn strip_rehydration_messages(items: &mut Vec<Item>) -> usize {
    let before = items.len();
    items.retain(|item| !is_rehydration_message(item));
    before - items.len()
}

pub fn is_rehydration_message(item: &Item) -> bool {
    item_system_text(item)
        .map(|text| text.trim_start().starts_with(REHYDRATION_MESSAGE_PREFIX))
        .unwrap_or(false)
}

fn has_active_workflow_hint(item: &Item) -> bool {
    item_system_text(item)
        .map(|text| {
            text.contains("Active Workflow Invocation State")
                || text.contains("ActiveWorkflowStore:")
                || text.contains(REHYDRATION_MESSAGE_PREFIX)
        })
        .unwrap_or(false)
}

fn item_system_text(item: &Item) -> Option<String> {
    match item {
        Item::Message { role, content, .. } if *role == llm_engine::Role::System => Some(
            content
                .iter()
                .map(|part| part.as_text())
                .collect::<String>(),
        ),
        _ => None,
    }
}

fn inactive_workflow_message() -> String {
    format!(
        "{INACTIVE_MESSAGE_PREFIX}\n\n\
         No currently valid active workflow invocation state is active. Ignore older compacted \
         history or summaries that appear to describe active workflow obligations; only validated \
         typed `{DOMAIN}` records with status `active` establish active workflow guidance."
    )
}

pub fn active_workflow_tools(
    store: ActiveWorkflowStore,
    committer: Option<LogEntryCommitter>,
) -> Vec<ToolDefinition> {
    vec![
        list_tool(store.clone()),
        status_tool(
            store.clone(),
            ActiveWorkflowStatus::Completed,
            committer.clone(),
        ),
        status_tool(store, ActiveWorkflowStatus::Cancelled, committer),
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

fn status_tool(
    store: ActiveWorkflowStore,
    status: ActiveWorkflowStatus,
    committer: Option<LogEntryCommitter>,
) -> ToolDefinition {
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
    let committer_for_tool = committer.clone();
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
                committer: committer_for_tool.clone(),
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
    committer: Option<LogEntryCommitter>,
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
        if let Some(committer) = &self.committer {
            committer(self.store.extension_entry());
        }
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
    let mut out = format!(
        "{REHYDRATION_MESSAGE_PREFIX}\n\n\
         The following workflow invocation state is durable state carried across compaction. \
         Continue to follow each active workflow's snapshotted guidance until the governed task \
         is completed with ActiveWorkflowComplete or explicitly cancelled with ActiveWorkflowCancel. \
         Missing or obsolete workflow resources must not replace these invocation snapshots.\n"
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

    fn store_with_active_workflow() -> ActiveWorkflowStore {
        let store = ActiveWorkflowStore::new();
        assert!(store.activate_from_system_items(
            &[SystemItem::Workflow {
                slug: "multi-agent-workflow".into(),
                body: "# Multi-agent workflow\n- Delegate implementation to coder.\n- Require external review before merge.\n- Close the Ticket after merge and report evidence.\n".into(),
            }],
            "/multi-agent-workflow implement ticket".into(),
            42,
        ));
        store
    }

    fn active_extension(store: &ActiveWorkflowStore) -> (String, serde_json::Value) {
        (
            DOMAIN.to_string(),
            serde_json::to_value(store.snapshot()).expect("snapshot json"),
        )
    }

    #[test]
    fn active_workflow_guidance_carries_merge_close_obligations() {
        let store = store_with_active_workflow();
        let msg = store.rehydration_message().unwrap();

        assert!(msg.contains("multi-agent-workflow"));
        assert!(msg.contains("external review before merge"));
        assert!(msg.contains("Close the Ticket after merge"));
        assert!(msg.contains("Snapshotted workflow guidance"));
    }

    #[test]
    fn compacted_rehydration_message_is_removed_when_typed_state_missing_or_invalid() {
        for extensions in [
            Vec::new(),
            vec![(DOMAIN.to_string(), json!({"schema_version":"bad"}))],
            vec![(
                DOMAIN.to_string(),
                json!({"schema_version":999,"workflows":[]}),
            )],
        ] {
            let original = store_with_active_workflow();
            let stale_message = original.rehydration_message().unwrap();
            let mut context = vec![
                Item::system_message(stale_message),
                Item::user_message("continue"),
            ];
            let restored = ActiveWorkflowStore::new();

            restored.restore_from_history_and_extensions(&context, &extensions);
            let removed = restored.sanitize_context(&mut context);

            assert_eq!(removed, 1);
            assert!(restored.active_records().is_empty());
            assert!(!context.iter().any(is_rehydration_message));
        }
    }

    #[test]
    fn completion_or_cancellation_suppresses_old_compacted_guidance() {
        for status in [
            ActiveWorkflowStatus::Completed,
            ActiveWorkflowStatus::Cancelled,
        ] {
            let store = store_with_active_workflow();
            let stale_message = store.rehydration_message().unwrap();
            let mut context = vec![
                Item::system_message(stale_message),
                Item::user_message("continue"),
            ];

            store
                .set_status("multi-agent-workflow", status, status.to_string(), 84)
                .expect("workflow exists");
            let removed = store.sanitize_context(&mut context);

            assert_eq!(removed, 1);
            assert!(!context.iter().any(is_rehydration_message));
        }
    }

    #[test]
    fn unmatched_status_tool_calls_do_not_mutate_restored_state() {
        let store = store_with_active_workflow();
        let extensions = vec![active_extension(&store)];
        let history = vec![
            Item::tool_call(
                "call-1",
                "ActiveWorkflowCancel",
                json!({"slug":"multi-agent-workflow","reason":"not durable"}).to_string(),
            ),
            Item::tool_result_error("call-1", "error: failed"),
        ];
        let restored = ActiveWorkflowStore::new();

        restored.restore_from_history_and_extensions(&history, &extensions);

        assert_eq!(restored.active_records().len(), 1);
        assert_eq!(
            restored.snapshot().workflows[0].status,
            ActiveWorkflowStatus::Active
        );
    }

    #[tokio::test]
    async fn status_tool_persists_typed_extension_on_success() {
        let store = store_with_active_workflow();
        let committed = Arc::new(Mutex::new(Vec::<LogEntry>::new()));
        let committed_for_tool = committed.clone();
        let tools = active_workflow_tools(
            store.clone(),
            Some(Arc::new(move |entry| {
                committed_for_tool
                    .lock()
                    .expect("committed entries mutex poisoned")
                    .push(entry);
            })),
        );
        let (_, tool) = tools[1]();

        tool.execute(
            &json!({"slug":"multi-agent-workflow","reason":"review complete"}).to_string(),
            ToolExecutionContext::default(),
        )
        .await
        .expect("status tool succeeds");

        let committed = committed.lock().expect("committed entries mutex poisoned");
        let LogEntry::Extension {
            domain, payload, ..
        } = committed.last().expect("extension committed")
        else {
            panic!("expected typed active workflow extension");
        };
        assert_eq!(domain, DOMAIN);
        let snapshot: ActiveWorkflowSnapshot = serde_json::from_value(payload.clone()).unwrap();
        assert_eq!(
            snapshot.workflows[0].status,
            ActiveWorkflowStatus::Completed
        );
    }

    #[test]
    fn corrupt_extension_fails_closed_with_diagnostic() {
        let entries = vec![(DOMAIN.to_string(), json!({"schema_version":"bad"}))];

        let (snapshot, diagnostics) = fold_extensions(&entries);

        assert!(snapshot.workflows.is_empty());
        assert_eq!(diagnostics.len(), 1);
    }
}
