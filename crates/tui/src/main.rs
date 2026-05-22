mod app;
mod block;
mod cache;
mod input;
mod markdown;
mod picker;
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
    Attach {
        pod_name: String,
        socket_override: Option<PathBuf>,
    },
    /// `tui --pod <name>`: attach to a live Pod by name if possible;
    /// otherwise launch `pod --pod <name>` so the pod process resumes from
    /// name-keyed state or creates a fresh same-name Pod.
    PodName {
        pod_name: String,
        socket_override: Option<PathBuf>,
    },
    /// `tui -r` / `tui --resume`: open the session picker first, then
    /// run the same name dialog as Spawn but in resume mode.
    Resume,
    /// `tui --session <UUID>`: skip the picker, go straight to the
    /// resume name dialog with `id` baked in.
    ResumeWithSession(SegmentId),
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
    let mut session: Option<SegmentId> = None;
    let mut pod: Option<String> = None;
    let mut socket_override: Option<PathBuf> = None;
    let mut positional: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-r" | "--resume" => {
                resume = true;
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
        return Ok(Mode::Attach {
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
            eprintln!("tui: {e}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(e) = enable_raw_mode() {
        eprintln!("tui: failed to enter raw mode: {e}");
        return ExitCode::FAILURE;
    }
    if let Err(e) = execute!(io::stdout(), EnableBracketedPaste) {
        let _ = disable_raw_mode();
        eprintln!("tui: {e}");
        return ExitCode::FAILURE;
    }

    let result = match mode {
        Mode::Spawn => run_spawn(None).await,
        Mode::Attach {
            pod_name,
            socket_override,
        } => run_attach(pod_name, socket_override).await,
        Mode::PodName {
            pod_name,
            socket_override,
        } => run_pod_name(pod_name, socket_override).await,
        Mode::Resume => run_resume().await,
        Mode::ResumeWithSession(id) => run_spawn(Some(id)).await,
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
            // duplicate. Other errors (attach-mode failures, terminal
            // setup hiccups, etc.) need surfacing here.
            if e.downcast_ref::<spawn::SpawnError>().is_none() {
                eprintln!("tui: {e}");
            }
            ExitCode::FAILURE
        }
    }
}

async fn run_attach(
    pod_name: String,
    socket_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let socket_path = resolve_socket(&pod_name, socket_override);
    let mut terminal = enter_fullscreen()?;
    run(&mut terminal, pod_name, &socket_path).await
}

async fn run_pod_name(
    pod_name: String,
    socket_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let socket_path = resolve_socket(&pod_name, socket_override);
    if let Ok(client) = PodClient::connect(&socket_path).await {
        let mut terminal = enter_fullscreen()?;
        let mut app = App::new(pod_name);
        app.connected = true;
        return run_loop(&mut terminal, &mut app, client).await;
    }

    let ready = match spawn::run_pod_name(pod_name).await? {
        SpawnOutcome::Ready(r) => r,
        SpawnOutcome::Cancelled => return Ok(()),
    };
    let SpawnReady {
        pod_name,
        socket_path,
    } = ready;

    let mut terminal = enter_fullscreen()?;
    let result = run(&mut terminal, pod_name, &socket_path).await;
    let _ = execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    );
    result
}

async fn run_resume() -> Result<(), Box<dyn std::error::Error>> {
    // Phase 1: pick a session in its own inline viewport, dropping the
    // viewport before the name dialog opens so each phase gets fresh
    // vertical room.
    let leaf_segment_id = match picker::run().await? {
        PickerOutcome::Picked { segment_id } => segment_id,
        PickerOutcome::Cancelled => return Ok(()),
    };
    run_spawn(Some(leaf_segment_id)).await
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

fn enter_fullscreen() -> Result<Terminal<CrosstermBackend<io::Stdout>>, Box<dyn std::error::Error>>
{
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    Ok(Terminal::new(backend)?)
}

async fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
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
    _thread: thread::JoinHandle<()>,
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
                _thread: thread,
            },
            rx,
        ))
    }
}

impl Drop for TerminalEventReader {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
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

fn drain_pod_events(app: &mut App, client: &mut PodClient) -> bool {
    let mut handled = false;
    for _ in 0..POD_EVENT_DRAIN_LIMIT {
        match client.try_next_event() {
            Some(ev) => {
                handled = true;
                app.handle_pod_event(ev);
            }
            None => break,
        }
    }
    handled
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
        let handled_pod_event = drain_pod_events(app, &mut client);
        if handled_term_event || handled_pod_event {
            terminal.draw(|f| ui::draw(f, app))?;
            continue;
        }

        match next_loop_input(&mut term_rx, app.connected, client.next_event()).await {
            LoopInput::Terminal(term_event) => {
                handle_terminal_event(app, &mut client, term_event?).await?;
            }
            LoopInput::Pod(event) => match event {
                Some(ev) => app.handle_pod_event(ev),
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
        KeyCode::Char('c') if ctrl => Some(handle_pause_or_quit(app)),
        KeyCode::Char('x') if ctrl => Some(match app.pod_status {
            PodStatus::Running => Some(Method::Cancel),
            PodStatus::Paused | PodStatus::Idle => Some(Method::Shutdown),
        }),
        KeyCode::Char('d') if ctrl => {
            app.quit = true;
            Some(None)
        }
        KeyCode::Enter if alt => {
            app.insert_newline();
            Some(app.refresh_completion())
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

const CONFIRM_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Running → send `Method::Pause`.
/// Idle / Paused → 2-tap to quit the TUI (the Pod keeps running).
fn handle_pause_or_quit(app: &mut App) -> Option<Method> {
    if app.pod_status == PodStatus::Running {
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
    fn parse_rejects_pod_and_session() {
        let segment_id = session_store::new_segment_id().to_string();
        let err = parse_args_from(["--pod", "agent", "--session", &segment_id]).unwrap_err();
        assert_eq!(
            err.to_string(),
            "--pod and --session are mutually exclusive"
        );
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
}
