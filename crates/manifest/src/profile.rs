//! Nix profile resolution.
//!
//! Profiles are a human-authored Nix entrypoint that evaluates to a typed
//! resolved artifact. Rust consumes the evaluated JSON artifact directly and
//! validates it into the existing [`crate::PodManifest`] runtime contract.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::{PodManifest, PodManifestConfig, ResolveError, paths};

const PROFILE_FORMAT_V1: &str = "insomnia.nix-profile.v1";
const BUILTIN_DEFAULT_PROFILE_NAME: &str = "default";

/// Registry source for discovered profiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileRegistrySource {
    Builtin,
    User,
    Project,
}

impl ProfileRegistrySource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::User => "user",
            Self::Project => "project",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "builtin" => Some(Self::Builtin),
            "user" => Some(Self::User),
            "project" => Some(Self::Project),
            _ => None,
        }
    }
}

impl std::fmt::Display for ProfileRegistrySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// User selection of a profile source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProfileSelector {
    /// A local Nix expression evaluated with `nix eval --json --file <path>`.
    Path { path: PathBuf },
    /// A named profile discovered from builtin/user/project registries.
    Named {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<ProfileRegistrySource>,
        name: String,
    },
    /// The effective default from the discovered profile registry.
    Default,
}

impl ProfileSelector {
    pub fn path(path: impl Into<PathBuf>) -> Self {
        Self::Path { path: path.into() }
    }

    pub fn named(name: impl Into<String>) -> Self {
        Self::Named {
            source: None,
            name: name.into(),
        }
    }

    pub fn source_named(source: ProfileRegistrySource, name: impl Into<String>) -> Self {
        Self::Named {
            source: Some(source),
            name: name.into(),
        }
    }

    /// Parse the CLI/TUI `--profile` argument.
    ///
    /// `path:<path>` always selects an explicit path. `builtin:<name>`,
    /// `user:<name>`, and `project:<name>` require the given registry source.
    /// Unqualified path-like values (containing `/`, starting with `.`, or ending
    /// in `.nix`) remain compatible with the original explicit-path flow; other
    /// values are discovered names. `default` asks discovery for the effective
    /// default profile.
    pub fn parse_cli(raw: &str) -> Self {
        if raw == "default" {
            return Self::Default;
        }
        if let Some(path) = raw.strip_prefix("path:") {
            return Self::path(path);
        }
        if let Some((prefix, name)) = raw.split_once(':') {
            if let Some(source) = ProfileRegistrySource::parse(prefix) {
                return Self::source_named(source, name);
            }
        }
        if raw.contains('/') || raw.starts_with('.') || raw.ends_with(".nix") {
            Self::path(raw)
        } else {
            Self::named(raw)
        }
    }

    pub fn display_label(&self) -> String {
        match self {
            Self::Path { path } => path.display().to_string(),
            Self::Named { source, name } => match source {
                Some(source) => format!("{source}:{name}"),
                None => name.clone(),
            },
            Self::Default => "default".to_string(),
        }
    }
}

/// Profile source recorded with a resolved artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProfileSource {
    Path {
        path: PathBuf,
    },
    Registry {
        source: ProfileRegistrySource,
        name: String,
        path: PathBuf,
    },
}

/// One profile discovered from a registry source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileRegistryEntry {
    pub source: ProfileRegistrySource,
    pub name: String,
    pub path: PathBuf,
    pub description: Option<String>,
    pub is_default: bool,
}

impl ProfileRegistryEntry {
    pub fn qualified_name(&self) -> String {
        format!("{}:{}", self.source, self.name)
    }
}

/// Discovered profile registry. User/project `profiles.toml` files contribute
/// only profile discovery metadata (entries and defaults); those files are
/// application/project UX configuration, not Pod runtime manifests and are not
/// merged into the selected profile's runtime manifest.
#[derive(Debug, Clone, Default)]
pub struct ProfileRegistry {
    entries: Vec<ProfileRegistryEntry>,
    default: Option<ProfileDefault>,
}

