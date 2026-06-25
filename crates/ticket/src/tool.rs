//! LLM tool implementations for typed Ticket backend operations.
//!
//! These tools are intentionally owned by the `ticket` crate so Worker features can
//! install Ticket behavior without reimplementing domain/backend logic or
//! granting generic filesystem write authority.

use std::sync::Arc;

use async_trait::async_trait;
use llm_engine::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    AcceptedOrchestrationPlan, LocalTicketBackend, MarkdownText, NewOrchestrationPlanRecord,
    NewTicket, NewTicketEvent, NewTicketRelation, OrchestrationPlanKind, Ticket, TicketBackend,
    TicketDoctorDiagnostic, TicketDoctorReport, TicketDoctorSeverity, TicketError, TicketEventKind,
    TicketIdOrSlug, TicketIntakeSummary, TicketRelationKind, TicketReview, TicketReviewResult,
    TicketStateChange, TicketSummary, TicketWorkflowState,
};

const DEFAULT_LIST_LIMIT: usize = 50;
const MAX_LIST_LIMIT: usize = 100;
const LIST_TITLE_MAX_CHARS: usize = 96;
const LIST_HINT_MAX_CHARS: usize = 80;
const DEFAULT_EVENT_LIMIT: usize = 20;
const MAX_EVENT_LIMIT: usize = 100;
const DEFAULT_ARTIFACT_LIMIT: usize = 50;
const MAX_ARTIFACT_LIMIT: usize = 200;
const DEFAULT_BODY_MAX_BYTES: usize = 16 * 1024;
const MAX_BODY_MAX_BYTES: usize = 64 * 1024;
const DEFAULT_DIAGNOSTIC_LIMIT: usize = 100;
const MAX_DIAGNOSTIC_LIMIT: usize = 500;

pub const TICKET_BASE_TOOL_NAMES: [&str; 9] = [
    "TicketCreate",
    "TicketList",
    "TicketShow",
    "TicketComment",
    "TicketReview",
    "TicketIntakeReady",
    "TicketWorkflowState",
    "TicketClose",
    "TicketDoctor",
];

pub const TICKET_BASE_READ_ONLY_TOOL_NAMES: [&str; 3] =
    ["TicketList", "TicketShow", "TicketDoctor"];

pub const TICKET_ORCHESTRATION_TOOL_NAMES: [&str; 4] = [
    "TicketRelationRecord",
    "TicketRelationQuery",
    "TicketOrchestrationPlanRecord",
    "TicketOrchestrationPlanQuery",
];

pub const TICKET_ORCHESTRATION_READ_ONLY_TOOL_NAMES: [&str; 2] =
    ["TicketRelationQuery", "TicketOrchestrationPlanQuery"];

pub const TICKET_TOOL_NAMES: [&str; 13] = [
    "TicketCreate",
    "TicketList",
    "TicketShow",
    "TicketComment",
    "TicketReview",
    "TicketIntakeReady",
    "TicketWorkflowState",
    "TicketClose",
    "TicketRelationRecord",
    "TicketRelationQuery",
    "TicketOrchestrationPlanRecord",
    "TicketOrchestrationPlanQuery",
    "TicketDoctor",
];

pub const TICKET_READ_ONLY_TOOL_NAMES: [&str; 5] = [
    "TicketList",
    "TicketShow",
    "TicketRelationQuery",
    "TicketOrchestrationPlanQuery",
    "TicketDoctor",
];

pub const TICKET_MUTATING_TOOL_NAMES: [&str; 8] = [
    "TicketCreate",
    "TicketComment",
    "TicketReview",
    "TicketIntakeReady",
    "TicketWorkflowState",
    "TicketClose",
    "TicketRelationRecord",
    "TicketOrchestrationPlanRecord",
];

const CREATE_DESCRIPTION: &str = "Create a Ticket through the configured typed Ticket backend. \
Inputs mirror the Ticket `item.md` fields; `title` is required, `body` is Markdown, and the \
backend assigns the id and writes the local Ticket file layout under the configured backend root.";
const LIST_DESCRIPTION: &str = "List Tickets from the configured typed Ticket backend as a \
lightweight bounded overview for selection only. Filter by state (`planning`, `ready`, `queued`, \
`inprogress`, `done`, `closed`, or `all`). Output is short summaries only; use TicketShow before \
routing, closing, planning, or implementation decisions.";
const SHOW_DESCRIPTION: &str = "Show one Ticket by id or exact query through the configured \
typed Ticket backend. Output includes bounded Markdown body, recent thread events, resolution, and \
artifact metadata.";
const COMMENT_DESCRIPTION: &str = "Append a typed Ticket thread event. `role` must be `comment`, \
`plan`, `decision`, or `implementation_report`; `body` is Markdown. Writes stay inside the \
configured Ticket backend root.";
const REVIEW_DESCRIPTION: &str = "Append a Ticket review event. `result` must be `approve` or \
`request_changes`; `body` is Markdown. Writes stay inside the configured Ticket backend root.";
const INTAKE_READY_DESCRIPTION: &str = "Mark an existing Ticket planning lane ready through the typed \
Ticket backend. The tool appends a bounded `intake_summary`, appends a typed `state_changed` event \
for `state`, and transitions state to `ready`.";
const WORKFLOW_STATE_DESCRIPTION: &str = "Transition Ticket `state` through the typed \
Ticket backend with a bounded `state_changed` event. Treat `queued -> inprogress` \
as the implementation acceptance step: implementation side effects should happen only after that \
transition is accepted and recorded. Orchestrator may return `ready` or `queued` Tickets to `planning` only with a concrete missing decision/information reason.";
const CLOSE_DESCRIPTION: &str = "Close a Ticket with a Markdown resolution through the typed Ticket \
backend. The backend sets `state: closed`, writes resolution.md, updates item.md, and appends \
a close event.";
const RELATION_RECORD_DESCRIPTION: &str = "Record a forward typed Ticket-to-Ticket relation as durable \
project-level metadata. Supported kinds are depends_on, blocks, related, supersedes, and duplicate_of; \
inverse views are derived, not stored.";
const RELATION_QUERY_DESCRIPTION: &str = "Query durable typed Ticket relation metadata. When a Ticket \
is provided, both outgoing records owned by it and incoming forward records that target it are returned.";
const ORCHESTRATION_PLAN_RECORD_DESCRIPTION: &str = "Append a typed Ticket orchestration plan record \
for ordering, dependency, conflict, waiting/capacity, or accepted-plan decisions. Records are durable \
Ticket artifacts and do not move state, reorder queues, or start work.";
const ORCHESTRATION_PLAN_QUERY_DESCRIPTION: &str = "Query durable Ticket orchestration plan records by \
Ticket id and/or relation kind. This is read-only planning context; Orchestrator must still make \
explicit state decisions.";
const DOCTOR_DESCRIPTION: &str = "Run typed Ticket backend consistency checks and return bounded \
diagnostics through the typed backend without shelling out to external commands.";

fn base_tool_description(name: &str) -> &'static str {
    match name {
        "TicketCreate" => CREATE_DESCRIPTION,
        "TicketList" => LIST_DESCRIPTION,
        "TicketShow" => SHOW_DESCRIPTION,
        "TicketComment" => COMMENT_DESCRIPTION,
        "TicketReview" => REVIEW_DESCRIPTION,
        "TicketIntakeReady" => INTAKE_READY_DESCRIPTION,
        "TicketWorkflowState" => WORKFLOW_STATE_DESCRIPTION,
        "TicketClose" => CLOSE_DESCRIPTION,
        "TicketRelationRecord" => RELATION_RECORD_DESCRIPTION,
        "TicketRelationQuery" => RELATION_QUERY_DESCRIPTION,
        "TicketOrchestrationPlanRecord" => ORCHESTRATION_PLAN_RECORD_DESCRIPTION,
        "TicketOrchestrationPlanQuery" => ORCHESTRATION_PLAN_QUERY_DESCRIPTION,
        "TicketDoctor" => DOCTOR_DESCRIPTION,
        _ => "Ticket backend tool.",
    }
}

