//! LLM tool implementations for typed Ticket backend operations.
//!
//! These tools are intentionally owned by the `ticket` crate so Worker features can
//! install Ticket behavior without reimplementing domain/backend logic or
//! granting generic filesystem write authority.

use std::sync::Arc;

use agen::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    AcceptedOrchestrationPlan, LocalTicketBackend, MarkdownText, NewOrchestrationPlanRecord,
    NewTicket, NewTicketEvent, NewTicketRelation, OrchestrationPlanKind, OrchestrationPlanRecord,
    Result as TicketResult, Ticket, TicketBackend, TicketBodyReplacement, TicketDoctorDiagnostic,
    TicketDoctorReport, TicketDoctorSeverity, TicketError, TicketEventKind, TicketIdOrSlug,
    TicketIntakeSummary, TicketListState, TicketMarkReady, TicketRef, TicketRelation,
    TicketRelationKind, TicketRelationView, TicketStateChange, TicketSummary, TicketWorkflowState,
    default_author,
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

pub const TICKET_BASE_TOOL_NAMES: [&str; 14] = [
    "TicketCreate",
    "TicketEditItem",
    "QueryTicket",
    "ShowTicket",
    "TicketComment",
    "TicketPlan",
    "TicketDecision",
    "TicketImplementationReport",
    "TicketMarkReady",
    "TicketQueue",
    "TicketWorkflowState",
    "TicketClose",
    "TicketDependencyCheck",
    "TicketDoctor",
];

pub const TICKET_BASE_READ_ONLY_TOOL_NAMES: [&str; 4] = [
    "QueryTicket",
    "ShowTicket",
    "TicketDependencyCheck",
    "TicketDoctor",
];

pub const TICKET_ORCHESTRATION_TOOL_NAMES: [&str; 5] = [
    "TicketRelationRecord",
    "TicketRelationRemove",
    "TicketRelationQuery",
    "TicketOrchestrationPlanRecord",
    "TicketOrchestrationPlanQuery",
];

pub const TICKET_ORCHESTRATION_READ_ONLY_TOOL_NAMES: [&str; 2] =
    ["TicketRelationQuery", "TicketOrchestrationPlanQuery"];

pub const TICKET_TOOL_NAMES: [&str; 20] = [
    "TicketCreate",
    "TicketEditItem",
    "QueryTicket",
    "ShowTicket",
    "TicketComment",
    "TicketPlan",
    "TicketDecision",
    "TicketImplementationReport",
    "TicketMarkReady",
    "TicketIntakeReady",
    "TicketQueue",
    "TicketWorkflowState",
    "TicketClose",
    "TicketDependencyCheck",
    "TicketDoctor",
    "TicketRelationRecord",
    "TicketRelationRemove",
    "TicketRelationQuery",
    "TicketOrchestrationPlanRecord",
    "TicketOrchestrationPlanQuery",
];

pub const TICKET_READ_ONLY_TOOL_NAMES: [&str; 6] = [
    "QueryTicket",
    "ShowTicket",
    "TicketDependencyCheck",
    "TicketDoctor",
    "TicketRelationQuery",
    "TicketOrchestrationPlanQuery",
];

pub const TICKET_MUTATING_TOOL_NAMES: [&str; 14] = [
    "TicketCreate",
    "TicketEditItem",
    "TicketComment",
    "TicketPlan",
    "TicketDecision",
    "TicketImplementationReport",
    "TicketMarkReady",
    "TicketIntakeReady",
    "TicketQueue",
    "TicketWorkflowState",
    "TicketClose",
    "TicketRelationRecord",
    "TicketRelationRemove",
    "TicketOrchestrationPlanRecord",
];

const CREATE_DESCRIPTION: &str = "Create a Ticket through the configured typed Ticket backend. \
Inputs mirror the Ticket `item.md` fields; `title` is required, `body` is Markdown, and the \
backend assigns the id and writes the local Ticket file layout under the configured backend root.";
const EDIT_ITEM_DESCRIPTION: &str = "Edit a Ticket item through the configured typed Ticket backend. \
This updates the current item title/body and appends an audited item_edit thread event. Intended for \
User/Companion authoring surfaces, not Orchestrator implementation control.";
const LIST_DESCRIPTION: &str = "Query Tickets from the configured typed Ticket backend as a bounded \
overview. The local backend supports workflow-state selection; Workspace-backed Workers replace this \
definition with the richer authoritative text/event/evidence/relation/Objective/time/attention query.";
const SHOW_DESCRIPTION: &str = "Show one Ticket by id or exact query through the configured typed \
Ticket backend. Output includes bounded Markdown body, recent thread events, resolution, and artifact \
metadata; Workspace-backed Workers replace this definition with the richer authoritative evidence projection.";
const COMMENT_DESCRIPTION: &str = "Append a typed Ticket comment event. `body` is Markdown.";
const PLAN_DESCRIPTION: &str = "Append a typed Ticket plan event. `body` is Markdown.";
const DECISION_DESCRIPTION: &str = "Append a typed Ticket decision event. `body` is Markdown.";
const IMPLEMENTATION_REPORT_DESCRIPTION: &str =
    "Append a typed Ticket implementation_report event. `body` is Markdown.";
const MARK_READY_DESCRIPTION: &str = "Mark a planning Ticket ready through the typed Ticket backend. \
The backend atomically validates and normalizes the persisted repository/ref target, records one typed \
state_changed event, and transitions planning -> ready. `reason` is optional.";
const INTAKE_READY_DESCRIPTION: &str = "Record a bounded intake summary and mark a planning Ticket ready. \
The backend applies the same target validation and lock as TicketMarkReady and commits the summary, \
state_changed event, effective target, and planning -> ready transition atomically.";
const QUEUE_DESCRIPTION: &str = "Queue a ready Ticket for Orchestrator routing through the typed \
Ticket backend. The backend rejects transitive planning dependencies and cycles, atomically queues the \
requested Ticket plus every transitive ready dependency, and leaves queued or in-progress dependencies unchanged.";
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
const RELATION_REMOVE_DESCRIPTION: &str = "Remove one exact forward typed Ticket relation identified by \
source Ticket, relation kind, and target Ticket. Use this to correct obsolete or erroneous project-level \
relation metadata; derived inverse views update automatically.";
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
const DEPENDENCY_CHECK_DESCRIPTION: &str = "Return a structured Ticket dependency / queue readiness \
check through the typed Ticket backend. This read-only guard does not queue or transition the Ticket.";

