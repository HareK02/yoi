use std::io;
use std::path::Path;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use manifest::ProfileDiscovery;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use thiserror::Error;

use crate::inline_terminal::{InlineTerminal, with_inline_terminal};

const VIEWPORT_HEIGHT: u16 = 6;
const FALLBACK_WORKER_NAME: &str = "worker";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StandaloneSpawnSelection {
    pub worker_name: String,
    pub profile: String,
}

#[derive(Debug, Error)]
pub(crate) enum StandaloneSpawnError {
    #[error("profile discovery failed: {0}")]
    ProfileDiscovery(#[from] manifest::ProfileError),
    #[error("no profiles are available")]
    NoProfiles,
    #[error("standalone spawn picker terminal error: {0}")]
    Terminal(#[from] io::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProfileChoice {
    selector: String,
    label: String,
    is_default: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatusKind {
    Info,
    Progress,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpawnAction {
    None,
    Submit,
    Cancel,
}

struct SpawnForm {
    worker_name: String,
    cursor: usize,
    profile_choices: Vec<ProfileChoice>,
    selected_profile: usize,
    status: Option<(String, StatusKind)>,
}

impl SpawnForm {
    fn new(
        worker_name: Option<String>,
        default_worker_name: String,
        profile_choices: Vec<ProfileChoice>,
    ) -> Self {
        let worker_name = worker_name.unwrap_or(default_worker_name);
        let cursor = worker_name.chars().count();
        let selected_profile = profile_choices
            .iter()
            .position(|choice| choice.is_default)
            .unwrap_or(0);
        Self {
            worker_name,
            cursor,
            profile_choices,
            selected_profile,
            status: None,
        }
    }

    fn selected_profile(&self) -> &ProfileChoice {
        &self.profile_choices[self.selected_profile]
    }

    fn apply_key(&mut self, key: KeyEvent) -> SpawnAction {
        if key.kind == KeyEventKind::Release {
            return SpawnAction::None;
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('c') | KeyCode::Char('u') => return SpawnAction::Cancel,
                _ => return SpawnAction::None,
            }
        }

        self.status = None;
        match key.code {
            KeyCode::Esc => SpawnAction::Cancel,
            KeyCode::Enter => {
                if self.worker_name.trim().is_empty() {
                    self.status =
                        Some(("worker name cannot be empty".to_owned(), StatusKind::Error));
                    SpawnAction::None
                } else {
                    SpawnAction::Submit
                }
            }
            KeyCode::Tab | KeyCode::Down => {
                self.selected_profile = (self.selected_profile + 1) % self.profile_choices.len();
                SpawnAction::None
            }
            KeyCode::BackTab | KeyCode::Up => {
                self.selected_profile = if self.selected_profile == 0 {
                    self.profile_choices.len() - 1
                } else {
                    self.selected_profile - 1
                };
                SpawnAction::None
            }
            KeyCode::Left => {
                self.cursor = self.cursor.saturating_sub(1);
                SpawnAction::None
            }
            KeyCode::Right => {
                self.cursor = (self.cursor + 1).min(self.worker_name.chars().count());
                SpawnAction::None
            }
            KeyCode::Home => {
                self.cursor = 0;
                SpawnAction::None
            }
            KeyCode::End => {
                self.cursor = self.worker_name.chars().count();
                SpawnAction::None
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    let idx = byte_index(&self.worker_name, self.cursor - 1);
                    self.worker_name.remove(idx);
                    self.cursor -= 1;
                }
                SpawnAction::None
            }
            KeyCode::Delete => {
                if self.cursor < self.worker_name.chars().count() {
                    let idx = byte_index(&self.worker_name, self.cursor);
                    self.worker_name.remove(idx);
                }
                SpawnAction::None
            }
            KeyCode::Char(ch) if is_safe_worker_char(ch) => {
                let idx = byte_index(&self.worker_name, self.cursor);
                self.worker_name.insert(idx, ch);
                self.cursor += 1;
                SpawnAction::None
            }
            _ => SpawnAction::None,
        }
    }
}

pub(crate) fn select(
    workspace_root: &Path,
    worker_name: Option<String>,
    profile: Option<String>,
) -> Result<Option<StandaloneSpawnSelection>, StandaloneSpawnError> {
    let default_worker_name = default_worker_name(workspace_root);
    if let Some(profile) = profile {
        return Ok(Some(StandaloneSpawnSelection {
            worker_name: worker_name.unwrap_or(default_worker_name),
            profile,
        }));
    }

    let registry = ProfileDiscovery::user_settings().discover()?;
    let choices = profile_choices(&registry);
    if choices.is_empty() {
        return Err(StandaloneSpawnError::NoProfiles);
    }

    with_inline_terminal(VIEWPORT_HEIGHT, |terminal| {
        run_picker(
            terminal,
            SpawnForm::new(worker_name, default_worker_name, choices),
        )
    })
}

fn run_picker(
    terminal: &mut InlineTerminal,
    mut form: SpawnForm,
) -> Result<Option<StandaloneSpawnSelection>, StandaloneSpawnError> {
    loop {
        terminal.draw(|frame| draw_form(frame, &form))?;
        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };

        match form.apply_key(key) {
            SpawnAction::None => {}
            SpawnAction::Cancel => {
                form.status = Some(("cancelled".to_owned(), StatusKind::Info));
                terminal.draw(|frame| draw_form(frame, &form))?;
                return Ok(None);
            }
            SpawnAction::Submit => {
                let selection = StandaloneSpawnSelection {
                    worker_name: form.worker_name.trim().to_owned(),
                    profile: form.selected_profile().selector.clone(),
                };
                form.status = Some(("starting worker...".to_owned(), StatusKind::Progress));
                terminal.draw(|frame| draw_form(frame, &form))?;
                return Ok(Some(selection));
            }
        }
    }
}

fn profile_choices(registry: &manifest::ProfileRegistry) -> Vec<ProfileChoice> {
    registry
        .entries()
        .iter()
        .map(|entry| {
            let selector = entry.qualified_name();
            let default_marker = if entry.is_default { " (default)" } else { "" };
            let mut label = format!("{selector}{default_marker}");
            if let Some(description) = &entry.description {
                label.push_str(" — ");
                label.push_str(description);
            }
            ProfileChoice {
                selector,
                label,
                is_default: entry.is_default,
            }
        })
        .collect()
}

fn draw_form(frame: &mut ratatui::Frame<'_>, form: &SpawnForm) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(frame.area());

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                "spawn worker",
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ])),
        chunks[0],
    );

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw("  "),
            Span::styled("name: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                &form.worker_name,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
        ])),
        chunks[1],
    );

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw("  "),
            Span::styled("profile: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                &form.selected_profile().label,
                Style::default().fg(Color::Green),
            ),
            Span::styled(
                "  (tab/down to change)",
                Style::default().fg(Color::DarkGray),
            ),
        ])),
        chunks[2],
    );

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "  enter spawn · left/right edit · esc cancel",
            Style::default().fg(Color::DarkGray),
        ))),
        chunks[3],
    );

    let (message, color) = form
        .status
        .as_ref()
        .map(|(message, kind)| {
            let color = match kind {
                StatusKind::Info => Color::DarkGray,
                StatusKind::Progress => Color::Yellow,
                StatusKind::Error => Color::Red,
            };
            (message.as_str(), color)
        })
        .unwrap_or(("", Color::Reset));
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw("  "),
            Span::styled(message, Style::default().fg(color)),
        ])),
        chunks[4],
    );

    let prefix_width = "  name: ".chars().count() as u16;
    let x = chunks[1]
        .x
        .saturating_add(prefix_width)
        .saturating_add(form.cursor as u16)
        .min(chunks[1].right().saturating_sub(1));
    frame.set_cursor_position((x, chunks[1].y));
}

