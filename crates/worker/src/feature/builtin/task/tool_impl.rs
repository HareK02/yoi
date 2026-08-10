//! Task built-in tool implementations.

use std::sync::Arc;

use async_trait::async_trait;
use llm_engine::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use serde::Deserialize;

use super::store::{DEFAULT_TASK_LIST_LIMIT, TaskEntry, TaskStatus, TaskStore, snapshot_overview};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TaskCreateParams {
    /// One-line task subject.
    subject: String,
    /// Detailed task description.
    description: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TaskListParams {
    /// Maximum number of active tasks to return. Defaults to 20.
    #[serde(default)]
    limit: Option<usize>,
}

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
const LIST_DESCRIPTION: &str = "List active session-lifetime tasks. Completed and deleted tasks are forgotten and omitted. Defaults to 20 tasks unless `limit` is provided.";
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
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: TaskCreateParams = serde_json::from_str(input_json)
            .map_err(|e| ToolError::InvalidArgument(format!("invalid TaskCreate input: {e}")))?;
        let created = self.store.create(params.subject, params.description);
        let tasks = self.store.list_active();
        Ok(task_output(
            format!(
                "Created task {} ({})\n{}",
                created.taskid,
                created.status,
                snapshot_overview(&tasks)
            ),
            &created,
        ))
    }
}

#[async_trait]
impl Tool for TaskListTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: TaskListParams = serde_json::from_str(input_json)
            .map_err(|e| ToolError::InvalidArgument(format!("invalid TaskList input: {e}")))?;
        let limit = params.limit.unwrap_or(DEFAULT_TASK_LIST_LIMIT);
        let active_tasks = self.store.list_active();
        let tasks: Vec<_> = active_tasks.iter().take(limit).cloned().collect();
        Ok(ToolOutput {
            summary: list_overview(active_tasks.len(), tasks.len()),
            content: Some(render_task_list(&tasks)),

            attachments: Vec::new(),
        })
    }
}

#[async_trait]
impl Tool for TaskGetTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: TaskGetParams = serde_json::from_str(input_json)
            .map_err(|e| ToolError::InvalidArgument(format!("invalid TaskGet input: {e}")))?;
        let task = self.store.get(params.taskid).ok_or_else(|| {
            ToolError::ExecutionFailed(format!("taskid {} not found", params.taskid))
        })?;
        let content = serde_json::to_string_pretty(&task).unwrap_or_else(|_| format!("{task:?}"));
        Ok(ToolOutput {
            summary: format!("Task {} ({}) {}", task.taskid, task.status, task.subject),
            content: Some(content),

            attachments: Vec::new(),
        })
    }
}

#[async_trait]
impl Tool for TaskUpdateTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
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
        let tasks = self.store.list_active();
        Ok(task_output(
            format!(
                "Updated task {} ({})\n{}",
                updated.taskid,
                updated.status,
                snapshot_overview(&tasks)
            ),
            &updated,
        ))
    }
}

fn task_output(summary: String, task: &TaskEntry) -> ToolOutput {
    ToolOutput {
        summary,
        content: Some(serde_json::to_string_pretty(task).unwrap_or_default()),

        attachments: Vec::new(),
    }
}

fn list_overview(total_active: usize, returned: usize) -> String {
    if returned < total_active {
        format!(
            "TaskStore: {returned} active task(s) shown; {} omitted.",
            total_active - returned
        )
    } else {
        format!("TaskStore: {returned} active task(s)")
    }
}

