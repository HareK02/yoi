//! Inline-viewport "pick a session to restore" UX.
//!
//! Reads the most recently updated sessions from the configured store,
//! lets the user pick one with the arrow keys, and returns the chosen
//! `SegmentId`. Closes its inline viewport before returning so the
//! caller can open a fresh viewport for the name dialog.
//!
//! The picker only handles selection. Forking, pod-registry checks, and
//! actual `pod` launch happen later in the resume flow.

use std::io;
use std::time::Duration;

use crossterm::event::{self, Event as TermEvent, KeyCode, KeyEventKind, KeyModifiers};
use pod_registry::lookup_segment;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::{Frame, TerminalOptions, Viewport};
use session_store::{
    FsStore, LogEntry, LoggedContentPart, LoggedItem, SegmentId, SessionId, Store,
};

const MAX_ROWS: usize = 10;
const VIEWPORT_LINES: u16 = MAX_ROWS as u16 + 4;

#[derive(Debug)]
pub enum PickerError {
    Io(io::Error),
    Store(session_store::StoreError),
    NoSessions,
}

impl std::fmt::Display for PickerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::Store(e) => write!(f, "session store error: {e}"),
            Self::NoSessions => write!(
                f,
                "no sessions found — start a fresh pod with `tui` and try again"
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
    /// User picked a session; resume at the segment represented by the
    /// selected row. The pod-cli rehydrates `session_id` via
    /// `Store::lookup_session_of` so we only need to surface the segment
    /// here.
    Picked {
        segment_id: SegmentId,
    },
    Cancelled,
}

/// One row in the picker view. Rendered from the most recently updated
/// segment of a Session so the user can recognise their conversation at a
/// glance without parsing UUIDs.
struct Row {
    session_id: SessionId,
    segment_id: SegmentId,
    /// Latest log-entry timestamp in the row's selected segment. Used only
    /// to order the picker newest-update first.
    updated_at: u64,
    /// Last user / assistant snippet, or a `[corrupt]` placeholder.
    preview: String,
    /// `Some(pod_name)` when a live Pod currently holds an allocation
    /// for this row's segment in `pods.json`. Picking such a row launches
    /// `pod --session <UUID>` which will fail with `SegmentConflict` — the
    /// badge warns the user up-front.
    live_pod: Option<String>,
}

pub async fn run() -> Result<PickerOutcome, PickerError> {
    let store = open_default_store()?;
    let sessions = store.list_sessions()?;
    if sessions.is_empty() {
        return Err(PickerError::NoSessions);
    }
    let rows = build_rows(&store, sessions)?;
    if rows.is_empty() {
        return Err(PickerError::NoSessions);
    }

    let mut selected = 0usize;
    let mut terminal = make_inline_terminal()?;
    loop {
        terminal.draw(|f| draw(f, &rows, selected))?;
        match poll_event()? {
            None => continue,
            Some(Action::Up) => {
                if selected > 0 {
                    selected -= 1;
                }
            }
            Some(Action::Down) => {
                if selected + 1 < rows.len() {
                    selected += 1;
                }
            }
            Some(Action::Submit) => {
                close_viewport(&mut terminal)?;
                return Ok(PickerOutcome::Picked {
                    segment_id: rows[selected].segment_id,
                });
            }
            Some(Action::Cancel) => {
                close_viewport(&mut terminal)?;
                return Ok(PickerOutcome::Cancelled);
            }
        }
    }
}

/// Park the cursor at the very bottom of the picker's inline viewport
/// and emit one newline before dropping the terminal. Without this the
/// inline area is left with the cursor still inside it, so the next
/// `Terminal::with_options(Inline(_))` call (the resume name dialog)
/// computes its own area starting from inside the picker — drawing the
/// new dialog on top of the lower picker rows.
///
/// Setting the cursor to `area.bottom() - 1` and writing `\r\n`
/// scrolls the terminal up exactly one row, so the next inline
/// viewport opens immediately below the picker rather than on top of
/// it.
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

fn open_default_store() -> Result<FsStore, PickerError> {
    let dir = manifest::paths::sessions_dir().ok_or_else(|| {
        PickerError::Io(io::Error::new(
            io::ErrorKind::NotFound,
            "could not resolve sessions directory \
             (set INSOMNIA_HOME, INSOMNIA_DATA_DIR, or HOME)",
        ))
    })?;
    Ok(FsStore::new(&dir)?)
}