fn default_worker_name(workspace_root: &Path) -> String {
    workspace_root
        .file_name()
        .and_then(|name| name.to_str())
        .map(sanitise_default_name)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| FALLBACK_WORKER_NAME.to_owned())
}

fn sanitise_default_name(name: &str) -> String {
    name.chars()
        .map(|ch| if is_safe_worker_char(ch) { ch } else { '-' })
        .collect()
}

fn is_safe_worker_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.')
}

fn byte_index(input: &str, char_index: usize) -> usize {
    input
        .char_indices()
        .nth(char_index)
        .map_or(input.len(), |(idx, _)| idx)
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyEvent, KeyModifiers};

    use super::*;

    fn choices() -> Vec<ProfileChoice> {
        vec![
            ProfileChoice {
                selector: "builtin:default".to_owned(),
                label: "builtin:default (default) — Default".to_owned(),
                is_default: true,
            },
            ProfileChoice {
                selector: "builtin:coder".to_owned(),
                label: "builtin:coder — Coder".to_owned(),
                is_default: false,
            },
        ]
    }

    #[test]
    fn default_form_preserves_old_spawn_layout_defaults() {
        let form = SpawnForm::new(None, "yoi".to_owned(), choices());
        assert_eq!(form.worker_name, "yoi");
        assert_eq!(form.selected_profile().selector, "builtin:default");
    }

    #[test]
    fn tab_and_arrows_cycle_profiles() {
        let mut form = SpawnForm::new(None, "yoi".to_owned(), choices());
        assert_eq!(
            form.apply_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            SpawnAction::None
        );
        assert_eq!(form.selected_profile().selector, "builtin:coder");
        form.apply_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(form.selected_profile().selector, "builtin:default");
        form.apply_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(form.selected_profile().selector, "builtin:coder");
    }

    #[test]
    fn name_input_uses_old_safe_character_policy() {
        let mut form = SpawnForm::new(Some("worker".to_owned()), "yoi".to_owned(), choices());
        form.apply_key(KeyEvent::new(KeyCode::Char('-'), KeyModifiers::NONE));
        form.apply_key(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE));
        form.apply_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        assert_eq!(form.worker_name, "worker-1");
    }

    #[test]
    fn enter_rejects_empty_name_and_escape_cancels() {
        let mut form = SpawnForm::new(Some(String::new()), "yoi".to_owned(), choices());
        assert_eq!(
            form.apply_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            SpawnAction::None
        );
        assert_eq!(
            form.status.as_ref().map(|(message, _)| message.as_str()),
            Some("worker name cannot be empty")
        );
        assert_eq!(
            form.apply_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            SpawnAction::Cancel
        );
    }

    #[test]
    fn renderer_preserves_legacy_inline_spawn_form() {
        let backend = ratatui::backend::TestBackend::new(100, VIEWPORT_HEIGHT);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let form = SpawnForm::new(None, "yoi".to_owned(), choices());
        terminal.draw(|frame| draw_form(frame, &form)).unwrap();
        let buffer = terminal.backend().buffer();
        let rendered = buffer
            .content
            .chunks(buffer.area.width as usize)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("spawn worker"));
        assert!(rendered.contains("name: yoi"));
        assert!(rendered.contains("profile: builtin:default (default) — Default"));
        assert!(rendered.contains("enter spawn · left/right edit · esc cancel"));
    }

    #[test]
    fn builtin_discovery_produces_a_default_profile_choice() {
        let registry = ProfileDiscovery::with_sources(None, None)
            .discover()
            .unwrap();
        let choices = profile_choices(&registry);
        let default = choices.iter().find(|choice| choice.is_default).unwrap();
        assert_eq!(default.selector, "builtin:default");
        assert!(default.label.contains("(default)"));
    }

    #[test]
    fn default_worker_name_comes_from_sanitised_directory_basename() {
        assert_eq!(
            default_worker_name(Path::new("/home/hare/Project/yoi")),
            "yoi"
        );
        assert_eq!(
            default_worker_name(Path::new("/home/hare/Project/my project")),
            "my-project"
        );
        assert_eq!(default_worker_name(Path::new("/")), "worker");
    }

    #[test]
    fn explicit_profile_bypasses_discovery_and_uses_directory_name() {
        let selection = select(
            Path::new("/home/hare/Project/yoi"),
            None,
            Some("builtin:coder".to_owned()),
        )
        .unwrap()
        .unwrap();
        assert_eq!(selection.worker_name, "yoi");
        assert_eq!(selection.profile, "builtin:coder");
    }
}
