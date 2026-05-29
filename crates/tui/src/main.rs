mod app;
mod block;
mod cache;
mod command;
mod input;
mod markdown;
mod multi_pod;
mod picker;
mod pod_list;
mod scroll;
mod spawn;
mod task;
mod tool;
mod ui;

use std::future::Future;
use std::io;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::Duration;

use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event as TermEvent, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use protocol::{Method, PodStatus};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use session_store::SegmentId;
use tokio::sync::mpsc;

use client::PodClient;

use crate::app::App;
use crate::picker::PickerOutcome;
use crate::spawn::{SpawnOutcome, SpawnReady};

type FullscreenTerminal = Terminal<CrosstermBackend<io::Stdout>>;

fn resolve_socket(pod_name: &str, override_path: Option<PathBuf>) -> PathBuf {
    if let Some(p) = override_path {
        return p;
    }
    manifest::paths::pod_socket_path(pod_name).unwrap_or_else(|| {
        PathBuf::from("/tmp")
            .join("insomnia")
            .join(pod_name)
            .join("sock")
    })
}

#[derive(Debug)]
enum Mode {
    Spawn,
    /// `insomnia <name>` / `insomnia --pod <name>`: attach to a live Pod by name if
    /// possible; otherwise launch `insomnia-pod --pod <name>` so the pod process
    /// resumes from name-keyed state or creates a fresh same-name Pod.
    PodName {
        pod_name: String,
        socket_override: Option<PathBuf>,
    },
    /// `insomnia -r` / `insomnia --resume`: open the Pod picker, then attach to the
    /// selected live Pod or restore the selected stopped Pod by name.
    Resume,
    /// `insomnia --session <UUID>`: skip the picker, go straight to the
    /// resume name dialog with `id` baked in.
    ResumeWithSession(SegmentId),
    /// `insomnia --multi`: open the multi-Pod dashboard. This is intentionally
    /// separate from `-r`/`--resume`, which keeps its single-Pod picker
    /// meaning.
    Multi,
}

#[derive(Debug)]
enum ParseError {
    Conflict(&'static str),
    InvalidSession(String),
    MissingValue(&'static str),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conflict(message) => write!(f, "{message}"),
            Self::InvalidSession(s) => write!(f, "invalid --session UUID: {s}"),
            Self::MissingValue(flag) => write!(f, "{flag} requires a value"),
        }
    }
}

fn parse_args() -> Result<Mode, ParseError> {
    parse_args_from(std::env::args().skip(1))
}

fn parse_args_from<I, S>(args: I) -> Result<Mode, ParseError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let args: Vec<String> = args.into_iter().map(Into::into).collect();
    let mut resume = false;
    let mut multi = false;
    let mut session: Option<SegmentId> = None;
    let mut pod: Option<String> = None;
    let mut socket_override: Option<PathBuf> = None;
    let mut socket_seen = false;
    let mut positional: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-r" | "--resume" => {
                resume = true;
                i += 1;
            }
            "--multi" => {
                multi = true;
                i += 1;
            }
            "--session" => {
                let raw = args
                    .get(i + 1)
                    .ok_or(ParseError::MissingValue("--session"))?;
                session = Some(
                    raw.parse::<SegmentId>()
                        .map_err(|_| ParseError::InvalidSession(raw.clone()))?,
                );
                i += 2;
            }
            "--pod" => {
                let raw = args.get(i + 1).ok_or(ParseError::MissingValue("--pod"))?;
                pod = Some(raw.clone());
                i += 2;
            }
            "--socket" => {
                socket_seen = true;
                let raw = args
                    .get(i + 1)
                    .ok_or(ParseError::MissingValue("--socket"))?;
                socket_override = Some(PathBuf::from(raw));
                i += 2;
            }
            other if positional.is_none() && !other.starts_with('-') => {
                positional = Some(other.to_string());
                i += 1;
            }
            _ => {
                // Unknown flag or extra positional — keep older
                // behaviour of ignoring unknowns rather than aborting.
                i += 1;
            }
        }
    }

    if multi {
        if resume {
            return Err(ParseError::Conflict(
                "--multi and --resume are mutually exclusive",
            ));
        }
        if session.is_some() {
            return Err(ParseError::Conflict(
                "--multi and --session are mutually exclusive",
            ));
        }
        if pod.is_some() {
            return Err(ParseError::Conflict(
                "--multi and --pod are mutually exclusive",
            ));
        }
        if positional.is_some() {
            return Err(ParseError::Conflict(
                "--multi cannot be used with a positional Pod name",
            ));
        }
        if socket_seen {
            return Err(ParseError::Conflict(
                "--multi and --socket are mutually exclusive",
            ));
        }
        return Ok(Mode::Multi);
    }

    if resume && session.is_some() {
        return Err(ParseError::Conflict(
            "--resume and --session are mutually exclusive",
        ));
    }
    if pod.is_some() && session.is_some() {
        return Err(ParseError::Conflict(
            "--pod and --session are mutually exclusive",
        ));
    }
    if pod.is_some() && resume {
        return Err(ParseError::Conflict(
            "--pod and --resume are mutually exclusive",
        ));
    }

    if let Some(pod_name) = pod {
        return Ok(Mode::PodName {
            pod_name,
            socket_override,
        });
    }
    if let Some(id) = session {
        return Ok(Mode::ResumeWithSession(id));
    }
    if resume {
        return Ok(Mode::Resume);
    }
    if let Some(pod_name) = positional {
        return Ok(Mode::PodName {
            pod_name,
            socket_override,
        });
    }
    Ok(Mode::Spawn)
}

