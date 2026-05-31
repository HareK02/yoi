use std::io::{self, Stdout};
use std::process::ExitCode;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use secrets::{SecretStore, SecretValue};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Mode {
    Normal,
    EditingId,
    EditingValue { id: String },
    ConfirmDelete { id: String },
}

#[derive(Clone)]
struct KeysApp {
    ids: Vec<String>,
    selected: usize,
    mode: Mode,
    input: String,
    notice: String,
    quit: bool,
}

impl std::fmt::Debug for KeysApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let input = match self.mode {
            Mode::EditingValue { .. } => "[redacted]".to_string(),
            _ => self.input.clone(),
        };
        f.debug_struct("KeysApp")
            .field("ids", &self.ids)
            .field("selected", &self.selected)
            .field("mode", &self.mode)
            .field("input", &input)
            .field("notice", &self.notice)
            .field("quit", &self.quit)
            .finish()
    }
}

impl KeysApp {
    fn new(ids: Vec<String>) -> Self {
        let mut app = Self {
            ids,
            selected: 0,
            mode: Mode::Normal,
            input: String::new(),
            notice: String::new(),
            quit: false,
        };
        app.clamp_selection();
        app
    }

    fn refresh(&mut self, ids: Vec<String>) {
        self.ids = ids;
        self.clamp_selection();
    }

    fn selected_id(&self) -> Option<&str> {
        self.ids.get(self.selected).map(String::as_str)
    }

    fn clamp_selection(&mut self) {
        if self.ids.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.ids.len() {
            self.selected = self.ids.len() - 1;
        }
    }

    fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Action {
        if modifiers.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('c') {
            self.quit = true;
            return Action::Quit;
        }
        match self.mode.clone() {
            Mode::Normal => self.handle_normal(code),
            Mode::EditingId => self.handle_editing_id(code),
            Mode::EditingValue { id } => self.handle_editing_value(code, id),
            Mode::ConfirmDelete { id } => self.handle_confirm_delete(code, id),
        }
    }

    fn handle_normal(&mut self, code: KeyCode) -> Action {
        match code {
            KeyCode::Char('q') | KeyCode::Esc => {
                self.quit = true;
                Action::Quit
            }
            KeyCode::Char('a') => {
                self.input.clear();
                self.notice = "Enter secret id, then Enter".into();
                self.mode = Mode::EditingId;
                Action::None
            }
            KeyCode::Char('d') => {
                if let Some(id) = self.selected_id() {
                    self.mode = Mode::ConfirmDelete { id: id.to_string() };
                    self.notice = "Delete selected secret? y/N".into();
                } else {
                    self.notice = "No key selected".into();
                }
                Action::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if !self.ids.is_empty() {
                    self.selected = (self.selected + 1).min(self.ids.len() - 1);
                }
                Action::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
                Action::None
            }
            _ => Action::None,
        }
    }

    fn handle_editing_id(&mut self, code: KeyCode) -> Action {
        match code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.input.clear();
                self.notice = "Add cancelled".into();
                Action::None
            }
            KeyCode::Enter => {
                let id = self.input.trim().to_string();
                if id.is_empty() {
                    self.notice = "Secret id must not be empty".into();
                    return Action::None;
                }
                self.input.clear();
                self.notice = "Enter secret value; input is masked".into();
                self.mode = Mode::EditingValue { id };
                Action::None
            }
            KeyCode::Backspace => {
                self.input.pop();
                Action::None
            }
            KeyCode::Char(c) if !c.is_control() => {
                self.input.push(c);
                Action::None
            }
            _ => Action::None,
        }
    }

    fn handle_editing_value(&mut self, code: KeyCode, id: String) -> Action {
        match code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.input.clear();
                self.notice = "Add cancelled".into();
                Action::None
            }
            KeyCode::Enter => {
                let value = std::mem::take(&mut self.input);
                self.mode = Mode::Normal;
                Action::Set { id, value }
            }
            KeyCode::Backspace => {
                self.input.pop();
                Action::None
            }
            KeyCode::Char(c) if !c.is_control() => {
                self.input.push(c);
                Action::None
            }
            _ => Action::None,
        }
    }

    fn handle_confirm_delete(&mut self, code: KeyCode, id: String) -> Action {
        self.mode = Mode::Normal;
        match code {
            KeyCode::Char('y') | KeyCode::Char('Y') => Action::Delete { id },
            _ => {
                self.notice = "Delete cancelled".into();
                Action::None
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
enum Action {
    None,
    Set { id: String, value: String },
    Delete { id: String },
    Quit,
}

impl std::fmt::Debug for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => f.write_str("None"),
            Self::Set { id, .. } => f
                .debug_struct("Set")
                .field("id", id)
                .field("value", &"[redacted]")
                .finish(),
            Self::Delete { id } => f.debug_struct("Delete").field("id", id).finish(),
            Self::Quit => f.write_str("Quit"),
        }
    }
}

pub async fn launch() -> ExitCode {
    let data_dir = match manifest::paths::data_dir() {
        Some(path) => path,
        None => {
            eprintln!("insomnia keys: could not determine insomnia data directory");
            return ExitCode::FAILURE;
        }
    };
    match run(SecretStore::new(data_dir)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("insomnia keys: {err}");
            ExitCode::FAILURE
        }
    }
}

