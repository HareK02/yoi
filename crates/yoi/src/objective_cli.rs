use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};

use chrono::Utc;
use serde::Deserialize;
use ticket::config::TicketConfig;

const OBJECTIVE_ROOT_RELATIVE_PATH: &str = ".yoi/objectives";
const REQUIRED_SECTION_HEADINGS: [&str; 5] = [
    "## Goal",
    "## Motivation / background",
    "## Strategy / design direction",
    "## Success criteria / exit conditions",
    "## Decision context",
];

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
    pub title: String,
    pub linked_tickets: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectiveListState {
    Active,
    Paused,
    Done,
    Archived,
    All,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListOptions {
    pub state: ObjectiveListState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectiveCliStatus {
    Success,
    Failure,
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

impl From<std::io::Error> for ObjectiveCliError {
    fn from(error: std::io::Error) -> Self {
        Self::new(error.to_string())
    }
}

impl From<ticket::config::TicketConfigError> for ObjectiveCliError {
    fn from(error: ticket::config::TicketConfigError) -> Self {
        Self::new(error.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObjectiveState {
    Active,
    Paused,
    Done,
    Archived,
}

impl ObjectiveState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Done => "done",
            Self::Archived => "archived",
        }
    }

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

#[derive(Debug, Clone, Deserialize)]
struct ObjectiveFrontmatter {
    title: String,
    state: String,
    created_at: String,
    updated_at: String,
    #[serde(default)]
    linked_tickets: Vec<String>,
}

#[derive(Debug, Clone)]
struct ObjectiveRecord {
    id: String,
    meta: ObjectiveFrontmatter,
    body: String,
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

pub fn run(cli: ObjectiveCli) -> Result<ObjectiveCliOutput, ObjectiveCliError> {
    let workspace = std::env::current_dir().map_err(|error| {
        ObjectiveCliError::new(format!("failed to resolve current directory: {error}"))
    })?;
    run_in_workspace(cli, &workspace)
}

pub fn run_in_workspace(
    cli: ObjectiveCli,
    workspace: &Path,
) -> Result<ObjectiveCliOutput, ObjectiveCliError> {
    match cli {
        ObjectiveCli::Help => Ok(success(help_text().to_string())),
        ObjectiveCli::Command(ObjectiveCommand::Create(options)) => create(workspace, options),
        ObjectiveCli::Command(ObjectiveCommand::List(options)) => list(workspace, options),
        ObjectiveCli::Command(ObjectiveCommand::Show { id }) => show(workspace, id),
        ObjectiveCli::Command(ObjectiveCommand::Doctor) => doctor(workspace),
    }
}

fn create(
    workspace: &Path,
    options: CreateOptions,
) -> Result<ObjectiveCliOutput, ObjectiveCliError> {
    let title = options.title.trim();
    if title.is_empty() {
        return Err(ObjectiveCliError::new("create --title must not be empty"));
    }
    validate_ticket_links(workspace, &options.linked_tickets)?;

    let root = objective_root(workspace);
    fs::create_dir_all(&root)?;
    let stamp = Utc::now().format("%Y%m%d-%H%M%S").to_string();
    let mut counter = 1_u32;
    let (id, dir) = loop {
        let candidate = format!("{stamp}-{counter:03}");
        let dir = root.join(&candidate);
        if !dir.exists() {
            break (candidate, dir);
        }
        counter += 1;
        if counter > 999 {
            return Err(ObjectiveCliError::new(format!(
                "too many objective id collisions for timestamp {stamp}"
            )));
        }
    };

    fs::create_dir_all(&dir)?;
    fs::write(
        dir.join("item.md"),
        render_objective_item(title, &options.linked_tickets),
    )?;
    Ok(success(format!("created\t{id}\n")))
}

fn list(workspace: &Path, options: ListOptions) -> Result<ObjectiveCliOutput, ObjectiveCliError> {
    let mut records = load_objectives(workspace)?;
    records.sort_by(|a, b| {
        b.meta
            .updated_at
            .cmp(&a.meta.updated_at)
            .then(a.id.cmp(&b.id))
    });
    let mut stdout = String::from("state\tid\ttitle\tupdated_at\tlinked_tickets\n");
    for record in records {
        let state = ObjectiveState::parse(&record.meta.state);
        if !list_state_matches(options.state, state) {
            continue;
        }
        stdout.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\n",
            record.meta.state,
            record.id,
            record.meta.title,
            record.meta.updated_at,
            record.meta.linked_tickets.join(",")
        ));
    }
    Ok(success(stdout))
}

fn show(workspace: &Path, id: String) -> Result<ObjectiveCliOutput, ObjectiveCliError> {
    validate_record_component(&id)?;
    let record = load_objective(&objective_root(workspace).join(&id), &id)?;
    let mut stdout = String::new();
    stdout.push_str(&format!("# {}\n\n", record.meta.title));
    stdout.push_str(&format!("State: {}\n", record.meta.state));
    stdout.push_str(&format!("ID: {}\n", record.id));
    stdout.push_str(&format!("Updated: {}\n", record.meta.updated_at));
    stdout.push_str("\n## item.md\n\n---\n");
    stdout.push_str(&format!("title: {}\n", yaml_string(&record.meta.title)));
    stdout.push_str(&format!("state: {}\n", yaml_string(&record.meta.state)));
    stdout.push_str(&format!(
        "created_at: {}\n",
        yaml_string(&record.meta.created_at)
    ));
    stdout.push_str(&format!(
        "updated_at: {}\n",
        yaml_string(&record.meta.updated_at)
    ));
    stdout.push_str(&format!(
        "linked_tickets: {}\n",
        yaml_string_array(&record.meta.linked_tickets)
    ));
    stdout.push_str("---\n\n");
    stdout.push_str(&record.body);
    if !stdout.ends_with('\n') {
        stdout.push('\n');
    }
    Ok(success(stdout))
}

fn doctor(workspace: &Path) -> Result<ObjectiveCliOutput, ObjectiveCliError> {
    let root = objective_root(workspace);
    if !root.exists() {
        return Ok(success("doctor: ok\n".to_string()));
    }
    let mut diagnostics = Vec::new();
    for entry in sorted_dirs(&root)? {
        let id = entry.file_name().to_string_lossy().to_string();
        if let Err(error) = validate_record_component(&id) {
            diagnostics.push(format!("error\t{id}\t{error}"));
            continue;
        }
        match load_objective(&entry.path(), &id) {
            Ok(record) => validate_record(workspace, &record, &mut diagnostics)?,
            Err(error) => diagnostics.push(format!("error\t{id}\t{error}")),
        }
    }
    if diagnostics.is_empty() {
        Ok(success("doctor: ok\n".to_string()))
    } else {
        let mut stdout = String::new();
        for diagnostic in diagnostics {
            stdout.push_str(&diagnostic);
            stdout.push('\n');
        }
        Ok(ObjectiveCliOutput {
            status: ObjectiveCliStatus::Failure,
            stdout,
        })
    }
}

fn validate_record(
    workspace: &Path,
    record: &ObjectiveRecord,
    diagnostics: &mut Vec<String>,
) -> Result<(), ObjectiveCliError> {
    if record.meta.title.trim().is_empty() {
        diagnostics.push(format!("error\t{}\ttitle must not be empty", record.id));
    }
    if ObjectiveState::parse(&record.meta.state).is_none() {
        diagnostics.push(format!(
            "error\t{}\tinvalid state {}; expected active|paused|done|archived",
            record.id, record.meta.state
        ));
    }
    for field in [
        record.meta.created_at.as_str(),
        record.meta.updated_at.as_str(),
    ] {
        if field.trim().is_empty() {
            diagnostics.push(format!(
                "error\t{}\ttimestamps must not be empty",
                record.id
            ));
        }
    }
    for heading in REQUIRED_SECTION_HEADINGS {
        if !record.body.contains(heading) {
            diagnostics.push(format!("error\t{}\tmissing section {heading}", record.id));
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    for ticket_id in &record.meta.linked_tickets {
        if !seen.insert(ticket_id) {
            diagnostics.push(format!(
                "warning\t{}\tduplicate linked ticket {}",
                record.id, ticket_id
            ));
        }
    }
    if let Err(error) = validate_ticket_links(workspace, &record.meta.linked_tickets) {
        diagnostics.push(format!("error\t{}\t{error}", record.id));
    }
    Ok(())
}

fn load_objectives(workspace: &Path) -> Result<Vec<ObjectiveRecord>, ObjectiveCliError> {
    let root = objective_root(workspace);
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut records = Vec::new();
    for entry in sorted_dirs(&root)? {
        let id = entry.file_name().to_string_lossy().to_string();
        records.push(load_objective(&entry.path(), &id)?);
    }
    Ok(records)
}

fn load_objective(dir: &Path, id: &str) -> Result<ObjectiveRecord, ObjectiveCliError> {
    validate_record_component(id)?;
    let path = dir.join("item.md");
    let raw = fs::read_to_string(&path)
        .map_err(|error| ObjectiveCliError::new(format!("{}: {error}", path.display())))?;
    let (frontmatter, body) = split_frontmatter(&raw).ok_or_else(|| {
        ObjectiveCliError::new(format!("{}: missing YAML frontmatter", path.display()))
    })?;
    let meta: ObjectiveFrontmatter = serde_yaml::from_str(frontmatter).map_err(|error| {
        ObjectiveCliError::new(format!(
            "{}: invalid YAML frontmatter: {error}",
            path.display()
        ))
    })?;
    Ok(ObjectiveRecord {
        id: id.to_string(),
        meta,
        body: body.to_string(),
    })
}

fn split_frontmatter(raw: &str) -> Option<(&str, &str)> {
    let rest = raw.strip_prefix("---\n")?;
    let (frontmatter, body) = rest.split_once("\n---\n")?;
    Some((frontmatter, body))
}

fn validate_ticket_links(workspace: &Path, ticket_ids: &[String]) -> Result<(), ObjectiveCliError> {
    let config = TicketConfig::load_workspace(workspace)?;
    let ticket_root = config.backend_root().to_path_buf();
    for ticket_id in ticket_ids {
        validate_record_component(ticket_id)?;
        let item = ticket_root.join(ticket_id).join("item.md");
        if !item.is_file() {
            return Err(ObjectiveCliError::new(format!(
                "linked ticket {ticket_id} does not exist as canonical Ticket id under {}",
                ticket_root.display()
            )));
        }
    }
    Ok(())
}

fn validate_record_component(value: &str) -> Result<(), ObjectiveCliError> {
    if value.is_empty() || value == "." || value == ".." {
        return Err(ObjectiveCliError::new(format!(
            "invalid path-derived id component: {value}"
        )));
    }
    let path = Path::new(value);
    if path.components().count() != 1 {
        return Err(ObjectiveCliError::new(format!(
            "invalid path-derived id component: {value}"
        )));
    }
    match path.components().next() {
        Some(Component::Normal(_)) => Ok(()),
        _ => Err(ObjectiveCliError::new(format!(
            "invalid path-derived id component: {value}"
        ))),
    }
}

fn sorted_dirs(root: &Path) -> Result<Vec<fs::DirEntry>, ObjectiveCliError> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            entries.push(entry);
        }
    }
    entries.sort_by_key(|entry| entry.file_name());
    Ok(entries)
}

fn objective_root(workspace: &Path) -> PathBuf {
    workspace.join(OBJECTIVE_ROOT_RELATIVE_PATH)
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

fn render_objective_item(title: &str, linked_tickets: &[String]) -> String {
    let now = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    format!(
        "---\ntitle: {}\nstate: {}\ncreated_at: {}\nupdated_at: {}\nlinked_tickets: {}\n---\n\n## Goal\n\nTBD\n\n## Motivation / background\n\nTBD\n\n## Strategy / design direction\n\nTBD\n\n## Success criteria / exit conditions\n\n- TBD\n\n## Decision context\n\n- TBD\n\n",
        yaml_string(title),
        yaml_string(ObjectiveState::Active.as_str()),
        yaml_string(&now),
        yaml_string(&now),
        yaml_string_array(linked_tickets)
    )
}

fn yaml_string(value: &str) -> String {
    format!("{:?}", value)
}

fn yaml_string_array(values: &[String]) -> String {
    if values.is_empty() {
        return "[]".to_string();
    }
    let items = values
        .iter()
        .map(|value| yaml_string(value))
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{items}]")
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

fn success(stdout: String) -> ObjectiveCliOutput {
    ObjectiveCliOutput {
        status: ObjectiveCliStatus::Success,
        stdout,
    }
}

fn help_text() -> &'static str {
    "yoi objective\n\nUsage:\n  yoi objective create --title <TITLE> [--ticket <TICKET_ID> ...]\n  yoi objective list [--state active|paused|done|archived|all]\n  yoi objective show <OBJECTIVE_ID>\n  yoi objective doctor\n\nObjective records are lightweight project records stored as .yoi/objectives/<objective-id>/item.md. Linked Tickets must be canonical opaque Ticket IDs; Objective links are non-blocking context, not Ticket dependencies.\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    fn run(temp: &TempDir, values: &[&str]) -> ObjectiveCliOutput {
        let cli = parse_objective_args(&args(values)).unwrap();
        run_in_workspace(cli, temp.path()).unwrap()
    }

    fn create_ticket_dir(temp: &TempDir, ticket_id: &str) {
        let dir = temp.path().join(".yoi/tickets").join(ticket_id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("item.md"),
            "---\ntitle: \"Ticket\"\nstate: \"planning\"\ncreated_at: \"2026-06-09T00:00:00Z\"\nupdated_at: \"2026-06-09T00:00:00Z\"\n---\n\nBody\n",
        )
        .unwrap();
    }

    fn created_id(output: &ObjectiveCliOutput) -> String {
        output
            .stdout
            .strip_prefix("created\t")
            .unwrap()
            .trim()
            .to_string()
    }

    #[test]
    fn objective_cli_creates_lists_and_shows_records() {
        let temp = TempDir::new().unwrap();
        create_ticket_dir(&temp, "20260608-125430-001");

        let created = run(
            &temp,
            &[
                "create",
                "--title",
                "Medium-term goal",
                "--ticket",
                "20260608-125430-001",
            ],
        );
        let objective_id = created_id(&created);
        assert!(
            temp.path()
                .join(".yoi/objectives")
                .join(&objective_id)
                .join("item.md")
                .exists()
        );

        let listed = run(&temp, &["list", "--state", "active"]);
        assert!(listed.stdout.contains(&objective_id));
        assert!(listed.stdout.contains("20260608-125430-001"));

        let shown = run(&temp, &["show", &objective_id]);
        assert!(shown.stdout.contains("# Medium-term goal"));
        assert!(
            shown
                .stdout
                .contains("## Success criteria / exit conditions")
        );
        assert!(shown.stdout.contains("20260608-125430-001"));
    }

    #[test]
    fn objective_cli_validates_ticket_links() {
        let temp = TempDir::new().unwrap();
        let cli = parse_objective_args(&args(&[
            "create",
            "--title",
            "Broken link",
            "--ticket",
            "missing-ticket",
        ]))
        .unwrap();
        let err = run_in_workspace(cli, temp.path()).unwrap_err();
        assert!(
            err.to_string()
                .contains("linked ticket missing-ticket does not exist")
        );
    }

    #[test]
    fn objective_doctor_reports_invalid_linked_ticket() {
        let temp = TempDir::new().unwrap();
        let dir = temp.path().join(".yoi/objectives/20260609-000000-001");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("item.md"),
            "---\ntitle: \"Goal\"\nstate: \"active\"\ncreated_at: \"2026-06-09T00:00:00Z\"\nupdated_at: \"2026-06-09T00:00:00Z\"\nlinked_tickets: [\"missing\"]\n---\n\n## Goal\n\nText\n\n## Motivation / background\n\nText\n\n## Strategy / design direction\n\nText\n\n## Success criteria / exit conditions\n\n- Text\n\n## Decision context\n\n- Text\n",
        )
        .unwrap();

        let output = run(&temp, &["doctor"]);
        assert_eq!(output.status, ObjectiveCliStatus::Failure);
        assert!(
            output
                .stdout
                .contains("linked ticket missing does not exist")
        );
    }

    #[test]
    fn objective_doctor_accepts_well_formed_records() {
        let temp = TempDir::new().unwrap();
        create_ticket_dir(&temp, "20260608-125430-001");
        run(
            &temp,
            &[
                "create",
                "--title",
                "Good objective",
                "--ticket",
                "20260608-125430-001",
            ],
        );

        let output = run(&temp, &["doctor"]);
        assert_eq!(output.status, ObjectiveCliStatus::Success);
        assert_eq!(output.stdout, "doctor: ok\n");
    }
}
