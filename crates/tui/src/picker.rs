//! Inline-viewport "pick a Pod to attach or restore" UX.
//!
//! Reads live Pod allocations from the runtime registry and stopped Pod state
//! from the session store's name-keyed metadata. Picking a live row attaches to
//! its socket; picking a stopped row restores via `pod --pod <name>`.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use client::PodClient;
use crossterm::event::{self, Event as TermEvent, KeyCode, KeyEventKind, KeyModifiers};
use pod_registry::{LockFileGuard, default_registry_path};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::{Frame, TerminalOptions, Viewport};
use session_store::{
    FsStore, LogEntry, LoggedContentPart, LoggedItem, PodMetadata, SegmentId, SessionId, Store,
};

const MAX_ROWS: usize = 10;
const VIEWPORT_LINES: u16 = MAX_ROWS as u16 + 4;

#[derive(Debug)]
pub enum PickerError {
    Io(io::Error),
    Store(session_store::StoreError),
    NoPods,
}

impl std::fmt::Display for PickerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::Store(e) => write!(f, "session store error: {e}"),
            Self::NoPods => write!(
                f,
                "no pods found — start a fresh pod with `tui` and try again"
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
    /// User picked a Pod. `socket_override` is set for live rows when the
    /// runtime registry knows the exact socket path; stopped rows leave it
    /// empty so the caller restores with `pod --pod <name>`.
    Picked {
        pod_name: String,
        socket_override: Option<PathBuf>,
    },
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PodRowState {
    Live,
    Stopped,
    Corrupt,
}

impl PodRowState {
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

/// One row in the Pod picker. The primary key is the Pod name; Session/Segment
/// IDs are included only as debug context.
#[derive(Debug, Clone)]
struct Row {
    pod_name: String,
    state: PodRowState,
    updated_at: u64,
    active_session_id: Option<SessionId>,
    active_segment_id: Option<SegmentId>,
    preview: Option<String>,
    socket_path: Option<PathBuf>,
}

#[derive(Debug)]
struct PodStateRecord {
    pod_name: String,
    state: Result<PodMetadata, String>,
}

#[derive(Debug, Clone)]
pub(crate) struct LivePodRecord {
    pub pod_name: String,
    pub socket_path: PathBuf,
    pub segment_id: Option<SegmentId>,
}

pub async fn run() -> Result<PickerOutcome, PickerError> {
    let store_dir = default_store_dir()?;
    let store = FsStore::new(&store_dir)?;
    let pod_states = read_pod_state_records(&store_dir)?;
    let live_pods = read_reachable_live_pod_records().await.unwrap_or_default();
    let rows = build_rows(&store, pod_states, live_pods)?;
    if rows.is_empty() {
        return Err(PickerError::NoPods);
    }

    let mut selected = 0usize;
    let mut terminal = make_inline_terminal()?;
    loop {
        terminal.draw(|f| draw(f, &rows, selected))?;
        match poll_event()? {
            None => continue,
            Some(Action::Up) => {
                selected = selected.saturating_sub(1);
            }
            Some(Action::Down) => {
                if selected + 1 < rows.len() {
                    selected += 1;
                }
            }
            Some(Action::Submit) => {
                close_viewport(&mut terminal)?;
                let row = &rows[selected];
                return Ok(PickerOutcome::Picked {
                    pod_name: row.pod_name.clone(),
                    socket_override: row.socket_path.clone(),
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
             (set INSOMNIA_HOME, INSOMNIA_DATA_DIR, or HOME)",
        ))
    })
}

fn read_pod_state_records(store_dir: &Path) -> Result<Vec<PodStateRecord>, PickerError> {
    let pods_dir = store_dir.join("pods");
    let mut records = Vec::new();
    if !pods_dir.exists() {
        return Ok(records);
    }

    for entry in fs::read_dir(pods_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let pod_name = entry.file_name().to_string_lossy().to_string();
        let path = entry.path().join("metadata.json");
        let state = match fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str::<PodMetadata>(&content).map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        };
        records.push(PodStateRecord { pod_name, state });
    }
    Ok(records)
}

fn read_live_pod_records() -> Result<Vec<LivePodRecord>, io::Error> {
    let path = default_registry_path()?;
    let guard = LockFileGuard::open(&path)?;
    Ok(guard
        .data()
        .allocations
        .iter()
        .map(|allocation| LivePodRecord {
            pod_name: allocation.pod_name.clone(),
            socket_path: allocation.socket.clone(),
            segment_id: allocation.segment_id,
        })
        .collect())
}

async fn read_reachable_live_pod_records() -> Result<Vec<LivePodRecord>, io::Error> {
    let records = read_live_pod_records()?;
    let mut reachable = Vec::new();
    for record in records {
        if PodClient::connect(&record.socket_path).await.is_ok() {
            reachable.push(record);
        }
    }
    Ok(reachable)
}

pub(crate) fn live_socket_for_pod(pod_name: &str) -> Option<PathBuf> {
    read_live_pod_records()
        .ok()?
        .into_iter()
        .find(|pod| pod.pod_name == pod_name)
        .map(|pod| pod.socket_path)
}

fn build_rows(
    store: &FsStore,
    pod_states: Vec<PodStateRecord>,
    live_pods: Vec<LivePodRecord>,
) -> Result<Vec<Row>, PickerError> {
    let mut rows_by_name: BTreeMap<String, Row> = BTreeMap::new();
    let mut live_by_name: HashMap<String, LivePodRecord> = HashMap::new();

    for live in live_pods {
        let (active_session_id, active_segment_id, updated_at, preview) =
            summarize_live_pod(store, &live);
        rows_by_name.insert(
            live.pod_name.clone(),
            Row {
                pod_name: live.pod_name.clone(),
                state: PodRowState::Live,
                updated_at,
                active_session_id,
                active_segment_id,
                preview,
                socket_path: Some(live.socket_path.clone()),
            },
        );
        live_by_name.insert(live.pod_name.clone(), live);
    }

    for record in pod_states {
        match record.state {
            Ok(metadata) => {
                let summary = summarize_metadata(store, &metadata);
                let state = if live_by_name.contains_key(&record.pod_name) {
                    PodRowState::Live
                } else {
                    PodRowState::Stopped
                };
                upsert_metadata_row(&mut rows_by_name, record.pod_name, metadata, summary, state);
            }
            Err(message) => {
                rows_by_name.entry(record.pod_name.clone()).or_insert(Row {
                    pod_name: record.pod_name,
                    state: PodRowState::Corrupt,
                    updated_at: 0,
                    active_session_id: None,
                    active_segment_id: None,
                    preview: Some(format!("metadata: {}", trim_one_line(&message, 48))),
                    socket_path: None,
                });
            }
        }
    }

    let mut rows: Vec<Row> = rows_by_name.into_values().collect();
    rows.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| a.pod_name.cmp(&b.pod_name))
    });
    rows.truncate(MAX_ROWS);
    Ok(rows)
}

