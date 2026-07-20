//! Ticket domain types and the local `.yoi/tickets/` file backend.
//!
//! The public domain name is **Ticket**. `LocalTicketBackend` preserves the
//! repository's current flat `.yoi/tickets/<ticket-id>/` layout and the
//! event/thread format while exposing typed Rust operations.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

use chrono::Utc;
use fs4::fs_std::FileExt;
use project_record::{allocate_record_id, unix_epoch_millis_now, validate_record_id};
use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping as YamlMapping, Value as YamlValue};
use thiserror::Error;

pub mod config;
pub mod tool;

const REQUIRED_FIELDS: [&str; 4] = ["title", "state", "created_at", "updated_at"];
const MAX_STATE_CHANGE_REASON_BYTES: usize = 1024;
const MAX_INTAKE_SUMMARY_BODY_BYTES: usize = 16 * 1024;
const ORCHESTRATION_PLAN_ARTIFACT: &str = "orchestration-plan.jsonl";
const TICKET_RELATIONS_ARTIFACT: &str = "relations.json";
const MAX_ORCHESTRATION_PLAN_TEXT_BYTES: usize = 16 * 1024;
const MAX_ORCHESTRATION_PLAN_FIELD_BYTES: usize = 1024;
const MAX_TICKET_RELATION_NOTE_BYTES: usize = 16 * 1024;
const MAX_TICKET_RELATION_FIELD_BYTES: usize = 1024;
const DEFAULT_TICKET_BODY: &str =
    "## Background\n\nCreated by LocalTicketBackend.\n\n## Acceptance criteria\n\n- TBD\n";
const JAPANESE_TICKET_BODY: &str =
    "## 背景\n\nLocalTicketBackend によって作成されました。\n\n## 受け入れ条件\n\n- 未定\n";

fn normalized_record_language(language: &str) -> Option<String> {
    let language = language.trim();
    (!language.is_empty()).then(|| language.to_string())
}

fn is_japanese_record_language(language: Option<&str>) -> bool {
    let Some(language) = language else {
        return false;
    };
    let language = language.trim();
    language.eq_ignore_ascii_case("japanese")
        || language.eq_ignore_ascii_case("ja")
        || language.eq_ignore_ascii_case("ja-JP")
        || language.contains("日本語")
}

pub type Result<T> = std::result::Result<T, TicketError>;

#[derive(Debug, Error)]
pub enum TicketError {
    #[error("ticket backend I/O error at {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("ticket not found: {0}")]
    NotFound(String),
    #[error("ambiguous ticket query {query}: {matches:?}")]
    Ambiguous {
        query: String,
        matches: Vec<PathBuf>,
    },
    #[error("invalid ticket filename component: {0}")]
    InvalidPathComponent(String),
    #[error("ticket path escapes configured root: {path}")]
    PathEscapesRoot { path: PathBuf },
    #[error("ticket backend is locked: {path}")]
    Locked { path: PathBuf },
    #[error("ticket conflict: {0}")]
    Conflict(String),
    #[error("ticket parse error in {path}: {message}")]
    Parse { path: PathBuf, message: String },
}

fn io_err(path: impl Into<PathBuf>, source: io::Error) -> TicketError {
    TicketError::Io {
        path: path.into(),
        source,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum TicketStatus {
    Open,
    Closed,
}

impl TicketStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
        }
    }

    pub fn parse_local(value: &str) -> Option<Self> {
        match value {
            "open" => Some(Self::Open),
            "closed" => Some(Self::Closed),
            _ => None,
        }
    }
}

impl fmt::Display for TicketStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ExtensibleTicketStatus {
    Open,
    Closed,
    Other(String),
}

impl ExtensibleTicketStatus {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
            Self::Other(value) => value.as_str(),
        }
    }

    pub fn as_local(&self) -> Option<TicketStatus> {
        match self {
            Self::Open => Some(TicketStatus::Open),
            Self::Closed => Some(TicketStatus::Closed),
            Self::Other(_) => None,
        }
    }
}

impl From<&str> for ExtensibleTicketStatus {
    fn from(value: &str) -> Self {
        match value {
            "open" => Self::Open,
            "closed" => Self::Closed,
            other => Self::Other(other.to_string()),
        }
    }
}

impl From<TicketStatus> for ExtensibleTicketStatus {
    fn from(value: TicketStatus) -> Self {
        match value {
            TicketStatus::Open => Self::Open,
            TicketStatus::Closed => Self::Closed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketWorkflowState {
    Planning,
    Ready,
    Queued,
    InProgress,
    Done,
    Closed,
}

impl TicketWorkflowState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Planning => "planning",
            Self::Ready => "ready",
            Self::Queued => "queued",
            Self::InProgress => "inprogress",
            Self::Done => "done",
            Self::Closed => "closed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "planning" => Some(Self::Planning),
            "ready" => Some(Self::Ready),
            "queued" => Some(Self::Queued),
            "inprogress" => Some(Self::InProgress),
            "done" => Some(Self::Done),
            "closed" => Some(Self::Closed),
            _ => None,
        }
    }

    pub fn default_for_status(status: &ExtensibleTicketStatus) -> Self {
        match status {
            ExtensibleTicketStatus::Closed => Self::Closed,
            _ => Self::Planning,
        }
    }

    pub fn is_planning_ready_transition(from: Self, to: Self) -> bool {
        from == Self::Planning && to == Self::Ready
    }

    pub fn is_queue_transition(from: Self, to: Self) -> bool {
        from == Self::Ready && to == Self::Queued
    }

    pub fn is_role_transition(from: Self, to: Self) -> bool {
        matches!(
            (from, to),
            (Self::Queued, Self::InProgress)
                | (Self::InProgress, Self::Done)
                | (Self::Ready, Self::Planning)
                | (Self::Queued, Self::Planning)
        )
    }
}

impl fmt::Display for TicketWorkflowState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkdownText(pub String);

