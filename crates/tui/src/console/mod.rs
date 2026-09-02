use std::fmt;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::Duration;

use crossterm::event::{
    self, DisableMouseCapture, Event as TermEvent, KeyCode, KeyEvent, KeyModifiers, MouseButton,
    MouseEvent, MouseEventKind,
};
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{Command, execute};
use protocol::{Event, Method, Segment, UploadedFileRef, WorkerStatus};
#[cfg(feature = "e2e-test")]
use protocol::{Greeting, RewindSummary, RewindTarget, RewindTargetId};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use standalone::{StandaloneHost, StandaloneLaunchConfig};
use tokio::sync::mpsc;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use client::transport::Socket;
use client::{BackendRuntimeTarget, Client, StandaloneWorkerResumeIntent, connect_backend_runtime};

use crate::app::{ActionbarNoticeLevel, ActionbarNoticeSource, App};
use crate::composer_keys::{ComposerEditAction, composer_edit_action};
use crate::ui;

pub(crate) type ConsoleTerminal = Terminal<CrosstermBackend<io::Stdout>>;

/// Enable SGR coordinates plus normal mouse tracking. This captures clicks,
/// releases, and wheel events without drag-capture modes (`?1002h`/`?1003h`)
/// so terminal-native drag selection remains available during startup.
#[derive(Debug, Clone, Copy)]
struct EnableSinglePodMouseCapture;

impl Command for EnableSinglePodMouseCapture {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        // 1006: SGR extended coordinates used by crossterm's parser
        // 1000: normal mouse tracking (button presses/releases and wheel)
        f.write_str("\x1B[?1006h\x1B[?1000h")
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> io::Result<()> {
        Ok(())
    }

    #[cfg(windows)]
    fn is_ansi_code_supported(&self) -> bool {
        true
    }
}

/// Enable Dashboard mouse input without drag tracking. The Dashboard only needs
/// button presses/releases and wheel events; enabling `?1002h` can make terminal
/// drag selection look captured and is intentionally avoided before startup.
#[derive(Debug, Clone, Copy)]
struct EnableDashboardMouseCapture;

impl Command for EnableDashboardMouseCapture {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        // 1006: SGR extended coordinates used by crossterm's parser
        // 1000: normal mouse tracking (button presses/releases and wheel)
        f.write_str("\x1B[?1006h\x1B[?1000h")
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> io::Result<()> {
        Ok(())
    }

    #[cfg(windows)]
    fn is_ansi_code_supported(&self) -> bool {
        true
    }
}

fn copy_to_terminal_clipboard<W: io::Write>(out: &mut W, text: &str) -> io::Result<()> {
    let encoded = BASE64_STANDARD.encode(text.as_bytes());
    write!(out, "\x1B]52;c;{}\x07", encoded)?;
    out.flush()
}

fn copy_selection_to_writer<W: io::Write>(app: &mut App, out: &mut W) -> bool {
    let Some(text) = app.selected_worker_view_mut().text_selection.copy_text() else {
        return false;
    };

    let result = copy_to_terminal_clipboard(out, &text);
    app.selected_worker_view_mut().text_selection.clear();
    match result {
        Ok(()) => {
            app.flash_actionbar_notice(
                "Copied selected text to terminal clipboard.",
                ActionbarNoticeLevel::Info,
                ActionbarNoticeSource::Tui,
                Duration::from_secs(3),
            );
        }
        Err(_) => {
            app.flash_actionbar_notice(
                "Copy failed: terminal clipboard write failed.",
                ActionbarNoticeLevel::Error,
                ActionbarNoticeSource::Tui,
                Duration::from_secs(5),
            );
        }
    }
    true
}

fn copy_selection_to_terminal(app: &mut App) -> bool {
    let mut stdout = io::stdout();
    copy_selection_to_writer(app, &mut stdout)
}

struct ConsoleConnection<T> {
    client: Client<T>,
    standalone_host: Option<StandaloneHost>,
    backend_target: Option<BackendRuntimeTarget>,
    pending_attachments: Vec<UploadedFileRef>,
}

fn attachment_media_type(path: &Path) -> Option<&'static str> {
    match path
        .extension()
        .and_then(|extension| extension.to_str())?
        .to_ascii_lowercase()
        .as_str()
    {
        "txt" | "log" | "rs" | "toml" | "yaml" | "yml" | "dcdl" | "csv" => Some("text/plain"),
        "md" => Some("text/markdown"),
        "json" => Some("application/json"),
        "pdf" => Some("application/pdf"),
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

impl<T: Socket> ConsoleConnection<T> {
    fn with_standalone_host(client: Client<T>, host: StandaloneHost) -> Self {
        Self {
            client,
            standalone_host: Some(host),
            backend_target: None,
            pending_attachments: Vec::new(),
        }
    }

    fn with_backend_target(client: Client<T>, target: BackendRuntimeTarget) -> Self {
        Self {
            client,
            standalone_host: None,
            backend_target: Some(target),
            pending_attachments: Vec::new(),
        }
    }

    fn try_next_event(&mut self) -> Result<Option<Event>, Box<dyn std::error::Error>> {
        Ok(self.client.try_next_event()?)
    }

    async fn next_event(&mut self) -> Result<Option<Event>, Box<dyn std::error::Error>> {
        Ok(self.client.next_event().await?)
    }

    async fn send(&mut self, method: &Method) -> Result<(), Box<dyn std::error::Error>> {
        let mut prepared = method.clone();
        let carries_attachments =
            matches!(prepared, Method::Run { .. }) && !self.pending_attachments.is_empty();
        if let Method::Run { input } = &mut prepared {
            input.extend(
                self.pending_attachments
                    .iter()
                    .cloned()
                    .map(|file| Segment::UploadedFile { file }),
            );
        }
        self.client.send(&prepared).await?;
        if carries_attachments {
            self.pending_attachments.clear();
        }
        Ok(())
    }

    async fn upload_path(
        &mut self,
        path: &Path,
    ) -> Result<UploadedFileRef, Box<dyn std::error::Error>> {
        let target = self.backend_target.as_ref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::Unsupported,
                "client-local file upload is available only for Backend Workers",
            )
        })?;
        let metadata = tokio::fs::metadata(path).await?;
        if !metadata.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "attachment path is not a file",
            )
            .into());
        }
        if metadata.len() > 10 * 1024 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "attachment exceeds the 10 MiB limit",
            )
            .into());
        }
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "attachment file name is not valid UTF-8",
                )
            })?;
        let media_type = attachment_media_type(path).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "attachment file type is not supported",
            )
        })?;
        let bytes = tokio::fs::read(path).await?;
        let reference = target.upload_file(file_name, media_type, bytes).await?;
        self.pending_attachments.push(reference.clone());
        Ok(reference)
    }

    async fn clear_pending_attachments(&mut self) {
        let references = std::mem::take(&mut self.pending_attachments);
        if let Some(target) = &self.backend_target {
            for reference in references {
                let _ = target.delete_uploaded_file(&reference.artifact_id).await;
            }
        }
    }

    async fn shutdown(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.clear_pending_attachments().await;
        if let Some(host) = self.standalone_host.take() {
            host.shutdown().await?;
        }
        Ok(())
    }
}

pub(crate) async fn run_standalone(
    workspace_root: PathBuf,
    state_dir: PathBuf,
    worker_name: Option<String>,
    profile: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let worker_name = worker_name.unwrap_or_else(|| "local".to_string());
    let profile = profile.map_or(manifest::ProfileSelector::Default, |profile| {
        manifest::ProfileSelector::parse_cli(&profile)
    });
    let history_root = workspace_root.clone();
    let launch = StandaloneLaunchConfig {
        state_dir,
        cwd: workspace_root,
        profile,
        worker_name: worker_name.clone(),
    }
    .resolve()
    .map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("Standalone launch configuration failed: {error}"),
        )
    })?;
    let host = StandaloneHost::start(launch)
        .await
        .map_err(|error| io::Error::other(format!("Standalone Worker startup failed: {error}")))?;
    run_standalone_host(host, worker_name, history_root).await
}

pub(crate) async fn run_standalone_restore(
    intent: StandaloneWorkerResumeIntent,
) -> Result<(), Box<dyn std::error::Error>> {
    let worker_id = intent.worker_id.parse().map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("Invalid standalone Worker ID: {error}"),
        )
    })?;
    let host = StandaloneHost::restore(intent.state_dir, worker_id)
        .await
        .map_err(|error| io::Error::other(format!("Standalone restore failed: {error}")))?;
    let worker_label = host.record().worker_name.clone();
    let history_root = host.record().cwd.canonical_path.clone();
    run_standalone_host(host, worker_label, history_root).await
}

fn standalone_console_app(worker_label: String, history_root: &Path) -> App {
    let mut app = App::new_with_persistent_input_history(worker_label, history_root);
    app.connected = true;
    app
}

