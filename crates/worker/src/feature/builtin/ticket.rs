//! Ticket Feature and native WIP projection connected to the Workspace API.
//!
//! The `ticket` crate owns the Ticket domain and regular Tools; the Backend owns Workspace state,
//! relationships, and audit. This module connects a scoped Workspace client to the enabled Tools.
//! Shared WIP references, permissions, and unknown outcomes follow [`crate::wip`].
//!
//! # Route-bound subjects
//!
//! The native projection exposes `/tickets` and one immediate child segment. Items accept canonical
//! `T-*` or canonical internal IDs, never resolving Objective keys or deeper suffixes as Tickets.
//! Item descriptors omit subject `ticket`/`id`, reconstructing original Tool arguments only from the
//! resolved route. Related/dependency Tickets remain separate inputs resolved by the Backend.
//! Operations come from regular Tools actually enabled by `TicketFeatureAccess`, retaining original
//! schemas and permission identities. WIP adds neither a broader business catalog nor another workflow.
//!
//! # Independent judgments and delegation
//!
//! Case judgments, classification, delegation handling, delivery/start evidence, and Worker
//! lifecycle retain their separate Backend contracts. Case updates compare content, state, and
//! the explicitly observed nullable closure key; they require no universal MR approval and create
//! none. Handling done neither closes a case nor releases work; reopening does not reissue a
//! delegation or restore execution rights. Dependencies remain diagnostics, not blanket start gates.
//! Classification mutations use stable tag IDs, tag-operation CAS, or full membership-set CAS,
//! never display names, case state, or the editable content digest as classification authority.
//!
//! # Bounded responses and observation invalidation
//!
//! Query returns bounded snippets and canonical item paths; read returns bounded Backend details.
//! Objectives and Merge Requests remain references, without expanding their content or native routes.
//! Successful item-read response hashes feed route validators; canonical/internal aliases share an
//! observation group. Mutation receipts are not complete representations, so updates discard
//! observation hashes. Related targets and targets in queue results are also invalidated; no mutation
//! counter is created. These local observations replace neither Backend item/state CAS nor current
//! authorization.
//!
//! Fetching canonical references after mutations uses failure classes acknowledging that writes may
//! already have succeeded. HTTP failures at that stage are not misclassified as "unexecuted".
//! Optional arguments distinguish omission from explicit null for original JSON Schema/permission
//! evaluation; defaults use the same implementation as regular Tools.

