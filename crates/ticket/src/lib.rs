//! Ticket domain types and the local `.yoi/tickets/` file backend.
//!
//! The public domain name is **Ticket**. `LocalTicketBackend` preserves the
//! repository's current `.yoi/tickets/{open,pending,closed}/<id>/` layout and the
//! event format used by `tickets.sh` while exposing typed Rust operations.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

use chrono::Utc;
use fs4::fs_std::FileExt;
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
}

impl NewTicket {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            slug: None,
            kind: "task".to_string(),
            priority: "P2".to_string(),
            labels: Vec::new(),
            body: MarkdownText::new(
                "## Background\n\nCreated by LocalTicketBackend.\n\n## Acceptance criteria\n\n- TBD\n",
            ),
            author: None,
            assignee: None,
            legacy_ticket: None,
            readiness: None,
            needs_preflight: None,
            risk_flags: Vec::new(),
            action_required: None,
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
    pub heading: Option<String>,
    pub body: MarkdownText,
    pub references: Vec<TicketReference>,
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
    fn review(&self, id: TicketIdOrSlug, review: TicketReview) -> Result<()>;
    fn set_status(&self, id: TicketIdOrSlug, status: TicketStatus) -> Result<()>;
    fn close(&self, id: TicketIdOrSlug, resolution: MarkdownText) -> Result<()>;
    fn doctor(&self) -> Result<TicketDoctorReport>;
}

#[derive(Debug, Clone)]
pub struct LocalTicketBackend {
    root: PathBuf,
}

impl LocalTicketBackend {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        self.root.as_path()
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
            raw_frontmatter: parsed.frontmatter,
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

