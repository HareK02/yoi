use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use client::{
    BackendWorkspaceCatalogTarget, CreateBackendWorkspaceRepository, CreateBackendWorkspaceRequest,
    create_backend_workspace, list_backend_workspace_repositories_blocking,
    list_backend_workspaces_blocking,
};

use super::{ParseError, client_global_config_path};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InitOptions {
    pub(crate) backend_url: String,
    pub(crate) display_name: String,
    pub(crate) repository_key: String,
    pub(crate) repository_root: PathBuf,
    pub(crate) default_ref: Option<String>,
}

pub(crate) async fn run_init(
    options: InitOptions,
) -> Result<workspace_api::WorkspaceCreateResponse, ParseError> {
    let repository_uri = options
        .repository_root
        .to_str()
        .ok_or_else(|| ParseError("the repository root is not valid UTF-8".to_string()))?;
    let target = BackendWorkspaceCatalogTarget {
        base_url: options.backend_url.clone(),
    };
    let response = create_backend_workspace(
        &target,
        &CreateBackendWorkspaceRequest {
            operation_id: uuid::Uuid::now_v7().to_string(),
            display_name: options.display_name,
            repository: CreateBackendWorkspaceRepository {
                repository_key: options.repository_key,
                source: repository_uri.to_string(),
                default_ref: options.default_ref,
            },
        },
    )
    .await
    .map_err(|error| ParseError(format!("Backend rejected Workspace creation: {error}")))?;
    record_workspace_backend_routing(&response.workspace.workspace_id, &options.backend_url)?;
    Ok(response)
}

pub(crate) fn select_backend_workspace_for_repository(
    base_url: &str,
    repository_path: &Path,
) -> Result<String, ParseError> {
    let repository = discover_open_repository(repository_path)?;
    let target = BackendWorkspaceCatalogTarget {
        base_url: base_url.to_string(),
    };
    let workspaces = list_backend_workspaces_blocking(&target).map_err(|error| {
        ParseError(format!(
            "failed to query Backend Workspace catalog: {error}"
        ))
    })?;
    let mut catalog = Vec::with_capacity(workspaces.len());
    for workspace in workspaces {
        let repositories =
            list_backend_workspace_repositories_blocking(&target, &workspace.workspace_id)
                .map_err(|error| {
                    ParseError(format!(
                        "failed to query Repository catalog for Workspace '{}': {error}",
                        workspace.display_name
                    ))
                })?;
        catalog.push((workspace, repositories));
    }
    select_workspace_from_repository_catalog(&repository, &catalog)
}

pub(crate) fn discover_repository_root(path: &Path) -> Result<PathBuf, ParseError> {
    Ok(discover_open_repository(path)?.root)
}

