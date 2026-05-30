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

use crate::config::{CompactionConfigPartial, PermissionConfigPartial, SessionConfigPartial};
use crate::model::{AuthRef, ModelManifest};
use crate::{
    MemoryConfig, Permission, PodManifest, PodManifestConfig, PodMetaConfig, ResolveError,
    ScopeConfig, ScopeRule, SkillsConfig, WebConfig, WorkerManifestConfig, paths,
};

const PROFILE_FORMAT_V1: &str = "insomnia.lua-profile.v1";
const BUILTIN_DEFAULT_PROFILE_NAME: &str = "default";
const DEFAULT_POD_NAME: &str = "insomnia";

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
        path: PathBuf,
    },
}

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
                let entry = registry.select(selector)?.clone();
                self.resolve_path(
                    &entry.path,
                    ProfileSource::Registry {
                        source: entry.source,
                        name: entry.name,
                        path: absolutize(&entry.path)?,
                    },
                    options,
                )
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
        let lua_value = evaluate_lua_profile(&absolute_path, &profile_dir)?;
        let raw_artifact = lua_value.clone();
        resolve_lua_profile_value(
            source,
            &profile_dir,
            &workspace_base,
            options,
            lua_value,
            raw_artifact,
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
        .unwrap_or_else(|| derive_pod_name(&source, profile.slug.as_deref()));
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
        scope: profile_scope_to_config(profile.scope, workspace_base),
        session: profile.session,
        permissions: profile.permissions,
        compaction,
        web: profile.web,
        memory: profile.memory,
        skills: profile.skills,
    };
    let config = PodManifestConfig::builtin_defaults().merge(config.resolve_paths(profile_dir));
    let mut manifest = PodManifest::try_from(config).map_err(ProfileError::ManifestResolve)?;
    manifest.profile = Some(ProfileManifestSnapshot {
        source: source.clone(),
        profile: profile_meta.clone(),
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
    session: Option<SessionConfigPartial>,
    #[serde(default)]
    permissions: Option<PermissionConfigPartial>,
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
    Intent { intent: ProfileScopeIntent },
    String(ProfileScopeIntent),
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
        if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("lua") {
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
            let profile = path.join("profile.lua");
            if profile.is_file()
                && let Some(name) = path.file_name().and_then(|s| s.to_str())
            {
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
    Ok(())
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
    let lua = Lua::new_with(
        StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8,
        LuaOptions::default(),
    )
    .map_err(ProfileError::Lua)?;
    install_lua_api(&lua, module_root.to_path_buf())?;
    let value: LuaValue = lua
        .load(&content)
        .set_name(path.display().to_string())
        .eval()
        .map_err(ProfileError::Lua)?;
    match value {
        LuaValue::Table(_) => lua.from_value(value).map_err(ProfileError::Lua),
        _ => Err(ProfileError::InvalidProfile(
            "Lua profile must return a table or profile { ... }".into(),
        )),
    }
}

fn install_lua_api(lua: &Lua, module_root: PathBuf) -> Result<(), ProfileError> {
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
    globals
        .set(
            "profile",
            lua.create_function(|_, table: Table| Ok(table))
                .map_err(ProfileError::Lua)?,
        )
        .map_err(ProfileError::Lua)?;
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
    root: PathBuf,
    cache: HashMap<String, RegistryKey>,
    loading: HashSet<String>,
}

fn require_module(
    lua: &Lua,
    loader: &Rc<RefCell<LocalModuleLoader>>,
    name: &str,
) -> mlua::Result<LuaValue> {
    if let Some(value) = host_module(lua, name)? {
        return Ok(value);
    }
    if name.starts_with("insomnia.") || name == "insomnia" {
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
    let path = local_module_path(&loader.borrow().root, name).map_err(mlua::Error::RuntimeError)?;
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
        "insomnia" => {
            let t = lua.create_table()?;
            t.set("profile", profile_function(lua)?)?;
            t.set("models", models_module(lua)?)?;
            t.set("compact", compact_module(lua)?)?;
            t.set("scope", scope_module(lua)?)?;
            Ok(Some(LuaValue::Table(t)))
        }
        "insomnia.profile" => Ok(Some(LuaValue::Function(profile_function(lua)?))),
        "insomnia.models" => Ok(Some(LuaValue::Table(models_module(lua)?))),
        "insomnia.compact" => Ok(Some(LuaValue::Table(compact_module(lua)?))),
        "insomnia.scope" => Ok(Some(LuaValue::Table(scope_module(lua)?))),
        _ => Ok(None),
    }
}
fn profile_function(lua: &Lua) -> mlua::Result<mlua::Function> {
    lua.create_function(|_, table: Table| Ok(table))
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
        lua.create_function(|lua, ()| {
            let v = lua.create_table()?;
            v.set("intent", "workspace_write")?;
            Ok(v)
        })?,
    )?;
    t.set(
        "workspace_read",
        lua.create_function(|lua, ()| {
            let v = lua.create_table()?;
            v.set("intent", "workspace_read")?;
            Ok(v)
        })?,
    )?;
    Ok(t)
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
                    "field `scope.{key}` grants concrete authority and is not allowed in reusable Profiles; use require(\"insomnia.scope\") intent helpers"
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
    if let Some(AuthRef::ApiKey {
        file: Some(file), ..
    }) = auth
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
) -> ScopeConfig {
    let intent = match scope {
        Some(ProfileScopeConfig::Intent { intent }) | Some(ProfileScopeConfig::String(intent)) => {
            intent
        }
        None => ProfileScopeIntent::WorkspaceWrite,
    };
    let permission = match intent {
        ProfileScopeIntent::WorkspaceRead => Permission::Read,
        ProfileScopeIntent::WorkspaceWrite => Permission::Write,
    };
    ScopeConfig {
        allow: vec![ScopeRule {
            target: workspace_base.to_path_buf(),
            permission,
            recursive: true,
        }],
        deny: Vec::new(),
    }
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
    let path = paths::resource_dir()?.join("models").join("builtin.toml");
    let content = std::fs::read_to_string(path).ok()?;
    let parsed: toml::Value = toml::from_str(&content).ok()?;
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
fn derive_pod_name(source: &ProfileSource, slug: Option<&str>) -> String {
    if matches!(source, ProfileSource::Registry { source: ProfileRegistrySource::Builtin, name, .. } if name == BUILTIN_DEFAULT_PROFILE_NAME)
        || slug == Some(BUILTIN_DEFAULT_PROFILE_NAME)
    {
        return DEFAULT_POD_NAME.to_string();
    }
    let raw = slug
        .map(str::to_string)
        .or_else(|| source_name(source))
        .unwrap_or_else(|| DEFAULT_POD_NAME.to_string());
    sanitise_pod_name(&raw)
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
fn sanitise_pod_name(raw: &str) -> String {
    let name: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    if name.is_empty() {
        DEFAULT_POD_NAME.to_string()
    } else {
        name
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
        ProfileResolveOptions::default(),
        raw_artifact.clone(),
        raw_artifact,
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
    #[error("no default profile is configured")]
    NoDefaultProfile,
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
                "INSOMNIA_RESOURCE_DIR",
                "INSOMNIA_HOME",
                "XDG_CONFIG_HOME",
                "HOME",
            ];
            let saved: Vec<_> = names.iter().map(|n| (*n, std::env::var(n).ok())).collect();
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
        let registry = ProfileDiscovery::with_sources(paths::builtin_profiles_dir(), None, None)
            .discover()
            .unwrap();
        let default = registry.default_entry().unwrap();
        assert_eq!(default.source, ProfileRegistrySource::Builtin);
        assert_eq!(default.name, BUILTIN_DEFAULT_PROFILE_NAME);
        assert!(default.is_default);
        assert!(default.path.ends_with("resources/profiles/default.lua"));
    }
    #[test]
    fn resolves_plain_lua_profile_with_runtime_pod_name_and_scope_intent() {
        let tmp = TempDir::new().unwrap();
        let profile = write_profile(
            tmp.path(),
            "coder.lua",
            r#"
local profile = require("insomnia.profile")
local scope = require("insomnia.scope")
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
    fn host_modules_and_local_require_work() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("shared.lua"),
            r#"return { model = require("insomnia.models").catalog("codex-oauth/gpt-5.5") }"#,
        )
        .unwrap();
        let profile = write_profile(
            tmp.path(),
            "main.lua",
            r#"
local insomnia = require("insomnia")
local shared = require("shared")
return insomnia.profile {
  slug = "main",
  model = shared.model,
  scope = insomnia.scope.workspace_write(),
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
local profile = require("insomnia.profile")
local models = require("insomnia.models")
local compact = require("insomnia.compact")
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
            .resolve(&ProfileSelector::Default, ProfileResolveOptions::default())
            .unwrap();
        assert_eq!(resolved.manifest.pod.name, "insomnia");
        assert_eq!(
            resolved.manifest.model.ref_.as_deref(),
            Some("codex-oauth/gpt-5.5")
        );
        assert_eq!(resolved.manifest.scope.allow[0].target, tmp.path());
        assert_eq!(
            resolved.manifest.scope.allow[0].permission,
            Permission::Write
        );
        assert!(resolved.manifest.session.record_event_trace);
        assert_eq!(
            resolved.profile.as_ref().unwrap().name.as_deref(),
            Some("default")
        );
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
            "[profiles]\ndefault = \"wrong\"\n[profiles.profile]\nwrong = \"wrong.lua\"\n",
        )
        .unwrap();
        std::fs::write(
            insomnia.join("profiles.toml"),
            "default = \"coder\"\n[profile]\ncoder = \"profiles/coder.lua\"\n",
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
            "[profiles]\ndefault = \"wrong\"\n[profiles.profile]\nwrong = \"wrong.lua\"\n",
        )
        .unwrap();
        std::fs::write(
            config_dir.join("profiles.toml"),
            "default = \"coder\"\n[profile]\ncoder = \"profiles/coder.lua\"\n",
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
            "default = \"coder\"\n[profile]\ncoder = \"profiles/user-coder.lua\"\n",
        )
        .unwrap();
        std::fs::write(&project_config, "default = \"project:coder\"\n[profile.coder]\npath = \"profiles/project-coder.lua\"\ndescription = \"Project coder\"\n").unwrap();
        let registry =
            ProfileDiscovery::with_sources(None, Some(user_config), Some(project_config))
                .discover()
                .unwrap();
        let default = registry.default_entry().unwrap();
        assert_eq!(default.source, ProfileRegistrySource::Project);
        assert_eq!(default.name, "coder");
        assert!(default.path.ends_with("profiles/project-coder.lua"));
    }
    #[test]
    fn default_marks_direct_profile_entry() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path().join("project/.insomnia");
        std::fs::create_dir_all(&project_dir).unwrap();
        let project_config = project_dir.join("profiles.toml");
        std::fs::write(
            &project_config,
            "default = \"coder\"\n[profile]\ncoder = \"profiles/coder.lua\"\n",
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
            path: PathBuf::from("/user/coder.lua"),
            description: None,
            is_default: false,
        });
        registry.push_entry(ProfileRegistryEntry {
            source: ProfileRegistrySource::Project,
            name: "coder".to_string(),
            path: PathBuf::from("/project/coder.lua"),
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
        assert_eq!(selected.path, PathBuf::from("/project/coder.lua"));
    }
}
