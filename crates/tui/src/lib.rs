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

use client::{Target, WorkerConnectionSelector, WorkerListRequest, WorkerSpawn};

#[derive(Debug)]
pub struct LaunchOptions {
    pub target: Box<dyn Target>,
    pub mode: LaunchMode,
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
    /// `yoi workers` / `yoi --backend <url>`: list workers through the selected
    /// connection target, then attach to the selected Worker.
    Workers {
        runtime_id: Option<String>,
        include_stopped: bool,
        all: bool,
    },
    /// `yoi --backend <url> --runtime-id <id> --worker-id <id>`: open one Worker
    /// through the selected connection target.
    OpenWorker {
        runtime_id: String,
        worker_id: String,
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
    Panel { include_stopped: bool },
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
        } => match target.spawn_worker() {
            Ok(WorkerSpawn::LegacyLocal { runtime_command }) => {
                console::run_spawn(None, worker_name, profile, runtime_command).await
            }
            Ok(WorkerSpawn::Standalone { state_dir }) => {
                console::run_standalone(workspace_root.clone(), state_dir, worker_name, profile)
                    .await
            }
            Err(e) => Err(Box::new(e) as Box<dyn std::error::Error>),
        },
        LaunchMode::WorkerName {
            worker_name,
            socket_override,
        } => match target.worker_by_name() {
            Ok(worker_by_name) => {
                console::run_worker_name(
                    worker_name,
                    socket_override,
                    worker_by_name.runtime_command,
                )
                .await
            }
            Err(e) => Err(Box::new(e) as Box<dyn std::error::Error>),
        },
        LaunchMode::Workers {
            runtime_id,
            include_stopped,
            all,
        } => match target.list_workers(if include_stopped {
            WorkerListRequest::with_stopped(runtime_id)
        } else {
            WorkerListRequest::new(runtime_id)
        }) {
            Ok(worker_list) => {
                if let Some(target) = worker_list.backend_target {
                    backend_worker_picker::run(target, worker_list.include_stopped).await
                } else if let Some(runtime_command) = worker_list.local_runtime_command {
                    console::run_worker_picker(
                        runtime_command,
                        workspace_root.clone(),
                        all,
                        worker_list.include_stopped,
                    )
                    .await
                } else {
                    Err(Box::new(io::Error::other(
                        "worker list target did not include a local or backend source",
                    )) as Box<dyn std::error::Error>)
                }
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
        LaunchMode::Resume { all } => match target.resume_worker() {
            Ok(resume) => {
                console::run_resume(resume.runtime_command, workspace_root.clone(), all).await
            }
            Err(e) => Err(Box::new(e) as Box<dyn std::error::Error>),
        },
        LaunchMode::ResumeWithSession { id, worker_name } => match target.spawn_worker() {
            Ok(WorkerSpawn::LegacyLocal { runtime_command }) => {
                console::run_spawn(Some(id), worker_name, None, runtime_command).await
            }
            Ok(WorkerSpawn::Standalone { .. }) => Err(Box::new(io::Error::new(
                io::ErrorKind::Unsupported,
                "Standalone session restore is not implemented",
            )) as Box<dyn std::error::Error>),
            Err(e) => Err(Box::new(e) as Box<dyn std::error::Error>),
        },
        LaunchMode::Panel { include_stopped } => match target.dashboard() {
            Ok(client::Dashboard::Local { runtime_command }) => {
                dashboard::launch(runtime_command, include_stopped).await
            }
            Ok(client::Dashboard::Backend {
                base_url,
                workspace_id,
            }) => backend_dashboard::launch(base_url, workspace_id).await,
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
