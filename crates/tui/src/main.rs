mod app;
mod client;
mod ui;

use std::io;
use std::path::PathBuf;

use crossterm::event::{self, Event as TermEvent, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{execute};
use protocol::Method;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::app::App;
use crate::client::PodClient;

fn resolve_socket(pod_name: &str, override_path: Option<PathBuf>) -> PathBuf {
    if let Some(p) = override_path {
        return p;
    }
    if let Ok(rd) = std::env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(rd).join("insomnia").join(pod_name).join("sock")
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

    // Install panic hook to restore terminal
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = terminal::disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        original_hook(info);
    }));

    // Setup terminal
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(pod_name);

    // Connect to pod
    match PodClient::connect(&socket_path).await {
        Ok(mut client) => {
            app.connected = true;
            let _ = client.send(&Method::GetHistory).await;
            run_loop(&mut terminal, &mut app, client).await?;
        }
        Err(e) => {
            app.messages.push(app::Message {
                kind: app::MessageKind::Error,
                content: format!("Failed to connect to {}: {e}", socket_path.display()),
            });
            // Show error and wait for quit
            run_disconnected(&mut terminal, &mut app)?;
        }
    }

    // Restore terminal
    terminal::disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    Ok(())
}

async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    mut client: PodClient,
) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        terminal.draw(|f| ui::draw(f, app))?;

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
            // Pod events
            event = client.next_event() => {
                match event {
                    Some(ev) => app.handle_pod_event(ev),
                    None => {
                        app.connected = false;
                        app.messages.push(app::Message {
                            kind: app::MessageKind::Error,
                            content: "Connection lost".into(),
                        });
                    }
                }
            }
        }
    }

    Ok(())
}

fn run_disconnected(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        terminal.draw(|f| ui::draw(f, app))?;

        if event::poll(std::time::Duration::from_millis(100))? {
            if let TermEvent::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Esc => break,
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                    _ => {}
                }
            }
        }
    }
    Ok(())
}

fn handle_key(app: &mut App, key: KeyEvent) -> Option<Method> {
    match key.code {
        KeyCode::Esc => {
            app.quit = true;
            None
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.quit = true;
            None
        }
        KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(Method::Resume)
        }
        KeyCode::Char('x') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(Method::Cancel)
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
        KeyCode::PageUp => {
            app.scroll_up();
            None
        }
        KeyCode::PageDown => {
            app.scroll_down();
            None
        }
        KeyCode::Char(c) => {
            app.insert_char(c);
            None
        }
        _ => None,
    }
}
