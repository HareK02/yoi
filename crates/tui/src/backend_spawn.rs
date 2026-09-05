use client::{
    BackendCreateWorkerRequest, BackendWorkerLaunchOptions, BackendWorkerLaunchProfileCandidate,
    BackendWorkerLaunchRuntimeOption, BackendWorkerLaunchTarget, create_backend_worker,
    get_backend_worker_launch_options,
};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::backend_workspace_picker::select_backend_workspace;
use crate::console;
use crate::inline_terminal::{InlineTerminal, with_inline_terminal};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Name,
    Runtime,
    Profile,
}

impl Field {
    fn next(self) -> Self {
        match self {
            Self::Name => Self::Runtime,
            Self::Runtime => Self::Profile,
            Self::Profile => Self::Name,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::Name => Self::Profile,
            Self::Runtime => Self::Name,
            Self::Profile => Self::Runtime,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Selection {
    runtime_id: String,
    display_name: String,
    profile: String,
}

struct FormState {
    field: Field,
    display_name: String,
    runtime_index: usize,
    profile_index: usize,
    status: String,
}

impl FormState {
    fn new(options: &BackendWorkerLaunchOptions) -> Self {
        let runtime_index = options
            .runtimes
            .iter()
            .position(runtime_supports_workdirless_creation)
            .unwrap_or(0);
        let profile_index = options
            .default_profile
            .as_deref()
            .and_then(|default| {
                options
                    .profiles
                    .iter()
                    .position(|candidate| candidate.id == default)
            })
            .unwrap_or(0);
        Self {
            field: Field::Name,
            display_name: "Worker".to_string(),
            runtime_index,
            profile_index,
            status: String::new(),
        }
    }

    fn current_runtime<'a>(
        &self,
        options: &'a BackendWorkerLaunchOptions,
    ) -> Option<&'a BackendWorkerLaunchRuntimeOption> {
        options.runtimes.get(self.runtime_index)
    }

    fn current_profile<'a>(
        &self,
        options: &'a BackendWorkerLaunchOptions,
    ) -> Option<&'a BackendWorkerLaunchProfileCandidate> {
        options.profiles.get(self.profile_index)
    }

    fn cycle_runtime(&mut self, options: &BackendWorkerLaunchOptions, delta: isize) {
        self.runtime_index = cycle_index(self.runtime_index, options.runtimes.len(), delta);
        self.status.clear();
    }

    fn cycle_profile(&mut self, options: &BackendWorkerLaunchOptions, delta: isize) {
        self.profile_index = cycle_index(self.profile_index, options.profiles.len(), delta);
        self.status.clear();
    }

    fn submit(&mut self, options: &BackendWorkerLaunchOptions) -> Option<Selection> {
        let display_name = self.display_name.trim();
        if display_name.is_empty() {
            self.status = "Worker name is required.".to_string();
            self.field = Field::Name;
            return None;
        }
        let Some(runtime) = self.current_runtime(options) else {
            self.status = "No Runtime is available in this Workspace.".to_string();
            self.field = Field::Runtime;
            return None;
        };
        if !runtime.worker_creation_available {
            self.status = "The selected Runtime cannot create Workers right now.".to_string();
            self.field = Field::Runtime;
            return None;
        }
        if runtime.working_directory_required {
            self.status =
                "The selected Runtime requires a workdir; this launch flow does not select one yet."
                    .to_string();
            self.field = Field::Runtime;
            return None;
        }
        let Some(profile) = self.current_profile(options) else {
            self.status = "No Worker profile is available.".to_string();
            self.field = Field::Profile;
            return None;
        };
        Some(Selection {
            runtime_id: runtime.runtime_id.clone(),
            display_name: display_name.to_string(),
            profile: profile.id.clone(),
        })
    }
}

pub async fn run(mut target: BackendWorkerLaunchTarget) -> Result<(), Box<dyn std::error::Error>> {
    if target.workspace_id().is_none() {
        let Some(workspace) = select_backend_workspace(&target.base_url).await? else {
            return Ok(());
        };
        target.select_workspace(workspace);
    }

    let options = get_backend_worker_launch_options(&target).await?;
    let Some(selection) = select_worker(&options)? else {
        return Ok(());
    };
    let request = request_from_selection(selection);
    let created = create_backend_worker(&target, &request).await?;
    let runtime_target = target.runtime_target(created.runtime_id, created.worker_id)?;
    console::run_backend_runtime(runtime_target).await
}

