//! Offline reading, validation, and writing of the `.yoi-plugin` format.
//!
//! Inspects only explicitly supplied directories or archives; does not implicitly
//! discover catalogs, register with Workers, or instantiate components. Validating
//! the format grants no execution authority. Worker Manifests/Profiles reject dynamic
//! Plugin configuration; package declarations and local paths do not grant Worker
//! capabilities.
//!
//! Archives use a restricted stored-ZIP subset. Unsafe paths, links, unsupported
//! entries, size/count limit violations, and inconsistent headers are rejected;
//! the file set is digested in order. Canonical ZIP32 output stores files in
//! path order with fixed metadata, CRC32, no compression, extras or comments.
//! Directory inputs also reject internal links and undergo the same validation,
//! so distribution formats share the package
//! contract. Component Model metadata is required; legacy core-Wasm runtime
//! declarations are rejected. Tool schemas use the offline 2020-12 subset in
//! [`schema`]; non-secret config uses closed static DCDL syntax, never JSON Schema.
//! Validation covers the offline format, not safety
//! established by running components.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

pub const SUPPORTED_PLUGIN_API_VERSION: u32 = 2;

pub mod component;
pub mod schema;
const ZIP_EOCD: u32 = 0x0605_4b50;
const ZIP_CENTRAL_DIRECTORY: u32 = 0x0201_4b50;
const ZIP_LOCAL_FILE: u32 = 0x0403_4b50;

const ZIP_COMPRESSION_STORED: u16 = 0;
const ZIP_UNIX_SYMLINK_TYPE: u32 = 0o120000;
const ZIP_UNIX_FILE_TYPE_MASK: u32 = 0o170000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PluginTemplateResource {
    pub path: &'static str,
    pub contents: &'static str,
}

/// Embedded starter template for Rust Component Model Tool Plugins.
///
/// The template is data only: it performs no filesystem/network operations and
/// grants no authority. Authoring commands materialize these files only into an
/// explicitly chosen empty destination.
pub const RUST_COMPONENT_TOOL_TEMPLATE: &[PluginTemplateResource] = &[
    PluginTemplateResource {
        path: "wit/yoi-plugin-v2.wit",
        contents: include_str!("../../../resources/plugin/wit/yoi-plugin-v2.wit"),
    },
    PluginTemplateResource {
        path: "wit/deps/yoi-host/yoi-host-v2.wit",
        contents: include_str!("../../../resources/plugin/wit/deps/yoi-host/yoi-host-v2.wit"),
    },
    PluginTemplateResource {
        path: "Cargo.toml",
        contents: include_str!(
            "../../../resources/plugin/templates/rust-component-tool/Cargo.toml"
        ),
    },
    PluginTemplateResource {
        path: "src/lib.rs",
        contents: include_str!(
            "../../../resources/plugin/templates/rust-component-tool/src/lib.rs"
        ),
    },
    PluginTemplateResource {
        path: "plugin.toml",
        contents: include_str!(
            "../../../resources/plugin/templates/rust-component-tool/plugin.toml"
        ),
    },
    PluginTemplateResource {
        path: "README.md",
        contents: include_str!("../../../resources/plugin/templates/rust-component-tool/README.md"),
    },
];

/// HTTPS authoring imports typed resources; it does not implement Host networking.
pub const RUST_COMPONENT_HTTPS_TOOL_TEMPLATE: &[PluginTemplateResource] = &[
    PluginTemplateResource {
        path: "wit/yoi-plugin-v2.wit",
        contents: include_str!("../../../resources/plugin/wit/yoi-plugin-v2.wit"),
    },
    PluginTemplateResource {
        path: "wit/deps/yoi-host/yoi-host-v2.wit",
        contents: include_str!("../../../resources/plugin/wit/deps/yoi-host/yoi-host-v2.wit"),
    },
    PluginTemplateResource {
        path: "Cargo.toml",
        contents: include_str!(
            "../../../resources/plugin/templates/rust-component-https/Cargo.toml"
        ),
    },
    PluginTemplateResource {
        path: "src/lib.rs",
        contents: include_str!(
            "../../../resources/plugin/templates/rust-component-https/src/lib.rs"
        ),
    },
    PluginTemplateResource {
        path: "plugin.toml",
        contents: include_str!(
            "../../../resources/plugin/templates/rust-component-https/plugin.toml"
        ),
    },
    PluginTemplateResource {
        path: "README.md",
        contents: include_str!(
            "../../../resources/plugin/templates/rust-component-https/README.md"
        ),
    },
];

/// The only supported v2 surface; future surfaces are not success stubs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginSurface {
    Tool,
}

impl fmt::Display for PluginSurface {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("tool")
    }
}

/// Capability requests are metadata, never effective grants.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PluginRequestedGrant {
    Logging,
    Https { slot: String },
}

