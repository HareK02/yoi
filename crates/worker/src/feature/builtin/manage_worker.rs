//! Workspace-authority-backed Worker session management tools.

use std::sync::Arc;

use agen::tool::{Tool, ToolDefinition, ToolError, ToolExecutionContext, ToolMeta, ToolOutput};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use protocol::Segment;

use crate::feature::{
    FeatureDescriptor, FeatureInstallContext, FeatureInstallError, FeatureModule,
    ServiceDeclaration, ServiceId, ToolContribution, ToolDeclaration,
};
use crate::spawn::registry::{SpawnedWorkerRegistry, SubWorkerStopSummary};
use crate::worker::{
    WorkspaceClient, WorkspaceClientError, WorkspaceRequest, WorkspaceRequestMethod,
    WorkspaceResponse,
};

const FEATURE_ID: &str = "worker";
const FEATURE_NAME: &str = "Worker";
const FEATURE_DESCRIPTION: &str =
    "Workspace-authority tools for managing Workdir-bound Backend/Runtime Worker sessions.";
pub const WORKER_LIFECYCLE_SERVICE_ID: &str = "worker.lifecycle";
pub const WORKER_CONTROL_SERVICE_ID: &str = "worker.control";
const WORKER_LIFECYCLE_SERVICE_VERSION: &str = "1";

