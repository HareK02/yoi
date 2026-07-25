//! Profile discovery and resolution.
//!
//! Profiles are reusable, human-authored recipes. They are intentionally not
//! complete runtime manifests: runtime-bound and authority-bearing fields such
//! as `worker.name` and concrete `scope.allow` rules are supplied by the resolver
//! from launch context.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::config::{
    CompactionConfigPartial, FeatureConfigPartial, PermissionConfigPartial, SessionConfigPartial,
};
use crate::model::{AuthRef, ModelManifest};
use crate::plugin::PluginConfig;
use crate::{
    EngineManifestConfig, McpConfig, McpStdioCwdPolicy, MemoryConfig, Permission, ResolveError,
    ScopeConfig, ScopeRule, SkillsConfig, WebConfig, WorkerManifest, WorkerManifestConfig,
    WorkerMetaConfig, paths,
};

const PROFILE_FORMAT_V1: &str = "yoi.profile.v1";
const BUILTIN_DEFAULT_PROFILE_NAME: &str = "default";
const BUILTIN_MODEL_CATALOG: &str = include_str!("../../../resources/models/builtin.toml");
const WORKSPACE_OVERRIDE_LOCAL_FILENAME: &str = "override.local.toml";

struct BuiltinProfile {
    name: &'static str,
    label: &'static str,
    description: &'static str,
}

