use std::fmt;
use std::fs;
use std::path::PathBuf;

use client::{BackendWorkspaceProductClient, ResolvedTarget};
use ticket::{
    MarkdownText, NewTicket, NewTicketEvent, NewTicketRelation, TicketBackend,
    TicketDoctorSeverity, TicketEventKind, TicketIdOrSlug, TicketListQuery, TicketListState,
    TicketRelationKind, TicketSummary, TicketWorkflowState,
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
    Create(CreateOptions),
    List(ListOptions),
    Show { query: String },
    Comment(CommentOptions),
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
    States(Vec<TicketListState>),
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
        "create" => TicketCommand::Create(parse_create(&args[1..])?),
        "list" => TicketCommand::List(parse_list(&args[1..])?),
        "show" => TicketCommand::Show {
            query: parse_one_positional("show", &args[1..])?,
        },
        "comment" => TicketCommand::Comment(parse_comment(&args[1..])?),
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

pub fn run(cli: TicketCli, target: ResolvedTarget) -> Result<TicketCliOutput, TicketCliError> {
    match target {
        ResolvedTarget::Standalone => Err(TicketCliError::new(
            "Standalone is a one-shot Worker host, not Ticket storage authority; select a Backend target; select a Backend target",
        )),
        ResolvedTarget::Backend {
            base_url,
            workspace_id,
        } => match cli {
            TicketCli::Help => Ok(TicketCliOutput {
                status: TicketCliStatus::Success,
                stdout: help_text().to_string(),
            }),
            TicketCli::Command(command) => {
                let backend = BackendWorkspaceProductClient::new(base_url, workspace_id)
                    .map_err(|error| TicketCliError::new(error.to_string()))?;
                run_backend_command(command, &backend)
            }
        },
    }
}

fn run_backend_command(
    command: TicketCommand,
    backend: &dyn TicketBackend,
) -> Result<TicketCliOutput, TicketCliError> {
    match command {
        TicketCommand::Create(options) => create(backend, options),
        TicketCommand::List(options) => list(backend, options),
        TicketCommand::Show { query } => show(backend, query),
        TicketCommand::Comment(options) => comment(backend, options),
        TicketCommand::State(options) => state(backend, options),
        TicketCommand::Close(options) => close(backend, options),
        TicketCommand::Relation(options) => relation(backend, options),
        TicketCommand::Doctor => doctor(backend),
    }
}

fn create(
    backend: &dyn TicketBackend,
    options: CreateOptions,
) -> Result<TicketCliOutput, TicketCliError> {
    let mut input = NewTicket::new(options.title);
    input.author = Some("yoi ticket".to_string());

    let created = backend.create(input)?;
    Ok(success(format!("created\t{}\n", created.id)))
}

fn list(
    backend: &dyn TicketBackend,
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

fn show(backend: &dyn TicketBackend, query: String) -> Result<TicketCliOutput, TicketCliError> {
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
    backend: &dyn TicketBackend,
    options: CommentOptions,
) -> Result<TicketCliOutput, TicketCliError> {
    let role = options.role.as_str().to_string();
    let mut event = NewTicketEvent::new(options.role, read_body_source(&options.body)?);
    event.author = Some(default_author());
    backend.add_event(TicketIdOrSlug::Query(options.query.clone()), event)?;
    Ok(success(format!("appended\t{}\t{}\n", options.query, role)))
}

fn state(
    backend: &dyn TicketBackend,
    options: StateOptions,
) -> Result<TicketCliOutput, TicketCliError> {
    let id = TicketIdOrSlug::Query(options.query.clone());
    let target_state = match options.state {
        StateTarget::Planning => TicketWorkflowState::Planning,
        StateTarget::Ready => {
            return Err(TicketCliError::new(
                "ready requires Workspace repository authority; use the Browser Mark ready action or TicketMarkReady",
            ));
        }
        StateTarget::Queued => {
            return Err(TicketCliError::new(
                "queued is an Orchestrator operation; use TicketQueue after MarkReady succeeds",
            ));
        }
        StateTarget::InProgress => TicketWorkflowState::InProgress,
        StateTarget::Done => {
            return Err(TicketCliError::new(
                "done is guarded by CompleteMergeRequest with an approved exact source ref and operation_id",
            ));
        }
        StateTarget::Closed => {
            return Err(TicketCliError::new(
                "yoi ticket state <ticket> closed cannot write resolution.md; use `yoi ticket close <ticket> --resolution <text>` instead",
            ));
        }
    };
    let current = backend.show(id.clone())?;
    let ticket_id = current.meta.id.clone();
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
    Ok(success(format!(
        "state\t{}\t{}\n",
        ticket_id,
        target_state.as_str()
    )))
}

fn close(
    backend: &dyn TicketBackend,
    options: CloseOptions,
) -> Result<TicketCliOutput, TicketCliError> {
    backend.close(
        TicketIdOrSlug::Query(options.query.clone()),
        MarkdownText::new(read_body_source(&options.resolution)?),
    )?;
    Ok(success(format!("closed\t{}\n", options.query)))
}

fn relation(
    backend: &dyn TicketBackend,
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

fn doctor(backend: &dyn TicketBackend) -> Result<TicketCliOutput, TicketCliError> {
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
        let state = TicketListState::parse(token).ok_or_else(|| {
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
    "yoi ticket\n\nUsage:\n  yoi ticket create --title <title>\n  yoi ticket list [--state active|all|planning|ready|queued|inprogress|done|closed[,..]] [--limit <n>]\n  yoi ticket show <id>\n  yoi ticket comment <id> [--role comment|plan|decision|implementation_report] (--file <path>|--message <text>)\n  yoi ticket state <id> <planning|ready|queued|inprogress|closed>\n  yoi ticket close <id> (--resolution <text>|--file <path>)\n  yoi ticket relation add --ticket <id> --kind <depends_on|blocks|related|supersedes|duplicate_of> --target <id> [--note <text>]\n  yoi ticket relation list [--ticket <id>] [--kind <kind>]\n  yoi ticket doctor\n\nOptions:\n  -h, --help    Print help\n\nTargets:\n  Ticket commands require the Workspace-scoped Backend selected by the shared client Target.\n  Standalone never falls back to repository-local Ticket storage.\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn standalone_rejects_ticket_storage_operations() {
        let cli = parse_ticket_args(&args(&["list"])).unwrap();
        let error = run(cli, ResolvedTarget::Standalone).unwrap_err();
        assert!(error.to_string().contains("select a Backend target"));
    }

    #[test]
    fn repository_local_ticket_commands_are_not_normal_target_commands() {
        for command in ["init", "import-local"] {
            let error = parse_ticket_args(&args(&[command])).unwrap_err();
            assert!(error.to_string().contains("unknown ticket command"));
        }
    }

    #[test]
    fn help_states_backend_authority() {
        assert!(help_text().contains("Workspace-scoped Backend"));
        assert!(!help_text().contains("repository-file Ticket backend"));
    }
}
