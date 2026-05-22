//! Session-lifetime TaskStore and builtin task tools.
//!
//! The store survives compaction and Pod restart — it is reconstructed
//! on resume by replaying TaskCreate / TaskUpdate tool-call arguments
//! from persisted history, so its effective lifetime is the
//! [`session_store::SessionId`] (the conversation), not the Pod process.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use llm_worker::Item;
use llm_worker::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Pending,
    Inprogress,
    Completed,
    Deleted,
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Pending => "pending",
            Self::Inprogress => "inprogress",
            Self::Completed => "completed",
            Self::Deleted => "deleted",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TaskEntry {
    pub taskid: u64,
    pub status: TaskStatus,
    pub subject: String,
    pub description: String,
}

#[derive(Debug, Default)]
struct Inner {
    next_taskid: u64,
    tasks: Vec<TaskEntry>,
}

#[derive(Debug, Clone, Default)]
pub struct TaskStore {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TaskSnapshot {
    pub tasks: Vec<TaskEntry>,
}

impl TaskStore {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                next_taskid: 1,
                tasks: Vec::new(),
            })),
        }
    }

    pub fn create(&self, subject: String, description: String) -> TaskEntry {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let task = TaskEntry {
            taskid: inner.next_taskid,
            status: TaskStatus::Pending,
            subject,
            description,
        };
        inner.next_taskid = inner.next_taskid.saturating_add(1);
        inner.tasks.push(task.clone());
        task
    }

    pub fn list(&self) -> Vec<TaskEntry> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .tasks
            .clone()
    }

    pub fn get(&self, taskid: u64) -> Option<TaskEntry> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .tasks
            .iter()
            .find(|t| t.taskid == taskid)
            .cloned()
    }

    pub fn update(
        &self,
        taskid: u64,
        status: Option<TaskStatus>,
        subject: Option<String>,
        description: Option<String>,
    ) -> Result<TaskEntry, TaskStoreError> {
        if status.is_none() && subject.is_none() && description.is_none() {
            return Err(TaskStoreError::NoFields);
        }
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let task = inner
            .tasks
            .iter_mut()
            .find(|t| t.taskid == taskid)
            .ok_or(TaskStoreError::Missing(taskid))?;
        if let Some(status) = status {
            task.status = status;
        }
        if let Some(subject) = subject {
            task.subject = subject;
        }
        if let Some(description) = description {
            task.description = description;
        }
        Ok(task.clone())
    }

    pub fn snapshot(&self) -> TaskSnapshot {
        TaskSnapshot { tasks: self.list() }
    }

    pub fn replay_history(&self, history: &[Item]) {
        for item in history {
            match item {
                Item::Message { content, .. } => {
                    for part in content {
                        let text = part.as_text();
                        if let Some(snapshot) = parse_compact_snapshot_text(text) {
                            self.replace_with(snapshot);
                        }
                    }
                }
                Item::ToolCall {
                    name, arguments, ..
                } => match name.as_str() {
                    "TaskCreate" => {
                        if let Ok(params) = serde_json::from_str::<TaskCreateParams>(arguments) {
                            let _ = self.create(params.subject, params.description);
                        }
                    }
                    "TaskUpdate" => {
                        if let Ok(params) = serde_json::from_str::<TaskUpdateParams>(arguments) {
                            let _ = self.update(
                                params.taskid,
                                params.status,
                                params.subject,
                                params.description,
                            );
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }

    pub fn replace_with(&self, tasks: Vec<TaskEntry>) {
        let next_taskid = tasks
            .iter()
            .map(|t| t.taskid)
            .max()
            .unwrap_or(0)
            .saturating_add(1)
            .max(1);
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.tasks = tasks;
        inner.next_taskid = next_taskid;
    }

    pub fn from_history(history: &[Item]) -> Self {
        let store = Self::new();
        store.replay_history(history);
        store
    }

    pub fn snapshot_text(&self) -> String {
        render_snapshot(&self.list())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskStoreError {
    Missing(u64),
    NoFields,
}

impl std::fmt::Display for TaskStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing(id) => write!(f, "taskid {id} not found"),
            Self::NoFields => {
                f.write_str("at least one of status, subject, description is required")
            }
        }
    }
}

impl std::error::Error for TaskStoreError {}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TaskCreateParams {
    /// One-line task subject.
    subject: String,
    /// Detailed task description.
    description: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TaskListParams {}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TaskGetParams {
    taskid: u64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TaskUpdateParams {
    taskid: u64,
    #[serde(default)]
    status: Option<TaskStatus>,
    #[serde(default)]
    subject: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

struct TaskCreateTool {
    store: TaskStore,
}

struct TaskListTool {
    store: TaskStore,
}

struct TaskGetTool {
    store: TaskStore,
}

struct TaskUpdateTool {
    store: TaskStore,
}

const CREATE_DESCRIPTION: &str = "Create a session-lifetime task only when user-visible \
progress tracking is genuinely useful: multiple active tasks must be remembered, or the work \
will involve long edits, long-running commands, extended investigation, or interruption-prone \
coordination. Do not create a task just because a request has several steps, and do not create \
one for short questions, quick checks, single reviews, or one-off commands. Prefer updating an \
existing active task over creating a duplicate. Input only `subject` and `description`; `taskid` \
is assigned automatically and initial `status` is `pending`.";
const LIST_DESCRIPTION: &str = "List every session-lifetime task, including completed and \
deleted entries. Tasks are user-visible real-time status for short-term current-work tracking. \
Takes an empty object as input.";
const GET_DESCRIPTION: &str = "Get one session-lifetime task by `taskid`. Tasks are \
user-visible real-time status for short-term current-work tracking. Returns an error if the task \
does not exist.";
const UPDATE_DESCRIPTION: &str = "Update an existing session-lifetime task when meaningful \
progress changes between substantial steps. Tasks are user-visible real-time status, so avoid \
churn for trivial substeps. Keep status current with `pending`, `inprogress`, `completed`, or \
`deleted`. Provide `taskid` and at least one of `status`, `subject`, or `description`; deletion is \
logical (`status = deleted`). If an unexpected problem blocks progress, do not force the next \
step: leave the task as-is, summarize the problem to the user, and end the turn.";

#[async_trait]
impl Tool for TaskCreateTool {
    async fn execute(&self, input_json: &str) -> Result<ToolOutput, ToolError> {
        let params: TaskCreateParams = serde_json::from_str(input_json)
            .map_err(|e| ToolError::InvalidArgument(format!("invalid TaskCreate input: {e}")))?;
        let created = self.store.create(params.subject, params.description);
        let tasks = self.store.list();
        Ok(task_output(
            format!(
                "Created task {} ({})\n{}",
                created.taskid,
                created.status,
                snapshot_overview(&tasks)
            ),
            &created,
            &tasks,
        ))
    }
}

#[async_trait]
impl Tool for TaskListTool {
    async fn execute(&self, input_json: &str) -> Result<ToolOutput, ToolError> {
        let _: TaskListParams = serde_json::from_str(input_json)
            .map_err(|e| ToolError::InvalidArgument(format!("invalid TaskList input: {e}")))?;
        let tasks = self.store.list();
        Ok(ToolOutput {
            summary: snapshot_overview(&tasks),
            content: Some(render_snapshot(&tasks)),
        })
    }
}

#[async_trait]
impl Tool for TaskGetTool {
    async fn execute(&self, input_json: &str) -> Result<ToolOutput, ToolError> {
        let params: TaskGetParams = serde_json::from_str(input_json)
            .map_err(|e| ToolError::InvalidArgument(format!("invalid TaskGet input: {e}")))?;
        let task = self.store.get(params.taskid).ok_or_else(|| {
            ToolError::ExecutionFailed(format!("taskid {} not found", params.taskid))
        })?;
        let content = serde_json::to_string_pretty(&task).unwrap_or_else(|_| format!("{task:?}"));
        Ok(ToolOutput {
            summary: format!("Task {} ({}) {}", task.taskid, task.status, task.subject),
            content: Some(content),
        })
    }
}

#[async_trait]
impl Tool for TaskUpdateTool {
    async fn execute(&self, input_json: &str) -> Result<ToolOutput, ToolError> {
        let params: TaskUpdateParams = serde_json::from_str(input_json)
            .map_err(|e| ToolError::InvalidArgument(format!("invalid TaskUpdate input: {e}")))?;
        let updated = self
            .store
            .update(
                params.taskid,
                params.status,
                params.subject,
                params.description,
            )
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
        let tasks = self.store.list();
        Ok(task_output(
            format!(
                "Updated task {} ({})\n{}",
                updated.taskid,
                updated.status,
                snapshot_overview(&tasks)
            ),
            &updated,
            &tasks,
        ))
    }
}

fn task_output(summary: String, task: &TaskEntry, tasks: &[TaskEntry]) -> ToolOutput {
    let content = serde_json::json!({
        "task": task,
        "snapshot": { "tasks": tasks },
    });
    ToolOutput {
        summary,
        content: Some(serde_json::to_string_pretty(&content).unwrap_or_default()),
    }
}

pub fn snapshot_overview(tasks: &[TaskEntry]) -> String {
    let pending = tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Pending)
        .count();
    let inprogress = tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Inprogress)
        .count();
    let completed = tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Completed)
        .count();
    let deleted = tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Deleted)
        .count();
    format!(
        "TaskStore: {} task(s) (pending: {pending}, inprogress: {inprogress}, completed: {completed}, deleted: {deleted})",
        tasks.len()
    )
}

pub fn render_snapshot(tasks: &[TaskEntry]) -> String {
    let snapshot = TaskSnapshot {
        tasks: tasks.to_vec(),
    };
    let json =
        serde_json::to_string_pretty(&snapshot).unwrap_or_else(|_| String::from("{\"tasks\":[]}"));
    format!("{}\n\n```json\n{}\n```\n", snapshot_overview(tasks), json)
}

fn parse_compact_snapshot_text(text: &str) -> Option<Vec<TaskEntry>> {
    if !text.starts_with("[Session TaskStore snapshot]") {
        return None;
    }
    let start_marker = "```json\n";
    let end_marker = "\n```";
    let start = text.find(start_marker)? + start_marker.len();
    let rest = &text[start..];
    let end = rest.find(end_marker)?;
    let snapshot: TaskSnapshot = serde_json::from_str(&rest[..end]).ok()?;
    Some(snapshot.tasks)
}

fn task_create_tool(store: TaskStore) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(TaskCreateParams);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("TaskCreate")
            .description(CREATE_DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(TaskCreateTool {
            store: store.clone(),
        });
        (meta, tool)
    })
}

