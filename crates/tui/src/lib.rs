mod app;
mod block;
mod cache;
mod command;
mod composer_history;
mod composer_keys;
mod console;
mod dashboard;
#[cfg(feature = "e2e-test")]
mod e2e_observer;
mod input;
pub mod keys;
mod markdown;
mod picker;
mod role_session_registry;
mod scroll;
pub mod setup_model;
mod spawn;
mod task;
mod text_selection;
mod tool;
mod ui;
mod view_mode;
mod worker_list;
mod workspace_panel;

use std::io;
use std::path::PathBuf;
use std::process::ExitCode;

use crossterm::event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste};
use crossterm::execute;
use crossterm::terminal::{LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use session_store::SegmentId;

use client::WorkerRuntimeCommand;

#[derive(Debug, Clone)]
pub struct LaunchOptions {
    pub mode: LaunchMode,
    pub runtime_command: WorkerRuntimeCommand,
    pub workspace_root: PathBuf,
}

#[derive(Debug, Clone)]
pub enum LaunchMode {
    Spawn {
        worker_name: Option<String>,
        profile: Option<String>,
    },
    /// `yoi --worker <name>`: attach to a live Worker by name if possible;
    /// otherwise launch the Worker runtime command with `--worker <name>` so it
    /// resumes from name-keyed state or creates a fresh same-name Worker.
    WorkerName {
        worker_name: String,
        socket_override: Option<PathBuf>,
    },
    /// `yoi resume`: open the Worker picker, then attach to the selected live Worker
    /// or restore the selected stopped Worker by name. Without `--all`, the picker
    /// is scoped to the current runtime workspace.
    Resume { all: bool },
    /// `yoi --session <UUID>`: skip the picker, go straight to the
    /// resume name dialog with `id` baked in.
    ResumeWithSession {
        id: SegmentId,
        worker_name: Option<String>,
    },
    /// `yoi panel`: open the workspace Dashboard from the current workspace.
    Panel,
}

pub async fn launch(options: LaunchOptions) -> ExitCode {
    let LaunchOptions {
        mode,
        runtime_command,
        workspace_root,
    } = options;

    if let Err(e) = std::env::set_current_dir(&workspace_root) {
        eprintln!(
            "yoi: failed to enter workspace {}: {e}",
            workspace_root.display()
        );
        return ExitCode::FAILURE;
    }

    if let Err(e) = enable_raw_mode() {
        eprintln!("yoi: failed to enter raw mode: {e}");
        return ExitCode::FAILURE;
    }
    if let Err(e) = execute!(io::stdout(), EnableBracketedPaste) {
        let _ = disable_raw_mode();
        eprintln!("yoi: {e}");
        return ExitCode::FAILURE;
    }

    let result = match mode {
        LaunchMode::Spawn {
            worker_name,
            profile,
        } => console::run_spawn(None, worker_name, profile, runtime_command).await,
        LaunchMode::WorkerName {
            worker_name,
            socket_override,
        } => console::run_worker_name(worker_name, socket_override, runtime_command).await,
        LaunchMode::Resume { all } => {
            console::run_resume(runtime_command, workspace_root.clone(), all).await
        }
        LaunchMode::ResumeWithSession { id, worker_name } => {
            console::run_spawn(Some(id), worker_name, None, runtime_command).await
        }
        LaunchMode::Panel => dashboard::launch(runtime_command).await,
    };

    // Always restore the terminal first so any pending eprintln below
    // shows up cleanly in scrollback rather than inside an active
    // alternate-screen buffer.
    #[cfg(feature = "e2e-test")]
    e2e_observer::emit("tui", "terminal_cleanup_started", serde_json::json!({}));
    let mut stdout = io::stdout();
    let _ = execute!(
        stdout,
        DisableMouseCapture,
        LeaveAlternateScreen,
        DisableBracketedPaste
    );
    let _ = disable_raw_mode();
    let _ = execute!(stdout, crossterm::cursor::Show);
    #[cfg(feature = "e2e-test")]
    e2e_observer::emit("tui", "terminal_cleanup_finished", serde_json::json!({}));

    match result {
        Ok(()) => {
            #[cfg(feature = "e2e-test")]
            e2e_observer::emit("tui", "exit", serde_json::json!({ "status": "success" }));
            ExitCode::SUCCESS
        }
        Err(e) => {
            // SpawnError has already been painted into the inline
            // viewport's final frame, so it's already visible in the
            // user's scrollback — printing it again would be a noisy
            // duplicate. Other errors (worker-name failures, terminal setup
            // hiccups, etc.) need surfacing here.
            if e.downcast_ref::<spawn::SpawnError>().is_none() {
                eprintln!("yoi: {e}");
            }
            #[cfg(feature = "e2e-test")]
            e2e_observer::emit("tui", "exit", serde_json::json!({ "status": "failure" }));
            ExitCode::FAILURE
        }
    }
}
