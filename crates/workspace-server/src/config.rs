use std::collections::HashSet;
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::{fs, io};

use serde::{Deserialize, Serialize};
use url::Url;

use crate::server::{AuthConfig, ServerConfig};
use crate::store::WorkspaceRecord;
use crate::{Error, Result};

pub const SERVER_HOST_CONFIG_FILE_NAME: &str = "server.toml";
const DEFAULT_LISTEN: &str = "127.0.0.1:8787";
const DEFAULT_AUTH_COOKIE_NAME: &str = "yoi_workspace_session";
const DEFAULT_MAX_RECORDS: usize = 200;

// Examples remain comments: generating a file must not authorize a browser origin.
const SERVER_HOST_CONFIG_TEMPLATE: &str = "\
[browser]
# Set the URLs people actually open in their browsers before starting the Server.
# HTTPS is required except for local HTTP access. Include every allowed origin.
# The first URL is used for device-login links when no browser origin is supplied.
# Example:
# public_urls = [\"https://example.com\", \"http://localhost:8787\"]
";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ServerHostConfigFile {
    pub browser: ServerBrowserConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ServerBrowserConfig {
    pub public_urls: Vec<String>,
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
        let path = Self::default_path().ok_or_else(|| {
            Error::Config("Server config directory is unavailable; specify --config <PATH> and create its template with `yoi-server init-config --config <PATH>`".into())
        })?;
        Self::load_from_path(path)
    }

    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let raw = fs::read_to_string(path).map_err(|error| {
            Error::Config(format!(
                "failed to read Server host config `{}`: {error}. Create a template with `yoi-server init-config --config <PATH>` using this path, then set browser.public_urls to your actual browser origins",
                path.display()
            ))
        })?;
        Self::parse_str(&raw, path)
    }

    pub fn parse_str(raw: &str, path: impl AsRef<Path>) -> Result<Self> {
        let parse = || -> Result<Self> {
            let mut config: Self =
                toml::from_str(raw).map_err(|error| Error::Config(error.to_string()))?;
            config.browser.public_urls = resolve_browser_public_urls(&config.browser.public_urls)?;
            Ok(config)
        };
        parse().map_err(|error| {
            Error::Config(format!(
                "invalid Server host config `{}`: {error}. Explicitly set [browser] public_urls to a nonempty list of actual browser origins (example: [\"https://example.com\"]); no default URL is used",
                path.as_ref().display()
            ))
        })
    }

    pub fn create_template(path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|error| Error::Config(format!(
                "cannot create Server config template `{}`: {error}; existing files are never overwritten",
                path.display()
            )))?;
        file.write_all(SERVER_HOST_CONFIG_TEMPLATE.as_bytes())?;
        file.sync_all()?;
        Ok(())
    }
}