async fn run_standalone_host(
    host: StandaloneHost,
    worker_label: String,
    history_root: PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let client = host.connect();
    let mut connection = ConsoleConnection::with_standalone_host(client, host);

    let mut terminal = match enter_fullscreen() {
        Ok(terminal) => terminal,
        Err(error) => {
            let _ = connection.shutdown().await;
            return Err(error);
        }
    };
    let mut app = standalone_console_app(worker_label, &history_root);
    let run_result = run_loop(&mut terminal, &mut app, &mut connection).await;
    let shutdown_result = connection
        .shutdown()
        .await
        .map_err(|error| io::Error::other(format!("Standalone Worker shutdown failed: {error}")));
    let leave_result = leave_fullscreen(&mut terminal);

    if let Err(error) = run_result {
        return Err(error);
    }
    shutdown_result?;
    leave_result?;
    Ok(())
}

pub(crate) async fn run_backend_runtime(
    target: BackendRuntimeTarget,
) -> Result<(), Box<dyn std::error::Error>> {
    let worker_label = target.display_label();
    let attachment_target = target.clone();
    let client = connect_backend_runtime(target).await?;
    let mut terminal = enter_fullscreen()?;
    let workspace_root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let mut app = App::new_with_persistent_input_history(worker_label, &workspace_root);
    app.connected = true;
    let mut connection = ConsoleConnection::with_backend_target(client, attachment_target);
    let result = run_loop(&mut terminal, &mut app, &mut connection).await;
    let _ = leave_fullscreen(&mut terminal);
    result
}

fn enter_fullscreen() -> Result<ConsoleTerminal, Box<dyn std::error::Error>> {
    let mut stdout = io::stdout();
    // Enable button-event tracking so the transcript can own drag selection;
    // avoid all-motion capture because hover-motion reports are unnecessary.
    execute!(stdout, EnterAlternateScreen, EnableSinglePodMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    Ok(Terminal::new(backend)?)
}

pub(crate) fn enter_dashboard_fullscreen() -> Result<ConsoleTerminal, Box<dyn std::error::Error>> {
    let mut stdout = io::stdout();
    // Dashboard needs clicks and wheel input only; do not capture drag motion before
    // the first visible frame.
    execute!(stdout, EnterAlternateScreen, EnableDashboardMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    Ok(Terminal::new(backend)?)
}

fn leave_fullscreen(terminal: &mut ConsoleTerminal) -> io::Result<()> {
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )
}

pub(crate) fn leave_dashboard_fullscreen(terminal: &mut ConsoleTerminal) -> io::Result<()> {
    leave_fullscreen(terminal)
}

type TerminalEventResult = io::Result<TermEvent>;

const TERMINAL_POLL_INTERVAL: Duration = Duration::from_millis(50);
const TERMINAL_EVENT_DRAIN_LIMIT: usize = 64;
const POD_EVENT_DRAIN_LIMIT: usize = 32;

struct TerminalEventReader {
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl TerminalEventReader {
    fn spawn() -> io::Result<(Self, mpsc::UnboundedReceiver<TerminalEventResult>)> {
        let (tx, rx) = mpsc::unbounded_channel();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread = thread::Builder::new()
            .name("yoi-tui-terminal-reader".to_string())
            .spawn(move || read_terminal_events(thread_stop, tx))?;

        Ok((
            Self {
                stop,
                thread: Some(thread),
            },
            rx,
        ))
    }
}

impl Drop for TerminalEventReader {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn read_terminal_events(stop: Arc<AtomicBool>, tx: mpsc::UnboundedSender<TerminalEventResult>) {
    while !stop.load(Ordering::Relaxed) {
        match event::poll(TERMINAL_POLL_INTERVAL) {
            Ok(false) => {}
            Ok(true) => {
                let event = event::read();
                let should_stop = event.is_err();
                if tx.send(event).is_err() || should_stop {
                    break;
                }
            }
            Err(e) => {
                let _ = tx.send(Err(e));
                break;
            }
        }
    }
}

#[cfg(feature = "e2e-test")]
async fn run_e2e_rewind_fixture(
    terminal: &mut ConsoleTerminal,
    worker_name: String,
) -> Result<(), Box<dyn std::error::Error>> {
    let workspace_root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let mut app = App::new_with_persistent_input_history(worker_name.clone(), &workspace_root);
    app.connected = true;
    app.handle_worker_event(Event::Snapshot {
        session: protocol::SessionSnapshot {
            entries: Vec::new(),
        },
        status: WorkerStatus::Idle,
        greeting: Greeting {
            worker_name: worker_name.clone(),
            cwd: workspace_root.display().to_string(),
            provider: "e2e-fixture".to_string(),
            model: "canned".to_string(),
            scope_summary: "isolated e2e rewind fixture".to_string(),
            tools: Vec::new(),
            context_window: 0,
            context_tokens: 0,
        },
    });

    let (_reader, mut term_rx) = TerminalEventReader::spawn()?;
    let target_id = RewindTargetId {
        segment_id: uuid::Uuid::from_u128(1),
        user_input_entry_index: 1,
    };
    let mut rewind_submit_count = 0usize;
    let mut pending_apply: Option<std::time::Instant> = None;
    let apply_delay = Duration::from_millis(400);
    #[cfg(feature = "e2e-test")]
    crate::e2e_observer::emit(
        "single_worker",
        "rewind_fixture_ready",
        serde_json::json!({ "worker": worker_name.clone() }),
    );
    terminal.draw(|frame| ui::draw(frame, &mut app))?;

    loop {
        let wait = pending_apply.map(|submitted_at| {
            apply_delay
                .checked_sub(submitted_at.elapsed())
                .unwrap_or(Duration::ZERO)
        });
        let input = match wait {
            Some(Duration::ZERO) => E2eRewindInput::Tick,
            Some(timeout) => match tokio::time::timeout(timeout, term_rx.recv()).await {
                Ok(Some(Ok(event))) => E2eRewindInput::Terminal(event),
                Ok(Some(Err(err))) => return Err(Box::new(err)),
                Ok(None) => E2eRewindInput::TerminalClosed,
                Err(_) => E2eRewindInput::Tick,
            },
            None => match term_rx.recv().await {
                Some(Ok(event)) => E2eRewindInput::Terminal(event),
                Some(Err(err)) => return Err(Box::new(err)),
                None => E2eRewindInput::TerminalClosed,
            },
        };

        let mut needs_draw = false;
        match input {
            E2eRewindInput::Terminal(TermEvent::Key(key)) => {
                let duplicate_enter_pending = matches!(key.code, KeyCode::Enter)
                    && app
                        .rewind_picker
                        .as_ref()
                        .map(|picker| picker.applying)
                        .unwrap_or(false);
                if let Some(method) = handle_key(&mut app, key) {
                    match method {
                        Method::ListRewindTargets => {
                            app.handle_worker_event(Event::RewindTargets {
                                head_entries: 3,
                                targets: vec![RewindTarget {
                                    id: target_id.clone(),
                                    expected_head_entries: 3,
                                    truncate_entries: 1,
                                    turn_index: 1,
                                    timestamp_ms: Some(1),
                                    preview: "candidate rewind target".to_string(),
                                    eligible: true,
                                    disabled_reason: None,
                                    warning: None,
                                }],
                            });
                            crate::e2e_observer::emit(
                                "single_worker",
                                "rewind_picker_opened",
                                serde_json::json!({
                                    "targets": 1,
                                    "selected_preview": "candidate rewind target",
                                }),
                            );
                        }
                        Method::RewindTo {
                            target,
                            expected_head_entries,
                        } => {
                            rewind_submit_count += 1;
                            pending_apply = Some(std::time::Instant::now());
                            crate::e2e_observer::emit(
                                "single_worker",
                                "rewind_submit_sent",
                                serde_json::json!({
                                    "segment_id": target.segment_id.to_string(),
                                    "user_input_entry_index": target.user_input_entry_index,
                                    "expected_head_entries": expected_head_entries,
                                    "submit_count": rewind_submit_count,
                                }),
                            );
                        }
                        _ => {}
                    }
                } else if duplicate_enter_pending {
                    crate::e2e_observer::emit(
                        "single_worker",
                        "rewind_duplicate_enter_suppressed",
                        serde_json::json!({ "submit_count": rewind_submit_count }),
                    );
                }
                needs_draw = true;
            }
            E2eRewindInput::Terminal(TermEvent::Mouse(_))
            | E2eRewindInput::Terminal(TermEvent::Resize(_, _))
            | E2eRewindInput::Tick => {
                needs_draw = true;
            }
            E2eRewindInput::TerminalClosed => break,
            E2eRewindInput::Terminal(_) => {}
        }

        if let Some(submitted_at) = pending_apply {
            if submitted_at.elapsed() >= apply_delay {
                app.handle_worker_event(Event::RewindApplied {
                    session: protocol::SessionSnapshot {
                        entries: Vec::new(),
                    },
                    input: vec![Segment::text("rewind-live-refresh")],
                    summary: RewindSummary {
                        truncated_to_entries: 1,
                        discarded_entries: 2,
                        tool_side_effect_warning: false,
                    },
                });
                pending_apply = None;
                let composer_text = Segment::flatten_to_text(&app.input.submit_segments());
                crate::e2e_observer::emit(
                    "single_worker",
                    "rewind_applied",
                    serde_json::json!({
                        "composer_text": composer_text,
                        "submit_count": rewind_submit_count,
                    }),
                );
                needs_draw = true;
            }
        }

        if app.quit {
            break;
        }
        if needs_draw {
            terminal.draw(|frame| ui::draw(frame, &mut app))?;
        }
    }

    Ok(())
}

#[cfg(feature = "e2e-test")]
enum E2eRewindInput {
    Terminal(TermEvent),
    TerminalClosed,
    Tick,
}

enum LoopInput<P> {
    Terminal(TerminalEventResult),
    Worker(P),
    Tick,
}

async fn next_loop_input<P, F, T>(
    term_rx: &mut mpsc::UnboundedReceiver<TerminalEventResult>,
    connected: bool,
    pod_next: F,
    animate: bool,
    animation_tick: T,
) -> LoopInput<P>
where
    F: Future<Output = P>,
    T: Future,
{
    tokio::select! {
        biased;

        term_event = term_rx.recv() => {
            LoopInput::Terminal(term_event.unwrap_or_else(|| {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "terminal event reader stopped",
                ))
            }))
        }
        event = pod_next, if connected => LoopInput::Worker(event),
        _ = animation_tick, if animate => LoopInput::Tick,
    }
}

async fn drain_terminal_events<T: Socket>(
    app: &mut App,
    client: &mut ConsoleConnection<T>,
    term_rx: &mut mpsc::UnboundedReceiver<TerminalEventResult>,
) -> Result<bool, Box<dyn std::error::Error>> {
    let mut handled = false;
    for _ in 0..TERMINAL_EVENT_DRAIN_LIMIT {
        match term_rx.try_recv() {
            Ok(event) => {
                handled = true;
                handle_terminal_event(app, client, event?).await?;
                if app.quit {
                    break;
                }
            }
            Err(mpsc::error::TryRecvError::Empty) => break,
            Err(mpsc::error::TryRecvError::Disconnected) => {
                return Err(Box::new(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "terminal event reader stopped",
                )));
            }
        }
    }
    Ok(handled)
}

