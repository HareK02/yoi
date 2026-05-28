use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crossterm::event::{Event as TermEvent, KeyCode, KeyEvent, KeyModifiers, read};
use protocol::stream::{JsonLineReader, JsonLineWriter};
use protocol::{ErrorCode, Event, InvokeKind, Method, PodStatus, Segment};
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block as UiBlock, Borders, Padding, Paragraph, Widget};
use session_store::FsStore;
use tokio::net::UnixStream;
use unicode_width::UnicodeWidthStr;

use crate::input::InputBuffer;
use crate::pod_list::{
    PodList, PodListEntry, PodVisibilitySource, StoredMetadataState, read_reachable_live_pod_infos,
    read_stored_pod_infos,
};

const MAX_ENTRIES: usize = 50;
const SOCKET_OP_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug)]
pub(crate) enum MultiPodError {
    Io(io::Error),
    Store(session_store::StoreError),
    NoPods,
}

impl std::fmt::Display for MultiPodError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::Store(e) => write!(f, "session store error: {e}"),
            Self::NoPods => write!(
                f,
                "no pods found — start a fresh pod with `tui` or restore one with `tui -r`"
            ),
        }
    }
}

impl std::error::Error for MultiPodError {}

impl From<io::Error> for MultiPodError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<session_store::StoreError> for MultiPodError {
    fn from(e: session_store::StoreError) -> Self {
        Self::Store(e)
    }
}

pub(crate) enum MultiPodOutcome {
    Quit,
    Open {
        pod_name: String,
        socket_override: Option<PathBuf>,
    },
}

pub(crate) async fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
) -> Result<MultiPodOutcome, MultiPodError> {
    let mut app = MultiPodApp::load(None).await?;
    if app.list.entries.is_empty() {
        return Err(MultiPodError::NoPods);
    }

    loop {
        terminal.draw(|f| draw(f, &mut app))?;
        match read()? {
            TermEvent::Key(key) => match app.handle_key(key) {
                MultiPodAction::None => {}
                MultiPodAction::Quit => return Ok(MultiPodOutcome::Quit),
                MultiPodAction::Open => {
                    if let Some(entry) = app.list.selected_entry() {
                        return Ok(MultiPodOutcome::Open {
                            pod_name: entry.name.clone(),
                            socket_override: entry.attach_socket_path().map(PathBuf::from),
                        });
                    }
                }
                MultiPodAction::Refresh => app.reload().await?,
                MultiPodAction::Send(request) => {
                    terminal.draw(|f| draw(f, &mut app))?;
                    let result = send_run_and_confirm(&request.socket_path, request.segments).await;
                    app.finish_send(result);
                    let _ = app.reload().await;
                }
            },
            TermEvent::Paste(text) => app.input.insert_paste(text),
            TermEvent::Resize(_, _) => {}
            _ => {}
        }
    }
}

