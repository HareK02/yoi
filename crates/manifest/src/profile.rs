//! Nix profile resolution.
//!
//! Profiles are a human-authored Nix entrypoint that evaluates to a typed
//! resolved artifact. Rust consumes the evaluated JSON artifact directly and
//! validates it into the existing [`crate::PodManifest`] runtime contract.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::{PodManifest, PodManifestConfig, ResolveError};

const PROFILE_FORMAT_V1: &str = "insomnia.nix-profile.v1";

/// User selection of a profile source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProfileSelector {
    /// A local Nix expression evaluated with `nix eval --json --file <path>`.
    Path { path: PathBuf },
}

impl ProfileSelector {
    pub fn path(path: impl Into<PathBuf>) -> Self {
        Self::Path { path: path.into() }
    }
}

/// Profile source recorded with a resolved artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProfileSource {
    Path { path: PathBuf },
}

/// Metadata optionally emitted by `mkProfile`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

/// Profile provenance embedded in a resolved manifest snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileManifestSnapshot {
    pub source: ProfileSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<ProfileMetadata>,
}

/// Validated result of evaluating and resolving a profile.
#[derive(Debug, Clone)]
pub struct ResolvedProfile {
    pub source: ProfileSource,
    pub profile: Option<ProfileMetadata>,
    pub manifest: PodManifest,
    /// The validated runtime manifest as JSON. This is the snapshot shape future
    /// Pod restore should prefer over re-evaluating the Nix source.
    pub manifest_snapshot: serde_json::Value,
    /// Raw JSON returned by Nix, retained for diagnostics/debugging.
    pub raw_artifact: serde_json::Value,
}

/// External-command based Nix resolver.
#[derive(Debug, Clone)]
pub struct NixProfileResolver {
    nix_bin: PathBuf,
}

impl Default for NixProfileResolver {
    fn default() -> Self {
        Self {
            nix_bin: PathBuf::from("nix"),
        }
    }
}

impl NixProfileResolver {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_nix_bin(nix_bin: impl Into<PathBuf>) -> Self {
        Self {
            nix_bin: nix_bin.into(),
        }
    }

    pub fn resolve(&self, selector: &ProfileSelector) -> Result<ResolvedProfile, ProfileError> {
        match selector {
            ProfileSelector::Path { path } => self.resolve_path(path),
        }
    }

    fn resolve_path(&self, path: &Path) -> Result<ResolvedProfile, ProfileError> {
        let absolute_path = absolutize(path)?;
        let base_dir = absolute_path
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| ProfileError::InvalidPath {
                path: absolute_path.clone(),
                message: "profile path has no parent directory".to_string(),
            })?;

        let output = Command::new(&self.nix_bin)
            .arg("eval")
            .arg("--json")
            .arg("--file")
            .arg(&absolute_path)
            .output()
            .map_err(|source| {
                if source.kind() == std::io::ErrorKind::NotFound {
                    ProfileError::NixUnavailable {
                        nix_bin: self.nix_bin.clone(),
                        profile: absolute_path.clone(),
                    }
                } else {
                    ProfileError::CommandIo {
                        path: absolute_path.clone(),
                        source,
                    }
                }
            })?;

        if !output.status.success() {
            return Err(ProfileError::NixFailed {
                path: absolute_path,
                status: output.status.code(),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            });
        }

        let raw_artifact: serde_json::Value =
            serde_json::from_slice(&output.stdout).map_err(|source| ProfileError::JsonParse {
                path: absolute_path.clone(),
                source,
            })?;

        resolve_profile_artifact(
            ProfileSource::Path {
                path: absolute_path,
            },
            &base_dir,
            raw_artifact,
        )
    }
}