type UiResult<T> = Result<T, Box<dyn std::error::Error>>;

struct TerminalRestoreGuard {
    active: bool,
}

impl TerminalRestoreGuard {
    fn new() -> Self {
        Self { active: true }
    }

    fn restore(mut self) {
        self.cleanup();
        self.active = false;
    }

    fn cleanup(&mut self) {
        let _ = execute!(io::stdout(), crossterm::cursor::Show, LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

impl Drop for TerminalRestoreGuard {
    fn drop(&mut self) {
        if self.active {
            self.cleanup();
        }
    }
}

fn run(store: SecretStore) -> UiResult<()> {
    enable_raw_mode()?;
    let guard = TerminalRestoreGuard::new();
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, crossterm::cursor::Hide)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let result = run_loop(&mut terminal, store);
    drop(terminal);
    guard.restore();
    result
}

fn run_loop(terminal: &mut Terminal<CrosstermBackend<Stdout>>, store: SecretStore) -> UiResult<()> {
    let mut app = KeysApp::new(load_ids(&store)?);
    loop {
        terminal.draw(|frame| draw(frame, &app))?;
        if app.quit {
            return Ok(());
        }
        if event::poll(Duration::from_millis(200))? {
            let Event::Key(key) = event::read()? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match app.handle_key(key.code, key.modifiers) {
                Action::None => {}
                Action::Quit => return Ok(()),
                Action::Set { id, value } => match store.set(&id, SecretValue::new(value)) {
                    Ok(()) => {
                        app.refresh(load_ids(&store)?);
                        app.notice = format!("Saved `{id}`");
                    }
                    Err(err) => {
                        app.notice = format!("Save failed for `{id}`: {err}");
                    }
                },
                Action::Delete { id } => match store.delete(&id) {
                    Ok(true) => {
                        app.refresh(load_ids(&store)?);
                        app.notice = format!("Deleted `{id}`");
                    }
                    Ok(false) => app.notice = format!("Secret `{id}` was already absent"),
                    Err(err) => app.notice = format!("Delete failed for `{id}`: {err}"),
                },
            }
        }
    }
}

fn load_ids(store: &SecretStore) -> UiResult<Vec<String>> {
    Ok(store
        .list_ids()?
        .into_iter()
        .map(|id| id.as_str().to_string())
        .collect())
}

fn draw(frame: &mut ratatui::Frame<'_>, app: &KeysApp) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(5),
        ])
        .split(area);

    let title = Paragraph::new("Local secret keys (values are never displayed)").block(
        Block::default()
            .borders(Borders::ALL)
            .title("insomnia keys"),
    );
    frame.render_widget(title, chunks[0]);

    let items: Vec<ListItem<'_>> = if app.ids.is_empty() {
        vec![ListItem::new(Line::from(Span::raw("No keys stored")))]
    } else {
        app.ids
            .iter()
            .map(|id| ListItem::new(Line::from(Span::raw(id.clone()))))
            .collect()
    };
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title("Key ids"))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    let mut state = ListState::default();
    if !app.ids.is_empty() {
        state.select(Some(app.selected));
    }
    frame.render_stateful_widget(list, chunks[1], &mut state);

    let input_line = match &app.mode {
        Mode::Normal => "a add/set  d delete  ↑/↓ select  q quit".to_string(),
        Mode::EditingId => format!("Secret id: {}", app.input),
        Mode::EditingValue { id } => format!(
            "Value for `{id}`: {}",
            "•".repeat(app.input.chars().count())
        ),
        Mode::ConfirmDelete { id } => format!("Delete `{id}`? y/N"),
    };
    let help = Paragraph::new(vec![
        Line::from(input_line),
        Line::from(app.notice.clone()),
        Line::from("Protection is local obfuscation at rest, not a system keychain."),
    ])
    .block(Block::default().borders(Borders::ALL).title("Actions"))
    .wrap(Wrap { trim: true });
    frame.render_widget(Clear, chunks[2]);
    frame.render_widget(help, chunks[2]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_masks_value_and_emits_set_action() {
        let mut app = KeysApp::new(vec![]);
        assert_eq!(
            app.handle_key(KeyCode::Char('a'), KeyModifiers::NONE),
            Action::None
        );
        for c in "web/brave/test".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::NONE);
        }
        assert_eq!(
            app.handle_key(KeyCode::Enter, KeyModifiers::NONE),
            Action::None
        );
        assert!(matches!(app.mode, Mode::EditingValue { .. }));
        for c in "secret-value".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::NONE);
        }
        assert_eq!(app.input, "secret-value");
        let action = app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(
            action,
            Action::Set {
                id: "web/brave/test".into(),
                value: "secret-value".into()
            }
        );
        assert!(!format!("{app:?}").contains("secret-value"));
    }

    #[test]
    fn model_confirms_delete() {
        let mut app = KeysApp::new(vec!["providers/anthropic/default".into()]);
        assert_eq!(
            app.handle_key(KeyCode::Char('d'), KeyModifiers::NONE),
            Action::None
        );
        let action = app.handle_key(KeyCode::Char('y'), KeyModifiers::NONE);
        assert_eq!(
            action,
            Action::Delete {
                id: "providers/anthropic/default".into()
            }
        );
    }
}
