//! Workspace-authority-backed Worker session management tools.

use std::sync::Arc;

use async_trait::async_trait;
use llm_engine::tool::{
    Tool, ToolDefinition, ToolError, ToolExecutionContext, ToolMeta, ToolOutput,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use protocol::Segment;

use crate::feature::{
    FeatureDescriptor, FeatureInstallContext, FeatureInstallError, FeatureModule, ToolContribution,
    ToolDeclaration,
};
use crate::worker::{WorkspaceClient, WorkspaceRequest, WorkspaceRequestMethod};

const FEATURE_ID: &str = "worker";
const FEATURE_NAME: &str = "Worker";
const FEATURE_DESCRIPTION: &str =
    "Workspace-authority tools for managing Workdir-bound Backend/Runtime Worker sessions.";

#[derive(Clone, Debug)]
pub struct ManageWorkerFeature {
    client: Arc<dyn WorkspaceClient>,
}

pub fn manage_worker_feature(client: Arc<dyn WorkspaceClient>) -> ManageWorkerFeature {
    ManageWorkerFeature { client }
}

impl FeatureModule for ManageWorkerFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        let mut descriptor = FeatureDescriptor::builtin(FEATURE_ID, FEATURE_NAME)
            .with_description(FEATURE_DESCRIPTION);
        for operation in WorkerOperation::ALL {
            descriptor = descriptor.with_tool(ToolDeclaration::new(
                operation.tool_name(),
                operation.description(),
            ));
        }
        descriptor
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        let workspace_id = self
            .client
            .workspace_id()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                FeatureInstallError::InvalidDescriptor(
                    "worker feature requires a Workspace id".to_string(),
                )
            })?
            .to_string();
        for operation in WorkerOperation::ALL {
            let definition = match operation {
                WorkerOperation::List => definition::<WorkerListInput>(
                    operation,
                    self.client.clone(),
                    workspace_id.clone(),
                ),
                WorkerOperation::Spawn => definition::<WorkerSpawnInput>(
                    operation,
                    self.client.clone(),
                    workspace_id.clone(),
                ),
                WorkerOperation::Stop => definition::<WorkerStopInput>(
                    operation,
                    self.client.clone(),
                    workspace_id.clone(),
                ),
                WorkerOperation::Restore => definition::<WorkerTargetInput>(
                    operation,
                    self.client.clone(),
                    workspace_id.clone(),
                ),
            };
            context
                .tools()
                .register(ToolContribution::new(operation.tool_name(), definition))?;
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WorkerListInput {}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WorkerSpawnInput {
    runtime_id: String,
    working_directory_id: String,
    profile: String,
    /// Optional queued Ticket to assign atomically to the new Coder Worker.
    #[serde(default)]
    ticket_id: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
    /// Normal typed initial user submission delivered after spawn. An empty
    /// vector starts the Worker without initial input.
    #[serde(default)]
    initial_submit: Vec<Segment>,
    #[serde(default)]
    relative_cwd: Option<String>,
}

#[derive(Debug, Serialize)]
struct WorkerSpawnTicketAssignmentRequest {
    ticket_id: String,
    operation_id: String,
}

#[derive(Debug, Serialize)]
struct WorkerSpawnRequest {
    runtime_id: String,
    display_name: String,
    profile: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    ticket_assignment: Option<WorkerSpawnTicketAssignmentRequest>,
    initial_submit: Vec<Segment>,
    working_directory: WorkerWorkingDirectorySelection,
}

#[derive(Debug, Serialize)]
struct WorkerWorkingDirectorySelection {
    working_directory_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    relative_cwd: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WorkerTargetInput {
    runtime_id: String,
    worker_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WorkerStopInput {
    runtime_id: String,
    worker_id: String,
    #[serde(default)]
    reason: Option<String>,
}

struct WorkspaceWorkerTool {
    operation: WorkerOperation,
    client: Arc<dyn WorkspaceClient>,
    workspace_id: String,
}

#[derive(Debug, Clone, Copy)]
enum WorkerOperation {
    List,
    Spawn,
    Stop,
    Restore,
}

impl WorkerOperation {
    const ALL: [Self; 4] = [Self::List, Self::Spawn, Self::Stop, Self::Restore];

    fn tool_name(self) -> &'static str {
        match self {
            Self::List => "WorkerList",
            Self::Spawn => "WorkerSpawn",
            Self::Stop => "WorkerStop",
            Self::Restore => "WorkerRestore",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::List => {
                "List Backend/Runtime Worker sessions in the current Workspace. SubWorkers are excluded."
            }
            Self::Spawn => {
                "Spawn a Backend/Runtime Worker session in an existing Workspace Workdir. The Workdir id is authority; filesystem paths and Runtime URLs are not accepted. `initial_submit` carries the normal typed user submission. Set `ticket_id` with a Flow segment in `initial_submit` to atomically assign a queued Ticket to the new Coder Worker; the operation id is derived from the durable tool call rather than model input."
            }
            Self::Stop => "Stop a Backend/Runtime Worker session in the current Workspace.",
            Self::Restore => {
                "Restore a stopped Backend/Runtime Worker session in the current Workspace."
            }
        }
    }
}

#[async_trait]
impl Tool for WorkspaceWorkerTool {
    async fn execute(
        &self,
        input_json: &str,
        ctx: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let request = match self.operation {
            WorkerOperation::List => {
                parse::<WorkerListInput>(input_json, "WorkerList")?;
                WorkspaceRequest::get(format!("/api/w/{}/workers", self.workspace_id))
            }
            WorkerOperation::Spawn => {
                let input = parse::<WorkerSpawnInput>(input_json, "WorkerSpawn")?;
                let ticket_assignment = input
                    .ticket_id
                    .map(|ticket_id| {
                        let ticket_id = authority_id(&ticket_id, "ticket_id")?;
                        let call_id = non_empty(ctx.call_id.clone(), "tool call_id")?;
                        Ok::<_, ToolError>(WorkerSpawnTicketAssignmentRequest {
                            operation_id: format!("worker-spawn:{ticket_id}:{call_id}"),
                            ticket_id,
                        })
                    })
                    .transpose()?;
                let request = WorkerSpawnRequest {
                    runtime_id: authority_id(&input.runtime_id, "runtime_id")?,
                    display_name: input
                        .display_name
                        .filter(|value| !value.trim().is_empty())
                        .unwrap_or_else(|| "Workspace Worker".to_string()),
                    profile: non_empty(input.profile, "profile")?,
                    ticket_assignment,
                    initial_submit: input.initial_submit,
                    working_directory: WorkerWorkingDirectorySelection {
                        working_directory_id: authority_id(
                            &input.working_directory_id,
                            "working_directory_id",
                        )?,
                        relative_cwd: input
                            .relative_cwd
                            .map(|value| validate_relative_cwd(&value))
                            .transpose()?,
                    },
                };
                WorkspaceRequest::json(
                    WorkspaceRequestMethod::Post,
                    format!("/api/w/{}/workers", self.workspace_id),
                    serde_json::to_string(&request)
                        .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?,
                )
            }
            WorkerOperation::Stop => {
                let input = parse::<WorkerStopInput>(input_json, "WorkerStop")?;
                let runtime_id = authority_id(&input.runtime_id, "runtime_id")?;
                let worker_id = authority_id(&input.worker_id, "worker_id")?;
                WorkspaceRequest::json(
                    WorkspaceRequestMethod::Post,
                    format!(
                        "/api/w/{}/runtimes/{runtime_id}/workers/{worker_id}/stop",
                        self.workspace_id
                    ),
                    serde_json::json!({ "reason": input.reason }).to_string(),
                )
            }
            WorkerOperation::Restore => {
                let input = parse::<WorkerTargetInput>(input_json, "WorkerRestore")?;
                let runtime_id = authority_id(&input.runtime_id, "runtime_id")?;
                let worker_id = authority_id(&input.worker_id, "worker_id")?;
                WorkspaceRequest::json(
                    WorkspaceRequestMethod::Post,
                    format!(
                        "/api/w/{}/runtimes/{runtime_id}/workers/{worker_id}/restore",
                        self.workspace_id
                    ),
                    "{}",
                )
            }
        };
        let response = self
            .client
            .execute(request)
            .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
        if !response.is_success() {
            return Err(ToolError::ExecutionFailed(format!(
                "Workspace Worker operation returned HTTP {}: {}",
                response.status, response.body
            )));
        }
        Ok(ToolOutput {
            summary: format!("{} completed", self.operation.tool_name()),
            content: Some(response.body),
        })
    }
}

fn definition<I: JsonSchema + 'static>(
    operation: WorkerOperation,
    client: Arc<dyn WorkspaceClient>,
    workspace_id: String,
) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(I);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new(operation.tool_name())
            .description(operation.description())
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(WorkspaceWorkerTool {
            operation,
            client: client.clone(),
            workspace_id: workspace_id.clone(),
        });
        (meta, tool)
    })
}

