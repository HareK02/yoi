use std::fmt;

use client::{BackendWorkspaceProductClient, ResolvedTarget};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectiveCli {
    Help,
    Command(ObjectiveCommand),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectiveCommand {
    Create(CreateOptions),
    List(ListOptions),
    Show { id: String },
    Doctor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateOptions {
    title: String,
    linked_tickets: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObjectiveListState {
    Active,
    Paused,
    Done,
    Archived,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListOptions {
    state: ObjectiveListState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectiveCliStatus {
    Success,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectiveCliOutput {
    pub status: ObjectiveCliStatus,
    pub stdout: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectiveCliError(String);

impl ObjectiveCliError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ObjectiveCliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ObjectiveCliError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObjectiveState {
    Active,
    Paused,
    Done,
    Archived,
}

impl ObjectiveState {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "paused" => Some(Self::Paused),
            "done" => Some(Self::Done),
            "archived" => Some(Self::Archived),
            _ => None,
        }
    }
}

pub fn parse_objective_args(args: &[String]) -> Result<ObjectiveCli, ObjectiveCliError> {
    if args.is_empty() || args.iter().any(|arg| arg == "--help" || arg == "-h") {
        return Ok(ObjectiveCli::Help);
    }

    let command = match args[0].as_str() {
        "create" => ObjectiveCommand::Create(parse_create(&args[1..])?),
        "list" => ObjectiveCommand::List(parse_list(&args[1..])?),
        "show" => ObjectiveCommand::Show {
            id: parse_one_positional("show", &args[1..])?,
        },
        "doctor" => {
            if args.len() != 1 {
                return Err(ObjectiveCliError::new(
                    "objective doctor takes no arguments",
                ));
            }
            ObjectiveCommand::Doctor
        }
        "help" => return Ok(ObjectiveCli::Help),
        other => {
            return Err(ObjectiveCliError::new(format!(
                "unknown objective command: {other}"
            )));
        }
    };

    Ok(ObjectiveCli::Command(command))
}

pub fn run(
    cli: ObjectiveCli,
    target: ResolvedTarget,
) -> Result<ObjectiveCliOutput, ObjectiveCliError> {
    match target {
        ResolvedTarget::Standalone => Err(ObjectiveCliError::new(
            "Standalone is a one-shot Worker host, not Objective storage authority; select a Backend target",
        )),
        ResolvedTarget::Backend {
            base_url,
            workspace_id,
        } => {
            let backend = BackendWorkspaceProductClient::new(base_url, workspace_id)
                .map_err(|error| ObjectiveCliError::new(error.to_string()))?;
            run_with_backend(cli, &backend)
        }
    }
}

fn run_with_backend(
    cli: ObjectiveCli,
    backend: &BackendWorkspaceProductClient,
) -> Result<ObjectiveCliOutput, ObjectiveCliError> {
    match cli {
        ObjectiveCli::Help => Ok(success(help_text().to_string())),
        ObjectiveCli::Command(ObjectiveCommand::Create(options)) => {
            let title = options.title.trim();
            if title.is_empty() {
                return Err(ObjectiveCliError::new("create --title must not be empty"));
            }
            let objective = backend
                .create_objective(&workspace_api::ObjectiveCreateRequest {
                    title: title.to_string(),
                    body_md: objective_body_template(),
                    state: "active".to_string(),
                    linked_tickets: options.linked_tickets,
                })
                .map_err(|error| ObjectiveCliError::new(error.to_string()))?;
            Ok(success(format!("created\t{}\n", objective.id)))
        }
        ObjectiveCli::Command(ObjectiveCommand::List(options)) => {
            let response = backend
                .list_objectives(BackendWorkspaceProductClient::default_product_list_limit())
                .map_err(|error| ObjectiveCliError::new(error.to_string()))?;
            let mut stdout = String::from("state\tid\ttitle\tupdated_at\tlinked_tickets\n");
            for objective in response.items {
                let state = ObjectiveState::parse(&objective.state);
                if !list_state_matches(options.state, state) {
                    continue;
                }
                stdout.push_str(&format!(
                    "{}\t{}\t{}\t{}\t{}\n",
                    objective.state,
                    objective.id,
                    objective.title,
                    objective.updated_at.unwrap_or_default(),
                    objective.linked_tickets.join(",")
                ));
            }
            Ok(success(stdout))
        }
        ObjectiveCli::Command(ObjectiveCommand::Show { id }) => {
            let objective = backend
                .show_objective(&id)
                .map_err(|error| ObjectiveCliError::new(error.to_string()))?;
            let mut stdout = String::new();
            stdout.push_str(&format!("# {}\n\n", objective.title));
            stdout.push_str(&format!("State: {}\n", objective.state));
            stdout.push_str(&format!("ID: {}\n", objective.id));
            stdout.push_str(&format!(
                "Updated: {}\n\n## item.md\n\n",
                objective.updated_at.unwrap_or_default()
            ));
            stdout.push_str(&objective.body);
            if !stdout.ends_with('\n') {
                stdout.push('\n');
            }
            Ok(success(stdout))
        }
        ObjectiveCli::Command(ObjectiveCommand::Doctor) => {
            let response = backend
                .list_objectives(BackendWorkspaceProductClient::default_product_list_limit())
                .map_err(|error| ObjectiveCliError::new(error.to_string()))?;
            for objective in response.items {
                backend
                    .show_objective(&objective.id)
                    .map_err(|error| ObjectiveCliError::new(error.to_string()))?;
            }
            Ok(success("doctor: ok\n".to_string()))
        }
    }
}

fn parse_create(args: &[String]) -> Result<CreateOptions, ObjectiveCliError> {
    let mut title = None;
    let mut linked_tickets = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match option_with_value(args, &mut i)? {
            Some(("--title", value)) => title = Some(value),
            Some(("--ticket", value)) => linked_tickets.push(value),
            Some((name, _)) => {
                return Err(ObjectiveCliError::new(format!(
                    "unknown create argument: {name}"
                )));
            }
            None => {
                return Err(ObjectiveCliError::new(format!(
                    "unknown create argument: {}",
                    args[i]
                )));
            }
        }
    }
    let title = title.ok_or_else(|| ObjectiveCliError::new("create requires --title"))?;
    Ok(CreateOptions {
        title,
        linked_tickets,
    })
}

fn parse_list(args: &[String]) -> Result<ListOptions, ObjectiveCliError> {
    let mut state = ObjectiveListState::All;
    let mut i = 0;
    while i < args.len() {
        match option_with_value(args, &mut i)? {
            Some(("--state", value)) => state = parse_list_state(&value)?,
            Some((name, _)) => {
                return Err(ObjectiveCliError::new(format!(
                    "unknown list argument: {name}"
                )));
            }
            None => {
                return Err(ObjectiveCliError::new(format!(
                    "unknown list argument: {}",
                    args[i]
                )));
            }
        }
    }
    Ok(ListOptions { state })
}

fn parse_one_positional(command: &str, args: &[String]) -> Result<String, ObjectiveCliError> {
    if args.len() != 1 || args[0].starts_with('-') {
        Err(ObjectiveCliError::new(format!("{command} requires <id>")))
    } else {
        Ok(args[0].clone())
    }
}

fn option_with_value(
    args: &[String],
    i: &mut usize,
) -> Result<Option<(&'static str, String)>, ObjectiveCliError> {
    let arg = &args[*i];
    for name in ["--title", "--ticket", "--state"] {
        if arg == name {
            let value = args
                .get(*i + 1)
                .ok_or_else(|| ObjectiveCliError::new(format!("{name} requires a value")))?;
            if value.starts_with('-') {
                return Err(ObjectiveCliError::new(format!("{name} requires a value")));
            }
            *i += 2;
            return Ok(Some((name, value.clone())));
        }
        if let Some(value) = arg.strip_prefix(&format!("{name}=")) {
            if value.is_empty() {
                return Err(ObjectiveCliError::new(format!("{name} requires a value")));
            }
            *i += 1;
            return Ok(Some((name, value.to_string())));
        }
    }
    Ok(None)
}

fn parse_list_state(value: &str) -> Result<ObjectiveListState, ObjectiveCliError> {
    match value {
        "active" => Ok(ObjectiveListState::Active),
        "paused" => Ok(ObjectiveListState::Paused),
        "done" => Ok(ObjectiveListState::Done),
        "archived" => Ok(ObjectiveListState::Archived),
        "all" => Ok(ObjectiveListState::All),
        _ => Err(ObjectiveCliError::new(format!(
            "invalid objective state: {value}"
        ))),
    }
}

fn list_state_matches(filter: ObjectiveListState, state: Option<ObjectiveState>) -> bool {
    match filter {
        ObjectiveListState::All => true,
        ObjectiveListState::Active => state == Some(ObjectiveState::Active),
        ObjectiveListState::Paused => state == Some(ObjectiveState::Paused),
        ObjectiveListState::Done => state == Some(ObjectiveState::Done),
        ObjectiveListState::Archived => state == Some(ObjectiveState::Archived),
    }
}

fn objective_body_template() -> String {
    "## Goal\n\nTBD\n\n## Motivation / background\n\nTBD\n\n## Strategy / design direction\n\nTBD\n\n## Success criteria / exit conditions\n\n- TBD\n\n## Decision context\n\n- TBD\n"
        .to_string()
}

fn success(stdout: String) -> ObjectiveCliOutput {
    ObjectiveCliOutput {
        status: ObjectiveCliStatus::Success,
        stdout,
    }
}

fn help_text() -> &'static str {
    "yoi objective\n\nUsage:\n  yoi objective create --title <TITLE> [--ticket <TICKET_ID> ...]\n  yoi objective list [--state active|paused|done|archived|all]\n  yoi objective show <OBJECTIVE_ID>\n  yoi objective doctor\n\nObjective commands require the Workspace-scoped Backend selected by the shared client Target. Standalone does not provide Objective authority.\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn standalone_rejects_objective_storage_operations() {
        let cli = parse_objective_args(&args(&["list"])).unwrap();
        let error = run(cli, ResolvedTarget::Standalone).unwrap_err();
        assert!(error.to_string().contains("select a Backend target"));
    }

    #[test]
    fn objective_parser_keeps_backend_command_contract() {
        assert!(matches!(
            parse_objective_args(&args(&["create", "--title", "Goal"])).unwrap(),
            ObjectiveCli::Command(ObjectiveCommand::Create(_))
        ));
        assert!(matches!(
            parse_objective_args(&args(&["list", "--state", "active"])).unwrap(),
            ObjectiveCli::Command(ObjectiveCommand::List(_))
        ));
    }

    #[test]
    fn help_states_backend_authority() {
        assert!(help_text().contains("require the Workspace-scoped Backend"));
        assert!(!help_text().contains("repository-file"));
    }
}
