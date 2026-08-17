use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use config_source::{ConfigContentType, ConfigSchemaContribution, VirtualPath};
use manifest::{ProfileSource, resolve_profile_artifact_value};
use serde::{Deserialize, Serialize};
use worker_runtime::config_bundle::{
    ConfigBundle, ConfigBundleMetadata, ConfigBundleProvenance, ConfigProfileDescriptor,
};
use worker_runtime::profile_archive::{ProfileSourceArchive, ProfileSourceArchiveInput};

use crate::config_source::{
    WorkspaceConfigSchemaProvider, WorkspaceConfigState, evaluate_workspace_config_state,
};
use crate::hosts::{DiagnosticSeverity, RuntimeDiagnostic};
use crate::{Error, Result};

const PROFILE_SCHEMA_SOURCE: &str = r#"{
    profile = {
        default_profile = String default "builtin:companion";
        entries = [...{
            selector = String;
            source = String;
            label = String default "";
            description = String default "";
        }] default [];
    };
}"#;

#[derive(Debug, Default)]
pub struct ProfileConfigSchemaProvider;

impl WorkspaceConfigSchemaProvider for ProfileConfigSchemaProvider {
    fn contribution(&self) -> Result<ConfigSchemaContribution> {
        ConfigSchemaContribution::new("builtin:profile", "profile", "1", PROFILE_SCHEMA_SOURCE)
            .map_err(|error| Error::Config(error.to_string()))
    }
}

