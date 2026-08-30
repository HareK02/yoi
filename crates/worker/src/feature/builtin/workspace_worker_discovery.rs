//! Privileged, read-only discovery of Workspace-visible Workers.
//!
//! This feature deliberately stays separate from the canonical `WorkerList`
//! control-grant surface. Discovery results carry the typed subject needed by a
//! later control operation, but discovery itself grants no control authority.

use std::sync::Arc;

use agen::tool::{Tool, ToolDefinition, ToolError, ToolExecutionContext, ToolMeta, ToolOutput};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use crate::feature::{
    FeatureDescriptor, FeatureInstallContext, FeatureInstallError, FeatureInstructionContribution,
    FeatureInstructionDeclaration, FeatureInstructionId, FeatureModule, ToolContribution,
    ToolDeclaration,
};
use crate::worker::{WorkspaceClient, WorkspaceWorkerDiscoveryRequest};

const FEATURE_ID: &str = "workspace-worker-discovery";
const TOOL_NAME: &str = "ListWorkspaceWorkers";
const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 100;
const INSTRUCTION_ID: &str = "workspace-worker-discovery.policy";
const PROMPT_REF: &str = "common.workspace_worker_discovery";
const DESCRIPTION: &str = "List or directly find Workspace-visible Workers through Backend authority. Results include each W-key and the typed runtime_worker subject needed by later Worker control calls, but do not grant control authority.";

fn instruction() -> FeatureInstructionDeclaration {
    FeatureInstructionDeclaration::new(
        FeatureInstructionId::builtin(INSTRUCTION_ID),
        PROMPT_REF,
        "Workspace Worker discovery and control-authority separation",
    )
    .expect("static Workspace Worker discovery instruction is valid")
}

#[derive(Clone)]
pub struct WorkspaceWorkerDiscoveryFeature {
    client: Arc<dyn WorkspaceClient>,
}

pub fn workspace_worker_discovery_feature(
    client: Arc<dyn WorkspaceClient>,
) -> WorkspaceWorkerDiscoveryFeature {
    WorkspaceWorkerDiscoveryFeature { client }
}

impl FeatureModule for WorkspaceWorkerDiscoveryFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builtin(FEATURE_ID, "Workspace Worker Discovery")
            .with_description(DESCRIPTION)
            .with_instruction(instruction())
            .with_tool(ToolDeclaration::new(TOOL_NAME, DESCRIPTION))
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        context
            .instructions()
            .register(FeatureInstructionContribution::new(instruction()))?;
        let client = self.client.clone();
        let definition: ToolDefinition = Arc::new(move || {
            (
                ToolMeta::new(TOOL_NAME)
                    .description(DESCRIPTION)
                    .input_schema(input_schema()),
                Arc::new(ListWorkspaceWorkersTool {
                    client: client.clone(),
                }) as Arc<dyn Tool>,
            )
        });
        context
            .tools()
            .register(ToolContribution::new(TOOL_NAME, definition))?;
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListWorkspaceWorkersInput {
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    query: Option<String>,
}

#[derive(Clone)]
struct ListWorkspaceWorkersTool {
    client: Arc<dyn WorkspaceClient>,
}

#[async_trait]
impl Tool for ListWorkspaceWorkersTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let input: ListWorkspaceWorkersInput = serde_json::from_str(input_json)
            .map_err(|error| ToolError::InvalidArgument(error.to_string()))?;
        let limit = input.limit.unwrap_or(DEFAULT_LIMIT);
        if !(1..=MAX_LIMIT).contains(&limit) {
            return Err(ToolError::InvalidArgument(format!(
                "limit must be between 1 and {MAX_LIMIT}"
            )));
        }
        let query = input.query.map(|query| query.trim().to_string());
        if query.as_deref().is_some_and(str::is_empty) {
            return Err(ToolError::InvalidArgument(
                "query must not be empty".to_string(),
            ));
        }
        if query.as_ref().is_some_and(|query| query.len() > 128) {
            return Err(ToolError::InvalidArgument(
                "query must not exceed 128 bytes".to_string(),
            ));
        }

        let page = self
            .client
            .list_workspace_workers(WorkspaceWorkerDiscoveryRequest {
                cursor: input.cursor,
                limit,
                query,
            })
            .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
        let count = page.workers.len();
        Ok(ToolOutput {
            summary: format!("Listed {count} Workspace Worker(s)"),
            content: Some(serde_json::to_string_pretty(&page).map_err(|error| {
                ToolError::ExecutionFailed(format!(
                    "encode Workspace Worker discovery result: {error}"
                ))
            })?),
            attachments: Vec::new(),
        })
    }
}

