//! Inline-viewport "spawn Pod and attach" UX.
//!
//! Rendered at the user's current cursor position when `tui` is invoked
//! with no positional argument. Walks the cwd for a `.insomnia/manifest.toml`
//! to seed defaults, prompts for the Pod's name, and on confirmation
//! launches the `pod` binary as an independent process with a freshly built
//! overlay (name + cwd scope when no project manifest exists). Once
//! the process reports its socket via the `INSOMNIA-READY` stderr line,
//! the dialog hands control back so main can switch the terminal to
//! alternate-screen mode.
//!
//! The viewport's last frame stays in the terminal's scrollback so the
//! user has a record of what was spawned (or why a spawn failed).

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use client::{SpawnConfig, spawn_pod};
use crossterm::event::{self, Event as TermEvent, KeyCode, KeyEventKind, KeyModifiers};
use manifest::{
    PodManifestConfig, ScopeConfig, find_project_manifest_from, load_layer, user_manifest_path,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::{Frame, TerminalOptions, Viewport};
use session_store::SegmentId;

const VIEWPORT_LINES: u16 = 6;

pub struct SpawnReady {
    pub pod_name: String,
    pub socket_path: PathBuf,
}

pub enum SpawnOutcome {
    Ready(SpawnReady),
    Cancelled,
}

#[derive(Debug)]
pub enum SpawnError {
    Io(io::Error),
    Store(session_store::StoreError),
    MissingResumeScope { segment_id: SegmentId },
    Spawn(client::SpawnError),
}

impl std::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::Store(e) => write!(f, "failed to read session log: {e}"),
            Self::MissingResumeScope { segment_id } => write!(
                f,
                "session {segment_id} has no persisted scope snapshot; refusing resume without explicit scope"
            ),
            Self::Spawn(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SpawnError {}

impl From<io::Error> for SpawnError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<session_store::StoreError> for SpawnError {
    fn from(e: session_store::StoreError) -> Self {
        Self::Store(e)
    }
}

impl From<client::SpawnError> for SpawnError {
    fn from(e: client::SpawnError) -> Self {
        Self::Spawn(e)
    }
}

type InlineTerminal = Terminal<CrosstermBackend<io::Stdout>>;

/// Source session for a resume run. `None` = fresh spawn (current
/// behaviour); `Some(id)` swaps the dialog into "Resume Pod" mode and
/// passes `--session <id>` to the spawned `pod` child.
pub async fn run(resume_from: Option<SegmentId>) -> Result<SpawnOutcome, SpawnError> {
    let cwd = std::env::current_dir().map_err(SpawnError::Io)?;

    // Run the same merge pod itself uses, then read what's missing
    // off the result. We only look at `scope.allow` here — `pod.name`
    // is intentionally an instance-level identifier and is always
    // taken from the dialog regardless of what (if anything) a layer
    // declared.
    let user_layer = user_manifest_path()
        .filter(|p| p.is_file())
        .and_then(|p| load_layer(&p).ok());
    let project_layer = find_project_manifest_from(&cwd).and_then(|p| load_layer(&p).ok());

    let mut cascade = PodManifestConfig::builtin_defaults();
    for layer in [user_layer.as_ref(), project_layer.as_ref()]
        .into_iter()
        .flatten()
    {
        cascade = cascade.merge(layer.clone());
    }
    let cascade_has_scope = !cascade.scope.allow.is_empty();

    let scope_origin = match (
        project_layer
            .as_ref()
            .is_some_and(|l| !l.scope.allow.is_empty()),
        user_layer
            .as_ref()
            .is_some_and(|l| !l.scope.allow.is_empty()),
    ) {
        (true, _) => ScopeOrigin::FromProject,
        (false, true) => ScopeOrigin::FromUser,
        (false, false) => ScopeOrigin::CwdDefault,
    };

    let default_name = cwd
        .file_name()
        .and_then(|s| s.to_str())
        .map(sanitise_default_name)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "pod".to_string());

    let mut form = Form {
        cwd: cwd.clone(),
        cascade_has_scope,
        scope_origin,
        name_cursor: default_name.chars().count(),
        name: default_name,
        message: None,
        editing: true,
        resume_from,
        resume_scope: None,
    };

    let mut terminal = make_inline_terminal()?;

    // Phase 1: confirm / cancel.
    loop {
        terminal.draw(|f| draw_form(f, &form))?;
        match poll_event()? {
            None => continue,
            Some(Action::Submit) => {
                if form.name.trim().is_empty() {
                    form.message = Some(("name is required".to_string(), MessageKind::Error));
                    continue;
                }
                break;
            }
            Some(Action::Cancel) => {
                form.editing = false;
                form.message = Some(("cancelled".to_string(), MessageKind::Info));
                terminal.draw(|f| draw_form(f, &form))?;
                drop(terminal);
                return Ok(SpawnOutcome::Cancelled);
            }
            Some(Action::Char(c)) => form.insert_char(c),
            Some(Action::Backspace) => form.backspace(),
            Some(Action::Delete) => form.delete_forward(),
            Some(Action::Left) => form.move_left(),
            Some(Action::Right) => form.move_right(),
            Some(Action::Home) => form.name_cursor = 0,
            Some(Action::End) => form.name_cursor = form.name.chars().count(),
        }
    }

    if let Some(id) = form.resume_from {
        form.resume_scope = Some(load_resume_scope(id).await?);
    }
    let overlay_toml = build_overlay_toml(&form);

    // Phase 2: launch pod and wait for ready line. Drop the cursor
    // out of the name field — subsequent frames are passive status
    // updates, not input — so the cursor doesn't end up parked there
    // when the inline terminal is finally dropped.
    form.editing = false;
    form.message = Some(("starting pod...".to_string(), MessageKind::Progress));
    terminal.draw(|f| draw_form(f, &form))?;

    match wait_for_ready(&mut terminal, &mut form, &overlay_toml).await {
        Ok(ready) => {
            form.message = Some((
                format!("ready: {}  attaching...", ready.pod_name),
                MessageKind::Ok,
            ));
            terminal.draw(|f| draw_form(f, &form))?;
            drop(terminal);
            Ok(SpawnOutcome::Ready(ready))
        }
        Err(e) => {
            form.message = Some((e.to_string(), MessageKind::Error));
            let _ = terminal.draw(|f| draw_form(f, &form));
            drop(terminal);
            Err(e)
        }
    }
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

enum Action {
    Submit,
    Cancel,
    Char(char),
    Backspace,
    Delete,
    Left,
    Right,
    Home,
    End,
}

fn poll_event() -> io::Result<Option<Action>> {
    if !event::poll(Duration::from_millis(100))? {
        return Ok(None);
    }
    match event::read()? {
        TermEvent::Key(k) if k.kind != KeyEventKind::Release => {
            let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
            Ok(match k.code {
                KeyCode::Enter => Some(Action::Submit),
                KeyCode::Esc => Some(Action::Cancel),
                KeyCode::Char('c') if ctrl => Some(Action::Cancel),
                KeyCode::Char('a') if ctrl => Some(Action::Home),
                KeyCode::Char('e') if ctrl => Some(Action::End),
                KeyCode::Char('u') if ctrl => Some(Action::Cancel),
                KeyCode::Backspace => Some(Action::Backspace),
                KeyCode::Delete => Some(Action::Delete),
                KeyCode::Left => Some(Action::Left),
                KeyCode::Right => Some(Action::Right),
                KeyCode::Home => Some(Action::Home),
                KeyCode::End => Some(Action::End),
                KeyCode::Char(c) if !ctrl && is_safe_name_char(c) => Some(Action::Char(c)),
                _ => None,
            })
        }
        _ => Ok(None),
    }
}

fn is_safe_name_char(c: char) -> bool {
    // Filesystem-safe; pod.name becomes a runtime-dir name.
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')
}

fn sanitise_default_name(s: &str) -> String {
    s.chars()
        .map(|c| if is_safe_name_char(c) { c } else { '-' })
        .collect()
}

async fn wait_for_ready(
    terminal: &mut InlineTerminal,
    form: &mut Form,
    overlay_toml: &str,
) -> Result<SpawnReady, SpawnError> {
    let cwd = std::env::current_dir().map_err(SpawnError::Io)?;

    let config = SpawnConfig {
        pod_name: form.name.clone(),
        overlay_toml: overlay_toml.to_string(),
        cwd,
        resume_from: form.resume_from,
    };
    let ready = spawn_pod(config, |line| {
        form.message = Some((line.to_string(), MessageKind::Progress));
        let _ = terminal.draw(|f| draw_form(f, form));
    })
    .await?;
    Ok(SpawnReady {
        pod_name: ready.pod_name,
        socket_path: ready.socket_path,
    })
}

fn build_overlay_toml(form: &Form) -> String {
    let mut root = toml::value::Table::new();

    let mut pod = toml::value::Table::new();
    pod.insert("name".into(), toml::Value::String(form.name.clone()));
    root.insert("pod".into(), toml::Value::Table(pod));

    if let Some(scope_config) = form.resume_scope.as_ref() {
        root.insert(
            "scope".into(),
            toml::Value::try_from(scope_config).expect("scope serialisation cannot fail"),
        );
    } else if !form.cascade_has_scope {
        let mut rule = toml::value::Table::new();
        rule.insert(
            "target".into(),
            toml::Value::String(form.cwd.display().to_string()),
        );
        rule.insert("permission".into(), toml::Value::String("write".into()));
        let mut scope = toml::value::Table::new();
        scope.insert(
            "allow".into(),
            toml::Value::Array(vec![toml::Value::Table(rule)]),
        );
        root.insert("scope".into(), toml::Value::Table(scope));
    }

    toml::to_string(&toml::Value::Table(root)).expect("overlay serialisation cannot fail")
}

async fn load_resume_scope(segment_id: SegmentId) -> Result<ScopeConfig, SpawnError> {
    let store_dir = manifest::paths::sessions_dir().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "could not resolve sessions directory (set INSOMNIA_HOME, INSOMNIA_DATA_DIR, or HOME)",
        )
    })?;
    let store = session_store::FsStore::new(&store_dir)?;
    let state = session_store::restore_by_segment(&store, segment_id)?;
    let snapshot = state
        .pod_scope
        .ok_or(SpawnError::MissingResumeScope { segment_id })?;
    Ok(ScopeConfig {
        allow: snapshot.allow,
        deny: snapshot.deny,
    })
}

