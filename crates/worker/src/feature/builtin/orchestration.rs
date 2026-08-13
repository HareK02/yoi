//! Semantic Ticket orchestration tools backed by Feature Services.

use std::sync::Arc;

use async_trait::async_trait;
use llm_engine::tool::{
    Tool, ToolDefinition, ToolError, ToolExecutionContext, ToolMeta, ToolOutput,
};
use protocol::Segment;
use schemars::JsonSchema;
use serde::Deserialize;

use super::manage_worker::{
    WORKER_LIFECYCLE_SERVICE_ID, WorkerLifecycleService, WorkerLifecycleSpawnRequest,
};
use super::ticket::{TICKET_SERVICE_ID, TicketService};
use crate::feature::{
    FeatureDescriptor, FeatureInstallContext, FeatureInstallError, FeatureModule, ServiceId,
    ServiceRequirement, ToolContribution, ToolDeclaration,
};

const FEATURE_ID: &str = "orchestration";
const TOOL_NAME: &str = "SpawnTicketCoder";
const CODER_PROFILE: &str = "builtin:coder";
const CODER_FLOW: &str = "builtin:coder-review";

#[derive(Debug, Default)]
pub struct OrchestrationFeature;

pub fn orchestration_feature() -> OrchestrationFeature {
    OrchestrationFeature
}

impl FeatureModule for OrchestrationFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builtin(FEATURE_ID, "Orchestration")
            .with_description("Semantic Ticket orchestration operations.")
            .with_service_requirement(ServiceRequirement::required(
                ServiceId::builtin(TICKET_SERVICE_ID),
                "SpawnTicketCoder requires current typed Ticket authority",
            ))
            .with_service_requirement(ServiceRequirement::required(
                ServiceId::builtin(WORKER_LIFECYCLE_SERVICE_ID),
                "SpawnTicketCoder requires Workspace Worker lifecycle authority",
            ))
            .with_tool(ToolDeclaration::new(
                TOOL_NAME,
                "Spawn and atomically assign a Coder Worker for an inprogress Ticket. The profile, Flow, display name, assignment operation, and initial message are fixed by orchestration policy.",
            ))
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        let ticket_service = context
            .services()
            .require::<dyn TicketService>(&ServiceId::builtin(TICKET_SERVICE_ID))?;
        let worker_service =
            context
                .services()
                .require::<dyn WorkerLifecycleService>(&ServiceId::builtin(
                    WORKER_LIFECYCLE_SERVICE_ID,
                ))?;
        context.tools().register(ToolContribution::new(
            TOOL_NAME,
            definition(ticket_service, worker_service),
        ))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SpawnTicketCoderInput {
    ticket_id: String,
    runtime_id: String,
    working_directory_id: String,
    #[serde(default)]
    relative_cwd: Option<String>,
}

struct SpawnTicketCoderTool {
    ticket_service: Arc<dyn TicketService>,
    worker_service: Arc<dyn WorkerLifecycleService>,
}

#[async_trait]
impl Tool for SpawnTicketCoderTool {
    async fn execute(
        &self,
        input_json: &str,
        ctx: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let input: SpawnTicketCoderInput = serde_json::from_str(input_json).map_err(|error| {
            ToolError::InvalidArgument(format!("invalid {TOOL_NAME} input: {error}"))
        })?;
        let ticket_id = authority_id(input.ticket_id, "ticket_id")?;
        let workflow_state = self
            .ticket_service
            .workflow_state(&ticket_id)
            .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
        if workflow_state != ticket::TicketWorkflowState::InProgress {
            return Err(ToolError::ExecutionFailed(format!(
                "Ticket {ticket_id} must be inprogress before spawning its Coder; current state is {}",
                workflow_state.as_str()
            )));
        }
        let call_id = non_empty(ctx.call_id, "tool call_id")?;
        let relative_cwd = input.relative_cwd.map(validate_relative_cwd).transpose()?;
        let response = self
            .worker_service
            .spawn(WorkerLifecycleSpawnRequest {
                runtime_id: authority_id(input.runtime_id, "runtime_id")?,
                working_directory_id: authority_id(
                    input.working_directory_id,
                    "working_directory_id",
                )?,
                relative_cwd,
                profile: CODER_PROFILE.to_string(),
                ticket_id: Some(ticket_id.clone()),
                operation_id: Some(format!("spawn-ticket-coder:{ticket_id}:{call_id}")),
                display_name: format!("Coder · {ticket_id}"),
                initial_submit: vec![
                    Segment::Flow {
                        selector: CODER_FLOW.to_string(),
                    },
                    Segment::text(format!("Implement Ticket {ticket_id}.")),
                ],
            })
            .await
            .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
        if !response.is_success() {
            return Err(ToolError::ExecutionFailed(format!(
                "Workspace Worker operation returned HTTP {}: {}",
                response.status, response.body
            )));
        }
        Ok(ToolOutput {
            summary: format!("Spawned Coder for Ticket {ticket_id}"),
            content: Some(response.body),
            attachments: Vec::new(),
        })
    }
}