async fn drain_worker_events<T: Socket>(
    app: &mut App,
    client: &mut ConsoleConnection<T>,
) -> Result<bool, Box<dyn std::error::Error>> {
    let mut handled = false;
    for _ in 0..POD_EVENT_DRAIN_LIMIT {
        match client.try_next_event()? {
            Some(ev) => {
                handled = true;
                if let Some(method) = app.handle_worker_event(ev) {
                    client.send(&method).await?;
                }
            }
            None => break,
        }
    }
    Ok(handled)
}

async fn run_loop<T: Socket>(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    client: &mut ConsoleConnection<T>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (_terminal_reader, mut term_rx) = TerminalEventReader::spawn()?;
    let mut animation_tick = tokio::time::interval(Duration::from_millis(80));
    animation_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    terminal.draw(|f| ui::draw(f, app))?;

    loop {
        if app.quit {
            break;
        }

        let handled_term_event = drain_terminal_events(app, client, &mut term_rx).await?;
        if app.quit {
            break;
        }
        let handled_worker_event = drain_worker_events(app, client).await?;
        if handled_term_event || handled_worker_event {
            terminal.draw(|f| ui::draw(f, app))?;
            continue;
        }

        match next_loop_input(
            &mut term_rx,
            app.connected,
            client.next_event(),
            app.running,
            animation_tick.tick(),
        )
        .await
        {
            LoopInput::Terminal(term_event) => {
                handle_terminal_event(app, client, term_event?).await?;
            }
            LoopInput::Worker(event) => match event? {
                Some(ev) => {
                    if let Some(method) = app.handle_worker_event(ev) {
                        client.send(&method).await?;
                    }
                }
                None => {
                    app.connected = false;
                    app.mark_orphan_compacts_incomplete();
                    app.push_error("Connection lost");
                }
            },
            LoopInput::Tick => {}
        }

        terminal.draw(|f| ui::draw(f, app))?;
    }

    Ok(())
}

fn attachment_command_path(method: &Method) -> Option<PathBuf> {
    let Method::Run { input } = method else {
        return None;
    };
    let [Segment::Text { content }] = input.as_slice() else {
        return None;
    };
    let path = content.strip_prefix("/attach ")?.trim();
    (!path.is_empty()).then(|| PathBuf::from(path))
}

fn is_clear_attachments_command(method: &Method) -> bool {
    let Method::Run { input } = method else {
        return false;
    };
    matches!(
        input.as_slice(),
        [Segment::Text { content }] if content.trim() == "/clear-attachments"
    )
}

async fn handle_terminal_event<T: Socket>(
    app: &mut App,
    client: &mut ConsoleConnection<T>,
    event: TermEvent,
) -> Result<(), Box<dyn std::error::Error>> {
    match event {
        TermEvent::Key(key) => {
            if let Some(method) = handle_key(app, key) {
                if let Some(path) = attachment_command_path(&method) {
                    match client.upload_path(&path).await {
                        Ok(reference) => app.push_notice(format!(
                            "Attached {} ({} bytes); it will be sent with the next message.",
                            reference.file_name, reference.byte_len
                        )),
                        Err(error) => {
                            app.push_error(format!("Attachment upload failed: {error}"));
                        }
                    }
                } else if is_clear_attachments_command(&method) {
                    client.clear_pending_attachments().await;
                    app.push_notice("Removed pending attachments.");
                } else {
                    client.send(&method).await?;
                }
            }
        }
        TermEvent::Mouse(mouse) => {
            handle_mouse(app, mouse);
        }
        TermEvent::Paste(s) => {
            app.insert_paste(s);
        }
        TermEvent::Resize(_, _) => {
            // No-op: next draw repaints in full.
        }
        _ => {}
    }
    Ok(())
}

/// Lines per wheel notch. Faster than Shift+↑/↓ (which is 1 line) so
/// hand-rolling through long histories isn't tedious, but slow enough
/// that a single notch doesn't blow past the section the user is
/// looking for.
const WHEEL_LINES: usize = 3;

/// Lines to advance per PageUp / PageDown when the task side pane is
/// open. Calibrated so a couple of presses moves through one entry's
/// subject + description block.
const PANE_SCROLL_LINES: usize = 5;

fn handle_mouse(app: &mut App, mouse: MouseEvent) {
    let rewind_picker_open = app.rewind_picker.is_some();
    let view = app.selected_worker_view_mut();
    match mouse.kind {
        MouseEventKind::ScrollUp => {
            view.text_selection.clear();
            view.scroll.scroll_up(WHEEL_LINES);
        }
        MouseEventKind::ScrollDown => {
            view.text_selection.clear();
            view.scroll.scroll_down(WHEEL_LINES);
        }
        MouseEventKind::Down(MouseButton::Left) if !rewind_picker_open => {
            if !view.text_selection.begin_drag(mouse.column, mouse.row) {
                view.text_selection.clear();
            }
        }
        MouseEventKind::Drag(MouseButton::Left) if !rewind_picker_open => {
            view.text_selection.update_drag(mouse.column, mouse.row);
        }
        MouseEventKind::Up(MouseButton::Left) if !rewind_picker_open => {
            view.text_selection.finish_drag(mouse.column, mouse.row);
        }
        _ => {}
    }
}

fn apply_composer_edit_action(app: &mut App, action: ComposerEditAction) -> Option<Method> {
    match action {
        ComposerEditAction::InsertChar(c) => app.insert_char(c),
        ComposerEditAction::InsertNewline => app.insert_newline(),
        ComposerEditAction::DeleteBefore => app.delete_char_before(),
        ComposerEditAction::DeleteAfter => app.delete_char_after(),
        ComposerEditAction::DeleteWordBefore => app.delete_word_before_cursor(),
        ComposerEditAction::MoveLeft => app.move_cursor_left(),
        ComposerEditAction::MoveRight => app.move_cursor_right(),
        ComposerEditAction::MoveWordLeft => app.move_cursor_word_left(),
        ComposerEditAction::MoveWordRight => app.move_cursor_word_right(),
        ComposerEditAction::MoveStart => app.move_cursor_start(),
        ComposerEditAction::MoveHome => app.move_cursor_home(),
        ComposerEditAction::MoveEnd => app.move_cursor_end(),
        ComposerEditAction::MoveUp => app.move_cursor_up(),
        ComposerEditAction::MoveDown => app.move_cursor_down(),
    }
    app.refresh_completion()
}

