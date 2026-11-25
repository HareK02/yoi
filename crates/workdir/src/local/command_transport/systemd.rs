//! A scope is created by the user manager outside the caller's tmux unit.
//! systemd-run --scope preserves cwd, environment and inherited descriptors, and
//! joins the scope before executing our fixed preflight wrapper. FD 3 acknowledges
//! verified membership and limits, then closes before arbitrary command code runs.
//! Failed scopes retain Result=oom-kill until we observe and reset them. Exit 137
//! alone is never attribution. No host policy is modified and no command is retried.

use super::*;
use crate::{CommandOomObservation as Oom, CommandResourceOutcome as Outcome};
use std::os::fd::AsRawFd;
use tokio::net::UnixStream;

const CONTROL_TIMEOUT: Duration = Duration::from_secs(3);
const WRAPPER: &str = r#"
fail() { printf '%s\n' "$1" >&2; exit 125; }
read -r membership 2>/dev/null </proc/self/cgroup || fail 'Workdir isolation: cgroup v2 membership unavailable'
cg=${membership#0::}
case "$cg" in */app.slice/"$1") ;; *) fail 'Workdir isolation: command is not in the expected sibling scope' ;; esac
base=/sys/fs/cgroup$cg
read -r limit 2>/dev/null <"$base/memory.max" || fail 'Workdir isolation: memory controller unavailable'
read -r swap 2>/dev/null <"$base/memory.swap.max" || fail 'Workdir isolation: swap controller unavailable'
read -r group 2>/dev/null <"$base/memory.oom.group" || fail 'Workdir isolation: OOM group unavailable'
read -r aggregate 2>/dev/null <"${base%/*}/memory.oom.group" || fail 'Workdir isolation: parent OOM group unavailable'
[ "$limit" = "$2" ] && [ "$swap" = 0 ] && [ "$group" = 1 ] && [ "$aggregate" = 0 ] || fail 'Workdir isolation: memory limits or OOM group policy do not match'
printf Y >&3 || exit 125
exec 3>&-
exec bash -c "$3"
"#;

pub(super) struct Scope {
    unit: String,
    ready: UnixStream,
    armed: bool,
}

async fn control(args: &[&str], user: bool) -> std::io::Result<std::process::Output> {
    let mut command = Command::new("systemctl");
    command.arg("--no-pager").arg("--no-ask-password");
    if user {
        command.arg("--user");
    }
    command.args(args).stdin(Stdio::null()).kill_on_drop(true);
    tokio::time::timeout(CONTROL_TIMEOUT, command.output())
        .await
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::TimedOut))?
}

fn property<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    text.lines()
        .find_map(|line| line.strip_prefix(name).and_then(|s| s.strip_prefix('=')))
}

fn invalid() -> std::io::Error {
    std::io::Error::from(std::io::ErrorKind::Unsupported)
}