fn request_from_selection(selection: Selection) -> BackendCreateWorkerRequest {
    BackendCreateWorkerRequest {
        runtime_id: selection.runtime_id,
        display_name: selection.display_name,
        profile: Some(selection.profile),
        initial_submit: Vec::new(),
        working_directory: None,
        ticket_assignment: None,
        control_operation_id: None,
    }
}

const VIEWPORT_LINES: u16 = 14;

fn select_worker(
    options: &BackendWorkerLaunchOptions,
) -> Result<Option<Selection>, Box<dyn std::error::Error>> {
    with_inline_terminal(VIEWPORT_LINES, |terminal| run_form(terminal, options))
}

fn run_form(
    terminal: &mut InlineTerminal,
    options: &BackendWorkerLaunchOptions,
) -> Result<Option<Selection>, Box<dyn std::error::Error>> {
    let mut state = FormState::new(options);

    loop {
        terminal.draw(|frame| render(frame, &state, options))?;
        let event = event::read()?;
        let Event::Key(key) = event else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Ok(None);
        }

        match key.code {
            KeyCode::Esc => {
                return Ok(None);
            }
            KeyCode::Tab | KeyCode::Down => {
                state.field = state.field.next();
                state.status.clear();
            }
            KeyCode::BackTab | KeyCode::Up => {
                state.field = state.field.previous();
                state.status.clear();
            }
            KeyCode::Left => match state.field {
                Field::Runtime => state.cycle_runtime(options, -1),
                Field::Profile => state.cycle_profile(options, -1),
                Field::Name => {}
            },
            KeyCode::Right => match state.field {
                Field::Runtime => state.cycle_runtime(options, 1),
                Field::Profile => state.cycle_profile(options, 1),
                Field::Name => {}
            },
            KeyCode::Enter => {
                if let Some(selection) = state.submit(options) {
                    return Ok(Some(selection));
                }
            }
            KeyCode::Backspace if state.field == Field::Name => {
                state.display_name.pop();
                state.status.clear();
            }
            KeyCode::Char(character)
                if state.field == Field::Name
                    && !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !character.is_control() =>
            {
                state.display_name.push(character);
                state.status.clear();
            }
            _ => {}
        }
    }
}

fn render(frame: &mut ratatui::Frame<'_>, state: &FormState, options: &BackendWorkerLaunchOptions) {
    let area = frame.area();
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(1),
        ])
        .split(area);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "New Backend Worker",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("  Workspace: {}", options.workspace_id)),
        ])),
        vertical[0],
    );

    let focused = Style::default().fg(Color::Cyan);
    frame.render_widget(
        Paragraph::new(state.display_name.as_str()).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Name ")
                .border_style(if state.field == Field::Name {
                    focused
                } else {
                    Style::default()
                }),
        ),
        vertical[1],
    );

    let runtime_text = state
        .current_runtime(options)
        .map(runtime_label)
        .unwrap_or_else(|| "No Runtime available".to_string());
    frame.render_widget(
        Paragraph::new(runtime_text).block(
            Block::default()
                .borders(Borders::ALL)
                .title(runtime_title(state, options))
                .border_style(if state.field == Field::Runtime {
                    focused
                } else {
                    Style::default()
                }),
        ),
        vertical[2],
    );

    let profile_text = state
        .current_profile(options)
        .map(|profile| {
            if profile.description.is_empty() {
                profile.label.clone()
            } else {
                format!("{} — {}", profile.label, profile.description)
            }
        })
        .unwrap_or_else(|| "No profile available".to_string());
    frame.render_widget(
        Paragraph::new(profile_text).block(
            Block::default()
                .borders(Borders::ALL)
                .title(profile_title(state, options))
                .border_style(if state.field == Field::Profile {
                    focused
                } else {
                    Style::default()
                }),
        ),
        vertical[3],
    );

    let status = if state.status.is_empty() {
        "Tab/↑/↓: field  ←/→: choice  Enter: create  Esc/Ctrl-C: cancel"
    } else {
        state.status.as_str()
    };
    frame.render_widget(
        Paragraph::new(status)
            .style(if state.status.is_empty() {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default().fg(Color::Yellow)
            })
            .wrap(Wrap { trim: true }),
        vertical[4],
    );

    if state.field == Field::Name {
        let max_cursor = vertical[1].width.saturating_sub(2) as usize;
        frame.set_cursor_position((
            vertical[1].x + 1 + state.display_name.chars().count().min(max_cursor) as u16,
            vertical[1].y + 1,
        ));
    }
}