fn select_workspace_from_repository_catalog(
    repository: &OpenRepositoryIdentity,
    catalog: &[(
        workspace_api::WorkspaceSummary,
        Vec<workspace_api::RepositorySummary>,
    )],
) -> Result<String, ParseError> {
    let matches = catalog
        .iter()
        .filter(|(_, repositories)| {
            repositories
                .iter()
                .any(|candidate| repository.matches(&candidate.source))
        })
        .map(|(workspace, _)| workspace)
        .collect::<Vec<_>>();

    match matches.as_slice() {
        [workspace] => Ok(workspace.workspace_id.clone()),
        [] => Err(ParseError(
            "the current Git repository is not registered in any accessible Workspace; run `yoi init` or pass `--workspace-id` explicitly"
                .to_string(),
        )),
        matches => {
            let names = matches
                .iter()
                .map(|workspace| workspace.display_name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            Err(ParseError(format!(
                "the current Git repository matches multiple accessible Workspaces ({names}); pass `--workspace-id` explicitly"
            )))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OpenRepositoryIdentity {
    root: PathBuf,
    remote_uris: Vec<String>,
}

impl OpenRepositoryIdentity {
    fn matches(&self, source: &workspace_api::RepositorySource) -> bool {
        match source.kind {
            workspace_api::RepositorySourceKind::LocalPath => fs::canonicalize(&source.uri)
                .ok()
                .is_some_and(|path| path == self.root),
            workspace_api::RepositorySourceKind::File => source
                .uri
                .strip_prefix("file://")
                .and_then(|path| fs::canonicalize(path).ok())
                .is_some_and(|path| path == self.root),
            workspace_api::RepositorySourceKind::Ssh
            | workspace_api::RepositorySourceKind::Https => self
                .remote_uris
                .iter()
                .any(|uri| normalize_git_uri(uri) == normalize_git_uri(&source.uri)),
            workspace_api::RepositorySourceKind::Invalid => false,
        }
    }
}

fn discover_open_repository(path: &Path) -> Result<OpenRepositoryIdentity, ParseError> {
    let root = git_stdout(path, ["rev-parse", "--show-toplevel"])?;
    let root = fs::canonicalize(root.trim()).map_err(|error| {
        ParseError(format!(
            "failed to resolve current Git repository root: {error}"
        ))
    })?;
    let remote_uris = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["remote", "get-url", "--all", "origin"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    Ok(OpenRepositoryIdentity { root, remote_uris })
}

fn git_stdout<const N: usize>(path: &Path, args: [&str; N]) -> Result<String, ParseError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .map_err(|error| ParseError(format!("failed to execute Git: {error}")))?;
    if !output.status.success() {
        return Err(ParseError(
            "the current directory is not inside a readable Git repository".to_string(),
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|_| ParseError("Git returned a non-UTF-8 repository path".to_string()))
}

fn normalize_git_uri(uri: &str) -> String {
    let uri = uri.trim().trim_end_matches('/');
    uri.strip_suffix(".git").unwrap_or(uri).to_string()
}

fn record_workspace_backend_routing(
    workspace_id: &str,
    backend_url: &str,
) -> Result<(), ParseError> {
    let path = client_global_config_path().ok_or_else(|| {
        ParseError("unable to resolve the global client configuration directory".to_string())
    })?;
    record_workspace_backend_routing_at(&path, workspace_id, backend_url)
}

fn record_workspace_backend_routing_at(
    path: &Path,
    workspace_id: &str,
    backend_url: &str,
) -> Result<(), ParseError> {
    let mut config = match fs::read_to_string(path) {
        Ok(raw) => toml::from_str::<toml::Value>(&raw).map_err(|error| {
            ParseError(format!(
                "failed to parse global client config {}: {error}",
                path.display()
            ))
        })?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            toml::Value::Table(toml::map::Map::new())
        }
        Err(error) => {
            return Err(ParseError(format!(
                "failed to read global client config {}: {error}",
                path.display()
            )));
        }
    };
    let table = config
        .as_table_mut()
        .ok_or_else(|| ParseError("global client config must be a TOML table".to_string()))?;
    let backends = table
        .entry("backends")
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()))
        .as_table_mut()
        .ok_or_else(|| ParseError("global client config `backends` must be a table".to_string()))?;
    let existing_name = backends.iter().find_map(|(name, value)| {
        value
            .get("url")
            .and_then(toml::Value::as_str)
            .filter(|url| *url == backend_url)
            .map(|_| name.clone())
    });
    let backend_name = existing_name.unwrap_or_else(|| {
        let mut candidate = "init".to_string();
        let mut suffix = 2_u32;
        while backends.contains_key(&candidate) {
            candidate = format!("init-{suffix}");
            suffix += 1;
        }
        backends.insert(
            candidate.clone(),
            toml::Value::Table(toml::map::Map::from_iter([(
                "url".to_string(),
                toml::Value::String(backend_url.to_string()),
            )])),
        );
        candidate
    });
    table
        .entry("default_connection")
        .or_insert_with(|| toml::Value::String("backend".to_string()));
    table
        .entry("default_backend")
        .or_insert_with(|| toml::Value::String(backend_name.clone()));
    let workspaces = table
        .entry("workspaces")
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()))
        .as_table_mut()
        .ok_or_else(|| {
            ParseError("global client config `workspaces` must be a table".to_string())
        })?;
    workspaces.insert(
        workspace_id.to_string(),
        toml::Value::Table(toml::map::Map::from_iter([(
            "backend".to_string(),
            toml::Value::String(backend_name),
        )])),
    );

    let parent = path.parent().ok_or_else(|| {
        ParseError("global client config path has no parent directory".to_string())
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        ParseError(format!(
            "failed to create global client config directory {}: {error}",
            parent.display()
        ))
    })?;
    let encoded = toml::to_string_pretty(&config)
        .map_err(|error| ParseError(format!("failed to encode global client config: {error}")))?;
    let temporary = path.with_extension(format!("toml.tmp-{}", uuid::Uuid::now_v7()));
    fs::write(&temporary, encoded).map_err(|error| {
        ParseError(format!(
            "failed to write global client config {}: {error}",
            temporary.display()
        ))
    })?;
    fs::rename(&temporary, path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        ParseError(format!(
            "failed to publish global client config {}: {error}",
            path.display()
        ))
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ClientDefaultConnection, read_client_config_from_global_path};

    fn workspace_summary(id: &str, name: &str) -> workspace_api::WorkspaceSummary {
        workspace_api::WorkspaceSummary {
            workspace_id: id.to_string(),
            owner_account_id: "owner-account".to_string(),
            display_name: name.to_string(),
            state: "active".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    fn repository_summary(
        source: workspace_api::RepositorySource,
    ) -> workspace_api::RepositorySummary {
        workspace_api::RepositorySummary {
            repository_key: "main".to_string(),
            kind: "git".to_string(),
            provider: "builtin:git".to_string(),
            source,
            source_revision: 1,
            source_fingerprint: "fingerprint".to_string(),
            observed_status: workspace_api::RepositoryObservedStatus::Unverified,
            observed_at: None,
            default_selector: Some("develop".to_string()),
            record_authority: "server_db".to_string(),
            git: None,
            diagnostics: None,
        }
    }

    #[test]
    fn repository_catalog_selection_handles_single_zero_and_multiple_matches() {
        let repository = tempfile::tempdir().unwrap();
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(repository.path())
            .status()
            .unwrap();
        let identity = discover_open_repository(repository.path()).unwrap();
        let source = workspace_api::RepositorySource {
            kind: workspace_api::RepositorySourceKind::LocalPath,
            uri: identity.root.display().to_string(),
        };
        let matching_repository = repository_summary(source);
        let workspace_a = workspace_summary("workspace-a", "Workspace A");
        let workspace_b = workspace_summary("workspace-b", "Workspace B");

        assert_eq!(
            select_workspace_from_repository_catalog(
                &identity,
                &[(workspace_a.clone(), vec![matching_repository.clone()])],
            )
            .unwrap(),
            "workspace-a"
        );
        assert!(
            select_workspace_from_repository_catalog(&identity, &[])
                .unwrap_err()
                .to_string()
                .contains("not registered in any accessible Workspace")
        );
        let multiple = select_workspace_from_repository_catalog(
            &identity,
            &[
                (workspace_a, vec![matching_repository.clone()]),
                (workspace_b, vec![matching_repository]),
            ],
        )
        .unwrap_err()
        .to_string();
        assert!(multiple.contains("matches multiple accessible Workspaces"));
        assert!(multiple.contains("--workspace-id"));
    }

    #[test]
    fn init_routing_config_survives_reload_without_repository_local_state() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("client.toml");

        record_workspace_backend_routing_at(&path, "workspace-a", "http://backend.example")
            .unwrap();
        let first = read_client_config_from_global_path(Some(&path))
            .unwrap()
            .unwrap();
        let second = read_client_config_from_global_path(Some(&path))
            .unwrap()
            .unwrap();

        assert_eq!(first.default_connection, ClientDefaultConnection::Backend);
        assert_eq!(first.default_backend.as_deref(), Some("init"));
        assert_eq!(
            first
                .workspaces
                .get("workspace-a")
                .and_then(|entry| entry.backend.as_deref()),
            Some("init")
        );
        assert_eq!(second, first);
        assert!(!temp.path().join(".yoi/workspace.toml").exists());
    }
}
