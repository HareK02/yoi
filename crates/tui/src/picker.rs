//! Inline-viewport "pick a Worker to attach or restore" UX.
//!
//! Reads live Worker allocations from the runtime registry and stopped Worker state
//! from the session-store worker metadata name-keyed metadata. Picking a live row attaches to
//! its socket; picking a stopped row restores via the Worker runtime command.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{self, Event as TermEvent, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::{Frame, TerminalOptions, Viewport};
use session_store::FsStore;
use session_store::FsWorkerStore;

use crate::worker_list::{
    LiveWorkerInfo, StoredMetadataState, StoredWorkerInfo, WorkerList, WorkerListEntry,
    WorkerVisibilitySource, live_socket_for_worker as worker_list_live_socket_for_worker,
    read_reachable_live_worker_infos, read_stored_worker_infos,
};

const MAX_ROWS: usize = 10;
const VIEWPORT_LINES: u16 = MAX_ROWS as u16 + 4;

#[derive(Debug)]
pub enum PickerError {
    Io(io::Error),
    Store(session_store::StoreError),
    NoWorkers { all: bool },
}

impl std::fmt::Display for PickerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::Store(e) => write!(f, "session store error: {e}"),
            Self::NoWorkers { all: true } => write!(
                f,
                "no workers found — start a fresh Worker with `yoi` and try again"
            ),
            Self::NoWorkers { all: false } => write!(
                f,
                "no workers found in this workspace — use `yoi resume --all` to list all host/data-dir Workers"
            ),
        }
    }
}

impl std::error::Error for PickerError {}

impl From<io::Error> for PickerError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<session_store::StoreError> for PickerError {
    fn from(e: session_store::StoreError) -> Self {
        Self::Store(e)
    }
}

pub enum PickerOutcome {
    /// User picked a Worker. `socket_override` is set for live rows when the
    /// runtime registry knows the exact socket path; stopped rows leave it
    /// empty so the caller restores by spawning the Worker runtime command.
    Picked {
        worker_name: String,
        socket_override: Option<PathBuf>,
    },
    Cancelled,
}

#[derive(Debug, Clone)]
pub(crate) struct PickerOptions {
    scope: PickerScope,
}

impl PickerOptions {
    pub(crate) fn workspace(workspace_root: PathBuf) -> Self {
        Self {
            scope: PickerScope::Workspace(workspace_root),
        }
    }

    pub(crate) fn all() -> Self {
        Self {
            scope: PickerScope::All,
        }
    }
}

#[derive(Debug, Clone)]
enum PickerScope {
    Workspace(PathBuf),
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkerRowState {
    Live,
    Stopped,
    Corrupt,
}

impl WorkerRowState {
    fn label(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Stopped => "stopped",
            Self::Corrupt => "corrupt",
        }
    }

    fn style(self) -> Style {
        match self {
            Self::Live => Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
            Self::Stopped => Style::default().fg(Color::Yellow),
            Self::Corrupt => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        }
    }
}

fn list_for_options(
    options: &PickerOptions,
    stored_workers: Vec<StoredWorkerInfo>,
    live_workers: Vec<LiveWorkerInfo>,
) -> WorkerList {
    match &options.scope {
        PickerScope::Workspace(workspace_root) => WorkerList::from_workspace_sources(
            WorkerVisibilitySource::ResumePicker,
            stored_workers,
            live_workers,
            None,
            MAX_ROWS,
            workspace_root,
        ),
        PickerScope::All => WorkerList::from_sources(
            WorkerVisibilitySource::ResumePicker,
            stored_workers,
            live_workers,
            None,
            MAX_ROWS,
        ),
    }
}

pub async fn run(options: PickerOptions) -> Result<PickerOutcome, PickerError> {
    let store_dir = default_store_dir()?;
    let store = FsStore::new(&store_dir)?;
    let worker_metadata_store =
        FsWorkerStore::new(default_worker_metadata_dir()?).map_err(io::Error::other)?;
    let stored_workers = read_stored_worker_infos(&store, &worker_metadata_store)?;
    let live_workers = read_reachable_live_worker_infos(&store)
        .await
        .unwrap_or_default();
    let mut list = list_for_options(&options, stored_workers, live_workers);
    if list.entries.is_empty() {
        return Err(PickerError::NoWorkers {
            all: matches!(options.scope, PickerScope::All),
        });
    }

    let mut terminal = make_inline_terminal()?;
    loop {
        terminal.draw(|f| draw(f, &list))?;
        match poll_event()? {
            None => continue,
            Some(Action::Up) => {
                let selected = list.selected_index().saturating_sub(1);
                list.select_index(selected);
            }
            Some(Action::Down) => {
                let selected = list.selected_index();
                if selected + 1 < list.entries.len() {
                    list.select_index(selected + 1);
                }
            }
            Some(Action::Submit) => {
                close_viewport(&mut terminal)?;
                let entry = list.selected_entry().expect("non-empty worker list");
                return Ok(PickerOutcome::Picked {
                    worker_name: entry.name.clone(),
                    socket_override: entry.attach_socket_path().map(PathBuf::from),
                });
            }
            Some(Action::Cancel) => {
                close_viewport(&mut terminal)?;
                return Ok(PickerOutcome::Cancelled);
            }
        }
    }
}

