use std::path::Path;

use sha2::{Digest, Sha256};
use url::Url;
use workspace_api::{RepositorySource, RepositorySourceKind};

use crate::{Error, Result};

const MAX_REPOSITORY_SOURCE_BYTES: usize = 4096;

/// Parse and canonicalize a user-authored Git source without accessing the
/// filesystem or network.
pub fn parse_repository_source(value: &str) -> Result<RepositorySource> {
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_REPOSITORY_SOURCE_BYTES {
        return Err(Error::InvalidInput(format!(
            "initial repository source must be between 1 and {MAX_REPOSITORY_SOURCE_BYTES} bytes"
        )));
    }
    if value.chars().any(char::is_control) {
        return Err(Error::InvalidInput(
            "initial repository source must not contain control characters".to_string(),
        ));
    }

    if Path::new(value).is_absolute() {
        return Ok(RepositorySource {
            kind: RepositorySourceKind::LocalPath,
            uri: value.to_string(),
        });
    }

    if is_scp_like_ssh(value) {
        validate_scp_like_ssh(value)?;
        return Ok(RepositorySource {
            kind: RepositorySourceKind::Ssh,
            uri: value.to_string(),
        });
    }

    let parsed = Url::parse(value).map_err(|_| {
        Error::InvalidInput(
            "initial repository source must be an absolute local path or a supported Git URI"
                .to_string(),
        )
    })?;
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(Error::InvalidInput(
            "initial repository source must not contain query parameters or fragments".to_string(),
        ));
    }
    if parsed.password().is_some() {
        return Err(Error::InvalidInput(
            "initial repository source must not embed a password or token".to_string(),
        ));
    }

    let kind = match parsed.scheme() {
        "file" => {
            if !parsed.username().is_empty() {
                return Err(Error::InvalidInput(
                    "file repository URI must not contain user information".to_string(),
                ));
            }
            if parsed.host_str().is_some_and(|host| host != "localhost") {
                return Err(Error::InvalidInput(
                    "file repository URI host must be empty or localhost".to_string(),
                ));
            }
            parsed.to_file_path().map_err(|_| {
                Error::InvalidInput("file repository URI must contain an absolute path".to_string())
            })?;
            RepositorySourceKind::File
        }
        "ssh" => {
            require_remote_host_and_path(&parsed)?;
            RepositorySourceKind::Ssh
        }
        "http" | "https" => {
            if !parsed.username().is_empty() {
                return Err(Error::InvalidInput(
                    "HTTP repository URI must not contain user information".to_string(),
                ));
            }
            require_remote_host_and_path(&parsed)?;
            if parsed.scheme() == "http" {
                RepositorySourceKind::Http
            } else {
                RepositorySourceKind::Https
            }
        }
        scheme => {
            return Err(Error::InvalidInput(format!(
                "unsupported initial repository source scheme `{scheme}`"
            )));
        }
    };

    Ok(RepositorySource {
        kind,
        uri: parsed.to_string(),
    })
}

/// Classify persisted pre-source-contract rows without guessing a usable remote
/// when the legacy value is malformed. No filesystem or network access occurs.
pub fn classify_legacy_repository_source(value: &str) -> RepositorySource {
    parse_repository_source(value).unwrap_or_else(|_| RepositorySource {
        kind: RepositorySourceKind::Invalid,
        uri: value.trim().to_string(),
    })
}

pub fn repository_source_fingerprint(source: &RepositorySource) -> String {
    let payload = serde_json::to_vec(source).expect("Repository source serializes");
    let mut hasher = Sha256::new();
    hasher.update(b"yoi.repository-source.v1\0");
    hasher.update(payload);
    let digest = hasher.finalize();
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    format!("sha256:{encoded}")
}

fn require_remote_host_and_path(parsed: &Url) -> Result<()> {
    if parsed.host_str().is_none() || parsed.path().is_empty() || parsed.path() == "/" {
        return Err(Error::InvalidInput(
            "remote repository URI must contain a host and repository path".to_string(),
        ));
    }
    Ok(())
}

fn is_scp_like_ssh(value: &str) -> bool {
    !value.contains("://")
        && value
            .split_once(':')
            .is_some_and(|(identity, _)| identity.contains('@'))
}

fn validate_scp_like_ssh(value: &str) -> Result<()> {
    let (identity, path) = value.split_once(':').ok_or_else(|| {
        Error::InvalidInput("scp-like SSH source must contain `host:path`".to_string())
    })?;
    let (username, host) = identity.split_once('@').ok_or_else(|| {
        Error::InvalidInput("scp-like SSH source must contain `user@host:path`".to_string())
    })?;
    if username.is_empty()
        || host.is_empty()
        || path.is_empty()
        || username.contains('@')
        || username.contains(':')
        || host.contains('@')
        || path.starts_with('-')
        || value.contains('?')
        || value.contains('#')
        || value.chars().any(char::is_whitespace)
    {
        return Err(Error::InvalidInput(
            "scp-like SSH source must use `user@host:path` without credentials or parameters"
                .to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_local_file_ssh_http_and_https_sources_without_io() {
        let cases = [
            ("/runtime/repos/project", RepositorySourceKind::LocalPath),
            ("file:///runtime/repos/project", RepositorySourceKind::File),
            (
                "ssh://git@example.test/org/project.git",
                RepositorySourceKind::Ssh,
            ),
            (
                "git@example.test:org/project.git",
                RepositorySourceKind::Ssh,
            ),
            (
                "http://git.test/org/project.git",
                RepositorySourceKind::Http,
            ),
            (
                "https://git.test/org/project.git",
                RepositorySourceKind::Https,
            ),
        ];
        for (source, expected_kind) in cases {
            assert_eq!(parse_repository_source(source).unwrap().kind, expected_kind);
        }
    }

    #[test]
    fn rejects_relative_unsupported_and_credential_bearing_sources() {
        for source in [
            "relative/project",
            "ftp://git.test/project.git",
            "https://user@git.test/project.git",
            "https://git.test/project.git?token=secret",
            "ssh://git:secret@git.test/project.git",
            "git@example.test:",
            "git:secret@example.test:org/project.git",
            "https://git.test/project.git\nother",
        ] {
            assert!(
                parse_repository_source(source).is_err(),
                "accepted {source:?}"
            );
        }
    }

    #[test]
    fn fingerprint_uses_canonical_source_identity() {
        let first = parse_repository_source(" https://EXAMPLE.test/a/../project.git ").unwrap();
        let second = parse_repository_source("https://example.test/project.git").unwrap();
        assert_eq!(first, second);
        assert_eq!(
            repository_source_fingerprint(&first),
            repository_source_fingerprint(&second)
        );
    }
}
