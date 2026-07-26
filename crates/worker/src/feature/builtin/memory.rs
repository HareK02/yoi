//! Workspace-HTTP backed Memory tools.
//!
//! Runtime workers may have Workspace authority without direct local filesystem
//! authority. In that case model-visible Memory tools must go through the
//! workspace backend instead of resolving `.yoi/memory` from a Worker workdir.

use std::sync::Arc;

use async_trait::async_trait;
use llm_engine::tool::{
    Tool, ToolDefinition, ToolError, ToolExecutionContext, ToolMeta, ToolOutput,
};
use memory::backend::{
    MemoryBackendHttpResponse, MemoryBackendOperation, MemoryBackendOperationResult,
    MemoryConsolidateStagingOperation, MemoryConsolidationOutput, MemoryDocumentReadOperation,
    MemoryDocumentUpdateOperation, MemoryQueryOperation, MemoryStagingCloseOperation,
    MemoryStagingListOperation, MemoryStagingReadOperation, MemoryToolOutput,
};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde_json::json;

use crate::worker::WorkspaceClient;

#[derive(Clone, Debug)]
pub struct WorkspaceHttpMemoryBackend {
    workspace_id: String,
    base_url: String,
}

impl WorkspaceHttpMemoryBackend {
    pub fn new(workspace_id: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            workspace_id: workspace_id.into(),
            base_url: base_url.into(),
        }
    }

    pub async fn execute_operation(
        &self,
        operation: MemoryBackendOperation,
    ) -> Result<MemoryBackendOperationResult, WorkspaceMemoryBackendError> {
        execute_http_memory_backend(&self.workspace_id, &self.base_url, operation).await
    }

    async fn execute(&self, operation: MemoryBackendOperation) -> Result<ToolOutput, ToolError> {
        match self.execute_operation(operation).await {
            Ok(MemoryBackendOperationResult::ToolOutput(output)) => Ok(tool_output(output)),
            Ok(result) => Err(ToolError::ExecutionFailed(format!(
                "unexpected memory backend result for model-visible tool: {result:?}"
            ))),
            Err(error) => Err(ToolError::ExecutionFailed(error.to_string())),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceMemoryBackendError {
    #[error("workspace memory backend is unavailable: {reason}")]
    Unavailable { reason: String },
    #[error("workspace memory backend request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("workspace memory backend returned HTTP {status}: {body}")]
    Http {
        status: reqwest::StatusCode,
        body: String,
    },
    #[error("decode memory backend response: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("workspace memory backend rejected operation: {0}")]
    Backend(String),
}

impl WorkspaceClient {
    pub async fn execute_memory_backend_operation(
        &self,
        operation: MemoryBackendOperation,
    ) -> Result<MemoryBackendOperationResult, WorkspaceMemoryBackendError> {
        match self {
            WorkspaceClient::Http {
                workspace_id,
                base_url,
            } => execute_http_memory_backend(workspace_id, base_url, operation).await,
            WorkspaceClient::Available { kind } => Err(WorkspaceMemoryBackendError::Unavailable {
                reason: format!(
                    "workspace client kind `{kind}` does not expose the Backend Workspace API"
                ),
            }),
            WorkspaceClient::Unavailable { reason } => {
                Err(WorkspaceMemoryBackendError::Unavailable {
                    reason: reason.clone(),
                })
            }
        }
    }

    pub async fn request_memory_staging_consolidation(
        &self,
        operation: MemoryConsolidateStagingOperation,
    ) -> Result<MemoryConsolidationOutput, WorkspaceMemoryBackendError> {
        match self {
            WorkspaceClient::Http {
                workspace_id,
                base_url,
            } => execute_http_memory_consolidation(workspace_id, base_url, operation).await,
            WorkspaceClient::Available { kind } => Err(WorkspaceMemoryBackendError::Unavailable {
                reason: format!(
                    "workspace client kind `{kind}` does not expose the Backend Workspace API"
                ),
            }),
            WorkspaceClient::Unavailable { reason } => {
                Err(WorkspaceMemoryBackendError::Unavailable {
                    reason: reason.clone(),
                })
            }
        }
    }
}

async fn execute_http_memory_backend(
    workspace_id: &str,
    base_url: &str,
    operation: MemoryBackendOperation,
) -> Result<MemoryBackendOperationResult, WorkspaceMemoryBackendError> {
    let url = format!(
        "{}/api/w/{}/memory/backend",
        base_url.trim_end_matches('/'),
        workspace_id
    );
    let response = reqwest::Client::new()
        .post(url)
        .json(&operation)
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        return Err(WorkspaceMemoryBackendError::Http { status, body });
    }
    match serde_json::from_str::<MemoryBackendHttpResponse>(&body)? {
        MemoryBackendHttpResponse::Ok { result } => Ok(result),
        MemoryBackendHttpResponse::Error { message } => {
            Err(WorkspaceMemoryBackendError::Backend(message))
        }
    }
}

async fn execute_http_memory_consolidation(
    workspace_id: &str,
    base_url: &str,
    operation: MemoryConsolidateStagingOperation,
) -> Result<MemoryConsolidationOutput, WorkspaceMemoryBackendError> {
    let url = format!(
        "{}/api/w/{}/memory/consolidation",
        base_url.trim_end_matches('/'),
        workspace_id
    );
    let response = reqwest::Client::new()
        .post(url)
        .json(&operation)
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        return Err(WorkspaceMemoryBackendError::Http { status, body });
    }
    serde_json::from_str::<MemoryConsolidationOutput>(&body).map_err(Into::into)
}

