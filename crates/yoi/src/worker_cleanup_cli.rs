use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use manifest::paths;
use pod_store::{FsWorkerStore, WorkerMetadata, WorkerMetadataStore, validate_worker_name};

const MAX_REPORT_ITEMS: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerCleanupCli {
    Help,
    Delete(WorkerDeleteOptions),
    Prune(WorkerPruneOptions),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerDeleteOptions {
    pub name: String,
    pub force: bool,
    pub dry_run: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerPruneOptions {
    pub older_than: Duration,
    pub force: bool,
    pub dry_run: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerCleanupCliOutput {
    pub stdout: String,
    pub status: WorkerCleanupCliStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerCleanupCliStatus {
    Success,
    Failure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerCleanupCliError(String);

impl fmt::Display for WorkerCleanupCliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for WorkerCleanupCliError {}

pub fn parse_worker_management_args(
    args: &[String],
) -> Result<Option<WorkerCleanupCli>, WorkerCleanupCliError> {
    let Some((subcommand, rest)) = args.split_first() else {
        return Ok(None);
    };
    match subcommand.as_str() {
        "delete" => parse_delete_args(rest)
            .map(WorkerCleanupCli::Delete)
            .map(Some),
        "prune" => parse_prune_args(rest)
            .map(WorkerCleanupCli::Prune)
            .map(Some),
        "help" => Ok(Some(WorkerCleanupCli::Help)),
        "--help" | "-h" => Ok(Some(WorkerCleanupCli::Help)),
        _ => Ok(None),
    }
}

fn parse_delete_args(args: &[String]) -> Result<WorkerDeleteOptions, WorkerCleanupCliError> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        return Err(WorkerCleanupCliError(delete_help_text().to_string()));
    }
    let mut name = None;
    let mut force = false;
    let mut dry_run = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--force" => force = true,
            "--dry-run" => dry_run = true,
            "--" => {
                for positional in iter {
                    set_name(&mut name, positional)?;
                }
                break;
            }
            value if value.starts_with('-') => {
                return Err(WorkerCleanupCliError(format!(
                    "unknown yoi worker delete option `{value}`"
                )));
            }
            positional => set_name(&mut name, positional)?,
        }
    }
    let name = name.ok_or_else(|| {
        WorkerCleanupCliError("yoi worker delete requires an explicit Worker name".into())
    })?;
    validate_worker_name(&name).map_err(|e| WorkerCleanupCliError(e.to_string()))?;
    Ok(WorkerDeleteOptions {
        name,
        force,
        dry_run,
    })
}

fn set_name(name: &mut Option<String>, value: &str) -> Result<(), WorkerCleanupCliError> {
    if name.is_some() {
        return Err(WorkerCleanupCliError(
            "yoi worker delete accepts exactly one Worker name".into(),
        ));
    }
    *name = Some(value.to_string());
    Ok(())
}

fn parse_prune_args(args: &[String]) -> Result<WorkerPruneOptions, WorkerCleanupCliError> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        return Err(WorkerCleanupCliError(prune_help_text().to_string()));
    }
    let mut older_than = None;
    let mut force = false;
    let mut dry_run = false;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--force" {
            force = true;
            index += 1;
        } else if arg == "--dry-run" {
            dry_run = true;
            index += 1;
        } else if arg == "--older-than" {
            let value = args.get(index + 1).ok_or_else(|| {
                WorkerCleanupCliError("--older-than requires a duration value".into())
            })?;
            if value.starts_with('-') {
                return Err(WorkerCleanupCliError(
                    "--older-than requires a duration value".into(),
                ));
            }
            older_than = Some(parse_duration(value)?);
            index += 2;
        } else if let Some(value) = arg.strip_prefix("--older-than=") {
            if value.is_empty() {
                return Err(WorkerCleanupCliError(
                    "--older-than requires a duration value".into(),
                ));
            }
            older_than = Some(parse_duration(value)?);
            index += 1;
        } else if arg.starts_with('-') {
            return Err(WorkerCleanupCliError(format!(
                "unknown yoi worker prune option `{arg}`"
            )));
        } else {
            return Err(WorkerCleanupCliError(format!(
                "yoi worker prune does not accept positional argument `{arg}`"
            )));
        }
    }
    let older_than = older_than.ok_or_else(|| {
        WorkerCleanupCliError("yoi worker prune requires --older-than <DURATION>".into())
    })?;
    Ok(WorkerPruneOptions {
        older_than,
        force,
        dry_run,
    })
}