#[tokio::main]
async fn main() -> ExitCode {
    let mode = match parse_args() {
        Ok(m) => m,
        Err(e) => {
            eprintln!("insomnia: {e}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(e) = enable_raw_mode() {
        eprintln!("insomnia: failed to enter raw mode: {e}");
        return ExitCode::FAILURE;
    }
    if let Err(e) = execute!(io::stdout(), EnableBracketedPaste) {
        let _ = disable_raw_mode();
        eprintln!("insomnia: {e}");
        return ExitCode::FAILURE;
    }

    let result = match mode {
        Mode::Spawn => run_spawn(None).await,
        Mode::PodName {
            pod_name,
            socket_override,
        } => run_pod_name(pod_name, socket_override).await,
        Mode::Resume => run_resume().await,
        Mode::ResumeWithSession(id) => run_spawn(Some(id)).await,
        Mode::Multi => run_multi().await,
    };

    // Always restore the terminal first so any pending eprintln below
    // shows up cleanly in scrollback rather than inside an active
    // alternate-screen buffer.
    let mut stdout = io::stdout();
    let _ = execute!(
        stdout,
        DisableMouseCapture,
        LeaveAlternateScreen,
        DisableBracketedPaste
    );
    let _ = disable_raw_mode();
    let _ = execute!(stdout, crossterm::cursor::Show);

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            // SpawnError has already been painted into the inline
            // viewport's final frame, so it's already visible in the
            // user's scrollback — printing it again would be a noisy
            // duplicate. Other errors (pod-name failures, terminal setup
            // hiccups, etc.) need surfacing here.
            if e.downcast_ref::<spawn::SpawnError>().is_none() {
                eprintln!("insomnia: {e}");
            }
            ExitCode::FAILURE
        }
    }
}

async fn run_pod_name(
    pod_name: String,
    socket_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(client) = try_connect_live_pod(&pod_name, socket_override.clone()).await {
        let mut terminal = enter_fullscreen()?;
        run_connected_pod(&mut terminal, pod_name, client).await?;
        return Ok(());
    }

    let ready = match spawn::run_pod_name(pod_name).await? {
        SpawnOutcome::Ready(r) => r,
        SpawnOutcome::Cancelled => return Ok(()),
    };
    let mut terminal = enter_fullscreen()?;
    terminal.clear()?;
    let result = run_ready_pod(&mut terminal, ready).await;
    let _ = leave_fullscreen(&mut terminal);
    result
}

async fn run_connected_pod(
    terminal: &mut FullscreenTerminal,
    pod_name: String,
    client: PodClient,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut app = App::new(pod_name);
    app.connected = true;
    run_loop(terminal, &mut app, client).await
}

async fn run_pod_name_nested(
    terminal: &mut FullscreenTerminal,
    request: multi_pod::OpenPodRequest,
) -> Result<(), Box<dyn std::error::Error>> {
    let multi_pod::OpenPodRequest {
        pod_name,
        socket_override,
    } = request;

    if let Some(client) = try_connect_live_pod(&pod_name, socket_override).await {
        return run_connected_pod(terminal, pod_name, client).await;
    }

    let ready = spawn_pod_name_from_fullscreen(terminal, &pod_name).await?;
    run_ready_pod(terminal, ready).await
}