fn base_tool_description(name: &str) -> &'static str {
    match name {
        "TicketCreate" => CREATE_DESCRIPTION,
        "TicketEditItem" => EDIT_ITEM_DESCRIPTION,
        "QueryTicket" => LIST_DESCRIPTION,
        "ShowTicket" => SHOW_DESCRIPTION,
        "TicketComment" => COMMENT_DESCRIPTION,
        "TicketPlan" => PLAN_DESCRIPTION,
        "TicketDecision" => DECISION_DESCRIPTION,
        "TicketImplementationReport" => IMPLEMENTATION_REPORT_DESCRIPTION,
        "TicketMarkReady" => MARK_READY_DESCRIPTION,
        "TicketIntakeReady" => INTAKE_READY_DESCRIPTION,
        "TicketQueue" => QUEUE_DESCRIPTION,
        "TicketWorkflowState" => WORKFLOW_STATE_DESCRIPTION,
        "TicketClose" => CLOSE_DESCRIPTION,
        "TicketRelationRecord" => RELATION_RECORD_DESCRIPTION,
        "TicketRelationRemove" => RELATION_REMOVE_DESCRIPTION,
        "TicketRelationQuery" => RELATION_QUERY_DESCRIPTION,
        "TicketOrchestrationPlanRecord" => ORCHESTRATION_PLAN_RECORD_DESCRIPTION,
        "TicketOrchestrationPlanQuery" => ORCHESTRATION_PLAN_QUERY_DESCRIPTION,
        "TicketDependencyCheck" => DEPENDENCY_CHECK_DESCRIPTION,
        "TicketDoctor" => DOCTOR_DESCRIPTION,
        _ => "Ticket backend tool.",
    }
}

/// Build the model-visible Ticket tool description for a configured Ticket backend.
///
/// `record_language` is the durable Ticket record/tool-body language, distinct from
/// worker response language and Memory language. Keeping this on the tool
/// surface ensures every Ticket-capable Worker sees the policy without hidden context
/// injection or role-launch-only prose.
pub fn ticket_tool_description(name: &str, record_language: Option<&str>) -> String {
    let mut description = base_tool_description(name).to_string();
    if let Some(language) = record_language.filter(|language| !language.trim().is_empty()) {
        description.push_str("\n\nTicket record language: ");
        description.push_str(language.trim());
        description.push_str(". Use this language for durable Ticket record and Ticket tool body text, including Ticket item bodies, thread comments/plans/decisions/implementation reports, reviews, resolutions, intake summaries, and orchestration plan notes. This policy is distinct from worker.language for normal prose and memory.language for Memory. Preserve protocol literals, file paths, commands, logs, identifiers, and quoted external text when translation would reduce fidelity.");
    }
    description
}

/// Backend object used by the LLM-facing Ticket tools.
///
/// Tool execution is intentionally parameterized by this wrapper rather than by
/// `LocalTicketBackend` so Worker hosts can supply an API-backed implementation
/// without changing the model-visible tool surface.
#[derive(Clone)]
pub struct TicketToolBackend {
    backend: Arc<dyn TicketBackend + Send + Sync>,
    record_language: Option<String>,
}

impl TicketToolBackend {
    pub fn new<B>(backend: B) -> Self
    where
        B: TicketBackend + Send + Sync + 'static,
    {
        Self {
            backend: Arc::new(backend),
            record_language: None,
        }
    }

    pub fn with_record_language(mut self, language: Option<&str>) -> Self {
        self.record_language = language
            .map(str::trim)
            .filter(|language| !language.is_empty())
            .map(str::to_string);
        self
    }

    pub fn record_language(&self) -> Option<&str> {
        self.record_language.as_deref()
    }
}

impl From<LocalTicketBackend> for TicketToolBackend {
    fn from(backend: LocalTicketBackend) -> Self {
        let record_language = backend.record_language().map(str::to_string);
        Self::new(backend).with_record_language(record_language.as_deref())
    }
}

impl TicketBackend for TicketToolBackend {
    fn default_intake_ready_state_change_body(&self, from: &str) -> String {
        self.backend.default_intake_ready_state_change_body(from)
    }

    fn list(&self, filter: crate::TicketListQuery) -> TicketResult<Vec<TicketSummary>> {
        self.backend.list(filter)
    }

    fn show(&self, id: TicketIdOrSlug) -> TicketResult<Ticket> {
        self.backend.show(id)
    }

    fn create(&self, input: NewTicket) -> TicketResult<TicketRef> {
        self.backend.create(input)
    }

    fn edit_item(&self, id: TicketIdOrSlug, edit: crate::TicketItemEdit) -> TicketResult<Ticket> {
        self.backend.edit_item(id, edit)
    }

    fn dependency_check(&self, id: TicketIdOrSlug) -> TicketResult<crate::TicketDependencyCheck> {
        self.backend.dependency_check(id)
    }

    fn add_event(&self, id: TicketIdOrSlug, event: NewTicketEvent) -> TicketResult<()> {
        self.backend.add_event(id, event)
    }

    fn add_state_changed(&self, id: TicketIdOrSlug, change: TicketStateChange) -> TicketResult<()> {
        self.backend.add_state_changed(id, change)
    }

    fn add_intake_summary(
        &self,
        id: TicketIdOrSlug,
        summary: TicketIntakeSummary,
    ) -> TicketResult<()> {
        self.backend.add_intake_summary(id, summary)
    }

    fn set_state_field(
        &self,
        id: TicketIdOrSlug,
        field: &str,
        change: TicketStateChange,
    ) -> TicketResult<()> {
        self.backend.set_state_field(id, field, change)
    }

    fn set_workflow_state(
        &self,
        id: TicketIdOrSlug,
        change: TicketStateChange,
    ) -> TicketResult<()> {
        self.backend.set_workflow_state(id, change)
    }