enum MessageKind {
    Info,
    Ok,
    Error,
    Progress,
}

enum ScopeOrigin {
    FromUser,
    FromProject,
    CwdDefault,
}

struct Form {
    cwd: PathBuf,
    /// True when at least one cascade layer (user or project manifest)
    /// already declares `scope.allow`. Drives whether the overlay
    /// should add a cwd-write rule.
    cascade_has_scope: bool,
    /// Display label for the scope row in the dialog.
    scope_origin: ScopeOrigin,
    name: String,
    /// Cursor position counted in **chars**, not bytes — `name`
    /// currently only accepts ASCII so the two coincide, but we keep
    /// char-based bookkeeping in case we relax `is_safe_name_char`.
    name_cursor: usize,
    message: Option<(String, MessageKind)>,
    /// True while the dialog is accepting name input. Drives whether
    /// the rendered frame parks the terminal cursor inside the name
    /// field — when false (post-confirm / cancel / failure frames) the
    /// cursor stays out so it does not collide with the shell prompt
    /// after the inline terminal is dropped.
    editing: bool,
    /// `Some(id)` flips the dialog into "Resume Pod" mode: the title
    /// switches, the source session is shown to the user, and the
    /// child pod is launched with `--session <id>` so it restores
    /// from `id` and appends to the same session log.
    resume_from: Option<SegmentId>,
    /// Scope snapshot recovered from the source session log. Set only for
    /// resume runs, and serialized into the overlay instead of cwd-default
    /// scope so resume does not silently broaden access.
    resume_scope: Option<ScopeConfig>,
}

