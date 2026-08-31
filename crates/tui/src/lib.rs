mod app;
mod backend_dashboard;
mod backend_worker_picker;
mod backend_workspace_picker;
mod block;
mod cache;
mod command;
mod composer_history;
mod composer_keys;
mod console;
#[cfg(feature = "e2e-test")]
mod e2e_observer;
mod input;
pub mod keys;
mod markdown;
mod scroll;
pub mod setup_model;
mod standalone_picker;
mod standalone_spawn;
mod task;
mod text_selection;
mod tool;
mod ui;
mod view_mode;

use std::io;
use std::path::PathBuf;
use std::process::ExitCode;

use crossterm::event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste};
use crossterm::execute;
use crossterm::terminal::{LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};

use client::{Target, WorkerConnectionSelector, WorkerListRequest};

#[derive(Debug)]
pub struct LaunchOptions {
    pub target: Box<dyn Target>,
    pub mode: LaunchMode,
    pub workspace_root: PathBuf,
}

#[derive(Debug, Clone)]
pub enum LaunchMode {
    /// Start one client-owned in-process Standalone Worker.
    Spawn {
        worker_name: Option<String>,
        profile: Option<String>,
    },
    /// Restore one client-owned standalone Worker. The current cwd is the default scope;
    /// `include_all` opts into all standalone Workers under the same client data root.
    StandaloneResume { include_all: bool },
    /// List Backend Workers and attach to the selected Worker.
    Workers {
        runtime_id: Option<String>,
        include_stopped: bool,
    },
    /// Open one Backend Worker through the selected connection target.
    OpenWorker {
        runtime_id: String,
        worker_id: String,
    },
    /// Open the Backend Workspace dashboard.
    Panel,
}

struct TerminalModeGuard {
    active: bool,
}

impl TerminalModeGuard {
    fn new() -> Self {
        Self { active: true }
    }

    fn restore(&mut self) -> io::Result<()> {
        if !self.active {
            return Ok(());
        }
        self.active = false;
        let mut stdout = io::stdout();
        execute!(
            stdout,
            DisableMouseCapture,
            LeaveAlternateScreen,
            DisableBracketedPaste,
            crossterm::cursor::Show
        )?;
        disable_raw_mode()
    }
}

impl Drop for TerminalModeGuard {
    fn drop(&mut self) {
        if self.active {
            let mut stdout = io::stdout();
            let _ = execute!(
                stdout,
                DisableMouseCapture,
                LeaveAlternateScreen,
                DisableBracketedPaste,
                crossterm::cursor::Show
            );
            let _ = disable_raw_mode();
            self.active = false;
        }
    }
}

pub async fn launch(options: LaunchOptions) -> ExitCode {
    let LaunchOptions {
        target,
        mode,
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
    let mut terminal_mode = TerminalModeGuard::new();

    let result = match mode {
        LaunchMode::Spawn {
            worker_name,
            profile,
        } => match standalone_spawn::select(&workspace_root, worker_name, profile) {
            Ok(Some(selection)) => match target.spawn_worker() {
                Ok(spawn) => {
                    console::run_standalone(
                        workspace_root.clone(),
                        spawn.state_dir,
                        Some(selection.worker_name),
                        Some(selection.profile),
                    )
                    .await
                }
                Err(error) => Err(Box::new(error) as Box<dyn std::error::Error>),
            },
            Ok(None) => Ok(()),
            Err(error) => Err(Box::new(error) as Box<dyn std::error::Error>),
        },
        LaunchMode::StandaloneResume { include_all } => {
            match standalone_picker::pick(target.as_ref(), include_all) {
                Ok(Some(intent)) => console::run_standalone_restore(intent).await,
                Ok(None) => Ok(()),
                Err(error) => Err(Box::new(error) as Box<dyn std::error::Error>),
            }
        }
        LaunchMode::Workers {
            runtime_id,
            include_stopped,
        } => match target.list_workers(if include_stopped {
            WorkerListRequest::with_stopped(runtime_id)
        } else {
            WorkerListRequest::new(runtime_id)
        }) {
            Ok(worker_list) => {
                backend_worker_picker::run(worker_list.backend_target, worker_list.include_stopped)
                    .await
            }
            Err(e) => Err(Box::new(e) as Box<dyn std::error::Error>),
        },
        LaunchMode::OpenWorker {
            runtime_id,
            worker_id,
        } => match target.connect_worker(WorkerConnectionSelector::new(runtime_id, worker_id)) {
            Ok(connection) => console::run_backend_runtime(connection.target).await,
            Err(e) => Err(Box::new(e) as Box<dyn std::error::Error>),
        },
        LaunchMode::Panel => match target.dashboard() {
            Ok(dashboard) => {
                backend_dashboard::launch(dashboard.base_url, dashboard.workspace_id).await
            }
            Err(e) => Err(Box::new(e) as Box<dyn std::error::Error>),
        },
    };

    // Always restore the terminal first so any pending eprintln below
    // shows up cleanly in scrollback rather than inside an active
    // alternate-screen buffer.
    #[cfg(feature = "e2e-test")]
    e2e_observer::emit("tui", "terminal_cleanup_started", serde_json::json!({}));
    let _ = terminal_mode.restore();
    #[cfg(feature = "e2e-test")]
    e2e_observer::emit("tui", "terminal_cleanup_finished", serde_json::json!({}));

    match result {
        Ok(()) => {
            #[cfg(feature = "e2e-test")]
            e2e_observer::emit("tui", "exit", serde_json::json!({ "status": "success" }));
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("yoi: {e}");
            #[cfg(feature = "e2e-test")]
            e2e_observer::emit("tui", "exit", serde_json::json!({ "status": "failure" }));
            ExitCode::FAILURE
        }
    }
}
