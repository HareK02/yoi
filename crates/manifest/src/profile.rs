//! Lua profile discovery and resolution.
//!
//! Profiles are reusable, human-authored recipes. They are intentionally not
//! complete runtime manifests: runtime-bound and authority-bearing fields such
//! as `pod.name` and concrete `scope.allow` rules are supplied by the resolver
//! from launch context.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use mlua::{Lua, LuaOptions, LuaSerdeExt, RegistryKey, StdLib, Table, Value as LuaValue};
use serde::{Deserialize, Serialize};

use crate::config::{
    CompactionConfigPartial, FeatureConfigPartial, PermissionConfigPartial, SessionConfigPartial,
};
use crate::model::{AuthRef, ModelManifest};
use crate::{
    MemoryConfig, Permission, PodManifest, PodManifestConfig, PodMetaConfig, ResolveError,
    ScopeConfig, ScopeRule, SkillsConfig, WebConfig, WorkerManifestConfig, paths,
};

const PROFILE_FORMAT_V1: &str = "yoi.lua-profile.v1";
const BUILTIN_DEFAULT_PROFILE_NAME: &str = "default";
const BUILTIN_DEFAULT_PROFILE: &str = include_str!("../../../resources/profiles/default.lua");
const BUILTIN_COMPANION_PROFILE: &str = include_str!("../../../resources/profiles/companion.lua");
const BUILTIN_INTAKE_PROFILE: &str = include_str!("../../../resources/profiles/intake.lua");
const BUILTIN_ORCHESTRATOR_PROFILE: &str =
    include_str!("../../../resources/profiles/orchestrator.lua");
const BUILTIN_CODER_PROFILE: &str = include_str!("../../../resources/profiles/coder.lua");
const BUILTIN_REVIEWER_PROFILE: &str = include_str!("../../../resources/profiles/reviewer.lua");
const BUILTIN_MODEL_CATALOG: &str = include_str!("../../../resources/models/builtin.toml");
const WORKSPACE_OVERRIDE_LOCAL_FILENAME: &str = "override.local.toml";

struct BuiltinProfile {
    name: &'static str,
    label: &'static str,
    content: &'static str,
    description: &'static str,
}

