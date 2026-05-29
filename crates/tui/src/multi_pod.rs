use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{Event as TermEvent, KeyCode, KeyEvent, KeyModifiers, poll, read};
use protocol::stream::{JsonLineReader, JsonLineWriter};
use protocol::{ErrorCode, Event, InvokeKind, Method, PodStatus, Segment};
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use session_store::FsStore;
use tokio::net::UnixStream;
use unicode_width::UnicodeWidthStr;

use crate::input::InputBuffer;
use crate::pod_list::{
    PodList, PodListEntry, PodVisibilitySource, StoredMetadataState, read_reachable_live_pod_infos,
    read_stored_pod_infos,
};

const MAX_ENTRIES: usize = 50;
const CLOSED_VISIBLE_ROWS: usize = 3;
const SOCKET_OP_TIMEOUT: Duration = Duration::from_secs(3);
const MULTI_POD_POLL_INTERVAL: Duration = Duration::from_millis(1_500);
const TERMINAL_EVENT_POLL_INTERVAL: Duration = Duration::from_millis(100);

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
                "no pods found — start a fresh pod with `insomnia` or restore one with `insomnia -r`"
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
    Open(OpenPodRequest),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenPodRequest {
    pub(crate) pod_name: String,
    pub(crate) socket_override: Option<PathBuf>,
}

pub(crate) async fn load_app() -> Result<MultiPodApp, MultiPodError> {
    MultiPodApp::load(None).await
}

pub(crate) async fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut MultiPodApp,
) -> Result<MultiPodOutcome, MultiPodError> {
    if app.list.entries.is_empty() {
        return Err(MultiPodError::NoPods);
    }

    let mut pending_reload = PendingReload::default();
    let mut next_poll = Instant::now() + MULTI_POD_POLL_INTERVAL;

    loop {
        if let Some(result) = pending_reload.finish_if_ready().await {
            app.apply_reload_result(result);
        }

        terminal.draw(|f| draw(f, app))?;

        let now = Instant::now();
        if now >= next_poll {
            pending_reload.start();
            next_poll = now + MULTI_POD_POLL_INTERVAL;
            continue;
        }

        let event_wait = TERMINAL_EVENT_POLL_INTERVAL.min(next_poll.saturating_duration_since(now));
        if !poll(event_wait)? {
            continue;
        }

        match read()? {
            TermEvent::Key(key) => match app.handle_key(key) {
                MultiPodAction::None => {}
                MultiPodAction::Quit => return Ok(MultiPodOutcome::Quit),
                MultiPodAction::Open => {
                    if let Some(request) = app.prepare_open() {
                        return Ok(MultiPodOutcome::Open(request));
                    }
                }
                MultiPodAction::Refresh => {
                    if !pending_reload.start() {
                        app.notice = Some("Refresh already in progress.".to_string());
                    }
                }
                MultiPodAction::Send(request) => {
                    pending_reload.abort();
                    terminal.draw(|f| draw(f, app))?;
                    let result = send_run_and_confirm(&request.socket_path, request.segments).await;
                    app.finish_send(result);
                    app.reload_or_notice().await;
                    next_poll = Instant::now() + MULTI_POD_POLL_INTERVAL;
                }
            },
            TermEvent::Paste(text) => app.input.insert_paste(text),
            TermEvent::Resize(_, _) => {}
            _ => {}
        }
    }
}

struct PendingReload {
    handle: Option<tokio::task::JoinHandle<Result<PodList, MultiPodError>>>,
}

impl PendingReload {
    fn start(&mut self) -> bool {
        if self.handle.is_some() {
            return false;
        }
        self.handle = Some(tokio::spawn(async { load_pod_list(None).await }));
        true
    }

    #[cfg(test)]
    fn start_with_handle(
        &mut self,
        handle: tokio::task::JoinHandle<Result<PodList, MultiPodError>>,
    ) -> bool {
        if self.handle.is_some() {
            handle.abort();
            return false;
        }
        self.handle = Some(handle);
        true
    }

