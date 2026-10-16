use std::fmt;
use std::fs;
use std::path::PathBuf;

use client::{BackendWorkspaceProductClient, ResolvedTarget};
use ticket::{
    NewTicket, NewTicketEvent, NewTicketRelation, TicketBackend, TicketCaseAction,
    TicketCaseUpdate, TicketClosureReason, TicketDoctorSeverity, TicketEventKind, TicketIdOrSlug,
    TicketListQuery, TicketListState, TicketReference, TicketRelationKind, TicketState,
    TicketSummary,
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
    Case(CaseOptions),
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

/// Explicit preconditions are required instead of silently refreshing them from
/// show. Repeating the same argv (and file bytes) submits the exact same request,
/// even after the original action changed the case state or closure key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaseOptions {
    pub query: String,
    pub expected_content_digest: String,
    pub expected_state: TicketState,
    pub expected_closure_operation_key: Option<String>,
    pub operation_key: String,
    pub action: TicketCaseAction,
    pub explanation: BodySource,
    pub references: Vec<TicketReference>,
    pub author: String,
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
        "close" | "reopen" | "correct-closure" => {
            TicketCommand::Case(parse_case(&args[0], &args[1..])?)
        }
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
    if cli == TicketCli::Help {
        return Ok(success(help_text().to_string()));
    }
    match target {
        ResolvedTarget::Standalone => Err(TicketCliError::new(
            "Standalone is a one-shot Worker host, not Ticket storage authority; select a Backend target",
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
        TicketCommand::Case(options) => update_case(backend, options),
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
    stdout.push_str(&format!(
        "Content digest: {}\nClosure operation key: {}\n",
        ticket::ticket_content_digest(&ticket),
        closure_key_token(ticket.closure_operation_key.as_deref())
    ));
    if let Some(closure) = &ticket.current_closure {
        stdout.push_str(&format!(
            "Closure reason: {}\nClosure author: {}\nClosure explanation: {}\n",
            closure_reason_label(&closure.reason),
            closure.author,
            closure.explanation
        ));
    }
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

// Escape reserved tokens so show supplies usable CAS even when an actual key
// equals "none" or begins with the literal-key prefix.
fn closure_key_token(key: Option<&str>) -> String {
    match key {
        None => "none".into(),
        Some(key) if key == "none" || key.starts_with("key:") => format!("key:{key}"),
        Some(key) => key.into(),
    }
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

fn update_case(
    backend: &dyn TicketBackend,
    options: CaseOptions,
) -> Result<TicketCliOutput, TicketCliError> {
    let explanation = match &options.explanation {
        BodySource::Message(text) => text.clone(),
        BodySource::File(path) => fs::read_to_string(path).map_err(|error| {
            TicketCliError::new(format!("failed to read {}: {error}", path.display()))
        })?,
    };
    require_nonblank("explanation", &explanation)?;
    // Resolve identity only. Never rewrite the explicit CAS with this snapshot:
    // doing so would change the fingerprint of an exact replay.
    let current = backend.show(TicketIdOrSlug::Query(options.query))?;
    let operation_key = options.operation_key.clone();
    let updated = backend.update_case(
        &current.meta.id,
        TicketCaseUpdate {
            expected_content_digest: options.expected_content_digest,
            expected_state: options.expected_state,
            expected_closure_operation_key: options.expected_closure_operation_key,
            operation_key: options.operation_key,
            action: options.action,
            explanation,
            references: options.references,
            author: Some(options.author),
        },
    )?;
    // A replay returns current authority, which may already have been reopened.
    // Report that state, not an inferred "closed"/"completed" success.
    Ok(success(format!(
        "case\t{}\t{}\toperation-key:{}\n",
        updated.meta.id,
        updated.meta.workflow_state.as_str(),
        operation_key
    )))
}

fn closure_reason_label(reason: &TicketClosureReason) -> String {
    match reason {
        TicketClosureReason::Completed => "completed".into(),
        TicketClosureReason::NotPlanned => "not_planned".into(),
        TicketClosureReason::Duplicate => "duplicate".into(),
        TicketClosureReason::Unknown => "unknown".into(),
        TicketClosureReason::Other(text) => format!("other:{text}"),
    }
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

fn parse_case(command: &str, args: &[String]) -> Result<CaseOptions, TicketCliError> {
    if args.is_empty() || args[0].starts_with('-') {
        return Err(TicketCliError::new(format!("{command} requires <id>")));
    }
    let query = args[0].clone();
    let mut digest = None;
    let mut state = None;
    let mut closure_key = None;
    let mut operation_key = None;
    let mut reason = None;
    let mut explanation = None;
    let mut file = None;
    let mut references = Vec::new();
    let mut author = None;
    let mut seen = std::collections::BTreeSet::new();
    let mut i = 1;
    while i < args.len() {
        let Some((name, value)) = option_with_value(args, &mut i)? else {
            return Err(TicketCliError::new(format!(
                "unknown {command} argument: {}",
                args[i]
            )));
        };
        if name != "--reference" && !seen.insert(name) {
            return Err(TicketCliError::new(format!(
                "{name} may only be supplied once"
            )));
        }
        require_nonblank(name, &value)?;
        match name {
            "--expected-content-digest" => {
                if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return Err(TicketCliError::new(
                        "--expected-content-digest requires a SHA-256 hex digest",
                    ));
                }
                digest = Some(value);
            }
            "--expected-state" => {
                state = Some(TicketState::parse(&value).ok_or_else(|| {
                    TicketCliError::new("--expected-state requires open or closed")
                })?)
            }
            "--expected-closure-operation-key" => {
                closure_key = Some(if value == "none" {
                    None
                } else {
                    let key = value.strip_prefix("key:").unwrap_or(&value);
                    require_nonblank("closure operation key", key)?;
                    Some(key.to_string())
                });
            }
            "--operation-key" => operation_key = Some(value),
            "--reason" if command != "reopen" => reason = Some(parse_closure_reason(&value)?),
            "--explanation" => explanation = Some(value),
            "--file" => file = Some(PathBuf::from(value)),
            "--reference" => {
                let (kind, target) = value
                    .split_once(':')
                    .ok_or_else(|| TicketCliError::new("--reference requires <kind>:<target>"))?;
                require_nonblank("reference kind", kind)?;
                require_nonblank("reference target", target)?;
                references.push(TicketReference {
                    kind: kind.into(),
                    target: target.into(),
                });
            }
            "--author" => author = Some(value),
            _ => {
                return Err(TicketCliError::new(format!(
                    "unknown {command} argument: {name}"
                )));
            }
        }
    }
    let action = match command {
        "reopen" => TicketCaseAction::Reopen,
        "close" => TicketCaseAction::Close {
            reason: reason.ok_or_else(|| TicketCliError::new("close requires --reason"))?,
        },
        "correct-closure" => TicketCaseAction::CorrectClosure {
            reason: reason
                .ok_or_else(|| TicketCliError::new("correct-closure requires --reason"))?,
        },
        _ => unreachable!("case command is selected by the command parser"),
    };
    let explanation = match (file, explanation) {
        (Some(path), None) => BodySource::File(path),
        (None, Some(text)) => BodySource::Message(text),
        _ => {
            return Err(TicketCliError::new(format!(
                "{command} requires exactly one of --explanation or --file"
            )));
        }
    };
    Ok(CaseOptions {
        query,
        expected_content_digest: digest
            .ok_or_else(|| TicketCliError::new("case action requires --expected-content-digest"))?,
        expected_state: state
            .ok_or_else(|| TicketCliError::new("case action requires --expected-state"))?,
        expected_closure_operation_key: closure_key.ok_or_else(|| {
            TicketCliError::new("case action requires --expected-closure-operation-key <key|none>")
        })?,
        operation_key: operation_key
            .ok_or_else(|| TicketCliError::new("case action requires --operation-key"))?,
        action,
        explanation,
        references,
        // A stable default keeps retries independent of process environment.
        author: author.unwrap_or_else(|| "yoi ticket".into()),
    })
}

fn require_nonblank(name: &str, value: &str) -> Result<(), TicketCliError> {
    if value.trim().is_empty() || value.contains('\0') {
        Err(TicketCliError::new(format!(
            "{name} must be nonblank and contain no NUL"
        )))
    } else {
        Ok(())
    }
}

fn parse_closure_reason(value: &str) -> Result<TicketClosureReason, TicketCliError> {
    match value {
        "completed" => Ok(TicketClosureReason::Completed),
        "not_planned" => Ok(TicketClosureReason::NotPlanned),
        "duplicate" => Ok(TicketClosureReason::Duplicate),
        "unknown" => Ok(TicketClosureReason::Unknown),
        _ => {
            if let Some(text) = value.strip_prefix("other:") {
                require_nonblank("other reason", text)?;
                if text.len() > 1024 {
                    return Err(TicketCliError::new(
                        "other reason must be at most 1024 UTF-8 bytes",
                    ));
                }
                Ok(TicketClosureReason::Other(text.into()))
            } else {
                Err(TicketCliError::new(
                    "--reason requires completed, not_planned, duplicate, unknown, or other:<text>",
                ))
            }
        }
    }
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
        "--reason",
        "--explanation",
        "--expected-content-digest",
        "--expected-state",
        "--expected-closure-operation-key",
        "--operation-key",
        "--reference",
        "--author",
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
            "--state active/all cannot be mixed with case states",
        ));
    }

    let mut states = Vec::new();
    for token in tokens {
        let state = TicketListState::parse(token).ok_or_else(|| {
            TicketCliError::new(format!(
                "invalid state: {token}; expected active, all, open, closed"
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
            "{command} accepts exactly one of --file or --message"
        ))),
        (Some(path), None) => Ok(BodySource::File(path)),
        (None, Some(message)) => Ok(BodySource::Message(ensure_trailing_newline(message))),
        (None, None) => Err(TicketCliError::new(format!(
            "{command} requires --file or --message"
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
    "yoi ticket

Usage:
  yoi ticket create --title <title>
  yoi ticket list [--state active|all|open|closed[,..]] [--limit <n>]
  yoi ticket show <id>
  yoi ticket comment <id> [--role comment|plan|decision|implementation_report] (--file <path>|--message <text>)
  yoi ticket close <id> --reason <reason> <case-options>
  yoi ticket reopen <id> <case-options>
  yoi ticket correct-closure <id> --reason <reason> <case-options>
  yoi ticket relation add --ticket <id> --kind <depends_on|blocks|related|supersedes|duplicate_of> --target <id> [--note <text>]
  yoi ticket relation list [--ticket <id>] [--kind <kind>]
  yoi ticket doctor

Case options (required for close, reopen, correct-closure):
  --expected-content-digest <sha256>   Content digest from show
  --expected-state <open|closed>       Observed case state
  --expected-closure-operation-key <key|none>   Last closure key from show, retained after reopening
  --operation-key <key>               Stable key for this decision
  --explanation <text> | --file <path> Exactly one nonblank explanation source
  --reference <kind>:<target>          Optional; repeat for multiple references
  --author <author>                    Optional; defaults to yoi ticket

Closure reasons:
  completed | not_planned | duplicate | unknown | other:<text>
  Closing does not imply completion; a reason must be explicitly supplied.

Replay:
  Repeat the same operation key, CAS values, reason, explanation bytes, references,
  and author for an exact replay. Do not refresh CAS values on a retry.
  Keep --file contents unchanged. Output reports current Backend case state,
  which may differ from the original action after subsequent decisions.
  The literal none denotes an absent closure key; active lists open cases.
  Prefix a closure key with key: to use a literal value (e.g. key:none or key:key:foo).

Options:
  -h, --help    Print help

Targets:
  Ticket commands require the Workspace-scoped Backend selected by the shared client Target.
  Standalone never falls back to repository-local Ticket storage.
"
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
    fn help_is_available_without_ticket_storage_authority() {
        let output = run(TicketCli::Help, ResolvedTarget::Standalone).unwrap();
        assert!(output.stdout.contains("Workspace-scoped Backend"));
        assert!(!output.stdout.contains("repository-file Ticket backend"));
    }

    fn case_args(command: &str, reason: Option<&str>) -> Vec<String> {
        let mut argv = args(&[
            command,
            "example",
            "--expected-content-digest",
            &"a".repeat(64),
            "--expected-state",
            "open",
            "--expected-closure-operation-key",
            "none",
            "--operation-key",
            "decision-1",
            "--explanation",
            "Intentional decision.",
        ]);
        if let Some(reason) = reason {
            argv.extend(args(&["--reason", reason]));
        }
        argv
    }

    fn command(argv: &[String]) -> TicketCommand {
        match parse_ticket_args(argv).unwrap() {
            TicketCli::Command(command) => command,
            TicketCli::Help => panic!("expected command"),
        }
    }

    fn replace_value(argv: &mut [String], flag: &str, value: &str) {
        let index = argv.iter().position(|arg| arg == flag).unwrap();
        argv[index + 1] = value.into();
    }

    #[test]
    fn case_commands_require_explicit_reason_and_explanation_with_replay_preconditions() {
        for name in ["close", "reopen", "correct-closure"] {
            let reason = (name != "reopen").then_some("unknown");
            let argv = case_args(name, reason);
            assert!(parse_ticket_args(&argv).is_ok(), "{name}");
            for flag in [
                "--expected-content-digest",
                "--expected-state",
                "--expected-closure-operation-key",
                "--operation-key",
                "--explanation",
            ] {
                let mut missing = argv.clone();
                let index = missing.iter().position(|arg| arg == flag).unwrap();
                missing.drain(index..index + 2);
                assert!(
                    parse_ticket_args(&missing).is_err(),
                    "{name} missing {flag}"
                );
                let mut blank = argv.clone();
                replace_value(&mut blank, flag, " \n ");
                assert!(parse_ticket_args(&blank).is_err(), "{name} blank {flag}");
            }
        }
        for name in ["close", "correct-closure"] {
            assert!(
                parse_ticket_args(&case_args(name, None))
                    .unwrap_err()
                    .to_string()
                    .contains("requires --reason")
            );
        }
        assert!(parse_ticket_args(&case_args("reopen", Some("completed"))).is_err());
        for name in ["state", "complete"] {
            assert!(parse_ticket_args(&args(&[name, "example", "closed"])).is_err());
        }
    }

    #[test]
    fn close_and_correction_parse_each_reason_without_a_success_default() {
        for name in ["close", "correct-closure"] {
            for (raw, expected) in [
                ("completed", TicketClosureReason::Completed),
                ("not_planned", TicketClosureReason::NotPlanned),
                ("duplicate", TicketClosureReason::Duplicate),
                ("unknown", TicketClosureReason::Unknown),
                (
                    "other:Deferred pending budget",
                    TicketClosureReason::Other("Deferred pending budget".into()),
                ),
            ] {
                let TicketCommand::Case(options) = command(&case_args(name, Some(raw))) else {
                    panic!()
                };
                let actual = match options.action {
                    TicketCaseAction::Close { reason }
                    | TicketCaseAction::CorrectClosure { reason } => reason,
                    TicketCaseAction::Reopen => panic!(),
                };
                assert_eq!(actual, expected);
            }
            for raw in [
                "success",
                "other:",
                "other:  ",
                "other:\0",
                &format!("other:{}", "é".repeat(513)),
            ] {
                assert!(
                    parse_ticket_args(&case_args(name, Some(raw))).is_err(),
                    "{raw:?}"
                );
            }
        }
    }

    #[test]
    fn case_parser_rejects_ambiguous_or_malformed_requests() {
        let base = case_args("close", Some("not_planned"));
        for (flag, value) in [
            ("--expected-content-digest", "not-a-digest"),
            ("--expected-state", "running"),
            ("--operation-key", "key\0"),
            ("--reference", "missing-separator"),
            ("--reference", ":target"),
            ("--reference", "result:"),
            ("--author", " "),
        ] {
            let mut argv = base.clone();
            if argv.iter().any(|arg| arg == flag) {
                replace_value(&mut argv, flag, value);
            } else {
                argv.extend(args(&[flag, value]));
            }
            assert!(parse_ticket_args(&argv).is_err(), "{flag} {value:?}");
        }
        for extra in [
            args(&["--file", "explanation.md"]),
            args(&["--operation-key", "second"]),
            args(&["--resolution", "old untyped close"]),
        ] {
            let mut argv = base.clone();
            argv.extend(extra);
            assert!(parse_ticket_args(&argv).is_err());
        }
    }

    #[test]
    fn list_accepts_only_case_states_and_active_means_open() {
        for (raw, expected) in [
            ("active", ListState::Active),
            ("all", ListState::All),
            ("open", ListState::States(vec![TicketListState::Open])),
            ("closed", ListState::States(vec![TicketListState::Closed])),
            (
                "open,closed,open",
                ListState::States(vec![TicketListState::Open, TicketListState::Closed]),
            ),
        ] {
            let TicketCommand::List(options) = command(&args(&["list", "--state", raw])) else {
                panic!()
            };
            assert_eq!(options.state, expected);
        }
        for raw in ["running", "active,closed", "all,open"] {
            assert!(parse_ticket_args(&args(&["list", "--state", raw])).is_err());
        }
        let fake = FakeBackend::new();
        run_backend_command(command(&args(&["list"])), &fake).unwrap();
        assert_eq!(
            fake.list_filter.borrow().as_ref(),
            Some(&TicketListQuery::active())
        );
    }

    // Narrow in-memory adapter: no Workspace, product process, network, or
    // filesystem Ticket storage. Unused authority methods panic so no fallback
    // event/state/storage operation can make these runtime tests pass.
    struct FakeBackend {
        current: std::cell::RefCell<ticket::Ticket>,
        receipts: std::cell::RefCell<std::collections::BTreeMap<String, TicketCaseUpdate>>,
        requests: std::cell::RefCell<Vec<(String, TicketCaseUpdate)>>,
        list_filter: std::cell::RefCell<Option<TicketListQuery>>,
    }

    fn fixture() -> ticket::Ticket {
        ticket::Ticket {
            meta: ticket::TicketMeta {
                id: "T-example".into(),
                resource_key: None,
                slug: "example".into(),
                title: "Case judgment".into(),
                status: ticket::ExtensibleTicketStatus::Open,
                kind: "task".into(),
                priority: "normal".into(),
                labels: vec![],
                created_at: None,
                created_by: None,
                updated_at: None,
                assignee: None,
                readiness: None,
                risk_flags: vec![],
                workflow_state: TicketState::Open,
                workflow_state_explicit: true,
                queued_by: None,
                queued_at: None,
                targets: vec![],
                raw: Default::default(),
            },
            document: ticket::TicketDocument {
                body: "Requirements".into(),
                raw_frontmatter: Default::default(),
            },
            events: vec![],
            artifacts: vec![],
            relations: Default::default(),
            resolution: None,
            closure_operation_key: None,
            current_closure: None,
            case_history: vec![],
            legacy_workflow_snapshot: None,
            tags: vec![],
        }
    }

    impl FakeBackend {
        fn new() -> Self {
            Self {
                current: std::cell::RefCell::new(fixture()),
                receipts: Default::default(),
                requests: Default::default(),
                list_filter: Default::default(),
            }
        }

        fn argv(&self, action: &str, reason: Option<&str>, key: &str) -> Vec<String> {
            let current = self.current.borrow();
            let mut argv = case_args(action, reason);
            replace_value(
                &mut argv,
                "--expected-content-digest",
                &ticket::ticket_content_digest(&current),
            );
            replace_value(
                &mut argv,
                "--expected-state",
                current.meta.workflow_state.as_str(),
            );
            replace_value(
                &mut argv,
                "--expected-closure-operation-key",
                &closure_key_token(current.closure_operation_key.as_deref()),
            );
            replace_value(&mut argv, "--operation-key", key);
            argv
        }
    }

    macro_rules! unused_backend_methods {
        ($(fn $name:ident(&self $(, $arg:ident: $ty:ty)*) -> $ret:ty;)*) => {
            $(fn $name(&self $(, _: $ty)*) -> $ret { panic!("unexpected authority method: {}", stringify!($name)) })*
        };
    }

    impl TicketBackend for FakeBackend {
        fn list(&self, filter: TicketListQuery) -> ticket::Result<Vec<TicketSummary>> {
            self.list_filter.replace(Some(filter));
            Ok(vec![])
        }
        fn show(&self, _: TicketIdOrSlug) -> ticket::Result<ticket::Ticket> {
            Ok(self.current.borrow().clone())
        }
        fn update_case(
            &self,
            id: &str,
            request: TicketCaseUpdate,
        ) -> ticket::Result<ticket::Ticket> {
            self.requests
                .borrow_mut()
                .push((id.into(), request.clone()));
            let mut current = self.current.borrow_mut();
            if let Some(receipt) = self.receipts.borrow().get(&request.operation_key) {
                if receipt != &request {
                    return Err(ticket::TicketError::OperationFingerprintMismatch {
                        operation_key: request.operation_key,
                    });
                }
                return Ok(current.clone());
            }
            let digest = ticket::ticket_content_digest(&current);
            if request.expected_content_digest != digest {
                return Err(ticket::TicketError::StaleContent {
                    expected: request.expected_content_digest,
                    actual: digest,
                });
            }
            if request.expected_state != current.meta.workflow_state {
                return Err(ticket::TicketError::StaleWorkflowState {
                    expected: request.expected_state.to_string(),
                    actual: current.meta.workflow_state.to_string(),
                });
            }
            if request.expected_closure_operation_key != current.closure_operation_key {
                return Err(ticket::TicketError::StaleClosureOperationKey {
                    expected: request.expected_closure_operation_key,
                    actual: current.closure_operation_key.clone(),
                });
            }
            match &request.action {
                TicketCaseAction::Close { reason }
                | TicketCaseAction::CorrectClosure { reason } => {
                    current.meta.workflow_state = TicketState::Closed;
                    current.closure_operation_key = Some(request.operation_key.clone());
                    current.current_closure = Some(ticket::TicketClosureJudgment {
                        operation_key: request.operation_key.clone(),
                        reason: reason.clone(),
                        explanation: request.explanation.clone(),
                        references: request.references.clone(),
                        author: request.author.clone().unwrap(),
                        at: "fixture-time".into(),
                    });
                }
                TicketCaseAction::Reopen => {
                    current.meta.workflow_state = TicketState::Open;
                    current.current_closure = None;
                }
            }
            self.receipts
                .borrow_mut()
                .insert(request.operation_key.clone(), request);
            Ok(current.clone())
        }
        unused_backend_methods! {
            fn edit_post_with_snapshots(&self, id: TicketIdOrSlug, edit: ticket::TicketPostEdit) -> ticket::Result<(ticket::Ticket, ticket::Ticket)>;
            fn list_workspace_tags(&self, request: ticket::TagListRequest) -> ticket::Result<Vec<ticket::WorkspaceTag>>;
            fn mutate_workspace_tag(&self, request: ticket::WorkspaceTagMutation) -> ticket::Result<ticket::WorkspaceTag>;
            fn set_ticket_tags(&self, id: TicketIdOrSlug, request: ticket::TicketTagSetRequest) -> ticket::Result<Vec<ticket::WorkspaceTag>>;
            fn default_intake_ready_state_change_body(&self, from: &str) -> String;
            fn create(&self, input: NewTicket) -> ticket::Result<ticket::TicketRef>;
            fn edit_item(&self, id: TicketIdOrSlug, edit: ticket::TicketItemEdit) -> ticket::Result<ticket::Ticket>;
            fn dependency_check(&self, id: TicketIdOrSlug) -> ticket::Result<ticket::TicketDependencyCheck>;
            fn add_event(&self, id: TicketIdOrSlug, event: NewTicketEvent) -> ticket::Result<()>;
            fn add_state_changed(&self, id: TicketIdOrSlug, change: ticket::TicketStateChange) -> ticket::Result<()>;
            fn add_intake_summary(&self, id: TicketIdOrSlug, summary: ticket::TicketIntakeSummary) -> ticket::Result<()>;
            fn mark_ready(&self, id: TicketIdOrSlug, request: ticket::TicketMarkReady) -> ticket::Result<ticket::Ticket>;
            fn queue_ready(&self, id: TicketIdOrSlug, queued_by: &str) -> ticket::Result<ticket::TicketQueueOutcome>;
            fn add_ticket_relation(&self, id: TicketIdOrSlug, relation: NewTicketRelation) -> ticket::Result<ticket::TicketRelation>;
            fn remove_ticket_relation(&self, id: TicketIdOrSlug, kind: TicketRelationKind, target: TicketIdOrSlug) -> ticket::Result<ticket::TicketRelation>;
            fn query_ticket_relations(&self, id: Option<TicketIdOrSlug>, kind: Option<TicketRelationKind>) -> ticket::Result<Vec<ticket::TicketRelation>>;
            fn relation_view(&self, id: TicketIdOrSlug) -> ticket::Result<ticket::TicketRelationView>;
            fn add_orchestration_plan_record(&self, id: TicketIdOrSlug, record: ticket::NewOrchestrationPlanRecord) -> ticket::Result<ticket::OrchestrationPlanRecord>;
            fn query_orchestration_plan_records(&self, id: Option<TicketIdOrSlug>, kind: Option<ticket::OrchestrationPlanKind>) -> ticket::Result<Vec<ticket::OrchestrationPlanRecord>>;
            fn doctor(&self) -> ticket::Result<ticket::TicketDoctorReport>;
        }
    }

    #[test]
    fn reasoned_actions_use_update_case_and_replay_reports_current_authority() {
        let fake = FakeBackend::new();
        let mut close = fake.argv("close", Some("not_planned"), "close-1");
        close.extend(args(&[
            "--author",
            "human:alice",
            "--reference",
            "document:https://example.invalid/result",
            "--reference",
            "ticket:T-other",
        ]));
        let output = run_backend_command(command(&close), &fake).unwrap();
        assert_eq!(
            output.stdout,
            "case\tT-example\tclosed\toperation-key:close-1\n"
        );
        let (id, request) = fake.requests.borrow()[0].clone();
        assert_eq!(id, "T-example");
        assert_eq!(request.explanation, "Intentional decision.");
        assert_eq!(request.author.as_deref(), Some("human:alice"));
        assert_eq!(request.references.len(), 2);
        assert_eq!(
            serde_json::to_value(&request).unwrap()["action"],
            serde_json::json!({"action": "close", "reason": "not_planned"})
        );
        assert_eq!(
            request.references[0].target,
            "https://example.invalid/result"
        );
        assert_eq!(
            request.action,
            TicketCaseAction::Close {
                reason: TicketClosureReason::NotPlanned
            }
        );
        let correct = fake.argv("correct-closure", Some("duplicate"), "correct-1");
        run_backend_command(command(&correct), &fake).unwrap();
        assert_eq!(
            fake.current
                .borrow()
                .current_closure
                .as_ref()
                .unwrap()
                .reason,
            TicketClosureReason::Duplicate
        );
        let reopen = fake.argv("reopen", None, "reopen-1");
        run_backend_command(command(&reopen), &fake).unwrap();
        let output = run_backend_command(command(&close), &fake).unwrap();
        assert_eq!(
            output.stdout,
            "case\tT-example\topen\toperation-key:close-1\n"
        );
        assert_eq!(fake.requests.borrow().last().unwrap().1, request);
        assert_eq!(fake.receipts.borrow().len(), 3);
        assert_eq!(
            fake.current.borrow().closure_operation_key.as_deref(),
            Some("correct-1")
        );
        let mut changed = close;
        replace_value(&mut changed, "--explanation", "A different judgment.");
        let error = run_backend_command(command(&changed), &fake).unwrap_err();
        assert!(error.to_string().contains("different request fingerprint"));
        assert_eq!(fake.current.borrow().meta.workflow_state, TicketState::Open);
    }

    #[test]
    fn stale_case_preconditions_propagate_backend_rejection_without_retry_or_success() {
        for (flag, value, diagnostic) in [
            (
                "--expected-content-digest",
                "b".repeat(64),
                "stale ticket content",
            ),
            (
                "--expected-state",
                "closed".into(),
                "stale ticket workflow state",
            ),
            (
                "--expected-closure-operation-key",
                "unobserved".into(),
                "stale Ticket closure operation key",
            ),
        ] {
            let fake = FakeBackend::new();
            let before = fake.current.borrow().clone();
            let mut argv = fake.argv("close", Some("completed"), "decision-1");
            replace_value(&mut argv, flag, &value);
            let error = run_backend_command(command(&argv), &fake).unwrap_err();
            assert!(error.to_string().contains(diagnostic), "{error}");
            assert_eq!(*fake.current.borrow(), before);
            assert_eq!(fake.requests.borrow().len(), 1);
            assert!(fake.receipts.borrow().is_empty());
        }
    }

    #[test]
    fn explanation_file_preserves_bytes_and_blank_files_never_submit_a_case_action() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("explanation.md");
        let fake = FakeBackend::new();
        let mut argv = fake.argv("close", Some("unknown"), "close-file");
        let index = argv.iter().position(|arg| arg == "--explanation").unwrap();
        argv[index] = "--file".into();
        argv[index + 1] = path.display().to_string();
        fs::write(&path, " \n ").unwrap();
        assert!(run_backend_command(command(&argv), &fake).is_err());
        assert!(fake.requests.borrow().is_empty());
        fs::write(&path, "Decision without trailing newline").unwrap();
        run_backend_command(command(&argv), &fake).unwrap();
        run_backend_command(command(&argv), &fake).unwrap();
        assert_eq!(
            fake.requests.borrow()[0].1.explanation,
            "Decision without trailing newline"
        );
        assert_eq!(fake.requests.borrow()[0].1, fake.requests.borrow()[1].1);
    }

    #[test]
    fn show_exposes_case_cas_and_explicit_closure_reason() {
        let fake = FakeBackend::new();
        let output = run_backend_command(command(&args(&["show", "example"])), &fake).unwrap();
        assert!(output.stdout.contains(&format!(
            "Content digest: {}",
            ticket::ticket_content_digest(&fake.current.borrow())
        )));
        assert!(output.stdout.contains("Closure operation key: none"));
        let argv = fake.argv("close", Some("not_planned"), "close-show");
        run_backend_command(command(&argv), &fake).unwrap();
        let output = run_backend_command(command(&args(&["show", "example"])), &fake).unwrap();
        assert!(output.stdout.contains("State: closed"));
        assert!(output.stdout.contains("Closure operation key: close-show"));
        assert!(output.stdout.contains("Closure reason: not_planned"));
        assert!(
            output
                .stdout
                .contains("Closure explanation: Intentional decision.")
        );
        assert!(!output.stdout.contains("Closure reason: completed"));
    }

    #[test]
    fn help_documents_reasoned_case_actions_and_exact_replay_not_bare_state_mutation() {
        let help = help_text();
        for snippet in [
            "yoi ticket close",
            "yoi ticket reopen",
            "yoi ticket correct-closure",
            "--reason",
            "--explanation",
            "--operation-key",
            "Do not refresh CAS values on a retry.",
            "active lists open cases",
        ] {
            assert!(help.contains(snippet), "{snippet}");
        }
        assert!(!help.contains("yoi ticket state"));
        assert!(!help.contains("--resolution"));
    }

    #[test]
    fn literal_closure_keys_and_tagged_actions_preserve_the_update_case_wire_contract() {
        for (token, key) in [
            ("none", None),
            ("key:none", Some("none")),
            ("key:key:example", Some("key:example")),
            ("close-1", Some("close-1")),
        ] {
            let mut argv = case_args("reopen", None);
            replace_value(&mut argv, "--expected-closure-operation-key", token);
            let TicketCommand::Case(options) = command(&argv) else {
                panic!()
            };
            assert_eq!(options.expected_closure_operation_key.as_deref(), key);
            assert_eq!(
                serde_json::to_value(&options.action).unwrap(),
                serde_json::json!({"action": "reopen"})
            );
        }
        let TicketCommand::Case(options) =
            command(&case_args("correct-closure", Some("other:Budget changed")))
        else {
            panic!()
        };
        assert_eq!(
            serde_json::to_value(&options.action).unwrap(),
            serde_json::json!({"action": "correct_closure", "reason": {"other": "Budget changed"}})
        );
    }
}