mod assignment_public;
mod assignment_tracking;
mod notification_diagnostics;
mod tags;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use manifest::{ToolPermissionAction, ToolPermissionConfig};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use ticket::{
    NewOrchestrationPlanRecord, NewTicket, NewTicketEvent, NewTicketRelation,
    OrchestrationPlanKind, OrchestrationPlanRecord, Result as TicketResult, Ticket, TicketBackend,
    TicketBackendOperation, TicketBackendOperationResult, TicketDoctorReport, TicketError,
    TicketIdOrSlug, TicketIntakeSummary, TicketListQuery, TicketRef, TicketRelation,
    TicketRelationKind, TicketRelationView, TicketState, TicketStateChange, TicketSummary,
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
                "Show one authoritative Workspace Ticket with its ordered implementation targets (repository_key, ref_selector, access), content digest, paged thread, links, historical implementation reports, and current Merge Request readiness evidence. Read targets here before choosing a work repository; a null selector remains unspecified, not an inferred default."
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
    UnresolvedReview,
    MissingCommit,
    Blocked,
    Unblocked,
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
#[serde(deny_unknown_fields)]
struct WorkspaceQueryTicketInput {
    /// Exact current closed-case reason; reopened/history-only reasons do not match.
    closure_reason: Option<ticket::TicketClosureReason>,
    /// Exact current assigned principal; retained relationships do not match.
    assigned_principal: Option<ticket::TicketAssignmentPrincipalSnapshot>,
    /// Explicit self-reported handling, never inferred from Worker/MR/delivery.
    handling_status: Option<ticket::TicketAssignmentTrackingStatus>,
    /// Full-text match over Ticket title, item body, and bounded thread excerpts.
    query: Option<String>,
    /// Exact case states (open or closed). Empty means every state.
    #[serde(default)]
    states: Vec<TicketState>,
    /// Exact typed event kinds that must occur in the bounded thread window.
    #[serde(default)]
    event_kinds: Vec<String>,
    /// Required evidence kinds: merge_request, commit, or approved_review.
    #[serde(default)]
    evidence: Vec<WorkspaceTicketEvidenceFilter>,
    /// Current authoritative Merge Request review status: none, pending, approved,
    /// request_changes, or unresolved_changes.
    review_status: Option<WorkspaceTicketReviewFilter>,
    /// Attention filters include unresolved_review, missing_commit,
    /// blocked, unblocked, awaiting_review, unresolved_changes,
    /// stale_after_rescope, and missing_evidence.
    #[serde(default)]
    attention: Vec<WorkspaceTicketAttentionFilter>,
    /// Related Ticket reference. Prefer `T-*`; canonical internal ids remain accepted for compatibility.
    related_ticket_id: Option<String>,
    relation_kind: Option<WorkspaceTicketRelationFilter>,
    /// All stable tag IDs must be attached (AND); at most 100 unique IDs.
    #[serde(default)]
    #[schemars(length(max = 100))]
    tag_ids: Vec<String>,
    /// At least one stable tag ID must be attached (OR), unless empty.
    #[serde(default)]
    #[schemars(length(max = 100))]
    tag_ids_any: Vec<String>,
    /// None of these stable tag IDs may be attached (NOT).
    #[serde(default)]
    #[schemars(length(max = 100))]
    tag_ids_none: Vec<String>,
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
        let (path, body, target) = match self.kind {
            WorkspaceTicketReadKind::Query => {
                let input: WorkspaceQueryTicketInput = serde_json::from_str(&input)
                    .map_err(|error| ToolError::InvalidArgument(error.to_string()))?;
                for (name, ids) in [
                    ("tag_ids", &input.tag_ids),
                    ("tag_ids_any", &input.tag_ids_any),
                    ("tag_ids_none", &input.tag_ids_none),
                ] {
                    tags::validate_ids(name, ids)?;
                }
                (
                    format!("/api/w/{workspace_id}/tickets/query"),
                    serde_json::to_value(input)
                        .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?,
                    None,
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
                let path = format!(
                    "/api/w/{workspace_id}/tickets/{}/show",
                    WorkspaceHttpTicketBackend::ticket_path(&TicketIdOrSlug::from(input.id.trim()))
                );
                let body = json!({
                    "event_limit": input.event_limit,
                    "event_cursor": input.event_cursor,
                });
                (path, body, Some(TicketIdOrSlug::from(input.id.trim())))
            }
        };
        match target {
            Some(id) => authorize_ticket_item(self.client.as_ref(), "ticket.read", &id),
            None => authorize_ticket_operation(self.client.as_ref(), "ticket.query"),
        }
        .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
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
            return Err(ToolError::ExecutionFailed(ticket_api_error_message(
                response.status,
                &response.body,
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
struct WorkspaceTicketCaseUpdateInput {
    /// Ticket reference. Prefer T-*; WIP binds this from the item route.
    ticket: String,
    /// Stable key for exact replay of this case judgment.
    operation_key: String,
    /// SHA-256 of the current editable Ticket content from ShowTicket.
    expected_content_digest: String,
    expected_state: ticket::TicketState,
    /// Explicitly observed null for never closed, or current closure operation key.
    #[serde(deserialize_with = "deserialize_observed_closure_key")]
    expected_closure_operation_key: Option<String>,
    action: ticket::TicketCaseAction,
    /// Required explanation of the judgment, independent of Repository/MR evidence.
    explanation: String,
    #[serde(default)]
    references: Vec<ticket::TicketReference>,
}

fn deserialize_observed_closure_key<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(deserializer)
}

const TICKET_CASE_UPDATE_DESCRIPTION: &str = "Record an authorized Ticket close, reopen, or closure-correction judgment with an explicit observed closure key, item/state CAS, and replay key. Open means the case remains to be handled, not ready/queued/inprogress; Closed does not imply success. Repository, Merge Request and approval evidence are not prerequisites. Return without conclusion need not call this tool. Backend checks actor authority, not real judgment of natural-language satisfaction; actor identity is injected by the server. This does not start/stop Workers, restore execution rights, change assignments, grant resources, approve or integrate Merge Requests.";

#[derive(Clone, Copy)]
enum WorkspaceTicketDecisionKind {
    CaseUpdate,
}

impl WorkspaceTicketDecisionKind {
    fn name(self) -> &'static str {
        "TicketCaseUpdate"
    }
    fn description(self) -> &'static str {
        TICKET_CASE_UPDATE_DESCRIPTION
    }
    fn schema(self) -> Value {
        let mut schema = json!(schemars::schema_for!(WorkspaceTicketCaseUpdateInput));
        // Observation must be present even when it is explicitly null. Keep the
        // Option's nullable wire schema instead of treating it as a String.
        let required = schema["required"]
            .as_array_mut()
            .expect("object schema required fields");
        if !required
            .iter()
            .any(|field| field == "expected_closure_operation_key")
        {
            required.push(json!("expected_closure_operation_key"));
        }
        schema
    }
}

#[derive(Clone)]
struct WorkspaceTicketDecisionTool {
    client: Arc<dyn WorkspaceClient>,
    kind: WorkspaceTicketDecisionKind,
}

#[async_trait]
impl Tool for WorkspaceTicketDecisionTool {
    async fn execute(&self, input: &str, _: ToolExecutionContext) -> Result<ToolOutput, ToolError> {
        self.client.workspace_id().ok_or_else(|| {
            ToolError::InvalidArgument("Ticket decisions require Workspace identity".into())
        })?;
        let value: WorkspaceTicketCaseUpdateInput = serde_json::from_str(input)
            .map_err(|error| ToolError::InvalidArgument(error.to_string()))?;
        for (name, value) in [
            ("ticket", value.ticket.as_str()),
            ("operation_key", value.operation_key.as_str()),
            (
                "expected_content_digest",
                value.expected_content_digest.as_str(),
            ),
            ("explanation", value.explanation.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(ToolError::InvalidArgument(format!(
                    "{name} must not be empty"
                )));
            }
        }
        if value
            .expected_closure_operation_key
            .as_deref()
            .is_some_and(|key| key.trim().is_empty())
        {
            return Err(ToolError::InvalidArgument(
                "expected_closure_operation_key must be null or nonempty".into(),
            ));
        }
        match &value.action {
            ticket::TicketCaseAction::Close { reason }
            | ticket::TicketCaseAction::CorrectClosure { reason }
                if matches!(reason, ticket::TicketClosureReason::Other(text) if text.trim().is_empty()) =>
            {
                return Err(ToolError::InvalidArgument(
                    "other closure reason must not be empty".into(),
                ));
            }
            _ => {}
        }
        let mut body =
            serde_json::to_value(&value).map_err(|error| ToolError::Internal(error.to_string()))?;
        body.as_object_mut()
            .expect("typed Ticket input is an object")
            .remove("ticket");
        // Actor identity is transport-bound, never a model-visible author/role input.
        let ticket = WorkspaceHttpTicketBackend::new(self.client.clone())
            .update_case(
                &value.ticket,
                serde_json::from_value(body)
                    .map_err(|error| ToolError::InvalidArgument(error.to_string()))?,
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
        "TicketCaseUpdate" => TICKET_CASE_UPDATE_DESCRIPTION.into(),
        "NotificationDiagnostics" => notification_diagnostics::DESCRIPTION.into(),
        name if assignment_public::TOOL_NAMES.contains(&name) => {
            assignment_public::description(name).into()
        }
        name if assignment_tracking::TOOL_NAMES.contains(&name) => {
            assignment_tracking::description(name).into()
        }
        name if tags::TAG_TOOL_NAMES.contains(&name) => tags::description(name).into(),
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

    /// Typed creation without a synthetic Tool context. The default keeps legacy
    /// handoff-only fixture implementations fail-closed; production installs
    /// provide creation through the shared authorized backend dispatch.
    fn create(&self, _input: NewTicket) -> TicketResult<TicketRef> {
        Err(TicketError::Conflict(
            "Ticket creation service is unavailable".into(),
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketHandoff {
    pub id: String,
    pub resource_key: String,
    pub workflow_state: TicketState,
}

fn is_canonical_ticket_resource_key(resource_key: &str) -> bool {
    resource_key.strip_prefix("T-").is_some_and(|sequence| {
        !sequence.is_empty() && sequence.bytes().all(|byte| byte.is_ascii_digit())
    })
}

struct WorkspaceTicketService {
    backend: WorkspaceHttpTicketBackend,
    access: TicketFeatureAccess,
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

    fn create(&self, input: NewTicket) -> TicketResult<TicketRef> {
        // Services retain the same Profile ceiling as Tools and WIP. Host
        // permission cannot turn a read-only Feature surface into an author.
        if !self.access.authoring {
            return Err(TicketError::Conflict(
                "Ticket creation is disabled by the Profile authoring policy".into(),
            ));
        }
        self.backend.create(input)
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
            .chain(tags::TAG_TOOL_NAMES.iter().copied().filter(|name| {
                *name == "WorkspaceTagList" || self.authoring || self.workflow || self.thread
            }))
            .chain(
                assignment_tracking::TOOL_NAMES
                    .iter()
                    .copied()
                    .filter(|name| *name != "TicketAssignmentTrackingUpdate" || self.workflow),
            )
            .chain(
                assignment_public::TOOL_NAMES
                    .iter()
                    .copied()
                    .filter(|name| {
                        assignment_public::READ_TOOL_NAMES.contains(name)
                            || self.authoring
                            || self.workflow
                            || (matches!(
                                *name,
                                "TicketAssignmentAnswer" | "TicketImplementationResult"
                            ) && self.thread)
                    }),
            )
            .chain(std::iter::once(notification_diagnostics::NAME))
            .collect()
    }

    fn allows_tool(self, name: &str) -> bool {
        // Profile tool policy controls case-decision access in both modes.
        // The Backend verifies actor/Workspace identity and CAS/replay, not
        // orchestration assignment as an additional case-decision permission.
        READ_ONLY_TOOL_NAMES.contains(&name)
            || (name == "TicketCaseUpdate" && (self.authoring || self.thread || self.workflow))
            || (self.authoring && AUTHORING_TOOL_NAMES.contains(&name))
            || (self.thread && THREAD_TOOL_NAMES.contains(&name))
            || (self.workflow && WORKFLOW_ADDITIONAL_TOOL_NAMES.contains(&name))
    }
}

const READ_ONLY_TOOL_NAMES: &[&str] = &[
    "QueryTicket",
    "ShowTicket",
    "WorkspaceTagList",
    "TicketAssignmentTrackingQuery",
    "TicketAssignmentTrackingHistory",
    "TicketAssignmentsQuery",
    "TicketAssignmentDiagnostics",
    "TicketImplementationResultQuery",
    "NotificationDiagnostics",
];

const AUTHORING_TOOL_NAMES: &[&str] = &[
    "TicketCreate",
    "TicketEditItem",
    "TicketEditPost",
    "TicketCaseUpdate",
    "TicketRelationRecord",
    "TicketRelationRemove",
];

const THREAD_TOOL_NAMES: &[&str] = &["TicketComment", "TicketImplementationResult"];

#[cfg(test)]
const WORKSPACE_AUTHORING_TOOL_NAMES: &[&str] = &[
    "TicketCreate",
    "TicketEditItem",
    "TicketEditPost",
    "QueryTicket",
    "ShowTicket",
    "TicketComment",
    "TicketCaseUpdate",
    "TicketRelationRecord",
    "TicketRelationRemove",
    "WorkspaceTagList",
    "WorkspaceTagCreate",
    "WorkspaceTagRename",
    "WorkspaceTagDelete",
    "TicketTagsSet",
    "TicketAssignmentTrackingQuery",
    "TicketAssignmentTrackingHistory",
    "TicketOrchestratorAssign",
    "TicketOrchestratorClear",
    "TicketAssignmentsQuery",
    "TicketAssignmentDiagnostics",
    "TicketAssignmentAnswer",
    "TicketImplementationResult",
    "TicketImplementationResultQuery",
    "NotificationDiagnostics",
];

#[cfg(test)]
const WORKFLOW_TOOL_NAMES: &[&str] = &[
    "QueryTicket",
    "ShowTicket",
    "TicketComment",
    "TicketCaseUpdate",
    "TicketDependencyCheck",
    "TicketRelationRecord",
    "TicketRelationRemove",
    "TicketOrchestrationPlanRecord",
    "TicketOrchestrationPlanQuery",
    "WorkspaceTagList",
    "WorkspaceTagCreate",
    "WorkspaceTagRename",
    "WorkspaceTagDelete",
    "TicketTagsSet",
    "TicketAssignmentTrackingQuery",
    "TicketAssignmentTrackingHistory",
    "TicketAssignmentTrackingUpdate",
    "TicketOrchestratorAssign",
    "TicketOrchestratorClear",
    "TicketAssignmentsQuery",
    "TicketAssignmentDiagnostics",
    "TicketAssignmentAnswer",
    "TicketImplementationResult",
    "TicketImplementationResultQuery",
    "NotificationDiagnostics",
];

const WORKFLOW_ADDITIONAL_TOOL_NAMES: &[&str] = &[
    "TicketCaseUpdate",
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
        // Installation only registers local surfaces. Protected operations recheck
        // current Host authority at each backend access, including after restore.
        let ticket_service: Arc<dyn TicketService> = Arc::new(WorkspaceTicketService {
            backend: WorkspaceHttpTicketBackend::new(workspace_client.clone()),
            access: self.access,
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

/// Collection operations use Workspace authority; provider actor, Ticket identity,
/// CAS and replay checks remain independent. No decision is cached across calls.
fn authorize_ticket_operation(client: &dyn WorkspaceClient, operation: &str) -> TicketResult<()> {
    let workspace_id = client
        .workspace_id()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| {
            TicketError::Conflict("Ticket authorization requires Workspace identity".into())
        })?;
    client
        .authorize_operation(
            operation,
            &server_api::authorization::PermissionResource::Workspace {
                workspace_id: workspace_id.to_owned(),
            },
        )
        .map_err(|error| {
            TicketError::Conflict(format!("Ticket operation authorization failed: {error}"))
        })
}

fn authorize_ticket_item(
    client: &dyn WorkspaceClient,
    operation: &str,
    id: &TicketIdOrSlug,
) -> TicketResult<()> {
    let ticket_id = match id {
        TicketIdOrSlug::Id(value) | TicketIdOrSlug::Slug(value) | TicketIdOrSlug::Query(value) => {
            value
        }
    };
    assignment_tracking::authorize_ticket_target(client, operation, ticket_id)
        .map_err(|error| TicketError::Conflict(error.to_string()))
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
        let id = TicketIdOrSlug::from(ticket_ref);
        let ticket_path = Self::ticket_path(&id);
        let request = move || {
            authorize_ticket_item(client.as_ref(), "ticket.read", &id)?;
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
            return Err(TicketError::Conflict(ticket_api_error_message(
                response.status,
                &response.body,
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
        authorize_ticket_item(client.as_ref(), "ticket.read", reference)?;
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
            return Err(TicketError::Conflict(ticket_api_error_message(
                response.status,
                &response.body,
            )));
        }
        Ok(TicketBackendOperationResult::Unit)
    }

    fn invoke_client(
        client: Arc<dyn WorkspaceClient>,
        workspace_id: String,
        operation: TicketBackendOperation,
    ) -> TicketResult<TicketBackendOperationResult> {
        let base = format!("/api/w/{workspace_id}/tickets");
        match operation {
            TicketBackendOperation::ListWorkspaceTags { request } => {
                authorize_ticket_operation(client.as_ref(), "ticket.tag.list")?;
                let mut endpoint = format!(
                    "/api/w/{workspace_id}/ticket-tags?limit={}",
                    request.limit.unwrap_or(200).min(200)
                );
                if let Some(after) = request.after {
                    endpoint.push_str("&after=");
                    endpoint.push_str(&Self::ticket_path(&TicketIdOrSlug::Id(after)));
                }
                Self::request(client, WorkspaceRequestMethod::Get, endpoint, None)
                    .map(TicketBackendOperationResult::WorkspaceTags)
            }
            TicketBackendOperation::MutateWorkspaceTag { request } => {
                let operation = match &request.action {
                    ticket::WorkspaceTagAction::Create { .. } => "ticket.tag.create",
                    ticket::WorkspaceTagAction::Rename { .. } => "ticket.tag.rename",
                    ticket::WorkspaceTagAction::Delete { .. } => "ticket.tag.delete",
                };
                authorize_ticket_operation(client.as_ref(), operation)?;
                Self::request(
                    client,
                    WorkspaceRequestMethod::Post,
                    format!("/api/w/{workspace_id}/ticket-tags"),
                    Some(json!(request)),
                )
                .map(TicketBackendOperationResult::WorkspaceTag)
            }
            TicketBackendOperation::SetTicketTags { id, request } => {
                authorize_ticket_item(client.as_ref(), "ticket.tags.set", &id)?;
                Self::request(
                    client,
                    WorkspaceRequestMethod::Put,
                    format!("{base}/{}/tags", Self::ticket_path(&id)),
                    Some(json!(request)),
                )
                .map(TicketBackendOperationResult::WorkspaceTags)
            }
            TicketBackendOperation::DefaultIntakeReadyStateChangeBody { from } => {
                authorize_ticket_operation(client.as_ref(), "ticket.intake.default_body")?;
                let value = Self::request::<String>(
                    client,
                    WorkspaceRequestMethod::Post,
                    format!("{base}/default-intake-ready-body"),
                    Some(serde_json::json!({ "from": from })),
                )?;
                Ok(TicketBackendOperationResult::Text(value))
            }
            TicketBackendOperation::List { filter } => {
                authorize_ticket_operation(client.as_ref(), "ticket.list")?;
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
                authorize_ticket_item(client.as_ref(), "ticket.read", &id)?;
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
                if input.workflow_state == Some(TicketState::Closed) {
                    return Err(TicketError::Conflict(
                        "Tickets must be created open; closing requires a case judgment".into(),
                    ));
                }
                authorize_ticket_operation(client.as_ref(), "ticket.create")?;
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
                authorize_ticket_item(client.as_ref(), "ticket.edit_item", &id)?;
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
            TicketBackendOperation::EditPost { id, edit } => {
                authorize_ticket_item(client.as_ref(), "ticket.edit_post", &id)?;
                let (previous, current): (Ticket, Ticket) = Self::request(
                    client,
                    WorkspaceRequestMethod::Patch,
                    format!("{base}/{}/posts", Self::ticket_path(&id)),
                    Some(serde_json::to_value(edit).map_err(|error| {
                        TicketError::Conflict(format!("serialize Ticket post edit: {error}"))
                    })?),
                )?;
                Ok(TicketBackendOperationResult::TicketSnapshots { previous, current })
            }
            TicketBackendOperation::DependencyCheck { id } => {
                authorize_ticket_item(client.as_ref(), "ticket.dependency.check", &id)?;
                let check = Self::request(
                    client,
                    WorkspaceRequestMethod::Get,
                    format!("{base}/{}/dependency-check", Self::ticket_path(&id)),
                    None,
                )?;
                Ok(TicketBackendOperationResult::DependencyCheck(check))
            }
            TicketBackendOperation::AddEvent { id, event } => {
                authorize_ticket_item(client.as_ref(), "ticket.event.add", &id)?;
                Self::request_unit(
                    client,
                    WorkspaceRequestMethod::Post,
                    format!("{base}/{}/thread-events", Self::ticket_path(&id)),
                    Some(serde_json::to_value(event).map_err(|error| {
                        TicketError::Conflict(format!("serialize Ticket event: {error}"))
                    })?),
                )
            }
            TicketBackendOperation::AddStateChanged { .. } => Err(TicketError::Conflict(
                "Case changes require an explicit TicketCaseUpdate judgment".into(),
            )),
            TicketBackendOperation::AddIntakeSummary { id, summary } => {
                authorize_ticket_item(client.as_ref(), "ticket.intake_summary.add", &id)?;
                Self::request_unit(
                    client,
                    WorkspaceRequestMethod::Post,
                    format!("{base}/{}/intake-summaries", Self::ticket_path(&id)),
                    Some(serde_json::to_value(summary).map_err(|error| {
                        TicketError::Conflict(format!("serialize Ticket intake summary: {error}"))
                    })?),
                )
            }
            TicketBackendOperation::UpdateCase { ticket, request } => {
                let id = TicketIdOrSlug::from(ticket.as_str());
                authorize_ticket_item(client.as_ref(), "ticket.case.update", &id)?;
                let mut body = serde_json::to_value(request).map_err(|error| {
                    TicketError::Conflict(format!("serialize Ticket case judgment: {error}"))
                })?;
                authorize_ticket_item(client.as_ref(), "ticket.read", &id)?;
                // Domain callers cannot supply HTTP actor authority either.
                body.as_object_mut()
                    .expect("typed case update is an object")
                    .remove("author");
                let encoded_ticket = Self::ticket_path(&TicketIdOrSlug::from(ticket.as_str()));
                authorize_ticket_item(client.as_ref(), "ticket.case.update", &id)?;
                let _: ticket::TicketSummary = Self::request(
                    client.clone(),
                    WorkspaceRequestMethod::Post,
                    format!("/workspaces/{workspace_id}/tickets/{encoded_ticket}/case"),
                    Some(body),
                )?;
                // The mutation receipt returns a bounded summary. Read the full
                // current authority; never fabricate history from that summary.
                authorize_ticket_item(client.as_ref(), "ticket.read", &id).map_err(|error| {
                    TicketError::Conflict(format!("Ticket case judgment may already have succeeded; follow-up read authorization failed: {error}"))
                })?;
                Self::request(
                    client,
                    WorkspaceRequestMethod::Get,
                    format!("{base}/{encoded_ticket}/record"),
                    None,
                )
                .map(TicketBackendOperationResult::Ticket)
            }
            TicketBackendOperation::MarkReady { id, request } => {
                authorize_ticket_item(client.as_ref(), "ticket.mark_ready", &id)?;
                Self::request(
                    client,
                    WorkspaceRequestMethod::Post,
                    format!("{base}/{}/workflow/mark-ready", Self::ticket_path(&id)),
                    Some(serde_json::to_value(request).map_err(|error| {
                        TicketError::Conflict(format!(
                            "serialize Ticket mark-ready request: {error}"
                        ))
                    })?),
                )
                .map(TicketBackendOperationResult::Ticket)
            }
            TicketBackendOperation::QueueReady { id, .. } => {
                authorize_ticket_item(client.as_ref(), "ticket.queue", &id)?;
                Self::request(
                    client,
                    WorkspaceRequestMethod::Post,
                    format!("{base}/{}/workflow/queue", Self::ticket_path(&id)),
                    None,
                )
                .map(TicketBackendOperationResult::QueueOutcome)
            }
            TicketBackendOperation::AddTicketRelation { id, relation } => {
                let target_id = TicketIdOrSlug::from(relation.target.as_str());
                authorize_ticket_item(client.as_ref(), "ticket.relation.record", &id)?;
                authorize_ticket_item(client.as_ref(), "ticket.relation.record", &target_id)?;
                let source_resource_key =
                    Self::resolve_ticket_resource_key(client.clone(), &base, &id)?;
                let target_resource_key = Self::resolve_ticket_resource_key(
                    client.clone(),
                    &base,
                    &TicketIdOrSlug::Id(relation.target.clone()),
                )?;
                authorize_ticket_item(client.as_ref(), "ticket.relation.record", &id)?;
                authorize_ticket_item(client.as_ref(), "ticket.relation.record", &target_id)?;
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
                let target_id = target.clone();
                authorize_ticket_item(client.as_ref(), "ticket.relation.remove", &id)?;
                authorize_ticket_item(client.as_ref(), "ticket.relation.remove", &target_id)?;
                let source_resource_key =
                    Self::resolve_ticket_resource_key(client.clone(), &base, &id)?;
                let target_resource_key =
                    Self::resolve_ticket_resource_key(client.clone(), &base, &target)?;
                let target = match target {
                    TicketIdOrSlug::Id(value)
                    | TicketIdOrSlug::Slug(value)
                    | TicketIdOrSlug::Query(value) => value,
                };
                authorize_ticket_item(client.as_ref(), "ticket.relation.remove", &id)?;
                authorize_ticket_item(client.as_ref(), "ticket.relation.remove", &target_id)?;
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
                match &ticket {
                    Some(id) => authorize_ticket_item(client.as_ref(), "ticket.relation.query", id),
                    None => authorize_ticket_operation(client.as_ref(), "ticket.relation.query"),
                }?;
                let relations = Self::request(
                    client,
                    WorkspaceRequestMethod::Post,
                    format!("{base}/relations/search"),
                    Some(serde_json::json!({ "ticket": ticket, "kind": kind })),
                )?;
                Ok(TicketBackendOperationResult::Relations(relations))
            }
            TicketBackendOperation::RelationView { id } => {
                authorize_ticket_item(client.as_ref(), "ticket.relation.read", &id)?;
                let view = Self::request(
                    client,
                    WorkspaceRequestMethod::Get,
                    format!("{base}/{}/relation-view", Self::ticket_path(&id)),
                    None,
                )?;
                Ok(TicketBackendOperationResult::RelationView(view))
            }
            TicketBackendOperation::AddOrchestrationPlanRecord { id, record } => {
                authorize_ticket_item(client.as_ref(), "ticket.orchestration_plan.record", &id)?;
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
                match &ticket {
                    Some(id) => authorize_ticket_item(
                        client.as_ref(),
                        "ticket.orchestration_plan.query",
                        id,
                    ),
                    None => authorize_ticket_operation(
                        client.as_ref(),
                        "ticket.orchestration_plan.query",
                    ),
                }?;
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
                authorize_ticket_operation(client.as_ref(), "ticket.doctor")?;
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
    fn edit_post_with_snapshots(
        &self,
        id: TicketIdOrSlug,
        edit: ticket::TicketPostEdit,
    ) -> TicketResult<(Ticket, Ticket)> {
        match self.invoke(TicketBackendOperation::EditPost { id, edit })? {
            TicketBackendOperationResult::TicketSnapshots { previous, current } => {
                Ok((previous, current))
            }
            other => Err(TicketError::Conflict(format!(
                "unexpected ticket post response: {other:?}"
            ))),
        }
    }

    fn list_workspace_tags(
        &self,
        request: ticket::TagListRequest,
    ) -> TicketResult<Vec<ticket::WorkspaceTag>> {
        expect_ticket_result!(
            self.invoke(TicketBackendOperation::ListWorkspaceTags { request }),
            TicketBackendOperationResult::WorkspaceTags
        )
    }
    fn mutate_workspace_tag(
        &self,
        request: ticket::WorkspaceTagMutation,
    ) -> TicketResult<ticket::WorkspaceTag> {
        expect_ticket_result!(
            self.invoke(TicketBackendOperation::MutateWorkspaceTag { request }),
            TicketBackendOperationResult::WorkspaceTag
        )
    }
    fn set_ticket_tags(
        &self,
        id: TicketIdOrSlug,
        request: ticket::TicketTagSetRequest,
    ) -> TicketResult<Vec<ticket::WorkspaceTag>> {
        expect_ticket_result!(
            self.invoke(TicketBackendOperation::SetTicketTags { id, request }),
            TicketBackendOperationResult::WorkspaceTags
        )
    }

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

    fn update_case(&self, ticket: &str, request: ticket::TicketCaseUpdate) -> TicketResult<Ticket> {
        expect_ticket_result!(
            self.invoke(TicketBackendOperation::UpdateCase {
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
        "WorkspaceTagList" => NativeTicketOperation {
            operation: "tag_list",
            surface: NativeTicketSurface::Collection,
            identity_field: None,
            mutating: false,
        },
        "WorkspaceTagCreate" => NativeTicketOperation {
            operation: "tag_create",
            surface: NativeTicketSurface::Collection,
            identity_field: None,
            mutating: true,
        },
        "WorkspaceTagRename" => NativeTicketOperation {
            operation: "tag_rename",
            surface: NativeTicketSurface::Collection,
            identity_field: None,
            mutating: true,
        },
        "WorkspaceTagDelete" => NativeTicketOperation {
            operation: "tag_delete",
            surface: NativeTicketSurface::Collection,
            identity_field: None,
            mutating: true,
        },
        "TicketTagsSet" => NativeTicketOperation {
            operation: "tags_set",
            surface: NativeTicketSurface::Item,
            identity_field: Some("ticket"),
            mutating: true,
        },
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
        "TicketEditPost" => NativeTicketOperation {
            operation: "edit_post",
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
        "TicketOrchestratorAssign" => NativeTicketOperation {
            operation: "orchestrator_assign",
            surface: NativeTicketSurface::Item,
            identity_field: Some("ticket"),
            mutating: true,
        },
        "TicketOrchestratorClear" => NativeTicketOperation {
            operation: "orchestrator_clear",
            surface: NativeTicketSurface::Item,
            identity_field: Some("ticket"),
            mutating: true,
        },
        "TicketAssignmentsQuery" => NativeTicketOperation {
            operation: "assignments_query",
            surface: NativeTicketSurface::Item,
            identity_field: Some("ticket"),
            mutating: false,
        },
        "NotificationDiagnostics" => NativeTicketOperation {
            operation: "notification_diagnostics",
            surface: NativeTicketSurface::Collection,
            identity_field: None,
            mutating: false,
        },
        "TicketAssignmentDiagnostics" => NativeTicketOperation {
            operation: "assignment_diagnostics",
            surface: NativeTicketSurface::Item,
            identity_field: Some("ticket"),
            mutating: false,
        },
        "TicketImplementationResult" => NativeTicketOperation {
            operation: "implementation_result",
            surface: NativeTicketSurface::Item,
            identity_field: Some("ticket"),
            mutating: true,
        },
        "TicketImplementationResultQuery" => NativeTicketOperation {
            operation: "implementation_result_query",
            surface: NativeTicketSurface::Item,
            identity_field: Some("ticket"),
            mutating: false,
        },
        "TicketAssignmentAnswer" => NativeTicketOperation {
            operation: "assignment_answer",
            surface: NativeTicketSurface::Item,
            identity_field: Some("ticket"),
            mutating: true,
        },
        "TicketAssignmentTrackingQuery" => NativeTicketOperation {
            operation: "assignment_tracking_query",
            surface: NativeTicketSurface::Item,
            identity_field: Some("ticket"),
            mutating: false,
        },
        "TicketAssignmentTrackingHistory" => NativeTicketOperation {
            operation: "assignment_tracking_history",
            surface: NativeTicketSurface::Item,
            identity_field: Some("ticket"),
            mutating: false,
        },
        "TicketAssignmentTrackingUpdate" => NativeTicketOperation {
            operation: "assignment_tracking_update",
            surface: NativeTicketSurface::Item,
            identity_field: Some("ticket"),
            mutating: true,
        },
        "TicketCaseUpdate" => NativeTicketOperation {
            operation: "case_update",
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
                "TicketCaseUpdate" => workspace_ticket_decision_definition(
                    client.clone(),
                    WorkspaceTicketDecisionKind::CaseUpdate,
                ),
                _ => definition,
            })
        })
        .collect();
    for kind in [
        tags::TagToolKind::List,
        tags::TagToolKind::Create,
        tags::TagToolKind::Rename,
        tags::TagToolKind::Delete,
        tags::TagToolKind::Set,
    ] {
        if allowed.contains(&kind.name()) {
            definitions.push(tags::definition(client.clone(), kind));
        }
    }
    for kind in [
        assignment_tracking::TrackingToolKind::Query,
        assignment_tracking::TrackingToolKind::History,
        assignment_tracking::TrackingToolKind::Update,
    ] {
        if allowed.contains(&kind.name()) {
            definitions.push(assignment_tracking::definition(client.clone(), kind));
        }
    }
    for kind in [
        assignment_public::PublicAssignmentKind::Assign,
        assignment_public::PublicAssignmentKind::Clear,
        assignment_public::PublicAssignmentKind::Assignments,
        assignment_public::PublicAssignmentKind::Diagnostics,
        assignment_public::PublicAssignmentKind::Answer,
        assignment_public::PublicAssignmentKind::ImplementationResult,
        assignment_public::PublicAssignmentKind::ImplementationResultQuery,
    ] {
        if allowed.contains(&kind.name()) {
            definitions.push(assignment_public::definition(client.clone(), kind));
        }
    }
    if allowed.contains(&notification_diagnostics::NAME) {
        definitions.push(notification_diagnostics::definition(client.clone()));
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
        // Classification has its own membership CAS and never changes item content.
        if operation != "tags_set" {
            record_ticket_observation(
                &self.observations,
                Some(&self.ticket_reference),
                &response,
                tool.projection.mutating,
            );
        }
        if tool.projection.mutating && operation != "tags_set" {
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

// Only render the public API error envelope, never an arbitrary proxy body or
// unknown fields. Backend owns redaction of its public message; bound it here
// and keep diagnostics to stable codes rather than raw provider details.
fn ticket_api_error_message(status: u16, body: &str) -> String {
    let mut message = format!("ticket REST API request failed with HTTP status {status}");
    if body.len() > 64 * 1024 {
        return message;
    }
    let Ok(error) = serde_json::from_str::<server_api::RepositoryApiError>(body) else {
        return message;
    };
    let detail = if error.message.trim().is_empty() {
        &error.error
    } else {
        &error.message
    };
    let mut chars = detail.trim().chars();
    let bounded: String = chars
        .by_ref()
        .take(1024)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if !bounded.is_empty() {
        message.push_str(": ");
        message.push_str(&bounded);
        if chars.next().is_some() {
            message.push('…');
        }
    }
    let codes: Vec<_> = error
        .diagnostics
        .iter()
        .take(4)
        .map(|diagnostic| diagnostic.code.as_str())
        .filter(|code| {
            !code.is_empty()
                && code.len() <= 64
                && code
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.'))
        })
        .collect();
    if !codes.is_empty() {
        message.push_str(" [");
        message.push_str(&codes.join(", "));
        message.push(']');
    }
    message
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
        ToolError::ExecutionFailed(message)
            if message.contains("Ticket case judgment may already have succeeded") =>
        {
            WipOperationError::OutcomeUnknown(message)
        }
        ToolError::ExecutionFailed(message) => {
            // Parse the transport status before the public detail. A message
            // mentioning a different HTTP status must not change classification.
            let status = message
                .split_once("HTTP status ")
                .and_then(|(_, tail)| tail.split(|c: char| c == ':' || c.is_whitespace()).next())
                .and_then(|status| status.parse::<u16>().ok());
            match status {
                Some(401 | 403) => protocol_failure(ProtocolErrorCode::PermissionDenied, message),
                Some(404) => protocol_failure(ProtocolErrorCode::NotFound, message),
                Some(400 | 409 | 422) => {
                    protocol_failure(ProtocolErrorCode::InvalidArguments, message)
                }
                _ => WipOperationError::OutcomeUnknown(message),
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
            if request.path.ends_with("/authorization/check") {
                return Ok(crate::worker::WorkspaceResponse {
                    status: 200,
                    body: "{}".into(),
                });
            }
            let mut response = self.response.clone();
            // Legacy adapters read a snapshot before the mutation. Let tests
            // inject mutation rejection independently of that successful read.
            if request.method == WorkspaceRequestMethod::Get {
                response.status = 200;
            } else if response.status == 200 && request.path.ends_with("/case") {
                let record: Value = serde_json::from_str(&response.body).unwrap();
                response.body = record["meta"].to_string();
            }
            self.requests.lock().unwrap().push(request);
            Ok(response)
        }
    }

    fn decision_client(status: u16, state: TicketState) -> Arc<DecisionClient> {
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
            "ticket": "T-739", "operation_key": "case-1",
            "expected_content_digest": "digest-1", "expected_state": "open",
            "expected_closure_operation_key": null,
            "action": {"action": "close", "reason": "not_planned"},
            "explanation": "No repository changes are needed"
        })
    }

    #[tokio::test]
    async fn ticket_api_errors_keep_public_details_in_tools_and_wip() {
        for (status, code) in [
            (403, ProtocolErrorCode::PermissionDenied),
            (404, ProtocolErrorCode::NotFound),
            (409, ProtocolErrorCode::InvalidArguments),
        ] {
            let client = Arc::new(DecisionClient {
                requests: Mutex::new(Vec::new()),
                response: crate::worker::WorkspaceResponse {
                    status,
                    body: json!({
                        "error": "Rejected",
                        "message": "Verified actor is required; unrelated HTTP status 401",
                        "diagnostics": [{"code":"ticket_actor_required", "severity":"error", "message":"provider-private-detail"}]
                    }).to_string(),
                },
            });
            let (_, tool) = workspace_ticket_decision_definition(
                client.clone(),
                WorkspaceTicketDecisionKind::CaseUpdate,
            )();
            let error = tool
                .execute(
                    &decision_input().to_string(),
                    ToolExecutionContext::direct(),
                )
                .await
                .unwrap_err();
            let text = error.to_string();
            assert!(text.contains(&format!("HTTP status {status}")), "{text}");
            assert!(text.contains("Verified actor is required"), "{text}");
            assert!(text.contains("ticket_actor_required"), "{text}");
            assert!(!text.contains("provider-private-detail"));
            assert!(
                matches!(map_ticket_tool_error(error), WipOperationError::Protocol(ProtocolError {code: actual, ..}) if actual == code)
            );

            // Unit-returning mutations and read tools share the same formatter.
            let error = WorkspaceHttpTicketBackend::request_unit(
                client.clone(),
                WorkspaceRequestMethod::Post,
                "/test".into(),
                None,
            )
            .unwrap_err();
            assert!(error.to_string().contains("Verified actor is required"));
            let (_, read) =
                workspace_ticket_read_definition(client.clone(), WorkspaceTicketReadKind::Show)();
            let error = read
                .execute(r#"{"id":"T-739"}"#, ToolExecutionContext::direct())
                .await
                .unwrap_err();
            assert!(error.to_string().contains("ticket_actor_required"));

            let tools =
                native_ticket_tools(client.clone(), TicketFeatureAccess::workspace_authoring())
                    .unwrap();
            let error = execute_native_ticket_tool(
                tools
                    .iter()
                    .find(|tool| tool.name == "TicketCaseUpdate")
                    .unwrap(),
                &None,
                decision_input(),
                WipCallContext {
                    execution: ToolExecutionContext::direct(),
                    security_context: String::new(),
                },
            )
            .await
            .unwrap_err();
            assert!(
                matches!(error, WipOperationError::Protocol(ProtocolError {code: actual, ref message}) if actual == code && message.contains("Verified actor is required"))
            );
        }
    }

    #[test]
    fn ticket_api_errors_bound_public_fields_and_ignore_unstructured_bodies() {
        let prefix = "ticket REST API request failed with HTTP status 502";
        for body in [
            "<html>proxy-private-detail</html>".into(),
            "not JSON".into(),
            json!({"message":"not the public envelope", "token":"private"}).to_string(),
            json!({"error":"Rejected", "message":"detail", "token":"private"}).to_string(),
            "x".repeat(64 * 1024 + 1),
        ] {
            assert_eq!(ticket_api_error_message(502, &body), prefix);
        }
        let body =
            json!({"error":"Rejected", "message":format!("line\n\u{1b}{}", "あ".repeat(2000)),
            "diagnostics":[{"code":"/private/path", "severity":"error", "message":"secret"}] })
            .to_string();
        let message = ticket_api_error_message(502, &body);
        assert!(message.ends_with('…'));
        assert!(message.len() < 4096);
        assert!(!message.chars().any(char::is_control));
        assert!(!message.contains("secret") && !message.contains("/private/path"));
        let message = ticket_api_error_message(
            502,
            &json!({"error":"Bad Gateway", "message":"upstream reported HTTP status 403"})
                .to_string(),
        );
        assert!(matches!(
            map_ticket_tool_error(ToolError::ExecutionFailed(message)),
            WipOperationError::OutcomeUnknown(_)
        ));
    }

    #[test]
    fn case_tool_availability_is_profile_controlled_in_both_interfaces() {
        for (access, allowed) in [
            (TicketFeatureAccess::read_only(), false),
            (TicketFeatureAccess::review(), false),
            (TicketFeatureAccess::workspace_authoring(), true),
            (TicketFeatureAccess::work_report(), true),
            (TicketFeatureAccess::workflow(), true),
        ] {
            assert_eq!(access.tool_names().contains(&"TicketCaseUpdate"), allowed);
            let tools =
                native_ticket_tools(decision_client(200, TicketState::Open), access).unwrap();
            assert_eq!(
                tools.iter().any(|tool| tool.name == "TicketCaseUpdate"),
                allowed
            );
        }
    }

    #[test]
    fn case_backend_sends_explicit_judgment_without_domain_author_claim() {
        let client = decision_client(200, TicketState::Closed);
        let backend = WorkspaceHttpTicketBackend::new(client.clone());
        let mut input = decision_input();
        input.as_object_mut().unwrap().remove("ticket");
        input["author"] = json!("self-reported-orchestrator");
        backend
            .update_case("T-739", serde_json::from_value(input.clone()).unwrap())
            .unwrap();
        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1].path, "/api/w/workspace/tickets/T-739/record");
        assert_eq!(requests[0].path, "/workspaces/workspace/tickets/T-739/case");
        let body: Value = serde_json::from_str(requests[0].body.as_deref().unwrap()).unwrap();
        input.as_object_mut().unwrap().remove("author");
        input["references"] = json!([]);
        assert_eq!(body, input);
    }

    #[test]
    fn case_backend_rejects_creation_closed_and_untyped_state_change_without_dispatch() {
        let client = decision_client(200, TicketState::Open);
        let backend = WorkspaceHttpTicketBackend::new(client.clone());
        let mut input = NewTicket::new("No bypass");
        input.workflow_state = Some(TicketState::Closed);
        assert!(
            backend
                .create(input)
                .unwrap_err()
                .to_string()
                .contains("created open")
        );
        let change = TicketStateChange::new("open", "closed", "Decision", "Explanation");
        assert!(
            backend
                .add_state_changed(TicketIdOrSlug::from("T-739"), change)
                .is_err()
        );
        assert!(client.requests.lock().unwrap().is_empty());
    }

    #[test]
    fn post_edit_backend_transports_atomic_snapshots_and_propagates_conflicts() {
        let record: Ticket =
            serde_json::from_str(&decision_client(200, TicketState::Open).response.body).unwrap();
        let input = json!({
            "event_ref": "post-reference",
            "expected_body": "Observed post",
            "body": "Updated post",
            "author": "standalone"
        });
        for status in [200, 409] {
            let client = Arc::new(DecisionClient {
                requests: Mutex::new(Vec::new()),
                response: crate::worker::WorkspaceResponse {
                    status,
                    body: serde_json::to_string(&(record.clone(), record.clone())).unwrap(),
                },
            });
            let backend = WorkspaceHttpTicketBackend::new(client.clone());
            let result = backend.edit_post_with_snapshots(
                TicketIdOrSlug::from("T-739"),
                serde_json::from_value(input.clone()).unwrap(),
            );
            if status == 200 {
                let (previous, current) = result.unwrap();
                assert_eq!(previous.meta.id, "T-718");
                assert_eq!(current.meta.id, "T-718");
            } else {
                assert!(result.unwrap_err().to_string().contains("409"));
            }
            let requests = client.requests.lock().unwrap();
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].method, WorkspaceRequestMethod::Patch);
            assert_eq!(requests[0].path, "/api/w/workspace/tickets/T-739/posts");
            assert_eq!(
                serde_json::from_str::<Value>(requests[0].body.as_deref().unwrap()).unwrap(),
                input,
            );
        }
    }

    fn target_detail_fixture(targets: Value) -> Value {
        json!({
            "resource_key": "T-739", "title": "Work location", "body": "Requirements",
            "body_truncated": false, "content_digest": "observed-content", "targets": targets,
            "initial_post": {
                "sequence": 0, "event_ref": "initial", "is_initial": true,
                "kind": "comment", "body": "Requirements", "body_truncated": false,
                "body_digest": "a".repeat(64), "editable": true,
                "author": null, "at": null, "edited_at": null, "edited_by": null
            },
            "state": "open", "closure_operation_key": null, "current_closure": null,
            "legacy_handoff": null, "closure_history": [], "assignment_tracking": [],
            "tags": [], "events": [], "linked_objectives": [], "assignments": [],
            "implementation_reports": [], "merge_requests": []
        })
    }

    #[tokio::test]
    async fn orchestrator_show_and_native_read_preserve_ordered_targets() {
        for targets in [
            json!([]),
            json!([
                {"repository_key": "main", "ref_selector": "develop", "access": "read_write"},
                {"repository_key": "docs", "ref_selector": null, "access": "read_only"},
                {"repository_key": "archive", "ref_selector": "refs/tags/v1", "access": "read_only"}
            ]),
        ] {
            let client = Arc::new(DecisionClient {
                requests: Mutex::new(Vec::new()),
                response: crate::worker::WorkspaceResponse {
                    status: 200,
                    body: target_detail_fixture(targets.clone()).to_string(),
                },
            });
            let definitions =
                enabled_ticket_definitions(client.clone(), TicketFeatureAccess::workflow());
            let definition = definitions
                .into_iter()
                .find(|definition| definition().0.name == "ShowTicket")
                .unwrap();
            let (meta, tool) = definition();
            assert!(meta.description.contains("targets"));
            let output = tool
                .execute(
                    r#"{"id":"T-739","event_limit":1}"#,
                    ToolExecutionContext::direct(),
                )
                .await
                .unwrap();
            assert_eq!(
                ticket_tool_output_json(output)
                    .unwrap_or_else(|_| panic!("Ticket read returned invalid JSON"))["targets"],
                targets
            );

            let native =
                native_ticket_tools(client.clone(), TicketFeatureAccess::workflow()).unwrap();
            let read = native
                .into_iter()
                .find(|tool| tool.projection.operation == "read")
                .unwrap();
            let output = read
                .tool
                .execute(
                    r#"{"id":"T-739","event_cursor":"10"}"#,
                    ToolExecutionContext::direct(),
                )
                .await
                .unwrap();
            assert_eq!(
                ticket_tool_output_json(output)
                    .unwrap_or_else(|_| panic!("Ticket read returned invalid JSON"))["targets"],
                targets
            );
            let requests = client.requests.lock().unwrap();
            assert_eq!(requests.len(), 2);
            assert!(
                requests
                    .iter()
                    .all(|request| request.path == "/api/w/workspace/tickets/T-739/show")
            );
        }
    }

    #[tokio::test]
    async fn show_rejects_missing_or_malformed_targets_instead_of_reporting_unassigned() {
        for malformed in [
            None,
            Some(Value::Null),
            Some(json!({})),
            Some(json!([{"repository_key":"main","access":"write"}])),
        ] {
            let mut detail = target_detail_fixture(json!([]));
            if let Some(value) = malformed {
                detail["targets"] = value;
            } else {
                detail.as_object_mut().unwrap().remove("targets");
            }
            let client = Arc::new(DecisionClient {
                requests: Mutex::new(Vec::new()),
                response: crate::worker::WorkspaceResponse {
                    status: 200,
                    body: detail.to_string(),
                },
            });
            let (_, tool) =
                workspace_ticket_read_definition(client, WorkspaceTicketReadKind::Show)();
            let error = tool
                .execute(r#"{"id":"T-739"}"#, ToolExecutionContext::direct())
                .await
                .unwrap_err();
            assert!(error.to_string().contains("targets"), "{error}");
        }
    }

    #[test]
    fn query_schema_allows_only_open_and_closed_case_states() {
        let client = decision_client(200, TicketState::Open);
        let (meta, _) = workspace_ticket_read_definition(client, WorkspaceTicketReadKind::Query)();
        let validator = jsonschema::validator_for(&meta.input_schema).unwrap();
        assert!(validator.is_valid(&json!({"states":["open", "closed"]})));
        for state in [
            "planning",
            "ready",
            "queued",
            "inprogress",
            "in_progress",
            "done",
        ] {
            assert!(!validator.is_valid(&json!({"states":[state]})));
            assert!(
                serde_json::from_value::<WorkspaceQueryTicketInput>(json!({"states":[state]}))
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn case_update_forwards_all_judgments_and_replays_without_actor_or_repo_requirements() {
        for (action, state, closure_key) in [
            (
                json!({"action":"close", "reason":"completed"}),
                "open",
                Value::Null,
            ),
            (
                json!({"action":"close", "reason":"not_planned"}),
                "open",
                Value::Null,
            ),
            (
                json!({"action":"close", "reason":"duplicate"}),
                "open",
                Value::Null,
            ),
            (
                json!({"action":"close", "reason":"unknown"}),
                "open",
                Value::Null,
            ),
            (json!({"action":"reopen"}), "closed", json!("closed-1")),
            (
                json!({"action":"correct_closure", "reason":{"other":"Changed judgment"}}),
                "closed",
                json!("closed-1"),
            ),
        ] {
            let client = decision_client(200, TicketState::Closed);
            let (meta, tool) = workspace_ticket_decision_definition(
                client.clone(),
                WorkspaceTicketDecisionKind::CaseUpdate,
            )();
            assert_eq!(meta.name, "TicketCaseUpdate");
            let mut input = decision_input();
            input["action"] = action;
            input["expected_state"] = json!(state);
            input["expected_closure_operation_key"] = closure_key;
            input["references"] = json!([{"kind":"result", "target":"notes"}]);
            assert!(
                jsonschema::validator_for(&meta.input_schema)
                    .unwrap()
                    .is_valid(&input)
            );
            for _ in 0..2 {
                let output = tool
                    .execute(&input.to_string(), ToolExecutionContext::direct())
                    .await
                    .unwrap();
                let projected: Value =
                    serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
                assert_eq!(projected["meta"]["workflow_state"], "closed");
            }
            let mut expected = input.as_object().unwrap().clone();
            expected.remove("ticket");
            let requests = client.requests.lock().unwrap();
            assert_eq!(requests.len(), 4);
            assert_eq!(
                requests
                    .iter()
                    .filter(|r| r.method == WorkspaceRequestMethod::Get)
                    .count(),
                2
            );
            for request in requests
                .iter()
                .filter(|r| r.method == WorkspaceRequestMethod::Post)
            {
                assert_eq!(request.path, "/workspaces/workspace/tickets/T-739/case");
                assert_eq!(request.method, WorkspaceRequestMethod::Post);
                let body: Value = serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
                assert_eq!(body, Value::Object(expected.clone()));
            }
        }
    }

    #[tokio::test]
    async fn case_update_requires_observed_nullable_closure_cas_and_rejects_malformed_or_actor_claims()
     {
        let client = decision_client(200, TicketState::Closed);
        let (meta, tool) = workspace_ticket_decision_definition(
            client.clone(),
            WorkspaceTicketDecisionKind::CaseUpdate,
        )();
        let validator = jsonschema::validator_for(&meta.input_schema).unwrap();
        let valid = decision_input();
        assert!(validator.is_valid(&valid));
        for field in [
            "ticket",
            "operation_key",
            "expected_content_digest",
            "expected_state",
            "expected_closure_operation_key",
            "action",
            "explanation",
        ] {
            let mut invalid = valid.clone();
            invalid.as_object_mut().unwrap().remove(field);
            assert!(
                !validator.is_valid(&invalid),
                "required schema field {field}"
            );
            assert!(
                tool.execute(&invalid.to_string(), ToolExecutionContext::direct())
                    .await
                    .is_err(),
                "required decode field {field}"
            );
        }
        for (field, value) in [
            ("expected_state", json!("done")),
            ("expected_state", json!("queued")),
            ("expected_closure_operation_key", json!(42)),
            ("action", json!({"action":"close"})),
            ("action", json!({"action":"close", "reason":"success"})),
            ("action", json!({"action":"reopen", "reason":"completed"})),
            ("actor", json!("orchestrator")),
            ("author", json!("coder")),
            ("role", json!("coder")),
            ("merge_request_ids", json!([])),
        ] {
            let mut invalid = valid.clone();
            invalid[field] = value;
            assert!(!validator.is_valid(&invalid), "schema accepts {invalid}");
            assert!(
                tool.execute(&invalid.to_string(), ToolExecutionContext::direct())
                    .await
                    .is_err(),
                "decoder accepts {invalid}"
            );
        }
        for field in [
            "ticket",
            "operation_key",
            "expected_content_digest",
            "explanation",
            "expected_closure_operation_key",
        ] {
            let mut invalid = valid.clone();
            invalid[field] = json!("  ");
            assert!(
                tool.execute(&invalid.to_string(), ToolExecutionContext::direct())
                    .await
                    .is_err()
            );
        }
        let mut invalid = valid;
        invalid["action"] = json!({"action":"close", "reason":{"other":" "}});
        assert!(
            tool.execute(&invalid.to_string(), ToolExecutionContext::direct())
                .await
                .is_err()
        );
        assert!(client.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn case_update_preserves_backend_cas_authority_rejections_and_wip_permission_gate() {
        for (status, code) in [
            (403, ProtocolErrorCode::PermissionDenied),
            (409, ProtocolErrorCode::InvalidArguments),
        ] {
            let client = decision_client(status, TicketState::Open);
            let tools =
                native_ticket_tools(client.clone(), TicketFeatureAccess::work_report()).unwrap();
            let tool = tools
                .iter()
                .find(|tool| tool.name == "TicketCaseUpdate")
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
        let client = decision_client(200, TicketState::Closed);
        let tools = native_ticket_tools(client.clone(), TicketFeatureAccess::workflow()).unwrap();
        let tool = tools
            .iter()
            .find(|tool| tool.name == "TicketCaseUpdate")
            .unwrap();
        let permissions = Some(ToolPermissionConfig {
            default_action: ToolPermissionAction::Deny,
            rules: Vec::new(),
        });
        let error = execute_native_ticket_tool(
            tool,
            &permissions,
            decision_input(),
            WipCallContext {
                execution: ToolExecutionContext::direct(),
                security_context: "author".into(),
            },
        )
        .await
        .expect_err("WIP denial prevents dispatch");
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
    async fn native_case_update_binds_subject_requires_closure_observation_and_stales_only_its_ticket()
     {
        let client = decision_client(200, TicketState::Closed);
        let tools =
            native_ticket_tools(client.clone(), TicketFeatureAccess::work_report()).unwrap();
        let descriptor = ticket_descriptor(&tools, "Ticket", "bound").unwrap();
        let case_update = descriptor
            .operations
            .iter()
            .find(|op| op.name == "case_update")
            .unwrap();
        assert!(
            !case_update
                .parameters
                .iter()
                .any(|p| p.name == "ticket" || p.name == "actor" || p.name == "author")
        );
        for field in [
            "operation_key",
            "expected_content_digest",
            "expected_state",
            "expected_closure_operation_key",
            "action",
            "explanation",
        ] {
            assert!(
                case_update
                    .parameters
                    .iter()
                    .any(|p| p.name == field && p.required)
            );
        }
        let summary = &case_update.documentation.as_ref().unwrap().summary;
        assert!(summary.contains("Closed does not imply success"));
        assert!(summary.contains("not real judgment"));
        assert!(summary.contains("Return without conclusion need not call this tool"));
        let resolver = TicketItemResolver {
            tools: operation_map(tools),
            permissions: None,
            collection_route: "/tickets".into(),
            observations: Arc::new(Mutex::new(TicketObservationState::default())),
        };
        record_ticket_observation(
            &resolver.observations,
            Some("T-739"),
            &json!({"ticket":"T-739", "body":"observed requirements"}),
            false,
        );
        let bound = resolver.resolve("T-739").unwrap();
        let before = bound.object.validator;
        let other = resolver.resolve("T-740").unwrap().object.validator;
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
                "case_update",
                &arguments,
                WipCallContext {
                    execution: ToolExecutionContext::direct(),
                    security_context: "coder".into(),
                },
            )
            .await
            .unwrap_or_else(|_| panic!("WIP case update rejected"));
        assert_ne!(before, resolver.resolve("T-739").unwrap().object.validator);
        assert_eq!(other, resolver.resolve("T-740").unwrap().object.validator);
        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].path, "/workspaces/workspace/tickets/T-739/case");
        assert_eq!(requests[1].path, "/api/w/workspace/tickets/T-739/record");
        let body: Value = serde_json::from_str(requests[0].body.as_deref().unwrap()).unwrap();
        let mut expected = input;
        expected["references"] = json!([]);
        assert_eq!(body, expected);
    }

    #[tokio::test]
    async fn case_update_never_falls_back_without_workspace_authority() {
        let client = crate::worker::unavailable_workspace_client(None, "no Workspace configured");
        let (_, tool) =
            workspace_ticket_decision_definition(client, WorkspaceTicketDecisionKind::CaseUpdate)();
        let error = tool
            .execute(
                &decision_input().to_string(),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Workspace identity"));
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
            WORKSPACE_AUTHORING_TOOL_NAMES
                .iter()
                .map(|name| native_ticket_operation(name).unwrap().operation)
                .collect::<Vec<_>>()
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
            WORKFLOW_TOOL_NAMES
                .iter()
                .map(|name| native_ticket_operation(name).unwrap().operation)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            native_ticket_tools(client, TicketFeatureAccess::review())
                .unwrap()
                .iter()
                .map(|tool| tool.projection.operation)
                .collect::<Vec<_>>(),
            READ_ONLY_TOOL_NAMES
                .iter()
                .map(|name| native_ticket_operation(name).unwrap().operation)
                .collect::<Vec<_>>()
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
        let response = json!({"ticket": "T-42", "body": "actual content", "state": "open"});
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
    fn related_ticket_mutation_invalidates_inverse_target_but_not_unrelated_tickets() {
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
        let unrelated_v1 = resolver.resolve("T-44").unwrap().object.validator;
        record_affected_ticket_mutations(
            &observations,
            "T-42",
            &json!({
                "ticket": "T-42",
                "target": "T-43"
            }),
        );
        assert_ne!(
            target_v1,
            resolver.resolve("T-43").unwrap().object.validator
        );
        assert_eq!(
            unrelated_v1,
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
        // Assignment tools use a separate adapter but share this WIP classifier.
        assert!(matches!(
            map_ticket_tool_error(ToolError::ExecutionFailed("Assignment request failed with HTTP status 403: denied; HTTP status 409 is unrelated".into())),
            WipOperationError::Protocol(ProtocolError { code: ProtocolErrorCode::PermissionDenied, .. })
        ));
        assert!(matches!(
            map_ticket_tool_error(ToolError::ExecutionFailed("Assignment tracking request failed with HTTP status 409: stale; HTTP status 403 is unrelated".into())),
            WipOperationError::Protocol(ProtocolError { code: ProtocolErrorCode::InvalidArguments, .. })
        ));
        assert!(matches!(
            map_ticket_tool_error(ToolError::ExecutionFailed("transport disconnected".into())),
            WipOperationError::OutcomeUnknown(_)
        ));
        assert!(matches!(
            map_ticket_tool_error(ToolError::Internal(
                "TicketCaseUpdate outcome unknown after mutation: HTTP status 404".into()
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
        let client: Arc<dyn WorkspaceClient> = Arc::new(CreateAuthorizationClient {
            workspace: Some("workspace"),
            allowed: true,
            backend_status: 200,
            requests: Default::default(),
        });
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
        assert!(!query_schema.contains("done_not_closed"));
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
        assert_eq!(tool_names, WORKSPACE_AUTHORING_TOOL_NAMES);
        let workflow_names = TicketFeatureAccess::workflow().tool_names();
        assert_eq!(workflow_names, WORKFLOW_TOOL_NAMES);
        assert_eq!(
            TicketFeatureAccess::review().tool_names(),
            READ_ONLY_TOOL_NAMES
        );
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
        assert!(workspace_tools.contains(&"TicketEditPost"));
        assert!(!workspace_tools.contains(&"TicketQueue"));
        assert!(workspace_tools.contains(&"TicketCaseUpdate"));

        let orchestration = workspace_feature(TicketFeatureAccess::workflow());
        let orchestration_descriptor = orchestration.descriptor();
        let orchestration_tools = orchestration_descriptor
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>();
        assert!(orchestration_tools.contains(&"TicketCaseUpdate"));
        assert!(orchestration_tools.contains(&"TicketDependencyCheck"));
        assert!(orchestration_tools.contains(&"TicketRelationRecord"));
        assert!(orchestration_tools.contains(&"TicketOrchestrationPlanRecord"));
        assert!(!orchestration_tools.contains(&"TicketEditItem"));
        assert!(!orchestration_tools.contains(&"TicketEditPost"));
        assert!(!orchestration_tools.contains(&"TicketQueue"));

        let work_report = workspace_feature(TicketFeatureAccess::work_report());
        let work_report_descriptor = work_report.descriptor();
        let work_report_tools = work_report_descriptor
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>();
        assert!(work_report_tools.contains(&"TicketComment"));
        assert!(work_report_tools.contains(&"TicketCaseUpdate"));

        let review = workspace_feature(TicketFeatureAccess::review());
        let review_descriptor = review.descriptor();
        let review_tools = review_descriptor
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>();
        assert!(!review_tools.contains(&"TicketCaseUpdate"));
    }

    #[test]
    fn normal_and_wip_catalogs_exclude_retired_ticket_workflow_operations() {
        let client: Arc<dyn WorkspaceClient> = Arc::new(
            crate::worker::TestWorkspaceHttpClient::new("workspace", "http://unused"),
        );
        let retired = [
            "TicketMarkReady",
            "TicketIntakeReady",
            "TicketQueue",
            "TicketWorkflowState",
            "CompleteTicket",
            "TicketClose",
        ];
        for access in [
            TicketFeatureAccess::workspace_authoring(),
            TicketFeatureAccess::intake(),
            TicketFeatureAccess::workflow(),
            TicketFeatureAccess::work_report(),
            TicketFeatureAccess::read_only(),
            TicketFeatureAccess {
                authoring: true,
                thread: true,
                intake: true,
                workflow: true,
            },
        ] {
            let normal = enabled_ticket_definitions(client.clone(), access);
            let native = native_ticket_tools(client.clone(), access).unwrap();
            for name in retired {
                assert!(
                    normal.iter().all(|definition| definition().0.name != name),
                    "{name}"
                );
                assert!(native.iter().all(|tool| tool.name != name), "{name}");
                assert!(native_ticket_operation(name).is_none(), "{name}");
            }
            assert_eq!(
                normal
                    .iter()
                    .any(|definition| definition().0.name == "TicketEditPost"),
                access.authoring,
            );
            assert_eq!(
                native
                    .iter()
                    .any(|tool| tool.projection.operation == "edit_post"),
                access.authoring,
            );
            if access.authoring || access.thread || access.workflow {
                assert!(
                    normal
                        .iter()
                        .any(|definition| definition().0.name == "TicketCaseUpdate")
                );
                assert!(
                    native
                        .iter()
                        .any(|tool| tool.projection.operation == "case_update")
                );
            }
        }
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
        assert!(!installed.iter().any(|tool| *tool == "TicketQueue"));
        assert!(!installed.iter().any(|tool| *tool == "TicketMarkReady"));
        assert!(!installed.iter().any(|tool| *tool == "TicketIntakeReady"));
        assert!(installed.iter().any(|tool| *tool == "TicketCaseUpdate"));
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
                from: "open".to_string(),
            })
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("Ticket operation authorization failed")
        );
    }

    fn permit_ticket_authorization(listener: &TcpListener, operation: &str) {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut buffer = [0_u8; 8192];
        let len = stream.read(&mut buffer).unwrap();
        let request = String::from_utf8_lossy(&buffer[..len]);
        assert!(
            request.starts_with("POST /api/w/workspace-a/authorization/check HTTP/1.1"),
            "{request}"
        );
        assert!(
            request.contains(&format!("\"operation\":\"{operation}\"")),
            "{request}"
        );
        assert!(request.contains("\"workspace_id\":\"workspace-a\""));
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}").unwrap();
    }

    #[test]
    fn workspace_http_backend_posts_ticket_event_subresource() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            permit_ticket_authorization(&listener, "ticket.event.add");
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
            permit_ticket_authorization(&listener, "ticket.relation.record");
            permit_ticket_authorization(&listener, "ticket.relation.record");
            for (expected_path, resource_key) in [
                ("GET /api/w/workspace-a/tickets/01SOURCE HTTP/1.1", "T-1"),
                ("GET /api/w/workspace-a/tickets/01TARGET HTTP/1.1", "T-2"),
            ] {
                permit_ticket_authorization(&listener, "ticket.read");
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

            permit_ticket_authorization(&listener, "ticket.relation.record");
            permit_ticket_authorization(&listener, "ticket.relation.record");
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
            permit_ticket_authorization(&listener, "ticket.relation.remove");
            permit_ticket_authorization(&listener, "ticket.relation.remove");
            for (expected_path, resource_key) in [
                ("GET /api/w/workspace-a/tickets/01SOURCE HTTP/1.1", "T-1"),
                ("GET /api/w/workspace-a/tickets/01TARGET HTTP/1.1", "T-2"),
            ] {
                permit_ticket_authorization(&listener, "ticket.read");
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

            permit_ticket_authorization(&listener, "ticket.relation.remove");
            permit_ticket_authorization(&listener, "ticket.relation.remove");
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
        ticket.meta.workflow_state = TicketState::Open;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let response_body = serde_json::to_string(&ticket).unwrap();
        let server = thread::spawn(move || {
            permit_ticket_authorization(&listener, "ticket.read");
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
            access: TicketFeatureAccess::read_only(),
            backend: WorkspaceHttpTicketBackend::new(Arc::new(
                crate::worker::TestWorkspaceHttpClient::new("workspace-a", base_url),
            )),
        };
        let handoff = service.ticket_handoff("T-548").unwrap();

        server.join().unwrap();
        assert_eq!(handoff.id, created.id);
        assert_eq!(handoff.resource_key, "T-548");
        assert_eq!(handoff.workflow_state, TicketState::Open);
    }

    #[test]
    fn ticket_handoff_accepts_only_canonical_ticket_resource_keys() {
        assert!(is_canonical_ticket_resource_key("T-482"));
        for invalid in ["", "00001KZVNXFNK", "T-", "T-key", "O-482"] {
            assert!(!is_canonical_ticket_resource_key(invalid));
        }
    }

    // This suite keeps the real Feature/Tool/WIP/backend-dispatch code; only the
    // Host transport is replaced. Host grant/storage behavior is tested at Server.
    #[derive(Debug)]
    struct CreateAuthorizationClient {
        workspace: Option<&'static str>,
        allowed: bool,
        backend_status: u16,
        requests: Mutex<Vec<WorkspaceRequest>>,
    }

    impl WorkspaceClient for CreateAuthorizationClient {
        fn workspace_id(&self) -> Option<&str> {
            self.workspace
        }
        fn kind(&self) -> &str {
            "create-authorization-fixture"
        }
        fn is_available(&self) -> bool {
            true
        }
        fn execute(
            &self,
            request: WorkspaceRequest,
        ) -> Result<crate::worker::WorkspaceResponse, crate::worker::WorkspaceClientError> {
            let authorization = request.path.ends_with("/authorization/check");
            let permitted = self.allowed;
            if authorization {
                assert_eq!(request.method, WorkspaceRequestMethod::Post);
                let body: Value = serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
                assert_eq!(
                    body,
                    json!({"operation":"ticket.create", "resource":{"kind":"workspace","workspace_id":"workspace"}})
                );
            } else {
                assert_eq!(request.path, "/api/w/workspace/tickets");
            }
            self.requests.lock().unwrap().push(request);
            Ok(crate::worker::WorkspaceResponse {
                status: if authorization {
                    if permitted { 200 } else { 403 }
                } else {
                    self.backend_status
                },
                body: if authorization {
                    if permitted {
                        "{}".into()
                    } else {
                        json!({"message":"Ticket permission revoked"}).to_string()
                    }
                } else if self.backend_status != 200 {
                    json!({"error":"Forbidden", "message":"backend create denied", "diagnostics":[]}).to_string()
                } else {
                    serde_json::to_string(&TicketRef {
                        id: "created-1".into(),
                        resource_key: Some("T-1".into()),
                        slug: "created".into(),
                        status: ticket::TicketStatus::Open,
                    })
                    .unwrap()
                },
            })
        }
    }

    #[tokio::test]
    async fn create_authorization_guards_tool_wip_installed_service_and_backend_without_local_root()
    {
        for (allowed, backend_status) in [(false, 200), (true, 200), (true, 403)] {
            for surface in ["tool", "wip", "service", "backend"] {
                let client = Arc::new(CreateAuthorizationClient {
                    workspace: Some("workspace"),
                    allowed,
                    backend_status,
                    requests: Default::default(),
                });
                let result = match surface {
                    "backend" => WorkspaceHttpTicketBackend::new(client.clone())
                        .create(NewTicket::new("Create"))
                        .map(|_| ())
                        .map_err(|error| error.to_string()),
                    "service" => {
                        let report = FeatureRegistryBuilder::new()
                            .with_module(ticket_tools_feature(
                                client.clone(),
                                TicketFeatureAccess::workspace_authoring(),
                            ))
                            .install_into_pending(
                                &mut Vec::new(),
                                &mut HookRegistryBuilder::default(),
                            );
                        assert!(!report.has_errors(), "{}", report.error_message());
                        let service = report
                            .services
                            .service::<dyn TicketService>(&ServiceId::builtin(TICKET_SERVICE_ID))
                            .unwrap();
                        service
                            .create(NewTicket::new("Create"))
                            .map(|_| ())
                            .map_err(|error| error.to_string())
                    }
                    "tool" => {
                        let tools = enabled_ticket_definitions(
                            client.clone(),
                            TicketFeatureAccess::workspace_authoring(),
                        );
                        let (_, tool) = tools
                            .into_iter()
                            .find(|definition| definition().0.name == "TicketCreate")
                            .unwrap()();
                        tool.execute(r#"{"title":"Create"}"#, ToolExecutionContext::direct())
                            .await
                            .map(|_| ())
                            .map_err(|error| error.to_string())
                    }
                    _ => {
                        let tools = native_ticket_tools(
                            client.clone(),
                            TicketFeatureAccess::workspace_authoring(),
                        )
                        .unwrap();
                        let handler = TicketCollectionWipHandler {
                            tools: operation_map(tools),
                            permissions: None,
                            collection_route: "/tickets".into(),
                            observations: Arc::new(Mutex::new(TicketObservationState::default())),
                        };
                        handler
                            .call(
                                "create",
                                &BTreeMap::from([(
                                    "title".into(),
                                    WipValue::String("Create".into()),
                                )]),
                                WipCallContext {
                                    execution: ToolExecutionContext::direct(),
                                    security_context: "test".into(),
                                },
                            )
                            .await
                            .map(|_| ())
                            .map_err(|error| match error {
                                WipOperationError::Protocol(ProtocolError {
                                    code: ProtocolErrorCode::PermissionDenied,
                                    message,
                                    ..
                                }) => message,
                                _ => panic!("a definitive denial is not outcome-unknown"),
                            })
                    }
                };
                assert_eq!(
                    result.is_ok(),
                    allowed && backend_status == 200,
                    "{surface}: {result:?}"
                );
                if allowed && backend_status == 403 {
                    let error = result.unwrap_err();
                    assert!(error.contains("HTTP status 403"), "{surface}: {error}");
                    assert!(
                        error.contains("backend create denied"),
                        "{surface}: {error}"
                    );
                }
                let requests = client.requests.lock().unwrap();
                assert_eq!(requests.len(), 1 + usize::from(allowed), "{surface}");
                assert!(requests[0].path.ends_with("/authorization/check"));
                assert_eq!(
                    requests
                        .iter()
                        .filter(|request| request.path.ends_with("/tickets"))
                        .count(),
                    usize::from(allowed)
                );
            }
        }
    }

    #[test]
    fn direct_ticket_install_registers_tools_and_services_without_activation_or_data_access() {
        assert!(!manifest::FeatureConfig::default().ticket.enabled);
        for allowed in [false, true] {
            let client = Arc::new(CreateAuthorizationClient {
                workspace: Some("workspace"),
                allowed,
                backend_status: 200,
                requests: Default::default(),
            });
            let mut tools = Vec::new();
            let report = FeatureRegistryBuilder::new()
                .with_module(ticket_tools_feature(
                    client.clone(),
                    TicketFeatureAccess::workspace_authoring(),
                ))
                .install_into_pending(&mut tools, &mut HookRegistryBuilder::default());
            assert!(!report.has_errors(), "{}", report.error_message());
            assert!(tools.iter().any(|tool| tool().0.name == "TicketCreate"));
            assert!(!report.installed_instruction_contributions().is_empty());
            assert!(!report.services.providers().is_empty());
            assert!(client.requests.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn restored_ticket_keeps_tools_and_services_but_rejects_revoked_operation() {
        let client = Arc::new(CreateAuthorizationClient {
            workspace: Some("workspace"),
            allowed: false,
            backend_status: 200,
            requests: Default::default(),
        });
        let mut tools = Vec::new();
        let report = FeatureRegistryBuilder::new()
            .with_module(ticket_tools_feature(
                client.clone(),
                TicketFeatureAccess::workspace_authoring(),
            ))
            .with_installation(crate::feature::FeatureInstallation::Restore)
            .install_into_pending(&mut tools, &mut HookRegistryBuilder::default());
        assert!(!report.has_errors(), "{}", report.error_message());
        assert!(tools.iter().any(|tool| tool().0.name == "TicketCreate"));
        assert!(!report.installed_instruction_contributions().is_empty());
        assert!(client.requests.lock().unwrap().is_empty());
        let service = report
            .services
            .service::<dyn TicketService>(&ServiceId::builtin(TICKET_SERVICE_ID))
            .unwrap();
        let error = service
            .create(NewTicket::new("Revoked operation"))
            .unwrap_err();
        assert!(error.to_string().contains("HTTP status 403"), "{error}");
        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 1, "denial must precede the backend effect");
        assert!(requests[0].path.ends_with("/authorization/check"));
        let body: Value = serde_json::from_str(requests[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(body["operation"], "ticket.create");
    }

    #[test]
    fn installed_ticket_service_readonly_profile_cannot_create_with_host_permission() {
        let client = Arc::new(CreateAuthorizationClient {
            workspace: Some("workspace"),
            allowed: true,
            backend_status: 200,
            requests: Default::default(),
        });
        let mut tools = Vec::new();
        let report = FeatureRegistryBuilder::new()
            .with_module(ticket_tools_feature(
                client.clone(),
                TicketFeatureAccess::read_only(),
            ))
            .install_into_pending(&mut tools, &mut HookRegistryBuilder::default());
        assert!(!report.has_errors(), "{}", report.error_message());
        assert!(!tools.iter().any(|tool| tool().0.name == "TicketCreate"));
        let service = report
            .services
            .service::<dyn TicketService>(&ServiceId::builtin(TICKET_SERVICE_ID))
            .unwrap();
        assert!(
            service
                .create(NewTicket::new("No profile bypass"))
                .unwrap_err()
                .to_string()
                .contains("Profile authoring policy")
        );
        assert!(
            client.requests.lock().unwrap().is_empty(),
            "no activation, authorization or create effect"
        );
    }

    #[test]
    fn legacy_handoff_only_service_does_not_gain_creation() {
        struct LegacyService;
        impl TicketService for LegacyService {
            fn ticket_handoff(&self, _: &str) -> Result<TicketHandoff, TicketError> {
                unreachable!("creation must not call handoff")
            }
        }
        assert!(
            LegacyService
                .create(NewTicket::new("No implicit creation"))
                .unwrap_err()
                .to_string()
                .contains("unavailable")
        );
    }

    #[test]
    fn create_without_workspace_identity_fails_before_transport_and_backend_denial_is_preserved() {
        for (workspace, allowed, backend_status, expected_calls) in
            [(None, true, 200, 0), (Some("workspace"), true, 403, 2)]
        {
            let client = Arc::new(CreateAuthorizationClient {
                workspace,
                allowed,
                backend_status,
                requests: Default::default(),
            });
            let result =
                WorkspaceHttpTicketBackend::new(client.clone()).create(NewTicket::new("Create"));
            assert!(result.is_err());
            assert_eq!(client.requests.lock().unwrap().len(), expected_calls);
        }
    }

    #[test]
    fn workspace_http_backend_executes_ticket_create_operation() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut authorization, _) = listener.accept().unwrap();
            authorization
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut buffer = [0_u8; 8192];
            let len = authorization.read(&mut buffer).unwrap();
            let request = String::from_utf8_lossy(&buffer[..len]);
            assert!(request.starts_with("POST /api/w/workspace-a/authorization/check HTTP/1.1"));
            assert!(request.contains("\"operation\":\"ticket.create\""));
            assert!(request.contains("\"workspace_id\":\"workspace-a\""));
            write!(authorization, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}").unwrap();
            drop(authorization);
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
    /// Connection suite: real Feature/Task/Tool/WIP/service/backend adapters and
    /// WorkspaceClient authorization transport; only the remote Host is replaced.
    /// The fixture evaluates exact shared permission containment and persists allowed
    /// creates in an isolated SQLite backend. Signed source/grant validation belongs
    /// to the Server suite, not to this in-process Host fixture.
    #[derive(Debug)]
    struct ScopedTicketHost {
        permissions: Mutex<Vec<server_api::authorization::ScopedPermission>>,
        requests: Mutex<Vec<WorkspaceRequest>>,
        store: ticket::SqliteTicketBackend,
        _root: tempfile::TempDir,
    }

    impl ScopedTicketHost {
        fn new(permissions: Vec<server_api::authorization::ScopedPermission>) -> Arc<Self> {
            let root = tempfile::tempdir().unwrap();
            Arc::new(Self {
                store: ticket::SqliteTicketBackend::open(
                    root.path().join("tickets.db"),
                    "workspace",
                )
                .unwrap(),
                _root: root,
                permissions: Mutex::new(permissions),
                requests: Mutex::new(Vec::new()),
            })
        }
        fn downstream_count(&self) -> usize {
            self.requests
                .lock()
                .unwrap()
                .iter()
                .filter(|request| !request.path.ends_with("/authorization/check"))
                .count()
        }
    }

    fn ticket_permission(
        operation: &str,
        workspace_id: &str,
    ) -> server_api::authorization::ScopedPermission {
        server_api::authorization::ScopedPermission {
            operation: operation.into(),
            resource: server_api::authorization::PermissionResource::Workspace {
                workspace_id: workspace_id.into(),
            },
        }
    }

    impl WorkspaceClient for ScopedTicketHost {
        fn workspace_id(&self) -> Option<&str> {
            Some("workspace")
        }
        fn kind(&self) -> &str {
            "scoped-ticket-host-fixture"
        }
        fn is_available(&self) -> bool {
            true
        }
        fn execute(
            &self,
            request: WorkspaceRequest,
        ) -> Result<crate::worker::WorkspaceResponse, crate::worker::WorkspaceClientError> {
            self.requests.lock().unwrap().push(request.clone());
            if request.path.ends_with("/authorization/check") {
                let needed: server_api::authorization::OperationAuthorizationRequest =
                    serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
                needed.validate().unwrap();
                let needed = server_api::authorization::ScopedPermission {
                    operation: needed.operation,
                    resource: needed.resource,
                };
                let scope = self
                    .permissions
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|permission| permission.covers(&needed))
                    .map(|permission| permission.resource.clone());
                return Ok(crate::worker::WorkspaceResponse {
                    status: if scope.is_some() { 200 } else { 403 },
                    body: scope
                        .map(|scope| json!({"scope":scope}))
                        .unwrap_or_else(
                            || json!({"message":"operation or resource is not granted"}),
                        )
                        .to_string(),
                });
            }
            assert_eq!(
                request.path, "/api/w/workspace/tickets",
                "unexpected protected downstream request"
            );
            assert_eq!(request.method, WorkspaceRequestMethod::Post);
            let created = self
                .store
                .create(serde_json::from_str(request.body.as_deref().unwrap()).unwrap())
                .unwrap();
            Ok(crate::worker::WorkspaceResponse {
                status: 200,
                body: serde_json::to_string(&created).unwrap(),
            })
        }
    }

    #[tokio::test]
    async fn task_and_ticket_install_with_create_only_then_deny_revoked_and_out_of_scope_without_downstream()
     {
        for surface in ["tool", "wip", "service", "backend"] {
            let client =
                ScopedTicketHost::new(vec![ticket_permission("ticket.create", "workspace")]);
            let mut tools = Vec::new();
            let report = FeatureRegistryBuilder::new()
                .with_module(super::super::task::task_tools_feature())
                .with_module(ticket_tools_feature(
                    client.clone(),
                    TicketFeatureAccess::workspace_authoring(),
                ))
                .install_into_pending(&mut tools, &mut HookRegistryBuilder::default());
            assert!(!report.has_errors(), "{}", report.error_message());
            assert!(
                client.requests.lock().unwrap().is_empty(),
                "registration has no protected access"
            );
            let (_, task) = tools
                .iter()
                .find(|definition| definition().0.name == "TaskCreate")
                .unwrap()();
            task.execute(
                r#"{"subject":"local", "description":"session only"}"#,
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
            assert!(
                client.requests.lock().unwrap().is_empty(),
                "local task needs no Host grant"
            );
            let service = report
                .services
                .service::<dyn TicketService>(&ServiceId::builtin(TICKET_SERVICE_ID))
                .unwrap();
            // The same bound adapters survive policy replacement; they must not cache success.
            for phase in ["allowed", "revoked", "out_of_scope", "legacy_feature_only"] {
                if phase != "allowed" {
                    *client.permissions.lock().unwrap() = match phase {
                        "out_of_scope" => {
                            vec![ticket_permission("ticket.create", "another-workspace")]
                        }
                        "legacy_feature_only" => {
                            vec![server_api::authorization::ScopedPermission {
                                operation: "ticket".into(),
                                resource: server_api::authorization::PermissionResource::Feature,
                            }]
                        }
                        _ => Vec::new(),
                    };
                }
                let before = client.downstream_count();
                let result = match surface {
                    "tool" => {
                        let (_, tool) = tools
                            .iter()
                            .find(|definition| definition().0.name == "TicketCreate")
                            .unwrap()();
                        tool.execute(
                            r#"{"title":"Authorized create"}"#,
                            ToolExecutionContext::direct(),
                        )
                        .await
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                    }
                    "wip" => {
                        let handler = TicketCollectionWipHandler {
                            tools: operation_map(
                                native_ticket_tools(
                                    client.clone(),
                                    TicketFeatureAccess::workspace_authoring(),
                                )
                                .unwrap(),
                            ),
                            permissions: None,
                            collection_route: "/tickets".into(),
                            observations: Arc::new(Mutex::new(TicketObservationState::default())),
                        };
                        handler
                            .call(
                                "create",
                                &BTreeMap::from([(
                                    "title".into(),
                                    WipValue::String("Authorized create".into()),
                                )]),
                                WipCallContext {
                                    execution: ToolExecutionContext::direct(),
                                    security_context: "test".into(),
                                },
                            )
                            .await
                            .map(|_| ())
                            .map_err(|error| match error {
                                WipOperationError::Protocol(ProtocolError {
                                    code: ProtocolErrorCode::PermissionDenied,
                                    message,
                                    ..
                                }) => message,
                                _ => {
                                    panic!("definitive authorization denial is not outcome-unknown")
                                }
                            })
                    }
                    "service" => service
                        .create(NewTicket::new("Authorized create"))
                        .map(|_| ())
                        .map_err(|e| e.to_string()),
                    _ => WorkspaceHttpTicketBackend::new(client.clone())
                        .create(NewTicket::new("Authorized create"))
                        .map(|_| ())
                        .map_err(|e| e.to_string()),
                };
                assert_eq!(
                    result.is_ok(),
                    phase == "allowed",
                    "{surface}/{phase}: {result:?}"
                );
                assert_eq!(
                    client.downstream_count() - before,
                    usize::from(phase == "allowed"),
                    "{surface}/{phase}"
                );
                assert_eq!(
                    client.store.list(TicketListQuery::all()).unwrap().len(),
                    1,
                    "only the allowed call persisted a Ticket"
                );
            }
        }
    }

    #[test]
    fn all_backend_operations_deny_before_any_protected_request() {
        let id = TicketIdOrSlug::from("T-739");
        let mut case = decision_input();
        case.as_object_mut().unwrap().remove("ticket");
        let mut operations = vec![
            (
                "ticket.tag.list",
                TicketBackendOperation::ListWorkspaceTags {
                    request: ticket::TagListRequest {
                        after: None,
                        limit: None,
                    },
                },
            ),
            (
                "ticket.tags.set",
                TicketBackendOperation::SetTicketTags {
                    id: id.clone(),
                    request: ticket::TicketTagSetRequest {
                        operation_key: "set".into(),
                        expected_tag_ids: vec![],
                        tag_ids: vec![],
                    },
                },
            ),
            (
                "ticket.intake.default_body",
                TicketBackendOperation::DefaultIntakeReadyStateChangeBody {
                    from: "open".into(),
                },
            ),
            (
                "ticket.list",
                TicketBackendOperation::List {
                    filter: TicketListQuery::all(),
                },
            ),
            (
                "ticket.read",
                TicketBackendOperation::Show { id: id.clone() },
            ),
            (
                "ticket.create",
                TicketBackendOperation::Create {
                    input: NewTicket::new("Denied"),
                },
            ),
            (
                "ticket.edit_item",
                TicketBackendOperation::EditItem {
                    id: id.clone(),
                    edit: ticket::TicketItemEdit {
                        title: Some("Denied".into()),
                        ..Default::default()
                    },
                },
            ),
            (
                "ticket.edit_post",
                TicketBackendOperation::EditPost {
                    id: id.clone(),
                    edit: ticket::TicketPostEdit {
                        event_ref: "post".into(),
                        expected_body: "old".into(),
                        body: "new".into(),
                        author: "not-authority".into(),
                    },
                },
            ),
            (
                "ticket.dependency.check",
                TicketBackendOperation::DependencyCheck { id: id.clone() },
            ),
            (
                "ticket.event.add",
                TicketBackendOperation::AddEvent {
                    id: id.clone(),
                    event: NewTicketEvent::new(ticket::TicketEventKind::Comment, "Denied"),
                },
            ),
            (
                "ticket.intake_summary.add",
                TicketBackendOperation::AddIntakeSummary {
                    id: id.clone(),
                    summary: TicketIntakeSummary::new("Denied"),
                },
            ),
            (
                "ticket.case.update",
                TicketBackendOperation::UpdateCase {
                    ticket: "T-739".into(),
                    request: serde_json::from_value(case).unwrap(),
                },
            ),
            (
                "ticket.mark_ready",
                TicketBackendOperation::MarkReady {
                    id: id.clone(),
                    request: ticket::TicketMarkReady {
                        operation_key: "ready".into(),
                        reason: None,
                        author: None,
                        intake_summary: None,
                    },
                },
            ),
            (
                "ticket.queue",
                TicketBackendOperation::QueueReady {
                    id: id.clone(),
                    queued_by: "not-authority".into(),
                },
            ),
            (
                "ticket.relation.record",
                TicketBackendOperation::AddTicketRelation {
                    id: id.clone(),
                    relation: NewTicketRelation {
                        kind: TicketRelationKind::Related,
                        target: "T-740".into(),
                        note: None,
                        author: None,
                    },
                },
            ),
            (
                "ticket.relation.remove",
                TicketBackendOperation::RemoveTicketRelation {
                    id: id.clone(),
                    kind: TicketRelationKind::Related,
                    target: TicketIdOrSlug::from("T-740"),
                },
            ),
            (
                "ticket.relation.query",
                TicketBackendOperation::QueryTicketRelations {
                    ticket: None,
                    kind: None,
                },
            ),
            (
                "ticket.relation.read",
                TicketBackendOperation::RelationView { id: id.clone() },
            ),
            (
                "ticket.orchestration_plan.record",
                TicketBackendOperation::AddOrchestrationPlanRecord {
                    id,
                    record: NewOrchestrationPlanRecord {
                        kind: OrchestrationPlanKind::Before,
                        related_ticket: Some("T-740".into()),
                        note: None,
                        accepted_plan: None,
                        author: None,
                    },
                },
            ),
            (
                "ticket.orchestration_plan.query",
                TicketBackendOperation::QueryOrchestrationPlanRecords {
                    ticket: None,
                    kind: None,
                },
            ),
            ("ticket.doctor", TicketBackendOperation::Doctor),
        ];
        for (name, action) in [
            (
                "ticket.tag.create",
                ticket::WorkspaceTagAction::Create { name: "tag".into() },
            ),
            (
                "ticket.tag.rename",
                ticket::WorkspaceTagAction::Rename {
                    tag_id: "tag".into(),
                    expected_last_operation_key: "old".into(),
                    name: "new".into(),
                },
            ),
            (
                "ticket.tag.delete",
                ticket::WorkspaceTagAction::Delete {
                    tag_id: "tag".into(),
                    expected_last_operation_key: "old".into(),
                },
            ),
        ] {
            operations.push((
                name,
                TicketBackendOperation::MutateWorkspaceTag {
                    request: ticket::WorkspaceTagMutation {
                        operation_key: "tag-op".into(),
                        action,
                    },
                },
            ));
        }
        for (name, operation) in operations {
            let client = ScopedTicketHost::new(vec![]);
            let error = WorkspaceHttpTicketBackend::new(client.clone())
                .invoke(operation)
                .unwrap_err();
            assert!(
                error.to_string().contains("HTTP status 403"),
                "{name}: {error}"
            );
            assert_eq!(client.downstream_count(), 0, "{name}");
            let requests = client.requests.lock().unwrap();
            assert_eq!(requests.len(), 1, "{name}");
            let check: Value = serde_json::from_str(requests[0].body.as_deref().unwrap()).unwrap();
            assert_eq!(check["operation"], name);
            let collection = matches!(
                name,
                "ticket.tag.list"
                    | "ticket.intake.default_body"
                    | "ticket.list"
                    | "ticket.create"
                    | "ticket.relation.query"
                    | "ticket.orchestration_plan.query"
                    | "ticket.doctor"
                    | "ticket.tag.create"
                    | "ticket.tag.rename"
                    | "ticket.tag.delete"
            );
            assert_eq!(
                check["resource"],
                if collection {
                    json!({"kind":"workspace", "workspace_id":"workspace"})
                } else {
                    json!({"kind":"ticket", "workspace_id":"workspace", "ticket_id":"T-739"})
                }
            );
        }
    }

    #[tokio::test]
    async fn ticket_read_tools_native_wip_and_handoff_service_deny_before_read() {
        for kind in [
            WorkspaceTicketReadKind::Query,
            WorkspaceTicketReadKind::Show,
        ] {
            let client = ScopedTicketHost::new(vec![]);
            let (_, tool) = workspace_ticket_read_definition(client.clone(), kind)();
            let input = match kind {
                WorkspaceTicketReadKind::Query => "{}",
                WorkspaceTicketReadKind::Show => r#"{"id":"T-739"}"#,
            };
            assert!(
                tool.execute(input, ToolExecutionContext::direct())
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("HTTP status 403")
            );
            assert_eq!(client.downstream_count(), 0);
        }
        let client = ScopedTicketHost::new(vec![]);
        let service = WorkspaceTicketService {
            backend: WorkspaceHttpTicketBackend::new(client.clone()),
            access: TicketFeatureAccess::read_only(),
        };
        assert!(
            service
                .ticket_handoff("T-739")
                .unwrap_err()
                .to_string()
                .contains("HTTP status 403")
        );
        assert_eq!(client.downstream_count(), 0);
        let handler = TicketItemWipHandler {
            tools: operation_map(
                native_ticket_tools(client.clone(), TicketFeatureAccess::read_only()).unwrap(),
            ),
            permissions: None,
            ticket_reference: "T-739".into(),
            observations: Arc::new(Mutex::new(TicketObservationState::default())),
        };
        let error = handler
            .call(
                "read",
                &BTreeMap::new(),
                WipCallContext {
                    execution: ToolExecutionContext::direct(),
                    security_context: "test".into(),
                },
            )
            .await
            .err()
            .expect("native read must reject before accessing Ticket data");
        assert!(matches!(
            error,
            WipOperationError::Protocol(ProtocolError {
                code: ProtocolErrorCode::PermissionDenied,
                ..
            })
        ));
        assert_eq!(client.downstream_count(), 0);
    }
    #[test]
    fn ticket_specific_grants_do_not_cross_tickets_or_allow_relationship_target_access() {
        let mut wrong_ticket = ticket_permission("ticket.read", "workspace");
        wrong_ticket.resource = server_api::authorization::PermissionResource::Ticket {
            workspace_id: "workspace".into(),
            ticket_id: "T-740".into(),
        };
        for permission in [ticket_permission("ticket.read", "workspace"), wrong_ticket] {
            let client = ScopedTicketHost::new(vec![permission]);
            assert!(
                WorkspaceHttpTicketBackend::new(client.clone())
                    .show(TicketIdOrSlug::from("T-739"))
                    .unwrap_err()
                    .to_string()
                    .contains("HTTP status 403")
            );
            assert_eq!(client.downstream_count(), 0);
        }
        let mut source_grant = ticket_permission("ticket.relation.record", "workspace");
        source_grant.resource = server_api::authorization::PermissionResource::Ticket {
            workspace_id: "workspace".into(),
            ticket_id: "T-739".into(),
        };
        let client = ScopedTicketHost::new(vec![source_grant]);
        assert!(
            WorkspaceHttpTicketBackend::new(client.clone())
                .add_ticket_relation(
                    TicketIdOrSlug::from("T-739"),
                    NewTicketRelation {
                        kind: TicketRelationKind::Related,
                        target: "T-740".into(),
                        note: None,
                        author: None
                    }
                )
                .unwrap_err()
                .to_string()
                .contains("HTTP status 403")
        );
        assert_eq!(
            client.downstream_count(),
            0,
            "deny target before reading either record"
        );
        let requests = client.requests.lock().unwrap();
        let targets: Vec<Value> = requests
            .iter()
            .map(|request| {
                serde_json::from_str::<Value>(request.body.as_deref().unwrap()).unwrap()["resource"]
                    ["ticket_id"]
                    .clone()
            })
            .collect();
        assert_eq!(targets, vec![json!("T-739"), json!("T-740")]);
    }

    #[derive(Debug)]
    struct MidCallRevocationHost {
        case: bool,
        provider_requests: Mutex<Vec<WorkspaceRequest>>,
        record: Value,
    }
    impl WorkspaceClient for MidCallRevocationHost {
        fn workspace_id(&self) -> Option<&str> {
            Some("workspace")
        }
        fn kind(&self) -> &str {
            "mid-call-revocation-fixture"
        }
        fn is_available(&self) -> bool {
            true
        }
        fn execute(
            &self,
            request: WorkspaceRequest,
        ) -> Result<crate::worker::WorkspaceResponse, crate::worker::WorkspaceClientError> {
            if request.path.ends_with("/authorization/check") {
                let check: Value = serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
                assert_eq!(check["resource"]["kind"], "ticket");
                let operation = check["operation"].as_str().unwrap();
                let calls = self.provider_requests.lock().unwrap().len();
                let revoked = if self.case {
                    calls > 0 && operation == "ticket.read"
                } else {
                    calls >= 2 && operation == "ticket.relation.record"
                };
                return Ok(crate::worker::WorkspaceResponse {
                    status: if revoked { 403 } else { 200 },
                    body: if revoked {
                        json!({"message":"permission revoked during operation"}).to_string()
                    } else {
                        "{}".into()
                    },
                });
            }
            assert!(if self.case {
                request.path.ends_with("/case")
            } else {
                request.method == WorkspaceRequestMethod::Get
            });
            self.provider_requests.lock().unwrap().push(request);
            Ok(crate::worker::WorkspaceResponse {
                status: 200,
                body: if self.case {
                    self.record["meta"].to_string()
                } else {
                    json!({"resource_key":"T-739"}).to_string()
                },
            })
        }
    }

    #[test]
    fn permission_revoked_during_operation_prevents_next_access_without_disguising_prior_effect() {
        for case in [false, true] {
            let client = Arc::new(MidCallRevocationHost {
                case,
                provider_requests: Default::default(),
                record: serde_json::from_str(
                    &decision_client(200, TicketState::Closed).response.body,
                )
                .unwrap(),
            });
            let backend = WorkspaceHttpTicketBackend::new(client.clone());
            let error = if case {
                let mut request = decision_input();
                request.as_object_mut().unwrap().remove("ticket");
                backend
                    .update_case("T-739", serde_json::from_value(request).unwrap())
                    .unwrap_err()
            } else {
                backend
                    .add_ticket_relation(
                        TicketIdOrSlug::from("T-739"),
                        NewTicketRelation {
                            kind: TicketRelationKind::Related,
                            target: "T-740".into(),
                            note: None,
                            author: None,
                        },
                    )
                    .unwrap_err()
            };
            assert!(error.to_string().contains("HTTP status 403"), "{error}");
            let requests = client.provider_requests.lock().unwrap();
            if case {
                assert_eq!(
                    requests.len(),
                    1,
                    "judgment dispatched, but no post-write record read"
                );
                assert!(matches!(
                    map_ticket_tool_error(ToolError::ExecutionFailed(error.to_string())),
                    WipOperationError::OutcomeUnknown(_)
                ));
            } else {
                assert_eq!(
                    requests.len(),
                    2,
                    "two permitted reads, no relationship effect after revocation"
                );
                assert!(
                    requests
                        .iter()
                        .all(|request| request.method == WorkspaceRequestMethod::Get)
                );
            }
        }
    }
}
