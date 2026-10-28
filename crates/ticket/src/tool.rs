//! LLM tool implementations for typed Ticket backend operations.
//!
//! These tools are intentionally owned by the `ticket` crate so Worker features can
//! install Ticket behavior without reimplementing domain/backend logic or
//! granting generic filesystem write authority. Publication/read access is not mutation
//! authorization: the scoped Workspace backend authenticates and fences each operation.
//! Case judgment, classification, and self-reported handling are independent; none attests
//! MR approval or Worker execution. After stale or unknown outcomes, reread authority;
//! exact retries retain the full original input and key rather than silently rekeying.

use std::sync::Arc;

use agen::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    AcceptedOrchestrationPlan, MarkdownText, NewOrchestrationPlanRecord, NewTicket, NewTicketEvent,
    NewTicketRelation, OrchestrationPlanKind, OrchestrationPlanRecord, Result as TicketResult,
    Ticket, TicketBackend, TicketBodyReplacement, TicketDoctorDiagnostic, TicketDoctorReport,
    TicketDoctorSeverity, TicketError, TicketEventKind, TicketIdOrSlug, TicketIntakeSummary,
    TicketListState, TicketMarkReady, TicketRef, TicketRelation, TicketRelationKind,
    TicketRelationView, TicketState, TicketStateChange, TicketSummary, default_author,
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
    "TicketEditPost",
    "QueryTicket",
    "ShowTicket",
    "TicketComment",
    "TicketPlan",
    "TicketDecision",
    "TicketImplementationReport",
    "TicketMarkReady",
    "TicketQueue",
    "TicketCaseUpdate",
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
    "TicketEditPost",
    "QueryTicket",
    "ShowTicket",
    "TicketComment",
    "TicketPlan",
    "TicketDecision",
    "TicketImplementationReport",
    "TicketMarkReady",
    "TicketIntakeReady",
    "TicketQueue",
    "TicketCaseUpdate",
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
    "TicketEditPost",
    "TicketComment",
    "TicketPlan",
    "TicketDecision",
    "TicketImplementationReport",
    "TicketMarkReady",
    "TicketIntakeReady",
    "TicketQueue",
    "TicketCaseUpdate",
    "TicketRelationRecord",
    "TicketRelationRemove",
    "TicketOrchestrationPlanRecord",
];

const CREATE_DESCRIPTION: &str = "Create a Ticket through the configured typed Ticket backend. \
`title` is required, `body` is Markdown, and the backend assigns the id.";
const EDIT_ITEM_DESCRIPTION: &str = "Edit a Ticket item through the configured typed Ticket backend. \
This updates the current item title and/or initial comment body without adding a discussion entry. Intended for \
User/Companion authoring surfaces, not Orchestrator implementation control.";
const EDIT_POST_DESCRIPTION: &str = "Edit an initial or discussion comment by stable event_ref and exact expected_body. \
Preserves original author and time and records edit metadata without adding a discussion entry. \
System events and case judgments cannot be edited. The Workspace backend supplies the authenticated editor.";
const LIST_DESCRIPTION: &str = "Query Tickets from the configured typed Ticket backend as a bounded \
overview. Workspace-backed Workers replace this definition with the richer authoritative \
text/event/evidence/relation/Objective/time/attention query.";
const SHOW_DESCRIPTION: &str = "Show one Ticket by id or exact query through the configured typed \
Ticket backend. Output includes the initial comment and recent events with stable event_ref, author/time and edit metadata, bounded Markdown body, resolution, and artifact \
metadata; Workspace-backed Workers replace this definition with the richer authoritative evidence projection.";
const COMMENT_DESCRIPTION: &str = "Append a typed Ticket comment event. `body` is Markdown.";
const PLAN_DESCRIPTION: &str = "Append a typed Ticket plan event. `body` is Markdown.";
const DECISION_DESCRIPTION: &str = "Append a typed Ticket decision event. `body` is Markdown.";
const IMPLEMENTATION_REPORT_DESCRIPTION: &str =
    "Append a typed Ticket implementation_report event. `body` is Markdown.";
const MARK_READY_DESCRIPTION: &str = "Mark an open Ticket ready for routing through the typed Ticket backend. \
Targets are optional; provided targets are validated against the Workspace catalog without Git resolution. \
Records one typed state_changed event and leaves the case open. `reason` is optional.";
const INTAKE_READY_DESCRIPTION: &str = "Record a bounded intake summary and mark an open Ticket ready for routing. \
The backend applies the same optional catalog targets validation as TicketMarkReady and commits the summary, \
state_changed event, effective targets, and routing readiness update atomically.";
const QUEUE_DESCRIPTION: &str = "Queue only the explicitly requested Ticket for Orchestrator routing through the typed \
Ticket backend, when the case is open, regardless of dependencies. Dependency diagnostics are retained; no dependencies are auto-queued.";
const CASE_UPDATE_DESCRIPTION: &str = "Close, reopen or correct a Ticket closure judgment with typed reasons and content/state/observed-closure-key CAS. Exact input replay does not duplicate history. No Repository/MR prerequisite. Other reasons must be nonblank, NUL-free and at most 1024 UTF-8 bytes. Reopening never revives released work identities.";
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
        "TicketEditPost" => EDIT_POST_DESCRIPTION,
        "QueryTicket" => LIST_DESCRIPTION,
        "ShowTicket" => SHOW_DESCRIPTION,
        "TicketComment" => COMMENT_DESCRIPTION,
        "TicketPlan" => PLAN_DESCRIPTION,
        "TicketDecision" => DECISION_DESCRIPTION,
        "TicketImplementationReport" => IMPLEMENTATION_REPORT_DESCRIPTION,
        "TicketMarkReady" => MARK_READY_DESCRIPTION,
        "TicketIntakeReady" => INTAKE_READY_DESCRIPTION,
        "TicketQueue" => QUEUE_DESCRIPTION,
        "TicketCaseUpdate" => CASE_UPDATE_DESCRIPTION,
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
/// `TicketBackend` so Worker hosts can supply an API-backed implementation
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

impl From<crate::SqliteTicketBackend> for TicketToolBackend {
    fn from(backend: crate::SqliteTicketBackend) -> Self {
        let record_language = backend.record_language().map(str::to_string);
        Self::new(backend).with_record_language(record_language.as_deref())
    }
}

impl TicketBackend for TicketToolBackend {
    fn edit_post_with_snapshots(
        &self,
        id: TicketIdOrSlug,
        edit: crate::TicketPostEdit,
    ) -> TicketResult<(Ticket, Ticket)> {
        self.backend.edit_post_with_snapshots(id, edit)
    }

    fn list_workspace_tags(
        &self,
        request: crate::TagListRequest,
    ) -> TicketResult<Vec<crate::WorkspaceTag>> {
        self.backend.list_workspace_tags(request)
    }