/// Park the cursor at the very bottom of the picker's inline viewport and emit
/// one newline before dropping the terminal. This keeps any next inline viewport
/// from drawing over the lower picker rows.
fn close_viewport(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> io::Result<()> {
    let area = terminal.get_frame().area();
    let last_row = area.bottom().saturating_sub(1);
    terminal.set_cursor_position((0, last_row))?;
    use std::io::Write;
    let mut out = io::stdout();
    out.write_all(b"\r\n")?;
    out.flush()?;
    Ok(())
}

fn default_store_dir() -> Result<PathBuf, PickerError> {
    manifest::paths::sessions_dir().ok_or_else(|| {
        PickerError::Io(io::Error::new(
            io::ErrorKind::NotFound,
            "could not resolve sessions directory \
             (set YOI_HOME, YOI_DATA_DIR, or HOME)",
        ))
    })
}

fn default_worker_metadata_dir() -> Result<PathBuf, PickerError> {
    manifest::paths::data_dir()
        .map(|dir| dir.join("workers"))
        .ok_or_else(|| {
            PickerError::Io(io::Error::new(
                io::ErrorKind::NotFound,
                "could not resolve worker state directory \
             (set YOI_HOME, YOI_DATA_DIR, or HOME)",
            ))
        })
}

pub(crate) fn live_socket_for_worker(worker_name: &str) -> Option<PathBuf> {
    worker_list_live_socket_for_worker(worker_name)
}

fn make_inline_terminal() -> io::Result<Terminal<CrosstermBackend<io::Stdout>>> {
    let backend = CrosstermBackend::new(io::stdout());
    Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Inline(VIEWPORT_LINES),
        },
    )
}

enum Action {
    Up,
    Down,
    Submit,
    Cancel,
}

fn poll_event() -> io::Result<Option<Action>> {
    if !event::poll(Duration::from_millis(100))? {
        return Ok(None);
    }
    match event::read()? {
        TermEvent::Key(k) if k.kind != KeyEventKind::Release => {
            let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
            Ok(match k.code {
                KeyCode::Up => Some(Action::Up),
                KeyCode::Down => Some(Action::Down),
                KeyCode::Char('k') if !ctrl => Some(Action::Up),
                KeyCode::Char('j') if !ctrl => Some(Action::Down),
                KeyCode::Enter => Some(Action::Submit),
                KeyCode::Esc => Some(Action::Cancel),
                KeyCode::Char('c') if ctrl => Some(Action::Cancel),
                _ => None,
            })
        }
        _ => Ok(None),
    }
}

fn draw(f: &mut Frame<'_>, list: &WorkerList) {
    let area = f.area();
    let mut constraints: Vec<Constraint> = Vec::with_capacity(list.entries.len() + 3);
    constraints.push(Constraint::Length(1)); // title
    for _ in &list.entries {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Length(1)); // hint
    constraints.push(Constraint::Length(1)); // spacer
    let layout = Layout::vertical(constraints).split(area);

    f.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            picker_title(),
            Style::default().add_modifier(Modifier::BOLD),
        )])),
        layout[0],
    );

    let selected = list.selected_index();
    for (i, entry) in list.entries.iter().enumerate() {
        f.render_widget(
            Paragraph::new(row_line(entry, i == selected)),
            layout[i + 1],
        );
    }

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw("  "),
            Span::styled("[↑/↓]", Style::default().fg(Color::DarkGray)),
            Span::raw(" select   "),
            Span::styled("[enter]", Style::default().fg(Color::Green)),
            Span::raw(" open/restore   "),
            Span::styled("[esc]", Style::default().fg(Color::Yellow)),
            Span::raw(" cancel"),
        ])),
        layout[list.entries.len() + 1],
    );
}

fn picker_title() -> &'static str {
    "resume worker   pick a worker"
}

