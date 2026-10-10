//! Built-in Ticket feature adapter.
//!
//! The ticket crate owns Ticket domain logic and Tool implementations. This
//! module binds an authority-scoped Workspace client, declares the built-in
//! feature, and contributes those tools through the normal registry path.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use manifest::{ToolPermissionAction, ToolPermissionConfig};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use ticket::{
    MarkdownText, NewOrchestrationPlanRecord, NewTicket, NewTicketEvent, NewTicketRelation,
    OrchestrationPlanKind, OrchestrationPlanRecord, Result as TicketResult, Ticket, TicketBackend,
    TicketBackendOperation, TicketBackendOperationResult, TicketDoctorReport, TicketError,
    TicketIdOrSlug, TicketIntakeSummary, TicketListQuery, TicketRef, TicketRelation,
    TicketRelationKind, TicketRelationView, TicketStateChange, TicketSummary, TicketWorkflowState,
    tool::{TICKET_TOOL_NAMES, TicketToolBackend, ticket_tool_description, ticket_tools},
};
use wip_protocol::{
    Documentation, INTERFACE_FORMAT_V1, InterfaceDescriptor, Object, OperationDeclaration,
    ParameterDeclaration, ProtocolError, ProtocolErrorCode, ReturnDeclaration, TypeExpr,
    Value as WipValue,
};

use crate::feature::{
    FeatureDescriptor, FeatureInstallContext, FeatureInstallError, FeatureInstructionContribution,
    FeatureInstructionDeclaration, FeatureInstructionId, FeatureModule, ServiceDeclaration,
    ServiceId, ToolContribution, ToolDeclaration, ToolDefinition,
};
use crate::permission::permission_action_for;
use crate::wip::{
    WipCallContext, WipDynamicItem, WipDynamicItemResolver, WipDynamicMount, WipMountError,
    WipMountRegistry, WipNamespaceRoute, WipOperationError, WipOperationHandler,
    WipOperationOutput, WipProjection, WipProjectionKind, json_to_wip, wip_to_json,
};
use crate::worker::{WorkspaceClient, WorkspaceRequest, WorkspaceRequestMethod};
use agen::tool::{Tool, ToolError, ToolExecutionContext, ToolMeta, ToolOutput};

use super::resource_projection::{project_ticket_detail, project_ticket_query};

#[derive(Clone, Copy)]
enum WorkspaceTicketReadKind {
    Query,
    Show,
}

