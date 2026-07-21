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
    MemoryDeleteOperation, MemoryEditOperation, MemoryQueryOperation, MemoryReadOperation,
    MemoryToolOutput, MemoryWriteOperation,
};
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

    pub fn execute_operation(
        &self,
        operation: MemoryBackendOperation,
    ) -> Result<MemoryBackendOperationResult, WorkspaceMemoryBackendError> {
        execute_http_memory_backend(&self.workspace_id, &self.base_url, operation)
    }

    fn execute(&self, operation: MemoryBackendOperation) -> Result<ToolOutput, ToolError> {
        match self.execute_operation(operation) {
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
    pub fn execute_memory_backend_operation(
        &self,
        operation: MemoryBackendOperation,
    ) -> Result<MemoryBackendOperationResult, WorkspaceMemoryBackendError> {
        match self {
            WorkspaceClient::Http {
                workspace_id,
                base_url,
            } => execute_http_memory_backend(workspace_id, base_url, operation),
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

fn execute_http_memory_backend(
    workspace_id: &str,
    base_url: &str,
    operation: MemoryBackendOperation,
) -> Result<MemoryBackendOperationResult, WorkspaceMemoryBackendError> {
    let url = format!(
        "{}/api/w/{}/memory/backend",
        base_url.trim_end_matches('/'),
        workspace_id
    );
    let response = reqwest::blocking::Client::new()
        .post(url)
        .json(&operation)
        .send()?;
    let status = response.status();
    let body = response.text()?;
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

pub fn workspace_http_memory_tools(
    workspace_id: impl Into<String>,
    base_url: impl Into<String>,
) -> Vec<ToolDefinition> {
    let backend = WorkspaceHttpMemoryBackend::new(workspace_id, base_url);
    vec![
        memory_tool(
            "MemoryRead",
            READ_DESCRIPTION,
            read_schema(),
            backend.clone(),
            |input| {
                Ok(MemoryBackendOperation::Read(parse_input::<
                    MemoryReadOperation,
                >(input)?))
            },
        ),
        memory_tool(
            "MemoryWrite",
            WRITE_DESCRIPTION,
            write_schema(),
            backend.clone(),
            |input| {
                Ok(MemoryBackendOperation::Write(parse_input::<
                    MemoryWriteOperation,
                >(input)?))
            },
        ),
        memory_tool(
            "MemoryEdit",
            EDIT_DESCRIPTION,
            edit_schema(),
            backend.clone(),
            |input| {
                Ok(MemoryBackendOperation::Edit(parse_input::<
                    MemoryEditOperation,
                >(input)?))
            },
        ),
        memory_tool(
            "MemoryDelete",
            DELETE_DESCRIPTION,
            delete_schema(),
            backend.clone(),
            |input| {
                Ok(MemoryBackendOperation::Delete(parse_input::<
                    MemoryDeleteOperation,
                >(input)?))
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
        self.backend.execute(operation)
    }
}

fn parse_input<T: DeserializeOwned>(input: &str) -> Result<T, ToolError> {
    serde_json::from_str(input).map_err(|error| ToolError::InvalidArgument(error.to_string()))
}

fn tool_output(output: MemoryToolOutput) -> ToolOutput {
    ToolOutput {
        summary: output.summary,
        content: output.content,
    }
}

const READ_DESCRIPTION: &str = "Read a durable memory record through Workspace authority.";
const WRITE_DESCRIPTION: &str =
    "Create or overwrite a durable memory record through Workspace authority.";
const EDIT_DESCRIPTION: &str =
    "Replace text in a durable memory record through Workspace authority.";
const DELETE_DESCRIPTION: &str = "Delete a durable memory record through Workspace authority.";
const QUERY_DESCRIPTION: &str = "Query durable memory records through Workspace authority.";

fn kind_schema() -> serde_json::Value {
    json!({"type":"string","enum":["summary","decision","request"]})
}

fn read_schema() -> serde_json::Value {
    json!({
        "type":"object",
        "additionalProperties": false,
        "required":["kind"],
        "properties":{
            "kind": kind_schema(),
            "slug":{"type":["string","null"]},
            "offset":{"type":["integer","null"],"minimum":0},
            "limit":{"type":["integer","null"],"minimum":0}
        }
    })
}

fn write_schema() -> serde_json::Value {
    json!({
        "type":"object",
        "additionalProperties": false,
        "required":["kind","content"],
        "properties":{
            "kind": kind_schema(),
            "slug":{"type":["string","null"]},
            "content":{"type":"string"}
        }
    })
}

fn edit_schema() -> serde_json::Value {
    json!({
        "type":"object",
        "additionalProperties": false,
        "required":["kind","old_string","new_string"],
        "properties":{
            "kind": kind_schema(),
            "slug":{"type":["string","null"]},
            "old_string":{"type":"string"},
            "new_string":{"type":"string"},
            "replace_all":{"type":"boolean","default":false}
        }
    })
}

fn delete_schema() -> serde_json::Value {
    json!({
        "type":"object",
        "additionalProperties": false,
        "required":["kind"],
        "properties":{
            "kind": kind_schema(),
            "slug":{"type":["string","null"]}
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