fn definition(
    ticket_service: Arc<dyn TicketService>,
    worker_service: Arc<dyn WorkerLifecycleService>,
) -> ToolDefinition {
    Arc::new(move || {
        let schema = serde_json::to_value(schemars::schema_for!(SpawnTicketCoderInput))
            .unwrap_or_else(|_| serde_json::json!({}));
        let meta = ToolMeta::new(TOOL_NAME)
            .description("Spawn and atomically assign a policy-configured Coder for a Ticket.")
            .input_schema(schema);
        let tool: Arc<dyn Tool> = Arc::new(SpawnTicketCoderTool {
            ticket_service: ticket_service.clone(),
            worker_service: worker_service.clone(),
        });
        (meta, tool)
    })
}

fn authority_id(value: String, field: &str) -> Result<String, ToolError> {
    let value = non_empty(value, field)?;
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

fn validate_relative_cwd(value: String) -> Result<String, ToolError> {
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

    use ticket::{TicketError, TicketWorkflowState};

    use crate::worker::{WorkspaceClientError, WorkspaceResponse};

    use super::*;

    #[derive(Default)]
    struct RecordingTicketService;

    impl TicketService for RecordingTicketService {
        fn workflow_state(&self, _ticket_id: &str) -> Result<TicketWorkflowState, TicketError> {
            Ok(TicketWorkflowState::InProgress)
        }
    }

    struct FixedTicketService(TicketWorkflowState);

    impl TicketService for FixedTicketService {
        fn workflow_state(&self, _ticket_id: &str) -> Result<TicketWorkflowState, TicketError> {
            Ok(self.0)
        }
    }

    #[derive(Default)]
    struct RecordingService {
        requests: Mutex<Vec<WorkerLifecycleSpawnRequest>>,
    }

    #[async_trait]
    impl WorkerLifecycleService for RecordingService {
        async fn spawn(
            &self,
            request: WorkerLifecycleSpawnRequest,
        ) -> Result<WorkspaceResponse, WorkspaceClientError> {
            self.requests.lock().unwrap().push(request);
            Ok(WorkspaceResponse {
                status: 200,
                body: r#"{"worker_id":"42"}"#.to_string(),
            })
        }
    }

    #[tokio::test]
    async fn spawn_ticket_coder_fixes_profile_flow_assignment_and_message() {
        let service = Arc::new(RecordingService::default());
        let tool = SpawnTicketCoderTool {
            ticket_service: Arc::new(RecordingTicketService),
            worker_service: service.clone(),
        };
        tool.execute(
            &serde_json::json!({
                "ticket_id": "00001KZXN51C7",
                "runtime_id": "runtime-1",
                "working_directory_id": "workdir-1"
            })
            .to_string(),
            ToolExecutionContext::new("call-7", "batch-1", 0),
        )
        .await
        .unwrap();

        let requests = service.requests.lock().unwrap();
        let request = &requests[0];
        assert_eq!(request.profile, CODER_PROFILE);
        assert_eq!(request.ticket_id.as_deref(), Some("00001KZXN51C7"));
        assert_eq!(
            request.operation_id.as_deref(),
            Some("spawn-ticket-coder:00001KZXN51C7:call-7")
        );
        assert_eq!(request.display_name, "Coder · 00001KZXN51C7");
        assert_eq!(
            request.initial_submit,
            vec![
                Segment::Flow {
                    selector: CODER_FLOW.to_string()
                },
                Segment::text("Implement Ticket 00001KZXN51C7.")
            ]
        );
    }

    #[tokio::test]
    async fn spawn_ticket_coder_rejects_ticket_before_worker_side_effect() {
        let worker_service = Arc::new(RecordingService::default());
        let tool = SpawnTicketCoderTool {
            ticket_service: Arc::new(FixedTicketService(TicketWorkflowState::Queued)),
            worker_service: worker_service.clone(),
        };
        let error = tool
            .execute(
                &serde_json::json!({
                    "ticket_id": "00001KZXN51C7",
                    "runtime_id": "runtime-1",
                    "working_directory_id": "workdir-1"
                })
                .to_string(),
                ToolExecutionContext::new("call-queued", "batch-1", 0),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("must be inprogress"));
        assert!(worker_service.requests.lock().unwrap().is_empty());
    }

    #[test]
    fn orchestration_descriptor_requires_ticket_and_worker_services() {
        let descriptor = orchestration_feature().descriptor();
        let required: Vec<_> = descriptor
            .requires_services
            .iter()
            .map(|requirement| requirement.id.clone())
            .collect();
        assert_eq!(
            required,
            vec![
                ServiceId::builtin(TICKET_SERVICE_ID),
                ServiceId::builtin(WORKER_LIFECYCLE_SERVICE_ID),
            ]
        );
    }

    #[test]
    fn input_surface_does_not_expose_profile_flow_or_assignment_controls() {
        let schema = serde_json::to_string(&schemars::schema_for!(SpawnTicketCoderInput)).unwrap();
        for field in [
            "ticket_id",
            "runtime_id",
            "working_directory_id",
            "relative_cwd",
        ] {
            assert!(schema.contains(field));
        }
        for forbidden in [
            "profile",
            "selector",
            "operation_id",
            "display_name",
            "initial_submit",
        ] {
            assert!(!schema.contains(forbidden), "schema leaked {forbidden}");
        }
    }
}
