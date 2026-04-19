mod app;
mod client;
mod ui;

use std::io;
use std::path::PathBuf;

use crossterm::event::{self, Event as TermEvent, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal;
use protocol::Method;
use ratatui::backend::CrosstermBackend;
use ratatui::{Terminal, TerminalOptions, Viewport};

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

    terminal::enable_raw_mode()?;
    let stdout = io::stdout();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Inline(3),
        },
    )?;

    let mut app = App::new(pod_name);

    match PodClient::connect(&socket_path).await {
        Ok(mut client) => {
            app.connected = true;
            let _ = client.send(&Method::GetHistory).await;
            run_loop(&mut terminal, &mut app, client).await?;
        }
        Err(e) => {
            app.output_queue.push(app::OutputItem::Padded(
                app::MessageKind::Error,
                format!("Failed to connect to {}: {e}", socket_path.display()),
            ));
            ui::flush_output(&mut terminal, &mut app)?;
            terminal.draw(|f| ui::draw(f, &app))?;
            run_disconnected(&mut app)?;
        }
    }

    terminal::disable_raw_mode()?;

    Ok(())
}

async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    mut client: PodClient,
) -> Result<(), Box<dyn std::error::Error>> {
    // Initial draw of the viewport
    terminal.draw(|f| ui::draw(f, app))?;

    loop {
        if app.quit {
            break;
        }

        tokio::select! {
            // Terminal input
            _ = tokio::task::spawn_blocking(|| event::poll(std::time::Duration::from_millis(50))) => {
                while event::poll(std::time::Duration::ZERO)? {
                    if let TermEvent::Key(key) = event::read()? {
                        if let Some(method) = handle_key(app, key) {
                            client.send(&method).await?;
                        }
                        if app.quit {
                            break;
                        }
                    }
                }
            }
            // Pod events (disabled after disconnect)
            event = client.next_event(), if app.connected => {
                match event {
                    Some(ev) => app.handle_pod_event(ev),
                    None => {
                        app.connected = false;
                        app.output_queue.push(app::OutputItem::Padded(
                            app::MessageKind::Error,
                            "Connection lost".into(),
                        ));
                    }
                }
            }
        }

        // Flush any queued output above the viewport
        ui::flush_output(terminal, app)?;
        // Redraw the fixed viewport (status + input)
        terminal.draw(|f| ui::draw(f, app))?;
    }

    Ok(())
}

fn run_disconnected(_app: &mut App) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        if event::poll(std::time::Duration::from_millis(100))? {
            if let TermEvent::Key(key) = event::read()? {
                if let KeyCode::Char('c') = key.code {
                    if key.modifiers.contains(KeyModifiers::CONTROL) {
                        break;
                    }
                }
            }
        }
    }
    Ok(())
}

fn handle_key(app: &mut App, key: KeyEvent) -> Option<Method> {
    match key.code {
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            handle_pause_or_quit(app)
        }
        KeyCode::Char('x') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if app.running {
                Some(Method::Cancel)
            } else {
                app.output_queue.push(app::OutputItem::Padded(
                    app::MessageKind::Error,
                    "Nothing to cancel (Pod is not running).".into(),
                ));
                None
            }
        }
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            return handle_shutdown(app);
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
    if let Some(t) = app.shutdown_confirm {
        if t.elapsed() < CONFIRM_TIMEOUT {
            app.shutdown_confirm = None;
            return Some(Method::Shutdown);
        }
    }
    app.shutdown_confirm = Some(std::time::Instant::now());
    app.output_queue.push(app::OutputItem::Padded(
        app::MessageKind::Error,
        "Turn is running. Press Ctrl-D again to cancel and shut down.".into(),
    ));
    None
}

/// Running → send `Method::Pause`.
/// Idle / Paused → 2-tap to quit the TUI (the Pod keeps running).
fn handle_pause_or_quit(app: &mut App) -> Option<Method> {
    if app.running {
        return Some(Method::Pause);
    }
    if let Some(t) = app.quit_confirm {
        if t.elapsed() < CONFIRM_TIMEOUT {
            app.quit_confirm = None;
            app.quit = true;
            return None;
        }
    }
    app.quit_confirm = Some(std::time::Instant::now());
    app.output_queue.push(app::OutputItem::Padded(
        app::MessageKind::Error,
        "Press Ctrl-C again within 3 s to exit the TUI (the Pod keeps running).".into(),
    ));
    None
}