async fn spawn_pod_name_from_fullscreen(
    terminal: &mut FullscreenTerminal,
    pod_name: &str,
) -> Result<SpawnReady, Box<dyn std::error::Error>> {
    leave_fullscreen(terminal)?;
    let outcome = spawn::run_pod_name(pod_name.to_string()).await;
    enter_fullscreen_existing(terminal)?;
    terminal.clear()?;

    match outcome? {
        SpawnOutcome::Ready(ready) => Ok(ready),
        SpawnOutcome::Cancelled => Err(Box::new(NestedOpenCancelled)),
    }
}

async fn try_connect_live_pod(
    pod_name: &str,
    socket_override: Option<PathBuf>,
) -> Option<PodClient> {
    let preferred_socket = resolve_socket(pod_name, socket_override.clone());
    connect_live_pod(pod_name, preferred_socket, socket_override.is_none())
        .await
        .map(|(_, client)| client)
}

#[derive(Debug)]
struct NestedOpenCancelled;

impl std::fmt::Display for NestedOpenCancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Pod open was cancelled")
    }
}

impl std::error::Error for NestedOpenCancelled {}

async fn run_ready_pod(
    terminal: &mut FullscreenTerminal,
    ready: SpawnReady,
) -> Result<(), Box<dyn std::error::Error>> {
    let SpawnReady {
        pod_name,
        socket_path,
    } = ready;
    run(terminal, pod_name, &socket_path).await
}

async fn connect_live_pod(
    pod_name: &str,
    preferred_socket: PathBuf,
    allow_registry_fallback: bool,
) -> Option<(PathBuf, PodClient)> {
    if let Ok(client) = PodClient::connect(&preferred_socket).await {
        return Some((preferred_socket, client));
    }

    if !allow_registry_fallback {
        return None;
    }
    let registry_socket = picker::live_socket_for_pod(pod_name)?;
    if registry_socket == preferred_socket {
        return None;
    }
    PodClient::connect(&registry_socket)
        .await
        .ok()
        .map(|client| (registry_socket, client))
}

async fn run_resume() -> Result<(), Box<dyn std::error::Error>> {
    // Pick a Pod in its own inline viewport, dropping the viewport before
    // attaching/restoring so each phase gets fresh vertical room.
    let (pod_name, socket_override) = match picker::run().await? {
        PickerOutcome::Picked {
            pod_name,
            socket_override,
        } => (pod_name, socket_override),
        PickerOutcome::Cancelled => return Ok(()),
    };
    run_pod_name(pod_name, socket_override).await
}

async fn run_multi() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = multi_pod::load_app().await?;
    let mut terminal = enter_fullscreen()?;

    loop {
        match multi_pod::run(&mut terminal, &mut app).await? {
            multi_pod::MultiPodOutcome::Quit => {
                let _ = leave_fullscreen(&mut terminal);
                return Ok(());
            }
            multi_pod::MultiPodOutcome::Open(request) => {
                let pod_name = request.pod_name.clone();
                match run_pod_name_nested(&mut terminal, request).await {
                    Ok(()) => app.finish_open(&pod_name, Ok(())),
                    Err(error) if is_recoverable_multi_open_error(error.as_ref()) => {
                        app.finish_open(&pod_name, Err(error.as_ref()));
                    }
                    Err(error) => {
                        let _ = leave_fullscreen(&mut terminal);
                        return Err(error);
                    }
                }
                app.reload().await?;
            }
        }
    }
}

fn is_recoverable_multi_open_error(error: &(dyn std::error::Error + 'static)) -> bool {
    error.is::<spawn::SpawnError>() || error.is::<NestedOpenCancelled>()
}

async fn run_spawn(resume_from: Option<SegmentId>) -> Result<(), Box<dyn std::error::Error>> {
    let ready = match spawn::run(resume_from).await? {
        SpawnOutcome::Ready(r) => r,
        SpawnOutcome::Cancelled => return Ok(()),
    };

    let SpawnReady {
        pod_name,
        socket_path,
    } = ready;

    let mut terminal = enter_fullscreen()?;
    let result = run(&mut terminal, pod_name, &socket_path).await;

    // Leave alt-screen explicitly before `main`'s terminal restore path.
    let _ = execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    );

    result
}

