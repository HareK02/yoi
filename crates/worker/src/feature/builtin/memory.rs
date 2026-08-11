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

use crate::worker::{
    WorkspaceClient, WorkspaceClientError, WorkspaceRequest, WorkspaceRequestMethod,
};

#[derive(Clone, Debug)]
pub struct WorkspaceHttpMemoryBackend {
    client: Arc<dyn WorkspaceClient>,
}

impl WorkspaceHttpMemoryBackend {
    pub fn new(client: Arc<dyn WorkspaceClient>) -> Self {
        Self { client }
    }

    pub async fn execute_operation(
        &self,
        operation: MemoryBackendOperation,
    ) -> Result<MemoryBackendOperationResult, WorkspaceMemoryBackendError> {
        execute_memory_backend(self.client.as_ref(), operation).await
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
    Request(#[from] WorkspaceClientError),
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

impl dyn WorkspaceClient + '_ {
    pub async fn execute_memory_backend_operation(
        &self,
        operation: MemoryBackendOperation,
    ) -> Result<MemoryBackendOperationResult, WorkspaceMemoryBackendError> {
        execute_memory_backend(self, operation).await
    }

    pub async fn request_memory_staging_consolidation(
        &self,
        operation: MemoryConsolidateStagingOperation,
    ) -> Result<MemoryConsolidationOutput, WorkspaceMemoryBackendError> {
        execute_memory_consolidation(self, operation).await
    }
}

async fn execute_memory_backend(
    client: &dyn WorkspaceClient,
    operation: MemoryBackendOperation,
) -> Result<MemoryBackendOperationResult, WorkspaceMemoryBackendError> {
    let workspace_id =
        client
            .workspace_id()
            .ok_or_else(|| WorkspaceMemoryBackendError::Unavailable {
                reason: format!(
                    "workspace client kind `{}` has no workspace id",
                    client.kind()
                ),
            })?;
    let response = client.execute(WorkspaceRequest::json(
        WorkspaceRequestMethod::Post,
        format!("/api/w/{workspace_id}/memory/backend"),
        serde_json::to_string(&operation)?,
    ))?;
    let status = reqwest::StatusCode::from_u16(response.status)
        .unwrap_or(reqwest::StatusCode::INTERNAL_SERVER_ERROR);
    if !response.is_success() {
        return Err(WorkspaceMemoryBackendError::Http {
            status,
            body: response.body,
        });
    }
    match serde_json::from_str::<MemoryBackendHttpResponse>(&response.body)? {
        MemoryBackendHttpResponse::Ok { result } => Ok(result),
        MemoryBackendHttpResponse::Error { message } => {
            Err(WorkspaceMemoryBackendError::Backend(message))
        }
    }
}

async fn execute_memory_consolidation(
    client: &dyn WorkspaceClient,
    operation: MemoryConsolidateStagingOperation,
) -> Result<MemoryConsolidationOutput, WorkspaceMemoryBackendError> {
    let workspace_id =
        client
            .workspace_id()
            .ok_or_else(|| WorkspaceMemoryBackendError::Unavailable {
                reason: format!(
                    "workspace client kind `{}` has no workspace id",
                    client.kind()
                ),
            })?;
    let response = client.execute(WorkspaceRequest::json(
        WorkspaceRequestMethod::Post,
        format!("/api/w/{workspace_id}/memory/consolidation"),
        serde_json::to_string(&operation)?,
    ))?;
    let status = reqwest::StatusCode::from_u16(response.status)
        .unwrap_or(reqwest::StatusCode::INTERNAL_SERVER_ERROR);
    if !response.is_success() {
        return Err(WorkspaceMemoryBackendError::Http {
            status,
            body: response.body,
        });
    }
    serde_json::from_str::<MemoryConsolidationOutput>(&response.body).map_err(Into::into)
}

pub fn workspace_http_memory_tools(client: Arc<dyn WorkspaceClient>) -> Vec<ToolDefinition> {
    let backend = WorkspaceHttpMemoryBackend::new(client);
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
    client: Arc<dyn WorkspaceClient>,
) -> Vec<ToolDefinition> {
    let mut tools = workspace_http_memory_tools(client.clone());
    let backend = WorkspaceHttpMemoryBackend::new(client);
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
        attachments: Vec::new(),
    }
}

const READ_DOCUMENT_DESCRIPTION: &str =
    "Read the Workspace memory Markdown document through Workspace authority.";
const UPDATE_DOCUMENT_DESCRIPTION: &str =
    "Edit the Workspace memory Markdown document by replacing an exact old_string with new_string.";
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
        "required":["old_string", "new_string"],
        "properties":{
            "old_string":{"type":"string", "minLength": 1},
            "new_string":{"type":"string"},
            "replace_all":{"type":"boolean", "default": false}
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

    fn test_client() -> Arc<dyn WorkspaceClient> {
        Arc::new(crate::worker::TestWorkspaceHttpClient::new(
            "workspace",
            "http://backend",
        ))
    }

    fn tool_names(definitions: Vec<ToolDefinition>) -> Vec<String> {
        let mut names = definitions
            .into_iter()
            .map(|tool| tool().0.name)
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    fn tool_meta(definitions: Vec<ToolDefinition>, name: &str) -> serde_json::Value {
        definitions
            .into_iter()
            .map(|tool| tool().0)
            .find(|meta| meta.name == name)
            .unwrap_or_else(|| panic!("missing tool meta for {name}"))
            .input_schema
    }

    #[test]
    fn normal_workspace_memory_tools_do_not_include_staging_tools() {
        let names = tool_names(workspace_http_memory_tools(test_client()));

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
    fn document_update_schema_is_edit_like_and_staging_close_has_no_legacy_kinds() {
        let update_schema = tool_meta(
            workspace_http_memory_tools(test_client()),
            "MemoryUpdateDocument",
        );
        assert_eq!(
            update_schema["required"],
            serde_json::json!(["old_string", "new_string"])
        );
        assert!(update_schema["properties"].get("old_string").is_some());
        assert!(update_schema["properties"].get("new_string").is_some());
        assert!(update_schema["properties"].get("replace_all").is_some());
        assert!(update_schema["properties"].get("body_md").is_none());

        let close_schema_text = tool_meta(
            workspace_http_memory_consolidation_tools(test_client()),
            "MemoryStagingClose",
        )
        .to_string();
        assert!(close_schema_text.contains("affected_memory"));
        assert!(close_schema_text.contains("edit"));
        assert!(!close_schema_text.contains("summary"));
        assert!(!close_schema_text.contains("decision"));
        assert!(!close_schema_text.contains("request"));
        assert!(!close_schema_text.contains("slug"));
    }

    #[test]
    fn consolidation_workspace_memory_tools_include_staging_tools() {
        let names = tool_names(workspace_http_memory_consolidation_tools(test_client()));

        assert!(names.contains(&"MemoryQuery".to_string()));
        assert!(names.contains(&"MemoryReadDocument".to_string()));
        assert!(names.contains(&"MemoryUpdateDocument".to_string()));
        assert!(names.contains(&"MemoryStagingList".to_string()));
        assert!(names.contains(&"MemoryStagingRead".to_string()));
        assert!(names.contains(&"MemoryStagingClose".to_string()));
    }
}