/// Resolve an already-evaluated profile artifact. Tests and future non-Nix
/// resolvers use this to share artifact validation semantics.
pub fn resolve_profile_artifact(
    source: ProfileSource,
    base_dir: &Path,
    raw_artifact: serde_json::Value,
) -> Result<ResolvedProfile, ProfileError> {
    if !base_dir.is_absolute() {
        return Err(ProfileError::InvalidPath {
            path: base_dir.to_path_buf(),
            message: "profile base directory must be absolute".to_string(),
        });
    }

    let envelope: ProfileEnvelope = serde_json::from_value(raw_artifact.clone())
        .map_err(|source| ProfileError::ArtifactShape { source })?;
    envelope.validate_format()?;

    let manifest_value = extract_manifest_value(&raw_artifact)?;
    let config: PodManifestConfig = serde_json::from_value(manifest_value.clone())
        .map_err(|source| ProfileError::ManifestDeserialize { source })?;
    let config = PodManifestConfig::builtin_defaults().merge(config.resolve_paths(base_dir));
    let mut manifest = PodManifest::try_from(config).map_err(ProfileError::ManifestResolve)?;
    manifest.profile = Some(ProfileManifestSnapshot {
        source: source.clone(),
        profile: envelope.profile.clone(),
    });
    let manifest_snapshot =
        serde_json::to_value(&manifest).map_err(ProfileError::SnapshotSerialize)?;

    Ok(ResolvedProfile {
        source,
        profile: envelope.profile,
        manifest,
        manifest_snapshot,
        raw_artifact,
    })
}

#[derive(Debug, Deserialize)]
struct ProfileEnvelope {
    #[serde(default)]
    profile: Option<ProfileMetadata>,
}

impl ProfileEnvelope {
    fn validate_format(&self) -> Result<(), ProfileError> {
        let Some(profile) = &self.profile else {
            return Ok(());
        };
        match profile.format.as_deref() {
            None | Some(PROFILE_FORMAT_V1) => Ok(()),
            Some(found) => Err(ProfileError::UnsupportedFormat {
                found: found.to_string(),
            }),
        }
    }
}

fn extract_manifest_value(raw: &serde_json::Value) -> Result<serde_json::Value, ProfileError> {
    match raw {
        serde_json::Value::Object(map) => {
            let manifest = map.get("manifest");
            let config = map.get("config");
            match (manifest, config) {
                (Some(_), Some(_)) => Err(ProfileError::InvalidArtifact(
                    "profile artifact must not contain both `manifest` and `config`".to_string(),
                )),
                (Some(value), None) | (None, Some(value)) => Ok(value.clone()),
                (None, None) => Ok(raw.clone()),
            }
        }
        _ => Err(ProfileError::InvalidArtifact(
            "profile artifact must be a JSON object".to_string(),
        )),
    }
}

fn absolutize(path: &Path) -> Result<PathBuf, ProfileError> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        let cwd = std::env::current_dir().map_err(|source| ProfileError::CommandIo {
            path: PathBuf::from("."),
            source,
        })?;
        Ok(cwd.join(path))
    }
}