impl Form {
    fn insert_char(&mut self, c: char) {
        let byte = self.char_offset_to_byte(self.name_cursor);
        self.name.insert(byte, c);
        self.name_cursor += 1;
    }

    fn backspace(&mut self) {
        if self.name_cursor == 0 {
            return;
        }
        let end = self.char_offset_to_byte(self.name_cursor);
        let start = self.char_offset_to_byte(self.name_cursor - 1);
        self.name.replace_range(start..end, "");
        self.name_cursor -= 1;
    }

    fn delete_forward(&mut self) {
        let total = self.name.chars().count();
        if self.name_cursor >= total {
            return;
        }
        let start = self.char_offset_to_byte(self.name_cursor);
        let end = self.char_offset_to_byte(self.name_cursor + 1);
        self.name.replace_range(start..end, "");
    }

    fn move_left(&mut self) {
        if self.name_cursor > 0 {
            self.name_cursor -= 1;
        }
    }

    fn move_right(&mut self) {
        let total = self.name.chars().count();
        if self.name_cursor < total {
            self.name_cursor += 1;
        }
    }

    fn char_offset_to_byte(&self, char_off: usize) -> usize {
        self.name
            .char_indices()
            .nth(char_off)
            .map(|(b, _)| b)
            .unwrap_or(self.name.len())
    }
}