pub fn parse_duration(value: &str) -> Result<Duration, WorkerCleanupCliError> {
    let split = value
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(value.len());
    let (amount, unit) = value.split_at(split);
    if amount.is_empty() || unit.is_empty() {
        return Err(WorkerCleanupCliError(format!(
            "duration `{value}` must use an explicit unit: s, m, h, d, or w"
        )));
    }
    let amount = amount
        .parse::<u64>()
        .map_err(|_| WorkerCleanupCliError(format!("invalid duration amount `{value}`")))?;
    if amount == 0 {
        return Err(WorkerCleanupCliError(
            "duration must be greater than zero".into(),
        ));
    }
    let seconds = match unit {
        "s" | "sec" | "secs" | "second" | "seconds" => amount,
        "m" | "min" | "mins" | "minute" | "minutes" => amount.saturating_mul(60),
        "h" | "hr" | "hrs" | "hour" | "hours" => amount.saturating_mul(60 * 60),
        "d" | "day" | "days" => amount.saturating_mul(60 * 60 * 24),
        "w" | "week" | "weeks" => amount.saturating_mul(60 * 60 * 24 * 7),
        _ => {
            return Err(WorkerCleanupCliError(format!(
                "unknown duration unit `{unit}` in `{value}`"
            )));
        }
    };
    Ok(Duration::from_secs(seconds))
}

pub async fn run(cli: WorkerCleanupCli) -> Result<WorkerCleanupCliOutput, WorkerCleanupCliError> {
    let data_dir = paths::data_dir()
        .ok_or_else(|| WorkerCleanupCliError("failed to resolve Yoi data directory".into()))?;
    let runtime_dir = paths::runtime_dir()
        .ok_or_else(|| WorkerCleanupCliError("failed to resolve Yoi runtime directory".into()))?;
    run_with_roots(cli, data_dir, runtime_dir).await
}

pub async fn run_with_roots(
    cli: WorkerCleanupCli,
    data_dir: PathBuf,
    runtime_dir: PathBuf,
) -> Result<WorkerCleanupCliOutput, WorkerCleanupCliError> {
    match cli {
        WorkerCleanupCli::Help => Ok(WorkerCleanupCliOutput {
            stdout: help_text().to_string(),
            status: WorkerCleanupCliStatus::Success,
        }),
        WorkerCleanupCli::Delete(options) => run_delete(options, data_dir, runtime_dir).await,
        WorkerCleanupCli::Prune(options) => run_prune(options, data_dir, runtime_dir).await,
    }
}

async fn run_delete(
    options: WorkerDeleteOptions,
    data_dir: PathBuf,
    runtime_dir: PathBuf,
) -> Result<WorkerCleanupCliOutput, WorkerCleanupCliError> {
    let store = FsWorkerStore::new(data_dir.join("pods")).map_err(to_error)?;
    let metadata = store.read_by_name(&options.name).map_err(to_error)?;
    let Some(metadata) = metadata else {
        return Ok(WorkerCleanupCliOutput {
            stdout: format!(
                "yoi worker delete\nstatus: refused\npod: {}\nreason: worker metadata is missing\n",
                options.name
            ),
            status: WorkerCleanupCliStatus::Failure,
        });
    };

    let probe = probe_worker_liveness(&runtime_dir, &options.name).await;
    if let Some(reason) = probe.refusal_reason() {
        return Ok(WorkerCleanupCliOutput {
            stdout: format!(
                "yoi worker delete\nstatus: refused\npod: {}\nreason: {}\nsocket: {}\n",
                options.name,
                reason,
                probe.socket_path.display()
            ),
            status: WorkerCleanupCliStatus::Failure,
        });
    }

    let delete = options.force && !options.dry_run;
    let mut stdout = String::new();
    stdout.push_str("yoi worker delete\n");
    stdout.push_str(if delete {
        "mode: force\n"
    } else {
        "mode: dry-run\n"
    });
    stdout.push_str(&format!("worker: {}\n", options.name));
    describe_metadata(&mut stdout, &metadata);
    if delete {
        store.delete_by_name(&options.name).map_err(to_error)?;
        stdout.push_str("deleted: worker metadata\n");
        stdout.push_str("preserved: session logs/history\n");
    } else {
        stdout.push_str("would_delete: worker metadata\n");
        stdout.push_str("would_preserve: session logs/history\n");
        stdout
            .push_str("note: pass --force to delete metadata; --dry-run keeps report-only mode\n");
    }
    Ok(WorkerCleanupCliOutput {
        stdout,
        status: WorkerCleanupCliStatus::Success,
    })
}