/// A slot requests HTTPS protocol bounds, not credentials or concrete bindings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginConnectionSlot {
    pub name: String,
    pub methods: Vec<PluginHttpMethod>,
    pub path_prefixes: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum PluginHttpMethod {
    Get,
    Head,
    Post,
    Put,
    Patch,
    Delete,
    Options,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginSourceKind {
    Explicit,
}

impl PluginSourceKind {
    fn qualifier(self) -> &'static str {
        "explicit"
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
            "explicit" => PluginSourceKind::Explicit,
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginPackageManifest {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub surfaces: Vec<PluginSurface>,
    pub runtime: PluginRuntimeManifest,
    pub tools: Vec<PluginToolManifest>,
    /// Closed static DCDL type expression, not JSON Schema or guest code.
    pub config_schema: String,
    #[serde(default)]
    pub connections: Vec<PluginConnectionSlot>,
    #[serde(default)]
    pub requested_grants: Vec<PluginRequestedGrant>,
}

impl PluginPackageManifest {
    pub fn declared_surfaces(&self) -> BTreeSet<PluginSurface> {
        self.surfaces.iter().copied().collect()
    }
}

pub const PLUGIN_RUNTIME_COMPONENT_KIND: &str = "wasm-component";
pub const PLUGIN_COMPONENT_TOOL_WORLD: &str = "yoi:plugin/tool-plugin@2.0.0";
pub const PLUGIN_COMPONENT_HTTPS_TOOL_WORLD: &str = "yoi:plugin/https-tool-plugin@2.0.0";
pub const PLUGIN_COMPONENT_LOGGED_TOOL_WORLD: &str = "yoi:plugin/logged-tool-plugin@2.0.0";
pub const PLUGIN_COMPONENT_HTTPS_LOGGED_TOOL_WORLD: &str =
    "yoi:plugin/https-logged-tool-plugin@2.0.0";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginRuntimeManifest {
    pub kind: String,
    pub component: String,
    pub world: String,
    /// Exact imports, including type interfaces, without semver ranges.
    pub required_imports: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginToolManifest {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub output_schema: serde_json::Value,
    #[serde(default)]
    pub error_codes: Vec<String>,
    #[serde(default)]
    pub external_write: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginPackageLimits {
    pub max_package_size_bytes: u64,
    pub max_manifest_size_bytes: usize,
    pub max_entries_per_package: usize,
    pub max_file_size_bytes: u64,
    pub max_expanded_size_bytes: u64,
}

impl Default for PluginPackageLimits {
    fn default() -> Self {
        Self {
            max_package_size_bytes: 16 * 1024 * 1024,
            max_manifest_size_bytes: 64 * 1024,
            max_entries_per_package: 512,
            max_file_size_bytes: 8 * 1024 * 1024,
            max_expanded_size_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct InspectedPluginPackage {
    pub identity: SourceQualifiedPluginId,
    pub input_path: PathBuf,
    pub package_label: String,
    pub package_digest: String,
    pub component: component::ComponentMetadata,
    pub manifest: PluginPackageManifest,
    pub entries: BTreeSet<String>,
}

/// Fully materialized package content used by local authoring checks and pack.
///
/// This is data-only metadata and bytes. Constructing it parses manifests and
/// validates package/archive shape, but it does not load, instantiate, or
/// execute Plugin code.
#[derive(Clone, Debug, PartialEq)]
pub struct MaterializedPluginPackage {
    pub package: InspectedPluginPackage,
    pub files: BTreeMap<String, Vec<u8>>,
    /// Digest and size of canonical archive bytes, not the upload ZIP.
    pub artifact_digest: String,
    pub artifact_size: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PackedPluginPackage {
    pub output_path: PathBuf,
    pub package: InspectedPluginPackage,
    pub artifact_digest: String,
    pub artifact_size: u64,
}

/// Immutable candidate bytes to publish once; fetch must return these exact bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct CanonicalPluginArtifact {
    pub bytes: Vec<u8>,
    pub package_digest: String,
    pub artifact_digest: String,
    pub artifact_size: u64,
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
    Inspection,
    Manifest,
}

pub fn read_plugin_package_file(
    path: &Path,
    source: PluginSourceKind,
    limits: &PluginPackageLimits,
) -> Result<MaterializedPluginPackage, PluginDiagnostic> {
    let label = package_label(path);
    let metadata = fs::metadata(path).map_err(|error| {
        PluginDiagnostic::new(
            PluginDiagnosticKind::Io,
            PluginDiagnosticPhase::Inspection,
            format!(
                "plugin package metadata could not be read: {}",
                safe_io_error(&error)
            ),
        )
        .with_source(source)
        .with_package(label.clone())
    })?;
    if !metadata.is_file() {
        return Err(PluginDiagnostic::new(
            PluginDiagnosticKind::Malformed,
            PluginDiagnosticPhase::Inspection,
            "plugin package candidate is not a regular file",
        )
        .with_source(source)
        .with_package(label));
    }
    if metadata.len() > limits.max_package_size_bytes {
        return Err(PluginDiagnostic::new(
            PluginDiagnosticKind::Bounds,
            PluginDiagnosticPhase::Inspection,
            "plugin package exceeds the configured package size bound",
        )
        .with_source(source)
        .with_package(label));
    }
    let bytes = read_bounded_file(path, limits.max_package_size_bytes).map_err(|error| {
        PluginDiagnostic::new(
            PluginDiagnosticKind::Io,
            PluginDiagnosticPhase::Inspection,
            format!(
                "plugin package content could not be read: {}",
                safe_io_error(&error)
            ),
        )
        .with_source(source)
        .with_package(label.clone())
    })?;
    materialize_archive(path, &label, source, &bytes, limits)
}

pub fn read_plugin_directory(
    path: &Path,
    source: PluginSourceKind,
    limits: &PluginPackageLimits,
) -> Result<MaterializedPluginPackage, PluginDiagnostic> {
    let label = package_label(path);
    let root = fs::canonicalize(path).map_err(|error| {
        PluginDiagnostic::new(
            PluginDiagnosticKind::Io,
            PluginDiagnosticPhase::Inspection,
            format!(
                "plugin directory could not be read: {}",
                safe_io_error(&error)
            ),
        )
        .with_source(source)
        .with_package(label.clone())
    })?;
    let metadata = fs::metadata(&root).map_err(|error| {
        PluginDiagnostic::new(
            PluginDiagnosticKind::Io,
            PluginDiagnosticPhase::Inspection,
            format!(
                "plugin directory metadata could not be read: {}",
                safe_io_error(&error)
            ),
        )
        .with_source(source)
        .with_package(label.clone())
    })?;
    if !metadata.is_dir() {
        return Err(PluginDiagnostic::new(
            PluginDiagnosticKind::Malformed,
            PluginDiagnosticPhase::Inspection,
            "plugin directory input is not a directory",
        )
        .with_source(source)
        .with_package(label));
    }

    let mut files = BTreeMap::new();
    collect_directory_files(&root, &root, &label, source, limits, &mut files)?;
    materialize_files(path, label, source, files, limits)
}

pub fn write_plugin_package_file(
    materialized: &MaterializedPluginPackage,
    output_path: &Path,
    limits: &PluginPackageLimits,
) -> Result<PackedPluginPackage, PluginDiagnostic> {
    let artifact = pack_plugin_package(materialized, limits)?;
    if let Some(parent) = output_path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|_| {
            malformed_zip(
                "output",
                materialized.package.source(),
                "cannot create output directory",
            )
        })?;
    }
    fs::write(output_path, &artifact.bytes).map_err(|_| {
        malformed_zip(
            "output",
            materialized.package.source(),
            "cannot write package",
        )
    })?;
    let package = read_plugin_package_file(output_path, materialized.package.source(), limits)?;
    Ok(PackedPluginPackage {
        output_path: output_path.to_path_buf(),
        package: package.package,
        artifact_digest: artifact.artifact_digest,
        artifact_size: artifact.artifact_size,
    })
}

/// Shared Server installer/authoring canonicalization. Revalidates mutable file data.
pub fn pack_plugin_package(
    materialized: &MaterializedPluginPackage,
    limits: &PluginPackageLimits,
) -> Result<CanonicalPluginArtifact, PluginDiagnostic> {
    let bytes = canonical_archive_bytes(&materialized.files, limits)?;
    let checked = read_plugin_package_bytes(&bytes, materialized.package.source(), limits)?;
    Ok(CanonicalPluginArtifact {
        package_digest: checked.package.package_digest,
        artifact_size: bytes.len() as u64,
        artifact_digest: artifact_digest(&bytes),
        bytes,
    })
}

/// Offline upload inspection; accepts bounded stored ZIP and returns canonical metadata.
pub fn read_plugin_package_bytes(
    bytes: &[u8],
    source: PluginSourceKind,
    limits: &PluginPackageLimits,
) -> Result<MaterializedPluginPackage, PluginDiagnostic> {
    materialize_archive(
        Path::new("explicit.yoi-plugin"),
        "explicit.yoi-plugin",
        source,
        bytes,
        limits,
    )
}

/// Runtime/cache verification of exact canonical bytes and both independent digests.
/// This function confers no access, admission or execution authority.
pub fn verify_plugin_artifact(
    bytes: &[u8],
    expected_package_digest: &str,
    expected_artifact_digest: &str,
    expected_size: u64,
    limits: &PluginPackageLimits,
) -> Result<MaterializedPluginPackage, PluginDiagnostic> {
    if bytes.len() as u64 > limits.max_package_size_bytes {
        return Err(PluginDiagnostic::new(
            PluginDiagnosticKind::Bounds,
            PluginDiagnosticPhase::Inspection,
            "artifact exceeds size limit",
        ));
    }
    if expected_size != bytes.len() as u64 || artifact_digest(bytes) != expected_artifact_digest {
        return Err(PluginDiagnostic::new(
            PluginDiagnosticKind::Digest,
            PluginDiagnosticPhase::Inspection,
            "artifact digest/size mismatch",
        ));
    }
    let package = read_plugin_package_bytes(bytes, PluginSourceKind::Explicit, limits)?;
    if package.package.package_digest != expected_package_digest
        || canonical_archive_bytes(&package.files, limits)? != bytes
    {
        return Err(PluginDiagnostic::new(
            PluginDiagnosticKind::Digest,
            PluginDiagnosticPhase::Inspection,
            "package digest mismatch or noncanonical artifact",
        ));
    }
    Ok(package)
}

fn artifact_digest(bytes: &[u8]) -> String {
    format!("sha256:{}", hex_lower(&Sha256::digest(bytes)))
}

impl InspectedPluginPackage {
    pub fn source(&self) -> PluginSourceKind {
        self.identity.source
    }
}

fn materialize_archive(
    path: &Path,
    label: &str,
    source: PluginSourceKind,
    bytes: &[u8],
    limits: &PluginPackageLimits,
) -> Result<MaterializedPluginPackage, PluginDiagnostic> {
    if bytes.len() as u64 > limits.max_package_size_bytes {
        return Err(PluginDiagnostic::new(
            PluginDiagnosticKind::Bounds,
            PluginDiagnosticPhase::Inspection,
            "archive exceeds size limit",
        ));
    }
    let archive = parse_stored_zip(bytes, label, source, limits)?;
    materialize_files(path, label.to_string(), source, archive.files, limits)
}

fn materialize_files(
    path: &Path,
    label: String,
    source: PluginSourceKind,
    files: BTreeMap<String, Vec<u8>>,
    limits: &PluginPackageLimits,
) -> Result<MaterializedPluginPackage, PluginDiagnostic> {
    let archive = StoredArchive {
        files: files.clone(),
    };
    let manifest_bytes = archive.files.get("plugin.toml").ok_or_else(|| {
        PluginDiagnostic::new(
            PluginDiagnosticKind::Missing,
            PluginDiagnosticPhase::Manifest,
            "plugin package is missing root plugin.toml",
        )
        .with_source(source)
        .with_package(label.clone())
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
        .with_package(label.clone())
    })?;
    let manifest: PluginPackageManifest = toml::from_str(manifest_text).map_err(|error| {
        PluginDiagnostic::new(
            PluginDiagnosticKind::Malformed,
            PluginDiagnosticPhase::Manifest,
            safe_toml_parse_message(&error),
        )
        .with_source(source)
        .with_package(label.clone())
    })?;
    let component = validate_manifest(&manifest, &archive, &label, source)?;
    let digest = deterministic_digest(&archive.files);
    let identity = SourceQualifiedPluginId::new(source, manifest.id.clone());
    let package = InspectedPluginPackage {
        identity,
        input_path: path.to_path_buf(),
        package_label: label,
        package_digest: digest,
        component,
        manifest,
        entries: archive.files.keys().cloned().collect(),
    };
    let canonical = canonical_archive_bytes(&files, limits)?;
    Ok(MaterializedPluginPackage {
        package,
        files,
        artifact_digest: artifact_digest(&canonical),
        artifact_size: canonical.len() as u64,
    })
}

fn collect_directory_files(
    root: &Path,
    dir: &Path,
    label: &str,
    source: PluginSourceKind,
    limits: &PluginPackageLimits,
    files: &mut BTreeMap<String, Vec<u8>>,
) -> Result<(), PluginDiagnostic> {
    let mut entries = fs::read_dir(dir)
        .map_err(|error| {
            PluginDiagnostic::new(
                PluginDiagnosticKind::Io,
                PluginDiagnosticPhase::Inspection,
                format!(
                    "plugin directory could not be listed: {}",
                    safe_io_error(&error)
                ),
            )
            .with_source(source)
            .with_package(label.to_string())
        })?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| malformed_zip(label, source, "cannot read directory entry"))?;
    entries.sort();

    for path in entries {
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            PluginDiagnostic::new(
                PluginDiagnosticKind::Io,
                PluginDiagnosticPhase::Inspection,
                format!(
                    "plugin directory entry metadata could not be read: {}",
                    safe_io_error(&error)
                ),
            )
            .with_source(source)
            .with_package(label.to_string())
        })?;
        if metadata.file_type().is_symlink() {
            return Err(PluginDiagnostic::new(
                PluginDiagnosticKind::Traversal,
                PluginDiagnosticPhase::Inspection,
                "plugin directory contains a symlink entry",
            )
            .with_source(source)
            .with_package(label.to_string()));
        }
        if metadata.is_dir() {
            collect_directory_files(root, &path, label, source, limits, files)?;
            continue;
        }
        if !metadata.is_file() {
            continue;
        }
        if metadata.len() > limits.max_file_size_bytes {
            return Err(PluginDiagnostic::new(
                PluginDiagnosticKind::Bounds,
                PluginDiagnosticPhase::Inspection,
                "plugin directory file exceeds the configured per-file bound",
            )
            .with_source(source)
            .with_package(label.to_string()));
        }
        let canonical = fs::canonicalize(&path).map_err(|error| {
            PluginDiagnostic::new(
                PluginDiagnosticKind::Io,
                PluginDiagnosticPhase::Inspection,
                format!(
                    "plugin directory file could not be read: {}",
                    safe_io_error(&error)
                ),
            )
            .with_source(source)
            .with_package(label.to_string())
        })?;
        if !canonical.starts_with(root) {
            return Err(PluginDiagnostic::new(
                PluginDiagnosticKind::Traversal,
                PluginDiagnosticPhase::Inspection,
                "plugin directory file escapes the package root",
            )
            .with_source(source)
            .with_package(label.to_string()));
        }
        let relative = canonical.strip_prefix(root).map_err(|_| {
            PluginDiagnostic::new(
                PluginDiagnosticKind::Traversal,
                PluginDiagnosticPhase::Inspection,
                "plugin directory file escapes the package root",
            )
            .with_source(source)
            .with_package(label.to_string())
        })?;
        let normalized = relative
            .components()
            .map(|component| component.as_os_str().to_str())
            .collect::<Option<Vec<_>>>()
            .and_then(|parts| normalize_archive_path(&parts.join("/")))
            .ok_or_else(|| {
                PluginDiagnostic::new(
                    PluginDiagnosticKind::Traversal,
                    PluginDiagnosticPhase::Inspection,
                    "plugin directory contains an unsafe relative path",
                )
                .with_source(source)
                .with_package(label.to_string())
            })?;
        let content = read_bounded_file(&path, limits.max_file_size_bytes).map_err(|error| {
            PluginDiagnostic::new(
                PluginDiagnosticKind::Io,
                PluginDiagnosticPhase::Inspection,
                format!(
                    "plugin directory file could not be read: {}",
                    safe_io_error(&error)
                ),
            )
            .with_source(source)
            .with_package(label.to_string())
        })?;
        files.insert(normalized, content);
        if files.len() > limits.max_entries_per_package {
            return Err(PluginDiagnostic::new(
                PluginDiagnosticKind::Bounds,
                PluginDiagnosticPhase::Inspection,
                "plugin directory contains more files than the configured bound",
            )
            .with_source(source)
            .with_package(label.to_string()));
        }
        let expanded_size = files
            .values()
            .map(|content| content.len() as u64)
            .sum::<u64>();
        if expanded_size > limits.max_expanded_size_bytes {
            return Err(PluginDiagnostic::new(
                PluginDiagnosticKind::Bounds,
                PluginDiagnosticPhase::Inspection,
                "plugin directory expanded size exceeds the configured bound",
            )
            .with_source(source)
            .with_package(label.to_string()));
        }
    }
    Ok(())
}

fn canonical_archive_bytes(
    files: &BTreeMap<String, Vec<u8>>,
    limits: &PluginPackageLimits,
) -> Result<Vec<u8>, PluginDiagnostic> {
    if files.len() > limits.max_entries_per_package {
        return Err(PluginDiagnostic::new(
            PluginDiagnosticKind::Bounds,
            PluginDiagnosticPhase::Inspection,
            "plugin package contains more entries than the configured bound",
        ));
    }
    let expanded = files
        .values()
        .try_fold(0u64, |sum, v| sum.checked_add(v.len() as u64));
    if files.len() > u16::MAX as usize
        || expanded
            .is_none_or(|n| n > limits.max_expanded_size_bytes || n > limits.max_package_size_bytes)
    {
        return Err(PluginDiagnostic::new(
            PluginDiagnosticKind::Bounds,
            PluginDiagnosticPhase::Inspection,
            "archive exceeds count/expanded bounds",
        ));
    }
    let archive_size = files.iter().try_fold(22u64, |total, (name, content)| {
        total
            .checked_add(76)?
            .checked_add((name.len() as u64).checked_mul(2)?)?
            .checked_add(content.len() as u64)
    });
    if archive_size.is_none_or(|n| n > limits.max_package_size_bytes || n > u32::MAX as u64) {
        return Err(PluginDiagnostic::new(
            PluginDiagnosticKind::Bounds,
            PluginDiagnosticPhase::Inspection,
            "archive exceeds ZIP32/package bounds",
        ));
    }
    let mut bytes = Vec::new();
    let mut central = Vec::new();
    for (name, content) in files {
        let name = normalize_archive_path(name).ok_or_else(|| {
            PluginDiagnostic::new(
                PluginDiagnosticKind::Traversal,
                PluginDiagnosticPhase::Inspection,
                "plugin package entry path escapes the archive root",
            )
        })?;
        if content.len() as u64 > limits.max_file_size_bytes {
            return Err(PluginDiagnostic::new(
                PluginDiagnosticKind::Bounds,
                PluginDiagnosticPhase::Inspection,
                "plugin package entry exceeds the configured per-file bound",
            ));
        }
        if name.len() > u16::MAX as usize
            || content.len() > u32::MAX as usize
            || bytes.len() > u32::MAX as usize
        {
            return Err(PluginDiagnostic::new(
                PluginDiagnosticKind::Bounds,
                PluginDiagnosticPhase::Inspection,
                "ZIP32 bounds exceeded",
            ));
        }
        let crc = crc32fast::hash(content);
        let local_offset = bytes.len() as u32;
        write_u32_vec(&mut bytes, ZIP_LOCAL_FILE);
        write_u16_vec(&mut bytes, 20);
        write_u16_vec(&mut bytes, 0x0800);
        write_u16_vec(&mut bytes, ZIP_COMPRESSION_STORED);
        write_u16_vec(&mut bytes, 0);
        write_u16_vec(&mut bytes, 0);
        write_u32_vec(&mut bytes, crc);
        write_u32_vec(&mut bytes, content.len() as u32);
        write_u32_vec(&mut bytes, content.len() as u32);
        write_u16_vec(&mut bytes, name.len() as u16);
        write_u16_vec(&mut bytes, 0);
        bytes.extend_from_slice(name.as_bytes());
        bytes.extend_from_slice(content);

        write_u32_vec(&mut central, ZIP_CENTRAL_DIRECTORY);
        write_u16_vec(&mut central, 20);
        write_u16_vec(&mut central, 20);
        write_u16_vec(&mut central, 0x0800);
        write_u16_vec(&mut central, ZIP_COMPRESSION_STORED);
        write_u16_vec(&mut central, 0);
        write_u16_vec(&mut central, 0);
        write_u32_vec(&mut central, crc);
        write_u32_vec(&mut central, content.len() as u32);
        write_u32_vec(&mut central, content.len() as u32);
        write_u16_vec(&mut central, name.len() as u16);
        write_u16_vec(&mut central, 0);
        write_u16_vec(&mut central, 0);
        write_u16_vec(&mut central, 0);
        write_u16_vec(&mut central, 0);
        write_u32_vec(&mut central, 0);
        write_u32_vec(&mut central, local_offset);
        central.extend_from_slice(name.as_bytes());
    }
    let central_offset = bytes.len() as u32;
    bytes.extend_from_slice(&central);
    write_u32_vec(&mut bytes, ZIP_EOCD);
    write_u16_vec(&mut bytes, 0);
    write_u16_vec(&mut bytes, 0);
    write_u16_vec(&mut bytes, files.len() as u16);
    write_u16_vec(&mut bytes, files.len() as u16);
    write_u32_vec(&mut bytes, central.len() as u32);
    write_u32_vec(&mut bytes, central_offset);
    write_u16_vec(&mut bytes, 0);
    if bytes.len() as u64 > limits.max_package_size_bytes {
        return Err(PluginDiagnostic::new(
            PluginDiagnosticKind::Bounds,
            PluginDiagnosticPhase::Inspection,
            "plugin package exceeds the configured package size bound",
        ));
    }
    Ok(bytes)
}

fn write_u16_vec(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn write_u32_vec(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn validate_manifest(
    manifest: &PluginPackageManifest,
    archive: &StoredArchive,
    label: &str,
    source: PluginSourceKind,
) -> Result<component::ComponentMetadata, PluginDiagnostic> {
    let reject = |kind, message: String| {
        PluginDiagnostic::new(kind, PluginDiagnosticPhase::Manifest, message)
            .with_source(source)
            .with_package(label)
            .with_identity(SourceQualifiedPluginId::new(source, &manifest.id))
    };
    if manifest.schema_version != SUPPORTED_PLUGIN_API_VERSION {
        return Err(reject(
            PluginDiagnosticKind::Api,
            "unsupported schema: v2 rebuild required".into(),
        ));
    }
    if !is_safe_id(&manifest.id)
        || manifest.name.trim().is_empty()
        || manifest.version.trim().is_empty()
    {
        return Err(reject(
            PluginDiagnosticKind::Malformed,
            "safe id, name and version are required".into(),
        ));
    }
    if manifest.surfaces != [PluginSurface::Tool] || manifest.tools.is_empty() {
        return Err(reject(
            PluginDiagnosticKind::Surface,
            "only nonempty Tool packages are supported".into(),
        ));
    }
    let runtime = &manifest.runtime;
    if runtime.kind != PLUGIN_RUNTIME_COMPONENT_KIND {
        return Err(reject(
            PluginDiagnosticKind::Api,
            "only wasm-component v2 is supported".into(),
        ));
    }
    validate_manifest_path(&runtime.component, archive, label, source, &manifest.id)?;
    schema::validate_config_schema(&manifest.config_schema).map_err(|e| {
        reject(
            PluginDiagnosticKind::Malformed,
            format!("invalid config schema: {e}"),
        )
    })?;
    let mut names = BTreeSet::new();
    for tool in &manifest.tools {
        if !is_safe_id(&tool.name)
            || tool.description.trim().is_empty()
            || !names.insert(&tool.name)
        {
            return Err(reject(
                PluginDiagnosticKind::Duplicate,
                "Tool names must be safe, unique and described".into(),
            ));
        }
        for value in [&tool.input_schema, &tool.output_schema] {
            schema::validate_tool_schema(value).map_err(|e| {
                reject(
                    PluginDiagnosticKind::Malformed,
                    format!("invalid Tool schema: {e}"),
                )
            })?;
        }
        let errors: BTreeSet<_> = tool.error_codes.iter().collect();
        if errors.len() != tool.error_codes.len() || tool.error_codes.iter().any(|v| !is_safe_id(v))
        {
            return Err(reject(
                PluginDiagnosticKind::Malformed,
                "business error codes must be safe and unique".into(),
            ));
        }
    }
    let mut slots = BTreeSet::new();
    for slot in &manifest.connections {
        if !is_safe_id(&slot.name)
            || !slots.insert(&slot.name)
            || slot.methods.is_empty()
            || slot.methods.iter().collect::<BTreeSet<_>>().len() != slot.methods.len()
            || slot.path_prefixes.is_empty()
            || slot
                .path_prefixes
                .iter()
                .any(|p| !valid_http_path_prefix(p))
            || slot.path_prefixes.iter().collect::<BTreeSet<_>>().len() != slot.path_prefixes.len()
        {
            return Err(reject(
                PluginDiagnosticKind::Malformed,
                "invalid or duplicate HTTPS slot/request bounds".into(),
            ));
        }
    }
    let mut requested_slots = BTreeSet::new();
    let mut logging = false;
    for grant in &manifest.requested_grants {
        match grant {
            PluginRequestedGrant::Logging if !logging => logging = true,
            PluginRequestedGrant::Https { slot }
                if slots.contains(slot) && requested_slots.insert(slot) => {}
            _ => {
                return Err(reject(
                    PluginDiagnosticKind::Malformed,
                    "undeclared or duplicate requested grant".into(),
                ));
            }
        }
    }
    if slots != requested_slots {
        return Err(reject(
            PluginDiagnosticKind::Malformed,
            "each connection slot requires an HTTPS grant request".into(),
        ));
    }
    let http = runtime
        .required_imports
        .iter()
        .any(|v| v == "yoi:host/http@2.0.0");
    let connections = runtime
        .required_imports
        .iter()
        .any(|v| v == "yoi:host/connections@2.0.0");
    let logs = runtime
        .required_imports
        .iter()
        .any(|v| v == "yoi:host/logging@2.0.0");
    if http != !slots.is_empty() || http != connections || logging != logs {
        return Err(reject(
            PluginDiagnosticKind::Api,
            "Host imports and requested grants/slots disagree".into(),
        ));
    }
    component::validate_component(
        &archive.files[&runtime.component],
        &runtime.world,
        &runtime.required_imports,
    )
    .map_err(|e| reject(PluginDiagnosticKind::Api, format!("Component ABI: {e}")))
}

fn valid_http_path_prefix(path: &str) -> bool {
    path.starts_with('/')
        && !path.starts_with("//")
        && !path.bytes().any(|b| {
            b.is_ascii_control()
                || b.is_ascii_whitespace()
                || matches!(b, b'?' | b'#' | b'\\' | b'%' | b':')
        })
        && !path.split('/').any(|v| matches!(v, "." | ".."))
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
    crc: u32,
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
    limits: &PluginPackageLimits,
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
    let comment_len = read_u16(eocd, 20).unwrap_or(u16::MAX) as usize;
    if eocd_offset.checked_add(22 + comment_len) != Some(bytes.len())
        || read_u16(eocd, 8) != Some(entry_count as u16)
        || central_offset.checked_add(central_size) != Some(eocd_offset)
    {
        return Err(malformed_zip(
            label,
            source,
            "inconsistent ZIP directory/end bounds",
        ));
    }
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
            PluginDiagnosticPhase::Inspection,
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
                PluginDiagnosticPhase::Inspection,
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
        if cursor > eocd_offset || read_u16(bytes, name_start - 12) != Some(0) {
            return Err(malformed_zip(
                label,
                source,
                "ZIP directory entry outside bounds or multi-disk entry",
            ));
        }
        entries.push(CentralDirectoryEntry {
            crc: read_u32(bytes, name_start - 30).unwrap(),
            name,
            compressed_size,
            uncompressed_size,
            local_header_offset,
            compression_method,
            flags,
            external_attributes,
        });
    }

    if cursor != eocd_offset {
        return Err(malformed_zip(label, source, "ZIP directory size mismatch"));
    }
    let mut files = BTreeMap::new();
    let mut expanded_size = 0u64;
    let mut ranges = Vec::new();
    for entry in entries {
        if entry.flags & !0x0800 != 0 {
            return Err(malformed_zip(label, source, "unsupported ZIP flags"));
        }
        if is_zip_symlink(entry.external_attributes) {
            return Err(PluginDiagnostic::new(
                PluginDiagnosticKind::Traversal,
                PluginDiagnosticPhase::Inspection,
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
                PluginDiagnosticPhase::Inspection,
                "plugin package uses an unsupported zip compression method",
            )
            .with_source(source)
            .with_package(label));
        }
        if u64::from(entry.uncompressed_size) > limits.max_file_size_bytes {
            return Err(PluginDiagnostic::new(
                PluginDiagnosticKind::Bounds,
                PluginDiagnosticPhase::Inspection,
                "plugin package entry exceeds the configured per-file bound",
            )
            .with_source(source)
            .with_package(label));
        }
        expanded_size = expanded_size.saturating_add(u64::from(entry.uncompressed_size));
        if expanded_size > limits.max_expanded_size_bytes {
            return Err(PluginDiagnostic::new(
                PluginDiagnosticKind::Bounds,
                PluginDiagnosticPhase::Inspection,
                "plugin package expanded size exceeds the configured bound",
            )
            .with_source(source)
            .with_package(label));
        }
        let (data, end) = read_stored_entry(bytes, &entry, label, source)?;
        if end > central_offset
            || ranges
                .iter()
                .any(|&(start, stop)| (entry.local_header_offset as usize) < stop && end > start)
        {
            return Err(malformed_zip(
                label,
                source,
                "overlapping ZIP entries or directory data",
            ));
        }
        ranges.push((entry.local_header_offset as usize, end));
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
                PluginDiagnosticPhase::Inspection,
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
) -> Result<(Vec<u8>, usize), PluginDiagnostic> {
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
    let name_start = cursor + 30;
    if bytes.get(name_start..name_start.saturating_add(name_len)) != Some(entry.name.as_bytes())
        || read_u32(bytes, cursor + 14) != Some(entry.crc)
        || read_u32(bytes, cursor + 18) != Some(entry.compressed_size)
        || read_u32(bytes, cursor + 22) != Some(entry.uncompressed_size)
        || local_flags != entry.flags
        || local_method != entry.compression_method
    {
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
    let data = &bytes[data_start..data_end];
    if crc32fast::hash(data) != entry.crc {
        return Err(malformed_zip(label, source, "ZIP CRC mismatch"));
    }
    Ok((data.to_vec(), data_end))
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
        PluginDiagnosticPhase::Inspection,
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

fn read_bounded_file(path: &Path, max: u64) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(max.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    Ok(bytes)
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
    use tempfile::tempdir;

    fn fixture_files() -> BTreeMap<String, Vec<u8>> {
        let mut resolve = wit_parser::Resolve::default();
        resolve
            .push_str(
                "host.wit",
                include_str!("../../../resources/plugin/wit/deps/yoi-host/yoi-host-v2.wit"),
            )
            .unwrap();
        let package = resolve
            .push_str(
                "plugin.wit",
                include_str!("../../../resources/plugin/wit/yoi-plugin-v2.wit"),
            )
            .unwrap();
        let world = resolve
            .select_world(&[package], Some("tool-plugin"))
            .unwrap();
        let mut module =
            wit_component::dummy_module(&resolve, world, wit_parser::ManglingAndAbi::Standard32);
        wit_component::embed_component_metadata(
            &mut module,
            &resolve,
            world,
            wit_component::StringEncoding::UTF8,
        )
        .unwrap();
        let component = wit_component::ComponentEncoder::default()
            .module(&module)
            .unwrap()
            .validate(true)
            .encode()
            .unwrap();
        let manifest = r#"
schema_version = 2
id = "example.pure"
name = "Pure"
version = "0.1.0"
surfaces = ["tool"]
config_schema = "{}"
[runtime]
kind = "wasm-component"
component = "plugin.component.wasm"
world = "yoi:plugin/tool-plugin@2.0.0"
required_imports = []
[[tools]]
name = "echo"
description = "Echo input"
input_schema = { type = "object", additionalProperties = false }
output_schema = { type = "object", additionalProperties = false }
"#;
        BTreeMap::from([
            ("plugin.toml".into(), manifest.as_bytes().to_vec()),
            ("plugin.component.wasm".into(), component),
        ])
    }

    fn inspect(
        files: BTreeMap<String, Vec<u8>>,
    ) -> Result<MaterializedPluginPackage, PluginDiagnostic> {
        materialize_files(
            Path::new("fixture"),
            "fixture".into(),
            PluginSourceKind::Explicit,
            files,
            &PluginPackageLimits::default(),
        )
    }

    #[test]
    fn explicit_package_round_trip_does_not_discover_ambient_stores() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("source");
        let ambient = dir.path().join(".yoi/plugins/ignored.yoi-plugin");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(ambient.parent().unwrap()).unwrap();
        fs::write(&ambient, b"malformed ambient package").unwrap();
        for (name, content) in fixture_files() {
            fs::write(source.join(name), content).unwrap();
        }
        let limits = PluginPackageLimits::default();
        let package = read_plugin_directory(&source, PluginSourceKind::Explicit, &limits).unwrap();
        let output = dir.path().join("explicit.yoi-plugin");
        let packed = write_plugin_package_file(&package, &output, &limits).unwrap();
        let reread =
            read_plugin_package_file(&output, PluginSourceKind::Explicit, &limits).unwrap();
        assert_eq!(reread.package.identity.source, PluginSourceKind::Explicit);
        assert_eq!(
            reread.package.package_digest,
            package.package.package_digest
        );
        assert_eq!(reread.artifact_digest, packed.artifact_digest);
        assert_eq!(reread.artifact_size, fs::metadata(output).unwrap().len());
    }

    #[test]
    fn canonical_pack_is_reproducible_and_digests_have_distinct_meanings() {
        let package = inspect(fixture_files()).unwrap();
        let limits = PluginPackageLimits::default();
        let a = pack_plugin_package(&package, &limits).unwrap();
        let b = pack_plugin_package(&package, &limits).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.package_digest, deterministic_digest(&package.files));
        assert_eq!(a.artifact_digest, artifact_digest(&a.bytes));
        assert_ne!(a.package_digest, a.artifact_digest);
        verify_plugin_artifact(
            &a.bytes,
            &a.package_digest,
            &a.artifact_digest,
            a.artifact_size,
            &limits,
        )
        .unwrap();
        let mut noncanonical = a.bytes.clone();
        // A ZIP comment changes transport bytes, not the logical file set.
        let end = noncanonical.len();
        noncanonical[end - 2..].copy_from_slice(&4u16.to_le_bytes());
        noncanonical.extend_from_slice(b"note");
        let uploaded =
            read_plugin_package_bytes(&noncanonical, PluginSourceKind::Explicit, &limits).unwrap();
        assert_eq!(uploaded.package.package_digest, a.package_digest);
        assert_eq!(pack_plugin_package(&uploaded, &limits).unwrap(), a);
        assert!(
            verify_plugin_artifact(
                &noncanonical,
                &a.package_digest,
                &artifact_digest(&noncanonical),
                noncanonical.len() as u64,
                &limits
            )
            .is_err()
        );
    }

    #[test]
    fn artifact_verification_rejects_each_digest_size_and_changed_bytes() {
        let artifact = pack_plugin_package(
            &inspect(fixture_files()).unwrap(),
            &PluginPackageLimits::default(),
        )
        .unwrap();
        for (package, transport, size) in [
            (
                "sha256:wrong",
                artifact.artifact_digest.as_str(),
                artifact.artifact_size,
            ),
            (
                artifact.package_digest.as_str(),
                "sha256:wrong",
                artifact.artifact_size,
            ),
            (
                artifact.package_digest.as_str(),
                artifact.artifact_digest.as_str(),
                artifact.artifact_size + 1,
            ),
        ] {
            assert_eq!(
                verify_plugin_artifact(
                    &artifact.bytes,
                    package,
                    transport,
                    size,
                    &PluginPackageLimits::default()
                )
                .unwrap_err()
                .kind,
                PluginDiagnosticKind::Digest
            );
        }
        let mut bytes = artifact.bytes.clone();
        bytes[40] ^= 1;
        assert!(
            verify_plugin_artifact(
                &bytes,
                &artifact.package_digest,
                &artifact.artifact_digest,
                artifact.artifact_size,
                &PluginPackageLimits::default()
            )
            .is_err()
        );
    }

    #[test]
    fn version_surface_schema_slot_and_component_mismatches_are_rejected() {
        let base = fixture_files();
        let text = String::from_utf8(base["plugin.toml"].clone()).unwrap();
        for (from, to) in [
            ("schema_version = 2", "schema_version = 1"),
            ("schema_version = 2", "schema_version = 999"),
            ("surfaces = [\"tool\"]", "surfaces = [\"service\"]"),
            ("tool-plugin@2.0.0", "tool-plugin@2.0.1"),
            ("tool-plugin@2.0.0", "https-tool-plugin@2.0.0"),
            ("type = \"object\"", "type = \"nonexistent\""),
            (
                "config_schema = \"{}\"",
                "config_schema = \"{ ...String }\"",
            ),
            (
                "required_imports = []",
                "required_imports = [\"yoi:host/http@2.0.0\"]",
            ),
        ] {
            let mut files = base.clone();
            let changed = text.replace(from, to);
            assert_ne!(changed, text);
            files.insert("plugin.toml".into(), changed.into_bytes());
            assert!(inspect(files).is_err(), "mutation {from} -> {to}");
        }
        for extra in [
            "\n[[connections]]\nname = \"api\"\nmethods = [\"GET\"]\npath_prefixes = [\"/\"]",
            "\n[[requested_grants]]\nkind = \"https\"\nslot = \"missing\"",
            "\n[[requested_grants]]\nkind = \"logging\"",
        ] {
            let mut files = base.clone();
            files.insert("plugin.toml".into(), format!("{text}{extra}").into_bytes());
            assert!(inspect(files).is_err());
        }
        let mut files = base;
        files.insert("plugin.component.wasm".into(), b"not a component".to_vec());
        assert!(inspect(files).is_err());
    }

    #[test]
    fn stored_archive_rejects_traversal_duplicates_corruption_and_size_bounds() {
        let limits = PluginPackageLimits::default();
        let mut files = fixture_files();
        files.insert("../escape".into(), vec![]);
        assert_eq!(
            canonical_archive_bytes(&files, &limits).unwrap_err().kind,
            PluginDiagnosticKind::Traversal
        );
        // Equal-length paths let both local and central names be changed coherently.
        let bytes = canonical_archive_bytes(
            &BTreeMap::from([("aa".into(), vec![1]), ("bb".into(), vec![2])]),
            &limits,
        )
        .unwrap();
        for (name, replacement, kind) in [
            (b"bb", b"aa", PluginDiagnosticKind::Duplicate),
            (b"bb", b"..", PluginDiagnosticKind::Traversal),
        ] {
            let mut mutated = bytes.clone();
            for index in 0..mutated.len() - 1 {
                if &mutated[index..index + 2] == name {
                    mutated[index..index + 2].copy_from_slice(replacement);
                }
            }
            assert_eq!(
                parse_stored_zip(&mutated, "fixture", PluginSourceKind::Explicit, &limits)
                    .unwrap_err()
                    .kind,
                kind
            );
        }
        let mut corrupt = bytes.clone();
        corrupt[32] ^= 1;
        assert!(
            parse_stored_zip(&corrupt, "fixture", PluginSourceKind::Explicit, &limits).is_err()
        );
        for small in [
            PluginPackageLimits {
                max_package_size_bytes: 1,
                ..limits.clone()
            },
            PluginPackageLimits {
                max_entries_per_package: 1,
                ..limits.clone()
            },
            PluginPackageLimits {
                max_file_size_bytes: 0,
                ..limits.clone()
            },
            PluginPackageLimits {
                max_expanded_size_bytes: 1,
                ..limits.clone()
            },
        ] {
            assert!(read_plugin_package_bytes(&bytes, PluginSourceKind::Explicit, &small).is_err());
        }
    }

    #[test]
    fn pack_revalidates_mutated_files_and_directory_rejects_symlinks() {
        let mut package = inspect(fixture_files()).unwrap();
        package.files.insert("plugin.component.wasm".into(), vec![]);
        assert!(pack_plugin_package(&package, &PluginPackageLimits::default()).is_err());
        #[cfg(unix)]
        {
            let dir = tempdir().unwrap();
            std::os::unix::fs::symlink("outside", dir.path().join("link")).unwrap();
            assert_eq!(
                read_plugin_directory(
                    dir.path(),
                    PluginSourceKind::Explicit,
                    &PluginPackageLimits::default()
                )
                .unwrap_err()
                .kind,
                PluginDiagnosticKind::Traversal
            );
        }
    }

    #[test]
    fn production_source_has_no_ambient_plugin_authority() {
        let source = include_str!("plugin.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        for forbidden in [
            ".yoi/plugins",
            "discover_plugins",
            "resolve_enabled_plugins",
            "resolve_plugin_config_for_startup",
            "ResolvedPluginRecord",
            "read_resolved_plugin_runtime_component",
            "package_path",
            "PluginSourceKind::Project",
            "PluginSourceKind::User",
        ] {
            assert!(
                !source.contains(forbidden),
                "ambient Plugin authority returned through {forbidden}"
            );
        }
    }
}