fn build_rows(store: &FsStore, sessions: Vec<SessionId>) -> Result<Vec<Row>, PickerError> {
    let mut rows = Vec::new();
    for session_id in sessions {
        let mut selected_segment: Option<(SegmentId, u64, String)> = None;
        for segment_id in store.list_segments(session_id)? {
            let (updated_at, preview) = summarize_segment(store, session_id, segment_id);
            if selected_segment
                .as_ref()
                .is_none_or(|(best_segment_id, best_updated_at, _)| {
                    updated_at > *best_updated_at
                        || (updated_at == *best_updated_at && segment_id > *best_segment_id)
                })
            {
                selected_segment = Some((segment_id, updated_at, preview));
            }
        }

        let Some((segment_id, updated_at, preview)) = selected_segment else {
            continue;
        };
        rows.push(Row {
            session_id,
            segment_id,
            updated_at,
            preview,
            live_pod: None,
        });
    }

    rows.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| b.segment_id.cmp(&a.segment_id))
            .then_with(|| b.session_id.cmp(&a.session_id))
    });
    rows.truncate(MAX_ROWS);
    for row in &mut rows {
        // Best-effort live check. A pods.json I/O hiccup downgrades
        // the row to "no badge" rather than killing the picker — the
        // user still gets to see the listing.
        row.live_pod = lookup_segment(row.segment_id)
            .ok()
            .flatten()
            .map(|info| info.pod_name);
    }
    Ok(rows)
}

fn summarize_segment(
    store: &FsStore,
    session_id: SessionId,
    segment_id: SegmentId,
) -> (u64, String) {
    match store.read_all(session_id, segment_id) {
        Ok(entries) => (
            last_entry_ts(&entries).unwrap_or(0),
            last_message_preview(&entries).unwrap_or_else(|| "[empty]".to_string()),
        ),
        Err(_) => (0, "[corrupt]".to_string()),
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

/// Walk the log from the tail looking for the most recent user-message
/// or assistant-message entry, then render its first text fragment in
/// a single line.
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
            "resume pod   pick a session",
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
            Span::raw(" pick   "),
            Span::styled("[esc]", Style::default().fg(Color::Yellow)),
            Span::raw(" cancel"),
        ])),
        layout[rows.len() + 1],
    );
}

fn row_line(row: &Row, selected: bool) -> Line<'_> {
    let marker = if selected { "▶ " } else { "  " };
    let id_style = if selected {
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
        Span::styled(short_segment(row.session_id), id_style),
        Span::raw("  "),
    ];
    if let Some(ref pod_name) = row.live_pod {
        spans.push(Span::styled(
            format!("[live: {pod_name}] "),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ));
    }
    spans.push(Span::styled(row.preview.clone(), preview_style));
    Line::from(spans)
}

fn short_segment(id: SessionId) -> String {
    let s = id.to_string();
    s.chars().take(8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm_worker::llm_client::types::RequestConfig;
    use session_store::{new_segment_id, new_session_id};
    use tempfile::tempdir;

    #[test]
    fn rows_are_sorted_by_latest_log_entry_timestamp() {
        let dir = tempdir().unwrap();
        let store = FsStore::new(dir.path()).unwrap();
        let earlier_session = new_session_id();
        let later_session = new_session_id();
        let earlier_segment = new_segment_id();
        let later_segment = new_segment_id();

        append_start(&store, earlier_session, earlier_segment, 10);
        append_start(&store, later_session, later_segment, 20);
        append_user(
            &store,
            earlier_session,
            earlier_segment,
            100,
            "latest update",
        );

        let rows = build_rows(&store, store.list_sessions().unwrap()).unwrap();

        assert_eq!(rows[0].session_id, earlier_session);
        assert_eq!(rows[0].segment_id, earlier_segment);
        assert_eq!(rows[0].updated_at, 100);
        assert_eq!(rows[0].preview, "user: latest update");
        assert_eq!(rows[1].session_id, later_session);
    }

    #[test]
    fn row_uses_the_most_recently_updated_segment_in_a_session() {
        let dir = tempdir().unwrap();
        let store = FsStore::new(dir.path()).unwrap();
        let session_id = new_session_id();
        let old_segment = new_segment_id();
        let new_segment = new_segment_id();

        append_start(&store, session_id, old_segment, 10);
        append_start(&store, session_id, new_segment, 20);
        append_user(&store, session_id, old_segment, 200, "continued old branch");

        let rows = build_rows(&store, vec![session_id]).unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].segment_id, old_segment);
        assert_eq!(rows[0].updated_at, 200);
        assert_eq!(rows[0].preview, "user: continued old branch");
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
}
