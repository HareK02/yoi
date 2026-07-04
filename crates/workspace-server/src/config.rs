use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::{fs, io};

use serde::{Deserialize, Serialize};

use crate::hosts::RemoteRuntimeConfig;
use crate::identity::WorkspaceIdentity;
use crate::repositories::ConfiguredRepository;
use crate::server::{AuthConfig, ServerConfig};
use crate::{Error, Result};

pub const WORKSPACE_BACKEND_CONFIG_RELATIVE_PATH: &str = ".yoi/workspace-backend.local.toml";
pub const WORKSPACE_BACKEND_CONFIG_TEMPLATE: &str =
    include_str!("../../../resources/workspace-backend.default.toml");
const DEFAULT_LISTEN: &str = "127.0.0.1:8787";
const DEFAULT_FRONTEND_URL: &str = "http://127.0.0.1:5173";
const DEFAULT_MAX_RECORDS: usize = 200;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceBackendConfigFile {
    #[serde(default)]
    pub server: WorkspaceBackendServerConfig,
    #[serde(default)]
    pub data: WorkspaceBackendDataConfig,
    #[serde(default)]
    pub limits: WorkspaceBackendLimitsConfig,
    #[serde(default)]
    pub repositories: Vec<WorkspaceRepositoryConfigFile>,
    #[serde(default)]
    pub runtimes: WorkspaceBackendRuntimesConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceBackendServerConfig {
    #[serde(default)]
    pub listen: Option<String>,
    #[serde(default)]
    pub frontend_url: Option<String>,
    #[serde(default)]
    pub static_assets_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceBackendDataConfig {
    #[serde(default)]
    pub root: Option<PathBuf>,
    #[serde(default)]
    pub workspace_database_path: Option<PathBuf>,
    #[serde(default)]
    pub embedded_runtime_store_root: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceBackendLimitsConfig {
    #[serde(default)]
    pub max_records: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceRepositoryConfigFile {
    pub id: String,
    pub provider: String,
    pub uri: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub default_selector: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceBackendRuntimesConfig {
    #[serde(default)]
    pub remote: Vec<RemoteRuntimeConfigFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RemoteRuntimeConfigFile {
    pub id: String,
    pub endpoint: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub token_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigDiff {
    pub differs: bool,
    pub text: String,
}

impl ConfigDiff {
    fn new(default: &str, local: &str) -> Self {
        if default == local {
            return Self {
                differs: false,
                text: "workspace backend local config matches the packaged default\n".to_string(),
            };
        }

        let mut text = String::from("--- packaged default\n+++ workspace local\n");
        let default_lines = default.lines().collect::<Vec<_>>();
        let local_lines = local.lines().collect::<Vec<_>>();
        let max = default_lines.len().max(local_lines.len());
        for index in 0..max {
            match (default_lines.get(index), local_lines.get(index)) {
                (Some(left), Some(right)) if left == right => {
                    text.push(' ');
                    text.push_str(left);
                    text.push('\n');
                }
                (Some(left), Some(right)) => {
                    text.push('-');
                    text.push_str(left);
                    text.push('\n');
                    text.push('+');
                    text.push_str(right);
                    text.push('\n');
                }
                (Some(left), None) => {
                    text.push('-');
                    text.push_str(left);
                    text.push('\n');
                }
                (None, Some(right)) => {
                    text.push('+');
                    text.push_str(right);
                    text.push('\n');
                }
                (None, None) => {}
            }
        }

        Self {
            differs: true,
            text,
        }
    }
}

#[derive(Clone)]
pub struct ResolvedWorkspaceBackendConfig {
    pub server: ServerConfig,
    pub listen: SocketAddr,
    pub database_path: PathBuf,
}

impl WorkspaceBackendConfigFile {
    pub fn path_for_workspace(workspace_root: impl AsRef<Path>) -> PathBuf {
        workspace_root
            .as_ref()
            .join(WORKSPACE_BACKEND_CONFIG_RELATIVE_PATH)
    }

    pub fn ensure_local_config_for_workspace(workspace_root: impl AsRef<Path>) -> Result<()> {
        let path = Self::path_for_workspace(workspace_root);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                use std::io::Write;
                file.write_all(WORKSPACE_BACKEND_CONFIG_TEMPLATE.as_bytes())?;
                file.sync_all()?;
                Ok(())
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
            Err(error) => Err(Error::Io(error)),
        }
    }

    pub fn local_config_diff_for_workspace(workspace_root: impl AsRef<Path>) -> Result<ConfigDiff> {
        let workspace_root = workspace_root.as_ref();
        let path = Self::path_for_workspace(workspace_root);
        match fs::read_to_string(&path) {
            Ok(local) => Ok(ConfigDiff::new(WORKSPACE_BACKEND_CONFIG_TEMPLATE, &local)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Err(Error::Config(format!(
                "workspace backend local config `{}` does not exist; run `yoi workspace init --workspace {}` first",
                path.display(),
                workspace_root.display()
            ))),
            Err(error) => Err(Error::Io(error)),
        }
    }

    pub fn load_for_workspace(workspace_root: impl AsRef<Path>) -> Result<Self> {
        let path = Self::path_for_workspace(workspace_root);
        match fs::read_to_string(&path) {
            Ok(raw) => Self::parse_str(&raw, &path),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(Error::Io(error)),
        }
    }

    pub fn write_for_workspace(&self, workspace_root: impl AsRef<Path>) -> Result<()> {
        let path = Self::path_for_workspace(workspace_root);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let raw = toml::to_string_pretty(self).map_err(|error| {
            Error::Config(format!(
                "failed to serialize workspace backend config: {error}"
            ))
        })?;
        fs::write(path, raw)?;
        Ok(())
    }

    pub fn parse_str(raw: &str, path: impl AsRef<Path>) -> Result<Self> {
        toml::from_str(raw).map_err(|error| {
            Error::Config(format!(
                "failed to parse workspace backend config `{}`: {error}",
                path.as_ref().display()
            ))
        })
    }

    pub fn resolve(
        &self,
        workspace_root: impl AsRef<Path>,
        identity: WorkspaceIdentity,
    ) -> Result<ResolvedWorkspaceBackendConfig> {
        let workspace_root = workspace_root.as_ref();
        let data_root = self
            .data
            .root
            .as_ref()
            .map(|path| resolve_workspace_path(workspace_root, path))
            .unwrap_or_else(|| {
                ServerConfig::default_workspace_backend_data_root(&identity.workspace_id)
            });
        let database_path = self
            .data
            .workspace_database_path
            .as_ref()
            .map(|path| resolve_workspace_path(workspace_root, path))
            .unwrap_or_else(|| data_root.join("workspace.db"));
        let embedded_runtime_store_root = self
            .data
            .embedded_runtime_store_root
            .as_ref()
            .map(|path| resolve_workspace_path(workspace_root, path))
            .unwrap_or_else(|| data_root.join("embedded-runtime"));
        let listen = self
            .server
            .listen
            .as_deref()
            .unwrap_or(DEFAULT_LISTEN)
            .parse::<SocketAddr>()
            .map_err(|_| {
                Error::Config(format!(
                    "invalid workspace backend server.listen `{}`",
                    self.server.listen.as_deref().unwrap_or(DEFAULT_LISTEN)
                ))
            })?;

        let mut server = ServerConfig::local_dev(workspace_root.to_path_buf(), identity);
        server.frontend_url = self
            .server
            .frontend_url
            .clone()
            .unwrap_or_else(|| DEFAULT_FRONTEND_URL.to_string());
        server.static_assets_dir = self
            .server
            .static_assets_dir
            .as_ref()
            .map(|path| resolve_workspace_path(workspace_root, path));
        server.embedded_runtime_store_root = embedded_runtime_store_root;
        server.max_records = self.limits.max_records.unwrap_or(DEFAULT_MAX_RECORDS);
        server.repositories = self
            .repositories
            .iter()
            .map(|repository| resolve_repository(workspace_root, repository))
            .collect::<Result<Vec<_>>>()?;
        server.remote_runtime_sources = self
            .runtimes
            .remote
            .iter()
            .map(resolve_remote_runtime)
            .collect::<Result<Vec<_>>>()?;
        server.auth = AuthConfig::LocalDevToken {
            token_configured: false,
        };

        Ok(ResolvedWorkspaceBackendConfig {
            server,
            listen,
            database_path,
        })
    }
}

impl ResolvedWorkspaceBackendConfig {
    pub fn with_database_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.database_path = path.into();
        self
    }

    pub fn with_static_assets_dir(mut self, path: Option<PathBuf>) -> Self {
        self.server.static_assets_dir = path;
        self
    }

    pub fn with_listen(mut self, listen: SocketAddr) -> Self {
        self.listen = listen;
        self
    }
}

fn resolve_repository(
    workspace_root: &Path,
    config: &WorkspaceRepositoryConfigFile,
) -> Result<ConfiguredRepository> {
    let id = normalize_required_string("repository id", &config.id)?;
    validate_repository_id(&id)?;
    let provider =
        normalize_required_string("repository provider", &config.provider)?.to_ascii_lowercase();
    let uri = normalize_required_string("repository uri", &config.uri)?;
    let path = resolve_repository_uri(workspace_root, &id, &uri)?;
    let display_name = normalize_optional_string(config.display_name.as_deref());
    let default_selector = normalize_optional_string(config.default_selector.as_deref());

    Ok(ConfiguredRepository {
        id,
        provider,
        uri,
        path,
        display_name,
        default_selector,
    })
}

fn normalize_required_string(field: &str, value: &str) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(Error::Config(format!("{field} must not be empty")));
    }
    Ok(trimmed.to_string())
}

fn normalize_optional_string(value: Option<&str>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn validate_repository_id(id: &str) -> Result<()> {
    if id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    {
        Ok(())
    } else {
        Err(Error::Config(format!(
            "repository id `{id}` must contain only ASCII letters, digits, `_`, `-`, or `.`"
        )))
    }
}

fn resolve_repository_uri(workspace_root: &Path, id: &str, uri: &str) -> Result<PathBuf> {
    if uri.contains("://") {
        return Err(Error::Config(format!(
            "repository `{id}` uses a remote URI, but remote repository materialization is not implemented"
        )));
    }
    Ok(resolve_workspace_path(workspace_root, Path::new(uri)))
}

pub(crate) fn resolve_remote_runtime(
    config: &RemoteRuntimeConfigFile,
) -> Result<RemoteRuntimeConfig> {
    if let Some(token_ref) = config.token_ref.as_deref() {
        return Err(Error::Config(format!(
            "remote runtime `{}` uses token_ref `{token_ref}`, but secret ref resolution is not implemented for workspace backend config yet",
            config.id
        )));
    }
    Ok(RemoteRuntimeConfig::new(
        config.id.clone(),
        config
            .display_name
            .clone()
            .unwrap_or_else(|| config.id.clone()),
        config.endpoint.clone(),
        None,
    ))
}

fn resolve_workspace_path(workspace_root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace_root.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> WorkspaceIdentity {
        WorkspaceIdentity {
            workspace_id: "018f6a2c-1111-7000-8000-000000000001".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            display_name: "Workspace".to_string(),
        }
    }

    #[test]
    fn missing_config_path_uses_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let config = WorkspaceBackendConfigFile::load_for_workspace(dir.path()).unwrap();
        let resolved = config.resolve(dir.path(), identity()).unwrap();

        assert_eq!(resolved.listen, "127.0.0.1:8787".parse().unwrap());
        assert_eq!(resolved.server.frontend_url, DEFAULT_FRONTEND_URL);
        assert_eq!(resolved.server.max_records, DEFAULT_MAX_RECORDS);
        assert!(resolved.database_path.ends_with("workspace.db"));
        assert!(
            resolved
                .server
                .embedded_runtime_store_root
                .ends_with("embedded-runtime")
        );
    }

    #[test]
    fn rejects_unknown_fields() {
        let error = WorkspaceBackendConfigFile::parse_str("[server]\nunknown = true\n", "test")
            .unwrap_err();
        assert!(
            error.to_string().contains("unknown field"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn resolves_relative_paths_against_workspace_root() {
        let dir = tempfile::tempdir().unwrap();
        let config = WorkspaceBackendConfigFile::parse_str(
            r#"
[server]
static_assets_dir = "web/build"

[data]
root = ".yoi/backend-data"
workspace_database_path = ".yoi/custom.db"
embedded_runtime_store_root = ".yoi/runtime-store"
"#,
            "test",
        )
        .unwrap();
        let resolved = config.resolve(dir.path(), identity()).unwrap();

        assert_eq!(
            resolved.server.static_assets_dir,
            Some(dir.path().join("web/build"))
        );
        assert_eq!(resolved.database_path, dir.path().join(".yoi/custom.db"));
        assert_eq!(
            resolved.server.embedded_runtime_store_root,
            dir.path().join(".yoi/runtime-store")
        );
    }

    #[test]
    fn absolute_paths_are_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let config = WorkspaceBackendConfigFile::parse_str(
            r#"
[data]
workspace_database_path = "/tmp/yoi-workspace.db"
embedded_runtime_store_root = "/tmp/yoi-runtime"
"#,
            "test",
        )
        .unwrap();
        let resolved = config.resolve(dir.path(), identity()).unwrap();

        assert_eq!(
            resolved.database_path,
            PathBuf::from("/tmp/yoi-workspace.db")
        );
        assert_eq!(
            resolved.server.embedded_runtime_store_root,
            PathBuf::from("/tmp/yoi-runtime")
        );
    }

    #[test]
    fn data_root_derives_database_and_runtime_store_paths() {
        let dir = tempfile::tempdir().unwrap();
        let config = WorkspaceBackendConfigFile::parse_str(
            r#"
[data]
root = ".local-data"
"#,
            "test",
        )
        .unwrap();
        let resolved = config.resolve(dir.path(), identity()).unwrap();

        assert_eq!(
            resolved.database_path,
            dir.path().join(".local-data/workspace.db")
        );
        assert_eq!(
            resolved.server.embedded_runtime_store_root,
            dir.path().join(".local-data/embedded-runtime")
        );
    }

    #[test]
    fn copies_local_config_without_overwriting() {
        let dir = tempfile::tempdir().unwrap();
        WorkspaceBackendConfigFile::ensure_local_config_for_workspace(dir.path()).unwrap();
        let path = WorkspaceBackendConfigFile::path_for_workspace(dir.path());
        let raw = fs::read_to_string(&path).unwrap();
        assert_eq!(raw, WORKSPACE_BACKEND_CONFIG_TEMPLATE);
        WorkspaceBackendConfigFile::parse_str(&raw, &path).unwrap();

        fs::write(&path, "# custom local config\n").unwrap();
        WorkspaceBackendConfigFile::ensure_local_config_for_workspace(dir.path()).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "# custom local config\n"
        );
    }

    #[test]
    fn local_config_diff_reports_match_and_difference() {
        let dir = tempfile::tempdir().unwrap();
        WorkspaceBackendConfigFile::ensure_local_config_for_workspace(dir.path()).unwrap();
        let matched =
            WorkspaceBackendConfigFile::local_config_diff_for_workspace(dir.path()).unwrap();
        assert!(!matched.differs);

        fs::write(
            WorkspaceBackendConfigFile::path_for_workspace(dir.path()),
            "[server]\nlisten = \"127.0.0.1:9999\"\n",
        )
        .unwrap();
        let diff = WorkspaceBackendConfigFile::local_config_diff_for_workspace(dir.path()).unwrap();
        assert!(diff.differs);
        assert!(diff.text.contains("+++ workspace local"));
        assert!(diff.text.contains("127.0.0.1:9999"));
    }

    #[test]
    fn resolves_repository_uri_relative_to_workspace_root() {
        let dir = tempfile::tempdir().unwrap();
        let config = WorkspaceBackendConfigFile::parse_str(
            r#"
[[repositories]]
id = "main"
provider = "git"
uri = "."
display_name = "Main"
default_selector = "HEAD"
"#,
            "test",
        )
        .unwrap();
        let resolved = config.resolve(dir.path(), identity()).unwrap();
        let repository = resolved.server.repositories.first().unwrap();

        assert_eq!(repository.id, "main");
        assert_eq!(repository.provider, "git");
        assert_eq!(repository.path, dir.path());
        assert_eq!(repository.display_name.as_deref(), Some("Main"));
        assert_eq!(repository.default_selector.as_deref(), Some("HEAD"));
    }

    #[test]
    fn remote_repository_uri_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let config = WorkspaceBackendConfigFile::parse_str(
            r#"
[[repositories]]
id = "main"
provider = "git"
uri = "https://example.com/org/repo.git"
"#,
            "test",
        )
        .unwrap();
        let error = match config.resolve(dir.path(), identity()) {
            Ok(_) => panic!("remote repository URI should fail closed"),
            Err(error) => error,
        };

        assert!(
            error
                .to_string()
                .contains("remote repository materialization is not implemented"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn token_value_field_is_not_in_schema() {
        let error = WorkspaceBackendConfigFile::parse_str(
            r#"
[[runtimes.remote]]
id = "remote"
endpoint = "http://127.0.0.1:8790"
token = "secret"
"#,
            "test",
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("unknown field"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn token_ref_fails_closed_until_secret_resolution_exists() {
        let dir = tempfile::tempdir().unwrap();
        let config = WorkspaceBackendConfigFile::parse_str(
            r#"
[[runtimes.remote]]
id = "remote"
endpoint = "http://127.0.0.1:8790"
token_ref = "local:remote-token"
"#,
            "test",
        )
        .unwrap();
        let error = match config.resolve(dir.path(), identity()) {
            Ok(_) => panic!("token_ref should fail closed until secret resolution exists"),
            Err(error) => error,
        };
        assert!(
            error
                .to_string()
                .contains("secret ref resolution is not implemented"),
            "unexpected error: {error}"
        );
    }
}
