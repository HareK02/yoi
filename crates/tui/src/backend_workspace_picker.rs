use client::{
    BackendWorkspace, BackendWorkspaceCatalogTarget, CreateBackendWorkspaceRepository,
    CreateBackendWorkspaceRequest, create_backend_workspace, list_backend_workspaces,
};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use std::error::Error;
use std::io::{self, BufRead, IsTerminal, Write};
use std::time::{SystemTime, UNIX_EPOCH};

type PickerResult<T> = Result<T, Box<dyn Error>>;

pub(crate) async fn select_backend_workspace(base_url: &str) -> PickerResult<Option<String>> {
    let target = BackendWorkspaceCatalogTarget::new(base_url);
    let mut workspaces = Vec::new();

    'catalog: loop {
        let error = match list_backend_workspaces(&target).await {
            Ok(items) => {
                workspaces = items;
                None
            }
            Err(fetch_error) => Some(format!("failed to refresh workspaces: {fetch_error}")),
        };

        match pick_workspace(&workspaces, error.as_deref())? {
            WorkspacePickerAction::Select(index) => {
                return Ok(workspaces.get(index).map(|item| item.workspace_id.clone()));
            }
            WorkspacePickerAction::Refresh => continue,
            WorkspacePickerAction::Create => {
                let Some(request) = prompt_create_request()? else {
                    continue;
                };
                loop {
                    match create_backend_workspace(&target, &request).await {
                        Ok(response) => return Ok(Some(response.workspace.workspace_id)),
                        Err(create_error) => {
                            let creation_error =
                                format!("workspace creation failed: {create_error}");
                            match pick_workspace(&workspaces, Some(&creation_error))? {
                                WorkspacePickerAction::Select(index) => {
                                    return Ok(workspaces
                                        .get(index)
                                        .map(|item| item.workspace_id.clone()));
                                }
                                // Retry the exact request and operation key.
                                WorkspacePickerAction::Create => continue,
                                WorkspacePickerAction::Refresh => continue 'catalog,
                                WorkspacePickerAction::Cancel => return Ok(None),
                            }
                        }
                    }
                }
            }
            WorkspacePickerAction::Cancel => return Ok(None),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkspacePickerAction {
    Select(usize),
    Create,
    Refresh,
    Cancel,
}

fn pick_workspace(
    workspaces: &[BackendWorkspace],
    error: Option<&str>,
) -> PickerResult<WorkspacePickerAction> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(
            "Backend target has no configured workspace; an interactive terminal is required to choose one"
                .into(),
        );
    }
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut selected = 0usize;
    loop {
        terminal.draw(|frame| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Min(3),
                    Constraint::Length(if error.is_some() { 3 } else { 1 }),
                ])
                .split(frame.area());
            frame.render_widget(
                Paragraph::new("Choose the Workspace for this Backend session")
                    .block(Block::default().title("Workspace").borders(Borders::ALL)),
                chunks[0],
            );
            let rows = workspaces
                .iter()
                .map(|workspace| {
                    ListItem::new(Line::from(vec![
                        Span::styled(
                            workspace.display_name.clone(),
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                        Span::raw(format!("  {}  {}", workspace.workspace_id, workspace.state)),
                    ]))
                })
                .collect::<Vec<_>>();
            let rows = if rows.is_empty() {
                vec![ListItem::new("No accessible Workspaces")]
            } else {
                rows
            };
            let mut state = ListState::default();
            if !workspaces.is_empty() {
                state.select(Some(selected));
            }
            frame.render_stateful_widget(
                List::new(rows)
                    .block(Block::default().borders(Borders::ALL))
                    .highlight_symbol("▶ "),
                chunks[1],
                &mut state,
            );
            let footer = error
                .map(|message| {
                    format!(
                        "{message}  [n] create/retry  [r] refresh  [Enter] select  [Esc] cancel"
                    )
                })
                .unwrap_or_else(|| {
                    "[Enter] select  [n] new  [r] refresh  [Esc] cancel".to_string()
                });
            frame.render_widget(Paragraph::new(footer), chunks[2]);
        })?;

        if let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Up if !workspaces.is_empty() => {
                    selected = selected.saturating_sub(1);
                }
                KeyCode::Down if !workspaces.is_empty() => {
                    selected = (selected + 1).min(workspaces.len() - 1);
                }
                KeyCode::Enter if !workspaces.is_empty() => {
                    terminal.clear()?;
                    return Ok(WorkspacePickerAction::Select(selected));
                }
                KeyCode::Char('n') => {
                    terminal.clear()?;
                    return Ok(WorkspacePickerAction::Create);
                }
                KeyCode::Char('r') => {
                    terminal.clear()?;
                    return Ok(WorkspacePickerAction::Refresh);
                }
                KeyCode::Esc | KeyCode::Char('q') => {
                    terminal.clear()?;
                    return Ok(WorkspacePickerAction::Cancel);
                }
                _ => {}
            }
        }
    }
}

