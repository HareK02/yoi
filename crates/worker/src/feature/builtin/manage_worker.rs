//! Workspace-authority-backed Worker session management tools.

use std::sync::Arc;

use async_trait::async_trait;
use llm_engine::tool::{
    Tool, ToolDefinition, ToolError, ToolExecutionContext, ToolMeta, ToolOutput,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    initial_text: Option<String>,
    #[serde(default)]
    relative_cwd: Option<String>,
}

#[derive(Debug, Serialize)]
struct WorkerSpawnRequest {
    runtime_id: String,
    display_name: String,
    profile: String,
    initial_text: String,
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
                "Spawn a Backend/Runtime Worker session in an existing Workspace Workdir. The Workdir id is authority; filesystem paths and Runtime URLs are not accepted."
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
        _ctx: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let request = match self.operation {
            WorkerOperation::List => {
                parse::<WorkerListInput>(input_json, "WorkerList")?;
                WorkspaceRequest::get(format!("/api/w/{}/workers", self.workspace_id))
            }
            WorkerOperation::Spawn => {
                let input = parse::<WorkerSpawnInput>(input_json, "WorkerSpawn")?;
                let request = WorkerSpawnRequest {
                    runtime_id: authority_id(&input.runtime_id, "runtime_id")?,
                    display_name: input
                        .display_name
                        .filter(|value| !value.trim().is_empty())
                        .unwrap_or_else(|| "Workspace Worker".to_string()),
                    profile: non_empty(input.profile, "profile")?,
                    initial_text: input.initial_text.unwrap_or_default(),
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
    use super::*;

    #[test]
    fn worker_tool_family_is_distinct_from_sub_worker_tools() {
        assert_eq!(
            WorkerOperation::ALL.map(WorkerOperation::tool_name),
            ["WorkerList", "WorkerSpawn", "WorkerStop", "WorkerRestore"]
        );
    }

    #[test]
    fn worker_spawn_request_uses_authority_ids_without_runtime_paths() {
        let request = WorkerSpawnRequest {
            runtime_id: "runtime-1".to_string(),
            display_name: "Coder".to_string(),
            profile: "builtin:coder".to_string(),
            initial_text: "Implement the Ticket".to_string(),
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