    async fn finish_if_ready(&mut self) -> Option<Result<PodList, MultiPodError>> {
        if !self.handle.as_ref()?.is_finished() {
            return None;
        }
        let handle = self.handle.take()?;
        Some(match handle.await {
            Ok(result) => result,
            Err(e) => Err(MultiPodError::Io(io::Error::other(format!(
                "reload task failed: {e}"
            )))),
        })
    }

    fn abort(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}

impl Default for PendingReload {
    fn default() -> Self {
        Self { handle: None }
    }
}

impl Drop for PendingReload {
    fn drop(&mut self) {
        self.abort();
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
        let mut app = Self {
            list: load_pod_list(selected_name).await?,
            input: InputBuffer::new(),
            notice: None,
            sending: false,
        };
        app.ensure_selection_visible();
        Ok(app)
    }

    pub(crate) async fn reload_or_notice(&mut self) {
        let result = load_pod_list(None).await;
        self.apply_reload_result(result);
    }

    fn apply_reload_result(&mut self, result: Result<PodList, MultiPodError>) {
        match result {
            Ok(list) => self.apply_reloaded_list(list),
            Err(error) => {
                self.notice = Some(format!("Refresh failed: {error}"));
            }
        }
    }

    fn apply_reloaded_list(&mut self, mut list: PodList) {
        list.selected_name = self
            .list
            .selected_name
            .clone()
            .filter(|name| list.entries.iter().any(|entry| entry.name == *name))
            .or_else(|| list.entries.first().map(|entry| entry.name.clone()));
        self.list = list;
        self.ensure_selection_visible();
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
        let visible = visible_entry_indices(&self.list);
        if visible.is_empty() {
            self.list.selected_name = None;
            return;
        }
        let selected = self.list.selected_index();
        let Some(selected_pos) = visible.iter().position(|index| *index == selected) else {
            self.list.select_index(visible[0]);
            return;
        };
        let next_pos = (selected_pos + 1).min(visible.len() - 1);
        self.list.select_index(visible[next_pos]);
    }

    pub(crate) fn select_prev(&mut self) {
        let visible = visible_entry_indices(&self.list);
        if visible.is_empty() {
            self.list.selected_name = None;
            return;
        }
        let selected = self.list.selected_index();
        let Some(selected_pos) = visible.iter().position(|index| *index == selected) else {
            self.list.select_index(visible[0]);
            return;
        };
        let prev_pos = selected_pos.saturating_sub(1);
        self.list.select_index(visible[prev_pos]);
    }

    fn ensure_selection_visible(&mut self) {
        let visible = visible_entry_indices(&self.list);
        if visible.is_empty() {
            self.list.selected_name = None;
            return;
        }
        let selected = self.list.selected_index();
        if !visible.contains(&selected) {
            self.list.select_index(visible[0]);
        }
    }

    pub(crate) fn prepare_open(&mut self) -> Option<OpenPodRequest> {
        let entry = match self.list.selected_entry() {
            Some(entry) => entry,
            None => {
                self.notice = Some("No Pod is selected.".to_string());
                return None;
            }
        };
        if !entry.actions.can_open {
            self.notice = Some("Selected Pod cannot be opened from this view.".to_string());
            return None;
        }
        self.notice = Some(format!("Opening {}…", entry.name));
        Some(OpenPodRequest {
            pod_name: entry.name.clone(),
            socket_override: entry.attach_socket_path().map(PathBuf::from),
        })
    }

    pub(crate) fn finish_open(
        &mut self,
        pod_name: &str,
        result: Result<(), &dyn std::fmt::Display>,
    ) {
        match result {
            Ok(()) => {
                self.notice = Some(format!("Returned from {pod_name}."));
            }
            Err(error) => {
                self.notice = Some(format!("Open failed for {pod_name}: {error}"));
            }
        }
    }

