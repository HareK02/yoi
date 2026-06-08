//! Ticket domain types and the local `.yoi/tickets/` file backend.
//!
//! The public domain name is **Ticket**. `LocalTicketBackend` preserves the
//! repository's current `.yoi/tickets/{open,pending,closed}/<id>/` layout and the
//! event/thread format while exposing typed Rust operations.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

use chrono::Utc;
use fs4::fs_std::FileExt;
use serde_yaml::{Mapping as YamlMapping, Value as YamlValue};
use thiserror::Error;

pub mod config;
pub mod tool;

const STATUSES: [TicketStatus; 3] = [
    TicketStatus::Open,
    TicketStatus::Pending,
    TicketStatus::Closed,
];
const REQUIRED_FIELDS: [&str; 11] = [
    "id",
    "slug",
    "title",
    "status",
    "kind",
    "priority",
    "labels",
    "created_at",
    "updated_at",
    "assignee",
    "legacy_ticket",
];
const MAX_STATE_CHANGE_REASON_BYTES: usize = 1024;
const MAX_INTAKE_SUMMARY_BODY_BYTES: usize = 16 * 1024;
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
    #[error("invalid local ticket status for mutation: {0}")]
    InvalidLocalStatus(String),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TicketStatus {
    Open,
    Pending,
    Closed,
}

impl TicketStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Pending => "pending",
            Self::Closed => "closed",
        }
    }

    pub fn parse_local(value: &str) -> Option<Self> {
        match value {
            "open" => Some(Self::Open),
            "pending" => Some(Self::Pending),
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ExtensibleTicketStatus {
    Open,
    Pending,
    Closed,
    Other(String),
}

impl ExtensibleTicketStatus {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Open => "open",
            Self::Pending => "pending",
            Self::Closed => "closed",
            Self::Other(value) => value.as_str(),
        }
    }

    pub fn as_local(&self) -> Option<TicketStatus> {
        match self {
            Self::Open => Some(TicketStatus::Open),
            Self::Pending => Some(TicketStatus::Pending),
            Self::Closed => Some(TicketStatus::Closed),
            Self::Other(_) => None,
        }
    }
}

impl From<&str> for ExtensibleTicketStatus {
    fn from(value: &str) -> Self {
        match value {
            "open" => Self::Open,
            "pending" => Self::Pending,
            "closed" => Self::Closed,
            other => Self::Other(other.to_string()),
        }
    }
}

impl From<TicketStatus> for ExtensibleTicketStatus {
    fn from(value: TicketStatus) -> Self {
        match value {
            TicketStatus::Open => Self::Open,
            TicketStatus::Pending => Self::Pending,
            TicketStatus::Closed => Self::Closed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TicketWorkflowState {
    Intake,
    Ready,
    Queued,
    InProgress,
    Done,
}

impl TicketWorkflowState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Intake => "intake",
            Self::Ready => "ready",
            Self::Queued => "queued",
            Self::InProgress => "inprogress",
            Self::Done => "done",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "intake" => Some(Self::Intake),
            "ready" => Some(Self::Ready),
            "queued" => Some(Self::Queued),
            "inprogress" => Some(Self::InProgress),
            "done" => Some(Self::Done),
            _ => None,
        }
    }

    pub fn default_for_status(status: &ExtensibleTicketStatus) -> Self {
        match status {
            ExtensibleTicketStatus::Closed => Self::Done,
            _ => Self::Intake,
        }
    }

    pub fn is_intake_ready_transition(from: Self, to: Self) -> bool {
        from == Self::Intake && to == Self::Ready
    }

    pub fn is_queue_transition(from: Self, to: Self) -> bool {
        from == Self::Ready && to == Self::Queued
    }

    pub fn is_role_transition(from: Self, to: Self) -> bool {
        matches!(
            (from, to),
            (Self::Queued, Self::InProgress) | (Self::InProgress, Self::Done)
        )
    }
}