/// Build the model-visible Ticket tool description for a configured Ticket backend.
///
/// `record_language` is the durable Ticket record/tool-body language, distinct from
/// worker response language and Memory/Knowledge language. Keeping this on the tool
/// surface ensures every Ticket-capable Worker sees the policy without hidden context
/// injection or role-launch-only prose.
pub fn ticket_tool_description(name: &str, record_language: Option<&str>) -> String {
    let mut description = base_tool_description(name).to_string();
    if let Some(language) = record_language.filter(|language| !language.trim().is_empty()) {
        description.push_str("\n\nTicket record language: ");
        description.push_str(language.trim());
        description.push_str(". Use this language for durable Ticket record and Ticket tool body text, including Ticket item bodies, thread comments/plans/decisions/implementation reports, reviews, resolutions, intake summaries, and orchestration plan notes. This policy is distinct from worker.language for normal prose and memory.language for Memory/Knowledge. Preserve protocol literals, file paths, commands, logs, identifiers, and quoted external text when translation would reduce fidelity.");
    }
    description
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TicketCreateParams {
    /// Ticket title. Must not be empty.
    title: String,
    /// Markdown body for item.md. If omitted, a small default body is used.
    #[serde(default)]
    body: Option<String>,
    /// Optional thread author for the create event.
    #[serde(default)]
    author: Option<String>,
    /// Optional assignee frontmatter value.
    #[serde(default)]
    assignee: Option<String>,
    /// Optional readiness frontmatter value.
    #[serde(default)]
    readiness: Option<String>,
    /// Optional risk flag frontmatter values.
    #[serde(default)]
    risk_flags: Vec<String>,
    /// Optional state frontmatter value. Defaults to `planning`.
    #[serde(default)]
    state: Option<TicketWorkflowStateParam>,
    /// Optional queued_by frontmatter value.
    #[serde(default)]
    queued_by: Option<String>,
    /// Optional queued_at frontmatter value.
    #[serde(default)]
    queued_at: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum TicketWorkflowStateParam {
    Planning,
    Ready,
    Queued,
    Inprogress,
    Done,
    Closed,
}

impl TicketWorkflowStateParam {
    fn into_state(self) -> TicketWorkflowState {
        match self {
            Self::Planning => TicketWorkflowState::Planning,
            Self::Ready => TicketWorkflowState::Ready,
            Self::Queued => TicketWorkflowState::Queued,
            Self::Inprogress => TicketWorkflowState::InProgress,
            Self::Done => TicketWorkflowState::Done,
            Self::Closed => TicketWorkflowState::Closed,
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum TicketListStateParam {
    Planning,
    Ready,
    Queued,
    Inprogress,
    Done,
    Closed,
    All,
}

impl TicketListStateParam {
    fn as_filter(self) -> (crate::TicketFilter, &'static str) {
        match self {
            Self::Planning => (
                crate::TicketFilter::state(TicketWorkflowState::Planning),
                "planning",
            ),
            Self::Ready => (
                crate::TicketFilter::state(TicketWorkflowState::Ready),
                "ready",
            ),
            Self::Queued => (
                crate::TicketFilter::state(TicketWorkflowState::Queued),
                "queued",
            ),
            Self::Inprogress => (
                crate::TicketFilter::state(TicketWorkflowState::InProgress),
                "inprogress",
            ),
            Self::Done => (
                crate::TicketFilter::state(TicketWorkflowState::Done),
                "done",
            ),
            Self::Closed => (
                crate::TicketFilter::state(TicketWorkflowState::Closed),
                "closed",
            ),
            Self::All => (crate::TicketFilter::all(), "all"),
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TicketListParams {
    /// State filter. Defaults to all Tickets.
    #[serde(default)]
    state: Option<TicketListStateParam>,
    /// Maximum number of summaries to return. Defaults to 50, max 100.
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TicketShowParams {
    /// Ticket id. Exactly one of `id` or `query` must be provided.
    #[serde(default)]
    id: Option<String>,
    /// Exact ticket id query. Exactly one of `id` or `query` must be provided.
    #[serde(default)]
    query: Option<String>,
    /// Maximum number of most-recent thread events to return. Defaults to 20, max 100.
    #[serde(default)]
    event_limit: Option<usize>,
    /// Maximum number of artifact metadata entries to return. Defaults to 50, max 200.
    #[serde(default)]
    artifact_limit: Option<usize>,
    /// Maximum bytes for each Markdown body field before adding a truncation marker. Defaults to 16 KiB, max 64 KiB.
    #[serde(default)]
    body_max_bytes: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum TicketCommentRoleParam {
    Comment,
    Plan,
    Decision,
    ImplementationReport,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TicketCommentParams {
    /// Ticket id.
    ticket: String,
    /// Thread event role: `comment`, `plan`, `decision`, or `implementation_report`.
    role: TicketCommentRoleParam,
    /// Markdown event body.
    body: String,
    /// Optional thread author.
    #[serde(default)]
    author: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum TicketReviewResultParam {
    Approve,
    RequestChanges,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TicketReviewParams {
    /// Ticket id.
    ticket: String,
    /// Review result: `approve` or `request_changes`.
    result: TicketReviewResultParam,
    /// Markdown review body.
    body: String,
    /// Optional thread author.
    #[serde(default)]
    author: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TicketIntakeReadyParams {
    /// Ticket id.
    ticket: String,
    /// Concise bounded intake summary to append as a typed intake_summary event.
    intake_summary: String,
    /// Optional author for both intake_summary and state_changed events.
    #[serde(default)]
    author: Option<String>,
    /// Reason attached to the state_changed event. Defaults to `planning_ready`.
    #[serde(default)]
    reason: Option<String>,
    /// Optional state_changed body. If omitted, a concise default is used.
    #[serde(default)]
    state_change_body: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TicketWorkflowStateParams {
    /// Ticket id.
    ticket: String,
    /// Expected current state. The backend rejects stale transitions.
    from: TicketWorkflowStateParam,
    /// Target state.
    to: TicketWorkflowStateParam,
    /// Reason attached to the typed state_changed event.
    reason: String,
    /// Markdown body for the typed state_changed event.
    body: String,
    /// Optional thread author.
    #[serde(default)]
    author: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TicketCloseParams {
    /// Ticket id.
    ticket: String,
    /// Markdown resolution written to resolution.md and thread.md.
    resolution: String,
}

#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum TicketRelationKindParam {
    DependsOn,
    Blocks,
    Related,
    Supersedes,
    DuplicateOf,
}

impl TicketRelationKindParam {
    fn into_kind(self) -> TicketRelationKind {
        match self {
            Self::DependsOn => TicketRelationKind::DependsOn,
            Self::Blocks => TicketRelationKind::Blocks,
            Self::Related => TicketRelationKind::Related,
            Self::Supersedes => TicketRelationKind::Supersedes,
            Self::DuplicateOf => TicketRelationKind::DuplicateOf,
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TicketRelationRecordParams {
    /// Ticket id that owns the forward relation.
    ticket: String,
    /// Forward relation kind: depends_on, blocks, related, supersedes, or duplicate_of.
    kind: TicketRelationKindParam,
    /// Target canonical Ticket id. Title/slug words are not accepted as relation authority.
    target: String,
    /// Optional bounded rationale/note.
    #[serde(default)]
    note: Option<String>,
    /// Optional record author.
    #[serde(default)]
    author: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TicketRelationQueryParams {
    /// Optional Ticket id to query. Includes outgoing and incoming forward records for that id.
    #[serde(default)]
    ticket: Option<String>,
    /// Optional forward relation kind filter.
    #[serde(default)]
    kind: Option<TicketRelationKindParam>,
    /// Maximum records to return. Defaults to 100, max 200.
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Serialize)]
struct TicketRelationQueryOutput {
    count: usize,
    returned: usize,
    truncated: bool,
    relations: Vec<Value>,
}

#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum OrchestrationPlanKindParam {
    Before,
    After,
    BlockedBy,
    Blocks,
    ConflictsWith,
    DoNotParallelize,
    WaitingCapacityNote,
    AcceptedPlan,
}

impl OrchestrationPlanKindParam {
    fn into_kind(self) -> OrchestrationPlanKind {
        match self {
            Self::Before => OrchestrationPlanKind::Before,
            Self::After => OrchestrationPlanKind::After,
            Self::BlockedBy => OrchestrationPlanKind::BlockedBy,
            Self::Blocks => OrchestrationPlanKind::Blocks,
            Self::ConflictsWith => OrchestrationPlanKind::ConflictsWith,
            Self::DoNotParallelize => OrchestrationPlanKind::DoNotParallelize,
            Self::WaitingCapacityNote => OrchestrationPlanKind::WaitingCapacityNote,
            Self::AcceptedPlan => OrchestrationPlanKind::AcceptedPlan,
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct AcceptedOrchestrationPlanParams {
    /// Bounded project-relevant accepted plan summary.
    summary: String,
    /// Optional branch name for the accepted plan. Do not include runtime/session/socket details.
    #[serde(default)]
    branch: Option<String>,
    /// Optional worktree path for the accepted plan. Do not include runtime/session/socket details.
    #[serde(default)]
    worktree: Option<String>,
    /// Optional bounded role/work allocation plan. Do not include raw model output or private runtime details.
    #[serde(default)]
    role_plan: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TicketOrchestrationPlanRecordParams {
    /// Ticket id that owns this orchestration plan record.
    ticket: String,
    /// Record kind: before/after, blocked_by/blocks, conflicts_with/do_not_parallelize, waiting_capacity_note, or accepted_plan.
    kind: OrchestrationPlanKindParam,
    /// Related Ticket id for ordering, dependency, and conflict records.
    #[serde(default)]
    related_ticket: Option<String>,
    /// Optional bounded rationale/note. Required for waiting_capacity_note.
    #[serde(default)]
    note: Option<String>,
    /// Accepted plan fields. Required for accepted_plan and invalid for other kinds.
    #[serde(default)]
    accepted_plan: Option<AcceptedOrchestrationPlanParams>,
    /// Optional record author.
    #[serde(default)]
    author: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TicketOrchestrationPlanQueryParams {
    /// Optional Ticket id to query. Omit to query across the backend root.
    #[serde(default)]
    ticket: Option<String>,
    /// Optional relation kind filter.
    #[serde(default)]
    relation_kind: Option<OrchestrationPlanKindParam>,
    /// Maximum records to return. Defaults to 100, max 200.
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Serialize)]
struct TicketOrchestrationPlanQueryOutput {
    count: usize,
    returned: usize,
    truncated: bool,
    records: Vec<Value>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TicketDoctorParams {
    /// Maximum diagnostics to return. Defaults to 100, max 500.
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Serialize)]
struct TicketRefOutput {
    id: String,
    state: String,
}

#[derive(Debug, Serialize)]
struct TicketListOutput {
    state_filter: String,
    count: usize,
    returned: usize,
    truncated: bool,
    limit: usize,
    tickets: Vec<TicketListTicketOutput>,
}

#[derive(Debug, Serialize)]
struct TicketListTicketOutput {
    id: String,
    title: String,
    state: String,
    updated_at: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    hints: Vec<String>,
}

#[derive(Debug, Serialize)]
struct TicketDoctorOutput {
    ok: bool,
    error_count: usize,
    diagnostic_count: usize,
    returned: usize,
    truncated: bool,
    diagnostics: Vec<Value>,
}

#[derive(Clone)]
struct TicketCreateTool {
    backend: LocalTicketBackend,
}

#[derive(Clone)]
struct TicketListTool {
    backend: LocalTicketBackend,
}

#[derive(Clone)]
struct TicketShowTool {
    backend: LocalTicketBackend,
}

#[derive(Clone)]
struct TicketCommentTool {
    backend: LocalTicketBackend,
}

#[derive(Clone)]
struct TicketReviewTool {
    backend: LocalTicketBackend,
}

#[derive(Clone)]
struct TicketIntakeReadyTool {
    backend: LocalTicketBackend,
}

#[derive(Clone)]
struct TicketWorkflowStateTool {
    backend: LocalTicketBackend,
}

#[derive(Clone)]
struct TicketCloseTool {
    backend: LocalTicketBackend,
}

#[derive(Clone)]
struct TicketRelationRecordTool {
    backend: LocalTicketBackend,
}

#[derive(Clone)]
struct TicketRelationQueryTool {
    backend: LocalTicketBackend,
}

#[derive(Clone)]
struct TicketOrchestrationPlanRecordTool {
    backend: LocalTicketBackend,
}

#[derive(Clone)]
struct TicketOrchestrationPlanQueryTool {
    backend: LocalTicketBackend,
}

#[derive(Clone)]
struct TicketDoctorTool {
    backend: LocalTicketBackend,
}

#[async_trait]
impl Tool for TicketCreateTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: TicketCreateParams = parse_input("TicketCreate", input_json)?;
        let mut input = NewTicket::new(params.title);
        if let Some(body) = params.body {
            input.body = MarkdownText::new(body);
        }
        input.author = params.author;
        input.assignee = params.assignee;
        input.readiness = params.readiness;
        input.risk_flags = params.risk_flags;
        input.workflow_state = params.state.map(TicketWorkflowStateParam::into_state);
        input.queued_by = params.queued_by;
        input.queued_at = params.queued_at;

        let created = self
            .backend
            .create(input)
            .map_err(|error| backend_error("TicketCreate", error))?;
        Ok(json_output(
            format!("Created ticket {}", created.id),
            json!(TicketRefOutput {
                id: created.id,
                state: "planning".to_string(),
            }),
        ))
    }
}

#[async_trait]
impl Tool for TicketListTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: TicketListParams = parse_input("TicketList", input_json)?;
        let state = params.state.unwrap_or(TicketListStateParam::All);
        let (filter, state_filter) = state.as_filter();
        let limit = bounded(params.limit, DEFAULT_LIST_LIMIT, MAX_LIST_LIMIT);
        let tickets = self
            .backend
            .list(filter)
            .map_err(|error| backend_error("TicketList", error))?;
        let count = tickets.len();
        let returned_tickets: Vec<_> = tickets
            .into_iter()
            .take(limit)
            .map(ticket_summary_json)
            .collect();
        let output = TicketListOutput {
            state_filter: state_filter.to_string(),
            count,
            returned: returned_tickets.len(),
            truncated: count > returned_tickets.len(),
            limit,
            tickets: returned_tickets,
        };
        Ok(json_output(
            format!(
                "Listed {} ticket(s) for state {state_filter}{}",
                output.returned,
                if output.truncated { " (truncated)" } else { "" }
            ),
            output,
        ))
    }
}

#[async_trait]
impl Tool for TicketShowTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: TicketShowParams = parse_input("TicketShow", input_json)?;
        let query = id_or_query(params.id, params.query)?;
        let event_limit = bounded(params.event_limit, DEFAULT_EVENT_LIMIT, MAX_EVENT_LIMIT);
        let artifact_limit = bounded(
            params.artifact_limit,
            DEFAULT_ARTIFACT_LIMIT,
            MAX_ARTIFACT_LIMIT,
        );
        let body_max_bytes = bounded(
            params.body_max_bytes,
            DEFAULT_BODY_MAX_BYTES,
            MAX_BODY_MAX_BYTES,
        );
        let ticket = self
            .backend
            .show(query)
            .map_err(|error| backend_error("TicketShow", error))?;
        let summary = format!(
            "Ticket {} state {}",
            ticket.meta.id,
            ticket.meta.workflow_state.as_str()
        );
        Ok(json_output(
            summary,
            ticket_json(&ticket, event_limit, artifact_limit, body_max_bytes),
        ))
    }
}

#[async_trait]
impl Tool for TicketCommentTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: TicketCommentParams = parse_input("TicketComment", input_json)?;
        let kind = match params.role {
            TicketCommentRoleParam::Comment => TicketEventKind::Comment,
            TicketCommentRoleParam::Plan => TicketEventKind::Plan,
            TicketCommentRoleParam::Decision => TicketEventKind::Decision,
            TicketCommentRoleParam::ImplementationReport => TicketEventKind::ImplementationReport,
        };
        let role = kind.as_str().to_string();
        let mut event = NewTicketEvent::new(kind, params.body);
        event.author = params.author;
        self.backend
            .add_event(TicketIdOrSlug::Query(params.ticket.clone()), event)
            .map_err(|error| backend_error("TicketComment", error))?;
        Ok(json_output(
            format!("Appended {role} event to ticket {}", params.ticket),
            json!({ "ticket": params.ticket, "event": role, "ok": true }),
        ))
    }
}

#[async_trait]
impl Tool for TicketReviewTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: TicketReviewParams = parse_input("TicketReview", input_json)?;
        let result = match params.result {
            TicketReviewResultParam::Approve => TicketReviewResult::Approve,
            TicketReviewResultParam::RequestChanges => TicketReviewResult::RequestChanges,
        };
        let result_str = result.as_str().to_string();
        let review = TicketReview {
            result,
            author: params.author,
            body: MarkdownText::new(params.body),
        };
        self.backend
            .review(TicketIdOrSlug::Query(params.ticket.clone()), review)
            .map_err(|error| backend_error("TicketReview", error))?;
        Ok(json_output(
            format!("Appended {result_str} review to ticket {}", params.ticket),
            json!({ "ticket": params.ticket, "review": result_str, "ok": true }),
        ))
    }
}

#[async_trait]
impl Tool for TicketIntakeReadyTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: TicketIntakeReadyParams = parse_input("TicketIntakeReady", input_json)?;
        let from = TicketWorkflowState::Planning;
        let reason = params
            .reason
            .unwrap_or_else(|| "planning_ready".to_string());
        let body = params.state_change_body.unwrap_or_else(|| {
            self.backend
                .default_intake_ready_state_change_body(from.as_str())
        });
        let mut summary = TicketIntakeSummary::new(params.intake_summary);
        summary.author = params.author.clone();
        let mut change = TicketStateChange::new(
            from.as_str(),
            TicketWorkflowState::Ready.as_str(),
            reason,
            body,
        );
        change.author = params.author;
        self.backend
            .mark_intake_ready(
                TicketIdOrSlug::Query(params.ticket.clone()),
                summary,
                change,
            )
            .map_err(|error| backend_error("TicketIntakeReady", error))?;
        Ok(json_output(
            format!("Marked ticket {} state ready", params.ticket),
            json!({ "ticket": params.ticket, "state": "ready", "ok": true }),
        ))
    }
}

#[async_trait]
impl Tool for TicketWorkflowStateTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: TicketWorkflowStateParams = parse_input("TicketWorkflowState", input_json)?;
        let from = params.from.into_state();
        let to = params.to.into_state();
        if from == to {
            return Err(ToolError::InvalidArgument(
                "state transition must change state".to_string(),
            ));
        }
        let mut change =
            TicketStateChange::new(from.as_str(), to.as_str(), params.reason, params.body);
        change.author = params.author;
        self.backend
            .set_workflow_state(TicketIdOrSlug::Query(params.ticket.clone()), change)
            .map_err(|error| backend_error("TicketWorkflowState", error))?;
        Ok(json_output(
            format!(
                "Transitioned ticket {} state {} -> {}",
                params.ticket,
                from.as_str(),
                to.as_str()
            ),
            json!({
                "ticket": params.ticket,
                "from": from.as_str(),
                "to": to.as_str(),
                "state": to.as_str(),
                "ok": true
            }),
        ))
    }
}

#[async_trait]
impl Tool for TicketCloseTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: TicketCloseParams = parse_input("TicketClose", input_json)?;
        self.backend
            .close(
                TicketIdOrSlug::Query(params.ticket.clone()),
                MarkdownText::new(params.resolution),
            )
            .map_err(|error| backend_error("TicketClose", error))?;
        Ok(json_output(
            format!("Closed ticket {}", params.ticket),
            json!({ "ticket": params.ticket, "state": "closed", "ok": true }),
        ))
    }
}

#[async_trait]
impl Tool for TicketRelationRecordTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: TicketRelationRecordParams = parse_input("TicketRelationRecord", input_json)?;
        let relation = NewTicketRelation {
            kind: params.kind.into_kind(),
            target: params.target.clone(),
            note: params.note,
            author: params.author,
        };
        let output = self
            .backend
            .add_ticket_relation(TicketIdOrSlug::Id(params.ticket.clone()), relation)
            .map_err(|error| backend_error("TicketRelationRecord", error))?;
        Ok(json_output(
            format!(
                "Recorded ticket relation {} {} {}",
                output.ticket_id, output.kind, output.target
            ),
            ticket_relation_json(&output),
        ))
    }
}

#[async_trait]
impl Tool for TicketRelationQueryTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: TicketRelationQueryParams = parse_input("TicketRelationQuery", input_json)?;
        let limit = bounded(params.limit, DEFAULT_LIST_LIMIT, MAX_LIST_LIMIT);
        let ticket = params.ticket.clone().map(TicketIdOrSlug::Id);
        let kind = params.kind.map(TicketRelationKindParam::into_kind);
        let relations = self
            .backend
            .query_ticket_relations(ticket, kind)
            .map_err(|error| backend_error("TicketRelationQuery", error))?;
        let count = relations.len();
        let truncated = count > limit;
        let returned_relations = relations
            .into_iter()
            .take(limit)
            .map(|relation| ticket_relation_json(&relation))
            .collect::<Vec<_>>();
        Ok(json_output(
            format!(
                "Found {} ticket relation(s){}",
                count,
                if truncated { " (truncated)" } else { "" }
            ),
            TicketRelationQueryOutput {
                count,
                returned: returned_relations.len(),
                truncated,
                relations: returned_relations,
            },
        ))
    }
}

#[async_trait]
impl Tool for TicketOrchestrationPlanRecordTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: TicketOrchestrationPlanRecordParams =
            parse_input("TicketOrchestrationPlanRecord", input_json)?;
        let accepted_plan = params.accepted_plan.map(|plan| AcceptedOrchestrationPlan {
            summary: plan.summary,
            branch: plan.branch,
            worktree: plan.worktree,
            role_plan: plan.role_plan,
        });
        let record = NewOrchestrationPlanRecord {
            kind: params.kind.into_kind(),
            related_ticket: params.related_ticket,
            note: params.note,
            accepted_plan,
            author: params.author,
        };
        let output = self
            .backend
            .add_orchestration_plan_record(TicketIdOrSlug::Query(params.ticket.clone()), record)
            .map_err(|error| backend_error("TicketOrchestrationPlanRecord", error))?;
        Ok(json_output(
            format!(
                "Recorded orchestration plan {} for ticket {}",
                output.kind, params.ticket
            ),
            output,
        ))
    }
}