    fn append_thread_event(
        &self,
        dir: &Path,
        event: &str,
        heading: &str,
        author: &str,
        status: Option<&str>,
        body: &MarkdownText,
    ) -> Result<()> {
        let at = now_utc();
        let thread = dir.join("thread.md");
        if !thread.exists() {
            File::create(&thread).map_err(|e| io_err(&thread, e))?;
        }
        let mut file = OpenOptions::new()
            .append(true)
            .open(&thread)
            .map_err(|e| io_err(&thread, e))?;
        write!(file, "\n<!-- event: {event} author: {author} at: {at}")
            .map_err(|e| io_err(&thread, e))?;
        if let Some(status) = status {
            write!(file, " status: {status}").map_err(|e| io_err(&thread, e))?;
        }
        write!(file, " -->\n\n## {heading}\n\n{}\n\n---\n", body.as_str())
            .map_err(|e| io_err(&thread, e))?;
        file.sync_data().map_err(|e| io_err(&thread, e))?;
        self.set_frontmatter_fields(&dir.join("item.md"), &[("updated_at", at.as_str())])
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
        fs::create_dir_all(dir.join("artifacts")).map_err(|e| io_err(&dir, e))?;
        atomic_write(&dir.join("artifacts/.gitkeep"), b"")?;
        let created = now_utc();
        let mut fields = Vec::new();
        fields.push(("id".to_string(), id.clone()));
        fields.push(("slug".to_string(), slug.clone()));
        fields.push(("title".to_string(), input.title));
        fields.push(("status".to_string(), "open".to_string()));
        fields.push(("kind".to_string(), input.kind));
        fields.push(("priority".to_string(), input.priority));
        fields.push(("labels".to_string(), labels_yaml(&input.labels)));
        fields.push(("created_at".to_string(), created.clone()));
        fields.push(("updated_at".to_string(), created.clone()));
        fields.push((
            "assignee".to_string(),
            input.assignee.unwrap_or_else(|| "null".to_string()),
        ));
        fields.push((
            "legacy_ticket".to_string(),
            input.legacy_ticket.unwrap_or_else(|| "null".to_string()),
        ));
        if let Some(readiness) = input.readiness {
            fields.push(("readiness".to_string(), readiness));
        }
        if let Some(needs_preflight) = input.needs_preflight {
            fields.push(("needs_preflight".to_string(), needs_preflight.to_string()));
        }
        if !input.risk_flags.is_empty() {
            fields.push(("risk_flags".to_string(), labels_yaml(&input.risk_flags)));
        }
        if let Some(action_required) = input.action_required {
            fields.push(("action_required".to_string(), action_required));
        }
        let item = serialize_item(&fields, input.body.as_str());
        atomic_write(&dir.join("item.md"), item.as_bytes())?;
        let author = input
            .author
            .unwrap_or_else(|| "LocalTicketBackend".to_string());
        let thread = format!(
            "<!-- event: create author: {author} at: {created} -->\n\n## Created\n\nCreated by LocalTicketBackend create.\n\n---\n"
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
            &event.body,
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
        let body = MarkdownText::new(format!("Status changed to `{}`.\n", status.as_str()));
        self.append_thread_event(
            &new_dir,
            "status_changed",
            "Status changed",
            &author,
            Some(status.as_str()),
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
        self.set_frontmatter_fields(
            &closed_dir.join("item.md"),
            &[("status", "closed"), ("updated_at", &at)],
        )?;
        atomic_write(
            &closed_dir.join("resolution.md"),
            resolution.as_str().as_bytes(),
        )?;
        let author = default_author();
        self.append_thread_event(
            &closed_dir,
            "close",
            "Closed",
            &author,
            Some("closed"),
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
    frontmatter: BTreeMap<String, String>,
    body: String,
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
    let mut frontmatter = BTreeMap::new();
    let mut found_close = false;
    let mut body = String::new();
    for line in &mut lines {
        if line == "---" {
            found_close = true;
            break;
        }
        if let Some((key, value)) = line.split_once(':') {
            frontmatter.insert(key.trim().to_string(), value.trim_start().to_string());
        }
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
    Ok(ParsedItem { frontmatter, body })
}

fn ticket_meta(frontmatter: BTreeMap<String, String>) -> TicketMeta {
    let id = frontmatter.get("id").cloned().unwrap_or_default();
    let slug = frontmatter.get("slug").cloned().unwrap_or_default();
    let title = frontmatter.get("title").cloned().unwrap_or_default();
    let status = frontmatter
        .get("status")
        .map(|value| ExtensibleTicketStatus::from(value.as_str()))
        .unwrap_or_else(|| ExtensibleTicketStatus::Other(String::new()));
    let kind = frontmatter.get("kind").cloned().unwrap_or_default();
    let priority = frontmatter.get("priority").cloned().unwrap_or_default();
    let labels = frontmatter
        .get("labels")
        .map(|value| parse_yaml_list(value))
        .unwrap_or_default();
    let risk_flags = frontmatter
        .get("risk_flags")
        .or_else(|| frontmatter.get("risks"))
        .map(|value| parse_yaml_list(value))
        .unwrap_or_default();
    TicketMeta {
        id,
        slug,
        title,
        status,
        kind,
        priority,
        labels,
        created_at: frontmatter.get("created_at").cloned(),
        updated_at: frontmatter.get("updated_at").cloned(),
        assignee: frontmatter.get("assignee").cloned().filter(|v| v != "null"),
        legacy_ticket: frontmatter
            .get("legacy_ticket")
            .cloned()
            .filter(|v| v != "null"),
        readiness: frontmatter.get("readiness").cloned(),
        needs_preflight: frontmatter
            .get("needs_preflight")
            .or_else(|| frontmatter.get("needs-preflight"))
            .and_then(|value| parse_bool(value)),
        risk_flags,
        action_required: frontmatter.get("action_required").cloned(),
        raw: frontmatter,
    }
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim() {
        "true" | "yes" | "1" => Some(true),
        "false" | "no" | "0" => Some(false),
        _ => None,
    }
}

fn parse_yaml_list(value: &str) -> Vec<String> {
    let trimmed = value.trim();
    if let Some(inner) = trimmed.strip_prefix('[').and_then(|v| v.strip_suffix(']')) {
        return inner
            .split(',')
            .map(|part| part.trim().trim_matches('"').trim_matches('\''))
            .filter(|part| !part.is_empty())
            .map(ToOwned::to_owned)
            .collect();
    }
    if trimmed.is_empty() || trimmed == "null" {
        Vec::new()
    } else {
        vec![trimmed.to_string()]
    }
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
                *line = format!("{key}: {value}");
                seen.insert(key);
            }
        }
    }
    let mut insert_at = end;
    for (key, value) in updates {
        if !seen.contains(*key) {
            lines.insert(insert_at, format!("{key}: {value}"));
            insert_at += 1;
        }
    }
    let mut out = lines.join("\n");
    if content.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
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
                heading,
                body: MarkdownText::new(body),
                references: Vec::new(),
            });
        } else {
            idx += 1;
        }
    }
    Ok(events)
}

