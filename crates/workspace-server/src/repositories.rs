use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde::{Deserialize, Serialize};

use crate::hosts::RuntimeDiagnostic;

const LOCAL_REPOSITORY_ID: &str = "local";
const MAX_COMMAND_OUTPUT: usize = 4096;
const DEFAULT_LOG_LIMIT: usize = 10;
const MAX_LOG_LIMIT: usize = 50;
const MAX_FIELD_LEN: usize = 240;

#[derive(Debug, Clone)]
pub struct LocalRepositoryReader {
    workspace_root: PathBuf,
}

impl LocalRepositoryReader {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }

    pub fn list(&self, workspace_display_name: &str) -> Vec<RepositorySummary> {
        vec![self.summary(workspace_display_name)]
    }

    pub fn summary(&self, workspace_display_name: &str) -> RepositorySummary {
        let git = inspect_git(&self.workspace_root);
        RepositorySummary {
            id: LOCAL_REPOSITORY_ID.to_string(),
            display_name: workspace_display_name.to_string(),
            kind: "local".to_string(),
            workspace_root: self.workspace_root.clone(),
            record_authority: "local_workspace_root".to_string(),
            git,
        }
    }

    pub fn recent_log(&self, requested_limit: Option<usize>) -> RepositoryLogRead {
        let limit = requested_limit
            .unwrap_or(DEFAULT_LOG_LIMIT)
            .clamp(1, MAX_LOG_LIMIT);
        git_log(&self.workspace_root, limit)
    }

    pub fn is_local_repository_id(id: &str) -> bool {
        id == LOCAL_REPOSITORY_ID
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositorySummary {
    pub id: String,
    pub display_name: String,
    pub kind: String,
    pub workspace_root: PathBuf,
    pub record_authority: String,
    pub git: GitRepositorySummary,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitRepositorySummary {
    pub status: String,
    pub root: Option<PathBuf>,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub dirty: Option<bool>,
    pub dirty_scope: String,
    pub remote: Option<GitRemoteSummary>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitRemoteSummary {
    pub name: String,
    pub url: String,
    pub redacted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitCommitSummary {
    pub hash: String,
    pub subject: String,
    pub author_name: String,
    pub author_email: String,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositoryLogRead {
    pub limit: usize,
    pub items: Vec<GitCommitSummary>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

fn inspect_git(workspace_root: &Path) -> GitRepositorySummary {
    let mut diagnostics = Vec::new();
    let root = match git_stdout(workspace_root, &["rev-parse", "--show-toplevel"]) {
        Ok(root) => PathBuf::from(root.trim()),
        Err(message) => {
            diagnostics.push(diagnostic(
                "git_unavailable",
                "info",
                format!("Workspace root is not available as a Git repository: {message}"),
            ));
            return GitRepositorySummary {
                status: "unavailable".to_string(),
                root: None,
                branch: None,
                head: None,
                dirty: None,
                dirty_scope: "tracked_changes_only".to_string(),
                remote: None,
                diagnostics,
            };
        }
    };

    let branch = git_stdout(workspace_root, &["branch", "--show-current"])
        .ok()
        .map(|value| truncate_field(value.trim(), MAX_FIELD_LEN))
        .filter(|value| !value.is_empty())
        .or_else(|| Some("detached".to_string()));
    let head = match git_stdout(workspace_root, &["rev-parse", "--verify", "HEAD"]) {
        Ok(value) => Some(truncate_field(value.trim(), 40)),
        Err(message) => {
            diagnostics.push(diagnostic(
                "git_head_unavailable",
                "warn",
                format!("Git HEAD summary is unavailable: {message}"),
            ));
            None
        }
    };
    let dirty = match git_stdout(
        workspace_root,
        &["status", "--porcelain=v1", "--untracked-files=no"],
    ) {
        Ok(value) => Some(!value.trim().is_empty()),
        Err(message) => {
            diagnostics.push(diagnostic(
                "git_status_unavailable",
                "warn",
                format!("Git dirty status is unavailable: {message}"),
            ));
            None
        }
    };
    let remote = match git_stdout(workspace_root, &["remote", "get-url", "origin"]) {
        Ok(value) => {
            let (url, redacted) = sanitize_remote_url(value.trim());
            Some(GitRemoteSummary {
                name: "origin".to_string(),
                url,
                redacted,
            })
        }
        Err(_) => {
            diagnostics.push(diagnostic(
                "git_origin_remote_missing",
                "info",
                "No origin remote is configured or visible through the bounded Git summary."
                    .to_string(),
            ));
            None
        }
    };

    GitRepositorySummary {
        status: "available".to_string(),
        root: Some(root),
        branch,
        head,
        dirty,
        dirty_scope: "tracked_changes_only".to_string(),
        remote,
        diagnostics,
    }
}

fn git_log(workspace_root: &Path, limit: usize) -> RepositoryLogRead {
    let mut diagnostics = Vec::new();
    if let Err(message) = git_stdout(workspace_root, &["rev-parse", "--show-toplevel"]) {
        diagnostics.push(diagnostic(
            "git_unavailable",
            "info",
            format!("Recent Git log is unavailable for this local repository: {message}"),
        ));
        return RepositoryLogRead {
            limit,
            items: Vec::new(),
            diagnostics,
        };
    }

    match git_stdout(
        workspace_root,
        &[
            "log",
            "--no-show-signature",
            "--date=iso-strict",
            "--format=%H%x1f%an%x1f%ae%x1f%aI%x1f%s%x1e",
            "-n",
            &limit.to_string(),
        ],
    ) {
        Ok(output) => RepositoryLogRead {
            limit,
            items: parse_log(output.as_str()),
            diagnostics,
        },
        Err(message) => {
            diagnostics.push(diagnostic(
                "git_log_unavailable",
                "warn",
                format!("Recent Git log is unavailable: {message}"),
            ));
            RepositoryLogRead {
                limit,
                items: Vec::new(),
                diagnostics,
            }
        }
    }
}

fn parse_log(output: &str) -> Vec<GitCommitSummary> {
    output
        .split('\u{1e}')
        .filter_map(|record| {
            let record = record.trim_matches('\n');
            if record.is_empty() {
                return None;
            }
            let mut fields = record.split('\u{1f}');
            Some(GitCommitSummary {
                hash: truncate_field(fields.next()?, 40),
                author_name: truncate_field(fields.next().unwrap_or_default(), MAX_FIELD_LEN),
                author_email: truncate_field(fields.next().unwrap_or_default(), MAX_FIELD_LEN),
                timestamp: truncate_field(fields.next().unwrap_or_default(), MAX_FIELD_LEN),
                subject: truncate_field(fields.next().unwrap_or_default(), MAX_FIELD_LEN),
            })
        })
        .collect()
}

fn git_stdout(workspace_root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(workspace_root)
        .args(args)
        .output()
        .map_err(|error| truncate_field(&error.to_string(), MAX_FIELD_LEN))?;
    command_stdout(output)
}

fn command_stdout(output: Output) -> Result<String, String> {
    if output.status.success() {
        return Ok(truncate_output(
            String::from_utf8_lossy(&output.stdout).as_ref(),
        ));
    }
    let stderr = truncate_output(String::from_utf8_lossy(&output.stderr).as_ref());
    if stderr.trim().is_empty() {
        Err(format!("git exited with status {}", output.status))
    } else {
        Err(stderr.trim().to_string())
    }
}

fn sanitize_remote_url(raw: &str) -> (String, bool) {
    let bounded = truncate_field(raw, MAX_FIELD_LEN);
    let Some(separator) = bounded.find("://") else {
        return (bounded, false);
    };
    let scheme_end = separator + 3;
    let after_scheme = &bounded[scheme_end..];
    let Some(at_index) = after_scheme.find('@') else {
        return (bounded, false);
    };
    let host_and_path = &after_scheme[(at_index + 1)..];
    (format!("{}{}", &bounded[..scheme_end], host_and_path), true)
}

fn truncate_output(value: &str) -> String {
    truncate_field(value, MAX_COMMAND_OUTPUT)
}

fn truncate_field(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_string();
    }
    let mut end = limit;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

fn diagnostic(code: &str, severity: &str, message: String) -> RuntimeDiagnostic {
    RuntimeDiagnostic {
        code: code.to_string(),
        severity: severity.to_string(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_userinfo_from_url_remotes() {
        assert_eq!(
            sanitize_remote_url("https://token@example.com/org/repo.git"),
            ("https://example.com/org/repo.git".to_string(), true)
        );
        assert_eq!(
            sanitize_remote_url("git@example.com:org/repo.git"),
            ("git@example.com:org/repo.git".to_string(), false)
        );
    }

    #[test]
    fn parses_bounded_git_log_records() {
        let parsed = parse_log(
            "0123456789abcdef\u{1f}Alice\u{1f}a@example.test\u{1f}2026-01-01T00:00:00+00:00\u{1f}Subject\u{1e}\n",
        );
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].hash, "0123456789abcdef");
        assert_eq!(parsed[0].subject, "Subject");
    }
}