fn upsert_metadata_row(
    rows_by_name: &mut BTreeMap<String, Row>,
    pod_name: String,
    metadata: PodMetadata,
    summary: SegmentSummary,
    state: PodRowState,
) {
    let active = metadata.active;
    let active_session_id = active.as_ref().map(|a| a.session_id);
    let active_segment_id = active.as_ref().and_then(|a| a.segment_id);

    match rows_by_name.get_mut(&pod_name) {
        Some(existing) => {
            existing.state = state;
            if summary.updated_at > existing.updated_at {
                existing.updated_at = summary.updated_at;
            }
            if existing.active_session_id.is_none() {
                existing.active_session_id = active_session_id;
            }
            if existing.active_segment_id.is_none() {
                existing.active_segment_id = active_segment_id;
            }
            if existing.preview.is_none() {
                existing.preview = summary.preview;
            }
        }
        None => {
            rows_by_name.insert(
                pod_name.clone(),
                Row {
                    pod_name,
                    state,
                    updated_at: summary.updated_at,
                    active_session_id,
                    active_segment_id,
                    preview: summary.preview,
                    socket_path: None,
                },
            );
        }
    }
}

#[derive(Debug, Clone)]
struct SegmentSummary {
    updated_at: u64,
    preview: Option<String>,
}