fn default_store_dir() -> Result<PathBuf, MultiPodError> {
    manifest::paths::sessions_dir().ok_or_else(|| {
        MultiPodError::Io(io::Error::new(
            io::ErrorKind::NotFound,
            "could not resolve sessions directory \
             (set INSOMNIA_HOME, INSOMNIA_DATA_DIR, or HOME)",
        ))
    })
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SendEligibility {
    SendNow,
    Disabled,
}

#[derive(Debug)]
pub(crate) struct DirectSendRequest {
    socket_path: PathBuf,
    segments: Vec<Segment>,
}

pub(crate) struct MultiPodApp {
    pub(crate) list: PodList,
    pub(crate) input: InputBuffer,
    notice: Option<String>,
    sending: bool,
}

impl MultiPodApp {
    async fn load(selected_name: Option<String>) -> Result<Self, MultiPodError> {
        Ok(Self {
            list: load_pod_list(selected_name).await?,
            input: InputBuffer::new(),
            notice: None,
            sending: false,
        })
    }

    async fn reload(&mut self) -> Result<(), MultiPodError> {
        self.list = load_pod_list(self.list.selected_name.clone()).await?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn selected_send_eligibility(&self) -> SendEligibility {
        match self.list.selected_entry() {
            Some(entry) if entry.actions.can_send_now => SendEligibility::SendNow,
            _ => SendEligibility::Disabled,
        }
    }

    pub(crate) fn selected_send_disabled_reason(&self) -> Option<String> {
        let entry = self.list.selected_entry()?;
        if entry.actions.can_send_now {
            return None;
        }
        Some(send_disabled_reason(entry))
    }

    pub(crate) fn select_next(&mut self) {
        let selected = self.list.selected_index();
        if selected + 1 < self.list.entries.len() {
            self.list.select_index(selected + 1);
        }
    }

    pub(crate) fn select_prev(&mut self) {
        let selected = self.list.selected_index().saturating_sub(1);
        self.list.select_index(selected);
    }

    pub(crate) fn prepare_send(&mut self) -> Option<DirectSendRequest> {
        let entry = match self.list.selected_entry() {
            Some(entry) => entry,
            None => {
                self.notice = Some("No Pod is selected.".to_string());
                return None;
            }
        };
        if !entry.actions.can_send_now {
            self.notice = Some(send_disabled_reason(entry));
            return None;
        }
        let Some(socket_path) = entry.attach_socket_path().map(PathBuf::from) else {
            self.notice = Some("Selected Pod has no reachable socket.".to_string());
            return None;
        };
        let segments = self.input.submit_segments();
        if segments_are_blank(&segments) {
            self.notice = Some("Composer is empty.".to_string());
            return None;
        }
        self.sending = true;
        self.notice = Some(format!("Sending to {}…", entry.name));
        Some(DirectSendRequest {
            socket_path,
            segments,
        })
    }

    pub(crate) fn finish_send(&mut self, result: Result<(), DirectSendError>) {
        self.sending = false;
        match result {
            Ok(()) => {
                let target = self
                    .list
                    .selected_entry()
                    .map(|entry| entry.name.clone())
                    .unwrap_or_else(|| "selected Pod".to_string());
                self.input.clear();
                self.notice = Some(format!("Delivered to {target}."));
            }
            Err(e) => {
                self.notice = Some(format!("Delivery failed; composer kept: {e}"));
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> MultiPodAction {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Char('d') if ctrl => MultiPodAction::Quit,
            KeyCode::Char('c') if ctrl => MultiPodAction::Quit,
            KeyCode::Esc => MultiPodAction::Quit,
            KeyCode::Up => {
                self.select_prev();
                MultiPodAction::None
            }
            KeyCode::Down => {
                self.select_next();
                MultiPodAction::None
            }
            KeyCode::Char('k') if !ctrl && !alt => {
                self.select_prev();
                MultiPodAction::None
            }
            KeyCode::Char('j') if !ctrl && !alt => {
                self.select_next();
                MultiPodAction::None
            }
            KeyCode::Char('o') if !ctrl && !alt => MultiPodAction::Open,
            KeyCode::Char('r') if !ctrl && !alt => MultiPodAction::Refresh,
            KeyCode::Enter if alt => {
                self.input.insert_newline();
                MultiPodAction::None
            }
            KeyCode::Enter => self
                .prepare_send()
                .map(MultiPodAction::Send)
                .unwrap_or(MultiPodAction::None),
            KeyCode::Backspace => {
                self.input.delete_before();
                MultiPodAction::None
            }
            KeyCode::Delete => {
                self.input.delete_after();
                MultiPodAction::None
            }
            KeyCode::Left => {
                self.input.move_left();
                MultiPodAction::None
            }
            KeyCode::Right => {
                self.input.move_right();
                MultiPodAction::None
            }
            KeyCode::Home => {
                self.input.move_home();
                MultiPodAction::None
            }
            KeyCode::End => {
                self.input.move_end();
                MultiPodAction::None
            }
            KeyCode::Char(c) if !ctrl => {
                self.input.insert_char(c);
                MultiPodAction::None
            }
            _ => MultiPodAction::None,
        }
    }
}

enum MultiPodAction {
    None,
    Quit,
    Open,
    Refresh,
    Send(DirectSendRequest),
}

async fn load_pod_list(selected_name: Option<String>) -> Result<PodList, MultiPodError> {
    let store_dir = default_store_dir()?;
    let store = FsStore::new(&store_dir)?;
    let stored = read_stored_pod_infos(&store_dir, &store)?;
    let live = read_reachable_live_pod_infos(&store)
        .await
        .unwrap_or_default();
    Ok(PodList::from_sources(
        PodVisibilitySource::ResumePicker,
        stored,
        live,
        selected_name,
        MAX_ENTRIES,
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DirectSendError {
    AlreadyRunning,
    Rejected { code: ErrorCode, message: String },
    Io(String),
}

impl std::fmt::Display for DirectSendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyRunning => write!(f, "target Pod is already running"),
            Self::Rejected { code, message } => {
                write!(f, "target rejected run ({code:?}): {message}")
            }
            Self::Io(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for DirectSendError {}

async fn send_run_and_confirm(socket: &Path, input: Vec<Segment>) -> Result<(), DirectSendError> {
    let stream = tokio::time::timeout(SOCKET_OP_TIMEOUT, UnixStream::connect(socket))
        .await
        .map_err(|_| DirectSendError::Io("connect timed out".into()))?
        .map_err(|e| DirectSendError::Io(format!("connect: {e}")))?;
    let (reader, writer) = stream.into_split();
    let mut reader = JsonLineReader::new(reader);
    let mut writer = JsonLineWriter::new(writer);

    loop {
        let event = tokio::time::timeout(SOCKET_OP_TIMEOUT, reader.next::<Event>())
            .await
            .map_err(|_| DirectSendError::Io("read initial Snapshot timed out".into()))?
            .map_err(|e| DirectSendError::Io(format!("read initial Snapshot: {e}")))?;
        match event {
            Some(Event::Snapshot { .. }) => break,
            Some(Event::Alert(_)) => continue,
            Some(Event::Error {
                code: ErrorCode::AlreadyRunning,
                ..
            }) => return Err(DirectSendError::AlreadyRunning),
            Some(Event::Error { code, message }) => {
                return Err(DirectSendError::Rejected { code, message });
            }
            Some(_) => continue,
            None => {
                return Err(DirectSendError::Io(
                    "connection closed before initial Snapshot".into(),
                ));
            }
        }
    }

    tokio::time::timeout(SOCKET_OP_TIMEOUT, writer.write(&Method::Run { input }))
        .await
        .map_err(|_| DirectSendError::Io("write timed out".into()))?
        .map_err(|e| DirectSendError::Io(format!("write: {e}")))?;

    loop {
        let event = tokio::time::timeout(SOCKET_OP_TIMEOUT, reader.next::<Event>())
            .await
            .map_err(|_| DirectSendError::Io("read response timed out".into()))?
            .map_err(|e| DirectSendError::Io(format!("read response: {e}")))?;
        match event {
            Some(Event::Error {
                code: ErrorCode::AlreadyRunning,
                ..
            }) => return Err(DirectSendError::AlreadyRunning),
            Some(Event::Error { code, message }) => {
                return Err(DirectSendError::Rejected { code, message });
            }
            Some(Event::InvokeStart {
                kind: InvokeKind::UserSend,
            })
            | Some(Event::UserMessage { .. })
            | Some(Event::TurnStart { .. }) => return Ok(()),
            Some(_) => continue,
            None => {
                return Err(DirectSendError::Io(
                    "connection closed before response".into(),
                ));
            }
        }
    }
}

fn segments_are_blank(segments: &[Segment]) -> bool {
    segments.iter().all(|segment| match segment {
        Segment::Text { content } => content.trim().is_empty(),
        _ => false,
    })
}

fn send_disabled_reason(entry: &PodListEntry) -> String {
    if let Some(live) = entry.live.as_ref() {
        if !live.reachable {
            return "Selected live Pod is unreachable.".to_string();
        }
        return match live.status {
            Some(PodStatus::Running) => {
                "Selected Pod is running; direct send is disabled in multi-Pod view.".to_string()
            }
            Some(PodStatus::Paused) => {
                "Selected Pod is paused; open it explicitly to resume or start a new turn."
                    .to_string()
            }
            Some(PodStatus::Idle) => "Selected Pod is not send-eligible.".to_string(),
            None => "Selected Pod did not report a live status.".to_string(),
        };
    }
    if entry.stored.is_some() {
        return "Selected Pod is stopped; open/restore it before sending.".to_string();
    }
    entry
        .actions
        .disabled_reason
        .clone()
        .unwrap_or_else(|| "Selected Pod is not send-eligible.".to_string())
}

fn row_status_label(entry: &PodListEntry) -> (&'static str, Style) {
    if let Some(live) = entry.live.as_ref() {
        if !live.reachable {
            return (
                "unreachable",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            );
        }
        return match live.status {
            Some(PodStatus::Idle) => (
                "live idle",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Some(PodStatus::Running) => (
                "live running",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Some(PodStatus::Paused) => (
                "live paused",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            None => ("live unknown", Style::default().fg(Color::DarkGray)),
        };
    }
    if entry
        .stored
        .as_ref()
        .is_some_and(|stored| matches!(stored.metadata_state, StoredMetadataState::Corrupt(_)))
    {
        return (
            "corrupt",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        );
    }
    ("stopped/restorable", Style::default().fg(Color::Yellow))
}

fn draw(frame: &mut Frame<'_>, app: &mut MultiPodApp) {
    let area = frame.area();
    let input_content_width = area.width.saturating_sub(2).max(1);
    let input_render = app.input.render(input_content_width);
    let input_height = input_area_height(&input_render, area.height);
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(input_height),
        Constraint::Length(1),
    ])
    .split(area);

    draw_title(frame, chunks[0]);
    draw_list(frame, app, chunks[1]);
    draw_separator(frame, chunks[2]);
    draw_target_status(frame, app, chunks[3]);
    draw_input(frame, &input_render, chunks[4]);
    draw_actionbar(frame, app, chunks[5]);
}

fn input_area_height(render: &crate::input::InputRender, terminal_height: u16) -> u16 {
    let needed = render.lines.len().max(1) as u16;
    let cap = (terminal_height / 3).max(1).min(10);
    needed.clamp(1, cap)
}

fn draw_title(frame: &mut Frame<'_>, area: Rect) {
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "multi-Pod dashboard",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "  Enter send to idle live Pod · o open/attach · r refresh",
                Style::default().fg(Color::DarkGray),
            ),
        ])),
        area,
    );
}

fn draw_list(frame: &mut Frame<'_>, app: &MultiPodApp, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let block = UiBlock::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(Color::DarkGray))
        .padding(Padding::horizontal(1));
    let inner = block.inner(area);
    let mut lines = Vec::new();
    let selected = app.list.selected_index();
    let visible_height = inner.height as usize;
    let start = if visible_height == 0 {
        0
    } else {
        selected.saturating_sub(visible_height.saturating_sub(1))
    };
    for (i, entry) in app
        .list
        .entries
        .iter()
        .enumerate()
        .skip(start)
        .take(visible_height)
    {
        lines.push(row_line(entry, i == selected, inner.width));
    }
    Paragraph::new(lines)
        .block(block)
        .render(area, frame.buffer_mut());
}

fn row_line(entry: &PodListEntry, selected: bool, width: u16) -> Line<'static> {
    let marker = if selected { "▶ " } else { "  " };
    let name_style = if selected {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Cyan)
    };
    let (status, status_style) = row_status_label(entry);
    let action = if entry.actions.can_send_now {
        "send"
    } else if entry.actions.can_open {
        "open"
    } else {
        "disabled"
    };
    let mut text = format!("{marker}{}  [{status}]  {action}", entry.name);
    if let Some(preview) = entry.summary.preview.as_deref() {
        text.push_str("  ");
        text.push_str(preview);
    }
    let truncated = truncate_with_ellipsis(&text, width as usize);
    let mut spans = Vec::new();
    let prefix = format!("{marker}{}", entry.name);
    let visible_prefix = format!("{marker}{}  ", entry.name);
    spans.push(Span::styled(visible_prefix, name_style));
    spans.push(Span::styled(format!("[{status}]"), status_style));
    let rest = truncated
        .strip_prefix(&format!("{prefix}  [{status}]"))
        .unwrap_or("");
    spans.push(Span::styled(
        rest.to_string(),
        Style::default().fg(Color::DarkGray),
    ));
    Line::from(spans)
}

fn draw_separator(frame: &mut Frame<'_>, area: Rect) {
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "─".repeat(area.width as usize),
            Style::default().fg(Color::DarkGray),
        ))),
        area,
    );
}