async fn run_prune(
    options: WorkerPruneOptions,
    data_dir: PathBuf,
    runtime_dir: PathBuf,
) -> Result<WorkerCleanupCliOutput, WorkerCleanupCliError> {
    let store = FsWorkerStore::new(data_dir.join("pods")).map_err(to_error)?;
    let names = store.list_names().map_err(to_error)?;
    let cutoff = SystemTime::now()
        .checked_sub(options.older_than)
        .ok_or_else(|| WorkerCleanupCliError("--older-than duration is too large".into()))?;
    let delete = options.force && !options.dry_run;
    let mut stdout = String::new();
    stdout.push_str("yoi worker prune\n");
    stdout.push_str(if delete {
        "mode: force\n"
    } else {
        "mode: dry-run\n"
    });
    stdout.push_str(&format!("older_than: {:?}\n", options.older_than));

    let mut deleted = 0usize;
    let mut would_delete = 0usize;
    let mut kept = 0usize;
    let mut refused = 0usize;
    for (index, name) in names.iter().enumerate() {
        let metadata = store.read_by_name(name).map_err(to_error)?;
        let Some(metadata) = metadata else {
            kept += 1;
            push_item_line(&mut stdout, index, "kept", name, "metadata disappeared");
            continue;
        };
        let modified = metadata_modified_at(store.root_dir().as_deref(), name).map_err(to_error)?;
        let Some(modified) = modified else {
            refused += 1;
            push_item_line(
                &mut stdout,
                index,
                "refused",
                name,
                "metadata mtime is unavailable",
            );
            continue;
        };
        if modified > cutoff {
            kept += 1;
            push_item_line(
                &mut stdout,
                index,
                "kept",
                name,
                "metadata is newer than threshold",
            );
            continue;
        }
        let probe = probe_worker_liveness(&runtime_dir, name).await;
        if let Some(reason) = probe.refusal_reason() {
            refused += 1;
            push_item_line(&mut stdout, index, "refused", name, &reason);
            continue;
        }
        if delete {
            store.delete_by_name(name).map_err(to_error)?;
            deleted += 1;
            push_item_line(
                &mut stdout,
                index,
                "deleted",
                name,
                "old worker metadata; session logs/history preserved",
            );
        } else {
            would_delete += 1;
            let reason = metadata
                .active
                .as_ref()
                .map(|active| format!("old metadata; active_session={}", active.session_id))
                .unwrap_or_else(|| "old metadata; no active session".to_string());
            push_item_line(&mut stdout, index, "would_delete", name, &reason);
        }
    }
    stdout.push_str(&format!(
        "summary: deleted={deleted} would_delete={would_delete} kept={kept} refused={refused}\n"
    ));
    if !delete {
        stdout
            .push_str("note: pass --force to delete metadata; --dry-run keeps report-only mode\n");
    }
    Ok(WorkerCleanupCliOutput {
        stdout,
        status: if refused > 0 {
            WorkerCleanupCliStatus::Failure
        } else {
            WorkerCleanupCliStatus::Success
        },
    })
}