    fn composer_is_blank(&self) -> bool {
        segments_are_blank(&self.input.submit_segments())
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
            KeyCode::Enter if self.composer_is_blank() => MultiPodAction::Open,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MultiPodSectionKind {
    Pending,
    Working,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MultiPodSection {
    kind: MultiPodSectionKind,
    entries: Vec<usize>,
}

impl MultiPodSection {
    fn hidden_count(&self) -> usize {
        self.entries
            .len()
            .saturating_sub(visible_section_len(self.kind, self.entries.len()))
    }
}

fn classify_entry(entry: &PodListEntry) -> MultiPodSectionKind {
    if entry.live.is_some() {
        if entry.actions.can_send_now {
            MultiPodSectionKind::Pending
        } else {
            MultiPodSectionKind::Working
        }
    } else {
        MultiPodSectionKind::Closed
    }
}

fn sectioned_entries(list: &PodList) -> Vec<MultiPodSection> {
    let mut pending = MultiPodSection {
        kind: MultiPodSectionKind::Pending,
        entries: Vec::new(),
    };
    let mut working = MultiPodSection {
        kind: MultiPodSectionKind::Working,
        entries: Vec::new(),
    };
    let mut closed = MultiPodSection {
        kind: MultiPodSectionKind::Closed,
        entries: Vec::new(),
    };

    for (index, entry) in list.entries.iter().enumerate() {
        match classify_entry(entry) {
            MultiPodSectionKind::Pending => pending.entries.push(index),
            MultiPodSectionKind::Working => working.entries.push(index),
            MultiPodSectionKind::Closed => closed.entries.push(index),
        }
    }

    vec![pending, working, closed]
}

fn visible_entry_indices(list: &PodList) -> Vec<usize> {
    sectioned_entries(list)
        .into_iter()
        .flat_map(|section| visible_section_indices(&section))
        .collect()
}

fn visible_section_indices(section: &MultiPodSection) -> Vec<usize> {
    section
        .entries
        .iter()
        .copied()
        .take(visible_section_len(section.kind, section.entries.len()))
        .collect()
}

fn visible_section_len(kind: MultiPodSectionKind, len: usize) -> usize {
    match kind {
        MultiPodSectionKind::Pending | MultiPodSectionKind::Working => len,
        MultiPodSectionKind::Closed => len.min(CLOSED_VISIBLE_ROWS),
    }
}

fn section_header_line(
    kind: MultiPodSectionKind,
    total: usize,
    hidden: usize,
    width: u16,
) -> Line<'static> {
    let label = match kind {
        MultiPodSectionKind::Pending => "pending",
        MultiPodSectionKind::Working => "working",
        MultiPodSectionKind::Closed => "closed",
    };
    let detail = if hidden > 0 {
        format!(" {total} total, +{hidden} hidden")
    } else {
        String::new()
    };
    let text = truncate_with_ellipsis(&format!("--{label}{detail}---"), width as usize);
    Line::from(Span::styled(
        text,
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MultiPodLayoutState {
    title: Rect,
    list: Rect,
    boundary: Rect,
    target_status: Rect,
    input: Rect,
    actionbar: Rect,
    list_draws_own_separator: bool,
}

fn multi_pod_layout(area: Rect, input_height: u16) -> MultiPodLayoutState {
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(input_height),
        Constraint::Length(1),
    ])
    .split(area);

    MultiPodLayoutState {
        title: chunks[0],
        list: chunks[1],
        boundary: chunks[2],
        target_status: chunks[3],
        input: chunks[4],
        actionbar: chunks[5],
        list_draws_own_separator: false,
    }
}

fn draw(frame: &mut Frame<'_>, app: &mut MultiPodApp) {
    let area = frame.area();
    let input_content_width = area.width.saturating_sub(2).max(1);
    let mut input_render = app.input.render(input_content_width);
    let input_height = input_area_height(&input_render, area.height);
    app.input
        .apply_cursor_viewport(&mut input_render, input_height);
    let layout = multi_pod_layout(area, input_height);

    draw_title(frame, layout.title);
    draw_list(frame, app, layout.list);
    draw_separator(frame, layout.boundary);
    draw_target_status(frame, app, layout.target_status);
    draw_input(frame, &input_render, layout.input);
    draw_actionbar(frame, app, layout.actionbar);
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
    let lines = list_lines(&app.list, area.width, area.height);
    Paragraph::new(lines).render(area, frame.buffer_mut());
}

fn list_lines(list: &PodList, width: u16, height: u16) -> Vec<Line<'static>> {
    let sections = sectioned_entries(list);
    let selected = list.selected_index();
    let live_lines = sections
        .iter()
        .filter(|section| section.kind != MultiPodSectionKind::Closed)
        .flat_map(|section| section_lines(list, section, selected, width))
        .collect::<Vec<_>>();
    let closed_lines = sections
        .iter()
        .find(|section| section.kind == MultiPodSectionKind::Closed)
        .map(|section| section_lines(list, section, selected, width))
        .unwrap_or_default();

    let available = height as usize;
    let closed_len = closed_lines.len().min(available);
    let live_len = live_lines.len().min(available.saturating_sub(closed_len));
    let spacer_len = available.saturating_sub(live_len + closed_len);

    let mut lines = Vec::with_capacity(available);
    lines.extend(live_lines.into_iter().take(live_len));
    lines.extend(std::iter::repeat_with(|| Line::from(Span::raw(""))).take(spacer_len));
    lines.extend(closed_lines.into_iter().take(closed_len));
    lines
}

fn section_lines(
    list: &PodList,
    section: &MultiPodSection,
    selected: usize,
    width: u16,
) -> Vec<Line<'static>> {
    let visible = visible_section_indices(section);
    if visible.is_empty() {
        return Vec::new();
    }

    let mut lines = Vec::with_capacity(visible.len() + 1);
    lines.push(section_header_line(
        section.kind,
        section.entries.len(),
        section.hidden_count(),
        width,
    ));
    for index in visible {
        if let Some(entry) = list.entries.get(index) {
            lines.push(row_line(entry, index == selected, width));
        }
    }
    lines
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
        let absolute_row = render.viewport_start_row as usize + i;
        let prefix = if absolute_row == 0 { "> " } else { "  " };
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
    fn multi_poll_reload_preserves_selection_composer_and_notice() {
        let mut app = test_app(vec![
            live_info_with_updated_at("alpha", PodStatus::Idle, 10),
            live_info_with_updated_at("beta", PodStatus::Idle, 20),
        ]);
        app.select_next();
        assert_eq!(app.list.selected_entry().unwrap().name, "alpha");
        app.input.insert_str("draft survives polling");
        app.notice = Some("keep this notice".to_string());
        let refreshed = PodList::from_sources(
            PodVisibilitySource::ResumePicker,
            vec![],
            vec![
                live_info_with_updated_at("gamma", PodStatus::Idle, 60),
                live_info_with_updated_at("alpha", PodStatus::Running, 50),
                live_info_with_updated_at("beta", PodStatus::Idle, 40),
            ],
            None,
            10,
        );

        app.apply_reloaded_list(refreshed);

        assert_eq!(app.list.selected_entry().unwrap().name, "alpha");
        assert_eq!(
            app.list
                .selected_entry()
                .unwrap()
                .live
                .as_ref()
                .unwrap()
                .status,
            Some(PodStatus::Running)
        );
        assert_eq!(input_text(&app), "draft survives polling");
        assert_eq!(app.notice.as_deref(), Some("keep this notice"));
    }

    #[test]
    fn multi_poll_reload_falls_back_when_selected_pod_disappears() {
        let mut app = test_app(vec![
            live_info_with_updated_at("alpha", PodStatus::Idle, 10),
            live_info_with_updated_at("beta", PodStatus::Running, 20),
        ]);
        assert_eq!(app.list.selected_entry().unwrap().name, "beta");
        let refreshed = PodList::from_sources(
            PodVisibilitySource::ResumePicker,
            vec![stopped_info_with_updated_at("closed", 30)],
            vec![live_info_with_updated_at("alpha", PodStatus::Idle, 40)],
            None,
            10,
        );

        app.apply_reloaded_list(refreshed);

        assert_eq!(app.list.selected_entry().unwrap().name, "alpha");
        assert_eq!(visible_entry_indices(&app.list), vec![0, 1]);
    }

    #[test]
    fn multi_poll_reload_error_keeps_previous_list_and_composer() {
        let mut app = test_app(vec![live_info("alpha", PodStatus::Idle)]);
        app.input.insert_str("keep draft");

        app.apply_reload_result(Err(MultiPodError::Io(io::Error::other("boom"))));

        assert_eq!(app.list.selected_entry().unwrap().name, "alpha");
        assert_eq!(input_text(&app), "keep draft");
        let notice = app.notice.as_deref().unwrap();
        assert!(notice.contains("Refresh failed"));
        assert!(notice.contains("boom"));
    }

    #[tokio::test]
    async fn multi_poll_reload_does_not_overlap_in_flight_reload() {
        let mut app = test_app(vec![live_info("alpha", PodStatus::Idle)]);
        let mut pending = PendingReload::default();

        assert!(pending.start_with_handle(tokio::spawn(async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            Err(MultiPodError::Io(io::Error::other("boom")))
        })));
        assert!(!pending.start_with_handle(tokio::spawn(async {
            Ok(PodList::from_sources(
                PodVisibilitySource::ResumePicker,
                vec![],
                vec![live_info("beta", PodStatus::Idle)],
                None,
                10,
            ))
        })));
        assert!(pending.finish_if_ready().await.is_none());

        tokio::time::sleep(Duration::from_millis(20)).await;
        let result = pending.finish_if_ready().await.unwrap();
        app.apply_reload_result(result);

        assert_eq!(app.list.selected_entry().unwrap().name, "alpha");
        assert!(app.notice.as_deref().unwrap().contains("Refresh failed"));
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
    fn multi_sections_classify_pending_working_and_closed() {
        let list = PodList::from_sources(
            PodVisibilitySource::ResumePicker,
            vec![stopped_info_with_updated_at("closed", 60)],
            vec![
                live_info_with_updated_at("idle", PodStatus::Idle, 50),
                live_info_with_updated_at("running", PodStatus::Running, 40),
                live_info_with_updated_at("paused", PodStatus::Paused, 30),
            ],
            Some("idle".to_string()),
            10,
        );

        let sections = sectioned_entries(&list);

        assert_eq!(section_names(&list, &sections[0]), vec!["idle"]);
        assert_eq!(
            section_names(&list, &sections[1]),
            vec!["running", "paused"]
        );
        assert_eq!(section_names(&list, &sections[2]), vec!["closed"]);
    }

    #[test]
    fn multi_closed_section_is_limited_to_three_visible_rows() {
        let list = closed_list(5, Some("closed-0"));
        let visible = visible_entry_indices(&list)
            .into_iter()
            .map(|index| list.entries[index].name.as_str())
            .collect::<Vec<_>>();
        let sections = sectioned_entries(&list);
        let closed = sections
            .iter()
            .find(|section| section.kind == MultiPodSectionKind::Closed)
            .unwrap();
        let lines = list_lines(&list, 80, 8)
            .into_iter()
            .map(|line| plain_line(&line))
            .collect::<Vec<_>>();

        assert_eq!(visible, vec!["closed-0", "closed-1", "closed-2"]);
        assert_eq!(closed.hidden_count(), 2);
        assert!(
            lines
                .iter()
                .any(|line| line.contains("closed 5 total, +2 hidden"))
        );
        assert!(lines.iter().any(|line| line.contains("closed-2")));
        assert!(!lines.iter().any(|line| line.contains("closed-3")));
    }

    #[test]
    fn multi_selection_follows_visible_section_order_without_hidden_closed_rows() {
        let list = PodList::from_sources(
            PodVisibilitySource::ResumePicker,
            (0..5)
                .map(|index| stopped_info_with_updated_at(&format!("closed-{index}"), 50 - index))
                .collect(),
            vec![
                live_info_with_updated_at("running", PodStatus::Running, 70),
                live_info_with_updated_at("idle", PodStatus::Idle, 60),
            ],
            Some("idle".to_string()),
            20,
        );
        let mut app = app_with_list(list);

        assert_eq!(app.list.selected_entry().unwrap().name, "idle");
        app.select_next();
        assert_eq!(app.list.selected_entry().unwrap().name, "running");
        app.select_next();
        assert_eq!(app.list.selected_entry().unwrap().name, "closed-0");
        app.select_next();
        assert_eq!(app.list.selected_entry().unwrap().name, "closed-1");
        app.select_next();
        assert_eq!(app.list.selected_entry().unwrap().name, "closed-2");
        app.select_next();
        assert_eq!(app.list.selected_entry().unwrap().name, "closed-2");
    }

    #[test]
    fn multi_list_pins_closed_section_below_live_flexible_area() {
        let list = PodList::from_sources(
            PodVisibilitySource::ResumePicker,
            (0..3)
                .map(|index| stopped_info_with_updated_at(&format!("closed-{index}"), 50 - index))
                .collect(),
            vec![
                live_info_with_updated_at("running", PodStatus::Running, 70),
                live_info_with_updated_at("idle", PodStatus::Idle, 60),
            ],
            Some("idle".to_string()),
            20,
        );
        let lines = list_lines(&list, 80, 12)
            .into_iter()
            .map(|line| plain_line(&line))
            .collect::<Vec<_>>();

        assert!(lines[0].contains("pending"));
        assert!(lines[2].contains("working"));
        assert!(lines[4].is_empty());
        assert!(lines[8].contains("closed"));
        assert!(lines[11].contains("closed-2"));
    }

    #[test]
    fn multi_layout_uses_single_boundary_separator_between_list_and_composer() {
        let layout = multi_pod_layout(Rect::new(0, 0, 80, 24), 1);

        assert_eq!(layout.boundary.height, 1);
        assert!(!layout.list_draws_own_separator);
        assert_eq!(layout.boundary.y, layout.list.y + layout.list.height);
        assert_eq!(
            layout.target_status.y,
            layout.boundary.y + layout.boundary.height
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

    #[test]
    fn multi_open_request_keeps_dashboard_state_for_nested_single_pod() {
        let mut app = test_app(vec![live_info("alpha", PodStatus::Idle)]);
        app.input.insert_str("draft survives open");

        let request = app.prepare_open().unwrap();

        assert_eq!(request.pod_name, "alpha");
        assert_eq!(
            request.socket_override,
            Some(PathBuf::from("/tmp/alpha.sock"))
        );
        assert_eq!(app.list.selected_entry().unwrap().name, "alpha");
        assert_eq!(input_text(&app), "draft survives open");
        assert!(app.notice.as_deref().unwrap().contains("Opening alpha"));
    }

    #[test]
    fn multi_open_failure_keeps_composer_and_sets_notice() {
        let mut app = test_app(vec![live_info("alpha", PodStatus::Idle)]);
        app.input.insert_str("keep this draft");
        let before = input_text(&app);
        let error = io::Error::other("boom");

        app.finish_open("alpha", Err(&error));

        assert_eq!(input_text(&app), before);
        assert_eq!(app.list.selected_entry().unwrap().name, "alpha");
        assert!(
            app.notice
                .as_deref()
                .unwrap()
                .contains("Open failed for alpha")
        );
    }

    #[test]
    fn multi_open_disabled_target_stays_in_dashboard() {
        let mut live = live_info("unreachable", PodStatus::Idle);
        live.reachable = false;
        live.status = None;
        let mut app = test_app(vec![live]);

        assert!(app.prepare_open().is_none());
        assert!(app.notice.as_deref().unwrap().contains("cannot be opened"));
    }

    #[test]
    fn multi_empty_enter_uses_open_action() {
        let mut app = test_app(vec![live_info("alpha", PodStatus::Idle)]);

        assert!(matches!(
            app.handle_key(key(KeyCode::Enter)),
            MultiPodAction::Open
        ));
        let request = app.prepare_open().unwrap();

        assert_eq!(request.pod_name, "alpha");
        assert_eq!(
            request.socket_override,
            Some(PathBuf::from("/tmp/alpha.sock"))
        );
        assert_eq!(input_text(&app), "");
        assert!(app.notice.as_deref().unwrap().contains("Opening alpha"));
    }

    #[test]
    fn multi_whitespace_only_enter_uses_open_action() {
        let mut app = test_app(vec![live_info("alpha", PodStatus::Idle)]);
        app.input.insert_str("  \n\t");

        assert!(matches!(
            app.handle_key(key(KeyCode::Enter)),
            MultiPodAction::Open
        ));
        let request = app.prepare_open().unwrap();

        assert_eq!(request.pod_name, "alpha");
        assert_eq!(input_text(&app), "  \n\t");
    }

    #[test]
    fn multi_non_empty_enter_uses_direct_send_action() {
        let mut app = test_app(vec![live_info("idle", PodStatus::Idle)]);
        app.input.insert_str("send me");

        let request = match app.handle_key(key(KeyCode::Enter)) {
            MultiPodAction::Send(request) => request,
            _ => panic!("non-empty Enter should direct-send"),
        };

        assert_eq!(request.socket_path, PathBuf::from("/tmp/idle.sock"));
        assert_eq!(Segment::flatten_to_text(&request.segments), "send me");
        assert!(app.sending);
        assert!(app.notice.as_deref().unwrap().contains("Sending to idle"));
    }

    #[test]
    fn multi_empty_enter_on_non_openable_row_matches_o_diagnostic() {
        let mut enter_app = test_app(vec![unreachable_live_info("unreachable")]);
        assert!(matches!(
            enter_app.handle_key(key(KeyCode::Enter)),
            MultiPodAction::Open
        ));
        assert!(enter_app.prepare_open().is_none());
        let enter_notice = enter_app.notice.clone();

        let mut open_app = test_app(vec![unreachable_live_info("unreachable")]);
        assert!(matches!(
            open_app.handle_key(key(KeyCode::Char('o'))),
            MultiPodAction::Open
        ));
        assert!(open_app.prepare_open().is_none());

        assert_eq!(enter_notice, open_app.notice);
    }

    fn test_app(live: Vec<LivePodInfo>) -> MultiPodApp {
        app_with_list(PodList::from_sources(
            PodVisibilitySource::ResumePicker,
            vec![],
            live,
            None,
            10,
        ))
    }

    fn app_with_list(list: PodList) -> MultiPodApp {
        let mut app = MultiPodApp {
            list,
            input: InputBuffer::new(),
            notice: None,
            sending: false,
        };
        app.ensure_selection_visible();
        app
    }

    fn closed_list(count: usize, selected: Option<&str>) -> PodList {
        PodList::from_sources(
            PodVisibilitySource::ResumePicker,
            (0..count)
                .map(|index| {
                    stopped_info_with_updated_at(&format!("closed-{index}"), 100 - index as u64)
                })
                .collect(),
            vec![],
            selected.map(str::to_string),
            count.max(1),
        )
    }

    fn live_info(pod_name: &str, status: PodStatus) -> LivePodInfo {
        live_info_with_updated_at(pod_name, status, 0)
    }

    fn unreachable_live_info(pod_name: &str) -> LivePodInfo {
        let mut live = live_info(pod_name, PodStatus::Idle);
        live.reachable = false;
        live.status = None;
        live
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
        stopped_info_with_updated_at(pod_name, 10)
    }

    fn stopped_info_with_updated_at(pod_name: &str, updated_at: u64) -> StoredPodInfo {
        StoredPodInfo {
            pod_name: pod_name.to_string(),
            metadata_state: StoredMetadataState::Present,
            active_session_id: None,
            active_segment_id: None,
            updated_at,
            preview: None,
        }
    }

    fn section_names<'a>(list: &'a PodList, section: &MultiPodSection) -> Vec<&'a str> {
        section
            .entries
            .iter()
            .map(|index| list.entries[*index].name.as_str())
            .collect()
    }

    fn plain_line(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    fn input_text(app: &MultiPodApp) -> String {
        Segment::flatten_to_text(&app.input.submit_segments())
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
}