fn enter_fullscreen() -> Result<FullscreenTerminal, Box<dyn std::error::Error>> {
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    Ok(Terminal::new(backend)?)
}

fn enter_fullscreen_existing(
    terminal: &mut FullscreenTerminal,
) -> Result<(), Box<dyn std::error::Error>> {
    execute!(
        terminal.backend_mut(),
        EnterAlternateScreen,
        EnableMouseCapture
    )?;
    Ok(())
}

fn leave_fullscreen(terminal: &mut FullscreenTerminal) -> io::Result<()> {
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )
}

async fn run(
    terminal: &mut FullscreenTerminal,
    pod_name: String,
    socket_path: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut app = App::new(pod_name);

    match PodClient::connect(socket_path).await {
        Ok(client) => {
            app.connected = true;
            // The Pod sends `Event::Snapshot` automatically on connect;
            // no explicit method call is required to fetch history.
            run_loop(terminal, &mut app, client).await?;
        }
        Err(e) => {
            app.push_error(format!(
                "Failed to connect to {}: {e}",
                socket_path.display()
            ));
            terminal.draw(|f| ui::draw(f, &mut app))?;
            run_disconnected(&mut app)?;
        }
    }
    Ok(())
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
            .name("insomnia-tui-terminal-reader".to_string())
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

enum LoopInput<P> {
    Terminal(TerminalEventResult),
    Pod(Option<P>),
}

async fn next_loop_input<P, F>(
    term_rx: &mut mpsc::UnboundedReceiver<TerminalEventResult>,
    connected: bool,
    pod_next: F,
) -> LoopInput<P>
where
    F: Future<Output = Option<P>>,
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
        event = pod_next, if connected => LoopInput::Pod(event),
    }
}

async fn drain_terminal_events(
    app: &mut App,
    client: &mut PodClient,
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

async fn drain_pod_events(
    app: &mut App,
    client: &mut PodClient,
) -> Result<bool, Box<dyn std::error::Error>> {
    let mut handled = false;
    for _ in 0..POD_EVENT_DRAIN_LIMIT {
        match client.try_next_event() {
            Some(ev) => {
                handled = true;
                if let Some(method) = app.handle_pod_event(ev) {
                    client.send(&method).await?;
                }
            }
            None => break,
        }
    }
    Ok(handled)
}

async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    mut client: PodClient,
) -> Result<(), Box<dyn std::error::Error>> {
    let (_terminal_reader, mut term_rx) = TerminalEventReader::spawn()?;

    terminal.draw(|f| ui::draw(f, app))?;

    loop {
        if app.quit {
            break;
        }

        let handled_term_event = drain_terminal_events(app, &mut client, &mut term_rx).await?;
        if app.quit {
            break;
        }
        let handled_pod_event = drain_pod_events(app, &mut client).await?;
        if handled_term_event || handled_pod_event {
            terminal.draw(|f| ui::draw(f, app))?;
            continue;
        }

        match next_loop_input(&mut term_rx, app.connected, client.next_event()).await {
            LoopInput::Terminal(term_event) => {
                handle_terminal_event(app, &mut client, term_event?).await?;
            }
            LoopInput::Pod(event) => match event {
                Some(ev) => {
                    if let Some(method) = app.handle_pod_event(ev) {
                        client.send(&method).await?;
                    }
                }
                None => {
                    app.connected = false;
                    app.mark_orphan_compacts_incomplete();
                    app.push_error("Connection lost");
                }
            },
        }

        terminal.draw(|f| ui::draw(f, app))?;
    }

    Ok(())
}