#[async_trait]
impl Tool for TicketOrchestrationPlanQueryTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: TicketOrchestrationPlanQueryParams =
            parse_input("TicketOrchestrationPlanQuery", input_json)?;
        let limit = bounded(params.limit, DEFAULT_LIST_LIMIT, MAX_LIST_LIMIT);
        let ticket = params.ticket.clone().map(TicketIdOrSlug::Query);
        let kind = params
            .relation_kind
            .map(OrchestrationPlanKindParam::into_kind);
        let records = self
            .backend
            .query_orchestration_plan_records(ticket, kind)
            .map_err(|error| backend_error("TicketOrchestrationPlanQuery", error))?;
        let count = records.len();
        let truncated = count > limit;
        let returned_records = records
            .into_iter()
            .take(limit)
            .map(|record| serde_json::to_value(record).unwrap_or_else(|_| json!({})))
            .collect::<Vec<_>>();
        Ok(json_output(
            format!(
                "Found {} orchestration plan record(s){}",
                count,
                if truncated { " (truncated)" } else { "" }
            ),
            TicketOrchestrationPlanQueryOutput {
                count,
                returned: returned_records.len(),
                truncated,
                records: returned_records,
            },
        ))
    }
}

#[async_trait]
impl Tool for TicketDoctorTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: TicketDoctorParams = parse_input("TicketDoctor", input_json)?;
        let limit = bounded(params.limit, DEFAULT_DIAGNOSTIC_LIMIT, MAX_DIAGNOSTIC_LIMIT);
        let report = self
            .backend
            .doctor()
            .map_err(|error| backend_error("TicketDoctor", error))?;
        let output = doctor_output(report, limit);
        Ok(json_output(
            format!(
                "Ticket doctor: {} error(s), {} diagnostic(s){}",
                output.error_count,
                output.diagnostic_count,
                if output.truncated { " (truncated)" } else { "" }
            ),
            output,
        ))
    }
}

