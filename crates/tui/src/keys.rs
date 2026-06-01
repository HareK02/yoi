use std::io::{self, Stdout, Write};
use std::process::ExitCode;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::{Frame, Terminal, TerminalOptions, Viewport};
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
            eprintln!("yoi keys: could not determine yoi data directory");
            return ExitCode::FAILURE;
        }
    };
    match run(SecretStore::new(data_dir)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("yoi keys: {err}");
            ExitCode::FAILURE
        }
    }
}

type UiResult<T> = Result<T, Box<dyn std::error::Error>>;
type InlineTerminal = Terminal<CrosstermBackend<Stdout>>;

const MAX_ROWS: usize = 10;
const VIEWPORT_LINES: u16 = MAX_ROWS as u16 + 5;

struct RawModeGuard {
    active: bool,
}

impl RawModeGuard {
    fn new() -> Self {
        Self { active: true }
    }

    fn restore(mut self) {
        self.cleanup();
        self.active = false;
    }

    fn cleanup(&mut self) {
        let _ = disable_raw_mode();
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if self.active {
            self.cleanup();
        }
    }
}

fn run(store: SecretStore) -> UiResult<()> {
    enable_raw_mode()?;
    let guard = RawModeGuard::new();
    let mut terminal = make_inline_terminal()?;
    let result = run_loop(&mut terminal, store);
    let close_result = close_viewport(&mut terminal);
    drop(terminal);
    guard.restore();
    result?;
    close_result?;
    Ok(())
}

fn make_inline_terminal() -> io::Result<InlineTerminal> {
    let backend = CrosstermBackend::new(io::stdout());
    Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Inline(VIEWPORT_LINES),
        },
    )
}

/// Park the cursor at the very bottom of the inline viewport and emit one
/// newline before dropping the terminal. This matches the resume picker and
/// keeps the shell prompt (or a later inline viewport) from drawing over rows.
fn close_viewport(terminal: &mut InlineTerminal) -> io::Result<()> {
    let area = terminal.get_frame().area();
    let last_row = area.bottom().saturating_sub(1);
    terminal.set_cursor_position((0, last_row))?;
    let mut out = io::stdout();
    out.write_all(b"\r\n")?;
    out.flush()?;
    Ok(())
}

fn run_loop(terminal: &mut InlineTerminal, store: SecretStore) -> UiResult<()> {
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

fn draw(frame: &mut Frame<'_>, app: &KeysApp) {
    let area = frame.area();
    let mut constraints = Vec::with_capacity(MAX_ROWS + 5);
    constraints.push(Constraint::Length(1)); // title
    for _ in 0..MAX_ROWS {
        constraints.push(Constraint::Length(1)); // key rows
    }
    constraints.push(Constraint::Length(1)); // mode / actions
    constraints.push(Constraint::Length(1)); // notice
    constraints.push(Constraint::Length(1)); // protection note
    constraints.push(Constraint::Length(1)); // spacer
    let layout = Layout::vertical(constraints).split(area);

    frame.render_widget(Paragraph::new(title_line(app)), layout[0]);

    let (start, end) = visible_key_window(app.ids.len(), app.selected);
    for row in 0..MAX_ROWS {
        let line = if app.ids.is_empty() && row == 0 {
            empty_keys_line()
        } else if let Some(id) = app.ids.get(start + row).filter(|_| start + row < end) {
            key_row_line(id, start + row == app.selected)
        } else {
            Line::from("")
        };
        frame.render_widget(Paragraph::new(line), layout[row + 1]);
    }

    let mode_row = MAX_ROWS + 1;
    frame.render_widget(Paragraph::new(mode_line(app)), layout[mode_row]);
    frame.render_widget(Paragraph::new(notice_line(app)), layout[MAX_ROWS + 2]);
    frame.render_widget(Paragraph::new(protection_line()), layout[MAX_ROWS + 3]);

    if let Some(cursor_col) = mode_cursor_col(app) {
        let col = cursor_col.min(area.width.saturating_sub(1) as usize);
        frame.set_cursor_position((layout[mode_row].x + col as u16, layout[mode_row].y));
    }
}

fn title_line(app: &KeysApp) -> Line<'_> {
    Line::from(vec![
        Span::styled(
            "yoi keys   local secrets",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw("   "),
        Span::styled(
            key_count_label(app.ids.len()),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw("   "),
        Span::styled("values hidden", Style::default().fg(Color::DarkGray)),
    ])
}

fn key_count_label(count: usize) -> String {
    match count {
        0 => "0 ids".to_string(),
        1 => "1 id".to_string(),
        n => format!("{n} ids"),
    }
}

fn visible_key_window(total: usize, selected: usize) -> (usize, usize) {
    if total <= MAX_ROWS {
        return (0, total);
    }
    let last_start = total - MAX_ROWS;
    let start = selected.saturating_sub(MAX_ROWS / 2).min(last_start);
    (start, start + MAX_ROWS)
}

fn empty_keys_line() -> Line<'static> {
    Line::from(vec![
        Span::raw("  "),
        Span::styled("No keys stored", Style::default().fg(Color::DarkGray)),
    ])
}

fn key_row_line(id: &str, selected: bool) -> Line<'_> {
    let marker = if selected { "▶ " } else { "  " };
    let id_style = if selected {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Cyan)
    };
    Line::from(vec![
        Span::raw(marker),
        Span::styled(id, id_style),
        Span::raw("  "),
        Span::styled("[secret id]", Style::default().fg(Color::DarkGray)),
    ])
}

