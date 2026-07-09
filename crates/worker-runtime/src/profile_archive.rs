use decodal::{Engine, LoadedSource, SourceLoader};
use manifest::{ProfileSource, WorkerManifest, resolve_profile_artifact_value};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read, Write};
use std::path::{Component, Path, PathBuf};
use tar::{Archive, Builder, Header};

const MANIFEST_PATH: &str = "manifest.json";
const MAX_SOURCES: usize = 64;
const MAX_SOURCE_BYTES: u64 = 256 * 1024;
const MAX_TOTAL_BYTES: u64 = 1024 * 1024;
const MAX_PATH_DEPTH: usize = 8;

#[derive(Debug, thiserror::Error)]
pub enum ProfileArchiveError {
    #[error("profile source archive has unsupported schema version {0}")]
    UnsupportedSchema(u32),
    #[error("profile source archive path must be relative and confined: {0}")]
    UnsafePath(String),
    #[error("profile source archive contains unsupported entry type at {0}")]
    UnsupportedEntry(String),
    #[error("profile source archive limit exceeded: {0}")]
    LimitExceeded(&'static str),
    #[error("profile source archive missing manifest.json")]
    MissingManifest,
    #[error("profile source archive missing source {0}")]
    MissingSource(String),
    #[error("profile source archive contains duplicate source {0}")]
    DuplicateSource(String),
    #[error("profile source archive contains source not declared in manifest {0}")]
    UndeclaredSource(String),
    #[error(
        "profile source archive source digest mismatch for {path}: expected {expected}, got {actual}"
    )]
    SourceDigestMismatch {
        path: String,
        expected: String,
        actual: String,
    },
    #[error("profile source archive digest mismatch for {id}: expected {expected}, got {actual}")]
    ArchiveDigestMismatch {
        id: String,
        expected: String,
        actual: String,
    },
    #[error("profile source archive has unsupported source kind {kind} at {path}")]
    UnsupportedSource { path: String, kind: String },
    #[error("profile source archive has no entrypoint for selector {0}")]
    MissingEntrypoint(String),
    #[error("profile source archive import is not declared in manifest import map: {0}")]
    ImportNotDeclared(String),
    #[error(
        "profile source archive import map mismatch for {specifier}: expected {expected}, got {actual}"
    )]
    ImportMapMismatch {
        specifier: String,
        expected: String,
        actual: String,
    },
    #[error("failed to read profile source archive: {0}")]
    Io(String),
    #[error("failed to parse profile source archive manifest: {0}")]
    Json(String),
    #[error("failed to evaluate Decodal profile source {path}: {message}")]
    Decodal { path: String, message: String },
    #[error("failed to resolve archive profile artifact {profile_source}: {message}")]
    Profile {
        profile_source: String,
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProfileSourceArchiveRef {
    pub id: String,
    pub digest: String,
    pub size_bytes: u64,
    pub source_graph: ProfileSourceGraphSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProfileSourceGraphSummary {
    pub source_count: usize,
    pub total_source_bytes: u64,
    pub entrypoints: BTreeMap<String, String>,
    pub import_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProfileSourceArchive {
    pub reference: ProfileSourceArchiveRef,
    pub content: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProfileSourceArchiveManifest {
    pub schema_version: u32,
    pub id: String,
    pub entrypoints: BTreeMap<String, String>,
    pub imports: BTreeMap<String, String>,
    pub sources: Vec<ProfileSourceArchiveSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProfileSourceArchiveSource {
    pub path: String,
    #[serde(default)]
    pub source_key: String,
    pub kind: String,
    #[serde(default = "default_decodal_content_type")]
    pub content_type: String,
    pub digest: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProfileSourceArchiveInput {
    pub id: String,
    pub entrypoints: BTreeMap<String, String>,
    pub imports: BTreeMap<String, String>,
    pub sources: BTreeMap<String, String>,
}

impl ProfileSourceArchive {
    pub fn build(input: ProfileSourceArchiveInput) -> Result<Self, ProfileArchiveError> {
        if input.sources.len() > MAX_SOURCES {
            return Err(ProfileArchiveError::LimitExceeded("source count"));
        }
        let mut total_source_bytes = 0u64;
        let mut source_meta = Vec::new();
        for (path, source) in &input.sources {
            validate_archive_path(path)?;
            let size = source.as_bytes().len() as u64;
            if size > MAX_SOURCE_BYTES {
                return Err(ProfileArchiveError::LimitExceeded("source bytes"));
            }
            total_source_bytes = total_source_bytes
                .checked_add(size)
                .ok_or(ProfileArchiveError::LimitExceeded("total source bytes"))?;
            if total_source_bytes > MAX_TOTAL_BYTES {
                return Err(ProfileArchiveError::LimitExceeded("total source bytes"));
            }
            source_meta.push(ProfileSourceArchiveSource {
                path: path.clone(),
                source_key: path.clone(),
                kind: "decodal".to_string(),
                content_type: default_decodal_content_type(),
                digest: sha256_hex(source.as_bytes()),
                size_bytes: size,
            });
        }
        for path in input.entrypoints.values().chain(input.imports.values()) {
            validate_archive_path(path)?;
            if !input.sources.contains_key(path) {
                return Err(ProfileArchiveError::MissingSource(path.clone()));
            }
        }
        let manifest = ProfileSourceArchiveManifest {
            schema_version: 1,
            id: input.id.clone(),
            entrypoints: input.entrypoints.clone(),
            imports: input.imports.clone(),
            sources: source_meta,
        };
        let manifest_json = serde_json::to_vec_pretty(&manifest)
            .map_err(|err| ProfileArchiveError::Json(err.to_string()))?;
        let mut bytes = Vec::new();
        {
            let mut builder = Builder::new(&mut bytes);
            append_bytes(&mut builder, MANIFEST_PATH, &manifest_json)?;
            for (path, source) in &input.sources {
                append_bytes(&mut builder, path, source.as_bytes())?;
            }
            builder
                .finish()
                .map_err(|err| ProfileArchiveError::Io(err.to_string()))?;
        }
        let digest = sha256_hex(&bytes);
        let reference = ProfileSourceArchiveRef {
            id: input.id,
            digest,
            size_bytes: bytes.len() as u64,
            source_graph: ProfileSourceGraphSummary {
                source_count: input.sources.len(),
                total_source_bytes,
                entrypoints: input.entrypoints,
                import_count: manifest.imports.len(),
            },
        };
        Ok(Self {
            reference,
            content: bytes,
        })
    }

    pub fn verify(&self) -> Result<VerifiedProfileSourceArchive, ProfileArchiveError> {
        let digest = sha256_hex(&self.content);
        if digest != self.reference.digest {
            return Err(ProfileArchiveError::ArchiveDigestMismatch {
                id: self.reference.id.clone(),
                expected: self.reference.digest.clone(),
                actual: digest,
            });
        }
        VerifiedProfileSourceArchive::from_bytes(self.reference.clone(), &self.content)
    }
}

#[derive(Debug, Clone)]
pub struct VerifiedProfileSourceArchive {
    reference: ProfileSourceArchiveRef,
    manifest: ProfileSourceArchiveManifest,
    sources: BTreeMap<String, String>,
}

impl VerifiedProfileSourceArchive {
    pub fn from_bytes(
        reference: ProfileSourceArchiveRef,
        bytes: &[u8],
    ) -> Result<Self, ProfileArchiveError> {
        if bytes.len() as u64 > MAX_TOTAL_BYTES + 64 * 1024 {
            return Err(ProfileArchiveError::LimitExceeded("archive bytes"));
        }
        let mut archive = Archive::new(Cursor::new(bytes));
        let mut manifest: Option<ProfileSourceArchiveManifest> = None;
        let mut entries = BTreeMap::new();
        for entry in archive
            .entries()
            .map_err(|err| ProfileArchiveError::Io(err.to_string()))?
        {
            let mut entry = entry.map_err(|err| ProfileArchiveError::Io(err.to_string()))?;
            let path = entry
                .path()
                .map_err(|err| ProfileArchiveError::Io(err.to_string()))?
                .to_string_lossy()
                .to_string();
            validate_archive_path(&path)?;
            if !entry.header().entry_type().is_file() {
                return Err(ProfileArchiveError::UnsupportedEntry(path));
            }
            let size = entry.size();
            if size > MAX_SOURCE_BYTES && path != MANIFEST_PATH {
                return Err(ProfileArchiveError::LimitExceeded("source bytes"));
            }
            let mut data = Vec::new();
            entry
                .read_to_end(&mut data)
                .map_err(|err| ProfileArchiveError::Io(err.to_string()))?;
            if path == MANIFEST_PATH {
                manifest = Some(
                    serde_json::from_slice(&data)
                        .map_err(|err| ProfileArchiveError::Json(err.to_string()))?,
                );
            } else {
                let source = String::from_utf8(data)
                    .map_err(|err| ProfileArchiveError::Io(err.to_string()))?;
                if entries.insert(path.clone(), source).is_some() {
                    return Err(ProfileArchiveError::DuplicateSource(path));
                }
            }
            if entries.len() > MAX_SOURCES {
                return Err(ProfileArchiveError::LimitExceeded("source count"));
            }
        }
        let manifest = manifest.ok_or(ProfileArchiveError::MissingManifest)?;
        if manifest.schema_version != 1 {
            return Err(ProfileArchiveError::UnsupportedSchema(
                manifest.schema_version,
            ));
        }
        if manifest.id != reference.id {
            return Err(ProfileArchiveError::ArchiveDigestMismatch {
                id: reference.id,
                expected: "matching archive id".to_string(),
                actual: manifest.id,
            });
        }
        let mut total = 0u64;
        let mut manifest_paths = BTreeSet::new();
        for source in &manifest.sources {
            validate_archive_path(&source.path)?;
            if !source.source_key.is_empty() && source.source_key != source.path {
                return Err(ProfileArchiveError::ImportMapMismatch {
                    specifier: source.path.clone(),
                    expected: source.path.clone(),
                    actual: source.source_key.clone(),
                });
            }
            if source.content_type != default_decodal_content_type() {
                return Err(ProfileArchiveError::UnsupportedSource {
                    path: source.path.clone(),
                    kind: source.content_type.clone(),
                });
            }
            if !manifest_paths.insert(source.path.clone()) {
                return Err(ProfileArchiveError::DuplicateSource(source.path.clone()));
            }
            if source.kind != "decodal" {
                return Err(ProfileArchiveError::UnsupportedSource {
                    path: source.path.clone(),
                    kind: source.kind.clone(),
                });
            }
            let content = entries
                .get(&source.path)
                .ok_or_else(|| ProfileArchiveError::MissingSource(source.path.clone()))?;
            let digest = sha256_hex(content.as_bytes());
            if digest != source.digest {
                return Err(ProfileArchiveError::SourceDigestMismatch {
                    path: source.path.clone(),
                    expected: source.digest.clone(),
                    actual: digest,
                });
            }
            let size = content.as_bytes().len() as u64;
            if size != source.size_bytes {
                return Err(ProfileArchiveError::LimitExceeded(
                    "source byte metadata mismatch",
                ));
            }
            total += size;
        }
        for entry_path in entries.keys() {
            if !manifest_paths.contains(entry_path) {
                return Err(ProfileArchiveError::UndeclaredSource(entry_path.clone()));
            }
        }
        if total != reference.source_graph.total_source_bytes
            || manifest.sources.len() != reference.source_graph.source_count
        {
            return Err(ProfileArchiveError::LimitExceeded(
                "source graph metadata mismatch",
            ));
        }
        for path in manifest
            .entrypoints
            .values()
            .chain(manifest.imports.values())
        {
            validate_archive_path(path)?;
            if !manifest_paths.contains(path) || !entries.contains_key(path) {
                return Err(ProfileArchiveError::MissingSource(path.clone()));
            }
        }
        for (key, path) in &manifest.imports {
            validate_archive_import_map_entry(key, path, &manifest_paths)?;
        }
        Ok(Self {
            reference,
            manifest,
            sources: entries,
        })
    }

    pub fn reference(&self) -> &ProfileSourceArchiveRef {
        &self.reference
    }

    pub fn manifest(&self) -> &ProfileSourceArchiveManifest {
        &self.manifest
    }

    pub fn resolve_profile(
        &self,
        selector: &str,
        worker_root: &Path,
        worker_name: &str,
    ) -> Result<WorkerManifest, ProfileArchiveError> {
        let path = self
            .manifest
            .entrypoints
            .get(selector)
            .ok_or_else(|| ProfileArchiveError::MissingEntrypoint(selector.to_string()))?;
        let mut engine = Engine::new(ArchiveSourceLoader { archive: self });
        let source = self
            .sources
            .get(path)
            .ok_or_else(|| ProfileArchiveError::MissingSource(path.clone()))?;
        let module = engine.add_root_source(path, path, source).map_err(|err| {
            ProfileArchiveError::Decodal {
                path: path.clone(),
                message: format!("{err:?}"),
            }
        })?;
        let value = engine
            .eval_module(module)
            .map_err(|err| ProfileArchiveError::Decodal {
                path: path.clone(),
                message: format!("{err:?}"),
            })?;
        let data = engine
            .materialize(&value)
            .map_err(|err| ProfileArchiveError::Decodal {
                path: path.clone(),
                message: format!("{err:?}"),
            })?;
        let json = decodal_data_to_json(&data);
        let resolved = resolve_profile_artifact_value(
            json,
            ProfileSource::Archive {
                archive_id: self.reference.id.clone(),
                source: path.clone(),
            },
            worker_root,
            worker_name,
        )
        .map_err(|err| ProfileArchiveError::Profile {
            profile_source: path.clone(),
            message: err.to_string(),
        })?;
        Ok(resolved.manifest)
    }
}

pub struct ArchiveSourceLoader<'a> {
    archive: &'a VerifiedProfileSourceArchive,
}

impl<'a> ArchiveSourceLoader<'a> {
    pub fn new(archive: &'a VerifiedProfileSourceArchive) -> Self {
        Self { archive }
    }
}

impl SourceLoader for ArchiveSourceLoader<'_> {
    fn load(
        &mut self,
        current_key: Option<&str>,
        specifier: &str,
    ) -> decodal::Result<LoadedSource> {
        let path =
            archive_import_map_lookup(&self.archive.manifest.imports, current_key, specifier)
                .map_err(import_diagnostic)?;
        if let Some(current_key) = current_key {
            match resolve_archive_import_path(current_key, specifier) {
                Ok(expected) if expected == path => {}
                Ok(expected) => {
                    return Err(import_diagnostic(ProfileArchiveError::ImportMapMismatch {
                        specifier: format!("{current_key}\0{specifier}"),
                        expected,
                        actual: path.clone(),
                    }));
                }
                Err(err) => return Err(import_diagnostic(err)),
            }
        }
        let source = self.archive.sources.get(&path).ok_or_else(|| {
            decodal::Diagnostic::new(
                decodal::DiagnosticKind::Import,
                decodal::Span::default(),
                format!("archive source missing: {path}"),
            )
        })?;
        Ok(LoadedSource {
            key: path.clone(),
            name: path.clone(),
            source: source.clone(),
        })
    }
}

fn default_decodal_content_type() -> String {
    "text/x-decodal".to_string()
}

fn import_diagnostic(err: ProfileArchiveError) -> decodal::Diagnostic {
    decodal::Diagnostic::new(
        decodal::DiagnosticKind::Import,
        decodal::Span::default(),
        err.to_string(),
    )
}

fn archive_import_map_lookup(
    imports: &BTreeMap<String, String>,
    current_key: Option<&str>,
    specifier: &str,
) -> Result<String, ProfileArchiveError> {
    if let Some(current_key) = current_key {
        let scoped = format!("{current_key}\0{specifier}");
        if let Some(path) = imports.get(&scoped) {
            return Ok(path.clone());
        }
    }
    imports
        .get(specifier)
        .cloned()
        .ok_or_else(|| ProfileArchiveError::ImportNotDeclared(specifier.to_string()))
}

fn validate_archive_import_map_entry(
    key: &str,
    path: &str,
    manifest_paths: &BTreeSet<String>,
) -> Result<(), ProfileArchiveError> {
    if !manifest_paths.contains(path) {
        return Err(ProfileArchiveError::MissingSource(path.to_string()));
    }
    if let Some((current_key, specifier)) = key.split_once('\0') {
        validate_archive_path(current_key)?;
        let expected = resolve_archive_import_path(current_key, specifier)?;
        if expected != path {
            return Err(ProfileArchiveError::ImportMapMismatch {
                specifier: key.to_string(),
                expected,
                actual: path.to_string(),
            });
        }
    } else {
        validate_archive_import_specifier(key)?;
    }
    Ok(())
}

fn resolve_archive_import_path(
    current_key: &str,
    specifier: &str,
) -> Result<String, ProfileArchiveError> {
    validate_archive_path(current_key)?;
    validate_archive_import_specifier(specifier)?;
    let raw = if let Some(rest) = specifier.strip_prefix("project:") {
        rest
    } else if let Some(rest) = specifier.strip_prefix("workspace:") {
        rest
    } else {
        specifier
    };
    let base = if raw.starts_with("./") || raw.starts_with("../") {
        Path::new(current_key)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default()
            .join(raw)
    } else {
        PathBuf::from(raw)
    };
    normalize_archive_virtual_path(&base.to_string_lossy())
}

fn validate_archive_import_specifier(specifier: &str) -> Result<(), ProfileArchiveError> {
    if specifier.is_empty()
        || specifier.contains("://")
        || specifier.starts_with('/')
        || Path::new(specifier).is_absolute()
    {
        return Err(ProfileArchiveError::UnsafePath(specifier.to_string()));
    }
    if let Some((namespace, rest)) = specifier.split_once(':') {
        if namespace != "project" && namespace != "workspace" {
            return Err(ProfileArchiveError::UnsafePath(specifier.to_string()));
        }
        if rest.is_empty() || rest.starts_with('/') || rest.contains("://") {
            return Err(ProfileArchiveError::UnsafePath(specifier.to_string()));
        }
    }
    Ok(())
}

fn normalize_archive_virtual_path(path: &str) -> Result<String, ProfileArchiveError> {
    let path_ref = Path::new(path);
    if path_ref.is_absolute() {
        return Err(ProfileArchiveError::UnsafePath(path.to_string()));
    }
    let mut out = PathBuf::new();
    for component in path_ref.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(value) => out.push(value),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(ProfileArchiveError::UnsafePath(path.to_string()));
            }
        }
    }
    let normalized = out.to_string_lossy().replace('\\', "/");
    validate_archive_path(&normalized)?;
    Ok(normalized)
}

fn decodal_data_to_json(data: &decodal::Data) -> serde_json::Value {
    match data {
        decodal::Data::Bool(value) => serde_json::Value::Bool(*value),
        decodal::Data::Int(value) => serde_json::Value::Number(serde_json::Number::from(*value)),
        decodal::Data::Float(value) => serde_json::Number::from_f64(*value)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        decodal::Data::String(value) => serde_json::Value::String(value.clone()),
        decodal::Data::Array(values) => {
            serde_json::Value::Array(values.iter().map(decodal_data_to_json).collect())
        }
        decodal::Data::Object(fields) => serde_json::Value::Object(
            fields
                .iter()
                .map(|field| (field.name.clone(), decodal_data_to_json(&field.value)))
                .collect(),
        ),
    }
}

fn append_bytes<W: Write>(
    builder: &mut Builder<W>,
    path: &str,
    data: &[u8],
) -> Result<(), ProfileArchiveError> {
    validate_archive_path(path)?;
    let mut header = Header::new_gnu();
    header.set_size(data.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    builder
        .append_data(&mut header, path, Cursor::new(data))
        .map_err(|err| ProfileArchiveError::Io(err.to_string()))
}

fn validate_archive_path(path: &str) -> Result<(), ProfileArchiveError> {
    if path.is_empty() || path == MANIFEST_PATH {
        return if path == MANIFEST_PATH {
            Ok(())
        } else {
            Err(ProfileArchiveError::UnsafePath(path.to_string()))
        };
    }
    let path_ref = Path::new(path);
    if path_ref.is_absolute() {
        return Err(ProfileArchiveError::UnsafePath(path.to_string()));
    }
    let mut depth = 0usize;
    for component in path_ref.components() {
        match component {
            Component::Normal(_) => depth += 1,
            _ => return Err(ProfileArchiveError::UnsafePath(path.to_string())),
        }
    }
    if depth == 0 || depth > MAX_PATH_DEPTH {
        return Err(ProfileArchiveError::UnsafePath(path.to_string()));
    }
    let normalized = PathBuf::from(path).to_string_lossy().replace('\\', "/");
    if normalized != path {
        return Err(ProfileArchiveError::UnsafePath(path.to_string()));
    }
    Ok(())
}

pub fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    let mut out = String::from("sha256:");
    for byte in digest.as_slice() {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_archive() -> ProfileSourceArchive {
        let mut entrypoints = BTreeMap::new();
        entrypoints.insert("default".to_string(), "profiles/default.dcdl".to_string());
        let mut sources = BTreeMap::new();
        sources.insert(
            "profiles/default.dcdl".to_string(),
            r#"{
                slug = "default";
                description = "Default";
                scope = "workspace_read";
            }"#
            .to_string(),
        );
        ProfileSourceArchive::build(ProfileSourceArchiveInput {
            id: "builtin".to_string(),
            entrypoints,
            imports: BTreeMap::new(),
            sources,
        })
        .unwrap()
    }

    #[test]
    fn build_and_verify_archive() {
        let archive = sample_archive();
        let verified = archive.verify().unwrap();
        assert_eq!(verified.reference().source_graph.source_count, 1);
    }

    #[test]
    fn rejects_digest_mismatch() {
        let mut archive = sample_archive();
        archive.reference.digest = "sha256:bad".to_string();
        assert!(archive.verify().is_err());
    }

    #[test]
    fn resolves_profile_from_archive_without_filesystem_loader() {
        let archive = sample_archive();
        let verified = archive.verify().unwrap();
        let root = tempfile::tempdir().unwrap();
        let manifest = verified
            .resolve_profile("default", root.path(), "archive-worker")
            .unwrap();
        assert_eq!(manifest.worker.name, "archive-worker");
    }

    #[test]
    fn unknown_selector_does_not_fallback_to_default() {
        let archive = sample_archive();
        let verified = archive.verify().unwrap();
        let root = tempfile::tempdir().unwrap();
        assert!(matches!(
            verified.resolve_profile("missing", root.path(), "archive-worker"),
            Err(ProfileArchiveError::MissingEntrypoint(selector)) if selector == "missing"
        ));
    }

    #[test]
    fn archive_loader_rejects_undeclared_import() {
        let archive = sample_archive();
        let verified = archive.verify().unwrap();
        let mut loader = ArchiveSourceLoader::new(&verified);
        assert!(loader.load(None, "missing").is_err());
    }

    #[test]
    fn archive_loader_resolves_scoped_virtual_imports() {
        let mut entrypoints = BTreeMap::new();
        entrypoints.insert("default".to_string(), "profiles/main.dcdl".to_string());
        let mut imports = BTreeMap::new();
        imports.insert(
            "profiles/main.dcdl\0./shared.dcdl".to_string(),
            "profiles/shared.dcdl".to_string(),
        );
        let mut sources = BTreeMap::new();
        sources.insert("profiles/main.dcdl".to_string(), "{}".to_string());
        sources.insert("profiles/shared.dcdl".to_string(), "{}".to_string());
        let archive = ProfileSourceArchive::build(ProfileSourceArchiveInput {
            id: "project".to_string(),
            entrypoints,
            imports,
            sources,
        })
        .unwrap();
        let verified = archive.verify().unwrap();
        let mut loader = ArchiveSourceLoader::new(&verified);
        let loaded = loader
            .load(Some("profiles/main.dcdl"), "./shared.dcdl")
            .unwrap();
        assert_eq!(loaded.key, "profiles/shared.dcdl");
    }

    #[test]
    fn archive_verify_rejects_import_map_that_bypasses_virtual_resolver() {
        let mut entrypoints = BTreeMap::new();
        entrypoints.insert("default".to_string(), "profiles/main.dcdl".to_string());
        let mut imports = BTreeMap::new();
        imports.insert(
            "profiles/main.dcdl\0./shared.dcdl".to_string(),
            "profiles/other.dcdl".to_string(),
        );
        let mut sources = BTreeMap::new();
        sources.insert("profiles/main.dcdl".to_string(), "{}".to_string());
        sources.insert("profiles/other.dcdl".to_string(), "{}".to_string());
        let archive = ProfileSourceArchive::build(ProfileSourceArchiveInput {
            id: "project".to_string(),
            entrypoints,
            imports,
            sources,
        })
        .unwrap();
        assert!(matches!(
            archive.verify(),
            Err(ProfileArchiveError::ImportMapMismatch { .. })
        ));
    }
}
