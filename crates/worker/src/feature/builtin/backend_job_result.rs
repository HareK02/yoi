//! Structured completion capability for Backend-owned Job Workers.
//!
//! The tool intentionally accepts only the result value. Workspace and Worker
//! identity come from the authenticated Workspace client, while Job, attempt,
//! and input revision are resolved from the Backend-owned binding.

use std::sync::Arc;

use agen::tool::{Tool, ToolError, ToolExecutionContext, ToolMeta, ToolOutput};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use crate::feature::{
    FeatureDescriptor, FeatureInstallContext, FeatureInstallError, FeatureModule, ToolContribution,
    ToolDeclaration, ToolDefinition,
};
use crate::worker::{WorkspaceClient, WorkspaceRequest, WorkspaceRequestMethod};

const TOOL_NAME: &str = "SubmitBackendJobResult";
const TOOL_DESCRIPTION: &str = "Submit the bounded structured result for this Backend-owned Job attempt. Workspace, Job, attempt, input revision, and source Worker identity are resolved and fenced by Backend authority; normal final prose is not success.";

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SubmitResultInput {
    result: Value,
}

#[derive(Clone)]
struct SubmitResultTool {
    client: Arc<dyn WorkspaceClient>,
}

#[async_trait]
impl Tool for SubmitResultTool {
    async fn execute(
        &self,
        input_json: &str,
        _context: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let input: SubmitResultInput = serde_json::from_str(input_json)
            .map_err(|error| ToolError::InvalidArgument(error.to_string()))?;
        let workspace_id = self.client.workspace_id().ok_or_else(|| {
            ToolError::ExecutionFailed(
                "Backend Job result submission requires Workspace identity".to_string(),
            )
        })?;
        let response = self
            .client
            .execute(WorkspaceRequest::json(
                WorkspaceRequestMethod::Post,
                format!("/api/w/{workspace_id}/workers/self/backend-job-result"),
                serde_json::to_string(&serde_json::json!({ "result": input.result }))
                    .map_err(|error| ToolError::Internal(error.to_string()))?,
            ))
            .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
        if !response.is_success() {
            return Err(ToolError::ExecutionFailed(format!(
                "Backend Job result submission was rejected with HTTP {}: {}",
                response.status, response.body
            )));
        }
        Ok(ToolOutput {
            summary: "Backend Job result accepted".to_string(),
            content: Some(response.body),
            attachments: Vec::new(),
        })
    }
}

fn definition(client: Arc<dyn WorkspaceClient>) -> ToolDefinition {
    let schema = serde_json::to_value(schemars::schema_for!(SubmitResultInput))
        .expect("Backend Job result schema must serialize");
    Arc::new(move || {
        (
            ToolMeta::new(TOOL_NAME)
                .description(TOOL_DESCRIPTION)
                .input_schema(schema.clone()),
            Arc::new(SubmitResultTool {
                client: client.clone(),
            }) as Arc<dyn Tool>,
        )
    })
}

#[derive(Clone)]
pub struct BackendJobResultFeature {
    client: Arc<dyn WorkspaceClient>,
}

impl BackendJobResultFeature {
    pub fn new(client: Arc<dyn WorkspaceClient>) -> Self {
        Self { client }
    }
}

impl FeatureModule for BackendJobResultFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builtin("backend-job-result", "Backend Job result")
            .with_description("Single-purpose structured result capability for Backend-owned Jobs.")
            .with_tool(ToolDeclaration::new(TOOL_NAME, TOOL_DESCRIPTION))
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        context.tools().register(ToolContribution::new(
            TOOL_NAME,
            definition(self.client.clone()),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::FeatureRegistryBuilder;
    use crate::hook::HookRegistryBuilder;
    use crate::worker::{WorkspaceClientError, WorkspaceResponse};
    use std::sync::Mutex;

    #[derive(Debug, Default)]
    struct RecordingClient {
        requests: Mutex<Vec<WorkspaceRequest>>,
    }

    impl WorkspaceClient for RecordingClient {
        fn workspace_id(&self) -> Option<&str> {
            Some("workspace-test")
        }
        fn kind(&self) -> &str {
            "test"
        }
        fn is_available(&self) -> bool {
            true
        }
        fn execute(
            &self,
            request: WorkspaceRequest,
        ) -> Result<WorkspaceResponse, WorkspaceClientError> {
            self.requests.lock().unwrap().push(request);
            Ok(WorkspaceResponse {
                status: 200,
                body: "{\"replayed\":false}".to_string(),
            })
        }
    }

    #[tokio::test]
    async fn feature_exposes_only_identity_free_structured_result_input() {
        let client = Arc::new(RecordingClient::default());
        let mut pending = Vec::new();
        let mut hooks = HookRegistryBuilder::default();
        let report = FeatureRegistryBuilder::new()
            .with_module(BackendJobResultFeature::new(client.clone()))
            .install_into_pending(&mut pending, &mut hooks);
        assert_eq!(report.installed_tool_names(), [TOOL_NAME]);
        let (meta, tool) = pending.remove(0)();
        assert_eq!(meta.input_schema["required"], serde_json::json!(["result"]));
        assert!(meta.input_schema["properties"].get("job_id").is_none());
        tool.execute(
            r#"{"result":{"valid":true}}"#,
            ToolExecutionContext::direct(),
        )
        .await
        .unwrap();
        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, WorkspaceRequestMethod::Post);
        assert!(
            requests[0]
                .path
                .ends_with("/workers/self/backend-job-result")
        );
    }
}