fn parse<T: for<'de> Deserialize<'de>>(input: &str, tool: &str) -> Result<T, ToolError> {
    serde_json::from_str(input)
        .map_err(|error| ToolError::InvalidArgument(format!("invalid {tool} input: {error}")))
}

fn authority_id(value: &str, field: &str) -> Result<String, ToolError> {
    let value = non_empty(value.to_string(), field)?;
    if value.contains('/') || value.contains('?') || value.contains('#') {
        return Err(ToolError::InvalidArgument(format!(
            "{field} must be an authority id, not a path or URL"
        )));
    }
    Ok(value)
}

fn non_empty(value: String, field: &str) -> Result<String, ToolError> {
    let value = value.trim().to_string();
    if value.is_empty() {
        return Err(ToolError::InvalidArgument(format!(
            "{field} must not be empty"
        )));
    }
    Ok(value)
}

fn validate_relative_cwd(value: &str) -> Result<String, ToolError> {
    let value = value.trim();
    if value.is_empty()
        || value.starts_with('/')
        || value.split('/').any(|part| matches!(part, "" | "." | ".."))
    {
        return Err(ToolError::InvalidArgument(
            "relative_cwd must be a normalized relative path inside the Workdir".to_string(),
        ));
    }
    Ok(value.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::worker::{WorkspaceClientError, WorkspaceResponse};

    #[derive(Debug, Default)]
    struct RecordingWorkspaceClient {
        requests: Mutex<Vec<WorkspaceRequest>>,
    }

    impl WorkspaceClient for RecordingWorkspaceClient {
        fn workspace_id(&self) -> Option<&str> {
            Some("workspace/test")
        }

        fn kind(&self) -> &str {
            "recording"
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
                body: "{}".to_string(),
            })
        }
    }

    #[tokio::test]
    async fn worker_spawn_forwards_typed_initial_submit_to_workspace_api() {
        let client = Arc::new(RecordingWorkspaceClient::default());
        let tool = WorkspaceWorkerTool {
            operation: WorkerOperation::Spawn,
            client: client.clone(),
            workspace_id: "workspace%2Ftest".to_string(),
        };
        tool.execute(
            &serde_json::json!({
                "runtime_id": "runtime-1",
                "working_directory_id": "workdir-1",
                "profile": "builtin:coder",
                "ticket_id": "00001KZ9E0DBS",
                "initial_submit": [
                    { "kind": "flow", "selector": "builtin:coder-review" },
                    { "kind": "text", "content": "Implement Ticket 00001" }
                ]
            })
            .to_string(),
            ToolExecutionContext::new("call-1", "batch-1", 0),
        )
        .await
        .unwrap();

        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, "/api/w/workspace%2Ftest/workers");
        let body: serde_json::Value =
            serde_json::from_str(requests[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(body["initial_submit"][0]["kind"], "flow");
        assert_eq!(
            body["initial_submit"][0]["selector"],
            "builtin:coder-review"
        );
        assert_eq!(body["initial_submit"][1]["kind"], "text");
        assert_eq!(
            body["ticket_assignment"],
            serde_json::json!({
                "ticket_id": "00001KZ9E0DBS",
                "operation_id": "worker-spawn:00001KZ9E0DBS:call-1"
            })
        );
        assert!(body.get("initial_text").is_none());
    }

    #[test]
    fn worker_tool_family_is_distinct_from_sub_worker_tools() {
        assert_eq!(
            WorkerOperation::ALL.map(WorkerOperation::tool_name),
            ["WorkerList", "WorkerSpawn", "WorkerStop", "WorkerRestore"]
        );
    }

    #[test]
    fn worker_spawn_schema_exposes_normal_typed_segment_variants() {
        let schema = serde_json::to_value(schemars::schema_for!(WorkerSpawnInput)).unwrap();
        let text = serde_json::to_string(&schema).unwrap();
        assert!(text.contains("initial_submit"));
        assert!(text.contains("ticket_id"));
        assert!(text.contains("selector"));
        assert!(text.contains("flow"));
        assert!(!text.contains("initial_text"));
    }

    #[test]
    fn worker_spawn_request_uses_authority_ids_without_runtime_paths() {
        let request = WorkerSpawnRequest {
            runtime_id: "runtime-1".to_string(),
            display_name: "Coder".to_string(),
            profile: "builtin:coder".to_string(),
            ticket_assignment: None,
            initial_submit: vec![
                Segment::Flow {
                    selector: "builtin:coder-review".to_string(),
                },
                Segment::text("Implement the Ticket"),
            ],
            working_directory: WorkerWorkingDirectorySelection {
                working_directory_id: "wd-1".to_string(),
                relative_cwd: Some("repo".to_string()),
            },
        };
        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["runtime_id"], "runtime-1");
        assert_eq!(value["working_directory"]["working_directory_id"], "wd-1");
        assert!(value.get("cwd").is_none());
        assert!(value.get("runtime_url").is_none());
        assert!(value["working_directory"].get("mode").is_none());
        assert_eq!(value["initial_submit"][0]["kind"], "flow");
        assert_eq!(
            value["initial_submit"][0]["selector"],
            "builtin:coder-review"
        );
        assert_eq!(value["initial_submit"][1]["kind"], "text");
        assert!(value.get("initial_text").is_none());
    }

    #[test]
    fn worker_inputs_reject_paths_and_parent_traversal() {
        assert!(authority_id("https://runtime.example", "runtime_id").is_err());
        assert!(authority_id("runtime/id", "runtime_id").is_err());
        assert!(validate_relative_cwd("../repo").is_err());
        assert!(validate_relative_cwd("/repo").is_err());
        assert_eq!(validate_relative_cwd("repo/src").unwrap(), "repo/src");
    }
}