fn draw_form(f: &mut Frame<'_>, form: &Form) {
    let area = f.area();
    let layout = Layout::vertical([
        Constraint::Length(1), // title
        Constraint::Length(1), // name field
        Constraint::Length(1), // context (manifest or scope default)
        Constraint::Length(1), // hint
        Constraint::Length(1), // message
        Constraint::Length(1), // spacer
    ])
    .split(area);

    let title_text = match form.resume_from {
        Some(id) => format!("resume pod   session: {}", short_segment(id)),
        None => "spawn pod".to_string(),
    };
    let title = Paragraph::new(Line::from(vec![Span::styled(
        title_text,
        Style::default().add_modifier(Modifier::BOLD),
    )]));
    f.render_widget(title, layout[0]);

    f.render_widget(Paragraph::new(name_line(form)), layout[1]);
    f.render_widget(Paragraph::new(context_line(form)), layout[2]);
    f.render_widget(Paragraph::new(hint_line()), layout[3]);
    f.render_widget(Paragraph::new(message_line(form)), layout[4]);

    if form.editing {
        // Place the cursor inside the name field while the user is
        // editing. Skipped on post-confirm frames so the inline
        // viewport's drop leaves the cursor at the bottom of the
        // rendered area rather than parked on the name line, which
        // would let the shell prompt (or any later eprintln) clobber
        // the rendered name field after exit.
        let cursor_col = 2 + "name: ".len() + form.name_cursor;
        f.set_cursor_position((layout[1].x + cursor_col as u16, layout[1].y));
    }
}

/// First 8 hex digits of a UUID — short enough to skim, long enough
/// to disambiguate inside a 10-row picker.
pub(crate) fn short_segment(id: SegmentId) -> String {
    let s = id.to_string();
    s.chars().take(8).collect()
}

