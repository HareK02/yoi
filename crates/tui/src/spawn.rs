//! Inline-viewport "spawn Pod and attach" UX.
//!
//! Rendered at the user's current cursor position when `insomnia` is invoked
//! with no positional argument. Discovers `.insomnia/profiles.toml` profile
//! choices plus bundled profiles, defaults to the builtin profile, prompts for
//! the Pod's name, and on confirmation launches the `insomnia-pod` binary as an
//! independent process. Once the process reports its socket via the
//! `INSOMNIA-READY` stderr line, the dialog hands control back so main can
//! switch the terminal to alternate-screen mode.
//!
//! The viewport's last frame stays in the terminal's scrollback so the
//! user has a record of what was spawned (or why a spawn failed).

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use client::{SpawnConfig, spawn_pod};
use crossterm::event::{self, Event as TermEvent, KeyCode, KeyEventKind, KeyModifiers};
use manifest::ProfileDiscovery;
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
    Spawn(client::SpawnError),
}

impl std::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io error: {e}"),
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

impl From<client::SpawnError> for SpawnError {
    fn from(e: client::SpawnError) -> Self {
        Self::Spawn(e)
    }
}

type InlineTerminal = Terminal<CrosstermBackend<io::Stdout>>;

/// Source session for a resume run. `None` = fresh spawn (current
/// behaviour); `Some(id)` swaps the dialog into "Resume Pod" mode and
/// passes `--session <id>` to the spawned `insomnia-pod` child.
pub async fn run(
    resume_from: Option<SegmentId>,
    profile: Option<String>,
) -> Result<SpawnOutcome, SpawnError> {
    let defaults = load_spawn_defaults()?;
    let mut profile_choices = if resume_from.is_some() {
        Vec::new()
    } else {
        defaults.profile_choices
    };
    let profile_index = initial_profile_index(
        &mut profile_choices,
        profile.as_deref(),
        defaults.default_profile_index,
    );

    let mut form = Form {
        cwd: defaults.cwd.clone(),
        scope_origin: defaults.scope_origin,
        name_cursor: defaults.default_name.chars().count(),
        name: defaults.default_name,
        message: None,
        editing: true,
        resume_from,
        resume_by_pod_name: false,
        profile_choices,
        profile_index,
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
            Some(Action::ProfileNext) => form.cycle_profile_next(),
            Some(Action::ProfilePrev) => form.cycle_profile_prev(),
        }
    }

    // Phase 2: launch pod and wait for ready line. Drop the cursor
    // out of the name field — subsequent frames are passive status
    // updates, not input — so the cursor doesn't end up parked there
    // when the inline terminal is finally dropped.
    form.editing = false;
    form.message = Some(("starting pod...".to_string(), MessageKind::Progress));
    terminal.draw(|f| draw_form(f, &form))?;

    match wait_for_ready(&mut terminal, &mut form).await {
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

/// Launch `insomnia-pod --pod <name>` without opening the name dialog. The child Pod
/// resolves persisted Pod metadata if present, or creates a fresh same-name Pod
/// from the default profile.
pub async fn run_pod_name(pod_name: String) -> Result<SpawnOutcome, SpawnError> {
    let defaults = load_spawn_defaults()?;
    let mut form = form_for_pod_name(pod_name, defaults);
    let mut terminal = make_inline_terminal()?;
    terminal.draw(|f| draw_form(f, &form))?;

    match wait_for_ready(&mut terminal, &mut form).await {
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

struct SpawnDefaults {
    cwd: PathBuf,
    scope_origin: ScopeOrigin,
    default_name: String,
    default_profile_index: usize,
    profile_choices: Vec<ProfileChoice>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProfileChoice {
    selector: Option<String>,
    label: String,
    is_default: bool,
}

fn load_spawn_defaults() -> Result<SpawnDefaults, SpawnError> {
    let cwd = std::env::current_dir().map_err(SpawnError::Io)?;

    let default_name = cwd
        .file_name()
        .and_then(|s| s.to_str())
        .map(sanitise_default_name)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "pod".to_string());

    let (profile_choices, default_profile_index) = profile_choices_for_cwd(&cwd);

    Ok(SpawnDefaults {
        cwd,
        scope_origin: ScopeOrigin::FromProfile,
        default_name,
        default_profile_index,
        profile_choices,
    })
}

fn profile_choices_for_cwd(cwd: &Path) -> (Vec<ProfileChoice>, usize) {
    let Ok(registry) = ProfileDiscovery::for_cwd(cwd).discover() else {
        return (Vec::new(), 0);
    };

    let mut choices = Vec::new();
    for entry in registry.entries() {
        let mut label = entry.qualified_name();
        if entry.is_default {
            label.push_str(" (default)");
        }
        if let Some(description) = entry.description.as_deref() {
            label.push_str(" — ");
            label.push_str(description);
        }
        choices.push(ProfileChoice {
            selector: Some(entry.qualified_name()),
            label,
            is_default: entry.is_default,
        });
    }

    let default_index = choices
        .iter()
        .position(|choice| choice.is_default)
        .unwrap_or(0);
    (choices, default_index)
}

fn initial_profile_index(
    choices: &mut Vec<ProfileChoice>,
    explicit_profile: Option<&str>,
    default_index: usize,
) -> usize {
    let Some(selector) = explicit_profile else {
        return default_index.min(choices.len().saturating_sub(1));
    };
    if let Some(index) = choices
        .iter()
        .position(|choice| choice.selector.as_deref() == Some(selector))
    {
        return index;
    }
    choices.push(ProfileChoice {
        selector: Some(selector.to_string()),
        label: selector.to_string(),
        is_default: false,
    });
    choices.len() - 1
}

fn form_for_pod_name(pod_name: String, defaults: SpawnDefaults) -> Form {
    Form {
        cwd: defaults.cwd,
        scope_origin: defaults.scope_origin,
        name_cursor: pod_name.chars().count(),
        name: pod_name,
        message: Some(("resuming pod...".to_string(), MessageKind::Progress)),
        editing: false,
        resume_from: None,
        resume_by_pod_name: true,
        profile_choices: Vec::new(),
        profile_index: 0,
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
    ProfileNext,
    ProfilePrev,
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
                KeyCode::Up | KeyCode::BackTab => Some(Action::ProfilePrev),
                KeyCode::Down | KeyCode::Tab => Some(Action::ProfileNext),
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
) -> Result<SpawnReady, SpawnError> {
    let config = SpawnConfig {
        pod_name: form.name.clone(),
        profile: form.selected_profile_selector(),
        cwd: form.cwd.clone(),
        resume_from: form.resume_from,
        resume_by_pod_name: form.resume_by_pod_name,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MessageKind {
    Info,
    Ok,
    Error,
    Progress,
}

enum ScopeOrigin {
    FromProfile,
}

struct Form {
    cwd: PathBuf,
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
    /// When true, launch the child with `--pod <name>` so the pod process
    /// resolves name-keyed state before falling back to fresh creation.
    resume_by_pod_name: bool,
    /// Optional profile choices passed to `insomnia-pod --profile` for
    /// fresh spawns. This is not used for resume/attach flows because those must
    /// restore Pod state rather than re-evaluate a profile source.
    profile_choices: Vec<ProfileChoice>,
    profile_index: usize,
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

    fn selected_profile(&self) -> Option<&ProfileChoice> {
        self.profile_choices
            .get(self.profile_index)
            .filter(|choice| choice.selector.is_some())
    }

    fn selected_profile_selector(&self) -> Option<String> {
        self.selected_profile()
            .and_then(|choice| choice.selector.clone())
    }

    fn cycle_profile_next(&mut self) {
        if self.profile_choices.is_empty() {
            return;
        }
        self.profile_index = (self.profile_index + 1) % self.profile_choices.len();
        self.message = None;
    }

    fn cycle_profile_prev(&mut self) {
        if self.profile_choices.is_empty() {
            return;
        }
        self.profile_index = if self.profile_index == 0 {
            self.profile_choices.len() - 1
        } else {
            self.profile_index - 1
        };
        self.message = None;
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
        Constraint::Length(1), // context (profile or scope default)
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
    if let Some(profile) = form.profile_choices.get(form.profile_index) {
        return Line::from(vec![
            Span::raw("  "),
            Span::styled("profile: ", Style::default().fg(Color::DarkGray)),
            Span::styled(profile.label.as_str(), Style::default().fg(Color::Green)),
            Span::styled(
                " (tab/down to change)",
                Style::default().fg(Color::DarkGray),
            ),
        ]);
    }

    match form.scope_origin {
        ScopeOrigin::FromProfile => Line::from(vec![
            Span::raw("  "),
            Span::styled("scope: ", Style::default().fg(Color::DarkGray)),
            Span::styled("from selected profile", Style::default().fg(Color::Green)),
        ]),
    }
}

fn hint_line() -> Line<'static> {
    Line::from(vec![Span::styled(
        "  enter spawn · tab/down next profile · shift-tab/up prev · esc cancel",
        Style::default().fg(Color::DarkGray),
    )])
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

    fn form(name: &str) -> Form {
        Form {
            cwd: PathBuf::from("/work/example"),
            scope_origin: ScopeOrigin::FromProfile,
            name: name.to_string(),
            name_cursor: name.chars().count(),
            message: None,
            editing: true,
            resume_from: None,
            resume_by_pod_name: false,
            profile_choices: Vec::new(),
            profile_index: 0,
        }
    }

    #[test]
    fn pod_name_form_restores_or_creates_by_pod_name() {
        let defaults = SpawnDefaults {
            cwd: PathBuf::from("/work/example"),
            scope_origin: ScopeOrigin::FromProfile,
            default_name: "ignored".to_string(),
            default_profile_index: 0,
            profile_choices: Vec::new(),
        };
        let f = form_for_pod_name("agent".to_string(), defaults);

        assert_eq!(f.name, "agent");
        assert_eq!(f.name_cursor, "agent".chars().count());
        assert_eq!(f.resume_from, None);
        assert!(f.resume_by_pod_name);
        assert!(!f.editing);
        assert_eq!(
            f.message,
            Some(("resuming pod...".to_string(), MessageKind::Progress))
        );
    }

    #[test]
    fn profile_choices_use_project_registry_default() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let insomnia = project.join(".insomnia");
        std::fs::create_dir_all(&insomnia).unwrap();
        std::fs::write(
            insomnia.join("profiles.toml"),
            r#"
default = "coder"
[profile]
coder = "profiles/coder.nix"
"#,
        )
        .unwrap();

        let (choices, default_index) = profile_choices_for_cwd(&project);
        assert_eq!(default_index, 1);
        let selected = &choices[default_index];
        assert_eq!(selected.selector.as_deref(), Some("project:coder"));
        assert_eq!(selected.label, "project:coder (default)");
        assert!(selected.is_default);
    }

    #[test]
    fn profile_choices_include_builtin_and_project_default_marker() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let insomnia = project.join(".insomnia");
        std::fs::create_dir_all(&insomnia).unwrap();
        std::fs::write(
            insomnia.join("profiles.toml"),
            r#"
default = "coder"
[profile.coder]
path = "profiles/coder.nix"
description = "Project coder"
"#,
        )
        .unwrap();

        let (choices, default_index) = profile_choices_for_cwd(&project);
        assert_eq!(choices[0].selector.as_deref(), Some("builtin:default"));
        assert_eq!(choices[0].label, "builtin:default");
        assert_eq!(default_index, 1);
        assert_eq!(choices[1].selector.as_deref(), Some("project:coder"));
        assert_eq!(choices[1].label, "project:coder (default) — Project coder");
    }

    #[test]
    fn profile_cycle_selects_only_discovered_profiles() {
        let mut form = form("coder");
        form.profile_choices = vec![
            ProfileChoice {
                selector: Some("project:coder".to_string()),
                label: "project:coder (default)".to_string(),
                is_default: true,
            },
            ProfileChoice {
                selector: Some("user:reviewer".to_string()),
                label: "user:reviewer".to_string(),
                is_default: false,
            },
        ];
        form.profile_index = 0;

        assert_eq!(
            form.selected_profile_selector().as_deref(),
            Some("project:coder")
        );
        form.cycle_profile_next();
        assert_eq!(
            form.selected_profile_selector().as_deref(),
            Some("user:reviewer")
        );
        form.cycle_profile_next();
        assert_eq!(
            form.selected_profile_selector().as_deref(),
            Some("project:coder")
        );
        form.cycle_profile_prev();
        assert_eq!(
            form.selected_profile_selector().as_deref(),
            Some("user:reviewer")
        );
    }

    #[test]
    fn initial_profile_index_adds_explicit_selector_not_in_discovery_list() {
        let mut choices = Vec::new();
        let selected = initial_profile_index(&mut choices, Some("coder"), 0);
        assert_eq!(selected, 0);
        assert_eq!(choices[0].selector.as_deref(), Some("coder"));
        assert_eq!(choices[0].label, "coder");
    }

    #[test]
    fn name_input_handles_insert_backspace_and_cursor() {
        let mut f = form("");
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