const BUILTIN_PROFILES: &[BuiltinProfile] = &[
    BuiltinProfile {
        name: BUILTIN_DEFAULT_PROFILE_NAME,
        label: "builtin:default",
        description: "Bundled default Yoi coding profile",
    },
    BuiltinProfile {
        name: "companion",
        label: "builtin:companion",
        description: "Bundled Companion role profile",
    },
    BuiltinProfile {
        name: "intake",
        label: "builtin:intake",
        description: "Bundled Intake role profile",
    },
    BuiltinProfile {
        name: "orchestrator",
        label: "builtin:orchestrator",
        description: "Bundled Orchestrator role profile",
    },
    BuiltinProfile {
        name: "coder",
        label: "builtin:coder",
        description: "Bundled Coder role profile",
    },
    BuiltinProfile {
        name: "reviewer",
        label: "builtin:reviewer",
        description: "Bundled Reviewer role profile",
    },
    BuiltinProfile {
        name: "memory-consolidation",
        label: "builtin:memory-consolidation",
        description: "Bundled Memory staging consolidation profile",
    },
];

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProfileSelector {
    Path {
        path: PathBuf,
    },
    Named {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<ProfileRegistrySource>,
        name: String,
    },
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
    pub fn parse_cli(raw: &str) -> Self {
        if raw == "default" {
            return Self::Default;
        }
        if let Some(path) = raw.strip_prefix("path:") {
            return Self::path(path);
        }
        if let Some((prefix, name)) = raw.split_once(':')
            && let Some(source) = ProfileRegistrySource::parse(prefix)
        {
            return Self::source_named(source, name);
        }
        if raw.contains('/')
            || raw.starts_with('.')
            || raw.ends_with(".json")
            || raw.ends_with(".toml")
        {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProfileSource {
    Path {
        path: PathBuf,
    },
    Registry {
        source: ProfileRegistrySource,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<PathBuf>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provenance: Option<String>,
    },
    Archive {
        archive_id: String,
        source: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileRegistryEntry {
    pub source: ProfileRegistrySource,
    pub name: String,
    pub path: Option<PathBuf>,
    pub provenance: String,
    pub description: Option<String>,
    pub is_default: bool,
    artifact: ProfileRegistryArtifact,
}

impl ProfileRegistryEntry {
    pub fn qualified_name(&self) -> String {
        format!("{}:{}", self.source, self.name)
    }

    fn path(
        source: ProfileRegistrySource,
        name: String,
        path: PathBuf,
        description: Option<String>,
    ) -> Self {
        let provenance = path.display().to_string();
        Self {
            source,
            name,
            path: Some(path.clone()),
            provenance,
            description,
            is_default: false,
            artifact: ProfileRegistryArtifact::Path(path),
        }
    }

    fn embedded(
        source: ProfileRegistrySource,
        name: &'static str,
        label: &'static str,
        description: Option<String>,
    ) -> Self {
        Self {
            source,
            name: name.to_string(),
            path: None,
            provenance: label.to_string(),
            description,
            is_default: false,
            artifact: ProfileRegistryArtifact::Builtin { label },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ProfileRegistryArtifact {
    Path(PathBuf),
    Builtin { label: &'static str },
}

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
            ProfileSelector::Path { .. } => Err(ProfileError::InvalidProfile(
                "path selectors are not registry entries".into(),
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
                selector: source.map_or_else(|| name.to_string(), |s| format!("{s}:{name}")),
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

#[derive(Debug, Clone)]
pub struct ProfileDiscovery {
    user_config: Option<PathBuf>,
    project_config: Option<PathBuf>,
}

impl ProfileDiscovery {
    pub fn for_cwd(cwd: &Path) -> Self {
        Self {
            user_config: paths::user_profiles_path(),
            project_config: find_project_profiles_from(cwd),
        }
    }
    pub fn with_sources(user_config: Option<PathBuf>, project_config: Option<PathBuf>) -> Self {
        Self {
            user_config,
            project_config,
        }
    }
    pub fn discover(&self) -> Result<ProfileRegistry, ProfileError> {
        let mut registry = ProfileRegistry::default();
        add_builtin_profiles(&mut registry);
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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileManifestSnapshot {
    pub source: ProfileSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<ProfileMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_override: Option<WorkspaceOverrideSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceOverrideSnapshot {
    pub path: PathBuf,
}

#[derive(Debug)]
struct WorkspaceOverrideLayer {
    path: PathBuf,
    config: WorkerManifestConfig,
}

#[derive(Debug, Clone)]
pub struct ResolvedProfile {
    pub source: ProfileSource,
    pub profile: Option<ProfileMetadata>,
    pub manifest: WorkerManifest,
    pub manifest_snapshot: serde_json::Value,
    pub raw_artifact: serde_json::Value,
}

#[derive(Debug, Clone, Default)]
pub struct ProfileResolveOptions {
    pub worker_name: Option<String>,
}
impl ProfileResolveOptions {
    pub fn with_worker_name(name: impl Into<String>) -> Self {
        Self {
            worker_name: Some(name.into()),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ProfileResolver {
    workspace_base: Option<PathBuf>,
}

impl ProfileResolver {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_workspace_base(mut self, workspace_base: impl Into<PathBuf>) -> Self {
        self.workspace_base = Some(workspace_base.into());
        self
    }
    pub fn resolve(
        &self,
        selector: &ProfileSelector,
        options: ProfileResolveOptions,
    ) -> Result<ResolvedProfile, ProfileError> {
        match selector {
            ProfileSelector::Path { path } => self.resolve_path(
                path,
                ProfileSource::Path {
                    path: absolutize(path)?,
                },
                options,
            ),
            ProfileSelector::Named { .. } | ProfileSelector::Default => {
                let cwd = std::env::current_dir().map_err(|source| ProfileError::CommandIo {
                    path: PathBuf::from("."),
                    source,
                })?;
                let registry = ProfileDiscovery::for_cwd(&cwd).discover()?;
                self.resolve_from_registry(selector, &registry, options)
            }
        }
    }
    /// Resolve a registry/default selector against an already-discovered
    /// registry. Callers such as SpawnWorker use this to bind discovery to the
    /// Worker's cwd instead of the process current directory.
    pub fn resolve_from_registry(
        &self,
        selector: &ProfileSelector,
        registry: &ProfileRegistry,
        options: ProfileResolveOptions,
    ) -> Result<ResolvedProfile, ProfileError> {
        match selector {
            ProfileSelector::Path { .. } => Err(ProfileError::InvalidProfile(
                "path selectors are not registry entries".into(),
            )),
            ProfileSelector::Named { .. } | ProfileSelector::Default => {
                let entry = registry.select(selector)?.clone();
                let source = ProfileSource::Registry {
                    source: entry.source,
                    name: entry.name.clone(),
                    path: entry.path.as_deref().map(absolutize).transpose()?,
                    provenance: (entry.path.is_none()).then(|| entry.provenance.clone()),
                };
                self.resolve_registry_entry(&entry, source, options)
            }
        }
    }

    fn resolve_registry_entry(
        &self,
        entry: &ProfileRegistryEntry,
        source: ProfileSource,
        options: ProfileResolveOptions,
    ) -> Result<ResolvedProfile, ProfileError> {
        match &entry.artifact {
            ProfileRegistryArtifact::Path(path) => self.resolve_path(path, source, options),
            ProfileRegistryArtifact::Builtin { label } => {
                self.resolve_builtin_profile(label, source, options)
            }
        }
    }

    fn resolve_path(
        &self,
        path: &Path,
        source: ProfileSource,
        options: ProfileResolveOptions,
    ) -> Result<ResolvedProfile, ProfileError> {
        let absolute_path = absolutize(path)?;
        let profile_dir = absolute_path
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| ProfileError::InvalidPath {
                path: absolute_path.clone(),
                message: "profile path has no parent directory".into(),
            })?;
        let profile_dir = canonicalize_existing_dir(&profile_dir)?;
        let workspace_base = absolutize(
            self.workspace_base
                .as_deref()
                .unwrap_or_else(|| Path::new(".")),
        )?;
        let workspace_override = load_workspace_override_from(&workspace_base)?;
        let raw_artifact = read_profile_artifact_file(&absolute_path)?;
        resolve_profile_value(
            source,
            &profile_dir,
            &workspace_base,
            options,
            raw_artifact.clone(),
            raw_artifact,
            workspace_override,
        )
    }

    fn resolve_builtin_profile(
        &self,
        label: &'static str,
        source: ProfileSource,
        options: ProfileResolveOptions,
    ) -> Result<ResolvedProfile, ProfileError> {
        let workspace_base = absolutize(
            self.workspace_base
                .as_deref()
                .unwrap_or_else(|| Path::new(".")),
        )?;
        let workspace_override = load_workspace_override_from(&workspace_base)?;
        let raw_artifact = builtin_profile_artifact(label).ok_or_else(|| {
            ProfileError::InvalidProfile(format!("unknown builtin profile artifact `{label}`"))
        })?;
        resolve_profile_value(
            source,
            &workspace_base,
            &workspace_base,
            options,
            raw_artifact.clone(),
            raw_artifact,
            workspace_override,
        )
    }
}

fn resolve_profile_value(
    source: ProfileSource,
    profile_dir: &Path,
    workspace_base: &Path,
    options: ProfileResolveOptions,
    value: serde_json::Value,
    raw_artifact: serde_json::Value,
    workspace_override: Option<WorkspaceOverrideLayer>,
) -> Result<ResolvedProfile, ProfileError> {
    if !workspace_base.is_absolute() {
        return Err(ProfileError::InvalidPath {
            path: workspace_base.to_path_buf(),
            message: "profile workspace base must be absolute".into(),
        });
    }
    reject_manifest_shaped_profile(&value)?;
    let profile: ProfileConfig = serde_json::from_value(value.clone())
        .map_err(|source| ProfileError::ProfileDeserialize { source })?;
    validate_profile_paths(&profile)?;
    let worker_name = options
        .worker_name
        .ok_or(ProfileError::MissingRuntimeWorkerName)?;
    let profile_meta = Some(ProfileMetadata {
        name: profile.slug.clone().or_else(|| source_name(&source)),
        description: profile.description.clone(),
        format: Some(PROFILE_FORMAT_V1.to_string()),
    });
    let compaction = profile_compaction_to_partial(profile.compaction, &profile.model)?;
    let config = WorkerManifestConfig {
        worker: WorkerMetaConfig {
            name: Some(worker_name),
            prompt_pack: None,
        },
        model: profile.model.unwrap_or_default(),
        engine: profile.engine.unwrap_or_default(),
        scope: profile_scope_to_config(profile.scope, workspace_base)?,
        delegation_scope: profile_delegation_scope_to_config(
            profile.delegation_scope,
            workspace_base,
        )?,
        session: profile.session,
        permissions: profile.permissions,
        feature: profile.feature,
        plugins: profile.plugins,
        mcp: profile.mcp,
        compaction,
        web: profile.web,
        memory: profile.memory,
        skills: profile.skills,
    };
    let mut config =
        WorkerManifestConfig::builtin_defaults().merge(config.resolve_paths(profile_dir));
    let workspace_override_snapshot = if let Some(override_layer) = workspace_override {
        let override_base =
            override_layer
                .path
                .parent()
                .ok_or_else(|| ProfileError::InvalidPath {
                    path: override_layer.path.clone(),
                    message: "workspace override path has no parent directory".into(),
                })?;
        config = config.merge(override_layer.config.resolve_paths(override_base));
        Some(WorkspaceOverrideSnapshot {
            path: override_layer.path,
        })
    } else {
        None
    };
    let mut manifest = WorkerManifest::try_from(config).map_err(ProfileError::ManifestResolve)?;
    manifest.profile = Some(ProfileManifestSnapshot {
        source: source.clone(),
        profile: profile_meta.clone(),
        workspace_override: workspace_override_snapshot,
    });
    let manifest_snapshot =
        serde_json::to_value(&manifest).map_err(ProfileError::SnapshotSerialize)?;
    Ok(ResolvedProfile {
        source,
        profile: profile_meta,
        manifest,
        manifest_snapshot,
        raw_artifact,
    })
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileConfig {
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    model: Option<ModelManifest>,
    #[serde(default)]
    engine: Option<EngineManifestConfig>,
    #[serde(default)]
    scope: Option<ProfileScopeConfig>,
    #[serde(default)]
    delegation_scope: Option<ProfileScopeConfig>,
    #[serde(default)]
    session: Option<SessionConfigPartial>,
    #[serde(default)]
    permissions: Option<PermissionConfigPartial>,
    #[serde(default)]
    feature: FeatureConfigPartial,
    #[serde(default)]
    plugins: PluginConfig,
    #[serde(default)]
    mcp: McpConfig,
    #[serde(default)]
    compaction: Option<serde_json::Value>,
    #[serde(default)]
    web: Option<WebConfig>,
    #[serde(default)]
    memory: Option<MemoryConfig>,
    #[serde(default)]
    skills: Option<SkillsConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum ProfileScopeConfig {
    Table(ProfileScopeTable),
    String(ProfileScopeIntent),
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileScopeTable {
    intent: ProfileScopeIntent,
    #[serde(default)]
    deny_write: Vec<PathBuf>,
}
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ProfileScopeIntent {
    WorkspaceRead,
    WorkspaceWrite,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RatioCompaction {
    #[allow(dead_code)]
    kind: String,
    #[serde(default)]
    threshold: Option<f64>,
    #[serde(default, alias = "request")]
    request_threshold: Option<f64>,
    #[serde(default, alias = "worker")]
    worker_context_max_tokens: Option<f64>,
}

#[derive(Debug, Deserialize)]
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
        registry.push_entry(ProfileRegistryEntry::path(
            source,
            name,
            join_if_relative(base, &entry_path),
            description,
        ));
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

fn load_workspace_override_from(
    workspace_base: &Path,
) -> Result<Option<WorkspaceOverrideLayer>, ProfileError> {
    find_workspace_override_from(workspace_base)
        .map(|path| load_workspace_override_file(&path))
        .transpose()
}

fn load_workspace_override_file(path: &Path) -> Result<WorkspaceOverrideLayer, ProfileError> {
    let content =
        std::fs::read_to_string(path).map_err(|source| ProfileError::WorkspaceOverrideRead {
            path: path.to_path_buf(),
            source,
        })?;
    let config = WorkerManifestConfig::from_toml(&content).map_err(|source| {
        ProfileError::WorkspaceOverrideParse {
            path: path.to_path_buf(),
            source,
        }
    })?;
    if config.worker.name.is_some() {
        return Err(ProfileError::InvalidWorkspaceOverride {
            path: path.to_path_buf(),
            message: "workspace-local manifest overrides cannot set worker.name; Worker identity is a runtime input".into(),
        });
    }
    Ok(WorkspaceOverrideLayer {
        path: path.to_path_buf(),
        config,
    })
}

fn find_workspace_override_from(start: &Path) -> Option<PathBuf> {
    let start = start
        .canonicalize()
        .ok()
        .unwrap_or_else(|| start.to_path_buf());
    let mut cur: Option<&Path> = Some(start.as_path());
    while let Some(dir) = cur {
        let candidate = dir.join(".yoi").join(WORKSPACE_OVERRIDE_LOCAL_FILENAME);
        if candidate.is_file() {
            return Some(candidate);
        }
        cur = dir.parent();
    }
    None
}

fn find_project_profiles_from(start: &Path) -> Option<PathBuf> {
    let start = start
        .canonicalize()
        .ok()
        .unwrap_or_else(|| start.to_path_buf());
    let mut cur: Option<&Path> = Some(start.as_path());
    while let Some(dir) = cur {
        let candidate = dir.join(".yoi").join("profiles.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
        cur = dir.parent();
    }
    None
}

fn add_builtin_profiles(registry: &mut ProfileRegistry) {
    for profile in BUILTIN_PROFILES {
        registry.push_entry(ProfileRegistryEntry::embedded(
            ProfileRegistrySource::Builtin,
            profile.name,
            profile.label,
            Some(profile.description.into()),
        ));
    }
}

fn parse_profile_ref(raw: &str) -> (Option<ProfileRegistrySource>, String) {
    if let Some((prefix, name)) = raw.split_once(':')
        && let Some(source) = ProfileRegistrySource::parse(prefix)
    {
        return (Some(source), name.to_string());
    }
    (None, raw.to_string())
}

fn read_profile_artifact_file(path: &Path) -> Result<serde_json::Value, ProfileError> {
    let content = std::fs::read_to_string(path).map_err(|source| ProfileError::ConfigRead {
        path: path.to_path_buf(),
        source,
    })?;
    match path.extension().and_then(|s| s.to_str()) {
        Some("json") => serde_json::from_str(&content)
            .map_err(|source| ProfileError::ProfileDeserialize { source }),
        Some("toml") => {
            let value: toml::Value =
                toml::from_str(&content).map_err(|source| ProfileError::ConfigParse {
                    path: path.to_path_buf(),
                    source,
                })?;
            serde_json::to_value(value).map_err(ProfileError::SnapshotSerialize)
        }
        other => Err(ProfileError::UnsupportedProfileType {
            path: path.to_path_buf(),
            message: format!(
                "unsupported profile extension {}; Profiles must be .json or .toml artifacts",
                other.map_or("<none>".to_string(), |s| format!(".{s}"))
            ),
        }),
    }
}

fn builtin_profile_artifact(label: &str) -> Option<serde_json::Value> {
    let mut value = builtin_default_profile_artifact();
    match label {
        "builtin:default" | "default" => Some(value),
        "builtin:companion" | "companion" => {
            apply_role_profile(
                &mut value,
                "companion",
                "Workspace companion profile.",
                "workspace_write",
                true,
                true,
                true,
                true,
            );
            Some(value)
        }
        "builtin:intake" | "intake" => {
            apply_role_profile(
                &mut value,
                "intake",
                "Ticket intake profile.",
                "workspace_write",
                true,
                true,
                true,
                false,
            );
            Some(value)
        }
        "builtin:orchestrator" | "orchestrator" => {
            apply_role_profile(
                &mut value,
                "orchestrator",
                "Ticket orchestrator profile.",
                "workspace_write",
                true,
                true,
                true,
                true,
            );
            Some(value)
        }
        "builtin:coder" | "coder" => {
            apply_role_profile(
                &mut value,
                "coder",
                "Ticket implementation coder profile.",
                "workspace_write",
                true,
                true,
                true,
                false,
            );
            Some(value)
        }
        "builtin:reviewer" | "reviewer" => {
            apply_role_profile(
                &mut value,
                "reviewer",
                "Ticket review profile.",
                "workspace_read",
                true,
                true,
                true,
                false,
            );
            Some(value)
        }
        _ => None,
    }
}

fn builtin_default_profile_artifact() -> serde_json::Value {
    serde_json::json!({
        "slug": "default",
        "description": "Default Yoi coding profile.",
        "model": { "ref": "codex-oauth/gpt-5.5" },
        "session": { "record_event_trace": true },
        "engine": { "reasoning": "high" },
        "compaction": {
            "kind": "tokens",
            "threshold": 240000,
            "request_threshold": 270000,
            "worker_context_max_tokens": 100000
        },
        "feature": {
            "task": { "enabled": true },
            "memory": { "enabled": true },
            "web": { "enabled": true },
            "workers": { "enabled": true },
            "ticket": { "enabled": true, "authoring": true, "thread": true }
        },
        "memory": {
            "extract_threshold": 50000,
            "consolidation_threshold_files": 5,
            "consolidation_threshold_bytes": 50000
        },
        "web": {
            "enabled": true,
            "search": {
                "provider": "brave",
                "api_key_secret": "web/brave/default"
            }
        }
    })
}

#[allow(clippy::too_many_arguments)]
fn apply_role_profile(
    value: &mut serde_json::Value,
    slug: &str,
    description: &str,
    _scope: &str,
    task: bool,
    memory: bool,
    web: bool,
    workers: bool,
) {
    value["slug"] = serde_json::Value::String(slug.to_string());
    value["description"] = serde_json::Value::String(description.to_string());
    value["feature"]["task"] = serde_json::json!({ "enabled": task });
    value["feature"]["memory"] = serde_json::json!({ "enabled": memory });
    value["feature"]["web"] = serde_json::json!({ "enabled": web });
    value["feature"]["workers"] = serde_json::json!({ "enabled": workers });
    let ticket = match slug {
        "companion" => serde_json::json!({ "enabled": true, "authoring": true, "thread": true }),
        "intake" => {
            serde_json::json!({ "enabled": true, "authoring": true, "thread": true, "intake": true })
        }
        "orchestrator" => {
            serde_json::json!({ "enabled": true, "thread": true, "orchestration_control": true })
        }
        "coder" => serde_json::json!({ "enabled": true, "thread": true }),
        "reviewer" => serde_json::json!({ "enabled": true, "thread": true }),
        _ => serde_json::json!({ "enabled": true, "authoring": true, "thread": true }),
    };
    value["feature"]["ticket"] = ticket;
}

fn reject_manifest_shaped_profile(value: &serde_json::Value) -> Result<(), ProfileError> {
    let Some(map) = value.as_object() else {
        return Err(ProfileError::InvalidProfile(
            "Profile artifact must be an object".into(),
        ));
    };
    for key in ["manifest", "config"] {
        if map.contains_key(key) {
            return Err(ProfileError::InvalidProfile(format!(
                "field `{key}` is a complete Manifest artifact boundary and is not allowed in reusable Profiles; use --manifest for complete Manifests"
            )));
        }
    }
    if map.contains_key("worker") {
        return Err(ProfileError::InvalidProfile("field `worker` is runtime-bound and is not allowed in reusable Profiles; pass the Worker name via CLI/TUI runtime inputs".into()));
    }
    if let Some(scope) = map.get("scope").and_then(|v| v.as_object()) {
        for key in ["allow", "deny"] {
            if scope.contains_key(key) {
                return Err(ProfileError::InvalidProfile(format!(
                    "field `scope.{key}` grants concrete authority and is not allowed in reusable Profiles; use profile scope intents"
                )));
            }
        }
    }
    Ok(())
}

fn validate_profile_paths(profile: &ProfileConfig) -> Result<(), ProfileError> {
    if let Some(model) = &profile.model {
        reject_absolute_auth_file(&model.auth, "model.auth.file")?;
    }
    if let Some(compaction) = &profile.compaction
        && let Some(model) = compaction.get("model")
    {
        let model: ModelManifest = serde_json::from_value(model.clone())
            .map_err(|source| ProfileError::ProfileDeserialize { source })?;
        reject_absolute_auth_file(&model.auth, "compaction.model.auth.file")?;
    }
    if let Some(memory) = &profile.memory
        && let Some(root) = &memory.workspace_root
        && root.is_absolute()
    {
        return Err(ProfileError::InvalidProfile("field `memory.workspace_root` is a resolved path and is not allowed in reusable Profiles".into()));
    }
    if let Some(skills) = &profile.skills {
        for dir in &skills.directories {
            if dir.is_absolute() {
                return Err(ProfileError::InvalidProfile(
                    "field `skills.directories` must be profile-relative in reusable Profiles"
                        .into(),
                ));
            }
        }
    }
    for server in &profile.mcp.stdio_servers {
        if let Some(McpStdioCwdPolicy::Path { path }) = &server.cwd
            && path.is_absolute()
        {
            return Err(ProfileError::InvalidProfile(
                "field `mcp.stdio_server.cwd.path` must be profile-relative in reusable Profiles"
                    .into(),
            ));
        }
    }
    Ok(())
}
fn reject_absolute_auth_file(
    auth: &Option<AuthRef>,
    field: &'static str,
) -> Result<(), ProfileError> {
    if let Some(AuthRef::ApiKey { file: Some(file) }) = auth
        && file.is_absolute()
    {
        return Err(ProfileError::InvalidProfile(format!(
            "field `{field}` is a resolved path and is not allowed in reusable Profiles"
        )));
    }
    Ok(())
}
fn profile_scope_to_config(
    scope: Option<ProfileScopeConfig>,
    workspace_base: &Path,
) -> Result<ScopeConfig, ProfileError> {
    profile_scope_intent_to_config(scope, workspace_base, None, "scope")
}

fn profile_delegation_scope_to_config(
    scope: Option<ProfileScopeConfig>,
    workspace_base: &Path,
) -> Result<ScopeConfig, ProfileError> {
    profile_scope_intent_to_config(scope, workspace_base, None, "delegation_scope")
}

fn profile_scope_intent_to_config(
    scope: Option<ProfileScopeConfig>,
    workspace_base: &Path,
    default_intent: Option<ProfileScopeIntent>,
    field: &'static str,
) -> Result<ScopeConfig, ProfileError> {
    let (intent, deny_write) = match scope {
        Some(ProfileScopeConfig::Table(table)) => (Some(table.intent), table.deny_write),
        Some(ProfileScopeConfig::String(intent)) => (Some(intent), Vec::new()),
        None => (default_intent, Vec::new()),
    };
    let Some(intent) = intent else {
        return Ok(ScopeConfig::default());
    };
    let permission = match intent {
        ProfileScopeIntent::WorkspaceRead => Permission::Read,
        ProfileScopeIntent::WorkspaceWrite => Permission::Write,
    };
    let mut deny = Vec::new();
    for path in deny_write {
        if path.is_absolute() {
            return Err(ProfileError::InvalidProfile(format!(
                "field `{field}.deny_write` must be workspace-relative in reusable Profiles"
            )));
        }
        deny.push(ScopeRule {
            target: workspace_base.join(path),
            permission: Permission::Write,
            recursive: true,
        });
    }
    Ok(ScopeConfig {
        allow: vec![ScopeRule {
            target: workspace_base.to_path_buf(),
            permission,
            recursive: true,
        }],
        deny,
    })
}
fn profile_compaction_to_partial(
    value: Option<serde_json::Value>,
    model: &Option<ModelManifest>,
) -> Result<Option<CompactionConfigPartial>, ProfileError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let Some(kind) = value.get("kind").and_then(|v| v.as_str()) else {
        return serde_json::from_value(value)
            .map(Some)
            .map_err(|source| ProfileError::ProfileDeserialize { source });
    };
    match kind {
        "tokens" => {
            let mut obj = value.as_object().cloned().unwrap_or_default();
            obj.remove("kind");
            serde_json::from_value(serde_json::Value::Object(obj))
                .map(Some)
                .map_err(|source| ProfileError::ProfileDeserialize { source })
        }
        "ratio" => {
            let ratio: RatioCompaction = serde_json::from_value(value)
                .map_err(|source| ProfileError::ProfileDeserialize { source })?;
            let context = model_context_window(model.as_ref()).ok_or_else(|| ProfileError::InvalidProfile("compact.ratio requires model.context_window/max_context_window or a known model ref; use compact.tokens for explicit token values".into()))?;
            Ok(Some(CompactionConfigPartial {
                threshold: ratio.threshold.map(|r| ratio_tokens(context, r)),
                request_threshold: ratio.request_threshold.map(|r| ratio_tokens(context, r)),
                worker_context_max_tokens: ratio
                    .worker_context_max_tokens
                    .map(|r| ratio_tokens(context, r)),
                ..Default::default()
            }))
        }
        other => Err(ProfileError::InvalidProfile(format!(
            "unknown compaction helper kind `{other}`"
        ))),
    }
}
fn ratio_tokens(context: u64, ratio: f64) -> u64 {
    ((context as f64) * ratio).floor() as u64
}
fn model_context_window(model: Option<&ModelManifest>) -> Option<u64> {
    let model = model?;
    if let Some(max) = model.max_context_window {
        return Some(model.context_window.map_or(max, |ctx| ctx.min(max)));
    }
    if let Some(ctx) = model.context_window {
        return Some(ctx);
    }
    builtin_model_context_window(model.ref_.as_deref()?)
}
fn builtin_model_context_window(reference: &str) -> Option<u64> {
    let (provider, model_id) = reference.split_once('/')?;
    let parsed: toml::Value = toml::from_str(BUILTIN_MODEL_CATALOG).ok()?;
    for entry in parsed.get("model")?.as_array()? {
        let table = entry.as_table()?;
        if table.get("provider")?.as_str()? == provider && table.get("id")?.as_str()? == model_id {
            let context = table.get("context_window")?.as_integer()? as u64;
            let max = table
                .get("max_context_window")
                .and_then(|v| v.as_integer())
                .map(|v| v as u64);
            return Some(max.map_or(context, |max| context.min(max)));
        }
    }
    None
}
fn source_name(source: &ProfileSource) -> Option<String> {
    match source {
        ProfileSource::Path { path } => path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(str::to_string),
        ProfileSource::Registry { name, .. } => Some(name.clone()),
        ProfileSource::Archive { source, .. } => Path::new(source)
            .file_stem()
            .and_then(|s| s.to_str())
            .map(str::to_string),
    }
}
fn canonicalize_existing_dir(path: &Path) -> Result<PathBuf, ProfileError> {
    path.canonicalize()
        .map_err(|source| ProfileError::CommandIo {
            path: path.to_path_buf(),
            source,
        })
}
fn absolutize(path: &Path) -> Result<PathBuf, ProfileError> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()
            .map_err(|source| ProfileError::CommandIo {
                path: PathBuf::from("."),
                source,
            })?
            .join(path))
    }
}
fn join_if_relative(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

pub fn resolve_profile_artifact(
    source: ProfileSource,
    base_dir: &Path,
    raw_artifact: serde_json::Value,
) -> Result<ResolvedProfile, ProfileError> {
    resolve_profile_artifact_value(raw_artifact, source, base_dir, "artifact-worker")
}

pub fn resolve_profile_artifact_value(
    raw_artifact: serde_json::Value,
    source: ProfileSource,
    base_dir: &Path,
    worker_name: &str,
) -> Result<ResolvedProfile, ProfileError> {
    resolve_profile_value(
        source,
        base_dir,
        base_dir,
        ProfileResolveOptions::with_worker_name(worker_name),
        raw_artifact.clone(),
        raw_artifact,
        None,
    )
}

#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("invalid profile path {}: {message}", .path.display())]
    InvalidPath { path: PathBuf, message: String },
    #[error("unsupported profile type {}: {message}", .path.display())]
    UnsupportedProfileType { path: PathBuf, message: String },
    #[error("failed to access profile path {}: {source}", .path.display())]
    CommandIo {
        path: PathBuf,
        #[source]
        source: std::io::Error,
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
    #[error("failed to read workspace local manifest override {}: {source}", .path.display())]
    WorkspaceOverrideRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse workspace local manifest override {}: {source}", .path.display())]
    WorkspaceOverrideParse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("invalid workspace local manifest override {}: {message}", .path.display())]
    InvalidWorkspaceOverride { path: PathBuf, message: String },
    #[error("no default profile is configured")]
    NoDefaultProfile,
    #[error("profile resolution requires an explicit runtime Worker name")]
    MissingRuntimeWorkerName,
    #[error("profile not found: {selector}")]
    ProfileNotFound { selector: String },
    #[error("ambiguous profile name `{name}`; use a source-qualified selector such as {matches:?}")]
    AmbiguousProfileName { name: String, matches: Vec<String> },
    #[error("invalid Profile artifact: {0}")]
    InvalidProfile(String),
    #[error("failed to decode Profile: {source}")]
    ProfileDeserialize {
        #[source]
        source: serde_json::Error,
    },
    #[error("failed to resolve Profile into Manifest: {0}")]
    ManifestResolve(#[source] ResolveError),
    #[error("failed to serialize resolved manifest snapshot: {0}")]
    SnapshotSerialize(#[source] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ReasoningControl, ReasoningEffort, SchemeKind};
    use tempfile::TempDir;

    fn write_profile(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn parse_cli_preserves_paths_and_source_qualified_names() {
        assert!(matches!(
            ProfileSelector::parse_cli("./coder.toml"),
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
        let registry = ProfileDiscovery::with_sources(None, None)
            .discover()
            .unwrap();
        let default = registry.default_entry().unwrap();
        assert_eq!(default.source, ProfileRegistrySource::Builtin);
        assert_eq!(default.name, BUILTIN_DEFAULT_PROFILE_NAME);
        assert!(default.is_default);
        assert_eq!(default.path, None);
        assert_eq!(default.provenance, "builtin:default");
    }
    #[test]
    fn builtin_role_profiles_are_registered_and_resolve() {
        let tmp = TempDir::new().unwrap();
        let registry = ProfileDiscovery::with_sources(None, None)
            .discover()
            .unwrap();
        for expected in ["companion", "intake", "orchestrator", "coder", "reviewer"] {
            let entry = registry
                .select(&ProfileSelector::source_named(
                    ProfileRegistrySource::Builtin,
                    expected,
                ))
                .unwrap();
            assert_eq!(entry.source, ProfileRegistrySource::Builtin);
            assert_eq!(entry.path, None);
            assert_eq!(entry.provenance, format!("builtin:{expected}"));

            let resolved = ProfileResolver::new()
                .with_workspace_base(tmp.path())
                .resolve(
                    &ProfileSelector::source_named(ProfileRegistrySource::Builtin, expected),
                    ProfileResolveOptions::with_worker_name("role-worker"),
                )
                .unwrap();
            assert_eq!(
                resolved.profile.as_ref().unwrap().name.as_deref(),
                Some(expected)
            );
            assert_eq!(resolved.manifest.worker.name, "role-worker");
        }
    }

    #[test]
    fn builtin_role_profiles_preserve_role_tool_policy() {
        let tmp = TempDir::new().unwrap();
        let resolve = |role: &str| {
            ProfileResolver::new()
                .with_workspace_base(tmp.path())
                .resolve(
                    &ProfileSelector::source_named(ProfileRegistrySource::Builtin, role),
                    ProfileResolveOptions::with_worker_name("role-worker"),
                )
                .unwrap()
                .manifest
        };

        let companion = resolve("companion");
        assert!(companion.feature.task.enabled);
        assert!(companion.feature.workers.enabled);
        assert!(companion.scope.allow.is_empty());
        assert!(companion.scope.deny.is_empty());
        assert!(companion.delegation_scope.allow.is_empty());
        assert_eq!(companion.model.ref_.as_deref(), Some("codex-oauth/gpt-5.5"));
        assert!(companion.web.is_some());
        assert!(companion.feature.ticket.enabled);
        assert!(companion.feature.ticket.authoring);
        assert!(companion.feature.ticket.thread);
        assert!(!companion.feature.ticket.intake);
        assert!(!companion.feature.ticket.orchestration_control);
        assert_eq!(
            companion.compaction.as_ref().unwrap().threshold,
            Some(240000)
        );
        assert_eq!(
            companion.compaction.as_ref().unwrap().request_threshold,
            Some(270000)
        );
        assert_eq!(
            companion
                .compaction
                .as_ref()
                .unwrap()
                .worker_context_max_tokens,
            100000
        );

        let intake = resolve("intake");
        assert!(intake.feature.task.enabled);
        assert!(!intake.feature.workers.enabled);
        assert!(intake.feature.ticket.enabled);
        assert!(intake.feature.ticket.enabled);
        assert!(intake.feature.ticket.authoring);
        assert!(intake.feature.ticket.thread);
        assert!(intake.feature.ticket.intake);
        assert!(!intake.feature.ticket.orchestration_control);
        assert!(intake.scope.allow.is_empty());
        assert!(intake.delegation_scope.allow.is_empty());
        assert_eq!(intake.model.ref_.as_deref(), Some("codex-oauth/gpt-5.5"));
        assert!(intake.web.is_some());
        assert!(intake.compaction.is_some());

        let orchestrator = resolve("orchestrator");
        assert!(orchestrator.feature.task.enabled);
        assert!(orchestrator.feature.workers.enabled);
        assert!(orchestrator.feature.ticket.enabled);
        assert!(orchestrator.feature.ticket.enabled);
        assert!(!orchestrator.feature.ticket.authoring);
        assert!(orchestrator.feature.ticket.thread);
        assert!(!orchestrator.feature.ticket.intake);
        assert!(orchestrator.feature.ticket.orchestration_control);
        assert!(orchestrator.scope.allow.is_empty());
        assert!(orchestrator.delegation_scope.allow.is_empty());
        assert_eq!(
            orchestrator.model.ref_.as_deref(),
            Some("codex-oauth/gpt-5.5")
        );
        assert!(orchestrator.web.is_some());
        assert!(orchestrator.compaction.is_some());

        let coder = resolve("coder");
        assert!(coder.feature.task.enabled);
        assert!(!coder.feature.workers.enabled);
        assert!(coder.scope.allow.is_empty());
        assert!(coder.delegation_scope.allow.is_empty());
        assert_eq!(coder.model.ref_.as_deref(), Some("codex-oauth/gpt-5.5"));
        assert!(coder.web.is_some());
        assert!(coder.compaction.is_some());
        assert!(coder.feature.ticket.enabled);
        assert!(coder.feature.ticket.enabled);
        assert!(!coder.feature.ticket.authoring);
        assert!(coder.feature.ticket.thread);
        assert!(!coder.feature.ticket.intake);
        assert!(!coder.feature.ticket.orchestration_control);
        let reviewer = resolve("reviewer");
        assert!(reviewer.feature.task.enabled);
        assert!(!reviewer.feature.workers.enabled);
        assert!(reviewer.feature.ticket.enabled);
        assert!(reviewer.feature.ticket.enabled);
        assert!(!reviewer.feature.ticket.authoring);
        assert!(reviewer.feature.ticket.thread);
        assert!(!reviewer.feature.ticket.intake);
        assert!(!reviewer.feature.ticket.orchestration_control);
        assert!(reviewer.scope.allow.is_empty());
        assert!(reviewer.delegation_scope.allow.is_empty());
        assert_eq!(reviewer.model.ref_.as_deref(), Some("codex-oauth/gpt-5.5"));
        assert!(reviewer.web.is_some());
        assert!(reviewer.compaction.is_some());
    }

    #[test]
    fn profile_resolution_requires_runtime_worker_name() {
        let tmp = TempDir::new().unwrap();
        let err = ProfileResolver::new()
            .with_workspace_base(tmp.path())
            .resolve(&ProfileSelector::Default, ProfileResolveOptions::default())
            .unwrap_err();
        assert!(matches!(err, ProfileError::MissingRuntimeWorkerName));
    }

    #[test]
    fn resolves_toml_profile_with_runtime_worker_name_and_scope_intent() {
        let tmp = TempDir::new().unwrap();
        let profile = write_profile(
            tmp.path(),
            "coder.toml",
            r#"
slug = "coder"
scope = "workspace_read"

[model]
scheme = "anthropic"
model_id = "claude-sonnet-4-20250514"

[engine]
reasoning = "high"
"#,
        );
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let resolved = ProfileResolver::new()
            .with_workspace_base(&workspace)
            .resolve(
                &ProfileSelector::path(&profile),
                ProfileResolveOptions::with_worker_name("runtime-worker"),
            )
            .unwrap();
        assert_eq!(resolved.manifest.worker.name, "runtime-worker");
        assert_eq!(resolved.manifest.model.scheme, Some(SchemeKind::Anthropic));
        assert_eq!(
            resolved.manifest.engine.reasoning,
            Some(ReasoningControl::Effort(ReasoningEffort::High))
        );
        assert_eq!(resolved.manifest.scope.allow[0].target, workspace);
        assert!(resolved.manifest.delegation_scope.allow.is_empty());
        assert_eq!(
            resolved.manifest.scope.allow[0].permission,
            Permission::Read
        );
        assert_eq!(
            resolved.profile.as_ref().unwrap().name.as_deref(),
            Some("coder")
        );
    }

    #[test]
    fn profile_artifact_resolves_named_mcp_stdio_config_without_starting_command() {
        let tmp = TempDir::new().unwrap();
        let profile = write_profile(
            tmp.path(),
            "mcp.json",
            r#"{
  "slug": "mcp",
  "model": { "scheme": "anthropic", "model_id": "claude-sonnet-4-20250514" },
  "mcp": {
    "stdio_server": [
      {
        "name": "filesystem",
        "command": "definitely-not-spawned-during-profile-resolution",
        "args": ["--root", "."],
        "cwd": { "kind": "path", "path": "servers" },
        "env": {
          "inherit": ["PATH"],
          "set": {
            "SAFE_MODE": { "kind": "literal", "value": "1" },
            "API_TOKEN": { "kind": "secret_ref", "ref": "providers/mcp-token" },
            "FROM_ENV": { "kind": "env_ref", "name": "MCP_TOKEN" }
          }
        }
      }
    ]
  }
}"#,
        );
        std::fs::create_dir(tmp.path().join("servers")).unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();

        let resolved = ProfileResolver::new()
            .with_workspace_base(&workspace)
            .resolve(
                &ProfileSelector::path(&profile),
                ProfileResolveOptions::with_worker_name("runtime-worker"),
            )
            .unwrap();

        let server = &resolved.manifest.mcp.stdio_servers[0];
        assert_eq!(server.name, "filesystem");
        assert_eq!(
            server.command,
            "definitely-not-spawned-during-profile-resolution"
        );
        assert!(matches!(
            server.cwd,
            Some(McpStdioCwdPolicy::Path { ref path }) if path == &tmp.path().join("servers")
        ));
        assert!(matches!(
            server.env.set["API_TOKEN"],
            crate::McpEnvValue::SecretRef { .. }
        ));
    }

    #[test]
    fn resolves_profile_feature_flags_without_runtime_state() {
        let tmp = TempDir::new().unwrap();
        let profile = write_profile(
            tmp.path(),
            "feature.toml",
            r#"
slug = "feature"
scope = "workspace_read"
delegation_scope = "workspace_write"

[model]
scheme = "anthropic"
model_id = "claude-sonnet-4-20250514"

[feature.task]
enabled = true

[feature.memory]
enabled = false

[feature.web]
enabled = true

[feature.workers]
enabled = true

[feature.ticket]
enabled = true
authoring = false
thread = false
intake = false
orchestration_control = false
"#,
        );
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let resolved = ProfileResolver::new()
            .with_workspace_base(&workspace)
            .resolve(
                &ProfileSelector::path(&profile),
                ProfileResolveOptions::with_worker_name("runtime-worker"),
            )
            .unwrap();
        assert_eq!(resolved.manifest.worker.name, "runtime-worker");
        assert!(resolved.manifest.feature.task.enabled);
        assert!(!resolved.manifest.feature.memory.enabled);
        assert!(resolved.manifest.feature.web.enabled);
        assert!(resolved.manifest.feature.workers.enabled);
        assert!(resolved.manifest.feature.ticket.enabled);
        assert!(!resolved.manifest.feature.ticket.authoring);
        assert!(!resolved.manifest.feature.ticket.thread);
        assert!(!resolved.manifest.feature.ticket.intake);
        assert!(!resolved.manifest.feature.ticket.orchestration_control);
        assert_eq!(
            resolved.manifest.delegation_scope.allow[0].target,
            workspace
        );
    }

    #[test]
    fn compact_tokens_uses_explicit_context_limits() {
        let tmp = TempDir::new().unwrap();
        let profile = write_profile(
            tmp.path(),
            "ratio.toml",
            r#"
[model]
ref = "codex-oauth/gpt-5.5"

[compaction]
kind = "tokens"
threshold = 136000
request_threshold = 204000
worker_context_max_tokens = 68000
"#,
        );
        let resolved = ProfileResolver::new()
            .with_workspace_base(tmp.path())
            .resolve(
                &ProfileSelector::path(profile),
                ProfileResolveOptions::with_worker_name("p"),
            )
            .unwrap();
        let c = resolved.manifest.compaction.unwrap();
        assert_eq!(c.threshold, Some(136000));
        assert_eq!(c.request_threshold, Some(204000));
        assert_eq!(c.worker_context_max_tokens, 68000);
    }

    #[test]
    fn rejects_manifest_shaped_runtime_and_authority_fields() {
        for (value, needle) in [
            (serde_json::json!({"manifest": {}}), "manifest"),
            (serde_json::json!({"config": {}}), "config"),
            (serde_json::json!({"worker": {"name": "bad"}}), "worker"),
            (
                serde_json::json!({"model": {"ref": "codex-oauth/gpt-5.5"}, "scope": {"allow": []}}),
                "scope.allow",
            ),
            (
                serde_json::json!({"model": {"ref": "codex-oauth/gpt-5.5"}, "scope": {"deny": []}}),
                "scope.deny",
            ),
        ] {
            let err = resolve_profile_artifact(
                ProfileSource::Path {
                    path: PathBuf::from("/profiles/bad.json"),
                },
                Path::new("/workspace"),
                value,
            )
            .unwrap_err();
            assert!(err.to_string().contains(needle), "unexpected error: {err}");
        }
    }
    #[test]
    fn rejects_absolute_profile_paths() {
        let err = resolve_profile_artifact(ProfileSource::Path { path: PathBuf::from("/profiles/bad.json") }, Path::new("/workspace"), serde_json::json!({"model": { "scheme": "anthropic", "model_id": "m", "auth": {"kind":"api_key", "file":"/secret/key"} }, "scope": "workspace_write"})).unwrap_err();
        assert!(err.to_string().contains("model.auth.file"));
    }
    #[test]
    fn builtin_default_resolves_without_external_evaluator() {
        let tmp = TempDir::new().unwrap();
        let resolved = ProfileResolver::new()
            .with_workspace_base(tmp.path())
            .resolve(
                &ProfileSelector::source_named(ProfileRegistrySource::Builtin, "default"),
                ProfileResolveOptions::with_worker_name("runtime-workspace"),
            )
            .unwrap();
        assert_eq!(resolved.manifest.worker.name, "runtime-workspace");
        assert_eq!(
            resolved.manifest.model.ref_.as_deref(),
            Some("codex-oauth/gpt-5.5")
        );
        assert!(resolved.manifest.feature.ticket.enabled);
        assert!(resolved.manifest.feature.ticket.authoring);
        assert!(resolved.manifest.feature.ticket.thread);
        assert!(!resolved.manifest.feature.ticket.intake);
        assert!(!resolved.manifest.feature.ticket.orchestration_control);
        assert_eq!(
            resolved.profile.as_ref().unwrap().name.as_deref(),
            Some("default")
        );
        assert_eq!(
            resolved.source,
            ProfileSource::Registry {
                source: ProfileRegistrySource::Builtin,
                name: "default".into(),
                path: None,
                provenance: Some("builtin:default".into()),
            }
        );
    }
    #[test]
    fn workspace_local_override_layers_over_profile_defaults() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("project");
        let nested = workspace.join("nested");
        let yoi_dir = workspace.join(".yoi");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(&yoi_dir).unwrap();
        let override_path = yoi_dir.join(WORKSPACE_OVERRIDE_LOCAL_FILENAME);
        std::fs::write(
            &override_path,
            r#"
[worker]
prompt_pack = "prompts.toml"
[engine]
language = "ja"
[session]
record_event_trace = false
"#,
        )
        .unwrap();

        let resolved = ProfileResolver::new()
            .with_workspace_base(&nested)
            .resolve(
                &ProfileSelector::Default,
                ProfileResolveOptions::with_worker_name("runtime-worker"),
            )
            .unwrap();

        assert_eq!(resolved.manifest.worker.name, "runtime-worker");
        assert_eq!(resolved.manifest.engine.language, "ja");
        assert!(!resolved.manifest.session.record_event_trace);
        assert_eq!(
            resolved.manifest.worker.prompt_pack.as_deref(),
            Some(yoi_dir.join("prompts.toml").as_path())
        );
        assert!(resolved.manifest.scope.allow.is_empty());
        assert_eq!(
            resolved
                .manifest
                .profile
                .as_ref()
                .and_then(|snapshot| snapshot.workspace_override.as_ref())
                .map(|snapshot| snapshot.path.as_path()),
            Some(override_path.as_path())
        );
    }

    #[test]
    fn workspace_local_override_uses_nearest_ancestor() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("project");
        let nested = workspace.join("nested");
        let child = nested.join("child");
        let parent_yoi = workspace.join(".yoi");
        let nested_yoi = nested.join(".yoi");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::create_dir_all(&parent_yoi).unwrap();
        std::fs::create_dir_all(&nested_yoi).unwrap();
        std::fs::write(
            parent_yoi.join(WORKSPACE_OVERRIDE_LOCAL_FILENAME),
            r#"
[worker]
prompt_pack = "parent-prompts.toml"
[engine]
language = "parent"
"#,
        )
        .unwrap();
        let nested_override_path = nested_yoi.join(WORKSPACE_OVERRIDE_LOCAL_FILENAME);
        std::fs::write(
            &nested_override_path,
            r#"
[worker]
prompt_pack = "nested-prompts.toml"
[engine]
language = "nested"
"#,
        )
        .unwrap();

        let resolved = ProfileResolver::new()
            .with_workspace_base(&child)
            .resolve(
                &ProfileSelector::Default,
                ProfileResolveOptions::with_worker_name("runtime-worker"),
            )
            .unwrap();

        assert_eq!(resolved.manifest.engine.language, "nested");
        assert_eq!(
            resolved.manifest.worker.prompt_pack.as_deref(),
            Some(nested_yoi.join("nested-prompts.toml").as_path())
        );
        assert_eq!(
            resolved
                .manifest
                .profile
                .as_ref()
                .and_then(|snapshot| snapshot.workspace_override.as_ref())
                .map(|snapshot| snapshot.path.as_path()),
            Some(nested_override_path.as_path())
        );
    }

    #[test]
    fn workspace_local_override_rejects_runtime_worker_name() {
        let tmp = TempDir::new().unwrap();
        let yoi_dir = tmp.path().join(".yoi");
        std::fs::create_dir_all(&yoi_dir).unwrap();
        std::fs::write(
            yoi_dir.join(WORKSPACE_OVERRIDE_LOCAL_FILENAME),
            "[worker]\nname = \"not-local\"\n",
        )
        .unwrap();

        let err = ProfileResolver::new()
            .with_workspace_base(tmp.path())
            .resolve(
                &ProfileSelector::Default,
                ProfileResolveOptions::with_worker_name("runtime-worker"),
            )
            .unwrap_err();
        assert!(matches!(err, ProfileError::InvalidWorkspaceOverride { .. }));
        assert!(err.to_string().contains("worker.name"));
    }

    #[test]
    fn unsupported_profile_extension_has_clear_diagnostic() {
        let tmp = TempDir::new().unwrap();
        let path = write_profile(tmp.path(), "legacy.txt", "{}");
        let err = ProfileResolver::new()
            .with_workspace_base(tmp.path())
            .resolve(
                &ProfileSelector::path(path),
                ProfileResolveOptions::default(),
            )
            .unwrap_err();
        assert!(matches!(err, ProfileError::UnsupportedProfileType { .. }));
        assert!(
            err.to_string()
                .contains("Profiles must be .json or .toml artifacts")
        );
    }
    #[test]
    fn discovery_reads_user_and_project_registry_and_project_default_wins() {
        let tmp = TempDir::new().unwrap();
        let user_config = tmp.path().join("profiles.toml");
        let project_dir = tmp.path().join("project/.yoi");
        std::fs::create_dir_all(&project_dir).unwrap();
        let project_config = project_dir.join("profiles.toml");
        std::fs::write(
            &user_config,
            "default = \"coder\"\n[profile]\ncoder = \"profiles/user-coder.toml\"\n",
        )
        .unwrap();
        std::fs::write(&project_config, "default = \"project:coder\"\n[profile.coder]\npath = \"profiles/project-coder.toml\"\ndescription = \"Project coder\"\n").unwrap();
        let registry = ProfileDiscovery::with_sources(Some(user_config), Some(project_config))
            .discover()
            .unwrap();
        let default = registry.default_entry().unwrap();
        assert_eq!(default.source, ProfileRegistrySource::Project);
        assert_eq!(default.name, "coder");
        assert!(
            default
                .path
                .as_ref()
                .unwrap()
                .ends_with("profiles/project-coder.toml")
        );
    }
    #[test]
    fn default_marks_direct_profile_entry() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path().join("project/.yoi");
        std::fs::create_dir_all(&project_dir).unwrap();
        let project_config = project_dir.join("profiles.toml");
        std::fs::write(
            &project_config,
            "default = \"coder\"\n[profile]\ncoder = \"profiles/coder.toml\"\n",
        )
        .unwrap();
        let registry = ProfileDiscovery::with_sources(None, Some(project_config))
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
        registry.push_entry(ProfileRegistryEntry::path(
            ProfileRegistrySource::User,
            "coder".to_string(),
            PathBuf::from("/user/coder.toml"),
            None,
        ));
        registry.push_entry(ProfileRegistryEntry::path(
            ProfileRegistrySource::Project,
            "coder".to_string(),
            PathBuf::from("/project/coder.toml"),
            None,
        ));
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
        assert_eq!(
            selected.path.as_deref(),
            Some(Path::new("/project/coder.toml"))
        );
    }
}
