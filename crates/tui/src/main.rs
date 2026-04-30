mod app;
mod block;
mod cache;
mod client;
mod input;
mod picker;
mod scroll;
mod spawn;
mod tool;
mod ui;

use std::io;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event as TermEvent, KeyCode, KeyEvent,
    KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use protocol::Method;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use session_store::SessionId;

use crate::app::App;
use crate::client::PodClient;
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

enum Mode {
    Spawn,
    Attach {
        pod_name: String,
        socket_override: Option<PathBuf>,
    },
    /// `tui -r` / `tui --resume`: open the session picker first, then
    /// run the same name dialog as Spawn but in resume mode.
    Resume,
    /// `tui --session <UUID>`: skip the picker, go straight to the
    /// resume name dialog with `id` baked in.
    ResumeWithSession(SessionId),
}

enum ParseError {
    Conflict,
    InvalidSession(String),
    MissingValue(&'static str),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conflict => write!(f, "--resume and --session are mutually exclusive"),
            Self::InvalidSession(s) => write!(f, "invalid --session UUID: {s}"),
            Self::MissingValue(flag) => write!(f, "{flag} requires a value"),
        }
    }
}

fn parse_args() -> Result<Mode, ParseError> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut resume = false;
    let mut session: Option<SessionId> = None;
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
                    raw.parse::<SessionId>()
                        .map_err(|_| ParseError::InvalidSession(raw.clone()))?,
                );
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
        return Err(ParseError::Conflict);
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
        Mode::Resume => run_resume().await,
        Mode::ResumeWithSession(id) => run_spawn(Some(id)).await,
    };

    // Always restore the terminal first so any pending eprintln below
    // shows up cleanly in scrollback rather than inside an active
    // alternate-screen buffer.
    let mut stdout = io::stdout();
    let _ = execute!(stdout, LeaveAlternateScreen, DisableBracketedPaste);
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
    run(&mut terminal, pod_name, &socket_path, false).await
}

async fn run_resume() -> Result<(), Box<dyn std::error::Error>> {
    // Phase 1: pick a session in its own inline viewport, dropping the
    // viewport before the name dialog opens so each phase gets fresh
    // vertical room.
    let id = match picker::run().await? {
        PickerOutcome::Picked(id) => id,
        PickerOutcome::Cancelled => return Ok(()),
    };
    run_spawn(Some(id)).await
}

async fn run_spawn(resume_from: Option<SessionId>) -> Result<(), Box<dyn std::error::Error>> {
    let ready = match spawn::run(resume_from).await? {
        SpawnOutcome::Ready(r) => r,
        SpawnOutcome::Cancelled => return Ok(()),
    };

    let SpawnReady {
        pod_name,
        socket_path,
        mut child,
        stderr_drain,
    } = ready;

    let mut terminal = enter_fullscreen()?;
    let result = run(&mut terminal, pod_name, &socket_path, true).await;

    // Leave alt-screen before reaping the child so any final pod stderr
    // (drained off-line by `stderr_drain`) cannot collide with the
    // restored scrollback.
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);

    match tokio::time::timeout(Duration::from_secs(3), child.wait()).await {
        Ok(Ok(_)) => {}
        _ => {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
    }
    stderr_drain.abort();

    result
}

fn enter_fullscreen() -> Result<Terminal<CrosstermBackend<io::Stdout>>, Box<dyn std::error::Error>>
{
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    Ok(Terminal::new(backend)?)
}

async fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    pod_name: String,
    socket_path: &std::path::Path,
    shutdown_pod_on_exit: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut app = App::new(pod_name);

    match PodClient::connect(socket_path).await {
        Ok(mut client) => {
            app.connected = true;
            let _ = client.send(&Method::GetHistory).await;
            run_loop(terminal, &mut app, client, shutdown_pod_on_exit).await?;
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

async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    mut client: PodClient,
    shutdown_pod_on_exit: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    terminal.draw(|f| ui::draw(f, app))?;

    loop {
        if app.quit {
            if shutdown_pod_on_exit {
                let _ = client.send(&Method::Shutdown).await;
            }
            break;
        }

        tokio::select! {
            _ = tokio::task::spawn_blocking(|| event::poll(std::time::Duration::from_millis(50))) => {
                while event::poll(std::time::Duration::ZERO)? {
                    match event::read()? {
                        TermEvent::Key(key) => {
                            if let Some(method) = handle_key(app, key) {
                                client.send(&method).await?;
                            }
                        }
                        TermEvent::Paste(s) => {
                            app.insert_paste(s);
                        }
                        TermEvent::Resize(_, _) => {
                            // No-op: next draw repaints in full.
                        }
                        _ => {}
                    }
                    if app.quit {
                        break;
                    }
                }
            }
            event = client.next_event(), if app.connected => {
                match event {
                    Some(ev) => app.handle_pod_event(ev),
                    None => {
                        app.connected = false;
                        app.push_error("Connection lost");
                    }
                }
            }
        }

        terminal.draw(|f| ui::draw(f, app))?;
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

fn handle_key(app: &mut App, key: KeyEvent) -> Option<Method> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    // Scroll / navigation (history view).
    match key.code {
        KeyCode::Up if shift => {
            app.scroll.scroll_up(1);
            return None;
        }
        KeyCode::Down if shift => {
            app.scroll.scroll_down(1);
            return None;
        }
        KeyCode::PageUp => {
            app.scroll.page_up();
            return None;
        }
        KeyCode::PageDown => {
            app.scroll.page_down();
            return None;
        }
        KeyCode::Home if ctrl => {
            app.scroll.to_top();
            return None;
        }
        KeyCode::End if ctrl => {
            app.scroll.to_bottom();
            return None;
        }
        KeyCode::Char('[') if ctrl => {
            app.scroll.jump_prev_turn();
            return None;
        }
        KeyCode::Char(']') if ctrl => {
            app.scroll.jump_next_turn();
            return None;
        }
        KeyCode::Char('o') if ctrl => {
            app.mode = app.mode.cycle();
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
        KeyCode::Char('c') if ctrl => handle_pause_or_quit(app),
        KeyCode::Char('x') if ctrl => {
            if app.running {
                Some(Method::Cancel)
            } else {
                app.push_error("Nothing to cancel (Pod is not running).");
                None
            }
        }
        KeyCode::Char('d') if ctrl => handle_shutdown(app),
        KeyCode::Esc => {
            // Close the popup if it's still showing (covers the
            // request-in-flight case where `is_active()` was false).
            app.cancel_completion();
            None
        }
        KeyCode::Enter if alt => {
            app.insert_newline();
            app.refresh_completion()
        }
        KeyCode::Enter => app.submit_input(),
        KeyCode::Backspace if ctrl => {
            app.delete_word_before();
            app.refresh_completion()
        }
        KeyCode::Backspace => {
            app.delete_char_before();
            app.refresh_completion()
        }
        KeyCode::Delete => {
            app.delete_char_after();
            app.refresh_completion()
        }
        KeyCode::Left if ctrl => {
            app.move_cursor_word_left();
            app.refresh_completion()
        }
        KeyCode::Left => {
            app.move_cursor_left();
            app.refresh_completion()
        }
        KeyCode::Right if ctrl => {
            app.move_cursor_word_right();
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

fn handle_shutdown(app: &mut App) -> Option<Method> {
    if !app.running {
        return Some(Method::Shutdown);
    }
    if let Some(t) = app.shutdown_confirm
        && t.elapsed() < CONFIRM_TIMEOUT
    {
        app.shutdown_confirm = None;
        return Some(Method::Shutdown);
    }
    app.shutdown_confirm = Some(std::time::Instant::now());
    app.push_error("Turn is running. Press Ctrl-D again to cancel and shut down.");
    None
}

/// Running → send `Method::Pause`.
/// Idle / Paused → 2-tap to quit the TUI (the Pod keeps running).
fn handle_pause_or_quit(app: &mut App) -> Option<Method> {
    if app.running {
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