impl WorkspaceTicketReadKind {
    fn name(self) -> &'static str {
        match self {
            Self::Query => "QueryTicket",
            Self::Show => "ShowTicket",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Query => {
                "Query authoritative Workspace Tickets with bounded typed filters, stable snippets, evidence summaries, and cursor metadata."
            }
            Self::Show => {
                "Show one authoritative Workspace Ticket with its content digest, paged thread, links, historical implementation reports, and current Merge Request readiness evidence."
            }
        }
    }

    fn schema(self) -> Value {
        match self {
            Self::Query => serde_json::to_value(schemars::schema_for!(WorkspaceQueryTicketInput))
                .expect("QueryTicket schema serializes"),
            Self::Show => serde_json::to_value(schemars::schema_for!(WorkspaceShowTicketInput))
                .expect("ShowTicket schema serializes"),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum WorkspaceTicketStateFilter {
    Planning,
    Ready,
    Queued,
    Inprogress,
    Done,
    Closed,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum WorkspaceTicketEvidenceFilter {
    MergeRequest,
    Commit,
    ApprovedReview,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum WorkspaceTicketReviewFilter {
    None,
    Unknown,
    Pending,
    Approved,
    RequestChanges,
    UnresolvedChanges,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum WorkspaceTicketAttentionFilter {
    DoneNotClosed,
    UnresolvedReview,
    MissingCommit,
    Blocked,
    Unblocked,
    Ready,
    AwaitingReview,
    UnresolvedChanges,
    StaleAfterRescope,
    MissingEvidence,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum WorkspaceTicketRelationFilter {
    DependsOn,
    Blocks,
    Related,
    Supersedes,
    DuplicateOf,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum WorkspaceTicketSort {
    Relevance,
    UpdatedDesc,
    CreatedDesc,
    Priority,
    Title,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct WorkspaceQueryTicketInput {
    /// Full-text match over Ticket title, item body, and bounded thread excerpts.
    query: Option<String>,
    /// Exact workflow states. Empty means every state.
    #[serde(default)]
    states: Vec<WorkspaceTicketStateFilter>,
    /// Exact typed event kinds that must occur in the bounded thread window.
    #[serde(default)]
    event_kinds: Vec<String>,
    /// Required evidence kinds: merge_request, commit, or approved_review.
    #[serde(default)]
    evidence: Vec<WorkspaceTicketEvidenceFilter>,
    /// Current authoritative Merge Request review status: none, pending, approved,
    /// request_changes, or unresolved_changes.
    review_status: Option<WorkspaceTicketReviewFilter>,
    /// Attention filters include done_not_closed, unresolved_review, missing_commit,
    /// blocked, unblocked, ready, awaiting_review, unresolved_changes,
    /// stale_after_rescope, and missing_evidence.
    #[serde(default)]
    attention: Vec<WorkspaceTicketAttentionFilter>,
    /// Related Ticket reference. Prefer `T-*`; canonical internal ids remain accepted for compatibility.
    related_ticket_id: Option<String>,
    relation_kind: Option<WorkspaceTicketRelationFilter>,
    /// Linked Objective reference. Prefer `O-*`; canonical internal ids remain accepted for compatibility.
    linked_objective_id: Option<String>,
    updated_after: Option<String>,
    updated_before: Option<String>,
    /// relevance (default when query is present), updated_desc, created_desc,
    /// priority, or title.
    sort: Option<WorkspaceTicketSort>,
    /// Page size; bounded by the Backend to 1..=100.
    limit: Option<usize>,
    /// Opaque cursor returned by a prior QueryTicket page.
    cursor: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct WorkspaceShowTicketInput {
    /// Ticket reference. Prefer `T-*`; canonical internal ids remain accepted for compatibility.
    id: String,
    /// Most-recent thread entries to return, bounded by the Backend to 1..=50.
    event_limit: Option<usize>,
    /// Opaque event cursor returned by a prior ShowTicket page.
    event_cursor: Option<String>,
}

#[derive(Clone)]
struct WorkspaceTicketReadTool {
    client: Arc<dyn WorkspaceClient>,
    kind: WorkspaceTicketReadKind,
}

#[async_trait]
impl Tool for WorkspaceTicketReadTool {
    async fn execute(
        &self,
        input: &str,
        _context: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let workspace_id = self.client.workspace_id().ok_or_else(|| {
            ToolError::InvalidArgument("Workspace Ticket reads require workspace identity".into())
        })?;
        let (path, body) = match self.kind {
            WorkspaceTicketReadKind::Query => {
                let input: WorkspaceQueryTicketInput = serde_json::from_str(&input)
                    .map_err(|error| ToolError::InvalidArgument(error.to_string()))?;
                (
                    format!("/api/w/{workspace_id}/tickets/query"),
                    serde_json::to_value(input)
                        .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?,
                )
            }
            WorkspaceTicketReadKind::Show => {
                let input: WorkspaceShowTicketInput = serde_json::from_str(&input)
                    .map_err(|error| ToolError::InvalidArgument(error.to_string()))?;
                if input.id.trim().is_empty() {
                    return Err(ToolError::InvalidArgument(
                        "ShowTicket.id must not be empty".into(),
                    ));
                }
                let path = format!("/api/w/{workspace_id}/tickets/{}/show", input.id.trim());
                let body = json!({
                    "event_limit": input.event_limit,
                    "event_cursor": input.event_cursor,
                });
                (path, body)
            }
        };
        let response = self
            .client
            .execute(WorkspaceRequest::json(
                WorkspaceRequestMethod::Post,
                path,
                serde_json::to_string(&body)
                    .map_err(|error| ToolError::Internal(error.to_string()))?,
            ))
            .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
        if !response.is_success() {
            return Err(ToolError::ExecutionFailed(format!(
                "Workspace Ticket API request failed with HTTP status {}",
                response.status
            )));
        }
        let response_value: Value = serde_json::from_str(&response.body).map_err(|error| {
            ToolError::ExecutionFailed(format!(
                "Workspace Ticket API returned invalid JSON: {error}"
            ))
        })?;
        let content = match self.kind {
            WorkspaceTicketReadKind::Query => serde_json::to_string(
                &project_ticket_query(response_value).map_err(ToolError::ExecutionFailed)?,
            ),
            WorkspaceTicketReadKind::Show => serde_json::to_string(
                &project_ticket_detail(response_value).map_err(ToolError::ExecutionFailed)?,
            ),
        }
        .map_err(|error| ToolError::Internal(error.to_string()))?;
        Ok(ToolOutput {
            summary: self.kind.name().to_string(),
            content: Some(content),
            attachments: Vec::new(),
        })
    }
}

fn workspace_ticket_read_definition(
    client: Arc<dyn WorkspaceClient>,
    kind: WorkspaceTicketReadKind,
) -> ToolDefinition {
    Arc::new(move || {
        let meta = ToolMeta::new(kind.name())
            .description(kind.description())
            .input_schema(kind.schema());
        let tool: Arc<dyn Tool> = Arc::new(WorkspaceTicketReadTool {
            client: client.clone(),
            kind,
        });
        (meta, tool)
    })
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WorkspaceCompleteTicketInput {
    /// Ticket reference. Prefer `T-*`.
    ticket: String,
    /// Stable key for exact replay of this completion decision.
    operation_key: String,
    /// SHA-256 of the current editable Ticket content from ShowTicket.
    expected_content_digest: String,
    expected_state: TicketWorkflowState,
    /// Required explanation of the completion decision; MR approval is not a prerequisite.
    reason: String,
    /// Optional typed supporting Ticket references, independent of MR integration evidence.
    #[serde(default)]
    references: Vec<ticket::TicketReference>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WorkspaceTicketStateUpdateInput {
    /// Ticket reference. Prefer `T-*`.
    ticket: String,
    operation_key: String,
    /// SHA-256 of the current editable Ticket content from ShowTicket.
    expected_content_digest: String,
    expected_state: TicketWorkflowState,
    /// Progress-display state only: this does not start Workers or integrate Merge Requests.
    state: TicketWorkflowState,
    /// Required explanation for the state decision.
    reason: String,
    #[serde(default)]
    references: Vec<ticket::TicketReference>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WorkspaceTicketCloseInput {
    /// Ticket reference. Prefer `T-*`; WIP binds this from the item route.
    ticket: String,
    /// Stable key for exact replay of this close decision.
    operation_key: String,
    /// SHA-256 of the current editable Ticket content from ShowTicket.
    expected_content_digest: String,
    expected_state: TicketWorkflowState,
    /// Required explanation for closing this Ticket.
    resolution: String,
    #[serde(default)]
    references: Vec<ticket::TicketReference>,
}

const TICKET_CLOSE_DESCRIPTION: &str = "Record an authorized Ticket closure decision with resolution, item/state CAS, and replay key when the requested return path calls for closure. No Merge Request or enforced state sequence is required. Backend checks actor authority, not natural-language satisfaction; this does not integrate MRs, stop/remove Workers, clean attachments, or expand Workdir grants.";
const COMPLETE_TICKET_DESCRIPTION: &str = "Record an authorized Ticket completion decision with reason, item/state CAS, and replay key when user intent calls for completion; return without conclusion need not call this tool. Merge Requests and approvals are not prerequisites. Backend checks assigned Worker/Workspace Orchestrator authority, not real judgment of natural-language satisfaction; model-supplied roles never grant authority. Conclusion ends unfinished work while retaining terminal responsibility for display; it does not grant review/merge authority or stop/remove Workers and attachments.";
const TICKET_STATE_UPDATE_DESCRIPTION: &str = "Update a Ticket progress-display state with reason, item/state CAS, and replay key. States need not follow an enforced sequence. Backend checks actor authority, not natural-language satisfaction. This does not start/stop Workers, queue dependencies, integrate Merge Requests, clean attachments, or enlarge live Workdir grants.";

#[derive(Clone, Copy)]
enum WorkspaceTicketDecisionKind {
    Complete,
    UpdateState,
    Close,
}

impl WorkspaceTicketDecisionKind {
    fn name(self) -> &'static str {
        match self {
            Self::Complete => "CompleteTicket",
            Self::UpdateState => "TicketWorkflowState",
            Self::Close => "TicketClose",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Complete => COMPLETE_TICKET_DESCRIPTION,
            Self::UpdateState => TICKET_STATE_UPDATE_DESCRIPTION,
            Self::Close => TICKET_CLOSE_DESCRIPTION,
        }
    }

    fn schema(self) -> Value {
        match self {
            Self::Complete => json!(schemars::schema_for!(WorkspaceCompleteTicketInput)),
            Self::UpdateState => json!(schemars::schema_for!(WorkspaceTicketStateUpdateInput)),
            Self::Close => json!(schemars::schema_for!(WorkspaceTicketCloseInput)),
        }
    }
}

#[derive(Clone)]
struct WorkspaceTicketDecisionTool {
    client: Arc<dyn WorkspaceClient>,
    kind: WorkspaceTicketDecisionKind,
}

fn validate_ticket_decision(
    ticket: &str,
    operation_key: &str,
    content_digest: &str,
    reason: &str,
) -> Result<(), ToolError> {
    for (name, value) in [
        ("ticket", ticket),
        ("operation_key", operation_key),
        ("expected_content_digest", content_digest),
        ("reason", reason),
    ] {
        if value.trim().is_empty() {
            return Err(ToolError::InvalidArgument(format!(
                "{name} must not be empty"
            )));
        }
    }
    Ok(())
}

#[async_trait]
impl Tool for WorkspaceTicketDecisionTool {
    async fn execute(&self, input: &str, _: ToolExecutionContext) -> Result<ToolOutput, ToolError> {
        let workspace_id = self.client.workspace_id().ok_or_else(|| {
            ToolError::InvalidArgument("Ticket decisions require Workspace identity".into())
        })?;
        let (ticket, endpoint, body) = match self.kind {
            WorkspaceTicketDecisionKind::Complete => {
                let value: WorkspaceCompleteTicketInput = serde_json::from_str(input)
                    .map_err(|error| ToolError::InvalidArgument(error.to_string()))?;
                validate_ticket_decision(
                    &value.ticket,
                    &value.operation_key,
                    &value.expected_content_digest,
                    &value.reason,
                )?;
                let mut body = serde_json::to_value(&value)
                    .map_err(|error| ToolError::Internal(error.to_string()))?;
                body.as_object_mut()
                    .expect("typed Ticket input is an object")
                    .remove("ticket");
                (value.ticket, "complete", body)
            }
            WorkspaceTicketDecisionKind::Close => {
                let value: WorkspaceTicketCloseInput = serde_json::from_str(input)
                    .map_err(|error| ToolError::InvalidArgument(error.to_string()))?;
                validate_ticket_decision(
                    &value.ticket,
                    &value.operation_key,
                    &value.expected_content_digest,
                    &value.resolution,
                )?;
                let body = json!({
                    "operation_key": value.operation_key,
                    "expected_content_digest": value.expected_content_digest,
                    "expected_state": value.expected_state,
                    "state": TicketWorkflowState::Closed,
                    "reason": value.resolution,
                    "references": value.references,
                });
                (value.ticket, "state-update", body)
            }
            WorkspaceTicketDecisionKind::UpdateState => {
                let value: WorkspaceTicketStateUpdateInput = serde_json::from_str(input)
                    .map_err(|error| ToolError::InvalidArgument(error.to_string()))?;
                validate_ticket_decision(
                    &value.ticket,
                    &value.operation_key,
                    &value.expected_content_digest,
                    &value.reason,
                )?;
                let mut body = serde_json::to_value(&value)
                    .map_err(|error| ToolError::Internal(error.to_string()))?;
                body.as_object_mut()
                    .expect("typed Ticket input is an object")
                    .remove("ticket");
                // Actor identity is transport-bound, never a model-visible author/role input.
                (value.ticket, "state-update", body)
            }
        };
        let ticket: Ticket = WorkspaceHttpTicketBackend::request(
            self.client.clone(),
            WorkspaceRequestMethod::Post,
            format!(
                "/api/w/{workspace_id}/tickets/{}/{endpoint}",
                WorkspaceHttpTicketBackend::ticket_path(&TicketIdOrSlug::from(ticket.as_str()))
            ),
            Some(body),
        )
        .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
        Ok(ToolOutput {
            summary: self.kind.name().into(),
            content: Some(
                serde_json::to_string(&ticket)
                    .map_err(|error| ToolError::Internal(error.to_string()))?,
            ),
            attachments: Vec::new(),
        })
    }
}

fn workspace_ticket_decision_definition(
    client: Arc<dyn WorkspaceClient>,
    kind: WorkspaceTicketDecisionKind,
) -> ToolDefinition {
    Arc::new(move || {
        (
            ToolMeta::new(kind.name())
                .description(kind.description())
                .input_schema(kind.schema()),
            Arc::new(WorkspaceTicketDecisionTool {
                client: client.clone(),
                kind,
            }) as Arc<dyn Tool>,
        )
    })
}

fn workspace_ticket_tool_description(name: &str) -> String {
    match name {
        "CompleteTicket" => COMPLETE_TICKET_DESCRIPTION.into(),
        "TicketWorkflowState" => TICKET_STATE_UPDATE_DESCRIPTION.into(),
        "TicketClose" => TICKET_CLOSE_DESCRIPTION.into(),
        _ => ticket_tool_description(name, None),
    }
}

const FEATURE_ID: &str = "ticket";
const FEATURE_NAME: &str = "Ticket tools";
const FEATURE_DESCRIPTION: &str = "Intent-led Ticket reads and authorized decisions through the Workspace API; no universal MR/approval gate, and no Backend judgment of natural-language satisfaction.";
const TICKET_WORKFLOW_INSTRUCTION_ID: &str = "ticket.workflow";
const TICKET_WORKFLOW_PROMPT_REF: &str = "common.tickets";
pub const TICKET_SERVICE_ID: &str = "ticket.authority";
const TICKET_SERVICE_VERSION: &str = "1";

pub trait TicketService: Send + Sync {
    fn ticket_handoff(&self, ticket_ref: &str) -> Result<TicketHandoff, TicketError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketHandoff {
    pub id: String,
    pub resource_key: String,
    pub workflow_state: TicketWorkflowState,
}

fn is_canonical_ticket_resource_key(resource_key: &str) -> bool {
    resource_key.strip_prefix("T-").is_some_and(|sequence| {
        !sequence.is_empty() && sequence.bytes().all(|byte| byte.is_ascii_digit())
    })
}

struct WorkspaceTicketService {
    backend: WorkspaceHttpTicketBackend,
}

fn ticket_handoff_from_record(ticket: Ticket) -> Result<TicketHandoff, TicketError> {
    let resource_key = ticket
        .meta
        .resource_key
        .filter(|key| is_canonical_ticket_resource_key(key))
        .ok_or_else(|| TicketError::Conflict("ticket resource key is unavailable".into()))?;
    Ok(TicketHandoff {
        id: ticket.meta.id,
        resource_key,
        workflow_state: ticket.meta.workflow_state,
    })
}

impl TicketService for WorkspaceTicketService {
    fn ticket_handoff(&self, ticket_ref: &str) -> Result<TicketHandoff, TicketError> {
        ticket_handoff_from_record(self.backend.show_unprojected(ticket_ref)?)
    }
}

fn ticket_workflow_instruction() -> FeatureInstructionDeclaration {
    FeatureInstructionDeclaration::new(
        FeatureInstructionId::builtin(TICKET_WORKFLOW_INSTRUCTION_ID),
        TICKET_WORKFLOW_PROMPT_REF,
        "Typed Ticket workflow guidance",
    )
    .expect("static Ticket workflow instruction declaration is valid")
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TicketFeatureAccess {
    pub authoring: bool,
    pub thread: bool,
    pub intake: bool,
    pub workflow: bool,
}

impl TicketFeatureAccess {
    pub const fn read_only() -> Self {
        Self {
            authoring: false,
            thread: false,
            intake: false,
            workflow: false,
        }
    }

    pub const fn workspace_authoring() -> Self {
        Self {
            authoring: true,
            thread: true,
            intake: false,
            workflow: false,
        }
    }

    pub const fn intake() -> Self {
        Self {
            authoring: true,
            thread: true,
            intake: true,
            workflow: false,
        }
    }

    pub const fn workflow() -> Self {
        Self {
            authoring: false,
            thread: true,
            intake: false,
            workflow: true,
        }
    }

    pub const fn work_report() -> Self {
        Self {
            authoring: false,
            thread: true,
            intake: false,
            workflow: false,
        }
    }

    pub const fn review() -> Self {
        Self {
            authoring: false,
            thread: false,
            intake: false,
            workflow: false,
        }
    }

    pub fn tool_names(self) -> Vec<&'static str> {
        TICKET_TOOL_NAMES
            .iter()
            .copied()
            .filter(|name| self.allows_tool(name))
            .chain((self.authoring || self.workflow || self.thread).then_some("CompleteTicket"))
            .collect()
    }

    fn allows_tool(self, name: &str) -> bool {
        // Publishing a decision tool is not an authority grant. The injected
        // Workspace transport lets the Backend validate the current assignment
        // or author/orchestrator permission on every decision, in both modes.
        READ_ONLY_TOOL_NAMES.contains(&name)
            || (name == "TicketWorkflowState" && (self.authoring || self.thread || self.workflow))
            || (self.authoring && AUTHORING_TOOL_NAMES.contains(&name))
            || (self.thread && THREAD_TOOL_NAMES.contains(&name))
            || (self.intake && INTAKE_TOOL_NAMES.contains(&name))
            || (self.workflow && WORKFLOW_ADDITIONAL_TOOL_NAMES.contains(&name))
    }
}

const READ_ONLY_TOOL_NAMES: &[&str] = &["QueryTicket", "ShowTicket"];

const AUTHORING_TOOL_NAMES: &[&str] = &[
    "TicketCreate",
    "TicketEditItem",
    "TicketMarkReady",
    "TicketQueue",
    "TicketClose",
    "TicketRelationRecord",
    "TicketRelationRemove",
];

const THREAD_TOOL_NAMES: &[&str] = &["TicketComment"];

const INTAKE_TOOL_NAMES: &[&str] = &["TicketIntakeReady"];

#[cfg(test)]
const WORKSPACE_AUTHORING_TOOL_NAMES: &[&str] = &[
    "TicketCreate",
    "TicketEditItem",
    "QueryTicket",
    "ShowTicket",
    "TicketComment",
    "TicketMarkReady",
    "TicketQueue",
    "TicketWorkflowState",
    "TicketClose",
    "TicketRelationRecord",
    "TicketRelationRemove",
    "CompleteTicket",
];

#[cfg(test)]
const WORKFLOW_TOOL_NAMES: &[&str] = &[
    "QueryTicket",
    "ShowTicket",
    "TicketComment",
    "TicketWorkflowState",
    "TicketClose",
    "TicketDependencyCheck",
    "TicketRelationRecord",
    "TicketRelationRemove",
    "TicketOrchestrationPlanRecord",
    "TicketOrchestrationPlanQuery",
    "CompleteTicket",
];

const WORKFLOW_ADDITIONAL_TOOL_NAMES: &[&str] = &[
    "TicketWorkflowState",
    "TicketClose",
    "TicketDependencyCheck",
    "TicketRelationRecord",
    "TicketRelationRemove",
    "TicketOrchestrationPlanRecord",
    "TicketOrchestrationPlanQuery",
];

#[derive(Clone, Debug)]
pub struct TicketFeature {
    workspace_client: Arc<dyn WorkspaceClient>,
    access: TicketFeatureAccess,
}

impl TicketFeature {
    pub fn new(workspace_client: Arc<dyn WorkspaceClient>, access: TicketFeatureAccess) -> Self {
        Self {
            workspace_client,
            access,
        }
    }

    pub fn access(&self) -> TicketFeatureAccess {
        self.access
    }

    fn enabled_tool_names(&self) -> Vec<&'static str> {
        self.access.tool_names()
    }

    fn workspace_client(&self) -> Arc<dyn WorkspaceClient> {
        self.workspace_client.clone()
    }
}

impl FeatureModule for TicketFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        let mut descriptor = FeatureDescriptor::builtin(FEATURE_ID, FEATURE_NAME)
            .with_description(FEATURE_DESCRIPTION)
            .with_instruction(ticket_workflow_instruction())
            .with_provided_service(ServiceDeclaration::new(
                ServiceId::builtin(TICKET_SERVICE_ID),
                TICKET_SERVICE_VERSION,
                "Current typed Ticket authority",
            ));
        let enabled_tool_names = self.enabled_tool_names();
        for name in enabled_tool_names {
            descriptor = descriptor.with_tool(ToolDeclaration::new(
                name,
                workspace_ticket_tool_description(name),
            ));
        }
        descriptor
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        let workspace_client = self.workspace_client();
        let ticket_service: Arc<dyn TicketService> = Arc::new(WorkspaceTicketService {
            backend: WorkspaceHttpTicketBackend::new(workspace_client.clone()),
        });
        context.services().provide(
            ServiceDeclaration::new(
                ServiceId::builtin(TICKET_SERVICE_ID),
                TICKET_SERVICE_VERSION,
                "Current typed Ticket authority",
            ),
            ticket_service,
        )?;
        context
            .instructions()
            .register(FeatureInstructionContribution::new(
                ticket_workflow_instruction(),
            ))?;
        let mut tools = context.tools();
        for definition in enabled_ticket_definitions(workspace_client, self.access) {
            let (meta, _) = definition();
            let name = meta.name.clone();
            tools.register(ToolContribution::new(name, definition))?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct WorkspaceHttpTicketBackend {
    client: Arc<dyn WorkspaceClient>,
}

impl WorkspaceHttpTicketBackend {
    fn new(client: Arc<dyn WorkspaceClient>) -> Self {
        Self { client }
    }

    fn invoke(
        &self,
        operation: TicketBackendOperation,
    ) -> TicketResult<TicketBackendOperationResult> {
        let client = self.client.clone();
        let workspace_id = self.client.workspace_id().unwrap_or_default().to_string();
        if tokio::runtime::Handle::try_current().is_ok() {
            return std::thread::spawn(move || {
                Self::invoke_client(client, workspace_id, operation)
            })
            .join()
            .map_err(|_| {
                TicketError::Conflict("ticket REST request thread panicked".to_string())
            })?;
        }
        Self::invoke_client(client, workspace_id, operation)
    }

    fn show_unprojected(&self, ticket_ref: &str) -> TicketResult<Ticket> {
        let client = self.client.clone();
        let workspace_id = self.client.workspace_id().unwrap_or_default().to_string();
        let ticket_path = Self::ticket_path(&TicketIdOrSlug::from(ticket_ref));
        let request = move || {
            Self::request_unprojected(
                client,
                WorkspaceRequestMethod::Get,
                format!("/api/w/{workspace_id}/tickets/{ticket_path}/record"),
                None,
            )
        };
        if tokio::runtime::Handle::try_current().is_ok() {
            return std::thread::spawn(request).join().map_err(|_| {
                TicketError::Conflict("ticket REST request thread panicked".to_string())
            })?;
        }
        request()
    }

    fn ticket_path(id: &TicketIdOrSlug) -> String {
        let value = match id {
            TicketIdOrSlug::Id(value)
            | TicketIdOrSlug::Slug(value)
            | TicketIdOrSlug::Query(value) => value,
        };
        let mut encoded = String::with_capacity(value.len());
        for byte in value.bytes() {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                encoded.push(byte as char);
            } else {
                use std::fmt::Write as _;
                let _ = write!(encoded, "%{byte:02X}");
            }
        }
        encoded
    }

    fn request<T: serde::de::DeserializeOwned>(
        client: Arc<dyn WorkspaceClient>,
        method: WorkspaceRequestMethod,
        endpoint: String,
        body: Option<serde_json::Value>,
    ) -> TicketResult<T> {
        let mut value = Self::request_value(client, method, endpoint, body)?;
        Self::canonicalize_ticket_references(&mut value);
        serde_json::from_value(value)
            .map_err(|error| TicketError::Conflict(format!("decode ticket REST response: {error}")))
    }

    fn request_unprojected<T: serde::de::DeserializeOwned>(
        client: Arc<dyn WorkspaceClient>,
        method: WorkspaceRequestMethod,
        endpoint: String,
        body: Option<serde_json::Value>,
    ) -> TicketResult<T> {
        let value = Self::request_value(client, method, endpoint, body)?;
        serde_json::from_value(value)
            .map_err(|error| TicketError::Conflict(format!("decode ticket REST response: {error}")))
    }

    fn request_value(
        client: Arc<dyn WorkspaceClient>,
        method: WorkspaceRequestMethod,
        endpoint: String,
        body: Option<serde_json::Value>,
    ) -> TicketResult<Value> {
        let request = match body {
            Some(body) => WorkspaceRequest::json(method, endpoint, body.to_string()),
            None if method == WorkspaceRequestMethod::Get => WorkspaceRequest::get(endpoint),
            None => WorkspaceRequest {
                method,
                path: endpoint,
                body: None,
            },
        };
        let response = client.execute(request).map_err(|error| {
            TicketError::Conflict(format!("ticket REST request failed: {error}"))
        })?;
        if !response.is_success() {
            return Err(TicketError::Conflict(format!(
                "ticket REST API request failed with HTTP status {}",
                response.status
            )));
        }
        serde_json::from_str(&response.body)
            .map_err(|error| TicketError::Conflict(format!("decode ticket REST response: {error}")))
    }

    fn canonicalize_ticket_references(value: &mut Value) {
        match value {
            Value::Array(values) => {
                for value in values {
                    Self::canonicalize_ticket_references(value);
                }
            }
            Value::Object(object) => {
                for value in object.values_mut() {
                    Self::canonicalize_ticket_references(value);
                }
                if let Some(resource_key) = object
                    .get("resource_key")
                    .and_then(Value::as_str)
                    .filter(|key| is_canonical_ticket_resource_key(key))
                    .map(ToOwned::to_owned)
                    && object.contains_key("id")
                {
                    object.insert("id".to_string(), Value::String(resource_key));
                }
            }
            _ => {}
        }
    }

    fn resolve_ticket_resource_key(
        client: Arc<dyn WorkspaceClient>,
        base: &str,
        reference: &TicketIdOrSlug,
    ) -> TicketResult<String> {
        let response: Value = Self::request(
            client,
            WorkspaceRequestMethod::Get,
            format!("{base}/{}", Self::ticket_path(reference)),
            None,
        )?;
        response
            .get("resource_key")
            .or_else(|| {
                response
                    .get("meta")
                    .and_then(|meta| meta.get("resource_key"))
            })
            .and_then(Value::as_str)
            .filter(|key| is_canonical_ticket_resource_key(key))
            .map(ToOwned::to_owned)
            .ok_or_else(|| TicketError::Conflict("required Ticket key is unavailable".to_string()))
    }

    fn request_unit(
        client: Arc<dyn WorkspaceClient>,
        method: WorkspaceRequestMethod,
        endpoint: String,
        body: Option<serde_json::Value>,
    ) -> TicketResult<TicketBackendOperationResult> {
        let request = match body {
            Some(body) => WorkspaceRequest::json(method, endpoint, body.to_string()),
            None => WorkspaceRequest {
                method,
                path: endpoint,
                body: None,
            },
        };
        let response = client.execute(request).map_err(|error| {
            TicketError::Conflict(format!("ticket REST request failed: {error}"))
        })?;
        if !response.is_success() {
            return Err(TicketError::Conflict(format!(
                "ticket REST API request failed with HTTP status {}",
                response.status
            )));
        }
        Ok(TicketBackendOperationResult::Unit)
    }

    fn legacy_state_request(
        client: Arc<dyn WorkspaceClient>,
        base: &str,
        id: &TicketIdOrSlug,
        change: TicketStateChange,
    ) -> TicketResult<Value> {
        // Preserve the caller's original `from`: refreshing it from the snapshot
        // would turn a stale transition into a successful write.
        let expected_state = TicketWorkflowState::parse(&change.from).ok_or_else(|| {
            TicketError::Conflict(format!("invalid Ticket from state: {}", change.from))
        })?;
        let state = TicketWorkflowState::parse(&change.to).ok_or_else(|| {
            TicketError::Conflict(format!("invalid Ticket to state: {}", change.to))
        })?;
        let reason = [change.reason.as_str(), change.body.as_str()]
            .into_iter()
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        if reason.is_empty() {
            return Err(TicketError::Conflict(
                "Ticket state decision requires a reason".into(),
            ));
        }
        let snapshot: Ticket = Self::request_unprojected(
            client,
            WorkspaceRequestMethod::Get,
            format!("{base}/{}/record", Self::ticket_path(id)),
            None,
        )?;
        serde_json::to_value(ticket::TicketStateUpdate {
            operation_key: uuid::Uuid::now_v7().to_string(),
            expected_content_digest: ticket::ticket_content_digest(&snapshot),
            expected_state,
            state,
            reason: reason.into(),
            references: change.references,
            author: change.author,
        })
        .map_err(|error| TicketError::Conflict(format!("serialize Ticket state decision: {error}")))
    }

    fn invoke_client(
        client: Arc<dyn WorkspaceClient>,
        workspace_id: String,
        operation: TicketBackendOperation,
    ) -> TicketResult<TicketBackendOperationResult> {
        let base = format!("/api/w/{workspace_id}/tickets");
        match operation {
            TicketBackendOperation::DefaultIntakeReadyStateChangeBody { from } => {
                let value = Self::request::<String>(
                    client,
                    WorkspaceRequestMethod::Post,
                    format!("{base}/default-intake-ready-body"),
                    Some(serde_json::json!({ "from": from })),
                )?;
                Ok(TicketBackendOperationResult::Text(value))
            }
            TicketBackendOperation::List { filter } => {
                let state = match filter.state {
                    ticket::TicketStateSelector::Active => "active".to_string(),
                    ticket::TicketStateSelector::All => "all".to_string(),
                    ticket::TicketStateSelector::States(states) => states
                        .into_iter()
                        .map(|state| state.as_str().to_string())
                        .collect::<Vec<_>>()
                        .join(","),
                };
                let tickets = Self::request(
                    client,
                    WorkspaceRequestMethod::Get,
                    format!("{base}/search?state={state}"),
                    None,
                )?;
                Ok(TicketBackendOperationResult::Tickets(tickets))
            }
            TicketBackendOperation::Show { id } => {
                let ticket: Ticket = Self::request(
                    client,
                    WorkspaceRequestMethod::Get,
                    format!("{base}/{}/record", Self::ticket_path(&id)),
                    None,
                )?;
                if !ticket
                    .meta
                    .resource_key
                    .as_deref()
                    .is_some_and(is_canonical_ticket_resource_key)
                {
                    return Err(TicketError::Conflict(
                        "required Ticket key is unavailable".to_string(),
                    ));
                }
                Ok(TicketBackendOperationResult::Ticket(ticket))
            }
            TicketBackendOperation::Create { input } => {
                let ticket = Self::request(
                    client,
                    WorkspaceRequestMethod::Post,
                    base,
                    Some(serde_json::to_value(input).map_err(|error| {
                        TicketError::Conflict(format!("serialize Ticket create: {error}"))
                    })?),
                )?;
                Ok(TicketBackendOperationResult::TicketRef(ticket))
            }
            TicketBackendOperation::EditItem { id, edit } => {
                let ticket = Self::request(
                    client,
                    WorkspaceRequestMethod::Patch,
                    format!("{base}/{}/item", Self::ticket_path(&id)),
                    Some(serde_json::to_value(edit).map_err(|error| {
                        TicketError::Conflict(format!("serialize Ticket edit: {error}"))
                    })?),
                )?;
                Ok(TicketBackendOperationResult::Ticket(ticket))
            }
            TicketBackendOperation::DependencyCheck { id } => {
                let check = Self::request(
                    client,
                    WorkspaceRequestMethod::Get,
                    format!("{base}/{}/dependency-check", Self::ticket_path(&id)),
                    None,
                )?;
                Ok(TicketBackendOperationResult::DependencyCheck(check))
            }
            TicketBackendOperation::AddEvent { id, event } => Self::request_unit(
                client,
                WorkspaceRequestMethod::Post,
                format!("{base}/{}/thread-events", Self::ticket_path(&id)),
                Some(serde_json::to_value(event).map_err(|error| {
                    TicketError::Conflict(format!("serialize Ticket event: {error}"))
                })?),
            ),
            TicketBackendOperation::AddStateChanged { id, change } => {
                let body = Self::legacy_state_request(client.clone(), &base, &id, change)?;
                Self::request_unit(
                    client,
                    WorkspaceRequestMethod::Post,
                    format!("{base}/{}/state-changes", Self::ticket_path(&id)),
                    Some(body),
                )
            }
            TicketBackendOperation::AddIntakeSummary { id, summary } => Self::request_unit(
                client,
                WorkspaceRequestMethod::Post,
                format!("{base}/{}/intake-summaries", Self::ticket_path(&id)),
                Some(serde_json::to_value(summary).map_err(|error| {
                    TicketError::Conflict(format!("serialize Ticket intake summary: {error}"))
                })?),
            ),
            TicketBackendOperation::SetStateField { id, field, change } => {
                if field != "workflow_state" && field != "state" {
                    return Err(TicketError::Conflict(format!(
                        "unsupported Ticket state field: {field}"
                    )));
                }
                let body = Self::legacy_state_request(client.clone(), &base, &id, change)?;
                Self::request_unit(
                    client,
                    WorkspaceRequestMethod::Post,
                    format!(
                        "{base}/{}/state-fields/{}",
                        Self::ticket_path(&id),
                        Self::ticket_path(&TicketIdOrSlug::Query(field))
                    ),
                    Some(body),
                )
            }
            TicketBackendOperation::SetWorkflowState { id, change } => {
                let body = Self::legacy_state_request(client.clone(), &base, &id, change)?;
                Self::request_unit(
                    client,
                    WorkspaceRequestMethod::Post,
                    format!("{base}/{}/workflow-state", Self::ticket_path(&id)),
                    Some(body),
                )
            }
            TicketBackendOperation::UpdateState { ticket, request } => Self::request(
                client,
                WorkspaceRequestMethod::Post,
                format!(
                    "{base}/{}/state-update",
                    Self::ticket_path(&TicketIdOrSlug::from(ticket.as_str()))
                ),
                Some(serde_json::to_value(request).map_err(|error| {
                    TicketError::Conflict(format!("serialize Ticket state update: {error}"))
                })?),
            )
            .map(TicketBackendOperationResult::Ticket),
            TicketBackendOperation::Complete { ticket, request } => Self::request(
                client,
                WorkspaceRequestMethod::Post,
                format!(
                    "{base}/{}/complete",
                    Self::ticket_path(&TicketIdOrSlug::from(ticket.as_str()))
                ),
                Some(serde_json::to_value(request).map_err(|error| {
                    TicketError::Conflict(format!("serialize Ticket completion: {error}"))
                })?),
            )
            .map(TicketBackendOperationResult::Ticket),
            TicketBackendOperation::MarkReady { id, request } => Self::request(
                client,
                WorkspaceRequestMethod::Post,
                format!("{base}/{}/workflow/mark-ready", Self::ticket_path(&id)),
                Some(serde_json::to_value(request).map_err(|error| {
                    TicketError::Conflict(format!("serialize Ticket mark-ready request: {error}"))
                })?),
            )
            .map(TicketBackendOperationResult::Ticket),
            TicketBackendOperation::QueueReady { id, .. } => Self::request(
                client,
                WorkspaceRequestMethod::Post,
                format!("{base}/{}/workflow/queue", Self::ticket_path(&id)),
                None,
            )
            .map(TicketBackendOperationResult::QueueOutcome),
            TicketBackendOperation::Close { id, resolution } => {
                if resolution.as_str().trim().is_empty() {
                    return Err(TicketError::Conflict(
                        "Ticket close requires a resolution".into(),
                    ));
                }
                let snapshot: Ticket = Self::request_unprojected(
                    client.clone(),
                    WorkspaceRequestMethod::Get,
                    format!("{base}/{}/record", Self::ticket_path(&id)),
                    None,
                )?;
                let body = serde_json::to_value(ticket::TicketCompletion {
                    operation_key: uuid::Uuid::now_v7().to_string(),
                    expected_content_digest: ticket::ticket_content_digest(&snapshot),
                    expected_state: snapshot.meta.workflow_state,
                    reason: resolution.as_str().to_owned(),
                    references: Vec::new(),
                    author: None,
                })
                .map_err(|error| {
                    TicketError::Conflict(format!("serialize Ticket close: {error}"))
                })?;
                Self::request_unit(
                    client,
                    WorkspaceRequestMethod::Post,
                    format!("{base}/{}/workflow/close", Self::ticket_path(&id)),
                    Some(body),
                )
            }
            TicketBackendOperation::AddTicketRelation { id, relation } => {
                let source_resource_key =
                    Self::resolve_ticket_resource_key(client.clone(), &base, &id)?;
                let target_resource_key = Self::resolve_ticket_resource_key(
                    client.clone(),
                    &base,
                    &TicketIdOrSlug::Id(relation.target.clone()),
                )?;
                let mut relation: TicketRelation = Self::request(
                    client,
                    WorkspaceRequestMethod::Post,
                    format!("{base}/{}/relations", Self::ticket_path(&id)),
                    Some(serde_json::to_value(relation).map_err(|error| {
                        TicketError::Conflict(format!("serialize Ticket relation: {error}"))
                    })?),
                )?;
                relation.ticket_id = source_resource_key;
                relation.target = target_resource_key;
                relation.author = "workspace".to_string();
                Ok(TicketBackendOperationResult::Relation(relation))
            }
            TicketBackendOperation::RemoveTicketRelation { id, kind, target } => {
                let source_resource_key =
                    Self::resolve_ticket_resource_key(client.clone(), &base, &id)?;
                let target_resource_key =
                    Self::resolve_ticket_resource_key(client.clone(), &base, &target)?;
                let target = match target {
                    TicketIdOrSlug::Id(value)
                    | TicketIdOrSlug::Slug(value)
                    | TicketIdOrSlug::Query(value) => value,
                };
                let mut relation: TicketRelation = Self::request(
                    client,
                    WorkspaceRequestMethod::Delete,
                    format!("{base}/{}/relations", Self::ticket_path(&id)),
                    Some(serde_json::json!({ "kind": kind, "target": target })),
                )?;
                relation.ticket_id = source_resource_key;
                relation.target = target_resource_key;
                relation.author = "workspace".to_string();
                Ok(TicketBackendOperationResult::Relation(relation))
            }
            TicketBackendOperation::QueryTicketRelations { ticket, kind } => {
                let relations = Self::request(
                    client,
                    WorkspaceRequestMethod::Post,
                    format!("{base}/relations/search"),
                    Some(serde_json::json!({ "ticket": ticket, "kind": kind })),
                )?;
                Ok(TicketBackendOperationResult::Relations(relations))
            }
            TicketBackendOperation::RelationView { id } => {
                let view = Self::request(
                    client,
                    WorkspaceRequestMethod::Get,
                    format!("{base}/{}/relation-view", Self::ticket_path(&id)),
                    None,
                )?;
                Ok(TicketBackendOperationResult::RelationView(view))
            }
            TicketBackendOperation::AddOrchestrationPlanRecord { id, record } => {
                let record = Self::request(
                    client,
                    WorkspaceRequestMethod::Post,
                    format!("{base}/{}/orchestration-plans", Self::ticket_path(&id)),
                    Some(serde_json::to_value(record).map_err(|error| {
                        TicketError::Conflict(format!(
                            "serialize Ticket orchestration plan: {error}"
                        ))
                    })?),
                )?;
                Ok(TicketBackendOperationResult::OrchestrationPlanRecord(
                    record,
                ))
            }
            TicketBackendOperation::QueryOrchestrationPlanRecords { ticket, kind } => {
                let records = Self::request(
                    client,
                    WorkspaceRequestMethod::Post,
                    format!("{base}/orchestration-plans/search"),
                    Some(serde_json::json!({ "ticket": ticket, "kind": kind })),
                )?;
                Ok(TicketBackendOperationResult::OrchestrationPlanRecords(
                    records,
                ))
            }
            TicketBackendOperation::Doctor => {
                let report = Self::request(
                    client,
                    WorkspaceRequestMethod::Get,
                    format!("{base}/doctor"),
                    None,
                )?;
                Ok(TicketBackendOperationResult::DoctorReport(report))
            }
        }
    }
}

macro_rules! expect_ticket_result {
    ($expr:expr, $variant:path) => {
        match $expr? {
            $variant(value) => Ok(value),
            other => Err(TicketError::Conflict(format!(
                "unexpected ticket backend response: {other:?}"
            ))),
        }
    };
}

impl TicketBackend for WorkspaceHttpTicketBackend {
    fn default_intake_ready_state_change_body(&self, from: &str) -> String {
        match self.invoke(TicketBackendOperation::DefaultIntakeReadyStateChangeBody {
            from: from.to_string(),
        }) {
            Ok(TicketBackendOperationResult::Text(value)) => value,
            Ok(other) => format!("unexpected ticket backend response: {other:?}"),
            Err(error) => error.to_string(),
        }
    }

    fn list(&self, filter: TicketListQuery) -> TicketResult<Vec<TicketSummary>> {
        expect_ticket_result!(
            self.invoke(TicketBackendOperation::List { filter }),
            TicketBackendOperationResult::Tickets
        )
    }

    fn show(&self, id: TicketIdOrSlug) -> TicketResult<Ticket> {
        expect_ticket_result!(
            self.invoke(TicketBackendOperation::Show { id }),
            TicketBackendOperationResult::Ticket
        )
    }

    fn create(&self, input: NewTicket) -> TicketResult<TicketRef> {
        expect_ticket_result!(
            self.invoke(TicketBackendOperation::Create { input }),
            TicketBackendOperationResult::TicketRef
        )
    }

    fn edit_item(&self, id: TicketIdOrSlug, edit: ticket::TicketItemEdit) -> TicketResult<Ticket> {
        expect_ticket_result!(
            self.invoke(TicketBackendOperation::EditItem { id, edit }),
            TicketBackendOperationResult::Ticket
        )
    }

    fn dependency_check(&self, id: TicketIdOrSlug) -> TicketResult<ticket::TicketDependencyCheck> {
        expect_ticket_result!(
            self.invoke(TicketBackendOperation::DependencyCheck { id }),
            TicketBackendOperationResult::DependencyCheck
        )
    }

    fn add_event(&self, id: TicketIdOrSlug, event: NewTicketEvent) -> TicketResult<()> {
        match self.invoke(TicketBackendOperation::AddEvent { id, event })? {
            TicketBackendOperationResult::Unit => Ok(()),
            other => Err(TicketError::Conflict(format!(
                "unexpected ticket backend response: {other:?}"
            ))),
        }
    }

    fn add_state_changed(&self, id: TicketIdOrSlug, change: TicketStateChange) -> TicketResult<()> {
        match self.invoke(TicketBackendOperation::AddStateChanged { id, change })? {
            TicketBackendOperationResult::Unit => Ok(()),
            other => Err(TicketError::Conflict(format!(
                "unexpected ticket backend response: {other:?}"
            ))),
        }
    }

    fn add_intake_summary(
        &self,
        id: TicketIdOrSlug,
        summary: TicketIntakeSummary,
    ) -> TicketResult<()> {
        match self.invoke(TicketBackendOperation::AddIntakeSummary { id, summary })? {
            TicketBackendOperationResult::Unit => Ok(()),
            other => Err(TicketError::Conflict(format!(
                "unexpected ticket backend response: {other:?}"
            ))),
        }
    }

    fn set_state_field(
        &self,
        id: TicketIdOrSlug,
        field: &str,
        change: TicketStateChange,
    ) -> TicketResult<()> {
        match self.invoke(TicketBackendOperation::SetStateField {
            id,
            field: field.to_string(),
            change,
        })? {
            TicketBackendOperationResult::Unit => Ok(()),
            other => Err(TicketError::Conflict(format!(
                "unexpected ticket backend response: {other:?}"
            ))),
        }
    }

    fn set_workflow_state(
        &self,
        id: TicketIdOrSlug,
        change: TicketStateChange,
    ) -> TicketResult<()> {
        match self.invoke(TicketBackendOperation::SetWorkflowState { id, change })? {
            TicketBackendOperationResult::Unit => Ok(()),
            other => Err(TicketError::Conflict(format!(
                "unexpected ticket backend response: {other:?}"
            ))),
        }
    }

    fn update_state(
        &self,
        ticket: &str,
        request: ticket::TicketStateUpdate,
    ) -> TicketResult<Ticket> {
        expect_ticket_result!(
            self.invoke(TicketBackendOperation::UpdateState {
                ticket: ticket.to_string(),
                request
            }),
            TicketBackendOperationResult::Ticket
        )
    }

    fn mark_ready(
        &self,
        id: TicketIdOrSlug,
        request: ticket::TicketMarkReady,
    ) -> TicketResult<Ticket> {
        expect_ticket_result!(
            self.invoke(TicketBackendOperation::MarkReady { id, request }),
            TicketBackendOperationResult::Ticket
        )
    }

    fn queue_ready(
        &self,
        id: TicketIdOrSlug,
        queued_by: &str,
    ) -> TicketResult<ticket::TicketQueueOutcome> {
        expect_ticket_result!(
            self.invoke(TicketBackendOperation::QueueReady {
                id,
                queued_by: queued_by.to_string(),
            }),
            TicketBackendOperationResult::QueueOutcome
        )
    }

    fn close(&self, id: TicketIdOrSlug, resolution: MarkdownText) -> TicketResult<()> {
        match self.invoke(TicketBackendOperation::Close { id, resolution })? {
            TicketBackendOperationResult::Unit => Ok(()),
            other => Err(TicketError::Conflict(format!(
                "unexpected ticket backend response: {other:?}"
            ))),
        }
    }

    fn add_ticket_relation(
        &self,
        id: TicketIdOrSlug,
        relation: NewTicketRelation,
    ) -> TicketResult<TicketRelation> {
        expect_ticket_result!(
            self.invoke(TicketBackendOperation::AddTicketRelation { id, relation }),
            TicketBackendOperationResult::Relation
        )
    }

    fn remove_ticket_relation(
        &self,
        id: TicketIdOrSlug,
        kind: TicketRelationKind,
        target: TicketIdOrSlug,
    ) -> TicketResult<TicketRelation> {
        expect_ticket_result!(
            self.invoke(TicketBackendOperation::RemoveTicketRelation { id, kind, target }),
            TicketBackendOperationResult::Relation
        )
    }

    fn query_ticket_relations(
        &self,
        ticket: Option<TicketIdOrSlug>,
        kind: Option<TicketRelationKind>,
    ) -> TicketResult<Vec<TicketRelation>> {
        expect_ticket_result!(
            self.invoke(TicketBackendOperation::QueryTicketRelations { ticket, kind }),
            TicketBackendOperationResult::Relations
        )
    }

    fn relation_view(&self, id: TicketIdOrSlug) -> TicketResult<TicketRelationView> {
        expect_ticket_result!(
            self.invoke(TicketBackendOperation::RelationView { id }),
            TicketBackendOperationResult::RelationView
        )
    }

    fn add_orchestration_plan_record(
        &self,
        id: TicketIdOrSlug,
        record: NewOrchestrationPlanRecord,
    ) -> TicketResult<OrchestrationPlanRecord> {
        expect_ticket_result!(
            self.invoke(TicketBackendOperation::AddOrchestrationPlanRecord { id, record }),
            TicketBackendOperationResult::OrchestrationPlanRecord
        )
    }

    fn query_orchestration_plan_records(
        &self,
        ticket: Option<TicketIdOrSlug>,
        kind: Option<OrchestrationPlanKind>,
    ) -> TicketResult<Vec<OrchestrationPlanRecord>> {
        expect_ticket_result!(
            self.invoke(TicketBackendOperation::QueryOrchestrationPlanRecords { ticket, kind }),
            TicketBackendOperationResult::OrchestrationPlanRecords
        )
    }

    fn doctor(&self) -> TicketResult<TicketDoctorReport> {
        expect_ticket_result!(
            self.invoke(TicketBackendOperation::Doctor),
            TicketBackendOperationResult::DoctorReport
        )
    }
}

const TICKET_COLLECTION_INTERFACE: &str = "yoi.ticket/collection/v1";
const TICKET_ITEM_INTERFACE: &str = "yoi.ticket/item/v1";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NativeTicketSurface {
    Collection,
    Item,
}

#[derive(Clone, Copy, Debug)]
struct NativeTicketOperation {
    operation: &'static str,
    surface: NativeTicketSurface,
    identity_field: Option<&'static str>,
    mutating: bool,
}

fn native_ticket_operation(tool_name: &str) -> Option<NativeTicketOperation> {
    let operation = match tool_name {
        "QueryTicket" => NativeTicketOperation {
            operation: "query",
            surface: NativeTicketSurface::Collection,
            identity_field: None,
            mutating: false,
        },
        "TicketCreate" => NativeTicketOperation {
            operation: "create",
            surface: NativeTicketSurface::Collection,
            identity_field: None,
            mutating: true,
        },
        "ShowTicket" => NativeTicketOperation {
            operation: "read",
            surface: NativeTicketSurface::Item,
            identity_field: Some("id"),
            mutating: false,
        },
        "TicketEditItem" => NativeTicketOperation {
            operation: "edit",
            surface: NativeTicketSurface::Item,
            identity_field: Some("ticket"),
            mutating: true,
        },
        "TicketComment" => NativeTicketOperation {
            operation: "comment",
            surface: NativeTicketSurface::Item,
            identity_field: Some("ticket"),
            mutating: true,
        },
        "TicketMarkReady" => NativeTicketOperation {
            operation: "mark_ready",
            surface: NativeTicketSurface::Item,
            identity_field: Some("ticket"),
            mutating: true,
        },
        "TicketIntakeReady" => NativeTicketOperation {
            operation: "intake_ready",
            surface: NativeTicketSurface::Item,
            identity_field: Some("ticket"),
            mutating: true,
        },
        "TicketQueue" => NativeTicketOperation {
            operation: "queue",
            surface: NativeTicketSurface::Item,
            identity_field: Some("ticket"),
            mutating: true,
        },
        "TicketWorkflowState" => NativeTicketOperation {
            operation: "transition",
            surface: NativeTicketSurface::Item,
            identity_field: Some("ticket"),
            mutating: true,
        },
        "CompleteTicket" => NativeTicketOperation {
            operation: "complete",
            surface: NativeTicketSurface::Item,
            identity_field: Some("ticket"),
            mutating: true,
        },
        "TicketClose" => NativeTicketOperation {
            operation: "close",
            surface: NativeTicketSurface::Item,
            identity_field: Some("ticket"),
            mutating: true,
        },
        "TicketDependencyCheck" => NativeTicketOperation {
            operation: "dependency_check",
            surface: NativeTicketSurface::Item,
            identity_field: Some("ticket"),
            mutating: false,
        },
        "TicketRelationRecord" => NativeTicketOperation {
            operation: "record_relation",
            surface: NativeTicketSurface::Item,
            identity_field: Some("ticket"),
            mutating: true,
        },
        "TicketRelationRemove" => NativeTicketOperation {
            operation: "remove_relation",
            surface: NativeTicketSurface::Item,
            identity_field: Some("ticket"),
            mutating: true,
        },
        "TicketOrchestrationPlanRecord" => NativeTicketOperation {
            operation: "record_orchestration_plan",
            surface: NativeTicketSurface::Item,
            identity_field: Some("ticket"),
            mutating: true,
        },
        "TicketOrchestrationPlanQuery" => NativeTicketOperation {
            operation: "query_orchestration_plans",
            surface: NativeTicketSurface::Item,
            identity_field: Some("ticket"),
            mutating: false,
        },
        _ => return None,
    };
    Some(operation)
}

#[derive(Clone)]
struct NativeTicketTool {
    name: String,
    schema: Value,
    tool: Arc<dyn Tool>,
    projection: NativeTicketOperation,
    description: String,
}

pub(crate) fn enabled_ticket_definitions(
    client: Arc<dyn WorkspaceClient>,
    access: TicketFeatureAccess,
) -> Vec<ToolDefinition> {
    let backend = TicketToolBackend::new(WorkspaceHttpTicketBackend::new(client.clone()));
    let allowed = access.tool_names();
    let mut definitions: Vec<ToolDefinition> = ticket_tools(backend)
        .into_iter()
        .filter_map(|definition| {
            let (meta, _) = definition();
            if !allowed.iter().any(|allowed| *allowed == meta.name) {
                return None;
            }
            Some(match meta.name.as_str() {
                "QueryTicket" => {
                    workspace_ticket_read_definition(client.clone(), WorkspaceTicketReadKind::Query)
                }
                "ShowTicket" => {
                    workspace_ticket_read_definition(client.clone(), WorkspaceTicketReadKind::Show)
                }
                "TicketClose" => workspace_ticket_decision_definition(
                    client.clone(),
                    WorkspaceTicketDecisionKind::Close,
                ),
                "TicketWorkflowState" => workspace_ticket_decision_definition(
                    client.clone(),
                    WorkspaceTicketDecisionKind::UpdateState,
                ),
                _ => definition,
            })
        })
        .collect();
    if allowed.contains(&"CompleteTicket") {
        definitions.push(workspace_ticket_decision_definition(
            client,
            WorkspaceTicketDecisionKind::Complete,
        ));
    }
    definitions
}

fn native_ticket_tools(
    client: Arc<dyn WorkspaceClient>,
    access: TicketFeatureAccess,
) -> Result<Vec<NativeTicketTool>, WipMountError> {
    enabled_ticket_definitions(client, access)
        .into_iter()
        .map(|definition| {
            let (meta, tool) = definition();
            let projection = native_ticket_operation(&meta.name).ok_or_else(|| {
                WipMountError::InvalidProjection {
                    route: "/tickets".into(),
                    message: format!(
                        "enabled Ticket tool `{}` has no native projection",
                        meta.name
                    ),
                }
            })?;
            jsonschema::validator_for(&meta.input_schema).map_err(|error| {
                WipMountError::InvalidProjection {
                    route: "/tickets".into(),
                    message: format!("Ticket tool schema cannot be retained: {error}"),
                }
            })?;
            Ok(NativeTicketTool {
                name: meta.name,
                schema: meta.input_schema,
                tool,
                projection,
                description: meta.description,
            })
        })
        .collect()
}

#[derive(Default)]
struct TicketObservationState {
    content_digests: HashMap<String, String>,
    aliases_by_canonical: HashMap<String, BTreeSet<String>>,
}

type TicketObservations = Arc<Mutex<TicketObservationState>>;

/// Mount the enabled Ticket Feature surface as native collection and route-bound
/// item objects. Only tools already enabled for this Worker are projected and
/// claimed from the WIP compatibility surface.
pub fn mount_workspace_http_ticket_wip(
    registry: &mut WipMountRegistry,
    client: Arc<dyn WorkspaceClient>,
    access: TicketFeatureAccess,
    permissions: Option<ToolPermissionConfig>,
    namespace_route: &WipNamespaceRoute,
) -> Result<(), WipMountError> {
    let collection_route = namespace_route.root().to_string();
    let tools = native_ticket_tools(client, access)?;
    let claimed_tools = tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    let collection_tools = tools
        .iter()
        .filter(|tool| tool.projection.surface == NativeTicketSurface::Collection)
        .cloned()
        .collect::<Vec<_>>();
    let item_tools = tools
        .iter()
        .filter(|tool| tool.projection.surface == NativeTicketSurface::Item)
        .cloned()
        .collect::<Vec<_>>();
    let observations = Arc::new(Mutex::new(TicketObservationState::default()));

    let collection_descriptor = ticket_descriptor(
        &collection_tools,
        "Search and create authoritative Workspace Tickets",
        "Search results remain bounded and carry canonical item paths. Operations are exactly the Ticket tools enabled for this Worker.",
    )?;
    registry.mount(WipProjection {
        route: collection_route.clone(),
        capability: "ticket:collection".into(),
        kind: WipProjectionKind::Native,
        object: Object {
            name: "tickets".into(),
            description: Some(
                "Authoritative Ticket collection through scoped Backend authority".into(),
            ),
            interfaces: vec![crate::wip::root_reference(TICKET_COLLECTION_INTERFACE)],
            r#ref: Some("ticket:collection".into()),
            validator: Some(route_validator(&collection_route, "collection")),
        },
        interface: crate::wip::root_reference(TICKET_COLLECTION_INTERFACE),
        interface_validator: Some(descriptor_validator(&collection_descriptor)),
        descriptor: collection_descriptor,
        handler: Arc::new(TicketCollectionWipHandler {
            tools: operation_map(collection_tools),
            permissions: permissions.clone(),
            collection_route: collection_route.clone(),
            observations: Arc::clone(&observations),
        }),
    })?;

    let item_descriptor = ticket_descriptor(
        &item_tools,
        "Read and operate on the Ticket bound to this object route",
        "The subject Ticket identity comes exclusively from the target route. Separate relation and dependency targets remain explicit operation inputs.",
    )?;
    registry.mount_dynamic(WipDynamicMount {
        collection_route: collection_route.clone(),
        capability: "ticket:item".into(),
        interface: crate::wip::root_reference(TICKET_ITEM_INTERFACE),
        interface_validator: Some(descriptor_validator(&item_descriptor)),
        descriptor: item_descriptor,
        resolver: Arc::new(TicketItemResolver {
            tools: operation_map(item_tools),
            permissions,
            collection_route: collection_route.clone(),
            observations,
        }),
    })?;
    registry.replace_compatibility_tools(&collection_route, claimed_tools)?;
    Ok(())
}

fn operation_map(tools: Vec<NativeTicketTool>) -> HashMap<String, NativeTicketTool> {
    tools
        .into_iter()
        .map(|tool| (tool.projection.operation.to_string(), tool))
        .collect()
}

struct TicketCollectionWipHandler {
    tools: HashMap<String, NativeTicketTool>,
    permissions: Option<ToolPermissionConfig>,
    collection_route: String,
    observations: TicketObservations,
}

#[async_trait]
impl WipOperationHandler for TicketCollectionWipHandler {
    async fn call(
        &self,
        operation: &str,
        arguments: &BTreeMap<String, WipValue>,
        context: WipCallContext,
    ) -> Result<WipOperationOutput, WipOperationError> {
        let tool = self.tools.get(operation).ok_or_else(operation_not_found)?;
        let input = native_ticket_input(arguments, None, tool.projection.identity_field)?;
        let output = execute_native_ticket_tool(tool, &self.permissions, input, context).await?;
        let mut response = ticket_tool_output_json(output)?;
        match operation {
            "query" => add_ticket_paths(&mut response, &self.collection_route),
            "create" => {
                record_ticket_observation(&self.observations, None, &response, true);
                if let Some(reference) = ticket_reference(&response).map(ToOwned::to_owned) {
                    response.as_object_mut().map(|object| {
                        object.insert(
                            "path".into(),
                            Value::String(format!("{}/{}", self.collection_route, reference)),
                        )
                    });
                }
            }
            _ => {}
        }
        Ok(WipOperationOutput::native(
            json_to_wip(&response).map_err(WipOperationError::OutcomeUnknown)?,
        ))
    }
}

struct TicketItemResolver {
    tools: HashMap<String, NativeTicketTool>,
    permissions: Option<ToolPermissionConfig>,
    collection_route: String,
    observations: TicketObservations,
}

impl WipDynamicItemResolver for TicketItemResolver {
    fn resolve(&self, item_reference: &str) -> Option<WipDynamicItem> {
        if !is_ticket_route_reference(item_reference) {
            return None;
        }
        let content_digest = self
            .observations
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .content_digests
            .get(item_reference)
            .cloned();
        let route = format!("{}/{}", self.collection_route, item_reference);
        Some(WipDynamicItem {
            object: Object {
                name: item_reference.into(),
                description: Some("Authoritative Ticket bound to this object route".into()),
                interfaces: vec![crate::wip::root_reference(TICKET_ITEM_INTERFACE)],
                r#ref: Some(format!("ticket:{item_reference}")),
                validator: content_digest
                    .as_deref()
                    .map(|digest| route_validator(&route, digest)),
            },
            handler: Arc::new(TicketItemWipHandler {
                tools: self.tools.clone(),
                permissions: self.permissions.clone(),
                ticket_reference: item_reference.into(),
                observations: Arc::clone(&self.observations),
            }),
        })
    }
}

struct TicketItemWipHandler {
    tools: HashMap<String, NativeTicketTool>,
    permissions: Option<ToolPermissionConfig>,
    ticket_reference: String,
    observations: TicketObservations,
}

#[async_trait]
impl WipOperationHandler for TicketItemWipHandler {
    async fn call(
        &self,
        operation: &str,
        arguments: &BTreeMap<String, WipValue>,
        context: WipCallContext,
    ) -> Result<WipOperationOutput, WipOperationError> {
        let tool = self.tools.get(operation).ok_or_else(operation_not_found)?;
        let input = native_ticket_input(
            arguments,
            Some(&self.ticket_reference),
            tool.projection.identity_field,
        )?;
        let output = execute_native_ticket_tool(tool, &self.permissions, input, context).await?;
        let response = ticket_tool_output_json(output)?;
        record_ticket_observation(
            &self.observations,
            Some(&self.ticket_reference),
            &response,
            tool.projection.mutating,
        );
        if tool.projection.mutating {
            record_affected_ticket_mutations(&self.observations, &self.ticket_reference, &response);
        }
        Ok(WipOperationOutput::native(
            json_to_wip(&response).map_err(WipOperationError::OutcomeUnknown)?,
        ))
    }
}

async fn execute_native_ticket_tool(
    tool: &NativeTicketTool,
    permissions: &Option<ToolPermissionConfig>,
    input: Value,
    context: WipCallContext,
) -> Result<ToolOutput, WipOperationError> {
    let validator = jsonschema::validator_for(&tool.schema).map_err(|error| {
        protocol_failure(
            ProtocolErrorCode::Internal,
            format!("stored Ticket tool schema is invalid: {error}"),
        )
    })?;
    if let Err(error) = validator.validate(&input) {
        return Err(protocol_failure(
            ProtocolErrorCode::InvalidArguments,
            format!("Ticket operation input violates its typed Tool schema: {error}"),
        ));
    }
    authorize_native_ticket(permissions, &tool.name, &input)?;
    let input = serde_json::to_string(&input).map_err(|error| {
        protocol_failure(ProtocolErrorCode::InvalidArguments, error.to_string())
    })?;
    tool.tool
        .execute(&input, context.execution)
        .await
        .map_err(map_ticket_tool_error)
}

fn native_ticket_input(
    arguments: &BTreeMap<String, WipValue>,
    bound_ticket: Option<&str>,
    identity_field: Option<&str>,
) -> Result<Value, WipOperationError> {
    if bound_ticket.is_some() && (arguments.contains_key("ticket") || arguments.contains_key("id"))
    {
        return Err(protocol_failure(
            ProtocolErrorCode::InvalidArguments,
            "the subject Ticket is bound exclusively by the object route",
        ));
    }
    let mut input = serde_json::Map::new();
    for (name, value) in arguments {
        input.insert(
            name.clone(),
            wip_to_json(value).map_err(|message| {
                protocol_failure(ProtocolErrorCode::InvalidArguments, message)
            })?,
        );
    }
    if let (Some(ticket), Some(field)) = (bound_ticket, identity_field) {
        input.insert(field.into(), Value::String(ticket.into()));
    }
    Ok(Value::Object(input))
}

fn authorize_native_ticket(
    permissions: &Option<ToolPermissionConfig>,
    tool_name: &str,
    input: &Value,
) -> Result<(), WipOperationError> {
    let Some(permissions) = permissions else {
        return Ok(());
    };
    match permission_action_for(permissions, tool_name, input) {
        ToolPermissionAction::Allow => Ok(()),
        ToolPermissionAction::Deny => Err(protocol_failure(
            ProtocolErrorCode::PermissionDenied,
            format!("permission denied for projected tool `{tool_name}`"),
        )),
        ToolPermissionAction::Ask => Err(protocol_failure(
            ProtocolErrorCode::PermissionDenied,
            format!(
                "permission approval is unavailable for projected tool `{tool_name}`; denied fail-closed"
            ),
        )),
    }
}

fn map_ticket_tool_error(error: ToolError) -> WipOperationError {
    match error {
        ToolError::InvalidArgument(message) => {
            protocol_failure(ProtocolErrorCode::InvalidArguments, message)
        }
        ToolError::Cancelled(output) => WipOperationError::Cancelled(output),
        ToolError::Interrupted(output) => WipOperationError::Interrupted(output),
        ToolError::StructuredConflict { code, message } => protocol_failure(
            ProtocolErrorCode::InvalidArguments,
            format!("{code}: {message}"),
        ),
        ToolError::ExecutionFailed(message) => {
            if message.contains("HTTP status 401") || message.contains("HTTP status 403") {
                protocol_failure(ProtocolErrorCode::PermissionDenied, message)
            } else if message.contains("HTTP status 404") {
                protocol_failure(ProtocolErrorCode::NotFound, message)
            } else if message.contains("HTTP status 400")
                || message.contains("HTTP status 409")
                || message.contains("HTTP status 422")
            {
                protocol_failure(ProtocolErrorCode::InvalidArguments, message)
            } else {
                WipOperationError::OutcomeUnknown(message)
            }
        }
        ToolError::Internal(message) => WipOperationError::OutcomeUnknown(message),
    }
}

fn ticket_tool_output_json(output: ToolOutput) -> Result<Value, WipOperationError> {
    match output.content {
        Some(content) => serde_json::from_str(&content)
            .map_err(|error| WipOperationError::OutcomeUnknown(error.to_string())),
        None => Ok(json!({"summary": output.summary, "ok": true})),
    }
}

fn add_ticket_paths(response: &mut Value, collection_route: &str) {
    let Some(tickets) = response.get_mut("tickets").and_then(Value::as_array_mut) else {
        return;
    };
    for ticket in tickets {
        let Some(reference) = ticket
            .get("ticket")
            .or_else(|| ticket.get("id"))
            .and_then(Value::as_str)
            .filter(|reference| is_canonical_ticket_resource_key(reference))
            .map(ToOwned::to_owned)
        else {
            continue;
        };
        if let Some(object) = ticket.as_object_mut() {
            object.insert(
                "path".into(),
                Value::String(format!("{collection_route}/{reference}")),
            );
        }
    }
}

fn ticket_reference(response: &Value) -> Option<&str> {
    response
        .get("ticket")
        .or_else(|| response.get("ticket_id"))
        .or_else(|| response.get("id"))
        .or_else(|| response.get("meta").and_then(|meta| meta.get("id")))
        .and_then(Value::as_str)
        .filter(|reference| is_canonical_ticket_resource_key(reference))
}

fn record_ticket_observation(
    observations: &TicketObservations,
    bound_reference: Option<&str>,
    response: &Value,
    mutation: bool,
) {
    let observed_canonical = ticket_reference(response).map(ToOwned::to_owned);
    let mut state = observations
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let existing_bound_group = bound_reference.and_then(|reference| {
        state
            .aliases_by_canonical
            .iter()
            .find(|(_, aliases)| aliases.contains(reference))
            .map(|(canonical, _)| canonical.clone())
    });
    let existing_canonical_group = observed_canonical.as_deref().and_then(|reference| {
        state
            .aliases_by_canonical
            .iter()
            .find(|(_, aliases)| aliases.contains(reference))
            .map(|(canonical, _)| canonical.clone())
    });
    let Some(group_key) = observed_canonical
        .clone()
        .or(existing_canonical_group.clone())
        .or(existing_bound_group.clone())
        .or_else(|| bound_reference.map(ToOwned::to_owned))
    else {
        return;
    };

    let mut aliases = BTreeSet::from([group_key.clone()]);
    for prior_group in [existing_bound_group, existing_canonical_group]
        .into_iter()
        .flatten()
        .chain(std::iter::once(group_key.clone()))
        .collect::<BTreeSet<_>>()
    {
        if let Some(existing) = state.aliases_by_canonical.remove(&prior_group) {
            aliases.extend(existing);
        }
    }
    if let Some(canonical) = observed_canonical {
        aliases.insert(canonical);
    }
    if let Some(reference) = bound_reference {
        aliases.insert(reference.to_string());
    }

    // Mutation receipts are not full representations. Forget the observed
    // content rather than manufacturing a local change counter.
    for alias in &aliases {
        if mutation {
            state.content_digests.remove(alias);
        } else {
            let bytes = Sha256::digest(response.to_string().as_bytes());
            let digest = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
            state.content_digests.insert(alias.clone(), digest);
        }
    }
    state.aliases_by_canonical.insert(group_key, aliases);
}

fn record_affected_ticket_mutations(
    observations: &TicketObservations,
    bound_reference: &str,
    response: &Value,
) {
    let mut subject_references = BTreeSet::from([bound_reference.to_string()]);
    if let Some(reference) = ticket_reference(response) {
        subject_references.insert(reference.to_string());
    }
    let mut affected = BTreeSet::new();
    if let Some(target) = response
        .get("target")
        .and_then(Value::as_str)
        .filter(|reference| is_ticket_route_reference(reference))
    {
        affected.insert(target.to_string());
    }
    if let Some(queued) = response.get("queued_tickets").and_then(Value::as_array) {
        affected.extend(
            queued
                .iter()
                .filter_map(Value::as_str)
                .filter(|reference| is_ticket_route_reference(reference))
                .map(ToOwned::to_owned),
        );
    }
    for reference in affected.difference(&subject_references) {
        record_ticket_observation(observations, Some(reference), &Value::Null, true);
    }
}

fn is_ticket_route_reference(reference: &str) -> bool {
    is_canonical_ticket_resource_key(reference)
        || (reference.len() >= 8
            && reference.len() <= 128
            && reference.bytes().all(|byte| byte.is_ascii_alphanumeric()))
}

fn route_validator(route: &str, content_digest: &str) -> Vec<u8> {
    let mut digest = Sha256::new();
    digest.update(route.as_bytes());
    digest.update([0]);
    digest.update(content_digest.as_bytes());
    digest.finalize().to_vec()
}

fn descriptor_validator(descriptor: &InterfaceDescriptor) -> Vec<u8> {
    let mut digest = Sha256::new();
    digest.update(format!("{descriptor:?}").as_bytes());
    digest.finalize().to_vec()
}

fn ticket_descriptor(
    tools: &[NativeTicketTool],
    summary: &str,
    details: &str,
) -> Result<InterfaceDescriptor, WipMountError> {
    let operations = tools
        .iter()
        .map(ticket_operation_declaration)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(InterfaceDescriptor {
        format: INTERFACE_FORMAT_V1.into(),
        documentation: Some(Documentation {
            summary: summary.into(),
            details: Some(details.into()),
        }),
        types: Vec::new(),
        operations,
    })
}

fn ticket_operation_declaration(
    tool: &NativeTicketTool,
) -> Result<OperationDeclaration, WipMountError> {
    let properties = tool
        .schema
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(|| WipMountError::InvalidProjection {
            route: "/tickets".into(),
            message: format!("Ticket tool `{}` schema has no properties", tool.name),
        })?;
    let required = tool
        .schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<BTreeSet<_>>();
    let parameters = properties
        .iter()
        .filter(|(name, _)| tool.projection.identity_field != Some(name.as_str()))
        .map(|(name, schema)| ParameterDeclaration {
            name: name.clone(),
            required: required.contains(name.as_str()),
            documentation: schema
                .get("description")
                .and_then(Value::as_str)
                .map(|summary| Documentation {
                    summary: summary.into(),
                    details: None,
                }),
            r#type: schema_type_expr(schema),
        })
        .collect();
    Ok(OperationDeclaration {
        name: tool.projection.operation.into(),
        documentation: Some(Documentation {
            summary: tool.description.clone(),
            details: Some(format!(
                "Delegates to the existing `{}` typed Ticket operation and retains its exact JSON Schema validation.",
                tool.name
            )),
        }),
        parameters,
        returns: ReturnDeclaration {
            documentation: Some(Documentation {
                summary: "Authoritative bounded Ticket operation result".into(),
                details: None,
            }),
            r#type: TypeExpr::Json,
        },
    })
}

fn schema_type_expr(schema: &Value) -> TypeExpr {
    match schema.get("type").and_then(Value::as_str) {
        Some("string") => TypeExpr::String,
        Some("boolean") => TypeExpr::Boolean,
        Some("integer") => TypeExpr::Integer,
        Some("number") => TypeExpr::Number,
        Some("array") => TypeExpr::List {
            items: Box::new(
                schema
                    .get("items")
                    .map(schema_type_expr)
                    .unwrap_or(TypeExpr::Json),
            ),
        },
        _ => TypeExpr::Json,
    }
}

fn protocol_failure(code: ProtocolErrorCode, message: impl Into<String>) -> WipOperationError {
    WipOperationError::Protocol(ProtocolError {
        code,
        message: message.into(),
    })
}

fn operation_not_found() -> WipOperationError {
    protocol_failure(
        ProtocolErrorCode::OperationNotFound,
        "operation is not published by this Ticket interface",
    )
}

pub fn ticket_tools_feature(
    workspace_client: Arc<dyn WorkspaceClient>,
    access: TicketFeatureAccess,
) -> TicketFeature {
    TicketFeature::new(workspace_client, access)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::{FeatureRegistryBuilder, FeatureRuntimeKind};
    use crate::hook::HookRegistryBuilder;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    #[derive(Debug)]
    struct DecisionClient {
        requests: Mutex<Vec<WorkspaceRequest>>,
        response: crate::worker::WorkspaceResponse,
    }

    impl WorkspaceClient for DecisionClient {
        fn workspace_id(&self) -> Option<&str> {
            Some("workspace")
        }
        fn kind(&self) -> &str {
            "test-decisions"
        }
        fn is_available(&self) -> bool {
            true
        }
        fn execute(
            &self,
            request: WorkspaceRequest,
        ) -> Result<crate::worker::WorkspaceResponse, crate::worker::WorkspaceClientError> {
            let mut response = self.response.clone();
            // Legacy adapters read a snapshot before the mutation. Let tests
            // inject mutation rejection independently of that successful read.
            if request.method == WorkspaceRequestMethod::Get {
                response.status = 200;
            }
            self.requests.lock().unwrap().push(request);
            Ok(response)
        }
    }

    fn decision_client(status: u16, state: TicketWorkflowState) -> Arc<DecisionClient> {
        let temp = tempfile::tempdir().unwrap();
        let db =
            ticket::SqliteTicketBackend::open(temp.path().join("tickets.db"), "workspace").unwrap();
        let created = db.create(NewTicket::new("Decision")).unwrap();
        let mut record = db.show(TicketIdOrSlug::Id(created.id)).unwrap();
        record.meta.resource_key = Some("T-718".into());
        record.meta.workflow_state = state;
        Arc::new(DecisionClient {
            requests: Mutex::new(Vec::new()),
            response: crate::worker::WorkspaceResponse {
                status,
                body: serde_json::to_string(&record).unwrap(),
            },
        })
    }

    fn decision_input() -> Value {
        json!({
            "ticket": "T-718", "operation_key": "decision-1",
            "expected_content_digest": "content_digest-1", "expected_state": "planning",
            "reason": "No repository changes are needed"
        })
    }

    fn close_input() -> Value {
        let mut input = decision_input();
        let reason = input.as_object_mut().unwrap().remove("reason").unwrap();
        input["resolution"] = reason;
        input
    }

    fn legacy_change() -> TicketStateChange {
        serde_json::from_value(json!({
            "from": "planning", "to": "closed", "reason": "Decision",
            "body": "Supporting explanation", "references": [], "author": "caller"
        }))
        .unwrap()
    }

    #[test]
    fn legacy_ticket_state_adapters_send_strong_cas_without_refreshing_stale_from() {
        for endpoint in [
            "state-changes",
            "workflow-state",
            "state-fields/workflow_state",
            "state-fields/state",
        ] {
            // The observed state is ready, deliberately different from change.from.
            let client = decision_client(200, TicketWorkflowState::Ready);
            let snapshot: Ticket = serde_json::from_str(&client.response.body).unwrap();
            let backend = WorkspaceHttpTicketBackend::new(client.clone());
            let id = TicketIdOrSlug::from("T-718");
            match endpoint {
                "state-changes" => backend.add_state_changed(id, legacy_change()),
                "workflow-state" => backend.set_workflow_state(id, legacy_change()),
                "state-fields/workflow_state" => {
                    backend.set_state_field(id, "workflow_state", legacy_change())
                }
                _ => backend.set_state_field(id, "state", legacy_change()),
            }
            .unwrap();
            let requests = client.requests.lock().unwrap();
            assert_eq!(requests.len(), 2);
            assert_eq!(requests[0].method, WorkspaceRequestMethod::Get);
            assert_eq!(requests[0].path, "/api/w/workspace/tickets/T-718/record");
            assert_eq!(requests[1].method, WorkspaceRequestMethod::Post);
            assert_eq!(
                requests[1].path,
                format!("/api/w/workspace/tickets/T-718/{endpoint}")
            );
            let body: Value = serde_json::from_str(requests[1].body.as_deref().unwrap()).unwrap();
            let request: ticket::TicketStateUpdate = serde_json::from_value(body.clone()).unwrap();
            assert!(uuid::Uuid::parse_str(&request.operation_key).is_ok());
            assert_eq!(
                request.expected_content_digest,
                ticket::ticket_content_digest(&snapshot)
            );
            assert_eq!(request.expected_state, TicketWorkflowState::Planning);
            assert_eq!(request.state, TicketWorkflowState::Closed);
            assert_eq!(request.reason, "Decision\n\nSupporting explanation");
            assert_eq!(body["author"], "caller");
            assert_eq!(body["references"], json!([]));
            assert!(body.get("from").is_none());
            assert!(body.get("to").is_none());
        }
    }

    #[test]
    fn legacy_ticket_state_adapter_parses_each_original_display_state() {
        let client = decision_client(200, TicketWorkflowState::Closed);
        let backend = WorkspaceHttpTicketBackend::new(client.clone());
        for state in [
            TicketWorkflowState::Planning,
            TicketWorkflowState::Ready,
            TicketWorkflowState::Queued,
            TicketWorkflowState::InProgress,
            TicketWorkflowState::Done,
            TicketWorkflowState::Closed,
        ] {
            let mut change = legacy_change();
            change.from = state.as_str().into();
            change.to = state.as_str().into();
            backend
                .set_workflow_state(TicketIdOrSlug::from("T-718"), change)
                .unwrap();
            let requests = client.requests.lock().unwrap();
            let request: ticket::TicketStateUpdate =
                serde_json::from_str(requests.last().unwrap().body.as_deref().unwrap()).unwrap();
            assert_eq!(request.expected_state, state);
            assert_eq!(request.state, state);
        }
    }

    #[test]
    fn legacy_ticket_adapters_reject_invalid_fields_states_and_empty_reasons_before_dispatch() {
        let client = decision_client(200, TicketWorkflowState::Planning);
        let backend = WorkspaceHttpTicketBackend::new(client.clone());
        assert!(
            backend
                .set_state_field(
                    TicketIdOrSlug::from("T-718"),
                    "approval_state",
                    legacy_change()
                )
                .is_err()
        );
        for field in ["from", "to"] {
            let mut change = legacy_change();
            if field == "from" {
                change.from = "approved".into();
            } else {
                change.to = "approved".into();
            }
            assert!(
                backend
                    .set_workflow_state(TicketIdOrSlug::from("T-718"), change)
                    .is_err()
            );
        }
        let mut change = legacy_change();
        change.reason = " ".into();
        change.body = " \n ".into();
        assert!(
            backend
                .add_state_changed(TicketIdOrSlug::from("T-718"), change)
                .is_err()
        );
        assert!(
            backend
                .close(TicketIdOrSlug::from("T-718"), " ".into())
                .is_err()
        );
        assert!(client.requests.lock().unwrap().is_empty());
    }

    #[test]
    fn legacy_ticket_close_snapshots_cas_and_generates_distinct_completion_keys() {
        let client = decision_client(200, TicketWorkflowState::Ready);
        let snapshot: Ticket = serde_json::from_str(&client.response.body).unwrap();
        let backend = WorkspaceHttpTicketBackend::new(client.clone());
        for _ in 0..2 {
            backend
                .close(
                    TicketIdOrSlug::from("T-718"),
                    "Resolved without repository changes".into(),
                )
                .unwrap();
        }
        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 4);
        let mut keys = BTreeSet::new();
        for pair in requests.chunks_exact(2) {
            assert_eq!(pair[0].method, WorkspaceRequestMethod::Get);
            assert_eq!(pair[0].path, "/api/w/workspace/tickets/T-718/record");
            assert_eq!(pair[1].method, WorkspaceRequestMethod::Post);
            assert_eq!(
                pair[1].path,
                "/api/w/workspace/tickets/T-718/workflow/close"
            );
            let request: ticket::TicketCompletion =
                serde_json::from_str(pair[1].body.as_deref().unwrap()).unwrap();
            assert!(uuid::Uuid::parse_str(&request.operation_key).is_ok());
            assert!(keys.insert(request.operation_key));
            assert_eq!(
                request.expected_content_digest,
                ticket::ticket_content_digest(&snapshot)
            );
            assert_eq!(request.expected_state, TicketWorkflowState::Ready);
            assert_eq!(request.reason, "Resolved without repository changes");
            assert!(request.references.is_empty());
            assert!(request.author.is_none());
        }
    }

    #[test]
    fn legacy_ticket_adapters_surface_backend_authority_and_stale_state_rejections() {
        for status in [403, 409] {
            for close in [false, true] {
                let client = decision_client(status, TicketWorkflowState::Ready);
                let backend = WorkspaceHttpTicketBackend::new(client.clone());
                let result = if close {
                    backend.close(TicketIdOrSlug::from("T-718"), "Resolved".into())
                } else {
                    backend.set_workflow_state(TicketIdOrSlug::from("T-718"), legacy_change())
                };
                assert!(
                    result
                        .unwrap_err()
                        .to_string()
                        .contains(&format!("HTTP status {status}"))
                );
                assert_eq!(client.requests.lock().unwrap().len(), 2);
            }
        }
    }

    #[tokio::test]
    async fn native_ticket_close_binds_subject_and_forwards_strong_closed_state_decision() {
        let client = decision_client(200, TicketWorkflowState::Closed);
        let tools = native_ticket_tools(client.clone(), TicketFeatureAccess::workspace_authoring())
            .unwrap();
        let descriptor = ticket_descriptor(&tools, "Ticket", "bound").unwrap();
        let close = descriptor
            .operations
            .iter()
            .find(|op| op.name == "close")
            .unwrap();
        assert!(
            !close
                .parameters
                .iter()
                .any(|p| p.name == "ticket" || p.name == "author" || p.name == "role")
        );
        for field in [
            "operation_key",
            "expected_content_digest",
            "expected_state",
            "resolution",
        ] {
            assert!(
                close
                    .parameters
                    .iter()
                    .any(|p| p.name == field && p.required)
            );
        }
        assert!(
            close
                .parameters
                .iter()
                .any(|p| p.name == "references" && !p.required)
        );
        let observations = Arc::new(Mutex::new(TicketObservationState::default()));
        let resolver = TicketItemResolver {
            tools: operation_map(tools),
            permissions: None,
            collection_route: "/tickets".into(),
            observations,
        };
        record_ticket_observation(
            &resolver.observations,
            Some("T-718"),
            &json!({"ticket": "T-718", "body": "observed requirements"}),
            false,
        );
        let bound = resolver.resolve("T-718").unwrap();
        let before = bound.object.validator;
        let other = resolver.resolve("T-719").unwrap().object.validator;
        let mut input = close_input();
        input.as_object_mut().unwrap().remove("ticket");
        let arguments = input
            .as_object()
            .unwrap()
            .iter()
            .map(|(key, value)| (key.clone(), json_to_wip(value).unwrap()))
            .collect();
        bound
            .handler
            .call(
                "close",
                &arguments,
                WipCallContext {
                    execution: ToolExecutionContext::direct(),
                    security_context: "author".into(),
                },
            )
            .await
            .unwrap_or_else(|_| panic!("bound close should succeed"));
        assert_ne!(before, resolver.resolve("T-718").unwrap().object.validator);
        assert_eq!(other, resolver.resolve("T-719").unwrap().object.validator);
        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].path,
            "/api/w/workspace/tickets/T-718/state-update"
        );
        let body: Value = serde_json::from_str(requests[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(
            body,
            json!({
                "operation_key": "decision-1", "expected_content_digest": "content_digest-1",
                "expected_state": "planning", "state": "closed",
                "reason": "No repository changes are needed", "references": []
            })
        );
    }

    #[tokio::test]
    async fn native_ticket_close_preserves_backend_rejections_and_wip_permission_gate() {
        for (status, code) in [
            (403, ProtocolErrorCode::PermissionDenied),
            (409, ProtocolErrorCode::InvalidArguments),
        ] {
            let client = decision_client(status, TicketWorkflowState::Planning);
            let tools =
                native_ticket_tools(client.clone(), TicketFeatureAccess::workflow()).unwrap();
            let tool = tools
                .iter()
                .find(|tool| tool.name == "TicketClose")
                .unwrap();
            let error = execute_native_ticket_tool(
                tool,
                &None,
                close_input(),
                WipCallContext {
                    execution: ToolExecutionContext::direct(),
                    security_context: "self-reported-author".into(),
                },
            )
            .await
            .expect_err("Backend rejection must be preserved");
            assert!(
                matches!(error, WipOperationError::Protocol(ProtocolError { code: actual, .. }) if actual == code)
            );
            assert_eq!(client.requests.lock().unwrap().len(), 1);
        }
        let client = decision_client(200, TicketWorkflowState::Closed);
        let tools = native_ticket_tools(client.clone(), TicketFeatureAccess::workflow()).unwrap();
        let tool = tools
            .iter()
            .find(|tool| tool.name == "TicketClose")
            .unwrap();
        let permissions = Some(ToolPermissionConfig {
            default_action: ToolPermissionAction::Deny,
            rules: Vec::new(),
        });
        let error = execute_native_ticket_tool(
            tool,
            &permissions,
            close_input(),
            WipCallContext {
                execution: ToolExecutionContext::direct(),
                security_context: "author".into(),
            },
        )
        .await
        .expect_err("WIP permission denial must prevent dispatch");
        assert!(matches!(
            error,
            WipOperationError::Protocol(ProtocolError {
                code: ProtocolErrorCode::PermissionDenied,
                ..
            })
        ));
        assert!(client.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn completion_accepts_mrless_decisions_and_forwards_cas_without_actor_claims() {
        let client = decision_client(200, TicketWorkflowState::Done);
        let (_, tool) = workspace_ticket_decision_definition(
            client.clone(),
            WorkspaceTicketDecisionKind::Complete,
        )();
        let output = tool
            .execute(
                &decision_input().to_string(),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
        let projected: Value = serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
        assert_eq!(projected["meta"]["id"], "T-718");
        assert_eq!(projected["meta"]["workflow_state"], "done");
        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, "/api/w/workspace/tickets/T-718/complete");
        assert_eq!(requests[0].method, WorkspaceRequestMethod::Post);
        let body: Value = serde_json::from_str(requests[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(
            body,
            json!({
                "operation_key": "decision-1", "expected_content_digest": "content_digest-1",
                "expected_state": "planning", "reason": "No repository changes are needed", "references": []
            })
        );
    }

    #[tokio::test]
    async fn state_updates_forward_each_display_state_without_other_control_operations() {
        for state in [
            "planning",
            "ready",
            "queued",
            "in_progress",
            "done",
            "closed",
        ] {
            let client = decision_client(200, TicketWorkflowState::Planning);
            let (_, tool) = workspace_ticket_decision_definition(
                client.clone(),
                WorkspaceTicketDecisionKind::UpdateState,
            )();
            let mut input = decision_input();
            input["state"] = json!(state);
            tool.execute(&input.to_string(), ToolExecutionContext::direct())
                .await
                .unwrap();
            let requests = client.requests.lock().unwrap();
            assert_eq!(requests.len(), 1);
            assert_eq!(
                requests[0].path,
                "/api/w/workspace/tickets/T-718/state-update"
            );
            let mut expected = input.as_object().unwrap().clone();
            expected.remove("ticket");
            expected.insert("references".into(), json!([]));
            let body: Value = serde_json::from_str(requests[0].body.as_deref().unwrap()).unwrap();
            assert_eq!(body, Value::Object(expected));
        }
    }

    #[tokio::test]
    async fn decision_inputs_require_reason_cas_and_reject_self_reported_authority_before_dispatch()
    {
        for kind in [
            WorkspaceTicketDecisionKind::Complete,
            WorkspaceTicketDecisionKind::UpdateState,
            WorkspaceTicketDecisionKind::Close,
        ] {
            let client = decision_client(200, TicketWorkflowState::Done);
            let (meta, tool) = workspace_ticket_decision_definition(client.clone(), kind)();
            let mut valid = decision_input();
            if matches!(kind, WorkspaceTicketDecisionKind::UpdateState) {
                valid["state"] = json!("ready");
            }
            let reason_field = if matches!(kind, WorkspaceTicketDecisionKind::Close) {
                let reason = valid.as_object_mut().unwrap().remove("reason").unwrap();
                valid["resolution"] = reason;
                "resolution"
            } else {
                "reason"
            };
            let validator = jsonschema::validator_for(&meta.input_schema).unwrap();
            assert!(validator.is_valid(&valid));
            for field in [
                "operation_key",
                "expected_content_digest",
                "expected_state",
                reason_field,
            ] {
                let mut invalid = valid.clone();
                invalid.as_object_mut().unwrap().remove(field);
                assert!(!validator.is_valid(&invalid));
                assert!(
                    tool.execute(&invalid.to_string(), ToolExecutionContext::direct())
                        .await
                        .is_err()
                );
            }
            for field in [
                reason_field,
                "operation_key",
                "expected_content_digest",
                "ticket",
            ] {
                let mut invalid = valid.clone();
                invalid[field] = json!("  ");
                assert!(
                    tool.execute(&invalid.to_string(), ToolExecutionContext::direct())
                        .await
                        .is_err()
                );
            }
            for field in [
                "role",
                "author",
                "merge_request_ids",
                "requirement_approval_event_id",
            ] {
                let mut invalid = valid.clone();
                invalid[field] = json!("coder");
                assert!(!validator.is_valid(&invalid));
                assert!(
                    tool.execute(&invalid.to_string(), ToolExecutionContext::direct())
                        .await
                        .is_err()
                );
            }
            let mut invalid = valid.clone();
            invalid["expected_state"] = json!("approved");
            assert!(!validator.is_valid(&invalid));
            assert!(
                tool.execute(&invalid.to_string(), ToolExecutionContext::direct())
                    .await
                    .is_err()
            );
            assert!(client.requests.lock().unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn decision_tools_preserve_backend_assignment_and_cas_rejections() {
        for (status, code) in [
            (403, ProtocolErrorCode::PermissionDenied),
            (409, ProtocolErrorCode::InvalidArguments),
        ] {
            let client = decision_client(status, TicketWorkflowState::Planning);
            let tools =
                native_ticket_tools(client.clone(), TicketFeatureAccess::work_report()).unwrap();
            let tool = tools
                .iter()
                .find(|tool| tool.name == "CompleteTicket")
                .unwrap();
            let error = execute_native_ticket_tool(
                tool,
                &None,
                decision_input(),
                WipCallContext {
                    execution: ToolExecutionContext::direct(),
                    security_context: "self-reported-coder".into(),
                },
            )
            .await
            .expect_err("Backend rejection must be preserved");
            assert!(
                matches!(error, WipOperationError::Protocol(ProtocolError { code: actual, .. }) if actual == code)
            );
            assert_eq!(client.requests.lock().unwrap().len(), 1);
        }
    }

    #[test]
    fn native_ticket_decision_descriptions_preserve_intent_and_independent_lifecycle() {
        let tools = native_ticket_tools(
            decision_client(200, TicketWorkflowState::Ready),
            TicketFeatureAccess::work_report(),
        )
        .unwrap();
        let descriptor = ticket_descriptor(&tools, "Ticket", "bound").unwrap();
        let complete = descriptor
            .operations
            .iter()
            .find(|op| op.name == "complete")
            .unwrap();
        let summary = &complete.documentation.as_ref().unwrap().summary;
        assert!(summary.contains("Merge Requests and approvals are not prerequisites"));
        assert!(summary.contains("return without conclusion need not call this tool"));
        assert!(summary.contains("not real judgment"));
        assert!(summary.contains("retaining terminal responsibility"));
        assert!(summary.contains("does not grant review/merge authority or stop/remove Workers"));
    }

    #[tokio::test]
    async fn native_ticket_completion_binds_subject_and_stales_only_its_ticket_not_mrs() {
        let client = decision_client(200, TicketWorkflowState::Done);
        let observations = Arc::new(Mutex::new(TicketObservationState::default()));
        let tools =
            native_ticket_tools(client.clone(), TicketFeatureAccess::work_report()).unwrap();
        let descriptor = ticket_descriptor(&tools, "Ticket", "bound").unwrap();
        let complete = descriptor
            .operations
            .iter()
            .find(|op| op.name == "complete")
            .unwrap();
        assert!(!complete.parameters.iter().any(|p| p.name == "ticket"));
        for field in [
            "operation_key",
            "expected_content_digest",
            "expected_state",
            "reason",
        ] {
            assert!(
                complete
                    .parameters
                    .iter()
                    .any(|p| p.name == field && p.required)
            );
        }
        let resolver = TicketItemResolver {
            tools: operation_map(tools),
            permissions: None,
            collection_route: "/tickets".into(),
            observations: observations.clone(),
        };
        record_ticket_observation(
            &resolver.observations,
            Some("T-718"),
            &json!({"ticket": "T-718", "body": "observed requirements"}),
            false,
        );
        let bound = resolver.resolve("T-718").unwrap();
        let before = bound.object.validator;
        let other = resolver.resolve("T-719").unwrap().object.validator;
        let mut input = decision_input();
        input.as_object_mut().unwrap().remove("ticket");
        let arguments = input
            .as_object()
            .unwrap()
            .iter()
            .map(|(key, value)| (key.clone(), json_to_wip(value).unwrap()))
            .collect();
        bound
            .handler
            .call(
                "complete",
                &arguments,
                WipCallContext {
                    execution: ToolExecutionContext::direct(),
                    security_context: "coder".into(),
                },
            )
            .await
            .unwrap_or_else(|_| panic!("bound completion should succeed"));
        assert_ne!(before, resolver.resolve("T-718").unwrap().object.validator);
        assert_eq!(other, resolver.resolve("T-719").unwrap().object.validator);
        assert_eq!(
            client.requests.lock().unwrap()[0].path,
            "/api/w/workspace/tickets/T-718/complete"
        );
    }

    #[test]
    fn native_ticket_projection_matches_role_inventory_and_binds_item_identity() {
        let client: Arc<dyn WorkspaceClient> = Arc::new(
            crate::worker::TestWorkspaceHttpClient::new("workspace", "http://backend"),
        );
        let authoring =
            native_ticket_tools(client.clone(), TicketFeatureAccess::workspace_authoring())
                .unwrap();
        let authoring_operations = authoring
            .iter()
            .map(|tool| tool.projection.operation)
            .collect::<Vec<_>>();
        assert_eq!(
            authoring_operations,
            [
                "create",
                "edit",
                "query",
                "read",
                "comment",
                "mark_ready",
                "queue",
                "transition",
                "close",
                "record_relation",
                "remove_relation",
                "complete",
            ]
        );
        let item = ticket_descriptor(
            &authoring
                .iter()
                .filter(|tool| tool.projection.surface == NativeTicketSurface::Item)
                .cloned()
                .collect::<Vec<_>>(),
            "items",
            "route bound",
        )
        .unwrap();
        assert!(item.operations.iter().all(|operation| {
            operation
                .parameters
                .iter()
                .all(|parameter| parameter.name != "ticket" && parameter.name != "id")
        }));
        let relation = item
            .operations
            .iter()
            .find(|operation| operation.name == "record_relation")
            .unwrap();
        assert!(
            relation
                .parameters
                .iter()
                .any(|parameter| parameter.name == "target")
        );

        let workflow =
            native_ticket_tools(client.clone(), TicketFeatureAccess::workflow()).unwrap();
        assert_eq!(
            workflow
                .iter()
                .map(|tool| tool.projection.operation)
                .collect::<Vec<_>>(),
            [
                "query",
                "read",
                "comment",
                "transition",
                "close",
                "dependency_check",
                "record_relation",
                "remove_relation",
                "record_orchestration_plan",
                "query_orchestration_plans",
                "complete",
            ]
        );
        assert_eq!(
            native_ticket_tools(client, TicketFeatureAccess::review())
                .unwrap()
                .iter()
                .map(|tool| tool.projection.operation)
                .collect::<Vec<_>>(),
            ["query", "read"]
        );
    }

    #[test]
    fn native_ticket_mount_uses_host_route_and_rejects_non_ticket_children() {
        let mut registry = WipMountRegistry::new();
        let namespace_route = registry.allocate_namespace("ticket", "tickets").unwrap();
        mount_workspace_http_ticket_wip(
            &mut registry,
            Arc::new(crate::worker::TestWorkspaceHttpClient::new(
                "workspace",
                "http://backend",
            )),
            TicketFeatureAccess::review(),
            None,
            &namespace_route,
        )
        .unwrap();
        assert_eq!(registry.routes().collect::<Vec<_>>(), ["/tickets"]);
        assert!(
            registry
                .allocate_namespace("ticket", "../objectives")
                .is_err()
        );

        let observations = Arc::new(Mutex::new(TicketObservationState::default()));
        let resolver = TicketItemResolver {
            tools: HashMap::new(),
            permissions: None,
            collection_route: "/tickets".into(),
            observations,
        };
        assert!(resolver.resolve("T-42").is_some());
        assert!(resolver.resolve("00001TICKET").is_some());
        assert!(resolver.resolve("O-42").is_none());
        assert!(resolver.resolve("T-42/nested").is_none());
    }

    #[test]
    fn native_ticket_permission_input_preserves_presence_and_route_identity() {
        let arguments = BTreeMap::from([
            ("body".into(), WipValue::String("comment".into())),
            ("reason".into(), WipValue::Unit),
        ]);
        let input = native_ticket_input(&arguments, Some("T-42"), Some("ticket"))
            .unwrap_or_else(|_| panic!("route-bound Ticket input should project"));
        assert_eq!(
            input,
            json!({"ticket": "T-42", "body": "comment", "reason": null})
        );
        let omitted = native_ticket_input(
            &BTreeMap::from([("body".into(), WipValue::String("comment".into()))]),
            Some("T-42"),
            Some("ticket"),
        )
        .unwrap_or_else(|_| panic!("omitted optional Ticket input should project"));
        assert_eq!(omitted, json!({"ticket": "T-42", "body": "comment"}));
        let replaced = native_ticket_input(
            &BTreeMap::from([("ticket".into(), WipValue::String("T-99".into()))]),
            Some("T-42"),
            Some("ticket"),
        );
        assert!(matches!(
            replaced,
            Err(WipOperationError::Protocol(ProtocolError {
                code: ProtocolErrorCode::InvalidArguments,
                ..
            }))
        ));

        let permissions = Some(ToolPermissionConfig {
            default_action: ToolPermissionAction::Allow,
            rules: vec![manifest::ToolPermissionRule {
                tool: "TicketComment".into(),
                pattern: serde_json::to_string(&input).unwrap(),
                action: ToolPermissionAction::Deny,
            }],
        });
        assert!(matches!(
            authorize_native_ticket(&permissions, "TicketComment", &input),
            Err(WipOperationError::Protocol(ProtocolError {
                code: ProtocolErrorCode::PermissionDenied,
                ..
            }))
        ));
    }

    #[test]
    fn ticket_alias_validators_refresh_together_after_mutation() {
        let observations = Arc::new(Mutex::new(TicketObservationState::default()));
        let resolver = TicketItemResolver {
            tools: HashMap::new(),
            permissions: None,
            collection_route: "/tickets".into(),
            observations: Arc::clone(&observations),
        };
        let unobserved_internal = resolver.resolve("00001TICKET").unwrap().object.validator;
        record_ticket_observation(
            &observations,
            Some("00001TICKET"),
            &json!({"ticket": "00001TICKET", "ok": true}),
            true,
        );
        assert_eq!(
            unobserved_internal,
            resolver.resolve("00001TICKET").unwrap().object.validator,
            "an unobserved route stays unvalidated after mutation"
        );

        record_ticket_observation(
            &observations,
            Some("00001TICKET"),
            &json!({"ticket": "T-42", "content_digest": "rev-1"}),
            false,
        );
        let canonical_v1 = resolver.resolve("T-42").unwrap().object.validator;
        let internal_v1 = resolver.resolve("00001TICKET").unwrap().object.validator;
        record_ticket_observation(
            &observations,
            Some("T-42"),
            &json!({"ticket": "T-42", "updated_at": "rev-2"}),
            true,
        );
        assert_ne!(
            canonical_v1,
            resolver.resolve("T-42").unwrap().object.validator
        );
        assert_ne!(
            internal_v1,
            resolver.resolve("00001TICKET").unwrap().object.validator
        );
    }

    #[test]
    fn ticket_observation_digest_is_content_based_and_repeated_mutations_only_invalidate() {
        let observations = Arc::new(Mutex::new(TicketObservationState::default()));
        let resolver = TicketItemResolver {
            tools: HashMap::new(),
            permissions: None,
            collection_route: "/tickets".into(),
            observations: Arc::clone(&observations),
        };
        let response = json!({"ticket": "T-42", "body": "actual content", "state": "planning"});
        assert!(resolver.resolve("T-42").unwrap().object.validator.is_none());
        record_ticket_observation(&observations, Some("T-42"), &response, false);
        let observed = resolver.resolve("T-42").unwrap().object.validator;
        assert!(observed.is_some());
        record_ticket_observation(&observations, Some("T-42"), &response, false);
        assert_eq!(resolver.resolve("T-42").unwrap().object.validator, observed);
        for _ in 0..2 {
            record_ticket_observation(
                &observations,
                Some("T-42"),
                &json!({"operation_key": "same-replay"}),
                true,
            );
            assert!(resolver.resolve("T-42").unwrap().object.validator.is_none());
        }
        record_ticket_observation(&observations, Some("T-42"), &response, false);
        assert_eq!(resolver.resolve("T-42").unwrap().object.validator, observed);
    }

    #[test]
    fn related_ticket_mutations_invalidate_target_and_queue_observations() {
        let observations = Arc::new(Mutex::new(TicketObservationState::default()));
        for ticket in ["T-42", "T-43", "T-44"] {
            record_ticket_observation(
                &observations,
                Some(ticket),
                &json!({"ticket": ticket, "content_digest": "rev-1"}),
                false,
            );
        }
        let resolver = TicketItemResolver {
            tools: HashMap::new(),
            permissions: None,
            collection_route: "/tickets".into(),
            observations: Arc::clone(&observations),
        };
        let target_v1 = resolver.resolve("T-43").unwrap().object.validator;
        let queued_v1 = resolver.resolve("T-44").unwrap().object.validator;
        record_affected_ticket_mutations(
            &observations,
            "T-42",
            &json!({
                "ticket": "T-42",
                "target": "T-43",
                "queued_tickets": ["T-42", "T-44"]
            }),
        );
        assert_ne!(
            target_v1,
            resolver.resolve("T-43").unwrap().object.validator
        );
        assert_ne!(
            queued_v1,
            resolver.resolve("T-44").unwrap().object.validator
        );
    }

    #[test]
    fn native_ticket_errors_keep_deterministic_protocol_outcomes() {
        for (status, code) in [
            (401, ProtocolErrorCode::PermissionDenied),
            (403, ProtocolErrorCode::PermissionDenied),
            (404, ProtocolErrorCode::NotFound),
            (409, ProtocolErrorCode::InvalidArguments),
        ] {
            assert!(matches!(
                map_ticket_tool_error(ToolError::ExecutionFailed(format!(
                    "ticket REST API request failed with HTTP status {status}"
                ))),
                WipOperationError::Protocol(ProtocolError { code: actual, .. }) if actual == code
            ));
        }
        assert!(matches!(
            map_ticket_tool_error(ToolError::ExecutionFailed("transport disconnected".into())),
            WipOperationError::OutcomeUnknown(_)
        ));
        assert!(matches!(
            map_ticket_tool_error(ToolError::Internal(
                "TicketClose outcome unknown after mutation: HTTP status 404".into()
            )),
            WipOperationError::OutcomeUnknown(_)
        ));
    }

    #[test]
    fn production_source_has_no_local_ticket_feature_backend() {
        let production = include_str!("ticket.rs")
            .split_once("#[cfg(test)]\nmod tests")
            .map(|(production, _)| production)
            .expect("Ticket feature test module marker");
        for forbidden in [
            "TicketFeatureBackend",
            "LocalTicketBackend",
            "for_workspace",
            ".yoi/tickets",
        ] {
            assert!(
                !production.contains(forbidden),
                "repository-local Ticket feature returned through {forbidden}"
            );
        }
    }

    fn workspace_feature(access: TicketFeatureAccess) -> TicketFeature {
        let client: Arc<dyn WorkspaceClient> = Arc::new(
            crate::worker::TestWorkspaceHttpClient::new("workspace", "http://backend"),
        );
        ticket_tools_feature(client, access)
    }

    #[test]
    fn workspace_ticket_backend_canonicalizes_model_facing_ticket_ids() {
        let mut value = serde_json::json!({
            "id": "00001INTERNAL",
            "resource_key": "T-42",
            "nested": {
                "id": "00002INTERNAL",
                "resource_key": "T-43"
            },
            "body": "user-authored 00003BODY stays unchanged"
        });
        WorkspaceHttpTicketBackend::canonicalize_ticket_references(&mut value);
        assert_eq!(value["id"], "T-42");
        assert_eq!(value["nested"]["id"], "T-43");
        assert_eq!(value["body"], "user-authored 00003BODY stays unchanged");
    }

    #[test]
    fn workspace_ticket_reads_expose_bounded_query_and_show_contracts_without_legacy_aliases() {
        let client: Arc<dyn WorkspaceClient> = Arc::new(
            crate::worker::TestWorkspaceHttpClient::new("workspace", "http://backend"),
        );
        let (query, _) =
            workspace_ticket_read_definition(client.clone(), WorkspaceTicketReadKind::Query)();
        assert_eq!(query.name, "QueryTicket");
        assert!(query.input_schema["properties"]["evidence"].is_object());
        assert!(query.input_schema["properties"]["attention"].is_object());
        assert!(query.input_schema["properties"]["cursor"].is_object());
        let query_schema = serde_json::to_string(&query.input_schema).unwrap();
        assert!(query_schema.contains("done_not_closed"));
        assert!(query_schema.contains("request_changes"));
        assert!(query_schema.contains("created_desc"));
        assert!(
            query_schema.len() < 8_000,
            "QueryTicket schema grew unexpectedly"
        );
        let (show, _) = workspace_ticket_read_definition(client, WorkspaceTicketReadKind::Show)();
        assert_eq!(show.name, "ShowTicket");
        assert!(show.input_schema["properties"]["event_limit"].is_object());
        let tool_names = TicketFeatureAccess::workspace_authoring().tool_names();
        assert_eq!(tool_names.len(), 12);
        assert!(
            tool_names.len() < 13,
            "authoring catalog must stay below the prior broad catalog"
        );
        let workflow_names = TicketFeatureAccess::workflow().tool_names();
        assert_eq!(workflow_names.len(), 11);
        assert!(
            workflow_names.len() < 12,
            "workflow catalog must stay below the prior broad catalog"
        );
        assert_eq!(TicketFeatureAccess::review().tool_names().len(), 2);
        assert!(tool_names.contains(&"QueryTicket"));
        assert!(tool_names.contains(&"ShowTicket"));
        assert!(!tool_names.contains(&"TicketList"));
        assert!(!tool_names.contains(&"TicketShow"));
    }

    #[test]
    fn descriptor_declares_ticket_tools() {
        let feature = workspace_feature(TicketFeatureAccess::workspace_authoring());
        let descriptor = feature.descriptor();
        assert_eq!(descriptor.id.to_string(), "builtin:ticket");
        assert_eq!(descriptor.runtime, FeatureRuntimeKind::Builtin);
        assert_eq!(descriptor.tools.len(), WORKSPACE_AUTHORING_TOOL_NAMES.len());
        assert_eq!(
            descriptor
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            WORKSPACE_AUTHORING_TOOL_NAMES
        );
    }

    #[test]
    fn read_only_descriptor_declares_only_state_tools() {
        let feature = workspace_feature(TicketFeatureAccess::read_only());
        let descriptor = feature.descriptor();
        assert_eq!(feature.access(), TicketFeatureAccess::read_only());
        assert_eq!(descriptor.tools.len(), READ_ONLY_TOOL_NAMES.len());
        assert_eq!(
            descriptor
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            READ_ONLY_TOOL_NAMES
        );
    }

    #[test]
    fn workflow_descriptor_declares_workflow_tools() {
        let feature = workspace_feature(TicketFeatureAccess::workflow());
        let descriptor = feature.descriptor();
        assert_eq!(feature.access(), TicketFeatureAccess::workflow());
        assert_eq!(
            descriptor
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            WORKFLOW_TOOL_NAMES
        );
    }

    #[test]
    fn additive_ticket_capabilities_expose_expected_tool_surfaces() {
        let workspace_authoring = workspace_feature(TicketFeatureAccess::workspace_authoring());
        let workspace_descriptor = workspace_authoring.descriptor();
        let workspace_tools = workspace_descriptor
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>();
        assert!(workspace_tools.contains(&"TicketCreate"));
        assert!(workspace_tools.contains(&"TicketEditItem"));
        assert!(workspace_tools.contains(&"TicketQueue"));
        assert!(workspace_tools.contains(&"TicketWorkflowState"));

        let orchestration = workspace_feature(TicketFeatureAccess::workflow());
        let orchestration_descriptor = orchestration.descriptor();
        let orchestration_tools = orchestration_descriptor
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>();
        assert!(orchestration_tools.contains(&"TicketWorkflowState"));
        assert!(orchestration_tools.contains(&"TicketDependencyCheck"));
        assert!(orchestration_tools.contains(&"TicketRelationRecord"));
        assert!(orchestration_tools.contains(&"TicketOrchestrationPlanRecord"));
        assert!(!orchestration_tools.contains(&"TicketEditItem"));
        assert!(!orchestration_tools.contains(&"TicketQueue"));

        let work_report = workspace_feature(TicketFeatureAccess::work_report());
        let work_report_descriptor = work_report.descriptor();
        let work_report_tools = work_report_descriptor
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>();
        assert!(work_report_tools.contains(&"TicketComment"));
        assert!(work_report_tools.contains(&"CompleteTicket"));
        assert!(work_report_tools.contains(&"TicketWorkflowState"));

        let review = workspace_feature(TicketFeatureAccess::review());
        let review_descriptor = review.descriptor();
        let review_tools = review_descriptor
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>();
        assert!(!review_tools.contains(&"TicketWorkflowState"));
        assert!(!review_tools.contains(&"CompleteTicket"));
    }

    #[test]
    fn read_only_installation_does_not_expose_mutating_tools() {
        let mut pending_tools = Vec::new();
        let mut hooks = HookRegistryBuilder::default();
        let report = FeatureRegistryBuilder::new()
            .with_module(workspace_feature(TicketFeatureAccess::read_only()))
            .install_into_pending(&mut pending_tools, &mut hooks);

        assert_eq!(pending_tools.len(), READ_ONLY_TOOL_NAMES.len());
        assert_eq!(report.reports[0].installed_tools, READ_ONLY_TOOL_NAMES);
        let pending_names = pending_tools
            .iter()
            .map(|definition| definition().0.name)
            .collect::<Vec<_>>();
        assert_eq!(pending_names, READ_ONLY_TOOL_NAMES);
        for name in ticket::tool::TICKET_MUTATING_TOOL_NAMES {
            assert!(
                !report.reports[0]
                    .installed_tools
                    .iter()
                    .any(|tool| tool == name)
            );
            assert!(!pending_names.iter().any(|tool| tool == name));
        }
    }

    #[test]
    fn workspace_authoring_installation_exposes_authoring_tools() {
        let mut pending_tools = Vec::new();
        let mut hooks = HookRegistryBuilder::default();
        let report = FeatureRegistryBuilder::new()
            .with_module(workspace_feature(TicketFeatureAccess::workspace_authoring()))
            .install_into_pending(&mut pending_tools, &mut hooks);

        assert_eq!(pending_tools.len(), WORKSPACE_AUTHORING_TOOL_NAMES.len());
        let installed = report.reports[0]
            .installed_tools
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        assert_eq!(installed, WORKSPACE_AUTHORING_TOOL_NAMES);
        assert!(installed.iter().any(|tool| *tool == "TicketCreate"));
        assert!(installed.iter().any(|tool| *tool == "TicketEditItem"));
        assert!(installed.iter().any(|tool| *tool == "TicketQueue"));
        assert!(installed.iter().any(|tool| *tool == "TicketMarkReady"));
        assert!(!installed.iter().any(|tool| *tool == "TicketIntakeReady"));
        assert!(installed.iter().any(|tool| *tool == "TicketWorkflowState"));
        assert!(
            !installed
                .iter()
                .any(|tool| *tool == "TicketOrchestrationPlanRecord")
        );
    }

    #[test]
    fn workspace_only_ticket_feature_preserves_unrelated_stale_repository_trees() {
        let ancestor = tempfile::tempdir().unwrap();
        let repository = ancestor.path().join("repository");
        std::fs::create_dir_all(repository.join(".yoi/tickets/broken")).unwrap();
        std::fs::create_dir_all(ancestor.path().join(".yoi")).unwrap();
        std::fs::write(
            ancestor.path().join(".yoi/workspace.toml"),
            "not valid toml",
        )
        .unwrap();
        std::fs::write(
            repository.join(".yoi/tickets/broken/item.md"),
            "not a Ticket",
        )
        .unwrap();

        // TicketFeature accepts only an injected Workspace client and access policy;
        // there is deliberately no repository/cwd path to pass to this installation.
        let mut pending_tools = Vec::new();
        let mut hooks = HookRegistryBuilder::default();
        let report = FeatureRegistryBuilder::new()
            .with_module(workspace_feature(TicketFeatureAccess::workspace_authoring()))
            .install_into_pending(&mut pending_tools, &mut hooks);

        assert_eq!(pending_tools.len(), WORKSPACE_AUTHORING_TOOL_NAMES.len());
        assert_eq!(
            report.reports[0].installed_tools,
            WORKSPACE_AUTHORING_TOOL_NAMES
        );
        assert!(report.reports[0].diagnostics.is_empty());
        assert_eq!(
            std::fs::read_to_string(ancestor.path().join(".yoi/workspace.toml")).unwrap(),
            "not valid toml"
        );
        assert_eq!(
            std::fs::read_to_string(repository.join(".yoi/tickets/broken/item.md")).unwrap(),
            "not a Ticket"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn workspace_http_backend_invoke_is_safe_inside_async_context() {
        let backend = WorkspaceHttpTicketBackend::new(Arc::new(
            crate::worker::TestWorkspaceHttpClient::new("workspace-a", "not-a-url"),
        ));

        let error = backend
            .invoke(TicketBackendOperation::DefaultIntakeReadyStateChangeBody {
                from: "planning".to_string(),
            })
            .unwrap_err();

        assert!(error.to_string().contains("ticket REST request failed"));
    }

    #[test]
    fn workspace_http_backend_posts_ticket_event_subresource() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0_u8; 8192];
            let size = stream.read(&mut buffer).unwrap();
            let request = String::from_utf8_lossy(&buffer[..size]);
            assert!(
                request
                    .starts_with("POST /api/w/workspace-a/tickets/01TEST/thread-events HTTP/1.1")
            );
            assert!(!request.contains("\"operation\""));
            assert!(request.contains("\"kind\":\"comment\""));
            stream
                .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
        });
        let client = Arc::new(crate::worker::TestWorkspaceHttpClient::new(
            "workspace-a",
            format!("http://{address}"),
        ));
        let backend = WorkspaceHttpTicketBackend::new(client);

        backend
            .add_event(
                TicketIdOrSlug::Id("01TEST".to_string()),
                NewTicketEvent::new(ticket::TicketEventKind::Comment, "REST comment"),
            )
            .unwrap();
        server.join().unwrap();
    }

    #[test]
    fn workspace_http_backend_records_relation_with_authoritative_keys() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            for (expected_path, resource_key) in [
                ("GET /api/w/workspace-a/tickets/01SOURCE HTTP/1.1", "T-1"),
                ("GET /api/w/workspace-a/tickets/01TARGET HTTP/1.1", "T-2"),
            ] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buffer = [0_u8; 8192];
                let len = stream.read(&mut buffer).unwrap();
                let request = String::from_utf8_lossy(&buffer[..len]);
                assert!(request.starts_with(expected_path));
                let body = serde_json::json!({"meta": {"resource_key": resource_key}}).to_string();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(), body
                )
                .unwrap();
            }

            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0_u8; 8192];
            let len = stream.read(&mut buffer).unwrap();
            let request = String::from_utf8_lossy(&buffer[..len]);
            assert!(
                request.starts_with("POST /api/w/workspace-a/tickets/01SOURCE/relations HTTP/1.1")
            );
            let body = serde_json::to_string(&TicketRelation {
                ticket_id: "01SOURCE".to_string(),
                kind: TicketRelationKind::DependsOn,
                target: "01TARGET".to_string(),
                note: None,
                author: "worker-internal".to_string(),
                at: "2026-08-06T00:00:00Z".to_string(),
            })
            .unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        let backend = WorkspaceHttpTicketBackend::new(Arc::new(
            crate::worker::TestWorkspaceHttpClient::new("workspace-a", format!("http://{addr}")),
        ));

        let relation = backend
            .add_ticket_relation(
                TicketIdOrSlug::Id("01SOURCE".to_string()),
                NewTicketRelation {
                    kind: TicketRelationKind::DependsOn,
                    target: "01TARGET".to_string(),
                    note: None,
                    author: None,
                },
            )
            .unwrap();

        server.join().unwrap();
        assert_eq!(relation.ticket_id, "T-1");
        assert_eq!(relation.target, "T-2");
        assert_eq!(relation.author, "workspace");
    }

    #[test]
    fn workspace_http_backend_deletes_exact_ticket_relation() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            for (expected_path, resource_key) in [
                ("GET /api/w/workspace-a/tickets/01SOURCE HTTP/1.1", "T-1"),
                ("GET /api/w/workspace-a/tickets/01TARGET HTTP/1.1", "T-2"),
            ] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buffer = [0_u8; 8192];
                let len = stream.read(&mut buffer).unwrap();
                let request = String::from_utf8_lossy(&buffer[..len]);
                assert!(request.starts_with(expected_path));
                let response_body = serde_json::json!({
                    "meta": {"resource_key": resource_key}
                })
                .to_string();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    response_body.len(),
                    response_body
                )
                .unwrap();
            }

            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0_u8; 8192];
            let len = stream.read(&mut buffer).unwrap();
            let request = String::from_utf8_lossy(&buffer[..len]);
            assert!(
                request
                    .starts_with("DELETE /api/w/workspace-a/tickets/01SOURCE/relations HTTP/1.1")
            );
            assert!(request.contains("\"kind\":\"depends_on\""));
            assert!(request.contains("\"target\":\"01TARGET\""));
            let response_body = serde_json::to_string(&TicketRelation {
                ticket_id: "01SOURCE".to_string(),
                kind: TicketRelationKind::DependsOn,
                target: "01TARGET".to_string(),
                note: Some("obsolete".to_string()),
                author: "tester".to_string(),
                at: "2026-08-06T00:00:00Z".to_string(),
            })
            .unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                response_body.len(),
                response_body
            )
            .unwrap();
        });

        let backend = WorkspaceHttpTicketBackend::new(Arc::new(
            crate::worker::TestWorkspaceHttpClient::new("workspace-a", base_url),
        ));
        let removed = backend
            .remove_ticket_relation(
                TicketIdOrSlug::Id("01SOURCE".to_string()),
                TicketRelationKind::DependsOn,
                TicketIdOrSlug::Id("01TARGET".to_string()),
            )
            .unwrap();

        server.join().unwrap();
        assert_eq!(removed.ticket_id, "T-1");
        assert_eq!(removed.target, "T-2");
    }

    #[test]
    fn workspace_ticket_service_preserves_internal_identity_for_handoff() {
        let temp = tempfile::tempdir().unwrap();
        let sqlite =
            ticket::SqliteTicketBackend::open(temp.path().join("tickets.db"), "workspace-test")
                .unwrap();
        let created = sqlite.create(NewTicket::new("Ticket handoff")).unwrap();
        let mut ticket = sqlite.show(TicketIdOrSlug::Id(created.id.clone())).unwrap();
        ticket.meta.resource_key = Some("T-548".to_string());
        ticket.meta.workflow_state = TicketWorkflowState::Queued;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let response_body = serde_json::to_string(&ticket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0_u8; 8192];
            let len = stream.read(&mut buffer).unwrap();
            let request = String::from_utf8_lossy(&buffer[..len]);
            assert!(request.starts_with("GET /api/w/workspace-a/tickets/T-548/record HTTP/1.1"));
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                response_body.len(),
                response_body
            )
            .unwrap();
        });

        let service = WorkspaceTicketService {
            backend: WorkspaceHttpTicketBackend::new(Arc::new(
                crate::worker::TestWorkspaceHttpClient::new("workspace-a", base_url),
            )),
        };
        let handoff = service.ticket_handoff("T-548").unwrap();

        server.join().unwrap();
        assert_eq!(handoff.id, created.id);
        assert_eq!(handoff.resource_key, "T-548");
        assert_eq!(handoff.workflow_state, TicketWorkflowState::Queued);
    }

    #[test]
    fn ticket_handoff_accepts_only_canonical_ticket_resource_keys() {
        assert!(is_canonical_ticket_resource_key("T-482"));
        for invalid in ["", "00001KZVNXFNK", "T-", "T-key", "O-482"] {
            assert!(!is_canonical_ticket_resource_key(invalid));
        }
    }

    #[test]
    fn workspace_http_backend_executes_ticket_create_operation() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0_u8; 8192];
            let len = stream.read(&mut buffer).unwrap();
            let request = String::from_utf8_lossy(&buffer[..len]);
            assert!(request.starts_with("POST /api/w/workspace-a/tickets HTTP/1.1"));
            assert!(!request.contains("\"operation\""));
            assert!(request.contains("\"title\":\"HTTP ticket\""));
            let response_body = serde_json::to_string(&TicketRef {
                id: "01TEST".to_string(),
                resource_key: None,
                slug: "http-ticket".to_string(),
                status: ticket::TicketStatus::Open,
            })
            .unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                response_body.len(),
                response_body
            )
            .unwrap();
        });

        let backend = WorkspaceHttpTicketBackend::new(Arc::new(
            crate::worker::TestWorkspaceHttpClient::new("workspace-a", base_url),
        ));
        let created = backend.create(NewTicket::new("HTTP ticket")).unwrap();

        server.join().unwrap();
        assert_eq!(created.id, "01TEST");
        assert_eq!(created.slug, "http-ticket");
    }
}