fn summarize_live_pod(
    store: &FsStore,
    live: &LivePodRecord,
) -> (Option<SessionId>, Option<SegmentId>, u64, Option<String>) {
    let Some(segment_id) = live.segment_id else {
        return (None, None, 0, None);
    };
    let session_id = store.lookup_session_of(segment_id).ok().flatten();
    let Some(session_id) = session_id else {
        return (None, Some(segment_id), 0, None);
    };
    let summary = summarize_segment(store, session_id, segment_id);
    (
        Some(session_id),
        Some(segment_id),
        summary.updated_at,
        summary.preview,
    )
}

fn summarize_metadata(store: &FsStore, metadata: &PodMetadata) -> SegmentSummary {
    let Some(active) = metadata.active.as_ref() else {
        return SegmentSummary {
            updated_at: 0,
            preview: None,
        };
    };
    let Some(segment_id) = active.segment_id else {
        return SegmentSummary {
            updated_at: 0,
            preview: Some("[pending segment]".to_string()),
        };
    };
    summarize_segment(store, active.session_id, segment_id)
}

fn summarize_segment(
    store: &FsStore,
    session_id: SessionId,
    segment_id: SegmentId,
) -> SegmentSummary {
    match store.read_all(session_id, segment_id) {
        Ok(entries) => SegmentSummary {
            updated_at: last_entry_ts(&entries).unwrap_or(0),
            preview: last_message_preview(&entries).or_else(|| Some("[empty]".to_string())),
        },
        Err(_) => SegmentSummary {
            updated_at: 0,
            preview: Some("[corrupt segment]".to_string()),
        },
    }
}

fn last_entry_ts(entries: &[LogEntry]) -> Option<u64> {
    entries.iter().map(log_entry_ts).max()
}

fn log_entry_ts(entry: &LogEntry) -> u64 {
    match entry {
        LogEntry::SegmentStart { ts, .. }
        | LogEntry::Invoke { ts, .. }
        | LogEntry::UserInput { ts, .. }
        | LogEntry::AssistantItem { ts, .. }
        | LogEntry::ToolResult { ts, .. }
        | LogEntry::SystemItem { ts, .. }
        | LogEntry::TurnEnd { ts, .. }
        | LogEntry::RunCompleted { ts, .. }
        | LogEntry::RunErrored { ts, .. }
        | LogEntry::ConfigChanged { ts, .. }
        | LogEntry::LlmUsage { ts, .. }
        | LogEntry::Extension { ts, .. } => *ts,
    }
}

/// Walk the log from the tail looking for the most recent user-message or
/// assistant-message entry, then render its first text fragment in a single line.
fn last_message_preview(entries: &[LogEntry]) -> Option<String> {
    for entry in entries.iter().rev() {
        match entry {
            LogEntry::UserInput { segments, .. } => {
                let text = protocol::Segment::flatten_to_text(segments);
                if !text.is_empty() {
                    return Some(format!("user: {}", trim_one_line(&text, 60)));
                }
            }
            LogEntry::AssistantItem { item, .. } => {
                if let Some(text) = first_text_logged(item) {
                    return Some(format!("assistant: {}", trim_one_line(&text, 60)));
                }
            }
            _ => {}
        }
    }
    None
}

fn first_text_logged(item: &LoggedItem) -> Option<String> {
    match item {
        LoggedItem::Message { content, .. } => content.iter().find_map(|p| match p {
            LoggedContentPart::Text { text } => Some(text.clone()),
            _ => None,
        }),
        _ => None,
    }
}