    fn mark_ready(&self, id: TicketIdOrSlug, request: TicketMarkReady) -> TicketResult<Ticket> {
        self.backend.mark_ready(id, request)
    }

    fn queue_ready(
        &self,
        id: TicketIdOrSlug,
        queued_by: &str,
    ) -> TicketResult<crate::TicketQueueOutcome> {
        self.backend.queue_ready(id, queued_by)
    }

    fn close(&self, id: TicketIdOrSlug, resolution: MarkdownText) -> TicketResult<()> {
        self.backend.close(id, resolution)
    }

    fn add_ticket_relation(
        &self,
        id: TicketIdOrSlug,
        relation: NewTicketRelation,
    ) -> TicketResult<TicketRelation> {
        self.backend.add_ticket_relation(id, relation)
    }

    fn remove_ticket_relation(
        &self,
        id: TicketIdOrSlug,
        kind: TicketRelationKind,
        target: TicketIdOrSlug,
    ) -> TicketResult<TicketRelation> {
        self.backend.remove_ticket_relation(id, kind, target)
    }

    fn query_ticket_relations(
        &self,
        ticket: Option<TicketIdOrSlug>,
        kind: Option<TicketRelationKind>,
    ) -> TicketResult<Vec<TicketRelation>> {
        self.backend.query_ticket_relations(ticket, kind)
    }

    fn relation_view(&self, id: TicketIdOrSlug) -> TicketResult<TicketRelationView> {
        self.backend.relation_view(id)
    }

    fn add_orchestration_plan_record(
        &self,
        id: TicketIdOrSlug,
        record: NewOrchestrationPlanRecord,
    ) -> TicketResult<OrchestrationPlanRecord> {
        self.backend.add_orchestration_plan_record(id, record)
    }

    fn query_orchestration_plan_records(
        &self,
        ticket: Option<TicketIdOrSlug>,
        kind: Option<OrchestrationPlanKind>,
    ) -> TicketResult<Vec<OrchestrationPlanRecord>> {
        self.backend.query_orchestration_plan_records(ticket, kind)
    }