fn parse_input<T: for<'de> Deserialize<'de>>(tool: &str, input_json: &str) -> Result<T, ToolError> {
    serde_json::from_str(input_json)
        .map_err(|error| ToolError::InvalidArgument(format!("invalid {tool} input: {error}")))
}

fn backend_error(tool: &str, error: TicketError) -> ToolError {
    ToolError::ExecutionFailed(format!("{tool} failed: {error}"))
}

fn bounded(value: Option<usize>, default: usize, max: usize) -> usize {
    value.unwrap_or(default).clamp(1, max)
}

fn id_or_query(id: Option<String>, query: Option<String>) -> Result<TicketIdOrSlug, ToolError> {
    let provided = id.iter().chain(query.iter()).count();
    if provided != 1 {
        return Err(ToolError::InvalidArgument(
            "exactly one of id or query must be provided".to_string(),
        ));
    }
    if let Some(id) = id {
        Ok(TicketIdOrSlug::Id(id))
    } else {
        Ok(TicketIdOrSlug::Query(
            query.expect("provided count checked"),
        ))
    }
}

fn ticket_summary_json(ticket: TicketSummary) -> TicketListTicketOutput {
    let hints = ticket_list_hints(&ticket);
    TicketListTicketOutput {
        id: ticket.id,
        title: truncate_inline(ticket.title.as_str(), LIST_TITLE_MAX_CHARS),
        state: ticket.workflow_state.as_str().to_string(),
        updated_at: ticket.updated_at,
        hints,
    }
}

fn ticket_list_hints(ticket: &TicketSummary) -> Vec<String> {
    let mut hints = Vec::new();
    if let Some(readiness) = ticket.readiness.as_deref() {
        hints.push(format!(
            "readiness:{}",
            truncate_inline(readiness, LIST_HINT_MAX_CHARS)
        ));
    }
    hints
}

fn ticket_relation_json(relation: &crate::TicketRelation) -> Value {
    json!({
        "ticket_id": relation.ticket_id,
        "kind": relation.kind.as_str(),
        "target": relation.target,
        "note": relation.note,
        "author": relation.author,
        "at": relation.at,
    })
}

fn ticket_relations_json(ticket: &Ticket) -> Value {
    let outgoing: Vec<_> = ticket
        .relations
        .outgoing
        .iter()
        .map(ticket_relation_json)
        .collect();
    let incoming: Vec<_> = ticket
        .relations
        .incoming
        .iter()
        .map(|relation| {
            json!({
                "source_ticket": relation.source_ticket,
                "inverse_kind": relation.inverse_kind,
                "forward_kind": relation.forward_kind.as_str(),
                "note": relation.note,
                "author": relation.author,
                "at": relation.at,
            })
        })
        .collect();
    let blockers: Vec<_> = ticket
        .relations
        .blockers
        .iter()
        .map(|blocker| {
            json!({
                "blocking_ticket": blocker.blocking_ticket,
                "reason_kind": blocker.reason_kind,
                "relation_kind": blocker.relation_kind.as_str(),
                "note": blocker.note,
                "blocking_state": blocker.blocking_state.as_str(),
            })
        })
        .collect();
    let notices: Vec<_> = ticket
        .relations
        .notices
        .iter()
        .map(|notice| {
            json!({
                "related_ticket": notice.related_ticket,
                "kind": notice.kind.as_str(),
                "message": notice.message,
            })
        })
        .collect();
    json!({
        "outgoing": outgoing,
        "incoming": incoming,
        "blockers": blockers,
        "notices": notices,
    })
}

