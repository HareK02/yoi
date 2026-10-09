//! Ticket domain types and SQLite Workspace authority backend.
//!
//! Repository-local Ticket trees are not an authority. Runtime Workers access
//! Tickets through the Workspace API, whose control-plane implementation uses
//! the SQLite backend defined here.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::Utc;
use project_record::{allocate_record_id, unix_epoch_millis_now};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub mod config;
mod sqlite_schema;
pub mod tool;

pub use sqlite_schema::{migrate_sqlite_ticket_schema, verify_sqlite_ticket_schema};

const MAX_INTAKE_SUMMARY_BODY_BYTES: usize = 16 * 1024;
const MAX_ORCHESTRATION_PLAN_TEXT_BYTES: usize = 16 * 1024;
const MAX_ORCHESTRATION_PLAN_FIELD_BYTES: usize = 1024;
const DEFAULT_TICKET_BODY: &str = "## Background\n\n## Acceptance criteria\n\n- TBD\n";

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
        "ticket target selector `{selector}` is invalid for repository `{repository_key}`: {reason}"
    )]
    InvalidTargetSelector {
        repository_key: String,
        selector: String,
        reason: String,
    },
    #[error("ticket target repository `{0}` appears more than once")]
    DuplicateTargetRepository(String),
    #[error("ticket requires at least one read_write target, found {0}")]
    InvalidReadWriteTargetCount(usize),
    #[error("ticket target authority is unavailable")]
    TargetAuthorityUnavailable,
    #[error("stale ticket item revision: expected `{expected}`, found `{actual}`")]
    StaleItemRevision { expected: String, actual: String },
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

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
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

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
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

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
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

    pub fn is_queue_transition(_from: Self, to: Self) -> bool {
        to == Self::Queued
    }

    /// Compatibility helper for internal callers; state is not a role phase path.
    pub fn is_role_transition(from: Self, to: Self) -> bool {
        from != to
    }
}

impl fmt::Display for TicketWorkflowState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TicketReference {
    pub kind: String,
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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

/// Audited completion independent of repositories, Git, or Merge Requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketCompletion {
    pub expected_item_revision: String,
    pub expected_state: TicketWorkflowState,
    pub operation_key: String,
    pub reason: String,
    #[serde(default)]
    pub references: Vec<TicketReference>,
    #[serde(default)]
    pub author: Option<String>,
}

/// External state mutation with item/state CAS and an immutable operation receipt.
/// Exact replay returns the current authoritative Ticket without a new event or
/// reapplying the recorded state, even after later edits or reopening.
/// Setting `state` to Closed also stores `reason` as the resolution; reopening
/// changes Ticket metadata only and never revives released work identities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketStateUpdate {
    pub expected_item_revision: String,
    pub expected_state: TicketWorkflowState,
    pub state: TicketWorkflowState,
    pub operation_key: String,
    pub reason: String,
    #[serde(default)]
    pub references: Vec<TicketReference>,
    #[serde(default)]
    pub author: Option<String>,
}

impl From<TicketCompletion> for TicketStateUpdate {
    fn from(request: TicketCompletion) -> Self {
        Self {
            expected_item_revision: request.expected_item_revision,
            expected_state: request.expected_state,
            state: TicketWorkflowState::Done,
            operation_key: request.operation_key,
            reason: request.reason,
            references: request.references,
            author: request.author,
        }
    }
}

/// Canonical item revision shared by Ticket authority, item checkers, and Merge
/// Request review records. Only persisted item_edit event indexes advance it.
pub fn ticket_item_revision(ticket: &Ticket) -> String {
    let index = ticket
        .events
        .iter()
        .filter(|event| event.kind == TicketEventKind::Other("item_edit".to_owned()))
        .filter_map(|event| event.attributes.get("event_sequence")?.parse::<i64>().ok())
        .max()
        .unwrap_or(0);
    format!("{}:{index}", ticket.meta.id)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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
    #[serde(default)]
    pub targets: Vec<TicketTarget>,
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
            targets: Vec::new(),
        }
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum TicketTargetAccess {
    ReadOnly,
    ReadWrite,
}

