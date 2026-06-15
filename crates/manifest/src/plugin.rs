use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const SUPPORTED_PLUGIN_API_VERSION: u32 = 1;
const ZIP_EOCD: u32 = 0x0605_4b50;
const ZIP_CENTRAL_DIRECTORY: u32 = 0x0201_4b50;
const ZIP_LOCAL_FILE: u32 = 0x0403_4b50;
const ZIP_FLAG_ENCRYPTED: u16 = 0x0001;
const ZIP_COMPRESSION_STORED: u16 = 0;
const ZIP_UNIX_SYMLINK_TYPE: u32 = 0o120000;
const ZIP_UNIX_FILE_TYPE_MASK: u32 = 0o170000;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PluginConfig {
    pub enabled: Vec<PluginEnablementConfig>,
    /// Runtime restore metadata. Fresh resolution fills this from discovered packages;
    /// restore uses it without selecting newer mutable-store contents.
    pub resolved: Vec<ResolvedPluginRecord>,
    /// Safe bounded discovery/resolution diagnostics recorded with the resolved plan.
    pub diagnostics: Vec<PluginDiagnostic>,
}

impl PluginConfig {
    pub fn is_empty(&self) -> bool {
        self.enabled.is_empty() && self.resolved.is_empty()
    }

    pub fn has_resolved_plan(&self) -> bool {
        !self.resolved.is_empty() || !self.diagnostics.is_empty()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PluginEnablementConfig {
    /// Source-qualified plugin id such as `user:example`, `project:example`, or `builtin:example`.
    pub id: String,
    /// Optional exact package version requirement. Rich version constraints are deferred.
    pub version: Option<PluginExactVersion>,
    /// Optional deterministic digest pin in `sha256:<hex>` form.
    pub digest: Option<String>,
    /// Optional explicit surface subset. When omitted, all declared package surfaces are selected.
    pub surfaces: Vec<PluginSurface>,
    /// Requested plugin grants. Non-empty authority-bearing grants currently fail closed.
    pub grants: PluginGrantConfig,
    /// Opaque plugin-local configuration copied into resolved metadata without interpretation.
    pub config: Option<toml::Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PluginExactVersion(pub String);

impl PluginExactVersion {
    pub fn matches(&self, version: &str) -> bool {
        self.0 == version
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PluginGrantConfig {
    pub tools: Vec<String>,
    pub secrets: Vec<String>,
    pub filesystem: Vec<String>,
    pub network: bool,
}

impl PluginGrantConfig {
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
            && self.secrets.is_empty()
            && self.filesystem.is_empty()
            && !self.network
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginSurface {
    Hook,
    Tool,
    Service,
    Ingress,
    Wasm,
}

impl fmt::Display for PluginSurface {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PluginSurface::Hook => f.write_str("hook"),
            PluginSurface::Tool => f.write_str("tool"),
            PluginSurface::Service => f.write_str("service"),
            PluginSurface::Ingress => f.write_str("ingress"),
            PluginSurface::Wasm => f.write_str("wasm"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginSourceKind {
    User,
    Project,
    Builtin,
}

impl PluginSourceKind {
    fn qualifier(self) -> &'static str {
        match self {
            PluginSourceKind::User => "user",
            PluginSourceKind::Project => "project",
            PluginSourceKind::Builtin => "builtin",
        }
    }
}

impl fmt::Display for PluginSourceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.qualifier())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SourceQualifiedPluginId {
    pub source: PluginSourceKind,
    pub local_id: String,
}

impl SourceQualifiedPluginId {
    pub fn new(source: PluginSourceKind, local_id: impl Into<String>) -> Self {
        Self {
            source,
            local_id: local_id.into(),
        }
    }

    pub fn parse(value: &str) -> Result<Self, PluginIdParseError> {
        let Some((source, local_id)) = value.split_once(':') else {
            return Err(PluginIdParseError::Unqualified);
        };
        if local_id.is_empty() || local_id.contains(':') || !is_safe_id(local_id) {
            return Err(PluginIdParseError::InvalidLocalId);
        }
        let source = match source {
            "user" => PluginSourceKind::User,
            "project" => PluginSourceKind::Project,
            "builtin" => PluginSourceKind::Builtin,
            _ => return Err(PluginIdParseError::InvalidSource),
        };
        Ok(Self {
            source,
            local_id: local_id.to_string(),
        })
    }
}

impl fmt::Display for SourceQualifiedPluginId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.source, self.local_id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PluginIdParseError {
    Unqualified,
    InvalidSource,
    InvalidLocalId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginPackageManifest {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    #[serde(default)]
    pub surfaces: Vec<PluginSurface>,
    #[serde(default)]
    pub runtime: Option<PluginRuntimeManifest>,
    #[serde(default)]
    pub hooks: Vec<PluginHookManifest>,
}

impl PluginPackageManifest {
    fn declared_surfaces(&self) -> BTreeSet<PluginSurface> {
        let mut surfaces: BTreeSet<_> = self.surfaces.iter().copied().collect();
        if !self.hooks.is_empty() {
            surfaces.insert(PluginSurface::Hook);
        }
        if self.runtime.is_some() {
            surfaces.insert(PluginSurface::Wasm);
        }
        surfaces
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginRuntimeManifest {
    pub kind: String,
    pub entry: String,
    pub abi: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginHookManifest {
    pub id: String,
    pub file: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginDiscoveryLimits {
    pub max_packages_per_store: usize,
    pub max_package_size_bytes: u64,
    pub max_manifest_size_bytes: usize,
    pub max_entries_per_package: usize,
    pub max_file_size_bytes: u64,
    pub max_expanded_size_bytes: u64,
}

impl Default for PluginDiscoveryLimits {
    fn default() -> Self {
        Self {
            max_packages_per_store: 128,
            max_package_size_bytes: 16 * 1024 * 1024,
            max_manifest_size_bytes: 64 * 1024,
            max_entries_per_package: 512,
            max_file_size_bytes: 8 * 1024 * 1024,
            max_expanded_size_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginDiscoveryOptions {
    pub workspace_root: PathBuf,
    pub user_data_home: Option<PathBuf>,
    pub limits: PluginDiscoveryLimits,
}

impl PluginDiscoveryOptions {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            user_data_home: None,
            limits: PluginDiscoveryLimits::default(),
        }
    }

    pub fn with_user_data_home(mut self, user_data_home: impl Into<PathBuf>) -> Self {
        self.user_data_home = Some(user_data_home.into());
        self
    }

    pub fn with_limits(mut self, limits: PluginDiscoveryLimits) -> Self {
        self.limits = limits;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredPluginPackage {
    pub identity: SourceQualifiedPluginId,
    pub package_path: PathBuf,
    pub package_label: String,
    pub digest: String,
    pub manifest: PluginPackageManifest,
    pub entries: BTreeSet<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PluginDiscoveryReport {
    pub packages: Vec<DiscoveredPluginPackage>,
    pub diagnostics: Vec<PluginDiagnostic>,
}

impl PluginDiscoveryReport {
    pub fn package(&self, identity: &SourceQualifiedPluginId) -> Vec<&DiscoveredPluginPackage> {
        self.packages
            .iter()
            .filter(|package| &package.identity == identity)
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedPlugin {
    pub identity: SourceQualifiedPluginId,
    pub source: PluginSourceKind,
    pub package_path: PathBuf,
    pub package_label: String,
    pub digest: String,
    pub manifest: PluginPackageManifest,
    pub enabled_surfaces: Vec<PluginSurface>,
    pub grants: PluginGrantConfig,
    pub config: Option<toml::Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResolvedPluginRecord {
    pub identity: SourceQualifiedPluginId,
    pub source: PluginSourceKind,
    pub package_path: PathBuf,
    pub package_label: String,
    pub digest: String,
    pub version: String,
    pub manifest: PluginPackageManifest,
    pub enabled_surfaces: Vec<PluginSurface>,
    pub grants: PluginGrantConfig,
    pub config: Option<toml::Value>,
}

impl ResolvedPluginRecord {
    fn from_resolved(resolved: &ResolvedPlugin) -> Self {
        Self {
            identity: resolved.identity.clone(),
            source: resolved.source,
            package_path: resolved.package_path.clone(),
            package_label: resolved.package_label.clone(),
            digest: resolved.digest.clone(),
            version: resolved.manifest.version.clone(),
            manifest: resolved.manifest.clone(),
            enabled_surfaces: resolved.enabled_surfaces.clone(),
            grants: resolved.grants.clone(),
            config: resolved.config.clone(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PluginResolution {
    pub resolved: Vec<ResolvedPlugin>,
    pub diagnostics: Vec<PluginDiagnostic>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginDiagnostic {
    pub kind: PluginDiagnosticKind,
    pub phase: PluginDiagnosticPhase,
    pub source: Option<PluginSourceKind>,
    pub identity: Option<String>,
    pub package: Option<String>,
    pub digest: Option<String>,
    pub message: String,
}

impl PluginDiagnostic {
    fn new(
        kind: PluginDiagnosticKind,
        phase: PluginDiagnosticPhase,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            phase,
            source: None,
            identity: None,
            package: None,
            digest: None,
            message: message.into(),
        }
    }

    fn with_source(mut self, source: PluginSourceKind) -> Self {
        self.source = Some(source);
        self
    }

    fn with_identity(mut self, identity: impl ToString) -> Self {
        self.identity = Some(identity.to_string());
        self
    }

    fn with_package(mut self, package: impl Into<String>) -> Self {
        self.package = Some(package.into());
        self
    }

    fn with_digest(mut self, digest: impl Into<String>) -> Self {
        self.digest = Some(digest.into());
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginDiagnosticKind {
    Missing,
    Duplicate,
    Ambiguous,
    Version,
    Digest,
    Api,
    Surface,
    Grant,
    Malformed,
    Traversal,
    Bounds,
    Io,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginDiagnosticPhase {
    Discovery,
    Manifest,
    Resolution,
}

pub fn discover_plugins(options: &PluginDiscoveryOptions) -> PluginDiscoveryReport {
    let mut report = PluginDiscoveryReport::default();
    let stores = plugin_stores(options);

    for store in stores {
        discover_store(&store, &options.limits, &mut report);
    }

    let mut counts: BTreeMap<SourceQualifiedPluginId, usize> = BTreeMap::new();
    for package in &report.packages {
        *counts.entry(package.identity.clone()).or_default() += 1;
    }
    for (identity, count) in counts {
        if count > 1 {
            report.diagnostics.push(
                PluginDiagnostic::new(
                    PluginDiagnosticKind::Duplicate,
                    PluginDiagnosticPhase::Discovery,
                    "duplicate plugin package identity in one source store",
                )
                .with_source(identity.source)
                .with_identity(identity),
            );
        }
    }

    report.packages.sort_by(|left, right| {
        left.identity
            .cmp(&right.identity)
            .then_with(|| left.digest.cmp(&right.digest))
            .then_with(|| left.package_label.cmp(&right.package_label))
    });
    report
}

pub fn resolve_enabled_plugins(
    config: &PluginConfig,
    discovery: &PluginDiscoveryReport,
) -> PluginResolution {
    let mut resolution = PluginResolution::default();

    for enablement in &config.enabled {
        let identity = match SourceQualifiedPluginId::parse(&enablement.id) {
            Ok(identity) => identity,
            Err(PluginIdParseError::Unqualified) => {
                resolution.diagnostics.push(
                    PluginDiagnostic::new(
                        PluginDiagnosticKind::Ambiguous,
                        PluginDiagnosticPhase::Resolution,
                        "plugin enablement id must be source-qualified as user:<id>, project:<id>, or builtin:<id>",
                    )
                    .with_identity(&enablement.id),
                );
                continue;
            }
            Err(PluginIdParseError::InvalidSource | PluginIdParseError::InvalidLocalId) => {
                resolution.diagnostics.push(
                    PluginDiagnostic::new(
                        PluginDiagnosticKind::Malformed,
                        PluginDiagnosticPhase::Resolution,
                        "plugin enablement id is not a valid source-qualified plugin id",
                    )
                    .with_identity(&enablement.id),
                );
                continue;
            }
        };

        let matches = discovery.package(&identity);
        let package = match matches.as_slice() {
            [] => {
                resolution.diagnostics.push(
                    PluginDiagnostic::new(
                        PluginDiagnosticKind::Missing,
                        PluginDiagnosticPhase::Resolution,
                        "enabled plugin package was not discovered",
                    )
                    .with_source(identity.source)
                    .with_identity(identity),
                );
                continue;
            }
            [package] => *package,
            _ => {
                resolution.diagnostics.push(
                    PluginDiagnostic::new(
                        PluginDiagnosticKind::Duplicate,
                        PluginDiagnosticPhase::Resolution,
                        "enabled plugin package identity resolved to multiple discovered packages",
                    )
                    .with_source(identity.source)
                    .with_identity(identity),
                );
                continue;
            }
        };

        if let Some(expected_digest) = &enablement.digest {
            if !digest_matches(expected_digest, &package.digest) {
                resolution.diagnostics.push(
                    PluginDiagnostic::new(
                        PluginDiagnosticKind::Digest,
                        PluginDiagnosticPhase::Resolution,
                        "enabled plugin digest pin does not match discovered package digest",
                    )
                    .with_source(identity.source)
                    .with_identity(&identity)
                    .with_package(&package.package_label)
                    .with_digest(&package.digest),
                );
                continue;
            }
        }

        if let Some(required_version) = &enablement.version {
            if !required_version.matches(&package.manifest.version) {
                resolution.diagnostics.push(
                    PluginDiagnostic::new(
                        PluginDiagnosticKind::Version,
                        PluginDiagnosticPhase::Resolution,
                        "enabled plugin exact version requirement does not match discovered package version",
                    )
                    .with_source(identity.source)
                    .with_identity(&identity)
                    .with_package(&package.package_label)
                    .with_digest(&package.digest),
                );
                continue;
            }
        }

        if !enablement.grants.is_empty() {
            resolution.diagnostics.push(
                PluginDiagnostic::new(
                    PluginDiagnosticKind::Grant,
                    PluginDiagnosticPhase::Resolution,
                    "plugin authority grants are not implemented and fail closed",
                )
                .with_source(identity.source)
                .with_identity(&identity)
                .with_package(&package.package_label)
                .with_digest(&package.digest),
            );
            continue;
        }

        let declared_surfaces = package.manifest.declared_surfaces();
        let selected_surfaces: BTreeSet<_> = if enablement.surfaces.is_empty() {
            declared_surfaces.clone()
        } else {
            enablement.surfaces.iter().copied().collect()
        };
        if let Some(surface) = selected_surfaces
            .iter()
            .find(|surface| !declared_surfaces.contains(surface))
        {
            resolution.diagnostics.push(
                PluginDiagnostic::new(
                    PluginDiagnosticKind::Surface,
                    PluginDiagnosticPhase::Resolution,
                    format!("enabled plugin requested undeclared surface `{surface}`"),
                )
                .with_source(identity.source)
                .with_identity(&identity)
                .with_package(&package.package_label)
                .with_digest(&package.digest),
            );
            continue;
        }

        resolution.resolved.push(ResolvedPlugin {
            identity: identity.clone(),
            source: identity.source,
            package_path: package.package_path.clone(),
            package_label: package.package_label.clone(),
            digest: package.digest.clone(),
            manifest: package.manifest.clone(),
            enabled_surfaces: selected_surfaces.into_iter().collect(),
            grants: enablement.grants.clone(),
            config: enablement.config.clone(),
        });
    }

    resolution
}

pub fn resolve_plugin_config_for_startup(
    config: &PluginConfig,
    options: &PluginDiscoveryOptions,
) -> PluginConfig {
    if config.enabled.is_empty() || config.has_resolved_plan() {
        return config.clone();
    }

    let discovery = discover_plugins(options);
    let resolution = resolve_enabled_plugins(config, &discovery);
    let mut snapshot = config.clone();
    snapshot.resolved = resolution
        .resolved
        .iter()
        .map(ResolvedPluginRecord::from_resolved)
        .collect();
    snapshot.diagnostics = discovery.diagnostics;
    snapshot.diagnostics.extend(resolution.diagnostics);
    snapshot
}

#[derive(Clone, Debug)]
struct PluginStore {
    source: PluginSourceKind,
    path: PathBuf,
}

fn plugin_stores(options: &PluginDiscoveryOptions) -> Vec<PluginStore> {
    let user_data_home = options
        .user_data_home
        .clone()
        .or_else(|| std::env::var_os("XDG_DATA_HOME").map(PathBuf::from))
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(".local/share"))
        });

    let mut stores = Vec::new();
    if let Some(user_data_home) = user_data_home {
        stores.push(PluginStore {
            source: PluginSourceKind::User,
            path: user_data_home.join("yoi/plugins"),
        });
    }
    stores.push(PluginStore {
        source: PluginSourceKind::Project,
        path: options.workspace_root.join(".yoi/plugins"),
    });
    stores
}

fn discover_store(
    store: &PluginStore,
    limits: &PluginDiscoveryLimits,
    report: &mut PluginDiscoveryReport,
) {
    let canonical_store = match fs::canonicalize(&store.path) {
        Ok(path) => path,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return,
        Err(error) => {
            report.diagnostics.push(
                PluginDiagnostic::new(
                    PluginDiagnosticKind::Io,
                    PluginDiagnosticPhase::Discovery,
                    format!("plugin store could not be read: {}", safe_io_error(&error)),
                )
                .with_source(store.source),
            );
            return;
        }
    };

    let entries = match fs::read_dir(&canonical_store) {
        Ok(entries) => entries,
        Err(error) => {
            report.diagnostics.push(
                PluginDiagnostic::new(
                    PluginDiagnosticKind::Io,
                    PluginDiagnosticPhase::Discovery,
                    format!(
                        "plugin store could not be listed: {}",
                        safe_io_error(&error)
                    ),
                )
                .with_source(store.source),
            );
            return;
        }
    };

    let mut candidates = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) == Some("yoi-plugin") {
            candidates.push(path);
        }
    }
    candidates.sort();

    if candidates.len() > limits.max_packages_per_store {
        report.diagnostics.push(
            PluginDiagnostic::new(
                PluginDiagnosticKind::Bounds,
                PluginDiagnosticPhase::Discovery,
                "plugin store contains more packages than the configured discovery bound",
            )
            .with_source(store.source),
        );
        candidates.truncate(limits.max_packages_per_store);
    }

    for candidate in candidates {
        let label = package_label(&candidate);
        let canonical_candidate = match fs::canonicalize(&candidate) {
            Ok(path) => path,
            Err(error) => {
                report.diagnostics.push(
                    PluginDiagnostic::new(
                        PluginDiagnosticKind::Io,
                        PluginDiagnosticPhase::Discovery,
                        format!(
                            "plugin package could not be read: {}",
                            safe_io_error(&error)
                        ),
                    )
                    .with_source(store.source)
                    .with_package(label),
                );
                continue;
            }
        };
        if !canonical_candidate.starts_with(&canonical_store) {
            report.diagnostics.push(
                PluginDiagnostic::new(
                    PluginDiagnosticKind::Traversal,
                    PluginDiagnosticPhase::Discovery,
                    "plugin package path escapes its source store",
                )
                .with_source(store.source)
                .with_package(package_label(&candidate)),
            );
            continue;
        }
        let metadata = match fs::metadata(&canonical_candidate) {
            Ok(metadata) => metadata,
            Err(error) => {
                report.diagnostics.push(
                    PluginDiagnostic::new(
                        PluginDiagnosticKind::Io,
                        PluginDiagnosticPhase::Discovery,
                        format!(
                            "plugin package metadata could not be read: {}",
                            safe_io_error(&error)
                        ),
                    )
                    .with_source(store.source)
                    .with_package(label),
                );
                continue;
            }
        };
        if !metadata.is_file() {
            report.diagnostics.push(
                PluginDiagnostic::new(
                    PluginDiagnosticKind::Malformed,
                    PluginDiagnosticPhase::Discovery,
                    "plugin package candidate is not a regular file",
                )
                .with_source(store.source)
                .with_package(label),
            );
            continue;
        }
        if metadata.len() > limits.max_package_size_bytes {
            report.diagnostics.push(
                PluginDiagnostic::new(
                    PluginDiagnosticKind::Bounds,
                    PluginDiagnosticPhase::Discovery,
                    "plugin package exceeds the configured package size bound",
                )
                .with_source(store.source)
                .with_package(label),
            );
            continue;
        }

        match read_package(&canonical_candidate, &label, store.source, limits) {
            Ok(package) => report.packages.push(package),
            Err(diagnostic) => report.diagnostics.push(diagnostic),
        }
    }
}

fn read_package(
    path: &Path,
    label: &str,
    source: PluginSourceKind,
    limits: &PluginDiscoveryLimits,
) -> Result<DiscoveredPluginPackage, PluginDiagnostic> {
    let bytes = fs::read(path).map_err(|error| {
        PluginDiagnostic::new(
            PluginDiagnosticKind::Io,
            PluginDiagnosticPhase::Discovery,
            format!(
                "plugin package content could not be read: {}",
                safe_io_error(&error)
            ),
        )
        .with_source(source)
        .with_package(label)
    })?;
    let archive = parse_stored_zip(&bytes, label, source, limits)?;
    let manifest_bytes = archive.files.get("plugin.toml").ok_or_else(|| {
        PluginDiagnostic::new(
            PluginDiagnosticKind::Missing,
            PluginDiagnosticPhase::Manifest,
            "plugin package is missing root plugin.toml",
        )
        .with_source(source)
        .with_package(label)
    })?;
    if manifest_bytes.len() > limits.max_manifest_size_bytes {
        return Err(PluginDiagnostic::new(
            PluginDiagnosticKind::Bounds,
            PluginDiagnosticPhase::Manifest,
            "plugin.toml exceeds the configured manifest size bound",
        )
        .with_source(source)
        .with_package(label));
    }
    let manifest_text = std::str::from_utf8(manifest_bytes).map_err(|_| {
        PluginDiagnostic::new(
            PluginDiagnosticKind::Malformed,
            PluginDiagnosticPhase::Manifest,
            "plugin.toml is not valid UTF-8",
        )
        .with_source(source)
        .with_package(label)
    })?;
    let manifest: PluginPackageManifest = toml::from_str(manifest_text).map_err(|error| {
        PluginDiagnostic::new(
            PluginDiagnosticKind::Malformed,
            PluginDiagnosticPhase::Manifest,
            safe_toml_parse_message(&error),
        )
        .with_source(source)
        .with_package(label)
    })?;
    validate_manifest(&manifest, &archive, label, source)?;
    let digest = deterministic_digest(&archive.files);
    let identity = SourceQualifiedPluginId::new(source, manifest.id.clone());

    Ok(DiscoveredPluginPackage {
        identity,
        package_path: path.to_path_buf(),
        package_label: label.to_string(),
        digest,
        manifest,
        entries: archive.files.keys().cloned().collect(),
    })
}

fn validate_manifest(
    manifest: &PluginPackageManifest,
    archive: &StoredArchive,
    label: &str,
    source: PluginSourceKind,
) -> Result<(), PluginDiagnostic> {
    if manifest.schema_version != SUPPORTED_PLUGIN_API_VERSION {
        return Err(PluginDiagnostic::new(
            PluginDiagnosticKind::Api,
            PluginDiagnosticPhase::Manifest,
            "plugin schema/API version is unsupported",
        )
        .with_source(source)
        .with_identity(SourceQualifiedPluginId::new(source, manifest.id.clone()))
        .with_package(label));
    }
    if !is_safe_id(&manifest.id) {
        return Err(PluginDiagnostic::new(
            PluginDiagnosticKind::Malformed,
            PluginDiagnosticPhase::Manifest,
            "plugin manifest id is not a safe local id",
        )
        .with_source(source)
        .with_package(label));
    }
    if manifest.name.trim().is_empty() || manifest.version.trim().is_empty() {
        return Err(PluginDiagnostic::new(
            PluginDiagnosticKind::Malformed,
            PluginDiagnosticPhase::Manifest,
            "plugin manifest name and version are required",
        )
        .with_source(source)
        .with_identity(SourceQualifiedPluginId::new(source, manifest.id.clone()))
        .with_package(label));
    }
    if let Some(runtime) = &manifest.runtime {
        if runtime.kind != "wasm" {
            return Err(PluginDiagnostic::new(
                PluginDiagnosticKind::Api,
                PluginDiagnosticPhase::Manifest,
                "plugin runtime kind is unsupported",
            )
            .with_source(source)
            .with_identity(SourceQualifiedPluginId::new(source, manifest.id.clone()))
            .with_package(label));
        }
        if runtime.abi.as_deref() != Some("yoi-plugin-wasm-1") {
            return Err(PluginDiagnostic::new(
                PluginDiagnosticKind::Api,
                PluginDiagnosticPhase::Manifest,
                "plugin WASM ABI is unsupported",
            )
            .with_source(source)
            .with_identity(SourceQualifiedPluginId::new(source, manifest.id.clone()))
            .with_package(label));
        }
        validate_manifest_path(&runtime.entry, archive, label, source, &manifest.id)?;
    }
    for hook in &manifest.hooks {
        if !is_safe_id(&hook.id) {
            return Err(PluginDiagnostic::new(
                PluginDiagnosticKind::Malformed,
                PluginDiagnosticPhase::Manifest,
                "plugin hook id is not safe",
            )
            .with_source(source)
            .with_identity(SourceQualifiedPluginId::new(source, manifest.id.clone()))
            .with_package(label));
        }
        validate_manifest_path(&hook.file, archive, label, source, &manifest.id)?;
    }
    Ok(())
}

fn validate_manifest_path(
    value: &str,
    archive: &StoredArchive,
    label: &str,
    source: PluginSourceKind,
    local_id: &str,
) -> Result<(), PluginDiagnostic> {
    let normalized = normalize_archive_path(value).ok_or_else(|| {
        PluginDiagnostic::new(
            PluginDiagnosticKind::Traversal,
            PluginDiagnosticPhase::Manifest,
            "plugin manifest references a path outside the package root",
        )
        .with_source(source)
        .with_identity(SourceQualifiedPluginId::new(source, local_id.to_string()))
        .with_package(label)
    })?;
    if !archive.files.contains_key(&normalized) {
        return Err(PluginDiagnostic::new(
            PluginDiagnosticKind::Missing,
            PluginDiagnosticPhase::Manifest,
            "plugin manifest references a path not present in the package",
        )
        .with_source(source)
        .with_identity(SourceQualifiedPluginId::new(source, local_id.to_string()))
        .with_package(label));
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct StoredArchive {
    files: BTreeMap<String, Vec<u8>>,
}

#[derive(Clone, Debug)]
struct CentralDirectoryEntry {
    name: String,
    compressed_size: u32,
    uncompressed_size: u32,
    local_header_offset: u32,
    compression_method: u16,
    flags: u16,
    external_attributes: u32,
}

fn parse_stored_zip(
    bytes: &[u8],
    label: &str,
    source: PluginSourceKind,
    limits: &PluginDiscoveryLimits,
) -> Result<StoredArchive, PluginDiagnostic> {
    let eocd_offset = find_eocd(bytes).ok_or_else(|| {
        malformed_zip(label, source, "zip end-of-central-directory was not found")
    })?;
    let eocd = &bytes[eocd_offset..];
    let disk_number = read_u16(eocd, 4)
        .ok_or_else(|| malformed_zip(label, source, "zip end record is truncated"))?;
    let central_disk = read_u16(eocd, 6)
        .ok_or_else(|| malformed_zip(label, source, "zip end record is truncated"))?;
    let entry_count = read_u16(eocd, 10)
        .ok_or_else(|| malformed_zip(label, source, "zip end record is truncated"))?
        as usize;
    let central_size = read_u32(eocd, 12)
        .ok_or_else(|| malformed_zip(label, source, "zip end record is truncated"))?
        as usize;
    let central_offset = read_u32(eocd, 16)
        .ok_or_else(|| malformed_zip(label, source, "zip end record is truncated"))?
        as usize;
    if disk_number != 0 || central_disk != 0 {
        return Err(malformed_zip(
            label,
            source,
            "multi-disk zip packages are unsupported",
        ));
    }
    if entry_count > limits.max_entries_per_package {
        return Err(PluginDiagnostic::new(
            PluginDiagnosticKind::Bounds,
            PluginDiagnosticPhase::Discovery,
            "plugin package contains more entries than the configured bound",
        )
        .with_source(source)
        .with_package(label));
    }
    if central_offset
        .checked_add(central_size)
        .is_none_or(|end| end > bytes.len())
    {
        return Err(malformed_zip(
            label,
            source,
            "zip central directory points outside the package",
        ));
    }

    let mut cursor = central_offset;
    let mut entries = Vec::with_capacity(entry_count);
    for _ in 0..entry_count {
        if read_u32(bytes, cursor) != Some(ZIP_CENTRAL_DIRECTORY) {
            return Err(malformed_zip(
                label,
                source,
                "zip central directory entry is malformed",
            ));
        }
        let flags = read_u16(bytes, cursor + 8).ok_or_else(|| {
            malformed_zip(label, source, "zip central directory entry is truncated")
        })?;
        let compression_method = read_u16(bytes, cursor + 10).ok_or_else(|| {
            malformed_zip(label, source, "zip central directory entry is truncated")
        })?;
        let compressed_size = read_u32(bytes, cursor + 20).ok_or_else(|| {
            malformed_zip(label, source, "zip central directory entry is truncated")
        })?;
        let uncompressed_size = read_u32(bytes, cursor + 24).ok_or_else(|| {
            malformed_zip(label, source, "zip central directory entry is truncated")
        })?;
        let name_len = read_u16(bytes, cursor + 28).ok_or_else(|| {
            malformed_zip(label, source, "zip central directory entry is truncated")
        })? as usize;
        let extra_len = read_u16(bytes, cursor + 30).ok_or_else(|| {
            malformed_zip(label, source, "zip central directory entry is truncated")
        })? as usize;
        let comment_len = read_u16(bytes, cursor + 32).ok_or_else(|| {
            malformed_zip(label, source, "zip central directory entry is truncated")
        })? as usize;
        let external_attributes = read_u32(bytes, cursor + 38).ok_or_else(|| {
            malformed_zip(label, source, "zip central directory entry is truncated")
        })?;
        let local_header_offset = read_u32(bytes, cursor + 42).ok_or_else(|| {
            malformed_zip(label, source, "zip central directory entry is truncated")
        })?;
        let name_start = cursor + 46;
        let name_end = name_start
            .checked_add(name_len)
            .ok_or_else(|| malformed_zip(label, source, "zip entry name is too large"))?;
        if name_end > bytes.len() {
            return Err(malformed_zip(
                label,
                source,
                "zip entry name points outside the package",
            ));
        }
        let raw_name = std::str::from_utf8(&bytes[name_start..name_end])
            .map_err(|_| malformed_zip(label, source, "zip entry name is not UTF-8"))?;
        let name = normalize_archive_path(raw_name).ok_or_else(|| {
            PluginDiagnostic::new(
                PluginDiagnosticKind::Traversal,
                PluginDiagnosticPhase::Discovery,
                "plugin package entry path escapes the archive root",
            )
            .with_source(source)
            .with_package(label)
        })?;
        cursor = name_end
            .checked_add(extra_len)
            .and_then(|cursor| cursor.checked_add(comment_len))
            .ok_or_else(|| {
                malformed_zip(label, source, "zip central directory entry is too large")
            })?;
        entries.push(CentralDirectoryEntry {
            name,
            compressed_size,
            uncompressed_size,
            local_header_offset,
            compression_method,
            flags,
            external_attributes,
        });
    }

    let mut files = BTreeMap::new();
    let mut expanded_size = 0u64;
    for entry in entries {
        if entry.flags & ZIP_FLAG_ENCRYPTED != 0 {
            return Err(malformed_zip(
                label,
                source,
                "encrypted zip packages are unsupported",
            ));
        }
        if is_zip_symlink(entry.external_attributes) {
            return Err(PluginDiagnostic::new(
                PluginDiagnosticKind::Traversal,
                PluginDiagnosticPhase::Discovery,
                "plugin package contains a symlink entry",
            )
            .with_source(source)
            .with_package(label));
        }
        if entry.name.ends_with('/') {
            continue;
        }
        if entry.compression_method != ZIP_COMPRESSION_STORED {
            return Err(PluginDiagnostic::new(
                PluginDiagnosticKind::Api,
                PluginDiagnosticPhase::Discovery,
                "plugin package uses an unsupported zip compression method",
            )
            .with_source(source)
            .with_package(label));
        }
        if u64::from(entry.uncompressed_size) > limits.max_file_size_bytes {
            return Err(PluginDiagnostic::new(
                PluginDiagnosticKind::Bounds,
                PluginDiagnosticPhase::Discovery,
                "plugin package entry exceeds the configured per-file bound",
            )
            .with_source(source)
            .with_package(label));
        }
        expanded_size = expanded_size.saturating_add(u64::from(entry.uncompressed_size));
        if expanded_size > limits.max_expanded_size_bytes {
            return Err(PluginDiagnostic::new(
                PluginDiagnosticKind::Bounds,
                PluginDiagnosticPhase::Discovery,
                "plugin package expanded size exceeds the configured bound",
            )
            .with_source(source)
            .with_package(label));
        }
        let data = read_stored_entry(bytes, &entry, label, source)?;
        if data.len() != entry.uncompressed_size as usize
            || data.len() != entry.compressed_size as usize
        {
            return Err(malformed_zip(
                label,
                source,
                "zip stored entry size does not match central directory",
            ));
        }
        if files.insert(entry.name.clone(), data).is_some() {
            return Err(PluginDiagnostic::new(
                PluginDiagnosticKind::Duplicate,
                PluginDiagnosticPhase::Discovery,
                "plugin package contains duplicate normalized entry paths",
            )
            .with_source(source)
            .with_package(label));
        }
    }

    Ok(StoredArchive { files })
}

fn read_stored_entry(
    bytes: &[u8],
    entry: &CentralDirectoryEntry,
    label: &str,
    source: PluginSourceKind,
) -> Result<Vec<u8>, PluginDiagnostic> {
    let cursor = entry.local_header_offset as usize;
    if read_u32(bytes, cursor) != Some(ZIP_LOCAL_FILE) {
        return Err(malformed_zip(
            label,
            source,
            "zip local file header is malformed",
        ));
    }
    let local_flags = read_u16(bytes, cursor + 6)
        .ok_or_else(|| malformed_zip(label, source, "zip local file header is truncated"))?;
    let local_method = read_u16(bytes, cursor + 8)
        .ok_or_else(|| malformed_zip(label, source, "zip local file header is truncated"))?;
    let name_len = read_u16(bytes, cursor + 26)
        .ok_or_else(|| malformed_zip(label, source, "zip local file header is truncated"))?
        as usize;
    let extra_len = read_u16(bytes, cursor + 28)
        .ok_or_else(|| malformed_zip(label, source, "zip local file header is truncated"))?
        as usize;
    if local_flags != entry.flags || local_method != entry.compression_method {
        return Err(malformed_zip(
            label,
            source,
            "zip local header disagrees with central directory",
        ));
    }
    let data_start = cursor
        .checked_add(30)
        .and_then(|cursor| cursor.checked_add(name_len))
        .and_then(|cursor| cursor.checked_add(extra_len))
        .ok_or_else(|| malformed_zip(label, source, "zip local file header is too large"))?;
    let data_end = data_start
        .checked_add(entry.compressed_size as usize)
        .ok_or_else(|| malformed_zip(label, source, "zip entry data is too large"))?;
    if data_end > bytes.len() {
        return Err(malformed_zip(
            label,
            source,
            "zip entry data points outside the package",
        ));
    }
    Ok(bytes[data_start..data_end].to_vec())
}

fn find_eocd(bytes: &[u8]) -> Option<usize> {
    let min_len = 22;
    if bytes.len() < min_len {
        return None;
    }
    let search_start = bytes.len().saturating_sub(65_557);
    (search_start..=bytes.len() - min_len)
        .rev()
        .find(|offset| read_u32(bytes, *offset) == Some(ZIP_EOCD))
}

fn malformed_zip(
    label: &str,
    source: PluginSourceKind,
    message: impl Into<String>,
) -> PluginDiagnostic {
    PluginDiagnostic::new(
        PluginDiagnosticKind::Malformed,
        PluginDiagnosticPhase::Discovery,
        message,
    )
    .with_source(source)
    .with_package(label)
}

fn deterministic_digest(files: &BTreeMap<String, Vec<u8>>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"yoi-plugin-package-digest-v1\0");
    for (path, content) in files {
        let mut file_hasher = Sha256::new();
        file_hasher.update(content);
        let file_digest = file_hasher.finalize();
        hasher.update(path.as_bytes());
        hasher.update([0]);
        hasher.update((content.len() as u64).to_be_bytes());
        hasher.update(file_digest);
    }
    format!("sha256:{}", hex_lower(&hasher.finalize()))
}

fn digest_matches(expected: &str, actual: &str) -> bool {
    if let Some(hex) = expected.strip_prefix("sha256:") {
        actual.strip_prefix("sha256:") == Some(hex)
    } else {
        false
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let bytes = bytes.get(offset..offset + 2)?;
    Some(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let bytes = bytes.get(offset..offset + 4)?;
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn is_zip_symlink(external_attributes: u32) -> bool {
    let unix_mode = external_attributes >> 16;
    unix_mode & ZIP_UNIX_FILE_TYPE_MASK == ZIP_UNIX_SYMLINK_TYPE
}

fn normalize_archive_path(raw: &str) -> Option<String> {
    if raw.is_empty() || raw.contains('\0') || raw.contains('\\') {
        return None;
    }
    if raw.starts_with('/') || raw.starts_with('~') || looks_like_windows_drive(raw) {
        return None;
    }
    let mut parts = Vec::new();
    for part in raw.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return None;
        }
        parts.push(part);
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("/"))
}

fn looks_like_windows_drive(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic()
}

fn package_label(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("<plugin-package>")
        .to_string()
}

fn safe_io_error(error: &io::Error) -> &'static str {
    match error.kind() {
        io::ErrorKind::NotFound => "not found",
        io::ErrorKind::PermissionDenied => "permission denied",
        io::ErrorKind::AlreadyExists => "already exists",
        io::ErrorKind::InvalidData => "invalid data",
        io::ErrorKind::InvalidInput => "invalid input",
        _ => "I/O error",
    }
}

fn safe_toml_parse_message(error: &toml::de::Error) -> String {
    let mut message = String::from("plugin.toml could not be parsed");
    if let Some(span) = error.span() {
        message.push_str(&format!(" near byte span {}..{}", span.start, span.end));
    }
    bounded_message(message)
}

fn bounded_message(message: String) -> String {
    const MAX: usize = 240;
    if message.len() <= MAX {
        return message;
    }
    let end = message
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= MAX)
        .last()
        .unwrap_or(0);
    format!("{}…", &message[..end])
}

fn is_safe_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        && !value.starts_with('.')
        && !value.ends_with('.')
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn discovers_valid_user_and_workspace_packages() {
        let temp = TempDir::new().unwrap();
        let workspace = temp.path().join("workspace");
        let user_data = temp.path().join("data");
        fs::create_dir_all(workspace.join(".yoi/plugins")).unwrap();
        fs::create_dir_all(user_data.join("yoi/plugins")).unwrap();
        write_plugin(
            &user_data.join("yoi/plugins/user-one.yoi-plugin"),
            "user_one",
            &[PluginSurface::Hook],
            &[("hooks/user.md", b"hello".as_slice())],
        );
        write_plugin(
            &workspace.join(".yoi/plugins/project-one.yoi-plugin"),
            "project_one",
            &[PluginSurface::Hook],
            &[("hooks/project.md", b"hello".as_slice())],
        );

        let report = discover_plugins(
            &PluginDiscoveryOptions::new(&workspace).with_user_data_home(&user_data),
        );

        assert_eq!(report.diagnostics, vec![]);
        let identities: BTreeSet<_> = report
            .packages
            .iter()
            .map(|package| package.identity.to_string())
            .collect();
        assert_eq!(
            identities,
            BTreeSet::from([
                "project:project_one".to_string(),
                "user:user_one".to_string()
            ])
        );
        assert!(
            report
                .packages
                .iter()
                .all(|package| package.digest.starts_with("sha256:"))
        );
    }

    #[test]
    fn discovery_only_does_not_activate_packages() {
        let (report, config) = fixture_with_enabled_plugin(false);

        let resolution = resolve_enabled_plugins(&config, &report);

        assert_eq!(report.packages.len(), 1);
        assert!(resolution.resolved.is_empty());
        assert!(resolution.diagnostics.is_empty());
    }

    #[test]
    fn explicit_enablement_resolves_typed_metadata() {
        let (report, config) = fixture_with_enabled_plugin(true);

        let resolution = resolve_enabled_plugins(&config, &report);

        assert_eq!(resolution.diagnostics, vec![]);
        assert_eq!(resolution.resolved.len(), 1);
        let resolved = &resolution.resolved[0];
        assert_eq!(resolved.identity.to_string(), "project:example");
        assert_eq!(resolved.enabled_surfaces, vec![PluginSurface::Hook]);
        assert!(resolved.grants.is_empty());
        assert_eq!(resolved.manifest.id, "example");
    }

    #[test]
    fn duplicate_and_unqualified_ids_fail_closed() {
        let temp = TempDir::new().unwrap();
        let workspace = temp.path().join("workspace");
        let plugins = workspace.join(".yoi/plugins");
        fs::create_dir_all(&plugins).unwrap();
        write_plugin(
            &plugins.join("one.yoi-plugin"),
            "dup",
            &[PluginSurface::Hook],
            &[("hooks/a.md", b"a")],
        );
        write_plugin(
            &plugins.join("two.yoi-plugin"),
            "dup",
            &[PluginSurface::Hook],
            &[("hooks/a.md", b"a")],
        );
        let report = discover_plugins(&PluginDiscoveryOptions::new(&workspace));

        assert!(
            report
                .diagnostics
                .iter()
                .any(|diag| diag.kind == PluginDiagnosticKind::Duplicate)
        );

        let resolution = resolve_enabled_plugins(
            &PluginConfig {
                enabled: vec![
                    PluginEnablementConfig {
                        id: "project:dup".to_string(),
                        ..PluginEnablementConfig::default()
                    },
                    PluginEnablementConfig {
                        id: "dup".to_string(),
                        ..PluginEnablementConfig::default()
                    },
                ],
                ..PluginConfig::default()
            },
            &report,
        );

        assert!(resolution.resolved.is_empty());
        assert!(
            resolution
                .diagnostics
                .iter()
                .any(|diag| diag.kind == PluginDiagnosticKind::Duplicate)
        );
        assert!(
            resolution
                .diagnostics
                .iter()
                .any(|diag| diag.kind == PluginDiagnosticKind::Ambiguous)
        );
    }

    #[test]
    fn digest_mismatch_fails_closed() {
        let (report, _) = fixture_with_enabled_plugin(false);
        let resolution = resolve_enabled_plugins(
            &PluginConfig {
                enabled: vec![PluginEnablementConfig {
                    id: "project:example".to_string(),
                    digest: Some("sha256:0000".to_string()),
                    ..PluginEnablementConfig::default()
                }],
                ..PluginConfig::default()
            },
            &report,
        );

        assert!(resolution.resolved.is_empty());
        assert_eq!(resolution.diagnostics[0].kind, PluginDiagnosticKind::Digest);
    }

    #[test]
    fn exact_version_mismatch_fails_closed_with_distinct_diagnostic() {
        let (report, _) = fixture_with_enabled_plugin(false);
        let resolution = resolve_enabled_plugins(
            &PluginConfig {
                enabled: vec![PluginEnablementConfig {
                    id: "project:example".to_string(),
                    version: Some(PluginExactVersion("9.9.9".to_string())),
                    ..PluginEnablementConfig::default()
                }],
                ..PluginConfig::default()
            },
            &report,
        );

        assert!(resolution.resolved.is_empty());
        assert_eq!(
            resolution.diagnostics[0].kind,
            PluginDiagnosticKind::Version
        );
        assert_eq!(
            resolution.diagnostics[0].phase,
            PluginDiagnosticPhase::Resolution
        );
        assert!(
            !resolution
                .diagnostics
                .iter()
                .any(|diag| diag.kind == PluginDiagnosticKind::Api)
        );
    }

    #[test]
    fn resolved_plan_pins_unpinned_enablement_for_restore() {
        let temp = TempDir::new().unwrap();
        let workspace = temp.path().join("workspace");
        let plugins = workspace.join(".yoi/plugins");
        fs::create_dir_all(&plugins).unwrap();
        let package = plugins.join("example.yoi-plugin");
        write_plugin_version(
            &package,
            "example",
            "0.1.0",
            &[PluginSurface::Hook],
            &[("hooks/example.md", b"v1")],
        );
        let options = PluginDiscoveryOptions::new(&workspace);
        let config = PluginConfig {
            enabled: vec![PluginEnablementConfig {
                id: "project:example".to_string(),
                ..PluginEnablementConfig::default()
            }],
            ..PluginConfig::default()
        };

        let startup_snapshot = resolve_plugin_config_for_startup(&config, &options);
        assert_eq!(startup_snapshot.resolved.len(), 1);
        let restored_digest = startup_snapshot.resolved[0].digest.clone();
        assert_eq!(startup_snapshot.resolved[0].version, "0.1.0");

        write_plugin_version(
            &package,
            "example",
            "0.2.0",
            &[PluginSurface::Hook],
            &[("hooks/example.md", b"v2")],
        );
        let fresh_snapshot = resolve_plugin_config_for_startup(&config, &options);
        assert_ne!(fresh_snapshot.resolved[0].digest, restored_digest);
        assert_eq!(fresh_snapshot.resolved[0].version, "0.2.0");

        let restored_snapshot = resolve_plugin_config_for_startup(&startup_snapshot, &options);
        assert_eq!(restored_snapshot.resolved[0].digest, restored_digest);
        assert_eq!(restored_snapshot.resolved[0].version, "0.1.0");
    }

    #[test]
    fn malformed_manifest_multibyte_diagnostic_is_bounded_and_redacted() {
        let temp = TempDir::new().unwrap();
        let workspace = temp.path().join("workspace");
        let plugins = workspace.join(".yoi/plugins");
        fs::create_dir_all(&plugins).unwrap();
        let malformed = format!("schema_version = [\n# {}", "機密".repeat(200));
        write_stored_zip(
            &plugins.join("bad-multibyte.yoi-plugin"),
            &[("plugin.toml", malformed.into_bytes(), 0)],
        );

        let report = discover_plugins(&PluginDiscoveryOptions::new(&workspace));

        assert!(report.packages.is_empty());
        let diagnostic = report
            .diagnostics
            .iter()
            .find(|diag| diag.kind == PluginDiagnosticKind::Malformed)
            .unwrap();
        assert!(diagnostic.message.len() <= 241);
        assert!(!diagnostic.message.contains("機密"));
    }

    #[test]
    fn traversal_root_escape_in_archive_fails_closed() {
        let temp = TempDir::new().unwrap();
        let workspace = temp.path().join("workspace");
        let plugins = workspace.join(".yoi/plugins");
        fs::create_dir_all(&plugins).unwrap();
        write_stored_zip(
            &plugins.join("escape.yoi-plugin"),
            &[
                (
                    "plugin.toml",
                    manifest("escape", "0.1.0", &[PluginSurface::Hook]).into_bytes(),
                    0,
                ),
                ("../evil", b"x".to_vec(), 0),
            ],
        );

        let report = discover_plugins(&PluginDiscoveryOptions::new(&workspace));

        assert!(report.packages.is_empty());
        assert_eq!(report.diagnostics[0].kind, PluginDiagnosticKind::Traversal);
    }

    #[cfg(unix)]
    #[test]
    fn package_symlink_store_escape_fails_closed() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let workspace = temp.path().join("workspace");
        let plugins = workspace.join(".yoi/plugins");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&plugins).unwrap();
        write_plugin(
            &outside,
            "outside",
            &[PluginSurface::Hook],
            &[("hooks/a.md", b"a")],
        );
        symlink(&outside, plugins.join("outside.yoi-plugin")).unwrap();

        let report = discover_plugins(&PluginDiscoveryOptions::new(&workspace));

        assert!(report.packages.is_empty());
        assert_eq!(report.diagnostics[0].kind, PluginDiagnosticKind::Traversal);
    }

    #[test]
    fn unsupported_api_and_malformed_manifest_fail_closed() {
        let temp = TempDir::new().unwrap();
        let workspace = temp.path().join("workspace");
        let plugins = workspace.join(".yoi/plugins");
        fs::create_dir_all(&plugins).unwrap();
        write_stored_zip(
            &plugins.join("bad-schema.yoi-plugin"),
            &[(
                "plugin.toml",
                manifest_with_schema("bad_schema", "0.1.0", 999).into_bytes(),
                0,
            )],
        );
        write_stored_zip(
            &plugins.join("bad-toml.yoi-plugin"),
            &[("plugin.toml", b"not = [valid".to_vec(), 0)],
        );

        let report = discover_plugins(&PluginDiscoveryOptions::new(&workspace));

        assert!(report.packages.is_empty());
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diag| diag.kind == PluginDiagnosticKind::Api)
        );
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diag| diag.kind == PluginDiagnosticKind::Malformed)
        );
    }

    #[test]
    fn surface_and_grant_failures_do_not_resolve() {
        let (report, _) = fixture_with_enabled_plugin(false);
        let resolution = resolve_enabled_plugins(
            &PluginConfig {
                enabled: vec![
                    PluginEnablementConfig {
                        id: "project:example".to_string(),
                        surfaces: vec![PluginSurface::Tool],
                        ..PluginEnablementConfig::default()
                    },
                    PluginEnablementConfig {
                        id: "project:example".to_string(),
                        grants: PluginGrantConfig {
                            filesystem: vec![".".to_string()],
                            ..PluginGrantConfig::default()
                        },
                        ..PluginEnablementConfig::default()
                    },
                ],
                ..PluginConfig::default()
            },
            &report,
        );

        assert!(resolution.resolved.is_empty());
        assert!(
            resolution
                .diagnostics
                .iter()
                .any(|diag| diag.kind == PluginDiagnosticKind::Surface)
        );
        assert!(
            resolution
                .diagnostics
                .iter()
                .any(|diag| diag.kind == PluginDiagnosticKind::Grant)
        );
    }

    fn fixture_with_enabled_plugin(enabled: bool) -> (PluginDiscoveryReport, PluginConfig) {
        let temp = TempDir::new().unwrap();
        let workspace = temp.path().join("workspace");
        let plugins = workspace.join(".yoi/plugins");
        fs::create_dir_all(&plugins).unwrap();
        write_plugin(
            &plugins.join("example.yoi-plugin"),
            "example",
            &[PluginSurface::Hook],
            &[("hooks/example.md", b"hello")],
        );
        let report = discover_plugins(&PluginDiscoveryOptions::new(&workspace));
        let config = PluginConfig {
            enabled: if enabled {
                vec![PluginEnablementConfig {
                    id: "project:example".to_string(),
                    ..PluginEnablementConfig::default()
                }]
            } else {
                vec![]
            },
            ..PluginConfig::default()
        };
        (report, config)
    }

    fn write_plugin(
        path: &Path,
        id: &str,
        surfaces: &[PluginSurface],
        extra_files: &[(&str, &[u8])],
    ) {
        write_plugin_version(path, id, "0.1.0", surfaces, extra_files);
    }

    fn write_plugin_version(
        path: &Path,
        id: &str,
        version: &str,
        surfaces: &[PluginSurface],
        extra_files: &[(&str, &[u8])],
    ) {
        let mut entries = vec![(
            "plugin.toml",
            manifest(id, version, surfaces).into_bytes(),
            0,
        )];
        if surfaces.contains(&PluginSurface::Hook)
            && !extra_files
                .iter()
                .any(|(path, _)| *path == "hooks/example.md")
        {
            entries.push(("hooks/example.md", b"hook".to_vec(), 0));
        }
        entries.extend(
            extra_files
                .iter()
                .map(|(path, content)| (*path, content.to_vec(), 0)),
        );
        write_stored_zip(path, &entries);
    }

    fn manifest(id: &str, version: &str, surfaces: &[PluginSurface]) -> String {
        let mut manifest = manifest_with_schema(id, version, SUPPORTED_PLUGIN_API_VERSION);
        if surfaces.contains(&PluginSurface::Hook) {
            manifest.push_str("\n[[hooks]]\nid = \"startup\"\nfile = \"hooks/example.md\"\n");
        }
        manifest
    }

    fn manifest_with_schema(id: &str, version: &str, schema_version: u32) -> String {
        format!(
            "schema_version = {schema_version}\nid = \"{id}\"\nname = \"Example\"\nversion = \"{version}\"\n"
        )
    }

    fn write_stored_zip(path: &Path, entries: &[(&str, Vec<u8>, u32)]) {
        let mut bytes = Vec::new();
        let mut central = Vec::new();
        for (name, content, external_attributes) in entries {
            let local_offset = bytes.len() as u32;
            write_u32(&mut bytes, ZIP_LOCAL_FILE);
            write_u16(&mut bytes, 20);
            write_u16(&mut bytes, 0x0800);
            write_u16(&mut bytes, ZIP_COMPRESSION_STORED);
            write_u16(&mut bytes, 0);
            write_u16(&mut bytes, 0);
            write_u32(&mut bytes, 0);
            write_u32(&mut bytes, content.len() as u32);
            write_u32(&mut bytes, content.len() as u32);
            write_u16(&mut bytes, name.len() as u16);
            write_u16(&mut bytes, 0);
            bytes.extend_from_slice(name.as_bytes());
            bytes.extend_from_slice(content);

            write_u32(&mut central, ZIP_CENTRAL_DIRECTORY);
            write_u16(&mut central, 20);
            write_u16(&mut central, 20);
            write_u16(&mut central, 0x0800);
            write_u16(&mut central, ZIP_COMPRESSION_STORED);
            write_u16(&mut central, 0);
            write_u16(&mut central, 0);
            write_u32(&mut central, 0);
            write_u32(&mut central, content.len() as u32);
            write_u32(&mut central, content.len() as u32);
            write_u16(&mut central, name.len() as u16);
            write_u16(&mut central, 0);
            write_u16(&mut central, 0);
            write_u16(&mut central, 0);
            write_u16(&mut central, 0);
            write_u32(&mut central, *external_attributes);
            write_u32(&mut central, local_offset);
            central.extend_from_slice(name.as_bytes());
        }
        let central_offset = bytes.len() as u32;
        bytes.extend_from_slice(&central);
        write_u32(&mut bytes, ZIP_EOCD);
        write_u16(&mut bytes, 0);
        write_u16(&mut bytes, 0);
        write_u16(&mut bytes, entries.len() as u16);
        write_u16(&mut bytes, entries.len() as u16);
        write_u32(&mut bytes, central.len() as u32);
        write_u32(&mut bytes, central_offset);
        write_u16(&mut bytes, 0);
        fs::write(path, bytes).unwrap();
    }

    fn write_u16(out: &mut Vec<u8>, value: u16) {
        out.extend_from_slice(&value.to_le_bytes());
    }

    fn write_u32(out: &mut Vec<u8>, value: u32) {
        out.extend_from_slice(&value.to_le_bytes());
    }
}
