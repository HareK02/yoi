mod app;
mod block;
mod cache;
mod command;
mod composer_history;
mod composer_keys;
#[cfg(feature = "e2e-test")]
mod e2e_observer;
mod input;
pub mod keys;
mod markdown;
mod multi_pod;
mod picker;
mod pod_list;
mod role_session_registry;
mod scroll;
pub mod setup_model;
mod single_pod;
mod spawn;
mod task;
mod text_selection;
mod tool;
mod ui;
mod view_mode;
mod workspace_panel;

use std::io;
use std::path::PathBuf;
use std::process::ExitCode;

use crossterm::event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste};
use crossterm::execute;
use crossterm::terminal::{LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use session_store::SegmentId;

use client::PodRuntimeCommand;

#[derive(Debug, Clone)]
pub struct LaunchOptions {
    pub mode: LaunchMode,
    pub runtime_command: PodRuntimeCommand,
    pub workspace_root: PathBuf,
}

#[derive(Debug, Clone)]
pub enum LaunchMode {
    Spawn {
        pod_name: Option<String>,
        profile: Option<String>,
    },
    /// `yoi <name>` / `yoi --pod <name>`: attach to a live Pod by name if
    /// possible; otherwise launch the Pod runtime command with `--pod <name>` so it
    /// resumes from name-keyed state or creates a fresh same-name Pod.
    PodName {
        pod_name: String,
        socket_override: Option<PathBuf>,
    },
    /// `yoi -r` / `yoi --resume`: open the Pod picker, then attach to the
    /// selected live Pod or restore the selected stopped Pod by name.
    Resume,
    /// `yoi --session <UUID>`: skip the picker, go straight to the
    /// resume name dialog with `id` baked in.
    ResumeWithSession {
        id: SegmentId,
        pod_name: Option<String>,
    },
    /// `yoi panel`: open the workspace panel from the current workspace.
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
        LaunchMode::Spawn { pod_name, profile } => {
            single_pod::run_spawn(None, pod_name, profile, runtime_command).await
        }
        LaunchMode::PodName {
            pod_name,
            socket_override,
        } => single_pod::run_pod_name(pod_name, socket_override, runtime_command).await,
        LaunchMode::Resume => single_pod::run_resume(runtime_command).await,
        LaunchMode::ResumeWithSession { id, pod_name } => {
            single_pod::run_spawn(Some(id), pod_name, None, runtime_command).await
        }
        LaunchMode::Panel => single_pod::run_panel(runtime_command).await,
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
            // duplicate. Other errors (pod-name failures, terminal setup
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
