//! Task built-in tool implementations.

use std::sync::Arc;

use async_trait::async_trait;
use llm_worker::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use serde::Deserialize;

use super::store::{TaskEntry, TaskStatus, TaskStore, render_snapshot, snapshot_overview};

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
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_worker::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
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
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_worker::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
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
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_worker::tool::ToolExecutionContext,
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
        })
    }
}

#[async_trait]
impl Tool for TaskUpdateTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_worker::tool::ToolExecutionContext,
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
