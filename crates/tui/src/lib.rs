mod app;
mod block;
mod cache;
mod command;
mod input;
pub mod keys;
mod markdown;
mod multi_pod;
mod picker;
mod pod_list;
mod scroll;
mod single_pod;
mod spawn;
mod task;
mod tool;
mod ui;
mod view_mode;

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
}

#[derive(Debug, Clone)]
pub enum LaunchMode {
    Spawn {
        profile: Option<String>,
    },
    /// `insomnia <name>` / `insomnia --pod <name>`: attach to a live Pod by name if
    /// possible; otherwise launch the Pod runtime command with `--pod <name>` so it
    /// resumes from name-keyed state or creates a fresh same-name Pod.
    PodName {
        pod_name: String,
        socket_override: Option<PathBuf>,
    },
    /// `insomnia -r` / `insomnia --resume`: open the Pod picker, then attach to the
    /// selected live Pod or restore the selected stopped Pod by name.
    Resume,
    /// `insomnia --session <UUID>`: skip the picker, go straight to the
    /// resume name dialog with `id` baked in.
    ResumeWithSession(SegmentId),
    /// `insomnia --multi`: open the multi-Pod dashboard. This is intentionally
    /// separate from `-r`/`--resume`, which keeps its single-Pod picker
    /// meaning.
    Multi,
}

pub async fn launch(options: LaunchOptions) -> ExitCode {
    let LaunchOptions {
        mode,
        runtime_command,
    } = options;

    if let Err(e) = enable_raw_mode() {
        eprintln!("insomnia: failed to enter raw mode: {e}");
        return ExitCode::FAILURE;
    }
    if let Err(e) = execute!(io::stdout(), EnableBracketedPaste) {
        let _ = disable_raw_mode();
        eprintln!("insomnia: {e}");
        return ExitCode::FAILURE;
    }

    let result = match mode {
        LaunchMode::Spawn { profile } => {
            single_pod::run_spawn(None, profile, runtime_command).await
        }
        LaunchMode::PodName {
            pod_name,
            socket_override,
        } => single_pod::run_pod_name(pod_name, socket_override, runtime_command).await,
        LaunchMode::Resume => single_pod::run_resume(runtime_command).await,
        LaunchMode::ResumeWithSession(id) => {
            single_pod::run_spawn(Some(id), None, runtime_command).await
        }
        LaunchMode::Multi => single_pod::run_multi(runtime_command).await,
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
            // duplicate. Other errors (pod-name failures, terminal setup
            // hiccups, etc.) need surfacing here.
            if e.downcast_ref::<spawn::SpawnError>().is_none() {
                eprintln!("insomnia: {e}");
            }
            ExitCode::FAILURE
        }
    }
}