impl ProfileRegistry {
    pub fn entries(&self) -> &[ProfileRegistryEntry] {
        &self.entries
    }

    pub fn default_entry(&self) -> Result<&ProfileRegistryEntry, ProfileError> {
        let default = self
            .default
            .as_ref()
            .ok_or(ProfileError::NoDefaultProfile)?;
        self.select_named(default.source, &default.name)
    }

    pub fn select(
        &self,
        selector: &ProfileSelector,
    ) -> Result<&ProfileRegistryEntry, ProfileError> {
        match selector {
            ProfileSelector::Path { .. } => Err(ProfileError::InvalidArtifact(
                "path selectors are not registry entries".to_string(),
            )),
            ProfileSelector::Default => self.default_entry(),
            ProfileSelector::Named { source, name } => self.select_named(*source, name),
        }
    }

    fn select_named(
        &self,
        source: Option<ProfileRegistrySource>,
        name: &str,
    ) -> Result<&ProfileRegistryEntry, ProfileError> {
        let matches: Vec<_> = self
            .entries
            .iter()
            .filter(|entry| entry.name == name && source.is_none_or(|s| s == entry.source))
            .collect();
        match matches.as_slice() {
            [entry] => Ok(*entry),
            [] => Err(ProfileError::ProfileNotFound {
                selector: match source {
                    Some(source) => format!("{source}:{name}"),
                    None => name.to_string(),
                },
            }),
            _ => Err(ProfileError::AmbiguousProfileName {
                name: name.to_string(),
                matches: matches.iter().map(|entry| entry.qualified_name()).collect(),
            }),
        }
    }

    fn push_entry(&mut self, entry: ProfileRegistryEntry) {
        self.entries.push(entry);
    }

    fn set_default(&mut self, default: ProfileDefault) {
        self.default = Some(default);
    }

    fn set_builtin_default_if_available(&mut self) {
        if self.default.is_some() {
            return;
        }
        if self
            .select_named(
                Some(ProfileRegistrySource::Builtin),
                BUILTIN_DEFAULT_PROFILE_NAME,
            )
            .is_ok()
        {
            self.default = Some(ProfileDefault {
                source: Some(ProfileRegistrySource::Builtin),
                name: BUILTIN_DEFAULT_PROFILE_NAME.to_string(),
            });
        }
    }