fn render_task_list(tasks: &[TaskEntry]) -> String {
    serde_json::to_string_pretty(tasks).unwrap_or_default()
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

pub(crate) fn task_tools(store: TaskStore) -> Vec<ToolDefinition> {
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
            .execute(
                r#"{"subject":"implement","description":"write code"}"#,
                Default::default(),
            )
            .await
            .unwrap();
        assert!(out.summary.contains("Created task 1"));
        let created_content = out.content.unwrap();
        let created_json: serde_json::Value = serde_json::from_str(&created_content).unwrap();
        assert_eq!(created_json["taskid"], 1);
        assert!(created_json.get("task").is_none());
        assert_eq!(store.get(1).unwrap().status, TaskStatus::Pending);

        let out = update
            .execute(
                r#"{"taskid":1,"status":"inprogress","subject":"implement tasks"}"#,
                Default::default(),
            )
            .await
            .unwrap();
        assert!(out.summary.contains("Updated task 1"));
        let task = store.get(1).unwrap();
        assert_eq!(task.status, TaskStatus::Inprogress);
        assert_eq!(task.subject, "implement tasks");

        let out = get
            .execute(r#"{"taskid":1}"#, Default::default())
            .await
            .unwrap();
        assert!(out.summary.contains("Task 1 (inprogress)"));
        assert!(out.content.unwrap().contains("implement tasks"));

        let out = list.execute("{}", Default::default()).await.unwrap();
        assert!(out.summary.contains("1 active task(s)"));
        let content = out.content.unwrap();
        assert!(content.contains("\"taskid\": 1"));
        assert!(!content.contains("\"limit\""));
        assert!(!content.contains("```json"));
    }

    #[tokio::test]
    async fn task_list_omits_completed_deleted_and_defaults_to_twenty() {
        let store = TaskStore::new();
        let create = tool(task_create_tool(store.clone()));
        let update = tool(task_update_tool(store.clone()));
        let list = tool(task_list_tool(store.clone()));

        for i in 0..25 {
            create
                .execute(
                    &format!(r#"{{"subject":"task {i}","description":"desc {i}"}}"#),
                    Default::default(),
                )
                .await
                .unwrap();
        }
        update
            .execute(r#"{"taskid":1,"status":"completed"}"#, Default::default())
            .await
            .unwrap();
        update
            .execute(r#"{"taskid":2,"status":"deleted"}"#, Default::default())
            .await
            .unwrap();

        let out = list.execute("{}", Default::default()).await.unwrap();
        assert_eq!(
            out.summary,
            "TaskStore: 20 active task(s) shown; 3 omitted."
        );
        let content = out.content.unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();
        let tasks = json.as_array().unwrap();
        assert_eq!(tasks.len(), 20);
        let ids: Vec<u64> = tasks
            .iter()
            .map(|task| task["taskid"].as_u64().unwrap())
            .collect();
        assert!(!ids.contains(&1));
        assert!(!ids.contains(&2));
        assert!(!content.contains("\"limit\""));
        assert!(!content.contains("\"total_active\""));
        assert!(!content.contains("\"truncated\""));

        let out = list
            .execute(r#"{"limit":3}"#, Default::default())
            .await
            .unwrap();
        assert_eq!(
            out.summary,
            "TaskStore: 3 active task(s) shown; 20 omitted."
        );
        let content = out.content.unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn task_create_and_update_results_do_not_include_full_snapshot() {
        let store = TaskStore::new();
        let create = tool(task_create_tool(store.clone()));
        let update = tool(task_update_tool(store.clone()));

        create
            .execute(
                r#"{"subject":"done","description":"completed task"}"#,
                Default::default(),
            )
            .await
            .unwrap();
        update
            .execute(r#"{"taskid":1,"status":"completed"}"#, Default::default())
            .await
            .unwrap();
        create
            .execute(
                r#"{"subject":"active","description":"active task"}"#,
                Default::default(),
            )
            .await
            .unwrap();

        let out = update
            .execute(r#"{"taskid":2,"status":"inprogress"}"#, Default::default())
            .await
            .unwrap();
        let content = out.content.unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(json["taskid"], 2);
        assert!(json.get("task").is_none());
        assert!(json.get("snapshot").is_none());
        assert!(!content.contains("\"taskid\": 1"));
        assert!(!content.contains("completed task"));
    }

    #[tokio::test]
    async fn task_update_validates_existing_and_at_least_one_field() {
        let store = TaskStore::new();
        store.create("s".into(), "d".into());
        let update = tool(task_update_tool(store));

        let err = update
            .execute(r#"{"taskid":1}"#, Default::default())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("at least one"));

        let err = update
            .execute(r#"{"taskid":99,"status":"deleted"}"#, Default::default())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("taskid 99 not found"));
    }
}