impl TicketTargetAccess {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::ReadWrite => "read_write",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "read_only" => Some(Self::ReadOnly),
            "read_write" => Some(Self::ReadWrite),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketTarget {
    pub repository_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ref_selector: Option<String>,
    pub access: TicketTargetAccess,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum TicketTargetsEdit {
    Set { targets: Vec<TicketTarget> },
    Clear,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedTicketTarget {
    pub repository_key: String,
    pub ref_selector: Option<String>,
    pub access: TicketTargetAccess,
}

/// Workspace-owned authority used to resolve and validate implementation targets.
///
/// Ticket storage never infers repositories from cwd or repository paths. The
/// Workspace Backend supplies this boundary from its authoritative repository
/// catalog, validating repository identity and selector syntax without Git
/// resolution. Empty targets are valid in every Ticket state.
pub trait TicketTargetAuthority: Send + Sync {
    fn resolve_target(
        &self,
        workspace_id: &str,
        repository_key: &str,
        ref_selector: Option<&str>,
    ) -> Result<ResolvedTicketTarget>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TicketMarkReady {
    pub operation_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intake_summary: Option<TicketIntakeSummary>,
}

impl TicketTargetsEdit {
    fn validate(&self) -> Result<()> {
        if let Self::Set { targets } = self {
            validate_ticket_targets(targets)?;
        }
        Ok(())
    }
}

fn validate_ticket_targets(targets: &[TicketTarget]) -> Result<()> {
    let mut repositories = BTreeSet::new();
    for target in targets {
        validate_required_event_value("repository_key", &target.repository_key)?;
        if let Some(ref_selector) = target.ref_selector.as_deref() {
            validate_required_event_value("ref_selector", ref_selector)?;
        }
        if !repositories.insert(target.repository_key.as_str()) {
            return Err(TicketError::DuplicateTargetRepository(
                target.repository_key.clone(),
            ));
        }
    }
    Ok(())
}

fn resolve_ready_targets(
    authority: Option<&Arc<dyn TicketTargetAuthority>>,
    workspace_id: &str,
    targets: &[TicketTarget],
) -> Result<Vec<ResolvedTicketTarget>> {
    validate_ticket_targets(targets)?;
    if targets.is_empty() {
        return Ok(Vec::new());
    }
    let authority = authority.ok_or(TicketError::TargetAuthorityUnavailable)?;
    let mut resolved = Vec::with_capacity(targets.len());
    let mut repositories = BTreeSet::new();
    for target in targets {
        let mut canonical = authority.resolve_target(
            workspace_id,
            &target.repository_key,
            target.ref_selector.as_deref(),
        )?;
        validate_required_event_value("repository_key", &canonical.repository_key)?;
        if let Some(selector) = canonical.ref_selector.as_deref() {
            validate_required_event_value("ref_selector", selector)?;
        }
        canonical.access = target.access;
        if !repositories.insert(canonical.repository_key.clone()) {
            return Err(TicketError::DuplicateTargetRepository(
                canonical.repository_key,
            ));
        }
        resolved.push(canonical);
    }
    Ok(resolved)
}

fn canonical_ticket_targets(targets: &[ResolvedTicketTarget]) -> Vec<TicketTarget> {
    targets
        .iter()
        .map(|target| TicketTarget {
            repository_key: target.repository_key.clone(),
            ref_selector: target.ref_selector.clone(),
            access: target.access,
        })
        .collect()
}

fn encoded_resolved_targets(targets: &[ResolvedTicketTarget]) -> Result<String> {
    serde_json::to_string(targets)
        .map_err(|error| TicketError::Conflict(format!("failed to encode Ticket targets: {error}")))
}

fn mark_ready_fingerprint_v1(
    ticket: &Ticket,
    request: &TicketMarkReady,
    target: &ResolvedTicketTarget,
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"ticket.mark-ready.v1\0");
    digest.update(ticket.meta.id.as_str().as_bytes());
    digest.update(b"\0planning\0");
    digest.update(target.repository_key.as_bytes());
    digest.update(b"\0");
    digest.update(
        target
            .ref_selector
            .as_deref()
            .unwrap_or_default()
            .as_bytes(),
    );
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

fn mark_ready_fingerprint(
    ticket: &Ticket,
    request: &TicketMarkReady,
    targets: &[ResolvedTicketTarget],
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"ticket.mark-ready.v2\0");
    digest.update(ticket.meta.id.as_str().as_bytes());
    digest.update(b"\0planning\0");
    for target in targets {
        digest.update(target.repository_key.as_bytes());
        digest.update(b"\0");
        digest.update(
            target
                .ref_selector
                .as_deref()
                .unwrap_or_default()
                .as_bytes(),
        );
        digest.update(b"\0");
        digest.update(target.access.as_str().as_bytes());
        digest.update(b"\0");
    }
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
    let targets = event
        .attributes
        .get("targets")
        .ok_or_else(|| TicketError::Conflict("mark-ready event is missing targets".to_owned()))
        .and_then(|targets| {
            serde_json::from_str::<Vec<ResolvedTicketTarget>>(targets).map_err(|error| {
                TicketError::Conflict(format!("mark-ready event has invalid targets: {error}"))
            })
        })?;
    let fingerprint = match event
        .attributes
        .get("fingerprint_version")
        .map(String::as_str)
        .unwrap_or("1")
    {
        "1" if targets.len() == 1 => mark_ready_fingerprint_v1(ticket, request, &targets[0]),
        "2" => mark_ready_fingerprint(ticket, request, &targets),
        version => {
            return Err(TicketError::Conflict(format!(
                "mark-ready event has unsupported fingerprint version {version:?}"
            )));
        }
    };
    if event.attributes.get("request_fingerprint") != Some(&fingerprint) {
        return Err(TicketError::OperationFingerprintMismatch {
            operation_key: request.operation_key.clone(),
        });
    }
    if ticket.meta.workflow_state != TicketWorkflowState::Ready
        || ticket.meta.targets != canonical_ticket_targets(&targets)
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
    if current == to {
        return Err(TicketError::InvalidWorkflowTransition {
            from: current.as_str().to_owned(),
            to: to.as_str().to_owned(),
        });
    }
    Ok(())
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TicketItemEdit {
    pub title: Option<String>,
    /// Whole-body replacement for legacy authoring surfaces. Prefer `body_replacement`
    /// for small edits that should be validated against the current body.
    pub body: Option<MarkdownText>,
    #[serde(default)]
    pub body_replacement: Option<TicketBodyReplacement>,
    pub targets: Option<TicketTargetsEdit>,
    pub author: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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
        if let Some(targets) = &self.targets {
            targets.validate()?;
        }

        Ok(())
    }

    fn has_changes(&self) -> bool {
        self.title.is_some()
            || self.body.is_some()
            || self.body_replacement.is_some()
            || self.targets.is_some()
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TicketDependencyCheck {
    pub ticket: TicketSummary,
    pub blockers: Vec<TicketRelationBlocker>,
    pub queue_guard: TicketQueueGuard,
    pub queue_tickets: Vec<String>,
    pub recommended_action: TicketWorkspaceNextAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TicketQueueOutcome {
    pub requested_ticket: String,
    pub queued_tickets: Vec<String>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TicketRef {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_key: Option<String>,
    pub slug: String,
    pub status: TicketStatus,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NewTicketRelation {
    pub kind: TicketRelationKind,
    pub target: String,
    pub note: Option<String>,
    pub author: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DerivedTicketRelation {
    pub source_ticket: String,
    pub inverse_kind: String,
    pub forward_kind: TicketRelationKind,
    pub note: Option<String>,
    pub author: String,
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TicketRelationBlocker {
    pub blocking_ticket: String,
    pub reason_kind: String,
    pub relation_kind: TicketRelationKind,
    pub note: Option<String>,
    pub blocking_state: TicketWorkflowState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TicketRelationNotice {
    pub related_ticket: String,
    pub kind: TicketRelationKind,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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
    _summary: &TicketSummary,
    relation_blockers: &[TicketRelationBlocker],
    _orchestration_overlay: Option<&TicketWorkspaceStateOverlay>,
) -> TicketQueueGuard {
    TicketQueueGuard {
        can_queue_for_orchestrator: true,
        reason: Some("Queue submits only the explicitly requested Ticket; state and dependencies are diagnostic context".to_owned()),
        blocked_reason: (!relation_blockers.is_empty()).then(|| format_relation_blockers(relation_blockers)),
    }
}

fn derive_ticket_workspace_projection(
    summary: &TicketSummary,
    relation_blockers: &[TicketRelationBlocker],
) -> TicketWorkspaceProjection {
    TicketWorkspaceProjection {
        kind: workspace_row_kind_for_state(summary.workflow_state),
        priority: if matches!(
            summary.workflow_state,
            TicketWorkflowState::Queued | TicketWorkflowState::InProgress
        ) {
            TicketWorkspaceActionPriority::ActiveWork
        } else {
            TicketWorkspaceActionPriority::ReadyForQueue
        },
        next_action: Some(TicketWorkspaceNextAction::QueueForOrchestrator),
        visible_state: summary.workflow_state.to_string(),
        visible_overlay: None,
        disabled_reason: None,
        key_hint: Some(format!("Queue only {} for Orchestrator", summary.id)),
        blocked_reason: (!relation_blockers.is_empty())
            .then(|| format_relation_blockers(relation_blockers)),
        queue_guard: ticket_queue_guard(summary, relation_blockers, None),
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

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NewOrchestrationPlanRecord {
    pub kind: OrchestrationPlanKind,
    pub related_ticket: Option<String>,
    pub note: Option<String>,
    pub accepted_plan: Option<AcceptedOrchestrationPlan>,
    pub author: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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
    #[serde(default)]
    pub targets: Vec<TicketTarget>,
    pub raw: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TicketDocument {
    pub body: MarkdownText,
    pub raw_frontmatter: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TicketArtifactRef {
    /// Path relative to the ticket's `artifacts/` directory.
    pub relative_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Ticket {
    pub meta: TicketMeta,
    pub document: TicketDocument,
    pub events: Vec<TicketEvent>,
    pub artifacts: Vec<TicketArtifactRef>,
    pub relations: TicketRelationView,
    pub resolution: Option<MarkdownText>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub enum TicketDoctorSeverity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TicketDoctorDiagnostic {
    pub severity: TicketDoctorSeverity,
    pub message: String,
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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
    fn create_with_snapshot(&self, input: NewTicket) -> Result<(TicketRef, Ticket)> {
        let reference = self.create(input)?;
        let ticket = self.show(reference.id.clone().into())?;
        Ok((reference, ticket))
    }
    fn edit_item(&self, id: TicketIdOrSlug, edit: TicketItemEdit) -> Result<Ticket>;
    fn edit_item_with_snapshots(
        &self,
        id: TicketIdOrSlug,
        edit: TicketItemEdit,
    ) -> Result<(Ticket, Ticket)> {
        let previous = self.show(id.clone())?;
        let current = self.edit_item(id, edit)?;
        Ok((previous, current))
    }
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
    fn update_state(&self, _ticket: &str, _request: TicketStateUpdate) -> Result<Ticket> {
        Err(TicketError::Conflict(
            "backend does not support audited state updates".to_owned(),
        ))
    }
    fn complete(&self, ticket: &str, request: TicketCompletion) -> Result<Ticket> {
        self.update_state(ticket, request.into())
    }
    fn mark_ready(&self, id: TicketIdOrSlug, request: TicketMarkReady) -> Result<Ticket>;
    fn queue_ready(&self, id: TicketIdOrSlug, queued_by: &str) -> Result<TicketQueueOutcome>;
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
    UpdateState {
        ticket: String,
        request: TicketStateUpdate,
    },
    Complete {
        ticket: String,
        request: TicketCompletion,
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
    QueueOutcome(TicketQueueOutcome),
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
        TicketBackendOperation::UpdateState { ticket, request } => {
            TicketBackendOperationResult::Ticket(backend.update_state(&ticket, request)?)
        }
        TicketBackendOperation::Complete { ticket, request } => {
            TicketBackendOperationResult::Ticket(backend.complete(&ticket, request)?)
        }
        TicketBackendOperation::MarkReady { id, request } => {
            TicketBackendOperationResult::Ticket(backend.mark_ready(id, request)?)
        }
        TicketBackendOperation::QueueReady { id, queued_by } => {
            TicketBackendOperationResult::QueueOutcome(backend.queue_ready(id, &queued_by)?)
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

    /// Opens a standalone Ticket backend at the current canonical schema baseline.
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
        let states = self.state_index(conn)?;
        let relations = self.all_relations(conn)?;
        summaries
            .iter()
            .map(|summary| {
                Ok((
                    summary.id.clone(),
                    transitive_dependency_blockers(&summary.id, &states, &relations)?,
                ))
            })
            .collect()
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

    /// The server supplies these attributes from authenticated runtime context.
    /// The live assignment lookup must stay inside the mutation's write transaction:
    /// an earlier server precheck alone cannot fence a concurrent release.
    fn require_active_source_assignment(&self, conn: &Connection, ticket_id: &str) -> Result<()> {
        if self
            .event_attributes
            .get("source_actor_role")
            .map(String::as_str)
            != Some("ticket_worker")
            && !self.event_attributes.contains_key("source_assignment_id")
        {
            return Ok(());
        }
        let required = |key: &str| {
            self.event_attributes
                .get(key)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    TicketError::Conflict(format!(
                        "Ticket mutation requires authenticated {key} for an active assignment"
                    ))
                })
        };
        let runtime_id = required("source_runtime_id")?;
        let worker_id = required("source_worker_id")?;
        let assignment_id = required("source_assignment_id")?;
        let active = conn.query_row(
            "SELECT 1 FROM ticket_active_worker_assignments WHERE workspace_id = ?1 AND ticket_id = ?2 AND runtime_id = ?3 AND worker_id = ?4 AND assignment_id = ?5 LIMIT 1",
            params![self.workspace_id, ticket_id, runtime_id, worker_id, assignment_id],
            |_| Ok(()),
        ).optional().map_err(sqlite_err)?.is_some();
        if !active {
            return Err(TicketError::Conflict("Ticket mutation source assignment is no longer active for this exact runtime/worker/Ticket identity".to_owned()));
        }
        Ok(())
    }

    fn state_update_fingerprint(
        &self,
        ticket_id: &str,
        request: &TicketStateUpdate,
        body: &MarkdownText,
    ) -> Result<String> {
        let source = [
            "source_runtime_id",
            "source_worker_id",
            "source_actor_role",
            "source_assignment_id",
        ]
        .map(|key| self.event_attributes.get(key));
        let mut digest = Sha256::new();
        digest.update(b"ticket.state-update.v1\0");
        digest.update(
            serde_json::to_vec(&(ticket_id, request, body, source))
                .map_err(|error| TicketError::Conflict(error.to_string()))?,
        );
        Ok(digest
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect())
    }

    fn next_legacy_state_operation_key(
        &self,
        conn: &Connection,
        ticket_id: &str,
    ) -> Result<String> {
        let index: i64 = conn.query_row(
            "SELECT COALESCE(MAX(event_index), -1) + 1 FROM typed_ticket_events WHERE workspace_id = ?1 AND ticket_id = ?2",
            params![self.workspace_id, ticket_id], |row| row.get(0),
        ).map_err(sqlite_err)?;
        Ok(format!("legacy-state:{ticket_id}:{index}"))
    }

    /// All callers supply a snapshot read inside this same write transaction.
    /// Legacy adapters retain their Markdown bodies while sharing CAS, source
    /// authorization, state/status/resolution writes, audit, and operation receipts.
    fn apply_state_update(
        &self,
        conn: &Connection,
        previous: Ticket,
        request: TicketStateUpdate,
        body: MarkdownText,
    ) -> Result<Ticket> {
        validate_required_event_value("operation_key", &request.operation_key)?;
        validate_required_event_value("expected_item_revision", &request.expected_item_revision)?;
        validate_state_change(&TicketStateChange {
            from: request.expected_state.to_string(),
            to: request.state.to_string(),
            author: request.author.clone(),
            reason: request.reason.clone(),
            body: body.clone(),
            references: request.references.clone(),
        })?;
        let ticket_id = &previous.meta.id;
        let fingerprint = self.state_update_fingerprint(ticket_id, &request, &body)?;
        if let Some(event) = previous
            .events
            .iter()
            .find(|event| event.attributes.get("operation_key") == Some(&request.operation_key))
        {
            if event
                .attributes
                .get("fingerprint_version")
                .map(String::as_str)
                != Some("state-update-v1")
                || event.attributes.get("request_fingerprint") != Some(&fingerprint)
            {
                return Err(TicketError::OperationFingerprintMismatch {
                    operation_key: request.operation_key.clone(),
                });
            }
            // The immutable event is the receipt; replay returns current authority
            // without appending an event or reapplying the receipt's old state.
            return Ok(previous);
        }
        // Exact recorded replay is independent of today's live role/assignment.
        // Parent transport authenticates identity before restoring receipt context.
        self.require_active_source_assignment(conn, ticket_id)?;
        let actual = ticket_item_revision(&previous);
        if actual != request.expected_item_revision {
            return Err(TicketError::StaleItemRevision {
                expected: request.expected_item_revision.clone(),
                actual,
            });
        }
        if previous.meta.workflow_state != request.expected_state {
            return Err(TicketError::StaleWorkflowState {
                expected: request.expected_state.to_string(),
                actual: previous.meta.workflow_state.to_string(),
            });
        }
        let at = now_utc();
        let updated = conn.execute(
            "UPDATE typed_tickets SET workflow_state = ?3, workflow_state_explicit = 1, status = CASE WHEN ?3 = 'closed' THEN 'closed' ELSE 'open' END, updated_at = ?4, resolution = CASE WHEN ?3 = 'closed' THEN ?5 ELSE resolution END WHERE workspace_id = ?1 AND ticket_id = ?2 AND workflow_state = ?6",
            params![self.workspace_id, ticket_id, request.state.as_str(), at, body.as_str(), request.expected_state.as_str()],
        ).map_err(sqlite_err)?;
        if updated != 1 {
            return Err(TicketError::Conflict(
                "Ticket changed during state mutation".to_owned(),
            ));
        }
        let kind = if request.state == TicketWorkflowState::Closed {
            TicketEventKind::Close
        } else {
            TicketEventKind::StateChanged
        };
        self.insert_event(
            conn,
            ticket_id,
            &TicketEvent {
                heading: Some(kind.heading()),
                kind,
                author: Some(request.author.clone().unwrap_or_else(default_author)),
                at: Some(at),
                status: Some(
                    if request.state == TicketWorkflowState::Closed {
                        "closed"
                    } else {
                        "open"
                    }
                    .to_owned(),
                ),
                from: Some(request.expected_state.to_string()),
                to: Some(request.state.to_string()),
                reason: Some(request.reason.clone()),
                state_field: Some("state".to_owned()),
                body,
                references: request.references.clone(),
                attributes: BTreeMap::from([
                    ("operation_key".to_owned(), request.operation_key.clone()),
                    ("request_fingerprint".to_owned(), fingerprint),
                    (
                        "fingerprint_version".to_owned(),
                        "state-update-v1".to_owned(),
                    ),
                    (
                        "expected_item_revision".to_owned(),
                        request.expected_item_revision.clone(),
                    ),
                ]),
            },
        )?;
        let result = self.load_ticket(conn, ticket_id)?;
        if !result
            .events
            .iter()
            .any(|event| event.attributes.get("operation_key") == Some(&request.operation_key))
        {
            return Err(TicketError::Conflict(
                "state update audit lost its operation key".to_owned(),
            ));
        }
        Ok(result)
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

    fn insert_ticket(&self, conn: &Connection, ticket: &Ticket) -> Result<()> {
        conn.execute(r#"INSERT INTO typed_tickets
            (workspace_id, ticket_id, slug, title, status, kind, priority, body, created_at, updated_at, assignee, readiness, workflow_state, workflow_state_explicit, queued_by, queued_at, resolution)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)"#,
            params![self.workspace_id, ticket.meta.id, ticket.meta.slug, ticket.meta.title, ticket.meta.status.as_str(), ticket.meta.kind, ticket.meta.priority, ticket.document.body.as_str(), ticket.meta.created_at, ticket.meta.updated_at, ticket.meta.assignee, ticket.meta.readiness, ticket.meta.workflow_state.as_str(), if ticket.meta.workflow_state_explicit { 1 } else { 0 }, ticket.meta.queued_by, ticket.meta.queued_at, ticket.resolution.as_ref().map(|body| body.as_str())]
        ).map_err(sqlite_err)?;
        self.replace_ticket_targets(conn, &ticket.meta.id, &ticket.meta.targets)?;
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

    fn replace_ticket_targets(
        &self,
        conn: &Connection,
        ticket_id: &str,
        targets: &[TicketTarget],
    ) -> Result<()> {
        validate_ticket_targets(targets)?;
        conn.execute(
            "DELETE FROM typed_ticket_targets WHERE workspace_id = ?1 AND ticket_id = ?2",
            params![self.workspace_id, ticket_id],
        )
        .map_err(sqlite_err)?;
        for (ordinal, target) in targets.iter().enumerate() {
            conn.execute(
                "INSERT INTO typed_ticket_targets
                 (workspace_id, ticket_id, ordinal, repository_key, ref_selector, access)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    self.workspace_id,
                    ticket_id,
                    ordinal as i64,
                    target.repository_key,
                    target.ref_selector,
                    target.access.as_str(),
                ],
            )
            .map_err(sqlite_err)?;
        }
        Ok(())
    }

    fn load_ticket_targets(&self, conn: &Connection, ticket_id: &str) -> Result<Vec<TicketTarget>> {
        let mut statement = conn
            .prepare(
                "SELECT repository_key, ref_selector, access
                 FROM typed_ticket_targets
                 WHERE workspace_id = ?1 AND ticket_id = ?2
                 ORDER BY ordinal ASC",
            )
            .map_err(sqlite_err)?;
        let rows = statement
            .query_map(params![self.workspace_id, ticket_id], |row| {
                let access = row.get::<_, String>(2)?;
                let access = TicketTargetAccess::parse(&access).ok_or_else(|| {
                    rusqlite::Error::FromSqlConversionFailure(
                        2,
                        rusqlite::types::Type::Text,
                        format!("invalid Ticket target access {access:?}").into(),
                    )
                })?;
                Ok(TicketTarget {
                    repository_key: row.get(0)?,
                    ref_selector: row.get(1)?,
                    access,
                })
            })
            .map_err(sqlite_err)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(sqlite_err)
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
            targets: Vec::new(),
            raw: BTreeMap::new(),
        })
    }

    fn load_ticket(&self, conn: &Connection, ticket_id: &str) -> Result<Ticket> {
        #[cfg(test)]
        self.full_ticket_load_count.fetch_add(1, Ordering::SeqCst);
        let (mut meta, body, resolution): (TicketMeta, String, Option<String>) = conn.query_row(r#"SELECT ticket_id, slug, title, status, kind, priority, created_at, updated_at, assignee, readiness, body, resolution, workflow_state, workflow_state_explicit, queued_by, queued_at FROM typed_tickets WHERE workspace_id = ?1 AND ticket_id = ?2"#,
            params![self.workspace_id, ticket_id], |row| Ok((Self::ticket_meta_from_row(row)?, row.get(10)?, row.get(11)?))).optional().map_err(sqlite_err)?.ok_or_else(|| TicketError::NotFound(ticket_id.to_string()))?;
        meta.resource_key = Self::resource_key_for(conn, &self.workspace_id, ticket_id)?;
        meta.targets = self.load_ticket_targets(conn, ticket_id)?;
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
        let mut stmt = conn.prepare(r#"SELECT ticket_id, slug, title, status, kind, priority, created_at, updated_at, assignee, readiness, body, resolution, workflow_state, workflow_state_explicit, queued_by, queued_at FROM typed_tickets WHERE workspace_id = ?1 ORDER BY ticket_id ASC"#).map_err(sqlite_err)?;
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
        self.create_with_snapshot(input)
            .map(|(reference, _)| reference)
    }

    fn create_with_snapshot(&self, input: NewTicket) -> Result<(TicketRef, Ticket)> {
        self.with_write(|conn| {
            if input.title.trim().is_empty() {
                return Err(TicketError::Conflict(
                    "ticket title must not be empty".to_string(),
                ));
            }
            validate_ticket_targets(&input.targets)?;
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
                targets: input.targets,
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
            let reference = TicketRef {
                id: id.clone(),
                resource_key: Some(resource_key),
                slug: id.clone(),
                status: TicketStatus::Open,
            };
            let persisted = self.load_ticket(conn, &id)?;
            Ok((reference, persisted))
        })
    }

    fn edit_item(&self, id: TicketIdOrSlug, edit: TicketItemEdit) -> Result<Ticket> {
        self.edit_item_with_snapshots(id, edit)
            .map(|(_, current)| current)
    }

    fn edit_item_with_snapshots(
        &self,
        id: TicketIdOrSlug,
        edit: TicketItemEdit,
    ) -> Result<(Ticket, Ticket)> {
        self.with_write(|conn| {
            edit.validate_body_edit_request()?;
            if !edit.has_changes() {
                return Err(TicketError::Conflict(
                    "TicketEditItem requires at least one of title, body, body_replacement, or targets"
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
            let previous = self.load_ticket(conn, &ticket_id)?;
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
            if let Some(targets) = edit.targets.as_ref() {
                let targets = match targets {
                    TicketTargetsEdit::Set { targets } => targets.as_slice(),
                    TicketTargetsEdit::Clear => &[],
                };
                self.replace_ticket_targets(conn, &ticket_id, targets)?;
                self.touch_ticket(conn, &ticket_id, &now)?;
            }

            let mut changes = Vec::new();
            if edit.title.is_some() {
                changes.push("title");
            }
            if !matches!(body_edit_audit, TicketBodyEditAudit::None) {
                changes.push("body");
            }
            if edit.targets.is_some() {
                changes.push("targets");
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
            let current = self.load_ticket(conn, &ticket_id)?;
            Ok((previous, current))
        })
    }

    fn dependency_check(&self, id: TicketIdOrSlug) -> Result<TicketDependencyCheck> {
        self.with_read(|conn| {
            let ticket_id = self.resolve_ticket_id(conn, id)?;
            let ticket = self.load_ticket(conn, &ticket_id)?;
            let states = self.state_index(conn)?;
            let relations = self.all_relations(conn)?;
            let blockers = transitive_dependency_blockers(&ticket_id, &states, &relations)?;
            let summary = ticket_summary_from_meta(ticket.meta);
            let mut projection = project_ticket_workspace_item(&summary, &blockers, None);
            projection.queue_guard = ticket_queue_guard(&summary, &blockers, None);
            let queue_tickets = vec![ticket_id];
            Ok(TicketDependencyCheck {
                ticket: summary,
                blockers,
                queue_guard: projection.queue_guard,
                queue_tickets,
                recommended_action: projection
                    .next_action
                    .unwrap_or(TicketWorkspaceNextAction::WaitForOrchestrator),
            })
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
        field: &str,
        change: TicketStateChange,
    ) -> Result<()> {
        if field != "state" {
            return Err(TicketError::Conflict(format!(
                "unsupported Ticket state field: {field}"
            )));
        }
        self.set_workflow_state(id, change)
    }

    fn set_workflow_state(&self, id: TicketIdOrSlug, change: TicketStateChange) -> Result<()> {
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
            let previous = self.load_ticket(conn, &ticket_id)?;
            let request = TicketStateUpdate {
                expected_item_revision: ticket_item_revision(&previous),
                expected_state: from,
                state: to,
                operation_key: self.next_legacy_state_operation_key(conn, &ticket_id)?,
                reason: change.reason,
                references: change.references,
                author: Some(change.author.unwrap_or_else(default_author)),
            };
            self.apply_state_update(conn, previous, request, change.body)
                .map(|_| ())
        })
    }

    fn update_state(&self, ticket: &str, request: TicketStateUpdate) -> Result<Ticket> {
        self.with_write(|conn| {
            let ticket_id = self.resolve_ticket_id(conn, ticket.into())?;
            let previous = self.load_ticket(conn, &ticket_id)?;
            let body = MarkdownText::new(&request.reason);
            self.apply_state_update(conn, previous, request, body)
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
            self.require_active_source_assignment(conn, &ticket_id)?;
            if ticket.meta.workflow_state != TicketWorkflowState::Planning {
                return Err(TicketError::StaleWorkflowState {
                    expected: TicketWorkflowState::Planning.as_str().to_owned(),
                    actual: ticket.meta.workflow_state.as_str().to_owned(),
                });
            }
            let targets = resolve_ready_targets(
                self.target_authority.as_ref(),
                &self.workspace_id,
                &ticket.meta.targets,
            )?;
            let fingerprint = mark_ready_fingerprint(&ticket, &request, &targets);
            let encoded_targets = encoded_resolved_targets(&targets)?;
            let target_summary = targets
                .iter()
                .map(|target| {
                    format!(
                        "{}@{} ({})",
                        target.repository_key,
                        target.ref_selector.as_deref().unwrap_or("(unspecified)"),
                        target.access.as_str()
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
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
                        "Implementation targets [{target_summary}] were validated and the Ticket was marked ready."
                    )),
                    references: Vec::new(),
                    attributes: BTreeMap::from([
                        ("operation_key".to_owned(), request.operation_key),
                        ("request_fingerprint".to_owned(), fingerprint),
                        ("fingerprint_version".to_owned(), "2".to_owned()),
                        ("targets".to_owned(), encoded_targets),
                    ]),
                },
            )?;
            let updated = conn.execute(
                "UPDATE typed_tickets SET workflow_state = 'ready', workflow_state_explicit = 1, updated_at = ?3 WHERE workspace_id = ?1 AND ticket_id = ?2 AND workflow_state = 'planning'",
                params![self.workspace_id, ticket_id, at],
            )
            .map_err(sqlite_err)?;
            if updated != 1 {
                return Err(TicketError::Conflict(
                    "Ticket changed while ready targets were being persisted".to_owned(),
                ));
            }
            self.replace_ticket_targets(
                conn,
                &ticket_id,
                &canonical_ticket_targets(&targets),
            )?;
            self.load_ticket(conn, &ticket_id)
        })
    }

    fn queue_ready(&self, id: TicketIdOrSlug, queued_by: &str) -> Result<TicketQueueOutcome> {
        validate_required_event_value("queued_by", queued_by)?;
        self.with_write(|conn| {
            let requested_ticket = self.resolve_ticket_id(conn, id)?;
            self.require_active_source_assignment(conn, &requested_ticket)?;
            let previous = self.load_ticket(conn, &requested_ticket)?;
            let mut attributes = BTreeMap::from([
                ("queue_root_ticket".to_owned(), requested_ticket.clone()),
                ("queued_by".to_owned(), queued_by.to_owned()),
            ]);
            if let Some(json) = self.event_attributes.get("queue_orchestrator_assignments") {
                let assignments = serde_json::from_str::<BTreeMap<String, String>>(json)
                    .map_err(|error| TicketError::Conflict(format!("invalid Queue assignment fence: {error}")))?;
                if assignments.len() != 1 || assignments.get(&requested_ticket).is_none_or(|value| value.trim().is_empty()) {
                    return Err(TicketError::Conflict("Queue assignment must name only the explicit Ticket".to_owned()));
                }
                attributes.insert("orchestrator_assignment_id".to_owned(), assignments[&requested_ticket].clone());
            }
            let at = now_utc();
            attributes.insert("queued_at".to_owned(), at.clone());
            conn.execute(
                "UPDATE typed_tickets SET workflow_state = 'queued', status = 'open', workflow_state_explicit = 1, queued_by = ?3, queued_at = ?4, updated_at = ?4 WHERE workspace_id = ?1 AND ticket_id = ?2",
                params![self.workspace_id, requested_ticket, queued_by, at],
            ).map_err(sqlite_err)?;
            self.insert_event(conn, &requested_ticket, &TicketEvent {
                kind: TicketEventKind::StateChanged,
                author: Some(queued_by.to_owned()),
                at: Some(at),
                status: Some("open".to_owned()),
                from: Some(previous.meta.workflow_state.to_string()),
                to: Some("queued".to_owned()),
                reason: Some("queued".to_owned()),
                state_field: Some("state".to_owned()),
                heading: Some(TicketEventKind::StateChanged.heading()),
                body: MarkdownText::new(format!("Queued for Orchestrator by {queued_by}.")),
                references: Vec::new(),
                attributes,
            })?;
            Ok(TicketQueueOutcome {
                queued_tickets: vec![requested_ticket.clone()],
                requested_ticket,
            })
        })
    }

    fn close(&self, id: TicketIdOrSlug, resolution: MarkdownText) -> Result<()> {
        if resolution.as_str().trim().is_empty() {
            return Err(TicketError::Conflict(
                "Ticket close requires a non-empty resolution".to_owned(),
            ));
        }
        self.with_write(|conn| {
            let ticket_id = self.resolve_ticket_id(conn, id)?;
            let previous = self.load_ticket(conn, &ticket_id)?;
            let request = TicketStateUpdate {
                expected_item_revision: ticket_item_revision(&previous),
                expected_state: previous.meta.workflow_state,
                state: TicketWorkflowState::Closed,
                operation_key: self.next_legacy_state_operation_key(conn, &ticket_id)?,
                reason: "closed".to_owned(),
                references: Vec::new(),
                author: Some(default_author()),
            };
            self.apply_state_update(conn, previous, request, resolution)
                .map(|_| ())
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

fn transitive_dependency_blockers(
    requested_ticket: &str,
    states: &HashMap<String, TicketWorkflowState>,
    relations: &[TicketRelation],
) -> Result<Vec<TicketRelationBlocker>> {
    type DependencyEdge = (String, String, TicketRelationKind, Option<String>);
    let mut prerequisites = BTreeMap::<String, Vec<DependencyEdge>>::new();
    for relation in relations {
        let edge = match relation.kind {
            TicketRelationKind::DependsOn => Some((
                relation.ticket_id.clone(),
                (
                    relation.target.clone(),
                    "depends_on".to_string(),
                    relation.kind,
                    relation.note.clone(),
                ),
            )),
            TicketRelationKind::Blocks => Some((
                relation.target.clone(),
                (
                    relation.ticket_id.clone(),
                    "blocked_by".to_string(),
                    relation.kind,
                    relation.note.clone(),
                ),
            )),
            TicketRelationKind::Related
            | TicketRelationKind::Supersedes
            | TicketRelationKind::DuplicateOf => None,
        };
        if let Some((ticket, edge)) = edge {
            prerequisites.entry(ticket).or_default().push(edge);
        }
    }
    for dependencies in prerequisites.values_mut() {
        dependencies.sort_by(|left, right| left.0.cmp(&right.0));
    }

    fn collect(
        ticket: &str,
        requested_ticket: &str,
        states: &HashMap<String, TicketWorkflowState>,
        prerequisites: &BTreeMap<String, Vec<DependencyEdge>>,
        marks: &mut BTreeMap<String, u8>,
        blockers: &mut BTreeMap<String, TicketRelationBlocker>,
    ) -> Result<()> {
        let state = states
            .get(ticket)
            .copied()
            .ok_or_else(|| TicketError::NotFound(ticket.to_owned()))?;
        if ticket_state_resolved(state) {
            return Ok(());
        }
        match marks.get(ticket).copied() {
            Some(2) => return Ok(()),
            Some(1) => {
                let requested_state = states
                    .get(requested_ticket)
                    .copied()
                    .ok_or_else(|| TicketError::NotFound(requested_ticket.to_owned()))?;
                blockers.insert(
                    requested_ticket.to_owned(),
                    TicketRelationBlocker {
                        blocking_ticket: requested_ticket.to_owned(),
                        reason_kind: "dependency_cycle".to_string(),
                        relation_kind: TicketRelationKind::DependsOn,
                        note: Some(format!(
                            "dependency cycle reachable through Ticket {ticket}"
                        )),
                        blocking_state: requested_state,
                    },
                );
                return Ok(());
            }
            _ => {}
        }
        marks.insert(ticket.to_owned(), 1);
        if let Some(dependencies) = prerequisites.get(ticket) {
            for (dependency, reason_kind, relation_kind, note) in dependencies {
                let dependency_state = states
                    .get(dependency)
                    .copied()
                    .ok_or_else(|| TicketError::NotFound(dependency.clone()))?;
                if !ticket_state_resolved(dependency_state) {
                    blockers
                        .entry(dependency.clone())
                        .or_insert_with(|| TicketRelationBlocker {
                            blocking_ticket: dependency.clone(),
                            reason_kind: reason_kind.clone(),
                            relation_kind: *relation_kind,
                            note: note.clone(),
                            blocking_state: dependency_state,
                        });
                    collect(
                        dependency,
                        requested_ticket,
                        states,
                        prerequisites,
                        marks,
                        blockers,
                    )?;
                }
            }
        }
        marks.insert(ticket.to_owned(), 2);
        Ok(())
    }

    let mut blockers = BTreeMap::new();
    collect(
        requested_ticket,
        requested_ticket,
        states,
        &prerequisites,
        &mut BTreeMap::new(),
        &mut blockers,
    )?;
    Ok(blockers.into_values().collect())
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
    // Reasons are persisted as SQLite text, not legacy thread-comment attributes.
    // Preserve free-form Markdown without per-field length or single-line limits.
    if change.reason.trim().is_empty() {
        return Err(TicketError::Conflict(
            "state_changed event requires non-empty reason".to_owned(),
        ));
    }
    if let Some(author) = change.author.as_deref() {
        validate_required_event_value("author", author)?;
    }
    for reference in &change.references {
        validate_required_event_value("reference_kind", &reference.kind)?;
        validate_required_event_value("reference_target", &reference.target)?;
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

fn now_utc() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn default_author() -> String {
    std::env::var("USER").unwrap_or_else(|_| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn production_source_has_no_repository_local_ticket_provider() {
        let production = include_str!("lib.rs")
            .split_once("#[cfg(test)]\nmod tests")
            .map(|(production, _)| production)
            .expect("Ticket test module marker");
        for forbidden in [
            "LocalTicketBackend",
            "import_from_local_backend",
            ".yoi/tickets",
            "TicketConfig::load_workspace",
            "ticket.config.toml",
            "workspace.toml",
        ] {
            assert!(
                !production.contains(forbidden),
                "repository-local Ticket provider returned through {forbidden}"
            );
        }
    }

    #[derive(Debug)]
    struct TestTargetAuthority;

    impl TicketTargetAuthority for TestTargetAuthority {
        fn resolve_target(
            &self,
            _workspace_id: &str,
            repository_key: &str,
            ref_selector: Option<&str>,
        ) -> Result<ResolvedTicketTarget> {
            if repository_key == "unknown" {
                return Err(TicketError::UnknownTargetRepository(
                    repository_key.to_owned(),
                ));
            }
            Ok(ResolvedTicketTarget {
                repository_key: repository_key.to_owned(),
                ref_selector: Some(ref_selector.unwrap_or("develop").to_owned()),
                access: TicketTargetAccess::ReadOnly,
            })
        }
    }

    fn ticket_target(
        repository_key: &str,
        ref_selector: Option<&str>,
        access: TicketTargetAccess,
    ) -> TicketTarget {
        TicketTarget {
            repository_key: repository_key.to_owned(),
            ref_selector: ref_selector.map(str::to_owned),
            access,
        }
    }

    fn write_target(repository_key: &str) -> TicketTarget {
        ticket_target(repository_key, None, TicketTargetAccess::ReadWrite)
    }

    fn backend(dir: &TempDir) -> SqliteTicketBackend {
        SqliteTicketBackend::open(dir.path().join("tickets.db"), "workspace-test")
            .unwrap()
            .with_target_authority(Arc::new(TestTargetAuthority))
    }

    fn assert_ticket_target_edit_semantics<B: TicketBackend>(backend: &B) {
        let mut input = NewTicket::new("Target Ticket");
        input.targets = vec![ticket_target(
            "main",
            Some("feature/api"),
            TicketTargetAccess::ReadWrite,
        )];
        let created = backend.create(input).unwrap();
        let ticket = backend
            .show(TicketIdOrSlug::Id(created.id.clone()))
            .unwrap();
        assert_eq!(
            ticket.meta.targets,
            vec![ticket_target(
                "main",
                Some("feature/api"),
                TicketTargetAccess::ReadWrite,
            )]
        );

        let edited = backend
            .edit_item(
                TicketIdOrSlug::Id(created.id.clone()),
                TicketItemEdit {
                    targets: Some(TicketTargetsEdit::Set {
                        targets: vec![
                            write_target("secondary"),
                            ticket_target("docs", Some("main"), TicketTargetAccess::ReadOnly),
                        ],
                    }),
                    author: Some("tester".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            edited.meta.targets,
            vec![
                write_target("secondary"),
                ticket_target("docs", Some("main"), TicketTargetAccess::ReadOnly),
            ]
        );
        let edit_event = edited
            .events
            .iter()
            .rev()
            .find(|event| event.kind == TicketEventKind::Other("item_edit".to_string()))
            .expect("item_edit event");
        assert_eq!(
            edit_event.attributes.get("changes"),
            Some(&"targets".to_string())
        );

        let cleared = backend
            .edit_item(
                TicketIdOrSlug::Id(created.id),
                TicketItemEdit {
                    targets: Some(TicketTargetsEdit::Clear),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(cleared.meta.targets.is_empty());
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

    fn dependency_relation(ticket_id: &str, target: &str) -> TicketRelation {
        TicketRelation {
            ticket_id: ticket_id.to_owned(),
            kind: TicketRelationKind::DependsOn,
            target: target.to_owned(),
            note: None,
            author: "test".to_owned(),
            at: String::new(),
        }
    }

    #[test]
    fn transitive_blockers_project_internal_dependency_cycle_as_blocking() {
        let states = HashMap::from([
            ("root".to_owned(), TicketWorkflowState::Ready),
            ("branch".to_owned(), TicketWorkflowState::Ready),
            ("cycle".to_owned(), TicketWorkflowState::Ready),
        ]);
        let relations = [
            dependency_relation("root", "branch"),
            dependency_relation("branch", "cycle"),
            dependency_relation("cycle", "branch"),
        ];

        let blockers = transitive_dependency_blockers("root", &states, &relations).unwrap();
        assert!(blockers.iter().any(|blocker| {
            blocker.blocking_ticket == "root" && blocker.reason_kind == "dependency_cycle"
        }));
        let summary = summary_with_state(TicketWorkflowState::Ready);
        let mut summary = summary;
        summary.id = "root".to_string();
        assert!(
            project_ticket_workspace_item(&summary, &blockers, None)
                .queue_guard
                .can_queue_for_orchestrator
        );
        assert!(
            ticket_queue_guard(&summary, &blockers, None)
                .blocked_reason
                .is_some()
        );
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
    fn workspace_projection_queues_ready_ticket_with_planning_dependency() {
        let summary = summary_with_state(TicketWorkflowState::Ready);
        let blockers = [blocker_with_state(TicketWorkflowState::Planning)];
        let projection = project_ticket_workspace_item(&summary, &blockers, None);

        assert_eq!(projection.kind, TicketWorkspaceRowKind::Ticket);
        assert_eq!(
            projection.next_action,
            Some(TicketWorkspaceNextAction::QueueForOrchestrator)
        );
        assert!(projection.queue_guard.can_queue_for_orchestrator);
        assert!(projection.blocked_reason.is_some());
        assert!(projection.disabled_reason.is_none());
    }

    #[test]
    fn workspace_projection_queues_ready_ticket_with_queued_dependency_context() {
        let summary = summary_with_state(TicketWorkflowState::Ready);
        let blockers = [blocker_with_state(TicketWorkflowState::Queued)];
        let projection = project_ticket_workspace_item(&summary, &blockers, None);

        assert_eq!(
            projection.next_action,
            Some(TicketWorkspaceNextAction::QueueForOrchestrator)
        );
        assert!(projection.queue_guard.can_queue_for_orchestrator);
        assert!(projection.blocked_reason.is_some());
        assert!(
            projection
                .key_hint
                .as_deref()
                .unwrap_or_default()
                .contains("Queue only")
        );
    }

    #[test]
    fn workspace_projection_overlay_retains_context_without_queue_gate() {
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
        assert!(projection.queue_guard.can_queue_for_orchestrator);
        assert!(projection.visible_overlay.is_some());
    }

    #[test]
    fn generic_state_change_allows_direct_start_without_phase_path() {
        assert!(
            validate_generic_state_change(
                TicketWorkflowState::Ready,
                TicketWorkflowState::InProgress
            )
            .is_ok()
        );
        assert!(
            validate_generic_state_change(
                TicketWorkflowState::Planning,
                TicketWorkflowState::InProgress
            )
            .is_ok()
        );
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
        assert!(TicketWorkflowState::is_role_transition(
            TicketWorkflowState::Planning,
            TicketWorkflowState::Queued
        ));
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
    fn sqlite_dependency_check_reports_transitive_planning_dependency() {
        let temp = TempDir::new().unwrap();
        let backend = SqliteTicketBackend::open(temp.path().join("tickets.db"), "workspace-test")
            .unwrap()
            .with_target_authority(Arc::new(TestTargetAuthority));
        let mut root_input = NewTicket::new("Ready root");
        root_input.workflow_state = Some(TicketWorkflowState::Ready);
        root_input.targets = vec![write_target("main")];
        let root = backend.create(root_input).unwrap();
        let mut middle_input = NewTicket::new("Queued middle");
        middle_input.workflow_state = Some(TicketWorkflowState::Queued);
        middle_input.targets = vec![write_target("main")];
        let middle = backend.create(middle_input).unwrap();
        let leaf = backend.create(NewTicket::new("Planning leaf")).unwrap();
        for (ticket, target) in [
            (root.id.clone(), middle.id.clone()),
            (middle.id.clone(), leaf.id.clone()),
        ] {
            backend
                .add_ticket_relation(
                    TicketIdOrSlug::Id(ticket),
                    NewTicketRelation {
                        kind: TicketRelationKind::DependsOn,
                        target,
                        note: None,
                        author: None,
                    },
                )
                .unwrap();
        }

        let check = backend
            .dependency_check(TicketIdOrSlug::Id(root.id))
            .unwrap();
        assert!(check.queue_guard.can_queue_for_orchestrator);
        assert_eq!(check.queue_tickets, vec![check.ticket.id.clone()]);
        assert!(check.blockers.iter().any(|blocker| {
            blocker.blocking_ticket == leaf.id
                && blocker.blocking_state == TicketWorkflowState::Planning
        }));

        let page = backend
            .list_workspace_projection_page(SqliteTicketListPageQuery {
                states: vec![TicketWorkflowState::Ready],
                limit: 10,
                after: None,
            })
            .unwrap();
        let item = page
            .items
            .iter()
            .find(|item| item.summary.id == check.ticket.id)
            .unwrap();
        assert!(item.relation_blockers.iter().any(|blocker| {
            blocker.blocking_ticket == leaf.id
                && blocker.blocking_state == TicketWorkflowState::Planning
        }));
        assert!(
            project_ticket_workspace_item(&item.summary, &item.relation_blockers, None)
                .queue_guard
                .can_queue_for_orchestrator
        );
    }

    #[test]
    fn queue_does_not_require_target_authority() {
        let tmp = TempDir::new().unwrap();
        let backend =
            SqliteTicketBackend::open(tmp.path().join("tickets.db"), "workspace-test").unwrap();
        let ticket = backend.create(NewTicket::new("No Git or targets")).unwrap();
        let check = backend.dependency_check(ticket.id.clone().into()).unwrap();
        assert!(check.queue_guard.can_queue_for_orchestrator);
        assert_eq!(check.queue_tickets, vec![ticket.id.clone()]);
        assert_eq!(
            backend
                .queue_ready(ticket.id.clone().into(), "actor")
                .unwrap()
                .queued_tickets,
            vec![ticket.id]
        );
    }

    #[test]
    fn sqlite_queue_ignores_cycle_behind_done_dependency() {
        let temp = TempDir::new().unwrap();
        let backend = SqliteTicketBackend::open(temp.path().join("tickets.db"), "workspace-test")
            .unwrap()
            .with_target_authority(Arc::new(TestTargetAuthority));
        let mut root_input = NewTicket::new("Ready root");
        root_input.workflow_state = Some(TicketWorkflowState::Ready);
        root_input.targets = vec![write_target("main")];
        let root = backend.create(root_input).unwrap();
        let mut done_input = NewTicket::new("Done dependency");
        done_input.workflow_state = Some(TicketWorkflowState::Done);
        done_input.targets = vec![write_target("main")];
        let done = backend.create(done_input).unwrap();
        for (ticket_id, target) in [
            (root.id.clone(), done.id.clone()),
            (done.id.clone(), root.id.clone()),
        ] {
            backend
                .add_ticket_relation(
                    TicketIdOrSlug::Id(ticket_id),
                    NewTicketRelation {
                        kind: TicketRelationKind::DependsOn,
                        target,
                        note: None,
                        author: None,
                    },
                )
                .unwrap();
        }

        let outcome = backend
            .queue_ready(TicketIdOrSlug::Id(root.id.clone()), "orchestrator")
            .unwrap();
        assert_eq!(outcome.queued_tickets, vec![root.id.clone()]);
        assert_eq!(
            backend
                .show(TicketIdOrSlug::Id(root.id))
                .unwrap()
                .meta
                .workflow_state,
            TicketWorkflowState::Queued
        );
        assert_eq!(
            backend
                .show(TicketIdOrSlug::Id(done.id))
                .unwrap()
                .meta
                .workflow_state,
            TicketWorkflowState::Done
        );
    }

    #[test]
    fn sqlite_queue_cycle_diagnostic_queues_only_explicit_ticket() {
        let temp = TempDir::new().unwrap();
        let backend = SqliteTicketBackend::open(temp.path().join("tickets.db"), "workspace-test")
            .unwrap()
            .with_target_authority(Arc::new(TestTargetAuthority));
        let mut first_input = NewTicket::new("First ready Ticket");
        first_input.workflow_state = Some(TicketWorkflowState::Ready);
        first_input.targets = vec![write_target("main")];
        let first = backend.create(first_input).unwrap();
        let mut second_input = NewTicket::new("Second ready Ticket");
        second_input.workflow_state = Some(TicketWorkflowState::Ready);
        second_input.targets = vec![write_target("main")];
        let second = backend.create(second_input).unwrap();
        let mut third_input = NewTicket::new("Third ready Ticket");
        third_input.workflow_state = Some(TicketWorkflowState::Ready);
        third_input.targets = vec![write_target("main")];
        let third = backend.create(third_input).unwrap();
        for (ticket, target) in [
            (first.id.clone(), second.id.clone()),
            (second.id.clone(), third.id.clone()),
            (third.id.clone(), second.id.clone()),
        ] {
            backend
                .add_ticket_relation(
                    TicketIdOrSlug::Id(ticket),
                    NewTicketRelation {
                        kind: TicketRelationKind::DependsOn,
                        target,
                        note: None,
                        author: None,
                    },
                )
                .unwrap();
        }

        let check = backend
            .dependency_check(TicketIdOrSlug::Id(first.id.clone()))
            .unwrap();
        assert!(check.queue_guard.can_queue_for_orchestrator);
        assert!(
            check
                .queue_guard
                .blocked_reason
                .as_deref()
                .unwrap_or_default()
                .contains("cycle")
        );
        let page = backend
            .list_workspace_projection_page(SqliteTicketListPageQuery {
                states: vec![TicketWorkflowState::Ready],
                limit: 10,
                after: None,
            })
            .unwrap();
        let item = page
            .items
            .iter()
            .find(|item| item.summary.id == check.ticket.id)
            .unwrap();
        assert!(
            project_ticket_workspace_item(&item.summary, &item.relation_blockers, None)
                .queue_guard
                .can_queue_for_orchestrator
        );
        let outcome = backend
            .queue_ready(first.id.clone().into(), "orchestrator")
            .unwrap();
        assert_eq!(outcome.queued_tickets, vec![first.id.clone()]);
        assert_eq!(
            backend.show(first.id.into()).unwrap().meta.workflow_state,
            TicketWorkflowState::Queued
        );
        for ticket_id in [second.id, third.id] {
            assert_eq!(
                backend
                    .show(TicketIdOrSlug::Id(ticket_id))
                    .unwrap()
                    .meta
                    .workflow_state,
                TicketWorkflowState::Ready
            );
        }
    }

    #[test]
    fn sqlite_queue_ignores_dependency_targets() {
        let temp = TempDir::new().unwrap();
        let backend = SqliteTicketBackend::open(temp.path().join("tickets.db"), "workspace-test")
            .unwrap()
            .with_target_authority(Arc::new(TestTargetAuthority));
        let mut dependency_input = NewTicket::new("Invalid ready dependency");
        dependency_input.workflow_state = Some(TicketWorkflowState::Ready);
        dependency_input.targets = vec![write_target("unknown")];
        let dependency = backend.create(dependency_input).unwrap();
        let mut root_input = NewTicket::new("Queue root");
        root_input.workflow_state = Some(TicketWorkflowState::Ready);
        root_input.targets = vec![write_target("main")];
        let root = backend.create(root_input).unwrap();
        backend
            .add_ticket_relation(
                TicketIdOrSlug::Id(root.id.clone()),
                NewTicketRelation {
                    kind: TicketRelationKind::DependsOn,
                    target: dependency.id.clone(),
                    note: None,
                    author: None,
                },
            )
            .unwrap();

        let check = backend
            .dependency_check(TicketIdOrSlug::Id(root.id.clone()))
            .unwrap();
        assert!(check.queue_guard.can_queue_for_orchestrator);
        let outcome = backend
            .queue_ready(root.id.clone().into(), "orchestrator")
            .unwrap();
        assert_eq!(outcome.queued_tickets, vec![root.id.clone()]);
        assert_eq!(
            backend.show(root.id.into()).unwrap().meta.workflow_state,
            TicketWorkflowState::Queued
        );
        for ticket_id in [dependency.id] {
            assert_eq!(
                backend
                    .show(TicketIdOrSlug::Id(ticket_id))
                    .unwrap()
                    .meta
                    .workflow_state,
                TicketWorkflowState::Ready
            );
        }
    }

    #[test]
    fn sqlite_queue_queues_only_explicit_ticket_not_ready_dependency() {
        let tmp = TempDir::new().unwrap();
        let backend = SqliteTicketBackend::open(tmp.path().join("workspace.db"), "workspace-test")
            .unwrap()
            .with_target_authority(Arc::new(TestTargetAuthority));
        let mut dependency = NewTicket::new("Dependency");
        dependency.targets = vec![write_target("main")];
        dependency.workflow_state = Some(TicketWorkflowState::Ready);
        let dependency = backend.create(dependency).unwrap();
        let mut implementation = NewTicket::new("Implementation");
        implementation.targets = vec![write_target("main")];
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
        assert_eq!(
            ready.meta.targets,
            vec![ticket_target(
                "main",
                Some("develop"),
                TicketTargetAccess::ReadWrite,
            )]
        );
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
        let outcome = backend
            .queue_ready(
                TicketIdOrSlug::Id(implementation.id.clone()),
                "orchestrator",
            )
            .unwrap();
        assert_eq!(outcome.requested_ticket, implementation.id);
        assert_eq!(outcome.queued_tickets, vec![implementation.id.clone()]);
        let queued = backend
            .show(TicketIdOrSlug::Id(implementation.id.clone()))
            .unwrap();
        let queued_dependency = backend
            .show(TicketIdOrSlug::Id(dependency.id.clone()))
            .unwrap();
        assert_eq!(queued.meta.workflow_state, TicketWorkflowState::Queued);
        assert_eq!(
            queued_dependency.meta.workflow_state,
            TicketWorkflowState::Ready
        );
        assert_eq!(queued.meta.queued_by.as_deref(), Some("orchestrator"));
        assert_eq!(queued.relations.blockers.len(), 1);
        assert_eq!(queued.relations.blockers[0].blocking_ticket, dependency.id);
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
    fn sqlite_backend_removes_ticket_relations() {
        let tmp = TempDir::new().unwrap();
        let backend =
            SqliteTicketBackend::open(tmp.path().join("workspace.db"), "workspace-test").unwrap();
        assert_ticket_relation_removal_semantics(&backend);
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
            .find("    fn connect(&self)")
            .map(|offset| start + offset)
            .expect("following SQLite method");
        let projection_source = &source[start..end];
        assert_eq!(projection_source.matches("self.with_read(").count(), 2);
        assert_eq!(projection_source.matches(".prepare(").count(), 2);
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
    fn create_uses_configured_japanese_record_language_for_generated_defaults() {
        let tmp = TempDir::new().unwrap();
        let backend = SqliteTicketBackend::open(tmp.path().join("tickets.db"), "workspace-test")
            .unwrap()
            .with_record_language(Some("Japanese"));

        let created = backend.create(NewTicket::new("日本語レコード")).unwrap();
        let ticket = backend.show(TicketIdOrSlug::Id(created.id)).unwrap();

        assert!(ticket.document.body.as_str().contains("## 背景"));
        assert!(
            ticket.events[0]
                .body
                .as_str()
                .contains("Ticket が作成されました。")
        );
    }

    fn assignment_source(role: &str) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("source_runtime_id".to_owned(), "runtime-a".to_owned()),
            ("source_worker_id".to_owned(), "worker-a".to_owned()),
            ("source_actor_role".to_owned(), role.to_owned()),
            ("source_assignment_id".to_owned(), "assignment-a".to_owned()),
        ])
    }

    fn active_assignment_fixture(backend: &SqliteTicketBackend, ticket_id: &str) {
        let conn = backend.open_connection().unwrap();
        conn.execute_batch(
            "CREATE TABLE test_worker_assignments (
            workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, runtime_id TEXT NOT NULL,
            worker_id TEXT NOT NULL, assignment_id TEXT NOT NULL, status TEXT NOT NULL
        );
        CREATE VIEW ticket_active_worker_assignments AS
            SELECT workspace_id, ticket_id, runtime_id, worker_id, assignment_id
            FROM test_worker_assignments WHERE status = 'active';",
        )
        .unwrap();
        conn.execute("INSERT INTO test_worker_assignments VALUES (?1, ?2, 'runtime-a', 'worker-a', 'assignment-a', 'active')", params![backend.workspace_id(), ticket_id]).unwrap();
    }

    fn release_assignment_fixture(backend: &SqliteTicketBackend) {
        backend
            .open_connection()
            .unwrap()
            .execute("UPDATE test_worker_assignments SET status = 'released'", [])
            .unwrap();
    }

    #[test]
    fn item_revision_uses_canonical_max_item_edit_index_not_edit_count_or_content_hash() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let (reference, ticket) = backend
            .create_with_snapshot(NewTicket::new("Revision format"))
            .unwrap();
        assert_eq!(ticket_item_revision(&ticket), format!("{}:0", reference.id));
        backend
            .add_event(
                reference.id.clone().into(),
                NewTicketEvent::new(TicketEventKind::Comment, "before edit"),
            )
            .unwrap();
        let first = backend
            .edit_item(
                reference.id.clone().into(),
                TicketItemEdit {
                    title: Some("Edited".to_owned()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(ticket_item_revision(&first), format!("{}:2", reference.id));
        backend
            .add_event(
                reference.id.clone().into(),
                NewTicketEvent::new(TicketEventKind::Comment, "between edits"),
            )
            .unwrap();
        let second = backend
            .edit_item(
                reference.id.clone().into(),
                TicketItemEdit {
                    body: Some(MarkdownText::new("Changed body")),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(ticket_item_revision(&second), format!("{}:4", reference.id));
        let mut reordered = second;
        reordered.events.reverse();
        assert_eq!(
            ticket_item_revision(&reordered),
            format!("{}:4", reference.id)
        );
    }

    #[test]
    fn unassigned_worker_actor_is_distinct_from_ticket_worker_requiring_assignment() {
        for role in ["worker", "ticket_worker"] {
            let tmp = TempDir::new().unwrap();
            let backend = backend(&tmp);
            let (reference, ticket) = backend
                .create_with_snapshot(NewTicket::new("Actor boundary"))
                .unwrap();
            let source = backend.clone().with_event_attributes(BTreeMap::from([
                ("source_actor_role".into(), role.into()),
                ("source_runtime_id".into(), "runtime".into()),
                ("source_worker_id".into(), "worker".into()),
            ]));
            let result = source.complete(&reference.id, completion_request(&ticket, "complete"));
            if role == "worker" {
                let completed = result.unwrap();
                assert_eq!(
                    completed
                        .events
                        .last()
                        .unwrap()
                        .attributes
                        .get("source_actor_role")
                        .map(String::as_str),
                    Some("worker")
                );
            } else {
                assert!(result.is_err());
                let reread = backend.show(reference.id.into()).unwrap();
                assert_eq!(reread.meta.workflow_state, ticket.meta.workflow_state);
                assert_eq!(reread.events.len(), ticket.events.len());
            }
        }
    }

    #[test]
    fn assignment_fence_accepts_exact_live_source_inside_state_and_close_transaction() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let (reference, ticket) = backend
            .create_with_snapshot(NewTicket::new("Active source"))
            .unwrap();
        active_assignment_fixture(&backend, &reference.id);
        let source = backend.clone().with_event_attributes(assignment_source("ticket_worker"))
            .with_mutation_hook(Arc::new(|conn, _| {
                assert!(!conn.is_autocommit());
                let count: i64 = conn.query_row("SELECT COUNT(*) FROM ticket_active_worker_assignments WHERE runtime_id = 'runtime-a' AND worker_id = 'worker-a' AND assignment_id = 'assignment-a'", [], |row| row.get(0)).map_err(sqlite_err)?;
                assert_eq!(count, 1);
                Ok(())
            }));
        let completed = source
            .complete(&reference.id, completion_request(&ticket, "complete"))
            .unwrap();
        source
            .set_workflow_state(
                reference.id.clone().into(),
                TicketStateChange::new("done", "planning", "reopen", "reopen"),
            )
            .unwrap();
        source
            .close(
                reference.id.clone().into(),
                MarkdownText::new("Closed by active coder"),
            )
            .unwrap();
        let closed = source.show(reference.id.clone().into()).unwrap();
        assert_eq!(closed.meta.workflow_state, TicketWorkflowState::Closed);
        assert_eq!(
            completed
                .events
                .last()
                .unwrap()
                .attributes
                .get("source_assignment_id")
                .map(String::as_str),
            Some("assignment-a")
        );
    }

    #[test]
    fn assignment_fence_rejects_every_mismatched_identity_without_mutation() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let (reference, ticket) = backend
            .create_with_snapshot(NewTicket::new("Exact identity"))
            .unwrap();
        active_assignment_fixture(&backend, &reference.id);
        for key in [
            "source_runtime_id",
            "source_worker_id",
            "source_assignment_id",
        ] {
            let mut attrs = assignment_source("ticket_worker");
            attrs.insert(key.to_owned(), "other".to_owned());
            let source = backend.clone().with_event_attributes(attrs);
            assert!(
                source
                    .complete(&reference.id, completion_request(&ticket, "complete"))
                    .is_err()
            );
            assert!(
                source
                    .set_workflow_state(
                        reference.id.clone().into(),
                        TicketStateChange::new("planning", "done", "finished", "finished")
                    )
                    .is_err()
            );
            assert!(
                source
                    .close(reference.id.clone().into(), MarkdownText::new("finished"))
                    .is_err()
            );
            assert_eq!(backend.show(reference.id.clone().into()).unwrap(), ticket);
        }
        let (other, other_ticket) = backend
            .create_with_snapshot(NewTicket::new("Different Ticket"))
            .unwrap();
        let source = backend
            .clone()
            .with_event_attributes(assignment_source("ticket_worker"));
        assert!(
            source
                .complete(&other.id, completion_request(&other_ticket, "complete"))
                .is_err()
        );
        assert_eq!(backend.show(other.id.into()).unwrap(), other_ticket);
        let other_workspace =
            SqliteTicketBackend::open(backend.db_path(), "other-workspace").unwrap();
        let (other, other_ticket) = other_workspace
            .create_with_snapshot(NewTicket::new("Different Workspace"))
            .unwrap();
        let source = other_workspace
            .clone()
            .with_event_attributes(assignment_source("ticket_worker"));
        assert!(
            source
                .complete(&other.id, completion_request(&other_ticket, "complete"))
                .is_err()
        );
        assert_eq!(other_workspace.show(other.id.into()).unwrap(), other_ticket);
    }

    #[test]
    fn assignment_fence_rejects_missing_identity_or_active_view_and_non_coder_assignment_sources() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let (reference, ticket) = backend
            .create_with_snapshot(NewTicket::new("Missing identity"))
            .unwrap();
        for key in [
            "source_runtime_id",
            "source_worker_id",
            "source_assignment_id",
        ] {
            let mut attrs = assignment_source("ticket_worker");
            attrs.remove(key);
            let source = backend.clone().with_event_attributes(attrs);
            assert!(
                source
                    .complete(&reference.id, completion_request(&ticket, "complete"))
                    .is_err()
            );
        }
        let source = backend
            .clone()
            .with_event_attributes(assignment_source("ticket_worker"));
        assert!(
            source
                .complete(&reference.id, completion_request(&ticket, "complete"))
                .is_err()
        );
        active_assignment_fixture(&backend, &reference.id);
        release_assignment_fixture(&backend);
        let source = backend
            .clone()
            .with_event_attributes(assignment_source("orchestrator"));
        assert!(
            source
                .complete(&reference.id, completion_request(&ticket, "complete"))
                .is_err()
        );
        assert_eq!(backend.show(reference.id.into()).unwrap(), ticket);
    }

    #[test]
    fn released_assignment_cannot_mutate_reopened_ticket_but_can_exactly_replay() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let (reference, ticket) = backend
            .create_with_snapshot(NewTicket::new("Released source"))
            .unwrap();
        active_assignment_fixture(&backend, &reference.id);
        let source = backend
            .clone()
            .with_event_attributes(assignment_source("ticket_worker"));
        let request = completion_request(&ticket, "complete");
        let completed = source.complete(&reference.id, request.clone()).unwrap();
        release_assignment_fixture(&backend);
        let mut orchestrator_attrs = assignment_source("orchestrator");
        orchestrator_attrs.remove("source_assignment_id");
        let orchestrator = backend.clone().with_event_attributes(orchestrator_attrs);
        let reopened = orchestrator
            .update_state(
                &reference.id,
                TicketStateUpdate {
                    state: TicketWorkflowState::Planning,
                    author: Some("registered-orchestrator".to_owned()),
                    ..completion_request(&completed, "reopen").into()
                },
            )
            .unwrap();
        assert_eq!(
            source.complete(&reference.id, request.clone()).unwrap(),
            reopened
        );
        assert_eq!(
            &reopened.events[..completed.events.len()],
            completed.events.as_slice()
        );
        assert!(
            source
                .complete(&reference.id, completion_request(&reopened, "new-complete"))
                .is_err()
        );
        assert!(
            source
                .set_workflow_state(
                    reference.id.clone().into(),
                    TicketStateChange::new("planning", "done", "finished", "finished")
                )
                .is_err()
        );
        assert!(
            source
                .close(reference.id.clone().into(), MarkdownText::new("finished"))
                .is_err()
        );
        let close = TicketStateUpdate {
            state: TicketWorkflowState::Closed,
            ..completion_request(&reopened, "close").into()
        };
        assert!(source.update_state(&reference.id, close).is_err());
        assert_eq!(backend.show(reference.id.into()).unwrap(), reopened);
        let conn = backend.open_connection().unwrap();
        let status: String = conn
            .query_row("SELECT status FROM test_worker_assignments", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(status, "released");
    }

    #[test]
    fn assignment_source_actor_is_bound_to_replay_fingerprint() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let (reference, ticket) = backend
            .create_with_snapshot(NewTicket::new("Actor fingerprint"))
            .unwrap();
        active_assignment_fixture(&backend, &reference.id);
        let source = backend
            .clone()
            .with_event_attributes(assignment_source("ticket_worker"));
        let request = completion_request(&ticket, "complete");
        let completed = source.complete(&reference.id, request.clone()).unwrap();
        release_assignment_fixture(&backend);
        for key in [
            "source_runtime_id",
            "source_worker_id",
            "source_assignment_id",
            "source_actor_role",
        ] {
            let mut attrs = assignment_source("ticket_worker");
            attrs.insert(key.to_owned(), "different-source".to_owned());
            let other_source = backend.clone().with_event_attributes(attrs);
            assert!(matches!(
                other_source.complete(&reference.id, request.clone()),
                Err(TicketError::OperationFingerprintMismatch { .. })
            ));
        }
        assert_eq!(backend.show(reference.id.into()).unwrap(), completed);
    }

    #[test]
    fn legacy_state_and_close_share_transactional_audit_and_operation_receipts() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let (reference, ticket) = backend
            .create_with_snapshot(NewTicket::new("Legacy boundary"))
            .unwrap();
        let body = MarkdownText::new("## Custom implementation report\n\nPreserved body.\n");
        let references = vec![TicketReference {
            kind: "report".to_owned(),
            target: "test-results".to_owned(),
        }];
        let mut change = TicketStateChange::new("planning", "done", "finished", body.clone());
        change.references = references.clone();
        change.author = Some("legacy-author".to_owned());
        backend
            .set_workflow_state(reference.id.clone().into(), change)
            .unwrap();
        let done = backend.show(reference.id.clone().into()).unwrap();
        let event = done.events.last().unwrap();
        assert_eq!(event.body, body);
        assert_eq!(event.references, references);
        assert_eq!(event.author.as_deref(), Some("legacy-author"));
        assert_eq!(event.from.as_deref(), Some("planning"));
        assert_eq!(event.reason.as_deref(), Some("finished"));
        assert_eq!(
            event.attributes.get("expected_item_revision"),
            Some(&ticket_item_revision(&ticket))
        );
        assert_eq!(
            event
                .attributes
                .get("fingerprint_version")
                .map(String::as_str),
            Some("state-update-v1")
        );
        let request = TicketStateUpdate {
            expected_item_revision: ticket_item_revision(&ticket),
            expected_state: TicketWorkflowState::Planning,
            state: TicketWorkflowState::Done,
            operation_key: event.attributes["operation_key"].clone(),
            reason: "finished".to_owned(),
            references,
            author: Some("legacy-author".to_owned()),
        };
        let replay = backend
            .with_write(|conn| {
                let snapshot = backend.load_ticket(conn, &reference.id)?;
                backend.apply_state_update(conn, snapshot, request.clone(), body.clone())
            })
            .unwrap();
        assert_eq!(replay, done);
        let resolution =
            MarkdownText::new(format!("## Resolution\n\n{}\n", "details ".repeat(200)));
        backend
            .close(reference.id.clone().into(), resolution.clone())
            .unwrap();
        let closed = backend.show(reference.id.clone().into()).unwrap();
        let close = closed.events.last().unwrap();
        assert_eq!(close.kind, TicketEventKind::Close);
        assert_eq!(close.from.as_deref(), Some("done"));
        assert_eq!(close.to.as_deref(), Some("closed"));
        assert_eq!(close.reason.as_deref(), Some("closed"));
        assert_eq!(close.body, resolution);
        assert_eq!(closed.resolution, Some(resolution));
        assert_eq!(
            close.attributes.get("expected_item_revision"),
            Some(&ticket_item_revision(&done))
        );
        assert_ne!(
            close.attributes["operation_key"],
            event.attributes["operation_key"]
        );
        let conn = backend.open_connection().unwrap();
        let receipt_attributes = backend
            .load_event_attributes(
                &conn,
                &reference.id,
                close.attributes["event_sequence"].parse().unwrap(),
            )
            .unwrap();
        assert_eq!(receipt_attributes.len(), 4);
        let replay = backend
            .with_write(|conn| {
                let current = backend.load_ticket(conn, &reference.id)?;
                backend.apply_state_update(conn, current, request, body)
            })
            .unwrap();
        assert_eq!(replay, closed);
        assert_eq!(&replay.events[..done.events.len()], done.events.as_slice());
    }

    #[test]
    fn legacy_state_and_close_hooks_observe_new_state_and_rollback_common_boundary() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let (reference, ticket) = backend
            .create_with_snapshot(NewTicket::new("Legacy rollback"))
            .unwrap();
        let failing = backend.clone().with_mutation_hook(Arc::new(|conn, event| {
            assert!(!conn.is_autocommit());
            let (current, audited): (String, String) = conn.query_row("SELECT t.workflow_state, e.to_state FROM typed_tickets t JOIN typed_ticket_events e ON e.workspace_id = t.workspace_id AND e.ticket_id = t.ticket_id WHERE t.workspace_id = ?1 AND t.ticket_id = ?2 AND e.event_index = ?3", params![event.workspace_id, event.ticket_id, event.event_index], |row| Ok((row.get(0)?, row.get(1)?))).map_err(sqlite_err)?;
            assert_eq!(current, audited);
            Err(TicketError::Conflict("reject mutation".to_owned()))
        }));
        assert!(
            failing
                .set_workflow_state(
                    reference.id.clone().into(),
                    TicketStateChange::new("planning", "done", "finished", "custom body")
                )
                .is_err()
        );
        assert_eq!(backend.show(reference.id.clone().into()).unwrap(), ticket);
        assert!(
            failing
                .close(reference.id.clone().into(), MarkdownText::new("Resolution"))
                .is_err()
        );
        assert_eq!(backend.show(reference.id.clone().into()).unwrap(), ticket);
        let count: i64 = backend
            .open_connection()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM typed_ticket_event_attributes WHERE key = 'operation_key'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn authenticated_receipt_context_replays_after_live_role_and_assignment_clearance() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let (reference, ticket) = backend
            .create_with_snapshot(NewTicket::new("Role clearance replay"))
            .unwrap();
        active_assignment_fixture(&backend, &reference.id);
        let source = backend
            .clone()
            .with_event_attributes(assignment_source("ticket_worker"));
        let request = completion_request(&ticket, "complete");
        let completed = source.complete(&reference.id, request.clone()).unwrap();
        release_assignment_fixture(&backend);
        let mut current_context = assignment_source("reader");
        current_context.remove("source_assignment_id");
        // Parent authenticates unchanged runtime/worker/author and operation before
        // restoring only receipt role/assignment metadata from the recorded audit.
        let receipt = completed.events.last().unwrap();
        for key in ["source_actor_role", "source_assignment_id"] {
            current_context.insert(key.to_owned(), receipt.attributes[key].clone());
        }
        let replay_source = backend.clone().with_event_attributes(current_context);
        assert_eq!(
            replay_source.complete(&reference.id, request).unwrap(),
            completed
        );
        assert!(
            replay_source
                .complete(
                    &reference.id,
                    completion_request(&completed, "new-complete")
                )
                .is_err()
        );
        assert_eq!(backend.show(reference.id.into()).unwrap(), completed);
    }

    #[test]
    fn common_state_mutation_preserves_actual_historical_ticket_completion_events() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let reference = backend
            .create(NewTicket::new("Historical MR completion"))
            .unwrap();
        let historical = backend.clone().with_event_attributes(BTreeMap::from([
            (
                "operation_key".to_owned(),
                "historical-mr-completion".to_owned(),
            ),
            ("merge_request_id".to_owned(), "MR-existing".to_owned()),
        ]));
        let mut event = NewTicketEvent::new(
            TicketEventKind::Other("ticket_completion".to_owned()),
            "Historical Merge Request completion",
        );
        event.references.push(TicketReference {
            kind: "merge_request".to_owned(),
            target: "MR-existing".to_owned(),
        });
        historical
            .add_event(reference.id.clone().into(), event)
            .unwrap();
        let previous = backend.show(reference.id.clone().into()).unwrap();
        backend
            .set_workflow_state(
                reference.id.clone().into(),
                TicketStateChange::new("planning", "done", "completed", "completed"),
            )
            .unwrap();
        backend
            .close(reference.id.clone().into(), MarkdownText::new("Resolved"))
            .unwrap();
        let current = backend.show(reference.id.into()).unwrap();
        assert_eq!(
            &current.events[..previous.events.len()],
            previous.events.as_slice()
        );
        assert_eq!(
            current.events[1].kind,
            TicketEventKind::Other("ticket_completion".to_owned())
        );
    }

    fn completion_request(ticket: &Ticket, key: &str) -> TicketCompletion {
        TicketCompletion {
            expected_item_revision: ticket_item_revision(ticket),
            expected_state: ticket.meta.workflow_state,
            operation_key: key.to_owned(),
            reason: "Acceptance criteria met".to_owned(),
            references: Vec::new(),
            author: Some("tester".to_owned()),
        }
    }

    #[test]
    fn completion_from_all_states_needs_no_git_targets_or_merge_request() {
        let tmp = TempDir::new().unwrap();
        let backend =
            SqliteTicketBackend::open(tmp.path().join("tickets.db"), "workspace-test").unwrap();
        for state in [
            TicketWorkflowState::Planning,
            TicketWorkflowState::Ready,
            TicketWorkflowState::Queued,
            TicketWorkflowState::InProgress,
            TicketWorkflowState::Done,
            TicketWorkflowState::Closed,
        ] {
            let mut input = NewTicket::new(format!("Complete {state}"));
            input.workflow_state = Some(state);
            let (reference, ticket) = backend.create_with_snapshot(input).unwrap();
            let completed = backend
                .complete(&reference.id, completion_request(&ticket, "complete"))
                .unwrap();
            assert_eq!(completed.meta.workflow_state, TicketWorkflowState::Done);
            assert_eq!(completed.meta.status, ExtensibleTicketStatus::Open);
            assert_eq!(completed.meta.targets, ticket.meta.targets);
            assert_eq!(completed.events.len(), ticket.events.len() + 1);
            let event = completed.events.last().unwrap();
            assert_eq!(event.author.as_deref(), Some("tester"));
            assert_eq!(event.from.as_deref(), Some(state.as_str()));
            assert_eq!(event.to.as_deref(), Some("done"));
            assert!(event.references.is_empty());
            assert_eq!(backend.show(reference.id.into()).unwrap(), completed);
        }
    }

    #[test]
    fn completion_preserves_long_and_multiline_reasons_across_reload_and_replay() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("tickets.db");
        let backend = SqliteTicketBackend::open(&db, "workspace-test").unwrap();
        for reason in [
            "x".repeat(1025),
            "日本語の完了理由を省略せず記録する。".repeat(1024),
            "## 完了理由\r\n\n- 検証済み\n<!-- 詳細 -->\n".repeat(1024),
        ] {
            let (reference, ticket) = backend
                .create_with_snapshot(NewTicket::new("Long completion reason"))
                .unwrap();
            let request = TicketCompletion {
                reason: reason.clone(),
                ..completion_request(&ticket, "complete")
            };
            let completed = backend.complete(&reference.id, request.clone()).unwrap();
            assert_eq!(completed.meta.workflow_state, TicketWorkflowState::Done);
            assert_eq!(completed.events.len(), ticket.events.len() + 1);
            let event = completed.events.last().unwrap();
            assert_eq!(event.reason.as_deref(), Some(reason.as_str()));
            assert_eq!(event.body.as_str(), reason);

            let reopened = SqliteTicketBackend::open(&db, "workspace-test").unwrap();
            assert_eq!(
                reopened.show(reference.id.clone().into()).unwrap(),
                completed
            );
            assert_eq!(
                reopened.complete(&reference.id, request.clone()).unwrap(),
                completed
            );
            let changed = TicketCompletion {
                reason: format!("{reason}追加の判断"),
                ..request
            };
            assert!(matches!(
                reopened.complete(&reference.id, changed),
                Err(TicketError::OperationFingerprintMismatch { .. })
            ));
        }
    }

    #[test]
    fn state_changes_and_close_preserve_long_markdown_reasons_and_bodies() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let (reference, _) = backend
            .create_with_snapshot(NewTicket::new("Long state and close reasons"))
            .unwrap();
        let reason = "## 判断\n\n長文の理由を記録する。\n".repeat(1024);
        let body = "## 検証結果\n\n本文も省略しない。\n".repeat(1024);
        backend
            .set_workflow_state(
                reference.id.clone().into(),
                TicketStateChange::new("planning", "inprogress", reason.clone(), body.clone()),
            )
            .unwrap();
        let current = backend.show(reference.id.clone().into()).unwrap();
        let event = current.events.last().unwrap();
        assert_eq!(event.reason.as_deref(), Some(reason.as_str()));
        assert_eq!(event.body.as_str(), body);
        let request = TicketStateUpdate {
            state: TicketWorkflowState::Closed,
            reason: reason.clone(),
            ..completion_request(&current, "close").into()
        };
        let closed = backend
            .update_state(&reference.id, request.clone())
            .unwrap();
        assert_eq!(closed.meta.workflow_state, TicketWorkflowState::Closed);
        assert_eq!(closed.resolution, Some(MarkdownText::new(&reason)));
        assert_eq!(
            closed.events.last().unwrap().reason.as_deref(),
            Some(reason.as_str())
        );
        assert_eq!(
            backend.update_state(&reference.id, request).unwrap(),
            closed
        );
    }

    #[test]
    fn completion_cas_rejects_stale_item_or_state_without_mutation() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let (reference, ticket) = backend.create_with_snapshot(NewTicket::new("CAS")).unwrap();
        let request = completion_request(&ticket, "complete");
        let edited = backend
            .edit_item(
                reference.id.clone().into(),
                TicketItemEdit {
                    title: Some("Edited".to_owned()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(matches!(
            backend.complete(&reference.id, request),
            Err(TicketError::StaleItemRevision { .. })
        ));
        assert_eq!(backend.show(reference.id.clone().into()).unwrap(), edited);
        let mut request = completion_request(&edited, "complete");
        request.expected_state = TicketWorkflowState::Ready;
        assert!(matches!(
            backend.complete(&reference.id, request),
            Err(TicketError::StaleWorkflowState { .. })
        ));
        assert_eq!(backend.show(reference.id.into()).unwrap(), edited);
    }

    #[test]
    fn completion_exact_replay_survives_later_edits_reopen_and_backend_restart() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("tickets.db");
        let backend = SqliteTicketBackend::open(&db, "workspace-test").unwrap();
        let (reference, ticket) = backend
            .create_with_snapshot(NewTicket::new("Replay"))
            .unwrap();
        let mut request = completion_request(&ticket, "complete");
        request.references = vec![TicketReference {
            kind: "test_report".to_owned(),
            target: "optional-evidence".to_owned(),
        }];
        let completed = backend.complete("T-1", request.clone()).unwrap();
        assert_eq!(
            completed.events.last().unwrap().references,
            request.references
        );
        let reopened = backend
            .update_state(
                &reference.id,
                TicketStateUpdate {
                    state: TicketWorkflowState::Planning,
                    operation_key: "reopen".to_owned(),
                    ..completion_request(&completed, "unused").into()
                },
            )
            .unwrap();
        let edited = backend
            .edit_item(
                reference.id.clone().into(),
                TicketItemEdit {
                    body: Some(MarkdownText::new("New body")),
                    ..Default::default()
                },
            )
            .unwrap();
        let backend = SqliteTicketBackend::open(&db, "workspace-test").unwrap();
        let replayed = backend.complete(&reference.id, request.clone()).unwrap();
        assert_eq!(replayed, edited);
        assert_eq!(replayed.meta.workflow_state, TicketWorkflowState::Planning);
        assert_eq!(replayed.document.body.as_str(), "New body");
        assert_eq!(
            &replayed.events[..completed.events.len()],
            completed.events.as_slice()
        );
        let original_receipt = replayed
            .events
            .iter()
            .find(|event| event.attributes.get("operation_key") == Some(&request.operation_key))
            .unwrap();
        assert_eq!(original_receipt, completed.events.last().unwrap());
        assert_eq!(replayed.events.len(), edited.events.len());
        assert_eq!(backend.show(reference.id.into()).unwrap(), edited);
        assert_eq!(reopened.meta.workflow_state, TicketWorkflowState::Planning);
    }

    #[test]
    fn state_operation_receipts_store_constant_metadata_without_ticket_snapshots() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let mut input = NewTicket::new("Linear receipt storage");
        input.body = MarkdownText::new("Large authoritative item body. ".repeat(2000));
        let (reference, mut current) = backend.create_with_snapshot(input).unwrap();
        let first_request = completion_request(&current, "complete-0");
        let operations = 16;
        for index in 0..operations {
            let request = completion_request(&current, &format!("complete-{index}"));
            current = backend.complete(&reference.id, request).unwrap();
        }
        let conn = backend.open_connection().unwrap();
        let (count, bytes): (i64, i64) = conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(length(value)), 0) FROM typed_ticket_event_attributes WHERE workspace_id = ?1 AND ticket_id = ?2",
            params![backend.workspace_id(), reference.id], |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        assert_eq!(count, operations * 4);
        assert!(bytes < operations * 256);
        let original_receipt = current.events[1].clone();
        let replayed = backend.complete(&reference.id, first_request).unwrap();
        assert_eq!(replayed, current);
        assert_eq!(replayed.events[1], original_receipt);
        assert_eq!(replayed.events.len(), operations as usize + 1);
        let count_after: i64 = conn.query_row(
            "SELECT COUNT(*) FROM typed_ticket_event_attributes WHERE workspace_id = ?1 AND ticket_id = ?2",
            params![backend.workspace_id(), reference.id], |row| row.get(0),
        ).unwrap();
        assert_eq!(count_after, count);
    }

    #[test]
    fn completion_fingerprint_binds_every_request_field_and_operation_kind() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let (reference, ticket) = backend
            .create_with_snapshot(NewTicket::new("Fingerprint"))
            .unwrap();
        let request = completion_request(&ticket, "complete");
        let completed = backend.complete(&reference.id, request.clone()).unwrap();
        let variants = [
            TicketCompletion {
                reason: "Changed reason".to_owned(),
                ..request.clone()
            },
            TicketCompletion {
                author: Some("other".to_owned()),
                ..request.clone()
            },
            TicketCompletion {
                author: None,
                ..request.clone()
            },
            TicketCompletion {
                expected_item_revision: "other-revision".to_owned(),
                ..request.clone()
            },
            TicketCompletion {
                expected_state: TicketWorkflowState::Ready,
                ..request.clone()
            },
            TicketCompletion {
                references: vec![TicketReference {
                    kind: "report".to_owned(),
                    target: "different".to_owned(),
                }],
                ..request.clone()
            },
        ];
        for changed in variants {
            assert!(matches!(
                backend.complete(&reference.id, changed),
                Err(TicketError::OperationFingerprintMismatch { .. })
            ));
        }
        let close = TicketStateUpdate {
            state: TicketWorkflowState::Closed,
            ..request.into()
        };
        assert!(matches!(
            backend.update_state(&reference.id, close),
            Err(TicketError::OperationFingerprintMismatch { .. })
        ));
        assert_eq!(backend.show(reference.id.into()).unwrap(), completed);
    }

    #[test]
    fn audited_close_reopen_preserves_resolution_history_and_resource_identity() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let (reference, ticket) = backend
            .create_with_snapshot(NewTicket::new("Close reopen"))
            .unwrap();
        let close = TicketStateUpdate {
            state: TicketWorkflowState::Closed,
            ..completion_request(&ticket, "close").into()
        };
        let closed = backend.update_state(&reference.id, close.clone()).unwrap();
        assert_eq!(closed.meta.status, ExtensibleTicketStatus::Closed);
        assert_eq!(closed.resolution, Some(MarkdownText::new(&close.reason)));
        assert_eq!(closed.events.last().unwrap().kind, TicketEventKind::Close);
        assert_eq!(closed.events.last().unwrap().author, close.author);
        let reopen = TicketStateUpdate {
            state: TicketWorkflowState::Planning,
            ..completion_request(&closed, "reopen").into()
        };
        let reopened = backend.update_state(&reference.id, reopen.clone()).unwrap();
        assert_eq!(reopened.meta.status, ExtensibleTicketStatus::Open);
        assert_eq!(reopened.meta.resource_key, closed.meta.resource_key);
        assert_eq!(reopened.resolution, closed.resolution);
        assert_eq!(
            &reopened.events[..closed.events.len()],
            closed.events.as_slice()
        );
        let replayed = backend.update_state(&reference.id, close).unwrap();
        assert_eq!(replayed, reopened);
        assert_eq!(replayed.meta.workflow_state, TicketWorkflowState::Planning);
        assert_eq!(
            &replayed.events[..closed.events.len()],
            closed.events.as_slice()
        );
        assert_eq!(
            backend.update_state(&reference.id, reopen).unwrap(),
            reopened
        );
        assert_eq!(backend.show(reference.id.into()).unwrap(), reopened);
    }

    #[test]
    fn completion_mutation_hook_failure_rolls_back_state_event_and_replay() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let (reference, ticket) = backend
            .create_with_snapshot(NewTicket::new("Rollback"))
            .unwrap();
        let request = completion_request(&ticket, "complete");
        let failing = backend.clone().with_mutation_hook(Arc::new(|_, _| {
            Err(TicketError::Conflict("reject audit".to_owned()))
        }));
        assert!(failing.complete(&reference.id, request.clone()).is_err());
        assert_eq!(backend.show(reference.id.clone().into()).unwrap(), ticket);
        let completed = backend.complete(&reference.id, request.clone()).unwrap();
        assert_eq!(backend.complete(&reference.id, request).unwrap(), completed);
    }

    #[test]
    fn completion_concurrent_exact_replay_appends_one_event() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let (reference, ticket) = backend
            .create_with_snapshot(NewTicket::new("Concurrent"))
            .unwrap();
        let request = completion_request(&ticket, "complete");
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let handles = (0..2)
            .map(|_| {
                let backend = backend.clone();
                let reference = reference.clone();
                let request = request.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    backend.complete(&reference.id, request).unwrap()
                })
            })
            .collect::<Vec<_>>();
        let results = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(results[0], results[1]);
        assert_eq!(results[0].events.len(), ticket.events.len() + 1);
        assert_eq!(backend.show(reference.id.into()).unwrap(), results[0]);
    }

    #[test]
    fn item_revision_tracks_target_edits_and_aba_but_not_comments_or_state() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let (reference, ticket) = backend
            .create_with_snapshot(NewTicket::new("Revision"))
            .unwrap();
        let revision = ticket_item_revision(&ticket);
        backend
            .add_event(
                reference.id.clone().into(),
                NewTicketEvent::new(TicketEventKind::Comment, "comment"),
            )
            .unwrap();
        let commented = backend.show(reference.id.clone().into()).unwrap();
        assert_eq!(ticket_item_revision(&commented), revision);
        let completed = backend
            .complete(&reference.id, completion_request(&ticket, "complete"))
            .unwrap();
        assert_eq!(ticket_item_revision(&completed), revision);
        backend
            .edit_item(
                reference.id.clone().into(),
                TicketItemEdit {
                    targets: Some(TicketTargetsEdit::Set {
                        targets: vec![write_target("main")],
                    }),
                    ..Default::default()
                },
            )
            .unwrap();
        let restored = backend
            .edit_item(
                reference.id.into(),
                TicketItemEdit {
                    targets: Some(TicketTargetsEdit::Clear),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_ne!(ticket_item_revision(&restored), revision);
    }

    #[test]
    fn completion_rejects_invalid_requests_without_recording_operation_key() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let (reference, ticket) = backend
            .create_with_snapshot(NewTicket::new("Validation"))
            .unwrap();
        let request = completion_request(&ticket, "complete");
        for invalid in [
            TicketCompletion {
                operation_key: " ".to_owned(),
                ..request.clone()
            },
            TicketCompletion {
                expected_item_revision: "".to_owned(),
                ..request.clone()
            },
            TicketCompletion {
                reason: "".to_owned(),
                ..request.clone()
            },
            TicketCompletion {
                reason: " \t ".to_owned(),
                ..request.clone()
            },
            TicketCompletion {
                references: vec![TicketReference {
                    kind: " \t ".to_owned(),
                    target: "report".to_owned(),
                }],
                ..request.clone()
            },
            TicketCompletion {
                references: vec![TicketReference {
                    kind: "report".to_owned(),
                    target: " \t ".to_owned(),
                }],
                ..request.clone()
            },
            TicketCompletion {
                author: Some("".to_owned()),
                ..request.clone()
            },
            TicketCompletion {
                references: vec![TicketReference {
                    kind: "".to_owned(),
                    target: "report".to_owned(),
                }],
                ..request.clone()
            },
            TicketCompletion {
                references: vec![TicketReference {
                    kind: "report".to_owned(),
                    target: "".to_owned(),
                }],
                ..request.clone()
            },
        ] {
            assert!(matches!(
                backend.complete(&reference.id, invalid),
                Err(TicketError::Conflict(_))
            ));
            assert_eq!(backend.show(reference.id.clone().into()).unwrap(), ticket);
        }
        assert!(backend.complete(&reference.id, request).is_ok());
    }

    #[test]
    fn legacy_state_and_close_reject_blank_reasons_resolution_and_references() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let (reference, ticket) = backend
            .create_with_snapshot(NewTicket::new("Legacy validation"))
            .unwrap();
        assert!(matches!(
            backend.set_workflow_state(
                reference.id.clone().into(),
                TicketStateChange::new("planning", "done", " \t ", "finished")
            ),
            Err(TicketError::Conflict(_))
        ));
        let mut change = TicketStateChange::new("planning", "done", "finished", "finished");
        change.references.push(TicketReference {
            kind: "report".to_owned(),
            target: " \t ".to_owned(),
        });
        assert!(matches!(
            backend.set_workflow_state(reference.id.clone().into(), change),
            Err(TicketError::Conflict(_))
        ));
        assert!(matches!(
            backend.close(reference.id.clone().into(), MarkdownText::new(" \n\t ")),
            Err(TicketError::Conflict(_))
        ));
        assert_eq!(backend.show(reference.id.into()).unwrap(), ticket);
    }

    #[test]
    fn completion_and_state_update_dispatch_round_trip_the_shared_api() {
        let tmp = TempDir::new().unwrap();
        let backend = crate::tool::TicketToolBackend::new(backend(&tmp));
        let (reference, ticket) = backend
            .create_with_snapshot(NewTicket::new("Dispatch"))
            .unwrap();
        let operation = TicketBackendOperation::Complete {
            ticket: "T-1".to_owned(),
            request: completion_request(&ticket, "complete"),
        };
        let encoded = serde_json::to_string(&operation).unwrap();
        let operation = serde_json::from_str(&encoded).unwrap();
        let TicketBackendOperationResult::Ticket(completed) =
            execute_ticket_backend_operation(&backend, operation).unwrap()
        else {
            panic!("expected Ticket")
        };
        assert_eq!(completed.meta.workflow_state, TicketWorkflowState::Done);
        let operation = TicketBackendOperation::UpdateState {
            ticket: reference.id,
            request: TicketStateUpdate {
                state: TicketWorkflowState::Closed,
                ..completion_request(&completed, "close").into()
            },
        };
        let encoded = serde_json::to_string(&operation).unwrap();
        let operation = serde_json::from_str(&encoded).unwrap();
        let TicketBackendOperationResult::Ticket(closed) =
            execute_ticket_backend_operation(&backend, operation).unwrap()
        else {
            panic!("expected Ticket")
        };
        assert_eq!(closed.meta.workflow_state, TicketWorkflowState::Closed);
    }

    #[test]
    fn queue_assignment_fence_names_only_the_explicit_ticket() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let (reference, ticket) = backend
            .create_with_snapshot(NewTicket::new("Queue fence"))
            .unwrap();
        let other = backend.create(NewTicket::new("Other")).unwrap();
        let assignments = BTreeMap::from([
            (reference.id.clone(), "assignment".to_owned()),
            (other.id, "other-assignment".to_owned()),
        ]);
        let fenced = backend.clone().with_event_attributes(BTreeMap::from([(
            "queue_orchestrator_assignments".to_owned(),
            serde_json::to_string(&assignments).unwrap(),
        )]));
        assert!(
            fenced
                .queue_ready(reference.id.clone().into(), "actor")
                .is_err()
        );
        assert_eq!(backend.show(reference.id.clone().into()).unwrap(), ticket);
        let assignments = BTreeMap::from([(reference.id.clone(), "assignment".to_owned())]);
        let fenced = backend.with_event_attributes(BTreeMap::from([(
            "queue_orchestrator_assignments".to_owned(),
            serde_json::to_string(&assignments).unwrap(),
        )]));
        assert_eq!(
            fenced
                .queue_ready(reference.id.clone().into(), "actor")
                .unwrap()
                .queued_tickets,
            vec![reference.id.clone()]
        );
        let current = fenced.show(reference.id.into()).unwrap();
        assert_eq!(
            current
                .events
                .last()
                .unwrap()
                .attributes
                .get("orchestrator_assignment_id")
                .map(String::as_str),
            Some("assignment")
        );
    }

    #[test]
    fn state_field_rejects_removed_phase_path_without_mutation() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let (reference, ticket) = backend
            .create_with_snapshot(NewTicket::new("No phase"))
            .unwrap();
        assert!(
            backend
                .set_state_field(
                    reference.id.clone().into(),
                    "phase",
                    TicketStateChange::new("planning", "done", "finished", "finished")
                )
                .is_err()
        );
        assert_eq!(backend.show(reference.id.into()).unwrap(), ticket);
    }

    #[test]
    fn state_defaults_and_queue_transition_round_trip() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let planning = backend.create(NewTicket::new("Default planning")).unwrap();
        let planning = backend.show(TicketIdOrSlug::Id(planning.id)).unwrap();
        assert_eq!(planning.meta.workflow_state, TicketWorkflowState::Planning);
        assert!(planning.meta.workflow_state_explicit);

        let mut closed_input = NewTicket::new("Closed state");
        closed_input.workflow_state = Some(TicketWorkflowState::Closed);
        let closed = backend.create(closed_input).unwrap();
        let closed = backend.show(TicketIdOrSlug::Id(closed.id)).unwrap();
        assert_eq!(closed.meta.workflow_state, TicketWorkflowState::Closed);
        assert!(closed.meta.workflow_state_explicit);

        let mut ready_input = NewTicket::new("Ready Workflow");
        ready_input.workflow_state = Some(TicketWorkflowState::Ready);
        ready_input.targets = vec![ticket_target(
            "main",
            Some("develop"),
            TicketTargetAccess::ReadWrite,
        )];
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
        assert!(!event.attributes.contains_key("targets"));
        assert_eq!(queued.meta.targets[0].repository_key, "main");
        assert!(!event.attributes.contains_key("repository_id"));
        assert!(!event.attributes.contains_key("ref_selector"));
    }

    #[test]
    fn queue_accepts_all_states_without_targets() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        for state in [
            TicketWorkflowState::Planning,
            TicketWorkflowState::Ready,
            TicketWorkflowState::Queued,
            TicketWorkflowState::InProgress,
            TicketWorkflowState::Done,
            TicketWorkflowState::Closed,
        ] {
            let mut input = NewTicket::new(format!("Queue {state}"));
            input.workflow_state = Some(state);
            let ticket = backend.create(input).unwrap();
            let outcome = backend
                .queue_ready(ticket.id.clone().into(), "actor")
                .unwrap();
            assert_eq!(outcome.queued_tickets, vec![ticket.id.clone()]);
            let current = backend.show(ticket.id.into()).unwrap();
            assert_eq!(current.meta.workflow_state, TicketWorkflowState::Queued);
            assert_eq!(current.meta.status, ExtensibleTicketStatus::Open);
            assert_eq!(current.events.last().unwrap().from, Some(state.to_string()));
        }
    }

    #[test]
    fn mark_ready_resolves_target_and_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let mut input = NewTicket::new("Planning Ready");
        input.targets = vec![
            write_target("main"),
            ticket_target("docs", None, TicketTargetAccess::ReadOnly),
        ];
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
        assert_eq!(
            first.meta.targets,
            vec![
                ticket_target("main", Some("develop"), TicketTargetAccess::ReadWrite,),
                ticket_target("docs", Some("develop"), TicketTargetAccess::ReadOnly),
            ]
        );
        assert_eq!(first.events, second.events);
        let ready_event = first
            .events
            .iter()
            .find(|event| event.attributes.contains_key("operation_key"))
            .unwrap();
        let event_targets = serde_json::from_str::<Vec<ResolvedTicketTarget>>(
            ready_event.attributes.get("targets").unwrap(),
        )
        .unwrap();
        assert_eq!(event_targets.len(), 2);
        assert_eq!(event_targets[0].repository_key, "main");
        assert_eq!(event_targets[0].access, TicketTargetAccess::ReadWrite);
        assert_eq!(event_targets[1].repository_key, "docs");
        assert_eq!(event_targets[1].access, TicketTargetAccess::ReadOnly);
        assert_eq!(
            ready_event
                .attributes
                .get("fingerprint_version")
                .map(String::as_str),
            Some("2")
        );
        assert!(!ready_event.attributes.contains_key("repository_id"));
        assert!(!ready_event.attributes.contains_key("ref_selector"));
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
    fn target_validation_rejects_duplicates_but_allows_read_only_targets() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);

        let mut duplicate = NewTicket::new("Duplicate targets");
        duplicate.targets = vec![
            write_target("main"),
            ticket_target("main", Some("docs"), TicketTargetAccess::ReadOnly),
        ];
        assert!(matches!(
            backend.create(duplicate),
            Err(TicketError::DuplicateTargetRepository(repository)) if repository == "main"
        ));

        let mut no_write = NewTicket::new("No write target");
        no_write.targets = vec![ticket_target("docs", None, TicketTargetAccess::ReadOnly)];
        let ticket = backend.create(no_write).unwrap();
        let ready = backend
            .mark_ready(
                TicketIdOrSlug::Id(ticket.id.clone()),
                TicketMarkReady {
                    operation_key: "ready-no-write".into(),
                    reason: None,
                    author: None,
                    intake_summary: None,
                },
            )
            .unwrap();
        assert_eq!(ready.meta.targets[0].access, TicketTargetAccess::ReadOnly);
        let unchanged = backend.show(TicketIdOrSlug::Id(ticket.id)).unwrap();
        assert_eq!(unchanged.meta.workflow_state, TicketWorkflowState::Ready);

        let mut multiple_writes = NewTicket::new("Multiple write targets");
        multiple_writes.targets = vec![write_target("main"), write_target("docs")];
        let ticket = backend.create(multiple_writes).unwrap();
        let ready = backend
            .mark_ready(
                TicketIdOrSlug::Id(ticket.id),
                TicketMarkReady {
                    operation_key: "ready-multiple-writes".into(),
                    reason: None,
                    author: None,
                    intake_summary: None,
                },
            )
            .unwrap();
        assert_eq!(ready.meta.workflow_state, TicketWorkflowState::Ready);
        assert_eq!(
            ready
                .meta
                .targets
                .iter()
                .filter(|target| target.access == TicketTargetAccess::ReadWrite)
                .count(),
            2
        );
    }

    #[test]
    fn targets_can_be_edited_and_cleared_in_every_state() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        for state in [
            TicketWorkflowState::Planning,
            TicketWorkflowState::Ready,
            TicketWorkflowState::Queued,
            TicketWorkflowState::InProgress,
            TicketWorkflowState::Done,
            TicketWorkflowState::Closed,
        ] {
            let mut input = NewTicket::new(format!("Targets {state}"));
            input.workflow_state = Some(state);
            let ticket = backend.create(input).unwrap();
            let set = backend
                .edit_item(
                    ticket.id.clone().into(),
                    TicketItemEdit {
                        targets: Some(TicketTargetsEdit::Set {
                            targets: vec![write_target("main")],
                        }),
                        ..Default::default()
                    },
                )
                .unwrap();
            assert_eq!(set.meta.workflow_state, state);
            assert_eq!(set.meta.targets, vec![write_target("main")]);
            let cleared = backend
                .edit_item(
                    ticket.id.into(),
                    TicketItemEdit {
                        targets: Some(TicketTargetsEdit::Clear),
                        ..Default::default()
                    },
                )
                .unwrap();
            assert_eq!(cleared.meta.workflow_state, state);
            assert!(cleared.meta.targets.is_empty());
            assert_eq!(cleared.meta.resource_key, set.meta.resource_key);
        }
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
        let close = record.events.last().unwrap();
        assert_eq!(close.kind, TicketEventKind::Close);
        assert_eq!(close.from.as_deref(), Some("planning"));
        assert_eq!(close.to.as_deref(), Some("closed"));
        assert_eq!(
            close.attributes.get("expected_item_revision"),
            Some(&format!("{}:0", record.meta.id))
        );
        assert!(close.attributes.contains_key("operation_key"));
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
        waiter_input.targets = vec![write_target("main")];
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
        incoming_input.targets = vec![write_target("main")];
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
    fn queue_retains_planning_dependency_and_incoming_blocker_diagnostics() {
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
        let outcome = backend
            .queue_ready(TicketIdOrSlug::Id(blocked.id.clone()), "test")
            .unwrap();
        assert_eq!(outcome.queued_tickets.len(), 1);
        let unchanged = backend
            .show(TicketIdOrSlug::Id(blocked.id.clone()))
            .unwrap();
        assert_eq!(unchanged.meta.workflow_state, TicketWorkflowState::Queued);
        assert_eq!(unchanged.relations.blockers.len(), 1);
        assert_eq!(
            unchanged.relations.blockers[0].blocking_ticket,
            dependency.id
        );

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
        let outcome = backend
            .queue_ready(TicketIdOrSlug::Id(incoming.id.clone()), "test")
            .unwrap();
        assert_eq!(outcome.queued_tickets.len(), 1);
        let unchanged_incoming = backend
            .show(TicketIdOrSlug::Id(incoming.id.clone()))
            .unwrap();
        assert_eq!(
            unchanged_incoming.meta.workflow_state,
            TicketWorkflowState::Queued
        );
        assert_eq!(unchanged_incoming.relations.blockers.len(), 1);
        assert_eq!(
            unchanged_incoming.relations.blockers[0].blocking_ticket,
            blocker.id
        );
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
        assert_eq!(backend.doctor().unwrap().error_count(), 0);
    }

    #[test]
    fn orchestration_plan_validation_rejects_missing_related_ticket() {
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
    }
}