impl MarkdownText {
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl From<&str> for MarkdownText {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl From<String> for MarkdownText {
    fn from(value: String) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TicketIdOrSlug {
    Id(String),
    Slug(String),
    Query(String),
}

impl TicketIdOrSlug {
    fn as_query(&self) -> &str {
        match self {
            Self::Id(value) | Self::Slug(value) | Self::Query(value) => value.as_str(),
        }
    }
}

impl From<&str> for TicketIdOrSlug {
    fn from(value: &str) -> Self {
        Self::Query(value.to_string())
    }
}

impl From<String> for TicketIdOrSlug {
    fn from(value: String) -> Self {
        Self::Query(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketEventKind {
    Create,
    Comment,
    Plan,
    Decision,
    ImplementationReport,
    Review,
    StateChanged,
    IntakeSummary,
    StatusChanged,
    Close,
    Other(String),
}

impl TicketEventKind {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Create => "create",
            Self::Comment => "comment",
            Self::Plan => "plan",
            Self::Decision => "decision",
            Self::ImplementationReport => "implementation_report",
            Self::Review => "review",
            Self::StateChanged => "state_changed",
            Self::IntakeSummary => "intake_summary",
            Self::StatusChanged => "status_changed",
            Self::Close => "close",
            Self::Other(value) => value.as_str(),
        }
    }

    fn heading(&self) -> String {
        match self {
            Self::Create => "Created".to_string(),
            Self::Comment => "Comment".to_string(),
            Self::Plan => "Plan".to_string(),
            Self::Decision => "Decision".to_string(),
            Self::ImplementationReport => "Implementation report".to_string(),
            Self::Review => "Review".to_string(),
            Self::StateChanged => "State changed".to_string(),
            Self::IntakeSummary => "Intake summary".to_string(),
            Self::StatusChanged => "Status changed".to_string(),
            Self::Close => "Closed".to_string(),
            Self::Other(value) => value.clone(),
        }
    }
}

impl From<&str> for TicketEventKind {
    fn from(value: &str) -> Self {
        match value {
            "create" => Self::Create,
            "comment" => Self::Comment,
            "plan" => Self::Plan,
            "decision" => Self::Decision,
            "implementation_report" => Self::ImplementationReport,
            "review" => Self::Review,
            "state_changed" => Self::StateChanged,
            "intake_summary" => Self::IntakeSummary,
            "status_changed" => Self::StatusChanged,
            "close" | "closed" => Self::Close,
            other => Self::Other(other.to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketReviewResult {
    Approve,
    RequestChanges,
    Other(String),
}

impl TicketReviewResult {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Approve => "approve",
            Self::RequestChanges => "request_changes",
            Self::Other(value) => value.as_str(),
        }
    }

    fn heading(&self) -> String {
        match self {
            Self::Approve => "Review: approve".to_string(),
            Self::RequestChanges => "Review: request changes".to_string(),
            Self::Other(value) => format!("Review: {value}"),
        }
    }
}

impl From<&str> for TicketReviewResult {
    fn from(value: &str) -> Self {
        match value {
            "approve" => Self::Approve,
            "request_changes" => Self::RequestChanges,
            other => Self::Other(other.to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketReference {
    pub kind: String,
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewTicketEvent {
    pub kind: TicketEventKind,
    pub author: Option<String>,
    pub body: MarkdownText,
    pub references: Vec<TicketReference>,
}

impl NewTicketEvent {
    pub fn new(kind: TicketEventKind, body: impl Into<MarkdownText>) -> Self {
        Self {
            kind,
            author: None,
            body: body.into(),
            references: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketStateChange {
    pub from: String,
    pub to: String,
    pub author: Option<String>,
    pub reason: String,
    pub body: MarkdownText,
    pub references: Vec<TicketReference>,
}

impl TicketStateChange {
    pub fn new(
        from: impl Into<String>,
        to: impl Into<String>,
        reason: impl Into<String>,
        body: impl Into<MarkdownText>,
    ) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
            author: None,
            reason: reason.into(),
            body: body.into(),
            references: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketIntakeSummary {
    pub author: Option<String>,
    pub body: MarkdownText,
    pub references: Vec<TicketReference>,
}

impl TicketIntakeSummary {
    pub fn new(body: impl Into<MarkdownText>) -> Self {
        Self {
            author: None,
            body: body.into(),
            references: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketReview {
    pub result: TicketReviewResult,
    pub author: Option<String>,
    pub body: MarkdownText,
}

impl TicketReview {
    pub fn approve(body: impl Into<MarkdownText>) -> Self {
        Self {
            result: TicketReviewResult::Approve,
            author: None,
            body: body.into(),
        }
    }

    pub fn request_changes(body: impl Into<MarkdownText>) -> Self {
        Self {
            result: TicketReviewResult::RequestChanges,
            author: None,
            body: body.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewTicket {
    pub title: String,
    pub slug: Option<String>,
    pub kind: String,
    pub priority: String,
    pub labels: Vec<String>,
    pub body: MarkdownText,
    pub author: Option<String>,
    pub assignee: Option<String>,
    pub readiness: Option<String>,
    pub risk_flags: Vec<String>,
    pub workflow_state: Option<TicketWorkflowState>,
    pub queued_by: Option<String>,
    pub queued_at: Option<String>,
}

impl NewTicket {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            slug: None,
            kind: "task".to_string(),
            priority: "P2".to_string(),
            labels: Vec::new(),
            body: MarkdownText::new(DEFAULT_TICKET_BODY),
            author: None,
            assignee: None,
            readiness: None,
            risk_flags: Vec::new(),
            workflow_state: None,
            queued_by: None,
            queued_at: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TicketListState {
    Planning,
    Ready,
    Queued,
    InProgress,
    Done,
    Closed,
}

impl TicketListState {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "planning" => Some(Self::Planning),
            "ready" => Some(Self::Ready),
            "queued" => Some(Self::Queued),
            "inprogress" => Some(Self::InProgress),
            "done" => Some(Self::Done),
            "closed" => Some(Self::Closed),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Planning => "planning",
            Self::Ready => "ready",
            Self::Queued => "queued",
            Self::InProgress => "inprogress",
            Self::Done => "done",
            Self::Closed => "closed",
        }
    }

    fn matches_workflow_state(self, state: TicketWorkflowState) -> bool {
        match self {
            Self::Planning => state == TicketWorkflowState::Planning,
            Self::Ready => state == TicketWorkflowState::Ready,
            Self::Queued => state == TicketWorkflowState::Queued,
            Self::InProgress => state == TicketWorkflowState::InProgress,
            Self::Done => state == TicketWorkflowState::Done,
            Self::Closed => state == TicketWorkflowState::Closed,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketStateSelector {
    /// All non-closed workflow states: planning, ready, queued, inprogress, and done.
    Active,
    /// Every workflow state, including closed.
    All,
    /// An explicit set of list-query state tokens.
    States(BTreeSet<TicketListState>),
}

impl Default for TicketStateSelector {
    fn default() -> Self {
        Self::Active
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketListQuery {
    pub state: TicketStateSelector,
}

impl Default for TicketListQuery {
    fn default() -> Self {
        Self::active()
    }
}

impl TicketListQuery {
    pub fn active() -> Self {
        Self {
            state: TicketStateSelector::Active,
        }
    }

    pub fn all() -> Self {
        Self {
            state: TicketStateSelector::All,
        }
    }

    pub fn state(state: TicketListState) -> Self {
        Self::states([state])
    }

    pub fn states(states: impl IntoIterator<Item = TicketListState>) -> Self {
        Self {
            state: TicketStateSelector::States(states.into_iter().collect()),
        }
    }

    pub fn matches_state(&self, state: TicketWorkflowState) -> bool {
        match &self.state {
            TicketStateSelector::Active => state != TicketWorkflowState::Closed,
            TicketStateSelector::All => true,
            TicketStateSelector::States(states) => states
                .iter()
                .any(|query_state| query_state.matches_workflow_state(state)),
        }
    }

    pub fn state_filter_label(&self) -> String {
        match &self.state {
            TicketStateSelector::Active => "active".to_string(),
            TicketStateSelector::All => "all".to_string(),
            TicketStateSelector::States(states) => states
                .iter()
                .map(|state| state.as_str())
                .collect::<Vec<_>>()
                .join(","),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketRef {
    pub id: String,
    pub slug: String,
    pub status: TicketStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketRelationKind {
    DependsOn,
    Blocks,
    Related,
    Supersedes,
    DuplicateOf,
}

impl TicketRelationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DependsOn => "depends_on",
            Self::Blocks => "blocks",
            Self::Related => "related",
            Self::Supersedes => "supersedes",
            Self::DuplicateOf => "duplicate_of",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "depends_on" => Some(Self::DependsOn),
            "blocks" => Some(Self::Blocks),
            "related" => Some(Self::Related),
            "supersedes" => Some(Self::Supersedes),
            "duplicate_of" => Some(Self::DuplicateOf),
            _ => None,
        }
    }
}

impl fmt::Display for TicketRelationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewTicketRelation {
    pub kind: TicketRelationKind,
    pub target: String,
    pub note: Option<String>,
    pub author: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TicketRelation {
    pub ticket_id: String,
    pub kind: TicketRelationKind,
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub author: String,
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TicketRelationArtifact {
    version: u32,
    #[serde(default)]
    relations: Vec<TicketRelation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DerivedTicketRelation {
    pub source_ticket: String,
    pub inverse_kind: String,
    pub forward_kind: TicketRelationKind,
    pub note: Option<String>,
    pub author: String,
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketRelationBlocker {
    pub blocking_ticket: String,
    pub reason_kind: String,
    pub relation_kind: TicketRelationKind,
    pub note: Option<String>,
    pub blocking_state: TicketWorkflowState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketRelationNotice {
    pub related_ticket: String,
    pub kind: TicketRelationKind,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketRelationView {
    pub outgoing: Vec<TicketRelation>,
    pub incoming: Vec<DerivedTicketRelation>,
    pub blockers: Vec<TicketRelationBlocker>,
    pub notices: Vec<TicketRelationNotice>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrchestrationPlanKind {
    Before,
    After,
    BlockedBy,
    Blocks,
    ConflictsWith,
    DoNotParallelize,
    WaitingCapacityNote,
    AcceptedPlan,
}

impl OrchestrationPlanKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Before => "before",
            Self::After => "after",
            Self::BlockedBy => "blocked_by",
            Self::Blocks => "blocks",
            Self::ConflictsWith => "conflicts_with",
            Self::DoNotParallelize => "do_not_parallelize",
            Self::WaitingCapacityNote => "waiting_capacity_note",
            Self::AcceptedPlan => "accepted_plan",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "before" => Some(Self::Before),
            "after" => Some(Self::After),
            "blocked_by" => Some(Self::BlockedBy),
            "blocks" => Some(Self::Blocks),
            "conflicts_with" => Some(Self::ConflictsWith),
            "do_not_parallelize" => Some(Self::DoNotParallelize),
            "waiting_capacity_note" => Some(Self::WaitingCapacityNote),
            "accepted_plan" => Some(Self::AcceptedPlan),
            _ => None,
        }
    }

    fn requires_related_ticket(self) -> bool {
        matches!(
            self,
            Self::Before
                | Self::After
                | Self::BlockedBy
                | Self::Blocks
                | Self::ConflictsWith
                | Self::DoNotParallelize
        )
    }
}

impl fmt::Display for OrchestrationPlanKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptedOrchestrationPlan {
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_plan: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewOrchestrationPlanRecord {
    pub kind: OrchestrationPlanKind,
    pub related_ticket: Option<String>,
    pub note: Option<String>,
    pub accepted_plan: Option<AcceptedOrchestrationPlan>,
    pub author: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrchestrationPlanRecord {
    pub id: String,
    pub ticket_id: String,
    pub kind: OrchestrationPlanKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub related_ticket: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted_plan: Option<AcceptedOrchestrationPlan>,
    pub author: String,
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketMeta {
    pub id: String,
    pub slug: String,
    pub title: String,
    pub status: ExtensibleTicketStatus,
    pub kind: String,
    pub priority: String,
    pub labels: Vec<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub assignee: Option<String>,
    pub readiness: Option<String>,
    pub risk_flags: Vec<String>,
    pub workflow_state: TicketWorkflowState,
    pub workflow_state_explicit: bool,
    pub queued_by: Option<String>,
    pub queued_at: Option<String>,
    pub raw: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketSummary {
    pub id: String,
    pub slug: String,
    pub title: String,
    pub status: ExtensibleTicketStatus,
    pub kind: String,
    pub priority: String,
    pub labels: Vec<String>,
    pub readiness: Option<String>,
    pub workflow_state: TicketWorkflowState,
    pub workflow_state_explicit: bool,
    pub queued_by: Option<String>,
    pub queued_at: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketInvalidRecord {
    pub label: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketPartialList {
    pub tickets: Vec<TicketSummary>,
    pub invalid_records: Vec<TicketInvalidRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketPartial {
    pub ticket: Ticket,
    pub invalid_records: Vec<TicketInvalidRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketDocument {
    pub body: MarkdownText,
    pub raw_frontmatter: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketEvent {
    pub kind: TicketEventKind,
    pub author: Option<String>,
    pub at: Option<String>,
    pub status: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub reason: Option<String>,
    pub state_field: Option<String>,
    pub heading: Option<String>,
    pub body: MarkdownText,
    pub references: Vec<TicketReference>,
    pub attributes: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketArtifactRef {
    /// Path relative to the ticket's `artifacts/` directory.
    pub relative_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ticket {
    pub meta: TicketMeta,
    pub document: TicketDocument,
    pub events: Vec<TicketEvent>,
    pub artifacts: Vec<TicketArtifactRef>,
    pub relations: TicketRelationView,
    pub resolution: Option<MarkdownText>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TicketDoctorSeverity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketDoctorDiagnostic {
    pub severity: TicketDoctorSeverity,
    pub message: String,
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketDoctorReport {
    pub diagnostics: Vec<TicketDoctorDiagnostic>,
}

impl TicketDoctorReport {
    pub fn is_ok(&self) -> bool {
        self.error_count() == 0
    }

    pub fn error_count(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == TicketDoctorSeverity::Error)
            .count()
    }

    pub fn push_error(&mut self, message: impl Into<String>, path: Option<PathBuf>) {
        self.diagnostics.push(TicketDoctorDiagnostic {
            severity: TicketDoctorSeverity::Error,
            message: message.into(),
            path,
        });
    }

    pub fn push_warning(&mut self, message: impl Into<String>, path: Option<PathBuf>) {
        self.diagnostics.push(TicketDoctorDiagnostic {
            severity: TicketDoctorSeverity::Warning,
            message: message.into(),
            path,
        });
    }
}

pub trait TicketBackend {
    fn default_intake_ready_state_change_body(&self, from: &str) -> String;
    fn list(&self, filter: TicketListQuery) -> Result<Vec<TicketSummary>>;
    fn show(&self, id: TicketIdOrSlug) -> Result<Ticket>;
    fn create(&self, input: NewTicket) -> Result<TicketRef>;
    fn add_event(&self, id: TicketIdOrSlug, event: NewTicketEvent) -> Result<()>;
    fn add_state_changed(&self, id: TicketIdOrSlug, change: TicketStateChange) -> Result<()>;
    fn add_intake_summary(&self, id: TicketIdOrSlug, summary: TicketIntakeSummary) -> Result<()>;
    fn set_state_field(
        &self,
        id: TicketIdOrSlug,
        field: &str,
        change: TicketStateChange,
    ) -> Result<()>;
    fn set_workflow_state(&self, id: TicketIdOrSlug, change: TicketStateChange) -> Result<()>;
    fn mark_intake_ready(
        &self,
        id: TicketIdOrSlug,
        summary: TicketIntakeSummary,
        change: TicketStateChange,
    ) -> Result<()>;
    fn queue_ready(&self, id: TicketIdOrSlug, queued_by: &str) -> Result<()>;
    fn review(&self, id: TicketIdOrSlug, review: TicketReview) -> Result<()>;
    fn close(&self, id: TicketIdOrSlug, resolution: MarkdownText) -> Result<()>;
    fn add_ticket_relation(
        &self,
        id: TicketIdOrSlug,
        relation: NewTicketRelation,
    ) -> Result<TicketRelation>;
    fn query_ticket_relations(
        &self,
        ticket: Option<TicketIdOrSlug>,
        kind: Option<TicketRelationKind>,
    ) -> Result<Vec<TicketRelation>>;
    fn relation_view(&self, id: TicketIdOrSlug) -> Result<TicketRelationView>;
    fn add_orchestration_plan_record(
        &self,
        id: TicketIdOrSlug,
        record: NewOrchestrationPlanRecord,
    ) -> Result<OrchestrationPlanRecord>;
    fn query_orchestration_plan_records(
        &self,
        ticket: Option<TicketIdOrSlug>,
        kind: Option<OrchestrationPlanKind>,
    ) -> Result<Vec<OrchestrationPlanRecord>>;
    fn doctor(&self) -> Result<TicketDoctorReport>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum TicketBackendOperation {
    DefaultIntakeReadyStateChangeBody {
        from: String,
    },
    List {
        filter: TicketListQuery,
    },
    Show {
        id: TicketIdOrSlug,
    },
    Create {
        input: NewTicket,
    },
    AddEvent {
        id: TicketIdOrSlug,
        event: NewTicketEvent,
    },
    AddStateChanged {
        id: TicketIdOrSlug,
        change: TicketStateChange,
    },
    AddIntakeSummary {
        id: TicketIdOrSlug,
        summary: TicketIntakeSummary,
    },
    SetStateField {
        id: TicketIdOrSlug,
        field: String,
        change: TicketStateChange,
    },
    SetWorkflowState {
        id: TicketIdOrSlug,
        change: TicketStateChange,
    },
    MarkIntakeReady {
        id: TicketIdOrSlug,
        summary: TicketIntakeSummary,
        change: TicketStateChange,
    },
    QueueReady {
        id: TicketIdOrSlug,
        queued_by: String,
    },
    Review {
        id: TicketIdOrSlug,
        review: TicketReview,
    },
    Close {
        id: TicketIdOrSlug,
        resolution: MarkdownText,
    },
    AddTicketRelation {
        id: TicketIdOrSlug,
        relation: NewTicketRelation,
    },
    QueryTicketRelations {
        ticket: Option<TicketIdOrSlug>,
        kind: Option<TicketRelationKind>,
    },
    RelationView {
        id: TicketIdOrSlug,
    },
    AddOrchestrationPlanRecord {
        id: TicketIdOrSlug,
        record: NewOrchestrationPlanRecord,
    },
    QueryOrchestrationPlanRecords {
        ticket: Option<TicketIdOrSlug>,
        kind: Option<OrchestrationPlanKind>,
    },
    Doctor,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "result", content = "value", rename_all = "snake_case")]
pub enum TicketBackendOperationResult {
    Text(String),
    Unit,
    Tickets(Vec<TicketSummary>),
    Ticket(Ticket),
    TicketRef(TicketRef),
    Relation(TicketRelation),
    Relations(Vec<TicketRelation>),
    RelationView(TicketRelationView),
    OrchestrationPlanRecord(OrchestrationPlanRecord),
    OrchestrationPlanRecords(Vec<OrchestrationPlanRecord>),
    DoctorReport(TicketDoctorReport),
}

pub fn execute_ticket_backend_operation<B>(
    backend: &B,
    operation: TicketBackendOperation,
) -> Result<TicketBackendOperationResult>
where
    B: TicketBackend + ?Sized,
{
    Ok(match operation {
        TicketBackendOperation::DefaultIntakeReadyStateChangeBody { from } => {
            TicketBackendOperationResult::Text(
                backend.default_intake_ready_state_change_body(&from),
            )
        }
        TicketBackendOperation::List { filter } => {
            TicketBackendOperationResult::Tickets(backend.list(filter)?)
        }
        TicketBackendOperation::Show { id } => {
            TicketBackendOperationResult::Ticket(backend.show(id)?)
        }
        TicketBackendOperation::Create { input } => {
            TicketBackendOperationResult::TicketRef(backend.create(input)?)
        }
        TicketBackendOperation::AddEvent { id, event } => {
            backend.add_event(id, event)?;
            TicketBackendOperationResult::Unit
        }
        TicketBackendOperation::AddStateChanged { id, change } => {
            backend.add_state_changed(id, change)?;
            TicketBackendOperationResult::Unit
        }
        TicketBackendOperation::AddIntakeSummary { id, summary } => {
            backend.add_intake_summary(id, summary)?;
            TicketBackendOperationResult::Unit
        }
        TicketBackendOperation::SetStateField { id, field, change } => {
            backend.set_state_field(id, &field, change)?;
            TicketBackendOperationResult::Unit
        }
        TicketBackendOperation::SetWorkflowState { id, change } => {
            backend.set_workflow_state(id, change)?;
            TicketBackendOperationResult::Unit
        }
        TicketBackendOperation::MarkIntakeReady {
            id,
            summary,
            change,
        } => {
            backend.mark_intake_ready(id, summary, change)?;
            TicketBackendOperationResult::Unit
        }
        TicketBackendOperation::QueueReady { id, queued_by } => {
            backend.queue_ready(id, &queued_by)?;
            TicketBackendOperationResult::Unit
        }
        TicketBackendOperation::Review { id, review } => {
            backend.review(id, review)?;
            TicketBackendOperationResult::Unit
        }
        TicketBackendOperation::Close { id, resolution } => {
            backend.close(id, resolution)?;
            TicketBackendOperationResult::Unit
        }
        TicketBackendOperation::AddTicketRelation { id, relation } => {
            TicketBackendOperationResult::Relation(backend.add_ticket_relation(id, relation)?)
        }
        TicketBackendOperation::QueryTicketRelations { ticket, kind } => {
            TicketBackendOperationResult::Relations(backend.query_ticket_relations(ticket, kind)?)
        }
        TicketBackendOperation::RelationView { id } => {
            TicketBackendOperationResult::RelationView(backend.relation_view(id)?)
        }
        TicketBackendOperation::AddOrchestrationPlanRecord { id, record } => {
            TicketBackendOperationResult::OrchestrationPlanRecord(
                backend.add_orchestration_plan_record(id, record)?,
            )
        }
        TicketBackendOperation::QueryOrchestrationPlanRecords { ticket, kind } => {
            TicketBackendOperationResult::OrchestrationPlanRecords(
                backend.query_orchestration_plan_records(ticket, kind)?,
            )
        }
        TicketBackendOperation::Doctor => {
            TicketBackendOperationResult::DoctorReport(backend.doctor()?)
        }
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TicketBackendHttpResponse {
    Ok {
        result: TicketBackendOperationResult,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone)]
pub struct LocalTicketBackend {
    root: PathBuf,
    record_language: Option<String>,
}

impl LocalTicketBackend {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            record_language: None,
        }
    }

    pub fn with_record_language(mut self, language: Option<&str>) -> Self {
        self.record_language = language.and_then(normalized_record_language);
        self
    }

    pub fn record_language(&self) -> Option<&str> {
        self.record_language.as_deref()
    }

    pub fn root(&self) -> &Path {
        self.root.as_path()
    }

    pub fn default_intake_ready_state_change_body(&self, from: &str) -> String {
        if is_japanese_record_language(self.record_language()) {
            format!("Ticket planning が完了しました。state {from} -> ready。\n")
        } else {
            format!("Ticket planning complete; state {from} -> ready.\n")
        }
    }

    pub fn list_partial(&self, filter: TicketListQuery) -> Result<TicketPartialList> {
        let mut output = TicketPartialList::default();
        let mut invalid_seen = BTreeSet::new();
        for dir in self.iter_ticket_dirs(TicketListQuery::all())? {
            let item = dir.join("item.md");
            if !item.exists() {
                continue;
            }
            match read_item_file(&item)
                .and_then(|parsed| ticket_meta_for_dir(&dir, parsed.frontmatter))
            {
                Ok(meta) => {
                    if !filter.matches_state(meta.workflow_state) {
                        continue;
                    }
                    output.tickets.push(ticket_summary_from_meta(meta));
                }
                Err(error) => push_invalid_ticket_record(
                    &mut output.invalid_records,
                    &mut invalid_seen,
                    &dir,
                    &error,
                ),
            }
        }
        Ok(output)
    }

    pub fn show_partial(&self, id: TicketIdOrSlug) -> Result<TicketPartial> {
        let dir = self.find_ticket_dir(&id)?;
        let mut invalid_records = Vec::new();
        let mut invalid_seen = BTreeSet::new();
        let ticket =
            self.ticket_from_dir_tolerant(&dir, &mut invalid_records, &mut invalid_seen)?;
        Ok(TicketPartial {
            ticket,
            invalid_records,
        })
    }

    fn generated_heading(&self, default: &'static str, japanese: &'static str) -> &'static str {
        if is_japanese_record_language(self.record_language()) {
            japanese
        } else {
            default
        }
    }

    fn generated_default_body(&self) -> &'static str {
        if is_japanese_record_language(self.record_language()) {
            JAPANESE_TICKET_BODY
        } else {
            DEFAULT_TICKET_BODY
        }
    }

    fn created_event_body(&self) -> &'static str {
        if is_japanese_record_language(self.record_language()) {
            "LocalTicketBackend によって作成されました。"
        } else {
            "Created by LocalTicketBackend create."
        }
    }

    fn queued_ready_body(&self, queued_by: &str) -> String {
        if is_japanese_record_language(self.record_language()) {
            format!("Ticket を `{queued_by}` が queued にしました。\n")
        } else {
            "Ticket queued for Orchestrator routing.\n".to_string()
        }
    }

    fn closed_workflow_state_body(&self) -> &'static str {
        if is_japanese_record_language(self.record_language()) {
            "Ticket を closed にしました。\n"
        } else {
            "Ticket closed.\n"
        }
    }

    fn ensure_backend_dirs(&self) -> Result<()> {
        fs::create_dir_all(&self.root).map_err(|e| io_err(&self.root, e))
    }

    fn ticket_dir(&self, id: &str) -> Result<PathBuf> {
        ensure_safe_component(id)?;
        let dir = self.root.join(id);
        ensure_child_of(&self.root, &dir)?;
        Ok(dir)
    }

    fn acquire_lock(&self) -> Result<BackendLock> {
        fs::create_dir_all(&self.root).map_err(|e| io_err(&self.root, e))?;
        let path = self.root.join(".ticket-backend.lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|e| io_err(&path, e))?;
        match FileExt::try_lock_exclusive(&file) {
            Ok(true) => Ok(BackendLock { file }),
            Ok(false) => Err(TicketError::Locked { path }),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Err(TicketError::Locked { path }),
            Err(e) => Err(io_err(path, e)),
        }
    }

    fn iter_ticket_dirs(&self, filter: TicketListQuery) -> Result<Vec<PathBuf>> {
        let mut dirs = Vec::new();
        if !self.root.exists() {
            return Ok(dirs);
        }
        let entries = fs::read_dir(&self.root).map_err(|e| io_err(&self.root, e))?;
        for entry in entries {
            let entry = entry.map_err(|e| io_err(&self.root, e))?;
            let path = entry.path();
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if !path.is_dir() || name.starts_with('.') {
                continue;
            }
            let item = path.join("item.md");
            if !item.is_file() {
                continue;
            }
            if !matches!(filter.state, TicketStateSelector::All) {
                let parsed = read_item_file(&item)?;
                let meta = ticket_meta_for_dir(&path, parsed.frontmatter)?;
                if !filter.matches_state(meta.workflow_state) {
                    continue;
                }
            }
            dirs.push(path);
        }
        dirs.sort();
        Ok(dirs)
    }

    fn find_ticket_dir(&self, query: &TicketIdOrSlug) -> Result<PathBuf> {
        let query = query.as_query();
        let dir = self.ticket_dir(query)?;
        if dir.join("item.md").is_file() {
            Ok(dir)
        } else {
            Err(TicketError::NotFound(query.to_string()))
        }
    }

    fn ticket_from_dir(&self, dir: &Path) -> Result<Ticket> {
        self.ticket_from_dir_with_relations(dir, |backend, meta| {
            backend.relation_view_for_meta(meta)
        })
    }

    fn ticket_from_dir_tolerant(
        &self,
        dir: &Path,
        invalid_records: &mut Vec<TicketInvalidRecord>,
        invalid_seen: &mut BTreeSet<String>,
    ) -> Result<Ticket> {
        self.ticket_from_dir_with_relations(dir, |backend, meta| {
            backend.relation_view_for_meta_tolerant(meta, invalid_records, invalid_seen)
        })
    }

    fn ticket_from_dir_with_relations(
        &self,
        dir: &Path,
        relation_view: impl FnOnce(&Self, &TicketMeta) -> Result<TicketRelationView>,
    ) -> Result<Ticket> {
        let item_path = dir.join("item.md");
        let parsed = read_item_file(&item_path)?;
        let meta = ticket_meta_for_dir(dir, parsed.frontmatter.clone())?;
        let document = TicketDocument {
            body: MarkdownText::new(parsed.body),
            raw_frontmatter: parsed.frontmatter.raw,
        };
        let thread_path = dir.join("thread.md");
        let events = if thread_path.exists() {
            parse_thread(&thread_path)?
        } else {
            Vec::new()
        };
        let artifacts = collect_artifacts(&dir.join("artifacts"))?;
        let relations = relation_view(self, &meta)?;
        let resolution_path = dir.join("resolution.md");
        let resolution = if resolution_path.exists() {
            Some(MarkdownText::new(
                fs::read_to_string(&resolution_path).map_err(|e| io_err(&resolution_path, e))?,
            ))
        } else {
            None
        };
        Ok(Ticket {
            meta,
            document,
            events,
            artifacts,
            relations,
            resolution,
        })
    }

    fn ticket_workflow_state_from_dir(&self, dir: &Path) -> Result<TicketWorkflowState> {
        let item = dir.join("item.md");
        let parsed = read_item_file(&item)?;
        let meta = ticket_meta_for_dir(dir, parsed.frontmatter)?;
        Ok(meta.workflow_state)
    }

    fn apply_workflow_state_change(
        &self,
        dir: &Path,
        expected_from: TicketWorkflowState,
        to: TicketWorkflowState,
        change: TicketStateChange,
        extra_updates: &[(&str, &str)],
    ) -> Result<()> {
        validate_state_change(&change)?;
        if change.from.as_str() != expected_from.as_str() || change.to.as_str() != to.as_str() {
            return Err(TicketError::Conflict(format!(
                "workflow_state change payload mismatch: expected {} -> {}, got {} -> {}",
                expected_from.as_str(),
                to.as_str(),
                change.from,
                change.to
            )));
        }
        let item = dir.join("item.md");
        let current = self.ticket_workflow_state_from_dir(dir)?;
        if current != expected_from {
            return Err(TicketError::Conflict(format!(
                "state changed concurrently: expected `{}`, found `{}`",
                expected_from.as_str(),
                current.as_str()
            )));
        }
        self.append_state_changed_event(dir, &change, Some("state"))?;
        let mut updates = vec![("state", to.as_str())];
        updates.extend_from_slice(extra_updates);
        self.set_frontmatter_fields(&item, &updates)
    }

    fn append_thread_event(
        &self,
        dir: &Path,
        event: &str,
        heading: &str,
        author: &str,
        status: Option<&str>,
        attrs: &[(&str, &str)],
        body: &MarkdownText,
    ) -> Result<()> {
        let at = now_utc();
        let mut event_attrs = vec![("event", event), ("author", author), ("at", at.as_str())];
        if let Some(status) = status {
            event_attrs.push(("status", status));
        }
        event_attrs.extend_from_slice(attrs);
        let comment = render_event_comment(&event_attrs)?;
        let entry = format!("\n{comment}\n\n## {heading}\n\n{}\n\n---\n", body.as_str());

        let thread = dir.join("thread.md");
        if !thread.exists() {
            File::create(&thread).map_err(|e| io_err(&thread, e))?;
        }
        let mut file = OpenOptions::new()
            .append(true)
            .open(&thread)
            .map_err(|e| io_err(&thread, e))?;
        file.write_all(entry.as_bytes())
            .map_err(|e| io_err(&thread, e))?;
        file.sync_data().map_err(|e| io_err(&thread, e))?;
        self.set_frontmatter_fields(&dir.join("item.md"), &[("updated_at", at.as_str())])
    }

    fn append_state_changed_event(
        &self,
        dir: &Path,
        change: &TicketStateChange,
        state_field: Option<&str>,
    ) -> Result<()> {
        validate_state_change(change)?;
        let author = change.author.clone().unwrap_or_else(default_author);
        let mut attrs = vec![
            ("from", change.from.as_str()),
            ("to", change.to.as_str()),
            ("reason", change.reason.as_str()),
        ];
        if let Some(state_field) = state_field {
            attrs.push(("field", state_field));
        }
        self.append_thread_event(
            dir,
            TicketEventKind::StateChanged.as_str(),
            &TicketEventKind::StateChanged.heading(),
            &author,
            None,
            &attrs,
            &change.body,
        )
    }

    fn append_intake_summary_event(&self, dir: &Path, summary: &TicketIntakeSummary) -> Result<()> {
        validate_intake_summary(summary)?;
        let author = summary.author.clone().unwrap_or_else(default_author);
        self.append_thread_event(
            dir,
            TicketEventKind::IntakeSummary.as_str(),
            &TicketEventKind::IntakeSummary.heading(),
            &author,
            None,
            &[],
            &summary.body,
        )
    }

    fn set_frontmatter_fields(&self, item: &Path, updates: &[(&str, &str)]) -> Result<()> {
        let content = fs::read_to_string(item).map_err(|e| io_err(item, e))?;
        let updated = replace_frontmatter_fields(&content, updates).map_err(|message| {
            TicketError::Parse {
                path: item.to_path_buf(),
                message,
            }
        })?;
        atomic_write(item, updated.as_bytes())
    }

    fn orchestration_plan_path(&self, dir: &Path) -> PathBuf {
        dir.join("artifacts").join(ORCHESTRATION_PLAN_ARTIFACT)
    }

    fn ticket_relations_path(&self, dir: &Path) -> PathBuf {
        dir.join("artifacts").join(TICKET_RELATIONS_ARTIFACT)
    }

    fn read_ticket_relations_for_dir(&self, dir: &Path) -> Result<Vec<TicketRelation>> {
        let item = dir.join("item.md");
        let meta = ticket_meta_for_dir(dir, read_item_file(&item)?.frontmatter)?;
        let path = self.ticket_relations_path(dir);
        read_ticket_relations_artifact(&path, Some(&meta))
    }

    fn all_ticket_relation_records(&self) -> Result<Vec<TicketRelation>> {
        let mut relations = Vec::new();
        for dir in self.iter_ticket_dirs(TicketListQuery::all())? {
            relations.extend(self.read_ticket_relations_for_dir(&dir)?);
        }
        sort_ticket_relations(&mut relations);
        Ok(relations)
    }

    fn all_ticket_relation_records_tolerant(
        &self,
        invalid_records: &mut Vec<TicketInvalidRecord>,
        invalid_seen: &mut BTreeSet<String>,
    ) -> Result<Vec<TicketRelation>> {
        let mut relations = Vec::new();
        for dir in self.iter_ticket_dirs(TicketListQuery::all())? {
            match self.read_ticket_relations_for_dir(&dir) {
                Ok(records) => relations.extend(records),
                Err(error) => {
                    push_invalid_ticket_record(invalid_records, invalid_seen, &dir, &error)
                }
            }
        }
        sort_ticket_relations(&mut relations);
        Ok(relations)
    }

    fn relation_view_for_meta(&self, meta: &TicketMeta) -> Result<TicketRelationView> {
        let states = self.ticket_state_index()?;
        let all = self.all_ticket_relation_records()?;
        Ok(relation_view_from_records(meta, &all, &states))
    }

    fn relation_view_for_meta_tolerant(
        &self,
        meta: &TicketMeta,
        invalid_records: &mut Vec<TicketInvalidRecord>,
        invalid_seen: &mut BTreeSet<String>,
    ) -> Result<TicketRelationView> {
        let states = self.ticket_state_index_tolerant(invalid_records, invalid_seen)?;
        let all = self.all_ticket_relation_records_tolerant(invalid_records, invalid_seen)?;
        Ok(relation_view_from_records(meta, &all, &states))
    }

    fn ticket_state_index(&self) -> Result<HashMap<String, TicketWorkflowState>> {
        let mut states = HashMap::new();
        for dir in self.iter_ticket_dirs(TicketListQuery::all())? {
            let item = dir.join("item.md");
            let meta = ticket_meta_for_dir(&dir, read_item_file(&item)?.frontmatter)?;
            states.insert(meta.id, meta.workflow_state);
        }
        Ok(states)
    }

    fn ticket_state_index_tolerant(
        &self,
        invalid_records: &mut Vec<TicketInvalidRecord>,
        invalid_seen: &mut BTreeSet<String>,
    ) -> Result<HashMap<String, TicketWorkflowState>> {
        let mut states = HashMap::new();
        for dir in self.iter_ticket_dirs(TicketListQuery::all())? {
            let item = dir.join("item.md");
            match read_item_file(&item)
                .and_then(|parsed| ticket_meta_for_dir(&dir, parsed.frontmatter))
            {
                Ok(meta) => {
                    states.insert(meta.id, meta.workflow_state);
                }
                Err(error) => {
                    push_invalid_ticket_record(invalid_records, invalid_seen, &dir, &error)
                }
            }
        }
        Ok(states)
    }

    fn relation_blockers_for_meta(&self, meta: &TicketMeta) -> Result<Vec<TicketRelationBlocker>> {
        Ok(self.relation_view_for_meta(meta)?.blockers)
    }

    fn read_orchestration_plan_records_for_dir(
        &self,
        dir: &Path,
    ) -> Result<Vec<OrchestrationPlanRecord>> {
        let item = dir.join("item.md");
        let meta = ticket_meta_for_dir(dir, read_item_file(&item)?.frontmatter)?;
        let path = self.orchestration_plan_path(dir);
        read_orchestration_plan_artifact(&path, Some(&meta))
    }
}

impl TicketBackend for LocalTicketBackend {
    fn default_intake_ready_state_change_body(&self, from: &str) -> String {
        self.default_intake_ready_state_change_body(from)
    }

    fn list(&self, filter: TicketListQuery) -> Result<Vec<TicketSummary>> {
        let mut tickets = Vec::new();
        for dir in self.iter_ticket_dirs(filter)? {
            let item = dir.join("item.md");
            if !item.exists() {
                continue;
            }
            let parsed = read_item_file(&item)?;
            let meta = ticket_meta_for_dir(&dir, parsed.frontmatter)?;
            tickets.push(ticket_summary_from_meta(meta));
        }
        Ok(tickets)
    }

    fn show(&self, id: TicketIdOrSlug) -> Result<Ticket> {
        let dir = self.find_ticket_dir(&id)?;
        self.ticket_from_dir(&dir)
    }

    fn create(&self, input: NewTicket) -> Result<TicketRef> {
        let _lock = self.acquire_lock()?;
        self.ensure_backend_dirs()?;
        if input.title.trim().is_empty() {
            return Err(TicketError::Conflict(
                "ticket title must not be empty".to_string(),
            ));
        }
        let base_millis = unix_epoch_millis_now().map_err(|err| {
            TicketError::Conflict(format!("failed to read ticket id timestamp: {err}"))
        })?;
        let id = allocate_record_id(base_millis, |candidate| match self.ticket_dir(candidate) {
            Ok(dir) => dir.exists(),
            Err(_) => true,
        })
        .map_err(|err| {
            TicketError::Conflict(format!("failed to allocate unique ticket id: {err}"))
        })?;
        let dir = self.ticket_dir(&id)?;
        let created = now_utc();
        let author = input
            .author
            .unwrap_or_else(|| "LocalTicketBackend".to_string());
        let create_comment = render_event_comment(&[
            ("event", TicketEventKind::Create.as_str()),
            ("author", &author),
            ("at", &created),
        ])?;

        fs::create_dir_all(dir.join("artifacts")).map_err(|e| io_err(&dir, e))?;
        atomic_write(&dir.join("artifacts/.gitkeep"), b"")?;
        let mut fields = Vec::new();
        fields.push((
            "title".to_string(),
            format_yaml_string_scalar(input.title.as_str()),
        ));
        fields.push((
            "state".to_string(),
            format_yaml_string_scalar(
                input
                    .workflow_state
                    .unwrap_or(TicketWorkflowState::Planning)
                    .as_str(),
            ),
        ));
        fields.push((
            "created_at".to_string(),
            format_yaml_string_scalar(&created),
        ));
        fields.push((
            "updated_at".to_string(),
            format_yaml_string_scalar(&created),
        ));
        fields.push((
            "assignee".to_string(),
            yaml_string_or_null(input.assignee.as_deref()),
        ));
        if let Some(readiness) = input.readiness {
            fields.push((
                "readiness".to_string(),
                format_yaml_string_scalar(readiness.as_str()),
            ));
        }
        if !input.risk_flags.is_empty() {
            fields.push(("risk_flags".to_string(), labels_yaml(&input.risk_flags)));
        }
        if let Some(queued_by) = input.queued_by {
            fields.push((
                "queued_by".to_string(),
                format_yaml_string_scalar(queued_by.as_str()),
            ));
        }
        if let Some(queued_at) = input.queued_at {
            fields.push((
                "queued_at".to_string(),
                format_yaml_string_scalar(queued_at.as_str()),
            ));
        }
        let item_body = if input.body.as_str() == DEFAULT_TICKET_BODY {
            self.generated_default_body()
        } else {
            input.body.as_str()
        };
        let item = serialize_item(&fields, item_body);
        atomic_write(&dir.join("item.md"), item.as_bytes())?;
        let thread = format!(
            "{create_comment}\n\n## {}\n\n{}\n\n---\n",
            self.generated_heading("Created", "作成"),
            self.created_event_body()
        );
        atomic_write(&dir.join("thread.md"), thread.as_bytes())?;
        Ok(TicketRef {
            id: id.clone(),
            slug: id,
            status: TicketStatus::Open,
        })
    }

    fn add_event(&self, id: TicketIdOrSlug, event: NewTicketEvent) -> Result<()> {
        let _lock = self.acquire_lock()?;
        let dir = self.find_ticket_dir(&id)?;
        let author = event.author.unwrap_or_else(default_author);
        self.append_thread_event(
            &dir,
            event.kind.as_str(),
            &event.kind.heading(),
            &author,
            None,
            &[],
            &event.body,
        )
    }

    fn add_state_changed(&self, id: TicketIdOrSlug, change: TicketStateChange) -> Result<()> {
        let _lock = self.acquire_lock()?;
        let dir = self.find_ticket_dir(&id)?;
        self.append_state_changed_event(&dir, &change, None)
    }

    fn add_intake_summary(&self, id: TicketIdOrSlug, summary: TicketIntakeSummary) -> Result<()> {
        let _lock = self.acquire_lock()?;
        let dir = self.find_ticket_dir(&id)?;
        self.append_intake_summary_event(&dir, &summary)
    }

    fn set_state_field(
        &self,
        id: TicketIdOrSlug,
        field: &str,
        change: TicketStateChange,
    ) -> Result<()> {
        validate_state_field_name(field)?;
        if field == "state" || field == "workflow_state" || field == "status" {
            return Err(TicketError::Conflict(
                "ticket lifecycle state transitions must use dedicated lifecycle APIs".to_string(),
            ));
        }
        let _lock = self.acquire_lock()?;
        let dir = self.find_ticket_dir(&id)?;
        let item = dir.join("item.md");
        let parsed = read_item_file(&item)?;
        let current = parsed
            .frontmatter
            .get(field)
            .map(String::as_str)
            .unwrap_or("");
        if current != change.from.as_str() {
            return Err(TicketError::Conflict(format!(
                "state field `{field}` changed concurrently: expected `{}`, found `{current}`",
                change.from
            )));
        }
        self.append_state_changed_event(&dir, &change, Some(field))?;
        self.set_frontmatter_fields(&item, &[(field, change.to.as_str())])
    }

    fn set_workflow_state(&self, id: TicketIdOrSlug, change: TicketStateChange) -> Result<()> {
        let from = TicketWorkflowState::parse(&change.from).ok_or_else(|| {
            TicketError::Conflict(format!(
                "invalid workflow_state transition source: {}",
                change.from
            ))
        })?;
        let to = TicketWorkflowState::parse(&change.to).ok_or_else(|| {
            TicketError::Conflict(format!(
                "invalid workflow_state transition target: {}",
                change.to
            ))
        })?;
        if !TicketWorkflowState::is_role_transition(from, to) {
            return Err(TicketError::Conflict(format!(
                "workflow_state transition {} -> {} is not allowed through set_workflow_state; use dedicated planning-ready or queue APIs for gated transitions",
                from.as_str(),
                to.as_str()
            )));
        }
        let _lock = self.acquire_lock()?;
        let dir = self.find_ticket_dir(&id)?;
        if from == TicketWorkflowState::Queued && to == TicketWorkflowState::InProgress {
            let item = dir.join("item.md");
            let meta = ticket_meta_for_dir(&dir, read_item_file(&item)?.frontmatter)?;
            let blockers = self.relation_blockers_for_meta(&meta)?;
            if !blockers.is_empty() {
                return Err(TicketError::Conflict(format!(
                    "ticket {} has unresolved blocking relation(s): {}",
                    meta.id,
                    format_relation_blockers(&blockers)
                )));
            }
        }
        self.apply_workflow_state_change(&dir, from, to, change, &[])
    }

    fn mark_intake_ready(
        &self,
        id: TicketIdOrSlug,
        summary: TicketIntakeSummary,
        change: TicketStateChange,
    ) -> Result<()> {
        let from = TicketWorkflowState::parse(&change.from).ok_or_else(|| {
            TicketError::Conflict(format!(
                "invalid workflow_state transition source: {}",
                change.from
            ))
        })?;
        let to = TicketWorkflowState::parse(&change.to).ok_or_else(|| {
            TicketError::Conflict(format!(
                "invalid workflow_state transition target: {}",
                change.to
            ))
        })?;
        if !TicketWorkflowState::is_planning_ready_transition(from, to) {
            return Err(TicketError::Conflict(format!(
                "mark_intake_ready only allows state planning -> ready, got {} -> {}",
                from.as_str(),
                to.as_str()
            )));
        }
        let _lock = self.acquire_lock()?;
        let dir = self.find_ticket_dir(&id)?;
        let current = self.ticket_workflow_state_from_dir(&dir)?;
        if current != from {
            return Err(TicketError::Conflict(format!(
                "state changed concurrently: expected `{}`, found `{}`",
                from.as_str(),
                current.as_str()
            )));
        }
        self.append_intake_summary_event(&dir, &summary)?;
        self.apply_workflow_state_change(&dir, from, to, change, &[])
    }

    fn queue_ready(&self, id: TicketIdOrSlug, queued_by: &str) -> Result<()> {
        validate_required_event_value("queued_by", queued_by)?;
        let _lock = self.acquire_lock()?;
        let dir = self.find_ticket_dir(&id)?;
        let item = dir.join("item.md");
        let meta = ticket_meta_for_dir(&dir, read_item_file(&item)?.frontmatter)?;
        let blockers = self.relation_blockers_for_meta(&meta)?;
        let active_blockers = blockers
            .into_iter()
            .filter(|blocker| !relation_blocker_allows_queue(blocker))
            .collect::<Vec<_>>();
        if !active_blockers.is_empty() {
            return Err(TicketError::Conflict(format!(
                "ticket {} has unresolved blocking relation(s): {}",
                meta.id,
                format_relation_blockers(&active_blockers)
            )));
        }
        let at = now_utc();
        let mut change = TicketStateChange::new(
            TicketWorkflowState::Ready.as_str(),
            TicketWorkflowState::Queued.as_str(),
            "queued",
            self.queued_ready_body(queued_by),
        );
        change.author = Some(queued_by.to_string());
        self.apply_workflow_state_change(
            &dir,
            TicketWorkflowState::Ready,
            TicketWorkflowState::Queued,
            change,
            &[("queued_by", queued_by), ("queued_at", at.as_str())],
        )
    }

    fn review(&self, id: TicketIdOrSlug, review: TicketReview) -> Result<()> {
        let _lock = self.acquire_lock()?;
        let dir = self.find_ticket_dir(&id)?;
        let author = review.author.unwrap_or_else(default_author);
        self.append_thread_event(
            &dir,
            "review",
            &review.result.heading(),
            &author,
            Some(review.result.as_str()),
            &[],
            &review.body,
        )
    }

    fn close(&self, id: TicketIdOrSlug, resolution: MarkdownText) -> Result<()> {
        let _lock = self.acquire_lock()?;
        self.ensure_backend_dirs()?;
        let dir = self.find_ticket_dir(&id)?;
        let at = now_utc();
        let current_workflow_state = self.ticket_workflow_state_from_dir(&dir)?;
        if current_workflow_state != TicketWorkflowState::Closed {
            let mut change = TicketStateChange::new(
                current_workflow_state.as_str(),
                TicketWorkflowState::Closed.as_str(),
                "closed",
                self.closed_workflow_state_body(),
            );
            change.author = Some(default_author());
            self.append_state_changed_event(&dir, &change, Some("state"))?;
        }
        self.set_frontmatter_fields(
            &dir.join("item.md"),
            &[
                ("state", TicketWorkflowState::Closed.as_str()),
                ("updated_at", &at),
            ],
        )?;
        atomic_write(&dir.join("resolution.md"), resolution.as_str().as_bytes())?;
        let author = default_author();
        self.append_thread_event(
            &dir,
            "close",
            self.generated_heading("Closed", "完了"),
            &author,
            Some("closed"),
            &[],
            &resolution,
        )
    }

    fn add_ticket_relation(
        &self,
        id: TicketIdOrSlug,
        relation: NewTicketRelation,
    ) -> Result<TicketRelation> {
        validate_new_ticket_relation(&relation)?;
        let _lock = self.acquire_lock()?;
        self.ensure_backend_dirs()?;
        let dir = self.find_ticket_dir(&id)?;
        let item = dir.join("item.md");
        let meta = ticket_meta_for_dir(&dir, read_item_file(&item)?.frontmatter)?;
        if relation.target.trim() == meta.id {
            return Err(TicketError::Conflict(format!(
                "ticket relation cannot target itself: {}",
                meta.id
            )));
        }
        let target_id = relation.target.trim().to_string();
        let target_dir = self.ticket_dir(&target_id)?;
        if !target_dir.join("item.md").is_file() {
            return Err(TicketError::NotFound(target_id));
        }
        let artifacts = dir.join("artifacts");
        fs::create_dir_all(&artifacts).map_err(|e| io_err(&artifacts, e))?;
        let path = self.ticket_relations_path(&dir);
        ensure_child_of(&artifacts, &path)?;
        let mut relations = read_ticket_relations_artifact(&path, Some(&meta))?;
        if relations
            .iter()
            .any(|existing| existing.kind == relation.kind && existing.target == target_id)
        {
            return Err(TicketError::Conflict(format!(
                "ticket relation already exists: {} {} {}",
                meta.id, relation.kind, target_id
            )));
        }
        let at = now_utc();
        let output = TicketRelation {
            ticket_id: meta.id.clone(),
            kind: relation.kind,
            target: target_id,
            note: relation
                .note
                .map(trim_owned)
                .filter(|note| !note.is_empty()),
            author: relation
                .author
                .map(trim_owned)
                .unwrap_or_else(default_author),
            at: at.clone(),
        };
        validate_ticket_relation(&output, Some(&meta))?;
        relations.push(output.clone());
        relations.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.target.cmp(&b.target)));
        write_ticket_relations_artifact(&path, &relations)?;
        self.set_frontmatter_fields(&item, &[("updated_at", &at)])?;
        Ok(output)
    }

    fn query_ticket_relations(
        &self,
        ticket: Option<TicketIdOrSlug>,
        kind: Option<TicketRelationKind>,
    ) -> Result<Vec<TicketRelation>> {
        let mut relations = Vec::new();
        if let Some(ticket) = ticket {
            let dir = self.find_ticket_dir(&ticket)?;
            let source_id = ticket_id_from_dir(&dir)?;
            relations.extend(self.read_ticket_relations_for_dir(&dir)?);
            relations.extend(
                self.all_ticket_relation_records()?
                    .into_iter()
                    .filter(|relation| relation.target == source_id),
            );
        } else {
            relations.extend(self.all_ticket_relation_records()?);
        }
        if let Some(kind) = kind {
            relations.retain(|relation| relation.kind == kind);
        }
        relations.sort_by(|a, b| {
            a.ticket_id
                .cmp(&b.ticket_id)
                .then_with(|| a.kind.cmp(&b.kind))
                .then_with(|| a.target.cmp(&b.target))
                .then_with(|| a.at.cmp(&b.at))
        });
        relations.dedup_by(|a, b| {
            a.ticket_id == b.ticket_id && a.kind == b.kind && a.target == b.target && a.at == b.at
        });
        Ok(relations)
    }

    fn relation_view(&self, id: TicketIdOrSlug) -> Result<TicketRelationView> {
        let dir = self.find_ticket_dir(&id)?;
        let item = dir.join("item.md");
        let meta = ticket_meta_for_dir(&dir, read_item_file(&item)?.frontmatter)?;
        self.relation_view_for_meta(&meta)
    }

    fn add_orchestration_plan_record(
        &self,
        id: TicketIdOrSlug,
        record: NewOrchestrationPlanRecord,
    ) -> Result<OrchestrationPlanRecord> {
        validate_new_orchestration_plan_record(&record)?;
        let _lock = self.acquire_lock()?;
        self.ensure_backend_dirs()?;
        let dir = self.find_ticket_dir(&id)?;
        let item = dir.join("item.md");
        let meta = ticket_meta_for_dir(&dir, read_item_file(&item)?.frontmatter)?;
        let artifacts = dir.join("artifacts");
        fs::create_dir_all(&artifacts).map_err(|e| io_err(&artifacts, e))?;
        let path = self.orchestration_plan_path(&dir);
        ensure_child_of(&artifacts, &path)?;
        let line_count = if path.exists() {
            fs::read_to_string(&path)
                .map_err(|e| io_err(&path, e))?
                .lines()
                .filter(|line| !line.trim().is_empty())
                .count()
        } else {
            0
        };
        let at = now_utc();
        let output = OrchestrationPlanRecord {
            id: format!("orch-plan-{}-{}", compact_now_utc(), line_count + 1),
            ticket_id: meta.id.clone(),
            kind: record.kind,
            related_ticket: record.related_ticket.map(trim_owned),
            note: record.note.map(trim_owned),
            accepted_plan: record.accepted_plan.map(trim_accepted_orchestration_plan),
            author: record.author.map(trim_owned).unwrap_or_else(default_author),
            at: at.clone(),
        };
        validate_orchestration_plan_record(&output, Some(&meta))?;
        let serialized = serde_json::to_string(&output).map_err(|e| {
            TicketError::Conflict(format!(
                "failed to serialize orchestration plan record: {e}"
            ))
        })?;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| io_err(&path, e))?;
        writeln!(file, "{serialized}").map_err(|e| io_err(&path, e))?;
        file.sync_all().map_err(|e| io_err(&path, e))?;
        self.set_frontmatter_fields(&item, &[("updated_at", &at)])?;
        Ok(output)
    }

    fn query_orchestration_plan_records(
        &self,
        ticket: Option<TicketIdOrSlug>,
        kind: Option<OrchestrationPlanKind>,
    ) -> Result<Vec<OrchestrationPlanRecord>> {
        let mut records = Vec::new();
        if let Some(ticket) = ticket {
            let dir = self.find_ticket_dir(&ticket)?;
            records.extend(self.read_orchestration_plan_records_for_dir(&dir)?);
        } else {
            for dir in self.iter_ticket_dirs(TicketListQuery::all())? {
                records.extend(self.read_orchestration_plan_records_for_dir(&dir)?);
            }
        }
        if let Some(kind) = kind {
            records.retain(|record| record.kind == kind);
        }
        records.sort_by(|a, b| a.at.cmp(&b.at).then_with(|| a.id.cmp(&b.id)));
        Ok(records)
    }

    fn doctor(&self) -> Result<TicketDoctorReport> {
        let mut report = TicketDoctorReport::default();

        let mut ids: HashMap<String, PathBuf> = HashMap::new();
        let mut duplicate_ids: BTreeSet<String> = BTreeSet::new();
        let mut state_index: HashMap<String, TicketWorkflowState> = HashMap::new();
        let mut relation_records: Vec<TicketRelation> = Vec::new();

        for legacy_bucket in ["open", "pending", "closed"] {
            let legacy_dir = self.root.join(legacy_bucket);
            if legacy_dir.is_dir() {
                report.push_error(
                    format!("legacy ticket bucket remains: {}", legacy_dir.display()),
                    Some(legacy_dir),
                );
            }
        }

        for dir in self.iter_ticket_dirs(TicketListQuery::all())? {
            let ticket_id = match ticket_id_from_dir(&dir) {
                Ok(id) => id,
                Err(err) => {
                    report.push_error(err.to_string(), Some(dir.clone()));
                    continue;
                }
            };
            if ids.insert(ticket_id.clone(), dir.clone()).is_some() {
                duplicate_ids.insert(ticket_id.clone());
            }
            let item = dir.join("item.md");
            let thread = dir.join("thread.md");
            let artifacts = dir.join("artifacts");
            if !thread.is_file() {
                report.push_error(
                    format!("missing thread.md: {}", dir.display()),
                    Some(thread.clone()),
                );
            }
            if !artifacts.is_dir() {
                report.push_error(
                    format!("missing artifacts/: {}", dir.display()),
                    Some(artifacts.clone()),
                );
            }
            let parsed = match read_item_file(&item) {
                Ok(parsed) => parsed,
                Err(TicketError::Parse { message, .. }) => {
                    report.push_error(message, Some(item.clone()));
                    continue;
                }
                Err(e) => return Err(e),
            };
            for field in REQUIRED_FIELDS {
                if parsed
                    .frontmatter
                    .get(field)
                    .is_none_or(|value| value.is_empty())
                {
                    report.push_error(
                        format!("missing required field '{field}': {}", item.display()),
                        Some(item.clone()),
                    );
                }
            }
            for obsolete in [
                "id",
                "slug",
                "status",
                "workflow_state",
                "kind",
                "labels",
                "action_required",
                "attention_required",
            ] {
                if parsed.frontmatter.get(obsolete).is_some() {
                    report.push_error(
                        format!(
                            "obsolete current frontmatter field '{obsolete}': {}",
                            item.display()
                        ),
                        Some(item.clone()),
                    );
                }
            }
            match parsed.frontmatter.get("state").map(String::as_str) {
                Some(value) if TicketWorkflowState::parse(value).is_none() => report.push_error(
                    format!("invalid state '{value}': {}", item.display()),
                    Some(item.clone()),
                ),
                _ => {}
            }
            if let Ok(meta) = ticket_meta_for_dir(&dir, parsed.frontmatter.clone()) {
                state_index.insert(meta.id.clone(), meta.workflow_state);
            }
            if parsed.frontmatter.get("state").map(String::as_str) == Some("closed")
                && !dir.join("resolution.md").is_file()
            {
                report.push_warning(
                    format!("closed ticket missing resolution.md: {}", dir.display()),
                    Some(dir.join("resolution.md")),
                );
            }
            if thread.exists() {
                doctor_thread_events(&thread, &mut report)?;
            }
            if artifacts.exists() {
                doctor_artifacts(&artifacts, &mut report)?;
                let meta = ticket_meta_for_dir(&dir, parsed.frontmatter.clone())?;
                doctor_ticket_relations_artifact(
                    &artifacts.join(TICKET_RELATIONS_ARTIFACT),
                    &meta,
                    &mut report,
                    &mut relation_records,
                )?;
                doctor_orchestration_plan_artifact(
                    &artifacts.join(ORCHESTRATION_PLAN_ARTIFACT),
                    &meta,
                    &mut report,
                )?;
            }
        }
        doctor_ticket_relation_references(&relation_records, &ids, &state_index, &mut report);
        doctor_ticket_relation_cycles(&relation_records, &state_index, &mut report);

        for duplicate in duplicate_ids {
            report.push_error(format!("duplicate id: {duplicate}"), None);
        }

        let todo = self
            .root
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("TODO.md");
        if todo.is_file() {
            let content = fs::read_to_string(&todo).map_err(|e| io_err(&todo, e))?;
            if content.contains("tickets/")
                && (content.contains(".md") || content.contains(".review.md"))
            {
                report.push_error("TODO.md still references legacy tickets/*.md", Some(todo));
            }
        }
        let legacy_dir = self
            .root
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("tickets");
        if legacy_dir.is_dir() {
            for entry in fs::read_dir(&legacy_dir).map_err(|e| io_err(&legacy_dir, e))? {
                let entry = entry.map_err(|e| io_err(&legacy_dir, e))?;
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) == Some("md") {
                    report.push_error(
                        format!("legacy ticket file remains: {}", path.display()),
                        Some(path),
                    );
                }
            }
        }
        Ok(report)
    }
}

struct BackendLock {
    file: File,
}

impl Drop for BackendLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

#[derive(Debug, Clone)]
struct ParsedItem {
    frontmatter: TicketItemFrontmatter,
    body: String,
}

#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
struct TicketItemFrontmatter {
    id: Option<String>,
    slug: Option<String>,
    title: Option<String>,
    status: Option<String>,
    kind: Option<String>,
    priority: Option<String>,
    labels: Vec<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
    assignee: Option<String>,
    readiness: Option<String>,
    risk_flags: Vec<String>,
    workflow_state: Option<TicketWorkflowState>,
    workflow_state_explicit: bool,
    state: Option<TicketWorkflowState>,
    state_explicit: bool,
    queued_by: Option<String>,
    queued_at: Option<String>,
    raw: BTreeMap<String, String>,
}

impl TicketItemFrontmatter {
    fn get(&self, key: &str) -> Option<&String> {
        self.raw.get(key)
    }
}

fn read_item_file(path: &Path) -> Result<ParsedItem> {
    let content = fs::read_to_string(path).map_err(|e| io_err(path, e))?;
    parse_item(&content).map_err(|message| TicketError::Parse {
        path: path.to_path_buf(),
        message,
    })
}

fn parse_item(content: &str) -> std::result::Result<ParsedItem, String> {
    let mut lines = content.lines();
    let Some(first) = lines.next() else {
        return Err("item.md is empty".to_string());
    };
    if first != "---" {
        return Err("item.md missing frontmatter opener".to_string());
    }
    let mut found_close = false;
    let mut frontmatter_lines = Vec::new();
    let mut body = String::new();
    for line in &mut lines {
        if line == "---" {
            found_close = true;
            break;
        }
        frontmatter_lines.push(line);
    }
    if !found_close {
        return Err("item.md missing frontmatter closer".to_string());
    }
    let rest: Vec<&str> = lines.collect();
    if !rest.is_empty() {
        body.push_str(&rest.join("\n"));
        if content.ends_with('\n') {
            body.push('\n');
        }
    }
    let frontmatter = parse_ticket_frontmatter(&frontmatter_lines.join("\n"))?;
    Ok(ParsedItem { frontmatter, body })
}

fn parse_ticket_frontmatter(content: &str) -> std::result::Result<TicketItemFrontmatter, String> {
    let value: YamlValue =
        serde_yaml::from_str(content).map_err(|err| format!("invalid YAML frontmatter: {err}"))?;
    let mapping = match value {
        YamlValue::Mapping(mapping) => mapping,
        YamlValue::Null => YamlMapping::new(),
        other => {
            return Err(format!(
                "frontmatter must be a YAML mapping, found {}",
                yaml_kind(&other)
            ));
        }
    };

    let mut raw = BTreeMap::new();
    for (key, value) in &mapping {
        let YamlValue::String(key) = key else {
            return Err("frontmatter keys must be strings".to_string());
        };
        raw.insert(key.clone(), raw_frontmatter_value(value)?);
    }

    let workflow_state_explicit = mapping.contains_key(YamlValue::String("workflow_state".into()));
    let workflow_state_value = yaml_string(&mapping, "workflow_state")?;
    let workflow_state = match workflow_state_value.as_deref() {
        Some(value) => Some(TicketWorkflowState::parse(value).ok_or_else(|| {
            format!("invalid workflow_state '{value}': expected planning, ready, queued, inprogress, done, or closed")
        })?),
        None => None,
    };
    let state_explicit = mapping.contains_key(YamlValue::String("state".into()));
    let state_value = yaml_string(&mapping, "state")?;
    let state = match state_value.as_deref() {
        Some(value) => Some(TicketWorkflowState::parse(value).ok_or_else(|| {
            format!("invalid state '{value}': expected planning, ready, queued, inprogress, done, or closed")
        })?),
        None => None,
    };

    Ok(TicketItemFrontmatter {
        id: yaml_string(&mapping, "id")?,
        slug: yaml_string(&mapping, "slug")?,
        title: yaml_string(&mapping, "title")?,
        status: yaml_string(&mapping, "status")?,
        kind: yaml_string(&mapping, "kind")?,
        priority: yaml_string(&mapping, "priority")?,
        labels: yaml_string_list(&mapping, "labels")?,
        created_at: yaml_string(&mapping, "created_at")?,
        updated_at: yaml_string(&mapping, "updated_at")?,
        assignee: yaml_string(&mapping, "assignee")?,
        readiness: yaml_string(&mapping, "readiness")?,
        risk_flags: yaml_string_list(&mapping, "risk_flags")?,
        workflow_state,
        workflow_state_explicit,
        state,
        state_explicit,
        queued_by: yaml_string(&mapping, "queued_by")?,
        queued_at: yaml_string(&mapping, "queued_at")?,
        raw,
    })
}

fn yaml_key(key: &str) -> YamlValue {
    YamlValue::String(key.to_string())
}

fn yaml_get<'a>(mapping: &'a YamlMapping, key: &str) -> Option<&'a YamlValue> {
    mapping.get(yaml_key(key))
}

fn yaml_string(mapping: &YamlMapping, key: &str) -> std::result::Result<Option<String>, String> {
    match yaml_get(mapping, key) {
        Some(YamlValue::Null) | None => Ok(None),
        Some(YamlValue::String(value)) => Ok(Some(value.clone())),
        Some(value) => Err(format!(
            "frontmatter field `{key}` must be a YAML string or null, found {}",
            yaml_kind(value)
        )),
    }
}

fn yaml_string_list(mapping: &YamlMapping, key: &str) -> std::result::Result<Vec<String>, String> {
    match yaml_get(mapping, key) {
        Some(YamlValue::Null) | None => Ok(Vec::new()),
        Some(YamlValue::Sequence(values)) => values
            .iter()
            .enumerate()
            .map(|(idx, value)| match value {
                YamlValue::String(value) => Ok(value.clone()),
                other => Err(format!(
                    "frontmatter field `{key}` item {idx} must be a YAML string, found {}",
                    yaml_kind(other)
                )),
            })
            .collect(),
        Some(value) => Err(format!(
            "frontmatter field `{key}` must be a YAML sequence or null, found {}",
            yaml_kind(value)
        )),
    }
}

fn raw_frontmatter_value(value: &YamlValue) -> std::result::Result<String, String> {
    match value {
        YamlValue::Null => Ok("null".to_string()),
        YamlValue::Bool(value) => Ok(value.to_string()),
        YamlValue::Number(value) => Ok(value.to_string()),
        YamlValue::String(value) => Ok(value.clone()),
        YamlValue::Sequence(values) => values
            .iter()
            .map(|value| match value {
                YamlValue::String(value) => Ok(format_yaml_string_scalar(value)),
                other => Err(format!(
                    "frontmatter sequence values must be strings, found {}",
                    yaml_kind(other)
                )),
            })
            .collect::<std::result::Result<Vec<_>, _>>()
            .map(|values| format!("[{}]", values.join(", "))),
        YamlValue::Mapping(_) => Err("frontmatter nested mappings are not supported".to_string()),
        YamlValue::Tagged(tagged) => raw_frontmatter_value(&tagged.value),
    }
}

fn yaml_kind(value: &YamlValue) -> &'static str {
    match value {
        YamlValue::Null => "null",
        YamlValue::Bool(_) => "boolean",
        YamlValue::Number(_) => "number",
        YamlValue::String(_) => "string",
        YamlValue::Sequence(_) => "sequence",
        YamlValue::Mapping(_) => "mapping",
        YamlValue::Tagged(_) => "tagged value",
    }
}

fn ticket_id_from_dir(dir: &Path) -> Result<String> {
    let Some(name) = dir.file_name().and_then(|name| name.to_str()) else {
        return Err(TicketError::Conflict(format!(
            "ticket directory has no UTF-8 id: {}",
            dir.display()
        )));
    };
    ensure_safe_component(name)?;
    validate_record_id(name).map_err(|err| {
        TicketError::InvalidPathComponent(format!("{name} is not a canonical record id: {err}"))
    })?;
    Ok(name.to_string())
}

fn ticket_meta_for_dir(dir: &Path, frontmatter: TicketItemFrontmatter) -> Result<TicketMeta> {
    Ok(ticket_meta(frontmatter, ticket_id_from_dir(dir)?))
}

fn ticket_meta(frontmatter: TicketItemFrontmatter, id: String) -> TicketMeta {
    let workflow_state = frontmatter
        .state
        .or(frontmatter.workflow_state)
        .or_else(|| {
            frontmatter
                .status
                .as_deref()
                .map(ExtensibleTicketStatus::from)
                .map(|status| TicketWorkflowState::default_for_status(&status))
        })
        .unwrap_or(TicketWorkflowState::Planning);
    let status = match workflow_state {
        TicketWorkflowState::Closed => ExtensibleTicketStatus::Closed,
        _ => ExtensibleTicketStatus::Open,
    };
    TicketMeta {
        id: id.clone(),
        slug: id,
        title: frontmatter.title.unwrap_or_default(),
        status,
        kind: String::new(),
        priority: frontmatter.priority.unwrap_or_default(),
        labels: Vec::new(),
        created_at: frontmatter.created_at,
        updated_at: frontmatter.updated_at,
        assignee: frontmatter.assignee,
        readiness: frontmatter.readiness,
        risk_flags: frontmatter.risk_flags,
        workflow_state,
        workflow_state_explicit: frontmatter.state_explicit,
        queued_by: frontmatter.queued_by,
        queued_at: frontmatter.queued_at,
        raw: frontmatter.raw,
    }
}

fn ticket_summary_from_meta(meta: TicketMeta) -> TicketSummary {
    TicketSummary {
        id: meta.id,
        slug: meta.slug,
        title: meta.title,
        status: meta.status,
        kind: meta.kind,
        priority: meta.priority,
        labels: meta.labels,
        readiness: meta.readiness,
        workflow_state: meta.workflow_state,
        workflow_state_explicit: meta.workflow_state_explicit,
        queued_by: meta.queued_by,
        queued_at: meta.queued_at,
        updated_at: meta.updated_at,
    }
}

fn sort_ticket_relations(relations: &mut [TicketRelation]) {
    relations.sort_by(|a, b| {
        a.ticket_id
            .cmp(&b.ticket_id)
            .then_with(|| a.kind.cmp(&b.kind))
            .then_with(|| a.target.cmp(&b.target))
            .then_with(|| a.at.cmp(&b.at))
    });
}

fn invalid_ticket_record_label(dir: &Path) -> String {
    dir.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| validate_record_id(name).is_ok())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| "invalid ticket record".to_string())
}

fn invalid_ticket_record_reason(error: &TicketError) -> &'static str {
    match error {
        TicketError::Io { .. } => "could not read ticket record",
        TicketError::Parse { .. } => "invalid ticket record schema",
        TicketError::InvalidPathComponent(_) | TicketError::PathEscapesRoot { .. } => {
            "invalid ticket record identity"
        }
        TicketError::Locked { .. } => "ticket backend is locked",
        TicketError::NotFound(_) => "ticket record is missing",
        TicketError::Ambiguous { .. } | TicketError::Conflict(_) => {
            "invalid ticket record metadata"
        }
    }
}

fn push_invalid_ticket_record(
    invalid_records: &mut Vec<TicketInvalidRecord>,
    invalid_seen: &mut BTreeSet<String>,
    dir: &Path,
    error: &TicketError,
) {
    let label = invalid_ticket_record_label(dir);
    if invalid_seen.insert(label.clone()) {
        invalid_records.push(TicketInvalidRecord {
            label,
            reason: invalid_ticket_record_reason(error).to_string(),
        });
    }
}

fn trim_owned(value: String) -> String {
    value.trim().to_string()
}

fn relation_inverse_kind(kind: TicketRelationKind) -> &'static str {
    match kind {
        TicketRelationKind::DependsOn => "dependency_of",
        TicketRelationKind::Blocks => "blocked_by",
        TicketRelationKind::Related => "related",
        TicketRelationKind::Supersedes => "superseded_by",
        TicketRelationKind::DuplicateOf => "duplicated_by",
    }
}

fn relation_notice_for_outgoing(relation: &TicketRelation) -> Option<TicketRelationNotice> {
    match relation.kind {
        TicketRelationKind::Supersedes => Some(TicketRelationNotice {
            related_ticket: relation.target.clone(),
            kind: relation.kind,
            message: format!(
                "ticket supersedes {}; verify replacement before routing",
                relation.target
            ),
        }),
        TicketRelationKind::DuplicateOf => Some(TicketRelationNotice {
            related_ticket: relation.target.clone(),
            kind: relation.kind,
            message: format!(
                "ticket is duplicate of {}; avoid duplicate implementation",
                relation.target
            ),
        }),
        _ => None,
    }
}

fn relation_notice_for_incoming(relation: &TicketRelation) -> Option<TicketRelationNotice> {
    match relation.kind {
        TicketRelationKind::Supersedes => Some(TicketRelationNotice {
            related_ticket: relation.ticket_id.clone(),
            kind: relation.kind,
            message: format!(
                "ticket is superseded by {}; verify replacement before routing",
                relation.ticket_id
            ),
        }),
        TicketRelationKind::DuplicateOf => Some(TicketRelationNotice {
            related_ticket: relation.ticket_id.clone(),
            kind: relation.kind,
            message: format!(
                "ticket has duplicate {}; avoid duplicate implementation",
                relation.ticket_id
            ),
        }),
        _ => None,
    }
}

fn ticket_state_resolved(state: TicketWorkflowState) -> bool {
    matches!(
        state,
        TicketWorkflowState::Done | TicketWorkflowState::Closed
    )
}

fn relation_view_from_records(
    meta: &TicketMeta,
    records: &[TicketRelation],
    states: &HashMap<String, TicketWorkflowState>,
) -> TicketRelationView {
    let mut view = TicketRelationView::default();
    for relation in records {
        if relation.ticket_id == meta.id {
            view.outgoing.push(relation.clone());
            if relation.kind == TicketRelationKind::DependsOn {
                let state = states
                    .get(&relation.target)
                    .copied()
                    .unwrap_or(TicketWorkflowState::Planning);
                if !ticket_state_resolved(state) {
                    view.blockers.push(TicketRelationBlocker {
                        blocking_ticket: relation.target.clone(),
                        reason_kind: "depends_on".to_string(),
                        relation_kind: relation.kind,
                        note: relation.note.clone(),
                        blocking_state: state,
                    });
                }
            }
            if let Some(notice) = relation_notice_for_outgoing(relation) {
                view.notices.push(notice);
            }
        }
        if relation.target == meta.id {
            view.incoming.push(DerivedTicketRelation {
                source_ticket: relation.ticket_id.clone(),
                inverse_kind: relation_inverse_kind(relation.kind).to_string(),
                forward_kind: relation.kind,
                note: relation.note.clone(),
                author: relation.author.clone(),
                at: relation.at.clone(),
            });
            if relation.kind == TicketRelationKind::Blocks {
                let state = states
                    .get(&relation.ticket_id)
                    .copied()
                    .unwrap_or(TicketWorkflowState::Planning);
                if !ticket_state_resolved(state) {
                    view.blockers.push(TicketRelationBlocker {
                        blocking_ticket: relation.ticket_id.clone(),
                        reason_kind: "blocked_by".to_string(),
                        relation_kind: relation.kind,
                        note: relation.note.clone(),
                        blocking_state: state,
                    });
                }
            }
            if let Some(notice) = relation_notice_for_incoming(relation) {
                view.notices.push(notice);
            }
        }
    }
    view.outgoing.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then_with(|| a.target.cmp(&b.target))
            .then_with(|| a.at.cmp(&b.at))
    });
    view.incoming.sort_by(|a, b| {
        a.inverse_kind
            .cmp(&b.inverse_kind)
            .then_with(|| a.source_ticket.cmp(&b.source_ticket))
            .then_with(|| a.at.cmp(&b.at))
    });
    view.blockers.sort_by(|a, b| {
        a.reason_kind
            .cmp(&b.reason_kind)
            .then_with(|| a.blocking_ticket.cmp(&b.blocking_ticket))
    });
    view.notices.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then_with(|| a.related_ticket.cmp(&b.related_ticket))
    });
    view
}

fn relation_blocker_allows_queue(blocker: &TicketRelationBlocker) -> bool {
    matches!(
        blocker.blocking_state,
        TicketWorkflowState::Queued | TicketWorkflowState::InProgress
    )
}

fn format_relation_blockers(blockers: &[TicketRelationBlocker]) -> String {
    blockers
        .iter()
        .map(|blocker| {
            format!(
                "{} via {} (state: {})",
                blocker.blocking_ticket, blocker.reason_kind, blocker.blocking_state
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn validate_relation_optional_text(
    label: &str,
    value: Option<&str>,
    max_bytes: usize,
) -> Result<()> {
    if let Some(value) = value {
        if value.as_bytes().len() > max_bytes {
            return Err(TicketError::Conflict(format!(
                "ticket relation {label} exceeds {max_bytes} bytes"
            )));
        }
        if value.contains('\0') {
            return Err(TicketError::Conflict(format!(
                "ticket relation {label} must not contain NUL bytes"
            )));
        }
    }
    Ok(())
}

fn validate_relation_optional_single_line(
    label: &str,
    value: Option<&str>,
    max_bytes: usize,
) -> Result<()> {
    validate_relation_optional_text(label, value, max_bytes)?;
    if let Some(value) = value {
        if value.contains('\n') || value.contains('\r') {
            return Err(TicketError::Conflict(format!(
                "ticket relation {label} must be a single line"
            )));
        }
    }
    Ok(())
}

fn validate_new_ticket_relation(relation: &NewTicketRelation) -> Result<()> {
    let target = relation.target.trim();
    validate_relation_optional_single_line(
        "target",
        Some(target),
        MAX_TICKET_RELATION_FIELD_BYTES,
    )?;
    if target.is_empty() {
        return Err(TicketError::Conflict(
            "ticket relation target must not be empty".to_string(),
        ));
    }
    validate_relation_optional_text(
        "note",
        relation.note.as_deref(),
        MAX_TICKET_RELATION_NOTE_BYTES,
    )?;
    validate_relation_optional_single_line(
        "author",
        relation.author.as_deref(),
        MAX_TICKET_RELATION_FIELD_BYTES,
    )
}

fn validate_ticket_relation(relation: &TicketRelation, meta: Option<&TicketMeta>) -> Result<()> {
    validate_relation_optional_single_line(
        "ticket_id",
        Some(&relation.ticket_id),
        MAX_TICKET_RELATION_FIELD_BYTES,
    )?;
    validate_relation_optional_single_line(
        "target",
        Some(&relation.target),
        MAX_TICKET_RELATION_FIELD_BYTES,
    )?;
    validate_relation_optional_text(
        "note",
        relation.note.as_deref(),
        MAX_TICKET_RELATION_NOTE_BYTES,
    )?;
    validate_relation_optional_single_line(
        "author",
        Some(&relation.author),
        MAX_TICKET_RELATION_FIELD_BYTES,
    )?;
    validate_relation_optional_single_line(
        "at",
        Some(&relation.at),
        MAX_TICKET_RELATION_FIELD_BYTES,
    )?;
    if let Some(meta) = meta {
        if relation.ticket_id != meta.id {
            return Err(TicketError::Conflict(format!(
                "ticket relation targets {} but artifact belongs to {}",
                relation.ticket_id, meta.id
            )));
        }
    }
    if relation.ticket_id == relation.target {
        return Err(TicketError::Conflict(format!(
            "ticket relation cannot target itself: {}",
            relation.ticket_id
        )));
    }
    Ok(())
}

fn read_ticket_relations_artifact(
    path: &Path,
    meta: Option<&TicketMeta>,
) -> Result<Vec<TicketRelation>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(path).map_err(|e| io_err(path, e))?;
    let artifact: TicketRelationArtifact =
        serde_json::from_str(&content).map_err(|e| TicketError::Parse {
            path: path.to_path_buf(),
            message: format!("invalid ticket relations artifact: {e}"),
        })?;
    if artifact.version != 1 {
        return Err(TicketError::Parse {
            path: path.to_path_buf(),
            message: format!(
                "unsupported ticket relations artifact version {}",
                artifact.version
            ),
        });
    }
    let mut seen = BTreeSet::new();
    for relation in &artifact.relations {
        validate_ticket_relation(relation, meta).map_err(|err| TicketError::Parse {
            path: path.to_path_buf(),
            message: format!("invalid ticket relation: {err}"),
        })?;
        if !seen.insert((relation.kind, relation.target.clone())) {
            return Err(TicketError::Parse {
                path: path.to_path_buf(),
                message: format!(
                    "duplicate ticket relation {} {}",
                    relation.kind, relation.target
                ),
            });
        }
    }
    Ok(artifact.relations)
}

fn write_ticket_relations_artifact(path: &Path, relations: &[TicketRelation]) -> Result<()> {
    let artifact = TicketRelationArtifact {
        version: 1,
        relations: relations.to_vec(),
    };
    let content = serde_json::to_string_pretty(&artifact).map_err(|e| TicketError::Parse {
        path: path.to_path_buf(),
        message: format!("failed to serialize ticket relations artifact: {e}"),
    })? + "\n";
    fs::write(path, content).map_err(|e| io_err(path, e))
}

fn trim_accepted_orchestration_plan(plan: AcceptedOrchestrationPlan) -> AcceptedOrchestrationPlan {
    AcceptedOrchestrationPlan {
        summary: plan.summary.trim().to_string(),
        branch: plan.branch.map(trim_owned),
        worktree: plan.worktree.map(trim_owned),
        role_plan: plan.role_plan.map(trim_owned),
    }
}

fn validate_plan_required_text(label: &str, value: &str, max_bytes: usize) -> Result<()> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(TicketError::Conflict(format!(
            "orchestration plan {label} must not be empty"
        )));
    }
    validate_plan_optional_text(label, Some(trimmed), max_bytes)
}

fn validate_plan_optional_text(label: &str, value: Option<&str>, max_bytes: usize) -> Result<()> {
    if let Some(value) = value {
        if value.as_bytes().len() > max_bytes {
            return Err(TicketError::Conflict(format!(
                "orchestration plan {label} exceeds {max_bytes} bytes"
            )));
        }
        if value.contains('\0') {
            return Err(TicketError::Conflict(format!(
                "orchestration plan {label} must not contain NUL bytes"
            )));
        }
    }
    Ok(())
}

fn validate_plan_optional_single_line(
    label: &str,
    value: Option<&str>,
    max_bytes: usize,
) -> Result<()> {
    validate_plan_optional_text(label, value, max_bytes)?;
    if let Some(value) = value {
        if value.contains('\n') || value.contains('\r') {
            return Err(TicketError::Conflict(format!(
                "orchestration plan {label} must be a single line"
            )));
        }
    }
    Ok(())
}

fn validate_accepted_orchestration_plan(plan: &AcceptedOrchestrationPlan) -> Result<()> {
    validate_plan_required_text(
        "accepted_plan.summary",
        &plan.summary,
        MAX_ORCHESTRATION_PLAN_TEXT_BYTES,
    )?;
    validate_plan_optional_single_line(
        "accepted_plan.branch",
        plan.branch.as_deref(),
        MAX_ORCHESTRATION_PLAN_FIELD_BYTES,
    )?;
    validate_plan_optional_single_line(
        "accepted_plan.worktree",
        plan.worktree.as_deref(),
        MAX_ORCHESTRATION_PLAN_FIELD_BYTES,
    )?;
    validate_plan_optional_text(
        "accepted_plan.role_plan",
        plan.role_plan.as_deref(),
        MAX_ORCHESTRATION_PLAN_TEXT_BYTES,
    )
}

fn validate_new_orchestration_plan_record(record: &NewOrchestrationPlanRecord) -> Result<()> {
    if record.kind.requires_related_ticket() {
        let related = record.related_ticket.as_deref().ok_or_else(|| {
            TicketError::Conflict(format!(
                "orchestration plan kind `{}` requires related_ticket",
                record.kind
            ))
        })?;
        validate_plan_required_text(
            "related_ticket",
            related,
            MAX_ORCHESTRATION_PLAN_FIELD_BYTES,
        )?;
        validate_plan_optional_single_line(
            "related_ticket",
            Some(related),
            MAX_ORCHESTRATION_PLAN_FIELD_BYTES,
        )?;
    } else if let Some(related) = record.related_ticket.as_deref() {
        validate_plan_optional_single_line(
            "related_ticket",
            Some(related),
            MAX_ORCHESTRATION_PLAN_FIELD_BYTES,
        )?;
    }

    if matches!(record.kind, OrchestrationPlanKind::AcceptedPlan) {
        let plan = record.accepted_plan.as_ref().ok_or_else(|| {
            TicketError::Conflict("accepted_plan record requires accepted_plan fields".to_string())
        })?;
        validate_accepted_orchestration_plan(plan)?;
    } else if record.accepted_plan.is_some() {
        return Err(TicketError::Conflict(
            "accepted_plan fields are only valid for accepted_plan records".to_string(),
        ));
    }

    if matches!(record.kind, OrchestrationPlanKind::WaitingCapacityNote) {
        let note = record.note.as_deref().ok_or_else(|| {
            TicketError::Conflict("waiting_capacity_note records require note".to_string())
        })?;
        validate_plan_required_text("note", note, MAX_ORCHESTRATION_PLAN_TEXT_BYTES)?;
    } else {
        validate_plan_optional_text(
            "note",
            record.note.as_deref(),
            MAX_ORCHESTRATION_PLAN_TEXT_BYTES,
        )?;
    }
    validate_plan_optional_single_line(
        "author",
        record.author.as_deref(),
        MAX_ORCHESTRATION_PLAN_FIELD_BYTES,
    )
}

fn validate_orchestration_plan_record(
    record: &OrchestrationPlanRecord,
    meta: Option<&TicketMeta>,
) -> Result<()> {
    validate_plan_required_text("id", &record.id, MAX_ORCHESTRATION_PLAN_FIELD_BYTES)?;
    validate_plan_optional_single_line("id", Some(&record.id), MAX_ORCHESTRATION_PLAN_FIELD_BYTES)?;
    validate_plan_required_text(
        "ticket_id",
        &record.ticket_id,
        MAX_ORCHESTRATION_PLAN_FIELD_BYTES,
    )?;
    validate_plan_optional_single_line(
        "ticket_id",
        Some(&record.ticket_id),
        MAX_ORCHESTRATION_PLAN_FIELD_BYTES,
    )?;
    validate_plan_required_text("author", &record.author, MAX_ORCHESTRATION_PLAN_FIELD_BYTES)?;
    validate_plan_optional_single_line(
        "author",
        Some(&record.author),
        MAX_ORCHESTRATION_PLAN_FIELD_BYTES,
    )?;
    validate_plan_required_text("at", &record.at, MAX_ORCHESTRATION_PLAN_FIELD_BYTES)?;
    validate_plan_optional_single_line("at", Some(&record.at), MAX_ORCHESTRATION_PLAN_FIELD_BYTES)?;
    let new_record = NewOrchestrationPlanRecord {
        kind: record.kind,
        related_ticket: record.related_ticket.clone(),
        note: record.note.clone(),
        accepted_plan: record.accepted_plan.clone(),
        author: Some(record.author.clone()),
    };
    validate_new_orchestration_plan_record(&new_record)?;
    if let Some(meta) = meta {
        if record.ticket_id != meta.id {
            return Err(TicketError::Conflict(format!(
                "orchestration plan record {} targets {} but artifact belongs to {}",
                record.id, record.ticket_id, meta.id
            )));
        }
    }
    Ok(())
}

fn read_orchestration_plan_artifact(
    path: &Path,
    meta: Option<&TicketMeta>,
) -> Result<Vec<OrchestrationPlanRecord>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(path).map_err(|e| io_err(path, e))?;
    let mut records = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let record: OrchestrationPlanRecord =
            serde_json::from_str(line).map_err(|e| TicketError::Parse {
                path: path.to_path_buf(),
                message: format!("invalid orchestration plan record on line {}: {e}", idx + 1),
            })?;
        validate_orchestration_plan_record(&record, meta).map_err(|err| TicketError::Parse {
            path: path.to_path_buf(),
            message: format!(
                "invalid orchestration plan record on line {}: {err}",
                idx + 1
            ),
        })?;
        records.push(record);
    }
    Ok(records)
}

fn format_yaml_string_scalar(value: &str) -> String {
    let mut out = String::from("'");
    for ch in value.chars() {
        if ch == '\'' {
            out.push_str("''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

fn yaml_string_or_null(value: Option<&str>) -> String {
    value
        .map(format_yaml_string_scalar)
        .unwrap_or_else(|| "null".to_string())
}

fn labels_yaml(labels: &[String]) -> String {
    if labels.is_empty() {
        return "[]".to_string();
    }
    format!(
        "[{}]",
        labels
            .iter()
            .map(|label| label.trim())
            .filter(|label| !label.is_empty())
            .map(format_yaml_string_scalar)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn serialize_item(fields: &[(String, String)], body: &str) -> String {
    let mut out = String::from("---\n");
    for (key, value) in fields {
        out.push_str(key);
        out.push_str(": ");
        out.push_str(value);
        out.push('\n');
    }
    out.push_str("---\n\n");
    out.push_str(body);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn replace_frontmatter_fields(
    content: &str,
    updates: &[(&str, &str)],
) -> std::result::Result<String, String> {
    let mut lines: Vec<String> = content.lines().map(ToOwned::to_owned).collect();
    if lines.first().map(String::as_str) != Some("---") {
        return Err("item.md missing frontmatter opener".to_string());
    }
    let Some(end) = lines
        .iter()
        .enumerate()
        .skip(1)
        .find_map(|(idx, line)| (line == "---").then_some(idx))
    else {
        return Err("item.md missing frontmatter closer".to_string());
    };
    let mut seen = BTreeSet::new();
    for line in lines.iter_mut().take(end).skip(1) {
        if let Some((key, _)) = line.split_once(':') {
            let key = key.trim().to_string();
            if let Some((_, value)) = updates.iter().find(|(update_key, _)| *update_key == key) {
                *line = format!("{key}: {}", format_yaml_string_scalar(value));
                seen.insert(key);
            }
        }
    }
    let mut insert_at = end;
    for (key, value) in updates {
        if !seen.contains(*key) {
            lines.insert(
                insert_at,
                format!("{key}: {}", format_yaml_string_scalar(value)),
            );
            insert_at += 1;
        }
    }
    let mut out = lines.join("\n");
    if content.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

fn render_event_comment(attrs: &[(&str, &str)]) -> Result<String> {
    let mut out = String::from("<!--");
    for (key, value) in attrs {
        validate_event_attr(key, value)?;
        out.push(' ');
        out.push_str(key);
        out.push_str(": ");
        out.push_str(&format_event_attr_value(value));
    }
    out.push_str(" -->");
    Ok(out)
}

fn format_event_attr_value(value: &str) -> String {
    if !value.is_empty()
        && !value.chars().any(char::is_whitespace)
        && !value.contains('"')
        && !value.contains('\\')
        && !value.contains("-->")
    {
        return value.to_string();
    }
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn validate_event_attr(key: &str, value: &str) -> Result<()> {
    if key.trim().is_empty() || key.chars().any(char::is_whitespace) || key.contains(':') {
        return Err(TicketError::Conflict(format!(
            "thread event attribute key is invalid: {key:?}"
        )));
    }
    if value.contains('\n') || value.contains('\r') || value.contains("-->") {
        return Err(TicketError::Conflict(format!(
            "thread event attribute `{key}` must be a single safe comment value"
        )));
    }
    Ok(())
}

fn validate_required_event_value(label: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(TicketError::Conflict(format!(
            "state_changed event requires non-empty {label}"
        )));
    }
    validate_event_attr(label, value)
}

fn validate_state_change(change: &TicketStateChange) -> Result<()> {
    validate_required_event_value("from", &change.from)?;
    validate_required_event_value("to", &change.to)?;
    validate_required_event_value("reason", &change.reason)?;
    if change.reason.len() > MAX_STATE_CHANGE_REASON_BYTES {
        return Err(TicketError::Conflict(format!(
            "state_changed reason exceeds {MAX_STATE_CHANGE_REASON_BYTES} bytes"
        )));
    }
    if let Some(author) = change.author.as_deref() {
        validate_required_event_value("author", author)?;
    }
    if change.body.as_str().len() > MAX_INTAKE_SUMMARY_BODY_BYTES {
        return Err(TicketError::Conflict(format!(
            "state_changed body exceeds {MAX_INTAKE_SUMMARY_BODY_BYTES} bytes"
        )));
    }
    Ok(())
}

fn validate_intake_summary(summary: &TicketIntakeSummary) -> Result<()> {
    let body = summary.body.as_str();
    if body.trim().is_empty() {
        return Err(TicketError::Conflict(
            "intake_summary event requires a non-empty body".to_string(),
        ));
    }
    if body.len() > MAX_INTAKE_SUMMARY_BODY_BYTES {
        return Err(TicketError::Conflict(format!(
            "intake_summary body exceeds {MAX_INTAKE_SUMMARY_BODY_BYTES} bytes"
        )));
    }
    if let Some(author) = summary.author.as_deref() {
        validate_required_event_value("author", author)?;
    }
    Ok(())
}

fn validate_state_field_name(field: &str) -> Result<()> {
    if field.trim().is_empty()
        || field.chars().any(char::is_whitespace)
        || field.contains(':')
        || field.contains("--")
    {
        return Err(TicketError::Conflict(format!(
            "state field name is invalid: {field:?}"
        )));
    }
    Ok(())
}

fn parse_thread(path: &Path) -> Result<Vec<TicketEvent>> {
    let content = fs::read_to_string(path).map_err(|e| io_err(path, e))?;
    let mut events = Vec::new();
    let lines: Vec<&str> = content.lines().collect();
    let mut idx = 0;
    while idx < lines.len() {
        let line = lines[idx].trim();
        if let Some(comment) = line
            .strip_prefix("<!-- ")
            .and_then(|v| v.strip_suffix(" -->"))
        {
            let attrs = parse_event_comment(comment);
            let kind = attrs
                .get("event")
                .map(|value| TicketEventKind::from(value.as_str()))
                .unwrap_or_else(|| TicketEventKind::Other(String::new()));
            idx += 1;
            while idx < lines.len() && lines[idx].trim().is_empty() {
                idx += 1;
            }
            let mut heading = None;
            if idx < lines.len() {
                if let Some(stripped) = lines[idx].strip_prefix("## ") {
                    heading = Some(stripped.to_string());
                    idx += 1;
                }
            }
            while idx < lines.len() && lines[idx].trim().is_empty() {
                idx += 1;
            }
            let mut body_lines = Vec::new();
            while idx < lines.len() {
                if lines[idx].trim() == "---" {
                    idx += 1;
                    break;
                }
                body_lines.push(lines[idx]);
                idx += 1;
            }
            let mut body = body_lines.join("\n");
            while body.ends_with('\n') {
                body.pop();
            }
            events.push(TicketEvent {
                kind,
                author: attrs.get("author").cloned(),
                at: attrs.get("at").cloned(),
                status: attrs.get("status").cloned(),
                from: attrs.get("from").cloned(),
                to: attrs.get("to").cloned(),
                reason: attrs.get("reason").cloned(),
                state_field: attrs.get("field").cloned(),
                heading,
                body: MarkdownText::new(body),
                references: Vec::new(),
                attributes: attrs,
            });
        } else {
            idx += 1;
        }
    }
    Ok(events)
}

fn parse_event_comment(comment: &str) -> BTreeMap<String, String> {
    let mut attrs = BTreeMap::new();
    let mut chars = comment.char_indices().peekable();
    while let Some((_, ch)) = chars.peek().copied() {
        if ch.is_whitespace() {
            chars.next();
            continue;
        }
        let start = chars.peek().map(|(idx, _)| *idx).unwrap_or(comment.len());
        while let Some((_, ch)) = chars.peek().copied() {
            if ch == ':' || ch.is_whitespace() {
                break;
            }
            chars.next();
        }
        let end = chars.peek().map(|(idx, _)| *idx).unwrap_or(comment.len());
        if chars.peek().map(|(_, ch)| *ch) != Some(':') {
            while let Some((_, ch)) = chars.peek().copied() {
                if ch.is_whitespace() {
                    break;
                }
                chars.next();
            }
            continue;
        }
        chars.next();
        while let Some((_, ch)) = chars.peek().copied() {
            if ch.is_whitespace() {
                chars.next();
            } else {
                break;
            }
        }
        let value = if chars.peek().map(|(_, ch)| *ch) == Some('"') {
            chars.next();
            let mut value = String::new();
            let mut escaped = false;
            for (_, ch) in chars.by_ref() {
                if escaped {
                    value.push(ch);
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    break;
                } else {
                    value.push(ch);
                }
            }
            value
        } else {
            let value_start = chars.peek().map(|(idx, _)| *idx).unwrap_or(comment.len());
            while let Some((_, ch)) = chars.peek().copied() {
                if ch.is_whitespace() {
                    break;
                }
                chars.next();
            }
            let value_end = chars.peek().map(|(idx, _)| *idx).unwrap_or(comment.len());
            comment[value_start..value_end].to_string()
        };
        let key = &comment[start..end];
        if !key.is_empty() {
            attrs.insert(key.to_string(), value);
        }
    }
    attrs
}

fn doctor_thread_events(path: &Path, report: &mut TicketDoctorReport) -> Result<()> {
    let content = fs::read_to_string(path).map_err(|e| io_err(path, e))?;
    let mut intake_summary_lines = Vec::new();
    for (line_no, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("<!-- event:") && !trimmed.ends_with("-->") {
            report.push_error(
                format!(
                    "malformed thread event comment at {}:{}",
                    path.display(),
                    line_no + 1
                ),
                Some(path.to_path_buf()),
            );
        }
        if let Some(comment) = trimmed
            .strip_prefix("<!-- ")
            .and_then(|v| v.strip_suffix(" -->"))
        {
            let attrs = parse_event_comment(comment);
            let Some(event) = attrs.get("event").map(String::as_str) else {
                continue;
            };
            if attrs
                .get("at")
                .map_or(true, |value| value.trim().is_empty())
            {
                report.push_error(
                    format!(
                        "thread event missing at: {}:{}",
                        path.display(),
                        line_no + 1
                    ),
                    Some(path.to_path_buf()),
                );
            }
            match event {
                "review" => match attrs.get("status").map(String::as_str) {
                    Some("approve" | "request_changes") => {}
                    _ => report.push_warning(
                        format!(
                            "legacy review event missing valid status at {}:{}",
                            path.display(),
                            line_no + 1
                        ),
                        Some(path.to_path_buf()),
                    ),
                },
                "state_changed" => {
                    for key in ["from", "to", "reason", "author"] {
                        if attrs.get(key).map_or(true, |value| value.trim().is_empty()) {
                            report.push_error(
                                format!(
                                    "state_changed event missing {key}: {}:{}",
                                    path.display(),
                                    line_no + 1
                                ),
                                Some(path.to_path_buf()),
                            );
                        }
                    }
                }
                "intake_summary" => {
                    if attrs
                        .get("author")
                        .map_or(true, |value| value.trim().is_empty())
                    {
                        report.push_error(
                            format!(
                                "intake_summary event missing author: {}:{}",
                                path.display(),
                                line_no + 1
                            ),
                            Some(path.to_path_buf()),
                        );
                    }
                    intake_summary_lines.push(line_no + 1);
                }
                _ => {}
            }
        }
    }
    if !intake_summary_lines.is_empty() {
        let summaries = parse_thread(path)?
            .into_iter()
            .filter(|event| event.kind == TicketEventKind::IntakeSummary);
        for (idx, event) in summaries.enumerate() {
            if event.body.as_str().trim().is_empty() {
                let line = intake_summary_lines.get(idx).copied().unwrap_or_default();
                report.push_error(
                    format!(
                        "intake_summary event missing body at {}:{}",
                        path.display(),
                        line
                    ),
                    Some(path.to_path_buf()),
                );
            }
        }
    }
    Ok(())
}

fn collect_artifacts(dir: &Path) -> Result<Vec<TicketArtifactRef>> {
    let mut artifacts = Vec::new();
    if !dir.exists() {
        return Ok(artifacts);
    }
    collect_artifacts_inner(dir, dir, &mut artifacts)?;
    artifacts.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(artifacts)
}

fn collect_artifacts_inner(
    root: &Path,
    dir: &Path,
    artifacts: &mut Vec<TicketArtifactRef>,
) -> Result<()> {
    for entry in fs::read_dir(dir).map_err(|e| io_err(dir, e))? {
        let entry = entry.map_err(|e| io_err(dir, e))?;
        let path = entry.path();
        if path.is_dir() {
            collect_artifacts_inner(root, &path, artifacts)?;
        } else if path.file_name().and_then(|name| name.to_str()) != Some(".gitkeep") {
            let relative_path = path
                .strip_prefix(root)
                .map_err(|_| TicketError::PathEscapesRoot { path: path.clone() })?
                .to_path_buf();
            artifacts.push(TicketArtifactRef { relative_path });
        }
    }
    Ok(())
}

fn doctor_ticket_relations_artifact(
    path: &Path,
    meta: &TicketMeta,
    report: &mut TicketDoctorReport,
    relation_records: &mut Vec<TicketRelation>,
) -> Result<()> {
    match read_ticket_relations_artifact(path, Some(meta)) {
        Ok(relations) => {
            relation_records.extend(relations);
            Ok(())
        }
        Err(TicketError::Parse { message, .. }) => {
            report.push_error(message, Some(path.to_path_buf()));
            Ok(())
        }
        Err(TicketError::Conflict(message)) => {
            report.push_error(message, Some(path.to_path_buf()));
            Ok(())
        }
        Err(err) => Err(err),
    }
}

fn doctor_ticket_relation_references(
    relations: &[TicketRelation],
    ticket_dirs: &HashMap<String, PathBuf>,
    _states: &HashMap<String, TicketWorkflowState>,
    report: &mut TicketDoctorReport,
) {
    for relation in relations {
        let path = ticket_dirs
            .get(&relation.ticket_id)
            .map(|dir| dir.join("artifacts").join(TICKET_RELATIONS_ARTIFACT));
        if relation.ticket_id == relation.target {
            report.push_error(
                format!(
                    "ticket relation cannot target itself: {} {} {}",
                    relation.ticket_id, relation.kind, relation.target
                ),
                path.clone(),
            );
        }
        if !ticket_dirs.contains_key(&relation.target) {
            report.push_error(
                format!(
                    "ticket relation has dangling target: {} {} {}",
                    relation.ticket_id, relation.kind, relation.target
                ),
                path,
            );
        }
    }
}

fn doctor_ticket_relation_cycles(
    relations: &[TicketRelation],
    states: &HashMap<String, TicketWorkflowState>,
    report: &mut TicketDoctorReport,
) {
    let mut graph: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for relation in relations {
        if !matches!(
            relation.kind,
            TicketRelationKind::DependsOn | TicketRelationKind::Blocks
        ) {
            continue;
        }
        if !states.contains_key(&relation.ticket_id) || !states.contains_key(&relation.target) {
            continue;
        }
        let (waiter, blocker) = match relation.kind {
            TicketRelationKind::DependsOn => (&relation.ticket_id, &relation.target),
            TicketRelationKind::Blocks => (&relation.target, &relation.ticket_id),
            _ => unreachable!(),
        };
        graph
            .entry(waiter.clone())
            .or_default()
            .push(blocker.clone());
    }
    let mut reported = BTreeSet::new();
    for start in graph.keys() {
        let mut path = Vec::new();
        detect_relation_cycle(start, start, &graph, &mut path, &mut reported, report);
        if reported.len() >= 32 {
            report.push_warning(
                "ticket relation cycle diagnostics truncated after 32 cycles".to_string(),
                None,
            );
            break;
        }
    }
}

fn detect_relation_cycle(
    start: &str,
    current: &str,
    graph: &BTreeMap<String, Vec<String>>,
    path: &mut Vec<String>,
    reported: &mut BTreeSet<String>,
    report: &mut TicketDoctorReport,
) {
    if path.len() > 64 {
        return;
    }
    path.push(current.to_string());
    if let Some(nexts) = graph.get(current) {
        for next in nexts {
            if next == start {
                let mut cycle = path.clone();
                cycle.push(start.to_string());
                let key = canonical_cycle_key(&cycle);
                if reported.insert(key) {
                    report.push_error(
                        format!(
                            "ticket relation dependency/blocking cycle: {}",
                            cycle.join(" -> ")
                        ),
                        None,
                    );
                }
                continue;
            }
            if path.iter().any(|value| value == next) {
                continue;
            }
            detect_relation_cycle(start, next, graph, path, reported, report);
            if reported.len() >= 32 {
                break;
            }
        }
    }
    path.pop();
}

fn canonical_cycle_key(cycle: &[String]) -> String {
    if cycle.len() <= 1 {
        return String::new();
    }
    let nodes = &cycle[..cycle.len() - 1];
    let Some((idx, _)) = nodes.iter().enumerate().min_by(|(_, a), (_, b)| a.cmp(b)) else {
        return String::new();
    };
    let mut ordered = Vec::new();
    for offset in 0..nodes.len() {
        ordered.push(nodes[(idx + offset) % nodes.len()].clone());
    }
    ordered.join(" -> ")
}

fn doctor_orchestration_plan_artifact(
    path: &Path,
    meta: &TicketMeta,
    report: &mut TicketDoctorReport,
) -> Result<()> {
    match read_orchestration_plan_artifact(path, Some(meta)) {
        Ok(_) => Ok(()),
        Err(TicketError::Parse { message, .. }) => {
            report.push_error(message, Some(path.to_path_buf()));
            Ok(())
        }
        Err(TicketError::Conflict(message)) => {
            report.push_error(message, Some(path.to_path_buf()));
            Ok(())
        }
        Err(err) => Err(err),
    }
}

fn doctor_artifacts(dir: &Path, report: &mut TicketDoctorReport) -> Result<()> {
    for entry in fs::read_dir(dir).map_err(|e| io_err(dir, e))? {
        let entry = entry.map_err(|e| io_err(dir, e))?;
        let path = entry.path();
        if path.is_dir() {
            doctor_artifacts(&path, report)?;
        } else if path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        {
            report.push_error(
                format!("artifact path escapes artifacts/: {}", path.display()),
                Some(path),
            );
        }
    }
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or_else(|| TicketError::PathEscapesRoot {
        path: path.to_path_buf(),
    })?;
    fs::create_dir_all(parent).map_err(|e| io_err(parent, e))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| TicketError::InvalidPathComponent(path.display().to_string()))?;
    let tmp = parent.join(format!(".{file_name}.tmp.{}", std::process::id()));
    {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)
            .map_err(|e| io_err(&tmp, e))?;
        file.write_all(bytes).map_err(|e| io_err(&tmp, e))?;
        file.sync_data().map_err(|e| io_err(&tmp, e))?;
    }
    fs::rename(&tmp, path).map_err(|e| io_err(path, e))?;
    Ok(())
}

fn ensure_child_of(root: &Path, path: &Path) -> Result<()> {
    let root = root.components().collect::<Vec<_>>();
    let path_components = path.components().collect::<Vec<_>>();
    if path_components.starts_with(&root) {
        Ok(())
    } else {
        Err(TicketError::PathEscapesRoot {
            path: path.to_path_buf(),
        })
    }
}

fn ensure_safe_component(value: &str) -> Result<()> {
    let invalid = value.is_empty()
        || value == "."
        || value == ".."
        || value.contains('/')
        || value.contains('\\')
        || value.contains('\0');
    if invalid {
        Err(TicketError::InvalidPathComponent(value.to_string()))
    } else {
        Ok(())
    }
}

fn now_utc() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn compact_now_utc() -> String {
    Utc::now().format("%Y%m%d-%H%M%S").to_string()
}

fn default_author() -> String {
    std::env::var("USER").unwrap_or_else(|_| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn backend(dir: &TempDir) -> LocalTicketBackend {
        LocalTicketBackend::new(dir.path().join("tickets"))
    }

    #[test]
    fn workflow_state_rejects_legacy_intake_alias() {
        assert_eq!(
            TicketWorkflowState::parse("planning"),
            Some(TicketWorkflowState::Planning)
        );
        assert_eq!(TicketWorkflowState::parse("intake"), None);
        assert_eq!(TicketWorkflowState::Planning.as_str(), "planning");
        assert_eq!(
            TicketWorkflowState::default_for_status(&ExtensibleTicketStatus::Open),
            TicketWorkflowState::Planning
        );
    }

    #[test]
    fn workflow_state_transition_graph_allows_planning_lane_and_returns() {
        assert!(TicketWorkflowState::is_planning_ready_transition(
            TicketWorkflowState::Planning,
            TicketWorkflowState::Ready
        ));
        assert!(TicketWorkflowState::is_queue_transition(
            TicketWorkflowState::Ready,
            TicketWorkflowState::Queued
        ));
        assert!(TicketWorkflowState::is_role_transition(
            TicketWorkflowState::Queued,
            TicketWorkflowState::InProgress
        ));
        assert!(TicketWorkflowState::is_role_transition(
            TicketWorkflowState::InProgress,
            TicketWorkflowState::Done
        ));
        assert!(TicketWorkflowState::is_role_transition(
            TicketWorkflowState::Ready,
            TicketWorkflowState::Planning
        ));
        assert!(TicketWorkflowState::is_role_transition(
            TicketWorkflowState::Queued,
            TicketWorkflowState::Planning
        ));
        assert!(!TicketWorkflowState::is_role_transition(
            TicketWorkflowState::Planning,
            TicketWorkflowState::Queued
        ));
    }

    #[test]
    fn parses_item_frontmatter_and_optional_fields() {
        let item = r#"---
title: Example
state: ready
priority: P1
created_at: 2026-06-05T00:00:00Z
updated_at: 2026-06-05T00:00:00Z
assignee: null
readiness: implementation-ready
risk_flags: [low, local]
queued_by: workspace-panel
queued_at: 2026-06-05T00:01:00Z
---

## Body
"#;
        let parsed = parse_item(item).unwrap();
        let meta = ticket_meta(parsed.frontmatter, "0000000000001".to_string());
        assert_eq!(meta.id, "0000000000001");
        assert_eq!(meta.slug, "0000000000001");
        assert!(meta.labels.is_empty());
        assert_eq!(meta.readiness.as_deref(), Some("implementation-ready"));
        assert_eq!(meta.risk_flags, vec!["low", "local"]);
        assert_eq!(meta.workflow_state, TicketWorkflowState::Ready);
        assert!(meta.workflow_state_explicit);
        assert_eq!(meta.queued_by.as_deref(), Some("workspace-panel"));
        assert_eq!(meta.queued_at.as_deref(), Some("2026-06-05T00:01:00Z"));
    }

    #[test]
    fn yaml_frontmatter_preserves_typed_nulls_lists_and_quoted_strings() {
        let frontmatter = parse_ticket_frontmatter(
            r#"risk_flags: [low, local]
assignee: ~
readiness: "~"
state: planning
"#,
        )
        .unwrap();
        let meta = ticket_meta(frontmatter, "0000000000001".to_string());
        assert!(meta.labels.is_empty());
        assert_eq!(meta.risk_flags, vec!["low", "local"]);
        assert_eq!(meta.assignee, None);
        assert_eq!(meta.readiness.as_deref(), Some("~"));
        assert_eq!(meta.workflow_state, TicketWorkflowState::Planning);
        assert!(meta.workflow_state_explicit);
    }

    #[test]
    fn yaml_frontmatter_rejects_legacy_raw_string_fallbacks() {
        let labels_error = parse_ticket_frontmatter("labels: ticket").unwrap_err();
        assert!(
            labels_error.contains("must be a YAML sequence"),
            "{labels_error}"
        );

        let state_error = parse_ticket_frontmatter("state: almost").unwrap_err();
        assert!(state_error.contains("invalid state"), "{state_error}");

        let intake_error = parse_ticket_frontmatter("state: intake").unwrap_err();
        assert!(intake_error.contains("invalid state"), "{intake_error}");
    }

    #[test]
    fn yaml_frontmatter_rejects_invalid_yaml() {
        let err = parse_ticket_frontmatter("labels: [ticket").unwrap_err();
        assert!(err.contains("invalid YAML frontmatter"), "{err}");
    }

    #[test]
    fn list_query_defaults_to_active_and_supports_all_or_explicit_states() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let planning = backend.create(NewTicket::new("Planning Ticket")).unwrap();
        let mut ready_input = NewTicket::new("Ready Ticket");
        ready_input.workflow_state = Some(TicketWorkflowState::Ready);
        let ready = backend.create(ready_input).unwrap();
        let mut closed_input = NewTicket::new("Closed Ticket");
        closed_input.workflow_state = Some(TicketWorkflowState::Closed);
        let closed = backend.create(closed_input).unwrap();

        let active = backend.list(TicketListQuery::default()).unwrap();
        let active_ids = active
            .iter()
            .map(|ticket| ticket.id.as_str())
            .collect::<Vec<_>>();
        assert!(active_ids.contains(&planning.id.as_str()));
        assert!(active_ids.contains(&ready.id.as_str()));
        assert!(!active_ids.contains(&closed.id.as_str()));

        let all = backend.list(TicketListQuery::all()).unwrap();
        let all_ids = all
            .iter()
            .map(|ticket| ticket.id.as_str())
            .collect::<Vec<_>>();
        assert!(all_ids.contains(&planning.id.as_str()));
        assert!(all_ids.contains(&ready.id.as_str()));
        assert!(all_ids.contains(&closed.id.as_str()));

        let ready_only = backend
            .list(TicketListQuery::state(TicketListState::Ready))
            .unwrap();
        assert_eq!(ready_only.len(), 1);
        assert_eq!(ready_only[0].id, ready.id);

        let planning_or_closed = backend
            .list(TicketListQuery::states([
                TicketListState::Planning,
                TicketListState::Closed,
            ]))
            .unwrap();
        let explicit_ids = planning_or_closed
            .iter()
            .map(|ticket| ticket.id.as_str())
            .collect::<Vec<_>>();
        assert!(explicit_ids.contains(&planning.id.as_str()));
        assert!(explicit_ids.contains(&closed.id.as_str()));
        assert!(!explicit_ids.contains(&ready.id.as_str()));
    }

    #[test]
    fn create_writes_local_ticket_layout() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let mut input = NewTicket::new("Example Ticket");
        input.labels = vec!["ticket".into(), "backend".into()];
        let ticket = backend.create(input).unwrap();
        let dir = tmp.path().join("tickets").join(&ticket.id);
        assert!(dir.join("item.md").exists());
        assert!(dir.join("thread.md").exists());
        assert!(dir.join("artifacts/.gitkeep").exists());
        assert!(!ticket.id.contains("example"));
        assert_eq!(ticket.id.len(), project_record::RECORD_ID_WIDTH);
        validate_record_id(&ticket.id).unwrap();
        assert_eq!(ticket.slug, ticket.id);
        let item = fs::read_to_string(dir.join("item.md")).unwrap();
        assert!(
            item.contains("state: planning")
                || item.contains("state: \"planning\"")
                || item.contains("state: 'planning'")
        );
        for obsolete in [
            "id:",
            "slug:",
            "status:",
            "workflow_state:",
            "kind:",
            "labels:",
            "action_required:",
            "attention_required:",
        ] {
            assert!(
                !item.contains(obsolete),
                "obsolete field {obsolete} in {item}"
            );
        }
        assert!(!item.contains("legacy_ticket:"));
        assert!(!item.contains("needs_preflight:"));
        let record = backend.show(TicketIdOrSlug::Id(ticket.id.clone())).unwrap();
        assert_eq!(record.meta.workflow_state, TicketWorkflowState::Planning);
        assert!(record.meta.workflow_state_explicit);
        let report = backend.doctor().unwrap();
        assert!(report.is_ok(), "{:?}", report.diagnostics);
    }

    #[test]
    fn partial_list_and_show_keep_valid_tickets_when_peer_record_is_invalid() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let mut ready = NewTicket::new("Ready Valid");
        ready.workflow_state = Some(TicketWorkflowState::Ready);
        let valid = backend.create(ready).unwrap();
        let invalid = backend
            .create(NewTicket::new("Invalid Secret Title"))
            .unwrap();
        fs::write(
            backend.root().join(&invalid.id).join("item.md"),
            "---\ntitle: Invalid Secret Title\nstate: super-secret-invalid\n---\nbody\n",
        )
        .unwrap();

        assert!(backend.list(TicketListQuery::all()).is_err());

        let partial = backend.list_partial(TicketListQuery::all()).unwrap();
        assert_eq!(partial.tickets.len(), 1);
        assert_eq!(partial.tickets[0].id, valid.id);
        assert_eq!(partial.invalid_records.len(), 1);
        assert_eq!(partial.invalid_records[0].label, invalid.id);
        assert_eq!(
            partial.invalid_records[0].reason,
            "invalid ticket record schema"
        );
        assert!(
            !partial.invalid_records[0]
                .reason
                .contains("super-secret-invalid")
        );

        let detail = backend
            .show_partial(TicketIdOrSlug::Id(valid.id.clone()))
            .unwrap();
        assert_eq!(detail.ticket.meta.title, "Ready Valid");
        assert_eq!(detail.invalid_records.len(), 1);
        assert_eq!(detail.invalid_records[0].label, invalid.id);
    }

    #[test]
    fn create_uses_configured_japanese_record_language_for_generated_defaults() {
        let tmp = TempDir::new().unwrap();
        let backend = LocalTicketBackend::new(tmp.path().join("tickets"))
            .with_record_language(Some("Japanese"));

        let created = backend.create(NewTicket::new("日本語レコード")).unwrap();
        let dir = backend.root().join(created.id.as_str());
        let item = fs::read_to_string(dir.join("item.md")).unwrap();
        let thread = fs::read_to_string(dir.join("thread.md")).unwrap();

        assert!(item.contains("## 背景"));
        assert!(item.contains("LocalTicketBackend によって作成されました。"));
        assert!(thread.contains("## 作成"));
        assert!(thread.contains("LocalTicketBackend によって作成されました。"));
    }

    #[test]
    fn create_round_trips_numeric_looking_string_frontmatter_values() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let mut input = NewTicket::new("123");
        input.risk_flags = vec!["1".into(), "42".into()];
        input.assignee = Some("42".into());
        let ticket = backend.create(input).unwrap();

        let record = backend.show(TicketIdOrSlug::Id(ticket.id.clone())).unwrap();
        assert_eq!(record.meta.title, "123");
        assert!(record.meta.labels.is_empty());
        assert_eq!(record.meta.risk_flags, vec!["1", "42"]);
        assert_eq!(record.meta.assignee.as_deref(), Some("42"));

        let item = fs::read_to_string(tmp.path().join("tickets").join(&ticket.id).join("item.md"))
            .unwrap();
        assert!(item.contains("title: '123'"), "{item}");
        assert!(!item.contains("labels:"), "{item}");
        assert!(item.contains("risk_flags: ['1', '42']"), "{item}");
        assert!(item.contains("assignee: '42'"), "{item}");
        assert!(!item.contains("attention_required:"), "{item}");
        assert!(!item.contains("action_required:"), "{item}");

        let report = backend.doctor().unwrap();
        assert!(report.is_ok(), "{:?}", report.diagnostics);
    }

    #[test]
    fn add_event_review_status_and_close_preserve_local_layout() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let ticket = backend.create(NewTicket::new("Flow Ticket")).unwrap();
        backend
            .add_event(
                TicketIdOrSlug::Id(ticket.id.clone()),
                NewTicketEvent::new(TicketEventKind::Plan, "Implementation plan."),
            )
            .unwrap();
        backend
            .review(
                TicketIdOrSlug::Id(ticket.id.clone()),
                TicketReview::approve("Looks good."),
            )
            .unwrap();
        let mut summary = TicketIntakeSummary::new("Ready for queue.");
        summary.author = Some("test".to_string());
        let mut change = TicketStateChange::new(
            "planning",
            "ready",
            "ready_for_queue",
            MarkdownText::new("Ready for queue."),
        );
        change.author = Some("test".to_string());
        backend
            .mark_intake_ready(TicketIdOrSlug::Id(ticket.id.clone()), summary, change)
            .unwrap();
        let current_item = tmp.path().join("tickets").join(&ticket.id).join("item.md");
        assert!(current_item.exists());
        backend
            .close(
                TicketIdOrSlug::Id(ticket.id.clone()),
                MarkdownText::new("Done.\n"),
            )
            .unwrap();
        let closed_dir = tmp.path().join("tickets").join(&ticket.id);
        assert!(closed_dir.join("resolution.md").exists());
        let thread = fs::read_to_string(closed_dir.join("thread.md")).unwrap();
        assert!(thread.contains("<!-- event: review"));
        assert!(thread.contains("status: approve"));
        assert!(thread.contains("<!-- event: close"));
        let report = backend.doctor().unwrap();
        assert!(report.is_ok(), "{:?}", report.diagnostics);
    }

    #[test]
    fn invalid_thread_event_attributes_do_not_modify_thread() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let ticket = backend
            .create(NewTicket::new("Append Safety Ticket"))
            .unwrap();
        let thread_path = tmp
            .path()
            .join("tickets")
            .join(&ticket.id)
            .join("thread.md");
        let original = fs::read_to_string(&thread_path).unwrap();

        let mut comment = NewTicketEvent::new(TicketEventKind::Comment, "This must not append.");
        comment.author = Some("bad\nauthor".into());
        assert!(matches!(
            backend.add_event(TicketIdOrSlug::Id(ticket.id.clone()), comment),
            Err(TicketError::Conflict(_))
        ));
        assert_eq!(fs::read_to_string(&thread_path).unwrap(), original);

        let mut review = TicketReview::approve("This must not append either.");
        review.author = Some("bad-->author".into());
        assert!(matches!(
            backend.review(TicketIdOrSlug::Id(ticket.id.clone()), review),
            Err(TicketError::Conflict(_))
        ));
        assert_eq!(fs::read_to_string(&thread_path).unwrap(), original);

        let invalid_kind = NewTicketEvent::new(
            TicketEventKind::Other("bad\nevent".into()),
            "Invalid event kind.",
        );
        assert!(matches!(
            backend.add_event(TicketIdOrSlug::Id(ticket.id.clone()), invalid_kind),
            Err(TicketError::Conflict(_))
        ));
        assert_eq!(fs::read_to_string(&thread_path).unwrap(), original);
    }

    #[test]
    fn create_rejects_invalid_author_before_writing_ticket_record() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let mut input = NewTicket::new("Invalid Author Ticket");
        input.author = Some("bad-->author".into());

        assert!(matches!(
            backend.create(input),
            Err(TicketError::Conflict(_))
        ));
        let ticket_dirs = fs::read_dir(tmp.path().join("tickets"))
            .unwrap()
            .filter(|entry| entry.as_ref().is_ok_and(|entry| entry.path().is_dir()))
            .count();
        assert_eq!(ticket_dirs, 0);
    }

    #[test]
    fn state_changed_and_intake_summary_events_round_trip() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let ticket = backend
            .create(NewTicket::new("Typed Thread Ticket"))
            .unwrap();
        let mut change = TicketStateChange::new(
            "requirements-sync",
            "implementation-ready",
            "requirements approved",
            "Planning sync finished; implementation can begin.",
        );
        change.author = Some("orchestrator".into());
        backend
            .add_state_changed(TicketIdOrSlug::Id(ticket.id.clone()), change)
            .unwrap();
        let mut summary = TicketIntakeSummary::new("## Accepted intent\n\nImplement typed events.");
        summary.author = Some("intake".into());
        backend
            .add_intake_summary(TicketIdOrSlug::Id(ticket.id.clone()), summary)
            .unwrap();

        let record = backend.show(TicketIdOrSlug::Id(ticket.id.clone())).unwrap();
        let state_event = record
            .events
            .iter()
            .find(|event| event.kind == TicketEventKind::StateChanged)
            .unwrap();
        assert_eq!(state_event.from.as_deref(), Some("requirements-sync"));
        assert_eq!(state_event.to.as_deref(), Some("implementation-ready"));
        assert_eq!(state_event.reason.as_deref(), Some("requirements approved"));
        assert_eq!(state_event.author.as_deref(), Some("orchestrator"));
        assert_eq!(
            state_event.attributes.get("reason").map(String::as_str),
            Some("requirements approved")
        );
        assert!(
            record
                .events
                .iter()
                .any(|event| event.kind == TicketEventKind::IntakeSummary
                    && event.body.as_str().contains("Accepted intent"))
        );
        let thread = fs::read_to_string(
            tmp.path()
                .join("tickets")
                .join(&ticket.id)
                .join("thread.md"),
        )
        .unwrap();
        assert!(thread.contains("event: state_changed"));
        assert!(thread.contains("reason: \"requirements approved\""));
        assert!(thread.contains("event: intake_summary"));
        let report = backend.doctor().unwrap();
        assert!(report.is_ok(), "{:?}", report.diagnostics);
    }

    #[test]
    fn set_state_field_updates_frontmatter_and_appends_transition() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let ticket = backend
            .create(NewTicket::new("State Field Ticket"))
            .unwrap();
        let item = tmp.path().join("tickets").join(&ticket.id).join("item.md");
        backend
            .set_frontmatter_fields(&item, &[("readiness", "requirements-sync")])
            .unwrap();

        let mut change = TicketStateChange::new(
            "requirements-sync",
            "implementation-ready",
            "requirements accepted",
            "Implementation is authorized.",
        );
        change.author = Some("orchestrator".into());
        backend
            .set_state_field(TicketIdOrSlug::Id(ticket.id.clone()), "readiness", change)
            .unwrap();

        let record = backend.show(TicketIdOrSlug::Id(ticket.id.clone())).unwrap();
        assert_eq!(
            record.meta.readiness.as_deref(),
            Some("implementation-ready")
        );
        let event = record
            .events
            .iter()
            .find(|event| event.kind == TicketEventKind::StateChanged)
            .unwrap();
        assert_eq!(event.state_field.as_deref(), Some("readiness"));
        let stale = TicketStateChange::new(
            "requirements-sync",
            "done",
            "stale update",
            "This must be rejected.",
        );
        assert!(matches!(
            backend.set_state_field(TicketIdOrSlug::Id(ticket.id), "readiness", stale),
            Err(TicketError::Conflict(_))
        ));
    }

    #[test]
    fn state_defaults_and_queue_transition_round_trip() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let missing_meta = ticket_meta(
            parse_ticket_frontmatter("title: Missing State").expect("missing state parses"),
            "0000000000001".to_string(),
        );
        assert_eq!(missing_meta.workflow_state, TicketWorkflowState::Planning);
        assert!(!missing_meta.workflow_state_explicit);

        let closed_meta = ticket_meta(
            parse_ticket_frontmatter("state: closed").expect("closed state parses"),
            "0000000000002".to_string(),
        );
        assert_eq!(closed_meta.workflow_state, TicketWorkflowState::Closed);
        assert!(closed_meta.workflow_state_explicit);

        let mut ready_input = NewTicket::new("Ready Workflow");
        ready_input.workflow_state = Some(TicketWorkflowState::Ready);
        let ready = backend.create(ready_input).unwrap();
        backend
            .queue_ready(TicketIdOrSlug::Id(ready.id.clone()), "workspace-panel")
            .unwrap();

        let queued = backend.show(TicketIdOrSlug::Id(ready.id)).unwrap();
        assert_eq!(queued.meta.workflow_state, TicketWorkflowState::Queued);
        assert!(queued.meta.workflow_state_explicit);
        assert_eq!(queued.meta.queued_by.as_deref(), Some("workspace-panel"));
        assert!(queued.meta.queued_at.is_some());
        let event = queued
            .events
            .iter()
            .find(|event| event.kind == TicketEventKind::StateChanged)
            .unwrap();
        assert_eq!(event.state_field.as_deref(), Some("state"));
        assert_eq!(event.from.as_deref(), Some("ready"));
        assert_eq!(event.to.as_deref(), Some("queued"));
        assert_eq!(event.reason.as_deref(), Some("queued"));
    }

    #[test]
    fn workflow_queue_rejects_non_ready_ticket_without_mutation() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let ticket = backend.create(NewTicket::new("Planning Ticket")).unwrap();

        assert!(matches!(
            backend.queue_ready(TicketIdOrSlug::Id(ticket.id.clone()), "workspace-panel"),
            Err(TicketError::Conflict(_))
        ));
        let record = backend.show(TicketIdOrSlug::Id(ticket.id)).unwrap();
        assert_eq!(record.meta.workflow_state, TicketWorkflowState::Planning);
        assert!(record.meta.queued_by.is_none());
        assert!(
            !record
                .events
                .iter()
                .any(|event| event.kind == TicketEventKind::StateChanged)
        );
    }

    #[test]
    fn state_cannot_be_changed_through_generic_field_api() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let ticket = backend
            .create(NewTicket::new("Generic Workflow Bypass"))
            .unwrap();
        let change = TicketStateChange::new(
            "planning",
            "done",
            "bypass",
            "Generic field API must not mutate state.",
        );

        assert!(matches!(
            backend.set_state_field(TicketIdOrSlug::Id(ticket.id.clone()), "state", change),
            Err(TicketError::Conflict(_))
        ));
        let record = backend.show(TicketIdOrSlug::Id(ticket.id)).unwrap();
        assert_eq!(record.meta.workflow_state, TicketWorkflowState::Planning);
    }

    #[test]
    fn mark_intake_ready_records_summary_and_state_change() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let ticket = backend.create(NewTicket::new("Planning Ready")).unwrap();
        let mut summary = TicketIntakeSummary::new("Concise accepted requirements.");
        summary.author = Some("intake".to_string());
        let mut change =
            TicketStateChange::new("planning", "ready", "accepted", "Ticket is ready to queue.");
        change.author = Some("intake".to_string());

        backend
            .mark_intake_ready(TicketIdOrSlug::Id(ticket.id.clone()), summary, change)
            .unwrap();
        let record = backend.show(TicketIdOrSlug::Id(ticket.id)).unwrap();
        assert_eq!(record.meta.workflow_state, TicketWorkflowState::Ready);
        assert!(
            record
                .events
                .iter()
                .any(|event| event.kind == TicketEventKind::IntakeSummary)
        );
        assert!(record.events.iter().any(|event| {
            event.kind == TicketEventKind::StateChanged
                && event.state_field.as_deref() == Some("state")
                && event.from.as_deref() == Some("planning")
                && event.to.as_deref() == Some("ready")
        }));
    }

    #[test]
    fn close_sets_state_closed() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let mut input = NewTicket::new("Close Workflow");
        input.workflow_state = Some(TicketWorkflowState::Queued);
        let ticket = backend.create(input).unwrap();

        backend
            .close(
                TicketIdOrSlug::Id(ticket.id.clone()),
                MarkdownText::new("Completed."),
            )
            .unwrap();
        let record = backend.show(TicketIdOrSlug::Id(ticket.id)).unwrap();
        assert_eq!(record.meta.status, ExtensibleTicketStatus::Closed);
        assert_eq!(record.meta.workflow_state, TicketWorkflowState::Closed);
        assert!(record.events.iter().any(|event| {
            event.kind == TicketEventKind::StateChanged
                && event.state_field.as_deref() == Some("state")
                && event.to.as_deref() == Some("closed")
        }));
    }

    #[test]
    fn doctor_reports_invalid_state() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("tickets");
        fs::create_dir_all(root.join("0000000000001/artifacts")).unwrap();
        fs::write(
            root.join("0000000000001/item.md"),
            "---\ntitle: Bad\nstate: almost\ncreated_at: x\nupdated_at: x\n---\n",
        )
        .unwrap();
        fs::write(root.join("0000000000001/thread.md"), "").unwrap();

        let report = LocalTicketBackend::new(&root).doctor().unwrap();
        let messages = report
            .diagnostics
            .iter()
            .map(|d| d.message.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!report.is_ok());
        assert!(messages.contains("invalid state"), "{messages}");
    }

    #[test]
    fn doctor_validates_typed_thread_event_attributes() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("tickets");
        fs::create_dir_all(root.join("0000000000001/artifacts")).unwrap();
        fs::write(
            root.join("0000000000001/item.md"),
            "---\ntitle: Bad\nstate: planning\ncreated_at: x\nupdated_at: x\n---\n",
        )
        .unwrap();
        fs::write(
            root.join("0000000000001/thread.md"),
            "<!-- event: state_changed author: bot at: now from: queued -->\n\n## State changed\n\n---\n\n<!-- event: intake_summary author: bot at: now -->\n\n## Intake summary\n\n---\n",
        )
        .unwrap();
        let report = LocalTicketBackend::new(&root).doctor().unwrap();
        let messages = report
            .diagnostics
            .iter()
            .map(|d| d.message.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!report.is_ok());
        assert!(messages.contains("state_changed event missing to"));
        assert!(messages.contains("state_changed event missing reason"));
        assert!(messages.contains("intake_summary event missing body"));
    }

    #[test]
    fn doctor_reports_core_consistency_errors() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("tickets");
        fs::create_dir_all(root.join("open/legacy/artifacts")).unwrap();
        fs::write(
            root.join("open/legacy/item.md"),
            "---\ntitle: Legacy\nstate: planning\ncreated_at: x\nupdated_at: x\n---\n",
        )
        .unwrap();
        fs::write(root.join("open/legacy/thread.md"), "").unwrap();
        fs::create_dir_all(root.join("0000000000001/artifacts")).unwrap();
        fs::write(
            root.join("0000000000001/item.md"),
            "---\nid: old\nslug: old\ntitle: Bad\nstatus: pending\nworkflow_state: ready\nkind: task\nlabels: []\naction_required: human\nattention_required: true\ncreated_at: x\nupdated_at: x\n---\n",
        )
        .unwrap();
        fs::write(
            root.join("0000000000001/thread.md"),
            "<!-- event: review author: a at: now -->\n",
        )
        .unwrap();
        let report = LocalTicketBackend::new(&root).doctor().unwrap();
        let messages = report
            .diagnostics
            .iter()
            .map(|d| d.message.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!report.is_ok());
        assert!(messages.contains("legacy ticket bucket remains"));
        assert!(messages.contains("obsolete current frontmatter field 'id'"));
        assert!(messages.contains("obsolete current frontmatter field 'slug'"));
        assert!(messages.contains("obsolete current frontmatter field 'status'"));
        assert!(messages.contains("obsolete current frontmatter field 'workflow_state'"));
        assert!(messages.contains("obsolete current frontmatter field 'kind'"));
        assert!(messages.contains("obsolete current frontmatter field 'labels'"));
        assert!(messages.contains("obsolete current frontmatter field 'action_required'"));
        assert!(messages.contains("obsolete current frontmatter field 'attention_required'"));
        assert!(messages.contains("review event missing valid status"));
    }

    #[test]
    fn lock_conflict_is_reported() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        fs::create_dir_all(backend.root()).unwrap();
        let lock_path = backend.root().join(".ticket-backend.lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .unwrap();
        FileExt::lock_exclusive(&file).unwrap();
        let err = backend.create(NewTicket::new("Locked")).unwrap_err();
        FileExt::unlock(&file).unwrap();
        assert!(matches!(err, TicketError::Locked { .. }));
    }

    #[test]
    fn ticket_relations_store_forward_and_derive_inverse_blockers() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let mut ready_input = NewTicket::new("Ready Relation Source");
        ready_input.workflow_state = Some(TicketWorkflowState::Ready);
        let source = backend.create(ready_input).unwrap();
        let target = backend
            .create(NewTicket::new("Planning Dependency"))
            .unwrap();

        let relation = backend
            .add_ticket_relation(
                TicketIdOrSlug::Id(source.id.clone()),
                NewTicketRelation {
                    kind: TicketRelationKind::DependsOn,
                    target: target.id.clone(),
                    note: Some("needs dependency first".to_string()),
                    author: Some("test".to_string()),
                },
            )
            .unwrap();
        assert_eq!(relation.ticket_id, source.id);
        assert_eq!(relation.kind, TicketRelationKind::DependsOn);
        assert_eq!(relation.target, target.id);

        let source_show = backend.show(TicketIdOrSlug::Id(source.id.clone())).unwrap();
        assert_eq!(source_show.relations.outgoing.len(), 1);
        assert_eq!(source_show.relations.blockers.len(), 1);
        assert_eq!(source_show.relations.blockers[0].blocking_ticket, target.id);
        assert_eq!(source_show.relations.blockers[0].reason_kind, "depends_on");

        let target_show = backend.show(TicketIdOrSlug::Id(target.id.clone())).unwrap();
        assert_eq!(target_show.relations.incoming.len(), 1);
        assert_eq!(target_show.relations.incoming[0].source_ticket, source.id);
        assert_eq!(
            target_show.relations.incoming[0].inverse_kind,
            "dependency_of"
        );

        let queried = backend
            .query_ticket_relations(
                Some(TicketIdOrSlug::Id(target.id.clone())),
                Some(TicketRelationKind::DependsOn),
            )
            .unwrap();
        assert_eq!(queried.len(), 1);
        assert_eq!(backend.doctor().unwrap().error_count(), 0);
    }

    #[test]
    fn queue_gate_allows_ready_ticket_when_blocking_relation_is_already_queued_or_inprogress() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let mut waiter_input = NewTicket::new("Ready After Queued Dependency");
        waiter_input.workflow_state = Some(TicketWorkflowState::Ready);
        let waiter = backend.create(waiter_input).unwrap();
        let mut dependency_input = NewTicket::new("Queued Dependency");
        dependency_input.workflow_state = Some(TicketWorkflowState::Queued);
        let dependency = backend.create(dependency_input).unwrap();
        backend
            .add_ticket_relation(
                TicketIdOrSlug::Id(waiter.id.clone()),
                NewTicketRelation {
                    kind: TicketRelationKind::DependsOn,
                    target: dependency.id.clone(),
                    note: None,
                    author: Some("test".to_string()),
                },
            )
            .unwrap();

        backend
            .queue_ready(TicketIdOrSlug::Id(waiter.id.clone()), "test")
            .unwrap();
        let queued = backend.show(TicketIdOrSlug::Id(waiter.id.clone())).unwrap();
        assert_eq!(queued.meta.workflow_state, TicketWorkflowState::Queued);
        assert_eq!(queued.meta.queued_by.as_deref(), Some("test"));

        let mut incoming_input = NewTicket::new("Ready After Inprogress Blocker");
        incoming_input.workflow_state = Some(TicketWorkflowState::Ready);
        let incoming = backend.create(incoming_input).unwrap();
        let mut blocker_input = NewTicket::new("Inprogress Blocker");
        blocker_input.workflow_state = Some(TicketWorkflowState::InProgress);
        let blocker = backend.create(blocker_input).unwrap();
        backend
            .add_ticket_relation(
                TicketIdOrSlug::Id(blocker.id.clone()),
                NewTicketRelation {
                    kind: TicketRelationKind::Blocks,
                    target: incoming.id.clone(),
                    note: None,
                    author: Some("test".to_string()),
                },
            )
            .unwrap();

        backend
            .queue_ready(TicketIdOrSlug::Id(incoming.id.clone()), "test")
            .unwrap();
        let queued_incoming = backend
            .show(TicketIdOrSlug::Id(incoming.id.clone()))
            .unwrap();
        assert_eq!(
            queued_incoming.meta.workflow_state,
            TicketWorkflowState::Queued
        );
    }

    #[test]
    fn queue_gate_rejects_unresolved_dependency_and_incoming_blocker() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let mut blocked_input = NewTicket::new("Blocked Ready");
        blocked_input.workflow_state = Some(TicketWorkflowState::Ready);
        let blocked = backend.create(blocked_input).unwrap();
        let dependency = backend.create(NewTicket::new("Dependency")).unwrap();
        backend
            .add_ticket_relation(
                TicketIdOrSlug::Id(blocked.id.clone()),
                NewTicketRelation {
                    kind: TicketRelationKind::DependsOn,
                    target: dependency.id.clone(),
                    note: None,
                    author: Some("test".to_string()),
                },
            )
            .unwrap();
        let err = backend
            .queue_ready(TicketIdOrSlug::Id(blocked.id.clone()), "test")
            .unwrap_err()
            .to_string();
        assert!(err.contains("unresolved blocking relation"), "{err}");
        assert!(err.contains(&dependency.id), "{err}");

        let mut incoming_input = NewTicket::new("Incoming Blocked Ready");
        incoming_input.workflow_state = Some(TicketWorkflowState::Ready);
        let incoming = backend.create(incoming_input).unwrap();
        let blocker = backend.create(NewTicket::new("Blocker")).unwrap();
        backend
            .add_ticket_relation(
                TicketIdOrSlug::Id(blocker.id.clone()),
                NewTicketRelation {
                    kind: TicketRelationKind::Blocks,
                    target: incoming.id.clone(),
                    note: None,
                    author: Some("test".to_string()),
                },
            )
            .unwrap();
        let err = backend
            .queue_ready(TicketIdOrSlug::Id(incoming.id.clone()), "test")
            .unwrap_err()
            .to_string();
        assert!(err.contains("unresolved blocking relation"), "{err}");
        assert!(err.contains(&blocker.id), "{err}");
    }

    #[test]
    fn doctor_validates_ticket_relations() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let first = backend.create(NewTicket::new("First")).unwrap();
        let second = backend.create(NewTicket::new("Second")).unwrap();
        backend
            .add_ticket_relation(
                TicketIdOrSlug::Id(first.id.clone()),
                NewTicketRelation {
                    kind: TicketRelationKind::DependsOn,
                    target: second.id.clone(),
                    note: None,
                    author: Some("test".to_string()),
                },
            )
            .unwrap();
        backend
            .add_ticket_relation(
                TicketIdOrSlug::Id(second.id.clone()),
                NewTicketRelation {
                    kind: TicketRelationKind::DependsOn,
                    target: first.id.clone(),
                    note: None,
                    author: Some("test".to_string()),
                },
            )
            .unwrap();

        let report = backend.doctor().unwrap();
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("cycle")),
            "{:?}",
            report.diagnostics
        );

        let artifacts = tmp.path().join("tickets").join(&first.id).join("artifacts");
        fs::write(
            artifacts.join(TICKET_RELATIONS_ARTIFACT),
            format!(
                r#"{{
  "version": 1,
  "relations": [
    {{"ticket_id":"{}","kind":"related","target":"{}","author":"test","at":"2026-06-09T00:00:00Z"}}
  ]
}}
"#,
                first.id, first.id
            ),
        )
        .unwrap();
        let report = backend.doctor().unwrap();
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("ticket relation cannot target itself")
        }));

        fs::write(
            artifacts.join(TICKET_RELATIONS_ARTIFACT),
            format!(
                r#"{{
  "version": 1,
  "relations": [
    {{"ticket_id":"{}","kind":"related","target":"missing-ticket","author":"test","at":"2026-06-09T00:00:01Z"}}
  ]
}}
"#,
                first.id
            ),
        )
        .unwrap();
        let report = backend.doctor().unwrap();
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("ticket relation has dangling target")
        }));

        fs::write(
            artifacts.join(TICKET_RELATIONS_ARTIFACT),
            format!(
                r#"{{
  "version": 1,
  "relations": [
    {{"ticket_id":"{}","kind":"parent","target":"{}","author":"test","at":"2026-06-09T00:00:00Z"}}
  ]
}}
"#,
                first.id, second.id
            ),
        )
        .unwrap();
        let report = backend.doctor().unwrap();
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("invalid ticket relations artifact")
        }));
    }

    #[test]
    fn orchestration_plan_records_persist_and_query_by_ticket_and_kind() {
        let temp = TempDir::new().unwrap();
        let backend = backend(&temp);
        let first = backend.create(NewTicket::new("First ticket")).unwrap();
        let second = backend.create(NewTicket::new("Second ticket")).unwrap();

        let before = backend
            .add_orchestration_plan_record(
                TicketIdOrSlug::Id(first.id.clone()),
                NewOrchestrationPlanRecord {
                    kind: OrchestrationPlanKind::Before,
                    related_ticket: Some(second.id.clone()),
                    note: Some(
                        "First must land before second because both touch routing.".to_string(),
                    ),
                    accepted_plan: None,
                    author: Some("orchestrator".to_string()),
                },
            )
            .unwrap();
        assert_eq!(before.ticket_id, first.id);
        assert_eq!(before.kind, OrchestrationPlanKind::Before);

        backend
            .add_orchestration_plan_record(
                TicketIdOrSlug::Id(first.id.clone()),
                NewOrchestrationPlanRecord {
                    kind: OrchestrationPlanKind::AcceptedPlan,
                    related_ticket: None,
                    note: Some("Accepted during routing.".to_string()),
                    accepted_plan: Some(AcceptedOrchestrationPlan {
                        summary: "Implement in a sibling coder worktree, then review before merge."
                            .to_string(),
                        branch: Some("ticket-orchestration-plan-tool".to_string()),
                        worktree: Some(".worktree/ticket-orchestration-plan-tool".to_string()),
                        role_plan: Some(
                            "Coder implements; Reviewer checks capability boundaries.".to_string(),
                        ),
                    }),
                    author: Some("orchestrator".to_string()),
                },
            )
            .unwrap();

        let ticket_records = backend
            .query_orchestration_plan_records(Some(TicketIdOrSlug::Query(first.id.clone())), None)
            .unwrap();
        assert_eq!(ticket_records.len(), 2);
        assert!(
            ticket_records
                .iter()
                .any(|record| record.kind == OrchestrationPlanKind::AcceptedPlan)
        );

        let before_records = backend
            .query_orchestration_plan_records(None, Some(OrchestrationPlanKind::Before))
            .unwrap();
        assert_eq!(before_records.len(), 1);
        assert_eq!(
            before_records[0].related_ticket.as_deref(),
            Some(second.id.as_str())
        );

        let path = temp
            .path()
            .join("tickets")
            .join(&first.id)
            .join("artifacts")
            .join(ORCHESTRATION_PLAN_ARTIFACT);
        assert!(path.is_file());
        let content = fs::read_to_string(path).unwrap();
        assert_eq!(content.lines().count(), 2);
        assert_eq!(backend.doctor().unwrap().error_count(), 0);
    }

    #[test]
    fn orchestration_plan_validation_rejects_missing_related_ticket_and_bad_artifacts() {
        let temp = TempDir::new().unwrap();
        let backend = backend(&temp);
        let ticket = backend
            .create(NewTicket::new("Needs plan validation"))
            .unwrap();

        let err = backend
            .add_orchestration_plan_record(
                TicketIdOrSlug::Id(ticket.id.clone()),
                NewOrchestrationPlanRecord {
                    kind: OrchestrationPlanKind::BlockedBy,
                    related_ticket: None,
                    note: Some("Missing related ticket should fail.".to_string()),
                    accepted_plan: None,
                    author: None,
                },
            )
            .unwrap_err();
        assert!(err.to_string().contains("requires related_ticket"));

        let artifact = temp
            .path()
            .join("tickets")
            .join(&ticket.id)
            .join("artifacts")
            .join(ORCHESTRATION_PLAN_ARTIFACT);
        fs::write(&artifact, "{not json}\n").unwrap();
        let report = backend.doctor().unwrap();
        assert!(report.error_count() > 0);
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("invalid orchestration plan record")
        }));
    }
}