pub(super) async fn prepare(
    memory: u64,
    script: &str,
    timeout_secs: u64,
) -> std::io::Result<(Command, Scope)> {
    // A sibling scope cannot protect the common user manager if its owning system
    // service propagates descendant OOM. Fail closed rather than editing policy.
    let uid = unsafe { libc::geteuid() };
    let owner = format!("user@{uid}.service");
    let output = control(
        &["show", &owner, "-p", "OOMPolicy", "-p", "ControlGroup"],
        false,
    )
    .await?;
    let text = std::str::from_utf8(&output.stdout).map_err(|_| invalid())?;
    let common = property(text, "ControlGroup")
        .filter(|v| !v.is_empty())
        .ok_or_else(invalid)?;
    let membership = tokio::fs::read_to_string("/proc/self/cgroup").await?;
    let current = membership.trim().strip_prefix("0::").ok_or_else(invalid)?;
    if !output.status.success()
        || property(text, "OOMPolicy") != Some("continue")
        || !current
            .strip_prefix(common)
            .is_some_and(|s| s.starts_with('/'))
    {
        return Err(invalid());
    }
    let mut path = PathBuf::from("/sys/fs/cgroup").join(common.trim_start_matches('/'));
    while path != Path::new("/sys/fs/cgroup") {
        if tokio::fs::read_to_string(path.join("memory.oom.group"))
            .await?
            .trim()
            != "0"
        {
            return Err(invalid());
        }
        if !path.pop() {
            return Err(invalid());
        }
    }
    let unit = format!("yoi-command-{}.scope", uuid::Uuid::now_v7().simple());
    let (parent, child) = std::os::unix::net::UnixStream::pair()?;
    parent.set_nonblocking(true)?;
    let ready = UnixStream::from_std(parent)?;
    let mut command = Command::new("systemd-run");
    command
        .args([
            "--user",
            "--scope",
            "--quiet",
            "--no-ask-password",
            "--expand-environment=no",
            "--slice=app.slice",
        ])
        .arg(format!("--unit={unit}"))
        .arg(format!("--property=MemoryMax={memory}"))
        // A host crash/aborted supervisor must not leave an indefinitely running
        // scope. Normal timeout/cancel still uses the provider's existing contract.
        .arg(format!(
            "--property=RuntimeMaxSec={}",
            timeout_secs.max(1).saturating_add(20)
        ))
        .args([
            "--property=MemorySwapMax=0",
            "--property=OOMPolicy=kill",
            "--property=KillMode=control-group",
            "--property=CollectMode=inactive",
        ])
        .args([
            "bash",
            "-c",
            WRAPPER,
            "yoi-command",
            &unit,
            &memory.to_string(),
            script,
        ]);
    // Only async-signal-safe syscalls run between fork and exec. Capturing the
    // descriptor keeps it alive through spawn; the wrapper closes it before user code.
    unsafe {
        command.pre_exec(move || {
            if child.as_raw_fd() != 3 && libc::dup2(child.as_raw_fd(), 3) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::fcntl(3, libc::F_SETFD, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok((
        command,
        Scope {
            unit,
            ready,
            armed: true,
        },
    ))
}

impl Scope {
    pub(super) async fn finish(&mut self) -> Outcome {
        let mut byte = [0];
        let verified = matches!(tokio::time::timeout(CONTROL_TIMEOUT, self.ready.read(&mut byte)).await,
            Ok(Ok(1)) if byte == [b'Y']);
        let observed = control(
            &[
                "show",
                &self.unit,
                "-p",
                "LoadState",
                "-p",
                "ActiveState",
                "-p",
                "Result",
            ],
            true,
        )
        .await;
        let oom = match observed {
            Ok(output) if output.status.success() => {
                let text = String::from_utf8_lossy(&output.stdout);
                match (
                    property(&text, "LoadState"),
                    property(&text, "Result"),
                    property(&text, "ActiveState"),
                ) {
                    (Some("loaded"), Some("oom-kill"), _) => Oom::SystemdOomKill,
                    (Some("loaded"), Some(_), Some("inactive" | "failed")) => Oom::NotObserved,
                    _ => Oom::Unknown,
                }
            }
            _ => Oom::Unknown,
        };
        // Detached descendants may ignore TERM. Kill the entire unit before a
        // stop job, rather than waiting for the manager's potentially long default.
        let _ = control(
            &["kill", "--signal=KILL", "--kill-whom=all", &self.unit],
            true,
        )
        .await;
        let stopped = control(&["stop", &self.unit], true).await;
        let state = control(
            &["show", &self.unit, "-p", "LoadState", "-p", "ActiveState"],
            true,
        )
        .await;
        let cleanup_confirmed = state.is_ok_and(|output| {
            let text = String::from_utf8_lossy(&output.stdout);
            property(&text, "LoadState") == Some("not-found")
                || (stopped.as_ref().is_ok_and(|o| o.status.success())
                    && matches!(property(&text, "ActiveState"), Some("inactive" | "failed")))
        });
        if cleanup_confirmed {
            // Reset only our exact, randomly named scope, after retaining evidence.
            let _ = control(&["reset-failed", &self.unit], true).await;
            self.armed = false;
        }
        if !verified {
            Outcome::SetupUnconfirmed {
                oom,
                cleanup_confirmed,
            }
        } else {
            Outcome::Systemd {
                oom,
                cleanup_confirmed,
            }
        }
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        if self.armed {
            // Abort is not normal terminal delivery. Request unit cleanup even if
            // the supervisor future is dropped; never claim confirmed completion.
            if let Ok(mut child) = std::process::Command::new("systemctl")
                .args([
                    "--user",
                    "--no-block",
                    "--no-ask-password",
                    "stop",
                    &self.unit,
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            }
        }
    }
}
