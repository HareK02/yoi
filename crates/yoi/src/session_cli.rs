use std::collections::BTreeSet;
use std::fmt;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use manifest::paths;
use session_store::{FsStore, SessionId, Store};
use session_store::{FsWorkerStore, WorkerMetadataStore};

use crate::worker_cleanup_cli::parse_duration;

const MAX_REPORT_ITEMS: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionCli {
    Help,
    Analyze(SessionAnalyzeOptions),
    Prune(SessionPruneOptions),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionAnalyzeOptions {
    pub path: PathBuf,
    pub json: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPruneOptions {
    pub unreferenced: bool,
    pub older_than: Option<Duration>,
    pub force: bool,
    pub dry_run: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCliOutput {
    pub stdout: String,
    pub status: SessionCliStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionCliStatus {
    Success,
    Failure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCliError(String);

impl fmt::Display for SessionCliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SessionCliError {}

pub fn parse_session_args(args: &[String]) -> Result<SessionCli, SessionCliError> {
    if args.is_empty() || args.iter().any(|arg| arg == "--help" || arg == "-h") {
        return Ok(SessionCli::Help);
    }
    match args[0].as_str() {
        "analyze" => parse_analyze_args(&args[1..]).map(SessionCli::Analyze),
        "prune" => parse_prune_args(&args[1..]).map(SessionCli::Prune),
        other => Err(SessionCliError(format!(
            "unknown yoi session command `{other}`"
        ))),
    }
}

fn parse_analyze_args(args: &[String]) -> Result<SessionAnalyzeOptions, SessionCliError> {
    let mut path = None;
    let mut json = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--json" => json = true,
            "--" => {
                for positional in iter {
                    set_path(&mut path, positional)?;
                }
                break;
            }
            value if value.starts_with('-') => {
                return Err(SessionCliError(format!(
                    "unknown yoi session analyze option `{value}`"
                )));
            }
            positional => set_path(&mut path, positional)?,
        }
    }
    let path = path.ok_or_else(|| {
        SessionCliError("yoi session analyze requires an explicit session JSONL path".into())
    })?;
    if !json {
        return Err(SessionCliError(
            "initial yoi session analyze output requires --json".into(),
        ));
    }
    Ok(SessionAnalyzeOptions { path, json })
}

fn parse_prune_args(args: &[String]) -> Result<SessionPruneOptions, SessionCliError> {
    let mut unreferenced = false;
    let mut older_than = None;
    let mut force = false;
    let mut dry_run = false;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--unreferenced" {
            unreferenced = true;
            index += 1;
        } else if arg == "--force" {
            force = true;
            index += 1;
        } else if arg == "--dry-run" {
            dry_run = true;
            index += 1;
        } else if arg == "--older-than" {
            let value = args
                .get(index + 1)
                .ok_or_else(|| SessionCliError("--older-than requires a duration value".into()))?;
            if value.starts_with('-') {
                return Err(SessionCliError(
                    "--older-than requires a duration value".into(),
                ));
            }
            older_than = Some(parse_duration(value).map_err(|e| SessionCliError(e.to_string()))?);
            index += 2;
        } else if let Some(value) = arg.strip_prefix("--older-than=") {
            if value.is_empty() {
                return Err(SessionCliError(
                    "--older-than requires a duration value".into(),
                ));
            }
            older_than = Some(parse_duration(value).map_err(|e| SessionCliError(e.to_string()))?);
            index += 1;
        } else if arg.starts_with('-') {
            return Err(SessionCliError(format!(
                "unknown yoi session prune option `{arg}`"
            )));
        } else {
            return Err(SessionCliError(format!(
                "yoi session prune does not accept positional argument `{arg}`"
            )));
        }
    }
    if !unreferenced {
        return Err(SessionCliError(
            "yoi session prune requires --unreferenced".into(),
        ));
    }
    Ok(SessionPruneOptions {
        unreferenced,
        older_than,
        force,
        dry_run,
    })
}

fn set_path(path: &mut Option<PathBuf>, value: &str) -> Result<(), SessionCliError> {
    if path.is_some() {
        return Err(SessionCliError(
            "yoi session analyze accepts exactly one path".into(),
        ));
    }
    *path = Some(PathBuf::from(value));
    Ok(())
}

pub fn run(cli: SessionCli) -> Result<SessionCliOutput, SessionCliError> {
    match cli {
        SessionCli::Help => Ok(SessionCliOutput {
            stdout: help_text().to_string(),
            status: SessionCliStatus::Success,
        }),
        SessionCli::Analyze(options) => {
            let report = session_analytics::analyze_session(&options.path)
                .map_err(|e| SessionCliError(e.to_string()))?;
            let stdout = serde_json::to_string_pretty(&report)
                .map_err(|e| SessionCliError(format!("failed to render JSON report: {e}")))?;
            Ok(SessionCliOutput {
                stdout: format!("{stdout}\n"),
                status: SessionCliStatus::Success,
            })
        }
        SessionCli::Prune(options) => {
            let data_dir = paths::data_dir()
                .ok_or_else(|| SessionCliError("failed to resolve Yoi data directory".into()))?;
            run_prune_with_roots(options, data_dir)
        }
    }
}

pub fn run_prune_with_roots(
    options: SessionPruneOptions,
    data_dir: PathBuf,
) -> Result<SessionCliOutput, SessionCliError> {
    if !options.unreferenced {
        return Err(SessionCliError(
            "yoi session prune requires --unreferenced".into(),
        ));
    }
    let session_store = FsStore::new(data_dir.join("sessions")).map_err(to_error)?;
    let worker_metadata_store = FsWorkerStore::new(data_dir.join("workers")).map_err(to_error)?;
    let referenced_sessions = referenced_sessions(&worker_metadata_store)?;
    let cutoff = options
        .older_than
        .map(|older_than| {
            SystemTime::now()
                .checked_sub(older_than)
                .ok_or_else(|| SessionCliError("--older-than duration is too large".into()))
        })
        .transpose()?;
    let delete = options.force && !options.dry_run;

    let mut deleted = 0usize;
    let mut would_delete = 0usize;
    let mut kept_referenced = 0usize;
    let mut kept_newer = 0usize;
    let mut refused = 0usize;
    let mut stdout = String::new();
    stdout.push_str("yoi session prune\n");
    stdout.push_str(if delete {
        "mode: force\n"
    } else {
        "mode: dry-run\n"
    });
    stdout.push_str("scope: unreferenced sessions\n");
    if let Some(older_than) = options.older_than {
        stdout.push_str(&format!("older_than: {older_than:?}\n"));
    }

    let sessions = session_store.list_sessions().map_err(to_error)?;
    for (index, session_id) in sessions.iter().enumerate() {
        if referenced_sessions.contains(session_id) {
            kept_referenced += 1;
            push_item_line(
                &mut stdout,
                index,
                "kept",
                *session_id,
                "referenced by worker metadata",
            );
            continue;
        }
        if let Some(cutoff) = cutoff {
            let modified = session_store
                .session_modified_at(*session_id)
                .map_err(to_error)?;
            match modified {
                Some(modified) if modified > cutoff => {
                    kept_newer += 1;
                    push_item_line(
                        &mut stdout,
                        index,
                        "kept",
                        *session_id,
                        "newer than threshold",
                    );
                    continue;
                }
                Some(_) => {}
                None => {
                    refused += 1;
                    push_item_line(
                        &mut stdout,
                        index,
                        "refused",
                        *session_id,
                        "session mtime is unavailable",
                    );
                    continue;
                }
            }
        }
        if delete {
            session_store
                .delete_session(*session_id)
                .map_err(to_error)?;
            deleted += 1;
            push_item_line(
                &mut stdout,
                index,
                "deleted",
                *session_id,
                "unreferenced session",
            );
        } else {
            would_delete += 1;
            push_item_line(
                &mut stdout,
                index,
                "would_delete",
                *session_id,
                "unreferenced session",
            );
        }
    }
    stdout.push_str(&format!(
        "summary: deleted={deleted} would_delete={would_delete} kept_referenced={kept_referenced} kept_newer={kept_newer} refused={refused}\n"
    ));
    if !delete {
        stdout
            .push_str("note: pass --force to delete sessions; --dry-run keeps report-only mode\n");
    }
    Ok(SessionCliOutput {
        stdout,
        status: if refused > 0 {
            SessionCliStatus::Failure
        } else {
            SessionCliStatus::Success
        },
    })
}

fn referenced_sessions(
    worker_metadata_store: &FsWorkerStore,
) -> Result<BTreeSet<SessionId>, SessionCliError> {
    let mut sessions = BTreeSet::new();
    for name in worker_metadata_store.list_names().map_err(to_error)? {
        let metadata = worker_metadata_store
            .read_by_name(&name)
            .map_err(to_error)?
            .ok_or_else(|| {
                SessionCliError(format!(
                    "worker metadata for `{name}` disappeared while checking references"
                ))
            })?;
        if let Some(active) = metadata.active {
            sessions.insert(active.session_id);
        }
    }
    Ok(sessions)
}

fn push_item_line(
    stdout: &mut String,
    index: usize,
    action: &str,
    session_id: SessionId,
    reason: &str,
) {
    if index < MAX_REPORT_ITEMS {
        stdout.push_str(&format!("{action}: {session_id} ({reason})\n"));
    } else if index == MAX_REPORT_ITEMS {
        stdout.push_str("... additional items omitted from bounded report ...\n");
    }
}

fn to_error<E: fmt::Display>(error: E) -> SessionCliError {
    SessionCliError(error.to_string())
}

pub fn help_text() -> &'static str {
    "yoi session\n\nUsage:\n  yoi session analyze <SESSION_JSONL_PATH> --json\n  yoi session prune --unreferenced [--older-than <DURATION>] [--force] [--dry-run]\n\nOptions:\n      --json          Emit a machine-readable JSON analytics report\n      --unreferenced  Prune only Sessions not referenced by Worker metadata\n      --older-than    Optional explicit age threshold for unreferenced cleanup (units: s, m, h, d, w)\n      --force         Perform deletion after safety checks\n      --dry-run       Report only, even with --force\n  -h, --help          Print help\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use session_store::{Store, new_segment_id, new_session_id};
    use session_store::{WorkerActiveSegmentRef, WorkerMetadata};
    use std::io::Write;

    #[test]
    fn parse_session_analyze_json() {
        let cli = parse_session_args(&[
            "analyze".to_string(),
            "/tmp/session.jsonl".to_string(),
            "--json".to_string(),
        ])
        .unwrap();
        assert_eq!(
            cli,
            SessionCli::Analyze(SessionAnalyzeOptions {
                path: PathBuf::from("/tmp/session.jsonl"),
                json: true,
            })
        );
    }

    #[test]
    fn parse_session_prune_unreferenced() {
        let cli = parse_session_args(&[
            "prune".to_string(),
            "--unreferenced".to_string(),
            "--older-than=2w".to_string(),
            "--dry-run".to_string(),
        ])
        .unwrap();
        assert_eq!(
            cli,
            SessionCli::Prune(SessionPruneOptions {
                unreferenced: true,
                older_than: Some(Duration::from_secs(14 * 24 * 60 * 60)),
                force: false,
                dry_run: true,
            })
        );
    }

    #[test]
    fn session_prune_requires_unreferenced() {
        let err = parse_session_args(&["prune".to_string()]).unwrap_err();
        assert!(err.to_string().contains("--unreferenced"));
    }

    #[test]
    fn run_session_analyze_outputs_json() {
        let mut fixture = tempfile::NamedTempFile::new().unwrap();
        let call = serde_json::json!({
            "kind":"assistant_item",
            "ts":1,
            "item":{
                "kind":"tool_call",
                "call_id":"r1",
                "name":"Read",
                "arguments":serde_json::json!({"file_path":"/tmp/a"}).to_string()
            }
        });
        writeln!(fixture, "{call}").unwrap();
        let output = run(SessionCli::Analyze(SessionAnalyzeOptions {
            path: fixture.path().to_path_buf(),
            json: true,
        }))
        .unwrap();
        assert_eq!(output.status, SessionCliStatus::Success);
        let value: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
        assert_eq!(value["tool_usage"]["total_tool_calls"], 1);
        assert_eq!(value["tool_usage"]["counts_by_tool"]["Read"], 1);
        assert_eq!(value["response_batches"]["total_responses"], 1);
        assert_eq!(value["response_batches"]["total_tool_calls"], 1);
        assert_eq!(
            value["response_batches"]["tools_per_response_histogram"][0]["tool_call_count"],
            1
        );
    }

    #[test]
    fn session_prune_unreferenced_preserves_active_pod_reference() {
        let tmp = tempfile::TempDir::new().unwrap();
        let data_dir = tmp.path().join("data");
        let session_store = FsStore::new(data_dir.join("sessions")).unwrap();
        let worker_metadata_store = FsWorkerStore::new(data_dir.join("workers")).unwrap();
        let referenced_session = new_session_id();
        let referenced_segment = new_segment_id();
        let orphan_session = new_session_id();
        let orphan_segment = new_segment_id();
        session_store
            .create_segment(referenced_session, referenced_segment, &[])
            .unwrap();
        session_store
            .create_segment(orphan_session, orphan_segment, &[])
            .unwrap();
        worker_metadata_store
            .write(&WorkerMetadata::new(
                "agent",
                Some(WorkerActiveSegmentRef::active_segment(
                    referenced_session,
                    referenced_segment,
                )),
            ))
            .unwrap();

        let output = run_prune_with_roots(
            SessionPruneOptions {
                unreferenced: true,
                older_than: None,
                force: true,
                dry_run: false,
            },
            data_dir,
        )
        .unwrap();

        assert_eq!(output.status, SessionCliStatus::Success);
        assert!(output.stdout.contains("deleted=1"));
        assert!(
            session_store
                .exists(referenced_session, referenced_segment)
                .unwrap()
        );
        assert!(
            !session_store
                .exists(orphan_session, orphan_segment)
                .unwrap()
        );
    }

    #[test]
    fn session_prune_without_force_is_dry_run() {
        let tmp = tempfile::TempDir::new().unwrap();
        let data_dir = tmp.path().join("data");
        let session_store = FsStore::new(data_dir.join("sessions")).unwrap();
        let orphan_session = new_session_id();
        let orphan_segment = new_segment_id();
        session_store
            .create_segment(orphan_session, orphan_segment, &[])
            .unwrap();

        let output = run_prune_with_roots(
            SessionPruneOptions {
                unreferenced: true,
                older_than: None,
                force: false,
                dry_run: false,
            },
            data_dir,
        )
        .unwrap();

        assert_eq!(output.status, SessionCliStatus::Success);
        assert!(output.stdout.contains("mode: dry-run"));
        assert!(
            session_store
                .exists(orphan_session, orphan_segment)
                .unwrap()
        );
    }

    #[test]
    fn analyze_requires_json_for_initial_cli() {
        let err = parse_session_args(&["analyze".to_string(), "/tmp/session.jsonl".to_string()])
            .unwrap_err();
        assert!(err.to_string().contains("--json"));
    }
}