fn trim_one_line(s: &str, max_chars: usize) -> String {
    let collapsed: String = s.chars().map(|c| if c == '\n' { ' ' } else { c }).collect();
    if collapsed.chars().count() <= max_chars {
        collapsed
    } else {
        let truncated: String = collapsed.chars().take(max_chars - 1).collect();
        format!("{truncated}…")
    }
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

fn draw(f: &mut Frame<'_>, rows: &[Row], selected: usize) {
    let area = f.area();
    let mut constraints: Vec<Constraint> = Vec::with_capacity(rows.len() + 3);
    constraints.push(Constraint::Length(1)); // title
    for _ in rows {
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

    for (i, row) in rows.iter().enumerate() {
        f.render_widget(Paragraph::new(row_line(row, i == selected)), layout[i + 1]);
    }

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw("  "),
            Span::styled("[↑/↓]", Style::default().fg(Color::DarkGray)),
            Span::raw(" select   "),
            Span::styled("[enter]", Style::default().fg(Color::Green)),
            Span::raw(" attach/restore   "),
            Span::styled("[esc]", Style::default().fg(Color::Yellow)),
            Span::raw(" cancel"),
        ])),
        layout[rows.len() + 1],
    );
}

fn picker_title() -> &'static str {
    "resume pod   pick a pod"
}

fn row_line(row: &Row, selected: bool) -> Line<'_> {
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

    let mut spans = vec![
        Span::raw(marker),
        Span::styled(row.pod_name.as_str(), name_style),
        Span::raw("  "),
        Span::styled(format!("[{}]", row.state.label()), row.state.style()),
        Span::raw("  "),
        Span::styled(
            format_updated_at(row.updated_at),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw("  "),
        Span::styled(debug_ids(row), Style::default().fg(Color::DarkGray)),
    ];
    if let Some(preview) = row.preview.as_ref() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(preview.as_str(), preview_style));
    }
    Line::from(spans)
}

fn format_updated_at(updated_at: u64) -> String {
    if updated_at == 0 {
        "updated: —".to_string()
    } else {
        format!("updated: {updated_at}")
    }
}