fn ticket_json(
    ticket: &Ticket,
    event_limit: usize,
    artifact_limit: usize,
    body_max_bytes: usize,
) -> Value {
    let event_count = ticket.events.len();
    let events: Vec<_> = ticket
        .events
        .iter()
        .skip(event_count.saturating_sub(event_limit))
        .map(|event| {
            json!({
                "kind": event.kind.as_str(),
                "author": event.author,
                "at": event.at,
                "state": event.status,
                "from": event.from,
                "to": event.to,
                "reason": event.reason,
                "state_field": event.state_field,
                "attributes": event.attributes,
                "heading": event.heading,
                "body": truncate_text(event.body.as_str(), body_max_bytes),
            })
        })
        .collect();
    let artifact_count = ticket.artifacts.len();
    let artifacts: Vec<_> = ticket
        .artifacts
        .iter()
        .take(artifact_limit)
        .map(|artifact| artifact.relative_path.display().to_string())
        .collect();
    json!({
        "meta": {
            "id": ticket.meta.id,
            "title": ticket.meta.title,
            "state": ticket.meta.workflow_state.as_str(),
            "created_at": ticket.meta.created_at,
            "updated_at": ticket.meta.updated_at,
            "assignee": ticket.meta.assignee,
            "readiness": ticket.meta.readiness,
            "risk_flags": ticket.meta.risk_flags,
            "queued_by": ticket.meta.queued_by,
            "queued_at": ticket.meta.queued_at,
        },
        "body": truncate_text(ticket.document.body.as_str(), body_max_bytes),
        "events": {
            "count": event_count,
            "returned": events.len(),
            "truncated": event_count > events.len(),
            "items": events,
        },
        "artifacts": {
            "count": artifact_count,
            "returned": artifacts.len(),
            "truncated": artifact_count > artifacts.len(),
            "items": artifacts,
        },
        "relations": ticket_relations_json(ticket),
        "resolution": ticket.resolution.as_ref().map(|resolution| truncate_text(resolution.as_str(), body_max_bytes)),
    })
}

fn doctor_output(report: TicketDoctorReport, limit: usize) -> TicketDoctorOutput {
    let diagnostic_count = report.diagnostics.len();
    let error_count = report.error_count();
    let diagnostics = report
        .diagnostics
        .into_iter()
        .take(limit)
        .map(diagnostic_json)
        .collect::<Vec<_>>();
    TicketDoctorOutput {
        ok: error_count == 0,
        error_count,
        diagnostic_count,
        returned: diagnostics.len(),
        truncated: diagnostic_count > diagnostics.len(),
        diagnostics,
    }
}

fn diagnostic_json(diagnostic: TicketDoctorDiagnostic) -> Value {
    let severity = match diagnostic.severity {
        TicketDoctorSeverity::Error => "error",
        TicketDoctorSeverity::Warning => "warning",
    };
    json!({
        "severity": severity,
        "message": diagnostic.message,
        "path": diagnostic.path.map(|path| path.display().to_string()),
    })
}

fn truncate_inline(text: &str, max_chars: usize) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= max_chars {
        return normalized;
    }
    let marker = "...";
    let take = max_chars.saturating_sub(marker.chars().count());
    let mut out = normalized.chars().take(take).collect::<String>();
    out.push_str(marker);
    out
}

fn truncate_text(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let marker = format!("\n\n[truncated: {} bytes dropped]", text.len() - max_bytes);
    let mut cut = max_bytes.saturating_sub(marker.len());
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut out = text[..cut].to_string();
    out.push_str(&marker);
    out
}

fn json_output(summary: String, value: impl Serialize) -> ToolOutput {
    ToolOutput {
        summary,
        content: Some(serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_string())),
    }
}

