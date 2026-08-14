use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use decodal::{
    Data, Diagnostic, DiagnosticKind, Engine, HostEnvironment, ImportCandidate, ImportLoader,
    LoadedImport, Span, Value,
};
use decodal_language_service::{CompletionResult, LanguageService};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const CONFIG_SOURCE_CONTRACT_VERSION: u32 = 2;
pub const DECODAL_VERSION: &str = "0.4.0";
pub const DEFAULT_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_IMPORT_POLICY_VERSION: u32 = 1;
pub const WORKSPACE_CONFIG_SCHEMA_GLOBAL: &str = "WorkspaceConfigSchema";
pub const WORKSPACE_CONFIG_SCHEMA_SOURCE: &str = "workspace-config-schema.dcdl";
pub const WORKSPACE_CONFIG_EVALUATION_SOURCE: &str =
    "import \"__MAIN_ENTRYPOINT__\" as WorkspaceConfigSchema";
pub const MAX_ENTRY_COUNT: usize = 256;
pub const MAX_CHANGE_COUNT: usize = 256;
pub const MAX_ENTRY_BYTES: usize = 256 * 1024;
pub const MAX_TOTAL_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_PATH_BYTES: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ts_rs::TS)]
#[serde(transparent)]
pub struct VirtualPath(String);

impl VirtualPath {
    pub fn parse(value: impl AsRef<str>) -> Result<Self, ConfigTreeError> {
        let value = value.as_ref();
        if value.is_empty() {
            return Err(ConfigTreeError::InvalidPath(
                "path must not be empty".into(),
            ));
        }
        if value.len() > MAX_PATH_BYTES {
            return Err(ConfigTreeError::LimitExceeded("path bytes"));
        }
        if value.starts_with('/')
            || value.contains('\\')
            || value.contains('\0')
            || value.contains("://")
        {
            return Err(ConfigTreeError::InvalidPath(value.into()));
        }
        let mut normalized = Vec::new();
        for component in value.split('/') {
            if component.is_empty() || component == "." || component == ".." {
                return Err(ConfigTreeError::InvalidPath(value.into()));
            }
            if component.chars().any(char::is_control) {
                return Err(ConfigTreeError::InvalidPath(value.into()));
            }
            normalized.push(component);
        }
        Ok(Self(normalized.join("/")))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn parent_components(&self) -> Vec<&str> {
        let mut components = self.0.split('/').collect::<Vec<_>>();
        components.pop();
        components
    }
}

impl fmt::Display for VirtualPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
pub enum ConfigContentType {
    Decodal,
    Text,
}

impl ConfigContentType {
    pub fn media_type(self) -> &'static str {
        match self {
            Self::Decodal => "text/x-decodal",
            Self::Text => "text/plain",
        }
    }
}

/// Stable value projection used when a virtual config source imports Markdown.
///
/// Frontmatter delimiters are transport syntax and are intentionally absent from
/// `content`; unknown frontmatter keys stay in `frontmatter` without a
/// domain-specific parser interpreting them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MarkdownDocumentProjection {
    pub frontmatter: serde_json::Map<String, serde_json::Value>,
    pub content: String,
}

/// Parse one Markdown source into the common virtual-config import shape.
///
/// Files without a leading YAML frontmatter delimiter produce an empty
/// frontmatter object and preserve the complete file as `content`.
pub fn project_markdown_document(source: &str) -> Result<MarkdownDocumentProjection, String> {
    let Some(after_opening) = source
        .strip_prefix("---\n")
        .or_else(|| source.strip_prefix("---\r\n"))
    else {
        return Ok(MarkdownDocumentProjection {
            frontmatter: serde_json::Map::new(),
            content: source.to_string(),
        });
    };

    let mut frontmatter_end = None;
    let mut offset = 0usize;
    for line_with_ending in after_opening.split_inclusive('\n') {
        let line = line_with_ending.trim_end_matches(['\r', '\n']);
        if line == "---" {
            frontmatter_end = Some((offset, offset + line_with_ending.len()));
            break;
        }
        offset += line_with_ending.len();
    }
    if frontmatter_end.is_none() && after_opening.ends_with("---") {
        let start = after_opening.len() - 3;
        if start == 0 || after_opening[..start].ends_with('\n') {
            frontmatter_end = Some((start, after_opening.len()));
        }
    }
    let Some((frontmatter_end, content_start)) = frontmatter_end else {
        return Err("opening YAML frontmatter delimiter has no closing delimiter".to_string());
    };

    let frontmatter_source = &after_opening[..frontmatter_end];
    let frontmatter = if frontmatter_source.trim().is_empty() {
        serde_json::Map::new()
    } else {
        let yaml = serde_yaml::from_str::<serde_yaml::Value>(frontmatter_source)
            .map_err(|error| format!("invalid YAML frontmatter: {error}"))?;
        let value = yaml_to_json(yaml)?;
        value
            .as_object()
            .cloned()
            .ok_or_else(|| "YAML frontmatter must be a mapping".to_string())?
    };

    Ok(MarkdownDocumentProjection {
        frontmatter,
        content: after_opening[content_start..].to_string(),
    })
}

fn yaml_to_json(value: serde_yaml::Value) -> Result<serde_json::Value, String> {
    match value {
        serde_yaml::Value::Null => Ok(serde_json::Value::Null),
        serde_yaml::Value::Bool(value) => Ok(serde_json::Value::Bool(value)),
        serde_yaml::Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                Ok(serde_json::Value::Number(value.into()))
            } else if let Some(value) = value.as_u64() {
                Ok(serde_json::Value::Number(value.into()))
            } else if let Some(value) = value.as_f64() {
                serde_json::Number::from_f64(value)
                    .map(serde_json::Value::Number)
                    .ok_or_else(|| "YAML frontmatter contains a non-finite number".to_string())
            } else {
                Err("YAML frontmatter contains an unsupported number".to_string())
            }
        }
        serde_yaml::Value::String(value) => Ok(serde_json::Value::String(value)),
        serde_yaml::Value::Sequence(values) => values
            .into_iter()
            .map(yaml_to_json)
            .collect::<Result<Vec<_>, _>>()
            .map(serde_json::Value::Array),
        serde_yaml::Value::Mapping(values) => {
            let mut object = serde_json::Map::new();
            for (key, value) in values {
                let serde_yaml::Value::String(key) = key else {
                    return Err("YAML frontmatter mapping keys must be strings".to_string());
                };
                object.insert(key, yaml_to_json(value)?);
            }
            Ok(serde_json::Value::Object(object))
        }
        serde_yaml::Value::Tagged(_) => Err("YAML frontmatter tags are not supported".to_string()),
    }
}

fn markdown_projection_to_value(projection: MarkdownDocumentProjection) -> Result<Value, String> {
    Ok(Value::object([
        (
            "frontmatter",
            json_to_decodal_value(serde_json::Value::Object(projection.frontmatter))?,
        ),
        ("content", Value::string(projection.content)),
    ]))
}