impl fmt::Display for TicketWorkflowState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketReference {
    pub kind: String,
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTicket {
    pub title: String,
    pub slug: Option<String>,
    pub kind: String,
    pub priority: String,
    pub labels: Vec<String>,
    pub body: MarkdownText,
    pub author: Option<String>,
    pub assignee: Option<String>,
    pub legacy_ticket: Option<String>,
    pub readiness: Option<String>,
    pub needs_preflight: Option<bool>,
    pub risk_flags: Vec<String>,
    pub action_required: Option<String>,
    pub workflow_state: Option<TicketWorkflowState>,
    pub attention_required: Option<String>,
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
            legacy_ticket: None,
            readiness: None,
            needs_preflight: None,
            risk_flags: Vec::new(),
            action_required: None,
            workflow_state: None,
            attention_required: None,
            queued_by: None,
            queued_at: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TicketFilter {
    pub status: Option<TicketStatus>,
}

impl TicketFilter {
    pub fn all() -> Self {
        Self { status: None }
    }

    pub fn status(status: TicketStatus) -> Self {
        Self {
            status: Some(status),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketRef {
    pub id: String,
    pub slug: String,
    pub status: TicketStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
    pub legacy_ticket: Option<String>,
    pub readiness: Option<String>,
    pub needs_preflight: Option<bool>,
    pub risk_flags: Vec<String>,
    pub action_required: Option<String>,
    pub workflow_state: TicketWorkflowState,
    pub workflow_state_explicit: bool,
    pub attention_required: Option<String>,
    pub queued_by: Option<String>,
    pub queued_at: Option<String>,
    pub raw: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketSummary {
    pub id: String,
    pub slug: String,
    pub title: String,
    pub status: ExtensibleTicketStatus,
    pub kind: String,
    pub priority: String,
    pub labels: Vec<String>,
    pub readiness: Option<String>,
    pub needs_preflight: Option<bool>,
    pub action_required: Option<String>,
    pub workflow_state: TicketWorkflowState,
    pub workflow_state_explicit: bool,
    pub attention_required: Option<String>,
    pub queued_by: Option<String>,
    pub queued_at: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketDocument {
    pub body: MarkdownText,
    pub raw_frontmatter: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketArtifactRef {
    /// Path relative to the ticket's `artifacts/` directory.
    pub relative_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ticket {
    pub meta: TicketMeta,
    pub document: TicketDocument,
    pub events: Vec<TicketEvent>,
    pub artifacts: Vec<TicketArtifactRef>,
    pub resolution: Option<MarkdownText>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TicketDoctorSeverity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketDoctorDiagnostic {
    pub severity: TicketDoctorSeverity,
    pub message: String,
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
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
    fn list(&self, filter: TicketFilter) -> Result<Vec<TicketSummary>>;
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
    fn set_status(&self, id: TicketIdOrSlug, status: TicketStatus) -> Result<()>;
    fn close(&self, id: TicketIdOrSlug, resolution: MarkdownText) -> Result<()>;
    fn doctor(&self) -> Result<TicketDoctorReport>;
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
            format!("Ticket intake が完了しました。workflow_state {from} -> ready。\n")
        } else {
            format!("Ticket intake complete; workflow_state {from} -> ready.\n")
        }
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

    fn status_changed_body(&self, status: TicketStatus) -> String {
        if is_japanese_record_language(self.record_language()) {
            format!("Ticket status を `{}` に変更しました。\n", status.as_str())
        } else {
            format!("Status changed to `{}`.\n", status.as_str())
        }
    }

    fn closed_workflow_state_body(&self) -> &'static str {
        if is_japanese_record_language(self.record_language()) {
            "Ticket closed; workflow_state を done に設定しました。\n"
        } else {
            "Ticket closed; workflow_state set to done.\n"
        }
    }

    fn ensure_backend_dirs(&self) -> Result<()> {
        for status in STATUSES {
            let dir = self.status_dir(status);
            fs::create_dir_all(&dir).map_err(|e| io_err(dir, e))?;
        }
        Ok(())
    }

    fn status_dir(&self, status: TicketStatus) -> PathBuf {
        self.root.join(status.as_str())
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

    fn iter_ticket_dirs(&self, filter: TicketFilter) -> Result<Vec<(TicketStatus, PathBuf)>> {
        let mut dirs = Vec::new();
        for status in STATUSES {
            if let Some(filter_status) = filter.status {
                if status != filter_status {
                    continue;
                }
            }
            let status_dir = self.status_dir(status);
            if !status_dir.exists() {
                continue;
            }
            let entries = fs::read_dir(&status_dir).map_err(|e| io_err(&status_dir, e))?;
            for entry in entries {
                let entry = entry.map_err(|e| io_err(&status_dir, e))?;
                let path = entry.path();
                if path.is_dir() {
                    dirs.push((status, path));
                }
            }
        }
        dirs.sort_by(|(_, a), (_, b)| a.cmp(b));
        Ok(dirs)
    }

    fn find_ticket_dir(&self, query: &TicketIdOrSlug) -> Result<PathBuf> {
        let query = query.as_query();
        let mut matches = Vec::new();
        for (_, dir) in self.iter_ticket_dirs(TicketFilter::all())? {
            let item = dir.join("item.md");
            if !item.exists() {
                continue;
            }
            let parsed = read_item_file(&item)?;
            let id = parsed.frontmatter.get("id").map(String::as_str);
            let slug = parsed.frontmatter.get("slug").map(String::as_str);
            if id == Some(query) || slug == Some(query) {
                matches.push(dir);
            }
        }
        match matches.len() {
            0 => Err(TicketError::NotFound(query.to_string())),
            1 => Ok(matches.remove(0)),
            _ => Err(TicketError::Ambiguous {
                query: query.to_string(),
                matches,
            }),
        }
    }

    fn ticket_from_dir(&self, dir: &Path) -> Result<Ticket> {
        let item_path = dir.join("item.md");
        let parsed = read_item_file(&item_path)?;
        let meta = ticket_meta(parsed.frontmatter.clone());
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
            resolution,
        })
    }

    fn ticket_workflow_state_from_item(&self, item: &Path) -> Result<TicketWorkflowState> {
        let parsed = read_item_file(item)?;
        let meta = ticket_meta(parsed.frontmatter);
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
        let current = self.ticket_workflow_state_from_item(&item)?;
        if current != expected_from {
            return Err(TicketError::Conflict(format!(
                "workflow_state changed concurrently: expected `{}`, found `{}`",
                expected_from.as_str(),
                current.as_str()
            )));
        }
        self.append_state_changed_event(dir, &change, Some("workflow_state"))?;
        let mut updates = vec![("workflow_state", to.as_str())];
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
}

impl TicketBackend for LocalTicketBackend {
    fn list(&self, filter: TicketFilter) -> Result<Vec<TicketSummary>> {
        let mut tickets = Vec::new();
        for (_, dir) in self.iter_ticket_dirs(filter)? {
            let item = dir.join("item.md");
            if !item.exists() {
                continue;
            }
            let parsed = read_item_file(&item)?;
            let meta = ticket_meta(parsed.frontmatter);
            tickets.push(TicketSummary {
                id: meta.id,
                slug: meta.slug,
                title: meta.title,
                status: meta.status,
                kind: meta.kind,
                priority: meta.priority,
                labels: meta.labels,
                readiness: meta.readiness,
                needs_preflight: meta.needs_preflight,
                action_required: meta.action_required,
                workflow_state: meta.workflow_state,
                workflow_state_explicit: meta.workflow_state_explicit,
                attention_required: meta.attention_required,
                queued_by: meta.queued_by,
                queued_at: meta.queued_at,
                updated_at: meta.updated_at,
            });
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
        let slug = slugify(input.slug.as_deref().unwrap_or(&input.title));
        let slug = if slug.is_empty() {
            "item".to_string()
        } else {
            slug
        };
        ensure_safe_component(&slug)?;
        let stamp = compact_now_utc();
        let mut id = format!("{stamp}-{slug}");
        ensure_safe_component(&id)?;
        let mut dir = self.status_dir(TicketStatus::Open).join(&id);
        if dir.exists() {
            id = format!("{id}-{}", std::process::id());
            ensure_safe_component(&id)?;
            dir = self.status_dir(TicketStatus::Open).join(&id);
        }
        if dir.exists() {
            return Err(TicketError::Conflict(format!(
                "target already exists: {}",
                dir.display()
            )));
        }
        ensure_child_of(&self.root, &dir)?;
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
        fields.push(("id".to_string(), format_yaml_string_scalar(&id)));
        fields.push(("slug".to_string(), format_yaml_string_scalar(&slug)));
        fields.push((
            "title".to_string(),
            format_yaml_string_scalar(input.title.as_str()),
        ));
        fields.push(("status".to_string(), format_yaml_string_scalar("open")));
        fields.push((
            "kind".to_string(),
            format_yaml_string_scalar(input.kind.as_str()),
        ));
        fields.push((
            "priority".to_string(),
            format_yaml_string_scalar(input.priority.as_str()),
        ));
        fields.push(("labels".to_string(), labels_yaml(&input.labels)));
        fields.push((
            "workflow_state".to_string(),
            format_yaml_string_scalar(
                input
                    .workflow_state
                    .unwrap_or(TicketWorkflowState::Intake)
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
        fields.push((
            "legacy_ticket".to_string(),
            yaml_string_or_null(input.legacy_ticket.as_deref()),
        ));
        if let Some(readiness) = input.readiness {
            fields.push((
                "readiness".to_string(),
                format_yaml_string_scalar(readiness.as_str()),
            ));
        }
        if let Some(needs_preflight) = input.needs_preflight {
            fields.push(("needs_preflight".to_string(), needs_preflight.to_string()));
        }
        if !input.risk_flags.is_empty() {
            fields.push(("risk_flags".to_string(), labels_yaml(&input.risk_flags)));
        }
        if let Some(action_required) = input.action_required {
            fields.push((
                "action_required".to_string(),
                format_yaml_string_scalar(action_required.as_str()),
            ));
        }
        if let Some(attention_required) = input.attention_required {
            fields.push((
                "attention_required".to_string(),
                format_yaml_string_scalar(attention_required.as_str()),
            ));
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
            id,
            slug,
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
        if field == "workflow_state" {
            return Err(TicketError::Conflict(
                "workflow_state transitions must use dedicated workflow APIs".to_string(),
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
                "workflow_state transition {} -> {} is not allowed through set_workflow_state; use dedicated intake-ready or queue APIs for gated transitions",
                from.as_str(),
                to.as_str()
            )));
        }
        let _lock = self.acquire_lock()?;
        let dir = self.find_ticket_dir(&id)?;
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
        if !TicketWorkflowState::is_intake_ready_transition(from, to) {
            return Err(TicketError::Conflict(format!(
                "mark_intake_ready only allows workflow_state intake -> ready, got {} -> {}",
                from.as_str(),
                to.as_str()
            )));
        }
        let _lock = self.acquire_lock()?;
        let dir = self.find_ticket_dir(&id)?;
        let current = self.ticket_workflow_state_from_item(&dir.join("item.md"))?;
        if current != from {
            return Err(TicketError::Conflict(format!(
                "workflow_state changed concurrently: expected `{}`, found `{}`",
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

    fn set_status(&self, id: TicketIdOrSlug, status: TicketStatus) -> Result<()> {
        let _lock = self.acquire_lock()?;
        self.ensure_backend_dirs()?;
        let old_dir = self.find_ticket_dir(&id)?;
        let item = old_dir.join("item.md");
        let parsed = read_item_file(&item)?;
        let ticket_id =
            parsed.frontmatter.get("id").cloned().ok_or_else(|| {
                TicketError::Conflict(format!("missing id in {}", item.display()))
            })?;
        ensure_safe_component(&ticket_id)?;
        let new_dir = self.status_dir(status).join(&ticket_id);
        ensure_child_of(&self.root, &new_dir)?;
        if old_dir != new_dir {
            if new_dir.exists() {
                return Err(TicketError::Conflict(format!(
                    "target already exists: {}",
                    new_dir.display()
                )));
            }
            fs::rename(&old_dir, &new_dir).map_err(|e| io_err(&new_dir, e))?;
        }
        self.set_frontmatter_fields(&new_dir.join("item.md"), &[("status", status.as_str())])?;
        let author = default_author();
        let body = MarkdownText::new(self.status_changed_body(status));
        self.append_thread_event(
            &new_dir,
            "status_changed",
            self.generated_heading("Status changed", "ステータス変更"),
            &author,
            Some(status.as_str()),
            &[],
            &body,
        )
    }

    fn close(&self, id: TicketIdOrSlug, resolution: MarkdownText) -> Result<()> {
        let _lock = self.acquire_lock()?;
        self.ensure_backend_dirs()?;
        let old_dir = self.find_ticket_dir(&id)?;
        let item = old_dir.join("item.md");
        let parsed = read_item_file(&item)?;
        let ticket_id =
            parsed.frontmatter.get("id").cloned().ok_or_else(|| {
                TicketError::Conflict(format!("missing id in {}", item.display()))
            })?;
        ensure_safe_component(&ticket_id)?;
        let closed_dir = self.status_dir(TicketStatus::Closed).join(&ticket_id);
        ensure_child_of(&self.root, &closed_dir)?;
        if old_dir != closed_dir {
            if closed_dir.exists() {
                return Err(TicketError::Conflict(format!(
                    "target already exists: {}",
                    closed_dir.display()
                )));
            }
            fs::rename(&old_dir, &closed_dir).map_err(|e| io_err(&closed_dir, e))?;
        }
        let at = now_utc();
        let current_workflow_state =
            self.ticket_workflow_state_from_item(&closed_dir.join("item.md"))?;
        if current_workflow_state != TicketWorkflowState::Done {
            let mut change = TicketStateChange::new(
                current_workflow_state.as_str(),
                TicketWorkflowState::Done.as_str(),
                "closed",
                self.closed_workflow_state_body(),
            );
            change.author = Some(default_author());
            self.append_state_changed_event(&closed_dir, &change, Some("workflow_state"))?;
        }
        self.set_frontmatter_fields(
            &closed_dir.join("item.md"),
            &[
                ("status", "closed"),
                ("workflow_state", TicketWorkflowState::Done.as_str()),
                ("updated_at", &at),
            ],
        )?;
        atomic_write(
            &closed_dir.join("resolution.md"),
            resolution.as_str().as_bytes(),
        )?;
        let author = default_author();
        self.append_thread_event(
            &closed_dir,
            "close",
            self.generated_heading("Closed", "完了"),
            &author,
            Some("closed"),
            &[],
            &resolution,
        )
    }

    fn doctor(&self) -> Result<TicketDoctorReport> {
        let mut report = TicketDoctorReport::default();
        for status in STATUSES {
            let dir = self.status_dir(status);
            if !dir.is_dir() {
                report.push_error(format!("missing directory: {}", dir.display()), Some(dir));
            }
        }

        let mut ids: HashMap<String, PathBuf> = HashMap::new();
        let mut duplicate_ids: BTreeSet<String> = BTreeSet::new();
        let mut slugs: HashMap<String, PathBuf> = HashMap::new();
        let mut duplicate_slugs: BTreeSet<String> = BTreeSet::new();

        for status in STATUSES {
            let status_dir = self.status_dir(status);
            if !status_dir.is_dir() {
                continue;
            }
            for entry in fs::read_dir(&status_dir).map_err(|e| io_err(&status_dir, e))? {
                let entry = entry.map_err(|e| io_err(&status_dir, e))?;
                let dir = entry.path();
                if !dir.is_dir() {
                    continue;
                }
                let item = dir.join("item.md");
                let thread = dir.join("thread.md");
                let artifacts = dir.join("artifacts");
                if !item.is_file() {
                    report.push_error(
                        format!("missing item.md: {}", dir.display()),
                        Some(dir.clone()),
                    );
                    continue;
                }
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
                if let Some(id) = parsed.frontmatter.get("id") {
                    if ids.insert(id.clone(), item.clone()).is_some() {
                        duplicate_ids.insert(id.clone());
                    }
                    if dir.file_name().and_then(|name| name.to_str()) != Some(id.as_str()) {
                        report.push_error(
                            format!("directory id mismatch: {} has id {id}", dir.display()),
                            Some(dir.clone()),
                        );
                    }
                }
                if let Some(slug) = parsed.frontmatter.get("slug") {
                    if slugs.insert(slug.clone(), item.clone()).is_some() {
                        duplicate_slugs.insert(slug.clone());
                    }
                }
                let fm_status = parsed
                    .frontmatter
                    .get("status")
                    .map(String::as_str)
                    .unwrap_or("");
                if fm_status != status.as_str() {
                    report.push_error(
                        format!(
                            "status mismatch: {} has '{fm_status}' under '{}'",
                            item.display(),
                            status.as_str()
                        ),
                        Some(item.clone()),
                    );
                }
                match parsed.frontmatter.get("workflow_state").map(String::as_str) {
                    Some(value) if TicketWorkflowState::parse(value).is_none() => report
                        .push_error(
                            format!("invalid workflow_state '{value}': {}", item.display()),
                            Some(item.clone()),
                        ),
                    _ => {}
                }
                if status == TicketStatus::Closed
                    && parsed
                        .frontmatter
                        .get("workflow_state")
                        .is_none_or(|value| value != TicketWorkflowState::Done.as_str())
                {
                    report.push_warning(
                        format!(
                            "closed ticket should have workflow_state: done: {}",
                            item.display()
                        ),
                        Some(item.clone()),
                    );
                }
                if status == TicketStatus::Closed && !dir.join("resolution.md").is_file() {
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
                }
            }
        }
        for duplicate in duplicate_ids {
            report.push_error(format!("duplicate id: {duplicate}"), None);
        }
        for duplicate in duplicate_slugs {
            report.push_error(format!("duplicate slug: {duplicate}"), None);
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
    legacy_ticket: Option<String>,
    readiness: Option<String>,
    needs_preflight: Option<bool>,
    risk_flags: Vec<String>,
    action_required: Option<String>,
    workflow_state: Option<TicketWorkflowState>,
    workflow_state_explicit: bool,
    attention_required: Option<String>,
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
            format!("invalid workflow_state '{value}': expected intake, ready, queued, inprogress, or done")
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
        legacy_ticket: yaml_string(&mapping, "legacy_ticket")?,
        readiness: yaml_string(&mapping, "readiness")?,
        needs_preflight: yaml_bool(&mapping, "needs_preflight")?,
        risk_flags: yaml_string_list(&mapping, "risk_flags")?,
        action_required: yaml_string(&mapping, "action_required")?,
        workflow_state,
        workflow_state_explicit,
        attention_required: yaml_string(&mapping, "attention_required")?,
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

fn yaml_bool(mapping: &YamlMapping, key: &str) -> std::result::Result<Option<bool>, String> {
    match yaml_get(mapping, key) {
        Some(YamlValue::Null) | None => Ok(None),
        Some(YamlValue::Bool(value)) => Ok(Some(*value)),
        Some(value) => Err(format!(
            "frontmatter field `{key}` must be a YAML boolean or null, found {}",
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

fn ticket_meta(frontmatter: TicketItemFrontmatter) -> TicketMeta {
    let status = frontmatter
        .status
        .as_deref()
        .map(ExtensibleTicketStatus::from)
        .unwrap_or_else(|| ExtensibleTicketStatus::Other(String::new()));
    let workflow_state = frontmatter
        .workflow_state
        .unwrap_or_else(|| TicketWorkflowState::default_for_status(&status));
    TicketMeta {
        id: frontmatter.id.unwrap_or_default(),
        slug: frontmatter.slug.unwrap_or_default(),
        title: frontmatter.title.unwrap_or_default(),
        status,
        kind: frontmatter.kind.unwrap_or_default(),
        priority: frontmatter.priority.unwrap_or_default(),
        labels: frontmatter.labels,
        created_at: frontmatter.created_at,
        updated_at: frontmatter.updated_at,
        assignee: frontmatter.assignee,
        legacy_ticket: frontmatter.legacy_ticket,
        readiness: frontmatter.readiness,
        needs_preflight: frontmatter.needs_preflight,
        risk_flags: frontmatter.risk_flags,
        action_required: frontmatter.action_required,
        workflow_state,
        workflow_state_explicit: frontmatter.workflow_state_explicit,
        attention_required: frontmatter.attention_required,
        queued_by: frontmatter.queued_by,
        queued_at: frontmatter.queued_at,
        raw: frontmatter.raw,
    }
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

fn slugify(value: &str) -> String {
    let mut out = String::new();
    let mut previous_dash = false;
    for ch in value.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            previous_dash = false;
        } else if !previous_dash {
            out.push('-');
            previous_dash = true;
        }
    }
    out.trim_matches('-').to_string()
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
    fn parses_item_frontmatter_and_optional_fields() {
        let item = r#"---
id: 20260605-000000-example
slug: example
title: Example
status: open
kind: task
priority: P1
labels: [ticket, backend]
created_at: 2026-06-05T00:00:00Z
updated_at: 2026-06-05T00:00:00Z
assignee: null
legacy_ticket: null
readiness: implementation-ready
needs_preflight: false
risk_flags: [low, local]
action_required: none
workflow_state: ready
attention_required: none
queued_by: workspace-panel
queued_at: 2026-06-05T00:01:00Z
---

## Body
"#;
        let parsed = parse_item(item).unwrap();
        let meta = ticket_meta(parsed.frontmatter);
        assert_eq!(meta.id, "20260605-000000-example");
        assert_eq!(meta.labels, vec!["ticket", "backend"]);
        assert_eq!(meta.readiness.as_deref(), Some("implementation-ready"));
        assert_eq!(meta.needs_preflight, Some(false));
        assert_eq!(meta.risk_flags, vec!["low", "local"]);
        assert_eq!(meta.action_required.as_deref(), Some("none"));
        assert_eq!(meta.workflow_state, TicketWorkflowState::Ready);
        assert!(meta.workflow_state_explicit);
        assert_eq!(meta.attention_required.as_deref(), Some("none"));
        assert_eq!(meta.queued_by.as_deref(), Some("workspace-panel"));
        assert_eq!(meta.queued_at.as_deref(), Some("2026-06-05T00:01:00Z"));
    }

    #[test]
    fn yaml_frontmatter_preserves_typed_nulls_lists_bools_and_quoted_strings() {
        let frontmatter = parse_ticket_frontmatter(
            r#"labels:
  - ticket
  - backend
risk_flags: [low, local]
assignee: ~
legacy_ticket:
attention_required: null
action_required: "null"
readiness: "~"
needs_preflight: false
workflow_state: intake
"#,
        )
        .unwrap();
        let meta = ticket_meta(frontmatter);
        assert_eq!(meta.labels, vec!["ticket", "backend"]);
        assert_eq!(meta.risk_flags, vec!["low", "local"]);
        assert_eq!(meta.assignee, None);
        assert_eq!(meta.legacy_ticket, None);
        assert_eq!(meta.attention_required, None);
        assert_eq!(meta.action_required.as_deref(), Some("null"));
        assert_eq!(meta.readiness.as_deref(), Some("~"));
        assert_eq!(meta.needs_preflight, Some(false));
        assert_eq!(meta.workflow_state, TicketWorkflowState::Intake);
        assert!(meta.workflow_state_explicit);
    }

    #[test]
    fn yaml_frontmatter_rejects_legacy_raw_string_fallbacks() {
        let labels_error = parse_ticket_frontmatter("labels: ticket").unwrap_err();
        assert!(
            labels_error.contains("must be a YAML sequence"),
            "{labels_error}"
        );

        let bool_error = parse_ticket_frontmatter("needs_preflight: 1").unwrap_err();
        assert!(
            bool_error.contains("must be a YAML boolean"),
            "{bool_error}"
        );

        let workflow_error = parse_ticket_frontmatter("workflow_state: almost").unwrap_err();
        assert!(
            workflow_error.contains("invalid workflow_state"),
            "{workflow_error}"
        );
    }

    #[test]
    fn yaml_frontmatter_rejects_invalid_yaml() {
        let err = parse_ticket_frontmatter("labels: [ticket").unwrap_err();
        assert!(err.contains("invalid YAML frontmatter"), "{err}");
    }

    #[test]
    fn create_writes_local_ticket_layout() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let mut input = NewTicket::new("Example Ticket");
        input.labels = vec!["ticket".into(), "backend".into()];
        let ticket = backend.create(input).unwrap();
        let dir = tmp.path().join("tickets/open").join(&ticket.id);
        assert!(dir.join("item.md").exists());
        assert!(dir.join("thread.md").exists());
        assert!(dir.join("artifacts/.gitkeep").exists());
        assert_eq!(ticket.slug, "example-ticket");
        let record = backend.show(TicketIdOrSlug::Id(ticket.id.clone())).unwrap();
        assert_eq!(record.meta.workflow_state, TicketWorkflowState::Intake);
        assert!(record.meta.workflow_state_explicit);
        let report = backend.doctor().unwrap();
        assert!(report.is_ok(), "{:?}", report.diagnostics);
    }

    #[test]
    fn create_uses_configured_japanese_record_language_for_generated_defaults() {
        let tmp = TempDir::new().unwrap();
        let backend = LocalTicketBackend::new(tmp.path().join("tickets"))
            .with_record_language(Some("Japanese"));

        let created = backend.create(NewTicket::new("日本語レコード")).unwrap();
        let dir = backend
            .root()
            .join(TicketStatus::Open.as_str())
            .join(created.id.as_str());
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
        input.slug = Some("numeric-looking-strings".to_string());
        input.labels = vec!["123".into(), "01".into()];
        input.risk_flags = vec!["1".into(), "42".into()];
        input.assignee = Some("42".into());
        input.attention_required = Some("0".into());
        input.action_required = Some("true".into());
        let ticket = backend.create(input).unwrap();

        let record = backend.show(TicketIdOrSlug::Id(ticket.id.clone())).unwrap();
        assert_eq!(record.meta.title, "123");
        assert_eq!(record.meta.labels, vec!["123", "01"]);
        assert_eq!(record.meta.risk_flags, vec!["1", "42"]);
        assert_eq!(record.meta.assignee.as_deref(), Some("42"));
        assert_eq!(record.meta.attention_required.as_deref(), Some("0"));
        assert_eq!(record.meta.action_required.as_deref(), Some("true"));

        let item = fs::read_to_string(
            tmp.path()
                .join("tickets/open")
                .join(&ticket.id)
                .join("item.md"),
        )
        .unwrap();
        assert!(item.contains("title: '123'"), "{item}");
        assert!(item.contains("labels: ['123', '01']"), "{item}");
        assert!(item.contains("risk_flags: ['1', '42']"), "{item}");
        assert!(item.contains("assignee: '42'"), "{item}");
        assert!(item.contains("attention_required: '0'"), "{item}");
        assert!(item.contains("action_required: 'true'"), "{item}");

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
                TicketIdOrSlug::Slug(ticket.slug.clone()),
                NewTicketEvent::new(TicketEventKind::Plan, "Implementation plan."),
            )
            .unwrap();
        backend
            .review(
                TicketIdOrSlug::Id(ticket.id.clone()),
                TicketReview::approve("Looks good."),
            )
            .unwrap();
        backend
            .set_status(TicketIdOrSlug::Id(ticket.id.clone()), TicketStatus::Pending)
            .unwrap();
        let pending_item = tmp
            .path()
            .join("tickets/pending")
            .join(&ticket.id)
            .join("item.md");
        assert!(pending_item.exists());
        backend
            .close(
                TicketIdOrSlug::Id(ticket.id.clone()),
                MarkdownText::new("Done.\n"),
            )
            .unwrap();
        let closed_dir = tmp.path().join("tickets/closed").join(&ticket.id);
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
            .join("tickets/open")
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
        input.slug = Some("invalid-author-ticket".into());
        input.author = Some("bad-->author".into());

        assert!(matches!(
            backend.create(input),
            Err(TicketError::Conflict(_))
        ));
        let open_dir = tmp.path().join("tickets/open");
        let entries = fs::read_dir(open_dir).unwrap().count();
        assert_eq!(entries, 0);
    }

    #[test]
    fn state_changed_and_intake_summary_events_round_trip() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let ticket = backend
            .create(NewTicket::new("Typed Thread Ticket"))
            .unwrap();
        let mut change = TicketStateChange::new(
            "preflight",
            "implementation-ready",
            "preflight approved",
            "Preflight finished; implementation can begin.",
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
        assert_eq!(state_event.from.as_deref(), Some("preflight"));
        assert_eq!(state_event.to.as_deref(), Some("implementation-ready"));
        assert_eq!(state_event.reason.as_deref(), Some("preflight approved"));
        assert_eq!(state_event.author.as_deref(), Some("orchestrator"));
        assert_eq!(
            state_event.attributes.get("reason").map(String::as_str),
            Some("preflight approved")
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
                .join("tickets/open")
                .join(&ticket.id)
                .join("thread.md"),
        )
        .unwrap();
        assert!(thread.contains("event: state_changed"));
        assert!(thread.contains("reason: \"preflight approved\""));
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
        let item = tmp
            .path()
            .join("tickets/open")
            .join(&ticket.id)
            .join("item.md");
        backend
            .set_frontmatter_fields(&item, &[("readiness", "preflight")])
            .unwrap();

        let mut change = TicketStateChange::new(
            "preflight",
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
            "preflight",
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
    fn workflow_state_defaults_and_queue_transition_round_trip() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let missing_meta = ticket_meta(
            parse_ticket_frontmatter("status: open").expect("missing workflow state parses"),
        );
        assert_eq!(missing_meta.workflow_state, TicketWorkflowState::Intake);
        assert!(!missing_meta.workflow_state_explicit);

        let closed_meta =
            ticket_meta(parse_ticket_frontmatter("status: closed").expect("closed default parses"));
        assert_eq!(closed_meta.workflow_state, TicketWorkflowState::Done);
        assert!(!closed_meta.workflow_state_explicit);

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
        assert_eq!(event.state_field.as_deref(), Some("workflow_state"));
        assert_eq!(event.from.as_deref(), Some("ready"));
        assert_eq!(event.to.as_deref(), Some("queued"));
        assert_eq!(event.reason.as_deref(), Some("queued"));
    }

    #[test]
    fn workflow_queue_rejects_non_ready_ticket_without_mutation() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let ticket = backend.create(NewTicket::new("Intake Ticket")).unwrap();

        assert!(matches!(
            backend.queue_ready(TicketIdOrSlug::Id(ticket.id.clone()), "workspace-panel"),
            Err(TicketError::Conflict(_))
        ));
        let record = backend.show(TicketIdOrSlug::Id(ticket.id)).unwrap();
        assert_eq!(record.meta.workflow_state, TicketWorkflowState::Intake);
        assert!(record.meta.queued_by.is_none());
        assert!(
            !record
                .events
                .iter()
                .any(|event| event.kind == TicketEventKind::StateChanged)
        );
    }

    #[test]
    fn workflow_state_cannot_be_changed_through_generic_state_field_api() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let ticket = backend
            .create(NewTicket::new("Generic Workflow Bypass"))
            .unwrap();
        let change = TicketStateChange::new(
            "intake",
            "done",
            "bypass",
            "Generic state field API must not mutate workflow_state.",
        );

        assert!(matches!(
            backend.set_state_field(
                TicketIdOrSlug::Id(ticket.id.clone()),
                "workflow_state",
                change
            ),
            Err(TicketError::Conflict(_))
        ));
        let record = backend.show(TicketIdOrSlug::Id(ticket.id)).unwrap();
        assert_eq!(record.meta.workflow_state, TicketWorkflowState::Intake);
    }

    #[test]
    fn mark_intake_ready_records_summary_and_state_change() {
        let tmp = TempDir::new().unwrap();
        let backend = backend(&tmp);
        let ticket = backend.create(NewTicket::new("Intake Ready")).unwrap();
        let mut summary = TicketIntakeSummary::new("Concise accepted requirements.");
        summary.author = Some("intake".to_string());
        let mut change =
            TicketStateChange::new("intake", "ready", "accepted", "Ticket is ready to queue.");
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
                && event.state_field.as_deref() == Some("workflow_state")
                && event.from.as_deref() == Some("intake")
                && event.to.as_deref() == Some("ready")
        }));
    }

    #[test]
    fn close_sets_workflow_state_done() {
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
        assert_eq!(record.meta.workflow_state, TicketWorkflowState::Done);
        assert!(record.events.iter().any(|event| {
            event.kind == TicketEventKind::StateChanged
                && event.state_field.as_deref() == Some("workflow_state")
                && event.to.as_deref() == Some("done")
        }));
    }

    #[test]
    fn doctor_reports_invalid_workflow_state() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("tickets");
        fs::create_dir_all(root.join("open/bad/artifacts")).unwrap();
        fs::write(
            root.join("open/bad/item.md"),
            "---\nid: bad\nslug: bad\ntitle: Bad\nstatus: open\nkind: task\npriority: P2\nworkflow_state: almost\nlabels: []\ncreated_at: x\nupdated_at: x\nassignee: null\nlegacy_ticket: null\n---\n",
        )
        .unwrap();
        fs::write(root.join("open/bad/thread.md"), "").unwrap();
        fs::create_dir_all(root.join("pending")).unwrap();
        fs::create_dir_all(root.join("closed")).unwrap();

        let report = LocalTicketBackend::new(&root).doctor().unwrap();
        let messages = report
            .diagnostics
            .iter()
            .map(|d| d.message.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!report.is_ok());
        assert!(messages.contains("invalid workflow_state"));
    }

    #[test]
    fn doctor_validates_typed_thread_event_attributes() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("tickets");
        fs::create_dir_all(root.join("open/bad/artifacts")).unwrap();
        fs::write(
            root.join("open/bad/item.md"),
            "---\nid: bad\nslug: bad\ntitle: Bad\nstatus: open\nkind: task\npriority: P2\nlabels: []\ncreated_at: x\nupdated_at: x\nassignee: null\nlegacy_ticket: null\n---\n",
        )
        .unwrap();
        fs::write(
            root.join("open/bad/thread.md"),
            "<!-- event: state_changed author: bot at: now from: queued -->\n\n## State changed\n\n---\n\n<!-- event: intake_summary author: bot at: now -->\n\n## Intake summary\n\n---\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("pending")).unwrap();
        fs::create_dir_all(root.join("closed")).unwrap();
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
        fs::create_dir_all(root.join("open/bad/artifacts")).unwrap();
        fs::write(
            root.join("open/bad/item.md"),
            "---\nid: other\nslug: dup\ntitle: Bad\nstatus: pending\nkind: task\npriority: P2\nlabels: []\ncreated_at: x\nupdated_at: x\nassignee: null\nlegacy_ticket: null\n---\n",
        )
        .unwrap();
        fs::write(
            root.join("open/bad/thread.md"),
            "<!-- event: review author: a at: now -->\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("pending/other/artifacts")).unwrap();
        fs::write(
            root.join("pending/other/item.md"),
            "---\nid: other\nslug: dup\ntitle: Dup\nstatus: pending\nkind: task\npriority: P2\nlabels: []\ncreated_at: x\nupdated_at: x\nassignee: null\nlegacy_ticket: null\n---\n",
        )
        .unwrap();
        fs::write(root.join("pending/other/thread.md"), "").unwrap();
        fs::create_dir_all(root.join("closed")).unwrap();
        let report = LocalTicketBackend::new(&root).doctor().unwrap();
        let messages = report
            .diagnostics
            .iter()
            .map(|d| d.message.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!report.is_ok());
        assert!(messages.contains("directory id mismatch"));
        assert!(messages.contains("status mismatch"));
        assert!(messages.contains("duplicate id: other"));
        assert!(messages.contains("duplicate slug: dup"));
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
    fn rejects_unsafe_components_for_status_moves() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("tickets");
        fs::create_dir_all(root.join("open/bad/artifacts")).unwrap();
        fs::write(
            root.join("open/bad/item.md"),
            "---\nid: ../bad\nslug: bad\ntitle: Bad\nstatus: open\nkind: task\npriority: P2\nlabels: []\ncreated_at: x\nupdated_at: x\nassignee: null\nlegacy_ticket: null\n---\n",
        )
        .unwrap();
        fs::write(root.join("open/bad/thread.md"), "").unwrap();
        fs::create_dir_all(root.join("pending")).unwrap();
        fs::create_dir_all(root.join("closed")).unwrap();
        let err = LocalTicketBackend::new(&root)
            .set_status(TicketIdOrSlug::Slug("bad".into()), TicketStatus::Pending)
            .unwrap_err();
        assert!(matches!(err, TicketError::InvalidPathComponent(_)));
    }
}