fn runtime_title(state: &FormState, options: &BackendWorkerLaunchOptions) -> String {
    if options.runtimes.is_empty() {
        " Runtime ".to_string()
    } else {
        format!(
            " Runtime ({}/{}) ",
            state.runtime_index + 1,
            options.runtimes.len()
        )
    }
}

fn profile_title(state: &FormState, options: &BackendWorkerLaunchOptions) -> String {
    if options.profiles.is_empty() {
        " Profile ".to_string()
    } else {
        format!(
            " Profile ({}/{}) ",
            state.profile_index + 1,
            options.profiles.len()
        )
    }
}

fn runtime_label(runtime: &BackendWorkerLaunchRuntimeOption) -> String {
    let availability = if !runtime.worker_creation_available {
        "unavailable"
    } else if runtime.working_directory_required {
        "workdir required"
    } else {
        "no workdir"
    };
    format!(
        "{} [{}] — {availability}",
        runtime.display_name, runtime.runtime_id
    )
}

fn runtime_supports_workdirless_creation(runtime: &BackendWorkerLaunchRuntimeOption) -> bool {
    runtime.worker_creation_available && !runtime.working_directory_required
}

fn cycle_index(current: usize, len: usize, delta: isize) -> usize {
    if len == 0 {
        return 0;
    }
    (current as isize + delta).rem_euclid(len as isize) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use client::{BackendDiagnostic, BackendWorkerLaunchOptions};

    fn options() -> BackendWorkerLaunchOptions {
        BackendWorkerLaunchOptions {
            workspace_id: "workspace-1".to_string(),
            runtimes: vec![
                BackendWorkerLaunchRuntimeOption {
                    runtime_id: "external".to_string(),
                    display_name: "External".to_string(),
                    built_in: false,
                    worker_creation_available: true,
                    working_directory_required: true,
                    status: "online".to_string(),
                    diagnostics: Vec::new(),
                },
                BackendWorkerLaunchRuntimeOption {
                    runtime_id: "embedded".to_string(),
                    display_name: "Embedded".to_string(),
                    built_in: true,
                    worker_creation_available: true,
                    working_directory_required: false,
                    status: "online".to_string(),
                    diagnostics: Vec::new(),
                },
            ],
            profiles: vec![
                BackendWorkerLaunchProfileCandidate {
                    id: "builtin:default".to_string(),
                    label: "Default".to_string(),
                    description: String::new(),
                },
                BackendWorkerLaunchProfileCandidate {
                    id: "builtin:coder".to_string(),
                    label: "Coder".to_string(),
                    description: "Ticket implementation".to_string(),
                },
            ],
            default_profile: Some("builtin:coder".to_string()),
            repositories: Vec::new(),
            working_directories: Vec::new(),
            diagnostics: Vec::<BackendDiagnostic>::new(),
        }
    }

    #[test]
    fn defaults_to_workdirless_runtime_and_backend_default_profile() {
        let options = options();
        let state = FormState::new(&options);
        assert_eq!(
            state.current_runtime(&options).unwrap().runtime_id,
            "embedded"
        );
        assert_eq!(state.current_profile(&options).unwrap().id, "builtin:coder");
        assert_eq!(state.display_name, "Worker");
    }

    #[test]
    fn workdir_required_runtime_cannot_be_submitted() {
        let options = options();
        let mut state = FormState::new(&options);
        state.runtime_index = 0;
        assert_eq!(state.submit(&options), None);
        assert!(state.status.contains("requires a workdir"));
        assert_eq!(state.field, Field::Runtime);
    }

    #[test]
    fn selection_builds_workdirless_create_request() {
        let request = request_from_selection(Selection {
            runtime_id: "embedded".to_string(),
            display_name: "Coder one".to_string(),
            profile: "builtin:coder".to_string(),
        });
        assert_eq!(request.runtime_id, "embedded");
        assert_eq!(request.display_name, "Coder one");
        assert_eq!(request.profile.as_deref(), Some("builtin:coder"));
        assert!(request.initial_submit.is_empty());
        assert!(request.working_directory.is_none());
        assert!(request.ticket_assignment.is_none());
    }
}
