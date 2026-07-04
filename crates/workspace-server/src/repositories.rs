use std::{collections::BTreeSet, path::PathBuf, process::Command};

use serde::{Deserialize, Serialize};

pub type RepositoryId = String;
pub type RepositorySelector = String;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfiguredRepository {
    pub id: RepositoryId,
    pub provider: String,
    pub uri: String,
    pub path: PathBuf,
    pub display_name: Option<String>,
    pub default_selector: Option<RepositorySelector>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositorySummary {
    pub id: RepositoryId,
    pub display_name: String,
    pub kind: String,
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_selector: Option<RepositorySelector>,
    pub record_authority: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git: Option<GitRepositorySummary>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<RepositoryDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitRepositorySummary {
    pub status: String,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub dirty: bool,
    pub remotes: Vec<GitRemoteSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitRemoteSummary {
    pub name: String,
    pub fetch_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryDiagnostic {
    pub severity: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryListProjection {
    pub items: Vec<RepositorySummary>,
    pub diagnostics: Vec<RepositoryDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryLogRead {
    pub repository_id: RepositoryId,
    pub default_selector: Option<RepositorySelector>,
    pub limit: usize,
    pub commits: Vec<GitCommitSummary>,
    pub diagnostics: Vec<RepositoryDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitCommitSummary {
    pub hash: String,
    pub short_hash: String,
    pub summary: String,
    pub author_name: String,
    pub author_email: String,
    pub author_date: String,
    pub parents: Vec<String>,
    pub refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepositoryLookupError {
    UnknownRepository { id: RepositoryId },
    UnsupportedProvider { id: RepositoryId, provider: String },
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
                diagnostics: vec![RepositoryDiagnostic {
                    severity: "warning".to_string(),
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

    pub fn summary(&self, id: &str) -> Result<RepositorySummary, RepositoryLookupError> {
        let repository = self
            .find(id)
            .ok_or_else(|| RepositoryLookupError::UnknownRepository { id: id.to_string() })?;
        Ok(self.summary_for_config(repository))
    }

    pub fn recent_log(
        &self,
        id: &str,
        limit: Option<usize>,
    ) -> Result<RepositoryLogRead, RepositoryLookupError> {
        let repository = self
            .find(id)
            .ok_or_else(|| RepositoryLookupError::UnknownRepository { id: id.to_string() })?;
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
                diagnostics.push(RepositoryDiagnostic {
                    severity: "warning".to_string(),
                    code: "repository_git_log_unavailable".to_string(),
                    message,
                });
                Vec::new()
            }
        };

        Ok(RepositoryLogRead {
            repository_id: repository.id.clone(),
            default_selector: repository.default_selector.clone(),
            limit,
            commits,
            diagnostics,
        })
    }

    fn find(&self, id: &str) -> Option<&ConfiguredRepository> {
        self.repositories
            .iter()
            .find(|repository| repository.id == id)
    }

    fn summary_for_config(&self, repository: &ConfiguredRepository) -> RepositorySummary {
        let display_name = repository
            .display_name
            .clone()
            .unwrap_or_else(|| repository.id.clone());
        let mut diagnostics = Vec::new();
        let git = match repository.provider.as_str() {
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
            id: repository.id.clone(),
            display_name,
            kind: repository.provider.clone(),
            provider: repository.provider.clone(),
            default_selector: repository.default_selector.clone(),
            record_authority: "workspace-backend-config".to_string(),
            git,
            diagnostics,
        }
    }

    fn inspect_git(
        &self,
        repository: &ConfiguredRepository,
    ) -> Result<GitRepositorySummary, String> {
        let head = git_stdout(&repository.path, ["rev-parse", "HEAD"])?;
        let branch = git_stdout(&repository.path, ["branch", "--show-current"])
            .ok()
            .and_then(|value| non_empty_string(value.trim()));
        let status = git_stdout(&repository.path, ["status", "--porcelain"])?;
        let remotes = git_stdout(&repository.path, ["remote", "-v"])
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
        let output = git_stdout(
            &repository.path,
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
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let Some((_credentials, host_path)) = rest.split_once('@') else {
        return url.to_string();
    };
    format!("{scheme}://<redacted>@{host_path}")
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
    fn sanitizes_remote_credentials() {
        assert_eq!(
            sanitize_remote_url("https://user:token@example.com/org/repo.git"),
            "https://<redacted>@example.com/org/repo.git"
        );
        assert_eq!(
            sanitize_remote_url("git@example.com:org/repo.git"),
            "git@example.com:org/repo.git"
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