fn handle_key(app: &mut App, key: KeyEvent) -> Option<Method> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    // Modifier-key bindings.
    if let Some(method) = match key.code {
        KeyCode::Up if shift => {
            app.selected_worker_view_mut().scroll.scroll_up(1);
            Some(None)
        }
        KeyCode::Down if shift => {
            app.selected_worker_view_mut().scroll.scroll_down(1);
            Some(None)
        }
        KeyCode::Home if ctrl => {
            app.selected_worker_view_mut().scroll.to_top();
            Some(None)
        }
        KeyCode::End if ctrl => {
            app.selected_worker_view_mut().scroll.to_bottom();
            Some(None)
        }
        KeyCode::Char('[') if ctrl => {
            app.selected_worker_view_mut().scroll.jump_prev_turn();
            Some(None)
        }
        KeyCode::Char(']') if ctrl => {
            app.selected_worker_view_mut().scroll.jump_next_turn();
            Some(None)
        }
        KeyCode::Char('o') if ctrl => {
            app.cycle_mode();
            Some(None)
        }
        KeyCode::Char('t') if ctrl => {
            app.toggle_task_pane();
            Some(None)
        }
        KeyCode::Char(c) if c.eq_ignore_ascii_case(&'r') && ctrl => {
            Some(app.request_rewind_picker())
        }
        _ if composer_edit_action(key).is_some_and(ComposerEditAction::is_modifier_action) => {
            if app.is_command_mode()
                && matches!(
                    composer_edit_action(key),
                    Some(ComposerEditAction::InsertNewline)
                )
            {
                Some(None)
            } else {
                Some(apply_composer_edit_action(
                    app,
                    composer_edit_action(key).expect("checked above"),
                ))
            }
        }
        KeyCode::Char('u') if ctrl && app.is_command_mode() => {
            app.clear_command_input();
            Some(None)
        }
        KeyCode::Char(c)
            if c.eq_ignore_ascii_case(&'q') && alt && !ctrl && !app.is_command_mode() =>
        {
            if app.restore_next_queued_input_to_composer() {
                Some(app.refresh_completion())
            } else {
                Some(None)
            }
        }
        KeyCode::Char(c) if c.eq_ignore_ascii_case(&'c') && alt && !ctrl => {
            app.clear_queued_inputs();
            Some(None)
        }
        KeyCode::Char('c') if ctrl => Some(handle_pause_or_quit(app)),
        KeyCode::Char('x') if ctrl => Some(handle_cancel_or_shutdown(app)),
        KeyCode::Char('d') if ctrl => {
            app.quit = true;
            Some(None)
        }
        KeyCode::Enter if alt => {
            if app.is_command_mode() {
                Some(None)
            } else {
                app.insert_newline();
                Some(app.refresh_completion())
            }
        }
        _ => None,
    } {
        return method;
    }

    // Unbound Ctrl+Char keys are ignored before the text-input path so
    // holding Ctrl while typing never inserts control characters.
    if ctrl && matches!(key.code, KeyCode::Char(_)) {
        return None;
    }

    // Scroll / navigation. PageUp / PageDown defaults to history; while
    // the task pane is open it scrolls the pane instead so the user can
    // browse past entries without first closing the pane.
    match key.code {
        KeyCode::PageUp => {
            if app.task_pane_open {
                app.scroll_task_pane_up(PANE_SCROLL_LINES);
            } else {
                app.selected_worker_view_mut().scroll.page_up();
            }
            return None;
        }
        KeyCode::PageDown => {
            if app.task_pane_open {
                app.scroll_task_pane_down(PANE_SCROLL_LINES);
            } else {
                app.selected_worker_view_mut().scroll.page_down();
            }
            return None;
        }
        _ => {}
    }

    if app.is_command_mode() {
        return handle_command_key(app, key);
    }

    if app.rewind_picker.is_some() {
        match key.code {
            KeyCode::Esc => {
                app.cancel_rewind_picker();
                return None;
            }
            KeyCode::Enter => return app.submit_rewind_picker(),
            KeyCode::Up => {
                app.rewind_picker_up();
                return None;
            }
            KeyCode::Down => {
                app.rewind_picker_down();
                return None;
            }
            _ => {}
        }
    }

    // Completion popup overrides — only when there's something to
    // navigate / commit. An empty popup (request in flight) falls
    // through to the default behaviour.
    if app.completion.as_ref().is_some_and(|c| c.is_active()) {
        match key.code {
            KeyCode::Tab if !alt => {
                // Insert the selected entry as raw text and let the
                // re-triggered popup fetch fresh candidates (drill-in
                // for directories, narrow-to-exact for files).
                return app.apply_completion_text();
            }
            KeyCode::Enter if !alt => {
                // While the popup has selectable entries, Enter
                // commits the selection rather than submitting the
                // message. The selected entry wins regardless of how
                // much of its value the user has typed — Enter on a
                // popup entry is "accept this suggestion". Directory
                // entries are the exception: they fall through to
                // text insertion so the popup re-fetches children
                // for drill-in. After a successful chip we append a
                // trailing space so the user can keep writing without
                // a manual separator (the Space path already has the
                // space the user typed, so it's not needed there).
                if app.chipify_selected_completion_if_committable() {
                    app.insert_char(' ');
                    return None;
                }
                return app.apply_completion_text();
            }
            KeyCode::Up => {
                app.move_completion_up();
                return None;
            }
            KeyCode::Down => {
                app.move_completion_down();
                return None;
            }
            KeyCode::Esc => {
                app.cancel_completion();
                return None;
            }
            _ => {}
        }
    }

    if key.code == KeyCode::Tab && key.modifiers.is_empty() && app.completion.is_none() {
        app.cycle_worker_view();
        return None;
    }

    if key.modifiers.is_empty() {
        match key.code {
            KeyCode::Esc if app.selected_worker_view_mut().text_selection.clear() => return None,
            KeyCode::Char('y') if app.selected_worker_view().text_selection.has_selection() => {
                if !copy_selection_to_terminal(app) {
                    app.selected_worker_view_mut().text_selection.clear();
                    app.flash_actionbar_notice(
                        "Selection contains no copyable text.",
                        ActionbarNoticeLevel::Warn,
                        ActionbarNoticeSource::Tui,
                        Duration::from_secs(3),
                    );
                }
                return None;
            }
            _ => {}
        }
    }

    match key.code {
        KeyCode::Esc => {
            // Close the popup if it's still showing (covers the
            // request-in-flight case where `is_active()` was false).
            app.cancel_completion();
            None
        }
        KeyCode::Enter => app.submit_input(),
        _ if composer_edit_action(key).is_some() => {
            match composer_edit_action(key).expect("checked above") {
                ComposerEditAction::MoveUp => {
                    if app.can_browse_input_history_older() && app.browse_input_history_older() {
                        app.refresh_completion()
                    } else {
                        apply_composer_edit_action(app, ComposerEditAction::MoveUp)
                    }
                }
                ComposerEditAction::MoveDown => {
                    if app.can_browse_input_history_newer() && app.browse_input_history_newer() {
                        app.refresh_completion()
                    } else {
                        apply_composer_edit_action(app, ComposerEditAction::MoveDown)
                    }
                }
                ComposerEditAction::InsertChar(':') if !alt && app.input.is_empty() => {
                    app.enter_command_mode();
                    None
                }
                ComposerEditAction::InsertChar(c) => {
                    // Whitespace ends an in-flight completion token. Try the
                    // auto-confirm path first so an exact match (e.g. typed
                    // `@src/main.rs` matches the only popup entry) becomes a
                    // chip on the way out. Directories also commit here —
                    // ending with a space is an explicit "I want this dir"
                    // signal, not a drill-in.
                    if c.is_whitespace() {
                        app.chipify_completion_if_exact_match();
                    }
                    apply_composer_edit_action(app, ComposerEditAction::InsertChar(c))
                }
                action => apply_composer_edit_action(app, action),
            }
        }
        _ => None,
    }
}

fn handle_command_key(app: &mut App, key: KeyEvent) -> Option<Method> {
    match key.code {
        KeyCode::Esc => {
            app.exit_command_mode();
            None
        }
        KeyCode::Enter => app.submit_command_with_completion(),
        KeyCode::Backspace => {
            if app.command_text().is_empty() {
                app.exit_command_mode();
            } else {
                app.delete_char_before();
            }
            None
        }
        KeyCode::Delete => {
            app.delete_char_after();
            None
        }
        KeyCode::Left => {
            app.move_cursor_left();
            None
        }
        KeyCode::Right => {
            app.move_cursor_right();
            None
        }
        KeyCode::Up => {
            if app.command_completion_active() {
                app.move_command_completion_up();
            } else {
                app.move_cursor_up();
            }
            None
        }
        KeyCode::Down => {
            if app.command_completion_active() {
                app.move_command_completion_down();
            } else {
                app.move_cursor_down();
            }
            None
        }
        KeyCode::Home => {
            app.move_cursor_home();
            None
        }
        KeyCode::End => {
            app.move_cursor_end();
            None
        }
        KeyCode::Tab => {
            app.apply_command_completion();
            None
        }
        KeyCode::Char(c) => {
            if key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            {
                return None;
            }
            app.insert_char(c);
            None
        }
        _ => None,
    }
}

const CONFIRM_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Running / Paused → send `Method::Cancel` immediately.
/// Idle / Stopped → 2-tap to shut down the Worker.
fn handle_cancel_or_shutdown(app: &mut App) -> Option<Method> {
    if matches!(
        app.worker_status,
        WorkerStatus::Running | WorkerStatus::Paused
    ) {
        app.shutdown_confirm = None;
        app.clear_queued_inputs();
        return Some(Method::Cancel);
    }
    if let Some(pressed_at) = app.shutdown_confirm
        && pressed_at.elapsed() < CONFIRM_TIMEOUT
    {
        app.shutdown_confirm = None;
        return Some(Method::Shutdown);
    }
    app.shutdown_confirm = Some(std::time::Instant::now());
    app.flash_actionbar_notice(
        "Press Ctrl-X again within 3 s to shut down the Worker.",
        ActionbarNoticeLevel::Warn,
        ActionbarNoticeSource::Tui,
        CONFIRM_TIMEOUT,
    );
    None
}

