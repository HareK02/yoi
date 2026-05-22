//! pod バイナリをサブプロセスとして立ち上げ、`INSOMNIA-READY` を待つ
//! ハンドシェイク。
//!
//! - 親プロセス (TUI / GUI / E2E) は overlay TOML を組み立ててこの関数に
//!   渡す。pod はそれを受けて socket を bind し、stderr に
//!   `INSOMNIA-READY\t<name>\t<socket>` を吐く。
//! - 待機中の stderr 行は `progress` コールバック越しに呼び出し側へ流す。
//!   UI の進捗表示や E2E のログ収集はここで賄う。
//! - `kill_on_drop = false` + `process_group(0)` により、親プロセス
//!   ライフサイクルから切り離した detached pod を作る。ready 後の lifecycle
//!   管理は runtime ディレクトリ / socket を介して行う。

use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;
use uuid::Uuid;

const READY_PREFIX: &str = "INSOMNIA-READY\t";
const READY_TIMEOUT: Duration = Duration::from_secs(20);

/// `spawn_pod` の入力。
pub struct SpawnConfig {
    /// `pod.name` として使う識別子。runtime ディレクトリ
    /// (`manifest::paths::pod_runtime_dir`) の解決と、ready 行に乗る
    /// 名前との突き合わせに使う。
    pub pod_name: String,
    /// `--overlay` で pod に渡す TOML 文字列。
    pub overlay_toml: String,
    /// pod の current_dir。
    pub cwd: PathBuf,
    /// `Some(id)` のとき `--session <id>` を付与し、当該セッションから
    /// resume させる。
    pub resume_from: Option<Uuid>,
    /// true のとき `--pod <pod_name>` を付与し、pod 側で name-keyed state
    /// があれば resume、なければ同名の新規 Pod として起動させる。
    pub resume_by_pod_name: bool,
}

pub struct SpawnReady {
    pub pod_name: String,
    pub socket_path: PathBuf,
}

#[derive(Debug)]
pub enum SpawnError {
    Io(io::Error),
    /// runtime ディレクトリが解決できなかった (環境変数未設定等)。
    RuntimeDirUnavailable,
    PodLaunchFailed(io::Error),
    PodExitedEarly {
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
                "could not resolve runtime directory (set INSOMNIA_HOME, INSOMNIA_RUNTIME_DIR, XDG_RUNTIME_DIR, or HOME)"
            ),
            Self::PodLaunchFailed(e) => write!(f, "failed to launch pod: {e}"),
            Self::PodExitedEarly { stderr_tail } => {
                if stderr_tail.is_empty() {
                    write!(f, "pod exited before becoming ready")
                } else {
                    write!(f, "pod exited before becoming ready: {stderr_tail}")
                }
            }
            Self::Timeout => write!(
                f,
                "pod did not become ready within {}s",
                READY_TIMEOUT.as_secs()
            ),
        }
    }
}

impl std::error::Error for SpawnError {}

impl From<io::Error> for SpawnError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// pod を spawn し、`INSOMNIA-READY` ハンドシェイクが終わるまで待つ。
///
/// `progress` は ready 行を見つけるまでに観測した stderr の各行で呼ばれる
/// (ready 行自体は除外される)。UI の表示更新や E2E ログ取得に使う。
pub async fn spawn_pod<F>(config: SpawnConfig, mut progress: F) -> Result<SpawnReady, SpawnError>
where
    F: FnMut(&str),
{
    let pod_bin = resolve_pod_command();

    let pod_runtime_dir = manifest::paths::pod_runtime_dir(&config.pod_name)
        .ok_or(SpawnError::RuntimeDirUnavailable)?;
    std::fs::create_dir_all(&pod_runtime_dir).map_err(SpawnError::Io)?;
    let stderr_path = pod_runtime_dir.join("stderr.log");
    let stderr_file = std::fs::File::create(&stderr_path).map_err(SpawnError::Io)?;

    let mut command = Command::new(&pod_bin);
    command
        .arg("--overlay")
        .arg(&config.overlay_toml)
        .current_dir(&config.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr_file))
        .process_group(0);
    if config.resume_by_pod_name {
        command.arg("--pod").arg(&config.pod_name);
    }
    if let Some(id) = config.resume_from {
        command.arg("--session").arg(id.to_string());
    }
    let mut child = command.spawn().map_err(SpawnError::PodLaunchFailed)?;

    // Default `kill_on_drop = false` plus `process_group(0)` makes this
    // a detached Pod once startup succeeds: dropping the handle does not
    // terminate it, and terminal-generated signals for the parent's
    // process group do not hit the Pod. Runtime state/socket files are
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
                    let pod_name = parts.next().unwrap_or("").to_string();
                    let socket_str = parts.next().unwrap_or("").to_string();
                    if pod_name.is_empty() || socket_str.is_empty() {
                        return Err(SpawnError::PodExitedEarly {
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
                        pod_name,
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
                // Pod は exit 直前に最終 stderr 行を flush することがある。
                // child.wait() が解決した後に再読みして、原因行を取りこ
                // ぼさず PodExitedEarly に載せる。
                drain_stderr_into_tail(stderr_path, &mut tail, &mut offset).await;
                return Err(SpawnError::PodExitedEarly {
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
                return Err(SpawnError::PodExitedEarly {
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

/// Resolves the binary used to launch a child Pod. Must point at a
/// `pod`-compatible executable — the parent reads the child's stderr
/// directly looking for `INSOMNIA-READY`, so any wrapper that emits
/// extra lines on stderr will pollute that handshake.
///
/// `INSOMNIA_POD_COMMAND` overrides the lookup (used by tests to inject
/// a mock binary). Otherwise we defer to `PATH` — missing binary
/// surfaces as the spawn `io::Error`.
fn resolve_pod_command() -> PathBuf {
    if let Ok(cmd) = std::env::var("INSOMNIA_POD_COMMAND")
        && !cmd.is_empty()
    {
        return PathBuf::from(cmd);
    }
    PathBuf::from("pod")
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