fn describe_metadata(stdout: &mut String, metadata: &WorkerMetadata) {
    match metadata.active.as_ref() {
        Some(active) => stdout.push_str(&format!(
            "active_session: {}\nactive_segment: {}\n",
            active.session_id,
            active
                .segment_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "<pending>".to_string())
        )),
        None => stdout.push_str("active_session: <none>\n"),
    }
}

fn metadata_modified_at(
    root: Option<&Path>,
    worker_name: &str,
) -> Result<Option<SystemTime>, io::Error> {
    let Some(root) = root else {
        return Ok(None);
    };
    let path = root.join(worker_name).join("metadata.json");
    match std::fs::metadata(path) {
        Ok(metadata) => metadata.modified().map(Some),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

fn push_item_line(stdout: &mut String, index: usize, action: &str, name: &str, reason: &str) {
    if index < MAX_REPORT_ITEMS {
        stdout.push_str(&format!("{action}: {name} ({reason})\n"));
    } else if index == MAX_REPORT_ITEMS {
        stdout.push_str("... additional items omitted from bounded report ...\n");
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LivenessProbe {
    socket_path: PathBuf,
    result: LivenessResult,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LivenessResult {
    NotReachable,
    Reachable,
    Uncertain(String),
}

impl LivenessProbe {
    fn refusal_reason(&self) -> Option<String> {
        match &self.result {
            LivenessResult::NotReachable => None,
            LivenessResult::Reachable => Some("worker is live/reachable".into()),
            LivenessResult::Uncertain(reason) => Some(format!(
                "worker liveness is uncertain; refusing destructive metadata cleanup ({reason})"
            )),
        }
    }
}

async fn probe_worker_liveness(runtime_dir: &Path, worker_name: &str) -> LivenessProbe {
    let socket_path = runtime_dir.join(worker_name).join("sock");
    let result = probe_socket(&socket_path).await;
    LivenessProbe {
        socket_path,
        result,
    }
}

#[cfg(unix)]
async fn probe_socket(socket_path: &Path) -> LivenessResult {
    use std::os::unix::net::UnixStream;

    let path = socket_path.to_path_buf();
    match tokio::task::spawn_blocking(move || UnixStream::connect(path)).await {
        Ok(Ok(_stream)) => LivenessResult::Reachable,
        Ok(Err(error)) if is_not_live_socket_error(&error) => LivenessResult::NotReachable,
        Ok(Err(error)) => LivenessResult::Uncertain(error.to_string()),
        Err(error) => LivenessResult::Uncertain(error.to_string()),
    }
}

#[cfg(unix)]
fn is_not_live_socket_error(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
    )
}

#[cfg(not(unix))]
async fn probe_socket(_socket_path: &Path) -> LivenessResult {
    LivenessResult::Uncertain("Unix socket probing is unavailable on this platform".into())
}

fn to_error<E: fmt::Display>(error: E) -> WorkerCleanupCliError {
    WorkerCleanupCliError(error.to_string())
}

pub fn help_text() -> &'static str {
    "yoi worker\n\nUsage:\n  yoi worker delete <NAME> [--force] [--dry-run]\n  yoi worker prune --older-than <DURATION> [--force] [--dry-run]\n  yoi worker [WORKER_OPTIONS]\n\nDescription:\n  delete/prune are safe Worker metadata cleanup commands. `worker delete` removes only name-keyed Worker metadata and never removes session logs/history. Live or uncertain Worker liveness is refused. Without --force the command reports only.\n\nDuration units: s, m, h, d, w\n\nOptions:\n      --force       Perform deletion after safety checks\n      --dry-run     Report only, even with --force\n      --older-than  Required explicit age threshold for prune\n  -h, --help        Print help\n"
}

fn delete_help_text() -> &'static str {
    "usage: yoi worker delete <NAME> [--force] [--dry-run]"
}

fn prune_help_text() -> &'static str {
    "usage: yoi worker prune --older-than <DURATION> [--force] [--dry-run]"
}

#[cfg(test)]
mod tests {
    use super::*;
    use pod_store::WorkerActiveSegmentRef;
    use session_store::{Store, new_segment_id, new_session_id};

    fn string_args(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| arg.to_string()).collect()
    }

    #[test]
    fn parse_worker_delete_command() {
        let cli = parse_worker_management_args(&string_args(&[
            "delete",
            "agent",
            "--force",
            "--dry-run",
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(
            cli,
            WorkerCleanupCli::Delete(WorkerDeleteOptions {
                name: "agent".into(),
                force: true,
                dry_run: true,
            })
        );
    }

    #[test]
    fn parse_worker_prune_requires_explicit_threshold() {
        let err = parse_worker_management_args(&string_args(&["prune"])).unwrap_err();
        assert!(err.to_string().contains("--older-than"));
    }

    #[test]
    fn parse_duration_requires_units() {
        let err = parse_duration("30").unwrap_err();
        assert!(err.to_string().contains("explicit unit"));
        assert_eq!(parse_duration("2d").unwrap(), Duration::from_secs(172_800));
    }

    #[tokio::test]
    async fn stopped_pod_delete_force_removes_only_metadata() {
        let tmp = tempfile::TempDir::new().unwrap();
        let data_dir = tmp.path().join("data");
        let runtime_dir = tmp.path().join("run");
        let pod_store = FsWorkerStore::new(data_dir.join("pods")).unwrap();
        let session_store = session_store::FsStore::new(data_dir.join("sessions")).unwrap();
        let session_id = new_session_id();
        let segment_id = new_segment_id();
        session_store
            .create_segment(session_id, segment_id, &[])
            .unwrap();
        pod_store
            .write(&WorkerMetadata::new(
                "agent",
                Some(WorkerActiveSegmentRef::active_segment(
                    session_id, segment_id,
                )),
            ))
            .unwrap();

        let output = run_with_roots(
            WorkerCleanupCli::Delete(WorkerDeleteOptions {
                name: "agent".into(),
                force: true,
                dry_run: false,
            }),
            data_dir.clone(),
            runtime_dir,
        )
        .await
        .unwrap();

        assert_eq!(output.status, WorkerCleanupCliStatus::Success);
        assert!(output.stdout.contains("deleted: worker metadata"));
        assert!(pod_store.read_by_name("agent").unwrap().is_none());
        assert!(session_store.exists(session_id, segment_id).unwrap());
    }

    #[tokio::test]
    async fn pod_delete_without_force_reports_dry_run() {
        let tmp = tempfile::TempDir::new().unwrap();
        let data_dir = tmp.path().join("data");
        let runtime_dir = tmp.path().join("run");
        let pod_store = FsWorkerStore::new(data_dir.join("pods")).unwrap();
        pod_store
            .write(&WorkerMetadata::new("agent", None))
            .unwrap();

        let output = run_with_roots(
            WorkerCleanupCli::Delete(WorkerDeleteOptions {
                name: "agent".into(),
                force: false,
                dry_run: false,
            }),
            data_dir,
            runtime_dir,
        )
        .await
        .unwrap();

        assert_eq!(output.status, WorkerCleanupCliStatus::Success);
        assert!(output.stdout.contains("mode: dry-run"));
        assert!(pod_store.read_by_name("agent").unwrap().is_some());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn live_pod_delete_is_refused() {
        use std::os::unix::net::UnixListener;

        let tmp = tempfile::TempDir::new().unwrap();
        let data_dir = tmp.path().join("data");
        let runtime_dir = tmp.path().join("run");
        let pod_store = FsWorkerStore::new(data_dir.join("pods")).unwrap();
        pod_store
            .write(&WorkerMetadata::new("agent", None))
            .unwrap();
        std::fs::create_dir_all(runtime_dir.join("agent")).unwrap();
        let listener = UnixListener::bind(runtime_dir.join("agent/sock")).unwrap();

        let output = run_with_roots(
            WorkerCleanupCli::Delete(WorkerDeleteOptions {
                name: "agent".into(),
                force: true,
                dry_run: false,
            }),
            data_dir,
            runtime_dir,
        )
        .await
        .unwrap();

        drop(listener);
        assert_eq!(output.status, WorkerCleanupCliStatus::Failure);
        assert!(output.stdout.contains("status: refused"));
        assert!(pod_store.read_by_name("agent").unwrap().is_some());
    }
}