/// Running → send `Method::Pause`.
/// Idle / Paused → 2-tap to quit the TUI (the Worker keeps running).
fn handle_pause_or_quit(app: &mut App) -> Option<Method> {
    if app.worker_status == WorkerStatus::Running {
        app.clear_queued_inputs();
        return Some(Method::Pause);
    }
    if let Some(t) = app.quit_confirm
        && t.elapsed() < CONFIRM_TIMEOUT
    {
        app.quit_confirm = None;
        app.quit = true;
        return None;
    }
    app.quit_confirm = Some(std::time::Instant::now());
    app.flash_actionbar_notice(
        "Press Ctrl-C again within 3 s to exit the TUI (the Worker keeps running).",
        ActionbarNoticeLevel::Warn,
        ActionbarNoticeSource::Tui,
        CONFIRM_TIMEOUT,
    );
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text_selection::{HistoryViewport, SelectionRow};
    use protocol::{Event, RewindTarget, RewindTargetId, Segment};

    #[test]
    fn standalone_console_starts_with_in_process_connection_ready() {
        let temp = tempfile::tempdir().expect("tempdir");
        let app = standalone_console_app("standalone".to_string(), temp.path());

        assert!(app.connected);
    }

    #[test]
    fn client_local_attachment_commands_are_typed_and_do_not_send_the_path() {
        let attach = Method::Run {
            input: vec![Segment::text("/attach /tmp/report.md")],
        };
        assert_eq!(
            attachment_command_path(&attach),
            Some(PathBuf::from("/tmp/report.md"))
        );
        assert!(!is_clear_attachments_command(&attach));

        let clear = Method::Run {
            input: vec![Segment::text("/clear-attachments")],
        };
        assert!(is_clear_attachments_command(&clear));
        assert_eq!(attachment_command_path(&clear), None);
        assert_eq!(
            attachment_media_type(Path::new("report.webp")),
            Some("image/webp")
        );
        assert_eq!(attachment_media_type(Path::new("program.exe")), None);
    }

    #[test]
    fn single_worker_mouse_capture_avoids_drag_and_all_motion_modes() {
        let mut ansi = String::new();
        Command::write_ansi(&EnableSinglePodMouseCapture, &mut ansi).unwrap();

        assert!(ansi.contains("?1000h"));
        assert!(!ansi.contains("?1002h"));
        assert!(ansi.contains("?1006h"));
        assert!(!ansi.contains("?1003h"));
    }

    #[test]
    fn mouse_drag_updates_selection_state() {
        let mut app = App::new("worker".into());
        app.text_selection.set_history_snapshot(
            HistoryViewport {
                x: 1,
                y: 2,
                width: 20,
                height: 3,
                top_offset: 0,
                total_lines: 1,
            },
            vec![SelectionRow::new("alpha".into(), true)],
        );

        handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 2,
                row: 2,
                modifiers: KeyModifiers::NONE,
            },
        );
        handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Drag(MouseButton::Left),
                column: 4,
                row: 2,
                modifiers: KeyModifiers::NONE,
            },
        );
        handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Up(MouseButton::Left),
                column: 4,
                row: 2,
                modifiers: KeyModifiers::NONE,
            },
        );

        assert_eq!(app.text_selection.copy_text().as_deref(), Some("lph"));
        assert!(!app.text_selection.active().unwrap().dragging);
    }

    #[test]
    fn esc_clears_selection_without_editing_composer() {
        let mut app = App::new("worker".into());
        app.text_selection.set_history_snapshot(
            HistoryViewport {
                x: 0,
                y: 0,
                width: 10,
                height: 1,
                top_offset: 0,
                total_lines: 1,
            },
            vec![SelectionRow::new("hello".into(), true)],
        );
        assert!(app.text_selection.begin_drag(0, 0));

        assert!(handle_key(&mut app, key(KeyCode::Esc)).is_none());
        assert!(!app.text_selection.has_selection());
        assert!(app.input.is_empty());
    }

    #[test]
    fn copy_selection_writes_osc52_and_clears_selection() {
        let mut app = App::new("worker".into());
        app.text_selection.set_history_snapshot(
            HistoryViewport {
                x: 0,
                y: 0,
                width: 10,
                height: 1,
                top_offset: 0,
                total_lines: 1,
            },
            vec![SelectionRow::new("hello".into(), true)],
        );
        assert!(app.text_selection.begin_drag(0, 0));
        assert!(app.text_selection.update_drag(4, 0));

        let mut out = Vec::new();
        assert!(copy_selection_to_writer(&mut app, &mut out));

        assert_eq!(String::from_utf8(out).unwrap(), "\x1B]52;c;aGVsbG8=\x07");
        assert!(!app.text_selection.has_selection());
        assert!(
            app.current_actionbar_notice(std::time::Instant::now())
                .is_some()
        );
    }

    #[tokio::test]
    async fn animation_tick_wakes_loop_while_running() {
        let (_tx, mut rx) = mpsc::unbounded_channel::<TerminalEventResult>();

        assert!(matches!(
            next_loop_input(
                &mut rx,
                true,
                std::future::pending::<Option<u8>>(),
                true,
                std::future::ready(()),
            )
            .await,
            LoopInput::Tick
        ));
    }

    #[tokio::test]
    async fn terminal_event_is_selected_before_ready_worker_event() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        tx.send(Ok(TermEvent::Key(KeyEvent::new(
            KeyCode::Char('x'),
            KeyModifiers::NONE,
        ))))
        .unwrap();

        match next_loop_input(
            &mut rx,
            true,
            std::future::ready(Some(())),
            false,
            std::future::pending::<()>(),
        )
        .await
        {
            LoopInput::Terminal(Ok(TermEvent::Key(key))) => {
                assert_eq!(key.code, KeyCode::Char('x'));
            }
            _ => panic!("ready terminal input should win over a ready Worker event"),
        }
    }

    #[tokio::test]
    async fn terminal_event_is_preserved_after_worker_event_wins() {
        let (tx, mut rx) = mpsc::unbounded_channel();

        match next_loop_input(
            &mut rx,
            true,
            std::future::ready(Some(1_u8)),
            false,
            std::future::pending::<()>(),
        )
        .await
        {
            LoopInput::Worker(Some(1)) => {}
            _ => panic!("expected the first ready Worker event to win before any terminal input"),
        }

        tx.send(Ok(TermEvent::Key(KeyEvent::new(
            KeyCode::Char('y'),
            KeyModifiers::NONE,
        ))))
        .unwrap();

        match next_loop_input(
            &mut rx,
            true,
            std::future::ready(Some(2_u8)),
            false,
            std::future::pending::<()>(),
        )
        .await
        {
            LoopInput::Terminal(Ok(TermEvent::Key(key))) => {
                assert_eq!(key.code, KeyCode::Char('y'));
            }
            _ => panic!("queued terminal input should not be lost to subsequent Worker events"),
        }
    }

    #[test]
    fn running_status_still_allows_text_editing() {
        let mut app = App::new("agent".to_string());
        app.set_worker_status(WorkerStatus::Running);

        assert!(
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)
            )
            .is_none()
        );
        assert!(
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE)
            )
            .is_none()
        );
        assert!(handle_key(&mut app, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)).is_none());
        assert!(
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE)
            )
            .is_none()
        );

        assert_eq!(
            protocol::Segment::flatten_to_text(&app.input.submit_segments()),
            "abc"
        );
    }

    #[test]
    fn running_enter_queues_instead_of_sending_run() {
        let mut app = App::new("agent".to_string());
        app.set_worker_status(WorkerStatus::Running);
        for c in "queued".chars() {
            assert!(
                handle_key(
                    &mut app,
                    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
                )
                .is_none()
            );
        }

        assert!(handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)).is_none());

        assert_eq!(app.queued_input_count(), 1);
        assert_eq!(app.next_queued_input_preview(), Some("queued"));
        assert_eq!(input_text(&app), "");
    }

    #[test]
    fn queued_input_keybindings_restore_and_clear() {
        let mut app = App::new("agent".to_string());
        app.set_worker_status(WorkerStatus::Running);
        for c in "edit queued".chars() {
            assert!(
                handle_key(
                    &mut app,
                    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
                )
                .is_none()
            );
        }
        assert!(handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)).is_none());

        assert!(
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char('q'), KeyModifiers::ALT)
            )
            .is_none()
        );
        assert_eq!(app.queued_input_count(), 0);
        assert_eq!(input_text(&app), "edit queued");

        app.input.clear();
        for c in "clear queued".chars() {
            assert!(
                handle_key(
                    &mut app,
                    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
                )
                .is_none()
            );
        }
        assert!(handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)).is_none());
        assert_eq!(app.queued_input_count(), 1);

        assert!(
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::ALT)
            )
            .is_none()
        );
        assert_eq!(app.queued_input_count(), 0);
    }

    #[test]
    fn pause_and_cancel_clear_queued_input() {
        let mut app = App::new("agent".to_string());
        app.set_worker_status(WorkerStatus::Running);
        for c in "queued".chars() {
            assert!(
                handle_key(
                    &mut app,
                    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
                )
                .is_none()
            );
        }
        assert!(handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)).is_none());
        assert_eq!(app.queued_input_count(), 1);

        let pause = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );
        assert!(matches!(pause, Some(Method::Pause)));
        assert_eq!(app.queued_input_count(), 0);

        for c in "queued again".chars() {
            assert!(
                handle_key(
                    &mut app,
                    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
                )
                .is_none()
            );
        }
        assert!(handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)).is_none());
        assert_eq!(app.queued_input_count(), 1);

        let cancel = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
        );
        assert!(matches!(cancel, Some(Method::Cancel)));
        assert_eq!(app.queued_input_count(), 0);
    }

    #[test]
    fn ctrl_x_cancels_paused_turn_without_shutdown() {
        let mut app = App::new("agent".to_string());
        app.set_worker_status(WorkerStatus::Paused);

        let cancel = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
        );
        assert!(matches!(cancel, Some(Method::Cancel)));
    }

    #[test]
    fn ctrl_x_requires_confirmation_before_shutdown_while_idle() {
        let mut app = App::new("agent".to_string());
        app.set_worker_status(WorkerStatus::Idle);
        let ctrl_x = || KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL);

        assert!(handle_key(&mut app, ctrl_x()).is_none());
        assert!(app.shutdown_confirm.is_some());
        let notice = app
            .current_actionbar_notice(std::time::Instant::now())
            .expect("first Ctrl-X should arm shutdown confirmation");
        assert_eq!(notice.level, ActionbarNoticeLevel::Warn);
        assert_eq!(notice.source, ActionbarNoticeSource::Tui);
        assert!(notice.text.contains("Ctrl-X"));
        assert!(notice.text.contains("shut down the Worker"));
        assert!(!has_alert(&app, "shut down the Worker"));

        assert!(matches!(
            handle_key(&mut app, ctrl_x()),
            Some(Method::Shutdown)
        ));
        assert!(app.shutdown_confirm.is_none());
    }

    #[test]
    fn ctrl_c_and_ctrl_x_confirmations_do_not_authorize_each_other() {
        let mut app = App::new("agent".to_string());
        app.set_worker_status(WorkerStatus::Idle);

        assert!(
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            )
            .is_none()
        );
        assert!(app.quit_confirm.is_some());
        assert!(app.shutdown_confirm.is_none());

        assert!(
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
            )
            .is_none()
        );
        assert!(!app.quit);
        assert!(app.shutdown_confirm.is_some());
    }

    #[test]
    fn word_navigation_keys_edit_composer() {
        let mut app = App::new("agent".to_string());
        for c in "foo bar".chars() {
            assert!(
                handle_key(
                    &mut app,
                    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
                )
                .is_none()
            );
        }

        assert!(
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL)
            )
            .is_none()
        );
        assert!(
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char('_'), KeyModifiers::NONE)
            )
            .is_none()
        );
        assert_eq!(input_text(&app), "foo _bar");

        assert!(handle_key(&mut app, KeyEvent::new(KeyCode::Right, KeyModifiers::ALT)).is_none());
        assert!(
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char('!'), KeyModifiers::NONE)
            )
            .is_none()
        );
        assert_eq!(input_text(&app), "foo _bar!");

        assert!(
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Backspace, KeyModifiers::CONTROL)
            )
            .is_none()
        );
        assert_eq!(input_text(&app), "foo ");
    }

    #[test]
    fn ctrl_w_deletes_word_before_cursor() {
        let mut app = App::new("agent".to_string());
        for c in "foo bar baz".chars() {
            assert!(
                handle_key(
                    &mut app,
                    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
                )
                .is_none()
            );
        }

        assert!(
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL)
            )
            .is_none()
        );
        assert_eq!(input_text(&app), "foo bar ");
    }

    #[test]
    fn word_navigation_keys_edit_command_input() {
        let mut app = App::new("agent".to_string());
        assert!(
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE)
            )
            .is_none()
        );
        for c in "peer alpha beta".chars() {
            assert!(
                handle_key(
                    &mut app,
                    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
                )
                .is_none()
            );
        }

        assert!(
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL)
            )
            .is_none()
        );
        assert!(
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char('_'), KeyModifiers::NONE)
            )
            .is_none()
        );
        assert_eq!(app.command_text(), "peer alpha _beta");

        assert!(
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL)
            )
            .is_none()
        );
        assert_eq!(app.command_text(), "peer alpha beta");
        assert_eq!(input_text(&app), "");
    }

    #[test]
    fn command_mode_enters_with_colon_and_esc_restores_composer() {
        let mut app = App::new("agent".to_string());
        app.insert_char('d');
        app.insert_char('r');
        app.insert_char('a');
        app.insert_char('f');
        app.insert_char('t');
        assert!(
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE)
            )
            .is_none()
        );
        assert!(!app.is_command_mode());
        assert_eq!(input_text(&app), "draft:");

        app.input.clear();
        assert!(
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE)
            )
            .is_none()
        );
        assert!(app.is_command_mode());
        for c in "help".chars() {
            assert!(
                handle_key(
                    &mut app,
                    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
                )
                .is_none()
            );
        }
        assert_eq!(input_text(&app), "");
        assert_eq!(app.command_text(), "help");

        assert!(handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)).is_none());
        assert!(!app.is_command_mode());
        assert_eq!(input_text(&app), "");
    }

    #[test]
    fn command_mode_empty_backspace_restores_composer() {
        let mut app = App::new("agent".to_string());
        assert!(
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE)
            )
            .is_none()
        );
        assert!(app.is_command_mode());
        assert_eq!(app.command_text(), "");

        assert!(
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)
            )
            .is_none()
        );
        assert!(!app.is_command_mode());
        assert_eq!(input_text(&app), "");
    }

    #[test]
    fn command_mode_non_empty_backspace_keeps_command_mode() {
        let mut app = App::new("agent".to_string());
        assert!(
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE)
            )
            .is_none()
        );
        assert!(
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE)
            )
            .is_none()
        );

        assert!(
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)
            )
            .is_none()
        );
        assert!(app.is_command_mode());
        assert_eq!(app.command_text(), "");
    }

    #[test]
    fn unknown_command_is_not_sent_as_user_message() {
        let mut app = App::new("agent".to_string());
        assert!(
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE)
            )
            .is_none()
        );
        for c in "does-not-exist".chars() {
            assert!(
                handle_key(
                    &mut app,
                    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
                )
                .is_none()
            );
        }

        let method = handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(method.is_none());
        assert!(app.is_command_mode());
        assert_eq!(input_text(&app), "");
        assert_eq!(app.queued_input_count(), 0);
        assert!(app.blocks.iter().any(|block| match block {
            crate::block::Block::Alert { message, .. } => message.contains("Unknown command"),
            _ => false,
        }));
    }

    #[test]
    fn command_enter_dispatches_registry_without_run() {
        let mut app = App::new("agent".to_string());
        assert!(
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE)
            )
            .is_none()
        );
        for c in "noop".chars() {
            assert!(
                handle_key(
                    &mut app,
                    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
                )
                .is_none()
            );
        }

        let method = handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(method.is_none());
        assert!(!app.is_command_mode());
        assert_eq!(input_text(&app), "");
        assert!(app.blocks.iter().any(|block| match block {
            crate::block::Block::Alert { message, .. } => message.contains("noop: no action"),
            _ => false,
        }));
    }

    #[test]
    fn compact_command_sends_compact_method_without_run() {
        let mut app = App::new("agent".to_string());
        app.connected = true;
        assert!(
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE)
            )
            .is_none()
        );
        for c in "compact".chars() {
            assert!(
                handle_key(
                    &mut app,
                    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
                )
                .is_none()
            );
        }

        let method = handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(method, Some(protocol::Method::Compact)));
        assert!(!app.is_command_mode());
        assert_eq!(input_text(&app), "");
        assert_eq!(app.queued_input_count(), 0);
        assert!(app.blocks.iter().any(|block| match block {
            crate::block::Block::Alert { message, .. } => message.contains("compact requested"),
            _ => false,
        }));
    }

    #[test]
    fn ctrl_c_quit_guard_uses_actionbar_notice_without_transcript_alert() {
        let mut app = App::new("agent".to_string());
        app.set_worker_status(WorkerStatus::Idle);

        let method = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );

        assert!(method.is_none());
        assert!(!app.quit);
        let notice = app
            .current_actionbar_notice(std::time::Instant::now())
            .expect("quit guard notice is active");
        assert!(notice.text.contains("Worker keeps running"));
        assert_eq!(notice.level, ActionbarNoticeLevel::Warn);
        assert_eq!(notice.source, ActionbarNoticeSource::Tui);
        assert!(!has_alert(&app, "Worker keeps running"));

        let method = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );
        assert!(method.is_none());
        assert!(app.quit);
    }

    #[test]
    fn ctrl_r_requests_rewind_picker_when_idle_or_paused() {
        let mut app = App::new("agent".to_string());
        app.connected = true;
        let idle = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
        );
        assert!(matches!(idle, Some(Method::ListRewindTargets)));

        app.set_worker_status(WorkerStatus::Paused);
        let paused = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
        );
        assert!(matches!(paused, Some(Method::ListRewindTargets)));
    }

    #[test]
    fn ctrl_r_is_rejected_while_running() {
        let mut app = App::new("agent".to_string());
        app.connected = true;
        app.set_worker_status(WorkerStatus::Running);

        let method = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
        );

        assert!(method.is_none());
        assert!(has_alert(&app, "cannot rewind while the Worker is running"));
    }

    #[test]
    fn rewind_picker_close_returns_to_history_view() {
        let mut app = App::new("agent".to_string());
        app.connected = true;
        app.handle_worker_event(Event::RewindTargets {
            head_entries: 1,
            targets: vec![],
        });
        assert!(app.rewind_picker.is_none());

        let method = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
        );
        assert!(matches!(method, Some(Method::ListRewindTargets)));
        app.handle_worker_event(Event::RewindTargets {
            head_entries: 1,
            targets: vec![],
        });
        assert!(app.rewind_picker.is_some());

        let method = handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert!(method.is_none());
        assert!(app.rewind_picker.is_none());
    }

    #[test]
    fn rewind_applied_reseeds_display_and_restores_composer() {
        let mut app = App::new("agent".to_string());
        app.handle_worker_event(Event::Snapshot {
            greeting: test_greeting(),
            session: protocol::SessionSnapshot { entries: vec![] },
            status: WorkerStatus::Idle,
            in_flight: Default::default(),
            internal_workers: Vec::new(),
        });
        app.handle_worker_event(Event::RewindApplied {
            session: protocol::SessionSnapshot { entries: vec![] },
            input: vec![Segment::Text {
                content: "retry this".into(),
            }],
            summary: protocol::RewindSummary {
                truncated_to_entries: 0,
                discarded_entries: 2,
                tool_side_effect_warning: true,
            },
        });

        assert_eq!(input_text(&app), "retry this");
        assert!(app.rewind_picker.is_none());
        assert!(has_alert(&app, "tool side effects"));
    }

    #[test]
    fn rewind_applied_keeps_non_empty_composer() {
        let mut app = App::new("agent".to_string());
        app.handle_worker_event(Event::Snapshot {
            greeting: test_greeting(),
            session: protocol::SessionSnapshot { entries: vec![] },
            status: WorkerStatus::Idle,
            in_flight: Default::default(),
            internal_workers: Vec::new(),
        });
        type_keys(&mut app, "draft");

        app.handle_worker_event(Event::RewindApplied {
            session: protocol::SessionSnapshot { entries: vec![] },
            input: vec![Segment::Text {
                content: "retry this".into(),
            }],
            summary: protocol::RewindSummary {
                truncated_to_entries: 0,
                discarded_entries: 2,
                tool_side_effect_warning: false,
            },
        });

        assert_eq!(input_text(&app), "draft");
        assert!(has_alert(
            &app,
            "composer not overwritten because it was not empty"
        ));
    }

    #[test]
    fn rewind_apply_rejects_non_empty_composer_and_paused_status() {
        let mut app = App::new("agent".to_string());
        app.rewind_picker = Some(crate::app::RewindPickerState::new(1, vec![rewind_target()]));
        type_keys(&mut app, "draft");
        assert!(app.submit_rewind_picker().is_none());
        assert!(has_alert(&app, "composer is not empty"));

        let mut app = App::new("agent".to_string());
        app.rewind_picker = Some(crate::app::RewindPickerState::new(1, vec![rewind_target()]));
        app.set_worker_status(WorkerStatus::Paused);
        assert!(app.submit_rewind_picker().is_none());
        assert!(has_alert(
            &app,
            "cannot apply rewind while the Worker is paused"
        ));
    }

    #[test]
    fn rewind_picker_draw_does_not_overwrite_history_scroll_state() {
        let mut app = App::new("agent".to_string());
        app.scroll.top_offset = 3;
        app.scroll.turn_starts = vec![0, 5, 9];
        app.scroll.total_lines = 42;
        app.rewind_picker = Some(crate::app::RewindPickerState::new(1, vec![rewind_target()]));
        let original_top_offset = app.scroll.top_offset;
        let original_turn_starts = app.scroll.turn_starts.clone();
        let original_total_lines = app.scroll.total_lines;

        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        app.close_rewind_picker();

        assert_eq!(app.scroll.top_offset, original_top_offset);
        assert_eq!(app.scroll.turn_starts, original_turn_starts);
        assert_eq!(app.scroll.total_lines, original_total_lines);
    }

    fn rewind_target() -> RewindTarget {
        RewindTarget {
            id: RewindTargetId {
                segment_id: uuid::Uuid::nil(),
                user_input_entry_index: 0,
            },
            expected_head_entries: 1,
            truncate_entries: 0,
            turn_index: 1,
            timestamp_ms: Some(1),
            preview: "retry this".into(),
            eligible: true,
            disabled_reason: None,
            warning: None,
        }
    }

    fn test_greeting() -> protocol::Greeting {
        protocol::Greeting {
            worker_name: "agent".into(),
            cwd: "/tmp".into(),
            provider: "test".into(),
            model: "test".into(),
            scope_summary: "".into(),
            tools: vec![],
            context_window: 0,
            context_tokens: 0,
        }
    }

    #[test]
    fn command_registry_suggestions_are_available() {
        let mut app = App::new("agent".to_string());
        assert!(
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE)
            )
            .is_none()
        );
        assert!(
            app.command_suggestions()
                .iter()
                .any(|candidate| candidate.name == "help")
        );
        assert!(
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE)
            )
            .is_none()
        );
        let suggestions = app.command_suggestions();
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].name, "noop");
    }

    #[test]
    fn command_completion_tab_applies_unambiguous_candidate() {
        let mut app = App::new("agent".to_string());
        app.handle_worker_event(Event::InternalWorker {
            worker: protocol::InternalWorkerRef {
                session_id: "child-session".into(),
                name: "subworker-hoge".into(),
                parent_session_id: Some("parent-session".into()),
                kind: protocol::InternalWorkerKind::SubWorker,
            },
            revision: 1,
            event: Box::new(Event::Status {
                status: WorkerStatus::Running,
            }),
        });
        enter_command_mode(&mut app);
        type_keys(&mut app, "no");

        assert!(handle_key(&mut app, key(KeyCode::Tab)).is_none());

        assert!(app.is_command_mode());
        assert_eq!(app.command_text(), "noop ");
        assert_eq!(app.selected_worker_view().worker_name, "agent");
        assert_eq!(input_text(&app), "");
    }

    #[test]
    fn command_completion_enter_applies_and_executes_unambiguous_candidate() {
        let mut app = App::new("agent".to_string());
        enter_command_mode(&mut app);
        type_keys(&mut app, "no");

        let method = handle_key(&mut app, key(KeyCode::Enter));

        assert!(method.is_none());
        assert!(!app.is_command_mode());
        assert_eq!(input_text(&app), "");
        assert!(has_alert(&app, "noop: no action"));
    }

    #[test]
    fn command_completion_ambiguous_candidate_requires_selection_or_more_input() {
        let mut app = App::new("agent".to_string());
        register_test_command(&mut app, "open", "open", parse_no_args, "open executed");
        register_test_command(
            &mut app,
            "options",
            "options",
            parse_no_args,
            "options executed",
        );
        enter_command_mode(&mut app);
        type_keys(&mut app, "o");

        assert!(handle_key(&mut app, key(KeyCode::Tab)).is_none());
        assert_eq!(app.command_text(), "o");
        assert!(app.is_command_mode());
        assert!(has_alert(&app, "Ambiguous command completion"));

        let before = app.blocks.len();
        let method = handle_key(&mut app, key(KeyCode::Enter));
        assert!(method.is_none());
        assert_eq!(app.command_text(), "o");
        assert!(app.is_command_mode());
        assert!(app.blocks.len() > before);
        assert!(!has_alert(&app, "open executed"));
        assert!(!has_alert(&app, "options executed"));
    }

    #[test]
    fn command_completion_selected_candidate_applies_on_enter() {
        let mut app = App::new("agent".to_string());
        register_test_command(&mut app, "open", "open", parse_no_args, "open executed");
        register_test_command(
            &mut app,
            "options",
            "options",
            parse_no_args,
            "options executed",
        );
        enter_command_mode(&mut app);
        type_keys(&mut app, "o");

        assert!(handle_key(&mut app, key(KeyCode::Down)).is_none());
        let method = handle_key(&mut app, key(KeyCode::Enter));

        assert!(method.is_none());
        assert!(!app.is_command_mode());
        assert!(has_alert(&app, "open executed"));
        assert!(!has_alert(&app, "options executed"));
    }

    #[test]
    fn command_completion_argument_required_keeps_command_mode_after_name_completion() {
        let mut app = App::new("agent".to_string());
        register_test_command(
            &mut app,
            "open",
            "open <path>",
            parse_required_arg,
            "open executed",
        );
        enter_command_mode(&mut app);
        type_keys(&mut app, "op");

        let method = handle_key(&mut app, key(KeyCode::Enter));

        assert!(method.is_none());
        assert!(app.is_command_mode());
        assert_eq!(app.command_text(), "open ");
        assert!(has_alert(&app, "Invalid arguments. Usage: open <path>"));
        assert!(!has_alert(&app, "open executed"));
        assert_eq!(input_text(&app), "");
    }

    #[test]
    fn tab_cycles_main_and_subworker_view_without_changing_composer() {
        let mut app = App::new("agent".to_string());
        type_keys(&mut app, "hello");
        app.handle_worker_event(Event::InternalWorker {
            worker: protocol::InternalWorkerRef {
                session_id: "child-session".into(),
                name: "subworker-hoge".into(),
                parent_session_id: Some("parent-session".into()),
                kind: protocol::InternalWorkerKind::SubWorker,
            },
            revision: 1,
            event: Box::new(Event::Status {
                status: WorkerStatus::Running,
            }),
        });

        assert!(handle_key(&mut app, key(KeyCode::Tab)).is_none());
        assert_eq!(app.selected_worker_view().worker_name, "subworker-hoge");
        assert_eq!(input_text(&app), "hello");

        assert!(handle_key(&mut app, key(KeyCode::Tab)).is_none());
        assert_eq!(app.selected_worker_view().worker_name, "agent");
        assert_eq!(input_text(&app), "hello");
    }

    #[test]
    fn subworker_view_does_not_redirect_parent_worker_controls() {
        let mut app = App::new("agent".to_string());
        app.set_worker_status(WorkerStatus::Idle);
        app.handle_worker_event(Event::InternalWorker {
            worker: protocol::InternalWorkerRef {
                session_id: "child-session".into(),
                name: "subworker-hoge".into(),
                parent_session_id: Some("parent-session".into()),
                kind: protocol::InternalWorkerKind::SubWorker,
            },
            revision: 1,
            event: Box::new(Event::Status {
                status: WorkerStatus::Running,
            }),
        });
        handle_key(&mut app, key(KeyCode::Tab));
        assert_eq!(app.selected_worker_view().worker_name, "subworker-hoge");

        let first = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
        );
        let second = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
        );

        assert!(first.is_none());
        assert!(matches!(second, Some(Method::Shutdown)));
        assert_eq!(app.worker_status, WorkerStatus::Idle);
    }

    #[test]
    fn active_composer_completion_takes_tab_priority_over_worker_view_cycle() {
        let mut app = App::new("agent".to_string());
        app.insert_char('@');
        app.insert_char('s');
        let _ = app.refresh_completion();
        app.completion.as_mut().unwrap().entries = vec![protocol::CompletionEntry {
            value: "src/main.rs".into(),
            is_dir: false,
        }];
        app.handle_worker_event(Event::InternalWorker {
            worker: protocol::InternalWorkerRef {
                session_id: "child-session".into(),
                name: "subworker-hoge".into(),
                parent_session_id: Some("parent-session".into()),
                kind: protocol::InternalWorkerKind::SubWorker,
            },
            revision: 1,
            event: Box::new(Event::Status {
                status: WorkerStatus::Running,
            }),
        });

        let _ = handle_key(&mut app, key(KeyCode::Tab));

        assert_eq!(app.selected_worker_view().worker_name, "agent");
        assert_eq!(input_text(&app), "@src/main.rs");
    }

    #[test]
    fn command_completion_does_not_affect_normal_composer_without_popup() {
        let mut app = App::new("agent".to_string());
        type_keys(&mut app, "hello");

        assert!(handle_key(&mut app, key(KeyCode::Tab)).is_none());

        assert!(!app.is_command_mode());
        assert_eq!(input_text(&app), "hello");
    }

    #[test]
    fn up_at_start_with_empty_history_preserves_draft_without_browsing() {
        let mut app = App::new("agent".to_string());
        type_keys(&mut app, "draft");
        app.move_cursor_start();

        assert!(handle_key(&mut app, key(KeyCode::Up)).is_none());

        assert_eq!(input_text(&app), "draft");
        assert!(!app.input_history_is_browsing());
    }

    #[test]
    fn up_from_empty_composer_recalls_history_and_down_restores_empty_draft() {
        let mut app = App::new("agent".to_string());
        type_keys(&mut app, "first");
        assert!(matches!(
            handle_key(&mut app, key(KeyCode::Enter)),
            Some(Method::Run { .. })
        ));
        type_keys(&mut app, "second");
        assert!(matches!(
            handle_key(&mut app, key(KeyCode::Enter)),
            Some(Method::Run { .. })
        ));

        assert_eq!(input_text(&app), "");
        assert!(handle_key(&mut app, key(KeyCode::Up)).is_none());
        assert_eq!(input_text(&app), "second");
        assert!(handle_key(&mut app, key(KeyCode::Up)).is_none());
        assert_eq!(input_text(&app), "first");
        assert!(handle_key(&mut app, key(KeyCode::Down)).is_none());
        assert_eq!(input_text(&app), "second");
        assert!(handle_key(&mut app, key(KeyCode::Down)).is_none());
        assert_eq!(input_text(&app), "");
    }

    #[test]
    fn up_inside_multiline_preserves_existing_cursor_up_behavior() {
        let mut app = App::new("agent".to_string());
        type_keys(&mut app, "ab\ncd");

        assert!(handle_key(&mut app, key(KeyCode::Up)).is_none());
        assert!(handle_key(&mut app, key(KeyCode::Char('X'))).is_none());

        assert_eq!(input_text(&app), "abX\ncd");
    }

    #[test]
    fn up_at_start_of_multiline_recalls_history() {
        let mut app = App::new("agent".to_string());
        type_keys(&mut app, "sent");
        assert!(matches!(
            handle_key(&mut app, key(KeyCode::Enter)),
            Some(Method::Run { .. })
        ));
        type_keys(&mut app, "draft\nbody");
        app.move_cursor_start();

        assert!(handle_key(&mut app, key(KeyCode::Up)).is_none());

        assert_eq!(input_text(&app), "sent");
    }

    fn enter_command_mode(app: &mut App) {
        assert!(handle_key(app, key(KeyCode::Char(':'))).is_none());
        assert!(app.is_command_mode());
    }

    fn type_keys(app: &mut App, text: &str) {
        for c in text.chars() {
            assert!(handle_key(app, key(KeyCode::Char(c))).is_none());
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn has_alert(app: &App, needle: &str) -> bool {
        app.blocks.iter().any(|block| match block {
            crate::block::Block::Alert { message, .. } => message.contains(needle),
            _ => false,
        })
    }

    fn register_test_command(
        app: &mut App,
        name: &'static str,
        usage: &'static str,
        argument_parser: crate::command::ArgumentParser,
        message: &'static str,
    ) {
        app.command_registry.register(crate::command::CommandSpec {
            name,
            aliases: &[],
            usage,
            description: "test command",
            argument_parser,
            can_execute: test_command_available,
            executor: test_command_executor,
        });
        TEST_COMMAND_MESSAGES.with(|messages| messages.borrow_mut().push((name, message)));
    }

    thread_local! {
        static TEST_COMMAND_MESSAGES: std::cell::RefCell<Vec<(&'static str, &'static str)>> =
            const { std::cell::RefCell::new(Vec::new()) };
    }

    fn parse_no_args(
        raw: &str,
    ) -> Result<crate::command::CommandArgs, crate::command::CommandDiagnostic> {
        Ok(crate::command::CommandArgs::parse_whitespace(raw))
    }

    fn parse_required_arg(
        raw: &str,
    ) -> Result<crate::command::CommandArgs, crate::command::CommandDiagnostic> {
        let args = crate::command::CommandArgs::parse_whitespace(raw);
        if args.argv().is_empty() {
            return Err(crate::command::CommandDiagnostic::new(
                "Invalid arguments. Usage: open <path>",
            ));
        }
        Ok(args)
    }

    fn test_command_available(
        _environment: &crate::command::CommandEnvironment,
    ) -> Result<(), crate::command::CommandDiagnostic> {
        Ok(())
    }

    fn test_command_executor(
        invocation: crate::command::CommandInvocation<'_>,
    ) -> crate::command::CommandExecution {
        let message = TEST_COMMAND_MESSAGES
            .with(|messages| {
                messages
                    .borrow()
                    .iter()
                    .find(|(name, _)| *name == invocation.command.name)
                    .map(|(_, message)| *message)
            })
            .unwrap_or("test command executed");
        crate::command::CommandExecution::notice(message)
    }

    fn input_text(app: &App) -> String {
        protocol::Segment::flatten_to_text(&app.input.submit_segments())
    }
}
