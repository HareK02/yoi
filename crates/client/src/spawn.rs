//! Worker runtime command をサブプロセスとして立ち上げ、`YOI-READY` を待つ
//! ハンドシェイク。
//!
//! - 親プロセス (TUI / GUI / E2E) は profile/default/typed restore flags を
//!   指定してこの関数に渡す。worker はそれを受けて socket を bind し、stderr に
//!   `YOI-READY\t<name>\t<socket>` を吐く。
//! - 待機中の stderr 行は `progress` コールバック越しに呼び出し側へ流す。
//!   UI の進捗表示や E2E のログ収集はここで賄う。
//! - `kill_on_drop = false` + `process_group(0)` により、親プロセス
//!   ライフサイクルから切り離した detached worker を作る。ready 後の lifecycle
//!   管理は runtime ディレクトリ / socket を介して行う。

use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use crate::WorkerRuntimeCommand;
use tokio::process::Command;
use uuid::Uuid;

const READY_PREFIX: &str = "YOI-READY\t";
const READY_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerProcessLaunchConfig {
    pub runtime_command: WorkerRuntimeCommand,
    /// `worker.name` として使う識別子。runtime ディレクトリ
    /// (`manifest::paths::worker_runtime_dir`) の解決と、ready 行に乗る
    /// 名前との突き合わせに使う。
    pub worker_name: String,
    /// Optional reusable Profile selector. Worker identity is always supplied
    /// separately with `--worker`; profile selection must not imply a name.
    pub profile: Option<String>,
    /// Explicit runtime workspace root. The child receives it via
    /// `--workspace` so startup does not infer workspace identity from the
    /// parent process cwd.
    pub workspace_root: PathBuf,
    /// Optional child process cwd. This is not runtime workspace identity and
    /// is not passed as a CLI argument; the child observes it as its ordinary
    /// process current directory.
    pub cwd: Option<PathBuf>,
    /// `Some(id)` のとき `--session <id>` を付与し、当該セッションから
    /// resume させる。
    pub resume_from: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkerProcessLaunchOptions {
    /// Extra child CLI arguments supplied by an upper resolver layer. The
    /// low-level launch config intentionally does not model Ticket IDs,
    /// Ticket roles, orchestration roles, executable authority, or raw
    /// browser-provided profile/cwd/workspace inputs.
    pub extra_args: Vec<String>,
}

impl WorkerProcessLaunchOptions {
    pub fn with_hidden_arg(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_args.extend([name.into(), value.into()]);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.extra_args.is_empty()
    }
}

pub type SpawnConfig = WorkerProcessLaunchConfig;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnReady {
    pub worker_name: String,
    pub socket_path: PathBuf,
}

#[derive(Debug)]
pub enum SpawnError {
    Io(io::Error),
    /// runtime ディレクトリが解決できなかった (環境変数未設定等)。
    RuntimeDirUnavailable,
    PodLaunchFailed {
        command: WorkerRuntimeCommand,
        source: io::Error,
    },
    WorkerExitedEarly {
        stderr_tail: String,
    },
    Timeout,
}

impl std::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::RuntimeDirUnavailable => write!(
                f,
                "could not resolve runtime directory (set YOI_HOME, YOI_RUNTIME_DIR, XDG_RUNTIME_DIR, or HOME)"
            ),
            Self::PodLaunchFailed { command, source } => write!(
                f,
                "failed to launch worker runtime command `{command}`: {source}"
            ),
            Self::WorkerExitedEarly { stderr_tail } => {
                if stderr_tail.is_empty() {
                    write!(f, "worker exited before becoming ready")
                } else {
                    write!(f, "worker exited before becoming ready: {stderr_tail}")
                }
            }
            Self::Timeout => write!(
                f,
                "worker did not become ready within {}s",
                READY_TIMEOUT.as_secs()
            ),
        }
    }
}

impl std::error::Error for SpawnError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) | Self::PodLaunchFailed { source: error, .. } => Some(error),
            Self::RuntimeDirUnavailable | Self::WorkerExitedEarly { .. } | Self::Timeout => None,
        }
    }
}

impl From<io::Error> for SpawnError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