fn name_line(form: &Form) -> Line<'_> {
    Line::from(vec![
        Span::raw("  "),
        Span::styled("name: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            form.name.as_str(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

fn context_line(form: &Form) -> Line<'_> {
    match form.scope_origin {
        ScopeOrigin::FromProject => Line::from(vec![
            Span::raw("  "),
            Span::styled("scope: ", Style::default().fg(Color::DarkGray)),
            Span::styled("from project manifest", Style::default().fg(Color::Green)),
        ]),
        ScopeOrigin::FromUser => Line::from(vec![
            Span::raw("  "),
            Span::styled("scope: ", Style::default().fg(Color::DarkGray)),
            Span::styled("from user manifest", Style::default().fg(Color::Green)),
        ]),
        ScopeOrigin::CwdDefault => Line::from(vec![
            Span::raw("  "),
            Span::styled("scope: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                form.cwd.display().to_string(),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled(" (write, default)", Style::default().fg(Color::DarkGray)),
        ]),
    }
}

fn hint_line() -> Line<'static> {
    Line::from(vec![
        Span::raw("  "),
        Span::styled("[enter]", Style::default().fg(Color::Green)),
        Span::raw(" spawn   "),
        Span::styled("[esc]", Style::default().fg(Color::Yellow)),
        Span::raw(" cancel"),
    ])
}

fn message_line(form: &Form) -> Line<'_> {
    let Some((text, kind)) = form.message.as_ref() else {
        return Line::from("");
    };
    let style = match kind {
        MessageKind::Info => Style::default().fg(Color::DarkGray),
        MessageKind::Ok => Style::default().fg(Color::Green),
        MessageKind::Error => Style::default().fg(Color::Red),
        MessageKind::Progress => Style::default().fg(Color::Yellow),
    };
    Line::from(vec![Span::raw("  "), Span::styled(text.as_str(), style)])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form(name: &str, cascade_has_scope: bool) -> Form {
        Form {
            cwd: PathBuf::from("/work/example"),
            cascade_has_scope,
            scope_origin: if cascade_has_scope {
                ScopeOrigin::FromProject
            } else {
                ScopeOrigin::CwdDefault
            },
            name: name.to_string(),
            name_cursor: name.chars().count(),
            message: None,
            editing: true,
            resume_from: None,
            resume_scope: None,
        }
    }

    #[test]
    fn overlay_adds_scope_default_when_cascade_lacks_scope() {
        let f = form("agent-1", false);
        let toml_str = build_overlay_toml(&f);
        let parsed: toml::Value = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed["pod"]["name"].as_str(), Some("agent-1"));
        let allow = parsed["scope"]["allow"].as_array().unwrap();
        assert_eq!(allow.len(), 1);
        assert_eq!(allow[0]["target"].as_str(), Some("/work/example"));
        assert_eq!(allow[0]["permission"].as_str(), Some("write"));
    }

    #[test]
    fn overlay_omits_scope_when_cascade_already_has_one() {
        let f = form("agent-2", true);
        let toml_str = build_overlay_toml(&f);
        let parsed: toml::Value = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed["pod"]["name"].as_str(), Some("agent-2"));
        assert!(parsed.get("scope").is_none());
    }

    #[test]
    fn overlay_uses_resume_scope_snapshot() {
        let mut f = form("agent-r", false);
        f.resume_from = Some(session_store::new_segment_id());
        f.resume_scope = Some(ScopeConfig {
            allow: vec![manifest::ScopeRule {
                target: PathBuf::from("/work/example"),
                permission: manifest::Permission::Write,
                recursive: true,
            }],
            deny: vec![manifest::ScopeRule {
                target: PathBuf::from("/work/example/child"),
                permission: manifest::Permission::Write,
                recursive: true,
            }],
        });
        let toml_str = build_overlay_toml(&f);
        let parsed: toml::Value = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed["pod"]["name"].as_str(), Some("agent-r"));
        assert_eq!(parsed["scope"]["allow"].as_array().unwrap().len(), 1);
        let deny = parsed["scope"]["deny"].as_array().unwrap();
        assert_eq!(deny[0]["target"].as_str(), Some("/work/example/child"));
    }

    #[test]
    fn cascade_merge_detects_scope_from_any_layer() {
        let user = PodManifestConfig::from_toml(
            r#"
[[scope.allow]]
target = "/from-user"
permission = "write"
"#,
        )
        .unwrap();
        let mut cascade = PodManifestConfig::builtin_defaults();
        cascade = cascade.merge(user);
        assert!(!cascade.scope.allow.is_empty());

        let empty_cascade = PodManifestConfig::builtin_defaults();
        assert!(empty_cascade.scope.allow.is_empty());
    }

    #[test]
    fn name_input_handles_insert_backspace_and_cursor() {
        let mut f = form("", false);
        for c in "abc".chars() {
            f.insert_char(c);
        }
        assert_eq!(f.name, "abc");
        assert_eq!(f.name_cursor, 3);

        f.move_left();
        f.move_left();
        f.insert_char('X');
        assert_eq!(f.name, "aXbc");

        f.backspace();
        assert_eq!(f.name, "abc");
        assert_eq!(f.name_cursor, 1);

        f.delete_forward();
        assert_eq!(f.name, "ac");
    }

    #[test]
    fn sanitise_default_name_replaces_unsafe_chars() {
        assert_eq!(sanitise_default_name("my project!"), "my-project-");
        assert_eq!(sanitise_default_name("ok-name_2.0"), "ok-name_2.0");
    }
}