#[derive(Debug, Clone, Deserialize)]
struct VirtualProfileConfig {
    profile: VirtualProfileSection,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct VirtualProfileSection {
    default_profile: String,
    entries: Vec<VirtualProfileEntry>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct VirtualProfileEntry {
    selector: String,
    source: String,
    label: String,
    description: String,
}

#[derive(Debug, Clone)]
pub struct ProfileConfigProjection {
    pub settings: ProfileSettingsResponse,
    entries: BTreeMap<String, VirtualProfileEntry>,
    sources: BTreeMap<String, String>,
}

pub fn project_profiles_from_workspace_config(
    workspace_id: &str,
    state: &WorkspaceConfigState,
) -> Result<ProfileConfigProjection> {
    let bundle = if state.contract.schema_bundle.contributions.is_empty() {
        config_source::WorkspaceConfigSchemaBundle::compose([
            ProfileConfigSchemaProvider.contribution()?
        ])
        .map_err(|error| Error::Config(error.to_string()))?
    } else {
        state.contract.schema_bundle.clone()
    };
    let evaluation = evaluate_workspace_config_state(state, bundle)?;
    if evaluation.projection_digest != state.projection_digest
        && state
            .contract
            .schema_bundle
            .contributions
            .iter()
            .any(|entry| entry.provider_id == "builtin:profile")
    {
        return Err(Error::RegistryInconsistency(format!(
            "Profile projection digest mismatch for Workspace {workspace_id}"
        )));
    }
    let projected = evaluation.projections.first().ok_or_else(|| {
        Error::RegistryInconsistency("Workspace config has no active projection".to_string())
    })?;
    let config: VirtualProfileConfig = serde_json::from_value(projected.data_json.clone())
        .map_err(|error| Error::RegistryInconsistency(error.to_string()))?;
    let mut profiles = builtin_profile_summaries(Some(&config.profile.default_profile));
    let mut entries = BTreeMap::new();
    let sources = state
        .snapshot
        .entries
        .iter()
        .filter(|(_, entry)| entry.content_type == ConfigContentType::Decodal)
        .map(|(path, entry)| (path.as_str().to_string(), entry.content.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut source_summaries = Vec::new();
    for entry in config.profile.entries {
        if !entry.selector.starts_with("project:") {
            return Err(profile_validation_error(
                "profile_selector_invalid",
                "Workspace Profile selectors must use project:*",
            ));
        }
        if entries.contains_key(&entry.selector) {
            return Err(profile_validation_error(
                "profile_selector_duplicate",
                "Workspace Profile selectors must be unique",
            ));
        }
        let source_path = VirtualPath::parse(&entry.source).map_err(|error| {
            profile_validation_error("profile_source_path_invalid", &error.to_string())
        })?;
        let source_entry = state.snapshot.get(&source_path).ok_or_else(|| {
            profile_validation_error(
                "profile_source_missing",
                &format!(
                    "Profile source {:?} is missing from the active config revision",
                    entry.source
                ),
            )
        })?;
        if source_entry.content_type != ConfigContentType::Decodal {
            return Err(profile_validation_error(
                "profile_source_type_invalid",
                "Profile sources must use Decodal content",
            ));
        }
        let archive_source = entry.source.clone();
        let label = if entry.label.is_empty() {
            entry.selector.trim_start_matches("project:").to_string()
        } else {
            entry.label.clone()
        };
        resolve_profile_artifact_value(
            evaluate_profile_source(&state.snapshot, &source_path)?,
            ProfileSource::Archive {
                archive_id: format!("workspace-config-r{}", state.snapshot.revision),
                source: archive_source.clone(),
            },
            Path::new("/"),
            "workspace-config-validation",
        )
        .map_err(|error| profile_validation_error("profile_source_invalid", &error.to_string()))?;
        profiles.push(WorkspaceProfileSummary {
            profile_id: entry.selector.clone(),
            selector: entry.selector.clone(),
            label,
            source_kind: "project".to_string(),
            profile_source_id: Some(entry.source.clone()),
            description: (!entry.description.is_empty()).then(|| entry.description.clone()),
            editable: false,
            is_default: config.profile.default_profile == entry.selector,
            diagnostics: Vec::new(),
        });
        source_summaries.push(WorkspaceProfileSourceSummary {
            profile_source_id: entry.source.clone(),
            display_path: entry.source.clone(),
            kind: "virtual_config".to_string(),
            content_type: "decodal".to_string(),
            content_digest: source_entry.content_digest.clone(),
            provenance: WorkspaceProfileSourceProvenance::ProjectProfileSourceTree,
            editable: false,
            revision: state.snapshot.revision.to_string(),
            size_bytes: source_entry.content.len() as u64,
            diagnostics: Vec::new(),
        });
        entries.insert(entry.selector.clone(), entry);
    }
    if !profiles
        .iter()
        .any(|profile| profile.selector == config.profile.default_profile)
    {
        return Err(profile_validation_error(
            "unknown_default_profile",
            "Default Profile must select a builtin or Workspace Profile",
        ));
    }
    Ok(ProfileConfigProjection {
        settings: ProfileSettingsResponse {
            workspace_id: workspace_id.to_string(),
            registry_revision: format!("config:{}", state.snapshot.revision),
            config_revision: Some(state.snapshot.revision),
            tree_digest: Some(state.snapshot.digest.clone()),
            projection_digest: Some(evaluation.projection_digest),
            default_profile: Some(config.profile.default_profile),
            profiles,
            sources: source_summaries,
            diagnostics: Vec::new(),
        },
        entries,
        sources,
    })
}

fn evaluate_profile_source(
    snapshot: &config_source::ConfigTreeSnapshot,
    source_path: &VirtualPath,
) -> Result<serde_json::Value> {
    let contract = config_source::ToolchainContract::with_schema_bundle(
        config_source::DEFAULT_SCHEMA_VERSION,
        vec![source_path.clone()],
        config_source::DEFAULT_IMPORT_POLICY_VERSION,
        config_source::WorkspaceConfigSchemaBundle::empty(),
    );
    let evaluation = config_source::SnapshotEnvironment::new(snapshot.clone())
        .evaluate_contract(&contract)
        .map_err(|diagnostics| {
            profile_validation_error(
                "profile_source_invalid",
                &serde_json::to_string(&diagnostics)
                    .unwrap_or_else(|_| "Profile source evaluation failed".to_string()),
            )
        })?;
    evaluation
        .projections
        .into_iter()
        .next()
        .map(|projection| projection.data_json)
        .ok_or_else(|| {
            profile_validation_error("profile_source_invalid", "Profile source has no projection")
        })
}

pub fn selector_for_workspace_candidate(
    projection: &ProfileConfigProjection,
    profile: &str,
) -> Option<worker_runtime::catalog::ProfileSelector> {
    if let Some(selector) = selector_for_builtin_candidate(profile) {
        return Some(selector);
    }
    projection
        .entries
        .contains_key(profile)
        .then(|| worker_runtime::catalog::ProfileSelector::Named(profile.to_string()))
}

pub fn build_virtual_profile_config_bundle(
    projection: &ProfileConfigProjection,
    state: &WorkspaceConfigState,
    workspace_id: &str,
    workspace_created_at: &str,
    selector: &str,
) -> Result<Option<ConfigBundle>> {
    let archive = projection
        .entries
        .get(selector)
        .map(|entry| build_virtual_profile_archive(selector, entry, &projection.sources, state))
        .transpose()?;
    let profile_selector = selector_for_builtin_candidate(selector)
        .unwrap_or_else(|| worker_runtime::catalog::ProfileSelector::Named(selector.to_string()));
    let bundle = ConfigBundle {
        metadata: ConfigBundleMetadata {
            id: format!("workspace-config-profile-r{}", state.snapshot.revision),
            digest: String::new(),
            revision: state.snapshot.revision.to_string(),
            workspace_id: workspace_id.to_string(),
            created_at: workspace_created_at.to_string(),
            provenance: ConfigBundleProvenance {
                source: "workspace_config".to_string(),
                detail: Some(format!(
                    "revision={} tree={} projection={}",
                    state.snapshot.revision, state.snapshot.digest, state.projection_digest
                )),
            },
        },
        profiles: vec![ConfigProfileDescriptor {
            selector: profile_selector,
            label: Some(selector.to_string()),
        }],
        declarations: Vec::new(),
        prompt_catalog: Some(crate::prompt_settings::project_prompts_from_workspace_config(state)?),
        profile_source_archive: archive,
        profile_source_archive_handle: None,
    }
    .with_computed_digest();
    Ok(Some(bundle))
}

fn build_virtual_profile_archive(
    selector: &str,
    entry: &VirtualProfileEntry,
    all_sources: &BTreeMap<String, String>,
    state: &WorkspaceConfigState,
) -> Result<ProfileSourceArchive> {
    let mut closure = BTreeMap::new();
    let mut imports = BTreeMap::new();
    collect_profile_import_closure(&entry.source, all_sources, &mut closure, &mut imports)?;
    ProfileSourceArchive::build(ProfileSourceArchiveInput {
        id: format!("workspace-config-profile-r{}", state.snapshot.revision),
        entrypoints: BTreeMap::from([(selector.to_string(), entry.source.clone())]),
        imports,
        sources: closure,
    })
    .map_err(|error| profile_validation_error("profile_source_archive_invalid", &error.to_string()))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceMetadataSettingsResponse {
    pub workspace_id: String,
    pub display_name: String,
    pub created_at: String,
    pub revision: String,
    pub source: String,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateWorkspaceMetadataRequest {
    pub display_name: String,
    pub revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceMetadataMutationResponse {
    pub workspace: WorkspaceMetadataSettingsResponse,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileSettingsResponse {
    pub workspace_id: String,
    pub registry_revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_profile: Option<String>,
    pub profiles: Vec<WorkspaceProfileSummary>,
    pub sources: Vec<WorkspaceProfileSourceSummary>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceProfileSummary {
    pub profile_id: String,
    pub selector: String,
    pub label: String,
    pub source_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_source_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub editable: bool,
    pub is_default: bool,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceProfileSourceSummary {
    pub profile_source_id: String,
    pub display_path: String,
    pub kind: String,
    pub content_type: String,
    pub content_digest: String,
    pub provenance: WorkspaceProfileSourceProvenance,
    pub editable: bool,
    pub revision: String,
    pub size_bytes: u64,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceProfileSourceProvenance {
    ProjectProfileSourceTree,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceIdentityFile {
    workspace_id: String,
    created_at: String,
    display_name: String,
}

pub fn workspace_metadata_settings(
    workspace_root: &Path,
    fallback_workspace_id: &str,
    fallback_created_at: &str,
    fallback_display_name: &str,
) -> WorkspaceMetadataSettingsResponse {
    let path = workspace_root.join(crate::identity::WORKSPACE_IDENTITY_RELATIVE_PATH);
    let mut diagnostics = Vec::new();
    let (workspace_id, created_at, display_name) = match fs::read_to_string(&path) {
        Ok(raw) => match toml::from_str::<WorkspaceIdentityFile>(&raw) {
            Ok(file) => (file.workspace_id, file.created_at, file.display_name),
            Err(err) => {
                diagnostics.push(diagnostic(
                    "workspace_identity_parse_failed",
                    DiagnosticSeverity::Error,
                    format!("Workspace identity could not be parsed: {err}"),
                ));
                (
                    fallback_workspace_id.to_string(),
                    fallback_created_at.to_string(),
                    fallback_display_name.to_string(),
                )
            }
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            diagnostics.push(diagnostic(
                "workspace_identity_missing",
                DiagnosticSeverity::Warning,
                "Workspace identity record is missing; showing active backend metadata.",
            ));
            (
                fallback_workspace_id.to_string(),
                fallback_created_at.to_string(),
                fallback_display_name.to_string(),
            )
        }
        Err(err) => {
            diagnostics.push(diagnostic(
                "workspace_identity_read_failed",
                DiagnosticSeverity::Error,
                format!(
                    "Workspace identity could not be read: {}",
                    sanitize_error(&err.to_string())
                ),
            ));
            (
                fallback_workspace_id.to_string(),
                fallback_created_at.to_string(),
                fallback_display_name.to_string(),
            )
        }
    };
    WorkspaceMetadataSettingsResponse {
        workspace_id,
        display_name,
        created_at,
        revision: file_revision(&path),
        source: "workspace_identity".to_string(),
        diagnostics,
    }
}

pub fn update_workspace_metadata(
    workspace_root: &Path,
    request: UpdateWorkspaceMetadataRequest,
) -> Result<WorkspaceMetadataSettingsResponse> {
    let path = workspace_root.join(crate::identity::WORKSPACE_IDENTITY_RELATIVE_PATH);
    let current_revision = file_revision(&path);
    if request.revision != current_revision {
        return Err(Error::RuntimeOperationFailed {
            runtime_id: "workspace-backend".to_string(),
            code: "workspace_metadata_revision_conflict".to_string(),
            message: "Workspace metadata changed before this update was applied".to_string(),
        });
    }
    let raw = fs::read_to_string(&path)?;
    let mut file: WorkspaceIdentityFile = toml::from_str(&raw)
        .map_err(|err| Error::Config(format!("failed to parse workspace identity: {err}")))?;
    let display_name = sanitize_display_name(&request.display_name)?;
    file.display_name = display_name;
    let encoded = toml::to_string_pretty(&file)
        .map_err(|err| Error::Config(format!("failed to serialize workspace identity: {err}")))?;
    fs::write(&path, encoded)?;
    Ok(workspace_metadata_settings(
        workspace_root,
        &file.workspace_id,
        &file.created_at,
        &file.display_name,
    ))
}

fn builtin_profile_summaries(default_profile: Option<&str>) -> Vec<WorkspaceProfileSummary> {
    let labels = [
        (
            "builtin:companion",
            "Companion",
            "Bundled Companion role profile",
        ),
        ("builtin:intake", "Intake", "Bundled Intake role profile"),
        (
            "builtin:orchestrator",
            "Orchestrator",
            "Bundled Orchestrator role profile",
        ),
        ("builtin:coder", "Coder", "Bundled Coder role profile"),
        (
            "builtin:reviewer",
            "Reviewer",
            "Bundled Reviewer role profile",
        ),
    ];
    labels
        .into_iter()
        .map(|(id, label, description)| WorkspaceProfileSummary {
            profile_id: id.to_string(),
            selector: id.to_string(),
            label: label.to_string(),
            source_kind: "builtin".to_string(),
            profile_source_id: None,
            description: Some(description.to_string()),
            editable: false,
            is_default: default_profile == Some(id),
            diagnostics: Vec::new(),
        })
        .collect()
}

fn profile_validation_error(code: impl Into<String>, message: impl Into<String>) -> Error {
    Error::RuntimeOperationFailed {
        runtime_id: "workspace-backend".to_string(),
        code: code.into(),
        message: message.into(),
    }
}

fn collect_profile_import_closure(
    current_path: &str,
    all_sources: &BTreeMap<String, String>,
    closure_sources: &mut BTreeMap<String, String>,
    imports: &mut BTreeMap<String, String>,
) -> Result<()> {
    if closure_sources.contains_key(current_path) {
        return Ok(());
    }
    let content = all_sources.get(current_path).ok_or_else(|| {
        profile_validation_error(
            "profile_source_import_missing",
            &format!("Profile source import closure is missing {current_path}"),
        )
    })?;
    closure_sources.insert(current_path.to_string(), content.clone());
    for specifier in collect_decodal_import_specifiers(content) {
        let target = resolve_profile_source_import(current_path, &specifier)?;
        if !all_sources.contains_key(&target) {
            return Err(profile_validation_error(
                "profile_source_import_missing",
                &format!(
                    "Profile source import {specifier:?} from {current_path} resolves to missing {target}"
                ),
            ));
        }
        imports.insert(format!("{current_path}\0{specifier}"), target.clone());
        collect_profile_import_closure(&target, all_sources, closure_sources, imports)?;
    }
    Ok(())
}

fn resolve_profile_source_import(current_path: &str, specifier: &str) -> Result<String> {
    if specifier.is_empty() || specifier.contains("://") || Path::new(specifier).is_absolute() {
        return Err(profile_validation_error(
            "profile_source_import_invalid",
            "Profile source import must be a virtual relative path",
        ));
    }
    let raw = specifier
        .strip_prefix("project:")
        .or_else(|| specifier.strip_prefix("workspace:"))
        .unwrap_or(specifier);
    if specifier.contains(':') && raw == specifier {
        return Err(profile_validation_error(
            "profile_source_import_invalid",
            "Unsupported profile source import namespace",
        ));
    }
    let base = if raw.starts_with("profiles/") || raw.starts_with("./profiles/") {
        PathBuf::from(raw)
    } else {
        Path::new(current_path)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default()
            .join(raw)
    };
    let normalized = normalize_virtual_profile_source_path(&base.to_string_lossy())?;
    Ok(normalized)
}

fn normalize_virtual_profile_source_path(path: &str) -> Result<String> {
    let mut normalized = PathBuf::new();
    for component in Path::new(path).components() {
        match component {
            Component::CurDir => {}
            Component::Normal(value) => normalized.push(value),
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(profile_validation_error(
                        "profile_source_import_invalid",
                        "Profile source import escapes the virtual tree",
                    ));
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(profile_validation_error(
                    "profile_source_import_invalid",
                    "Profile source import must be relative",
                ));
            }
        }
    }
    Ok(normalized.to_string_lossy().replace('\\', "/"))
}

fn collect_decodal_import_specifiers(content: &str) -> Vec<String> {
    let mut specifiers = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with('#') {
            continue;
        }
        let Some(index) = trimmed.find("import") else {
            continue;
        };
        let after = trimmed[index + "import".len()..].trim_start();
        let Some(first) = after.chars().next() else {
            continue;
        };
        if first == '"' || first == '\'' {
            if let Some(end) = after[1..].find(first) {
                specifiers.push(after[1..1 + end].to_string());
            }
        } else {
            let ident: String = after
                .chars()
                .take_while(|ch| !ch.is_whitespace() && *ch != ';' && *ch != ',' && *ch != '{')
                .collect();
            if !ident.is_empty() {
                specifiers.push(ident);
            }
        }
    }
    specifiers
}

fn sanitize_display_name(value: &str) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_control) || trimmed.len() > 120 {
        return Err(Error::RuntimeOperationFailed {
            runtime_id: "workspace-backend".to_string(),
            code: "workspace_display_name_invalid".to_string(),
            message: "Workspace display name must be non-empty, bounded, and must not contain control characters".to_string(),
        });
    }
    Ok(trimmed.to_string())
}
pub fn selector_for_builtin_candidate(
    id: &str,
) -> Option<worker_runtime::catalog::ProfileSelector> {
    match id {
        "builtin:companion"
        | "builtin:intake"
        | "builtin:orchestrator"
        | "builtin:coder"
        | "builtin:reviewer" => Some(worker_runtime::catalog::ProfileSelector::Builtin(
            id.to_string(),
        )),
        _ => None,
    }
}
fn file_revision(path: &Path) -> String {
    let Ok(metadata) = fs::metadata(path) else {
        return "missing".to_string();
    };
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("rev:{modified}:{}", metadata.len())
}
fn diagnostic(
    code: impl Into<String>,
    severity: DiagnosticSeverity,
    message: impl Into<String>,
) -> RuntimeDiagnostic {
    RuntimeDiagnostic {
        code: code.into(),
        severity,
        message: message.into(),
    }
}
fn sanitize_error(value: &str) -> String {
    value
        .split_whitespace()
        .map(|token| {
            if token.starts_with('/') || token.contains("/.yoi/") || token.contains(".yoi/sessions")
            {
                "<redacted-path>"
            } else {
                token
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_decodal(slug: &str) -> String {
        format!(r#"{{ slug = "{slug}"; model = {{ id = "gpt-5.4"; }}; }}"#)
    }

    fn virtual_state(entries: Vec<config_source::ConfigEntry>) -> WorkspaceConfigState {
        let snapshot = config_source::ConfigTreeSnapshot::from_entries(7, entries).unwrap();
        let schema_bundle = config_source::WorkspaceConfigSchemaBundle::compose([
            ProfileConfigSchemaProvider.contribution().unwrap(),
            crate::prompt_settings::PromptConfigSchemaProvider
                .contribution()
                .unwrap(),
        ])
        .unwrap();
        let contract = config_source::ToolchainContract::with_schema_bundle(
            config_source::DEFAULT_SCHEMA_VERSION,
            vec![VirtualPath::parse("main.dcdl").unwrap()],
            config_source::DEFAULT_IMPORT_POLICY_VERSION,
            schema_bundle,
        );
        let projection_digest = config_source::SnapshotEnvironment::new(snapshot.clone())
            .evaluate_contract(&contract)
            .unwrap()
            .projection_digest;
        WorkspaceConfigState {
            projection_digest,
            contract,
            snapshot,
        }
    }

    #[test]
    fn virtual_config_projection_builds_archive_from_active_revision() {
        let state = virtual_state(vec![
            config_source::ConfigEntry::new(
                VirtualPath::parse("main.dcdl").unwrap(),
                ConfigContentType::Decodal,
                r#"{ profile = { default_profile = "project:alpha"; entries = [{ selector = "project:alpha"; source = "profiles/alpha.dcdl"; label = "Alpha"; }]; }; }"#,
            )
            .unwrap(),
            config_source::ConfigEntry::new(
                VirtualPath::parse("profiles/alpha.dcdl").unwrap(),
                ConfigContentType::Decodal,
                valid_decodal("alpha"),
            )
            .unwrap(),
        ]);
        let projection = project_profiles_from_workspace_config("workspace-test", &state).unwrap();
        let bundle = build_virtual_profile_config_bundle(
            &projection,
            &state,
            "workspace-test",
            "2026-01-01T00:00:00Z",
            "project:alpha",
        )
        .unwrap()
        .unwrap();
        assert_eq!(projection.settings.config_revision, Some(7));
        let prompt_catalog = bundle.prompt_catalog.as_ref().unwrap();
        assert_eq!(prompt_catalog.config_revision, 7);
        assert!(!prompt_catalog.templates.is_empty());
        assert!(
            bundle
                .metadata
                .provenance
                .detail
                .unwrap()
                .contains("revision=7")
        );
        let archive = bundle.profile_source_archive.unwrap();
        assert_eq!(
            archive
                .reference
                .source_graph
                .entrypoints
                .get("project:alpha")
                .map(String::as_str),
            Some("profiles/alpha.dcdl")
        );
        assert_eq!(archive.reference.source_graph.source_count, 1);
    }

    #[test]
    fn virtual_config_projection_preserves_import_closure() {
        let state = virtual_state(vec![
            config_source::ConfigEntry::new(
                VirtualPath::parse("main.dcdl").unwrap(),
                ConfigContentType::Decodal,
                r#"{ profile = { entries = [{ selector = "project:alpha"; source = "profiles/alpha.dcdl"; }]; }; }"#,
            )
            .unwrap(),
            config_source::ConfigEntry::new(
                VirtualPath::parse("profiles/alpha.dcdl").unwrap(),
                ConfigContentType::Decodal,
                r#"import "../shared/profile.dcdl""#,
            )
            .unwrap(),
            config_source::ConfigEntry::new(
                VirtualPath::parse("shared/profile.dcdl").unwrap(),
                ConfigContentType::Decodal,
                valid_decodal("alpha"),
            )
            .unwrap(),
        ]);
        let projection = project_profiles_from_workspace_config("workspace-test", &state).unwrap();
        let bundle = build_virtual_profile_config_bundle(
            &projection,
            &state,
            "workspace-test",
            "2026-01-01T00:00:00Z",
            "project:alpha",
        )
        .unwrap()
        .unwrap();
        let archive = bundle.profile_source_archive.unwrap();
        assert_eq!(archive.reference.source_graph.source_count, 2);
        assert_eq!(archive.reference.source_graph.import_count, 1);
    }

    #[test]
    fn virtual_config_projection_rejects_missing_profile_source() {
        let state = virtual_state(vec![
            config_source::ConfigEntry::new(
                VirtualPath::parse("main.dcdl").unwrap(),
                ConfigContentType::Decodal,
                r#"{ profile = { entries = [{ selector = "project:alpha"; source = "profiles/missing.dcdl"; }]; }; }"#,
            )
            .unwrap(),
        ]);
        let error = project_profiles_from_workspace_config("workspace-test", &state).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("missing from the active config revision")
        );
    }
}