fn draw_target_status(frame: &mut Frame<'_>, app: &MultiPodApp, area: Rect) {
    let line = match app.list.selected_entry() {
        Some(entry) => {
            let (status, status_style) = row_status_label(entry);
            let send_text = if entry.actions.can_send_now {
                "send enabled"
            } else {
                "send disabled"
            };
            Line::from(vec![
                Span::styled("target ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    entry.name.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(format!("[{status}]"), status_style),
                Span::raw("  "),
                Span::styled(
                    send_text,
                    if entry.actions.can_send_now {
                        Style::default().fg(Color::Green)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    },
                ),
            ])
        }
        None => Line::from(Span::styled(
            "target — none",
            Style::default().fg(Color::DarkGray),
        )),
    };
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_input(frame: &mut Frame<'_>, render: &crate::input::InputRender, area: Rect) {
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(render.lines.len());
    for (i, src) in render.lines.iter().enumerate() {
        let prefix = if i == 0 { "> " } else { "  " };
        let mut spans = vec![Span::styled(prefix, Style::default().fg(Color::DarkGray))];
        spans.extend(src.spans.iter().cloned());
        lines.push(Line::from(spans));
    }
    frame.render_widget(Paragraph::new(lines), area);

    let cursor_x = area.x + 2 + render.cursor_col;
    let cursor_y = area.y + render.cursor_row;
    if cursor_y < area.y + area.height {
        frame.set_cursor_position(Position::new(cursor_x, cursor_y));
    }
}

fn draw_actionbar(frame: &mut Frame<'_>, app: &MultiPodApp, area: Rect) {
    let left = if app.sending {
        "sending…".to_string()
    } else if let Some(notice) = app.notice.as_deref() {
        notice.to_string()
    } else if let Some(reason) = app.selected_send_disabled_reason() {
        reason
    } else {
        "idle live target: Enter sends directly without opening conversation".to_string()
    };
    let right = "↑/↓ select  Enter send  o open  r refresh  Esc quit";
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            truncate_with_ellipsis(&left, area.width.saturating_sub(42) as usize),
            Style::default().fg(Color::DarkGray),
        ))),
        area,
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            right,
            Style::default().fg(Color::DarkGray),
        )))
        .alignment(ratatui::layout::Alignment::Right),
        area,
    );
}