async fn handle_terminal_event(
    app: &mut App,
    client: &mut PodClient,
    event: TermEvent,
) -> Result<(), Box<dyn std::error::Error>> {
    match event {
        TermEvent::Key(key) => {
            if let Some(method) = handle_key(app, key) {
                client.send(&method).await?;
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

fn run_disconnected(_app: &mut App) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        if event::poll(std::time::Duration::from_millis(100))?
            && let TermEvent::Key(key) = event::read()?
            && let KeyCode::Char('c') = key.code
            && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            break;
        }
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
    match mouse.kind {
        MouseEventKind::ScrollUp => app.scroll.scroll_up(WHEEL_LINES),
        MouseEventKind::ScrollDown => app.scroll.scroll_down(WHEEL_LINES),
        _ => {}
    }
}

fn handle_key(app: &mut App, key: KeyEvent) -> Option<Method> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    // Modifier-key bindings.
    if let Some(method) = match key.code {
        KeyCode::Up if shift => {
            app.scroll.scroll_up(1);
            Some(None)
        }
        KeyCode::Down if shift => {
            app.scroll.scroll_down(1);
            Some(None)
        }
        KeyCode::Home if ctrl => {
            app.scroll.to_top();
            Some(None)
        }
        KeyCode::End if ctrl => {
            app.scroll.to_bottom();
            Some(None)
        }
        KeyCode::Char('[') if ctrl => {
            app.scroll.jump_prev_turn();
            Some(None)
        }
        KeyCode::Char(']') if ctrl => {
            app.scroll.jump_next_turn();
            Some(None)
        }
        KeyCode::Char('o') if ctrl => {
            app.mode = app.mode.cycle();
            Some(None)
        }
        KeyCode::Char('t') if ctrl => {
            app.toggle_task_pane();
            Some(None)
        }
        KeyCode::Char('a') if ctrl => {
            app.move_cursor_start();
            Some(app.refresh_completion())
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
        KeyCode::Char('x') if ctrl => Some(match app.pod_status {
            PodStatus::Running => {
                app.clear_queued_inputs();
                Some(Method::Cancel)
            }
            PodStatus::Paused | PodStatus::Idle => Some(Method::Shutdown),
        }),
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
                app.scroll.page_up();
            }
            return None;
        }
        KeyCode::PageDown => {
            if app.task_pane_open {
                app.scroll_task_pane_down(PANE_SCROLL_LINES);
            } else {
                app.scroll.page_down();
            }
            return None;
        }
        _ => {}
    }

    if app.is_command_mode() {
        return handle_command_key(app, key);
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

    match key.code {
        KeyCode::Esc => {
            // Close the popup if it's still showing (covers the
            // request-in-flight case where `is_active()` was false).
            app.cancel_completion();
            None
        }
        KeyCode::Enter => app.submit_input(),
        KeyCode::Backspace => {
            app.delete_char_before();
            app.refresh_completion()
        }
        KeyCode::Delete => {
            app.delete_char_after();
            app.refresh_completion()
        }
        KeyCode::Left => {
            app.move_cursor_left();
            app.refresh_completion()
        }
        KeyCode::Right => {
            app.move_cursor_right();
            app.refresh_completion()
        }
        KeyCode::Up => {
            app.move_cursor_up();
            app.refresh_completion()
        }
        KeyCode::Down => {
            app.move_cursor_down();
            app.refresh_completion()
        }
        KeyCode::Home => {
            app.move_cursor_home();
            app.refresh_completion()
        }
        KeyCode::End => {
            app.move_cursor_end();
            app.refresh_completion()
        }
        KeyCode::Char(':') if !alt && app.input.is_empty() => {
            app.enter_command_mode();
            None
        }
        KeyCode::Char(c) => {
            // Whitespace ends an in-flight completion token. Try the
            // auto-confirm path first so an exact match (e.g. typed
            // `@src/main.rs` matches the only popup entry) becomes a
            // chip on the way out. Directories also commit here —
            // ending with a space is an explicit "I want this dir"
            // signal, not a drill-in.
            if c.is_whitespace() {
                app.chipify_completion_if_exact_match();
            }
            app.insert_char(c);
            app.refresh_completion()
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

/// Running → send `Method::Pause`.
/// Idle / Paused → 2-tap to quit the TUI (the Pod keeps running).
fn handle_pause_or_quit(app: &mut App) -> Option<Method> {
    if app.pod_status == PodStatus::Running {
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
    app.push_error("Press Ctrl-C again within 3 s to exit the TUI (the Pod keeps running).");
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_pod_name_mode() {
        match parse_args_from(["--pod", "agent", "--socket", "/tmp/agent.sock"]).unwrap() {
            Mode::PodName {
                pod_name,
                socket_override,
            } => {
                assert_eq!(pod_name, "agent");
                assert_eq!(socket_override, Some(PathBuf::from("/tmp/agent.sock")));
            }
            _ => panic!("expected PodName mode"),
        }
    }

    #[test]
    fn parse_positional_name_uses_pod_name_mode() {
        match parse_args_from(["agent"]).unwrap() {
            Mode::PodName {
                pod_name,
                socket_override,
            } => {
                assert_eq!(pod_name, "agent");
                assert_eq!(socket_override, None);
            }
            _ => panic!("expected PodName mode"),
        }
    }

    #[test]
    fn parse_rejects_pod_and_session() {
        let segment_id = session_store::new_segment_id().to_string();
        let err = parse_args_from(["--pod", "agent", "--session", &segment_id]).unwrap_err();
        assert_eq!(
            err.to_string(),
            "--pod and --session are mutually exclusive"
        );
    }

    #[test]
    fn parse_multi_mode() {
        match parse_args_from(["--multi"]).unwrap() {
            Mode::Multi => {}
            _ => panic!("expected Multi mode"),
        }
    }

    #[test]
    fn parse_multi_conflicts_are_clear() {
        let segment_id = session_store::new_segment_id().to_string();
        let cases = [
            (
                vec!["--multi".to_string(), "--resume".to_string()],
                "--multi and --resume are mutually exclusive",
            ),
            (
                vec!["--multi".to_string(), "--session".to_string(), segment_id],
                "--multi and --session are mutually exclusive",
            ),
            (
                vec![
                    "--multi".to_string(),
                    "--pod".to_string(),
                    "agent".to_string(),
                ],
                "--multi and --pod are mutually exclusive",
            ),
            (
                vec!["--multi".to_string(), "agent".to_string()],
                "--multi cannot be used with a positional Pod name",
            ),
            (
                vec![
                    "--multi".to_string(),
                    "--socket".to_string(),
                    "/tmp/a.sock".to_string(),
                ],
                "--multi and --socket are mutually exclusive",
            ),
        ];

        for (args, message) in cases {
            let err = parse_args_from(args).unwrap_err();
            assert_eq!(err.to_string(), message);
        }
    }

    #[tokio::test]
    async fn terminal_event_is_selected_before_ready_pod_event() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        tx.send(Ok(TermEvent::Key(KeyEvent::new(
            KeyCode::Char('x'),
            KeyModifiers::NONE,
        ))))
        .unwrap();

        match next_loop_input(&mut rx, true, std::future::ready(Some(()))).await {
            LoopInput::Terminal(Ok(TermEvent::Key(key))) => {
                assert_eq!(key.code, KeyCode::Char('x'));
            }
            _ => panic!("ready terminal input should win over a ready Pod event"),
        }
    }

    #[tokio::test]
    async fn terminal_event_is_preserved_after_pod_event_wins() {
        let (tx, mut rx) = mpsc::unbounded_channel();

        match next_loop_input(&mut rx, true, std::future::ready(Some(1_u8))).await {
            LoopInput::Pod(Some(1)) => {}
            _ => panic!("expected the first ready Pod event to win before any terminal input"),
        }

        tx.send(Ok(TermEvent::Key(KeyEvent::new(
            KeyCode::Char('y'),
            KeyModifiers::NONE,
        ))))
        .unwrap();

        match next_loop_input(&mut rx, true, std::future::ready(Some(2_u8))).await {
            LoopInput::Terminal(Ok(TermEvent::Key(key))) => {
                assert_eq!(key.code, KeyCode::Char('y'));
            }
            _ => panic!("queued terminal input should not be lost to subsequent Pod events"),
        }
    }

    #[test]
    fn running_status_still_allows_text_editing() {
        let mut app = App::new("agent".to_string());
        app.set_pod_status(PodStatus::Running);

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
        app.set_pod_status(PodStatus::Running);
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
        app.set_pod_status(PodStatus::Running);
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
        app.set_pod_status(PodStatus::Running);
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
        enter_command_mode(&mut app);
        type_keys(&mut app, "no");

        assert!(handle_key(&mut app, key(KeyCode::Tab)).is_none());

        assert!(app.is_command_mode());
        assert_eq!(app.command_text(), "noop ");
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
    fn command_completion_does_not_affect_normal_composer_without_popup() {
        let mut app = App::new("agent".to_string());
        type_keys(&mut app, "hello");

        assert!(handle_key(&mut app, key(KeyCode::Tab)).is_none());

        assert!(!app.is_command_mode());
        assert_eq!(input_text(&app), "hello");
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