pub fn workspace_http_memory_tools(
    workspace_id: impl Into<String>,
    base_url: impl Into<String>,
) -> Vec<ToolDefinition> {
    let backend = WorkspaceHttpMemoryBackend::new(workspace_id, base_url);
    vec![
        memory_tool(
            "MemoryReadDocument",
            READ_DOCUMENT_DESCRIPTION,
            document_read_schema(),
            backend.clone(),
            |input| {
                Ok(MemoryBackendOperation::ReadDocument(parse_input::<
                    MemoryDocumentReadOperation,
                >(
                    input
                )?))
            },
        ),
        memory_tool(
            "MemoryUpdateDocument",
            UPDATE_DOCUMENT_DESCRIPTION,
            document_update_schema(),
            backend.clone(),
            |input| {
                Ok(MemoryBackendOperation::UpdateDocument(parse_input::<
                    MemoryDocumentUpdateOperation,
                >(
                    input
                )?))
            },
        ),
        memory_tool(
            "MemoryQuery",
            QUERY_DESCRIPTION,
            query_schema(),
            backend,
            |input| {
                Ok(MemoryBackendOperation::Query(parse_input::<
                    MemoryQueryOperation,
                >(input)?))
            },
        ),
    ]
}

pub fn workspace_http_memory_consolidation_tools(
    workspace_id: impl Into<String>,
    base_url: impl Into<String>,
) -> Vec<ToolDefinition> {
    let workspace_id = workspace_id.into();
    let base_url = base_url.into();
    let mut tools = workspace_http_memory_tools(workspace_id.clone(), base_url.clone());
    let backend = WorkspaceHttpMemoryBackend::new(workspace_id, base_url);
    tools.extend([
        memory_tool(
            "MemoryStagingList",
            STAGING_LIST_DESCRIPTION,
            schema_for::<MemoryStagingListOperation>(),
            backend.clone(),
            |input| {
                Ok(MemoryBackendOperation::StagingList(parse_input::<
                    MemoryStagingListOperation,
                >(
                    input
                )?))
            },
        ),
        memory_tool(
            "MemoryStagingRead",
            STAGING_READ_DESCRIPTION,
            schema_for::<MemoryStagingReadOperation>(),
            backend.clone(),
            |input| {
                Ok(MemoryBackendOperation::StagingRead(parse_input::<
                    MemoryStagingReadOperation,
                >(
                    input
                )?))
            },
        ),
        memory_tool(
            "MemoryStagingClose",
            STAGING_CLOSE_DESCRIPTION,
            schema_for::<MemoryStagingCloseOperation>(),
            backend,
            |input| {
                Ok(MemoryBackendOperation::StagingClose(parse_input::<
                    MemoryStagingCloseOperation,
                >(
                    input
                )?))
            },
        ),
    ]);
    tools
}

type OperationBuilder = fn(&str) -> Result<MemoryBackendOperation, ToolError>;

