//! Semantic Ticket orchestration tools backed by Feature Services.

use std::collections::BTreeSet;
use std::sync::Arc;

use agen::tool::{Tool, ToolDefinition, ToolError, ToolExecutionContext, ToolMeta, ToolOutput};
use async_trait::async_trait;
use protocol::Segment;
use schemars::JsonSchema;
use serde::Deserialize;

use super::manage_worker::{
    WORKER_LIFECYCLE_SERVICE_ID, WorkerLifecycleService, WorkerLifecycleSpawnRequest,
    WorkerLifecycleWorkdirAttachment,
};
use super::ticket::{TICKET_SERVICE_ID, TicketService};
use crate::feature::{
    FeatureDescriptor, FeatureInstallContext, FeatureInstallError, FeatureModule, ServiceId,
    ServiceRequirement, ToolContribution, ToolDeclaration,
};

const FEATURE_ID: &str = "orchestration";
const TOOL_NAME: &str = "SpawnTicketWorker";

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
                "SpawnTicketWorker requires current typed Ticket authority",
            ))
            .with_service_requirement(ServiceRequirement::required(
                ServiceId::builtin(WORKER_LIFECYCLE_SERVICE_ID),
                "SpawnTicketWorker requires Workspace Worker lifecycle authority",
            ))
            .with_tool(ToolDeclaration::new(
                TOOL_NAME,
                "Spawn and atomically assign a configurable Worker for a Ticket. Supply a registered profile selector, an initial request, and optionally a Flow selector. Workdir attachments may be empty; Backend authority validates claims and derives each attachment's access from the Ticket targets. The guarded operation records acceptance only after spawn, initial input, assignment, and resource finalization. Selecting a profile or Flow does not grant review authority.",
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
struct SpawnTicketWorkerWorkdirInput {
    /// Stable Worker-local routing alias (for example `checkout` or `docs`).
    alias: String,
    /// Workspace-authoritative Workdir id. Paths and URLs are not accepted.
    working_directory_id: String,
    /// Optional normalized path inside this Workdir for the Worker's initial cwd.
    #[serde(default)]
    relative_cwd: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SpawnTicketWorkerInput {
    ticket_id: String,
    runtime_id: String,
    /// Registered profile selector. Backend resolves the profile and validates claims.
    profile: String,
    /// Initial user request delivered through the normal typed submission path.
    initial_request: String,
    /// Optional Flow selector, delivered as a typed Flow segment before the request.
    #[serde(default)]
    flow: Option<String>,
    /// Alias-keyed Workdir selections; omit or leave empty for a Worker without
    /// Workdirs. Backend derives effective capabilities; callers cannot request them.
    #[serde(default)]
    workdir_attachments: Vec<SpawnTicketWorkerWorkdirInput>,
}

struct SpawnTicketWorkerTool {
    ticket_service: Arc<dyn TicketService>,
    worker_service: Arc<dyn WorkerLifecycleService>,
}

#[async_trait]
impl Tool for SpawnTicketWorkerTool {
    async fn execute(
        &self,
        input_json: &str,
        ctx: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let input: SpawnTicketWorkerInput = serde_json::from_str(input_json).map_err(|error| {
            ToolError::InvalidArgument(format!("invalid {TOOL_NAME} input: {error}"))
        })?;
        let ticket_ref = authority_id(input.ticket_id, "ticket_id")?;
        let ticket = self
            .ticket_service
            .ticket_handoff(&ticket_ref)
            .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
        let call_id = non_empty(ctx.call_id, "tool call_id")?;
        let runtime_id = authority_id(input.runtime_id, "runtime_id")?;
        let profile = non_empty(input.profile, "profile")?;
        let initial_request = non_empty(input.initial_request, "initial_request")?;
        let flow = input.flow.map(|flow| non_empty(flow, "flow")).transpose()?;
        let workdir_attachments = validate_workdir_attachments(input.workdir_attachments)?;
        let mut initial_submit = Vec::new();
        if let Some(selector) = flow {
            initial_submit.push(Segment::Flow { selector });
        }
        initial_submit.push(Segment::text(initial_request));
        let response = self
            .worker_service
            .spawn(WorkerLifecycleSpawnRequest {
                runtime_id,
                workdir_attachments,
                profile,
                singleton_key: None,
                ticket_id: Some(ticket.id.clone()),
                operation_id: Some(format!("spawn-ticket-worker:{}:{call_id}", ticket.id)),
                display_name: format!("Worker · {}", ticket.resource_key),
                initial_submit,
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
            summary: format!("Spawned Worker for Ticket {}", ticket.resource_key),
            content: Some(response.body),
            attachments: Vec::new(),
        })
    }
}

fn validate_workdir_attachments(
    attachments: Vec<SpawnTicketWorkerWorkdirInput>,
) -> Result<Vec<WorkerLifecycleWorkdirAttachment>, ToolError> {
    let mut aliases = BTreeSet::new();
    let mut workdir_ids = BTreeSet::new();
    attachments
        .into_iter()
        .map(|attachment| {
            let alias = authority_id(attachment.alias, "workdir_attachments.alias")?;
            if !aliases.insert(alias.clone()) {
                return Err(ToolError::InvalidArgument(format!(
                    "duplicate Workdir attachment alias `{alias}`"
                )));
            }
            let working_directory_id = authority_id(
                attachment.working_directory_id,
                "workdir_attachments.working_directory_id",
            )?;
            if !workdir_ids.insert(working_directory_id.clone()) {
                return Err(ToolError::InvalidArgument(format!(
                    "duplicate Workdir attachment `{working_directory_id}`"
                )));
            }
            Ok(WorkerLifecycleWorkdirAttachment {
                alias,
                working_directory_id,
                relative_cwd: attachment
                    .relative_cwd
                    .map(validate_relative_cwd)
                    .transpose()?,
            })
        })
        .collect()
}

fn definition(
    ticket_service: Arc<dyn TicketService>,
    worker_service: Arc<dyn WorkerLifecycleService>,
) -> ToolDefinition {
    Arc::new(move || {
        let schema = serde_json::to_value(schemars::schema_for!(SpawnTicketWorkerInput))
            .unwrap_or_else(|_| serde_json::json!({}));
        let meta = ToolMeta::new(TOOL_NAME)
            .description(
                "Spawn and atomically assign a configurable Ticket Worker with a registered profile, initial request, optional typed Flow, and zero or more alias-keyed Workdir attachments. Backend validates claims and resources; profile and Flow selection do not grant review authority.",
            )
            .input_schema(schema);
        let tool: Arc<dyn Tool> = Arc::new(SpawnTicketWorkerTool {
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

    use super::*;
    use crate::feature::builtin::ticket::TicketHandoff;
    use crate::worker::{WorkspaceClientError, WorkspaceResponse};

    struct FixedTicketService(TicketWorkflowState);

    impl TicketService for FixedTicketService {
        fn ticket_handoff(&self, ticket_ref: &str) -> Result<TicketHandoff, TicketError> {
            assert_eq!(ticket_ref, "T-719");
            Ok(TicketHandoff {
                id: "00001KZXN51C7".to_string(),
                resource_key: "T-719".to_string(),
                workflow_state: self.0,
            })
        }
    }

    #[derive(Default)]
    struct RecordingService {
        requests: Mutex<Vec<WorkerLifecycleSpawnRequest>>,
        response: Option<WorkspaceResponse>,
    }

    #[async_trait]
    impl WorkerLifecycleService for RecordingService {
        async fn spawn(
            &self,
            request: WorkerLifecycleSpawnRequest,
        ) -> Result<WorkspaceResponse, WorkspaceClientError> {
            self.requests.lock().unwrap().push(request);
            Ok(self.response.clone().unwrap_or_else(|| WorkspaceResponse {
                status: 200,
                body: r#"{"worker_id":"42"}"#.to_string(),
            }))
        }
    }

    fn input() -> serde_json::Value {
        serde_json::json!({
            "ticket_id": "T-719",
            "runtime_id": "runtime-1",
            "profile": "project:ticket-worker",
            "initial_request": "Investigate Ticket T-719."
        })
    }

    fn tool(service: Arc<RecordingService>, state: TicketWorkflowState) -> SpawnTicketWorkerTool {
        SpawnTicketWorkerTool {
            ticket_service: Arc::new(FixedTicketService(state)),
            worker_service: service,
        }
    }

    #[tokio::test]
    async fn spawn_ticket_worker_forwards_selected_profile_flow_request_and_workdirs() {
        let service = Arc::new(RecordingService::default());
        let mut input = input();
        input["flow"] = serde_json::json!("project:investigation");
        input["workdir_attachments"] = serde_json::json!([
            {
                "alias": "checkout",
                "working_directory_id": "workdir-1",
                "relative_cwd": "crates/worker"
            },
            {"alias": "docs", "working_directory_id": "workdir-2"}
        ]);
        let output = tool(service.clone(), TicketWorkflowState::Queued)
            .execute(
                &input.to_string(),
                ToolExecutionContext::new("call-7", "batch-1", 0),
            )
            .await
            .unwrap();

        let requests = service.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert_eq!(request.runtime_id, "runtime-1");
        assert_eq!(request.profile, "project:ticket-worker");
        assert_eq!(request.ticket_id.as_deref(), Some("00001KZXN51C7"));
        assert_eq!(
            request.operation_id.as_deref(),
            Some("spawn-ticket-worker:00001KZXN51C7:call-7")
        );
        assert_eq!(request.display_name, "Worker · T-719");
        assert_eq!(request.singleton_key, None);
        assert_eq!(
            request.workdir_attachments,
            vec![
                WorkerLifecycleWorkdirAttachment {
                    alias: "checkout".into(),
                    working_directory_id: "workdir-1".into(),
                    relative_cwd: Some("crates/worker".into()),
                },
                WorkerLifecycleWorkdirAttachment {
                    alias: "docs".into(),
                    working_directory_id: "workdir-2".into(),
                    relative_cwd: None,
                },
            ]
        );
        assert_eq!(
            request.initial_submit,
            vec![
                Segment::Flow {
                    selector: "project:investigation".into()
                },
                Segment::text("Investigate Ticket T-719."),
            ]
        );
        assert_eq!(output.summary, "Spawned Worker for Ticket T-719");
        assert_eq!(output.content.as_deref(), Some(r#"{"worker_id":"42"}"#));
    }

    #[tokio::test]
    async fn spawn_ticket_worker_allows_zero_workdirs_and_omitted_flow() {
        let service = Arc::new(RecordingService::default());
        for explicit_empty in [false, true] {
            let mut input = input();
            if explicit_empty {
                input["workdir_attachments"] = serde_json::json!([]);
                input["flow"] = serde_json::Value::Null;
            }
            tool(service.clone(), TicketWorkflowState::Queued)
                .execute(
                    &input.to_string(),
                    ToolExecutionContext::new("call-empty", "batch-1", 0),
                )
                .await
                .unwrap();
        }
        let requests = service.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        for request in requests.iter() {
            assert!(request.workdir_attachments.is_empty());
            assert_eq!(
                request.initial_submit,
                vec![Segment::text("Investigate Ticket T-719.")]
            );
        }
    }

    #[tokio::test]
    async fn spawn_ticket_worker_defers_claim_eligibility_to_backend() {
        let service = Arc::new(RecordingService {
            response: Some(WorkspaceResponse {
                status: 409,
                body: "Backend rejected Ticket claim".into(),
            }),
            ..Default::default()
        });
        let error = tool(service.clone(), TicketWorkflowState::Planning)
            .execute(
                &input().to_string(),
                ToolExecutionContext::new("call-claim", "batch-1", 0),
            )
            .await
            .unwrap_err();
        assert_eq!(service.requests.lock().unwrap().len(), 1);
        assert!(
            error
                .to_string()
                .contains("HTTP 409: Backend rejected Ticket claim")
        );
    }

    #[tokio::test]
    async fn spawn_ticket_worker_rejects_invalid_input_before_worker_side_effect() {
        let service = Arc::new(RecordingService::default());
        let tool = tool(service.clone(), TicketWorkflowState::Queued);
        let cases = [
            (
                "profile",
                serde_json::json!("  "),
                "profile must not be empty",
            ),
            (
                "initial_request",
                serde_json::json!(" \n "),
                "initial_request must not be empty",
            ),
            ("flow", serde_json::json!(" "), "flow must not be empty"),
            (
                "runtime_id",
                serde_json::json!("runtime/1"),
                "runtime_id must be an authority id",
            ),
            (
                "ticket_id",
                serde_json::json!("https://example.test/ticket"),
                "ticket_id must be an authority id",
            ),
            (
                "workdir_attachments",
                serde_json::json!([
                    {"alias": "checkout", "working_directory_id": "workdir-1"},
                    {"alias": " checkout ", "working_directory_id": "workdir-2"}
                ]),
                "duplicate Workdir attachment alias `checkout`",
            ),
            (
                "workdir_attachments",
                serde_json::json!([
                    {"alias": "checkout", "working_directory_id": "workdir-1"},
                    {"alias": "docs", "working_directory_id": " workdir-1 "}
                ]),
                "duplicate Workdir attachment `workdir-1`",
            ),
            (
                "workdir_attachments",
                serde_json::json!([
                    {"alias": "docs/path", "working_directory_id": "workdir-1"}
                ]),
                "workdir_attachments.alias must be an authority id",
            ),
            (
                "workdir_attachments",
                serde_json::json!([
                    {"alias": "docs", "working_directory_id": "https://example.test/workdir"}
                ]),
                "workdir_attachments.working_directory_id must be an authority id",
            ),
            (
                "workdir_attachments",
                serde_json::json!([
                    {"alias": "checkout", "working_directory_id": "workdir-1", "relative_cwd": "../outside"}
                ]),
                "relative_cwd must be a normalized relative path inside the Workdir",
            ),
            (
                "workdir_attachments",
                serde_json::json!([
                    {"alias": "checkout", "working_directory_id": "workdir-1", "access": "write"}
                ]),
                "unknown field `access`",
            ),
            (
                "review",
                serde_json::json!({"ticket_id": "T-719", "merge_request_id": "MR-1"}),
                "unknown field `review`",
            ),
            (
                "operation_id",
                serde_json::json!("caller-claim"),
                "unknown field `operation_id`",
            ),
            (
                "display_name",
                serde_json::json!("Reviewer"),
                "unknown field `display_name`",
            ),
            (
                "initial_submit",
                serde_json::json!([]),
                "unknown field `initial_submit`",
            ),
        ];
        for (index, (field, value, expected)) in cases.into_iter().enumerate() {
            let mut input = input();
            input[field] = value;
            let error = tool
                .execute(
                    &input.to_string(),
                    ToolExecutionContext::new(format!("call-{index}"), "batch-validation", index),
                )
                .await
                .unwrap_err();
            assert!(
                error.to_string().contains(expected),
                "unexpected error for {field}: {error}"
            );
        }
        for field in ["ticket_id", "runtime_id", "profile", "initial_request"] {
            let mut input = input();
            input.as_object_mut().unwrap().remove(field);
            let error = tool
                .execute(
                    &input.to_string(),
                    ToolExecutionContext::new("call-missing", "batch-validation", 0),
                )
                .await
                .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains(&format!("missing field `{field}`")),
                "{error}"
            );
        }
        assert!(service.requests.lock().unwrap().is_empty());
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
    fn spawn_ticket_worker_schema_exposes_configuration_not_claim_or_review_grants() {
        let definition = definition(
            Arc::new(FixedTicketService(TicketWorkflowState::Queued)),
            Arc::new(RecordingService::default()),
        );
        let (meta, _) = definition();
        assert_eq!(meta.name, "SpawnTicketWorker");
        let schema = serde_json::to_value(schemars::schema_for!(SpawnTicketWorkerInput)).unwrap();
        assert_eq!(schema["additionalProperties"], false);
        let required = schema["required"].as_array().unwrap();
        for field in ["ticket_id", "runtime_id", "profile", "initial_request"] {
            assert!(
                required.contains(&serde_json::json!(field)),
                "missing required field {field}"
            );
        }
        for field in ["flow", "workdir_attachments"] {
            assert!(schema["properties"].get(field).is_some());
            assert!(!required.contains(&serde_json::json!(field)));
        }
        for forbidden in [
            "review",
            "operation_id",
            "display_name",
            "initial_submit",
            "claim",
            "access",
        ] {
            assert!(
                schema["properties"].get(forbidden).is_none(),
                "schema leaked {forbidden}"
            );
        }
    }
}