/// Errors raised while evaluating and validating a profile.
#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("invalid profile path {}: {message}", .path.display())]
    InvalidPath { path: PathBuf, message: String },

    #[error("Nix profile resolution requires the `nix` command ({}) but it was not found while resolving {}; install Nix or use --manifest with a resolved TOML manifest", .nix_bin.display(), .profile.display())]
    NixUnavailable { nix_bin: PathBuf, profile: PathBuf },

    #[error("failed to execute nix for profile {}: {source}", .path.display())]
    CommandIo {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("nix eval failed for profile {} (status {}): {stderr}", .path.display(), status.map_or_else(|| "signal".to_string(), |s| s.to_string()))]
    NixFailed {
        path: PathBuf,
        status: Option<i32>,
        stderr: String,
    },

    #[error("nix eval did not produce valid JSON for profile {}: {source}", .path.display())]
    JsonParse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("failed to decode profile artifact envelope: {source}")]
    ArtifactShape {
        #[source]
        source: serde_json::Error,
    },

    #[error("unsupported profile artifact format: {found}")]
    UnsupportedFormat { found: String },

    #[error("invalid profile artifact: {0}")]
    InvalidArtifact(String),

    #[error("failed to decode profile manifest/config: {source}")]
    ManifestDeserialize {
        #[source]
        source: serde_json::Error,
    },

    #[error("failed to resolve profile manifest/config: {0}")]
    ManifestResolve(#[source] ResolveError),

    #[error("failed to serialize resolved manifest snapshot: {0}")]
    SnapshotSerialize(#[source] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AuthRef, Permission, SchemeKind};

    fn artifact() -> serde_json::Value {
        serde_json::json!({
            "profile": {
                "format": "insomnia.nix-profile.v1",
                "name": "coder",
                "description": "Coder profile"
            },
            "manifest": {
                "pod": { "name": "coder-pod" },
                "model": {
                    "scheme": "anthropic",
                    "model_id": "claude-sonnet-4-20250514",
                    "auth": { "kind": "secret_ref", "ref": "llm.anthropic.default" }
                },
                "scope": {
                    "allow": [
                        { "target": ".", "permission": "write" }
                    ]
                }
            }
        })
    }

    #[test]
    fn resolves_profile_artifact_with_relative_paths() {
        let resolved = resolve_profile_artifact(
            ProfileSource::Path {
                path: PathBuf::from("/profiles/coder.nix"),
            },
            Path::new("/workspace/project"),
            artifact(),
        )
        .unwrap();

        assert_eq!(
            resolved.profile.as_ref().unwrap().name.as_deref(),
            Some("coder")
        );
        assert_eq!(resolved.manifest.pod.name, "coder-pod");
        assert_eq!(resolved.manifest.model.scheme, Some(SchemeKind::Anthropic));
        assert_eq!(
            resolved.manifest.scope.allow[0].target,
            PathBuf::from("/workspace/project")
        );
        assert_eq!(
            resolved.manifest.scope.allow[0].permission,
            Permission::Write
        );
        assert!(matches!(
            resolved.manifest.model.auth,
            Some(AuthRef::SecretRef { ref_ }) if ref_ == "llm.anthropic.default"
        ));
        assert_eq!(
            resolved.manifest_snapshot["model"]["auth"],
            serde_json::json!({ "kind": "secret_ref", "ref": "llm.anthropic.default" })
        );
    }

    #[test]
    fn rejects_both_manifest_and_config_fields() {
        let err = resolve_profile_artifact(
            ProfileSource::Path {
                path: PathBuf::from("/profiles/bad.nix"),
            },
            Path::new("/workspace/project"),
            serde_json::json!({ "manifest": {}, "config": {} }),
        )
        .unwrap_err();

        assert!(matches!(err, ProfileError::InvalidArtifact(_)));
    }

    #[test]
    fn accepts_raw_manifest_object_for_debug_paths() {
        let raw = serde_json::json!({
            "pod": { "name": "raw" },
            "model": { "scheme": "anthropic", "model_id": "claude-sonnet-4-20250514" },
            "scope": { "allow": [{ "target": "/tmp/raw", "permission": "read" }] }
        });

        let resolved = resolve_profile_artifact(
            ProfileSource::Path {
                path: PathBuf::from("/profiles/raw.nix"),
            },
            Path::new("/profiles"),
            raw,
        )
        .unwrap();

        assert_eq!(resolved.manifest.pod.name, "raw");
        assert_eq!(
            resolved.manifest.scope.allow[0].target,
            PathBuf::from("/tmp/raw")
        );
    }

    #[test]
    fn rejects_unknown_profile_format() {
        let mut raw = artifact();
        raw["profile"]["format"] = serde_json::json!("insomnia.nix-profile.v99");

        let err = resolve_profile_artifact(
            ProfileSource::Path {
                path: PathBuf::from("/profiles/coder.nix"),
            },
            Path::new("/workspace/project"),
            raw,
        )
        .unwrap_err();

        assert!(matches!(err, ProfileError::UnsupportedFormat { .. }));
    }

    #[test]
    fn missing_nix_has_clear_diagnostic() {
        let resolver = NixProfileResolver::with_nix_bin("/definitely/missing/nix");
        let err = resolver
            .resolve(&ProfileSelector::path("/profiles/coder.nix"))
            .unwrap_err();

        assert!(matches!(err, ProfileError::NixUnavailable { .. }));
        assert!(err.to_string().contains("requires the `nix` command"));
        assert!(err.to_string().contains("--manifest"));
    }
}
