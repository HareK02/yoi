use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::{fs, io};

use serde::{Deserialize, Serialize};
use url::Url;

use crate::identity::WorkspaceIdentity;
use crate::server::{AuthConfig, ServerConfig};
use crate::{Error, Result};

pub const SERVER_HOST_CONFIG_FILE_NAME: &str = "server.toml";
const DEFAULT_LISTEN: &str = "127.0.0.1:8787";
const DEFAULT_BROWSER_PUBLIC_URL: &str = "http://localhost:5173";
const DEFAULT_AUTH_COOKIE_NAME: &str = "yoi_workspace_session";
const DEFAULT_MAX_RECORDS: usize = 200;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ServerHostConfigFile {
    #[serde(default)]
    pub browser: ServerBrowserConfig,
}

impl Default for ServerHostConfigFile {
    fn default() -> Self {
        Self {
            browser: ServerBrowserConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ServerBrowserConfig {
    #[serde(default = "default_browser_public_url")]
    pub public_url: String,
}

impl Default for ServerBrowserConfig {
    fn default() -> Self {
        Self {
            public_url: default_browser_public_url(),
        }
    }
}

fn default_browser_public_url() -> String {
    DEFAULT_BROWSER_PUBLIC_URL.to_string()
}

#[derive(Clone)]
pub struct ResolvedWorkspaceBackendConfig {
    pub server: ServerConfig,
    pub listen: SocketAddr,
    pub database_path: PathBuf,
}

impl ServerHostConfigFile {
    pub fn path_for_config_dir(config_dir: impl AsRef<Path>) -> PathBuf {
        config_dir.as_ref().join(SERVER_HOST_CONFIG_FILE_NAME)
    }

    pub fn default_path() -> Option<PathBuf> {
        manifest::paths::config_dir().map(Self::path_for_config_dir)
    }

    pub fn load_default() -> Result<Self> {
        let Some(path) = Self::default_path() else {
            return Ok(Self::default());
        };
        match fs::read_to_string(&path) {
            Ok(raw) => Self::parse_str(&raw, &path),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(Error::Io(error)),
        }
    }

    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let raw = fs::read_to_string(path).map_err(|error| {
            Error::Config(format!(
                "failed to read Server host config `{}`: {error}",
                path.display()
            ))
        })?;
        Self::parse_str(&raw, path)
    }

    pub fn parse_str(raw: &str, path: impl AsRef<Path>) -> Result<Self> {
        toml::from_str(raw).map_err(|error| {
            Error::Config(format!(
                "failed to parse Server host config `{}`: {error}",
                path.as_ref().display()
            ))
        })
    }
}

impl ResolvedWorkspaceBackendConfig {
    pub fn local_dev(
        workspace_root: impl AsRef<Path>,
        identity: WorkspaceIdentity,
        host_config: &ServerHostConfigFile,
    ) -> Result<Self> {
        let workspace_root = workspace_root.as_ref();
        let data_root = ServerConfig::default_workspace_backend_data_root(&identity.workspace_id);
        let database_path = ServerConfig::default_server_database_path();
        let (browser_public_url, browser_rp_id) =
            resolve_browser_public_url(&host_config.browser.public_url)?;
        let mut server = ServerConfig::local_dev(workspace_root.to_path_buf(), identity);
        server.database_path = database_path.clone();
        server.embedded_runtime_store_root = data_root.join("embedded-runtime");
        server.max_records = DEFAULT_MAX_RECORDS;
        server.remote_runtime_sources = Vec::new();
        server.auth = AuthConfig::Passkey {
            rp_id: browser_rp_id,
            origin: browser_public_url.clone(),
            public_base_url: browser_public_url,
            cookie_name: DEFAULT_AUTH_COOKIE_NAME.to_string(),
        };
        let listen = DEFAULT_LISTEN.parse::<SocketAddr>().map_err(|error| {
            Error::Config(format!("invalid built-in Server listen address: {error}"))
        })?;

        Ok(Self {
            server,
            listen,
            database_path,
        })
    }
}

impl ResolvedWorkspaceBackendConfig {
    pub fn with_backend_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.server.backend_base_url = Some(base_url.into().trim_end_matches('/').to_string());
        self
    }

    pub fn with_listen(mut self, listen: SocketAddr) -> Self {
        self.listen = listen;
        self
    }
}

fn normalize_required_string(field: &str, value: &str) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(Error::Config(format!("{field} must not be empty")));
    }
    Ok(trimmed.to_string())
}

