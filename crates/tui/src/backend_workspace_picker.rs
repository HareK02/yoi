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
use std::io::{self, IsTerminal, Write};
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
    println!("Create Workspace (leave display name empty to cancel)");
    let display_name = prompt_line("Workspace display name: ")?;
    if display_name.is_empty() {
        return Ok(None);
    }
    let uri = prompt_line("Initial repository absolute path/URI: ")?;
    if uri.is_empty() {
        println!("Repository path/URI is required.");
        return Ok(None);
    }
    let repository_name = prompt_line("Repository display name [Main]: ")?;
    let default_ref = prompt_line("Default ref [repository default]: ")?;
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
            display_name: Some(if repository_name.is_empty() {
                "Main".to_string()
            } else {
                repository_name
            }),
            default_ref: (!default_ref.is_empty()).then_some(default_ref),
        },
    }))
}

fn prompt_line(prompt: &str) -> PickerResult<String> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut value = String::new();
    io::stdin().read_line(&mut value)?;
    Ok(value.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