fn prompt_create_request() -> PickerResult<Option<CreateBackendWorkspaceRequest>> {
    disable_raw_mode()?;
    let result = prompt_create_request_inner();
    enable_raw_mode()?;
    result
}

fn prompt_create_request_inner() -> PickerResult<Option<CreateBackendWorkspaceRequest>> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    prompt_create_request_with_io(&mut stdin.lock(), &mut stdout.lock())
}

fn prompt_create_request_with_io(
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> PickerResult<Option<CreateBackendWorkspaceRequest>> {
    writeln!(
        output,
        "Create Workspace (leave display name empty to cancel)"
    )?;
    let Some(display_name) = prompt_line(input, output, "Workspace display name: ")? else {
        return Ok(None);
    };
    if display_name.is_empty() {
        return Ok(None);
    }
    let Some(uri) = prompt_line(input, output, "Initial repository absolute path/URI: ")? else {
        return Ok(None);
    };
    if uri.is_empty() {
        writeln!(output, "Repository path/URI is required.")?;
        return Ok(None);
    }
    let repository_key = loop {
        let Some(repository_key) = prompt_line(input, output, "Repository key (required): ")?
        else {
            return Ok(None);
        };
        if repository_key.is_empty() {
            writeln!(output, "Repository key is required.")?;
            continue;
        }
        break repository_key;
    };
    let Some(default_ref) = prompt_line(input, output, "Default ref [repository default]: ")?
    else {
        return Ok(None);
    };
    let operation_key = format!(
        "tui-workspace-create-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    Ok(Some(CreateBackendWorkspaceRequest {
        operation_key,
        display_name,
        repository: CreateBackendWorkspaceRepository {
            uri,
            repository_key,
            default_ref: (!default_ref.is_empty()).then_some(default_ref),
        },
    }))
}

fn prompt_line(
    input: &mut impl BufRead,
    output: &mut impl Write,
    prompt: &str,
) -> PickerResult<Option<String>> {
    write!(output, "{prompt}")?;
    output.flush()?;
    let mut value = String::new();
    if input.read_line(&mut value)? == 0 {
        return Ok(None);
    }
    Ok(Some(value.trim().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn prompt_create(input: &str) -> (Option<CreateBackendWorkspaceRequest>, String) {
        let mut input = Cursor::new(input.as_bytes());
        let mut output = Vec::new();
        let request = prompt_create_request_with_io(&mut input, &mut output).unwrap();
        (request, String::from_utf8(output).unwrap())
    }

    #[test]
    fn picker_actions_distinguish_switch_refresh_create_and_cancel() {
        assert_ne!(
            WorkspacePickerAction::Create,
            WorkspacePickerAction::Refresh
        );
        assert_ne!(
            WorkspacePickerAction::Select(0),
            WorkspacePickerAction::Cancel
        );
    }

    #[test]
    fn workspace_creation_preserves_an_explicit_non_main_repository_key() {
        let (request, output) = prompt_create("Platform\n/srv/platform\nplatform\ndevelop\n");
        let request = request.expect("complete input should create a request");

        assert_eq!(request.display_name, "Platform");
        assert_eq!(request.repository.uri, "/srv/platform");
        assert_eq!(request.repository.repository_key, "platform");
        assert_eq!(request.repository.default_ref.as_deref(), Some("develop"));
        assert!(output.contains("Repository key (required): "));
        assert!(!output.contains("Repository key [main]"));
    }

    #[test]
    fn workspace_creation_reprompts_for_blank_repository_keys() {
        let (request, output) = prompt_create("Platform\n/srv/platform\n\n   \nplatform\n\n");
        let request = request.expect("a later explicit key should create a request");

        assert_eq!(request.repository.repository_key, "platform");
        assert_eq!(output.matches("Repository key is required.").count(), 2);
        assert_eq!(output.matches("Repository key (required): ").count(), 3);
    }

    #[test]
    fn workspace_creation_accepts_explicit_main_repository_key() {
        let (request, _) = prompt_create("Workspace\n/srv/repository\nmain\n\n");

        assert_eq!(
            request.unwrap().repository.repository_key,
            "main",
            "main remains valid when the creator enters it explicitly"
        );
    }

    #[test]
    fn workspace_creation_cancels_on_eof_instead_of_retrying_forever() {
        let (request, output) = prompt_create("Workspace\n/srv/repository\n   \n");

        assert!(request.is_none());
        assert_eq!(output.matches("Repository key is required.").count(), 1);
        assert_eq!(output.matches("Repository key (required): ").count(), 2);
    }
}
