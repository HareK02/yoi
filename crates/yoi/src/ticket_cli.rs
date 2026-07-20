use std::fmt;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use ticket::config::{
    DEFAULT_TICKET_BACKEND_RELATIVE_PATH, TICKET_CONFIG_RELATIVE_PATH, TicketConfig,
    WORKSPACE_SETTINGS_RELATIVE_PATH, ticket_config_scaffold,
};
use ticket::{
    LocalTicketBackend, MarkdownText, NewTicket, NewTicketEvent, NewTicketRelation, TicketBackend,
    TicketDoctorSeverity, TicketEventKind, TicketIdOrSlug, TicketIntakeSummary, TicketListQuery,
    TicketRelationKind, TicketReview, TicketReviewResult, TicketSummary, TicketWorkflowState,
};

const DEFAULT_LIST_LIMIT: usize = 50;
const MAX_LIST_LIMIT: usize = 100;
const LIST_TITLE_MAX_CHARS: usize = 96;
const LIST_HINT_MAX_CHARS: usize = 80;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TicketCli {
    Help,
    Command(TicketCommand),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TicketCommand {
    Init,
    Create(CreateOptions),
    List(ListOptions),
    Show { query: String },
    Comment(CommentOptions),
    Review(ReviewOptions),
    State(StateOptions),
    Close(CloseOptions),
    Relation(RelationOptions),
    Doctor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateOptions {
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListState {
    Active,
    All,
    States(Vec<TicketWorkflowState>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListOptions {
    pub state: ListState,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentOptions {
    pub query: String,
    pub role: TicketEventKind,
    pub body: BodySource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewOptions {
    pub query: String,
    pub result: TicketReviewResult,
    pub body: BodySource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateTarget {
    Planning,
    Ready,
    Queued,
    InProgress,
    Done,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateOptions {
    pub query: String,
    pub state: StateTarget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseOptions {
    pub query: String,
    pub resolution: BodySource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelationAction {
    Add {
        ticket: String,
        kind: TicketRelationKind,
        target: String,
        note: Option<String>,
    },
    List {
        ticket: Option<String>,
        kind: Option<TicketRelationKind>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationOptions {
    pub action: RelationAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodySource {
    Message(String),
    File(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TicketCliStatus {
    Success,
    Failure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketCliOutput {
    pub status: TicketCliStatus,
    pub stdout: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketCliError(String);

impl TicketCliError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for TicketCliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TicketCliError {}

impl From<ticket::TicketError> for TicketCliError {
    fn from(error: ticket::TicketError) -> Self {
        Self::new(error.to_string())
    }
}

impl From<ticket::config::TicketConfigError> for TicketCliError {
    fn from(error: ticket::config::TicketConfigError) -> Self {
        Self::new(error.to_string())
    }
}

impl From<std::io::Error> for TicketCliError {
    fn from(error: std::io::Error) -> Self {
        Self::new(error.to_string())
    }
}

pub fn parse_ticket_args(args: &[String]) -> Result<TicketCli, TicketCliError> {
    if args.is_empty() || args.iter().any(|arg| arg == "--help" || arg == "-h") {
        return Ok(TicketCli::Help);
    }

    let command = match args[0].as_str() {
        "init" => {
            if args.len() != 1 {
                return Err(TicketCliError::new("ticket init takes no arguments"));
            }
            TicketCommand::Init
        }
        "create" => TicketCommand::Create(parse_create(&args[1..])?),
        "list" => TicketCommand::List(parse_list(&args[1..])?),
        "show" => TicketCommand::Show {
            query: parse_one_positional("show", &args[1..])?,
        },
        "comment" => TicketCommand::Comment(parse_comment(&args[1..])?),
        "review" => TicketCommand::Review(parse_review(&args[1..])?),
        "state" => TicketCommand::State(parse_state(&args[1..])?),
        "close" => TicketCommand::Close(parse_close(&args[1..])?),
        "relation" => TicketCommand::Relation(parse_relation(&args[1..])?),
        "doctor" => {
            if args.len() != 1 {
                return Err(TicketCliError::new("ticket doctor takes no arguments"));
            }
            TicketCommand::Doctor
        }
        "help" => return Ok(TicketCli::Help),
        other => {
            return Err(TicketCliError::new(format!(
                "unknown ticket command: {other}"
            )));
        }
    };

    Ok(TicketCli::Command(command))
}

pub fn run(cli: TicketCli) -> Result<TicketCliOutput, TicketCliError> {
    let workspace = std::env::current_dir().map_err(|error| {
        TicketCliError::new(format!("failed to resolve current directory: {error}"))
    })?;
    run_in_workspace(cli, &workspace)
}

pub fn run_in_workspace(
    cli: TicketCli,
    workspace: &Path,
) -> Result<TicketCliOutput, TicketCliError> {
    match cli {
        TicketCli::Help => Ok(TicketCliOutput {
            status: TicketCliStatus::Success,
            stdout: help_text().to_string(),
        }),
        TicketCli::Command(command) => run_command(command, workspace),
    }
}

fn run_command(
    command: TicketCommand,
    workspace: &Path,
) -> Result<TicketCliOutput, TicketCliError> {
    match command {
        TicketCommand::Init => init(workspace),
        command => {
            let backend = backend_for_workspace(workspace)?;
            match command {
                TicketCommand::Create(options) => create(&backend, options),
                TicketCommand::List(options) => list(&backend, options),
                TicketCommand::Show { query } => show(&backend, query),
                TicketCommand::Comment(options) => comment(&backend, options),
                TicketCommand::Review(options) => review(&backend, options),
                TicketCommand::State(options) => state(&backend, options),
                TicketCommand::Close(options) => close(&backend, options),
                TicketCommand::Relation(options) => relation(&backend, options),
                TicketCommand::Doctor => doctor(&backend),
                TicketCommand::Init => unreachable!("init handled before backend setup"),
            }
        }
    }
}

fn init(workspace: &Path) -> Result<TicketCliOutput, TicketCliError> {
    let legacy_config_path = workspace.join(TICKET_CONFIG_RELATIVE_PATH);
    if legacy_config_path.exists() {
        return Err(TicketCliError::new(format!(
            "legacy ticket config exists at {}; `.yoi/ticket.config.toml` is obsolete and read-only. Move its policy into {} before running `yoi ticket init`.",
            legacy_config_path.display(),
            WORKSPACE_SETTINGS_RELATIVE_PATH
        )));
    }

    let yoi_dir = workspace.join(".yoi");
    fs::create_dir_all(&yoi_dir)?;
    let tickets_dir = workspace.join(DEFAULT_TICKET_BACKEND_RELATIVE_PATH);
    fs::create_dir_all(&tickets_dir)?;
    fs::write(tickets_dir.join(".gitkeep"), b"")?;

    let settings_path = workspace.join(WORKSPACE_SETTINGS_RELATIVE_PATH);
    let scaffold = ticket_config_scaffold();
    let created_settings = if settings_path.exists() {
        let content = fs::read_to_string(&settings_path)?;
        if TicketConfig::workspace_settings_has_ticket_config(&settings_path, &content)? {
            return Err(TicketCliError::new(format!(
                "workspace Ticket settings already exist at {}; refusing to overwrite. Edit the [ticket] table manually before running `yoi ticket init`.",
                settings_path.display()
            )));
        }
        let mut file = fs::OpenOptions::new().append(true).open(&settings_path)?;
        if !content.ends_with('\n') {
            file.write_all(b"\n")?;
        }
        file.write_all(b"\n")?;
        file.write_all(scaffold.as_bytes())?;
        false
    } else {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&settings_path)
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    TicketCliError::new(format!(
                        "workspace settings already exists at {}; retry `yoi ticket init` to append Ticket settings safely.",
                        settings_path.display()
                    ))
                } else {
                    TicketCliError::from(error)
                }
            })?;
        let identity = workspace_settings_identity_header(workspace)?;
        file.write_all(identity.as_bytes())?;
        file.write_all(b"\n")?;
        file.write_all(scaffold.as_bytes())?;
        true
    };

    let verb = if created_settings {
        "created"
    } else {
        "updated"
    };
    Ok(success(format!(
        "{verb}\t{}\nensured\t{}\n",
        WORKSPACE_SETTINGS_RELATIVE_PATH, DEFAULT_TICKET_BACKEND_RELATIVE_PATH
    )))
}

fn workspace_settings_identity_header(workspace: &Path) -> Result<String, TicketCliError> {
    let display_name = workspace
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("workspace");
    if display_name.contains('\0') || display_name.chars().any(|ch| ch.is_control()) {
        return Err(TicketCliError::new(
            "workspace display name derived from path must not contain control characters",
        ));
    }
    Ok(format!(
        "workspace_id = \"{}\"\ncreated_at = \"{}\"\ndisplay_name = \"{}\"\n",
        workspace_settings_uuid_v7(workspace),
        Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        display_name.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

fn workspace_settings_uuid_v7(workspace: &Path) -> String {
    let now = Utc::now();
    let timestamp_ms = (now.timestamp_millis().max(0) as u64) & 0xffff_ffff_ffff;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    workspace.hash(&mut hasher);
    std::process::id().hash(&mut hasher);
    now.timestamp_nanos_opt()
        .unwrap_or_default()
        .hash(&mut hasher);
    let random = hasher.finish();

    let time_low = (timestamp_ms >> 16) as u32;
    let time_mid = (timestamp_ms & 0xffff) as u16;
    let version_and_rand = 0x7000 | (((random >> 52) as u16) & 0x0fff);
    let variant_and_rand = 0x8000 | (((random >> 38) as u16) & 0x3fff);
    let node = random & 0xffff_ffff_ffff;
    format!(
        "{time_low:08x}-{time_mid:04x}-{version_and_rand:04x}-{variant_and_rand:04x}-{node:012x}"
    )
}

fn backend_for_workspace(workspace: &Path) -> Result<LocalTicketBackend, TicketCliError> {
    let config = TicketConfig::load_workspace(workspace)?;
    Ok(LocalTicketBackend::new(config.backend_root().to_path_buf())
        .with_record_language(config.ticket_record_language()))
}

fn create(
    backend: &LocalTicketBackend,
    options: CreateOptions,
) -> Result<TicketCliOutput, TicketCliError> {
    let mut input = NewTicket::new(options.title);
    input.author = Some("yoi ticket".to_string());

    let created = backend.create(input)?;
    Ok(success(format!("created\t{}\n", created.id)))
}

fn list(
    backend: &LocalTicketBackend,
    options: ListOptions,
) -> Result<TicketCliOutput, TicketCliError> {
    let filter = match options.state {
        ListState::Active => TicketListQuery::active(),
        ListState::All => TicketListQuery::all(),
        ListState::States(states) => TicketListQuery::states(states),
    };
    let tickets = backend.list(filter)?;
    let count = tickets.len();
    let limit = options
        .limit
        .unwrap_or(DEFAULT_LIST_LIMIT)
        .min(MAX_LIST_LIMIT);
    let mut stdout = String::from("state\tid\ttitle\tupdated_at\thints\n");
    for ticket in tickets.into_iter().take(limit) {
        let title = truncate_inline(ticket.title.as_str(), LIST_TITLE_MAX_CHARS);
        let updated_at = ticket.updated_at.as_deref().unwrap_or_default();
        let hints = ticket_cli_hints(&ticket);
        stdout.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\n",
            ticket.workflow_state.as_str(),
            ticket.id.as_str(),
            title,
            updated_at,
            hints
        ));
    }
    if count > limit {
        stdout.push_str(&format!(
            "# truncated: returned {limit} of {count}; use --limit up to {MAX_LIST_LIMIT} or a narrower --state, then yoi ticket show <id> for details\n"
        ));
    }
    Ok(success(stdout))
}

fn show(backend: &LocalTicketBackend, query: String) -> Result<TicketCliOutput, TicketCliError> {
    let ticket = backend.show(TicketIdOrSlug::Query(query))?;
    let mut stdout = String::new();
    stdout.push_str(&format!("# {}\n\n", ticket.meta.title));
    stdout.push_str(&format!("State: {}\n", ticket.meta.workflow_state.as_str()));
    stdout.push_str(&format!("ID: {}\n", ticket.meta.id));
    if let Some(updated_at) = &ticket.meta.updated_at {
        stdout.push_str(&format!("Updated: {updated_at}\n"));
    }

    stdout.push_str("\n## item.md\n\n---\n");
    for (key, value) in &ticket.document.raw_frontmatter {
        if is_obsolete_ticket_frontmatter_key(key) {
            continue;
        }
        stdout.push_str(&format!("{key}: {value}\n"));
    }
    stdout.push_str("---\n\n");
    stdout.push_str(ticket.document.body.as_str());
    if !stdout.ends_with('\n') {
        stdout.push('\n');
    }

    stdout.push_str("\n## thread.md\n\n");
    if ticket.events.is_empty() {
        stdout.push_str("(no events)\n");
    } else {
        for event in &ticket.events {
            stdout.push_str(&format!(
                "- {}{}{}{}\n",
                event.kind.as_str(),
                event
                    .status
                    .as_ref()
                    .map(|status| format!(" [{status}]"))
                    .unwrap_or_default(),
                event
                    .author
                    .as_ref()
                    .map(|author| format!(" by {author}"))
                    .unwrap_or_default(),
                event
                    .at
                    .as_ref()
                    .map(|at| format!(" at {at}"))
                    .unwrap_or_default()
            ));
            if let Some(heading) = &event.heading {
                stdout.push_str(&format!("  ## {heading}\n"));
            }
            if !event.body.as_str().is_empty() {
                stdout.push_str(event.body.as_str());
                if !stdout.ends_with('\n') {
                    stdout.push('\n');
                }
            }
        }
    }

    if !ticket.relations.outgoing.is_empty()
        || !ticket.relations.incoming.is_empty()
        || !ticket.relations.blockers.is_empty()
        || !ticket.relations.notices.is_empty()
    {
        stdout.push_str("\n## relations\n\n");
        if !ticket.relations.outgoing.is_empty() {
            stdout.push_str("### outgoing\n\n");
            for relation in &ticket.relations.outgoing {
                stdout.push_str(&format!("- {} {}", relation.kind.as_str(), relation.target));
                if let Some(note) = &relation.note {
                    stdout.push_str(&format!(" — {}", note.replace('\n', " ")));
                }
                stdout.push('\n');
            }
        }
        if !ticket.relations.incoming.is_empty() {
            stdout.push_str("### incoming / derived inverse\n\n");
            for relation in &ticket.relations.incoming {
                stdout.push_str(&format!(
                    "- {} {} (forward: {})",
                    relation.inverse_kind,
                    relation.source_ticket,
                    relation.forward_kind.as_str()
                ));
                if let Some(note) = &relation.note {
                    stdout.push_str(&format!(" — {}", note.replace('\n', " ")));
                }
                stdout.push('\n');
            }
        }
        if !ticket.relations.blockers.is_empty() {
            stdout.push_str("### unresolved queue blockers\n\n");
            for blocker in &ticket.relations.blockers {
                stdout.push_str(&format!(
                    "- {} via {} (state: {})\n",
                    blocker.blocking_ticket,
                    blocker.reason_kind,
                    blocker.blocking_state.as_str()
                ));
            }
        }
        if !ticket.relations.notices.is_empty() {
            stdout.push_str("### notices\n\n");
            for notice in &ticket.relations.notices {
                stdout.push_str(&format!("- {}\n", notice.message));
            }
        }
    }

    if !ticket.artifacts.is_empty() {
        stdout.push_str("\n## artifacts\n\n");
        for artifact in &ticket.artifacts {
            stdout.push_str(&format!("- {}\n", artifact.relative_path.display()));
        }
    }

    if let Some(resolution) = &ticket.resolution {
        stdout.push_str("\n## resolution.md\n\n");
        stdout.push_str(resolution.as_str());
        if !stdout.ends_with('\n') {
            stdout.push('\n');
        }
    }

    Ok(success(stdout))
}

fn is_obsolete_ticket_frontmatter_key(key: &str) -> bool {
    matches!(
        key,
        "legacy_ticket" | "needs_preflight" | "action_required" | "attention_required"
    )
}

fn comment(
    backend: &LocalTicketBackend,
    options: CommentOptions,
) -> Result<TicketCliOutput, TicketCliError> {
    let role = options.role.as_str().to_string();
    let mut event = NewTicketEvent::new(options.role, read_body_source(&options.body)?);
    event.author = Some(default_author());
    backend.add_event(TicketIdOrSlug::Query(options.query.clone()), event)?;
    Ok(success(format!("appended\t{}\t{}\n", options.query, role)))
}

fn review(
    backend: &LocalTicketBackend,
    options: ReviewOptions,
) -> Result<TicketCliOutput, TicketCliError> {
    let result = options.result.as_str().to_string();
    let review = TicketReview {
        result: options.result,
        author: Some(default_author()),
        body: MarkdownText::new(read_body_source(&options.body)?),
    };
    backend.review(TicketIdOrSlug::Query(options.query.clone()), review)?;
    Ok(success(format!(
        "reviewed\t{}\t{}\n",
        options.query, result
    )))
}

fn state(
    backend: &LocalTicketBackend,
    options: StateOptions,
) -> Result<TicketCliOutput, TicketCliError> {
    let id = TicketIdOrSlug::Query(options.query.clone());
    let target_state = match options.state {
        StateTarget::Planning => TicketWorkflowState::Planning,
        StateTarget::Ready => TicketWorkflowState::Ready,
        StateTarget::Queued => TicketWorkflowState::Queued,
        StateTarget::InProgress => TicketWorkflowState::InProgress,
        StateTarget::Done => TicketWorkflowState::Done,
        StateTarget::Closed => {
            return Err(TicketCliError::new(
                "yoi ticket state <ticket> closed cannot write resolution.md; use `yoi ticket close <ticket> --resolution <text>` instead",
            ));
        }
    };
    let current = backend.show(id.clone())?;
    let ticket_id = current.meta.id.clone();
    match target_state {
        TicketWorkflowState::Ready => backend.mark_intake_ready(
            id,
            TicketIntakeSummary::new("Marked ready by `yoi ticket state`."),
            ticket::TicketStateChange {
                from: current.meta.workflow_state.as_str().to_string(),
                to: TicketWorkflowState::Ready.as_str().to_string(),
                reason: "cli_state".to_string(),
                author: Some("yoi ticket".to_string()),
                body: "Marked ready by `yoi ticket state`.\n".into(),
                references: Vec::new(),
            },
        )?,
        TicketWorkflowState::Queued => backend.queue_ready(id, "yoi ticket")?,
        _ => {
            let from = current.meta.workflow_state;
            let change = ticket::TicketStateChange {
                from: from.as_str().to_string(),
                to: target_state.as_str().to_string(),
                reason: "cli_state".to_string(),
                author: Some("yoi ticket".to_string()),
                body: format!("State changed to `{}`.\n", target_state.as_str()).into(),
                references: Vec::new(),
            };
            backend.set_workflow_state(id, change)?;
        }
    }
    Ok(success(format!(
        "state\t{}\t{}\n",
        ticket_id,
        target_state.as_str()
    )))
}

fn close(
    backend: &LocalTicketBackend,
    options: CloseOptions,
) -> Result<TicketCliOutput, TicketCliError> {
    backend.close(
        TicketIdOrSlug::Query(options.query.clone()),
        MarkdownText::new(read_body_source(&options.resolution)?),
    )?;
    Ok(success(format!("closed\t{}\n", options.query)))
}

fn relation(
    backend: &LocalTicketBackend,
    options: RelationOptions,
) -> Result<TicketCliOutput, TicketCliError> {
    match options.action {
        RelationAction::Add {
            ticket,
            kind,
            target,
            note,
        } => {
            let created = backend.add_ticket_relation(
                TicketIdOrSlug::Query(ticket.clone()),
                NewTicketRelation {
                    kind,
                    target: target.clone(),
                    note,
                    author: Some("yoi ticket".to_string()),
                },
            )?;
            Ok(success(format!(
                "relation\t{}\t{}\t{}\n",
                created.ticket_id,
                created.kind.as_str(),
                created.target
            )))
        }
        RelationAction::List { ticket, kind } => {
            let ticket = ticket.map(TicketIdOrSlug::Query);
            let relations = backend.query_ticket_relations(ticket, kind)?;
            let mut stdout = String::from("ticket\tkind\ttarget\tauthor\tat\tnote\n");
            for relation in relations {
                stdout.push_str(&format!(
                    "{}\t{}\t{}\t{}\t{}\t{}\n",
                    relation.ticket_id,
                    relation.kind.as_str(),
                    relation.target,
                    relation.author,
                    relation.at,
                    relation.note.unwrap_or_default().replace('\n', " ")
                ));
            }
            Ok(success(stdout))
        }
    }
}

fn doctor(backend: &LocalTicketBackend) -> Result<TicketCliOutput, TicketCliError> {
    let report = backend.doctor()?;
    let mut stdout = String::new();
    if report.is_ok() {
        stdout.push_str("doctor: ok\n");
        return Ok(success(stdout));
    }

    for diagnostic in &report.diagnostics {
        let severity = match diagnostic.severity {
            TicketDoctorSeverity::Error => "error",
            TicketDoctorSeverity::Warning => "warning",
        };
        stdout.push_str(&format!("doctor: {severity}: {}", diagnostic.message));
        if let Some(path) = &diagnostic.path {
            stdout.push_str(&format!(" ({})", path.display()));
        }
        stdout.push('\n');
    }
    stdout.push_str(&format!("doctor: {} error(s)\n", report.error_count()));
    Ok(TicketCliOutput {
        status: TicketCliStatus::Failure,
        stdout,
    })
}

fn success(stdout: String) -> TicketCliOutput {
    TicketCliOutput {
        status: TicketCliStatus::Success,
        stdout,
    }
}

fn parse_create(args: &[String]) -> Result<CreateOptions, TicketCliError> {
    let mut title = None;
    let mut i = 0;
    while i < args.len() {
        match option_with_value(args, &mut i)? {
            Some(("--title", value)) => title = Some(value),
            Some((name, _)) => {
                return Err(TicketCliError::new(format!(
                    "unknown create argument: {name}"
                )));
            }
            None => {
                return Err(TicketCliError::new(format!(
                    "unknown create argument: {}",
                    args[i]
                )));
            }
        }
    }
    let title = title.ok_or_else(|| TicketCliError::new("create requires --title"))?;
    if title.trim().is_empty() {
        return Err(TicketCliError::new("create --title must not be empty"));
    }
    Ok(CreateOptions { title })
}

fn parse_list(args: &[String]) -> Result<ListOptions, TicketCliError> {
    let mut state = ListState::Active;
    let mut limit = None;
    let mut i = 0;
    while i < args.len() {
        match option_with_value(args, &mut i)? {
            Some(("--state", value)) => state = parse_list_state(&value)?,
            Some(("--limit", value)) => limit = Some(parse_list_limit(&value)?),
            Some((name, _)) => {
                return Err(TicketCliError::new(format!(
                    "unknown list argument: {name}"
                )));
            }
            None => {
                return Err(TicketCliError::new(format!(
                    "unknown list argument: {}",
                    args[i]
                )));
            }
        }
    }
    Ok(ListOptions { state, limit })
}

fn parse_relation(args: &[String]) -> Result<RelationOptions, TicketCliError> {
    if args.is_empty() {
        return Err(TicketCliError::new(
            "ticket relation requires `add` or `list`",
        ));
    }
    match args[0].as_str() {
        "add" => parse_relation_add(&args[1..]),
        "list" => parse_relation_list(&args[1..]),
        other => Err(TicketCliError::new(format!(
            "unknown ticket relation action: {other}"
        ))),
    }
}

fn parse_relation_add(args: &[String]) -> Result<RelationOptions, TicketCliError> {
    let mut ticket = None;
    let mut kind = None;
    let mut target = None;
    let mut note = None;
    let mut i = 0;
    while i < args.len() {
        match option_with_value(args, &mut i)? {
            Some(("--ticket", value)) => ticket = Some(value),
            Some(("--kind", value)) => kind = Some(parse_relation_kind(&value)?),
            Some(("--target", value)) => target = Some(value),
            Some(("--note", value)) => note = Some(value),
            Some((name, _)) => {
                return Err(TicketCliError::new(format!(
                    "unknown relation add argument: {name}"
                )));
            }
            None => {
                return Err(TicketCliError::new(format!(
                    "unknown relation add argument: {}",
                    args[i]
                )));
            }
        }
    }
    let ticket = ticket.ok_or_else(|| TicketCliError::new("relation add requires --ticket"))?;
    let kind = kind.ok_or_else(|| TicketCliError::new("relation add requires --kind"))?;
    let target = target.ok_or_else(|| TicketCliError::new("relation add requires --target"))?;
    Ok(RelationOptions {
        action: RelationAction::Add {
            ticket,
            kind,
            target,
            note,
        },
    })
}

fn parse_relation_list(args: &[String]) -> Result<RelationOptions, TicketCliError> {
    let mut ticket = None;
    let mut kind = None;
    let mut i = 0;
    while i < args.len() {
        match option_with_value(args, &mut i)? {
            Some(("--ticket", value)) => ticket = Some(value),
            Some(("--kind", value)) => kind = Some(parse_relation_kind(&value)?),
            Some((name, _)) => {
                return Err(TicketCliError::new(format!(
                    "unknown relation list argument: {name}"
                )));
            }
            None => {
                return Err(TicketCliError::new(format!(
                    "unknown relation list argument: {}",
                    args[i]
                )));
            }
        }
    }
    Ok(RelationOptions {
        action: RelationAction::List { ticket, kind },
    })
}

fn parse_relation_kind(value: &str) -> Result<TicketRelationKind, TicketCliError> {
    TicketRelationKind::parse(value).ok_or_else(|| {
        TicketCliError::new(format!(
            "unknown relation kind `{value}`; expected depends_on, blocks, related, supersedes, or duplicate_of"
        ))
    })
}

fn parse_comment(args: &[String]) -> Result<CommentOptions, TicketCliError> {
    if args.is_empty() || args[0].starts_with('-') {
        return Err(TicketCliError::new("comment requires <id>"));
    }
    let query = args[0].clone();
    let mut role = TicketEventKind::Comment;
    let mut file = None;
    let mut message = None;
    let mut i = 1;
    while i < args.len() {
        match option_with_value(args, &mut i)? {
            Some(("--role", value)) => role = parse_comment_role(&value)?,
            Some(("--file", value)) => file = Some(PathBuf::from(value)),
            Some(("--message", value)) => message = Some(value),
            Some((name, _)) => {
                return Err(TicketCliError::new(format!(
                    "unknown comment argument: {name}"
                )));
            }
            None => {
                return Err(TicketCliError::new(format!(
                    "unknown comment argument: {}",
                    args[i]
                )));
            }
        }
    }
    Ok(CommentOptions {
        query,
        role,
        body: exactly_one_body("comment", file, message)?,
    })
}

fn parse_review(args: &[String]) -> Result<ReviewOptions, TicketCliError> {
    if args.is_empty() || args[0].starts_with('-') {
        return Err(TicketCliError::new("review requires <id>"));
    }
    let query = args[0].clone();
    let mut approve = false;
    let mut request_changes = false;
    let mut file = None;
    let mut message = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--approve" => {
                approve = true;
                i += 1;
            }
            "--request-changes" => {
                request_changes = true;
                i += 1;
            }
            _ => match option_with_value(args, &mut i)? {
                Some(("--file", value)) => file = Some(PathBuf::from(value)),
                Some(("--message", value)) => message = Some(value),
                Some((name, _)) => {
                    return Err(TicketCliError::new(format!(
                        "unknown review argument: {name}"
                    )));
                }
                None => {
                    return Err(TicketCliError::new(format!(
                        "unknown review argument: {}",
                        args[i]
                    )));
                }
            },
        }
    }
    let result = match (approve, request_changes) {
        (true, false) => TicketReviewResult::Approve,
        (false, true) => TicketReviewResult::RequestChanges,
        (false, false) => {
            return Err(TicketCliError::new(
                "review requires exactly one of --approve or --request-changes",
            ));
        }
        (true, true) => {
            return Err(TicketCliError::new(
                "review accepts exactly one of --approve or --request-changes",
            ));
        }
    };
    Ok(ReviewOptions {
        query,
        result,
        body: exactly_one_body("review", file, message)?,
    })
}

fn parse_state(args: &[String]) -> Result<StateOptions, TicketCliError> {
    if args.len() != 2 {
        return Err(TicketCliError::new(
            "state requires <id> <planning|ready|queued|inprogress|done|closed>",
        ));
    }
    Ok(StateOptions {
        query: args[0].clone(),
        state: parse_state_target(&args[1])?,
    })
}

fn parse_close(args: &[String]) -> Result<CloseOptions, TicketCliError> {
    if args.is_empty() || args[0].starts_with('-') {
        return Err(TicketCliError::new("close requires <id>"));
    }
    let query = args[0].clone();
    let mut file = None;
    let mut resolution = None;
    let mut i = 1;
    while i < args.len() {
        match option_with_value(args, &mut i)? {
            Some(("--resolution", value)) => resolution = Some(value),
            Some(("--file", value)) => file = Some(PathBuf::from(value)),
            Some((name, _)) => {
                return Err(TicketCliError::new(format!(
                    "unknown close argument: {name}"
                )));
            }
            None => {
                return Err(TicketCliError::new(format!(
                    "unknown close argument: {}",
                    args[i]
                )));
            }
        }
    }
    Ok(CloseOptions {
        query,
        resolution: exactly_one_body("close", file, resolution)?,
    })
}

fn parse_one_positional(command: &str, args: &[String]) -> Result<String, TicketCliError> {
    if args.len() != 1 || args[0].starts_with('-') {
        Err(TicketCliError::new(format!("{command} requires <id>")))
    } else {
        Ok(args[0].clone())
    }
}

fn option_with_value(
    args: &[String],
    i: &mut usize,
) -> Result<Option<(&'static str, String)>, TicketCliError> {
    let arg = &args[*i];
    for name in [
        "--title",
        "--state",
        "--role",
        "--file",
        "--message",
        "--resolution",
        "--ticket",
        "--kind",
        "--target",
        "--note",
        "--limit",
    ] {
        if arg == name {
            let value = args
                .get(*i + 1)
                .ok_or_else(|| TicketCliError::new(format!("{name} requires a value")))?;
            if value.starts_with('-') {
                return Err(TicketCliError::new(format!("{name} requires a value")));
            }
            *i += 2;
            return Ok(Some((name, value.clone())));
        }
        if let Some(value) = arg.strip_prefix(&format!("{name}=")) {
            if value.is_empty() {
                return Err(TicketCliError::new(format!("{name} requires a value")));
            }
            *i += 1;
            return Ok(Some((name, value.to_string())));
        }
    }
    Ok(None)
}

fn parse_list_state(raw: &str) -> Result<ListState, TicketCliError> {
    let tokens = raw
        .split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return Err(TicketCliError::new("--state must not be empty"));
    }
    if tokens.len() == 1 {
        match tokens[0] {
            "active" => return Ok(ListState::Active),
            "all" => return Ok(ListState::All),
            _ => {}
        }
    } else if tokens
        .iter()
        .any(|token| *token == "active" || *token == "all")
    {
        return Err(TicketCliError::new(
            "--state active/all cannot be mixed with workflow states",
        ));
    }

    let mut states = Vec::new();
    for token in tokens {
        let state = TicketWorkflowState::parse(token).ok_or_else(|| {
            TicketCliError::new(format!(
                "invalid state: {token}; expected active, all, planning, ready, queued, inprogress, done, closed"
            ))
        })?;
        if !states.contains(&state) {
            states.push(state);
        }
    }
    Ok(ListState::States(states))
}

fn parse_list_limit(value: &str) -> Result<usize, TicketCliError> {
    value
        .parse::<usize>()
        .map_err(|_| TicketCliError::new(format!("invalid limit: {value}")))
}

fn ticket_cli_hints(ticket: &TicketSummary) -> String {
    let mut hints = Vec::new();
    if let Some(readiness) = ticket.readiness.as_deref() {
        hints.push(format!(
            "readiness:{}",
            truncate_inline(readiness, LIST_HINT_MAX_CHARS)
        ));
    }
    hints.join("; ")
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

fn parse_state_target(value: &str) -> Result<StateTarget, TicketCliError> {
    match value {
        "planning" => Ok(StateTarget::Planning),
        "ready" => Ok(StateTarget::Ready),
        "queued" => Ok(StateTarget::Queued),
        "inprogress" => Ok(StateTarget::InProgress),
        "done" => Ok(StateTarget::Done),
        "closed" => Ok(StateTarget::Closed),
        _ => Err(TicketCliError::new(format!("invalid state: {value}"))),
    }
}

fn parse_comment_role(value: &str) -> Result<TicketEventKind, TicketCliError> {
    match value {
        "comment" => Ok(TicketEventKind::Comment),
        "plan" => Ok(TicketEventKind::Plan),
        "decision" => Ok(TicketEventKind::Decision),
        "implementation_report" => Ok(TicketEventKind::ImplementationReport),
        _ => Err(TicketCliError::new(format!(
            "invalid comment role: {value}"
        ))),
    }
}

fn exactly_one_body(
    command: &str,
    file: Option<PathBuf>,
    message: Option<String>,
) -> Result<BodySource, TicketCliError> {
    match (file, message) {
        (Some(_), Some(_)) => Err(TicketCliError::new(format!(
            "{command} accepts exactly one of --file or --message/--resolution"
        ))),
        (Some(path), None) => Ok(BodySource::File(path)),
        (None, Some(message)) => Ok(BodySource::Message(ensure_trailing_newline(message))),
        (None, None) => Err(TicketCliError::new(format!(
            "{command} requires --file or --message/--resolution"
        ))),
    }
}

fn read_body_source(source: &BodySource) -> Result<String, TicketCliError> {
    match source {
        BodySource::Message(message) => Ok(message.clone()),
        BodySource::File(path) => fs::read_to_string(path)
            .map(ensure_trailing_newline)
            .map_err(|error| {
                TicketCliError::new(format!("failed to read {}: {error}", path.display()))
            }),
    }
}

fn ensure_trailing_newline(mut value: String) -> String {
    if !value.ends_with('\n') {
        value.push('\n');
    }
    value
}

fn default_author() -> String {
    std::env::var("USER").unwrap_or_else(|_| "unknown".to_string())
}

fn help_text() -> &'static str {
    "yoi ticket\n\nUsage:\n  yoi ticket init\n  yoi ticket create --title <title>\n  yoi ticket list [--state active|all|planning|ready|queued|inprogress|done|closed[,..]] [--limit <n>]\n  yoi ticket show <id>\n  yoi ticket comment <id> [--role comment|plan|decision|implementation_report] (--file <path>|--message <text>)\n  yoi ticket review <id> (--approve|--request-changes) (--file <path>|--message <text>)\n  yoi ticket state <id> <planning|ready|queued|inprogress|done|closed>\n  yoi ticket close <id> (--resolution <text>|--file <path>)\n  yoi ticket relation add --ticket <id> --kind <depends_on|blocks|related|supersedes|duplicate_of> --target <id> [--note <text>]\n  yoi ticket relation list [--ticket <id>] [--kind <kind>]\n  yoi ticket doctor\n\nOptions:\n  -h, --help    Print help\n\nBackend:\n  `yoi ticket init` writes explicit fixed role profiles and optional [ticket].language into .yoi/workspace.toml.\n  Uses workspace Ticket settings from .yoi/workspace.toml [ticket] when present; .yoi/ticket.config.toml is a read-only migration fallback only.\n  Supported provider: builtin:yoi_local.\n  Without configured Ticket settings, the local backend root is <cwd>/.yoi/tickets.\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use ticket::TicketEventKind;
    use ticket::config::TicketRole;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    fn run(temp: &TempDir, items: &[&str]) -> TicketCliOutput {
        let cli = parse_ticket_args(&args(items)).unwrap();
        run_in_workspace(cli, temp.path()).unwrap()
    }

    fn created_id(output: &TicketCliOutput) -> String {
        output
            .stdout
            .strip_prefix("created\t")
            .and_then(|rest| rest.lines().next())
            .expect("create output contains created id")
            .to_string()
    }

    #[test]
    fn ticket_cli_init_writes_explicit_workspace_ticket_settings() {
        let temp = TempDir::new().unwrap();

        let initialized = run(&temp, &["init"]);
        assert_eq!(initialized.status, TicketCliStatus::Success);
        assert!(initialized.stdout.contains("created\t.yoi/workspace.toml"));
        assert!(initialized.stdout.contains("ensured\t.yoi/tickets"));
        assert!(temp.path().join(".yoi/tickets").exists());
        assert!(temp.path().join(".yoi/tickets/.gitkeep").exists());

        let config = fs::read_to_string(temp.path().join(".yoi/workspace.toml")).unwrap();
        assert!(config.contains("workspace_id = \""));
        assert!(config.contains("[ticket]\n"));
        assert!(config.contains("[ticket.backend]\n"));
        assert!(config.contains("provider = \"builtin:yoi_local\""));
        assert!(config.contains("root = \".yoi/tickets\""));
        assert!(config.contains("# language = \"Japanese\""));
        for role in TicketRole::ALL {
            assert!(config.contains(&format!(
                "[ticket.roles.{role}]\nprofile = \"{}\"",
                role.default_profile()
            )));
        }
        assert!(!config.contains("[ticket.roles.investigator]"));
    }

    #[test]
    fn ticket_cli_init_does_not_overwrite_existing_config() {
        let temp = TempDir::new().unwrap();
        fs::create_dir_all(temp.path().join(".yoi")).unwrap();
        let config_path = temp.path().join(".yoi/ticket.config.toml");
        fs::write(
            &config_path,
            "[backend]\nprovider = \"builtin:yoi_local\"\n",
        )
        .unwrap();

        let cli = parse_ticket_args(&args(&["init"])).unwrap();
        let err = run_in_workspace(cli, temp.path()).unwrap_err();
        assert!(err.to_string().contains("legacy ticket config exists"));
        assert!(err.to_string().contains("obsolete and read-only"));
        assert!(err.to_string().contains(WORKSPACE_SETTINGS_RELATIVE_PATH));
        assert_eq!(
            fs::read_to_string(config_path).unwrap(),
            "[backend]\nprovider = \"builtin:yoi_local\"\n"
        );
    }

    #[test]
    fn ticket_cli_list_is_bounded_and_truncates_titles() {
        let temp = TempDir::new().unwrap();
        let long_title = format!("Long {}", "x".repeat(LIST_TITLE_MAX_CHARS + 40));
        let first = run(&temp, &["create", "--title", long_title.as_str()]);
        assert_eq!(first.status, TicketCliStatus::Success);
        for index in 0..DEFAULT_LIST_LIMIT {
            let title = format!("Ticket {index:03}");
            let created = run(&temp, &["create", "--title", title.as_str()]);
            assert_eq!(created.status, TicketCliStatus::Success);
        }

        let listed = run(&temp, &["list", "--state", "all"]);
        assert_eq!(listed.status, TicketCliStatus::Success);
        assert!(listed.stdout.contains("# truncated: returned"));
        let non_note_lines = listed
            .stdout
            .lines()
            .filter(|line| !line.starts_with('#'))
            .count();
        assert_eq!(non_note_lines, DEFAULT_LIST_LIMIT + 1);
        let first_ticket_line = listed.stdout.lines().nth(1).unwrap();
        let listed_title = first_ticket_line.split('\t').nth(2).unwrap();
        assert!(listed_title.starts_with("Long "));
        assert!(listed_title.chars().count() <= LIST_TITLE_MAX_CHARS);
        assert!(listed_title.ends_with("..."));
    }

    #[test]
    fn ticket_cli_list_limit_is_capped() {
        let temp = TempDir::new().unwrap();
        for index in 0..(MAX_LIST_LIMIT + 5) {
            let title = format!("Ticket {index:03}");
            let created = run(&temp, &["create", "--title", title.as_str()]);
            assert_eq!(created.status, TicketCliStatus::Success);
        }

        let listed = run(&temp, &["list", "--state", "all", "--limit", "1000"]);
        assert_eq!(listed.status, TicketCliStatus::Success);
        assert!(listed.stdout.contains("# truncated: returned"));
        let non_note_lines = listed
            .stdout
            .lines()
            .filter(|line| !line.starts_with('#'))
            .count();
        assert_eq!(non_note_lines, MAX_LIST_LIMIT + 1);
    }

    #[test]
    fn ticket_cli_create_list_show_comment_review_state_close_and_doctor() {
        let temp = TempDir::new().unwrap();

        let created = run(&temp, &["create", "--title", "CLI Created"]);
        assert_eq!(created.status, TicketCliStatus::Success);
        assert!(created.stdout.contains("created\t"));
        let ticket_id = created_id(&created);
        assert!(temp.path().join(".yoi/tickets").join(&ticket_id).exists());
        assert!(!temp.path().join("work-items").exists());
        let created_item = fs::read_to_string(
            temp.path()
                .join(".yoi/tickets")
                .join(&ticket_id)
                .join("item.md"),
        )
        .unwrap();
        assert!(created_item.contains("state:"));
        assert!(created_item.contains("planning"));
        assert!(!created_item.contains("legacy_ticket:"));
        assert!(!created_item.contains("needs_preflight:"));
        assert!(!created_item.contains("slug:"));
        assert!(!created_item.contains("workflow_state:"));

        let listed = run(&temp, &["list", "--state", "planning"]);
        assert!(listed.stdout.contains("state\tid\ttitle"));
        assert!(listed.stdout.contains(&ticket_id));
        assert!(listed.stdout.contains("CLI Created"));
        assert!(!listed.stdout.contains("legacy_ticket"));
        assert!(!listed.stdout.contains("needs_preflight"));

        let shown = run(&temp, &["show", &ticket_id]);
        assert!(shown.stdout.contains("# CLI Created"));
        assert!(shown.stdout.contains(&format!("ID: {ticket_id}")));
        assert!(shown.stdout.contains("State: planning"));
        assert!(!shown.stdout.contains("legacy_ticket"));
        assert!(!shown.stdout.contains("needs_preflight"));

        let commented = run(
            &temp,
            &[
                "comment",
                &ticket_id,
                "--role",
                "implementation_report",
                "--message",
                "Implemented.",
            ],
        );
        assert!(
            commented
                .stdout
                .contains(&format!("appended\t{}\timplementation_report", ticket_id))
        );

        let reviewed = run(
            &temp,
            &[
                "review",
                &ticket_id,
                "--approve",
                "--message",
                "Looks good.",
            ],
        );
        assert!(
            reviewed
                .stdout
                .contains(&format!("reviewed\t{}\tapprove", ticket_id))
        );

        let ready = run(&temp, &["state", &ticket_id, "ready"]);
        assert_eq!(ready.stdout, format!("state\t{}\tready\n", ticket_id));
        let ready_listed = run(&temp, &["list", "--state", "ready"]);
        assert!(ready_listed.stdout.contains(&ticket_id));

        let queued = run(&temp, &["state", &ticket_id, "queued"]);
        assert_eq!(queued.stdout, format!("state\t{}\tqueued\n", ticket_id));
        let queued_listed = run(&temp, &["list", "--state", "queued"]);
        assert!(queued_listed.stdout.contains(&ticket_id));

        let inprogress = run(&temp, &["state", &ticket_id, "inprogress"]);
        assert_eq!(
            inprogress.stdout,
            format!("state\t{}\tinprogress\n", ticket_id)
        );
        let inprogress_listed = run(&temp, &["list", "--state", "inprogress"]);
        assert!(inprogress_listed.stdout.contains(&ticket_id));

        let done = run(&temp, &["state", &ticket_id, "done"]);
        assert_eq!(done.stdout, format!("state\t{}\tdone\n", ticket_id));
        let done_listed = run(&temp, &["list", "--state", "done"]);
        assert!(done_listed.stdout.contains(&ticket_id));

        let closed = run(
            &temp,
            &["close", &ticket_id, "--resolution", "Done via yoi ticket."],
        );
        assert!(closed.stdout.contains(&format!("closed\t{}", ticket_id)));

        let doctor = run(&temp, &["doctor"]);
        assert_eq!(doctor.status, TicketCliStatus::Success);
        assert_eq!(doctor.stdout, "doctor: ok\n");

        let backend = LocalTicketBackend::new(temp.path().join(".yoi/tickets"));
        let ticket = backend.show(TicketIdOrSlug::Id(ticket_id.clone())).unwrap();
        assert!(ticket.resolution.is_some());
        assert_eq!(ticket.meta.workflow_state, TicketWorkflowState::Closed);
        assert!(
            ticket
                .events
                .iter()
                .any(|event| event.kind == TicketEventKind::ImplementationReport)
        );
        assert!(
            ticket
                .events
                .iter()
                .any(|event| event.kind == TicketEventKind::Review)
        );
    }

    #[test]
    fn ticket_cli_show_omits_obsolete_overlay_fields_from_legacy_frontmatter() {
        let temp = TempDir::new().unwrap();
        let created = run(&temp, &["create", "--title", "Legacy Overlay"]);
        let ticket_id = created_id(&created);
        let item_path = temp
            .path()
            .join(".yoi/tickets")
            .join(&ticket_id)
            .join("item.md");
        let item = fs::read_to_string(&item_path).unwrap();
        fs::write(
            &item_path,
            item.replacen(
                "---\n",
                "---\naction_required: legacy action\nattention_required: legacy attention\n",
                1,
            ),
        )
        .unwrap();

        let shown = run(&temp, &["show", &ticket_id]);
        assert_eq!(shown.status, TicketCliStatus::Success);
        assert!(shown.stdout.contains("# Legacy Overlay"));
        assert!(!shown.stdout.contains("action_required"));
        assert!(!shown.stdout.contains("attention_required"));
        assert!(!shown.stdout.contains("legacy action"));
        assert!(!shown.stdout.contains("legacy attention"));
    }

    #[test]
    fn ticket_cli_records_lists_and_shows_relations() {
        let temp = TempDir::new().unwrap();
        let source = created_id(&run(&temp, &["create", "--title", "Relation Source"]));
        let target = created_id(&run(&temp, &["create", "--title", "Relation Target"]));

        let added = run(
            &temp,
            &[
                "relation",
                "add",
                "--ticket",
                &source,
                "--kind",
                "depends_on",
                "--target",
                &target,
                "--note",
                "target first",
            ],
        );
        assert_eq!(
            added.stdout,
            format!("relation\t{source}\tdepends_on\t{target}\n")
        );

        let listed = run(&temp, &["relation", "list", "--ticket", &target]);
        assert!(listed.stdout.contains("ticket\tkind\ttarget"));
        assert!(
            listed
                .stdout
                .contains(&format!("{source}\tdepends_on\t{target}"))
        );

        let shown_source = run(&temp, &["show", &source]);
        assert!(shown_source.stdout.contains("## relations"));
        assert!(
            shown_source
                .stdout
                .contains(&format!("- depends_on {target}"))
        );
        assert!(shown_source.stdout.contains("unresolved queue blockers"));

        let shown_target = run(&temp, &["show", &target]);
        assert!(shown_target.stdout.contains("incoming / derived inverse"));
        assert!(
            shown_target
                .stdout
                .contains(&format!("dependency_of {source}"))
        );
    }

    #[test]
    fn ticket_cli_uses_configured_backend_root() {
        let temp = TempDir::new().unwrap();
        fs::create_dir_all(temp.path().join(".yoi")).unwrap();
        fs::write(
            temp.path().join(".yoi/workspace.toml"),
            "[ticket]\nlanguage = \"Japanese\"\n\n[ticket.backend]\nprovider = \"builtin:yoi_local\"\nroot = \"custom-tickets\"\n",
        )
        .unwrap();

        let created = run(&temp, &["create", "--title", "Configured Root"]);
        let ticket_id = created_id(&created);

        assert!(temp.path().join("custom-tickets").join(ticket_id).exists());
        assert!(!temp.path().join("custom-tickets/open").exists());
        assert!(!temp.path().join("work-items").exists());
    }

    #[test]
    fn ticket_cli_rejects_ambiguous_body_sources() {
        let err = parse_ticket_args(&args(&[
            "comment",
            "ticket",
            "--file",
            "body.md",
            "--message",
            "body",
        ]))
        .unwrap_err();
        assert!(err.to_string().contains("exactly one"));
    }

    #[test]
    fn ticket_cli_rejects_ambiguous_review_result() {
        let err = parse_ticket_args(&args(&[
            "review",
            "ticket",
            "--approve",
            "--request-changes",
            "--message",
            "body",
        ]))
        .unwrap_err();
        assert!(err.to_string().contains("exactly one"));
    }

    #[test]
    fn ticket_cli_state_closed_requires_close_command() {
        let temp = TempDir::new().unwrap();
        let created = run(&temp, &["create", "--title", "Close Me"]);
        let ticket_id = created_id(&created);
        let cli = parse_ticket_args(&args(&["state", &ticket_id, "closed"])).unwrap();
        let err = run_in_workspace(cli, temp.path()).unwrap_err();
        assert!(err.to_string().contains("use `yoi ticket close"));
    }

    #[test]
    fn ticket_cli_list_defaults_to_active_and_accepts_multi_state_filter() {
        let default = parse_ticket_args(&args(&["list"])).unwrap();
        match default {
            TicketCommand::List(options) => assert_eq!(options.state, ListState::Active),
            other => panic!("unexpected command: {other:?}"),
        }

        let explicit = parse_ticket_args(&args(&["list", "--state", "planning,closed"])).unwrap();
        match explicit {
            TicketCommand::List(options) => assert_eq!(
                options.state,
                ListState::States(vec![
                    TicketWorkflowState::Planning,
                    TicketWorkflowState::Closed
                ])
            ),
            other => panic!("unexpected command: {other:?}"),
        }

        let mixed = parse_ticket_args(&args(&["list", "--state", "active,planning"])).unwrap_err();
        assert!(mixed.to_string().contains("cannot be mixed"));
    }

    #[test]
    fn ticket_cli_help_lists_required_commands() {
        let help = parse_ticket_args(&args(&["--help"])).unwrap();
        let output = run_in_workspace(help, Path::new(".")).unwrap();
        assert!(output.stdout.contains("yoi ticket init"));
        assert!(output.stdout.contains("yoi ticket create"));
        assert!(output.stdout.contains("yoi ticket doctor"));
    }
}
