//! Backend Workspace API backed Objective read tools.
//!
//! Objectives are project-level planning context. Runtime Workers may not know
//! local `.yoi/objectives` paths, so model-visible Objective tools go through
//! the scoped Workspace API.

use std::sync::Arc;

use async_trait::async_trait;
use llm_engine::tool::{
    Tool, ToolDefinition, ToolError, ToolExecutionContext, ToolMeta, ToolOutput,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Clone, Debug)]
pub struct WorkspaceHttpObjectiveBackend {
    workspace_id: String,
    base_url: String,
}

impl WorkspaceHttpObjectiveBackend {
    pub fn new(workspace_id: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            workspace_id: workspace_id.into(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
        }
    }

    async fn list(&self, input: ObjectiveListInput) -> Result<ToolOutput, ToolError> {
        let mut url = format!("{}/api/w/{}/objectives", self.base_url, self.workspace_id);
        if let Some(limit) = input.limit {
            url.push_str(&format!("?limit={}", limit.min(1000)));
        }
        let response = get_json::<ObjectiveListResponse>(&url)
            .await
            .map_err(backend_error)?;
        let count = response.items.len();
        Ok(ToolOutput {
            summary: format!("Listed {count} objective(s)"),
            content: Some(serde_json::to_string_pretty(&response).map_err(decode_error)?),
        })
    }

    async fn show(&self, input: ObjectiveShowInput) -> Result<ToolOutput, ToolError> {
        let id = input.id.trim();
        if id.is_empty() || id.contains('/') {
            return Err(ToolError::InvalidArgument(
                "ObjectiveShow requires non-empty canonical id without '/'".to_string(),
            ));
        }
        let url = format!(
            "{}/api/w/{}/objectives/{}",
            self.base_url, self.workspace_id, id
        );
        let response = get_json::<ObjectiveDetail>(&url)
            .await
            .map_err(backend_error)?;
        Ok(ToolOutput {
            summary: format!("Read objective {}", response.id),
            content: Some(serde_json::to_string_pretty(&response).map_err(decode_error)?),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceObjectiveBackendError {
    #[error("workspace objective backend request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("workspace objective backend returned HTTP {status}: {body}")]
    Http {
        status: reqwest::StatusCode,
        body: String,
    },
    #[error("decode objective backend response: {0}")]
    Decode(#[from] serde_json::Error),
}

fn decode_error(error: serde_json::Error) -> ToolError {
    ToolError::ExecutionFailed(format!("decode objective backend response: {error}"))
}

fn backend_error(error: WorkspaceObjectiveBackendError) -> ToolError {
    ToolError::ExecutionFailed(error.to_string())
}

async fn get_json<T: for<'de> Deserialize<'de>>(
    url: &str,
) -> Result<T, WorkspaceObjectiveBackendError> {
    let response = reqwest::Client::new().get(url).send().await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        return Err(WorkspaceObjectiveBackendError::Http { status, body });
    }
    serde_json::from_str(&body).map_err(Into::into)
}

pub fn workspace_http_objective_tools(
    workspace_id: impl Into<String>,
    base_url: impl Into<String>,
) -> Vec<ToolDefinition> {
    let backend = WorkspaceHttpObjectiveBackend::new(workspace_id, base_url);
    vec![
        objective_tool(
            "ObjectiveList",
            LIST_DESCRIPTION,
            list_schema(),
            backend.clone(),
            ObjectiveOperation::List,
        ),
        objective_tool(
            "ObjectiveShow",
            SHOW_DESCRIPTION,
            show_schema(),
            backend,
            ObjectiveOperation::Show,
        ),
    ]
}

#[derive(Clone, Copy)]
enum ObjectiveOperation {
    List,
    Show,
}

fn objective_tool(
    name: &'static str,
    description: &'static str,
    schema: serde_json::Value,
    backend: WorkspaceHttpObjectiveBackend,
    operation: ObjectiveOperation,
) -> ToolDefinition {
    Arc::new(move || {
        (
            ToolMeta::new(name)
                .description(description)
                .input_schema(schema.clone()),
            Arc::new(WorkspaceHttpObjectiveTool {
                backend: backend.clone(),
                operation,
            }) as Arc<dyn Tool>,
        )
    })
}

#[derive(Clone)]
struct WorkspaceHttpObjectiveTool {
    backend: WorkspaceHttpObjectiveBackend,
    operation: ObjectiveOperation,
}

#[async_trait]
impl Tool for WorkspaceHttpObjectiveTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        match self.operation {
            ObjectiveOperation::List => {
                let input = parse_input::<ObjectiveListInput>(input_json)?;
                self.backend.list(input).await
            }
            ObjectiveOperation::Show => {
                let input = parse_input::<ObjectiveShowInput>(input_json)?;
                self.backend.show(input).await
            }
        }
    }
}

fn parse_input<T: for<'de> Deserialize<'de>>(input: &str) -> Result<T, ToolError> {
    serde_json::from_str(input).map_err(|error| ToolError::InvalidArgument(error.to_string()))
}

const LIST_DESCRIPTION: &str =
    "List Objective records through Backend Workspace API authority as bounded summaries.";
const SHOW_DESCRIPTION: &str =
    "Show one Objective record by canonical id through Backend Workspace API authority.";

fn list_schema() -> serde_json::Value {
    json!({
        "type":"object",
        "additionalProperties": false,
        "properties":{
            "limit":{"type":["integer","null"],"minimum":0,"maximum":1000}
        }
    })
}

fn show_schema() -> serde_json::Value {
    json!({
        "type":"object",
        "additionalProperties": false,
        "required":["id"],
        "properties":{
            "id":{"type":"string"}
        }
    })
}

#[derive(Debug, Deserialize)]
struct ObjectiveListInput {
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ObjectiveShowInput {
    id: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct ObjectiveListResponse {
    items: Vec<ObjectiveSummary>,
    invalid_records: Vec<InvalidProjectRecord>,
    record_authority: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct InvalidProjectRecord {
    label: String,
    reason: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct ObjectiveSummary {
    id: String,
    title: String,
    state: String,
    updated_at: Option<String>,
    summary: String,
    linked_tickets: Vec<String>,
    record_source: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct ObjectiveDetail {
    id: String,
    title: String,
    state: String,
    created_at: Option<String>,
    updated_at: Option<String>,
    linked_tickets: Vec<String>,
    body: String,
    body_truncated: bool,
    record_source: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm_engine::tool::ToolDefinition;

    fn tool_names(definitions: Vec<ToolDefinition>) -> Vec<String> {
        let mut names = definitions
            .into_iter()
            .map(|tool| tool().0.name)
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    #[test]
    fn workspace_http_objective_tools_include_read_only_objective_tools() {
        let names = tool_names(workspace_http_objective_tools(
            "workspace".to_string(),
            "http://backend".to_string(),
        ));

        assert_eq!(names, vec!["ObjectiveList", "ObjectiveShow"]);
    }

    #[test]
    fn objective_tool_schemas_are_bounded_and_read_only() {
        let list = list_schema();
        assert_eq!(list["properties"]["limit"]["maximum"], 1000);
        let show = show_schema();
        assert_eq!(show["required"][0], "id");
    }
}
