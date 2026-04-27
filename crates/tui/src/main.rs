mod app;
mod block;
mod cache;
mod client;
mod input;
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

use crate::app::App;
use crate::client::PodClient;
use crate::spawn::{SpawnOutcome, SpawnReady};

fn resolve_socket(pod_name: &str, override_path: Option<PathBuf>) -> PathBuf {
    if let Some(p) = override_path {
        return p;
    }
    if let Ok(rd) = std::env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(rd)
            .join("insomnia")
            .join(pod_name)
            .join("sock")
    } else if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home)
            .join(".insomnia")
            .join("run")
            .join(pod_name)
            .join("sock")
    } else {
        PathBuf::from("/tmp")
            .join("insomnia")
            .join(pod_name)
            .join("sock")
    }
}

enum Mode {
    Spawn,
    Attach {
        pod_name: String,
        socket_override: Option<PathBuf>,
    },
}

fn parse_args() -> Mode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        return Mode::Spawn;
    }
    let pod_name = args[0].clone();
    let socket_override = args
        .windows(2)
        .find(|w| w[0] == "--socket")
        .map(|w| PathBuf::from(&w[1]));
    Mode::Attach {
        pod_name,
        socket_override,
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let mode = parse_args();

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
        Mode::Spawn => run_spawn().await,
        Mode::Attach {
            pod_name,
            socket_override,
        } => run_attach(pod_name, socket_override).await,
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

async fn run_spawn() -> Result<(), Box<dyn std::error::Error>> {
    let ready = match spawn::run().await? {
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
            app.push_error(format!("Failed to connect to {}: {e}", socket_path.display()));
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
        KeyCode::Enter if alt => {
            app.insert_newline();
            None
        }
        KeyCode::Enter => app.submit_input(),
        KeyCode::Backspace => {
            app.delete_char_before();
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
            app.move_cursor_up();
            None
        }
        KeyCode::Down => {
            app.move_cursor_down();
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
        KeyCode::Char(c) => {
            app.insert_char(c);
            None
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
