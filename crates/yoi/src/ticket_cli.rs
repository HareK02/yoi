use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use ticket::config::{
    DEFAULT_TICKET_BACKEND_RELATIVE_PATH, TICKET_CONFIG_RELATIVE_PATH, TicketConfig,
    ticket_config_scaffold,
};
use ticket::{
    LocalTicketBackend, MarkdownText, NewTicket, NewTicketEvent, TicketBackend,
    TicketDoctorSeverity, TicketEventKind, TicketFilter, TicketIdOrSlug, TicketReview,
    TicketReviewResult, TicketWorkflowState,
};

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
    Doctor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateOptions {
    pub title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListState {
    Planning,
    Ready,
    Queued,
    InProgress,
    Done,
    Closed,
    All,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListOptions {
    pub state: ListState,
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
                TicketCommand::Doctor => doctor(&backend),
                TicketCommand::Init => unreachable!("init handled before backend setup"),
            }
        }
    }
}

fn init(workspace: &Path) -> Result<TicketCliOutput, TicketCliError> {
    let config_path = workspace.join(TICKET_CONFIG_RELATIVE_PATH);
    if config_path.exists() {
        return Err(TicketCliError::new(format!(
            "ticket config already exists at {}; refusing to overwrite. Edit it manually or remove it before running `yoi ticket init`.",
            config_path.display()
        )));
    }

    let yoi_dir = workspace.join(".yoi");
    fs::create_dir_all(&yoi_dir)?;
    let tickets_dir = workspace.join(DEFAULT_TICKET_BACKEND_RELATIVE_PATH);
    fs::create_dir_all(&tickets_dir)?;

    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&config_path)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                TicketCliError::new(format!(
                    "ticket config already exists at {}; refusing to overwrite. Edit it manually or remove it before running `yoi ticket init`.",
                    config_path.display()
                ))
            } else {
                TicketCliError::from(error)
            }
        })?;
    file.write_all(ticket_config_scaffold().as_bytes())?;

    Ok(success(format!(
        "created\t{}\nensured\t{}\n",
        TICKET_CONFIG_RELATIVE_PATH, DEFAULT_TICKET_BACKEND_RELATIVE_PATH
    )))
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
        ListState::Planning => TicketFilter::state(TicketWorkflowState::Planning),
        ListState::Ready => TicketFilter::state(TicketWorkflowState::Ready),
        ListState::Queued => TicketFilter::state(TicketWorkflowState::Queued),
        ListState::InProgress => TicketFilter::state(TicketWorkflowState::InProgress),
        ListState::Done => TicketFilter::state(TicketWorkflowState::Done),
        ListState::Closed => TicketFilter::state(TicketWorkflowState::Closed),
        ListState::All => TicketFilter::all(),
    };
    let tickets = backend.list(filter)?;
    let mut stdout = String::from("state\tid\ttitle\tupdated_at\n");
    for ticket in tickets {
        stdout.push_str(&format!(
            "{}\t{}\t{}\t{}\n",
            ticket.workflow_state.as_str(),
            ticket.id,
            ticket.title,
            ticket.updated_at.unwrap_or_default()
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
    matches!(key, "legacy_ticket" | "needs_preflight")
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
    let (from, to) = match options.state {
        StateTarget::Planning => (TicketWorkflowState::Planning, TicketWorkflowState::Planning),
        StateTarget::Ready => (TicketWorkflowState::Planning, TicketWorkflowState::Ready),
        StateTarget::Queued => (TicketWorkflowState::Ready, TicketWorkflowState::Queued),
        StateTarget::InProgress => (TicketWorkflowState::Queued, TicketWorkflowState::InProgress),
        StateTarget::Done => (TicketWorkflowState::InProgress, TicketWorkflowState::Done),
        StateTarget::Closed => {
            return Err(TicketCliError::new(
                "yoi ticket state <ticket> closed cannot write resolution.md; use `yoi ticket close <ticket> --resolution <text>` instead",
            ));
        }
    };
    backend.set_workflow_state(
        TicketIdOrSlug::Query(options.query.clone()),
        ticket::TicketStateChange {
            from: from.as_str().to_string(),
            to: to.as_str().to_string(),
            reason: "cli_state".to_string(),
            author: Some("yoi ticket".to_string()),
            body: format!("State changed to `{}`.\n", to.as_str()).into(),
            references: Vec::new(),
        },
    )?;
    Ok(success(format!(
        "state\t{}\t{}\n",
        options.query,
        to.as_str()
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
    let mut state = ListState::All;
    let mut i = 0;
    while i < args.len() {
        match option_with_value(args, &mut i)? {
            Some(("--state", value)) => state = parse_list_state(&value)?,
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
    Ok(ListOptions { state })
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

fn parse_list_state(value: &str) -> Result<ListState, TicketCliError> {
    match value {
        "planning" => Ok(ListState::Planning),
        "ready" => Ok(ListState::Ready),
        "queued" => Ok(ListState::Queued),
        "inprogress" => Ok(ListState::InProgress),
        "done" => Ok(ListState::Done),
        "closed" => Ok(ListState::Closed),
        "all" => Ok(ListState::All),
        _ => Err(TicketCliError::new(format!("invalid state: {value}"))),
    }
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
    "yoi ticket\n\nUsage:\n  yoi ticket init\n  yoi ticket create --title <title>\n  yoi ticket list [--state planning|ready|queued|inprogress|done|closed|all]\n  yoi ticket show <id>\n  yoi ticket comment <id> [--role comment|plan|decision|implementation_report] (--file <path>|--message <text>)\n  yoi ticket review <id> (--approve|--request-changes) (--file <path>|--message <text>)\n  yoi ticket state <id> <planning|ready|queued|inprogress|done|closed>\n  yoi ticket close <id> (--resolution <text>|--file <path>)\n  yoi ticket doctor\n\nOptions:\n  -h, --help    Print help\n\nBackend:\n  `yoi ticket init` writes .yoi/ticket.config.toml with explicit fixed role profiles and an optional commented [ticket].language setting.\n  Uses the workspace Ticket config at .yoi/ticket.config.toml when present.\n  Supported provider: builtin:yoi_local.\n  Without config, the local backend root is <cwd>/.yoi/tickets.\n"
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

    #[test]
    fn ticket_cli_init_writes_explicit_ticket_config_scaffold() {
        let temp = TempDir::new().unwrap();

        let initialized = run(&temp, &["init"]);
        assert_eq!(initialized.status, TicketCliStatus::Success);
        assert!(
            initialized
                .stdout
                .contains("created\t.yoi/ticket.config.toml")
        );
        assert!(initialized.stdout.contains("ensured\t.yoi/tickets"));
        assert!(temp.path().join(".yoi/tickets").exists());

        let config = fs::read_to_string(temp.path().join(".yoi/ticket.config.toml")).unwrap();
        assert!(config.contains("[backend]\n"));
        assert!(config.contains("provider = \"builtin:yoi_local\""));
        assert!(config.contains("root = \".yoi/tickets\""));
        assert!(config.contains("# [ticket]\n# language = \"Japanese\""));
        for role in TicketRole::ALL {
            assert!(config.contains(&format!(
                "[roles.{role}]\nprofile = \"builtin:default\"\nworkflow = \"{}\"",
                role.default_workflow()
            )));
        }
        assert!(!config.contains("[roles.investigator]"));
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
        assert!(err.to_string().contains("already exists"));
        assert!(err.to_string().contains("refusing to overwrite"));
        assert!(err.to_string().contains("yoi ticket init"));
        assert_eq!(
            fs::read_to_string(config_path).unwrap(),
            "[backend]\nprovider = \"builtin:yoi_local\"\n"
        );
    }

    #[test]
    fn ticket_cli_create_list_show_comment_review_status_close_and_doctor() {
        let temp = TempDir::new().unwrap();

        let created = run(
            &temp,
            &[
                "create",
                "--title",
                "CLI Created",
                "--slug",
                "cli-created",
                "--kind",
                "task",
                "--priority",
                "P1",
                "--label",
                "ticket,cli",
            ],
        );
        assert_eq!(created.status, TicketCliStatus::Success);
        assert!(created.stdout.contains("created\t"));
        assert!(created.stdout.contains("\tcli-created\topen"));
        assert!(temp.path().join(".yoi/tickets/open").exists());
        assert!(!temp.path().join("work-items").exists());
        let created_item = fs::read_to_string(
            temp.path()
                .join(".yoi/tickets/open")
                .join(created.stdout.split('\t').nth(1).unwrap())
                .join("item.md"),
        )
        .unwrap();
        assert!(!created_item.contains("legacy_ticket:"));
        assert!(!created_item.contains("needs_preflight:"));

        let listed = run(&temp, &["list", "--status", "open"]);
        assert!(listed.stdout.contains("status\tid\tslug"));
        assert!(listed.stdout.contains("CLI Created"));
        assert!(!listed.stdout.contains("legacy_ticket"));
        assert!(!listed.stdout.contains("needs_preflight"));

        let shown = run(&temp, &["show", "cli-created"]);
        assert!(shown.stdout.contains("# CLI Created"));
        assert!(shown.stdout.contains("Labels: ticket, cli"));
        assert!(!shown.stdout.contains("legacy_ticket"));
        assert!(!shown.stdout.contains("needs_preflight"));

        let commented = run(
            &temp,
            &[
                "comment",
                "cli-created",
                "--role",
                "implementation_report",
                "--message",
                "Implemented.",
            ],
        );
        assert!(
            commented
                .stdout
                .contains("appended\tcli-created\timplementation_report")
        );

        let reviewed = run(
            &temp,
            &[
                "review",
                "cli-created",
                "--approve",
                "--message",
                "Looks good.",
            ],
        );
        assert!(reviewed.stdout.contains("reviewed\tcli-created\tapprove"));

        let pending = run(&temp, &["status", "cli-created", "pending"]);
        assert!(pending.stdout.contains("status\tcli-created\tpending"));

        let closed = run(
            &temp,
            &[
                "close",
                "cli-created",
                "--resolution",
                "Done via yoi ticket.",
            ],
        );
        assert!(closed.stdout.contains("closed\tcli-created"));

        let doctor = run(&temp, &["doctor"]);
        assert_eq!(doctor.status, TicketCliStatus::Success);
        assert_eq!(doctor.stdout, "doctor: ok\n");

        let backend = LocalTicketBackend::new(temp.path().join(".yoi/tickets"));
        let ticket = backend
            .show(TicketIdOrSlug::Query("cli-created".to_string()))
            .unwrap();
        assert!(ticket.resolution.is_some());
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
    fn ticket_cli_uses_configured_backend_root() {
        let temp = TempDir::new().unwrap();
        fs::create_dir_all(temp.path().join(".yoi")).unwrap();
        fs::write(
            temp.path().join(".yoi/ticket.config.toml"),
            "[backend]\nprovider = \"builtin:yoi_local\"\nroot = \"custom-tickets\"\n",
        )
        .unwrap();

        run(
            &temp,
            &[
                "create",
                "--title",
                "Configured Root",
                "--slug",
                "configured-root",
            ],
        );

        assert!(
            temp.path()
                .join("custom-tickets/open")
                .read_dir()
                .unwrap()
                .any(|entry| entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains("configured-root"))
        );
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
    fn ticket_cli_status_closed_requires_close_command() {
        let temp = TempDir::new().unwrap();
        run(
            &temp,
            &["create", "--title", "Close Me", "--slug", "close-me"],
        );
        let cli = parse_ticket_args(&args(&["status", "close-me", "closed"])).unwrap();
        let err = run_in_workspace(cli, temp.path()).unwrap_err();
        assert!(err.to_string().contains("use `yoi ticket close"));
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