    fn doctor(&self) -> TicketResult<TicketDoctorReport> {
        self.backend.doctor()
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TicketCreateParams {
    /// Ticket title. Must not be empty.
    title: String,
    /// Markdown body for item.md. If omitted, a small default body is used.
    #[serde(default)]
    body: Option<String>,
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
    /// Optional queued_at frontmatter value.
    #[serde(default)]
    queued_at: Option<String>,
    /// Optional target Workspace repository id.
    #[serde(default)]
    repository_id: Option<String>,
    /// Optional target Git ref selector. Requires `repository_id`.
    #[serde(default)]
    ref_selector: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TicketEditItemParams {
    /// Ticket id.
    ticket: String,
    /// Optional replacement title.
    #[serde(default)]
    title: Option<String>,
    /// Optional replacement Markdown body. This replaces the entire item body.
    #[serde(default)]
    body: Option<String>,
    /// Exact body substring to replace. Must be provided with `new_string`; omitted for whole-body edits.
    #[serde(default)]
    old_string: Option<String>,
    /// Replacement text for `old_string`. Must be provided with `old_string`.
    #[serde(default)]
    new_string: Option<String>,
    /// Replace every occurrence of `old_string`; by default exactly one occurrence is required.
    #[serde(default)]
    replace_all: bool,
    /// Optional target repository/ref update.
    #[serde(default)]
    target: Option<crate::TicketTargetEdit>,
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

    fn into_list_state(self) -> TicketListState {
        match self {
            Self::Planning => TicketListState::Planning,
            Self::Ready => TicketListState::Ready,
            Self::Queued => TicketListState::Queued,
            Self::Inprogress => TicketListState::InProgress,
            Self::Done => TicketListState::Done,
            Self::Closed => TicketListState::Closed,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum QueryTicketStateParam {
    Active,
    Planning,
    Ready,
    Queued,
    Inprogress,
    Done,
    Closed,
    All,
}

impl QueryTicketStateParam {
    fn as_list_state(self) -> Option<TicketListState> {
        match self {
            Self::Planning => Some(TicketListState::Planning),
            Self::Ready => Some(TicketListState::Ready),
            Self::Queued => Some(TicketListState::Queued),
            Self::Inprogress => Some(TicketListState::InProgress),
            Self::Done => Some(TicketListState::Done),
            Self::Closed => Some(TicketListState::Closed),
            Self::Active | Self::All => None,
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct QueryTicketParams {
    /// State filter. Defaults to active Tickets (all non-closed states). Use `all` to include closed Tickets.
    #[serde(default)]
    state: Option<QueryTicketStateParam>,
    /// Explicit workflow-state filter list. Cannot be combined with `state`.
    #[serde(default)]
    states: Option<Vec<TicketWorkflowStateParam>>,
    /// Maximum number of summaries to return. Defaults to 50, max 100.
    #[serde(default)]
    limit: Option<usize>,
}

impl QueryTicketParams {
    fn into_query(self) -> Result<(crate::TicketListQuery, String, Option<usize>), TicketError> {
        let query = if let Some(states) = self.states {
            if self.state.is_some() {
                return Err(TicketError::Conflict(
                    "QueryTicket accepts either `state` or `states`, not both".to_string(),
                ));
            }
            if states.is_empty() {
                return Err(TicketError::Conflict(
                    "QueryTicket `states` must include at least one workflow state".to_string(),
                ));
            }
            crate::TicketListQuery::states(states.into_iter().map(|state| state.into_list_state()))
        } else {
            match self.state.unwrap_or(QueryTicketStateParam::Active) {
                QueryTicketStateParam::Active => crate::TicketListQuery::active(),
                QueryTicketStateParam::All => crate::TicketListQuery::all(),
                state => crate::TicketListQuery::state(
                    state
                        .as_list_state()
                        .expect("workflow state list param maps to QueryTicketState"),
                ),
            }
        };
        let label = query.state_filter_label();
        Ok((query, label, self.limit))
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ShowTicketParams {
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
struct TicketThreadEventParams {
    /// Ticket id.
    ticket: String,
    /// Markdown event body.
    body: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TicketMarkReadyParams {
    /// Ticket id.
    ticket: String,
    /// Optional reason attached to the state_changed event.
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TicketIntakeReadyParams {
    /// Ticket id.
    ticket: String,
    /// Concise bounded intake summary appended before the ready transition.
    intake_summary: String,
    /// Optional reason attached to the state_changed event.
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TicketQueueParams {
    /// Ticket id.
    ticket: String,
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
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TicketCloseParams {
    /// Ticket id.
    ticket: String,
    /// Markdown resolution written to resolution.md and thread.md.
    resolution: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TicketDependencyCheckParams {
    /// Ticket id.
    ticket: String,
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
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TicketRelationRemoveParams {
    /// Ticket id that owns the forward relation.
    ticket: String,
    /// Forward relation kind to remove.
    kind: TicketRelationKindParam,
    /// Target canonical Ticket id.
    target: String,
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
struct QueryTicketOutput {
    state_filter: String,
    count: usize,
    returned: usize,
    truncated: bool,
    limit: usize,
    tickets: Vec<QueryTicketTicketOutput>,
}

#[derive(Debug, Serialize)]
struct QueryTicketTicketOutput {
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
    backend: TicketToolBackend,
}

#[derive(Clone)]
struct TicketEditItemTool {
    backend: TicketToolBackend,
}

#[derive(Clone)]
struct QueryTicketTool {
    backend: TicketToolBackend,
}

#[derive(Clone)]
struct ShowTicketTool {
    backend: TicketToolBackend,
}

#[derive(Clone)]
struct TicketCommentTool {
    backend: TicketToolBackend,
}

#[derive(Clone)]
struct TicketPlanTool {
    backend: TicketToolBackend,
}

#[derive(Clone)]
struct TicketDecisionTool {
    backend: TicketToolBackend,
}

#[derive(Clone)]
struct TicketImplementationReportTool {
    backend: TicketToolBackend,
}

#[derive(Clone)]
struct TicketMarkReadyTool {
    backend: TicketToolBackend,
}

#[derive(Clone)]
struct TicketIntakeReadyTool {
    backend: TicketToolBackend,
}

#[derive(Clone)]
struct TicketQueueTool {
    backend: TicketToolBackend,
}

#[derive(Clone)]
struct TicketWorkflowStateTool {
    backend: TicketToolBackend,
}

#[derive(Clone)]
struct TicketCloseTool {
    backend: TicketToolBackend,
}

#[derive(Clone)]
struct TicketRelationRecordTool {
    backend: TicketToolBackend,
}

#[derive(Clone)]
struct TicketRelationRemoveTool {
    backend: TicketToolBackend,
}

#[derive(Clone)]
struct TicketRelationQueryTool {
    backend: TicketToolBackend,
}

#[derive(Clone)]
struct TicketOrchestrationPlanRecordTool {
    backend: TicketToolBackend,
}

#[derive(Clone)]
struct TicketOrchestrationPlanQueryTool {
    backend: TicketToolBackend,
}

#[derive(Clone)]
struct TicketDoctorTool {
    backend: TicketToolBackend,
}

#[derive(Clone)]
struct TicketDependencyCheckTool {
    backend: TicketToolBackend,
}

#[async_trait]
impl Tool for TicketCreateTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: TicketCreateParams = parse_input("TicketCreate", input_json)?;
        if params
            .state
            .is_some_and(|state| !matches!(state.into_state(), TicketWorkflowState::Planning))
        {
            return Err(backend_error(
                "TicketCreate",
                TicketError::InvalidWorkflowTransition {
                    from: "creation".to_owned(),
                    to: params
                        .state
                        .expect("checked non-planning state")
                        .into_state()
                        .as_str()
                        .to_owned(),
                },
            ));
        }
        let mut input = NewTicket::new(params.title);
        if let Some(body) = params.body {
            input.body = MarkdownText::new(body);
        }
        input.author = None;
        input.assignee = params.assignee;
        input.readiness = params.readiness;
        input.risk_flags = params.risk_flags;
        input.workflow_state = params.state.map(TicketWorkflowStateParam::into_state);
        input.queued_by = None;
        input.queued_at = params.queued_at;
        input.repository_id = params.repository_id;
        input.ref_selector = params.ref_selector;

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
impl Tool for TicketEditItemTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: TicketEditItemParams = parse_input("TicketEditItem", input_json)?;
        let body_replacement = match (params.old_string, params.new_string) {
            (Some(old_string), Some(new_string)) => Some(TicketBodyReplacement {
                old_string,
                new_string,
                replace_all: params.replace_all,
            }),
            (None, None) => None,
            _ => {
                return Err(backend_error(
                    "TicketEditItem",
                    TicketError::Conflict(
                        "old_string and new_string must be provided together".to_string(),
                    ),
                ));
            }
        };
        let edit = crate::TicketItemEdit {
            title: params.title,
            body: params.body.map(MarkdownText::new),
            body_replacement,
            target: params.target,
            author: None,
        };
        let ticket = self
            .backend
            .edit_item(TicketIdOrSlug::from(params.ticket), edit)
            .map_err(|error| backend_error("TicketEditItem", error))?;
        Ok(json_output(
            format!("Edited ticket {}", ticket.meta.id),
            ticket_json(
                &ticket,
                DEFAULT_EVENT_LIMIT,
                DEFAULT_ARTIFACT_LIMIT,
                16 * 1024,
            ),
        ))
    }
}

#[async_trait]
impl Tool for QueryTicketTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: QueryTicketParams = parse_input("QueryTicket", input_json)?;
        let (filter, state_filter, params_limit) = params
            .into_query()
            .map_err(|error| backend_error("QueryTicket", error))?;
        let limit = bounded(params_limit, DEFAULT_LIST_LIMIT, MAX_LIST_LIMIT);
        let tickets = self
            .backend
            .list(filter)
            .map_err(|error| backend_error("QueryTicket", error))?;
        let count = tickets.len();
        let returned_tickets: Vec<_> = tickets
            .into_iter()
            .take(limit)
            .map(ticket_summary_json)
            .collect();
        let output = QueryTicketOutput {
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
impl Tool for ShowTicketTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: ShowTicketParams = parse_input("ShowTicket", input_json)?;
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
            .map_err(|error| backend_error("ShowTicket", error))?;
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

fn execute_ticket_thread_event(
    backend: &TicketToolBackend,
    tool_name: &str,
    kind: TicketEventKind,
    input_json: &str,
) -> Result<ToolOutput, ToolError> {
    let params: TicketThreadEventParams = parse_input(tool_name, input_json)?;
    let role = kind.as_str().to_string();
    backend
        .add_event(
            TicketIdOrSlug::Query(params.ticket.clone()),
            NewTicketEvent::new(kind, params.body),
        )
        .map_err(|error| backend_error(tool_name, error))?;
    Ok(json_output(
        format!("Appended {role} event to ticket {}", params.ticket),
        json!({ "ticket": params.ticket, "event": role, "ok": true }),
    ))
}

#[async_trait]
impl Tool for TicketCommentTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        execute_ticket_thread_event(
            &self.backend,
            "TicketComment",
            TicketEventKind::Comment,
            input_json,
        )
    }
}

macro_rules! impl_ticket_thread_event_tool {
    ($tool:ty, $name:literal, $kind:expr) => {
        #[async_trait]
        impl Tool for $tool {
            async fn execute(
                &self,
                input_json: &str,
                _ctx: agen::tool::ToolExecutionContext,
            ) -> Result<ToolOutput, ToolError> {
                execute_ticket_thread_event(&self.backend, $name, $kind, input_json)
            }
        }
    };
}

impl_ticket_thread_event_tool!(TicketPlanTool, "TicketPlan", TicketEventKind::Plan);
impl_ticket_thread_event_tool!(
    TicketDecisionTool,
    "TicketDecision",
    TicketEventKind::Decision
);
impl_ticket_thread_event_tool!(
    TicketImplementationReportTool,
    "TicketImplementationReport",
    TicketEventKind::ImplementationReport
);

#[async_trait]
impl Tool for TicketMarkReadyTool {
    async fn execute(
        &self,
        input_json: &str,
        ctx: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: TicketMarkReadyParams = parse_input("TicketMarkReady", input_json)?;
        let ticket = self
            .backend
            .mark_ready(
                TicketIdOrSlug::Query(params.ticket.clone()),
                TicketMarkReady {
                    operation_key: format!("ticket-mark-ready:{}", ctx.call_id),
                    reason: params.reason,
                    author: None,
                    intake_summary: None,
                },
            )
            .map_err(|error| backend_error("TicketMarkReady", error))?;
        Ok(json_output(
            format!("Marked ticket {} state ready", params.ticket),
            json!({
                "ticket": ticket.meta.id,
                "state": ticket.meta.workflow_state.as_str(),
                "repository_id": ticket.meta.repository_id,
                "ref_selector": ticket.meta.ref_selector,
                "ok": true
            }),
        ))
    }
}

#[async_trait]
impl Tool for TicketIntakeReadyTool {
    async fn execute(
        &self,
        input_json: &str,
        ctx: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: TicketIntakeReadyParams = parse_input("TicketIntakeReady", input_json)?;
        let ticket = self
            .backend
            .mark_ready(
                TicketIdOrSlug::Query(params.ticket.clone()),
                TicketMarkReady {
                    operation_key: format!("ticket-intake-ready:{}", ctx.call_id),
                    reason: params.reason,
                    author: None,
                    intake_summary: Some(TicketIntakeSummary::new(params.intake_summary)),
                },
            )
            .map_err(|error| backend_error("TicketIntakeReady", error))?;
        Ok(json_output(
            format!("Marked ticket {} state ready after intake", params.ticket),
            json!({
                "ticket": ticket.meta.id,
                "state": ticket.meta.workflow_state.as_str(),
                "repository_id": ticket.meta.repository_id,
                "ref_selector": ticket.meta.ref_selector,
                "ok": true
            }),
        ))
    }
}

#[async_trait]
impl Tool for TicketQueueTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: TicketQueueParams = parse_input("TicketQueue", input_json)?;
        let queued_by = default_author();
        let outcome = self
            .backend
            .queue_ready(TicketIdOrSlug::Query(params.ticket.clone()), &queued_by)
            .map_err(|error| backend_error("TicketQueue", error))?;
        Ok(json_output(
            format!(
                "Queued {} ticket(s) for Orchestrator",
                outcome.queued_tickets.len()
            ),
            json!({
                "ticket": outcome.requested_ticket,
                "queued_tickets": outcome.queued_tickets,
                "state": "queued",
                "queued_by": queued_by,
                "ok": true
            }),
        ))
    }
}

#[async_trait]
impl Tool for TicketWorkflowStateTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: agen::tool::ToolExecutionContext,
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
        change.author = None;
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
        _ctx: agen::tool::ToolExecutionContext,
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
        _ctx: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: TicketRelationRecordParams = parse_input("TicketRelationRecord", input_json)?;
        let relation = NewTicketRelation {
            kind: params.kind.into_kind(),
            target: params.target.clone(),
            note: params.note,
            author: None,
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
impl Tool for TicketRelationRemoveTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: TicketRelationRemoveParams = parse_input("TicketRelationRemove", input_json)?;
        let output = self
            .backend
            .remove_ticket_relation(
                TicketIdOrSlug::Id(params.ticket),
                params.kind.into_kind(),
                TicketIdOrSlug::Id(params.target),
            )
            .map_err(|error| backend_error("TicketRelationRemove", error))?;
        Ok(json_output(
            format!(
                "Removed ticket relation {} {} {}",
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
        _ctx: agen::tool::ToolExecutionContext,
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
        _ctx: agen::tool::ToolExecutionContext,
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
            author: None,
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
        _ctx: agen::tool::ToolExecutionContext,
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
        _ctx: agen::tool::ToolExecutionContext,
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

#[async_trait]
impl Tool for TicketDependencyCheckTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: TicketDependencyCheckParams = parse_input("TicketDependencyCheck", input_json)?;
        let check = self
            .backend
            .dependency_check(TicketIdOrSlug::Query(params.ticket.clone()))
            .map_err(|error| backend_error("TicketDependencyCheck", error))?;
        Ok(json_output(
            format!(
                "Ticket {} dependency check: {}",
                params.ticket,
                if check.queue_guard.can_queue_for_orchestrator {
                    "queueable"
                } else {
                    "not queueable"
                }
            ),
            check,
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

fn ticket_summary_json(ticket: TicketSummary) -> QueryTicketTicketOutput {
    let hints = ticket_list_hints(&ticket);
    QueryTicketTicketOutput {
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
        attachments: Vec::new(),
    }
}

fn tool_definition<T>(name: &'static str, backend: TicketToolBackend) -> ToolDefinition
where
    T: Tool + From<TicketToolBackend> + 'static,
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
        "TicketEditItem" => serde_json::to_value(schemars::schema_for!(TicketEditItemParams)),
        "QueryTicket" => serde_json::to_value(schemars::schema_for!(QueryTicketParams)),
        "ShowTicket" => serde_json::to_value(schemars::schema_for!(ShowTicketParams)),
        "TicketComment" | "TicketPlan" | "TicketDecision" | "TicketImplementationReport" => {
            serde_json::to_value(schemars::schema_for!(TicketThreadEventParams))
        }
        "TicketMarkReady" => serde_json::to_value(schemars::schema_for!(TicketMarkReadyParams)),
        "TicketIntakeReady" => serde_json::to_value(schemars::schema_for!(TicketIntakeReadyParams)),
        "TicketQueue" => serde_json::to_value(schemars::schema_for!(TicketQueueParams)),
        "TicketWorkflowState" => {
            serde_json::to_value(schemars::schema_for!(TicketWorkflowStateParams))
        }
        "TicketClose" => serde_json::to_value(schemars::schema_for!(TicketCloseParams)),
        "TicketDependencyCheck" => {
            serde_json::to_value(schemars::schema_for!(TicketDependencyCheckParams))
        }
        "TicketRelationRecord" => {
            serde_json::to_value(schemars::schema_for!(TicketRelationRecordParams))
        }
        "TicketRelationRemove" => {
            serde_json::to_value(schemars::schema_for!(TicketRelationRemoveParams))
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
        impl From<TicketToolBackend> for $tool {
            fn from(backend: TicketToolBackend) -> Self {
                Self { backend }
            }
        }
    };
}

impl_from_backend!(TicketCreateTool);
impl_from_backend!(TicketEditItemTool);
impl_from_backend!(QueryTicketTool);
impl_from_backend!(ShowTicketTool);
impl_from_backend!(TicketCommentTool);
impl_from_backend!(TicketPlanTool);
impl_from_backend!(TicketDecisionTool);
impl_from_backend!(TicketImplementationReportTool);
impl_from_backend!(TicketMarkReadyTool);
impl_from_backend!(TicketIntakeReadyTool);
impl_from_backend!(TicketQueueTool);
impl_from_backend!(TicketWorkflowStateTool);
impl_from_backend!(TicketCloseTool);
impl_from_backend!(TicketRelationRecordTool);
impl_from_backend!(TicketRelationRemoveTool);
impl_from_backend!(TicketRelationQueryTool);
impl_from_backend!(TicketOrchestrationPlanRecordTool);
impl_from_backend!(TicketOrchestrationPlanQueryTool);
impl_from_backend!(TicketDoctorTool);
impl_from_backend!(TicketDependencyCheckTool);

/// Build all MVP Ticket tool definitions over the supplied backend.
pub fn ticket_tools(backend: impl Into<TicketToolBackend>) -> Vec<ToolDefinition> {
    let backend = backend.into();
    vec![
        tool_definition::<TicketCreateTool>("TicketCreate", backend.clone()),
        tool_definition::<TicketEditItemTool>("TicketEditItem", backend.clone()),
        tool_definition::<QueryTicketTool>("QueryTicket", backend.clone()),
        tool_definition::<ShowTicketTool>("ShowTicket", backend.clone()),
        tool_definition::<TicketCommentTool>("TicketComment", backend.clone()),
        tool_definition::<TicketPlanTool>("TicketPlan", backend.clone()),
        tool_definition::<TicketDecisionTool>("TicketDecision", backend.clone()),
        tool_definition::<TicketImplementationReportTool>(
            "TicketImplementationReport",
            backend.clone(),
        ),
        tool_definition::<TicketMarkReadyTool>("TicketMarkReady", backend.clone()),
        tool_definition::<TicketIntakeReadyTool>("TicketIntakeReady", backend.clone()),
        tool_definition::<TicketQueueTool>("TicketQueue", backend.clone()),
        tool_definition::<TicketWorkflowStateTool>("TicketWorkflowState", backend.clone()),
        tool_definition::<TicketCloseTool>("TicketClose", backend.clone()),
        tool_definition::<TicketDependencyCheckTool>("TicketDependencyCheck", backend.clone()),
        tool_definition::<TicketDoctorTool>("TicketDoctor", backend.clone()),
        tool_definition::<TicketRelationRecordTool>("TicketRelationRecord", backend.clone()),
        tool_definition::<TicketRelationRemoveTool>("TicketRelationRemove", backend.clone()),
        tool_definition::<TicketRelationQueryTool>("TicketRelationQuery", backend.clone()),
        tool_definition::<TicketOrchestrationPlanRecordTool>(
            "TicketOrchestrationPlanRecord",
            backend.clone(),
        ),
        tool_definition::<TicketOrchestrationPlanQueryTool>(
            "TicketOrchestrationPlanQuery",
            backend.clone(),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[derive(Debug)]
    struct TestTargetAuthority;

    impl crate::TicketTargetAuthority for TestTargetAuthority {
        fn resolve_target(
            &self,
            _workspace_id: &str,
            repository_id: Option<&str>,
            ref_selector: Option<&str>,
        ) -> crate::Result<crate::ResolvedTicketTarget> {
            Ok(crate::ResolvedTicketTarget {
                repository_id: repository_id.unwrap_or("main").to_owned(),
                ref_selector: ref_selector.unwrap_or("develop").to_owned(),
            })
        }
    }

    fn backend(temp: &TempDir) -> LocalTicketBackend {
        LocalTicketBackend::new(temp.path().join("tickets"))
            .with_target_authority(Arc::new(TestTargetAuthority))
    }

    fn tool(definition: ToolDefinition) -> Arc<dyn Tool> {
        let (_, tool) = definition();
        tool
    }

    fn tool_by_name(backend: impl Into<TicketToolBackend>, name: &str) -> Arc<dyn Tool> {
        ticket_tools(backend)
            .into_iter()
            .find_map(|definition| {
                let (meta, tool) = definition();
                (meta.name == name).then_some(tool)
            })
            .expect("tool exists")
    }

    fn tool_description_by_name(backend: impl Into<TicketToolBackend>, name: &str) -> String {
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
                "QueryTicket",
                "ShowTicket",
                "TicketDependencyCheck",
                "TicketDoctor",
                "TicketRelationQuery",
                "TicketOrchestrationPlanQuery"
            ]
        );
        assert_eq!(
            TICKET_MUTATING_TOOL_NAMES,
            [
                "TicketCreate",
                "TicketEditItem",
                "TicketComment",
                "TicketPlan",
                "TicketDecision",
                "TicketImplementationReport",
                "TicketMarkReady",
                "TicketIntakeReady",
                "TicketQueue",
                "TicketWorkflowState",
                "TicketClose",
                "TicketRelationRecord",
                "TicketRelationRemove",
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
        let list = tool_by_name(backend.clone(), "QueryTicket");
        let show = tool_by_name(backend.clone(), "ShowTicket");
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
        let list = tool_by_name(backend.clone(), "QueryTicket");
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
        let list = tool_by_name(backend.clone(), "QueryTicket");
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
        let list = tool_by_name(backend.clone(), "QueryTicket");
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

        let active = list
            .execute(&json!({}).to_string(), Default::default())
            .await
            .unwrap();
        let active_json: Value = serde_json::from_str(&active.content.unwrap()).unwrap();
        assert_eq!(active_json["state_filter"], "active");
        assert_eq!(active_json["count"].as_u64(), Some(3));
        assert_eq!(active_json["returned"].as_u64(), Some(3));
        assert_eq!(active_json["truncated"].as_bool(), Some(false));

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
    async fn ticket_list_tool_accepts_multi_state_list_and_rejects_mixed_filters() {
        let temp = TempDir::new().unwrap();
        let backend = backend(&temp);
        let list = tool_by_name(backend.clone(), "QueryTicket");
        let planning = backend.create(NewTicket::new("Planning Ticket")).unwrap();
        let mut ready_input = NewTicket::new("Ready Ticket");
        ready_input.workflow_state = Some(TicketWorkflowState::Ready);
        let ready = backend.create(ready_input).unwrap();
        let mut closed_input = NewTicket::new("Closed Ticket");
        closed_input.workflow_state = Some(TicketWorkflowState::Closed);
        let closed = backend.create(closed_input).unwrap();

        let listed = list
            .execute(
                &json!({ "states": ["planning", "closed"] }).to_string(),
                Default::default(),
            )
            .await
            .unwrap();
        let listed_json: Value = serde_json::from_str(&listed.content.unwrap()).unwrap();
        assert_eq!(listed_json["state_filter"], "planning,closed");
        assert_eq!(listed_json["count"].as_u64(), Some(2));
        let listed_ids = listed_json["tickets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|ticket| ticket["id"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(listed_ids.contains(&planning.id.as_str()));
        assert!(listed_ids.contains(&closed.id.as_str()));
        assert!(!listed_ids.contains(&ready.id.as_str()));

        let mixed = list
            .execute(
                &json!({ "state": "active", "states": ["planning"] }).to_string(),
                Default::default(),
            )
            .await;
        assert!(mixed.is_err());

        let empty = list
            .execute(&json!({ "states": [] }).to_string(), Default::default())
            .await;
        assert!(empty.is_err());
    }

    #[tokio::test]
    async fn ticket_list_tool_omits_body_thread_artifact_and_resolution_content() {
        let temp = TempDir::new().unwrap();
        let backend = backend(&temp);
        let list = tool_by_name(backend.clone(), "QueryTicket");
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
    async fn ticket_edit_item_tool_supports_exact_body_replacement() {
        let temp = TempDir::new().unwrap();
        let backend = backend(&temp);
        let mut input = NewTicket::new("Tool Body Edit");
        input.body = MarkdownText::new("one\ntwo\none\n");
        let created = backend.create(input).unwrap();
        let edit = tool_by_name(backend.clone(), "TicketEditItem");

        let output = edit
            .execute(
                &json!({
                    "ticket": created.id.clone(),
                    "old_string": "one",
                    "new_string": "ONE",
                    "replace_all": true,
                    "author": "tool-test"
                })
                .to_string(),
                Default::default(),
            )
            .await
            .unwrap();
        let value: Value = serde_json::from_str(&output.content.unwrap()).unwrap();
        assert_eq!(
            value["body"].as_str().unwrap().trim_start_matches('\n'),
            "ONE\ntwo\nONE\n"
        );

        let record = backend
            .show(TicketIdOrSlug::Id(created.id.clone()))
            .unwrap();
        let event = record
            .events
            .iter()
            .rev()
            .find(|event| event.kind == TicketEventKind::Other("item_edit".to_string()))
            .expect("item_edit event");
        assert_eq!(
            event.attributes.get("body_edit"),
            Some(&"partial".to_string())
        );
        assert_eq!(
            event.attributes.get("replacement_count"),
            Some(&"2".to_string())
        );

        let error = edit
            .execute(
                &json!({
                    "ticket": created.id,
                    "old_string": "ONE"
                })
                .to_string(),
                Default::default(),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("old_string and new_string"));
    }

    #[tokio::test]
    async fn ticket_relation_tools_record_query_remove_and_show_derived_view() {
        let temp = TempDir::new().unwrap();
        let backend = backend(&temp);
        let source = backend.create(NewTicket::new("Relation Source")).unwrap();
        let target = backend.create(NewTicket::new("Relation Target")).unwrap();
        let record = tool_by_name(backend.clone(), "TicketRelationRecord");
        let remove = tool_by_name(backend.clone(), "TicketRelationRemove");
        let query = tool_by_name(backend.clone(), "TicketRelationQuery");
        let show = tool_by_name(backend.clone(), "ShowTicket");

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

        let removed = remove
            .execute(
                &json!({
                    "ticket": source.id.clone(),
                    "kind": "depends_on",
                    "target": target.id.clone()
                })
                .to_string(),
                Default::default(),
            )
            .await
            .unwrap();
        assert!(removed.summary.contains("Removed ticket relation"));
        let removed_json: Value = serde_json::from_str(&removed.content.unwrap()).unwrap();
        assert_eq!(removed_json["kind"], "depends_on");
        assert_eq!(removed_json["target"], target.id);

        let queried_after_remove = query
            .execute(
                &json!({ "ticket": source.id }).to_string(),
                Default::default(),
            )
            .await
            .unwrap();
        let queried_after_remove_json: Value =
            serde_json::from_str(&queried_after_remove.content.unwrap()).unwrap();
        assert_eq!(queried_after_remove_json["count"], 0);

        let shown_after_remove = show
            .execute(&json!({ "id": target.id }).to_string(), Default::default())
            .await
            .unwrap();
        let shown_after_remove_json: Value =
            serde_json::from_str(&shown_after_remove.content.unwrap()).unwrap();
        assert_eq!(shown_after_remove_json["relations"]["incoming"], json!([]));
    }

    #[tokio::test]
    async fn ticket_tools_report_state_and_close_are_doctor_clean() {
        let temp = TempDir::new().unwrap();
        let backend = backend(&temp);
        let created = backend.create(NewTicket::new("Flow Tool")).unwrap();
        let report = tool_by_name(backend.clone(), "TicketImplementationReport");
        let close = tool_by_name(backend.clone(), "TicketClose");
        let doctor = tool_by_name(backend.clone(), "TicketDoctor");

        report
            .execute(
                &json!({
                    "ticket": created.id.clone(),
                    "body": "Implemented."
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
                .any(|event| event.kind == TicketEventKind::StateChanged)
        );
    }

    #[tokio::test]
    async fn ticket_workflow_tools_mark_ready_and_transition_state() {
        let temp = TempDir::new().unwrap();
        let backend = backend(&temp);
        let mut input = NewTicket::new("Workflow Tool");
        input.repository_id = Some("main".to_owned());
        let created = backend.create(input).unwrap();
        let intake_ready = tool_by_name(backend.clone(), "TicketMarkReady");
        let workflow = tool_by_name(backend.clone(), "TicketWorkflowState");

        intake_ready
            .execute(
                &json!({
                    "ticket": created.id.clone(),
                    "reason": "requirements accepted"
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
        assert!(record.events.iter().any(|event| {
            event.kind == TicketEventKind::StateChanged
                && event.from.as_deref() == Some("planning")
                && event.to.as_deref() == Some("ready")
                && event.attributes.contains_key("request_fingerprint")
        }));
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
    async fn ticket_intake_ready_records_summary_with_validated_target() {
        let temp = TempDir::new().unwrap();
        let backend = backend(&temp);
        let mut input = NewTicket::new("Intake Workflow");
        input.repository_id = Some("main".to_owned());
        let created = backend.create(input).unwrap();
        tool_by_name(backend.clone(), "TicketIntakeReady")
            .execute(
                &json!({
                    "ticket": created.id.clone(),
                    "intake_summary": "Requirements and target are accepted.",
                    "reason": "intake_complete"
                })
                .to_string(),
                Default::default(),
            )
            .await
            .unwrap();
        let record = backend.show(TicketIdOrSlug::Id(created.id)).unwrap();
        assert_eq!(record.meta.workflow_state, TicketWorkflowState::Ready);
        assert_eq!(record.meta.ref_selector.as_deref(), Some("develop"));
        assert_eq!(
            record
                .events
                .iter()
                .filter(|event| event.kind == TicketEventKind::IntakeSummary)
                .count(),
            1
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
        assert!(
            ready_error
                .to_string()
                .contains("invalid ticket workflow transition")
        );

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
        assert!(
            backward_error
                .to_string()
                .contains("invalid ticket workflow transition")
        );

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
        assert!(
            skip_error
                .to_string()
                .contains("invalid ticket workflow transition")
        );
    }

    #[tokio::test]
    async fn ticket_mark_ready_tool_rejects_non_planning_ticket() {
        let temp = TempDir::new().unwrap();
        let backend = backend(&temp);
        let mut input = NewTicket::new("Already Ready");
        input.workflow_state = Some(TicketWorkflowState::Ready);
        let created = backend.create(input).unwrap();
        let intake_ready = tool_by_name(backend.clone(), "TicketMarkReady");

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

        assert!(error.to_string().contains("stale ticket workflow state"));
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
        let show = tool_by_name(backend(&temp), "ShowTicket");
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
        assert_eq!(
            backend.list(crate::TicketListQuery::all()).unwrap().len(),
            1
        );
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
        let edit_schema = tools
            .iter()
            .map(|definition| definition().0)
            .find(|meta| meta.name == "TicketEditItem")
            .unwrap()
            .input_schema
            .to_string();
        assert!(edit_schema.contains("old_string"));
        assert!(edit_schema.contains("new_string"));
        assert!(edit_schema.contains("replace_all"));
        for name in [
            "TicketCreate",
            "TicketEditItem",
            "TicketComment",
            "TicketPlan",
            "TicketDecision",
            "TicketImplementationReport",
            "TicketMarkReady",
            "TicketQueue",
            "TicketRelationRecord",
            "TicketOrchestrationPlanRecord",
        ] {
            let schema = tools
                .iter()
                .map(|definition| definition().0)
                .find(|meta| meta.name == name)
                .unwrap()
                .input_schema;
            let properties = schema["properties"].as_object().unwrap();
            assert!(!properties.contains_key("author"), "{name} exposes author");
            assert!(
                !properties.contains_key("queued_by"),
                "{name} exposes queued_by"
            );
            if matches!(
                name,
                "TicketComment" | "TicketPlan" | "TicketDecision" | "TicketImplementationReport"
            ) {
                assert!(!properties.contains_key("role"), "{name} exposes role");
            }
        }
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
            backend(&temp).into(),
        ));
        let _ = create;
    }
}