fn tool_definition<T>(name: &'static str, backend: LocalTicketBackend) -> ToolDefinition
where
    T: Tool + From<LocalTicketBackend> + 'static,
{
    let description = ticket_tool_description(name, backend.record_language());
    Arc::new(move || {
        let schema_value = input_schema(name);
        let meta = ToolMeta::new(name)
            .description(description.clone())
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(T::from(backend.clone()));
        (meta, tool)
    })
}

fn input_schema(name: &str) -> Value {
    match name {
        "TicketCreate" => serde_json::to_value(schemars::schema_for!(TicketCreateParams)),
        "TicketList" => serde_json::to_value(schemars::schema_for!(TicketListParams)),
        "TicketShow" => serde_json::to_value(schemars::schema_for!(TicketShowParams)),
        "TicketComment" => serde_json::to_value(schemars::schema_for!(TicketCommentParams)),
        "TicketReview" => serde_json::to_value(schemars::schema_for!(TicketReviewParams)),
        "TicketIntakeReady" => serde_json::to_value(schemars::schema_for!(TicketIntakeReadyParams)),
        "TicketWorkflowState" => {
            serde_json::to_value(schemars::schema_for!(TicketWorkflowStateParams))
        }
        "TicketClose" => serde_json::to_value(schemars::schema_for!(TicketCloseParams)),
        "TicketRelationRecord" => {
            serde_json::to_value(schemars::schema_for!(TicketRelationRecordParams))
        }
        "TicketRelationQuery" => {
            serde_json::to_value(schemars::schema_for!(TicketRelationQueryParams))
        }
        "TicketOrchestrationPlanRecord" => {
            serde_json::to_value(schemars::schema_for!(TicketOrchestrationPlanRecordParams))
        }
        "TicketOrchestrationPlanQuery" => {
            serde_json::to_value(schemars::schema_for!(TicketOrchestrationPlanQueryParams))
        }
        "TicketDoctor" => serde_json::to_value(schemars::schema_for!(TicketDoctorParams)),
        _ => Ok(json!({})),
    }
    .unwrap_or_else(|_| json!({}))
}

macro_rules! impl_from_backend {
    ($tool:ident) => {
        impl From<LocalTicketBackend> for $tool {
            fn from(backend: LocalTicketBackend) -> Self {
                Self { backend }
            }
        }
    };
}

impl_from_backend!(TicketCreateTool);
impl_from_backend!(TicketListTool);
impl_from_backend!(TicketShowTool);
impl_from_backend!(TicketCommentTool);
impl_from_backend!(TicketReviewTool);
impl_from_backend!(TicketIntakeReadyTool);
impl_from_backend!(TicketWorkflowStateTool);
impl_from_backend!(TicketCloseTool);
impl_from_backend!(TicketRelationRecordTool);
impl_from_backend!(TicketRelationQueryTool);
impl_from_backend!(TicketOrchestrationPlanRecordTool);
impl_from_backend!(TicketOrchestrationPlanQueryTool);
impl_from_backend!(TicketDoctorTool);

/// Build all MVP Ticket tool definitions over one local backend root.
pub fn ticket_tools(backend: LocalTicketBackend) -> Vec<ToolDefinition> {
    vec![
        tool_definition::<TicketCreateTool>("TicketCreate", backend.clone()),
        tool_definition::<TicketListTool>("TicketList", backend.clone()),
        tool_definition::<TicketShowTool>("TicketShow", backend.clone()),
        tool_definition::<TicketCommentTool>("TicketComment", backend.clone()),
        tool_definition::<TicketReviewTool>("TicketReview", backend.clone()),
        tool_definition::<TicketIntakeReadyTool>("TicketIntakeReady", backend.clone()),
        tool_definition::<TicketWorkflowStateTool>("TicketWorkflowState", backend.clone()),
        tool_definition::<TicketCloseTool>("TicketClose", backend.clone()),
        tool_definition::<TicketRelationRecordTool>("TicketRelationRecord", backend.clone()),
        tool_definition::<TicketRelationQueryTool>("TicketRelationQuery", backend.clone()),
        tool_definition::<TicketOrchestrationPlanRecordTool>(
            "TicketOrchestrationPlanRecord",
            backend.clone(),
        ),
        tool_definition::<TicketOrchestrationPlanQueryTool>(
            "TicketOrchestrationPlanQuery",
            backend.clone(),
        ),
        tool_definition::<TicketDoctorTool>("TicketDoctor", backend),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn backend(temp: &TempDir) -> LocalTicketBackend {
        LocalTicketBackend::new(temp.path().join("tickets"))
    }

    fn tool(definition: ToolDefinition) -> Arc<dyn Tool> {
        let (_, tool) = definition();
        tool
    }

    fn tool_by_name(backend: LocalTicketBackend, name: &str) -> Arc<dyn Tool> {
        ticket_tools(backend)
            .into_iter()
            .find_map(|definition| {
                let (meta, tool) = definition();
                (meta.name == name).then_some(tool)
            })
            .expect("tool exists")
    }

    fn tool_description_by_name(backend: LocalTicketBackend, name: &str) -> String {
        ticket_tools(backend)
            .into_iter()
            .find_map(|definition| {
                let (meta, _) = definition();
                (meta.name == name).then_some(meta.description)
            })
            .expect("tool exists")
    }

    #[test]
    fn ticket_tool_name_partitions_are_explicit() {
        assert_eq!(
            TICKET_READ_ONLY_TOOL_NAMES,
            [
                "TicketList",
                "TicketShow",
                "TicketRelationQuery",
                "TicketOrchestrationPlanQuery",
                "TicketDoctor"
            ]
        );
        assert_eq!(
            TICKET_MUTATING_TOOL_NAMES,
            [
                "TicketCreate",
                "TicketComment",
                "TicketReview",
                "TicketIntakeReady",
                "TicketWorkflowState",
                "TicketClose",
                "TicketRelationRecord",
                "TicketOrchestrationPlanRecord"
            ]
        );
        for name in TICKET_READ_ONLY_TOOL_NAMES {
            assert!(TICKET_TOOL_NAMES.contains(&name));
            assert!(!TICKET_MUTATING_TOOL_NAMES.contains(&name));
        }
        for name in TICKET_MUTATING_TOOL_NAMES {
            assert!(TICKET_TOOL_NAMES.contains(&name));
            assert!(!TICKET_READ_ONLY_TOOL_NAMES.contains(&name));
        }
        assert_eq!(
            TICKET_READ_ONLY_TOOL_NAMES.len() + TICKET_MUTATING_TOOL_NAMES.len(),
            TICKET_TOOL_NAMES.len()
        );
    }

    #[test]
    fn state_tool_description_explains_queued_acceptance() {
        let temp = TempDir::new().unwrap();
        let definition = ticket_tools(backend(&temp))
            .into_iter()
            .find(|definition| definition().0.name == "TicketWorkflowState")
            .expect("state tool exists");
        let (meta, _) = definition();
        assert!(meta.description.contains("queued -> inprogress"));
        assert!(meta.description.contains("implementation side effects"));
    }

    #[test]
    fn tool_descriptions_include_configured_ticket_record_language_guidance() {
        let temp = TempDir::new().unwrap();
        let backend = backend(&temp).with_record_language(Some("Japanese"));
        let description = tool_description_by_name(backend, "TicketComment");

        assert!(description.contains("Ticket record language: Japanese"));
        assert!(description.contains("durable Ticket record and Ticket tool body text"));
        assert!(description.contains("distinct from worker.language"));
        assert!(description.contains("memory.language"));
        assert!(description.contains("Preserve protocol literals"));
        assert!(description.contains("file paths, commands, logs, identifiers"));
    }

    #[test]
    fn tool_descriptions_omit_ticket_record_language_guidance_when_unset() {
        let temp = TempDir::new().unwrap();
        let description = tool_description_by_name(backend(&temp), "TicketComment");

        assert!(!description.contains("Ticket record language:"));
        assert!(!description.contains("worker.language"));
    }

    #[tokio::test]
    async fn ticket_tools_create_list_show_and_doctor() {
        let temp = TempDir::new().unwrap();
        let backend = backend(&temp);
        let create = tool_by_name(backend.clone(), "TicketCreate");
        let list = tool_by_name(backend.clone(), "TicketList");
        let show = tool_by_name(backend.clone(), "TicketShow");
        let doctor = tool_by_name(backend.clone(), "TicketDoctor");

        let created = create
            .execute(
                &json!({
                    "title": "Tool Created",
                    "body": "## Background\n\nCreated by tool.\n"
                })
                .to_string(),
                Default::default(),
            )
            .await
            .unwrap();
        assert!(created.summary.contains("Created ticket"));
        let created_json: Value = serde_json::from_str(&created.content.unwrap()).unwrap();
        let id = created_json["id"].as_str().unwrap().to_string();
        let created_text = created_json.to_string();
        assert!(!created_text.contains("legacy_ticket"));
        assert!(!created_text.contains("needs_preflight"));
        assert!(!created_text.contains("action_required"));
        assert!(!created_text.contains("attention_required"));

        let listed = list
            .execute(
                &json!({ "state": "planning" }).to_string(),
                Default::default(),
            )
            .await
            .unwrap();
        assert!(listed.summary.contains("Listed 1 ticket"));
        let listed_content = listed.content.unwrap();
        assert!(listed_content.contains("Tool Created"));
        assert!(!listed_content.contains("legacy_ticket"));
        assert!(!listed_content.contains("needs_preflight"));

        let shown = show
            .execute(
                &json!({ "id": id, "event_limit": 10 }).to_string(),
                Default::default(),
            )
            .await
            .unwrap();
        assert!(shown.summary.contains(&id));
        let shown_content = shown.content.unwrap();
        assert!(shown_content.contains("Created by tool"));
        assert!(!shown_content.contains("legacy_ticket"));
        assert!(!shown_content.contains("needs_preflight"));
        assert!(!shown_content.contains("action_required"));
        assert!(!shown_content.contains("attention_required"));

        let report = doctor
            .execute(&json!({}).to_string(), Default::default())
            .await
            .unwrap();
        assert!(report.summary.contains("0 error(s)"));
    }

    #[tokio::test]
    async fn ticket_list_tool_truncates_long_titles_and_hints() {
        let temp = TempDir::new().unwrap();
        let backend = backend(&temp);
        let list = tool_by_name(backend.clone(), "TicketList");
        let mut ticket = NewTicket::new(format!(
            "Long Title {}",
            "x".repeat(LIST_TITLE_MAX_CHARS + 40)
        ));
        ticket.readiness = Some(format!(
            "Ready after review {}",
            "a".repeat(LIST_HINT_MAX_CHARS + 40)
        ));
        backend.create(ticket).unwrap();

        let listed = list
            .execute(&json!({}).to_string(), Default::default())
            .await
            .unwrap();
        let listed_json: Value = serde_json::from_str(&listed.content.unwrap()).unwrap();
        let title = listed_json["tickets"][0]["title"].as_str().unwrap();
        assert!(title.chars().count() <= LIST_TITLE_MAX_CHARS);
        assert!(title.ends_with("..."));
        let hint = listed_json["tickets"][0]["hints"][0].as_str().unwrap();
        assert!(hint.chars().count() <= "readiness:".chars().count() + LIST_HINT_MAX_CHARS);
        assert!(hint.ends_with("..."));
    }

    #[tokio::test]
    async fn ticket_list_tool_default_and_max_limits_are_bounded() {
        let temp = TempDir::new().unwrap();
        let backend = backend(&temp);
        let list = tool_by_name(backend.clone(), "TicketList");
        for index in 0..(MAX_LIST_LIMIT + 5) {
            backend
                .create(NewTicket::new(format!("Ticket {index:03}")))
                .unwrap();
        }

        let default_list = list
            .execute(&json!({}).to_string(), Default::default())
            .await
            .unwrap();
        let default_json: Value = serde_json::from_str(&default_list.content.unwrap()).unwrap();
        assert_eq!(
            default_json["count"].as_u64(),
            Some((MAX_LIST_LIMIT + 5) as u64)
        );
        assert_eq!(
            default_json["returned"].as_u64(),
            Some(DEFAULT_LIST_LIMIT as u64)
        );
        assert_eq!(
            default_json["limit"].as_u64(),
            Some(DEFAULT_LIST_LIMIT as u64)
        );
        assert_eq!(default_json["truncated"].as_bool(), Some(true));
        assert_eq!(
            default_json["tickets"].as_array().unwrap().len(),
            DEFAULT_LIST_LIMIT
        );

        let high_limit = list
            .execute(
                &json!({ "limit": MAX_LIST_LIMIT + 500 }).to_string(),
                Default::default(),
            )
            .await
            .unwrap();
        let high_json: Value = serde_json::from_str(&high_limit.content.unwrap()).unwrap();
        assert_eq!(high_json["returned"].as_u64(), Some(MAX_LIST_LIMIT as u64));
        assert_eq!(high_json["limit"].as_u64(), Some(MAX_LIST_LIMIT as u64));
        assert_eq!(high_json["truncated"].as_bool(), Some(true));
        assert_eq!(
            high_json["tickets"].as_array().unwrap().len(),
            MAX_LIST_LIMIT
        );
    }

    #[tokio::test]
    async fn ticket_list_tool_caps_all_and_closed_default_listing() {
        let temp = TempDir::new().unwrap();
        let backend = backend(&temp);
        let list = tool_by_name(backend.clone(), "TicketList");
        for index in 0..(DEFAULT_LIST_LIMIT + 3) {
            let mut ticket = NewTicket::new(format!("Closed Ticket {index:03}"));
            ticket.workflow_state = Some(TicketWorkflowState::Closed);
            backend.create(ticket).unwrap();
        }
        for index in 0..3 {
            backend
                .create(NewTicket::new(format!("Planning Ticket {index:03}")))
                .unwrap();
        }

        let all = list
            .execute(&json!({ "state": "all" }).to_string(), Default::default())
            .await
            .unwrap();
        let all_json: Value = serde_json::from_str(&all.content.unwrap()).unwrap();
        assert_eq!(all_json["state_filter"], "all");
        assert_eq!(
            all_json["returned"].as_u64(),
            Some(DEFAULT_LIST_LIMIT as u64)
        );
        assert_eq!(all_json["truncated"].as_bool(), Some(true));

        let closed = list
            .execute(
                &json!({ "state": "closed" }).to_string(),
                Default::default(),
            )
            .await
            .unwrap();
        let closed_json: Value = serde_json::from_str(&closed.content.unwrap()).unwrap();
        assert_eq!(closed_json["state_filter"], "closed");
        assert_eq!(
            closed_json["count"].as_u64(),
            Some((DEFAULT_LIST_LIMIT + 3) as u64)
        );
        assert_eq!(
            closed_json["returned"].as_u64(),
            Some(DEFAULT_LIST_LIMIT as u64)
        );
        assert_eq!(closed_json["truncated"].as_bool(), Some(true));
    }

    #[tokio::test]
    async fn ticket_list_tool_omits_body_thread_artifact_and_resolution_content() {
        let temp = TempDir::new().unwrap();
        let backend = backend(&temp);
        let list = tool_by_name(backend.clone(), "TicketList");
        let close = tool_by_name(backend.clone(), "TicketClose");
        let body_secret = "ITEM_BODY_SECRET_DO_NOT_LIST";
        let thread_secret = "THREAD_SECRET_DO_NOT_LIST";
        let artifact_secret = "ARTIFACT_SECRET_DO_NOT_LIST";
        let resolution_secret = "RESOLUTION_SECRET_DO_NOT_LIST";
        let mut ticket = NewTicket::new("Leak Probe");
        ticket.body = MarkdownText::new(format!("Item body {body_secret}"));
        ticket.workflow_state = Some(TicketWorkflowState::Done);
        let created = backend.create(ticket).unwrap();
        backend
            .add_event(
                TicketIdOrSlug::Id(created.id.clone()),
                NewTicketEvent::new(TicketEventKind::Comment, format!("Thread {thread_secret}")),
            )
            .unwrap();
        std::fs::write(
            temp.path()
                .join("tickets")
                .join(&created.id)
                .join("artifacts")
                .join("secret.txt"),
            artifact_secret,
        )
        .unwrap();
        close
            .execute(
                &json!({
                    "ticket": created.id,
                    "resolution": format!("Resolution {resolution_secret}")
                })
                .to_string(),
                Default::default(),
            )
            .await
            .unwrap();

        let listed = list
            .execute(
                &json!({ "state": "closed" }).to_string(),
                Default::default(),
            )
            .await
            .unwrap();
        let listed_content = listed.content.unwrap();
        for secret in [
            body_secret,
            thread_secret,
            artifact_secret,
            resolution_secret,
        ] {
            assert!(!listed_content.contains(secret));
        }
        let listed_json: Value = serde_json::from_str(&listed_content).unwrap();
        let ticket = listed_json["tickets"][0].as_object().unwrap();
        for forbidden_key in [
            "body",
            "document",
            "events",
            "thread",
            "artifacts",
            "resolution",
        ] {
            assert!(!ticket.contains_key(forbidden_key));
        }
    }

    #[tokio::test]
    async fn ticket_relation_tools_record_query_and_show_derived_view() {
        let temp = TempDir::new().unwrap();
        let backend = backend(&temp);
        let source = backend.create(NewTicket::new("Relation Source")).unwrap();
        let target = backend.create(NewTicket::new("Relation Target")).unwrap();
        let record = tool_by_name(backend.clone(), "TicketRelationRecord");
        let query = tool_by_name(backend.clone(), "TicketRelationQuery");
        let show = tool_by_name(backend.clone(), "TicketShow");

        let recorded = record
            .execute(
                &json!({
                    "ticket": source.id.clone(),
                    "kind": "depends_on",
                    "target": target.id.clone(),
                    "note": "target first",
                    "author": "test"
                })
                .to_string(),
                Default::default(),
            )
            .await
            .unwrap();
        assert!(recorded.summary.contains("Recorded ticket relation"));
        let recorded_json: Value = serde_json::from_str(&recorded.content.unwrap()).unwrap();
        assert_eq!(recorded_json["kind"], "depends_on");
        assert_eq!(recorded_json["target"], target.id);

        let queried = query
            .execute(
                &json!({ "ticket": target.id.clone() }).to_string(),
                Default::default(),
            )
            .await
            .unwrap();
        let queried_json: Value = serde_json::from_str(&queried.content.unwrap()).unwrap();
        assert_eq!(queried_json["count"], 1);
        assert_eq!(queried_json["relations"][0]["ticket_id"], source.id);

        let shown = show
            .execute(
                &json!({ "id": target.id.clone() }).to_string(),
                Default::default(),
            )
            .await
            .unwrap();
        let shown_json: Value = serde_json::from_str(&shown.content.unwrap()).unwrap();
        assert_eq!(
            shown_json["relations"]["incoming"][0]["inverse_kind"],
            "dependency_of"
        );
    }

    #[tokio::test]
    async fn ticket_tools_comment_review_state_and_close_are_doctor_clean() {
        let temp = TempDir::new().unwrap();
        let backend = backend(&temp);
        let created = backend.create(NewTicket::new("Flow Tool")).unwrap();
        let comment = tool_by_name(backend.clone(), "TicketComment");
        let review = tool_by_name(backend.clone(), "TicketReview");
        let close = tool_by_name(backend.clone(), "TicketClose");
        let doctor = tool_by_name(backend.clone(), "TicketDoctor");

        comment
            .execute(
                &json!({
                    "ticket": created.id.clone(),
                    "role": "implementation_report",
                    "body": "Implemented."
                })
                .to_string(),
                Default::default(),
            )
            .await
            .unwrap();
        review
            .execute(
                &json!({
                    "ticket": created.id.clone(),
                    "result": "approve",
                    "body": "Looks good."
                })
                .to_string(),
                Default::default(),
            )
            .await
            .unwrap();
        close
            .execute(
                &json!({ "ticket": created.id, "resolution": "Done via TicketClose.\n" })
                    .to_string(),
                Default::default(),
            )
            .await
            .unwrap();

        let report = doctor
            .execute(&json!({}).to_string(), Default::default())
            .await
            .unwrap();
        assert!(report.summary.contains("0 error(s)"));
        let closed = backend.show(TicketIdOrSlug::Id(created.id)).unwrap();
        assert!(closed.resolution.is_some());
        assert!(
            closed
                .events
                .iter()
                .any(|event| event.kind == TicketEventKind::ImplementationReport)
        );
        assert!(
            closed
                .events
                .iter()
                .any(|event| event.kind == TicketEventKind::Review)
        );
        assert!(
            closed
                .events
                .iter()
                .any(|event| event.kind == TicketEventKind::StateChanged)
        );
    }

    #[tokio::test]
    async fn ticket_workflow_tools_mark_ready_and_transition_state() {
        let temp = TempDir::new().unwrap();
        let backend = backend(&temp);
        let created = backend.create(NewTicket::new("Workflow Tool")).unwrap();
        let intake_ready = tool_by_name(backend.clone(), "TicketIntakeReady");
        let workflow = tool_by_name(backend.clone(), "TicketWorkflowState");

        intake_ready
            .execute(
                &json!({
                    "ticket": created.id.clone(),
                    "intake_summary": "Requirements accepted; implementation can be queued.",
                    "author": "intake-worker"
                })
                .to_string(),
                Default::default(),
            )
            .await
            .unwrap();
        backend
            .queue_ready(TicketIdOrSlug::Id(created.id.clone()), "panel")
            .unwrap();
        workflow
            .execute(
                &json!({
                    "ticket": created.id.clone(),
                    "from": "queued",
                    "to": "inprogress",
                    "reason": "orchestrator_started",
                    "body": "Orchestrator started implementation.\n",
                    "author": "orchestrator"
                })
                .to_string(),
                Default::default(),
            )
            .await
            .unwrap();
        workflow
            .execute(
                &json!({
                    "ticket": created.id.clone(),
                    "from": "inprogress",
                    "to": "done",
                    "reason": "implementation_complete",
                    "body": "Implementation finished and is ready for close.\n",
                    "author": "orchestrator"
                })
                .to_string(),
                Default::default(),
            )
            .await
            .unwrap();

        let record = backend.show(TicketIdOrSlug::Id(created.id)).unwrap();
        assert_eq!(record.meta.workflow_state, TicketWorkflowState::Done);
        assert!(
            record
                .events
                .iter()
                .any(|event| event.kind == TicketEventKind::IntakeSummary)
        );
        let transitions = record
            .events
            .iter()
            .filter(|event| {
                event.kind == TicketEventKind::StateChanged
                    && event.state_field.as_deref() == Some("state")
            })
            .map(|event| (event.from.as_deref(), event.to.as_deref()))
            .collect::<Vec<_>>();
        assert_eq!(
            transitions,
            vec![
                (Some("planning"), Some("ready")),
                (Some("ready"), Some("queued")),
                (Some("queued"), Some("inprogress")),
                (Some("inprogress"), Some("done"))
            ]
        );
    }

    #[tokio::test]
    async fn ticket_workflow_tool_allows_return_to_planning_from_ready_and_queued() {
        let temp = TempDir::new().unwrap();
        let backend = backend(&temp);
        let workflow = tool_by_name(backend.clone(), "TicketWorkflowState");

        let mut ready_input = NewTicket::new("Ready Needs Planning");
        ready_input.workflow_state = Some(TicketWorkflowState::Ready);
        let ready = backend.create(ready_input).unwrap();
        workflow
            .execute(
                &json!({
                    "ticket": ready.id,
                    "from": "ready",
                    "to": "planning",
                    "reason": "missing_acceptance_decision",
                    "body": "Missing decision: clarify acceptance criteria before queueing.\n",
                    "author": "orchestrator"
                })
                .to_string(),
                Default::default(),
            )
            .await
            .unwrap();
        let ready_record = backend.show(TicketIdOrSlug::Id(ready.id)).unwrap();
        assert_eq!(
            ready_record.meta.workflow_state,
            TicketWorkflowState::Planning
        );
        assert!(ready_record.events.iter().any(|event| {
            event.kind == TicketEventKind::StateChanged
                && event.from.as_deref() == Some("ready")
                && event.to.as_deref() == Some("planning")
                && event.reason.as_deref() == Some("missing_acceptance_decision")
        }));

        let mut queued_input = NewTicket::new("Queued Needs Planning");
        queued_input.workflow_state = Some(TicketWorkflowState::Queued);
        let queued = backend.create(queued_input).unwrap();
        workflow
            .execute(
                &json!({
                    "ticket": queued.id,
                    "from": "queued",
                    "to": "planning",
                    "reason": "missing_authority_decision",
                    "body": "Missing decision: define authority boundary before implementation side effects.\n",
                    "author": "orchestrator"
                })
                .to_string(),
            Default::default(),
            )
            .await
            .unwrap();
        let queued_record = backend.show(TicketIdOrSlug::Id(queued.id)).unwrap();
        assert_eq!(
            queued_record.meta.workflow_state,
            TicketWorkflowState::Planning
        );
        assert!(queued_record.events.iter().any(|event| {
            event.kind == TicketEventKind::StateChanged
                && event.from.as_deref() == Some("queued")
                && event.to.as_deref() == Some("planning")
                && event.reason.as_deref() == Some("missing_authority_decision")
        }));
    }

    #[tokio::test]
    async fn ticket_workflow_tool_rejects_stale_transition_without_state_move() {
        let temp = TempDir::new().unwrap();
        let backend = backend(&temp);
        let created = backend
            .create(NewTicket::new("Stale Workflow Tool"))
            .unwrap();
        let workflow = tool_by_name(backend.clone(), "TicketWorkflowState");

        let error = workflow
            .execute(
                &json!({
                    "ticket": created.id.clone(),
                    "from": "queued",
                    "to": "inprogress",
                    "reason": "orchestrator_started",
                    "body": "Should not apply.\n"
                })
                .to_string(),
                Default::default(),
            )
            .await
            .unwrap_err();

        assert!(error.to_string().contains("state changed concurrently"));
        let record = backend.show(TicketIdOrSlug::Id(created.id)).unwrap();
        assert_eq!(record.meta.workflow_state, TicketWorkflowState::Planning);
        assert!(!record.events.iter().any(|event| {
            event.kind == TicketEventKind::StateChanged
                && event.state_field.as_deref() == Some("state")
        }));
    }

    #[tokio::test]
    async fn ticket_workflow_tool_rejects_disallowed_transition_graph_edges() {
        let temp = TempDir::new().unwrap();
        let backend = backend(&temp);
        let workflow = tool_by_name(backend.clone(), "TicketWorkflowState");

        let mut ready_input = NewTicket::new("Ready Bypass");
        ready_input.workflow_state = Some(TicketWorkflowState::Ready);
        let ready = backend.create(ready_input).unwrap();
        let ready_error = workflow
            .execute(
                &json!({
                    "ticket": ready.id,
                    "from": "ready",
                    "to": "inprogress",
                    "reason": "bypass_queue",
                    "body": "Should not bypass Queue.\n"
                })
                .to_string(),
                Default::default(),
            )
            .await
            .unwrap_err();
        assert!(ready_error.to_string().contains("not allowed"));

        let mut done_input = NewTicket::new("Backward Bypass");
        done_input.workflow_state = Some(TicketWorkflowState::Done);
        let done = backend.create(done_input).unwrap();
        let backward_error = workflow
            .execute(
                &json!({
                    "ticket": done.id,
                    "from": "done",
                    "to": "planning",
                    "reason": "backwards",
                    "body": "Should not move backwards.\n"
                })
                .to_string(),
                Default::default(),
            )
            .await
            .unwrap_err();
        assert!(backward_error.to_string().contains("not allowed"));

        let mut queued_input = NewTicket::new("Skip Bypass");
        queued_input.workflow_state = Some(TicketWorkflowState::Queued);
        let queued = backend.create(queued_input).unwrap();
        let skip_error = workflow
            .execute(
                &json!({
                    "ticket": queued.id,
                    "from": "queued",
                    "to": "done",
                    "reason": "skip_inprogress",
                    "body": "Should not skip inprogress.\n"
                })
                .to_string(),
                Default::default(),
            )
            .await
            .unwrap_err();
        assert!(skip_error.to_string().contains("not allowed"));
    }

    #[tokio::test]
    async fn ticket_intake_ready_tool_rejects_non_planning_ticket() {
        let temp = TempDir::new().unwrap();
        let backend = backend(&temp);
        let mut input = NewTicket::new("Already Ready");
        input.workflow_state = Some(TicketWorkflowState::Ready);
        let created = backend.create(input).unwrap();
        let intake_ready = tool_by_name(backend.clone(), "TicketIntakeReady");

        let error = intake_ready
            .execute(
                &json!({
                    "ticket": created.id.clone(),
                    "intake_summary": "Should not rewrite ready ticket."
                })
                .to_string(),
                Default::default(),
            )
            .await
            .unwrap_err();

        assert!(error.to_string().contains("state changed concurrently"));
        let record = backend.show(TicketIdOrSlug::Id(created.id)).unwrap();
        assert_eq!(record.meta.workflow_state, TicketWorkflowState::Ready);
        assert!(!record.events.iter().any(|event| {
            event.kind == TicketEventKind::StateChanged
                && event.state_field.as_deref() == Some("state")
        }));
    }

    #[tokio::test]
    async fn ticket_orchestration_plan_tools_record_and_query_without_state_changes() {
        let temp = TempDir::new().unwrap();
        let backend = backend(&temp);
        let first = backend.create(NewTicket::new("Plan Tool First")).unwrap();
        let second = backend.create(NewTicket::new("Plan Tool Second")).unwrap();
        let record = tool_by_name(backend.clone(), "TicketOrchestrationPlanRecord");
        let query = tool_by_name(backend.clone(), "TicketOrchestrationPlanQuery");

        let recorded = record
            .execute(
                &json!({
                    "ticket": first.id.clone(),
                    "kind": "blocked_by",
                    "related_ticket": second.id.clone(),
                    "note": "Wait for the second Ticket's API boundary decision.",
                    "author": "orchestrator"
                })
                .to_string(),
                Default::default(),
            )
            .await
            .unwrap();
        assert!(
            recorded
                .summary
                .contains("Recorded orchestration plan blocked_by")
        );

        let found = query
            .execute(
                &json!({
                    "ticket": first.id,
                    "relation_kind": "blocked_by"
                })
                .to_string(),
                Default::default(),
            )
            .await
            .unwrap();
        let found_json: Value = serde_json::from_str(&found.content.unwrap()).unwrap();
        assert_eq!(found_json["count"], 1);
        assert_eq!(found_json["records"][0]["kind"], "blocked_by");
        assert_eq!(found_json["records"][0]["related_ticket"], second.id);

        let current = backend.show(TicketIdOrSlug::Id(first.id)).unwrap();
        assert_eq!(current.meta.workflow_state, TicketWorkflowState::Planning);
    }

    #[tokio::test]
    async fn ticket_show_requires_exactly_one_identifier() {
        let temp = TempDir::new().unwrap();
        let show = tool_by_name(backend(&temp), "TicketShow");
        let error = show
            .execute(
                &json!({ "id": "a", "query": "b" }).to_string(),
                Default::default(),
            )
            .await
            .unwrap_err();
        assert!(matches!(error, ToolError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn ticket_create_uses_opaque_id_under_backend_root() {
        let temp = TempDir::new().unwrap();
        let backend = backend(&temp);
        let create = tool_by_name(backend.clone(), "TicketCreate");
        let output = create
            .execute(
                &json!({ "title": "Escape" }).to_string(),
                Default::default(),
            )
            .await
            .unwrap();
        let value: Value = serde_json::from_str(&output.content.unwrap()).unwrap();
        let id = value["id"].as_str().unwrap();
        assert!(!id.contains("escape"));
        assert!(!temp.path().join("escape").exists());
        assert!(temp.path().join("tickets").join(id).is_dir());
        assert_eq!(backend.list(crate::TicketFilter::all()).unwrap().len(), 1);
    }

    #[test]
    fn ticket_tool_definitions_have_expected_names_and_schemas() {
        let temp = TempDir::new().unwrap();
        let tools = ticket_tools(backend(&temp));
        let create_schema = tools
            .iter()
            .map(|definition| definition().0)
            .find(|meta| meta.name == "TicketCreate")
            .unwrap()
            .input_schema
            .to_string();
        assert!(!create_schema.contains("legacy_ticket"));
        assert!(!create_schema.contains("needs_preflight"));
        assert!(!create_schema.contains("action_required"));
        assert!(!create_schema.contains("attention_required"));
        let plan_record_schema = tools
            .iter()
            .map(|definition| definition().0)
            .find(|meta| meta.name == "TicketOrchestrationPlanRecord")
            .unwrap()
            .input_schema
            .to_string();
        assert!(plan_record_schema.contains("accepted_plan"));
        assert!(plan_record_schema.contains("related_ticket"));
        let plan_query_schema = tools
            .iter()
            .map(|definition| definition().0)
            .find(|meta| meta.name == "TicketOrchestrationPlanQuery")
            .unwrap()
            .input_schema
            .to_string();
        assert!(plan_query_schema.contains("relation_kind"));
        let names = tools
            .into_iter()
            .map(|definition| definition().0)
            .map(|meta| {
                assert_eq!(meta.input_schema["type"], "object");
                meta.name
            })
            .collect::<Vec<_>>();
        assert_eq!(names, TICKET_TOOL_NAMES);
    }

    #[test]
    fn individual_tool_definition_factory_is_callable() {
        let temp = TempDir::new().unwrap();
        let create = tool(tool_definition::<TicketCreateTool>(
            "TicketCreate",
            backend(&temp),
        ));
        let _ = create;
    }
}