fn task_list_tool(store: TaskStore) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(TaskListParams);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("TaskList")
            .description(LIST_DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(TaskListTool {
            store: store.clone(),
        });
        (meta, tool)
    })
}

fn task_get_tool(store: TaskStore) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(TaskGetParams);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("TaskGet")
            .description(GET_DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(TaskGetTool {
            store: store.clone(),
        });
        (meta, tool)
    })
}

fn task_update_tool(store: TaskStore) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(TaskUpdateParams);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("TaskUpdate")
            .description(UPDATE_DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(TaskUpdateTool {
            store: store.clone(),
        });
        (meta, tool)
    })
}

pub fn task_tools(store: TaskStore) -> Vec<ToolDefinition> {
    vec![
        task_create_tool(store.clone()),
        task_list_tool(store.clone()),
        task_get_tool(store.clone()),
        task_update_tool(store),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(def: ToolDefinition) -> Arc<dyn Tool> {
        let (_, tool) = def();
        tool
    }

    #[tokio::test]
    async fn task_tools_create_list_get_update() {
        let store = TaskStore::new();
        let create = tool(task_create_tool(store.clone()));
        let list = tool(task_list_tool(store.clone()));
        let get = tool(task_get_tool(store.clone()));
        let update = tool(task_update_tool(store.clone()));

        let out = create
            .execute(r#"{"subject":"implement","description":"write code"}"#)
            .await
            .unwrap();
        assert!(out.summary.contains("Created task 1"));
        assert_eq!(store.get(1).unwrap().status, TaskStatus::Pending);

        let out = update
            .execute(r#"{"taskid":1,"status":"inprogress","subject":"implement tasks"}"#)
            .await
            .unwrap();
        assert!(out.summary.contains("Updated task 1"));
        let task = store.get(1).unwrap();
        assert_eq!(task.status, TaskStatus::Inprogress);
        assert_eq!(task.subject, "implement tasks");

        let out = get.execute(r#"{"taskid":1}"#).await.unwrap();
        assert!(out.summary.contains("Task 1 (inprogress)"));
        assert!(out.content.unwrap().contains("implement tasks"));

        let out = list.execute("{}").await.unwrap();
        assert!(out.summary.contains("1 task(s)"));
        let content = out.content.unwrap();
        assert!(content.contains("\"taskid\": 1"));
        assert!(content.contains("```json"));
    }

    #[tokio::test]
    async fn task_update_validates_existing_and_at_least_one_field() {
        let store = TaskStore::new();
        store.create("s".into(), "d".into());
        let update = tool(task_update_tool(store));

        let err = update.execute(r#"{"taskid":1}"#).await.unwrap_err();
        assert!(err.to_string().contains("at least one"));

        let err = update
            .execute(r#"{"taskid":99,"status":"deleted"}"#)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("taskid 99 not found"));
    }

    #[test]
    fn replay_history_reconstructs_store_and_ignores_malformed_calls() {
        let history = vec![
            Item::tool_call("c1", "TaskCreate", r#"{"subject":"a","description":"A"}"#),
            Item::tool_call("bad", "TaskCreate", r#"{"subject":1}"#),
            Item::tool_call("c2", "TaskCreate", r#"{"subject":"b","description":"B"}"#),
            Item::tool_call("u1", "TaskUpdate", r#"{"taskid":2,"status":"completed"}"#),
            Item::tool_call("bad2", "TaskUpdate", r#"{"taskid":99,"status":"deleted"}"#),
        ];
        let store = TaskStore::from_history(&history);
        let tasks = store.list();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].taskid, 1);
        assert_eq!(tasks[0].status, TaskStatus::Pending);
        assert_eq!(tasks[1].taskid, 2);
        assert_eq!(tasks[1].status, TaskStatus::Completed);
    }

    /// Wrap snapshot text the way `Pod::try_pre_run_compact` does, so tests
    /// exercise the exact format that goes through the session log.
    fn wrap_snapshot_system_message(snapshot: &str) -> String {
        format!(
            "[Session TaskStore snapshot]\n\n{snapshot}\n\n\
             This is the complete session task list preserved across compaction. \
             The following TaskList tool result presents the same state through the tool lane."
        )
    }

    #[test]
    fn replay_history_uses_compact_snapshot_and_continues_updates() {
        let pre = TaskStore::new();
        pre.create("kept".into(), "from compact".into());
        pre.update(1, Some(TaskStatus::Inprogress), None, None)
            .unwrap();
        let history = vec![
            Item::system_message(wrap_snapshot_system_message(&pre.snapshot_text())),
            Item::tool_call("u1", "TaskUpdate", r#"{"taskid":1,"status":"completed"}"#),
            Item::tool_call(
                "c2",
                "TaskCreate",
                r#"{"subject":"new","description":"after compact"}"#,
            ),
        ];
        let store = TaskStore::from_history(&history);
        let tasks = store.list();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].taskid, 1);
        assert_eq!(tasks[0].status, TaskStatus::Completed);
        assert_eq!(tasks[1].taskid, 2);
        assert_eq!(tasks[1].subject, "new");
    }

    #[test]
    fn trailing_snapshot_supersedes_pre_compact_taskcreates_in_retained() {
        // Mirrors the post-compact layout: pre-compact `TaskCreate` calls are
        // preserved verbatim in retained_items, and the snapshot trails them.
        // The trailing snapshot must reset the store to the captured state so
        // pre-compact `TaskCreate`s do not surface as duplicates.
        let pre = TaskStore::new();
        pre.create("A".into(), "A-desc".into());
        pre.update(1, Some(TaskStatus::Completed), None, None)
            .unwrap();
        pre.create("B".into(), "B-desc".into());
        pre.update(2, Some(TaskStatus::Inprogress), None, None)
            .unwrap();
        let history = vec![
            Item::tool_call(
                "c1",
                "TaskCreate",
                r#"{"subject":"A","description":"A-desc"}"#,
            ),
            Item::tool_call("u1", "TaskUpdate", r#"{"taskid":1,"status":"completed"}"#),
            Item::tool_call(
                "c2",
                "TaskCreate",
                r#"{"subject":"B","description":"B-desc"}"#,
            ),
            Item::tool_call("u2", "TaskUpdate", r#"{"taskid":2,"status":"inprogress"}"#),
            Item::system_message(wrap_snapshot_system_message(&pre.snapshot_text())),
            Item::tool_call("compact-tasklist", "TaskList", "{}"),
            Item::tool_call(
                "c3",
                "TaskCreate",
                r#"{"subject":"C","description":"after compact"}"#,
            ),
        ];
        let store = TaskStore::from_history(&history);
        let tasks = store.list();
        assert_eq!(tasks.len(), 3);
        assert_eq!(tasks[0].taskid, 1);
        assert_eq!(tasks[0].subject, "A");
        assert_eq!(tasks[0].status, TaskStatus::Completed);
        assert_eq!(tasks[1].taskid, 2);
        assert_eq!(tasks[1].subject, "B");
        assert_eq!(tasks[1].status, TaskStatus::Inprogress);
        assert_eq!(tasks[2].taskid, 3);
        assert_eq!(tasks[2].subject, "C");
    }

    #[test]
    fn snapshot_round_trips_multiline_subject_and_description() {
        // Subject / description with embedded newlines and shape-breaking
        // characters must survive snapshot serialization unchanged.
        let pre = TaskStore::new();
        pre.create(
            "subject with\nembedded newline\n- bullet".into(),
            "desc:\n  status: not-actually-a-field\n  ```code fence```".into(),
        );
        pre.update(1, Some(TaskStatus::Inprogress), None, None)
            .unwrap();

        let history = vec![Item::system_message(wrap_snapshot_system_message(
            &pre.snapshot_text(),
        ))];
        let store = TaskStore::from_history(&history);
        let tasks = store.list();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].subject, "subject with\nembedded newline\n- bullet");
        assert_eq!(
            tasks[0].description,
            "desc:\n  status: not-actually-a-field\n  ```code fence```"
        );
        assert_eq!(tasks[0].status, TaskStatus::Inprogress);
    }

    #[test]
    fn synthetic_compact_tasklist_pair_is_well_formed() {
        // Mirrors `Pod::try_pre_run_compact`'s synthetic insertion:
        // a system snapshot message followed by a TaskList tool_call/tool_result
        // pair sharing the `compact-tasklist` id. Verify the structural
        // contract every provider request builder relies on (matched call_id,
        // tool name, content recoverable to the same TaskStore state).
        let pre = TaskStore::new();
        pre.create("plan".into(), "do A then B".into());

        let snapshot_text = pre.snapshot_text();
        let system = Item::system_message(wrap_snapshot_system_message(&snapshot_text));
        let call = Item::tool_call("compact-tasklist", "TaskList", "{}");
        let result = Item::tool_result_with_content(
            "compact-tasklist",
            snapshot_overview(&pre.list()),
            snapshot_text.clone(),
        );

        // The system message embeds a parseable snapshot.
        let extracted = system
            .as_text()
            .and_then(parse_compact_snapshot_text)
            .expect("system message should parse as snapshot");
        assert_eq!(extracted, pre.list());

        // The synthetic call/result pair shares one call_id and carries the
        // expected tool name + detailed content.
        match (&call, &result) {
            (
                Item::ToolCall {
                    call_id: c_id,
                    name,
                    ..
                },
                Item::ToolResult {
                    call_id: r_id,
                    content,
                    ..
                },
            ) => {
                assert_eq!(c_id.as_str(), r_id.as_str());
                assert_eq!(c_id.as_str(), "compact-tasklist");
                assert_eq!(name, "TaskList");
                assert_eq!(content.as_deref(), Some(snapshot_text.as_str()));
            }
            other => panic!("unexpected synthetic pair shape: {other:?}"),
        }

        // Replaying the full triple reconstructs the same TaskStore.
        let store = TaskStore::from_history(&[system, call, result]);
        assert_eq!(store.list(), pre.list());
    }
}