fn json_to_decodal_value(value: serde_json::Value) -> Result<Value, String> {
    match value {
        serde_json::Value::Null => Err(
            "YAML null values cannot be represented as concrete Decodal import values".to_string(),
        ),
        serde_json::Value::Bool(value) => Ok(Value::bool(value)),
        serde_json::Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                Ok(Value::int(value))
            } else if let Some(value) = value.as_u64() {
                i64::try_from(value)
                    .map(Value::int)
                    .map_err(|_| "YAML integer exceeds the Decodal i64 range".to_string())
            } else if let Some(value) = value.as_f64() {
                Ok(Value::float(value))
            } else {
                Err("JSON number cannot be represented as a Decodal value".to_string())
            }
        }
        serde_json::Value::String(value) => Ok(Value::string(value)),
        serde_json::Value::Array(values) => values
            .into_iter()
            .map(json_to_decodal_value)
            .collect::<Result<Vec<_>, _>>()
            .map(Value::array),
        serde_json::Value::Object(values) => values
            .into_iter()
            .map(|(name, value)| Ok((name, json_to_decodal_value(value)?)))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::object),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
pub struct ConfigEntry {
    pub path: VirtualPath,
    pub content_type: ConfigContentType,
    pub content: String,
    pub content_digest: String,
}

impl ConfigEntry {
    pub fn new(
        path: VirtualPath,
        content_type: ConfigContentType,
        content: impl Into<String>,
    ) -> Result<Self, ConfigTreeError> {
        let content = content.into();
        if content.len() > MAX_ENTRY_BYTES {
            return Err(ConfigTreeError::LimitExceeded("entry bytes"));
        }
        Ok(Self {
            path,
            content_type,
            content_digest: digest_bytes(content.as_bytes()),
            content,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
pub struct ConfigTreeSnapshot {
    #[ts(type = "number")]
    pub revision: u64,
    pub digest: String,
    pub entries: BTreeMap<VirtualPath, ConfigEntry>,
}

impl ConfigTreeSnapshot {
    pub fn empty() -> Self {
        Self::from_entries(0, Vec::new()).expect("empty config snapshot is valid")
    }

    pub fn from_entries(
        revision: u64,
        entries: impl IntoIterator<Item = ConfigEntry>,
    ) -> Result<Self, ConfigTreeError> {
        let mut ordered = BTreeMap::new();
        let mut total = 0usize;
        for entry in entries {
            total = total
                .checked_add(entry.content.len())
                .ok_or(ConfigTreeError::LimitExceeded("total bytes"))?;
            if total > MAX_TOTAL_BYTES {
                return Err(ConfigTreeError::LimitExceeded("total bytes"));
            }
            if ordered.insert(entry.path.clone(), entry).is_some() {
                return Err(ConfigTreeError::DuplicatePath);
            }
        }
        if ordered.len() > MAX_ENTRY_COUNT {
            return Err(ConfigTreeError::LimitExceeded("entry count"));
        }
        let digest = snapshot_digest(&ordered);
        Ok(Self {
            revision,
            digest,
            entries: ordered,
        })
    }

    pub fn list_prefix(&self, prefix: Option<&VirtualPath>) -> Vec<&ConfigEntry> {
        self.entries
            .values()
            .filter(|entry| {
                prefix.is_none_or(|prefix| {
                    entry.path == *prefix
                        || entry
                            .path
                            .as_str()
                            .strip_prefix(prefix.as_str())
                            .is_some_and(|rest| rest.starts_with('/'))
                })
            })
            .collect()
    }

    pub fn get(&self, path: &VirtualPath) -> Option<&ConfigEntry> {
        self.entries.get(path)
    }

    pub fn changes_to(&self, candidate: &Self) -> Vec<ConfigTreeChange> {
        let mut changes = Vec::new();
        for (path, base_entry) in &self.entries {
            match candidate.entries.get(path) {
                None => changes.push(ConfigTreeChange::Delete {
                    path: path.clone(),
                    expected_digest: base_entry.content_digest.clone(),
                }),
                Some(candidate_entry)
                    if candidate_entry.content_digest != base_entry.content_digest =>
                {
                    changes.push(ConfigTreeChange::Update {
                        path: path.clone(),
                        expected_digest: base_entry.content_digest.clone(),
                        content: candidate_entry.content.clone(),
                    });
                }
                Some(_) => {}
            }
        }
        for (path, entry) in &candidate.entries {
            if !self.entries.contains_key(path) {
                changes.push(ConfigTreeChange::Create {
                    path: path.clone(),
                    content_type: entry.content_type,
                    content: entry.content.clone(),
                });
            }
        }
        changes
    }

    pub fn apply(&self, changes: &[ConfigTreeChange]) -> Result<Self, ConfigTreeError> {
        if changes.len() > MAX_CHANGE_COUNT {
            return Err(ConfigTreeError::LimitExceeded("change count"));
        }
        let mut entries = self.entries.clone();
        let mut touched = BTreeSet::new();
        for change in changes {
            for path in change.paths() {
                if !touched.insert(path.clone()) {
                    return Err(ConfigTreeError::PathChangedMoreThanOnce(path.clone()));
                }
            }
            match change {
                ConfigTreeChange::Create {
                    path,
                    content_type,
                    content,
                } => {
                    if entries.contains_key(path) {
                        return Err(ConfigTreeError::AlreadyExists(path.clone()));
                    }
                    entries.insert(
                        path.clone(),
                        ConfigEntry::new(path.clone(), *content_type, content.clone())?,
                    );
                }
                ConfigTreeChange::Update {
                    path,
                    expected_digest,
                    content,
                } => {
                    let current = entries
                        .get(path)
                        .ok_or_else(|| ConfigTreeError::NotFound(path.clone()))?;
                    if &current.content_digest != expected_digest {
                        return Err(ConfigTreeError::EntryConflict(path.clone()));
                    }
                    entries.insert(
                        path.clone(),
                        ConfigEntry::new(path.clone(), current.content_type, content.clone())?,
                    );
                }
                ConfigTreeChange::Rename {
                    from,
                    to,
                    expected_digest,
                } => {
                    if entries.contains_key(to) {
                        return Err(ConfigTreeError::AlreadyExists(to.clone()));
                    }
                    let current = entries
                        .remove(from)
                        .ok_or_else(|| ConfigTreeError::NotFound(from.clone()))?;
                    if &current.content_digest != expected_digest {
                        return Err(ConfigTreeError::EntryConflict(from.clone()));
                    }
                    entries.insert(
                        to.clone(),
                        ConfigEntry::new(to.clone(), current.content_type, current.content)?,
                    );
                }
                ConfigTreeChange::Delete {
                    path,
                    expected_digest,
                } => {
                    let current = entries
                        .get(path)
                        .ok_or_else(|| ConfigTreeError::NotFound(path.clone()))?;
                    if &current.content_digest != expected_digest {
                        return Err(ConfigTreeError::EntryConflict(path.clone()));
                    }
                    entries.remove(path);
                }
            }
        }
        Self::from_entries(self.revision, entries.into_values())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConfigTreeChange {
    Create {
        path: VirtualPath,
        content_type: ConfigContentType,
        content: String,
    },
    Update {
        path: VirtualPath,
        expected_digest: String,
        content: String,
    },
    Rename {
        from: VirtualPath,
        to: VirtualPath,
        expected_digest: String,
    },
    Delete {
        path: VirtualPath,
        expected_digest: String,
    },
}

impl ConfigTreeChange {
    fn paths(&self) -> Vec<&VirtualPath> {
        match self {
            Self::Create { path, .. } | Self::Update { path, .. } | Self::Delete { path, .. } => {
                vec![path]
            }
            Self::Rename { from, to, .. } => vec![from, to],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ConfigSchemaContribution {
    pub provider_id: String,
    pub namespace: String,
    pub version: String,
    pub source: String,
    pub source_digest: String,
}

impl ConfigSchemaContribution {
    pub fn new(
        provider_id: impl Into<String>,
        namespace: impl Into<String>,
        version: impl Into<String>,
        source: impl Into<String>,
    ) -> Result<Self, ConfigTreeError> {
        let provider_id = provider_id.into();
        let namespace = namespace.into();
        let version = version.into();
        let source = source.into();
        if provider_id.trim().is_empty() {
            return Err(ConfigTreeError::InvalidSchemaContribution(
                "provider_id must not be empty".to_string(),
            ));
        }
        if namespace.trim().is_empty() {
            return Err(ConfigTreeError::InvalidSchemaContribution(
                "namespace must not be empty".to_string(),
            ));
        }
        if version.trim().is_empty() {
            return Err(ConfigTreeError::InvalidSchemaContribution(
                "version must not be empty".to_string(),
            ));
        }
        if source.trim().is_empty() {
            return Err(ConfigTreeError::InvalidSchemaContribution(
                "schema source must not be empty".to_string(),
            ));
        }
        Ok(Self {
            provider_id,
            namespace,
            version,
            source_digest: digest_bytes(source.as_bytes()),
            source,
        })
    }

    fn validate(&self) -> Result<(), ConfigTreeError> {
        let expected = digest_bytes(self.source.as_bytes());
        if self.source_digest != expected {
            return Err(ConfigTreeError::InvalidSchemaContribution(format!(
                "schema contribution {} digest mismatch",
                self.provider_id
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct WorkspaceConfigSchemaBundle {
    pub contributions: Vec<ConfigSchemaContribution>,
    pub source: String,
    pub fingerprint: String,
}

impl WorkspaceConfigSchemaBundle {
    pub fn compose(
        contributions: impl IntoIterator<Item = ConfigSchemaContribution>,
    ) -> Result<Self, ConfigTreeError> {
        let mut contributions = contributions.into_iter().collect::<Vec<_>>();
        contributions.sort_by(|left, right| left.provider_id.cmp(&right.provider_id));
        for contribution in &contributions {
            contribution.validate()?;
        }
        for pair in contributions.windows(2) {
            if pair[0].provider_id == pair[1].provider_id {
                return Err(ConfigTreeError::DuplicateSchemaProvider(
                    pair[0].provider_id.clone(),
                ));
            }
        }
        let mut namespaces = std::collections::BTreeSet::new();
        for contribution in &contributions {
            if !namespaces.insert(contribution.namespace.clone()) {
                return Err(ConfigTreeError::DuplicateSchemaNamespace(
                    contribution.namespace.clone(),
                ));
            }
        }
        let source = if contributions.is_empty() {
            "{}".to_string()
        } else {
            contributions
                .iter()
                .map(|contribution| format!("({})", contribution.source))
                .collect::<Vec<_>>()
                .join(" & ")
        };
        let fingerprint = digest_bytes(
            serde_json::to_vec(&(
                CONFIG_SOURCE_CONTRACT_VERSION,
                DECODAL_VERSION,
                contributions
                    .iter()
                    .map(|contribution| {
                        (
                            contribution.provider_id.as_str(),
                            contribution.namespace.as_str(),
                            contribution.version.as_str(),
                            contribution.source_digest.as_str(),
                        )
                    })
                    .collect::<Vec<_>>(),
                digest_bytes(source.as_bytes()),
            ))
            .expect("schema bundle fingerprint input serializes")
            .as_slice(),
        );
        Ok(Self {
            contributions,
            source,
            fingerprint,
        })
    }

    pub fn empty() -> Self {
        Self::compose(Vec::new()).expect("empty schema bundle is valid")
    }

    pub fn validate(&self) -> Result<(), ConfigTreeError> {
        let recomposed = Self::compose(self.contributions.clone())?;
        if recomposed.source != self.source || recomposed.fingerprint != self.fingerprint {
            return Err(ConfigTreeError::InvalidSchemaContribution(
                "workspace config schema bundle fingerprint mismatch".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
pub struct ToolchainContract {
    pub contract_version: u32,
    pub decodal_version: String,
    pub schema_version: u32,
    pub entrypoints: Vec<VirtualPath>,
    pub import_policy_version: u32,
    pub schema_bundle: WorkspaceConfigSchemaBundle,
    pub fingerprint: String,
}

impl ToolchainContract {
    pub fn new(
        schema_version: u32,
        entrypoints: Vec<VirtualPath>,
        import_policy_version: u32,
    ) -> Self {
        Self::with_schema_bundle(
            schema_version,
            entrypoints,
            import_policy_version,
            WorkspaceConfigSchemaBundle::empty(),
        )
    }

    pub fn with_schema_bundle(
        schema_version: u32,
        mut entrypoints: Vec<VirtualPath>,
        import_policy_version: u32,
        schema_bundle: WorkspaceConfigSchemaBundle,
    ) -> Self {
        entrypoints.sort();
        entrypoints.dedup();
        let mut contract = Self {
            contract_version: CONFIG_SOURCE_CONTRACT_VERSION,
            decodal_version: DECODAL_VERSION.to_string(),
            schema_version,
            entrypoints,
            import_policy_version,
            schema_bundle,
            fingerprint: String::new(),
        };
        contract.fingerprint = digest_bytes(
            serde_json::to_vec(&(
                contract.contract_version,
                &contract.decodal_version,
                contract.schema_version,
                &contract.entrypoints,
                contract.import_policy_version,
                &contract.schema_bundle.fingerprint,
            ))
            .expect("toolchain contract serializes")
            .as_slice(),
        );
        contract
    }

    pub fn validate(&self) -> Result<(), ConfigTreeError> {
        self.schema_bundle.validate()?;
        let expected = Self::with_schema_bundle(
            self.schema_version,
            self.entrypoints.clone(),
            self.import_policy_version,
            self.schema_bundle.clone(),
        );
        if self.contract_version != expected.contract_version
            || self.decodal_version != expected.decodal_version
            || self.fingerprint != expected.fingerprint
        {
            return Err(ConfigTreeError::InvalidSchemaContribution(
                "toolchain contract fingerprint mismatch".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
pub struct ConfigSpan {
    pub start_byte: u32,
    pub end_byte: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
pub struct ConfigDiagnosticLabel {
    pub span: ConfigSpan,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
pub struct ConfigDiagnostic {
    pub path: VirtualPath,
    #[ts(type = "number")]
    pub revision: u64,
    pub tree_digest: String,
    pub kind: String,
    pub span: ConfigSpan,
    pub message: String,
    pub labels: Vec<ConfigDiagnosticLabel>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct EvaluatedProjection {
    pub entrypoint: VirtualPath,
    #[ts(type = "unknown")]
    pub data_json: serde_json::Value,
    pub projection_digest: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
pub struct EvaluationResult {
    pub projections: Vec<EvaluatedProjection>,
    pub projection_digest: String,
}

#[derive(Debug, Clone)]
pub struct SnapshotEnvironment {
    snapshot: ConfigTreeSnapshot,
}

impl SnapshotEnvironment {
    pub fn new(snapshot: ConfigTreeSnapshot) -> Self {
        Self { snapshot }
    }

    pub fn snapshot(&self) -> &ConfigTreeSnapshot {
        &self.snapshot
    }

    pub fn evaluate_contract(
        &self,
        contract: &ToolchainContract,
    ) -> Result<EvaluationResult, Vec<ConfigDiagnostic>> {
        if let Err(error) = contract.validate() {
            return Err(vec![self.config_error(
                VirtualPath::parse(WORKSPACE_CONFIG_SCHEMA_SOURCE).expect("schema path is valid"),
                "schema_contract",
                &error.to_string(),
            )]);
        }
        let service = LanguageService::new(self);
        let diagnostics = self
            .snapshot
            .entries
            .values()
            .filter(|entry| entry.content_type == ConfigContentType::Decodal)
            .flat_map(|entry| {
                service
                    .analyze(entry.path.as_str(), entry.path.as_str(), &entry.content)
                    .diagnostics
                    .iter()
                    .map(|diagnostic| {
                        project_diagnostic(&self.snapshot, entry.path.clone(), diagnostic)
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        if !diagnostics.is_empty() {
            return Err(diagnostics);
        }
        let Some(entrypoint) = contract.entrypoints.first() else {
            return Ok(EvaluationResult {
                projections: Vec::new(),
                projection_digest: digest_bytes(b"[]"),
            });
        };
        if contract.entrypoints.len() != 1 {
            return Err(vec![self.config_error(
                entrypoint.clone(),
                "entrypoint_count",
                "Workspace config evaluation requires exactly one entrypoint",
            )]);
        }
        let Some(entry) = self.snapshot.get(entrypoint) else {
            return Err(vec![self.config_error(
                entrypoint.clone(),
                "entrypoint_missing",
                "configured entrypoint is missing",
            )]);
        };
        if entry.content_type != ConfigContentType::Decodal {
            return Err(vec![self.config_error(
                entrypoint.clone(),
                "entrypoint_not_decodal",
                "configured entrypoint is not Decodal source",
            )]);
        }
        let mut engine = decodal::Engine::new(SnapshotImportLoader {
            snapshot: self.snapshot.clone(),
        });
        let schema_module = engine
            .add_root_source(
                WORKSPACE_CONFIG_SCHEMA_SOURCE,
                WORKSPACE_CONFIG_SCHEMA_SOURCE,
                &contract.schema_bundle.source,
            )
            .map_err(|diagnostic| {
                vec![project_engine_diagnostic(
                    &engine,
                    &self.snapshot,
                    VirtualPath::parse(WORKSPACE_CONFIG_SCHEMA_SOURCE)
                        .expect("schema path is valid"),
                    &diagnostic,
                )]
            })?;
        let schema = engine.eval_module(schema_module).map_err(|diagnostic| {
            vec![project_engine_diagnostic(
                &engine,
                &self.snapshot,
                VirtualPath::parse(WORKSPACE_CONFIG_SCHEMA_SOURCE).expect("schema path is valid"),
                &diagnostic,
            )]
        })?;
        engine.bind_global_runtime(WORKSPACE_CONFIG_SCHEMA_GLOBAL, schema);
        let evaluation_source = if contract.schema_bundle.contributions.is_empty() {
            format!("import \"{}\"", entrypoint.as_str())
        } else {
            WORKSPACE_CONFIG_EVALUATION_SOURCE.replace("__MAIN_ENTRYPOINT__", entrypoint.as_str())
        };
        let evaluation_module = engine
            .add_root_source(
                "workspace-config-evaluation.dcdl",
                "workspace-config-evaluation.dcdl",
                &evaluation_source,
            )
            .map_err(|diagnostic| {
                vec![project_engine_diagnostic(
                    &engine,
                    &self.snapshot,
                    entrypoint.clone(),
                    &diagnostic,
                )]
            })?;
        let value = engine
            .eval_module(evaluation_module)
            .map_err(|diagnostic| {
                vec![project_engine_diagnostic(
                    &engine,
                    &self.snapshot,
                    entrypoint.clone(),
                    &diagnostic,
                )]
            })?;
        let data = engine.materialize(&value).map_err(|diagnostic| {
            vec![project_engine_diagnostic(
                &engine,
                &self.snapshot,
                entrypoint.clone(),
                &diagnostic,
            )]
        })?;
        let data_json = decodal_data_to_json(&data);
        let projection_digest = digest_bytes(
            serde_json::to_vec(&data_json)
                .expect("Decodal projection serializes")
                .as_slice(),
        );
        let projections = vec![EvaluatedProjection {
            entrypoint: entrypoint.clone(),
            data_json,
            projection_digest,
        }];
        let projection_digest = digest_bytes(
            serde_json::to_vec(&projections)
                .expect("projection set serializes")
                .as_slice(),
        );
        Ok(EvaluationResult {
            projections,
            projection_digest,
        })
    }

    pub fn analyze(
        &self,
        entrypoint: &VirtualPath,
        source_override: Option<&str>,
    ) -> Vec<ConfigDiagnostic> {
        let Some(entry) = self.snapshot.get(entrypoint) else {
            return vec![self.config_error(
                entrypoint.clone(),
                "entrypoint_missing",
                "configured entrypoint is missing",
            )];
        };
        let service = LanguageService::new(self);
        service
            .analyze(
                entrypoint.as_str(),
                entrypoint.as_str(),
                source_override.unwrap_or(&entry.content),
            )
            .diagnostics
            .iter()
            .map(|diagnostic| project_diagnostic(&self.snapshot, entrypoint.clone(), diagnostic))
            .collect()
    }

    pub fn complete(
        &self,
        entrypoint: &VirtualPath,
        source: &str,
        utf8_byte_offset: usize,
        explicit: bool,
    ) -> decodal::Result<Option<CompletionResult>> {
        LanguageService::new(self).complete(entrypoint.as_str(), source, utf8_byte_offset, explicit)
    }

    pub fn format(&self, source: &str) -> Result<String, String> {
        decodal_language_tools::format_source(source).map_err(|error| error.to_string())
    }

    fn config_error(
        &self,
        path: VirtualPath,
        kind: impl Into<String>,
        message: impl Into<String>,
    ) -> ConfigDiagnostic {
        ConfigDiagnostic {
            path,
            revision: self.snapshot.revision,
            tree_digest: self.snapshot.digest.clone(),
            kind: kind.into(),
            span: ConfigSpan {
                start_byte: 0,
                end_byte: 0,
            },
            message: message.into(),
            labels: Vec::new(),
            notes: Vec::new(),
        }
    }
}

impl HostEnvironment for &SnapshotEnvironment {
    type Loader = SnapshotImportLoader;

    fn create_loader(&self) -> Self::Loader {
        SnapshotImportLoader {
            snapshot: self.snapshot.clone(),
        }
    }

    fn configure_engine(&self, _engine: &mut Engine<Self::Loader>) -> decodal::Result<()> {
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct SnapshotImportLoader {
    snapshot: ConfigTreeSnapshot,
}

impl SnapshotImportLoader {
    pub fn resolve(
        &self,
        current_key: Option<&str>,
        specifier: &str,
    ) -> Result<VirtualPath, ConfigTreeError> {
        let current = current_key
            .map(VirtualPath::parse)
            .transpose()?
            .ok_or_else(|| ConfigTreeError::InvalidImport(specifier.into()))?;
        resolve_import(&current, specifier)
    }
}

impl ImportLoader for SnapshotImportLoader {
    fn load(
        &mut self,
        current_key: Option<&str>,
        specifier: &str,
    ) -> decodal::Result<LoadedImport> {
        let path = self.resolve(current_key, specifier).map_err(import_error)?;
        let entry = self.snapshot.get(&path).ok_or_else(|| {
            Diagnostic::new(
                DiagnosticKind::Import,
                Span::default(),
                format!("virtual config import is missing: {path}"),
            )
        })?;
        if path.as_str().ends_with(".md") {
            let projection = project_markdown_document(&entry.content).map_err(|message| {
                Diagnostic::new(
                    DiagnosticKind::Import,
                    Span::default(),
                    format!("failed to import Markdown `{path}`: {message}"),
                )
            })?;
            let value = markdown_projection_to_value(projection).map_err(|message| {
                Diagnostic::new(
                    DiagnosticKind::Import,
                    Span::default(),
                    format!("failed to import Markdown `{path}`: {message}"),
                )
            })?;
            return Ok(LoadedImport::value(path.as_str(), value));
        }
        Ok(LoadedImport::source(
            path.as_str(),
            path.as_str(),
            entry.content.clone(),
        ))
    }

    fn complete_import(
        &mut self,
        current_key: Option<&str>,
        prefix: &str,
    ) -> decodal::Result<Vec<ImportCandidate>> {
        let current = current_key
            .map(VirtualPath::parse)
            .transpose()
            .map_err(import_error)?;
        Ok(import_completions(&self.snapshot, current.as_ref(), prefix)
            .into_iter()
            .map(|specifier| ImportCandidate::new(specifier).with_detail("virtual config source"))
            .collect())
    }
}

pub fn resolve_import(
    current: &VirtualPath,
    specifier: &str,
) -> Result<VirtualPath, ConfigTreeError> {
    if specifier.is_empty()
        || specifier.starts_with('/')
        || specifier.contains('\\')
        || specifier.contains('\0')
        || specifier.contains("://")
        || specifier.contains(':')
    {
        return Err(ConfigTreeError::InvalidImport(specifier.into()));
    }
    let mut components = if specifier.starts_with("./") || specifier.starts_with("../") {
        current.parent_components()
    } else {
        Vec::new()
    };
    for component in specifier.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                components
                    .pop()
                    .ok_or_else(|| ConfigTreeError::ImportEscape(specifier.into()))?;
            }
            value => components.push(value),
        }
    }
    VirtualPath::parse(components.join("/"))
}

pub fn import_completions(
    snapshot: &ConfigTreeSnapshot,
    current: Option<&VirtualPath>,
    prefix: &str,
) -> Vec<String> {
    let mut candidates = BTreeSet::new();
    for path in snapshot.entries.keys() {
        if current == Some(path) {
            continue;
        }
        let absolute = path.as_str().to_string();
        if absolute.starts_with(prefix) {
            candidates.insert(absolute);
        }
        if let Some(current) = current {
            let current_parent = current.parent_components();
            let target = path.as_str().split('/').collect::<Vec<_>>();
            let mut common = 0usize;
            while common < current_parent.len()
                && common < target.len()
                && current_parent[common] == target[common]
            {
                common += 1;
            }
            let mut relative = vec![".."; current_parent.len().saturating_sub(common)];
            relative.extend_from_slice(&target[common..]);
            let specifier = if relative.first().is_some_and(|item| *item == "..") {
                relative.join("/")
            } else {
                format!("./{}", relative.join("/"))
            };
            if specifier.starts_with(prefix) {
                candidates.insert(specifier);
            }
        }
    }
    candidates.into_iter().collect()
}

fn project_engine_diagnostic(
    engine: &decodal::Engine<SnapshotImportLoader>,
    snapshot: &ConfigTreeSnapshot,
    fallback_path: VirtualPath,
    diagnostic: &Diagnostic,
) -> ConfigDiagnostic {
    let path = engine
        .source_name(diagnostic.span.source)
        .and_then(|name| VirtualPath::parse(name).ok())
        .filter(|path| snapshot.entries.contains_key(path))
        .unwrap_or(fallback_path);
    ConfigDiagnostic {
        path,
        revision: snapshot.revision,
        tree_digest: snapshot.digest.clone(),
        span: ConfigSpan {
            start_byte: diagnostic.span.start,
            end_byte: diagnostic.span.end,
        },
        kind: format!("{:?}", diagnostic.kind).to_ascii_lowercase(),
        message: diagnostic.message.clone(),
        labels: diagnostic
            .labels
            .iter()
            .map(|label| ConfigDiagnosticLabel {
                span: ConfigSpan {
                    start_byte: label.span.start,
                    end_byte: label.span.end,
                },
                message: label.message.clone(),
            })
            .collect(),
        notes: diagnostic.notes.clone(),
    }
}

fn project_diagnostic(
    snapshot: &ConfigTreeSnapshot,
    fallback_path: VirtualPath,
    diagnostic: &Diagnostic,
) -> ConfigDiagnostic {
    ConfigDiagnostic {
        path: fallback_path,
        revision: snapshot.revision,
        tree_digest: snapshot.digest.clone(),
        kind: diagnostic_kind(diagnostic.kind).to_string(),
        span: ConfigSpan {
            start_byte: diagnostic.span.start,
            end_byte: diagnostic.span.end,
        },
        message: diagnostic.message.clone(),
        labels: diagnostic
            .labels
            .iter()
            .map(|label| ConfigDiagnosticLabel {
                span: ConfigSpan {
                    start_byte: label.span.start,
                    end_byte: label.span.end,
                },
                message: label.message.clone(),
            })
            .collect(),
        notes: diagnostic.notes.clone(),
    }
}

fn diagnostic_kind(kind: DiagnosticKind) -> &'static str {
    match kind {
        DiagnosticKind::Syntax => "syntax",
        DiagnosticKind::UnresolvedIdentifier => "unresolved_identifier",
        DiagnosticKind::TypeMismatch => "type_mismatch",
        DiagnosticKind::ConstraintViolation => "constraint_violation",
        DiagnosticKind::Conflict => "conflict",
        DiagnosticKind::DefaultConflict => "default_conflict",
        DiagnosticKind::Cycle => "cycle",
        DiagnosticKind::Import => "import",
        DiagnosticKind::MatchFailure => "match_failure",
        DiagnosticKind::Materialize => "materialize",
        DiagnosticKind::UnsupportedFeature => "unsupported_feature",
    }
}

fn import_error(error: ConfigTreeError) -> Diagnostic {
    Diagnostic::new(DiagnosticKind::Import, Span::default(), error.to_string())
}

fn decodal_data_to_json(data: &Data) -> serde_json::Value {
    match data {
        Data::Bool(value) => serde_json::Value::Bool(*value),
        Data::Int(value) => serde_json::Value::Number((*value).into()),
        Data::Float(value) => serde_json::Number::from_f64(*value)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Data::String(value) => serde_json::Value::String(value.clone()),
        Data::Array(values) => {
            serde_json::Value::Array(values.iter().map(decodal_data_to_json).collect())
        }
        Data::Object(fields) => serde_json::Value::Object(
            fields
                .iter()
                .map(|field| (field.name.clone(), decodal_data_to_json(&field.value)))
                .collect(),
        ),
    }
}

fn snapshot_digest(entries: &BTreeMap<VirtualPath, ConfigEntry>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"yoi-config-tree-v1\0");
    for (path, entry) in entries {
        hasher.update(path.as_str().as_bytes());
        hasher.update([0]);
        hasher.update(entry.content_type.media_type().as_bytes());
        hasher.update([0]);
        hasher.update(entry.content.as_bytes());
        hasher.update([0]);
    }
    format_digest(hasher.finalize().as_slice())
}

pub fn digest_bytes(bytes: &[u8]) -> String {
    format_digest(Sha256::digest(bytes).as_slice())
}

fn format_digest(bytes: &[u8]) -> String {
    let mut output = String::from("sha256:");
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum ConfigTreeError {
    #[error("invalid virtual config path: {0}")]
    InvalidPath(String),
    #[error("invalid virtual config import: {0}")]
    InvalidImport(String),
    #[error("virtual config import escapes the tree: {0}")]
    ImportEscape(String),
    #[error("virtual config path already exists: {0}")]
    AlreadyExists(VirtualPath),
    #[error("virtual config path was not found: {0}")]
    NotFound(VirtualPath),
    #[error("virtual config entry changed: {0}")]
    EntryConflict(VirtualPath),
    #[error("virtual config path changed more than once in one candidate: {0}")]
    PathChangedMoreThanOnce(VirtualPath),
    #[error("duplicate virtual config path")]
    DuplicatePath,
    #[error("duplicate Workspace config schema provider: {0}")]
    DuplicateSchemaProvider(String),
    #[error("duplicate Workspace config schema namespace owner: {0}")]
    DuplicateSchemaNamespace(String),
    #[error("invalid Workspace config schema contribution: {0}")]
    InvalidSchemaContribution(String),
    #[error("virtual config limit exceeded: {0}")]
    LimitExceeded(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use ts_rs::TS;

    #[test]
    fn exports_typescript_contract() {
        let output = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../web/workspace/src/lib/workspace/config-source/generated/types");
        std::fs::create_dir_all(&output).unwrap();
        macro_rules! export {
            ($type:ty) => {
                <$type>::export_all(&ts_rs::Config::default().with_out_dir(&output)).unwrap();
            };
        }
        export!(VirtualPath);
        export!(ConfigContentType);
        export!(ConfigEntry);
        export!(ConfigTreeSnapshot);
        export!(ConfigTreeChange);
        export!(ConfigSchemaContribution);
        export!(WorkspaceConfigSchemaBundle);
        export!(ToolchainContract);
        export!(ConfigSpan);
        export!(ConfigDiagnosticLabel);
        export!(ConfigDiagnostic);
        export!(EvaluatedProjection);
        export!(EvaluationResult);
    }

    fn path(value: &str) -> VirtualPath {
        VirtualPath::parse(value).unwrap()
    }

    fn entry(path_value: &str, content: &str) -> ConfigEntry {
        ConfigEntry::new(path(path_value), ConfigContentType::Decodal, content).unwrap()
    }

    fn text_entry(path_value: &str, content: &str) -> ConfigEntry {
        ConfigEntry::new(path(path_value), ConfigContentType::Text, content).unwrap()
    }

    #[test]
    fn virtual_paths_reject_ambiguous_or_escaping_forms() {
        for invalid in ["", "/root.dcdl", "a//b", "a/./b", "a/../b", "a\\b", "a\0b"] {
            assert!(VirtualPath::parse(invalid).is_err(), "{invalid:?}");
        }
        assert_eq!(path("profiles/main.dcdl").as_str(), "profiles/main.dcdl");
    }

    #[test]
    fn candidate_changes_are_atomic_ordered_and_conflict_checked() {
        let base = ConfigTreeSnapshot::from_entries(
            7,
            [
                entry("profiles/a.dcdl", "{ a = 1; }"),
                entry("shared.dcdl", "{}"),
            ],
        )
        .unwrap();
        let updated = base
            .apply(&[
                ConfigTreeChange::Update {
                    path: path("profiles/a.dcdl"),
                    expected_digest: base.entries[&path("profiles/a.dcdl")]
                        .content_digest
                        .clone(),
                    content: "{ a = 2; }".into(),
                },
                ConfigTreeChange::Rename {
                    from: path("shared.dcdl"),
                    to: path("lib/shared.dcdl"),
                    expected_digest: base.entries[&path("shared.dcdl")].content_digest.clone(),
                },
            ])
            .unwrap();
        assert_eq!(
            updated
                .entries
                .keys()
                .map(VirtualPath::as_str)
                .collect::<Vec<_>>(),
            ["lib/shared.dcdl", "profiles/a.dcdl"]
        );
        assert_ne!(updated.digest, base.digest);
        assert!(matches!(
            base.apply(&[ConfigTreeChange::Delete {
                path: path("shared.dcdl"),
                expected_digest: "sha256:stale".into(),
            }]),
            Err(ConfigTreeError::EntryConflict(_))
        ));
    }

    #[test]
    fn snapshot_digest_is_deterministic() {
        let left = ConfigTreeSnapshot::from_entries(
            1,
            [entry("z.dcdl", "{}"), entry("a.dcdl", "{ x = 1; }")],
        )
        .unwrap();
        let right = ConfigTreeSnapshot::from_entries(
            99,
            [entry("a.dcdl", "{ x = 1; }"), entry("z.dcdl", "{}")],
        )
        .unwrap();
        assert_eq!(left.digest, right.digest);
    }

    #[test]
    fn relative_imports_and_completion_share_the_snapshot_namespace() {
        let snapshot = ConfigTreeSnapshot::from_entries(
            1,
            [
                entry("profiles/main.dcdl", r#"import "../shared/value.dcdl""#),
                entry("shared/value.dcdl", "{ answer = 42; }"),
                entry("other.dcdl", "{}"),
            ],
        )
        .unwrap();
        assert_eq!(
            resolve_import(&path("profiles/main.dcdl"), "../shared/value.dcdl").unwrap(),
            path("shared/value.dcdl")
        );
        assert!(resolve_import(&path("main.dcdl"), "../escape.dcdl").is_err());
        assert_eq!(
            import_completions(&snapshot, Some(&path("profiles/main.dcdl")), "../sh"),
            ["../shared/value.dcdl"]
        );
    }

    #[test]
    fn schema_bundle_is_order_independent_and_rejects_duplicate_provider() {
        let web = ConfigSchemaContribution::new(
            "builtin:web",
            "web",
            "1",
            "{ web = { enabled = Bool default false; }; }",
        )
        .unwrap();
        let tickets = ConfigSchemaContribution::new(
            "builtin:tickets",
            "tickets",
            "1",
            "{ tickets = { enabled = Bool default true; }; }",
        )
        .unwrap();
        let left = WorkspaceConfigSchemaBundle::compose([web.clone(), tickets.clone()]).unwrap();
        let right = WorkspaceConfigSchemaBundle::compose([tickets, web.clone()]).unwrap();
        assert_eq!(left, right);
        assert!(matches!(
            WorkspaceConfigSchemaBundle::compose([web.clone(), web.clone()]),
            Err(ConfigTreeError::DuplicateSchemaProvider(provider)) if provider == "builtin:web"
        ));
        let conflicting_namespace = ConfigSchemaContribution::new(
            "project:web-extension",
            "web",
            "1",
            "{ web = { extension = true; }; }",
        )
        .unwrap();
        assert!(matches!(
            WorkspaceConfigSchemaBundle::compose([web.clone(), conflicting_namespace]),
            Err(ConfigTreeError::DuplicateSchemaNamespace(namespace)) if namespace == "web"
        ));
    }

    #[test]
    fn workspace_schema_applies_defaults_with_asymmetric_decodal_validation() {
        let snapshot =
            ConfigTreeSnapshot::from_entries(1, [entry("main.dcdl", "{ web = {}; }")]).unwrap();
        let schema = WorkspaceConfigSchemaBundle::compose([ConfigSchemaContribution::new(
            "builtin:web",
            "web",
            "1",
            "{ web = { enabled = Bool default false; }; }",
        )
        .unwrap()])
        .unwrap();
        let contract = ToolchainContract::with_schema_bundle(1, vec![path("main.dcdl")], 1, schema);
        let result = SnapshotEnvironment::new(snapshot)
            .evaluate_contract(&contract)
            .unwrap();
        assert_eq!(result.projections[0].data_json["web"]["enabled"], false);
    }

    #[test]
    fn workspace_schema_rejects_unknown_root_and_nested_fields() {
        let schema = || {
            WorkspaceConfigSchemaBundle::compose([ConfigSchemaContribution::new(
                "builtin:web",
                "web",
                "1",
                "{ web = { enabled = Bool default false; }; }",
            )
            .unwrap()])
            .unwrap()
        };
        for (source, unknown_field) in [
            ("{ web = {}; custom = 42; }", "custom"),
            ("{ web = { typo = true; }; }", "typo"),
        ] {
            let snapshot =
                ConfigTreeSnapshot::from_entries(1, [entry("main.dcdl", source)]).unwrap();
            let diagnostics = SnapshotEnvironment::new(snapshot)
                .evaluate_contract(&ToolchainContract::with_schema_bundle(
                    1,
                    vec![path("main.dcdl")],
                    1,
                    schema(),
                ))
                .unwrap_err();
            assert_eq!(diagnostics[0].path, path("main.dcdl"));
            assert_eq!(diagnostics[0].kind, "constraintviolation");
            assert!(diagnostics[0].message.contains(unknown_field));
            assert!(diagnostics[0].span.end_byte > diagnostics[0].span.start_byte);
        }
    }

    #[test]
    fn workspace_schema_supports_typed_associative_collections() {
        let snapshot = ConfigTreeSnapshot::from_entries(
            1,
            [entry(
                "main.dcdl",
                "{ features = { web = { enabled = true; }; tickets = { enabled = false; }; }; }",
            )],
        )
        .unwrap();
        let schema = WorkspaceConfigSchemaBundle::compose([ConfigSchemaContribution::new(
            "builtin:features",
            "features",
            "1",
            "{ features = {...{ enabled = Bool; }}; }",
        )
        .unwrap()])
        .unwrap();
        let result = SnapshotEnvironment::new(snapshot)
            .evaluate_contract(&ToolchainContract::with_schema_bundle(
                1,
                vec![path("main.dcdl")],
                1,
                schema,
            ))
            .unwrap();
        assert_eq!(
            result.projections[0].data_json["features"]["web"]["enabled"],
            true
        );
        assert_eq!(
            result.projections[0].data_json["features"]["tickets"]["enabled"],
            false
        );
    }

    #[test]
    fn workspace_typed_associative_values_remain_closed_and_typed() {
        let schema = || {
            WorkspaceConfigSchemaBundle::compose([ConfigSchemaContribution::new(
                "builtin:features",
                "features",
                "1",
                "{ features = {...{ enabled = Bool; }}; }",
            )
            .unwrap()])
            .unwrap()
        };
        for (source, expected_kind) in [
            (
                "{ features = { web = { enabled = \"yes\"; }; }; }",
                "constraintviolation",
            ),
            ("{ features = { web = {}; }; }", "materialize"),
            (
                "{ features = { web = { enabled = true; typo = 1; }; }; }",
                "constraintviolation",
            ),
        ] {
            let snapshot =
                ConfigTreeSnapshot::from_entries(1, [entry("main.dcdl", source)]).unwrap();
            let diagnostics = SnapshotEnvironment::new(snapshot)
                .evaluate_contract(&ToolchainContract::with_schema_bundle(
                    1,
                    vec![path("main.dcdl")],
                    1,
                    schema(),
                ))
                .unwrap_err();
            assert_eq!(diagnostics[0].path, path("main.dcdl"));
            assert_eq!(diagnostics[0].kind, expected_kind);
            assert!(diagnostics[0].span.end_byte > diagnostics[0].span.start_byte);
        }
    }

    #[test]
    fn language_service_and_formatter_accept_decodal_0_4_schema_syntax() {
        let source =
            "{} as { features = {...{ enabled = Bool; }}; web = { enabled = Bool; ...Unknown }; }";
        let snapshot = ConfigTreeSnapshot::from_entries(1, [entry("schema.dcdl", source)]).unwrap();
        let environment = SnapshotEnvironment::new(snapshot);
        let diagnostics = environment.analyze(&path("schema.dcdl"), None);
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.kind != "syntax"),
            "{diagnostics:#?}"
        );

        let completion_source = "{} as { web = { enabled = Unk } }";
        let completion = environment
            .complete(
                &path("schema.dcdl"),
                completion_source,
                completion_source.find("Unk").unwrap() + "Unk".len(),
                true,
            )
            .unwrap()
            .expect("explicit completion is available");
        assert!(format!("{completion:?}").contains("Unknown"));

        let formatted = environment.format(source).unwrap();
        assert!(formatted.contains(" as "));
        assert!(formatted.contains("...Unknown"));
        assert!(
            environment
                .analyze(&path("schema.dcdl"), Some(&formatted))
                .iter()
                .all(|diagnostic| diagnostic.kind != "syntax")
        );
    }

    #[test]
    fn workspace_schema_preserves_fields_only_where_rest_is_explicit() {
        let snapshot = ConfigTreeSnapshot::from_entries(
            1,
            [entry(
                "main.dcdl",
                "{ web = { enabled = true; extension_value = 42; }; }",
            )],
        )
        .unwrap();
        let schema = WorkspaceConfigSchemaBundle::compose([ConfigSchemaContribution::new(
            "builtin:web",
            "web",
            "1",
            "{ web = { enabled = Bool; ...Unknown }; }",
        )
        .unwrap()])
        .unwrap();
        let result = SnapshotEnvironment::new(snapshot)
            .evaluate_contract(&ToolchainContract::with_schema_bundle(
                1,
                vec![path("main.dcdl")],
                1,
                schema,
            ))
            .unwrap();
        assert_eq!(
            result.projections[0].data_json["web"]["extension_value"],
            42
        );
    }

    #[test]
    fn unresolved_unknown_cannot_be_materialized() {
        let snapshot =
            ConfigTreeSnapshot::from_entries(1, [entry("main.dcdl", "Unknown")]).unwrap();
        let diagnostics = SnapshotEnvironment::new(snapshot)
            .evaluate_contract(&ToolchainContract::new(1, vec![path("main.dcdl")], 1))
            .unwrap_err();
        assert_eq!(diagnostics[0].path, path("main.dcdl"));
        assert!(!diagnostics[0].message.is_empty());
    }

    #[test]
    fn workspace_schema_type_mismatch_is_a_decodal_diagnostic() {
        let snapshot = ConfigTreeSnapshot::from_entries(
            1,
            [entry("main.dcdl", "{ web = { enabled = 1; }; }")],
        )
        .unwrap();
        let schema = WorkspaceConfigSchemaBundle::compose([ConfigSchemaContribution::new(
            "builtin:web",
            "web",
            "1",
            "{ web = { enabled = Bool default false; }; }",
        )
        .unwrap()])
        .unwrap();
        let diagnostics = SnapshotEnvironment::new(snapshot)
            .evaluate_contract(&ToolchainContract::with_schema_bundle(
                1,
                vec![path("main.dcdl")],
                1,
                schema,
            ))
            .unwrap_err();
        assert_eq!(diagnostics[0].kind, "constraintviolation");
        assert!(!diagnostics[0].message.is_empty());
    }

    #[test]
    fn schema_bundle_changes_toolchain_fingerprint() {
        let empty = ToolchainContract::new(1, vec![path("main.dcdl")], 1);
        let schema = WorkspaceConfigSchemaBundle::compose([ConfigSchemaContribution::new(
            "builtin:web",
            "web",
            "1",
            "{ web = {}; }",
        )
        .unwrap()])
        .unwrap();
        let configured =
            ToolchainContract::with_schema_bundle(1, vec![path("main.dcdl")], 1, schema);
        assert_ne!(empty.fingerprint, configured.fingerprint);
    }

    #[test]
    fn markdown_import_projects_frontmatter_and_content_as_a_value() {
        let markdown = concat!(
            "---\n",
            "name: debug-rust\n",
            "description: Debug Rust failures\n",
            "custom-authority: no\n",
            "allowed-tools: Read Grep\n",
            "metadata:\n  owner: platform\n",
            "---\n",
            "# Debug Rust\n",
        );
        let snapshot = ConfigTreeSnapshot::from_entries(
            3,
            [
                entry(
                    "main.dcdl",
                    r#"{ skill = import "./skills/debug-rust/SKILL.md" as { frontmatter = { name = String; description = String; ...Unknown }; content = String; }; }"#,
                ),
                text_entry("skills/debug-rust/SKILL.md", markdown),
            ],
        )
        .unwrap();
        let result = SnapshotEnvironment::new(snapshot.clone())
            .evaluate_contract(&ToolchainContract::new(1, vec![path("main.dcdl")], 1))
            .unwrap();
        let skill = &result.projections[0].data_json["skill"];
        assert_eq!(skill["frontmatter"]["name"], "debug-rust");
        assert_eq!(skill["frontmatter"]["custom-authority"], "no");
        assert_eq!(skill["frontmatter"]["allowed-tools"], "Read Grep");
        assert_eq!(skill["frontmatter"]["metadata"]["owner"], "platform");
        assert_eq!(skill["content"], "# Debug Rust\n");
        assert_eq!(
            snapshot.entries[&path("skills/debug-rust/SKILL.md")].content_digest,
            digest_bytes(markdown.as_bytes())
        );
    }

    #[test]
    fn markdown_import_without_frontmatter_preserves_the_whole_body() {
        let source = "# Plain skill\nKeep --- inside the body.\n";
        assert_eq!(
            project_markdown_document(source).unwrap(),
            MarkdownDocumentProjection {
                frontmatter: serde_json::Map::new(),
                content: source.to_string(),
            }
        );
        let snapshot = ConfigTreeSnapshot::from_entries(
            1,
            [
                entry("main.dcdl", r#"import "./skills/plain/SKILL.md""#),
                text_entry("skills/plain/SKILL.md", source),
            ],
        )
        .unwrap();
        let result = SnapshotEnvironment::new(snapshot)
            .evaluate_contract(&ToolchainContract::new(1, vec![path("main.dcdl")], 1))
            .unwrap();
        assert_eq!(
            result.projections[0].data_json["frontmatter"],
            serde_json::json!({})
        );
        assert_eq!(result.projections[0].data_json["content"], source);
    }

    #[test]
    fn malformed_markdown_frontmatter_is_an_import_diagnostic() {
        assert_eq!(
            project_markdown_document("---\nname: missing-close\nbody\n").unwrap_err(),
            "opening YAML frontmatter delimiter has no closing delimiter"
        );
        let snapshot = ConfigTreeSnapshot::from_entries(
            1,
            [
                entry("main.dcdl", r#"import "./skills/broken/SKILL.md""#),
                text_entry(
                    "skills/broken/SKILL.md",
                    "---\nname: [unterminated\n---\nbody\n",
                ),
            ],
        )
        .unwrap();
        let diagnostics = SnapshotEnvironment::new(snapshot)
            .evaluate_contract(&ToolchainContract::new(1, vec![path("main.dcdl")], 1))
            .unwrap_err();
        assert_eq!(diagnostics[0].kind, "import");
        assert!(diagnostics[0].message.contains("invalid YAML frontmatter"));
        assert!(diagnostics[0].message.contains("skills/broken/SKILL.md"));
    }

    #[test]
    fn host_environment_evaluation_uses_only_snapshot_imports() {
        let snapshot = ConfigTreeSnapshot::from_entries(
            3,
            [
                entry("profiles/main.dcdl", r#"import "./shared.dcdl""#),
                entry("profiles/shared.dcdl", "{ answer = 42; }"),
            ],
        )
        .unwrap();
        let contract = ToolchainContract::new(
            DEFAULT_SCHEMA_VERSION,
            vec![path("profiles/main.dcdl")],
            DEFAULT_IMPORT_POLICY_VERSION,
        );
        let result = SnapshotEnvironment::new(snapshot)
            .evaluate_contract(&contract)
            .unwrap();
        assert_eq!(result.projections[0].data_json["answer"], 42);
    }

    #[test]
    fn candidate_evaluation_rejects_invalid_unreferenced_decodal_source() {
        let snapshot = ConfigTreeSnapshot::from_entries(
            1,
            [
                entry("workspace.dcdl", "{ answer = 42; }"),
                entry("unused.dcdl", "{ broken = ; }"),
            ],
        )
        .unwrap();
        let diagnostics = SnapshotEnvironment::new(snapshot)
            .evaluate_contract(&ToolchainContract::new(1, vec![path("workspace.dcdl")], 1))
            .unwrap_err();
        assert_eq!(diagnostics[0].path, path("unused.dcdl"));
        assert_eq!(diagnostics[0].kind, "syntax");
    }

    #[test]
    fn missing_import_and_cycles_are_structured_failures() {
        let missing =
            ConfigTreeSnapshot::from_entries(1, [entry("main.dcdl", r#"import "./missing.dcdl""#)])
                .unwrap();
        let contract = ToolchainContract::new(1, vec![path("main.dcdl")], 1);
        let diagnostics = SnapshotEnvironment::new(missing)
            .evaluate_contract(&contract)
            .unwrap_err();
        assert_eq!(diagnostics[0].kind, "import");

        let cycle = ConfigTreeSnapshot::from_entries(
            1,
            [
                entry("a.dcdl", r#"import "./b.dcdl""#),
                entry("b.dcdl", r#"import "./a.dcdl""#),
            ],
        )
        .unwrap();
        let diagnostics = SnapshotEnvironment::new(cycle)
            .evaluate_contract(&ToolchainContract::new(1, vec![path("a.dcdl")], 1))
            .unwrap_err();
        assert_eq!(diagnostics[0].kind, "cycle");
    }
}
