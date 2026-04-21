mod app;
mod block;
mod cache;
mod client;
mod input;
mod scroll;
mod tool;
mod ui;

use std::io;
use std::path::PathBuf;

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

fn parse_args() -> (String, Option<PathBuf>) {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: tui <pod_name> [--socket <path>]");
        std::process::exit(1);
    }
    let pod_name = args[1].clone();
    let socket = args
        .windows(2)
        .find(|w| w[0] == "--socket")
        .map(|w| PathBuf::from(&w[1]));
    (pod_name, socket)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (pod_name, socket_override) = parse_args();
    let socket_path = resolve_socket(&pod_name, socket_override);

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run(&mut terminal, pod_name, &socket_path).await;

    // Always restore the terminal, even on error.
    let _ = execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        LeaveAlternateScreen
    );
    let _ = disable_raw_mode();
    terminal.show_cursor().ok();

    result
}

async fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    pod_name: String,
    socket_path: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut app = App::new(pod_name);

    match PodClient::connect(socket_path).await {
        Ok(mut client) => {
            app.connected = true;
            let _ = client.send(&Method::GetHistory).await;
            run_loop(terminal, &mut app, client).await?;
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
) -> Result<(), Box<dyn std::error::Error>> {
    terminal.draw(|f| ui::draw(f, app))?;

    loop {
        if app.quit {
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