impl ResolvedWorkspaceBackendConfig {
    pub fn local_dev(
        workspace_root: impl AsRef<Path>,
        workspace: WorkspaceRecord,
        host_config: &ServerHostConfigFile,
    ) -> Result<Self> {
        let workspace_root = workspace_root.as_ref();
        let public_urls = resolve_browser_public_urls(&host_config.browser.public_urls)?;
        let mut server = ServerConfig::local_dev(workspace_root.to_path_buf(), workspace);
        let database_path = server.database_path.clone();
        server.max_records = DEFAULT_MAX_RECORDS;
        server.remote_runtime_sources = Vec::new();
        server.auth = AuthConfig::Passkey {
            public_urls,
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

    pub fn with_backend_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.server.backend_base_url = Some(base_url.into().trim_end_matches('/').to_string());
        self
    }

    pub fn with_listen(mut self, listen: SocketAddr) -> Self {
        self.listen = listen;
        self
    }
}

fn resolve_browser_public_urls(values: &[String]) -> Result<Vec<String>> {
    if values.is_empty() {
        return Err(Error::Config(
            "browser.public_urls must not be empty".into(),
        ));
    }
    let mut seen = HashSet::new();
    values
        .iter()
        .map(|value| {
            let origin = resolve_browser_public_url(value)?;
            if !seen.insert(origin.clone()) {
                return Err(Error::Config(format!(
                    "browser.public_urls contains duplicate origin `{origin}`"
                )));
            }
            Ok(origin)
        })
        .collect()
}

fn resolve_browser_public_url(value: &str) -> Result<String> {
    let url = Url::parse(value.trim()).map_err(|error| {
        Error::Config(format!(
            "browser.public_urls must contain absolute http(s) URLs: {error}"
        ))
    })?;
    let local = match url.host() {
        Some(url::Host::Domain(host)) => host == "localhost" || host.ends_with(".localhost"),
        Some(url::Host::Ipv4(host)) => host.is_loopback(),
        Some(url::Host::Ipv6(host)) => host.is_loopback(),
        None => false,
    };
    if url.scheme() != "https" && !(url.scheme() == "http" && local) {
        return Err(Error::Config(
            "browser.public_urls requires HTTPS except for localhost or loopback HTTP".into(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(Error::Config(
            "browser.public_urls must not contain user information".into(),
        ));
    }
    if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
        return Err(Error::Config(
            "browser.public_urls must contain only origins without paths, queries, or fragments"
                .into(),
        ));
    }
    if url.host_str().is_none() {
        return Err(Error::Config(
            "browser.public_urls must contain a host".into(),
        ));
    }
    Ok(url.origin().ascii_serialization())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> WorkspaceRecord {
        WorkspaceRecord {
            workspace_id: "018f6a2c-1111-7000-8000-000000000001".into(),
            owner_account_id: "018f6a2c-1111-7000-8000-000000000002".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            display_name: "Workspace".into(),
            state: "active".into(),
        }
    }

    fn resolved() -> ResolvedWorkspaceBackendConfig {
        let config = ServerHostConfigFile::parse_str(
            "[browser]\npublic_urls = [\"https://deploy.example.test\", \"http://localhost:8787\"]",
            "server.toml",
        )
        .unwrap();
        ResolvedWorkspaceBackendConfig::local_dev(
            tempfile::tempdir().unwrap().path(),
            workspace(),
            &config,
        )
        .unwrap()
    }

    #[test]
    fn explicit_browser_origins_are_independent_of_listen_and_backend_url() {
        let listen = "127.0.0.1:48787".parse().unwrap();
        let resolved = resolved()
            .with_listen(listen)
            .with_backend_base_url("http://127.0.0.1:48787/");
        assert_eq!(resolved.listen, listen);
        assert_eq!(
            resolved.server.backend_base_url.as_deref(),
            Some("http://127.0.0.1:48787")
        );
        let AuthConfig::Passkey { public_urls, .. } = &resolved.server.auth;
        assert_eq!(
            public_urls,
            &["https://deploy.example.test", "http://localhost:8787"]
        );
        assert!(resolved.server.remote_runtime_sources.is_empty());
        assert!(resolved.database_path.ends_with("server.db"));
    }

    #[test]
    fn missing_empty_and_old_single_url_settings_fail_with_path_and_guidance() {
        for raw in [
            "",
            "[browser]",
            "[browser]\npublic_urls = []",
            "[browser]\npublic_url = \"https://example.test\"",
        ] {
            let error = ServerHostConfigFile::parse_str(raw, "custom/server.toml")
                .unwrap_err()
                .to_string();
            assert!(error.contains("custom/server.toml"), "{error}");
            assert!(
                error.contains("browser") && error.contains("public_urls"),
                "{error}"
            );
        }
    }

    #[test]
    fn browser_origins_are_normalized_and_duplicates_rejected() {
        let config = ServerHostConfigFile::parse_str(
            "[browser]\npublic_urls = [\" https://Yoi.Example:443/ \", \"http://localhost:8787/\"]",
            "server.toml",
        )
        .unwrap();
        assert_eq!(
            config.browser.public_urls,
            ["https://yoi.example", "http://localhost:8787"]
        );
        let error = ServerHostConfigFile::parse_str(
            "[browser]\npublic_urls = [\"https://Yoi.Example:443/\", \"https://yoi.example\"]",
            "server.toml",
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("duplicate origin"), "{error}");
    }

    #[test]
    fn browser_origins_reject_insecure_remote_and_non_origin_urls() {
        for value in [
            "",
            "https://example.test/path",
            "https://example.test?query=true",
            "https://example.test/#fragment",
            "file:///tmp/web",
            "https://user:secret@example.test",
            "http://example.test",
            "http://localhost.evil.test",
            "http://192.168.1.1",
        ] {
            assert!(
                resolve_browser_public_url(value).is_err(),
                "accepted {value}"
            );
        }
        for value in [
            "https://example.test",
            "http://localhost:8787",
            "http://127.0.0.1:8787",
            "http://[::1]:8787",
        ] {
            assert!(
                resolve_browser_public_url(value).is_ok(),
                "rejected {value}"
            );
        }
    }

    #[test]
    fn missing_config_fails_without_creating_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = ServerHostConfigFile::path_for_config_dir(dir.path());
        let error = ServerHostConfigFile::load_from_path(&path)
            .unwrap_err()
            .to_string();
        assert!(error.contains(path.to_str().unwrap()));
        assert!(error.contains("init-config"));
        assert!(!path.exists());
    }

    #[test]
    fn explicit_template_creation_requires_editing_and_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config/server.toml");
        ServerHostConfigFile::create_template(&path).unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.contains("# public_urls = [\"https://example.com\""));
        assert!(!raw.contains("5173"));
        assert!(ServerHostConfigFile::load_from_path(&path).is_err());
        fs::write(
            &path,
            "[browser]\npublic_urls = [\"https://deploy.example.test\"]",
        )
        .unwrap();
        assert!(ServerHostConfigFile::create_template(&path).is_err());
        let loaded = ServerHostConfigFile::load_from_path(&path).unwrap();
        assert_eq!(loaded.browser.public_urls, ["https://deploy.example.test"]);
    }
}