const BUILTIN_PROFILES: &[BuiltinProfile] = &[
    BuiltinProfile {
        name: BUILTIN_DEFAULT_PROFILE_NAME,
        label: "builtin:default",
        content: BUILTIN_DEFAULT_PROFILE,
        description: "Bundled default Yoi coding profile",
    },
    BuiltinProfile {
        name: "companion",
        label: "builtin:companion",
        content: BUILTIN_COMPANION_PROFILE,
        description: "Bundled Companion role profile",
    },
    BuiltinProfile {
        name: "intake",
        label: "builtin:intake",
        content: BUILTIN_INTAKE_PROFILE,
        description: "Bundled Intake role profile",
    },
    BuiltinProfile {
        name: "orchestrator",
        label: "builtin:orchestrator",
        content: BUILTIN_ORCHESTRATOR_PROFILE,
        description: "Bundled Orchestrator role profile",
    },
    BuiltinProfile {
        name: "coder",
        label: "builtin:coder",
        content: BUILTIN_CODER_PROFILE,
        description: "Bundled Coder role profile",
    },
    BuiltinProfile {
        name: "reviewer",
        label: "builtin:reviewer",
        content: BUILTIN_REVIEWER_PROFILE,
        description: "Bundled Reviewer role profile",
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
        if raw.contains('/') || raw.starts_with('.') || raw.ends_with(".lua") {
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
        content: &'static str,
        description: Option<String>,
    ) -> Self {
        Self {
            source,
            name: name.to_string(),
            path: None,
            provenance: label.to_string(),
            description,
            is_default: false,
            artifact: ProfileRegistryArtifact::Embedded { label, content },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ProfileRegistryArtifact {
    Path(PathBuf),
    Embedded {
        label: &'static str,
        content: &'static str,
    },
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
    config: PodManifestConfig,
}

#[derive(Debug, Clone)]
pub struct ResolvedProfile {
    pub source: ProfileSource,
    pub profile: Option<ProfileMetadata>,
    pub manifest: PodManifest,
    pub manifest_snapshot: serde_json::Value,
    pub raw_artifact: serde_json::Value,
}

#[derive(Debug, Clone, Default)]
pub struct ProfileResolveOptions {
    pub pod_name: Option<String>,
}
impl ProfileResolveOptions {
    pub fn with_pod_name(name: impl Into<String>) -> Self {
        Self {
            pod_name: Some(name.into()),
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
    /// registry. Callers such as SpawnPod use this to bind discovery to the
    /// Pod's cwd instead of the process current directory.
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
            ProfileRegistryArtifact::Embedded { label, content } => {
                self.resolve_embedded_profile(label, content, source, options)
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
        let extension = absolute_path
            .extension()
            .and_then(|s| s.to_str())
            .map(str::to_string);
        match extension.as_deref() {
            Some("lua") => {}
            other => {
                return Err(ProfileError::UnsupportedProfileType {
                    path: absolute_path,
                    message: format!(
                        "unsupported profile extension {}; Lua profiles must end in .lua",
                        other.map_or("<none>".to_string(), |s| format!(".{s}"))
                    ),
                });
            }
        }
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
        let lua_value = evaluate_lua_profile(&absolute_path, &profile_dir)?;
        let raw_artifact = lua_value.clone();
        resolve_lua_profile_value(
            source,
            &profile_dir,
            &workspace_base,
            options,
            lua_value,
            raw_artifact,
            workspace_override,
        )
    }

    fn resolve_embedded_profile(
        &self,
        label: &'static str,
        content: &'static str,
        source: ProfileSource,
        options: ProfileResolveOptions,
    ) -> Result<ResolvedProfile, ProfileError> {
        let workspace_base = absolutize(
            self.workspace_base
                .as_deref()
                .unwrap_or_else(|| Path::new(".")),
        )?;
        let workspace_override = load_workspace_override_from(&workspace_base)?;
        let lua_value = evaluate_embedded_lua_profile(label, content)?;
        let raw_artifact = lua_value.clone();
        resolve_lua_profile_value(
            source,
            &workspace_base,
            &workspace_base,
            options,
            lua_value,
            raw_artifact,
            workspace_override,
        )
    }
}

fn resolve_lua_profile_value(
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
    let pod_name = options
        .pod_name
        .ok_or(ProfileError::MissingRuntimePodName)?;
    let profile_meta = Some(ProfileMetadata {
        name: profile.slug.clone().or_else(|| source_name(&source)),
        description: profile.description.clone(),
        format: Some(PROFILE_FORMAT_V1.to_string()),
    });
    let compaction = profile_compaction_to_partial(profile.compaction, &profile.model)?;
    let config = PodManifestConfig {
        pod: PodMetaConfig {
            name: Some(pod_name),
            prompt_pack: None,
        },
        model: profile.model.unwrap_or_default(),
        worker: profile.worker.unwrap_or_default(),
        scope: profile_scope_to_config(profile.scope, workspace_base)?,
        delegation_scope: profile_delegation_scope_to_config(
            profile.delegation_scope,
            workspace_base,
        )?,
        session: profile.session,
        permissions: profile.permissions,
        feature: profile.feature,
        compaction,
        web: profile.web,
        memory: profile.memory,
        skills: profile.skills,
    };
    let mut config = PodManifestConfig::builtin_defaults().merge(config.resolve_paths(profile_dir));
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
    let mut manifest = PodManifest::try_from(config).map_err(ProfileError::ManifestResolve)?;
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
    worker: Option<WorkerManifestConfig>,
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
    let config = PodManifestConfig::from_toml(&content).map_err(|source| {
        ProfileError::WorkspaceOverrideParse {
            path: path.to_path_buf(),
            source,
        }
    })?;
    if config.pod.name.is_some() {
        return Err(ProfileError::InvalidWorkspaceOverride {
            path: path.to_path_buf(),
            message: "workspace-local manifest overrides cannot set pod.name; Pod identity is a runtime input".into(),
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
            profile.content,
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

fn evaluate_lua_profile(
    path: &Path,
    module_root: &Path,
) -> Result<serde_json::Value, ProfileError> {
    let content = std::fs::read_to_string(path).map_err(|source| ProfileError::ConfigRead {
        path: path.to_path_buf(),
        source,
    })?;
    evaluate_lua_profile_source(
        &content,
        path.display().to_string(),
        LocalModuleRoot::Filesystem(module_root.to_path_buf()),
    )
}

fn evaluate_embedded_lua_profile(
    label: &'static str,
    content: &'static str,
) -> Result<serde_json::Value, ProfileError> {
    evaluate_lua_profile_source(
        content,
        label.to_string(),
        LocalModuleRoot::Disabled { label },
    )
}

fn evaluate_lua_profile_source(
    content: &str,
    chunk_name: String,
    module_root: LocalModuleRoot,
) -> Result<serde_json::Value, ProfileError> {
    let lua = Lua::new_with(
        StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8,
        LuaOptions::default(),
    )
    .map_err(ProfileError::Lua)?;
    install_lua_api(&lua, module_root)?;
    let value: LuaValue = lua
        .load(content)
        .set_name(chunk_name)
        .eval()
        .map_err(ProfileError::Lua)?;
    match value {
        LuaValue::Table(_) => lua.from_value(value).map_err(ProfileError::Lua),
        _ => Err(ProfileError::InvalidProfile(
            "Lua profile must return a table or profile { ... }".into(),
        )),
    }
}

fn install_lua_api(lua: &Lua, module_root: LocalModuleRoot) -> Result<(), ProfileError> {
    let loader = Rc::new(RefCell::new(LocalModuleLoader {
        root: module_root,
        cache: HashMap::new(),
        loading: HashSet::new(),
    }));
    let require_loader = Rc::clone(&loader);
    let require = lua
        .create_function(move |lua, name: String| require_module(lua, &require_loader, &name))
        .map_err(ProfileError::Lua)?;
    let globals = lua.globals();
    globals.set("require", require).map_err(ProfileError::Lua)?;
    let yoi = yoi_module(lua).map_err(ProfileError::Lua)?;
    let profile = yoi
        .get::<mlua::Value>("profile")
        .map_err(ProfileError::Lua)?;
    globals.set("yoi", yoi).map_err(ProfileError::Lua)?;
    globals.set("profile", profile).map_err(ProfileError::Lua)?;
    for denied in [
        "os",
        "io",
        "debug",
        "package",
        "dofile",
        "loadfile",
        "load",
        "collectgarbage",
    ] {
        globals
            .set(denied, LuaValue::Nil)
            .map_err(ProfileError::Lua)?;
    }
    Ok(())
}

struct LocalModuleLoader {
    root: LocalModuleRoot,
    cache: HashMap<String, RegistryKey>,
    loading: HashSet<String>,
}

enum LocalModuleRoot {
    Filesystem(PathBuf),
    Disabled { label: &'static str },
}

fn require_module(
    lua: &Lua,
    loader: &Rc<RefCell<LocalModuleLoader>>,
    name: &str,
) -> mlua::Result<LuaValue> {
    if let Some(value) = host_module(lua, name)? {
        return Ok(value);
    }
    if name.starts_with("yoi.") || name == "yoi" {
        return Err(mlua::Error::RuntimeError(format!(
            "unknown host module `{name}`"
        )));
    }
    validate_module_name(name).map_err(mlua::Error::RuntimeError)?;
    if let Some(key) = loader.borrow().cache.get(name) {
        return lua.registry_value(key);
    }
    {
        let mut state = loader.borrow_mut();
        if !state.loading.insert(name.to_string()) {
            return Err(mlua::Error::RuntimeError(format!(
                "cyclic local require `{name}`"
            )));
        }
    }
    let path = {
        let state = loader.borrow();
        match &state.root {
            LocalModuleRoot::Filesystem(root) => {
                local_module_path(root, name).map_err(mlua::Error::RuntimeError)?
            }
            LocalModuleRoot::Disabled { label } => {
                return Err(mlua::Error::RuntimeError(format!(
                    "local require `{name}` is not available for embedded profile `{label}`"
                )));
            }
        }
    };
    let content = std::fs::read_to_string(&path).map_err(|e| {
        mlua::Error::RuntimeError(format!(
            "failed to read local module `{name}` ({}): {e}",
            path.display()
        ))
    })?;
    let result: mlua::Result<LuaValue> = lua
        .load(&content)
        .set_name(path.display().to_string())
        .eval();
    loader.borrow_mut().loading.remove(name);
    let value = result?;
    let key = lua.create_registry_value(value.clone())?;
    loader.borrow_mut().cache.insert(name.to_string(), key);
    Ok(value)
}

fn host_module(lua: &Lua, name: &str) -> mlua::Result<Option<LuaValue>> {
    match name {
        "yoi" => Ok(Some(LuaValue::Table(yoi_module(lua)?))),
        "yoi.profile" => Ok(Some(LuaValue::Table(profile_module(lua)?))),
        "yoi.models" => Ok(Some(LuaValue::Table(models_module(lua)?))),
        "yoi.compact" => Ok(Some(LuaValue::Table(compact_module(lua)?))),
        "yoi.scope" => Ok(Some(LuaValue::Table(scope_module(lua)?))),
        _ => Ok(None),
    }
}
fn yoi_module(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("profile", profile_module(lua)?)?;
    t.set("models", models_module(lua)?)?;
    t.set("compact", compact_module(lua)?)?;
    t.set("scope", scope_module(lua)?)?;
    Ok(t)
}
fn profile_module(lua: &Lua) -> mlua::Result<Table> {
    let module = lua.create_table()?;
    module.set(
        "import",
        lua.create_function(|lua, reference: String| import_profile_artifact(lua, &reference))?,
    )?;
    module.set(
        "extend",
        lua.create_function(|_, (_reference, _overrides): (String, LuaValue)| {
            Err::<LuaValue, _>(mlua::Error::RuntimeError(
                "yoi.profile.extend has been removed; use yoi.profile.import(...) and explicit Lua assignment instead".to_string(),
            ))
        })?,
    )?;
    let meta = lua.create_table()?;
    meta.set(
        "__call",
        lua.create_function(|_, (_this, table): (LuaValue, Table)| Ok(table))?,
    )?;
    module.set_metatable(Some(meta))?;
    Ok(module)
}
fn import_profile_artifact(lua: &Lua, reference: &str) -> mlua::Result<LuaValue> {
    let profile = builtin_profile_by_ref(reference).ok_or_else(|| {
        mlua::Error::RuntimeError(format!("unsupported profile import `{reference}`"))
    })?;
    lua.load(profile.content)
        .set_name(profile.label)
        .eval::<LuaValue>()
}
fn builtin_profile_by_ref(reference: &str) -> Option<&'static BuiltinProfile> {
    let name = reference.strip_prefix("builtin:").unwrap_or(reference);
    BUILTIN_PROFILES
        .iter()
        .find(|profile| profile.name == name || profile.label == reference)
}
fn models_module(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set(
        "catalog",
        lua.create_function(|lua, reference: String| {
            let model = lua.create_table()?;
            model.set("ref", reference)?;
            Ok(model)
        })?,
    )?;
    Ok(t)
}
fn compact_module(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set(
        "ratio",
        lua.create_function(|_, table: Table| {
            table.set("kind", "ratio")?;
            Ok(table)
        })?,
    )?;
    t.set(
        "tokens",
        lua.create_function(|_, table: Table| {
            table.set("kind", "tokens")?;
            Ok(table)
        })?,
    )?;
    Ok(t)
}
fn scope_module(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set(
        "workspace_write",
        lua.create_function(|lua, options: LuaValue| {
            scope_intent_table(lua, "workspace_write", options)
        })?,
    )?;
    t.set(
        "workspace_read",
        lua.create_function(|lua, options: LuaValue| {
            scope_intent_table(lua, "workspace_read", options)
        })?,
    )?;
    Ok(t)
}
fn scope_intent_table(lua: &Lua, intent: &str, options: LuaValue) -> mlua::Result<Table> {
    let v = lua.create_table()?;
    v.set("intent", intent)?;
    match options {
        LuaValue::Nil => {}
        LuaValue::Table(options) => {
            for pair in options.pairs::<String, LuaValue>() {
                let (key, value) = pair?;
                match key.as_str() {
                    "deny_write" => v.set("deny_write", value)?,
                    other => {
                        return Err(mlua::Error::RuntimeError(format!(
                            "unsupported yoi.scope option `{other}`"
                        )));
                    }
                }
            }
        }
        other => {
            return Err(mlua::Error::RuntimeError(format!(
                "yoi.scope.{intent} options must be a table, got {}",
                other.type_name()
            )));
        }
    }
    Ok(v)
}

fn validate_module_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("empty module name".into());
    }
    for part in name.split('.') {
        let mut chars = part.chars();
        let Some(first) = chars.next() else {
            return Err(format!("invalid local module name `{name}`"));
        };
        if !(first == '_' || first.is_ascii_alphabetic())
            || !chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
        {
            return Err(format!("invalid local module name `{name}`"));
        }
    }
    Ok(())
}
fn local_module_path(root: &Path, name: &str) -> Result<PathBuf, String> {
    let mut path = root.to_path_buf();
    for part in name.split('.') {
        path.push(part);
    }
    path.set_extension("lua");
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("local module `{name}` not found: {e}"))?;
    if !canonical.starts_with(root) {
        return Err(format!("local module `{name}` escapes profile directory"));
    }
    Ok(canonical)
}

fn reject_manifest_shaped_profile(value: &serde_json::Value) -> Result<(), ProfileError> {
    let Some(map) = value.as_object() else {
        return Err(ProfileError::InvalidProfile(
            "Lua profile must return an object/table".into(),
        ));
    };
    for key in ["manifest", "config"] {
        if map.contains_key(key) {
            return Err(ProfileError::InvalidProfile(format!(
                "field `{key}` is a complete Manifest artifact boundary and is not allowed in reusable Profiles; use --manifest for complete Manifests"
            )));
        }
    }
    if map.contains_key("pod") {
        return Err(ProfileError::InvalidProfile("field `pod` is runtime-bound and is not allowed in reusable Profiles; pass the Pod name via CLI/TUI runtime inputs".into()));
    }
    if let Some(scope) = map.get("scope").and_then(|v| v.as_object()) {
        for key in ["allow", "deny"] {
            if scope.contains_key(key) {
                return Err(ProfileError::InvalidProfile(format!(
                    "field `scope.{key}` grants concrete authority and is not allowed in reusable Profiles; use require(\"yoi.scope\") intent helpers"
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
    resolve_lua_profile_value(
        source,
        base_dir,
        base_dir,
        ProfileResolveOptions::with_pod_name("artifact-pod"),
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
    #[error("profile resolution requires an explicit runtime Pod name")]
    MissingRuntimePodName,
    #[error("profile not found: {selector}")]
    ProfileNotFound { selector: String },
    #[error("ambiguous profile name `{name}`; use a source-qualified selector such as {matches:?}")]
    AmbiguousProfileName { name: String, matches: Vec<String> },
    #[error("failed to evaluate Lua profile: {0}")]
    Lua(#[source] mlua::Error),
    #[error("invalid Lua profile: {0}")]
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
            ProfileSelector::parse_cli("./coder.lua"),
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
                    ProfileResolveOptions::with_pod_name("role-pod"),
                )
                .unwrap();
            assert_eq!(
                resolved.profile.as_ref().unwrap().name.as_deref(),
                Some(expected)
            );
            assert_eq!(resolved.manifest.pod.name, "role-pod");
            if matches!(expected, "intake" | "orchestrator" | "coder" | "reviewer") {
                let expected_instruction = format!("$yoi/role/{expected}");
                assert_eq!(resolved.manifest.worker.instruction, expected_instruction);
            }
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
                    ProfileResolveOptions::with_pod_name("role-pod"),
                )
                .unwrap()
                .manifest
        };

        let companion = resolve("companion");
        assert!(companion.feature.task.enabled);
        assert!(companion.feature.pods.enabled);
        assert!(companion.feature.ticket.enabled);
        assert!(companion.scope.allow.is_empty());
        assert!(companion.scope.deny.is_empty());
        assert!(companion.delegation_scope.allow.is_empty());
        assert_eq!(companion.model.ref_.as_deref(), Some("codex-oauth/gpt-5.5"));
        assert!(companion.web.is_some());

        let intake = resolve("intake");
        assert!(!intake.feature.task.enabled);
        assert!(!intake.feature.pods.enabled);
        assert!(intake.feature.ticket.enabled);
        assert!(intake.scope.allow.is_empty());
        assert!(intake.delegation_scope.allow.is_empty());
        assert_eq!(intake.model.ref_.as_deref(), Some("codex-oauth/gpt-5.5"));
        assert!(intake.web.is_some());
        assert!(!intake.feature.ticket_orchestration.enabled);

        let orchestrator = resolve("orchestrator");
        assert!(!orchestrator.feature.task.enabled);
        assert!(orchestrator.feature.pods.enabled);
        assert!(orchestrator.feature.ticket.enabled);
        assert!(orchestrator.feature.ticket_orchestration.enabled);
        assert!(orchestrator.scope.allow.is_empty());
        assert!(orchestrator.delegation_scope.allow.is_empty());
        assert_eq!(
            orchestrator.model.ref_.as_deref(),
            Some("codex-oauth/gpt-5.5")
        );
        assert!(orchestrator.web.is_some());

        let coder = resolve("coder");
        assert!(coder.feature.task.enabled);
        assert!(!coder.feature.pods.enabled);
        assert!(coder.scope.allow.is_empty());
        assert!(coder.delegation_scope.allow.is_empty());
        assert_eq!(coder.model.ref_.as_deref(), Some("codex-oauth/gpt-5.5"));
        assert!(coder.web.is_some());

        let reviewer = resolve("reviewer");
        assert!(!reviewer.feature.task.enabled);
        assert!(!reviewer.feature.pods.enabled);
        assert!(!reviewer.feature.ticket.enabled);
        assert!(reviewer.scope.allow.is_empty());
        assert!(reviewer.delegation_scope.allow.is_empty());
        assert_eq!(reviewer.model.ref_.as_deref(), Some("codex-oauth/gpt-5.5"));
        assert!(reviewer.web.is_some());
    }

    #[test]
    fn profile_resolution_requires_runtime_pod_name() {
        let tmp = TempDir::new().unwrap();
        let err = ProfileResolver::new()
            .with_workspace_base(tmp.path())
            .resolve(&ProfileSelector::Default, ProfileResolveOptions::default())
            .unwrap_err();
        assert!(matches!(err, ProfileError::MissingRuntimePodName));
    }

    #[test]
    fn resolves_plain_lua_profile_with_runtime_pod_name_and_scope_intent() {
        let tmp = TempDir::new().unwrap();
        let profile = write_profile(
            tmp.path(),
            "coder.lua",
            r#"
local profile = require("yoi.profile")
local scope = require("yoi.scope")
return profile {
  slug = "coder",
  model = { scheme = "anthropic", model_id = "claude-sonnet-4-20250514" },
  worker = { reasoning = "high" },
  scope = scope.workspace_read(),
}
"#,
        );
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let resolved = ProfileResolver::new()
            .with_workspace_base(&workspace)
            .resolve(
                &ProfileSelector::path(&profile),
                ProfileResolveOptions::with_pod_name("runtime-pod"),
            )
            .unwrap();
        assert_eq!(resolved.manifest.pod.name, "runtime-pod");
        assert_eq!(resolved.manifest.model.scheme, Some(SchemeKind::Anthropic));
        assert_eq!(
            resolved.manifest.worker.reasoning,
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
    fn resolves_lua_profile_feature_flags_without_runtime_state() {
        let tmp = TempDir::new().unwrap();
        let profile = write_profile(
            tmp.path(),
            "feature.lua",
            r#"
local profile = require("yoi.profile")
local scope = require("yoi.scope")
return profile {
  slug = "feature",
  model = { scheme = "anthropic", model_id = "claude-sonnet-4-20250514" },
  scope = scope.workspace_read(),
  delegation_scope = scope.workspace_write(),
  feature = {
    task = { enabled = true },
    memory = { enabled = false },
    web = { enabled = true },
    pods = { enabled = true },
    ticket = { enabled = true, access = "read_only" },
    ticket_orchestration = { enabled = false },
  },
}
"#,
        );
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let resolved = ProfileResolver::new()
            .with_workspace_base(&workspace)
            .resolve(
                &ProfileSelector::path(&profile),
                ProfileResolveOptions::with_pod_name("runtime-pod"),
            )
            .unwrap();
        assert_eq!(resolved.manifest.pod.name, "runtime-pod");
        assert!(resolved.manifest.feature.task.enabled);
        assert!(!resolved.manifest.feature.memory.enabled);
        assert!(resolved.manifest.feature.web.enabled);
        assert!(resolved.manifest.feature.pods.enabled);
        assert!(resolved.manifest.feature.ticket.enabled);
        assert_eq!(
            resolved.manifest.feature.ticket.access,
            crate::TicketFeatureAccessConfig::ReadOnly
        );
        assert!(!resolved.manifest.feature.ticket_orchestration.enabled);
        assert_eq!(
            resolved.manifest.delegation_scope.allow[0].target,
            workspace
        );
    }

    #[test]
    fn host_modules_and_local_require_work() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("shared.lua"),
            r#"return { model = require("yoi.models").catalog("codex-oauth/gpt-5.5") }"#,
        )
        .unwrap();
        let profile = write_profile(
            tmp.path(),
            "main.lua",
            r#"
local yoi = require("yoi")
local shared = require("shared")
return yoi.profile {
  slug = "main",
  model = shared.model,
  scope = yoi.scope.workspace_write(),
  delegation_scope = yoi.scope.workspace_write(),
}
"#,
        );
        let resolved = ProfileResolver::new()
            .with_workspace_base(tmp.path())
            .resolve(
                &ProfileSelector::path(profile),
                ProfileResolveOptions::with_pod_name("p"),
            )
            .unwrap();
        assert_eq!(
            resolved.manifest.model.ref_.as_deref(),
            Some("codex-oauth/gpt-5.5")
        );
        assert_eq!(
            resolved.manifest.scope.allow[0].permission,
            Permission::Write
        );
        assert_eq!(
            resolved.manifest.delegation_scope.allow[0].target,
            tmp.path().canonicalize().unwrap()
        );
        assert_eq!(
            resolved.manifest.delegation_scope.allow[0].permission,
            Permission::Write
        );
    }
    #[test]
    fn global_yoi_import_supports_explicit_lua_assignment() {
        let tmp = TempDir::new().unwrap();
        let profile = write_profile(
            tmp.path(),
            "assigned.lua",
            r#"
local p = yoi.profile.import("builtin:default")
assert(p.model.ref == "codex-oauth/gpt-5.5")
p.slug = "assigned"
p.model = yoi.models.catalog("anthropic/claude-sonnet-4-6")
p.feature = {
  task = { enabled = false },
  pods = { enabled = true },
}
p.web = { enabled = false }
p.compaction = yoi.compact.tokens { threshold = 123, request_threshold = 456 }
return p
"#,
        );
        let resolved = ProfileResolver::new()
            .with_workspace_base(tmp.path())
            .resolve(
                &ProfileSelector::path(profile),
                ProfileResolveOptions::with_pod_name("p"),
            )
            .unwrap();
        assert_eq!(
            resolved.manifest.model.ref_.as_deref(),
            Some("anthropic/claude-sonnet-4-6")
        );
        assert!(!resolved.manifest.feature.task.enabled);
        assert!(resolved.manifest.feature.pods.enabled);
        assert_eq!(resolved.manifest.web.as_ref().unwrap().enabled, Some(false));
        assert!(resolved.manifest.web.as_ref().unwrap().search.is_none());
        assert_eq!(
            resolved.manifest.compaction.as_ref().unwrap().threshold,
            Some(123)
        );
        assert_eq!(
            resolved.profile.as_ref().unwrap().name.as_deref(),
            Some("assigned")
        );
    }

    #[test]
    fn global_yoi_extend_fails_with_removed_api_diagnostic() {
        let tmp = TempDir::new().unwrap();
        let profile = write_profile(
            tmp.path(),
            "bad.lua",
            r#"
return yoi.profile.extend("builtin:default", {
  slug = "bad",
})
"#,
        );
        let err = ProfileResolver::new()
            .with_workspace_base(tmp.path())
            .resolve(
                &ProfileSelector::path(profile),
                ProfileResolveOptions::with_pod_name("p"),
            )
            .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("yoi.profile.extend"), "{message}");
        assert!(message.contains("removed"), "{message}");
        assert!(message.contains("yoi.profile.import"), "{message}");
    }

    #[test]
    fn sandbox_denies_unsafe_libraries() {
        let tmp = TempDir::new().unwrap();
        for (name, body) in [
            ("os.lua", "return os.getenv('HOME')"),
            ("io.lua", "return io.open('x')"),
            ("debug.lua", "return debug.getinfo(1)"),
            ("package.lua", "return package.path"),
        ] {
            let path = write_profile(tmp.path(), name, body);
            let err = ProfileResolver::new()
                .with_workspace_base(tmp.path())
                .resolve(
                    &ProfileSelector::path(path),
                    ProfileResolveOptions::with_pod_name("p"),
                )
                .unwrap_err();
            assert!(matches!(
                err,
                ProfileError::Lua(_) | ProfileError::InvalidProfile(_)
            ));
        }
    }
    #[test]
    fn rejects_manifest_shaped_runtime_and_authority_fields() {
        for (value, needle) in [
            (serde_json::json!({"manifest": {}}), "manifest"),
            (serde_json::json!({"config": {}}), "config"),
            (serde_json::json!({"pod": {"name": "bad"}}), "pod"),
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
                    path: PathBuf::from("/profiles/bad.lua"),
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
        let err = resolve_profile_artifact(ProfileSource::Path { path: PathBuf::from("/profiles/bad.lua") }, Path::new("/workspace"), serde_json::json!({"model": { "scheme": "anthropic", "model_id": "m", "auth": {"kind":"api_key", "file":"/secret/key"} }, "scope": "workspace_write"})).unwrap_err();
        assert!(err.to_string().contains("model.auth.file"));
    }
    #[test]
    fn compact_ratio_uses_known_model_context() {
        let tmp = TempDir::new().unwrap();
        let profile = write_profile(
            tmp.path(),
            "ratio.lua",
            r#"
local profile = require("yoi.profile")
local models = require("yoi.models")
local compact = require("yoi.compact")
return profile {
  model = models.catalog("codex-oauth/gpt-5.5"),
  compaction = compact.ratio { threshold = 0.5, request = 0.75, worker = 0.25 },
}
"#,
        );
        let resolved = ProfileResolver::new()
            .with_workspace_base(tmp.path())
            .resolve(
                &ProfileSelector::path(profile),
                ProfileResolveOptions::with_pod_name("p"),
            )
            .unwrap();
        let c = resolved.manifest.compaction.unwrap();
        assert_eq!(c.threshold, Some(136000));
        assert_eq!(c.request_threshold, Some(204000));
        assert_eq!(c.worker_context_max_tokens, 68000);
    }
    #[test]
    fn builtin_default_resolves_without_external_evaluator() {
        let tmp = TempDir::new().unwrap();
        let resolved = ProfileResolver::new()
            .with_workspace_base(tmp.path())
            .resolve(
                &ProfileSelector::source_named(ProfileRegistrySource::Builtin, "default"),
                ProfileResolveOptions::with_pod_name("runtime-workspace"),
            )
            .unwrap();
        assert_eq!(resolved.manifest.pod.name, "runtime-workspace");
        assert_eq!(
            resolved.manifest.model.ref_.as_deref(),
            Some("codex-oauth/gpt-5.5")
        );
        assert!(resolved.manifest.scope.allow.is_empty());
        assert!(resolved.manifest.delegation_scope.allow.is_empty());
        assert!(resolved.manifest.session.record_event_trace);
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
[pod]
prompt_pack = "prompts.toml"
[worker]
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
                ProfileResolveOptions::with_pod_name("runtime-pod"),
            )
            .unwrap();

        assert_eq!(resolved.manifest.pod.name, "runtime-pod");
        assert_eq!(resolved.manifest.worker.language, "ja");
        assert!(!resolved.manifest.session.record_event_trace);
        assert_eq!(
            resolved.manifest.pod.prompt_pack.as_deref(),
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
[pod]
prompt_pack = "parent-prompts.toml"
[worker]
language = "parent"
"#,
        )
        .unwrap();
        let nested_override_path = nested_yoi.join(WORKSPACE_OVERRIDE_LOCAL_FILENAME);
        std::fs::write(
            &nested_override_path,
            r#"
[pod]
prompt_pack = "nested-prompts.toml"
[worker]
language = "nested"
"#,
        )
        .unwrap();

        let resolved = ProfileResolver::new()
            .with_workspace_base(&child)
            .resolve(
                &ProfileSelector::Default,
                ProfileResolveOptions::with_pod_name("runtime-pod"),
            )
            .unwrap();

        assert_eq!(resolved.manifest.worker.language, "nested");
        assert_eq!(
            resolved.manifest.pod.prompt_pack.as_deref(),
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
    fn workspace_local_override_rejects_runtime_pod_name() {
        let tmp = TempDir::new().unwrap();
        let yoi_dir = tmp.path().join(".yoi");
        std::fs::create_dir_all(&yoi_dir).unwrap();
        std::fs::write(
            yoi_dir.join(WORKSPACE_OVERRIDE_LOCAL_FILENAME),
            "[pod]\nname = \"not-local\"\n",
        )
        .unwrap();

        let err = ProfileResolver::new()
            .with_workspace_base(tmp.path())
            .resolve(
                &ProfileSelector::Default,
                ProfileResolveOptions::with_pod_name("runtime-pod"),
            )
            .unwrap_err();
        assert!(matches!(err, ProfileError::InvalidWorkspaceOverride { .. }));
        assert!(err.to_string().contains("pod.name"));
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
        assert!(err.to_string().contains("Lua profiles must end in .lua"));
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
            "default = \"coder\"\n[profile]\ncoder = \"profiles/user-coder.lua\"\n",
        )
        .unwrap();
        std::fs::write(&project_config, "default = \"project:coder\"\n[profile.coder]\npath = \"profiles/project-coder.lua\"\ndescription = \"Project coder\"\n").unwrap();
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
                .ends_with("profiles/project-coder.lua")
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
            "default = \"coder\"\n[profile]\ncoder = \"profiles/coder.lua\"\n",
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
            PathBuf::from("/user/coder.lua"),
            None,
        ));
        registry.push_entry(ProfileRegistryEntry::path(
            ProfileRegistrySource::Project,
            "coder".to_string(),
            PathBuf::from("/project/coder.lua"),
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
            Some(Path::new("/project/coder.lua"))
        );
    }
}
