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

    fn execute(&self, operation: MemoryBackendOperation) -> Result<ToolOutput, ToolError> {
        let url = format!(
            "{}/api/w/{}/memory/backend",
            self.base_url.trim_end_matches('/'),
            self.workspace_id
        );
        let response = reqwest::blocking::Client::new()
            .post(url)
            .json(&operation)
            .send()
            .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
        let status = response.status();
        let body = response
            .text()
            .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
        if !status.is_success() {
            return Err(ToolError::ExecutionFailed(format!(
                "workspace memory backend returned HTTP {status}: {body}"
            )));
        }
        let response: MemoryBackendHttpResponse = serde_json::from_str(&body).map_err(|error| {
            ToolError::ExecutionFailed(format!("decode memory backend response: {error}"))
        })?;
        match response {
            MemoryBackendHttpResponse::Ok {
                result: MemoryBackendOperationResult::ToolOutput(output),
            } => Ok(tool_output(output)),
            MemoryBackendHttpResponse::Ok { result } => Err(ToolError::ExecutionFailed(format!(
                "unexpected memory backend result for model-visible tool: {result:?}"
            ))),
            MemoryBackendHttpResponse::Error { message } => {
                Err(ToolError::ExecutionFailed(message))
            }
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