fn row_line(entry: &WorkerListEntry, selected: bool) -> Line<'_> {
    let marker = if selected { "▶ " } else { "  " };
    let name_style = if selected {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Cyan)
    };
    let preview_style = if selected {
        Style::default().fg(Color::White)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let state = row_state(entry);
    let _visibility = entry.visibility;
    let _source_kinds = &entry.source_kinds;

    let mut spans = vec![
        Span::raw(marker),
        Span::styled(entry.name.as_str(), name_style),
        Span::raw("  "),
        Span::styled(format!("[{}]", state.label()), state.style()),
        Span::raw("  "),
        Span::styled(
            format_updated_at(entry.summary.updated_at),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw("  "),
        Span::styled(debug_ids(entry), Style::default().fg(Color::DarkGray)),
    ];
    if let Some(preview) = entry.summary.preview.as_ref() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(preview.as_str(), preview_style));
    }
    Line::from(spans)
}

fn row_state(entry: &WorkerListEntry) -> WorkerRowState {
    if entry.live.as_ref().is_some_and(|live| live.reachable) {
        return WorkerRowState::Live;
    }
    if entry
        .stored
        .as_ref()
        .is_some_and(|stored| matches!(stored.metadata_state, StoredMetadataState::Corrupt(_)))
    {
        return WorkerRowState::Corrupt;
    }
    WorkerRowState::Stopped
}

fn format_updated_at(updated_at: u64) -> String {
    if updated_at == 0 {
        "updated: —".to_string()
    } else {
        format!("updated: {updated_at}")
    }
}

fn debug_ids(entry: &WorkerListEntry) -> String {
    let session = entry
        .summary
        .active_session_id
        .map(short_id)
        .unwrap_or_else(|| "--------".to_string());
    let segment = entry
        .summary
        .active_segment_id
        .map(short_id)
        .unwrap_or_else(|| "--------".to_string());
    format!("s:{session} g:{segment}")
}

fn short_id<T: ToString>(id: T) -> String {
    id.to_string().chars().take(8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picker_title_names_pods_not_sessions() {
        assert_eq!(picker_title(), "resume worker   pick a worker");
    }

    #[test]
    fn picker_no_pods_message_mentions_all_for_workspace_scope() {
        let message = PickerError::NoWorkers { all: false }.to_string();
        assert!(message.contains("no workers found in this workspace"));
        assert!(message.contains("yoi resume --all"));
    }

    #[test]
    fn picker_no_pods_message_keeps_fresh_pod_hint_for_all_scope() {
        let message = PickerError::NoWorkers { all: true }.to_string();
        assert!(message.contains("start a fresh Worker with `yoi`"));
        assert!(!message.contains("yoi resume --all"));
    }

    #[test]
    fn picker_workspace_options_filter_by_workspace_metadata() {
        let list = list_for_options(
            &PickerOptions::workspace(PathBuf::from("/workspace/current")),
            vec![
                stored_pod("current", Some("/workspace/current"), 3),
                stored_pod("other", Some("/workspace/other"), 2),
                stored_pod("legacy", None, 1),
            ],
            vec![],
        );

        let names: Vec<_> = list
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(names, vec!["current"]);
    }

    #[test]
    fn picker_all_options_include_host_wide_and_legacy_pods() {
        let list = list_for_options(
            &PickerOptions::all(),
            vec![
                stored_pod("current", Some("/workspace/current"), 3),
                stored_pod("other", Some("/workspace/other"), 2),
                stored_pod("legacy", None, 1),
            ],
            vec![],
        );

        let names: Vec<_> = list
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(names, vec!["current", "other", "legacy"]);
    }

    fn stored_pod(name: &str, workspace_root: Option<&str>, updated_at: u64) -> StoredWorkerInfo {
        StoredWorkerInfo {
            worker_name: name.to_string(),
            metadata_state: StoredMetadataState::Present,
            active_session_id: None,
            active_segment_id: None,
            updated_at,
            workspace_root: workspace_root.map(PathBuf::from),
            preview: None,
        }
    }

    #[test]
    fn picker_row_shows_live_pending_preview_and_runtime_segment_id() {
        let segment_id = session_store::new_segment_id();
        let entry = WorkerList::from_sources(
            WorkerVisibilitySource::ResumePicker,
            vec![],
            vec![crate::worker_list::LiveWorkerInfo {
                worker_name: "pending".to_string(),
                socket_path: PathBuf::from("/tmp/pending.sock"),
                status: Some(protocol::WorkerStatus::Idle),
                reachable: true,
                segment_id: Some(segment_id),
                summary: crate::worker_list::WorkerEntrySummary::default(),
            }],
            None,
            10,
        )
        .entries
        .into_iter()
        .next()
        .unwrap();

        let text = row_line(&entry, false)
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert!(text.contains("[live]"));
        assert!(text.contains("[live, pending segment]"));
        assert!(text.contains(&format!("g:{}", short_id(segment_id))));
    }
}