    fn mutate_workspace_tag(
        &self,
        request: crate::WorkspaceTagMutation,
    ) -> TicketResult<crate::WorkspaceTag> {
        self.backend.mutate_workspace_tag(request)
    }

    fn set_ticket_tags(
        &self,
        id: TicketIdOrSlug,
        request: crate::TicketTagSetRequest,
    ) -> TicketResult<Vec<crate::WorkspaceTag>> {
        self.backend.set_ticket_tags(id, request)
    }

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

    fn create_with_snapshot(&self, input: NewTicket) -> TicketResult<(TicketRef, Ticket)> {
        self.backend.create_with_snapshot(input)
    }

    fn edit_item(&self, id: TicketIdOrSlug, edit: crate::TicketItemEdit) -> TicketResult<Ticket> {
        self.backend.edit_item(id, edit)
    }

    fn edit_item_with_snapshots(
        &self,
        id: TicketIdOrSlug,
        edit: crate::TicketItemEdit,
    ) -> TicketResult<(Ticket, Ticket)> {
        self.backend.edit_item_with_snapshots(id, edit)
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

    fn update_case(&self, ticket: &str, request: crate::TicketCaseUpdate) -> TicketResult<Ticket> {
        self.backend.update_case(ticket, request)
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
    /// Optional state frontmatter value. Defaults to `open`.
    #[serde(default)]
    state: Option<TicketStateParam>,
    /// Optional queued_at frontmatter value.
    #[serde(default)]
    queued_at: Option<String>,
    /// Ordered implementation targets. Selectors may be omitted while planning.
    #[serde(default)]
    targets: Vec<crate::TicketTarget>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TicketEditItemParams {
    /// Ticket reference. Prefer `T-*`; canonical internal ids remain accepted for compatibility.
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
    /// Optional ordered targets replacement or clear operation.
    #[serde(default)]
    targets: Option<crate::TicketTargetsEdit>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct TicketEditPostParams {
    /// Ticket reference. Prefer T-*; canonical internal ids remain accepted.
    ticket: String,
    /// Persisted post identity returned by ShowTicket, not a display ordinal.
    event_ref: String,
    /// Exact observed Markdown body. A stale or truncated body cannot authorize an edit.
    expected_body: String,
    /// Replacement Markdown body.
    body: String,
}

#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum TicketStateParam {
    Open,
    Closed,
}

impl TicketStateParam {
    fn into_state(self) -> TicketState {
        match self {
            Self::Open => TicketState::Open,
            Self::Closed => TicketState::Closed,
        }
    }
    fn into_list_state(self) -> TicketListState {
        match self {
            Self::Open => TicketListState::Open,
            Self::Closed => TicketListState::Closed,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum QueryTicketStateParam {
    Active,
    Open,
    Closed,
    All,
}

impl QueryTicketStateParam {
    fn as_list_state(self) -> Option<TicketListState> {
        match self {
            Self::Open => Some(TicketListState::Open),
            Self::Closed => Some(TicketListState::Closed),
            Self::Active | Self::All => None,
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct QueryTicketParams {
    /// State filter. Defaults to active Tickets (open cases). Use `all` to include closed Tickets.
    #[serde(default)]
    state: Option<QueryTicketStateParam>,
    /// Explicit workflow-state filter list. Cannot be combined with `state`.
    #[serde(default)]
    states: Option<Vec<TicketStateParam>>,
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
    /// Ticket reference. Prefer `T-*`; canonical internal ids remain accepted for compatibility. Exactly one of `id` or `query` must be provided.
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
    /// Ticket reference. Prefer `T-*`; canonical internal ids remain accepted for compatibility.
    ticket: String,
    /// Markdown event body.
    body: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TicketMarkReadyParams {
    /// Ticket reference. Prefer `T-*`; canonical internal ids remain accepted for compatibility.
    ticket: String,
    /// Optional reason attached to the state_changed event.
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TicketIntakeReadyParams {
    /// Ticket reference. Prefer `T-*`; canonical internal ids remain accepted for compatibility.
    ticket: String,
    /// Concise bounded intake summary appended before the ready transition.
    intake_summary: String,
    /// Optional reason attached to the state_changed event.
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TicketQueueParams {
    /// Ticket reference. Prefer `T-*`; canonical internal ids remain accepted for compatibility.
    ticket: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct TicketCaseUpdateParams {
    /// Ticket reference. Prefer T-*.
    ticket: String,
    expected_content_digest: String,
    expected_state: TicketState,
    /// Explicit null means no closure has been observed. Omission is rejected.
    #[serde(deserialize_with = "crate::deserialize_case_closure_observation")]
    #[schemars(required)]
    expected_closure_operation_key: Option<String>,
    operation_key: String,
    action: crate::TicketCaseAction,
    explanation: String,
    #[serde(default)]
    references: Vec<crate::TicketReference>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TicketDependencyCheckParams {
    /// Ticket reference. Prefer `T-*`; canonical internal ids remain accepted for compatibility.
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
    /// Target Ticket reference. Prefer `T-*`; canonical internal ids remain accepted for compatibility.
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
    /// Target Ticket reference. Prefer `T-*`; canonical internal ids remain accepted for compatibility.
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
struct TicketEditPostTool {
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
struct TicketCaseUpdateTool {
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
        execute_ticket_create(&self.backend, params, _ctx)
    }
}

fn execute_ticket_create(
    backend: &TicketToolBackend,
    params: TicketCreateParams,
    _context: agen::tool::ToolExecutionContext,
) -> Result<ToolOutput, ToolError> {
    if params
        .state
        .is_some_and(|state| !matches!(state.into_state(), TicketState::Open))
    {
        return Err(backend_error(
            "TicketCreate",
            TicketError::InvalidWorkflowTransition {
                from: "creation".to_owned(),
                to: params
                    .state
                    .expect("checked non-open state")
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
    input.workflow_state = params.state.map(TicketStateParam::into_state);
    input.queued_by = None;
    input.queued_at = params.queued_at;
    input.targets = params.targets;

    let created = backend
        .create(input)
        .map_err(|error| backend_error("TicketCreate", error))?;
    Ok(json_output(
        format!("Created ticket {}", created.id),
        json!(TicketRefOutput {
            id: created.id,
            state: "open".to_string(),
        }),
    ))
}

#[async_trait]
impl Tool for TicketEditItemTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: TicketEditItemParams = parse_input("TicketEditItem", input_json)?;
        execute_ticket_edit_item(&self.backend, params, _ctx)
    }
}

fn execute_ticket_edit_item(
    backend: &TicketToolBackend,
    params: TicketEditItemParams,
    _context: agen::tool::ToolExecutionContext,
) -> Result<ToolOutput, ToolError> {
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
        targets: params.targets,
        author: None,
    };
    let ticket = backend
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

#[async_trait]
impl Tool for TicketEditPostTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: TicketEditPostParams = parse_input("TicketEditPost", input_json)?;
        execute_ticket_edit_post(&self.backend, params, _ctx)
    }
}

fn execute_ticket_edit_post(
    backend: &TicketToolBackend,
    params: TicketEditPostParams,
    _context: agen::tool::ToolExecutionContext,
) -> Result<ToolOutput, ToolError> {
    let ticket = backend
        .edit_post(
            TicketIdOrSlug::from(params.ticket),
            crate::TicketPostEdit {
                event_ref: params.event_ref,
                expected_body: MarkdownText::new(params.expected_body),
                body: MarkdownText::new(params.body),
                author: default_author(),
            },
        )
        .map_err(|error| backend_error("TicketEditPost", error))?;
    Ok(json_output(
        format!("Edited Ticket post in {}", ticket.meta.id),
        ticket_json(
            &ticket,
            DEFAULT_EVENT_LIMIT,
            DEFAULT_ARTIFACT_LIMIT,
            DEFAULT_BODY_MAX_BYTES,
        ),
    ))
}

#[async_trait]
impl Tool for QueryTicketTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: QueryTicketParams = parse_input("QueryTicket", input_json)?;
        execute_query_ticket(&self.backend, params, _ctx)
    }
}

fn execute_query_ticket(
    backend: &TicketToolBackend,
    params: QueryTicketParams,
    _context: agen::tool::ToolExecutionContext,
) -> Result<ToolOutput, ToolError> {
    let (filter, state_filter, params_limit) = params
        .into_query()
        .map_err(|error| backend_error("QueryTicket", error))?;
    let limit = bounded(params_limit, DEFAULT_LIST_LIMIT, MAX_LIST_LIMIT);
    let tickets = backend
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

#[async_trait]
impl Tool for ShowTicketTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: ShowTicketParams = parse_input("ShowTicket", input_json)?;
        execute_show_ticket(&self.backend, params, _ctx)
    }
}

fn execute_show_ticket(
    backend: &TicketToolBackend,
    params: ShowTicketParams,
    _context: agen::tool::ToolExecutionContext,
) -> Result<ToolOutput, ToolError> {
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
    let ticket = backend
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

fn execute_ticket_thread_event(
    backend: &TicketToolBackend,
    tool_name: &str,
    kind: TicketEventKind,
    params: TicketThreadEventParams,
) -> Result<ToolOutput, ToolError> {
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
            parse_input("TicketComment", input_json)?,
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
                execute_ticket_thread_event(
                    &self.backend,
                    $name,
                    $kind,
                    parse_input($name, input_json)?,
                )
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
        execute_ticket_mark_ready(&self.backend, params, ctx)
    }
}

fn execute_ticket_mark_ready(
    backend: &TicketToolBackend,
    params: TicketMarkReadyParams,
    ctx: agen::tool::ToolExecutionContext,
) -> Result<ToolOutput, ToolError> {
    let ticket = backend
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
        format!("Marked ticket {} routing readiness ready", params.ticket),
        json!({
            "ticket": ticket.meta.id,
            "state": ticket.meta.workflow_state.as_str(),
            "targets": ticket.meta.targets,
            "ok": true
        }),
    ))
}

#[async_trait]
impl Tool for TicketIntakeReadyTool {
    async fn execute(
        &self,
        input_json: &str,
        ctx: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: TicketIntakeReadyParams = parse_input("TicketIntakeReady", input_json)?;
        execute_ticket_intake_ready(&self.backend, params, ctx)
    }
}

fn execute_ticket_intake_ready(
    backend: &TicketToolBackend,
    params: TicketIntakeReadyParams,
    ctx: agen::tool::ToolExecutionContext,
) -> Result<ToolOutput, ToolError> {
    let ticket = backend
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
        format!(
            "Marked ticket {} routing readiness ready after intake",
            params.ticket
        ),
        json!({
            "ticket": ticket.meta.id,
            "state": ticket.meta.workflow_state.as_str(),
            "targets": ticket.meta.targets,
            "ok": true
        }),
    ))
}

#[async_trait]
impl Tool for TicketQueueTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: TicketQueueParams = parse_input("TicketQueue", input_json)?;
        execute_ticket_queue(&self.backend, params, _ctx)
    }
}

fn execute_ticket_queue(
    backend: &TicketToolBackend,
    params: TicketQueueParams,
    _context: agen::tool::ToolExecutionContext,
) -> Result<ToolOutput, ToolError> {
    let queued_by = default_author();
    let mut outcome = backend
        .queue_ready(TicketIdOrSlug::Query(params.ticket.clone()), &queued_by)
        .map_err(|error| backend_error("TicketQueue", error))?;
    outcome.requested_ticket =
        model_ticket_reference(backend, &outcome.requested_ticket, "TicketQueue")?;
    outcome.queued_tickets = outcome
        .queued_tickets
        .into_iter()
        .map(|ticket| model_ticket_reference(backend, &ticket, "TicketQueue"))
        .collect::<Result<Vec<_>, _>>()?;
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

#[async_trait]
impl Tool for TicketCaseUpdateTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: TicketCaseUpdateParams = parse_input("TicketCaseUpdate", input_json)?;
        execute_ticket_case_update(&self.backend, params, _ctx)
    }
}

fn execute_ticket_case_update(
    backend: &TicketToolBackend,
    params: TicketCaseUpdateParams,
    _context: agen::tool::ToolExecutionContext,
) -> Result<ToolOutput, ToolError> {
    let record = backend
        .update_case(
            &params.ticket,
            crate::TicketCaseUpdate {
                expected_content_digest: params.expected_content_digest,
                expected_state: params.expected_state,
                expected_closure_operation_key: params.expected_closure_operation_key,
                operation_key: params.operation_key,
                action: params.action,
                explanation: params.explanation,
                references: params.references,
                author: None,
            },
        )
        .map_err(|error| backend_error("TicketCaseUpdate", error))?;
    let reference = model_ticket_reference(backend, &params.ticket, "TicketCaseUpdate")?;
    Ok(json_output(
        format!(
            "Updated Ticket {} case to {}",
            reference, record.meta.workflow_state
        ),
        json!({
            "ticket": reference, "state": record.meta.workflow_state, "closure_operation_key": record.closure_operation_key, "current_closure": record.current_closure.as_ref().map(|j| json!({"operation_key": j.operation_key, "reason": j.reason, "author": j.author, "explanation": truncate_text(&j.explanation, DEFAULT_BODY_MAX_BYTES)})), "ok": true
        }),
    ))
}

#[async_trait]
impl Tool for TicketRelationRecordTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: TicketRelationRecordParams = parse_input("TicketRelationRecord", input_json)?;
        execute_ticket_relation_record(&self.backend, params, _ctx)
    }
}

fn execute_ticket_relation_record(
    backend: &TicketToolBackend,
    params: TicketRelationRecordParams,
    _context: agen::tool::ToolExecutionContext,
) -> Result<ToolOutput, ToolError> {
    let relation = NewTicketRelation {
        kind: params.kind.into_kind(),
        target: params.target.clone(),
        note: params.note,
        author: None,
    };
    let output = backend
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

#[async_trait]
impl Tool for TicketRelationRemoveTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: TicketRelationRemoveParams = parse_input("TicketRelationRemove", input_json)?;
        execute_ticket_relation_remove(&self.backend, params, _ctx)
    }
}

fn execute_ticket_relation_remove(
    backend: &TicketToolBackend,
    params: TicketRelationRemoveParams,
    _context: agen::tool::ToolExecutionContext,
) -> Result<ToolOutput, ToolError> {
    let output = backend
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

#[async_trait]
impl Tool for TicketRelationQueryTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: TicketRelationQueryParams = parse_input("TicketRelationQuery", input_json)?;
        execute_ticket_relation_query(&self.backend, params, _ctx)
    }
}

fn execute_ticket_relation_query(
    backend: &TicketToolBackend,
    params: TicketRelationQueryParams,
    _context: agen::tool::ToolExecutionContext,
) -> Result<ToolOutput, ToolError> {
    let limit = bounded(params.limit, DEFAULT_LIST_LIMIT, MAX_LIST_LIMIT);
    let ticket = params.ticket.clone().map(TicketIdOrSlug::Id);
    let kind = params.kind.map(TicketRelationKindParam::into_kind);
    let relations = backend
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

#[async_trait]
impl Tool for TicketOrchestrationPlanRecordTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: TicketOrchestrationPlanRecordParams =
            parse_input("TicketOrchestrationPlanRecord", input_json)?;
        execute_ticket_orchestration_plan_record(&self.backend, params, _ctx)
    }
}