fn resolve_browser_public_url(value: &str) -> Result<(String, String)> {
    let value = normalize_required_string("browser.public_url", value)?;
    let url = Url::parse(&value).map_err(|error| {
        Error::Config(format!(
            "browser.public_url must be an absolute http(s) URL: {error}"
        ))
    })?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(Error::Config(
            "browser.public_url must use the http or https scheme".to_string(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(Error::Config(
            "browser.public_url must not contain user information".to_string(),
        ));
    }
    if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
        return Err(Error::Config(
            "browser.public_url must contain only an origin without a path, query, or fragment"
                .to_string(),
        ));
    }
    let rp_id = url
        .host_str()
        .ok_or_else(|| Error::Config("browser.public_url must contain a host".to_string()))?
        .to_string();
    Ok((url.origin().ascii_serialization(), rp_id))
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

    fn resolved() -> ResolvedWorkspaceBackendConfig {
        let dir = tempfile::tempdir().unwrap();
        ResolvedWorkspaceBackendConfig::local_dev(
            dir.path(),
            identity(),
            &ServerHostConfigFile::default(),
        )
        .unwrap()
    }

    #[test]
    fn default_settings_resolve_without_a_repository_file() {
        let resolved = resolved();

        assert_eq!(resolved.listen, "127.0.0.1:8787".parse().unwrap());
        let AuthConfig::Passkey {
            rp_id,
            origin,
            public_base_url,
            ..
        } = &resolved.server.auth;
        assert_eq!(rp_id, "localhost");
        assert_eq!(origin, DEFAULT_BROWSER_PUBLIC_URL);
        assert_eq!(public_base_url, DEFAULT_BROWSER_PUBLIC_URL);
        assert_eq!(resolved.server.max_records, DEFAULT_MAX_RECORDS);
        assert!(resolved.database_path.ends_with("server.db"));
        assert!(
            resolved
                .server
                .embedded_runtime_store_root
                .ends_with("embedded-runtime")
        );
    }

    #[test]
    fn backend_base_url_is_explicit_and_normalized() {
        let listen = "127.0.0.1:48787".parse().unwrap();
        let resolved = resolved()
            .with_listen(listen)
            .with_backend_base_url("http://127.0.0.1:48787/");

        assert_eq!(resolved.listen, listen);
        assert_eq!(
            resolved.server.backend_base_url.as_deref(),
            Some("http://127.0.0.1:48787")
        );
    }

    #[test]
    fn browser_public_url_from_host_config_drives_all_browser_auth_settings() {
        let host_config = ServerHostConfigFile::parse_str(
            "[browser]\npublic_url = \"https://Yoi.Example:443/\"\n",
            "server.toml",
        )
        .unwrap();
        let resolved = ResolvedWorkspaceBackendConfig::local_dev(
            tempfile::tempdir().unwrap().path(),
            identity(),
            &host_config,
        )
        .unwrap();

        let AuthConfig::Passkey {
            rp_id,
            origin,
            public_base_url,
            ..
        } = &resolved.server.auth;
        assert_eq!(rp_id, "yoi.example");
        assert_eq!(origin, "https://yoi.example");
        assert_eq!(public_base_url, "https://yoi.example");
    }

    #[test]
    fn browser_public_url_rejects_non_origin_urls() {
        for value in [
            "https://example.test/path",
            "https://example.test?query=true",
            "file:///tmp/web",
        ] {
            let host_config = ServerHostConfigFile {
                browser: ServerBrowserConfig {
                    public_url: value.to_string(),
                },
            };
            let result = ResolvedWorkspaceBackendConfig::local_dev(
                tempfile::tempdir().unwrap().path(),
                identity(),
                &host_config,
            );
            let error = match result {
                Ok(_) => panic!("expected {value} to be rejected"),
                Err(error) => error,
            };
            assert!(
                error.to_string().contains("browser.public_url"),
                "unexpected error for {value}: {error}"
            );
        }
    }

    #[test]
    fn server_host_config_loads_only_from_the_explicit_host_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = ServerHostConfigFile::path_for_config_dir(dir.path());
        fs::write(
            &path,
            "[browser]\npublic_url = \"https://deploy.example.test\"\n",
        )
        .unwrap();

        let loaded = ServerHostConfigFile::load_from_path(&path).unwrap();
        assert_eq!(loaded.browser.public_url, "https://deploy.example.test");
        assert_eq!(path, dir.path().join("server.toml"));
    }

    #[test]
    fn explicit_missing_server_host_config_fails_closed() {
        let error = ServerHostConfigFile::load_from_path("/missing/yoi/server.toml").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("failed to read Server host config")
        );
    }

    #[test]
    fn local_host_config_does_not_supply_runtime_authority() {
        let resolved = resolved();
        assert!(resolved.server.remote_runtime_sources.is_empty());
    }
}