#[async_trait]
pub trait WorkerControlService: Send + Sync {
    fn workspace_id(&self) -> &str;
    fn known_subworkers(&self) -> Vec<serde_json::Value>;
    async fn send_subworker(
        &self,
        name: &str,
        content: String,
    ) -> Result<WorkspaceResponse, WorkspaceClientError>;
    async fn stop_subworker(&self, name: &str) -> Result<WorkspaceResponse, WorkspaceClientError>;
    async fn spawn_worker(
        &self,
        request: WorkerLifecycleSpawnRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError>;
    fn remove_runtime_worker(
        &self,
        runtime_id: &str,
        worker_id: &str,
        reason: &str,
    ) -> Result<WorkspaceResponse, WorkspaceClientError>;
    async fn execute_runtime(
        &self,
        request: WorkspaceRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError>;
    async fn ensure_permission(
        &self,
        subject: &super::worker_observation::WorkerObservationSubjectRef,
        permission: &str,
    ) -> Result<(), WorkspaceClientError>;
}

struct WorkspaceWorkerControlService {
    client: Arc<dyn WorkspaceClient>,
    workspace_id: String,
    registry: Option<Arc<SpawnedWorkerRegistry>>,
}

impl std::fmt::Debug for WorkspaceWorkerControlService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkspaceWorkerControlService")
            .field("workspace_id", &self.workspace_id)
            .field("has_subworker_registry", &self.registry.is_some())
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl WorkerControlService for WorkspaceWorkerControlService {
    fn workspace_id(&self) -> &str {
        &self.workspace_id
    }

    fn known_subworkers(&self) -> Vec<serde_json::Value> {
        self.registry
            .as_ref()
            .map(|registry| {
                registry
                    .list_internal()
                    .into_iter()
                    .map(|internal| {
                        serde_json::json!({
                            "subject": { "kind": "sub_worker", "name": internal.worker_name },
                            "relation": "direct_child",
                            "origin": "sub_worker_spawn",
                            "permissions": ["send_input", "stop", "observe"],
                            "summary": {
                                "display_name": internal.worker_name,
                                "status": format!("{:?}", internal.session.status()).to_lowercase(),
                            }
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    async fn send_subworker(
        &self,
        name: &str,
        content: String,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        let record = self
            .registry
            .as_ref()
            .and_then(|registry| registry.get_internal(name))
            .ok_or_else(|| {
                WorkspaceClientError::Request(
                    "unknown Worker or permission not granted".to_string(),
                )
            })?;
        record
            .session
            .send(content)
            .await
            .map_err(|error| WorkspaceClientError::Request(error.to_string()))?;
        Ok(WorkspaceResponse {
            status: 200,
            body: serde_json::json!({ "subject": { "kind": "sub_worker", "name": name } })
                .to_string(),
        })
    }

    async fn stop_subworker(&self, name: &str) -> Result<WorkspaceResponse, WorkspaceClientError> {
        let registry = self.registry.as_ref().ok_or_else(|| {
            WorkspaceClientError::Request("unknown Worker or permission not granted".to_string())
        })?;
        let summary = registry
            .remove_internal(name)
            .await
            .map_err(|error| WorkspaceClientError::Request(error.to_string()))?
            .ok_or_else(|| {
                WorkspaceClientError::Request(
                    "unknown Worker or permission not granted".to_string(),
                )
            })?;
        Ok(WorkspaceResponse {
            status: 200,
            body: serde_json::to_string(&summary)
                .map_err(|error| WorkspaceClientError::Request(error.to_string()))?,
        })
    }

    async fn spawn_worker(
        &self,
        request: WorkerLifecycleSpawnRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        WorkspaceWorkerLifecycleService {
            client: self.client.clone(),
            workspace_id: self.workspace_id.clone(),
        }
        .spawn(request)
        .await
    }

    fn remove_runtime_worker(
        &self,
        runtime_id: &str,
        worker_id: &str,
        reason: &str,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        self.client
            .execute_worker_remove(runtime_id, worker_id, reason)
    }

    async fn execute_runtime(
        &self,
        request: WorkspaceRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        self.client.execute(request)
    }

    async fn ensure_permission(
        &self,
        subject: &super::worker_observation::WorkerObservationSubjectRef,
        permission: &str,
    ) -> Result<(), WorkspaceClientError> {
        match subject {
            super::worker_observation::WorkerObservationSubjectRef::SubWorker { name } => {
                let known = self
                    .registry
                    .as_ref()
                    .and_then(|registry| registry.get_internal(name))
                    .is_some();
                if known && matches!(permission, "send_input" | "stop" | "observe") {
                    Ok(())
                } else {
                    Err(WorkspaceClientError::Request(
                        "unknown Worker or permission not granted".to_string(),
                    ))
                }
            }
            super::worker_observation::WorkerObservationSubjectRef::RuntimeWorker {
                runtime_id,
                worker_id,
            } => {
                let response = self.client.execute(WorkspaceRequest::get(format!(
                    "/api/w/{}/worker-control/workers",
                    self.workspace_id
                )))?;
                if !response.is_success() {
                    return Err(WorkspaceClientError::Request(format!(
                        "Workspace control request returned {}: {}",
                        response.status, response.body
                    )));
                }
                let body: serde_json::Value =
                    serde_json::from_str(&response.body).map_err(|error| {
                        WorkspaceClientError::Request(format!(
                            "invalid Workspace control response: {error}"
                        ))
                    })?;
                let granted = body
                    .get("items")
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
                    .any(|item| {
                        item.get("subject")
                            .and_then(|value| value.get("runtime_id"))
                            == Some(&serde_json::Value::String(runtime_id.clone()))
                            && item.get("subject").and_then(|value| value.get("worker_id"))
                                == Some(&serde_json::Value::String(worker_id.clone()))
                            && item
                                .get("permissions")
                                .and_then(serde_json::Value::as_array)
                                .is_some_and(|permissions| {
                                    permissions.iter().any(|candidate| candidate == permission)
                                })
                    });
                if granted {
                    Ok(())
                } else {
                    Err(WorkspaceClientError::Request(
                        "unknown Worker or permission not granted".to_string(),
                    ))
                }
            }
        }
    }
}

#[async_trait]
pub trait WorkerLifecycleService: Send + Sync {
    async fn spawn(
        &self,
        request: WorkerLifecycleSpawnRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError>;
}

#[derive(Debug, Clone)]
pub struct WorkerLifecycleSpawnRequest {
    pub runtime_id: String,
    pub working_directory_id: String,
    pub relative_cwd: Option<String>,
    pub profile: String,
    pub ticket_id: Option<String>,
    pub operation_id: Option<String>,
    pub display_name: String,
    pub initial_submit: Vec<Segment>,
}

struct WorkspaceWorkerLifecycleService {
    client: Arc<dyn WorkspaceClient>,
    workspace_id: String,
}

#[async_trait]
impl WorkerLifecycleService for WorkspaceWorkerLifecycleService {
    async fn spawn(
        &self,
        request: WorkerLifecycleSpawnRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        let ticket_assignment = match (request.ticket_id, request.operation_id) {
            (Some(ticket_id), Some(operation_id)) => Some(WorkerSpawnTicketAssignmentRequest {
                ticket_id,
                operation_id,
            }),
            (None, None) => None,
            _ => {
                return Err(WorkspaceClientError::Request(
                    "ticket_id and operation_id must be provided together".to_string(),
                ));
            }
        };
        let control_operation_id = ticket_assignment
            .as_ref()
            .map(|assignment| assignment.operation_id.clone())
            .unwrap_or_else(|| format!("worker-spawn-{}", Uuid::now_v7()));
        let body = WorkerSpawnRequest {
            runtime_id: request.runtime_id,
            display_name: request.display_name,
            profile: request.profile,
            control_operation_id,
            ticket_assignment,
            initial_submit: request.initial_submit,
            working_directory: WorkerWorkingDirectorySelection {
                working_directory_id: request.working_directory_id,
                relative_cwd: request.relative_cwd,
            },
        };
        self.client.execute(WorkspaceRequest::json(
            WorkspaceRequestMethod::Post,
            format!("/api/w/{}/worker-control/workers", self.workspace_id),
            serde_json::to_string(&body)
                .map_err(|error| WorkspaceClientError::Request(error.to_string()))?,
        ))
    }
}

#[derive(Clone)]
pub struct ManageWorkerFeature {
    client: Arc<dyn WorkspaceClient>,
    control: Arc<dyn WorkerControlService>,
    direct_spawn: bool,
}

impl std::fmt::Debug for ManageWorkerFeature {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ManageWorkerFeature")
            .field(
                "has_subworker_registry",
                &!self.control.known_subworkers().is_empty(),
            )
            .field("direct_spawn", &self.direct_spawn)
            .finish_non_exhaustive()
    }
}

pub fn manage_worker_feature(
    client: Arc<dyn WorkspaceClient>,
    registry: Option<Arc<SpawnedWorkerRegistry>>,
    direct_spawn: bool,
) -> ManageWorkerFeature {
    let workspace_id = client.workspace_id().unwrap_or_default().to_string();
    let control: Arc<dyn WorkerControlService> = Arc::new(WorkspaceWorkerControlService {
        client: client.clone(),
        workspace_id,
        registry,
    });
    ManageWorkerFeature {
        client,
        control,
        direct_spawn,
    }
}

#[derive(Clone)]
pub struct SubWorkerControlFeature {
    control: Arc<dyn WorkerControlService>,
}

impl std::fmt::Debug for SubWorkerControlFeature {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SubWorkerControlFeature")
            .finish_non_exhaustive()
    }
}

pub fn sub_worker_control_feature(
    client: Arc<dyn WorkspaceClient>,
    registry: Arc<SpawnedWorkerRegistry>,
) -> SubWorkerControlFeature {
    let workspace_id = client.workspace_id().unwrap_or_default().to_string();
    SubWorkerControlFeature {
        control: Arc::new(WorkspaceWorkerControlService {
            client,
            workspace_id,
            registry: Some(registry),
        }),
    }
}

impl FeatureModule for SubWorkerControlFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builtin("sub_worker", "SubWorker")
            .with_description("Parent-owned SubWorker control authority.")
            .with_provided_service(ServiceDeclaration::new(
                ServiceId::builtin(WORKER_CONTROL_SERVICE_ID),
                WORKER_LIFECYCLE_SERVICE_VERSION,
                "Known-SubWorker discovery and permission-fenced control operations",
            ))
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        context.services().provide(
            ServiceDeclaration::new(
                ServiceId::builtin(WORKER_CONTROL_SERVICE_ID),
                WORKER_LIFECYCLE_SERVICE_VERSION,
                "Known-SubWorker discovery and permission-fenced control operations",
            ),
            self.control.clone(),
        )?;
        Ok(())
    }
}

impl FeatureModule for ManageWorkerFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        let mut descriptor = FeatureDescriptor::builtin(FEATURE_ID, FEATURE_NAME)
            .with_description(FEATURE_DESCRIPTION)
            .with_provided_service(ServiceDeclaration::new(
                ServiceId::builtin(WORKER_LIFECYCLE_SERVICE_ID),
                WORKER_LIFECYCLE_SERVICE_VERSION,
                "Workspace-authoritative Worker lifecycle operations",
            ))
            .with_provided_service(ServiceDeclaration::new(
                ServiceId::builtin(WORKER_CONTROL_SERVICE_ID),
                WORKER_LIFECYCLE_SERVICE_VERSION,
                "Known-Worker discovery and permission-fenced control operations",
            ));
        for operation in WorkerOperation::ALL {
            if operation != WorkerOperation::Spawn || self.direct_spawn {
                descriptor = descriptor.with_tool(ToolDeclaration::new(
                    operation.tool_name(),
                    operation.description(),
                ));
            }
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
        let lifecycle: Arc<dyn WorkerLifecycleService> =
            Arc::new(WorkspaceWorkerLifecycleService {
                client: self.client.clone(),
                workspace_id: workspace_id.clone(),
            });
        context.services().provide(
            ServiceDeclaration::new(
                ServiceId::builtin(WORKER_LIFECYCLE_SERVICE_ID),
                WORKER_LIFECYCLE_SERVICE_VERSION,
                "Workspace-authoritative Worker lifecycle operations",
            ),
            lifecycle,
        )?;
        context.services().provide(
            ServiceDeclaration::new(
                ServiceId::builtin(WORKER_CONTROL_SERVICE_ID),
                WORKER_LIFECYCLE_SERVICE_VERSION,
                "Known-Worker discovery and permission-fenced control operations",
            ),
            self.control.clone(),
        )?;
        for operation in WorkerOperation::ALL {
            if operation == WorkerOperation::Spawn && !self.direct_spawn {
                continue;
            }
            let definition = match operation {
                WorkerOperation::List => {
                    definition::<WorkerListInput>(operation, self.control.clone())
                }
                WorkerOperation::Spawn => {
                    definition::<WorkerSpawnInput>(operation, self.control.clone())
                }
                WorkerOperation::SendInput | WorkerOperation::Notify => {
                    definition::<WorkerMessageInput>(operation, self.control.clone())
                }
                WorkerOperation::Cancel | WorkerOperation::Stop => {
                    definition::<WorkerStopInput>(operation, self.control.clone())
                }
                WorkerOperation::Restore => {
                    definition::<WorkerTargetInput>(operation, self.control.clone())
                }
                WorkerOperation::Remove => {
                    definition::<WorkerRemoveInput>(operation, self.control.clone())
                }
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
    /// Optional inprogress Ticket already accepted by the Orchestrator. Set
    /// this with one Flow segment to assign the new Coder atomically.
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
    control_operation_id: String,
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

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum WorkerSubjectInput {
    RuntimeWorker {
        runtime_id: String,
        worker_id: String,
    },
    SubWorker {
        name: String,
    },
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WorkerTargetInput {
    subject: WorkerSubjectInput,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WorkerMessageInput {
    subject: WorkerSubjectInput,
    content: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WorkerStopInput {
    subject: WorkerSubjectInput,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WorkerRemoveInput {
    subject: WorkerSubjectInput,
    reason: String,
}

struct WorkspaceWorkerTool {
    operation: WorkerOperation,
    control: Arc<dyn WorkerControlService>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkerOperation {
    List,
    Spawn,
    SendInput,
    Notify,
    Cancel,
    Stop,
    Restore,
    Remove,
}

impl WorkerOperation {
    const ALL: [Self; 8] = [
        Self::List,
        Self::Spawn,
        Self::SendInput,
        Self::Notify,
        Self::Cancel,
        Self::Stop,
        Self::Restore,
        Self::Remove,
    ];

    fn tool_name(self) -> &'static str {
        match self {
            Self::List => "WorkerList",
            Self::Spawn => "WorkerSpawn",
            Self::SendInput => "WorkerSendInput",
            Self::Notify => "WorkerNotify",
            Self::Cancel => "WorkerCancel",
            Self::Stop => "WorkerStop",
            Self::Restore => "WorkerRestore",
            Self::Remove => "WorkerRemove",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::List => {
                "List only known Runtime Workers and direct SubWorkers granted to the current Worker."
            }
            Self::Spawn => {
                "Spawn a Backend/Runtime Worker session in an existing Workspace Workdir. The Workdir id is authority; filesystem paths and Runtime URLs are not accepted. `initial_submit` carries the normal typed user submission. After the Orchestrator has committed a Ticket to `inprogress`, set `ticket_id` with a Flow segment in `initial_submit` to atomically assign the new Coder Worker; the operation id is derived from the durable tool call rather than model input."
            }
            Self::SendInput => "Send user input to a known Runtime Worker when allowed.",
            Self::Notify => "Send an advisory notification to a known Runtime Worker when allowed.",
            Self::Cancel => "Cancel the current turn of a known Runtime Worker when allowed.",
            Self::Stop => "Stop a known Runtime Worker when allowed.",
            Self::Restore => {
                "Restore a stopped Backend/Runtime Worker session in the current Workspace."
            }
            Self::Remove => {
                "Remove an eligible stopped, unassigned, non-internal Worker. Supply a bounded reason; Backend validation and retention are authoritative."
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
        let response = match self.operation {
            WorkerOperation::List => {
                parse::<WorkerListInput>(input_json, "WorkerList")?;
                let response = self
                    .control
                    .execute_runtime(WorkspaceRequest::get(format!(
                        "/api/w/{}/worker-control/workers",
                        self.control.workspace_id()
                    )))
                    .await
                    .map_err(control_tool_error)?;
                self.with_subworkers(response)?
            }
            WorkerOperation::Spawn => {
                let input = parse::<WorkerSpawnInput>(input_json, "WorkerSpawn")?;
                let ticket_id = input
                    .ticket_id
                    .map(|ticket_id| authority_id(&ticket_id, "ticket_id"))
                    .transpose()?;
                let operation_id = ticket_id
                    .as_ref()
                    .map(|ticket_id| {
                        let call_id = non_empty(ctx.call_id.clone(), "tool call_id")?;
                        Ok::<_, ToolError>(format!("worker-spawn:{ticket_id}:{call_id}"))
                    })
                    .transpose()?;
                self.control
                    .spawn_worker(WorkerLifecycleSpawnRequest {
                        runtime_id: authority_id(&input.runtime_id, "runtime_id")?,
                        working_directory_id: authority_id(
                            &input.working_directory_id,
                            "working_directory_id",
                        )?,
                        relative_cwd: input
                            .relative_cwd
                            .map(|value| validate_relative_cwd(&value))
                            .transpose()?,
                        profile: non_empty(input.profile, "profile")?,
                        ticket_id,
                        operation_id,
                        display_name: input
                            .display_name
                            .filter(|value| !value.trim().is_empty())
                            .unwrap_or_else(|| "Workspace Worker".to_string()),
                        initial_submit: input.initial_submit,
                    })
                    .await
                    .map_err(control_tool_error)?
            }
            WorkerOperation::SendInput | WorkerOperation::Notify => {
                let input = parse::<WorkerMessageInput>(input_json, self.operation.tool_name())?;
                let content = non_empty(input.content, "content")?;
                if content.len() > 16 * 1024 {
                    return Err(ToolError::ExecutionFailed(
                        "content must contain at most 16384 bytes".to_string(),
                    ));
                }
                match input.subject {
                    WorkerSubjectInput::SubWorker { name } => {
                        if self.operation != WorkerOperation::SendInput {
                            return Err(unsupported_subject(self.operation, "sub_worker"));
                        }
                        let subject = subworker_subject(&name)?;
                        self.control
                            .ensure_permission(&subject, "send_input")
                            .await
                            .map_err(control_tool_error)?;
                        self.control
                            .send_subworker(&name, content)
                            .await
                            .map_err(control_tool_error)?
                    }
                    subject @ WorkerSubjectInput::RuntimeWorker { .. } => {
                        let (runtime_id, worker_id) =
                            runtime_subject_ids(&subject, self.operation)?;
                        self.control.execute_runtime(WorkspaceRequest::json(
                            WorkspaceRequestMethod::Post,
                            format!("/api/w/{}/worker-control/workers/{runtime_id}/{worker_id}/input", self.control.workspace_id()),
                            serde_json::json!({
                                "kind": if self.operation == WorkerOperation::Notify { "notify" } else { "user" },
                                "content": content,
                            }).to_string(),
                        )).await.map_err(control_tool_error)?
                    }
                }
            }
            WorkerOperation::Cancel | WorkerOperation::Stop => {
                let input = parse::<WorkerStopInput>(input_json, self.operation.tool_name())?;
                match input.subject {
                    WorkerSubjectInput::SubWorker { name } => {
                        if self.operation != WorkerOperation::Stop {
                            return Err(unsupported_subject(self.operation, "sub_worker"));
                        }
                        let subject = subworker_subject(&name)?;
                        self.control
                            .ensure_permission(&subject, "stop")
                            .await
                            .map_err(control_tool_error)?;
                        self.control
                            .stop_subworker(&name)
                            .await
                            .map_err(control_tool_error)?
                    }
                    subject @ WorkerSubjectInput::RuntimeWorker { .. } => {
                        let (runtime_id, worker_id) =
                            runtime_subject_ids(&subject, self.operation)?;
                        let action = if self.operation == WorkerOperation::Cancel {
                            "cancel"
                        } else {
                            "stop"
                        };
                        self.control.execute_runtime(WorkspaceRequest::json(
                            WorkspaceRequestMethod::Post,
                            format!("/api/w/{}/worker-control/workers/{runtime_id}/{worker_id}/{action}", self.control.workspace_id()),
                            serde_json::json!({ "reason": input.reason }).to_string(),
                        )).await.map_err(control_tool_error)?
                    }
                }
            }
            WorkerOperation::Restore => {
                let input = parse::<WorkerTargetInput>(input_json, "WorkerRestore")?;
                let (runtime_id, worker_id) = runtime_subject_ids(&input.subject, self.operation)?;
                self.control
                    .execute_runtime(WorkspaceRequest::json(
                        WorkspaceRequestMethod::Post,
                        format!(
                            "/api/w/{}/worker-control/workers/{runtime_id}/{worker_id}/restore",
                            self.control.workspace_id()
                        ),
                        "{}",
                    ))
                    .await
                    .map_err(control_tool_error)?
            }
            WorkerOperation::Remove => {
                let input = parse::<WorkerRemoveInput>(input_json, "WorkerRemove")?;
                let (runtime_id, worker_id) = runtime_subject_ids(&input.subject, self.operation)?;
                let reason = non_empty(input.reason, "reason")?;
                if reason.len() > 512 {
                    return Err(ToolError::ExecutionFailed(
                        "reason must contain at most 512 bytes".to_string(),
                    ));
                }
                self.control
                    .remove_runtime_worker(&runtime_id, &worker_id, &reason)
                    .map_err(control_tool_error)?
            }
        };
        tool_output(self.operation, response)
    }
}

impl WorkspaceWorkerTool {
    fn with_subworkers(
        &self,
        mut response: WorkspaceResponse,
    ) -> Result<WorkspaceResponse, ToolError> {
        if !response.is_success() {
            return Ok(response);
        }
        let mut body: serde_json::Value =
            serde_json::from_str(&response.body).map_err(|error| {
                ToolError::ExecutionFailed(format!("WorkerList returned invalid JSON: {error}"))
            })?;
        let items = body
            .get_mut("items")
            .and_then(serde_json::Value::as_array_mut)
            .ok_or_else(|| {
                ToolError::ExecutionFailed(
                    "WorkerList response did not contain an items array".to_string(),
                )
            })?;
        items.extend(self.control.known_subworkers());
        response.body = serde_json::to_string(&body).map_err(|error| {
            ToolError::ExecutionFailed(format!("WorkerList could not encode its response: {error}"))
        })?;
        Ok(response)
    }
}

fn runtime_subject_ids(
    subject: &WorkerSubjectInput,
    operation: WorkerOperation,
) -> Result<(String, String), ToolError> {
    match subject {
        WorkerSubjectInput::RuntimeWorker {
            runtime_id,
            worker_id,
        } => Ok((
            authority_id(runtime_id, "runtime_id")?,
            authority_id(worker_id, "worker_id")?,
        )),
        WorkerSubjectInput::SubWorker { .. } => Err(unsupported_subject(operation, "sub_worker")),
    }
}

fn subworker_subject(
    name: &str,
) -> Result<super::worker_observation::WorkerObservationSubjectRef, ToolError> {
    Ok(
        super::worker_observation::WorkerObservationSubjectRef::SubWorker {
            name: authority_id(name, "name")?,
        },
    )
}

fn unsupported_subject(operation: WorkerOperation, kind: &str) -> ToolError {
    ToolError::InvalidArgument(format!(
        "{} does not support subject kind '{kind}'",
        operation.tool_name()
    ))
}

fn control_tool_error(error: WorkspaceClientError) -> ToolError {
    ToolError::ExecutionFailed(error.to_string())
}

fn tool_output(
    operation: WorkerOperation,
    response: WorkspaceResponse,
) -> Result<ToolOutput, ToolError> {
    if !response.is_success() {
        return Err(ToolError::ExecutionFailed(format!(
            "Workspace Worker operation returned HTTP {}: {}",
            response.status, response.body
        )));
    }
    if operation == WorkerOperation::Stop
        && let Ok(summary) = serde_json::from_str::<SubWorkerStopSummary>(&response.body)
    {
        return Ok(ToolOutput {
            summary: render_subworker_stop_summary(&summary),
            content: Some(response.body),
            attachments: Vec::new(),
        });
    }
    Ok(ToolOutput {
        summary: format!("{} completed", operation.tool_name()),
        content: Some(response.body),
        attachments: Vec::new(),
    })
}

fn render_subworker_stop_summary(summary: &SubWorkerStopSummary) -> String {
    let tools = if summary.tool_counts.is_empty() {
        "No tool calls".to_string()
    } else {
        summary
            .tool_counts
            .iter()
            .map(|tool| format!("{} {}", tool.count, tool.name))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let elapsed = format_elapsed(summary.elapsed_ms);
    let changes = summary
        .change_stat
        .as_ref()
        .map(|stat| format!("+{}/-{} Changes · ", stat.added, stat.deleted))
        .unwrap_or_default();
    format!("SubWorkerStop - done\n  {tools}\n  {changes}{elapsed}",)
}

fn format_elapsed(elapsed_ms: u64) -> String {
    let seconds = elapsed_ms / 1_000;
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    }
}

fn definition<I: JsonSchema + 'static>(
    operation: WorkerOperation,
    control: Arc<dyn WorkerControlService>,
) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(I);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new(operation.tool_name())
            .description(operation.description())
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(WorkspaceWorkerTool {
            operation,
            control: control.clone(),
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
        removals: Mutex<Vec<(String, String, String)>>,
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

        fn execute_worker_remove(
            &self,
            target_runtime_id: &str,
            target_worker_id: &str,
            reason: &str,
        ) -> Result<WorkspaceResponse, WorkspaceClientError> {
            self.removals.lock().unwrap().push((
                target_runtime_id.to_string(),
                target_worker_id.to_string(),
                reason.to_string(),
            ));
            Ok(WorkspaceResponse {
                status: 200,
                body: r#"{"removed":true}"#.to_string(),
            })
        }
    }

    fn test_control(client: Arc<RecordingWorkspaceClient>) -> Arc<dyn WorkerControlService> {
        Arc::new(WorkspaceWorkerControlService {
            client,
            workspace_id: "workspace%2Ftest".to_string(),
            registry: None,
        })
    }

    #[tokio::test]
    async fn worker_spawn_forwards_typed_initial_submit_to_workspace_api() {
        let client = Arc::new(RecordingWorkspaceClient::default());
        let tool = WorkspaceWorkerTool {
            operation: WorkerOperation::Spawn,
            control: test_control(client.clone()),
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
        assert_eq!(
            requests[0].path,
            "/api/w/workspace%2Ftest/worker-control/workers"
        );
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
    fn worker_service_can_remain_enabled_without_direct_spawn_surface() {
        let client = Arc::new(RecordingWorkspaceClient::default());
        let descriptor = manage_worker_feature(client, None, false).descriptor();
        let tools: Vec<_> = descriptor
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();
        assert!(!tools.contains(&"WorkerSpawn"));
        assert!(tools.contains(&"WorkerList"));
        assert_eq!(
            descriptor.provides_services[1].id,
            ServiceId::builtin(WORKER_CONTROL_SERVICE_ID)
        );
    }

    #[test]
    fn worker_tool_family_is_distinct_from_sub_worker_tools() {
        assert_eq!(
            WorkerOperation::ALL.map(WorkerOperation::tool_name),
            [
                "WorkerList",
                "WorkerSpawn",
                "WorkerSendInput",
                "WorkerNotify",
                "WorkerCancel",
                "WorkerStop",
                "WorkerRestore",
                "WorkerRemove",
            ]
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
            control_operation_id: "spawn-operation-1".to_string(),
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

    #[tokio::test]
    async fn worker_message_and_cancel_use_permission_fenced_control_routes() {
        let client = Arc::new(RecordingWorkspaceClient::default());
        for (operation, args, expected_suffix, expected_kind) in [
            (
                WorkerOperation::SendInput,
                serde_json::json!({
                    "subject": {
                        "kind": "runtime_worker",
                        "runtime_id": "runtime-1",
                        "worker_id": "worker-7",
                    },
                    "content": "continue",
                }),
                "/input",
                Some("user"),
            ),
            (
                WorkerOperation::Notify,
                serde_json::json!({
                    "subject": {
                        "kind": "runtime_worker",
                        "runtime_id": "runtime-1",
                        "worker_id": "worker-7",
                    },
                    "content": "review ready",
                }),
                "/input",
                Some("notify"),
            ),
            (
                WorkerOperation::Cancel,
                serde_json::json!({
                    "subject": {
                        "kind": "runtime_worker",
                        "runtime_id": "runtime-1",
                        "worker_id": "worker-7",
                    },
                    "reason": "superseded",
                }),
                "/cancel",
                None,
            ),
        ] {
            WorkspaceWorkerTool {
                operation,
                control: test_control(client.clone()),
            }
            .execute(
                &args.to_string(),
                ToolExecutionContext::new("call-control", "batch-control", 0),
            )
            .await
            .unwrap();
            let request = client.requests.lock().unwrap().last().cloned().unwrap();
            assert!(request.path.ends_with(expected_suffix));
            assert!(request.path.contains("/worker-control/workers/"));
            if let Some(expected_kind) = expected_kind {
                let body: serde_json::Value =
                    serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
                assert_eq!(body["kind"], expected_kind);
            }
        }
    }

    #[tokio::test]
    async fn worker_remove_forwards_only_target_and_bounded_reason() {
        let client = Arc::new(RecordingWorkspaceClient::default());
        let tool = WorkspaceWorkerTool {
            operation: WorkerOperation::Remove,
            control: test_control(client.clone()),
        };
        tool.execute(
            &serde_json::json!({
                "subject": {
                    "kind": "runtime_worker",
                    "runtime_id": "runtime-1",
                    "worker_id": "worker-7",
                },
                "reason": "  retire completed Worker  "
            })
            .to_string(),
            ToolExecutionContext::new("call-remove", "batch-remove", 0),
        )
        .await
        .unwrap();
        assert_eq!(
            client.removals.lock().unwrap().as_slice(),
            [(
                "runtime-1".to_string(),
                "worker-7".to_string(),
                "retire completed Worker".to_string(),
            )]
        );

        let schema = serde_json::to_value(schemars::schema_for!(WorkerRemoveInput))
            .unwrap()
            .to_string();
        for field in ["runtime_id", "worker_id", "reason"] {
            assert!(schema.contains(field));
        }
        for forbidden in [
            "expected_worker_revision",
            "proof",
            "actor",
            "workspace_id",
            "policy",
            "plan",
            "stage",
        ] {
            assert!(!schema.contains(forbidden), "schema leaked {forbidden}");
        }
    }

    #[tokio::test]
    async fn worker_remove_rejects_empty_oversized_and_unknown_authority_input() {
        let client = Arc::new(RecordingWorkspaceClient::default());
        let tool = WorkspaceWorkerTool {
            operation: WorkerOperation::Remove,
            control: test_control(client.clone()),
        };
        for reason in ["   ".to_string(), "x".repeat(513)] {
            let _error = tool
                .execute(
                    &serde_json::json!({
                        "subject": {
                            "kind": "runtime_worker",
                            "runtime_id": "runtime-1",
                            "worker_id": "worker-7",
                        },
                        "reason": reason,
                    })
                    .to_string(),
                    ToolExecutionContext::new("call-invalid", "batch-remove", 0),
                )
                .await
                .unwrap_err();
        }
        let _error = tool
            .execute(
                &serde_json::json!({
                    "subject": {
                        "kind": "runtime_worker",
                        "runtime_id": "runtime-1",
                        "worker_id": "worker-7",
                    },
                    "expected_worker_revision": "revision-1",
                    "reason": "retire",
                    "source_proof": "caller-controlled"
                })
                .to_string(),
                ToolExecutionContext::new("call-spoof", "batch-remove", 0),
            )
            .await
            .unwrap_err();
        assert!(client.removals.lock().unwrap().is_empty());
    }

    #[test]
    fn subworker_stop_output_is_compact_and_keeps_typed_evidence() {
        let summary = SubWorkerStopSummary {
            session_id: "session-1".to_string(),
            display_name: "research".to_string(),
            outcome: crate::spawn::registry::SubWorkerFinalOutcome::Done,
            elapsed_ms: 78_000,
            tool_counts: vec![
                crate::spawn::registry::SubWorkerToolCount {
                    name: "Read".to_string(),
                    count: 26,
                },
                crate::spawn::registry::SubWorkerToolCount {
                    name: "Grep".to_string(),
                    count: 5,
                },
            ],
            change_stat: Some(crate::spawn::registry::SubWorkerChangeStat {
                added: 215,
                deleted: 148,
                source: "tracked_write_edit_tools".to_string(),
            }),
        };
        let response = WorkspaceResponse {
            status: 200,
            body: serde_json::to_string(&summary).unwrap(),
        };

        let output = tool_output(WorkerOperation::Stop, response).unwrap();

        assert_eq!(
            output.summary,
            "SubWorkerStop - done\n  26 Read, 5 Grep\n  +215/-148 Changes · 1m 18s"
        );
        assert_eq!(
            serde_json::from_str::<SubWorkerStopSummary>(output.content.as_deref().unwrap())
                .unwrap(),
            summary
        );
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