fn runtime_args(
    config: &WorkerProcessLaunchConfig,
    options: &WorkerProcessLaunchOptions,
) -> Vec<String> {
    let mut args = vec![
        "--workspace".to_string(),
        config.workspace_root.display().to_string(),
    ];
    if let Some(id) = config.resume_from {
        args.extend([
            "--session".to_string(),
            id.to_string(),
            "--worker".to_string(),
            config.worker_name.clone(),
        ]);
    } else {
        args.extend(["--worker".to_string(), config.worker_name.clone()]);
        if let Some(profile) = &config.profile {
            args.extend(["--profile".to_string(), profile.clone()]);
        }
    }
    args.extend(options.extra_args.clone());
    args
}

/// worker を spawn し、`YOI-READY` ハンドシェイクが終わるまで待つ。
///
/// `progress` は ready 行を見つけるまでに観測した stderr の各行で呼ばれる
/// (ready 行自体は除外される)。UI の表示更新や E2E ログ取得に使う。
pub async fn spawn_worker<F>(
    config: WorkerProcessLaunchConfig,
    progress: F,
) -> Result<SpawnReady, SpawnError>
where
    F: FnMut(&str),
{
    spawn_worker_with_options(config, WorkerProcessLaunchOptions::default(), progress).await
}

pub async fn spawn_worker_with_options<F>(
    config: WorkerProcessLaunchConfig,
    options: WorkerProcessLaunchOptions,
    mut progress: F,
) -> Result<SpawnReady, SpawnError>
where
    F: FnMut(&str),
{
    let worker_runtime_dir = manifest::paths::worker_runtime_dir(&config.worker_name)
        .ok_or(SpawnError::RuntimeDirUnavailable)?;
    std::fs::create_dir_all(&worker_runtime_dir).map_err(SpawnError::Io)?;
    let stderr_path = worker_runtime_dir.join("stderr.log");
    let stderr_file = std::fs::File::create(&stderr_path).map_err(SpawnError::Io)?;

    let mut command = Command::new(config.runtime_command.program());
    command
        .args(config.runtime_command.prefix_args())
        .current_dir(config.cwd.as_ref().unwrap_or(&config.workspace_root))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr_file))
        .process_group(0);
    for arg in runtime_args(&config, &options) {
        command.arg(arg);
    }
    let mut child = command
        .spawn()
        .map_err(|source| SpawnError::PodLaunchFailed {
            command: config.runtime_command.clone(),
            source,
        })?;

    // Default `kill_on_drop = false` plus `process_group(0)` makes this
    // a detached Worker once startup succeeds: dropping the handle does not
    // terminate it, and terminal-generated signals for the parent's
    // process group do not hit the Worker. Runtime state/socket files are
    // the source of truth after that point.
    let ready = match wait_for_ready_file(&mut progress, &stderr_path, &mut child).await {
        Ok(ready) => ready,
        Err(e) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(e);
        }
    };
    tokio::spawn(async move {
        let _ = child.wait().await;
    });
    Ok(ready)
}