    fn mark_default_flags(&mut self) {
        let Some(default) = self.default.clone() else {
            return;
        };
        let Ok(default_entry) = self.select_named(default.source, &default.name) else {
            return;
        };
        let source = default_entry.source;
        let name = default_entry.name.clone();
        for entry in &mut self.entries {
            entry.is_default = entry.source == source && entry.name == name;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProfileDefault {
    source: Option<ProfileRegistrySource>,
    name: String,
}

/// Filesystem-backed profile discovery.
#[derive(Debug, Clone)]
pub struct ProfileDiscovery {
    builtin_dir: Option<PathBuf>,
    user_config: Option<PathBuf>,
    project_config: Option<PathBuf>,
}

impl ProfileDiscovery {
    pub fn for_cwd(cwd: &Path) -> Self {
        Self {
            builtin_dir: paths::builtin_profiles_dir(),
            user_config: paths::user_profiles_path(),
            project_config: find_project_profiles_from(cwd),
        }
    }

    pub fn with_sources(
        builtin_dir: Option<PathBuf>,
        user_config: Option<PathBuf>,
        project_config: Option<PathBuf>,
    ) -> Self {
        Self {
            builtin_dir,
            user_config,
            project_config,
        }
    }

    pub fn discover(&self) -> Result<ProfileRegistry, ProfileError> {
        let mut registry = ProfileRegistry::default();
        if let Some(dir) = &self.builtin_dir {
            discover_profile_dir(&mut registry, ProfileRegistrySource::Builtin, dir)?;
        }
        if let Some(path) = &self.user_config {
            load_profile_registry_file(&mut registry, ProfileRegistrySource::User, path)?;
        }
        if let Some(path) = &self.project_config {
            load_profile_registry_file(&mut registry, ProfileRegistrySource::Project, path)?;
        }
        registry.set_builtin_default_if_available();
        registry.mark_default_flags();
        Ok(registry)
    }
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
    workspace_base: Option<PathBuf>,
}

impl Default for NixProfileResolver {
    fn default() -> Self {
        Self {
            nix_bin: PathBuf::from("nix"),
            workspace_base: None,
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
            workspace_base: None,
        }
    }

    pub fn with_workspace_base(mut self, workspace_base: impl Into<PathBuf>) -> Self {
        self.workspace_base = Some(workspace_base.into());
        self
    }

    pub fn resolve(&self, selector: &ProfileSelector) -> Result<ResolvedProfile, ProfileError> {
        match selector {
            ProfileSelector::Path { path } => self.resolve_path(
                path,
                ProfileSource::Path {
                    path: absolutize(path)?,
                },
                None,
            ),
            ProfileSelector::Named { .. } | ProfileSelector::Default => {
                let cwd = std::env::current_dir().map_err(|source| ProfileError::CommandIo {
                    path: PathBuf::from("."),
                    source,
                })?;
                let registry = ProfileDiscovery::for_cwd(&cwd).discover()?;
                let entry = registry.select(selector)?.clone();
                let artifact_base = if entry.source == ProfileRegistrySource::Builtin {
                    Some(absolutize(self.workspace_base.as_deref().unwrap_or(&cwd))?)
                } else {
                    None
                };
                self.resolve_path(
                    &entry.path,
                    ProfileSource::Registry {
                        source: entry.source,
                        name: entry.name,
                        path: absolutize(&entry.path)?,
                    },
                    artifact_base,
                )
            }
        }
    }

    fn resolve_path(
        &self,
        path: &Path,
        source: ProfileSource,
        manifest_base_override: Option<PathBuf>,
    ) -> Result<ResolvedProfile, ProfileError> {
        let absolute_path = absolutize(path)?;
        let file_base_dir = absolute_path
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| ProfileError::InvalidPath {
                path: absolute_path.clone(),
                message: "profile path has no parent directory".to_string(),
            })?;
        let base_dir = manifest_base_override.unwrap_or(file_base_dir);

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
                path: absolute_path,
                source,
            })?;

        resolve_profile_artifact(source, &base_dir, raw_artifact)
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

#[derive(Debug, Default, Deserialize)]
struct ProfileRegistryDocument {
    #[serde(default)]
    default: Option<String>,
    #[serde(default, alias = "entries")]
    profile: BTreeMap<String, ProfileEntryConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ProfileEntryConfig {
    Path(String),
    Table {
        path: PathBuf,
        #[serde(default)]
        description: Option<String>,
    },
}

impl ProfileEntryConfig {
    fn into_parts(self) -> (PathBuf, Option<String>) {
        match self {
            Self::Path(path) => (PathBuf::from(path), None),
            Self::Table { path, description } => (path, description),
        }
    }
}

fn load_profile_registry_file(
    registry: &mut ProfileRegistry,
    source: ProfileRegistrySource,
    path: &Path,
) -> Result<(), ProfileError> {
    if !path.exists() {
        return Ok(());
    }
    let content = std::fs::read_to_string(path).map_err(|source| ProfileError::ConfigRead {
        path: path.to_path_buf(),
        source,
    })?;
    let config: ProfileRegistryDocument =
        toml::from_str(&content).map_err(|source| ProfileError::ConfigParse {
            path: path.to_path_buf(),
            source,
        })?;
    let base = path.parent().unwrap_or_else(|| Path::new("."));

    for (name, entry_config) in config.profile {
        let (entry_path, description) = entry_config.into_parts();
        registry.push_entry(ProfileRegistryEntry {
            source,
            name,
            path: join_if_relative(base, &entry_path),
            description,
            is_default: false,
        });
    }

    if let Some(default) = config.default {
        let (default_source, default_name) = parse_profile_ref(&default);
        registry.set_default(ProfileDefault {
            source: default_source.or(Some(source)),
            name: default_name,
        });
    }

    Ok(())
}

/// Walk up from `start` looking for `.insomnia/profiles.toml`. Returns the
/// closest match, or `None` if none is found before reaching the filesystem
/// root.
fn find_project_profiles_from(start: &Path) -> Option<PathBuf> {
    let start = start
        .canonicalize()
        .ok()
        .unwrap_or_else(|| start.to_path_buf());
    let mut cur: Option<&Path> = Some(start.as_path());
    while let Some(dir) = cur {
        let candidate = dir.join(".insomnia").join("profiles.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
        cur = dir.parent();
    }
    None
}

fn discover_profile_dir(
    registry: &mut ProfileRegistry,
    source: ProfileRegistrySource,
    dir: &Path,
) -> Result<(), ProfileError> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir).map_err(|source| ProfileError::ConfigRead {
        path: dir.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| ProfileError::ConfigRead {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("nix") {
            if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                registry.push_entry(ProfileRegistryEntry {
                    source,
                    name: name.to_string(),
                    path,
                    description: None,
                    is_default: false,
                });
            }
        } else if path.is_dir() {
            let profile = path.join("profile.nix");
            if profile.is_file() {
                if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                    registry.push_entry(ProfileRegistryEntry {
                        source,
                        name: name.to_string(),
                        path: profile,
                        description: None,
                        is_default: false,
                    });
                }
            }
        }
    }
    Ok(())
}

fn parse_profile_ref(raw: &str) -> (Option<ProfileRegistrySource>, String) {
    if let Some((prefix, name)) = raw.split_once(':') {
        if let Some(source) = ProfileRegistrySource::parse(prefix) {
            return (Some(source), name.to_string());
        }
    }
    (None, raw.to_string())
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

fn join_if_relative(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

/// Errors raised while evaluating, discovering, and validating a profile.
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

    #[error("failed to read profile registry config {}: {source}", .path.display())]
    ConfigRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse profile registry config {}: {source}", .path.display())]
    ConfigParse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("no default profile is configured")]
    NoDefaultProfile,

    #[error("profile not found: {selector}")]
    ProfileNotFound { selector: String },

    #[error("ambiguous profile name `{name}`; use a source-qualified selector such as {matches:?}")]
    AmbiguousProfileName { name: String, matches: Vec<String> },

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
    use std::sync::{Mutex, MutexGuard, OnceLock};
    use tempfile::TempDir;

    fn env_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    struct EnvGuard {
        vars: Vec<(&'static str, Option<String>)>,
        _lock: MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn new(overrides: &[(&'static str, Option<&str>)]) -> Self {
            let lock = env_lock();
            let names = [
                "INSOMNIA_CONFIG_DIR",
                "INSOMNIA_USER_MANIFEST",
                "INSOMNIA_HOME",
                "XDG_CONFIG_HOME",
                "HOME",
            ];
            let saved: Vec<_> = names.iter().map(|n| (*n, std::env::var(n).ok())).collect();
            // SAFETY: env_lock() protects environment mutation within this test binary.
            unsafe {
                for (n, _) in &saved {
                    std::env::remove_var(n);
                }
                for (n, v) in overrides {
                    if let Some(v) = v {
                        std::env::set_var(n, v);
                    }
                }
            }
            Self {
                vars: saved,
                _lock: lock,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: env_lock() is still held while restoring the environment.
            unsafe {
                for (n, v) in &self.vars {
                    match v {
                        Some(v) => std::env::set_var(n, v),
                        None => std::env::remove_var(n),
                    }
                }
            }
        }
    }

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
    fn profile_artifact_preserves_session_record_event_trace() {
        let mut raw = artifact();
        raw["manifest"]["session"] = serde_json::json!({ "record_event_trace": true });

        let resolved = resolve_profile_artifact(
            ProfileSource::Path {
                path: PathBuf::from("/profiles/coder.nix"),
            },
            Path::new("/workspace/project"),
            raw,
        )
        .unwrap();

        assert!(resolved.manifest.session.record_event_trace);
        assert_eq!(
            resolved.manifest_snapshot["session"]["record_event_trace"],
            serde_json::json!(true)
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

    #[test]
    fn parse_cli_preserves_paths_and_source_qualified_names() {
        assert!(matches!(
            ProfileSelector::parse_cli("./coder.nix"),
            ProfileSelector::Path { .. }
        ));
        assert_eq!(
            ProfileSelector::parse_cli("project:coder"),
            ProfileSelector::source_named(ProfileRegistrySource::Project, "coder")
        );
        assert_eq!(
            ProfileSelector::parse_cli("coder"),
            ProfileSelector::named("coder")
        );
        assert_eq!(
            ProfileSelector::parse_cli("default"),
            ProfileSelector::Default
        );
    }

    #[test]
    fn builtin_default_profile_is_registered_as_default() {
        let registry = ProfileDiscovery::with_sources(paths::builtin_profiles_dir(), None, None)
            .discover()
            .unwrap();
        let default = registry.default_entry().unwrap();
        assert_eq!(default.source, ProfileRegistrySource::Builtin);
        assert_eq!(default.name, BUILTIN_DEFAULT_PROFILE_NAME);
        assert!(default.is_default);
        assert!(default.path.ends_with("resources/nix/profiles/default.nix"));
    }

    #[cfg(unix)]
    #[test]
    fn builtin_profile_relative_paths_resolve_against_workspace_base() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let profile_path = tmp.path().join("default.nix");
        std::fs::write(&profile_path, "{}").unwrap();
        let nix_bin = tmp.path().join("fake-nix");
        std::fs::write(
            &nix_bin,
            r#"#!/bin/sh
cat <<'JSON'
{"profile":{"format":"insomnia.nix-profile.v1","name":"default"},"manifest":{"pod":{"name":"default"},"model":{"scheme":"anthropic","model_id":"claude-sonnet-4-20250514"},"scope":{"allow":[{"target":".","permission":"write"}]}}}
JSON
"#,
        )
        .unwrap();
        let mut perms = std::fs::metadata(&nix_bin).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&nix_bin, perms).unwrap();
        let resolved = NixProfileResolver::with_nix_bin(&nix_bin)
            .resolve_path(
                &profile_path,
                ProfileSource::Registry {
                    source: ProfileRegistrySource::Builtin,
                    name: BUILTIN_DEFAULT_PROFILE_NAME.to_string(),
                    path: profile_path.clone(),
                },
                Some(workspace.clone()),
            )
            .unwrap();

        assert_eq!(resolved.manifest.scope.allow[0].target, workspace);
    }

    #[test]
    fn for_cwd_reads_profiles_toml_and_ignores_manifest_profiles() {
        let tmp = TempDir::new().unwrap();
        let config_dir = tmp.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        let _env = EnvGuard::new(&[("INSOMNIA_CONFIG_DIR", Some(config_dir.to_str().unwrap()))]);

        let project = tmp.path().join("project").join("nested");
        let insomnia = tmp.path().join("project").join(".insomnia");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&insomnia).unwrap();
        std::fs::write(
            insomnia.join("manifest.toml"),
            r#"
[profiles]
default = "wrong"
[profiles.profile]
wrong = "wrong.nix"
"#,
        )
        .unwrap();
        std::fs::write(
            insomnia.join("profiles.toml"),
            r#"
default = "coder"
[profile]
coder = "profiles/coder.nix"
"#,
        )
        .unwrap();

        let registry = ProfileDiscovery::for_cwd(&project).discover().unwrap();
        assert!(registry.select_named(None, "wrong").is_err());
        let selected = registry.default_entry().unwrap();
        assert_eq!(selected.source, ProfileRegistrySource::Project);
        assert_eq!(selected.name, "coder");
    }

    #[test]
    fn user_manifest_env_does_not_affect_profile_registry_discovery() {
        let tmp = TempDir::new().unwrap();
        let config_dir = tmp.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        let env_manifest = tmp.path().join("env-manifest.toml");
        std::fs::write(
            &env_manifest,
            r#"
[profiles]
default = "wrong"
[profiles.profile]
wrong = "wrong.nix"
"#,
        )
        .unwrap();
        std::fs::write(
            config_dir.join("profiles.toml"),
            r#"
default = "coder"
[profile]
coder = "profiles/coder.nix"
"#,
        )
        .unwrap();
        let _env = EnvGuard::new(&[
            ("INSOMNIA_CONFIG_DIR", Some(config_dir.to_str().unwrap())),
            (
                "INSOMNIA_USER_MANIFEST",
                Some(env_manifest.to_str().unwrap()),
            ),
        ]);

        let registry = ProfileDiscovery::for_cwd(tmp.path()).discover().unwrap();
        assert!(registry.select_named(None, "wrong").is_err());
        let selected = registry.default_entry().unwrap();
        assert_eq!(selected.source, ProfileRegistrySource::User);
        assert_eq!(selected.name, "coder");
    }

    #[test]
    fn discovery_reads_user_and_project_registry_and_project_default_wins() {
        let tmp = TempDir::new().unwrap();
        let user_config = tmp.path().join("profiles.toml");
        let project_dir = tmp.path().join("project/.insomnia");
        std::fs::create_dir_all(&project_dir).unwrap();
        let project_config = project_dir.join("profiles.toml");
        std::fs::write(
            &user_config,
            r#"
default = "coder"
[profile]
coder = "profiles/user-coder.nix"
"#,
        )
        .unwrap();
        std::fs::write(
            &project_config,
            r#"
default = "project:coder"
[profile.coder]
path = "profiles/project-coder.nix"
description = "Project coder"
"#,
        )
        .unwrap();

        let registry =
            ProfileDiscovery::with_sources(None, Some(user_config), Some(project_config))
                .discover()
                .unwrap();

        let default = registry.default_entry().unwrap();
        assert_eq!(default.source, ProfileRegistrySource::Project);
        assert_eq!(default.name, "coder");
        assert!(default.path.ends_with("profiles/project-coder.nix"));
    }

    #[test]
    fn default_marks_direct_profile_entry() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path().join("project/.insomnia");
        std::fs::create_dir_all(&project_dir).unwrap();
        let project_config = project_dir.join("profiles.toml");
        std::fs::write(
            &project_config,
            r#"
default = "coder"
[profile]
coder = "profiles/coder.nix"
"#,
        )
        .unwrap();

        let registry = ProfileDiscovery::with_sources(None, None, Some(project_config))
            .discover()
            .unwrap();
        let default = registry.default_entry().unwrap();
        assert_eq!(default.source, ProfileRegistrySource::Project);
        assert_eq!(default.name, "coder");
        assert!(default.is_default);
        assert_eq!(
            registry
                .entries()
                .iter()
                .filter(|entry| entry.is_default)
                .count(),
            1
        );
    }

    #[test]
    fn unqualified_ambiguous_names_fail_closed() {
        let mut registry = ProfileRegistry::default();
        registry.push_entry(ProfileRegistryEntry {
            source: ProfileRegistrySource::User,
            name: "coder".to_string(),
            path: PathBuf::from("/user/coder.nix"),
            description: None,
            is_default: false,
        });
        registry.push_entry(ProfileRegistryEntry {
            source: ProfileRegistrySource::Project,
            name: "coder".to_string(),
            path: PathBuf::from("/project/coder.nix"),
            description: None,
            is_default: false,
        });

        let err = registry
            .select(&ProfileSelector::named("coder"))
            .unwrap_err();
        assert!(matches!(err, ProfileError::AmbiguousProfileName { .. }));
        let selected = registry
            .select(&ProfileSelector::source_named(
                ProfileRegistrySource::Project,
                "coder",
            ))
            .unwrap();
        assert_eq!(selected.path, PathBuf::from("/project/coder.nix"));
    }
}
