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
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::Utc;
use fs4::fs_std::FileExt;
use project_record::{allocate_record_id, unix_epoch_millis_now, validate_record_id};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping as YamlMapping, Value as YamlValue};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub mod config;
mod sqlite_schema;
pub mod tool;

pub use sqlite_schema::{
    LATEST_SQLITE_TICKET_SCHEMA_VERSION, migrate_sqlite_ticket_resource_key_schema_in_transaction,
    migrate_sqlite_ticket_schema, migrate_sqlite_ticket_schema_through,
    verify_sqlite_ticket_schema,
};

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
    #[error("ticket target repository is required")]
    MissingTargetRepository,
    #[error("ticket target repository `{0}` is not registered in this workspace")]
    UnknownTargetRepository(String),
    #[error("ticket target selector is required for repository `{0}`")]
    MissingTargetSelector(String),
    #[error(
        "ticket target selector `{selector}` is invalid for repository `{repository_id}`: {reason}"
    )]
    InvalidTargetSelector {
        repository_id: String,
        selector: String,
        reason: String,
    },
    #[error("ticket target authority is unavailable")]
    TargetAuthorityUnavailable,
    #[error("stale ticket workflow state: expected `{expected}`, found `{actual}`")]
    StaleWorkflowState { expected: String, actual: String },
    #[error("invalid ticket workflow transition `{from}` -> `{to}`")]
    InvalidWorkflowTransition { from: String, to: String },
    #[error("ticket has unresolved blocking relations: {0}")]
    BlockingRelations(String),
    #[error(
        "ticket operation key `{operation_key}` was reused with a different request fingerprint"
    )]
    OperationFingerprintMismatch { operation_key: String },
    #[error("SQLite ticket backend error: {0}")]
    Sqlite(String),
    #[error("ticket parse error in {path}: {message}")]
    Parse { path: PathBuf, message: String },
}

fn io_err(path: impl Into<PathBuf>, source: io::Error) -> TicketError {
    TicketError::Io {
        path: path.into(),
        source,
    }
}