fn parse_event_comment(comment: &str) -> BTreeMap<String, String> {
    // tickets.sh emits unquoted `key: value` pairs separated by spaces. Values
    // currently do not contain spaces; this parser intentionally preserves the
    // compatibility shape instead of treating thread.md as strict YAML.
    let mut attrs = BTreeMap::new();
    let mut iter = comment.split_whitespace().peekable();
    while let Some(token) = iter.next() {
        if let Some(key) = token.strip_suffix(':') {
            if let Some(value) = iter.next() {
                attrs.insert(key.to_string(), value.to_string());
            }
        }
    }
    attrs
}

fn doctor_thread_events(path: &Path, report: &mut TicketDoctorReport) -> Result<()> {
    let content = fs::read_to_string(path).map_err(|e| io_err(path, e))?;
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
            if attrs.contains_key("event") && attrs.get("at").is_none() {
                report.push_error(
                    format!(
                        "thread event missing at: {}:{}",
                        path.display(),
                        line_no + 1
                    ),
                    Some(path.to_path_buf()),
                );
            }
            if attrs.get("event").map(String::as_str) == Some("review") {
                match attrs.get("status").map(String::as_str) {
                    Some("approve" | "request_changes") => {}
                    _ => report.push_warning(
                        format!(
                            "legacy review event missing valid status at {}:{}",
                            path.display(),
                            line_no + 1
                        ),
                        Some(path.to_path_buf()),
                    ),
                }
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

    fn script_path() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..//tickets.sh")
    }

    fn run_tickets_sh(work_items: &Path, args: &[&str]) -> std::process::Output {
        std::process::Command::new(script_path())
            .current_dir(work_items.parent().unwrap())
            .env("WORK_ITEMS_DIR", work_items)
            .args(args)
            .output()
            .unwrap()
    }

    fn assert_script_ok(work_items: &Path, args: &[&str]) -> String {
        let output = run_tickets_sh(work_items, args);
        assert!(
            output.status.success(),
            "tickets.sh {:?} failed\nstdout:\n{}\nstderr:\n{}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
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
    }

    #[test]
    fn create_writes_tickets_sh_compatible_layout() {
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
        assert_script_ok(&tmp.path().join("tickets"), &["doctor"]);
    }

    #[test]
    fn add_event_review_status_and_close_are_script_compatible() {
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
        assert_script_ok(&tmp.path().join("tickets"), &["doctor"]);
    }

    #[test]
    fn reads_ticket_created_by_tickets_sh_and_script_mutates_rust_ticket() {
        let tmp = TempDir::new().unwrap();
        let work_items = tmp.path().join("tickets");
        let id = assert_script_ok(
            &work_items,
            &[
                "create",
                "--title",
                "Shell Created",
                "--slug",
                "shell-created",
            ],
        );
        let id = id.trim();
        let backend = LocalTicketBackend::new(&work_items);
        let ticket = backend.show(TicketIdOrSlug::Id(id.to_string())).unwrap();
        assert_eq!(ticket.meta.slug, "shell-created");
        let rust_ticket = backend.create(NewTicket::new("Rust Created")).unwrap();
        assert_script_ok(
            &work_items,
            &["comment", &rust_ticket.slug, "--author", "test"],
        );
        let shown = backend.show(TicketIdOrSlug::Id(rust_ticket.id)).unwrap();
        assert!(
            shown
                .events
                .iter()
                .any(|event| event.kind == TicketEventKind::Comment)
        );
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