fn execute_ticket_orchestration_plan_record(
    backend: &TicketToolBackend,
    params: TicketOrchestrationPlanRecordParams,
    _context: agen::tool::ToolExecutionContext,
) -> Result<ToolOutput, ToolError> {
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
    let output = backend
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

#[async_trait]
impl Tool for TicketOrchestrationPlanQueryTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: TicketOrchestrationPlanQueryParams =
            parse_input("TicketOrchestrationPlanQuery", input_json)?;
        execute_ticket_orchestration_plan_query(&self.backend, params, _ctx)
    }
}

fn execute_ticket_orchestration_plan_query(
    backend: &TicketToolBackend,
    params: TicketOrchestrationPlanQueryParams,
    _context: agen::tool::ToolExecutionContext,
) -> Result<ToolOutput, ToolError> {
    let limit = bounded(params.limit, DEFAULT_LIST_LIMIT, MAX_LIST_LIMIT);
    let ticket = params.ticket.clone().map(TicketIdOrSlug::Query);
    let kind = params
        .relation_kind
        .map(OrchestrationPlanKindParam::into_kind);
    let records = backend
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

#[async_trait]
impl Tool for TicketDoctorTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: TicketDoctorParams = parse_input("TicketDoctor", input_json)?;
        execute_ticket_doctor(&self.backend, params, _ctx)
    }
}