fn memory_tool(
    name: &'static str,
    description: &'static str,
    schema: serde_json::Value,
    backend: WorkspaceHttpMemoryBackend,
    build: OperationBuilder,
) -> ToolDefinition {
    Arc::new(move || {
        (
            ToolMeta::new(name)
                .description(description)
                .input_schema(schema.clone()),
            Arc::new(WorkspaceHttpMemoryTool {
                backend: backend.clone(),
                build,
            }) as Arc<dyn Tool>,
        )
    })
}

#[derive(Clone)]
struct WorkspaceHttpMemoryTool {
    backend: WorkspaceHttpMemoryBackend,
    build: OperationBuilder,
}

#[async_trait]
impl Tool for WorkspaceHttpMemoryTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let operation = (self.build)(input_json)?;
        self.backend.execute(operation).await
    }
}

fn parse_input<T: DeserializeOwned>(input: &str) -> Result<T, ToolError> {
    serde_json::from_str(input).map_err(|error| ToolError::InvalidArgument(error.to_string()))
}

fn schema_for<T: JsonSchema>() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(T)).expect("memory tool schema should serialize")
}

fn tool_output(output: MemoryToolOutput) -> ToolOutput {
    ToolOutput {
        summary: output.summary,
        content: output.content,
    }
}

const READ_DOCUMENT_DESCRIPTION: &str =
    "Read the Workspace memory Markdown document through Workspace authority.";
const UPDATE_DOCUMENT_DESCRIPTION: &str =
    "Replace the Workspace memory Markdown document through Workspace authority.";
const QUERY_DESCRIPTION: &str = "Query the Workspace memory document through Workspace authority.";
const STAGING_LIST_DESCRIPTION: &str =
    "List pending Memory staging candidates without loading full record payloads.";
const STAGING_READ_DESCRIPTION: &str = "Read one pending Memory staging candidate by candidate_id.";
const STAGING_CLOSE_DESCRIPTION: &str = "Close one staging candidate with a required reason; records disposition and deletes the staging record.";

fn document_read_schema() -> serde_json::Value {
    json!({
        "type":"object",
        "additionalProperties": false,
        "properties":{
            "offset":{"type":["integer","null"],"minimum":0},
            "limit":{"type":["integer","null"],"minimum":0}
        }
    })
}

fn document_update_schema() -> serde_json::Value {
    json!({
        "type":"object",
        "additionalProperties": false,
        "required":["body_md"],
        "properties":{
            "body_md":{"type":"string"}
        }
    })
}

fn query_schema() -> serde_json::Value {
    json!({
        "type":"object",
        "additionalProperties": false,
        "properties":{
            "query":{"type":["string","null"]}
        }
    })
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
    fn normal_workspace_memory_tools_do_not_include_staging_tools() {
        let names = tool_names(workspace_http_memory_tools(
            "workspace".to_string(),
            "http://backend".to_string(),
        ));

        assert!(names.contains(&"MemoryQuery".to_string()));
        assert!(names.contains(&"MemoryReadDocument".to_string()));
        assert!(names.contains(&"MemoryUpdateDocument".to_string()));
        assert!(!names.contains(&"MemoryRead".to_string()));
        assert!(!names.contains(&"MemoryWrite".to_string()));
        assert!(!names.contains(&"MemoryEdit".to_string()));
        assert!(!names.contains(&"MemoryDelete".to_string()));
        assert!(!names.contains(&"MemoryStagingList".to_string()));
        assert!(!names.contains(&"MemoryStagingRead".to_string()));
        assert!(!names.contains(&"MemoryStagingClose".to_string()));
    }

    #[test]
    fn consolidation_workspace_memory_tools_include_staging_tools() {
        let names = tool_names(workspace_http_memory_consolidation_tools(
            "workspace".to_string(),
            "http://backend".to_string(),
        ));

        assert!(names.contains(&"MemoryQuery".to_string()));
        assert!(names.contains(&"MemoryReadDocument".to_string()));
        assert!(names.contains(&"MemoryUpdateDocument".to_string()));
        assert!(names.contains(&"MemoryStagingList".to_string()));
        assert!(names.contains(&"MemoryStagingRead".to_string()));
        assert!(names.contains(&"MemoryStagingClose".to_string()));
    }
}