fn read_ticket_summary_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TicketSummary> {
    let workflow_state = row.get::<_, String>(7)?;
    Ok(TicketSummary {
        id: row.get(0)?,
        resource_key: None,
        slug: row.get(1)?,
        title: row.get(2)?,
        status: ExtensibleTicketStatus::from(row.get::<_, String>(3)?.as_str()),
        kind: row.get(4)?,
        priority: row.get(5)?,
        labels: Vec::new(),
        readiness: row.get(6)?,
        workflow_state: TicketWorkflowState::parse(&workflow_state)
            .unwrap_or(TicketWorkflowState::Planning),
        workflow_state_explicit: row.get::<_, i64>(8)? != 0,
        queued_by: row.get(9)?,
        queued_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

fn sqlite_err(error: impl std::fmt::Display) -> TicketError {
    TicketError::Sqlite(error.to_string())
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
            "review" => Self::Comment,
            "state_changed" => Self::StateChanged,
            "intake_summary" => Self::IntakeSummary,
            "status_changed" => Self::StatusChanged,
            "close" | "closed" => Self::Close,
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
    pub repository_id: Option<String>,
    pub ref_selector: Option<String>,
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
            repository_id: None,
            ref_selector: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum TicketTargetEdit {
    Set {
        repository_id: String,
        ref_selector: Option<String>,
    },
    Clear,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedTicketTarget {
    pub repository_id: String,
    pub ref_selector: String,
}

/// Workspace-owned authority used to resolve and validate implementation targets.
///
/// Ticket storage never infers repositories from cwd or repository paths. The
/// Workspace Backend supplies this boundary from its authoritative repository
/// catalog. Backends without it fail closed for ready/queue transitions.
pub trait TicketTargetAuthority: Send + Sync {
    fn resolve_target(
        &self,
        workspace_id: &str,
        repository_id: Option<&str>,
        ref_selector: Option<&str>,
    ) -> Result<ResolvedTicketTarget>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketMarkReady {
    pub operation_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intake_summary: Option<TicketIntakeSummary>,
}

impl TicketTargetEdit {
    fn validate(&self) -> Result<()> {
        if let Self::Set {
            repository_id,
            ref_selector,
        } = self
        {
            validate_ticket_target(Some(repository_id), ref_selector.as_deref())?;
        }
        Ok(())
    }
}

fn validate_ticket_target(repository_id: Option<&str>, ref_selector: Option<&str>) -> Result<()> {
    let Some(repository_id) = repository_id else {
        if ref_selector.is_some() {
            return Err(TicketError::Conflict(
                "ref_selector requires repository_id".to_string(),
            ));
        }
        return Ok(());
    };
    validate_required_event_value("repository_id", repository_id)?;
    if let Some(ref_selector) = ref_selector {
        validate_required_event_value("ref_selector", ref_selector)?;
    }
    Ok(())
}

fn resolve_ready_target(
    authority: Option<&Arc<dyn TicketTargetAuthority>>,
    workspace_id: &str,
    ticket: &Ticket,
) -> Result<ResolvedTicketTarget> {
    authority
        .ok_or(TicketError::TargetAuthorityUnavailable)?
        .resolve_target(
            workspace_id,
            ticket.meta.repository_id.as_deref(),
            ticket.meta.ref_selector.as_deref(),
        )
}

fn mark_ready_fingerprint(
    ticket: &Ticket,
    request: &TicketMarkReady,
    target: &ResolvedTicketTarget,
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"ticket.mark-ready.v1\0");
    digest.update(ticket.meta.id.as_str().as_bytes());
    digest.update(b"\0planning\0");
    digest.update(target.repository_id.as_bytes());
    digest.update(b"\0");
    digest.update(target.ref_selector.as_bytes());
    digest.update(b"\0");
    if let Some(reason) = request.reason.as_deref() {
        digest.update(reason.as_bytes());
    }
    if let Some(summary) = request.intake_summary.as_ref() {
        digest.update(b"\0intake-summary\0");
        digest.update(summary.body.as_str().as_bytes());
        for reference in &summary.references {
            digest.update(b"\0");
            digest.update(reference.kind.as_bytes());
            digest.update(b":");
            digest.update(reference.target.as_bytes());
        }
    }
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn validate_mark_ready_replay(ticket: &Ticket, request: &TicketMarkReady) -> Result<bool> {
    let Some(event) = ticket
        .events
        .iter()
        .find(|event| event.attributes.get("operation_key") == Some(&request.operation_key))
    else {
        return Ok(false);
    };
    let target = ResolvedTicketTarget {
        repository_id: event
            .attributes
            .get("repository_id")
            .cloned()
            .ok_or_else(|| {
                TicketError::Conflict("mark-ready event is missing repository_id".to_owned())
            })?,
        ref_selector: event
            .attributes
            .get("ref_selector")
            .cloned()
            .ok_or_else(|| {
                TicketError::Conflict("mark-ready event is missing ref_selector".to_owned())
            })?,
    };
    let fingerprint = mark_ready_fingerprint(ticket, request, &target);
    if event.attributes.get("request_fingerprint") != Some(&fingerprint) {
        return Err(TicketError::OperationFingerprintMismatch {
            operation_key: request.operation_key.clone(),
        });
    }
    if ticket.meta.workflow_state != TicketWorkflowState::Ready
        || ticket.meta.repository_id.as_deref() != Some(target.repository_id.as_str())
        || ticket.meta.ref_selector.as_deref() != Some(target.ref_selector.as_str())
    {
        return Err(TicketError::StaleWorkflowState {
            expected: TicketWorkflowState::Ready.as_str().to_owned(),
            actual: ticket.meta.workflow_state.as_str().to_owned(),
        });
    }
    Ok(true)
}

fn validate_generic_state_change(
    current: TicketWorkflowState,
    to: TicketWorkflowState,
) -> Result<()> {
    if current == TicketWorkflowState::Planning && to == TicketWorkflowState::Ready
        || current == TicketWorkflowState::Ready && to == TicketWorkflowState::Queued
    {
        return Err(TicketError::InvalidWorkflowTransition {
            from: current.as_str().to_owned(),
            to: to.as_str().to_owned(),
        });
    }
    if !TicketWorkflowState::is_role_transition(current, to) {
        return Err(TicketError::InvalidWorkflowTransition {
            from: current.as_str().to_owned(),
            to: to.as_str().to_owned(),
        });
    }
    Ok(())
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketItemEdit {
    pub title: Option<String>,
    /// Whole-body replacement for legacy authoring surfaces. Prefer `body_replacement`
    /// for small edits that should be validated against the current body.
    pub body: Option<MarkdownText>,
    #[serde(default)]
    pub body_replacement: Option<TicketBodyReplacement>,
    pub target: Option<TicketTargetEdit>,
    pub author: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketBodyReplacement {
    pub old_string: String,
    pub new_string: String,
    #[serde(default)]
    pub replace_all: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TicketBodyReplacementOutcome {
    body: MarkdownText,
    replacement_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TicketBodyEditAudit {
    None,
    WholeBody,
    Partial { replacement_count: usize },
}

impl TicketItemEdit {
    fn validate_body_edit_request(&self) -> Result<()> {
        if self.body.is_some() && self.body_replacement.is_some() {
            return Err(TicketError::Conflict(
                "body and body_replacement cannot both be provided".into(),
            ));
        }

        if let Some(replacement) = &self.body_replacement {
            replacement.validate()?;
        }
        if let Some(target) = &self.target {
            target.validate()?;
        }

        Ok(())
    }

    fn has_changes(&self) -> bool {
        self.title.is_some()
            || self.body.is_some()
            || self.body_replacement.is_some()
            || self.target.is_some()
    }
}

impl TicketBodyReplacement {
    fn validate(&self) -> Result<()> {
        if self.old_string.is_empty() {
            return Err(TicketError::Conflict(
                "body_replacement.old_string must not be empty".into(),
            ));
        }
        if self.old_string == self.new_string {
            return Err(TicketError::Conflict(
                "body_replacement.old_string and new_string must differ".into(),
            ));
        }
        Ok(())
    }

    fn apply(&self, current_body: &str) -> Result<TicketBodyReplacementOutcome> {
        self.validate()?;

        let replacement_count = current_body.matches(&self.old_string).count();
        if replacement_count == 0 {
            return Err(TicketError::NotFound(format!(
                "old_string not found in ticket item body"
            )));
        }
        if replacement_count > 1 && !self.replace_all {
            return Err(TicketError::Conflict(format!(
                "old_string matched {replacement_count} times; set replace_all=true to replace all occurrences"
            )));
        }

        let body = if self.replace_all {
            current_body.replace(&self.old_string, &self.new_string)
        } else {
            current_body.replacen(&self.old_string, &self.new_string, 1)
        };

        Ok(TicketBodyReplacementOutcome {
            body: MarkdownText(body),
            replacement_count,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketDependencyCheck {
    pub ticket: TicketSummary,
    pub blockers: Vec<TicketRelationBlocker>,
    pub queue_guard: TicketQueueGuard,
    pub recommended_action: TicketWorkspaceNextAction,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_key: Option<String>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketWorkspaceActionPriority {
    ReadyForQueue,
    ActiveWork,
    Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketWorkspaceNextAction {
    Clarify,
    QueueForOrchestrator,
    Close,
    WaitForOrchestrator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketWorkspaceRowKind {
    Planning,
    Ticket,
    Review,
    ActiveWork,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketWorkspaceStateOverlay {
    pub source: String,
    pub workflow_state: TicketWorkflowState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketQueueGuard {
    pub can_queue_for_orchestrator: bool,
    pub reason: Option<String>,
    pub blocked_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketWorkspaceProjection {
    pub kind: TicketWorkspaceRowKind,
    pub priority: TicketWorkspaceActionPriority,
    pub next_action: Option<TicketWorkspaceNextAction>,
    pub visible_state: String,
    pub visible_overlay: Option<TicketWorkspaceStateOverlay>,
    pub disabled_reason: Option<String>,
    pub key_hint: Option<String>,
    pub blocked_reason: Option<String>,
    pub queue_guard: TicketQueueGuard,
}

pub fn project_ticket_workspace_item(
    summary: &TicketSummary,
    relation_blockers: &[TicketRelationBlocker],
    orchestration_overlay: Option<&TicketWorkspaceStateOverlay>,
) -> TicketWorkspaceProjection {
    let visible_overlay = orchestration_overlay
        .filter(|overlay| {
            ticket_overlay_state_has_progressed(summary.workflow_state, overlay.workflow_state)
        })
        .cloned();
    let mut projection = derive_ticket_workspace_projection(summary, relation_blockers);
    if let Some(overlay) = visible_overlay.as_ref() {
        apply_workspace_overlay_to_projection(&mut projection, summary.workflow_state, overlay);
    }
    projection.visible_state =
        ticket_workspace_state_display(summary.workflow_state, visible_overlay.as_ref());
    projection.visible_overlay = visible_overlay;
    projection.queue_guard = ticket_queue_guard(
        summary,
        relation_blockers,
        projection.visible_overlay.as_ref(),
    );
    projection
}

pub fn ticket_queue_guard(
    summary: &TicketSummary,
    relation_blockers: &[TicketRelationBlocker],
    orchestration_overlay: Option<&TicketWorkspaceStateOverlay>,
) -> TicketQueueGuard {
    if orchestration_overlay.is_some() {
        return TicketQueueGuard {
            can_queue_for_orchestrator: false,
            reason: Some(
                "orchestration overlay already shows progress; duplicate queue is suppressed"
                    .to_string(),
            ),
            blocked_reason: None,
        };
    }
    if summary.workflow_state != TicketWorkflowState::Ready {
        return TicketQueueGuard {
            can_queue_for_orchestrator: false,
            reason: Some(format!(
                "Ticket state is {}; only ready Tickets can be queued for Orchestrator",
                summary.workflow_state.as_str()
            )),
            blocked_reason: None,
        };
    }
    let active_blockers = relation_blockers
        .iter()
        .filter(|blocker| !relation_blocker_allows_ready_queue(blocker))
        .collect::<Vec<_>>();
    if !active_blockers.is_empty() {
        let blockers = format_workspace_relation_blockers(&active_blockers);
        return TicketQueueGuard {
            can_queue_for_orchestrator: false,
            reason: Some(format!("waiting for {blockers}")),
            blocked_reason: Some(blockers),
        };
    }
    TicketQueueGuard {
        can_queue_for_orchestrator: true,
        reason: None,
        blocked_reason: None,
    }
}

fn derive_ticket_workspace_projection(
    summary: &TicketSummary,
    relation_blockers: &[TicketRelationBlocker],
) -> TicketWorkspaceProjection {
    if !relation_blockers.is_empty() {
        let active_blockers = relation_blockers
            .iter()
            .filter(|blocker| !relation_blocker_allows_ready_queue(blocker))
            .collect::<Vec<_>>();
        if !active_blockers.is_empty() || summary.workflow_state != TicketWorkflowState::Ready {
            let blockers_to_report = if active_blockers.is_empty() {
                relation_blockers.iter().collect::<Vec<_>>()
            } else {
                active_blockers
            };
            let blockers = format_workspace_relation_blockers(&blockers_to_report);
            let waiting_reason = format!("waiting for {blockers}");
            return TicketWorkspaceProjection {
                kind: workspace_row_kind_for_state(summary.workflow_state),
                priority: match summary.workflow_state {
                    TicketWorkflowState::Queued | TicketWorkflowState::InProgress => {
                        TicketWorkspaceActionPriority::ActiveWork
                    }
                    _ => TicketWorkspaceActionPriority::Background,
                },
                next_action: Some(TicketWorkspaceNextAction::WaitForOrchestrator),
                visible_state: summary.workflow_state.as_str().to_string(),
                visible_overlay: None,
                disabled_reason: Some(format!(
                    "Queue disabled: {waiting_reason}. Resolve dependency/blocker before ready -> queued."
                )),
                key_hint: Some(format!("Gate: {waiting_reason}")),
                blocked_reason: Some(blockers),
                queue_guard: TicketQueueGuard {
                    can_queue_for_orchestrator: false,
                    reason: Some(waiting_reason),
                    blocked_reason: None,
                },
            };
        }

        let blockers = format_workspace_relation_blockers(
            &relation_blockers
                .iter()
                .collect::<Vec<&TicketRelationBlocker>>(),
        );
        return TicketWorkspaceProjection {
            kind: TicketWorkspaceRowKind::Ticket,
            priority: TicketWorkspaceActionPriority::ReadyForQueue,
            next_action: Some(TicketWorkspaceNextAction::QueueForOrchestrator),
            visible_state: summary.workflow_state.as_str().to_string(),
            visible_overlay: None,
            disabled_reason: None,
            key_hint: Some(format!(
                "Queue allowed: prerequisites are already queued/in progress; Orchestrator will preserve order ({blockers})."
            )),
            blocked_reason: None,
            queue_guard: TicketQueueGuard {
                can_queue_for_orchestrator: true,
                reason: None,
                blocked_reason: None,
            },
        };
    }

    match summary.workflow_state {
        TicketWorkflowState::Ready => TicketWorkspaceProjection {
            kind: TicketWorkspaceRowKind::Ticket,
            priority: TicketWorkspaceActionPriority::ReadyForQueue,
            next_action: Some(TicketWorkspaceNextAction::QueueForOrchestrator),
            visible_state: summary.workflow_state.as_str().to_string(),
            visible_overlay: None,
            disabled_reason: None,
            key_hint: Some(
                "Queue transitions ready -> queued and may notify Orchestrator".to_string(),
            ),
            blocked_reason: None,
            queue_guard: TicketQueueGuard {
                can_queue_for_orchestrator: true,
                reason: None,
                blocked_reason: None,
            },
        },
        TicketWorkflowState::Queued => TicketWorkspaceProjection {
            kind: TicketWorkspaceRowKind::ActiveWork,
            priority: TicketWorkspaceActionPriority::ActiveWork,
            next_action: Some(TicketWorkspaceNextAction::WaitForOrchestrator),
            visible_state: summary.workflow_state.as_str().to_string(),
            visible_overlay: None,
            disabled_reason: Some("Ticket is queued for Orchestrator routing.".to_string()),
            key_hint: None,
            blocked_reason: None,
            queue_guard: TicketQueueGuard {
                can_queue_for_orchestrator: false,
                reason: Some("Ticket is already queued for Orchestrator routing".to_string()),
                blocked_reason: None,
            },
        },
        TicketWorkflowState::InProgress => TicketWorkspaceProjection {
            kind: TicketWorkspaceRowKind::ActiveWork,
            priority: TicketWorkspaceActionPriority::ActiveWork,
            next_action: Some(TicketWorkspaceNextAction::WaitForOrchestrator),
            visible_state: summary.workflow_state.as_str().to_string(),
            visible_overlay: None,
            disabled_reason: Some("Ticket is already in progress.".to_string()),
            key_hint: None,
            blocked_reason: None,
            queue_guard: TicketQueueGuard {
                can_queue_for_orchestrator: false,
                reason: Some("Ticket is already in progress".to_string()),
                blocked_reason: None,
            },
        },
        TicketWorkflowState::Done => TicketWorkspaceProjection {
            kind: TicketWorkspaceRowKind::Review,
            priority: TicketWorkspaceActionPriority::Background,
            next_action: Some(TicketWorkspaceNextAction::Close),
            visible_state: summary.workflow_state.as_str().to_string(),
            visible_overlay: None,
            disabled_reason: Some(
                "state is done; close if a resolution is still missing.".to_string(),
            ),
            key_hint: None,
            blocked_reason: None,
            queue_guard: TicketQueueGuard {
                can_queue_for_orchestrator: false,
                reason: Some("Ticket is done; close or review instead of queueing".to_string()),
                blocked_reason: None,
            },
        },
        TicketWorkflowState::Planning => TicketWorkspaceProjection {
            kind: TicketWorkspaceRowKind::Planning,
            priority: TicketWorkspaceActionPriority::Background,
            next_action: Some(TicketWorkspaceNextAction::Clarify),
            visible_state: summary.workflow_state.as_str().to_string(),
            visible_overlay: None,
            disabled_reason: Some(
                "Ticket is still in planning; mark it ready before queueing.".to_string(),
            ),
            key_hint: Some("Planning/Intake helpers can set state = ready".to_string()),
            blocked_reason: None,
            queue_guard: TicketQueueGuard {
                can_queue_for_orchestrator: false,
                reason: Some("Ticket is still in planning".to_string()),
                blocked_reason: None,
            },
        },
        TicketWorkflowState::Closed => TicketWorkspaceProjection {
            kind: TicketWorkspaceRowKind::Review,
            priority: TicketWorkspaceActionPriority::Background,
            next_action: Some(TicketWorkspaceNextAction::WaitForOrchestrator),
            visible_state: summary.workflow_state.as_str().to_string(),
            visible_overlay: None,
            disabled_reason: Some("Ticket is closed.".to_string()),
            key_hint: None,
            blocked_reason: None,
            queue_guard: TicketQueueGuard {
                can_queue_for_orchestrator: false,
                reason: Some("Ticket is closed".to_string()),
                blocked_reason: None,
            },
        },
    }
}

fn workspace_row_kind_for_state(state: TicketWorkflowState) -> TicketWorkspaceRowKind {
    match state {
        TicketWorkflowState::Planning => TicketWorkspaceRowKind::Planning,
        TicketWorkflowState::Queued | TicketWorkflowState::InProgress => {
            TicketWorkspaceRowKind::ActiveWork
        }
        TicketWorkflowState::Done | TicketWorkflowState::Closed => TicketWorkspaceRowKind::Review,
        TicketWorkflowState::Ready => TicketWorkspaceRowKind::Ticket,
    }
}

fn apply_workspace_overlay_to_projection(
    projection: &mut TicketWorkspaceProjection,
    local: TicketWorkflowState,
    overlay: &TicketWorkspaceStateOverlay,
) {
    projection.next_action = Some(TicketWorkspaceNextAction::WaitForOrchestrator);
    let overlay_state = overlay.workflow_state.as_str();
    match overlay.workflow_state {
        TicketWorkflowState::Done | TicketWorkflowState::Closed => {
            projection.kind = TicketWorkspaceRowKind::Review;
            projection.priority = TicketWorkspaceActionPriority::Background;
            projection.disabled_reason = Some(format!(
                "{} worktree overlay shows Ticket state {overlay_state}; local state remains {} until merge/review/close authority updates the current branch.",
                overlay.source,
                local.as_str()
            ));
            projection.key_hint = Some(format!(
                "Merge pending: local: {} · {}: {overlay_state}",
                local.as_str(),
                overlay.source
            ));
        }
        TicketWorkflowState::InProgress | TicketWorkflowState::Queued => {
            projection.kind = TicketWorkspaceRowKind::ActiveWork;
            projection.priority = TicketWorkspaceActionPriority::ActiveWork;
            projection.disabled_reason = Some(format!(
                "{} worktree overlay shows Ticket state {overlay_state}; local state remains {} and duplicate queue/start actions are suppressed.",
                overlay.source,
                local.as_str()
            ));
            projection.key_hint = Some(format!(
                "Progress overlay: local: {} · {}: {overlay_state}",
                local.as_str(),
                overlay.source
            ));
        }
        TicketWorkflowState::Planning | TicketWorkflowState::Ready => {}
    }
}

fn ticket_workspace_state_display(
    local: TicketWorkflowState,
    overlay: Option<&TicketWorkspaceStateOverlay>,
) -> String {
    match overlay {
        Some(overlay) => format!(
            "{}→{}",
            compact_ticket_state_label(local),
            compact_ticket_state_label(overlay.workflow_state)
        ),
        None => local.as_str().to_string(),
    }
}

fn ticket_overlay_state_has_progressed(
    local: TicketWorkflowState,
    overlay: TicketWorkflowState,
) -> bool {
    workflow_state_progress_rank(overlay) > workflow_state_progress_rank(local)
}

fn workflow_state_progress_rank(state: TicketWorkflowState) -> u8 {
    match state {
        TicketWorkflowState::Planning => 0,
        TicketWorkflowState::Ready => 1,
        TicketWorkflowState::Queued => 2,
        TicketWorkflowState::InProgress => 3,
        TicketWorkflowState::Done => 4,
        TicketWorkflowState::Closed => 5,
    }
}

fn compact_ticket_state_label(state: TicketWorkflowState) -> &'static str {
    match state {
        TicketWorkflowState::Planning => "plan",
        TicketWorkflowState::Ready => "ready",
        TicketWorkflowState::Queued => "q",
        TicketWorkflowState::InProgress => "prog",
        TicketWorkflowState::Done => "done",
        TicketWorkflowState::Closed => "cls",
    }
}

fn relation_blocker_allows_ready_queue(blocker: &TicketRelationBlocker) -> bool {
    matches!(
        blocker.blocking_state,
        TicketWorkflowState::Queued | TicketWorkflowState::InProgress
    )
}

fn format_workspace_relation_blockers(blockers: &[&TicketRelationBlocker]) -> String {
    let shown_blockers = blockers.iter().take(3).count();
    let mut formatted = blockers
        .iter()
        .take(3)
        .map(|blocker| {
            format!(
                "{} via {} (state: {})",
                blocker.blocking_ticket,
                blocker.reason_kind,
                blocker.blocking_state.as_str()
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let remaining_blockers = blockers.len().saturating_sub(shown_blockers);
    if remaining_blockers > 0 {
        formatted.push_str(&format!(" (+{remaining_blockers} more)"));
    }
    formatted
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_key: Option<String>,
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
    pub repository_id: Option<String>,
    pub ref_selector: Option<String>,
    pub raw: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketSummary {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_key: Option<String>,
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

/// Bounded SQLite list projection used by Workspace list surfaces.
///
/// This intentionally contains only summary fields and relation blockers. Full Ticket bodies,
/// events, references, and artifacts remain exclusive to [`TicketBackend::show`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqliteTicketListItem {
    pub summary: TicketSummary,
    pub relation_blockers: Vec<TicketRelationBlocker>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SqliteTicketListProjection {
    pub items: Vec<SqliteTicketListItem>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SqliteTicketListCursor {
    pub state_rank: i64,
    pub updated_at: Option<String>,
    pub ticket_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SqliteTicketListPageQuery {
    pub states: Vec<TicketWorkflowState>,
    pub limit: usize,
    pub after: Option<SqliteTicketListCursor>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SqliteTicketListPage {
    pub items: Vec<SqliteTicketListItem>,
    pub has_more: bool,
    pub next: Option<SqliteTicketListCursor>,
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
    fn edit_item(&self, id: TicketIdOrSlug, edit: TicketItemEdit) -> Result<Ticket>;
    fn dependency_check(&self, id: TicketIdOrSlug) -> Result<TicketDependencyCheck>;
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
    fn mark_ready(&self, id: TicketIdOrSlug, request: TicketMarkReady) -> Result<Ticket>;
    fn queue_ready(&self, id: TicketIdOrSlug, queued_by: &str) -> Result<()>;
    fn close(&self, id: TicketIdOrSlug, resolution: MarkdownText) -> Result<()>;
    fn add_ticket_relation(
        &self,
        id: TicketIdOrSlug,
        relation: NewTicketRelation,
    ) -> Result<TicketRelation>;
    fn remove_ticket_relation(
        &self,
        id: TicketIdOrSlug,
        kind: TicketRelationKind,
        target: TicketIdOrSlug,
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
    EditItem {
        id: TicketIdOrSlug,
        edit: TicketItemEdit,
    },
    DependencyCheck {
        id: TicketIdOrSlug,
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
    MarkReady {
        id: TicketIdOrSlug,
        request: TicketMarkReady,
    },
    QueueReady {
        id: TicketIdOrSlug,
        queued_by: String,
    },
    Close {
        id: TicketIdOrSlug,
        resolution: MarkdownText,
    },
    AddTicketRelation {
        id: TicketIdOrSlug,
        relation: NewTicketRelation,
    },
    RemoveTicketRelation {
        id: TicketIdOrSlug,
        kind: TicketRelationKind,
        target: TicketIdOrSlug,
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
    DependencyCheck(TicketDependencyCheck),
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
        TicketBackendOperation::EditItem { id, edit } => {
            TicketBackendOperationResult::Ticket(backend.edit_item(id, edit)?)
        }
        TicketBackendOperation::DependencyCheck { id } => {
            TicketBackendOperationResult::DependencyCheck(backend.dependency_check(id)?)
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
        TicketBackendOperation::MarkReady { id, request } => {
            TicketBackendOperationResult::Ticket(backend.mark_ready(id, request)?)
        }
        TicketBackendOperation::QueueReady { id, queued_by } => {
            backend.queue_ready(id, &queued_by)?;
            TicketBackendOperationResult::Unit
        }
        TicketBackendOperation::Close { id, resolution } => {
            backend.close(id, resolution)?;
            TicketBackendOperationResult::Unit
        }
        TicketBackendOperation::AddTicketRelation { id, relation } => {
            TicketBackendOperationResult::Relation(backend.add_ticket_relation(id, relation)?)
        }
        TicketBackendOperation::RemoveTicketRelation { id, kind, target } => {
            TicketBackendOperationResult::Relation(
                backend.remove_ticket_relation(id, kind, target)?,
            )
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

#[derive(Clone)]
pub struct LocalTicketBackend {
    root: PathBuf,
    record_language: Option<String>,
    target_authority: Option<Arc<dyn TicketTargetAuthority>>,
}

impl fmt::Debug for LocalTicketBackend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalTicketBackend")
            .field("root", &self.root)
            .field("record_language", &self.record_language)
            .field(
                "target_authority",
                &self.target_authority.as_ref().map(|_| "configured"),
            )
            .finish()
    }
}

impl LocalTicketBackend {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            record_language: None,
            target_authority: None,
        }
    }

    pub fn with_record_language(mut self, language: Option<&str>) -> Self {
        self.record_language = language.and_then(normalized_record_language);
        self
    }

    pub fn with_target_authority(mut self, authority: Arc<dyn TicketTargetAuthority>) -> Self {
        self.target_authority = Some(authority);
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
        self.append_state_changed_event_with_attributes(dir, change, state_field, &[])
    }

    fn append_state_changed_event_with_attributes(
        &self,
        dir: &Path,
        change: &TicketStateChange,
        state_field: Option<&str>,
        extra_attributes: &[(&str, &str)],
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
        attrs.extend_from_slice(extra_attributes);
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqliteTicketMutationEvent {
    pub workspace_id: String,
    pub ticket_id: String,
    pub event_index: i64,
    pub event_kind: TicketEventKind,
}

pub type SqliteTicketMutationHook =
    dyn Fn(&Connection, &SqliteTicketMutationEvent) -> Result<()> + Send + Sync;

#[derive(Clone)]
pub struct SqliteTicketBackend {
    db_path: PathBuf,
    workspace_id: String,
    record_language: Option<String>,
    event_attributes: BTreeMap<String, String>,
    mutation_hook: Option<Arc<SqliteTicketMutationHook>>,
    target_authority: Option<Arc<dyn TicketTargetAuthority>>,
    #[cfg(test)]
    full_ticket_load_count: Arc<AtomicUsize>,
}

impl fmt::Debug for SqliteTicketBackend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SqliteTicketBackend")
            .field("db_path", &self.db_path)
            .field("workspace_id", &self.workspace_id)
            .field("record_language", &self.record_language)
            .field("event_attributes", &self.event_attributes)
            .field(
                "mutation_hook",
                &self.mutation_hook.as_ref().map(|_| "configured"),
            )
            .field(
                "target_authority",
                &self.target_authority.as_ref().map(|_| "configured"),
            )
            .finish()
    }
}

impl SqliteTicketBackend {
    fn configured(db_path: impl Into<PathBuf>, workspace_id: impl Into<String>) -> Self {
        Self {
            db_path: db_path.into(),
            workspace_id: workspace_id.into(),
            record_language: None,
            event_attributes: BTreeMap::new(),
            mutation_hook: None,
            target_authority: None,
            #[cfg(test)]
            full_ticket_load_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Opens a standalone Ticket backend, applying all Ticket-owned migrations once.
    pub fn open(db_path: impl Into<PathBuf>, workspace_id: impl Into<String>) -> Result<Self> {
        let backend = Self::configured(db_path, workspace_id);
        let connection = backend.connect()?;
        migrate_sqlite_ticket_schema(&connection)?;
        Ok(backend)
    }

    /// Connects to a database whose Ticket schema was composed by its startup owner.
    ///
    /// This performs verification only and never creates or alters schema objects.
    pub fn open_verified(
        db_path: impl Into<PathBuf>,
        workspace_id: impl Into<String>,
    ) -> Result<Self> {
        let backend = Self::configured(db_path, workspace_id);
        let connection = backend.connect()?;
        verify_sqlite_ticket_schema(&connection)?;
        Ok(backend)
    }

    pub fn with_event_attributes(mut self, attributes: BTreeMap<String, String>) -> Self {
        self.event_attributes = attributes;
        self
    }

    pub fn with_mutation_hook(mut self, hook: Arc<SqliteTicketMutationHook>) -> Self {
        self.mutation_hook = Some(hook);
        self
    }

    pub fn with_record_language(mut self, language: Option<&str>) -> Self {
        self.record_language = language.and_then(normalized_record_language);
        self
    }

    pub fn with_target_authority(mut self, authority: Arc<dyn TicketTargetAuthority>) -> Self {
        self.target_authority = Some(authority);
        self
    }

    pub fn db_path(&self) -> &Path {
        self.db_path.as_path()
    }
    pub fn workspace_id(&self) -> &str {
        self.workspace_id.as_str()
    }
    pub fn record_language(&self) -> Option<&str> {
        self.record_language.as_deref()
    }

    /// Lists a bounded Workspace projection in one verified SQLite read.
    ///
    /// The summary query applies `updated_at DESC, ticket_id ASC` and `limit` before relation
    /// enrichment. A second bulk query loads only relations touching those returned Tickets,
    /// including the blocking Ticket states needed to preserve relation blocker semantics.
    pub fn list_workspace_projection(&self, limit: usize) -> Result<SqliteTicketListProjection> {
        self.with_read(|conn| {
            let summaries = self.list_workspace_summaries(conn, limit)?;
            if summaries.is_empty() {
                return Ok(SqliteTicketListProjection::default());
            }
            let blockers = self.list_workspace_blockers(conn, &summaries)?;
            Ok(SqliteTicketListProjection {
                items: summaries
                    .into_iter()
                    .map(|summary| {
                        let relation_blockers =
                            blockers.get(&summary.id).cloned().unwrap_or_default();
                        SqliteTicketListItem {
                            summary,
                            relation_blockers,
                        }
                    })
                    .collect(),
            })
        })
    }

    /// Lists one stable keyset-paginated Workspace summary page.
    ///
    /// Filtering, ordering, and `limit + 1` are applied by SQLite before the bounded blocker
    /// hydration query. The returned cursor is storage data; callers must wrap it in their own
    /// opaque, query-bound transport cursor.
    pub fn list_workspace_projection_page(
        &self,
        query: SqliteTicketListPageQuery,
    ) -> Result<SqliteTicketListPage> {
        self.with_read(|conn| {
            let states = serde_json::to_string(
                &query
                    .states
                    .iter()
                    .map(|state| state.as_str())
                    .collect::<Vec<_>>(),
            )
            .map_err(|error| TicketError::Sqlite(error.to_string()))?;
            let cursor_rank = query.after.as_ref().map(|cursor| cursor.state_rank);
            let cursor_updated_at = query
                .after
                .as_ref()
                .and_then(|cursor| cursor.updated_at.clone());
            let cursor_id = query
                .after
                .as_ref()
                .map(|cursor| cursor.ticket_id.as_str());
            let fetch_limit = query.limit.saturating_add(1);
            let mut statement = conn
                .prepare(
                    "SELECT ticket_id, slug, title, status, kind, priority, readiness,
                            workflow_state, workflow_state_explicit, queued_by, queued_at, updated_at
                     FROM typed_tickets AS ticket
                     WHERE workspace_id = ?1
                       AND (json_array_length(?2) = 0 OR EXISTS (
                           SELECT 1 FROM json_each(?2) AS state
                           WHERE state.value = ticket.workflow_state
                       ))
                       AND (?3 IS NULL OR
                            CASE ticket.workflow_state
                              WHEN 'ready' THEN 0 WHEN 'planning' THEN 1
                              WHEN 'inprogress' THEN 2 WHEN 'queued' THEN 3
                              WHEN 'done' THEN 4 WHEN 'closed' THEN 5 ELSE 6 END > ?4
                            OR (CASE ticket.workflow_state
                              WHEN 'ready' THEN 0 WHEN 'planning' THEN 1
                              WHEN 'inprogress' THEN 2 WHEN 'queued' THEN 3
                              WHEN 'done' THEN 4 WHEN 'closed' THEN 5 ELSE 6 END = ?4
                              AND (COALESCE(ticket.updated_at, '') < COALESCE(?5, '')
                                OR (COALESCE(ticket.updated_at, '') = COALESCE(?5, '')
                                  AND ticket.ticket_id > ?3))))
                     ORDER BY CASE ticket.workflow_state
                                WHEN 'ready' THEN 0 WHEN 'planning' THEN 1
                                WHEN 'inprogress' THEN 2 WHEN 'queued' THEN 3
                                WHEN 'done' THEN 4 WHEN 'closed' THEN 5 ELSE 6 END ASC,
                              ticket.updated_at DESC, ticket.ticket_id ASC
                     LIMIT ?6",
                )
                .map_err(sqlite_err)?;
            let rows = statement
                .query_map(
                    params![
                        self.workspace_id,
                        states,
                        cursor_id,
                        cursor_rank,
                        cursor_updated_at,
                        i64::try_from(fetch_limit).unwrap_or(i64::MAX)
                    ],
                    read_ticket_summary_row,
                )
                .map_err(sqlite_err)?;
            let mut summaries = rows
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(sqlite_err)?;
            for summary in &mut summaries {
                summary.resource_key = Self::resource_key_for(conn, &self.workspace_id, &summary.id)?;
            }
            let has_more = summaries.len() > query.limit;
            summaries.truncate(query.limit);
            let next = has_more.then(|| {
                let summary = summaries.last().expect("non-empty page with continuation");
                SqliteTicketListCursor {
                    state_rank: match summary.workflow_state {
                        TicketWorkflowState::Ready => 0,
                        TicketWorkflowState::Planning => 1,
                        TicketWorkflowState::InProgress => 2,
                        TicketWorkflowState::Queued => 3,
                        TicketWorkflowState::Done => 4,
                        TicketWorkflowState::Closed => 5,
                    },
                    updated_at: summary.updated_at.clone(),
                    ticket_id: summary.id.clone(),
                }
            });
            let blockers = self.list_workspace_blockers(conn, &summaries)?;
            Ok(SqliteTicketListPage {
                items: summaries
                    .into_iter()
                    .map(|summary| SqliteTicketListItem {
                        relation_blockers: blockers.get(&summary.id).cloned().unwrap_or_default(),
                        summary,
                    })
                    .collect(),
                has_more,
                next,
            })
        })
    }

    fn list_workspace_summaries(
        &self,
        conn: &Connection,
        limit: usize,
    ) -> Result<Vec<TicketSummary>> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut statement = conn
            .prepare(
                "SELECT ticket_id, slug, title, status, kind, priority, readiness,
                        workflow_state, workflow_state_explicit, queued_by, queued_at, updated_at
                 FROM typed_tickets
                 WHERE workspace_id = ?1
                 ORDER BY updated_at DESC, ticket_id ASC
                 LIMIT ?2",
            )
            .map_err(sqlite_err)?;
        let rows = statement
            .query_map(params![self.workspace_id, limit], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, i64>(8)? != 0,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                ))
            })
            .map_err(sqlite_err)?;
        let mut summaries = Vec::new();
        for row in rows {
            let (
                id,
                slug,
                title,
                status,
                kind,
                priority,
                readiness,
                workflow_state,
                workflow_state_explicit,
                queued_by,
                queued_at,
                updated_at,
            ) = row.map_err(sqlite_err)?;
            summaries.push(TicketSummary {
                resource_key: Self::resource_key_for(conn, &self.workspace_id, &id)?,
                id,
                slug,
                title,
                status: ExtensibleTicketStatus::from(status.as_str()),
                kind,
                priority,
                labels: Vec::new(),
                readiness,
                workflow_state: TicketWorkflowState::parse(&workflow_state)
                    .unwrap_or(TicketWorkflowState::Planning),
                workflow_state_explicit,
                queued_by,
                queued_at,
                updated_at,
            });
        }
        Ok(summaries)
    }

    fn list_workspace_blockers(
        &self,
        conn: &Connection,
        summaries: &[TicketSummary],
    ) -> Result<HashMap<String, Vec<TicketRelationBlocker>>> {
        let listed_ids = summaries
            .iter()
            .map(|summary| summary.id.as_str())
            .collect::<BTreeSet<_>>();
        let mut statement = conn
            .prepare(
                "SELECT relation.ticket_id, relation.kind, relation.target, relation.note,
                        source.workflow_state, target.workflow_state
                 FROM typed_ticket_relations AS relation
                 LEFT JOIN typed_tickets AS source
                   ON source.workspace_id = relation.workspace_id
                  AND source.ticket_id = relation.ticket_id
                 LEFT JOIN typed_tickets AS target
                   ON target.workspace_id = relation.workspace_id
                  AND target.ticket_id = relation.target
                 WHERE relation.workspace_id = ?1
                   AND relation.kind IN ('depends_on', 'blocks')
                   AND EXISTS (
                       SELECT 1 FROM json_each(?2) AS listed
                       WHERE listed.value = relation.ticket_id
                          OR listed.value = relation.target
                   )",
            )
            .map_err(sqlite_err)?;
        let listed_ids_json = serde_json::to_string(
            &summaries
                .iter()
                .map(|summary| summary.id.as_str())
                .collect::<Vec<_>>(),
        )
        .map_err(|error| TicketError::Sqlite(error.to_string()))?;
        let rows = statement
            .query_map(params![self.workspace_id, listed_ids_json], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            })
            .map_err(sqlite_err)?;
        let mut blockers = HashMap::<String, Vec<TicketRelationBlocker>>::new();
        for row in rows {
            let (source, kind, target, note, source_state, target_state) =
                row.map_err(sqlite_err)?;
            let (listed_ticket, blocking_ticket, reason_kind, relation_kind, blocking_state) =
                match kind.as_str() {
                    "depends_on" if listed_ids.contains(source.as_str()) => (
                        source,
                        target,
                        "depends_on",
                        TicketRelationKind::DependsOn,
                        target_state,
                    ),
                    "blocks" if listed_ids.contains(target.as_str()) => (
                        target,
                        source,
                        "blocked_by",
                        TicketRelationKind::Blocks,
                        source_state,
                    ),
                    _ => continue,
                };
            let blocking_state = blocking_state
                .as_deref()
                .and_then(TicketWorkflowState::parse)
                .unwrap_or(TicketWorkflowState::Planning);
            if ticket_state_resolved(blocking_state) {
                continue;
            }
            blockers
                .entry(listed_ticket)
                .or_default()
                .push(TicketRelationBlocker {
                    blocking_ticket,
                    reason_kind: reason_kind.to_string(),
                    relation_kind,
                    note,
                    blocking_state,
                });
        }
        for ticket_blockers in blockers.values_mut() {
            ticket_blockers.sort_by(|a, b| {
                a.reason_kind
                    .cmp(&b.reason_kind)
                    .then_with(|| a.blocking_ticket.cmp(&b.blocking_ticket))
            });
        }
        Ok(blockers)
    }

    pub fn import_from_local_backend(&self, local: &LocalTicketBackend) -> Result<()> {
        let conn = self.open_connection()?;
        conn.execute_batch("BEGIN IMMEDIATE").map_err(sqlite_err)?;
        let result = (|| {
            for summary in local.list(TicketListQuery::all())? {
                let ticket = local.show(TicketIdOrSlug::Id(summary.id.clone()))?;
                self.store_ticket(&conn, &ticket)?;
                self.import_artifact_content(&conn, local.root(), &ticket)?;
            }
            Ok(())
        })();
        finish_sqlite_transaction(&conn, result)
    }

    fn connect(&self) -> Result<Connection> {
        if let Some(parent) = self.db_path.parent() {
            fs::create_dir_all(parent).map_err(|error| io_err(parent, error))?;
        }
        let conn = Connection::open(&self.db_path).map_err(sqlite_err)?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(sqlite_err)?;
        Ok(conn)
    }

    fn open_connection(&self) -> Result<Connection> {
        let connection = self.connect()?;
        verify_sqlite_ticket_schema(&connection)?;
        Ok(connection)
    }

    fn with_write<R>(&self, op: impl FnOnce(&Connection) -> Result<R>) -> Result<R> {
        let conn = self.open_connection()?;
        conn.execute_batch("BEGIN IMMEDIATE").map_err(sqlite_err)?;
        finish_sqlite_transaction(&conn, op(&conn))
    }

    fn with_read<R>(&self, op: impl FnOnce(&Connection) -> Result<R>) -> Result<R> {
        let conn = self.open_connection()?;
        op(&conn)
    }

    fn created_event_body(&self) -> &'static str {
        if is_japanese_record_language(self.record_language()) {
            "Ticket が作成されました。"
        } else {
            "Ticket created."
        }
    }

    fn default_item_body(&self) -> &'static str {
        if is_japanese_record_language(self.record_language()) {
            "## 背景\n\n## 要件\n\n## 受け入れ条件\n"
        } else {
            DEFAULT_TICKET_BODY
        }
    }

    fn resolve_ticket_id(&self, conn: &Connection, id: TicketIdOrSlug) -> Result<String> {
        let query = id.as_query().to_string();
        let mut stmt = conn
            .prepare(
                "SELECT ticket_id FROM typed_tickets
             WHERE workspace_id = ?1 AND (ticket_id = ?2 OR slug = ?2)
             UNION
             SELECT resource_id FROM workspace_resource_keys
             WHERE workspace_id = ?1 AND resource_kind = 'ticket' AND resource_key = ?2
             ORDER BY 1",
            )
            .map_err(sqlite_err)?;
        let rows = stmt
            .query_map(params![self.workspace_id, query], |row| {
                row.get::<_, String>(0)
            })
            .map_err(sqlite_err)?;
        let matches = rows
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(sqlite_err)?;
        match matches.as_slice() {
            [one] => Ok(one.clone()),
            [] => Err(TicketError::NotFound(id.as_query().to_string())),
            _ => Err(TicketError::Ambiguous {
                query: id.as_query().to_string(),
                matches: matches.into_iter().map(PathBuf::from).collect(),
            }),
        }
    }

    fn resource_key_for(
        conn: &Connection,
        workspace_id: &str,
        ticket_id: &str,
    ) -> Result<Option<String>> {
        conn.query_row(
            "SELECT resource_key FROM workspace_resource_keys
             WHERE workspace_id = ?1 AND resource_kind = 'ticket' AND resource_id = ?2",
            params![workspace_id, ticket_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(sqlite_err)
    }

    fn allocate_resource_key(
        conn: &Connection,
        workspace_id: &str,
        ticket_id: &str,
        allocated_at: &str,
    ) -> Result<String> {
        if let Some(existing) = Self::resource_key_for(conn, workspace_id, ticket_id)? {
            return Ok(existing);
        }
        conn.execute(
            "INSERT OR IGNORE INTO workspace_resource_key_counters
             (workspace_id, resource_kind, next_sequence) VALUES (?1, 'ticket', 1)",
            params![workspace_id],
        )
        .map_err(sqlite_err)?;
        let sequence: i64 = conn
            .query_row(
                "SELECT next_sequence FROM workspace_resource_key_counters
                 WHERE workspace_id = ?1 AND resource_kind = 'ticket'",
                params![workspace_id],
                |row| row.get(0),
            )
            .map_err(sqlite_err)?;
        conn.execute(
            "UPDATE workspace_resource_key_counters SET next_sequence = ?3
             WHERE workspace_id = ?1 AND resource_kind = 'ticket' AND next_sequence = ?2",
            params![workspace_id, sequence, sequence + 1],
        )
        .map_err(sqlite_err)?;
        let resource_key = format!("T-{sequence}");
        conn.execute(
            "INSERT INTO workspace_resource_keys
             (workspace_id, resource_kind, resource_id, sequence, resource_key, allocated_at)
             VALUES (?1, 'ticket', ?2, ?3, ?4, ?5)",
            params![
                workspace_id,
                ticket_id,
                sequence,
                resource_key,
                allocated_at
            ],
        )
        .map_err(sqlite_err)?;
        Ok(resource_key)
    }

    fn ticket_exists(&self, conn: &Connection, id: &str) -> Result<bool> {
        Ok(conn
            .query_row(
                "SELECT 1 FROM typed_tickets WHERE workspace_id = ?1 AND ticket_id = ?2",
                params![self.workspace_id, id],
                |_| Ok(()),
            )
            .optional()
            .map_err(sqlite_err)?
            .is_some())
    }

    fn insert_event(&self, conn: &Connection, ticket_id: &str, event: &TicketEvent) -> Result<()> {
        let next_index: i64 = conn.query_row(
            "SELECT COALESCE(MAX(event_index), -1) + 1 FROM typed_ticket_events WHERE workspace_id = ?1 AND ticket_id = ?2",
            params![self.workspace_id, ticket_id],
            |row| row.get(0),
        ).map_err(sqlite_err)?;
        conn.execute(r#"INSERT INTO typed_ticket_events
            (workspace_id, ticket_id, event_index, kind, author, at, status, from_state, to_state, reason, state_field, heading, body)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)"#,
            params![self.workspace_id, ticket_id, next_index, event.kind.as_str(), event.author, event.at, event.status, event.from, event.to, event.reason, event.state_field, event.heading, event.body.as_str()]
        ).map_err(sqlite_err)?;
        for (ordinal, reference) in event.references.iter().enumerate() {
            conn.execute("INSERT INTO typed_ticket_event_references (workspace_id, ticket_id, event_index, ordinal, kind, target) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![self.workspace_id, ticket_id, next_index, ordinal as i64, reference.kind, reference.target]).map_err(sqlite_err)?;
        }
        let mut attributes = event.attributes.clone();
        for (key, value) in &self.event_attributes {
            attributes.insert(key.clone(), value.clone());
        }
        for (key, value) in &attributes {
            conn.execute("INSERT INTO typed_ticket_event_attributes (workspace_id, ticket_id, event_index, key, value) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![self.workspace_id, ticket_id, next_index, key, value]).map_err(sqlite_err)?;
        }
        if let Some(hook) = &self.mutation_hook {
            hook(
                conn,
                &SqliteTicketMutationEvent {
                    workspace_id: self.workspace_id.clone(),
                    ticket_id: ticket_id.to_string(),
                    event_index: next_index,
                    event_kind: event.kind.clone(),
                },
            )?;
        }
        Ok(())
    }

    fn touch_ticket(&self, conn: &Connection, ticket_id: &str, updated_at: &str) -> Result<()> {
        conn.execute(
            "UPDATE typed_tickets SET updated_at = ?3 WHERE workspace_id = ?1 AND ticket_id = ?2",
            params![self.workspace_id, ticket_id, updated_at],
        )
        .map_err(sqlite_err)?;
        Ok(())
    }

    fn store_ticket(&self, conn: &Connection, ticket: &Ticket) -> Result<()> {
        conn.execute(
            "DELETE FROM typed_tickets WHERE workspace_id = ?1 AND ticket_id = ?2",
            params![self.workspace_id, ticket.meta.id],
        )
        .map_err(sqlite_err)?;
        self.insert_ticket(conn, ticket)
    }

    fn insert_ticket(&self, conn: &Connection, ticket: &Ticket) -> Result<()> {
        conn.execute(r#"INSERT INTO typed_tickets
            (workspace_id, ticket_id, slug, title, status, kind, priority, body, created_at, updated_at, assignee, readiness, workflow_state, workflow_state_explicit, queued_by, queued_at, resolution, repository_id, ref_selector)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)"#,
            params![self.workspace_id, ticket.meta.id, ticket.meta.slug, ticket.meta.title, ticket.meta.status.as_str(), ticket.meta.kind, ticket.meta.priority, ticket.document.body.as_str(), ticket.meta.created_at, ticket.meta.updated_at, ticket.meta.assignee, ticket.meta.readiness, ticket.meta.workflow_state.as_str(), if ticket.meta.workflow_state_explicit { 1 } else { 0 }, ticket.meta.queued_by, ticket.meta.queued_at, ticket.resolution.as_ref().map(|body| body.as_str()), ticket.meta.repository_id, ticket.meta.ref_selector]
        ).map_err(sqlite_err)?;
        self.insert_ordered_values(
            conn,
            "typed_ticket_labels",
            "label",
            &ticket.meta.id,
            &ticket.meta.labels,
        )?;
        self.insert_ordered_values(
            conn,
            "typed_ticket_risk_flags",
            "risk_flag",
            &ticket.meta.id,
            &ticket.meta.risk_flags,
        )?;
        for (key, value) in &ticket.meta.raw {
            conn.execute("INSERT INTO typed_ticket_raw_frontmatter (workspace_id, ticket_id, key, value) VALUES (?1, ?2, ?3, ?4)", params![self.workspace_id, ticket.meta.id, key, value]).map_err(sqlite_err)?;
        }
        for event in &ticket.events {
            self.insert_event(conn, &ticket.meta.id, event)?;
        }
        for relation in &ticket.relations.outgoing {
            self.insert_relation(conn, relation)?;
        }
        Ok(())
    }

    fn insert_ordered_values(
        &self,
        conn: &Connection,
        table: &str,
        column: &str,
        ticket_id: &str,
        values: &[String],
    ) -> Result<()> {
        for (ordinal, value) in values.iter().enumerate() {
            let sql = format!(
                "INSERT INTO {table} (workspace_id, ticket_id, ordinal, {column}) VALUES (?1, ?2, ?3, ?4)"
            );
            conn.execute(
                &sql,
                params![self.workspace_id, ticket_id, ordinal as i64, value],
            )
            .map_err(sqlite_err)?;
        }
        Ok(())
    }

    fn import_artifact_content(
        &self,
        conn: &Connection,
        local_root: &Path,
        ticket: &Ticket,
    ) -> Result<()> {
        for artifact in &ticket.artifacts {
            validate_artifact_relative_path(&artifact.relative_path)?;
            let path = local_root
                .join(&ticket.meta.id)
                .join("artifacts")
                .join(&artifact.relative_path);
            let content = fs::read(&path).map_err(|error| io_err(&path, error))?;
            conn.execute("INSERT OR REPLACE INTO typed_ticket_artifacts (workspace_id, ticket_id, relative_path, content) VALUES (?1, ?2, ?3, ?4)",
                params![self.workspace_id, ticket.meta.id, artifact.relative_path.to_string_lossy(), content]).map_err(sqlite_err)?;
        }
        Ok(())
    }

    fn ticket_meta_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TicketMeta> {
        let state_raw: String = row.get(12)?;
        Ok(TicketMeta {
            id: row.get(0)?,
            resource_key: None,
            slug: row.get(1)?,
            title: row.get(2)?,
            status: ExtensibleTicketStatus::from(row.get::<_, String>(3)?.as_str()),
            kind: row.get(4)?,
            priority: row.get(5)?,
            labels: Vec::new(),
            created_at: row.get(6)?,
            updated_at: row.get(7)?,
            assignee: row.get(8)?,
            readiness: row.get(9)?,
            risk_flags: Vec::new(),
            workflow_state: TicketWorkflowState::parse(&state_raw)
                .unwrap_or(TicketWorkflowState::Planning),
            workflow_state_explicit: row.get::<_, i64>(13)? != 0,
            queued_by: row.get(14)?,
            queued_at: row.get(15)?,
            repository_id: row.get(16)?,
            ref_selector: row.get(17)?,
            raw: BTreeMap::new(),
        })
    }

    fn load_ticket(&self, conn: &Connection, ticket_id: &str) -> Result<Ticket> {
        #[cfg(test)]
        self.full_ticket_load_count.fetch_add(1, Ordering::SeqCst);
        let (mut meta, body, resolution): (TicketMeta, String, Option<String>) = conn.query_row(r#"SELECT ticket_id, slug, title, status, kind, priority, created_at, updated_at, assignee, readiness, body, resolution, workflow_state, workflow_state_explicit, queued_by, queued_at, repository_id, ref_selector FROM typed_tickets WHERE workspace_id = ?1 AND ticket_id = ?2"#,
            params![self.workspace_id, ticket_id], |row| Ok((Self::ticket_meta_from_row(row)?, row.get(10)?, row.get(11)?))).optional().map_err(sqlite_err)?.ok_or_else(|| TicketError::NotFound(ticket_id.to_string()))?;
        meta.resource_key = Self::resource_key_for(conn, &self.workspace_id, ticket_id)?;
        meta.labels = self.load_ordered_values(conn, "typed_ticket_labels", "label", ticket_id)?;
        meta.risk_flags =
            self.load_ordered_values(conn, "typed_ticket_risk_flags", "risk_flag", ticket_id)?;
        meta.raw = self.load_key_values(conn, "typed_ticket_raw_frontmatter", ticket_id)?;
        let events = self.load_events(conn, ticket_id)?;
        let artifacts = self.load_artifacts(conn, ticket_id)?;
        let relations = self.relation_view_for_meta(conn, &meta)?;
        Ok(Ticket {
            meta,
            document: TicketDocument {
                body: MarkdownText::new(body),
                raw_frontmatter: BTreeMap::new(),
            },
            events,
            artifacts,
            relations,
            resolution: resolution.map(MarkdownText::new),
        })
    }

    fn load_ordered_values(
        &self,
        conn: &Connection,
        table: &str,
        column: &str,
        ticket_id: &str,
    ) -> Result<Vec<String>> {
        let sql = format!(
            "SELECT {column} FROM {table} WHERE workspace_id = ?1 AND ticket_id = ?2 ORDER BY ordinal ASC"
        );
        let mut stmt = conn.prepare(&sql).map_err(sqlite_err)?;
        let rows = stmt
            .query_map(params![self.workspace_id, ticket_id], |row| {
                row.get::<_, String>(0)
            })
            .map_err(sqlite_err)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(sqlite_err)
    }

    fn load_key_values(
        &self,
        conn: &Connection,
        table: &str,
        ticket_id: &str,
    ) -> Result<BTreeMap<String, String>> {
        let sql = format!(
            "SELECT key, value FROM {table} WHERE workspace_id = ?1 AND ticket_id = ?2 ORDER BY key ASC"
        );
        let mut stmt = conn.prepare(&sql).map_err(sqlite_err)?;
        let rows = stmt
            .query_map(params![self.workspace_id, ticket_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(sqlite_err)?;
        let mut map = BTreeMap::new();
        for row in rows {
            let (key, value) = row.map_err(sqlite_err)?;
            map.insert(key, value);
        }
        Ok(map)
    }

    fn load_events(&self, conn: &Connection, ticket_id: &str) -> Result<Vec<TicketEvent>> {
        let mut stmt = conn.prepare(r#"SELECT event_index, kind, author, at, status, from_state, to_state, reason, state_field, heading, body FROM typed_ticket_events WHERE workspace_id = ?1 AND ticket_id = ?2 ORDER BY event_index ASC"#).map_err(sqlite_err)?;
        let rows = stmt
            .query_map(params![self.workspace_id, ticket_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, String>(10)?,
                ))
            })
            .map_err(sqlite_err)?;
        let mut events = Vec::new();
        for row in rows {
            let (index, kind, author, at, status, from, to, reason, state_field, heading, body) =
                row.map_err(sqlite_err)?;
            let mut attributes = self.load_event_attributes(conn, ticket_id, index)?;
            attributes.insert("event_id".to_string(), format!("{ticket_id}:{index}"));
            attributes.insert("event_sequence".to_string(), index.to_string());
            events.push(TicketEvent {
                kind: TicketEventKind::from(kind.as_str()),
                author,
                at,
                status,
                from,
                to,
                reason,
                state_field,
                heading,
                body: MarkdownText::new(body),
                references: self.load_event_references(conn, ticket_id, index)?,
                attributes,
            });
        }
        Ok(events)
    }

    fn load_event_references(
        &self,
        conn: &Connection,
        ticket_id: &str,
        event_index: i64,
    ) -> Result<Vec<TicketReference>> {
        let mut stmt = conn.prepare("SELECT kind, target FROM typed_ticket_event_references WHERE workspace_id = ?1 AND ticket_id = ?2 AND event_index = ?3 ORDER BY ordinal ASC").map_err(sqlite_err)?;
        let rows = stmt
            .query_map(params![self.workspace_id, ticket_id, event_index], |row| {
                Ok(TicketReference {
                    kind: row.get(0)?,
                    target: row.get(1)?,
                })
            })
            .map_err(sqlite_err)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(sqlite_err)
    }

    fn load_event_attributes(
        &self,
        conn: &Connection,
        ticket_id: &str,
        event_index: i64,
    ) -> Result<BTreeMap<String, String>> {
        let mut stmt = conn.prepare("SELECT key, value FROM typed_ticket_event_attributes WHERE workspace_id = ?1 AND ticket_id = ?2 AND event_index = ?3 ORDER BY key ASC").map_err(sqlite_err)?;
        let rows = stmt
            .query_map(params![self.workspace_id, ticket_id, event_index], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(sqlite_err)?;
        let mut map = BTreeMap::new();
        for row in rows {
            let (key, value) = row.map_err(sqlite_err)?;
            map.insert(key, value);
        }
        Ok(map)
    }

    fn load_artifacts(&self, conn: &Connection, ticket_id: &str) -> Result<Vec<TicketArtifactRef>> {
        let mut stmt = conn.prepare("SELECT relative_path FROM typed_ticket_artifacts WHERE workspace_id = ?1 AND ticket_id = ?2 ORDER BY relative_path ASC").map_err(sqlite_err)?;
        let rows = stmt
            .query_map(params![self.workspace_id, ticket_id], |row| {
                Ok(TicketArtifactRef {
                    relative_path: PathBuf::from(row.get::<_, String>(0)?),
                })
            })
            .map_err(sqlite_err)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(sqlite_err)
    }

    fn list_summaries(
        &self,
        conn: &Connection,
        filter: TicketListQuery,
    ) -> Result<Vec<TicketSummary>> {
        let mut stmt = conn.prepare(r#"SELECT ticket_id, slug, title, status, kind, priority, created_at, updated_at, assignee, readiness, body, resolution, workflow_state, workflow_state_explicit, queued_by, queued_at, repository_id, ref_selector FROM typed_tickets WHERE workspace_id = ?1 ORDER BY ticket_id ASC"#).map_err(sqlite_err)?;
        let rows = stmt
            .query_map(params![self.workspace_id], Self::ticket_meta_from_row)
            .map_err(sqlite_err)?;
        let mut summaries = Vec::new();
        for row in rows {
            let mut meta = row.map_err(sqlite_err)?;
            meta.resource_key = Self::resource_key_for(conn, &self.workspace_id, &meta.id)?;
            if !filter.matches_state(meta.workflow_state) {
                continue;
            }
            meta.labels =
                self.load_ordered_values(conn, "typed_ticket_labels", "label", &meta.id)?;
            summaries.push(ticket_summary_from_meta(meta));
        }
        Ok(summaries)
    }

    fn state_index(&self, conn: &Connection) -> Result<HashMap<String, TicketWorkflowState>> {
        let mut stmt = conn
            .prepare("SELECT ticket_id, workflow_state FROM typed_tickets WHERE workspace_id = ?1")
            .map_err(sqlite_err)?;
        let rows = stmt
            .query_map(params![self.workspace_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(sqlite_err)?;
        let mut states = HashMap::new();
        for row in rows {
            let (id, state) = row.map_err(sqlite_err)?;
            states.insert(
                id,
                TicketWorkflowState::parse(&state).unwrap_or(TicketWorkflowState::Planning),
            );
        }
        Ok(states)
    }

    fn all_relations(&self, conn: &Connection) -> Result<Vec<TicketRelation>> {
        let mut stmt = conn.prepare(r#"SELECT ticket_id, kind, target, note, author, at FROM typed_ticket_relations WHERE workspace_id = ?1 ORDER BY ticket_id ASC, kind ASC, target ASC"#).map_err(sqlite_err)?;
        let rows = stmt
            .query_map(params![self.workspace_id], |row| {
                Ok(TicketRelation {
                    ticket_id: row.get(0)?,
                    kind: TicketRelationKind::parse(&row.get::<_, String>(1)?)
                        .unwrap_or(TicketRelationKind::Related),
                    target: row.get(2)?,
                    note: row.get(3)?,
                    author: row.get(4)?,
                    at: row.get(5)?,
                })
            })
            .map_err(sqlite_err)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(sqlite_err)
    }

    fn insert_relation(&self, conn: &Connection, relation: &TicketRelation) -> Result<()> {
        conn.execute(r#"INSERT OR REPLACE INTO typed_ticket_relations (workspace_id, ticket_id, kind, target, note, author, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"#,
            params![self.workspace_id, relation.ticket_id, relation.kind.as_str(), relation.target, relation.note, relation.author, relation.at]).map_err(sqlite_err)?;
        Ok(())
    }

    fn relation_view_for_meta(
        &self,
        conn: &Connection,
        meta: &TicketMeta,
    ) -> Result<TicketRelationView> {
        let relations = self.all_relations(conn)?;
        let states = self.state_index(conn)?;
        Ok(relation_view_from_records(meta, &relations, &states))
    }
}

fn finish_sqlite_transaction<R>(conn: &Connection, result: Result<R>) -> Result<R> {
    match result {
        Ok(output) => {
            conn.execute_batch("COMMIT").map_err(sqlite_err)?;
            Ok(output)
        }
        Err(error) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

fn validate_artifact_relative_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(TicketError::PathEscapesRoot {
            path: path.to_path_buf(),
        });
    }
    for component in path.components() {
        match component {
            Component::Normal(_) => {}
            _ => {
                return Err(TicketError::PathEscapesRoot {
                    path: path.to_path_buf(),
                });
            }
        }
    }
    Ok(())
}

impl TicketBackend for SqliteTicketBackend {
    fn default_intake_ready_state_change_body(&self, from: &str) -> String {
        if is_japanese_record_language(self.record_language()) {
            format!("Ticket planning が完了しました。state {from} -> ready。\n")
        } else {
            format!("Ticket planning complete; state {from} -> ready.\n")
        }
    }

    fn list(&self, filter: TicketListQuery) -> Result<Vec<TicketSummary>> {
        self.with_read(|conn| self.list_summaries(conn, filter))
    }

    fn show(&self, id: TicketIdOrSlug) -> Result<Ticket> {
        self.with_read(|conn| {
            let ticket_id = self.resolve_ticket_id(conn, id)?;
            self.load_ticket(conn, &ticket_id)
        })
    }

    fn create(&self, input: NewTicket) -> Result<TicketRef> {
        self.with_write(|conn| {
            if input.title.trim().is_empty() {
                return Err(TicketError::Conflict(
                    "ticket title must not be empty".to_string(),
                ));
            }
            validate_ticket_target(
                input.repository_id.as_deref(),
                input.ref_selector.as_deref(),
            )?;
            let base_millis = unix_epoch_millis_now().map_err(|err| {
                TicketError::Conflict(format!("failed to read ticket id timestamp: {err}"))
            })?;
            let id = allocate_record_id(base_millis, |candidate| {
                self.ticket_exists(conn, candidate).unwrap_or(true)
            })
            .map_err(|err| {
                TicketError::Conflict(format!("failed to allocate unique ticket id: {err}"))
            })?;
            let now = now_utc();
            let state = input
                .workflow_state
                .unwrap_or(TicketWorkflowState::Planning);
            let status = if state == TicketWorkflowState::Closed {
                ExtensibleTicketStatus::Closed
            } else {
                ExtensibleTicketStatus::Open
            };
            let meta = TicketMeta {
                id: id.clone(),
                resource_key: None,
                slug: input.slug.clone().unwrap_or_else(|| id.clone()),
                title: input.title,
                status,
                kind: input.kind,
                priority: input.priority,
                labels: input.labels,
                created_at: Some(now.clone()),
                updated_at: Some(now.clone()),
                assignee: input.assignee,
                readiness: input.readiness,
                risk_flags: input.risk_flags,
                workflow_state: state,
                workflow_state_explicit: true,
                queued_by: input.queued_by,
                queued_at: input.queued_at,
                repository_id: input.repository_id,
                ref_selector: input.ref_selector,
                raw: BTreeMap::new(),
            };
            let body = if input.body.as_str() == DEFAULT_TICKET_BODY {
                MarkdownText::new(self.default_item_body())
            } else {
                input.body
            };
            let author = input
                .author
                .unwrap_or_else(|| "SqliteTicketBackend".to_string());
            let ticket = Ticket {
                meta,
                document: TicketDocument {
                    body,
                    raw_frontmatter: BTreeMap::new(),
                },
                events: vec![TicketEvent {
                    kind: TicketEventKind::Create,
                    author: Some(author),
                    at: Some(now.clone()),
                    status: None,
                    from: None,
                    to: None,
                    reason: None,
                    state_field: None,
                    heading: Some(TicketEventKind::Create.heading()),
                    body: MarkdownText::new(self.created_event_body()),
                    references: Vec::new(),
                    attributes: BTreeMap::new(),
                }],
                artifacts: Vec::new(),
                relations: TicketRelationView::default(),
                resolution: None,
            };
            let resource_key = Self::allocate_resource_key(conn, &self.workspace_id, &id, &now)?;
            self.insert_ticket(conn, &ticket)?;
            Ok(TicketRef {
                id: id.clone(),
                resource_key: Some(resource_key),
                slug: id,
                status: TicketStatus::Open,
            })
        })
    }

    fn edit_item(&self, id: TicketIdOrSlug, edit: TicketItemEdit) -> Result<Ticket> {
        self.with_write(|conn| {
            edit.validate_body_edit_request()?;
            if !edit.has_changes() {
                return Err(TicketError::Conflict(
                    "TicketEditItem requires at least one of title, body, body_replacement, or target"
                        .to_string(),
                ));
            }
            if let Some(title) = edit.title.as_ref() {
                validate_required_event_value("title", title)?;
            }
            if let Some(author) = edit.author.as_deref() {
                validate_required_event_value("author", author)?;
            }
            let ticket_id = self.resolve_ticket_id(conn, id)?;
            if edit.target.is_some() {
                let current = self.load_ticket(conn, &ticket_id)?.meta.workflow_state;
                if current != TicketWorkflowState::Planning {
                    return Err(TicketError::Conflict(format!(
                        "ticket implementation target is locked after planning (current state: {})",
                        current.as_str()
                    )));
                }
            }
            let now = now_utc();
            let mut body_edit_audit = TicketBodyEditAudit::None;
            if let Some(title) = edit.title.as_ref() {
                conn.execute("UPDATE typed_tickets SET title = ?3, updated_at = ?4 WHERE workspace_id = ?1 AND ticket_id = ?2", params![self.workspace_id, ticket_id, title, now]).map_err(sqlite_err)?;
            }
            let mut updated_body = None;
            if let Some(body) = edit.body.as_ref() {
                updated_body = Some(body.clone());
                body_edit_audit = TicketBodyEditAudit::WholeBody;
            }
            if let Some(replacement) = edit.body_replacement.as_ref() {
                let current_body: String = conn
                    .query_row(
                        "SELECT body FROM typed_tickets WHERE workspace_id = ?1 AND ticket_id = ?2",
                        params![self.workspace_id, ticket_id],
                        |row| row.get(0),
                    )
                    .map_err(sqlite_err)?;
                let outcome = replacement.apply(current_body.as_str())?;
                updated_body = Some(outcome.body);
                body_edit_audit = TicketBodyEditAudit::Partial {
                    replacement_count: outcome.replacement_count,
                };
            }
            if let Some(body) = updated_body.as_ref() {
                conn.execute("UPDATE typed_tickets SET body = ?3, updated_at = ?4 WHERE workspace_id = ?1 AND ticket_id = ?2", params![self.workspace_id, ticket_id, body.as_str(), now]).map_err(sqlite_err)?;
            }
            if let Some(target) = edit.target.as_ref() {
                let (repository_id, ref_selector) = match target {
                    TicketTargetEdit::Set {
                        repository_id,
                        ref_selector,
                    } => (Some(repository_id.as_str()), ref_selector.as_deref()),
                    TicketTargetEdit::Clear => (None, None),
                };
                conn.execute(
                    "UPDATE typed_tickets SET repository_id = ?3, ref_selector = ?4, updated_at = ?5 WHERE workspace_id = ?1 AND ticket_id = ?2",
                    params![self.workspace_id, ticket_id, repository_id, ref_selector, now],
                )
                .map_err(sqlite_err)?;
            }

            let mut changes = Vec::new();
            if edit.title.is_some() {
                changes.push("title");
            }
            if !matches!(body_edit_audit, TicketBodyEditAudit::None) {
                changes.push("body");
            }
            if edit.target.is_some() {
                changes.push("target");
            }
            let mut attributes = BTreeMap::new();
            attributes.insert("changes".to_string(), changes.join(","));
            let body = match body_edit_audit {
                TicketBodyEditAudit::None => {
                    MarkdownText::new(format!("Ticket item updated: {}.", changes.join(", ")))
                }
                TicketBodyEditAudit::WholeBody => {
                    attributes.insert("body_edit".to_string(), "whole".to_string());
                    MarkdownText::new(format!(
                        "Ticket item updated: {}. Body was replaced as a whole.",
                        changes.join(", ")
                    ))
                }
                TicketBodyEditAudit::Partial { replacement_count } => {
                    attributes.insert("body_edit".to_string(), "partial".to_string());
                    attributes.insert(
                        "replacement_count".to_string(),
                        replacement_count.to_string(),
                    );
                    MarkdownText::new(format!(
                        "Ticket item updated: {}. Body replacement applied to {replacement_count} occurrence(s).",
                        changes.join(", ")
                    ))
                }
            };
            let event = TicketEvent { kind: TicketEventKind::Other("item_edit".to_string()), author: Some(edit.author.unwrap_or_else(default_author)), at: Some(now), status: None, from: None, to: None, reason: None, state_field: None, heading: Some("Item edit".to_string()), body, references: Vec::new(), attributes };
            self.insert_event(conn, &ticket_id, &event)?;
            self.load_ticket(conn, &ticket_id)
        })
    }

    fn dependency_check(&self, id: TicketIdOrSlug) -> Result<TicketDependencyCheck> {
        let ticket = self.show(id)?;
        let blockers = ticket.relations.blockers.clone();
        let summary = ticket_summary_from_meta(ticket.meta.clone());
        let projection = project_ticket_workspace_item(&summary, &blockers, None);
        Ok(TicketDependencyCheck {
            ticket: summary,
            blockers,
            queue_guard: projection.queue_guard,
            recommended_action: projection
                .next_action
                .unwrap_or(TicketWorkspaceNextAction::WaitForOrchestrator),
        })
    }

    fn add_event(&self, id: TicketIdOrSlug, event: NewTicketEvent) -> Result<()> {
        self.with_write(|conn| {
            let ticket_id = self.resolve_ticket_id(conn, id)?;
            let at = now_utc();
            self.insert_event(
                conn,
                &ticket_id,
                &TicketEvent {
                    kind: event.kind.clone(),
                    author: event.author.or_else(|| Some(default_author())),
                    at: Some(at.clone()),
                    status: None,
                    from: None,
                    to: None,
                    reason: None,
                    state_field: None,
                    heading: Some(event.kind.heading()),
                    body: event.body,
                    references: event.references,
                    attributes: BTreeMap::new(),
                },
            )?;
            self.touch_ticket(conn, &ticket_id, &at)
        })
    }

    fn add_state_changed(&self, id: TicketIdOrSlug, change: TicketStateChange) -> Result<()> {
        self.set_workflow_state(id, change)
    }

    fn add_intake_summary(&self, id: TicketIdOrSlug, summary: TicketIntakeSummary) -> Result<()> {
        self.with_write(|conn| {
            validate_intake_summary(&summary)?;
            let ticket_id = self.resolve_ticket_id(conn, id)?;
            let at = now_utc();
            self.insert_event(
                conn,
                &ticket_id,
                &TicketEvent {
                    kind: TicketEventKind::IntakeSummary,
                    author: Some(summary.author.unwrap_or_else(default_author)),
                    at: Some(at.clone()),
                    status: None,
                    from: None,
                    to: None,
                    reason: None,
                    state_field: None,
                    heading: Some(TicketEventKind::IntakeSummary.heading()),
                    body: summary.body,
                    references: summary.references,
                    attributes: BTreeMap::new(),
                },
            )?;
            self.touch_ticket(conn, &ticket_id, &at)
        })
    }

    fn set_state_field(
        &self,
        id: TicketIdOrSlug,
        _field: &str,
        change: TicketStateChange,
    ) -> Result<()> {
        self.set_workflow_state(id, change)
    }

    fn set_workflow_state(&self, id: TicketIdOrSlug, change: TicketStateChange) -> Result<()> {
        validate_state_change(&change)?;
        let from = TicketWorkflowState::parse(&change.from).ok_or_else(|| {
            TicketError::InvalidWorkflowTransition {
                from: change.from.clone(),
                to: change.to.clone(),
            }
        })?;
        let to = TicketWorkflowState::parse(&change.to).ok_or_else(|| {
            TicketError::InvalidWorkflowTransition {
                from: change.from.clone(),
                to: change.to.clone(),
            }
        })?;
        validate_generic_state_change(from, to)?;
        self.with_write(|conn| {
            let ticket_id = self.resolve_ticket_id(conn, id)?;
            let current = self.load_ticket(conn, &ticket_id)?.meta.workflow_state;
            if current != from {
                return Err(TicketError::StaleWorkflowState {
                    expected: from.as_str().to_owned(),
                    actual: current.as_str().to_owned(),
                });
            }
            if from == TicketWorkflowState::Queued && to == TicketWorkflowState::InProgress {
                let ticket = self.load_ticket(conn, &ticket_id)?;
                let blockers = ticket
                    .relations
                    .blockers
                    .into_iter()
                    .filter(|blocker| !relation_blocker_allows_queue(blocker))
                    .collect::<Vec<_>>();
                if !blockers.is_empty() {
                    return Err(TicketError::BlockingRelations(format_relation_blockers(&blockers)));
                }
            }
            let at = now_utc();
            self.insert_event(conn, &ticket_id, &TicketEvent { kind: TicketEventKind::StateChanged, author: Some(change.author.clone().unwrap_or_else(default_author)), at: Some(at.clone()), status: None, from: Some(change.from), to: Some(change.to), reason: Some(change.reason), state_field: Some("state".to_string()), heading: Some(TicketEventKind::StateChanged.heading()), body: change.body, references: change.references, attributes: BTreeMap::new() })?;
            conn.execute("UPDATE typed_tickets SET workflow_state = ?3, workflow_state_explicit = 1, updated_at = ?4, status = CASE WHEN ?3 = 'closed' THEN 'closed' ELSE status END WHERE workspace_id = ?1 AND ticket_id = ?2", params![self.workspace_id, ticket_id, to.as_str(), at]).map_err(sqlite_err)?;
            Ok(())
        })
    }

    fn mark_ready(&self, id: TicketIdOrSlug, request: TicketMarkReady) -> Result<Ticket> {
        validate_required_event_value("operation_key", &request.operation_key)?;
        self.with_write(|conn| {
            let ticket_id = self.resolve_ticket_id(conn, id)?;
            let ticket = self.load_ticket(conn, &ticket_id)?;
            if validate_mark_ready_replay(&ticket, &request)? {
                return Ok(ticket);
            }
            let target = resolve_ready_target(
                self.target_authority.as_ref(),
                &self.workspace_id,
                &ticket,
            )?;
            let fingerprint = mark_ready_fingerprint(&ticket, &request, &target);
            if ticket.meta.workflow_state != TicketWorkflowState::Planning {
                return Err(TicketError::StaleWorkflowState {
                    expected: TicketWorkflowState::Planning.as_str().to_owned(),
                    actual: ticket.meta.workflow_state.as_str().to_owned(),
                });
            }
            let reason = request
                .reason
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("implementation target validated")
                .to_owned();
            let at = now_utc();
            if let Some(mut summary) = request.intake_summary.clone() {
                validate_intake_summary(&summary)?;
                summary.author = request.author.clone().or(summary.author);
                self.insert_event(
                    conn,
                    &ticket_id,
                    &TicketEvent {
                        kind: TicketEventKind::IntakeSummary,
                        author: summary.author,
                        at: None,
                        status: None,
                        from: None,
                        to: None,
                        reason: None,
                        state_field: None,
                        heading: Some(TicketEventKind::IntakeSummary.heading()),
                        body: summary.body,
                        references: summary.references,
                        attributes: BTreeMap::new(),
                    },
                )?;
            }
            self.insert_event(
                conn,
                &ticket_id,
                &TicketEvent {
                    kind: TicketEventKind::StateChanged,
                    author: Some(request.author.unwrap_or_else(default_author)),
                    at: Some(at.clone()),
                    status: None,
                    from: Some(TicketWorkflowState::Planning.as_str().to_owned()),
                    to: Some(TicketWorkflowState::Ready.as_str().to_owned()),
                    reason: Some(reason),
                    state_field: Some("state".to_owned()),
                    heading: Some(TicketEventKind::StateChanged.heading()),
                    body: MarkdownText::new(format!(
                        "Implementation target `{}` at selector `{}` was validated and the Ticket was marked ready.",
                        target.repository_id, target.ref_selector
                    )),
                    references: Vec::new(),
                    attributes: BTreeMap::from([
                        ("operation_key".to_owned(), request.operation_key),
                        ("request_fingerprint".to_owned(), fingerprint),
                        ("repository_id".to_owned(), target.repository_id.clone()),
                        ("ref_selector".to_owned(), target.ref_selector.clone()),
                    ]),
                },
            )?;
            conn.execute(
                "UPDATE typed_tickets SET workflow_state = 'ready', workflow_state_explicit = 1, repository_id = ?3, ref_selector = ?4, updated_at = ?5 WHERE workspace_id = ?1 AND ticket_id = ?2 AND workflow_state = 'planning'",
                params![self.workspace_id, ticket_id, target.repository_id, target.ref_selector, at],
            )
            .map_err(sqlite_err)?;
            self.load_ticket(conn, &ticket_id)
        })
    }

    fn queue_ready(&self, id: TicketIdOrSlug, queued_by: &str) -> Result<()> {
        validate_required_event_value("queued_by", queued_by)?;
        self.with_write(|conn| {
            let ticket_id = self.resolve_ticket_id(conn, id)?;
            let ticket = self.load_ticket(conn, &ticket_id)?;
            if ticket.meta.workflow_state != TicketWorkflowState::Ready {
                return Err(TicketError::StaleWorkflowState {
                    expected: TicketWorkflowState::Ready.as_str().to_owned(),
                    actual: ticket.meta.workflow_state.as_str().to_owned(),
                });
            }
            let target = resolve_ready_target(
                self.target_authority.as_ref(),
                &self.workspace_id,
                &ticket,
            )?;
            let blockers = ticket
                .relations
                .blockers
                .iter()
                .filter(|blocker| !relation_blocker_allows_queue(blocker))
                .cloned()
                .collect::<Vec<_>>();
            if !blockers.is_empty() {
                return Err(TicketError::BlockingRelations(format_relation_blockers(&blockers)));
            }
            let at = now_utc();
            conn.execute("UPDATE typed_tickets SET workflow_state = 'queued', workflow_state_explicit = 1, queued_by = ?3, queued_at = ?4, repository_id = ?5, ref_selector = ?6, updated_at = ?4 WHERE workspace_id = ?1 AND ticket_id = ?2 AND workflow_state = 'ready'", params![self.workspace_id, ticket_id, queued_by, at, target.repository_id, target.ref_selector]).map_err(sqlite_err)?;
            self.insert_event(conn, &ticket_id, &TicketEvent { kind: TicketEventKind::StateChanged, author: Some(queued_by.to_string()), at: Some(at.clone()), status: None, from: Some("ready".to_string()), to: Some("queued".to_string()), reason: Some("queued".to_string()), state_field: Some("state".to_string()), heading: Some(TicketEventKind::StateChanged.heading()), body: MarkdownText::new(format!("Queued for Orchestrator by {queued_by}.")), references: Vec::new(), attributes: BTreeMap::from([("queued_by".to_owned(), queued_by.to_owned()), ("queued_at".to_owned(), at), ("repository_id".to_owned(), target.repository_id), ("ref_selector".to_owned(), target.ref_selector)]) })
        })
    }

    fn close(&self, id: TicketIdOrSlug, resolution: MarkdownText) -> Result<()> {
        self.with_write(|conn| {
            let ticket_id = self.resolve_ticket_id(conn, id)?;
            let at = now_utc();
            conn.execute("UPDATE typed_tickets SET status = 'closed', workflow_state = 'closed', workflow_state_explicit = 1, updated_at = ?3, resolution = ?4 WHERE workspace_id = ?1 AND ticket_id = ?2", params![self.workspace_id, ticket_id, at, resolution.as_str()]).map_err(sqlite_err)?;
            self.insert_event(conn, &ticket_id, &TicketEvent { kind: TicketEventKind::Close, author: Some(default_author()), at: Some(at), status: Some("closed".to_string()), from: None, to: Some("closed".to_string()), reason: None, state_field: Some("state".to_string()), heading: Some(TicketEventKind::Close.heading()), body: resolution, references: Vec::new(), attributes: BTreeMap::new() })
        })
    }

    fn add_ticket_relation(
        &self,
        id: TicketIdOrSlug,
        relation: NewTicketRelation,
    ) -> Result<TicketRelation> {
        self.with_write(|conn| {
            let ticket_id = self.resolve_ticket_id(conn, id)?;
            let target =
                self.resolve_ticket_id(conn, TicketIdOrSlug::Query(relation.target.clone()))?;
            let output = TicketRelation {
                ticket_id,
                kind: relation.kind,
                target,
                note: relation.note,
                author: relation.author.unwrap_or_else(default_author),
                at: now_utc(),
            };
            self.insert_relation(conn, &output)?;
            Ok(output)
        })
    }

    fn remove_ticket_relation(
        &self,
        id: TicketIdOrSlug,
        kind: TicketRelationKind,
        target: TicketIdOrSlug,
    ) -> Result<TicketRelation> {
        self.with_write(|conn| {
            let ticket_id = self.resolve_ticket_id(conn, id)?;
            let target = self.resolve_ticket_id(conn, target)?;
            let relation = conn
                .query_row(
                    "SELECT note, author, at FROM typed_ticket_relations WHERE workspace_id = ?1 AND ticket_id = ?2 AND kind = ?3 AND target = ?4",
                    params![self.workspace_id, ticket_id, kind.as_str(), target],
                    |row| {
                        Ok(TicketRelation {
                            ticket_id: ticket_id.clone(),
                            kind,
                            target: target.clone(),
                            note: row.get(0)?,
                            author: row.get(1)?,
                            at: row.get(2)?,
                        })
                    },
                )
                .optional()
                .map_err(sqlite_err)?
                .ok_or_else(|| {
                    TicketError::NotFound(format!(
                        "relation {} {} {}",
                        ticket_id, kind, target
                    ))
                })?;
            let deleted = conn
                .execute(
                    "DELETE FROM typed_ticket_relations WHERE workspace_id = ?1 AND ticket_id = ?2 AND kind = ?3 AND target = ?4",
                    params![self.workspace_id, ticket_id, kind.as_str(), target],
                )
                .map_err(sqlite_err)?;
            if deleted != 1 {
                return Err(TicketError::Conflict(format!(
                    "expected to remove one ticket relation, removed {deleted}"
                )));
            }
            conn.execute(
                "UPDATE typed_tickets SET updated_at = ?3 WHERE workspace_id = ?1 AND ticket_id = ?2",
                params![self.workspace_id, ticket_id, now_utc()],
            )
            .map_err(sqlite_err)?;
            Ok(relation)
        })
    }

    fn query_ticket_relations(
        &self,
        ticket: Option<TicketIdOrSlug>,
        kind: Option<TicketRelationKind>,
    ) -> Result<Vec<TicketRelation>> {
        self.with_read(|conn| {
            let ticket_id = ticket
                .map(|id| self.resolve_ticket_id(conn, id))
                .transpose()?;
            let mut relations = self.all_relations(conn)?;
            if let Some(ticket_id) = ticket_id {
                relations.retain(|relation| {
                    relation.ticket_id == ticket_id || relation.target == ticket_id
                });
            }
            if let Some(kind) = kind {
                relations.retain(|relation| relation.kind == kind);
            }
            sort_ticket_relations(&mut relations);
            Ok(relations)
        })
    }

    fn relation_view(&self, id: TicketIdOrSlug) -> Result<TicketRelationView> {
        self.with_read(|conn| {
            let ticket_id = self.resolve_ticket_id(conn, id)?;
            Ok(self.load_ticket(conn, &ticket_id)?.relations)
        })
    }

    fn add_orchestration_plan_record(
        &self,
        id: TicketIdOrSlug,
        record: NewOrchestrationPlanRecord,
    ) -> Result<OrchestrationPlanRecord> {
        self.with_write(|conn| {
            let ticket_id = self.resolve_ticket_id(conn, id)?;
            let meta = self.load_ticket(conn, &ticket_id)?.meta;
            let output = OrchestrationPlanRecord { id: allocate_record_id(unix_epoch_millis_now().unwrap_or(0), |_| false).map_err(|err| TicketError::Conflict(format!("failed to allocate orchestration plan id: {err}")))?, ticket_id, kind: record.kind, related_ticket: record.related_ticket, note: record.note, accepted_plan: record.accepted_plan, author: record.author.unwrap_or_else(default_author), at: now_utc() };
            validate_orchestration_plan_record(&output, Some(&meta))?;
            conn.execute(r#"INSERT INTO typed_ticket_orchestration_plans (workspace_id, ticket_id, record_id, kind, related_ticket, note, accepted_summary, accepted_branch, accepted_worktree, accepted_role_plan, author, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"#,
                params![self.workspace_id, output.ticket_id, output.id, output.kind.as_str(), output.related_ticket, output.note, output.accepted_plan.as_ref().map(|plan| plan.summary.as_str()), output.accepted_plan.as_ref().and_then(|plan| plan.branch.as_deref()), output.accepted_plan.as_ref().and_then(|plan| plan.worktree.as_deref()), output.accepted_plan.as_ref().and_then(|plan| plan.role_plan.as_deref()), output.author, output.at]).map_err(sqlite_err)?;
            Ok(output)
        })
    }

    fn query_orchestration_plan_records(
        &self,
        ticket: Option<TicketIdOrSlug>,
        kind: Option<OrchestrationPlanKind>,
    ) -> Result<Vec<OrchestrationPlanRecord>> {
        self.with_read(|conn| {
            let ticket_id = ticket.map(|id| self.resolve_ticket_id(conn, id)).transpose()?;
            let mut stmt = conn.prepare("SELECT ticket_id, record_id, kind, related_ticket, note, accepted_summary, accepted_branch, accepted_worktree, accepted_role_plan, author, at FROM typed_ticket_orchestration_plans WHERE workspace_id = ?1 ORDER BY at ASC, record_id ASC").map_err(sqlite_err)?;
            let rows = stmt.query_map(params![self.workspace_id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, Option<String>>(3)?, row.get::<_, Option<String>>(4)?, row.get::<_, Option<String>>(5)?, row.get::<_, Option<String>>(6)?, row.get::<_, Option<String>>(7)?, row.get::<_, Option<String>>(8)?, row.get::<_, String>(9)?, row.get::<_, String>(10)?))).map_err(sqlite_err)?;
            let mut records = Vec::new();
            for row in rows {
                let (record_ticket, id, kind_raw, related_ticket, note, summary, branch, worktree, role_plan, author, at) = row.map_err(sqlite_err)?;
                if ticket_id.as_ref().is_some_and(|ticket_id| ticket_id != &record_ticket) { continue; }
                let Some(record_kind) = OrchestrationPlanKind::parse(&kind_raw) else { continue; };
                if kind.is_some_and(|expected| expected != record_kind) { continue; }
                records.push(OrchestrationPlanRecord { ticket_id: record_ticket, id, kind: record_kind, related_ticket, note, accepted_plan: summary.map(|summary| AcceptedOrchestrationPlan { summary, branch, worktree, role_plan }), author, at });
            }
            Ok(records)
        })
    }

    fn doctor(&self) -> Result<TicketDoctorReport> {
        self.with_read(|conn| {
            let _ = self.list_summaries(conn, TicketListQuery::all())?;
            Ok(TicketDoctorReport::default())
        })
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
        validate_ticket_target(
            input.repository_id.as_deref(),
            input.ref_selector.as_deref(),
        )?;
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
        if let Some(repository_id) = input.repository_id {
            fields.push((
                "repository_id".to_string(),
                format_yaml_string_scalar(repository_id.as_str()),
            ));
        }
        if let Some(ref_selector) = input.ref_selector {
            fields.push((
                "ref_selector".to_string(),
                format_yaml_string_scalar(ref_selector.as_str()),
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
            resource_key: None,
            slug: id,
            status: TicketStatus::Open,
        })
    }

    fn edit_item(&self, id: TicketIdOrSlug, edit: TicketItemEdit) -> Result<Ticket> {
        edit.validate_body_edit_request()?;
        if !edit.has_changes() {
            return Err(TicketError::Conflict(
                "TicketEditItem requires at least one of title, body, or body_replacement"
                    .to_string(),
            ));
        }
        if let Some(title) = edit.title.as_deref() {
            validate_required_event_value("title", title)?;
        }
        if let Some(author) = edit.author.as_deref() {
            validate_required_event_value("author", author)?;
        }
        let _lock = self.acquire_lock()?;
        let dir = self.find_ticket_dir(&id)?;
        let item = dir.join("item.md");
        if edit.target.is_some() {
            let current = self.ticket_workflow_state_from_dir(&dir)?;
            if current != TicketWorkflowState::Planning {
                return Err(TicketError::Conflict(format!(
                    "ticket implementation target is locked after planning (current state: {})",
                    current.as_str()
                )));
            }
        }
        let mut content = fs::read_to_string(&item).map_err(|e| io_err(&item, e))?;
        let mut body_edit_audit = TicketBodyEditAudit::None;
        let mut updates = Vec::new();
        if let Some(title) = edit.title.as_deref() {
            updates.push(("title", title));
        }
        if !updates.is_empty() {
            content = replace_frontmatter_fields(&content, &updates).map_err(|message| {
                TicketError::Parse {
                    path: item.clone(),
                    message,
                }
            })?;
        }
        if let Some(target) = edit.target.as_ref() {
            match target {
                TicketTargetEdit::Set {
                    repository_id,
                    ref_selector,
                } => {
                    content = replace_frontmatter_fields(
                        &content,
                        &[("repository_id", repository_id.as_str())],
                    )
                    .map_err(|message| TicketError::Parse {
                        path: item.clone(),
                        message,
                    })?;
                    if let Some(ref_selector) = ref_selector {
                        content = replace_frontmatter_fields(
                            &content,
                            &[("ref_selector", ref_selector.as_str())],
                        )
                        .map_err(|message| TicketError::Parse {
                            path: item.clone(),
                            message,
                        })?;
                    } else {
                        content = remove_frontmatter_fields(&content, &["ref_selector"]).map_err(
                            |message| TicketError::Parse {
                                path: item.clone(),
                                message,
                            },
                        )?;
                    }
                }
                TicketTargetEdit::Clear => {
                    content =
                        remove_frontmatter_fields(&content, &["repository_id", "ref_selector"])
                            .map_err(|message| TicketError::Parse {
                                path: item.clone(),
                                message,
                            })?;
                }
            }
        }
        if let Some(body) = edit.body.as_ref() {
            content = replace_item_body(&content, body.as_str()).map_err(|message| {
                TicketError::Parse {
                    path: item.clone(),
                    message,
                }
            })?;
            body_edit_audit = TicketBodyEditAudit::WholeBody;
        }
        if let Some(replacement) = edit.body_replacement.as_ref() {
            let parsed = parse_item(&content).map_err(|message| TicketError::Parse {
                path: item.clone(),
                message,
            })?;
            let outcome = replacement.apply(parsed.body.as_str())?;
            content = replace_item_body(&content, outcome.body.as_str()).map_err(|message| {
                TicketError::Parse {
                    path: item.clone(),
                    message,
                }
            })?;
            body_edit_audit = TicketBodyEditAudit::Partial {
                replacement_count: outcome.replacement_count,
            };
        }
        atomic_write(&item, content.as_bytes())?;

        let author = edit.author.unwrap_or_else(default_author);
        let mut changes = Vec::new();
        if edit.title.is_some() {
            changes.push("title");
        }
        if !matches!(body_edit_audit, TicketBodyEditAudit::None) {
            changes.push("body");
        }
        if edit.target.is_some() {
            changes.push("target");
        }
        let mut attrs = Vec::new();
        attrs.push(("changes", changes.join(",")));
        let body = match body_edit_audit {
            TicketBodyEditAudit::None => {
                MarkdownText::new(format!("Ticket item updated: {}.", changes.join(", ")))
            }
            TicketBodyEditAudit::WholeBody => {
                attrs.push(("body_edit", "whole".to_string()));
                MarkdownText::new(format!(
                    "Ticket item updated: {}. Body was replaced as a whole.",
                    changes.join(", ")
                ))
            }
            TicketBodyEditAudit::Partial { replacement_count } => {
                attrs.push(("body_edit", "partial".to_string()));
                attrs.push(("replacement_count", replacement_count.to_string()));
                MarkdownText::new(format!(
                    "Ticket item updated: {}. Body replacement applied to {replacement_count} occurrence(s).",
                    changes.join(", ")
                ))
            }
        };
        self.append_thread_event(
            &dir,
            "item_edit",
            self.generated_heading("Item updated", "項目更新"),
            &author,
            None,
            &attrs
                .iter()
                .map(|(key, value)| (*key, value.as_str()))
                .collect::<Vec<_>>(),
            &body,
        )?;
        self.ticket_from_dir(&dir)
    }

    fn dependency_check(&self, id: TicketIdOrSlug) -> Result<TicketDependencyCheck> {
        let ticket = self.show(id)?;
        let summary = ticket_summary_from_meta(ticket.meta.clone());
        let projection = project_ticket_workspace_item(&summary, &ticket.relations.blockers, None);
        Ok(TicketDependencyCheck {
            ticket: summary,
            blockers: ticket.relations.blockers,
            queue_guard: projection.queue_guard,
            recommended_action: projection
                .next_action
                .unwrap_or(TicketWorkspaceNextAction::WaitForOrchestrator),
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
        validate_generic_state_change(from, to)?;
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

    fn mark_ready(&self, id: TicketIdOrSlug, request: TicketMarkReady) -> Result<Ticket> {
        validate_required_event_value("operation_key", &request.operation_key)?;
        let _lock = self.acquire_lock()?;
        let dir = self.find_ticket_dir(&id)?;
        let ticket = self.ticket_from_dir(&dir)?;
        if validate_mark_ready_replay(&ticket, &request)? {
            return Ok(ticket);
        }
        let target = resolve_ready_target(self.target_authority.as_ref(), "local", &ticket)?;
        let fingerprint = mark_ready_fingerprint(&ticket, &request, &target);
        if ticket.meta.workflow_state != TicketWorkflowState::Planning {
            return Err(TicketError::StaleWorkflowState {
                expected: TicketWorkflowState::Planning.as_str().to_owned(),
                actual: ticket.meta.workflow_state.as_str().to_owned(),
            });
        }
        let reason = request
            .reason
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("implementation target validated");
        let mut change = TicketStateChange::new(
            TicketWorkflowState::Planning.as_str(),
            TicketWorkflowState::Ready.as_str(),
            reason,
            MarkdownText::new(format!(
                "Implementation target `{}` at selector `{}` was validated and the Ticket was marked ready.",
                target.repository_id, target.ref_selector
            )),
        );
        change.author = request.author.clone().or_else(|| Some(default_author()));
        if let Some(mut summary) = request.intake_summary {
            summary.author = request.author.clone().or(summary.author);
            self.append_intake_summary_event(&dir, &summary)?;
        }
        self.append_state_changed_event_with_attributes(
            &dir,
            &change,
            Some("state"),
            &[
                ("operation_key", request.operation_key.as_str()),
                ("request_fingerprint", fingerprint.as_str()),
                ("repository_id", target.repository_id.as_str()),
                ("ref_selector", target.ref_selector.as_str()),
            ],
        )?;
        self.set_frontmatter_fields(
            &dir.join("item.md"),
            &[
                ("state", TicketWorkflowState::Ready.as_str()),
                ("repository_id", target.repository_id.as_str()),
                ("ref_selector", target.ref_selector.as_str()),
            ],
        )?;
        self.ticket_from_dir(&dir)
    }

    fn queue_ready(&self, id: TicketIdOrSlug, queued_by: &str) -> Result<()> {
        validate_required_event_value("queued_by", queued_by)?;
        let _lock = self.acquire_lock()?;
        let dir = self.find_ticket_dir(&id)?;
        let item = dir.join("item.md");
        let meta = ticket_meta_for_dir(&dir, read_item_file(&item)?.frontmatter)?;
        if meta.workflow_state != TicketWorkflowState::Ready {
            return Err(TicketError::StaleWorkflowState {
                expected: TicketWorkflowState::Ready.as_str().to_owned(),
                actual: meta.workflow_state.as_str().to_owned(),
            });
        }
        let ticket = self.ticket_from_dir(&dir)?;
        let target = resolve_ready_target(self.target_authority.as_ref(), "local", &ticket)?;
        let blockers = self.relation_blockers_for_meta(&meta)?;
        let active_blockers = blockers
            .into_iter()
            .filter(|blocker| !relation_blocker_allows_queue(blocker))
            .collect::<Vec<_>>();
        if !active_blockers.is_empty() {
            return Err(TicketError::BlockingRelations(format!(
                "{}: {}",
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
            &[
                ("queued_by", queued_by),
                ("queued_at", at.as_str()),
                ("repository_id", target.repository_id.as_str()),
                ("ref_selector", target.ref_selector.as_str()),
            ],
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

    fn remove_ticket_relation(
        &self,
        id: TicketIdOrSlug,
        kind: TicketRelationKind,
        target: TicketIdOrSlug,
    ) -> Result<TicketRelation> {
        let _lock = self.acquire_lock()?;
        self.ensure_backend_dirs()?;
        let dir = self.find_ticket_dir(&id)?;
        let item = dir.join("item.md");
        let meta = ticket_meta_for_dir(&dir, read_item_file(&item)?.frontmatter)?;
        let target_id = match target {
            TicketIdOrSlug::Id(value) => value,
            other => ticket_id_from_dir(&self.find_ticket_dir(&other)?)?,
        };
        let path = self.ticket_relations_path(&dir);
        let mut relations = read_ticket_relations_artifact(&path, Some(&meta))?;
        let Some(index) = relations
            .iter()
            .position(|relation| relation.kind == kind && relation.target == target_id)
        else {
            return Err(TicketError::NotFound(format!(
                "relation {} {} {}",
                meta.id, kind, target_id
            )));
        };
        let removed = relations.remove(index);
        write_ticket_relations_artifact(&path, &relations)?;
        let at = now_utc();
        self.set_frontmatter_fields(&item, &[("updated_at", &at)])?;
        Ok(removed)
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
    repository_id: Option<String>,
    ref_selector: Option<String>,
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
        repository_id: yaml_string(&mapping, "repository_id")?,
        ref_selector: yaml_string(&mapping, "ref_selector")?,
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
        resource_key: None,
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
        repository_id: frontmatter.repository_id,
        ref_selector: frontmatter.ref_selector,
        raw: frontmatter.raw,
    }
}

fn ticket_summary_from_meta(meta: TicketMeta) -> TicketSummary {
    TicketSummary {
        id: meta.id,
        resource_key: meta.resource_key,
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
        TicketError::Sqlite(_) => "could not read ticket record",
        TicketError::NotFound(_) => "ticket record is missing",
        TicketError::Ambiguous { .. }
        | TicketError::Conflict(_)
        | TicketError::MissingTargetRepository
        | TicketError::UnknownTargetRepository(_)
        | TicketError::MissingTargetSelector(_)
        | TicketError::InvalidTargetSelector { .. }
        | TicketError::TargetAuthorityUnavailable
        | TicketError::StaleWorkflowState { .. }
        | TicketError::InvalidWorkflowTransition { .. }
        | TicketError::BlockingRelations(_)
        | TicketError::OperationFingerprintMismatch { .. } => "invalid ticket record metadata",
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

fn remove_frontmatter_fields(
    content: &str,
    fields: &[&str],
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
    for index in (1..end).rev() {
        let should_remove = lines[index]
            .split_once(':')
            .is_some_and(|(key, _)| fields.contains(&key.trim()));
        if should_remove {
            lines.remove(index);
        }
    }
    let mut out = lines.join("\n");
    if content.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

fn replace_item_body(content: &str, body: &str) -> std::result::Result<String, String> {
    let mut lines = content.lines();
    if lines.next() != Some("---") {
        return Err("item.md missing frontmatter opener".to_string());
    }
    let mut frontmatter = vec!["---".to_string()];
    let mut found_close = false;
    for line in lines.by_ref() {
        frontmatter.push(line.to_string());
        if line == "---" {
            found_close = true;
            break;
        }
    }
    if !found_close {
        return Err("item.md missing frontmatter closer".to_string());
    }
    let mut out = frontmatter.join("\n");
    out.push_str("\n");
    out.push_str(body);
    if !out.ends_with('\n') {
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
            let mut attrs = parse_event_comment(comment);
            let legacy_review = attrs.get("event").is_some_and(|value| value == "review");
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
            if legacy_review {
                heading = Some("Legacy review (non-authoritative)".to_string());
                attrs.remove("status");
                attrs.remove("result");
                attrs.insert("event".to_string(), "comment".to_string());
                attrs.insert("legacy_event_kind".to_string(), "review".to_string());
            }
            events.push(TicketEvent {
                kind,
                author: attrs.get("author").cloned(),
                at: attrs.get("at").cloned(),
                status: if legacy_review {
                    None
                } else {
                    attrs.get("status").cloned()
                },
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

    #[derive(Debug)]
    struct TestTargetAuthority;

    impl TicketTargetAuthority for TestTargetAuthority {
        fn resolve_target(
            &self,
            _workspace_id: &str,
            repository_id: Option<&str>,
            ref_selector: Option<&str>,
        ) -> Result<ResolvedTicketTarget> {
            let repository_id = repository_id.unwrap_or("main");
            if repository_id == "unknown" {
                return Err(TicketError::UnknownTargetRepository(
                    repository_id.to_owned(),
                ));
            }
            Ok(ResolvedTicketTarget {
                repository_id: repository_id.to_owned(),
                ref_selector: ref_selector.unwrap_or("develop").to_owned(),
            })
        }
    }

    fn backend(dir: &TempDir) -> LocalTicketBackend {
        LocalTicketBackend::new(dir.path().join("tickets"))
            .with_target_authority(Arc::new(TestTargetAuthority))
    }

    fn assert_ticket_target_edit_semantics<B: TicketBackend>(backend: &B) {
        let mut input = NewTicket::new("Target Ticket");
        input.repository_id = Some("main".to_string());
        input.ref_selector = Some("feature/api".to_string());
        let created = backend.create(input).unwrap();
        let ticket = backend
            .show(TicketIdOrSlug::Id(created.id.clone()))
            .unwrap();
        assert_eq!(ticket.meta.repository_id.as_deref(), Some("main"));
        assert_eq!(ticket.meta.ref_selector.as_deref(), Some("feature/api"));

        let edited = backend
            .edit_item(
                TicketIdOrSlug::Id(created.id.clone()),
                TicketItemEdit {
                    target: Some(TicketTargetEdit::Set {
                        repository_id: "secondary".to_string(),
                        ref_selector: None,
                    }),
                    author: Some("tester".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(edited.meta.repository_id.as_deref(), Some("secondary"));
        assert_eq!(edited.meta.ref_selector, None);
        let edit_event = edited
            .events
            .iter()
            .rev()
            .find(|event| event.kind == TicketEventKind::Other("item_edit".to_string()))
            .expect("item_edit event");
        assert_eq!(
            edit_event.attributes.get("changes"),
            Some(&"target".to_string())
        );

        let cleared = backend
            .edit_item(
                TicketIdOrSlug::Id(created.id),
                TicketItemEdit {
                    target: Some(TicketTargetEdit::Clear),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(cleared.meta.repository_id, None);
        assert_eq!(cleared.meta.ref_selector, None);
    }

    fn assert_partial_body_replacement_semantics<B: TicketBackend>(backend: &B) {
        let mut input = NewTicket::new("Body Edit Ticket");
        input.body = MarkdownText::new("alpha\nbeta\nalpha\n");
        let created = backend.create(input).unwrap();

        let edited = backend
            .edit_item(
                TicketIdOrSlug::Id(created.id.clone()),
                TicketItemEdit {
                    body_replacement: Some(TicketBodyReplacement {
                        old_string: "alpha".to_string(),
                        new_string: "ALPHA".to_string(),
                        replace_all: true,
                    }),
                    author: Some("tester".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            edited.document.body.as_str().trim_start_matches('\n'),
            "ALPHA\nbeta\nALPHA\n"
        );
        let edit_event = edited
            .events
            .iter()
            .rev()
            .find(|event| event.kind == TicketEventKind::Other("item_edit".to_string()))
            .expect("item_edit event");
        assert_eq!(
            edit_event.attributes.get("body_edit"),
            Some(&"partial".to_string())
        );
        assert_eq!(
            edit_event.attributes.get("replacement_count"),
            Some(&"2".to_string())
        );
        assert!(edit_event.body.as_str().contains("2 occurrence"));

        let duplicate_err = backend
            .edit_item(
                TicketIdOrSlug::Id(created.id.clone()),
                TicketItemEdit {
                    body_replacement: Some(TicketBodyReplacement {
                        old_string: "ALPHA".to_string(),
                        new_string: "alpha".to_string(),
                        replace_all: false,
                    }),
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert!(matches!(duplicate_err, TicketError::Conflict(_)));

        let missing_err = backend
            .edit_item(
                TicketIdOrSlug::Id(created.id.clone()),
                TicketItemEdit {
                    body_replacement: Some(TicketBodyReplacement {
                        old_string: "missing".to_string(),
                        new_string: "present".to_string(),
                        replace_all: false,
                    }),
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert!(matches!(missing_err, TicketError::NotFound(_)));

        let ambiguous_err = backend
            .edit_item(
                TicketIdOrSlug::Id(created.id),
                TicketItemEdit {
                    body: Some(MarkdownText::new("whole body")),
                    body_replacement: Some(TicketBodyReplacement {
                        old_string: "beta".to_string(),
                        new_string: "BETA".to_string(),
                        replace_all: false,
                    }),
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert!(matches!(ambiguous_err, TicketError::Conflict(_)));
    }

    fn assert_ticket_relation_removal_semantics<B: TicketBackend>(backend: &B) {
        let source = backend.create(NewTicket::new("source")).unwrap();
        let target = backend.create(NewTicket::new("target")).unwrap();
        backend
            .add_ticket_relation(
                TicketIdOrSlug::Id(source.id.clone()),
                NewTicketRelation {
                    kind: TicketRelationKind::DependsOn,
                    target: target.id.clone(),
                    note: Some("obsolete blocker".to_string()),
                    author: Some("tester".to_string()),
                },
            )
            .unwrap();

        let removed = backend
            .remove_ticket_relation(
                TicketIdOrSlug::Id(source.id.clone()),
                TicketRelationKind::DependsOn,
                TicketIdOrSlug::Id(target.id.clone()),
            )
            .unwrap();
        assert_eq!(removed.ticket_id, source.id);
        assert_eq!(removed.target, target.id);
        assert_eq!(removed.note.as_deref(), Some("obsolete blocker"));
        assert!(
            backend
                .relation_view(TicketIdOrSlug::Id(source.id.clone()))
                .unwrap()
                .outgoing
                .is_empty()
        );
        assert!(
            backend
                .relation_view(TicketIdOrSlug::Id(target.id.clone()))
                .unwrap()
                .incoming
                .is_empty()
        );
        assert!(matches!(
            backend.remove_ticket_relation(
                TicketIdOrSlug::Id(source.id),
                TicketRelationKind::DependsOn,
                TicketIdOrSlug::Id(target.id),
            ),
            Err(TicketError::NotFound(_))
        ));
    }

    fn summary_with_state(state: TicketWorkflowState) -> TicketSummary {
        TicketSummary {
            id: "000TEST".to_string(),
            resource_key: Some("T-1".to_string()),
            slug: "000TEST".to_string(),
            title: "Test Ticket".to_string(),
            status: ExtensibleTicketStatus::Open,
            kind: "ticket".to_string(),
            priority: "P2".to_string(),
            labels: Vec::new(),
            readiness: None,
            workflow_state: state,
            workflow_state_explicit: true,
            queued_by: None,
            queued_at: None,
            updated_at: Some("2026-07-20T00:00:00Z".to_string()),
        }
    }

    fn blocker_with_state(state: TicketWorkflowState) -> TicketRelationBlocker {
        TicketRelationBlocker {
            blocking_ticket: "000BLOCK".to_string(),
            reason_kind: "depends_on".to_string(),
            relation_kind: TicketRelationKind::DependsOn,
            note: None,
            blocking_state: state,
        }
    }

    #[test]
    fn workspace_projection_queues_ready_ticket_for_orchestrator() {
        let summary = summary_with_state(TicketWorkflowState::Ready);
        let projection = project_ticket_workspace_item(&summary, &[], None);

        assert_eq!(projection.kind, TicketWorkspaceRowKind::Ticket);
        assert_eq!(
            projection.priority,
            TicketWorkspaceActionPriority::ReadyForQueue
        );
        assert_eq!(
            projection.next_action,
            Some(TicketWorkspaceNextAction::QueueForOrchestrator)
        );
        assert!(projection.queue_guard.can_queue_for_orchestrator);
        assert!(projection.disabled_reason.is_none());
    }

    #[test]
    fn workspace_projection_blocks_ready_queue_on_unstarted_dependency() {
        let summary = summary_with_state(TicketWorkflowState::Ready);
        let blockers = [blocker_with_state(TicketWorkflowState::Planning)];
        let projection = project_ticket_workspace_item(&summary, &blockers, None);

        assert_eq!(projection.kind, TicketWorkspaceRowKind::Ticket);
        assert_eq!(
            projection.next_action,
            Some(TicketWorkspaceNextAction::WaitForOrchestrator)
        );
        assert!(!projection.queue_guard.can_queue_for_orchestrator);
        assert!(projection.blocked_reason.is_some());
        assert!(projection.disabled_reason.is_some());
    }

    #[test]
    fn workspace_projection_allows_ready_queue_when_dependency_is_already_queued() {
        let summary = summary_with_state(TicketWorkflowState::Ready);
        let blockers = [blocker_with_state(TicketWorkflowState::Queued)];
        let projection = project_ticket_workspace_item(&summary, &blockers, None);

        assert_eq!(
            projection.next_action,
            Some(TicketWorkspaceNextAction::QueueForOrchestrator)
        );
        assert!(projection.queue_guard.can_queue_for_orchestrator);
        assert!(projection.blocked_reason.is_none());
        assert!(
            projection
                .key_hint
                .as_deref()
                .unwrap_or_default()
                .contains("Orchestrator will preserve order")
        );
    }

    #[test]
    fn workspace_projection_overlay_suppresses_duplicate_queue() {
        let summary = summary_with_state(TicketWorkflowState::Ready);
        let overlay = TicketWorkspaceStateOverlay {
            source: "orchestration".to_string(),
            workflow_state: TicketWorkflowState::InProgress,
        };
        let projection = project_ticket_workspace_item(&summary, &[], Some(&overlay));

        assert_eq!(projection.kind, TicketWorkspaceRowKind::ActiveWork);
        assert_eq!(
            projection.next_action,
            Some(TicketWorkspaceNextAction::WaitForOrchestrator)
        );
        assert_eq!(projection.visible_state, "ready→prog");
        assert!(!projection.queue_guard.can_queue_for_orchestrator);
        assert!(projection.visible_overlay.is_some());
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
    fn local_backend_persists_and_edits_ticket_target() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        assert_ticket_target_edit_semantics(&backend);
    }

    #[test]
    fn sqlite_mark_ready_and_queue_enforce_target_and_blockers_atomically() {
        let tmp = TempDir::new().unwrap();
        let backend = SqliteTicketBackend::open(tmp.path().join("workspace.db"), "workspace-test")
            .unwrap()
            .with_target_authority(Arc::new(TestTargetAuthority));
        let mut dependency = NewTicket::new("Dependency");
        dependency.repository_id = Some("main".to_owned());
        let dependency = backend.create(dependency).unwrap();
        let mut implementation = NewTicket::new("Implementation");
        implementation.repository_id = Some("main".to_owned());
        let implementation = backend.create(implementation).unwrap();
        backend
            .add_ticket_relation(
                TicketIdOrSlug::Id(implementation.id.clone()),
                NewTicketRelation {
                    kind: TicketRelationKind::DependsOn,
                    target: dependency.id.clone(),
                    note: None,
                    author: Some("test".to_owned()),
                },
            )
            .unwrap();
        let request = TicketMarkReady {
            operation_key: "sqlite-ready".to_owned(),
            reason: Some("target accepted".to_owned()),
            author: Some("test".to_owned()),
            intake_summary: None,
        };
        let ready = backend
            .mark_ready(
                TicketIdOrSlug::Id(implementation.id.clone()),
                request.clone(),
            )
            .unwrap();
        assert_eq!(ready.meta.ref_selector.as_deref(), Some("develop"));
        let replay = backend
            .mark_ready(TicketIdOrSlug::Id(implementation.id.clone()), request)
            .unwrap();
        assert_eq!(
            replay
                .events
                .iter()
                .filter(|event| event.attributes.contains_key("operation_key"))
                .count(),
            1
        );
        assert!(matches!(
            backend.queue_ready(
                TicketIdOrSlug::Id(implementation.id.clone()),
                "orchestrator",
            ),
            Err(TicketError::BlockingRelations(_))
        ));
        let after_rejection = backend
            .show(TicketIdOrSlug::Id(implementation.id.clone()))
            .unwrap();
        assert_eq!(
            after_rejection.meta.workflow_state,
            TicketWorkflowState::Ready
        );
        assert!(!after_rejection.events.iter().any(|event| {
            event.from.as_deref() == Some("ready") && event.to.as_deref() == Some("queued")
        }));
        backend
            .close(
                TicketIdOrSlug::Id(dependency.id),
                MarkdownText::new("resolved"),
            )
            .unwrap();
        backend
            .queue_ready(
                TicketIdOrSlug::Id(implementation.id.clone()),
                "orchestrator",
            )
            .unwrap();
        assert_eq!(
            backend
                .show(TicketIdOrSlug::Id(implementation.id))
                .unwrap()
                .meta
                .workflow_state,
            TicketWorkflowState::Queued
        );
    }

    #[test]
    fn sqlite_resource_keys_are_workspace_scoped_monotonic_and_resolvable() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("workspace.db");
        let backend = SqliteTicketBackend::open(&db_path, "workspace-a").unwrap();
        let first = backend.create(NewTicket::new("First")).unwrap();
        let second = backend.create(NewTicket::new("Second")).unwrap();
        assert_eq!(first.resource_key.as_deref(), Some("T-1"));
        assert_eq!(second.resource_key.as_deref(), Some("T-2"));
        assert_eq!(backend.show("T-1".into()).unwrap().meta.id, first.id);
        let projection = backend.list_workspace_projection(100).unwrap();
        let projected_second = projection
            .items
            .iter()
            .find(|item| item.summary.id == second.id)
            .unwrap();
        assert_eq!(
            projected_second.summary.resource_key.as_deref(),
            Some("T-2")
        );

        let other = SqliteTicketBackend::open(&db_path, "workspace-b").unwrap();
        let other_first = other.create(NewTicket::new("Other")).unwrap();
        assert_eq!(other_first.resource_key.as_deref(), Some("T-1"));
        assert_eq!(other.show("T-1".into()).unwrap().meta.id, other_first.id);
    }

    #[test]
    fn sqlite_resource_key_allocation_is_concurrency_safe() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("workspace.db");
        SqliteTicketBackend::open(&db_path, "workspace-a").unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(8));
        let handles = (0..8)
            .map(|index| {
                let db_path = db_path.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let backend = SqliteTicketBackend::open(db_path, "workspace-a").unwrap();
                    barrier.wait();
                    backend
                        .create(NewTicket::new(format!("Ticket {index}")))
                        .unwrap()
                        .resource_key
                        .unwrap()
                })
            })
            .collect::<Vec<_>>();
        let mut keys = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        keys.sort_by_key(|key| key.trim_start_matches("T-").parse::<u64>().unwrap());
        assert_eq!(keys, (1..=8).map(|n| format!("T-{n}")).collect::<Vec<_>>());
    }

    #[test]
    fn sqlite_backend_persists_and_edits_ticket_target() {
        let tmp = TempDir::new().unwrap();
        let backend =
            SqliteTicketBackend::open(tmp.path().join("workspace.db"), "workspace-test").unwrap();
        assert_ticket_target_edit_semantics(&backend);
    }

    #[test]
    fn local_backend_removes_ticket_relations() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        assert_ticket_relation_removal_semantics(&backend);
    }

    #[test]
    fn sqlite_backend_removes_ticket_relations() {
        let tmp = TempDir::new().unwrap();
        let backend =
            SqliteTicketBackend::open(tmp.path().join("workspace.db"), "workspace-test").unwrap();
        assert_ticket_relation_removal_semantics(&backend);
    }

    #[test]
    fn local_backend_edit_item_supports_partial_body_replacement() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        assert_partial_body_replacement_semantics(&backend);
    }

    #[test]
    fn sqlite_backend_edit_item_supports_partial_body_replacement() {
        let tmp = TempDir::new().unwrap();
        let backend =
            SqliteTicketBackend::open(tmp.path().join("workspace.db"), "workspace-test").unwrap();
        assert_partial_body_replacement_semantics(&backend);
    }

    #[test]
    fn sqlite_mutation_hook_failure_rolls_back_ticket_event() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("workspace.db");
        let backend = SqliteTicketBackend::open(&db_path, "workspace-test").unwrap();
        let created = backend.create(NewTicket::new("Atomic mutation")).unwrap();
        let before = backend
            .show(TicketIdOrSlug::Id(created.id.clone()))
            .unwrap()
            .events
            .len();
        let failing = backend.clone().with_mutation_hook(Arc::new(|_, event| {
            Err(TicketError::Conflict(format!(
                "reject outbox for {}:{}",
                event.ticket_id, event.event_index
            )))
        }));
        assert!(
            failing
                .add_event(
                    TicketIdOrSlug::Id(created.id.clone()),
                    NewTicketEvent::new(TicketEventKind::Comment, "must roll back"),
                )
                .is_err()
        );
        let after = backend.show(TicketIdOrSlug::Id(created.id)).unwrap();
        assert_eq!(after.events.len(), before);
        assert!(
            after
                .events
                .iter()
                .all(|event| event.body.as_str() != "must roll back")
        );
    }

    #[test]
    fn sqlite_workspace_projection_preserves_order_limit_and_blockers_without_full_loads() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("workspace.db");
        let backend = SqliteTicketBackend::open(&db_path, "workspace-test").unwrap();
        let mut blocker_input = NewTicket::new("Blocker");
        blocker_input.workflow_state = Some(TicketWorkflowState::InProgress);
        let blocker = backend.create(blocker_input).unwrap();
        let mut blocked_input = NewTicket::new("Blocked");
        blocked_input.workflow_state = Some(TicketWorkflowState::Ready);
        let blocked = backend.create(blocked_input).unwrap();
        let mut newest_input = NewTicket::new("Newest");
        newest_input.workflow_state = Some(TicketWorkflowState::Ready);
        let newest = backend.create(newest_input).unwrap();
        backend
            .add_ticket_relation(
                TicketIdOrSlug::Id(blocked.id.clone()),
                NewTicketRelation {
                    kind: TicketRelationKind::DependsOn,
                    target: blocker.id.clone(),
                    note: Some("wait".to_string()),
                    author: Some("test".to_string()),
                },
            )
            .unwrap();
        let connection = Connection::open(&db_path).unwrap();
        connection
            .execute(
                "UPDATE typed_tickets SET updated_at = CASE ticket_id
                    WHEN ?2 THEN '2026-08-12T03:00:00Z'
                    WHEN ?3 THEN '2026-08-12T02:00:00Z'
                    ELSE '2026-08-12T01:00:00Z' END
                 WHERE workspace_id = ?1",
                params!["workspace-test", newest.id, blocked.id],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO typed_ticket_events
                    (workspace_id, ticket_id, event_index, kind, author, at, body)
                 VALUES (?1, ?2, 99, 'comment', 'test', '2026-08-12T00:00:00Z', ?3)",
                params!["workspace-test", blocked.id, "full-event-marker"],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO typed_ticket_artifacts
                    (workspace_id, ticket_id, relative_path, content)
                 VALUES (?1, ?2, 'full-artifact-marker', X'01')",
                params!["workspace-test", blocked.id],
            )
            .unwrap();
        drop(connection);

        let full_loads_before = backend.full_ticket_load_count.load(Ordering::SeqCst);
        let all_projection = backend.list_workspace_projection(3).unwrap();
        let blocked_with_both_relation_ends_listed = all_projection
            .items
            .iter()
            .find(|item| item.summary.id == blocked.id)
            .expect("blocked Ticket is listed");
        assert_eq!(
            blocked_with_both_relation_ends_listed
                .relation_blockers
                .len(),
            1,
            "a relation must not duplicate when both endpoints are listed",
        );
        let projection = backend.list_workspace_projection(2).unwrap();
        assert_eq!(
            backend.full_ticket_load_count.load(Ordering::SeqCst),
            full_loads_before,
            "bulk projection must not run a full Ticket load per listed item",
        );
        assert_eq!(
            projection
                .items
                .iter()
                .map(|item| item.summary.id.as_str())
                .collect::<Vec<_>>(),
            vec![newest.id.as_str(), blocked.id.as_str()]
        );
        let blocked_item = &projection.items[1];
        assert_eq!(blocked_item.relation_blockers.len(), 1);
        assert_eq!(
            blocked_item.relation_blockers[0].blocking_ticket,
            blocker.id
        );
        assert_eq!(
            blocked_item.relation_blockers[0].blocking_state,
            TicketWorkflowState::InProgress
        );
        assert_eq!(blocked_item.relation_blockers[0].reason_kind, "depends_on");
        let debug = format!("{projection:?}");
        assert!(!debug.contains("full-event-marker"));
        assert!(!debug.contains("full-artifact-marker"));
    }

    #[test]
    fn sqlite_workspace_projection_page_uses_stable_keyset_and_state_filter() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("workspace.db");
        let backend = SqliteTicketBackend::open(&db_path, "workspace-test").unwrap();
        let mut ids = Vec::new();
        for (title, state, updated_at) in [
            ("Newest", TicketWorkflowState::Ready, "2026-08-12T03:00:00Z"),
            ("Middle", TicketWorkflowState::Ready, "2026-08-12T02:00:00Z"),
            (
                "Oldest",
                TicketWorkflowState::Planning,
                "2026-08-12T01:00:00Z",
            ),
        ] {
            let mut input = NewTicket::new(title);
            input.workflow_state = Some(state);
            let ticket = backend.create(input).unwrap();
            Connection::open(&db_path)
                .unwrap()
                .execute(
                    "UPDATE typed_tickets SET updated_at=?3 WHERE workspace_id=?1 AND ticket_id=?2",
                    params!["workspace-test", ticket.id, updated_at],
                )
                .unwrap();
            ids.push(ticket.id);
        }

        Connection::open(&db_path)
            .unwrap()
            .execute(
                "UPDATE typed_tickets SET updated_at='2026-08-12T05:00:00Z' WHERE workspace_id=?1 AND ticket_id=?2",
                params!["workspace-test", ids[2]],
            )
            .unwrap();
        let combined_lane = backend
            .list_workspace_projection_page(SqliteTicketListPageQuery {
                states: vec![TicketWorkflowState::Ready, TicketWorkflowState::Planning],
                limit: 1,
                after: None,
            })
            .unwrap();
        assert_eq!(
            combined_lane.items[0].summary.workflow_state,
            TicketWorkflowState::Ready,
            "lane pagination order must match the UI's state-primary order",
        );

        let first = backend
            .list_workspace_projection_page(SqliteTicketListPageQuery {
                states: vec![TicketWorkflowState::Ready],
                limit: 1,
                after: None,
            })
            .unwrap();
        assert_eq!(first.items[0].summary.id, ids[0]);
        assert!(first.has_more);

        let mut inserted = NewTicket::new("Inserted");
        inserted.workflow_state = Some(TicketWorkflowState::Ready);
        let inserted = backend.create(inserted).unwrap();
        Connection::open(&db_path)
            .unwrap()
            .execute(
                "UPDATE typed_tickets SET updated_at='2026-08-12T04:00:00Z' WHERE workspace_id=?1 AND ticket_id=?2",
                params!["workspace-test", inserted.id],
            )
            .unwrap();

        let second = backend
            .list_workspace_projection_page(SqliteTicketListPageQuery {
                states: vec![TicketWorkflowState::Ready],
                limit: 1,
                after: first.next,
            })
            .unwrap();
        assert_eq!(second.items[0].summary.id, ids[1]);
        assert!(!second.has_more);
    }

    #[test]
    fn sqlite_workspace_projection_sql_shape_is_constant_for_ticket_count() {
        let source = include_str!("lib.rs");
        let start = source
            .find("pub fn list_workspace_projection")
            .expect("projection method");
        let end = source[start..]
            .find("pub fn import_from_local_backend")
            .map(|offset| start + offset)
            .expect("following method");
        let projection_source = &source[start..end];
        assert_eq!(projection_source.matches("self.with_read(").count(), 2);
        assert_eq!(projection_source.matches(".prepare(").count(), 3);
        let item_loop = projection_source
            .split("items: summaries")
            .nth(1)
            .expect("summary projection loop");
        assert!(!item_loop.contains("open_connection("));
        assert!(!item_loop.contains("verify_sqlite_ticket_schema("));
        assert!(!item_loop.contains("self.load_ticket("));
        assert!(!projection_source.contains("typed_ticket_events"));
        assert!(!projection_source.contains("typed_ticket_event_references"));
        assert!(!projection_source.contains("typed_ticket_artifacts"));
    }

    #[test]
    fn sqlite_backend_persists_core_ticket_operations() {
        let tmp = TempDir::new().unwrap();
        let backend =
            SqliteTicketBackend::open(tmp.path().join("workspace.db"), "workspace-test").unwrap();
        let created = backend.create(NewTicket::new("SQLite Ticket")).unwrap();
        backend
            .add_event(
                TicketIdOrSlug::Id(created.id.clone()),
                NewTicketEvent::new(TicketEventKind::Comment, "Imported into SQLite."),
            )
            .unwrap();
        backend
            .close(
                TicketIdOrSlug::Id(created.id.clone()),
                MarkdownText::new("Done."),
            )
            .unwrap();

        let reopened =
            SqliteTicketBackend::open_verified(tmp.path().join("workspace.db"), "workspace-test")
                .unwrap();
        let list = reopened.list(TicketListQuery::all()).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, created.id);
        assert_eq!(list[0].workflow_state, TicketWorkflowState::Closed);
        let ticket = reopened.show(TicketIdOrSlug::Id(created.id)).unwrap();
        assert_eq!(ticket.meta.title, "SQLite Ticket");
        assert!(ticket.events.iter().any(|event| {
            event.kind == TicketEventKind::Comment && event.body.0.contains("Imported into SQLite")
        }));
        assert!(
            ticket
                .resolution
                .as_ref()
                .is_some_and(|resolution| resolution.0.contains("Done"))
        );
    }

    #[test]
    fn sqlite_backend_imports_legacy_local_layout_explicitly() {
        let tmp = TempDir::new().unwrap();
        let local = backend(&tmp);
        let created = local.create(NewTicket::new("Legacy Ticket")).unwrap();
        let db =
            SqliteTicketBackend::open(tmp.path().join("workspace.db"), "workspace-test").unwrap();
        db.import_from_local_backend(&local).unwrap();

        let ticket = db.show(TicketIdOrSlug::Id(created.id.clone())).unwrap();
        assert_eq!(ticket.meta.id, created.id);
        assert_eq!(ticket.meta.title, "Legacy Ticket");

        fs::remove_dir_all(local.root()).unwrap();
        let ticket = db.show(TicketIdOrSlug::Id(created.id)).unwrap();
        assert_eq!(ticket.meta.title, "Legacy Ticket");
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
    fn add_event_status_and_close_preserve_local_layout() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let mut input = NewTicket::new("Flow Ticket");
        input.repository_id = Some("main".to_owned());
        let ticket = backend.create(input).unwrap();
        backend
            .add_event(
                TicketIdOrSlug::Id(ticket.id.clone()),
                NewTicketEvent::new(TicketEventKind::Plan, "Implementation plan."),
            )
            .unwrap();
        backend
            .mark_ready(
                TicketIdOrSlug::Id(ticket.id.clone()),
                TicketMarkReady {
                    operation_key: "test-flow-ready".to_owned(),
                    reason: Some("ready_for_queue".to_owned()),
                    author: Some("test".to_owned()),
                    intake_summary: None,
                },
            )
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
        ready_input.repository_id = Some("main".to_owned());
        ready_input.ref_selector = Some("develop".to_owned());
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
            Err(TicketError::StaleWorkflowState { .. })
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
    fn mark_ready_resolves_target_and_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let mut input = NewTicket::new("Planning Ready");
        input.repository_id = Some("main".to_owned());
        let ticket = backend.create(input).unwrap();
        let request = TicketMarkReady {
            operation_key: "ready-op-1".to_owned(),
            reason: Some("accepted".to_owned()),
            author: Some("intake".to_owned()),
            intake_summary: None,
        };

        let first = backend
            .mark_ready(TicketIdOrSlug::Id(ticket.id.clone()), request.clone())
            .unwrap();
        let second = backend
            .mark_ready(TicketIdOrSlug::Id(ticket.id.clone()), request)
            .unwrap();
        assert_eq!(first.meta.workflow_state, TicketWorkflowState::Ready);
        assert_eq!(first.meta.repository_id.as_deref(), Some("main"));
        assert_eq!(first.meta.ref_selector.as_deref(), Some("develop"));
        assert_eq!(first.events, second.events);
        assert_eq!(
            first
                .events
                .iter()
                .filter(|event| {
                    event.kind == TicketEventKind::StateChanged
                        && event.from.as_deref() == Some("planning")
                        && event.to.as_deref() == Some("ready")
                })
                .count(),
            1
        );
        assert!(matches!(
            backend.mark_ready(
                TicketIdOrSlug::Id(ticket.id),
                TicketMarkReady {
                    operation_key: "ready-op-1".to_owned(),
                    reason: Some("different".to_owned()),
                    author: Some("intake".to_owned()),
                    intake_summary: None,
                },
            ),
            Err(TicketError::OperationFingerprintMismatch { .. })
        ));
    }

    #[test]
    fn close_sets_state_closed() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let input = NewTicket::new("Close Workflow");
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