fn execute_ticket_doctor(
    backend: &TicketToolBackend,
    params: TicketDoctorParams,
    _context: agen::tool::ToolExecutionContext,
) -> Result<ToolOutput, ToolError> {
    let limit = bounded(params.limit, DEFAULT_DIAGNOSTIC_LIMIT, MAX_DIAGNOSTIC_LIMIT);
    let report = backend
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

#[async_trait]
impl Tool for TicketDependencyCheckTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: TicketDependencyCheckParams = parse_input("TicketDependencyCheck", input_json)?;
        execute_ticket_dependency_check(&self.backend, params, _ctx)
    }
}

fn execute_ticket_dependency_check(
    backend: &TicketToolBackend,
    params: TicketDependencyCheckParams,
    _context: agen::tool::ToolExecutionContext,
) -> Result<ToolOutput, ToolError> {
    let check = backend
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

/// Execute an internal Ticket command after an adapter validates and converts its contract.
///
/// These DTOs are shared with ordinary Tool input conversion, not a native public contract.
/// This boundary never invokes Tools or validates their JSON schemas. Backend authorization,
/// business validation and durable operation identities remain shared.
pub async fn execute_ticket_command(
    backend: TicketToolBackend,
    operation: &str,
    command: Value,
    context: agen::tool::ToolExecutionContext,
) -> Result<ToolOutput, ToolError> {
    fn convert<T: serde::de::DeserializeOwned>(command: Value) -> Result<T, ToolError> {
        serde_json::from_value(command)
            .map_err(|error| ToolError::InvalidArgument(format!("invalid Ticket command: {error}")))
    }
    match operation {
        "TicketCreate" => {
            execute_ticket_create(&backend, convert::<TicketCreateParams>(command)?, context)
        }
        "TicketEditItem" => {
            execute_ticket_edit_item(&backend, convert::<TicketEditItemParams>(command)?, context)
        }
        "TicketEditPost" => {
            execute_ticket_edit_post(&backend, convert::<TicketEditPostParams>(command)?, context)
        }
        "QueryTicket" => {
            execute_query_ticket(&backend, convert::<QueryTicketParams>(command)?, context)
        }
        "ShowTicket" => {
            execute_show_ticket(&backend, convert::<ShowTicketParams>(command)?, context)
        }
        "TicketMarkReady" => execute_ticket_mark_ready(
            &backend,
            convert::<TicketMarkReadyParams>(command)?,
            context,
        ),
        "TicketIntakeReady" => execute_ticket_intake_ready(
            &backend,
            convert::<TicketIntakeReadyParams>(command)?,
            context,
        ),
        "TicketQueue" => {
            execute_ticket_queue(&backend, convert::<TicketQueueParams>(command)?, context)
        }
        "TicketCaseUpdate" => execute_ticket_case_update(
            &backend,
            convert::<TicketCaseUpdateParams>(command)?,
            context,
        ),
        "TicketRelationRecord" => execute_ticket_relation_record(
            &backend,
            convert::<TicketRelationRecordParams>(command)?,
            context,
        ),
        "TicketRelationRemove" => execute_ticket_relation_remove(
            &backend,
            convert::<TicketRelationRemoveParams>(command)?,
            context,
        ),
        "TicketRelationQuery" => execute_ticket_relation_query(
            &backend,
            convert::<TicketRelationQueryParams>(command)?,
            context,
        ),
        "TicketOrchestrationPlanRecord" => execute_ticket_orchestration_plan_record(
            &backend,
            convert::<TicketOrchestrationPlanRecordParams>(command)?,
            context,
        ),
        "TicketOrchestrationPlanQuery" => execute_ticket_orchestration_plan_query(
            &backend,
            convert::<TicketOrchestrationPlanQueryParams>(command)?,
            context,
        ),
        "TicketDoctor" => {
            execute_ticket_doctor(&backend, convert::<TicketDoctorParams>(command)?, context)
        }
        "TicketDependencyCheck" => execute_ticket_dependency_check(
            &backend,
            convert::<TicketDependencyCheckParams>(command)?,
            context,
        ),
        "TicketComment" => execute_ticket_thread_event(
            &backend,
            operation,
            TicketEventKind::Comment,
            convert(command)?,
        ),
        "TicketPlan" => execute_ticket_thread_event(
            &backend,
            operation,
            TicketEventKind::Plan,
            convert(command)?,
        ),
        "TicketDecision" => execute_ticket_thread_event(
            &backend,
            operation,
            TicketEventKind::Decision,
            convert(command)?,
        ),
        "TicketImplementationReport" => execute_ticket_thread_event(
            &backend,
            operation,
            TicketEventKind::ImplementationReport,
            convert(command)?,
        ),
        _ => Err(ToolError::InvalidArgument(format!(
            "unknown Ticket command: {operation}"
        ))),
    }
}

fn model_ticket_reference(
    backend: &TicketToolBackend,
    reference: &str,
    tool_name: &str,
) -> Result<String, ToolError> {
    let ticket = backend
        .show(TicketIdOrSlug::Id(reference.to_string()))
        .map_err(|error| {
            ToolError::Internal(format!(
                "{tool_name} outcome unknown after mutation while resolving Ticket reference: {error}"
            ))
        })?;
    match ticket.meta.resource_key {
        Some(resource_key) if is_canonical_ticket_resource_key(&resource_key) => Ok(resource_key),
        Some(_) => Err(ToolError::Internal(format!(
            "{tool_name} outcome unknown after mutation: required Ticket key is unavailable"
        ))),
        None => Ok(ticket.meta.id),
    }
}

fn is_canonical_ticket_resource_key(resource_key: &str) -> bool {
    resource_key.strip_prefix("T-").is_some_and(|sequence| {
        !sequence.is_empty() && sequence.bytes().all(|byte| byte.is_ascii_digit())
    })
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

fn ticket_event_json(event: &crate::TicketEvent, body_max_bytes: usize) -> Value {
    json!({
        "event_ref": event.event_ref,
        "is_initial": event.is_initial,
        "kind": event.kind.as_str(),
        "author": event.author,
        "at": event.at,
        "edited_at": event.edited_at,
        "edited_by": event.edited_by,
        "state": event.status,
        "from": event.from,
        "to": event.to,
        "reason": event.reason,
        "state_field": event.state_field,
        "attributes": event.attributes,
        "references": event.references,
        "heading": event.heading,
        "body": truncate_text(event.body.as_str(), body_max_bytes),
        "body_truncated": event.body.as_str().len() > body_max_bytes,
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
        .map(|event| ticket_event_json(event, body_max_bytes))
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
            "created_by": ticket.meta.created_by,
            "updated_at": ticket.meta.updated_at,
            "assignee": ticket.meta.assignee,
            "readiness": ticket.meta.readiness,
            "risk_flags": ticket.meta.risk_flags,
            "queued_by": ticket.meta.queued_by,
            "queued_at": ticket.meta.queued_at,
            "targets": ticket.meta.targets,
        },
        "content_digest": crate::ticket_content_digest(ticket),
        "closure_operation_key": ticket.closure_operation_key,
        "current_closure": ticket.current_closure.as_ref().map(|j| json!({"operation_key": j.operation_key, "reason": j.reason, "explanation": truncate_text(&j.explanation, body_max_bytes), "author": j.author, "references": j.references, "at": j.at})),
        "case_history": { "count": ticket.case_history.len(), "items": ticket.case_history.iter().rev().take(event_limit).map(|j| json!({"operation_key": j.operation_key, "previous_closure_operation_key": j.previous_closure_operation_key, "action": j.action, "explanation": truncate_text(&j.explanation, body_max_bytes), "author": j.author, "references": j.references, "at": j.at})).collect::<Vec<_>>() },
        "legacy_workflow_snapshot": ticket.legacy_workflow_snapshot,
        "initial_post": ticket.events.iter().find(|event| event.is_initial).map(|event| ticket_event_json(event, body_max_bytes)),
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
        "TicketEditPost" => serde_json::to_value(schemars::schema_for!(TicketEditPostParams)),
        "QueryTicket" => serde_json::to_value(schemars::schema_for!(QueryTicketParams)),
        "ShowTicket" => serde_json::to_value(schemars::schema_for!(ShowTicketParams)),
        "TicketComment" | "TicketPlan" | "TicketDecision" | "TicketImplementationReport" => {
            serde_json::to_value(schemars::schema_for!(TicketThreadEventParams))
        }
        "TicketMarkReady" => serde_json::to_value(schemars::schema_for!(TicketMarkReadyParams)),
        "TicketIntakeReady" => serde_json::to_value(schemars::schema_for!(TicketIntakeReadyParams)),
        "TicketQueue" => serde_json::to_value(schemars::schema_for!(TicketQueueParams)),
        "TicketCaseUpdate" => serde_json::to_value(schemars::schema_for!(TicketCaseUpdateParams)),
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
impl_from_backend!(TicketEditPostTool);
impl_from_backend!(QueryTicketTool);
impl_from_backend!(ShowTicketTool);
impl_from_backend!(TicketCommentTool);
impl_from_backend!(TicketPlanTool);
impl_from_backend!(TicketDecisionTool);
impl_from_backend!(TicketImplementationReportTool);
impl_from_backend!(TicketMarkReadyTool);
impl_from_backend!(TicketIntakeReadyTool);
impl_from_backend!(TicketQueueTool);
impl_from_backend!(TicketCaseUpdateTool);
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
        tool_definition::<TicketEditPostTool>("TicketEditPost", backend.clone()),
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
        tool_definition::<TicketCaseUpdateTool>("TicketCaseUpdate", backend.clone()),
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
            repository_key: &str,
            ref_selector: Option<&str>,
        ) -> crate::Result<crate::ResolvedTicketTarget> {
            Ok(crate::ResolvedTicketTarget {
                repository_key: repository_key.to_owned(),
                ref_selector: Some(ref_selector.unwrap_or("develop").to_owned()),
                access: crate::TicketTargetAccess::ReadOnly,
            })
        }
    }

    fn write_target(repository_key: &str) -> crate::TicketTarget {
        crate::TicketTarget {
            repository_key: repository_key.to_owned(),
            ref_selector: None,
            access: crate::TicketTargetAccess::ReadWrite,
        }
    }

    fn backend(temp: &TempDir) -> crate::SqliteTicketBackend {
        sqlite_backend(temp)
    }

    fn sqlite_backend(temp: &TempDir) -> crate::SqliteTicketBackend {
        crate::SqliteTicketBackend::open(temp.path().join("tickets.db"), "workspace")
            .unwrap()
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

    #[tokio::test]
    async fn internal_commands_share_persisted_edits_post_cas_and_dependency_results_with_tools() {
        let temp = TempDir::new().unwrap();
        let backend = backend(&temp);
        let created = execute_ticket_command(
            backend.clone().into(),
            "TicketCreate",
            json!({"title":"Shared execution", "body":"Original"}),
            Default::default(),
        )
        .await
        .unwrap();
        let id = serde_json::from_str::<Value>(created.content.as_deref().unwrap()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let edit = tool_by_name(backend.clone(), "TicketEditItem");
        edit.execute(
            &json!({"ticket":id, "old_string":"Original", "new_string":"Tool edit"}).to_string(),
            Default::default(),
        )
        .await
        .unwrap();
        execute_ticket_command(
            backend.clone().into(),
            "TicketEditItem",
            json!({"ticket":id, "old_string":"Tool edit", "new_string":"Command edit"}),
            Default::default(),
        )
        .await
        .unwrap();
        execute_ticket_command(
            backend.clone().into(),
            "TicketComment",
            json!({"ticket":id, "body":"Discussion"}),
            Default::default(),
        )
        .await
        .unwrap();
        let before = backend.show(id.clone().into()).unwrap();
        assert_eq!(before.document.body.as_str(), "Command edit");
        let post = before
            .events
            .iter()
            .find(|event| !event.is_initial)
            .unwrap();
        let input = json!({"ticket":id, "event_ref":post.event_ref, "expected_body":"Discussion", "body":"Updated"});
        execute_ticket_command(
            backend.clone().into(),
            "TicketEditPost",
            input.clone(),
            Default::default(),
        )
        .await
        .unwrap();
        let native_error = execute_ticket_command(
            backend.clone().into(),
            "TicketEditPost",
            input.clone(),
            Default::default(),
        )
        .await
        .unwrap_err();
        let tool_error = tool_by_name(backend.clone(), "TicketEditPost")
            .execute(&input.to_string(), Default::default())
            .await
            .unwrap_err();
        assert_eq!(native_error.to_string(), tool_error.to_string());
        let after = backend.show(id.clone().into()).unwrap();
        assert_eq!(
            after
                .events
                .iter()
                .find(|event| !event.is_initial)
                .unwrap()
                .body
                .as_str(),
            "Updated"
        );
        let input = json!({"ticket":id});
        let native = execute_ticket_command(
            backend.clone().into(),
            "TicketDependencyCheck",
            input.clone(),
            Default::default(),
        )
        .await
        .unwrap();
        let ordinary = tool_by_name(backend, "TicketDependencyCheck")
            .execute(&input.to_string(), Default::default())
            .await
            .unwrap();
        assert_eq!(native.content, ordinary.content);
    }

    #[tokio::test]
    async fn internal_relation_and_plan_commands_share_tools_business_validation_and_queries() {
        let temp = TempDir::new().unwrap();
        let backend = backend(&temp);
        let source = backend.create(NewTicket::new("Source")).unwrap().id;
        let target = backend.create(NewTicket::new("Target")).unwrap().id;
        let relation = json!({"ticket":source, "kind":"related", "target":target});
        execute_ticket_command(
            backend.clone().into(),
            "TicketRelationRecord",
            relation.clone(),
            Default::default(),
        )
        .await
        .unwrap();
        let query = json!({"ticket":source});
        let native = execute_ticket_command(
            backend.clone().into(),
            "TicketRelationQuery",
            query.clone(),
            Default::default(),
        )
        .await
        .unwrap();
        let ordinary = tool_by_name(backend.clone(), "TicketRelationQuery")
            .execute(&query.to_string(), Default::default())
            .await
            .unwrap();
        assert_eq!(native.content, ordinary.content);
        execute_ticket_command(
            backend.clone().into(),
            "TicketRelationRemove",
            relation,
            Default::default(),
        )
        .await
        .unwrap();
        let plan = json!({"ticket":source, "kind":"accepted_plan", "accepted_plan":{"summary":"Shared execution plan", "branch":"work"}});
        execute_ticket_command(
            backend.clone().into(),
            "TicketOrchestrationPlanRecord",
            plan,
            Default::default(),
        )
        .await
        .unwrap();
        let native = execute_ticket_command(
            backend.clone().into(),
            "TicketOrchestrationPlanQuery",
            query.clone(),
            Default::default(),
        )
        .await
        .unwrap();
        let ordinary = tool_by_name(backend.clone(), "TicketOrchestrationPlanQuery")
            .execute(&query.to_string(), Default::default())
            .await
            .unwrap();
        assert_eq!(native.content, ordinary.content);
        let invalid = json!({"ticket":source, "kind":"accepted_plan"});
        let native = execute_ticket_command(
            backend.clone().into(),
            "TicketOrchestrationPlanRecord",
            invalid.clone(),
            Default::default(),
        )
        .await
        .unwrap_err();
        let ordinary = tool_by_name(backend.clone(), "TicketOrchestrationPlanRecord")
            .execute(&invalid.to_string(), Default::default())
            .await
            .unwrap_err();
        assert_eq!(native.to_string(), ordinary.to_string());
        assert!(matches!(
            execute_ticket_command(
                backend.clone().into(),
                "TicketRelationRecord",
                json!({"ticket":source,"target":target,"kind":"invalid"}),
                Default::default()
            )
            .await,
            Err(ToolError::InvalidArgument(_))
        ));
        assert!(
            execute_ticket_command(backend.into(), "Unknown", json!({}), Default::default())
                .await
                .unwrap_err()
                .to_string()
                .contains("unknown Ticket command")
        );
    }

    #[tokio::test]
    async fn edit_post_tool_and_show_preserve_common_identity_and_metadata() {
        let temp = TempDir::new().unwrap();
        let backend = backend(&temp);
        let mut input = NewTicket::new("Post tools");
        input.author = Some("creator".into());
        input.body = MarkdownText::new("Initial body");
        let reference = backend.create(input).unwrap();
        let comment = tool_by_name(backend.clone(), "TicketComment");
        comment
            .execute(
                &json!({"ticket": reference.id, "body": "Discussion body"}).to_string(),
                Default::default(),
            )
            .await
            .unwrap();
        let before = backend.show(reference.id.clone().into()).unwrap();
        let edit = tool_by_name(backend.clone(), "TicketEditPost");
        let show = tool_by_name(backend.clone(), "ShowTicket");
        let schema = input_schema("TicketEditPost");
        assert_eq!(schema["additionalProperties"], false);
        for key in ["ticket", "event_ref", "expected_body", "body"] {
            assert!(schema["required"].as_array().unwrap().contains(&json!(key)));
        }
        assert!(
            !schema["properties"]
                .as_object()
                .unwrap()
                .contains_key("author")
        );
        for post in &before.events {
            let args = json!({"ticket": reference.id, "event_ref": post.event_ref, "expected_body": post.body.as_str(), "body": format!("Edited {}", post.body.as_str())});
            let output = edit
                .execute(&args.to_string(), Default::default())
                .await
                .unwrap();
            let value: Value = serde_json::from_str(&output.content.unwrap()).unwrap();
            assert_eq!(value["events"]["count"], 2);
            let projected = value["events"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["event_ref"] == post.event_ref)
                .unwrap();
            assert_eq!(projected["author"], json!(post.author));
            assert_eq!(projected["at"], json!(post.at));
            assert_eq!(projected["is_initial"], post.is_initial);
            assert_eq!(projected["edited_by"], default_author());
            assert!(projected["edited_at"].is_string());
            assert_eq!(projected["body_truncated"], false);
            assert!(
                edit.execute(&args.to_string(), Default::default())
                    .await
                    .is_err()
            );
            let mut spoofed = args.clone();
            spoofed["author"] = json!("spoofed-author");
            assert!(
                edit.execute(&spoofed.to_string(), Default::default())
                    .await
                    .is_err()
            );
        }
        let output = show
            .execute(
                &json!({"id": reference.id, "event_limit": 1}).to_string(),
                Default::default(),
            )
            .await
            .unwrap();
        let value: Value = serde_json::from_str(&output.content.unwrap()).unwrap();
        assert_eq!(value["events"]["returned"], 1);
        assert_eq!(value["events"]["items"][0]["is_initial"], false);
        assert_eq!(value["initial_post"]["is_initial"], true);
        assert_eq!(
            value["initial_post"]["event_ref"],
            before.events[0].event_ref
        );
        assert_eq!(value["initial_post"]["body"], value["body"]);
        assert_eq!(value["meta"]["created_by"], "creator");
        assert_eq!(value["initial_post"]["author"], "creator");
        let initial_keys = value["initial_post"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>();
        let comment_keys = value["events"]["items"][0]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>();
        assert_eq!(initial_keys, comment_keys);
        let clipped = ticket_json(&backend.show(reference.id.clone().into()).unwrap(), 1, 0, 3);
        assert_eq!(clipped["initial_post"]["body_truncated"], true);
        assert_eq!(
            clipped["initial_post"]["event_ref"],
            before.events[0].event_ref
        );
        assert_eq!(clipped["initial_post"]["author"], "creator");
        let decision = tool_by_name(backend.clone(), "TicketDecision");
        decision
            .execute(
                &json!({"ticket": reference.id, "body": "Immutable judgment"}).to_string(),
                Default::default(),
            )
            .await
            .unwrap();
        let current = backend.show(reference.id.clone().into()).unwrap();
        let system = current.events.last().unwrap();
        assert!(edit.execute(&json!({"ticket": reference.id, "event_ref": system.event_ref, "expected_body": system.body.as_str(), "body": "Changed judgment"}).to_string(), Default::default()).await.is_err());
        assert_eq!(backend.show(reference.id.into()).unwrap(), current);
        let (_, unknown) = backend
            .create_with_snapshot(NewTicket::new("Unknown creator"))
            .unwrap();
        let unknown_json = ticket_json(&unknown, 1, 0, 100);
        assert!(
            unknown_json["meta"]
                .as_object()
                .unwrap()
                .contains_key("created_by")
        );
        assert!(unknown_json["meta"]["created_by"].is_null());
        assert!(unknown_json["initial_post"]["author"].is_null());
    }

    #[test]
    fn post_mutation_reference_failures_are_outcome_unknown() {
        let temp = TempDir::new().unwrap();
        let backend = TicketToolBackend::new(backend(&temp));
        let error = model_ticket_reference(&backend, "missing", "TicketCaseUpdate").unwrap_err();
        assert!(matches!(
            error,
            ToolError::Internal(message)
                if message.contains("outcome unknown after mutation")
        ));
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
                "TicketEditPost",
                "TicketComment",
                "TicketPlan",
                "TicketDecision",
                "TicketImplementationReport",
                "TicketMarkReady",
                "TicketIntakeReady",
                "TicketQueue",
                "TicketCaseUpdate",
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
                    "body": "## Background\n\nCreated by tool.\n",
                    "targets": [
                        {
                            "repository_key": "main",
                            "ref_selector": "develop",
                            "access": "read_write"
                        },
                        {
                            "repository_key": "docs",
                            "access": "read_only"
                        }
                    ]
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
            .execute(&json!({ "state": "open" }).to_string(), Default::default())
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
        let shown_json: Value = serde_json::from_str(&shown_content).unwrap();
        assert_eq!(shown_json["meta"]["targets"][0]["repository_key"], "main");
        assert_eq!(shown_json["meta"]["targets"][0]["access"], "read_write");
        assert_eq!(shown_json["meta"]["targets"][1]["repository_key"], "docs");
        assert_eq!(shown_json["meta"]["targets"][1]["access"], "read_only");
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
            ticket.workflow_state = Some(TicketState::Closed);
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
        ready_input.workflow_state = Some(TicketState::Open);
        let ready = backend.create(ready_input).unwrap();
        let mut closed_input = NewTicket::new("Closed Ticket");
        closed_input.workflow_state = Some(TicketState::Closed);
        let closed = backend.create(closed_input).unwrap();

        let listed = list
            .execute(
                &json!({ "states": ["open", "closed"] }).to_string(),
                Default::default(),
            )
            .await
            .unwrap();
        let listed_json: Value = serde_json::from_str(&listed.content.unwrap()).unwrap();
        assert_eq!(listed_json["state_filter"], "open,closed");
        assert_eq!(listed_json["count"].as_u64(), Some(3));
        let listed_ids = listed_json["tickets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|ticket| ticket["id"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(listed_ids.contains(&planning.id.as_str()));
        assert!(listed_ids.contains(&closed.id.as_str()));
        assert!(listed_ids.contains(&ready.id.as_str()));

        let mixed = list
            .execute(
                &json!({ "state": "active", "states": ["open"] }).to_string(),
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
        assert_eq!(record.events.len(), 1);
        assert_eq!(record.events[0].kind, TicketEventKind::Comment);
        assert_eq!(record.events[0].body, record.document.body);
        assert!(record.events[0].edited_at.is_some());

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
    async fn ticket_intake_ready_records_summary_with_validated_target() {
        let temp = TempDir::new().unwrap();
        let backend = backend(&temp);
        let mut input = NewTicket::new("Intake Workflow");
        input.targets = vec![write_target("main")];
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
        assert_eq!(record.meta.workflow_state, TicketState::Open);
        assert_eq!(
            record.meta.targets[0].ref_selector.as_deref(),
            Some("develop")
        );
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
    async fn ticket_mark_ready_tool_rejects_closed_ticket() {
        let temp = TempDir::new().unwrap();
        let backend = backend(&temp);
        let mut input = NewTicket::new("Already Closed");
        input.workflow_state = Some(TicketState::Closed);
        let created = backend.create(input).unwrap();
        let intake_ready = tool_by_name(backend.clone(), "TicketMarkReady");

        let error = intake_ready
            .execute(
                &json!({
                    "ticket": created.id.clone(),
                    "intake_summary": "Should not rewrite closed case."
                })
                .to_string(),
                Default::default(),
            )
            .await
            .unwrap_err();

        assert!(error.to_string().contains("stale ticket workflow state"));
        let record = backend.show(TicketIdOrSlug::Id(created.id)).unwrap();
        assert_eq!(record.meta.workflow_state, TicketState::Closed);
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
        assert_eq!(current.meta.workflow_state, TicketState::Open);
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
    async fn ticket_create_uses_opaque_id_without_repository_local_writes() {
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
        assert!(!temp.path().join(".yoi").exists());
        assert!(!temp.path().join("tickets").exists());
        assert!(temp.path().join("tickets.db").is_file());
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
        let create_properties = input_schema("TicketCreate")["properties"]
            .as_object()
            .unwrap()
            .clone();
        assert!(create_properties.contains_key("targets"));
        assert!(!create_properties.contains_key("repository_key"));
        assert!(!create_properties.contains_key("ref_selector"));
        assert!(create_schema.contains("read_only"));
        assert!(create_schema.contains("read_write"));
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
        assert!(edit_schema.contains("targets"));
        assert!(
            !input_schema("TicketEditItem")["properties"]
                .as_object()
                .unwrap()
                .contains_key("target")
        );
        for name in [
            "TicketCreate",
            "TicketEditItem",
            "TicketEditPost",
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
    #[test]
    fn case_tool_schema_exposes_common_cas_and_no_legacy_unguarded_mutation() {
        let temp = TempDir::new().unwrap();
        let definition = ticket_tools(backend(&temp))
            .into_iter()
            .find(|d| d().0.name == "TicketCaseUpdate")
            .unwrap();
        let (meta, _) = definition();
        let schema = &meta.input_schema;
        for field in [
            "ticket",
            "expected_content_digest",
            "expected_state",
            "expected_closure_operation_key",
            "operation_key",
            "action",
            "explanation",
        ] {
            assert!(
                schema["required"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|name| name == field),
                "missing {field}: {schema}"
            );
        }
        assert!(
            schema["properties"]
                .get("expected_closure_operation_key")
                .is_some()
        );
        assert!(!TICKET_TOOL_NAMES.contains(&"TicketWorkflowState"));
        assert!(!TICKET_TOOL_NAMES.contains(&"TicketClose"));
        assert!(meta.description.contains("1024 UTF-8 bytes"));
    }

    #[tokio::test]
    async fn case_tool_closes_replays_corrects_and_reopens_with_bounded_show_history() {
        let temp = TempDir::new().unwrap();
        let backend = backend(&temp);
        let created = backend.create(NewTicket::new("Case via tool")).unwrap();
        let observed = backend.show(created.id.clone().into()).unwrap();
        let update = tool_by_name(backend.clone(), "TicketCaseUpdate");
        let input = json!({"ticket": created.id, "expected_content_digest": crate::ticket_content_digest(&observed), "expected_state": "open", "expected_closure_operation_key": null,
            "operation_key": "tool-close", "action": {"action":"close", "reason":"not_planned"}, "explanation": "Not pursuing this case.\n\nThis is not a failure milestone.", "references": [{"kind":"result", "target":"doc:decision"}]});
        update
            .execute(&input.to_string(), Default::default())
            .await
            .unwrap();
        update
            .execute(&input.to_string(), Default::default())
            .await
            .unwrap();
        let closed = backend.show(created.id.clone().into()).unwrap();
        assert_eq!(closed.case_history.len(), 1);
        assert_eq!(
            closed.current_closure.as_ref().unwrap().reason,
            crate::TicketClosureReason::NotPlanned
        );
        let mut correction = input.clone();
        correction["expected_state"] = json!("closed");
        correction["expected_closure_operation_key"] = json!("tool-close");
        correction["operation_key"] = json!("tool-correct");
        correction["action"] = json!({"action":"correct_closure", "reason":"duplicate"});
        correction["explanation"] = json!("A long judgment ".repeat(1000));
        update
            .execute(&correction.to_string(), Default::default())
            .await
            .unwrap();
        let show = tool_by_name(backend.clone(), "ShowTicket");
        let output = show
            .execute(
                &json!({"id":created.id, "event_limit":1, "body_max_bytes":128}).to_string(),
                Default::default(),
            )
            .await
            .unwrap();
        let value: Value = serde_json::from_str(&output.content.unwrap()).unwrap();
        assert_eq!(value["closure_operation_key"], "tool-correct");
        assert_eq!(value["case_history"]["count"], 2);
        assert_eq!(value["case_history"]["items"].as_array().unwrap().len(), 1);
        assert!(
            value["current_closure"]["explanation"]
                .as_str()
                .unwrap()
                .len()
                < 256
        );
        assert!(value["content_digest"].as_str().unwrap().len() == 64);
        let mut reopen = correction;
        reopen["expected_closure_operation_key"] = json!("tool-correct");
        reopen["operation_key"] = json!("tool-reopen");
        reopen["action"] = json!({"action":"reopen"});
        reopen["explanation"] = json!("Reconsider");
        update
            .execute(&reopen.to_string(), Default::default())
            .await
            .unwrap();
        let reopened = backend.show(created.id.clone().into()).unwrap();
        assert_eq!(reopened.meta.workflow_state, TicketState::Open);
        assert_eq!(reopened.case_history.len(), 3);
        assert_eq!(
            reopened.closure_operation_key.as_deref(),
            Some("tool-correct")
        );
        assert!(
            update
                .execute(
                    &reopen.to_string().replace("tool-correct", "wrong-closure"),
                    Default::default()
                )
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn case_tool_rejects_legacy_states_and_unrecognized_input_fields() {
        let temp = TempDir::new().unwrap();
        let backend = backend(&temp);
        let created = backend.create(NewTicket::new("Validation")).unwrap();
        let observed = backend.show(created.id.clone().into()).unwrap();
        let update = tool_by_name(backend.clone(), "TicketCaseUpdate");
        let input = json!({"ticket":created.id, "expected_content_digest":crate::ticket_content_digest(&observed), "expected_state":"open", "operation_key":"key", "action":{"action":"close", "reason":"unknown"}, "explanation":"Reason"});
        let mut legacy = input.clone();
        legacy["expected_state"] = json!("done");
        assert!(
            update
                .execute(&legacy.to_string(), Default::default())
                .await
                .is_err()
        );
        let mut unknown = input;
        unknown["from"] = json!("open");
        assert!(
            update
                .execute(&unknown.to_string(), Default::default())
                .await
                .is_err()
        );
        assert_eq!(backend.show(created.id.into()).unwrap(), observed);
    }
}