fn mode_line(app: &KeysApp) -> Line<'_> {
    match &app.mode {
        Mode::Normal => Line::from(vec![
            Span::raw("  "),
            Span::styled("[a]", Style::default().fg(Color::Green)),
            Span::raw(" add/set   "),
            Span::styled("[d]", Style::default().fg(Color::Yellow)),
            Span::raw(" delete   "),
            Span::styled("[↑/↓]", Style::default().fg(Color::DarkGray)),
            Span::raw(" select   "),
            Span::styled("[q]", Style::default().fg(Color::Yellow)),
            Span::raw(" quit"),
        ]),
        Mode::EditingId => Line::from(vec![
            Span::raw("  "),
            Span::styled("secret id: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                app.input.as_str(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Mode::EditingValue { id } => Line::from(vec![
            Span::raw("  "),
            Span::styled("value for `", Style::default().fg(Color::DarkGray)),
            Span::styled(id.as_str(), Style::default().fg(Color::Cyan)),
            Span::styled("`: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "•".repeat(app.input.chars().count()),
                Style::default().fg(Color::Green),
            ),
        ]),
        Mode::ConfirmDelete { id } => Line::from(vec![
            Span::raw("  "),
            Span::styled("delete `", Style::default().fg(Color::DarkGray)),
            Span::styled(id.as_str(), Style::default().fg(Color::Cyan)),
            Span::styled("`? ", Style::default().fg(Color::DarkGray)),
            Span::styled("[y]", Style::default().fg(Color::Red)),
            Span::raw(" yes   "),
            Span::styled("[n/esc]", Style::default().fg(Color::Yellow)),
            Span::raw(" cancel"),
        ]),
    }
}

fn notice_line(app: &KeysApp) -> Line<'_> {
    if app.notice.is_empty() {
        return Line::from("");
    }
    Line::from(vec![
        Span::raw("  "),
        Span::styled(app.notice.as_str(), notice_style(app.notice.as_str())),
    ])
}

fn notice_style(notice: &str) -> Style {
    if notice.contains("failed") || notice.contains("must not") {
        Style::default().fg(Color::Red)
    } else if notice.starts_with("Saved") || notice.starts_with("Deleted") {
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

fn protection_line() -> Line<'static> {
    Line::from(vec![
        Span::raw("  "),
        Span::styled(
            "local obfuscation at rest; not a system keychain",
            Style::default().fg(Color::DarkGray),
        ),
    ])
}

fn mode_cursor_col(app: &KeysApp) -> Option<usize> {
    match &app.mode {
        Mode::EditingId => Some("  secret id: ".chars().count() + app.input.chars().count()),
        Mode::EditingValue { id } => Some(
            "  value for `".chars().count()
                + id.chars().count()
                + "`: ".chars().count()
                + app.input.chars().count(),
        ),
        Mode::Normal | Mode::ConfirmDelete { .. } => None,
    }
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

    fn line_text(line: Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn renderer_masks_editing_value() {
        let mut app = KeysApp::new(vec!["web/brave/test".into()]);
        app.mode = Mode::EditingValue {
            id: "web/brave/test".into(),
        };
        app.input = "secret-value".into();

        let text = line_text(mode_line(&app));

        assert!(text.contains("web/brave/test"));
        assert!(text.contains("••••••••••••"));
        assert!(!text.contains("secret-value"));
    }
}