async fn wait_for_ready_file<F>(
    progress: &mut F,
    stderr_path: &Path,
    child: &mut tokio::process::Child,
) -> Result<SpawnReady, SpawnError>
where
    F: FnMut(&str),
{
    let mut tail = StderrTail::new();
    let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
    let mut offset = 0usize;

    loop {
        let content = match tokio::fs::read_to_string(stderr_path).await {
            Ok(content) => content,
            Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(SpawnError::Io(e)),
        };
        if content.len() > offset {
            for line in content[offset..].lines() {
                if let Some(rest) = line.strip_prefix(READY_PREFIX) {
                    let mut parts = rest.splitn(2, '\t');
                    let worker_name = parts.next().unwrap_or("").to_string();
                    let socket_str = parts.next().unwrap_or("").to_string();
                    if worker_name.is_empty() || socket_str.is_empty() {
                        return Err(SpawnError::WorkerExitedEarly {
                            stderr_tail: format!("malformed ready line: {line}"),
                        });
                    }
                    let socket_path = PathBuf::from(socket_str);
                    wait_for_socket(
                        &socket_path,
                        deadline,
                        child,
                        stderr_path,
                        &mut tail,
                        &mut offset,
                    )
                    .await?;
                    return Ok(SpawnReady {
                        worker_name,
                        socket_path,
                    });
                }
                tail.push(line);
                progress(line);
            }
            offset = content.len();
        }

        if tokio::time::Instant::now() >= deadline {
            return Err(SpawnError::Timeout);
        }
        tokio::select! {
            status = child.wait() => {
                let _ = status;
                // Worker は exit 直前に最終 stderr 行を flush することがある。
                // child.wait() が解決した後に再読みして、原因行を取りこ
                // ぼさず WorkerExitedEarly に載せる。
                drain_stderr_into_tail(stderr_path, &mut tail, &mut offset).await;
                return Err(SpawnError::WorkerExitedEarly {
                    stderr_tail: tail.into_string(),
                });
            }
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }
}

async fn wait_for_socket(
    socket_path: &Path,
    deadline: tokio::time::Instant,
    child: &mut tokio::process::Child,
    stderr_path: &Path,
    tail: &mut StderrTail,
    offset: &mut usize,
) -> Result<(), SpawnError> {
    loop {
        match tokio::net::UnixStream::connect(socket_path).await {
            Ok(_) => return Ok(()),
            Err(e)
                if e.kind() == io::ErrorKind::NotFound
                    || e.kind() == io::ErrorKind::ConnectionRefused => {}
            Err(e) => return Err(SpawnError::Io(e)),
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(SpawnError::Timeout);
        }
        tokio::select! {
            status = child.wait() => {
                let _ = status;
                drain_stderr_into_tail(stderr_path, tail, offset).await;
                return Err(SpawnError::WorkerExitedEarly {
                    stderr_tail: tail.as_string(),
                });
            }
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
    }
}

async fn drain_stderr_into_tail(stderr_path: &Path, tail: &mut StderrTail, offset: &mut usize) {
    let Ok(content) = tokio::fs::read_to_string(stderr_path).await else {
        return;
    };
    if content.len() <= *offset {
        return;
    }
    for line in content[*offset..].lines() {
        if !line.starts_with(READY_PREFIX) {
            tail.push(line);
        }
    }
    *offset = content.len();
}

struct StderrTail {
    lines: std::collections::VecDeque<String>,
}

impl StderrTail {
    fn new() -> Self {
        Self {
            lines: std::collections::VecDeque::with_capacity(8),
        }
    }
    fn push(&mut self, line: &str) {
        if self.lines.len() == 8 {
            self.lines.pop_front();
        }
        self.lines.push_back(line.to_string());
    }
    fn as_string(&self) -> String {
        self.lines.iter().cloned().collect::<Vec<_>>().join(" | ")
    }
    fn into_string(self) -> String {
        self.lines.into_iter().collect::<Vec<_>>().join(" | ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn base_config() -> WorkerProcessLaunchConfig {
        WorkerProcessLaunchConfig {
            runtime_command: WorkerRuntimeCommand::new("/bin/yoi", vec![OsString::from("worker")]),
            worker_name: "explicit-worker".to_string(),
            profile: Some("project:companion".to_string()),
            workspace_root: PathBuf::from("/work/other-project"),
            cwd: None,
            resume_from: None,
        }
    }

    #[test]
    fn runtime_args_keep_workspace_worker_and_profile_separate() {
        assert_eq!(
            runtime_args(&base_config(), &WorkerProcessLaunchOptions::default()),
            vec![
                "--workspace",
                "/work/other-project",
                "--worker",
                "explicit-worker",
                "--profile",
                "project:companion",
            ]
        );
    }

    #[test]
    fn runtime_args_use_session_mode_without_profile_identity_alias() {
        let mut config = base_config();
        config.resume_from = Some(Uuid::nil());
        assert_eq!(
            runtime_args(&config, &WorkerProcessLaunchOptions::default()),
            vec![
                "--workspace",
                "/work/other-project",
                "--session",
                "00000000-0000-0000-0000-000000000000",
                "--worker",
                "explicit-worker",
            ]
        );
    }

    #[test]
    fn runtime_args_include_upper_resolver_extra_args_without_child_cwd() {
        let mut config = base_config();
        config.cwd = Some(PathBuf::from("/work/main/.worktree/orchestration/yoi"));

        assert_eq!(
            runtime_args(
                &config,
                &WorkerProcessLaunchOptions::default()
                    .with_hidden_arg("--ticket-role", "orchestrator"),
            ),
            vec![
                "--workspace",
                "/work/other-project",
                "--worker",
                "explicit-worker",
                "--profile",
                "project:companion",
                "--ticket-role",
                "orchestrator",
            ]
        );
    }
}