fn debug_ids(row: &Row) -> String {
    let session = row
        .active_session_id
        .map(short_id)
        .unwrap_or_else(|| "--------".to_string());
    let segment = row
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
    use llm_worker::llm_client::types::RequestConfig;
    use session_store::{PodActiveSegmentRef, PodMetadataStore, new_segment_id, new_session_id};
    use tempfile::tempdir;

    #[test]
    fn pod_rows_are_sorted_by_active_segment_timestamp() {
        let dir = tempdir().unwrap();
        let store = FsStore::new(dir.path()).unwrap();
        let earlier_session = new_session_id();
        let later_session = new_session_id();
        let earlier_segment = new_segment_id();
        let later_segment = new_segment_id();

        append_start(&store, earlier_session, earlier_segment, 10);
        append_user(
            &store,
            earlier_session,
            earlier_segment,
            100,
            "old pod update",
        );
        append_start(&store, later_session, later_segment, 20);
        append_user(&store, later_session, later_segment, 200, "new pod update");

        let records = vec![
            metadata_record("older", earlier_session, earlier_segment),
            metadata_record("newer", later_session, later_segment),
        ];
        let rows = build_rows(&store, records, vec![]).unwrap();

        assert_eq!(rows[0].pod_name, "newer");
        assert_eq!(rows[0].state, PodRowState::Stopped);
        assert_eq!(rows[0].updated_at, 200);
        assert_eq!(rows[0].preview.as_deref(), Some("user: new pod update"));
        assert_eq!(rows[1].pod_name, "older");
    }

    #[test]
    fn pod_rows_include_live_and_stopped_pods() {
        let dir = tempdir().unwrap();
        let store = FsStore::new(dir.path()).unwrap();
        let stopped_session = new_session_id();
        let stopped_segment = new_segment_id();
        let live_session = new_session_id();
        let live_segment = new_segment_id();

        append_start(&store, stopped_session, stopped_segment, 10);
        append_user(
            &store,
            stopped_session,
            stopped_segment,
            50,
            "stopped preview",
        );
        append_start(&store, live_session, live_segment, 20);
        append_user(&store, live_session, live_segment, 70, "live preview");

        let rows = build_rows(
            &store,
            vec![metadata_record("stopped", stopped_session, stopped_segment)],
            vec![LivePodRecord {
                pod_name: "live".to_string(),
                socket_path: PathBuf::from("/tmp/live.sock"),
                segment_id: Some(live_segment),
            }],
        )
        .unwrap();

        let live = rows.iter().find(|row| row.pod_name == "live").unwrap();
        assert_eq!(live.state, PodRowState::Live);
        assert_eq!(live.active_session_id, Some(live_session));
        assert_eq!(
            live.socket_path.as_deref(),
            Some(Path::new("/tmp/live.sock"))
        );

        let stopped = rows.iter().find(|row| row.pod_name == "stopped").unwrap();
        assert_eq!(stopped.state, PodRowState::Stopped);
        assert_eq!(stopped.socket_path, None);
    }

    #[test]
    fn corrupt_pod_state_is_rendered_as_corrupt_row() {
        let dir = tempdir().unwrap();
        let store = FsStore::new(dir.path()).unwrap();
        let rows = build_rows(
            &store,
            vec![PodStateRecord {
                pod_name: "broken".to_string(),
                state: Err("expected value".to_string()),
            }],
            vec![],
        )
        .unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].pod_name, "broken");
        assert_eq!(rows[0].state, PodRowState::Corrupt);
        assert!(
            rows[0]
                .preview
                .as_deref()
                .unwrap()
                .contains("expected value")
        );
    }

    #[test]
    fn picker_title_names_pods_not_sessions() {
        assert_eq!(picker_title(), "resume pod   pick a pod");
    }

    fn metadata_record(
        pod_name: &str,
        session_id: SessionId,
        segment_id: SegmentId,
    ) -> PodStateRecord {
        PodStateRecord {
            pod_name: pod_name.to_string(),
            state: Ok(PodMetadata::new(
                pod_name,
                Some(PodActiveSegmentRef::active_segment(session_id, segment_id)),
            )),
        }
    }

    fn append_start(store: &FsStore, session_id: SessionId, segment_id: SegmentId, ts: u64) {
        store
            .append(
                session_id,
                segment_id,
                &LogEntry::SegmentStart {
                    ts,
                    session_id,
                    system_prompt: None,
                    config: RequestConfig::default(),
                    history: vec![],
                    forked_from: None,
                    compacted_from: None,
                },
            )
            .unwrap();
    }

    fn append_user(
        store: &FsStore,
        session_id: SessionId,
        segment_id: SegmentId,
        ts: u64,
        text: &str,
    ) {
        store
            .append(
                session_id,
                segment_id,
                &LogEntry::UserInput {
                    ts,
                    segments: vec![protocol::Segment::text(text)],
                },
            )
            .unwrap();
    }

    #[test]
    fn read_pod_state_records_reports_corrupt_metadata() {
        let dir = tempdir().unwrap();
        let pod_dir = dir.path().join("pods").join("broken");
        fs::create_dir_all(&pod_dir).unwrap();
        fs::write(pod_dir.join("metadata.json"), "{not-json").unwrap();

        let records = read_pod_state_records(dir.path()).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].pod_name, "broken");
        assert!(records[0].state.is_err());
    }

    #[test]
    fn read_pod_state_records_reads_metadata() {
        let dir = tempdir().unwrap();
        let store = FsStore::new(dir.path()).unwrap();
        let session_id = new_session_id();
        let segment_id = new_segment_id();
        store
            .write(&PodMetadata::new(
                "agent",
                Some(PodActiveSegmentRef::active_segment(session_id, segment_id)),
            ))
            .unwrap();

        let records = read_pod_state_records(dir.path()).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].pod_name, "agent");
        assert!(records[0].state.is_ok());
    }
}