fn truncate_with_ellipsis(s: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if s.width() <= max_width {
        return s.to_string();
    }
    if max_width == 1 {
        return "…".to_string();
    }
    let mut out = String::new();
    let mut width = 0usize;
    for c in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if width + cw > max_width - 1 {
            break;
        }
        out.push(c);
        width += cw;
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pod_list::{LivePodInfo, PodEntrySummary, StoredMetadataState, StoredPodInfo};

    #[test]
    fn multi_selection_changes_preserve_composer_contents() {
        let mut app = test_app(vec![
            live_info("alpha", PodStatus::Idle),
            live_info("beta", PodStatus::Idle),
        ]);
        app.input.insert_str("draft message");
        let before = input_text(&app);

        app.select_next();

        assert_eq!(input_text(&app), before);
        assert_eq!(app.list.selected_entry().unwrap().name, "beta");
    }

    #[test]
    fn multi_idle_live_selected_target_is_send_eligible() {
        let app = test_app(vec![live_info("idle", PodStatus::Idle)]);

        assert_eq!(app.selected_send_eligibility(), SendEligibility::SendNow);
        assert!(app.selected_send_disabled_reason().is_none());
    }

    #[test]
    fn multi_running_paused_and_stopped_targets_are_direct_send_disabled() {
        let mut app = test_app(vec![
            live_info("running", PodStatus::Running),
            live_info("paused", PodStatus::Paused),
        ]);
        let stopped = stopped_info("stopped");
        app.list = PodList::from_sources(
            PodVisibilitySource::ResumePicker,
            vec![stopped],
            vec![
                live_info_with_updated_at("running", PodStatus::Running, 30),
                live_info_with_updated_at("paused", PodStatus::Paused, 20),
            ],
            Some("running".to_string()),
            10,
        );

        assert_eq!(app.selected_send_eligibility(), SendEligibility::Disabled);
        assert!(
            app.selected_send_disabled_reason()
                .unwrap()
                .contains("running")
        );
        app.select_next();
        assert_eq!(app.list.selected_entry().unwrap().name, "paused");
        assert_eq!(app.selected_send_eligibility(), SendEligibility::Disabled);
        assert!(
            app.selected_send_disabled_reason()
                .unwrap()
                .contains("paused")
        );
        app.select_next();
        assert_eq!(app.list.selected_entry().unwrap().name, "stopped");
        assert_eq!(app.selected_send_eligibility(), SendEligibility::Disabled);
        assert!(
            app.selected_send_disabled_reason()
                .unwrap()
                .contains("stopped")
        );
    }

    #[test]
    fn multi_delivery_failure_keeps_composer_contents() {
        let mut app = test_app(vec![live_info("idle", PodStatus::Idle)]);
        app.input.insert_str("keep me");
        let before = input_text(&app);

        app.finish_send(Err(DirectSendError::Io("boom".to_string())));

        assert_eq!(input_text(&app), before);
        assert!(app.notice.as_deref().unwrap().contains("composer kept"));
    }

    #[test]
    fn multi_delivery_success_clears_composer_contents() {
        let mut app = test_app(vec![live_info("idle", PodStatus::Idle)]);
        app.input.insert_str("send me");

        app.finish_send(Ok(()));

        assert_eq!(input_text(&app), "");
        assert!(app.notice.as_deref().unwrap().contains("Delivered"));
    }

    fn test_app(live: Vec<LivePodInfo>) -> MultiPodApp {
        MultiPodApp {
            list: PodList::from_sources(PodVisibilitySource::ResumePicker, vec![], live, None, 10),
            input: InputBuffer::new(),
            notice: None,
            sending: false,
        }
    }

    fn live_info(pod_name: &str, status: PodStatus) -> LivePodInfo {
        live_info_with_updated_at(pod_name, status, 0)
    }

    fn live_info_with_updated_at(
        pod_name: &str,
        status: PodStatus,
        updated_at: u64,
    ) -> LivePodInfo {
        LivePodInfo {
            pod_name: pod_name.to_string(),
            socket_path: PathBuf::from(format!("/tmp/{pod_name}.sock")),
            status: Some(status),
            reachable: true,
            segment_id: None,
            summary: PodEntrySummary {
                active_session_id: None,
                active_segment_id: None,
                updated_at,
                preview: None,
            },
        }
    }

    fn stopped_info(pod_name: &str) -> StoredPodInfo {
        StoredPodInfo {
            pod_name: pod_name.to_string(),
            metadata_state: StoredMetadataState::Present,
            active_session_id: None,
            active_segment_id: None,
            updated_at: 10,
            preview: None,
        }
    }

    fn input_text(app: &MultiPodApp) -> String {
        Segment::flatten_to_text(&app.input.submit_segments())
    }
}