fn input_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "cursor": {
                "type": "string",
                "description": "Opaque cursor returned by a prior page."
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_LIMIT,
                "default": DEFAULT_LIMIT
            },
            "query": {
                "type": "string",
                "maxLength": 128,
                "description": "Exact W-key or Worker display name lookup."
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use workspace_api::{
        WorkspaceWorkerDiscoveryItem, WorkspaceWorkerDiscoveryPage, WorkspaceWorkerSubject,
    };

    use super::*;
    use crate::worker::{WorkspaceClientError, WorkspaceRequest, WorkspaceResponse};

    #[derive(Debug)]
    struct RecordingClient {
        requests: Mutex<Vec<WorkspaceWorkerDiscoveryRequest>>,
        result: WorkspaceWorkerDiscoveryPage,
        unavailable: bool,
    }

    impl WorkspaceClient for RecordingClient {
        fn workspace_id(&self) -> Option<&str> {
            Some("workspace-1")
        }

        fn kind(&self) -> &str {
            "recording"
        }

        fn is_available(&self) -> bool {
            !self.unavailable
        }

        fn execute(
            &self,
            _request: WorkspaceRequest,
        ) -> Result<WorkspaceResponse, WorkspaceClientError> {
            panic!("discovery must not use generic Workspace request authority")
        }

        fn list_workspace_workers(
            &self,
            request: WorkspaceWorkerDiscoveryRequest,
        ) -> Result<WorkspaceWorkerDiscoveryPage, WorkspaceClientError> {
            self.requests.lock().unwrap().push(request);
            if self.unavailable {
                Err(WorkspaceClientError::Unavailable("denied".to_string()))
            } else {
                Ok(self.result.clone())
            }
        }
    }

    fn page() -> WorkspaceWorkerDiscoveryPage {
        WorkspaceWorkerDiscoveryPage {
            workers: vec![WorkspaceWorkerDiscoveryItem {
                subject: WorkspaceWorkerSubject::RuntimeWorker {
                    runtime_id: "arcadia".to_string(),
                    worker_id: "worker-1".to_string(),
                },
                resource_key: "W-12".to_string(),
                display_name: "coder-one".to_string(),
                profile: Some("builtin:coder".to_string()),
                status: Some("idle".to_string()),
            }],
            next_cursor: Some("v1:1".to_string()),
        }
    }

    #[tokio::test]
    async fn tool_preserves_typed_subject_and_forwards_lookup() {
        let client = Arc::new(RecordingClient {
            requests: Mutex::new(Vec::new()),
            result: page(),
            unavailable: false,
        });
        let tool = ListWorkspaceWorkersTool {
            client: client.clone(),
        };
        let output = tool
            .execute(
                r#"{"query":" W-12 ","limit":1}"#,
                ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        let value: serde_json::Value =
            serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
        assert_eq!(value["workers"][0]["resource_key"], "W-12");
        assert_eq!(value["workers"][0]["subject"]["kind"], "runtime_worker");
        assert_eq!(value["workers"][0]["subject"]["runtime_id"], "arcadia");
        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].query.as_deref(), Some("W-12"));
        assert_eq!(requests[0].limit, 1);
    }

    #[tokio::test]
    async fn missing_backend_authority_fails_closed() {
        let client = Arc::new(RecordingClient {
            requests: Mutex::new(Vec::new()),
            result: page(),
            unavailable: true,
        });
        let error = ListWorkspaceWorkersTool { client }
            .execute("{}", ToolExecutionContext::default())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("denied"));
    }
}
