use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::Command,
};

use workspace_api::{
    Diagnostic, DiagnosticSeverity, GitCommitSummary, GitRemoteSummary, GitRepositorySummary,
    RepositoryDiagnostic, RepositoryObservedStatus, RepositorySource, RepositorySummary,
};

pub type RepositoryId = String;
pub type RepositorySelector = String;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfiguredRepository {
    pub id: RepositoryId,
    pub repository_key: String,
    pub provider: String,
    pub source: RepositorySource,
    pub source_revision: u64,
    pub source_fingerprint: String,
    pub observed_status: RepositoryObservedStatus,
    pub observed_at: Option<String>,
    pub path: Option<PathBuf>,
    pub default_selector: Option<RepositorySelector>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryListProjection {
    pub items: Vec<RepositorySummary>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryLogRead {
    pub repository_key: String,
    pub default_selector: Option<RepositorySelector>,
    pub limit: usize,
    pub commits: Vec<GitCommitSummary>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeTargetObservation {
    pub selector: RepositorySelector,
    pub commit: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitObservation {
    pub commit: String,
    pub parents: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepositoryLookupError {
    UnknownRepository { id: RepositoryId },
    UnsupportedProvider { id: RepositoryId, provider: String },
    MissingDefaultSelector { id: RepositoryId },
    InvalidSelector { id: RepositoryId, selector: String },
    CommitNotFound { id: RepositoryId, commit: String },
    InvalidCommitRelation { id: RepositoryId, detail: String },
    ProviderFailure { id: RepositoryId, operation: String },
}

#[derive(Debug, Clone)]
pub struct RepositoryRegistryReader {
    repositories: Vec<ConfiguredRepository>,
}

impl RepositoryRegistryReader {
    pub fn new(repositories: Vec<ConfiguredRepository>) -> Self {
        Self { repositories }
    }

    pub fn list(&self) -> RepositoryListProjection {
        if self.repositories.is_empty() {
            return RepositoryListProjection {
                items: Vec::new(),
                diagnostics: vec![Diagnostic {
                    severity: DiagnosticSeverity::Warning,
                    code: "repository_config_empty".to_string(),
                    message: "No repositories are configured for this workspace backend."
                        .to_string(),
                }],
            };
        }

        RepositoryListProjection {
            items: self
                .repositories
                .iter()
                .map(|repository| self.summary_for_config(repository))
                .collect(),
            diagnostics: Vec::new(),
        }
    }

    pub fn summary(
        &self,
        repository_key: &str,
    ) -> Result<RepositorySummary, RepositoryLookupError> {
        let repository = self.find_by_key(repository_key).ok_or_else(|| {
            RepositoryLookupError::UnknownRepository {
                id: repository_key.to_string(),
            }
        })?;
        Ok(self.summary_for_config(repository))
    }

    pub fn recent_log(
        &self,
        repository_key: &str,
        limit: Option<usize>,
    ) -> Result<RepositoryLogRead, RepositoryLookupError> {
        let repository = self.find_by_key(repository_key).ok_or_else(|| {
            RepositoryLookupError::UnknownRepository {
                id: repository_key.to_string(),
            }
        })?;
        if repository.provider != "git" {
            return Err(RepositoryLookupError::UnsupportedProvider {
                id: repository.id.clone(),
                provider: repository.provider.clone(),
            });
        }

        let limit = limit.unwrap_or(40).clamp(1, 200);
        let mut diagnostics = Vec::new();
        let commits = match self.git_log(repository, limit) {
            Ok(commits) => commits,
            Err(message) => {
                diagnostics.push(Diagnostic {
                    severity: DiagnosticSeverity::Warning,
                    code: "repository_git_log_unavailable".to_string(),
                    message,
                });
                Vec::new()
            }
        };

        Ok(RepositoryLogRead {
            repository_key: repository.repository_key.clone(),
            default_selector: repository.default_selector.clone(),
            limit,
            commits,
            diagnostics,
        })
    }

    pub fn observe_merge_target(
        &self,
        id: &str,
        requested_selector: Option<&str>,
    ) -> Result<MergeTargetObservation, RepositoryLookupError> {
        let repository = self.merge_repository(id)?;
        let selector = requested_selector
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .or_else(|| repository.default_selector.clone())
            .ok_or_else(|| RepositoryLookupError::MissingDefaultSelector { id: id.to_string() })?;
        let selector = normalize_target_branch_selector(id, &selector)?;
        let spec = format!("{selector}^{{commit}}");
        let commit = merge_git_stdout(
            repository,
            "resolve target",
            &["rev-parse", "--verify", "--end-of-options", &spec],
        )?
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned();
        if commit.is_empty() {
            return Err(RepositoryLookupError::InvalidSelector {
                id: id.to_string(),
                selector,
            });
        }
        Ok(MergeTargetObservation { selector, commit })
    }

    pub fn observe_commit(
        &self,
        id: &str,
        commit: &str,
    ) -> Result<CommitObservation, RepositoryLookupError> {
        let repository = self.merge_repository(id)?;
        let commit = commit.trim();
        if commit.is_empty() || commit.starts_with('-') {
            return Err(RepositoryLookupError::CommitNotFound {
                id: id.to_string(),
                commit: commit.into(),
            });
        }
        let line = merge_git_stdout(
            repository,
            "read commit",
            &[
                "show",
                "--no-patch",
                "--format=%H %P",
                "--end-of-options",
                commit,
            ],
        )?;
        let mut parts = line.split_whitespace();
        let canonical = parts.next().unwrap_or_default().to_owned();
        if canonical.is_empty() {
            return Err(RepositoryLookupError::CommitNotFound {
                id: id.to_string(),
                commit: commit.into(),
            });
        }
        Ok(CommitObservation {
            commit: canonical,
            parents: parts.map(str::to_owned).collect(),
        })
    }

    pub fn ensure_ancestor(
        &self,
        id: &str,
        ancestor: &str,
        descendant: &str,
    ) -> Result<(), RepositoryLookupError> {
        let repository = self.merge_repository(id)?;
        let repository_path =
            repository
                .path
                .as_ref()
                .ok_or_else(|| RepositoryLookupError::ProviderFailure {
                    id: id.into(),
                    operation: "repository source is not materialized for local Git access".into(),
                })?;
        let status = Command::new("git")
            .arg("-C")
            .arg(repository_path)
            .args(["merge-base", "--is-ancestor", ancestor, descendant])
            .status()
            .map_err(|_| RepositoryLookupError::ProviderFailure {
                id: id.into(),
                operation: "check commit ancestry".into(),
            })?;
        if status.success() {
            Ok(())
        } else {
            Err(RepositoryLookupError::InvalidCommitRelation {
                id: id.into(),
                detail: format!("commit {ancestor} is not an ancestor of {descendant}"),
            })
        }
    }

    fn merge_repository(&self, id: &str) -> Result<&ConfiguredRepository, RepositoryLookupError> {
        let repository = self
            .find(id)
            .ok_or_else(|| RepositoryLookupError::UnknownRepository { id: id.into() })?;
        if repository.provider != "git" {
            return Err(RepositoryLookupError::UnsupportedProvider {
                id: id.into(),
                provider: repository.provider.clone(),
            });
        }
        Ok(repository)
    }

    fn find_by_key(&self, repository_key: &str) -> Option<&ConfiguredRepository> {
        self.repositories
            .iter()
            .find(|repository| repository.repository_key == repository_key)
    }

    fn find(&self, id: &str) -> Option<&ConfiguredRepository> {
        self.repositories
            .iter()
            .find(|repository| repository.id == id)
    }

    fn summary_for_config(&self, repository: &ConfiguredRepository) -> RepositorySummary {
        let mut diagnostics = Vec::new();
        if repository.source.kind == workspace_api::RepositorySourceKind::Http {
            diagnostics.push(RepositoryDiagnostic {
                severity: "warning".to_string(),
                code: "repository_source_insecure_http".to_string(),
                message:
                    "HTTP Repository source is unencrypted; prefer HTTPS or SSH when available."
                        .to_string(),
            });
        }
        let git = match repository.provider.as_str() {
            "git" if repository.path.is_none() => {
                diagnostics.push(RepositoryDiagnostic {
                    severity: "info".to_string(),
                    code: "repository_source_unverified".to_string(),
                    message: "Remote Repository source is registered but is not materialized for server-local inspection.".to_string(),
                });
                None
            }
            "git" => match self.inspect_git(repository) {
                Ok(git) => Some(git),
                Err(message) => {
                    diagnostics.push(RepositoryDiagnostic {
                        severity: "warning".to_string(),
                        code: "repository_git_unavailable".to_string(),
                        message,
                    });
                    None
                }
            },
            provider => {
                diagnostics.push(RepositoryDiagnostic {
                    severity: "warning".to_string(),
                    code: "repository_provider_unsupported".to_string(),
                    message: format!(
                        "Repository provider `{provider}` is configured but is not supported by the workspace backend API."
                    ),
                });
                None
            }
        };

        RepositorySummary {
            repository_key: repository.repository_key.clone(),
            kind: repository.provider.clone(),
            provider: repository.provider.clone(),
            source: repository.source.clone(),
            source_revision: repository.source_revision,
            source_fingerprint: repository.source_fingerprint.clone(),
            observed_status: repository.observed_status,
            observed_at: repository.observed_at.clone(),
            default_selector: repository.default_selector.clone(),
            record_authority: "workspace-control-plane".to_string(),
            git,
            diagnostics: (!diagnostics.is_empty()).then_some(diagnostics),
        }
    }

    fn inspect_git(
        &self,
        repository: &ConfiguredRepository,
    ) -> Result<GitRepositorySummary, String> {
        let path = repository.path.as_ref().ok_or_else(|| {
            "Repository source is not materialized for local Git inspection.".to_string()
        })?;
        let head = git_stdout(path, ["rev-parse", "HEAD"])?;
        let branch = git_stdout(path, ["branch", "--show-current"])
            .ok()
            .and_then(|value| non_empty_string(value.trim()));
        let status = git_stdout(path, ["status", "--porcelain"])?;
        let remotes = git_stdout(path, ["remote", "-v"])
            .map(|raw| parse_remotes(&raw))
            .unwrap_or_default();
        Ok(GitRepositorySummary {
            status: "available".to_string(),
            head: non_empty_string(head.trim()),
            branch,
            dirty: !status.trim().is_empty(),
            remotes,
        })
    }

    fn git_log(
        &self,
        repository: &ConfiguredRepository,
        limit: usize,
    ) -> Result<Vec<GitCommitSummary>, String> {
        let limit_arg = format!("-{limit}");
        let path = repository.path.as_ref().ok_or_else(|| {
            "Repository source is not materialized for local Git log access.".to_string()
        })?;
        let output = git_stdout(
            path,
            [
                "log",
                "--date=iso-strict",
                "--decorate=short",
                "--pretty=format:%H%x1f%h%x1f%s%x1f%an%x1f%ae%x1f%aI%x1f%P%x1f%D%x1e",
                limit_arg.as_str(),
            ],
        )?;
        Ok(parse_git_log(&output))
    }
}

fn normalize_target_branch_selector(
    id: &str,
    selector: &str,
) -> Result<String, RepositoryLookupError> {
    let selector = selector.trim();
    let target_ref = if selector.starts_with("refs/heads/") {
        selector.to_owned()
    } else if selector.starts_with("refs/") {
        return Err(RepositoryLookupError::InvalidSelector {
            id: id.into(),
            selector: selector.into(),
        });
    } else {
        format!("refs/heads/{selector}")
    };
    let valid_ref = Command::new("git")
        .args(["check-ref-format", target_ref.as_str()])
        .status()
        .map_err(|_| RepositoryLookupError::ProviderFailure {
            id: id.into(),
            operation: "validate target branch ref".into(),
        })?;
    if !valid_ref.success() || selector.starts_with('-') || selector.as_bytes().contains(&0) {
        return Err(RepositoryLookupError::InvalidSelector {
            id: id.into(),
            selector: selector.into(),
        });
    }
    Ok(target_ref)
}

fn merge_git_stdout(
    repository: &ConfiguredRepository,
    operation: &str,
    args: &[&str],
) -> Result<String, RepositoryLookupError> {
    let path = repository
        .path
        .as_ref()
        .ok_or_else(|| RepositoryLookupError::ProviderFailure {
            id: repository.id.clone(),
            operation: "repository source is not materialized for local Git access".into(),
        })?;
    git_stdout(path, args.iter().copied()).map_err(|_| RepositoryLookupError::ProviderFailure {
        id: repository.id.clone(),
        operation: operation.into(),
    })
}

fn git_stdout<'a, I>(repository_path: &PathBuf, args: I) -> Result<String, String>
where
    I: IntoIterator<Item = &'a str>,
{
    let output = Command::new("git")
        .arg("-C")
        .arg(repository_path)
        .args(args)
        .output()
        .map_err(|_| {
            "Git command could not be executed; backend-private path details were omitted."
                .to_string()
        })?;
    if !output.status.success() {
        return Err("Git command failed; backend-private path details were omitted.".to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn parse_remotes(raw: &str) -> Vec<GitRemoteSummary> {
    let mut seen = BTreeSet::new();
    let mut remotes = Vec::new();
    for line in raw.lines() {
        let mut parts = line.split_whitespace();
        let Some(name) = parts.next() else {
            continue;
        };
        let Some(url) = parts.next() else {
            continue;
        };
        if !seen.insert((name.to_string(), url.to_string())) {
            continue;
        }
        remotes.push(GitRemoteSummary {
            name: name.to_string(),
            fetch_url: sanitize_remote_url(url),
        });
    }
    remotes
}

fn parse_git_log(raw: &str) -> Vec<GitCommitSummary> {
    raw.split('\u{1e}')
        .filter_map(|record| {
            let trimmed = record.trim_matches('\n').trim_end();
            if trimmed.is_empty() {
                return None;
            }
            let mut fields = trimmed.split('\u{1f}');
            let hash = fields.next()?.to_string();
            let short_hash = fields.next().unwrap_or_default().to_string();
            let summary = fields.next().unwrap_or_default().to_string();
            let author_name = fields.next().unwrap_or_default().to_string();
            let author_email = fields.next().unwrap_or_default().to_string();
            let author_date = fields.next().unwrap_or_default().to_string();
            let parents = fields
                .next()
                .unwrap_or_default()
                .split_whitespace()
                .map(ToString::to_string)
                .collect();
            let refs = fields
                .next()
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|reference| !reference.is_empty())
                .map(ToString::to_string)
                .collect();
            Some(GitCommitSummary {
                hash,
                short_hash,
                summary,
                author_name,
                author_email,
                author_date,
                parents,
                refs,
            })
        })
        .collect()
}

fn sanitize_remote_url(url: &str) -> String {
    let trimmed = url.trim();
    if is_local_path_like(trimmed) {
        return "<redacted-local-path>".to_string();
    }

    let Some((scheme, rest)) = trimmed.split_once("://") else {
        return trimmed.to_string();
    };
    if scheme.eq_ignore_ascii_case("file") {
        return "file://<redacted-local-path>".to_string();
    }
    let Some((_credentials, host_path)) = rest.split_once('@') else {
        return trimmed.to_string();
    };
    format!("{scheme}://<redacted>@{host_path}")
}

fn is_local_path_like(value: &str) -> bool {
    Path::new(value).is_absolute() || is_windows_absolute_path_like(value)
}

fn is_windows_absolute_path_like(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
}

fn non_empty_string(value: &str) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_remote_credentials_and_local_paths() {
        assert_eq!(
            sanitize_remote_url("https://user:token@example.com/org/repo.git"),
            "https://<redacted>@example.com/org/repo.git"
        );
        assert_eq!(
            sanitize_remote_url("git@example.com:org/repo.git"),
            "git@example.com:org/repo.git"
        );
        assert_eq!(
            sanitize_remote_url("/home/alice/private/repo.git"),
            "<redacted-local-path>"
        );
        assert_eq!(
            sanitize_remote_url("/Users/alice/private/repo.git"),
            "<redacted-local-path>"
        );
        assert_eq!(
            sanitize_remote_url("C:\\Users\\alice\\private\\repo.git"),
            "<redacted-local-path>"
        );
        assert_eq!(
            sanitize_remote_url("file:///home/alice/private/repo.git"),
            "file://<redacted-local-path>"
        );
        assert_eq!(
            sanitize_remote_url("file://localhost/home/alice/private/repo.git"),
            "file://<redacted-local-path>"
        );
    }

    #[test]
    fn empty_registry_reports_diagnostic_without_implicit_repository() {
        let projection = RepositoryRegistryReader::new(Vec::new()).list();

        assert!(projection.items.is_empty());
        assert_eq!(projection.diagnostics.len(), 1);
        assert_eq!(projection.diagnostics[0].code, "repository_config_empty");
    }

    #[test]
    fn remote_source_is_visible_but_local_provider_operations_fail_closed() {
        let source = RepositorySource {
            kind: workspace_api::RepositorySourceKind::Ssh,
            uri: "git@example.test:org/repository.git".to_string(),
        };
        let reader = RepositoryRegistryReader::new(vec![ConfiguredRepository {
            id: "remote".into(),
            repository_key: "remote".into(),
            provider: "git".into(),
            source_fingerprint: crate::repository_source::repository_source_fingerprint(&source),
            source,
            source_revision: 1,
            observed_status: RepositoryObservedStatus::Unverified,
            observed_at: None,
            path: None,
            default_selector: Some("main".into()),
        }]);

        let projection = reader.list();
        let summary = &projection.items[0];
        assert_eq!(
            summary.source.kind,
            workspace_api::RepositorySourceKind::Ssh
        );
        assert_eq!(
            summary.observed_status,
            RepositoryObservedStatus::Unverified
        );
        assert!(summary.git.is_none());
        assert_eq!(
            summary.diagnostics.as_ref().unwrap()[0].code,
            "repository_source_unverified"
        );

        let repository = reader.merge_repository("remote").unwrap();
        let error = merge_git_stdout(&repository, "inspect", &["rev-parse", "HEAD"]).unwrap_err();
        assert!(matches!(
            error,
            RepositoryLookupError::ProviderFailure { operation, .. }
                if operation.contains("not materialized")
        ));
    }

    #[test]
    fn merge_evidence_is_resolved_by_repository_identity() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path();
        assert!(
            Command::new("git")
                .args(["init", "-b", "main"])
                .arg(path)
                .status()
                .unwrap()
                .success()
        );
        for args in [
            vec!["config", "user.email", "test@example.com"],
            vec!["config", "user.name", "Test"],
        ] {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(path)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        std::fs::write(path.join("file.txt"), "base\n").unwrap();
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(path)
                .args(["add", "file.txt"])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(path)
                .args(["commit", "-m", "base"])
                .status()
                .unwrap()
                .success()
        );
        let base = git_stdout(&path.to_path_buf(), ["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string();
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(path)
                .args(["checkout", "-b", "feature"])
                .status()
                .unwrap()
                .success()
        );
        std::fs::write(path.join("file.txt"), "base\nfeature\n").unwrap();
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(path)
                .args(["commit", "-am", "feature"])
                .status()
                .unwrap()
                .success()
        );
        let source = git_stdout(&path.to_path_buf(), ["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string();
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(path)
                .args(["update-ref", "refs/tags/main", &source])
                .status()
                .unwrap()
                .success()
        );

        let source_descriptor = RepositorySource {
            kind: workspace_api::RepositorySourceKind::LocalPath,
            uri: path.display().to_string(),
        };
        let reader = RepositoryRegistryReader::new(vec![ConfiguredRepository {
            id: "main".into(),
            repository_key: "main".into(),
            provider: "git".into(),
            source_fingerprint: crate::repository_source::repository_source_fingerprint(
                &source_descriptor,
            ),
            source: source_descriptor,
            source_revision: 1,
            observed_status: RepositoryObservedStatus::Unverified,
            observed_at: None,
            path: Some(path.to_path_buf()),
            default_selector: Some("main".into()),
        }]);
        let target = reader.observe_merge_target("main", Some("main")).unwrap();
        assert_eq!(target.selector, "refs/heads/main");
        assert_eq!(target.commit, base);
        assert_eq!(
            reader.observe_commit("main", &source).unwrap().parents,
            vec![base.clone()]
        );
        reader.ensure_ancestor("main", &base, &source).unwrap();
        assert_eq!(
            reader
                .observe_merge_target("main", Some("refs/heads/main"))
                .unwrap()
                .commit,
            base
        );
        assert!(matches!(
            reader.ensure_ancestor("main", &source, &base),
            Err(RepositoryLookupError::InvalidCommitRelation { .. })
        ));
    }

    #[test]
    fn unknown_repository_is_not_resolved_from_fallback() {
        let reader = RepositoryRegistryReader::new(Vec::new());

        assert_eq!(
            reader.summary("main").unwrap_err(),
            RepositoryLookupError::UnknownRepository {
                id: "main".to_string()
            }
        );
    }
}
