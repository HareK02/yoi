use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use worker_runtime::config_bundle::{
    ConfigBundle, ConfigBundleMetadata, ConfigBundleProvenance, ConfigProfileDescriptor,
};
use worker_runtime::profile_archive::{ProfileSourceArchive, ProfileSourceArchiveInput};

use crate::hosts::{DiagnosticSeverity, RuntimeDiagnostic};
use crate::{Error, Result};

const PROFILE_REGISTRY_RELATIVE_PATH: &str = ".yoi/profiles.toml";
const PROFILE_SOURCE_ROOT_RELATIVE_PATH: &str = ".yoi/profiles";
const MAX_PROFILE_SOURCE_BYTES: u64 = 256 * 1024;
const BUILTIN_PROFILE_IDS: &[&str] = &[
    "builtin:default",
    "builtin:companion",
    "builtin:intake",
    "builtin:orchestrator",
    "builtin:coder",
    "builtin:reviewer",
];
const BUILTIN_PROFILE_SLUGS: &[&str] = &[
    "default",
    "companion",
    "intake",
    "orchestrator",
    "coder",
    "reviewer",
];

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
    pub editable: bool,
    pub revision: String,
    pub size_bytes: u64,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceProfileSourceDetailResponse {
    pub workspace_id: String,
    pub profile: WorkspaceProfileSummary,
    pub source: WorkspaceProfileSourceSummary,
    pub content: String,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateWorkspaceProfileSourceRequest {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub content: String,
    pub registry_revision: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateWorkspaceProfileRegistryRequest {
    pub registry_revision: String,
    #[serde(default)]
    pub default_profile: Option<String>,
    pub profiles: Vec<WorkspaceProfileRegistryEntryUpdate>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceProfileRegistryEntryUpdate {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub profile_source_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateWorkspaceProfileSourceRequest {
    pub content: String,
    pub revision: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteWorkspaceProfileSourceRequest {
    pub registry_revision: String,
    pub source_revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileSettingsMutationResponse {
    pub workspace_id: String,
    pub settings: ProfileSettingsResponse,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceIdentityFile {
    workspace_id: String,
    created_at: String,
    display_name: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileRegistryDocument {
    #[serde(default)]
    default: Option<String>,
    #[serde(default)]
    profile: BTreeMap<String, ProfileEntryFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum ProfileEntryFile {
    Path(String),
    Table(ProfileEntryTable),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileEntryTable {
    path: String,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Clone)]
struct ProjectProfileEntry {
    name: String,
    description: Option<String>,
    relative_path: PathBuf,
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
                format!("Workspace identity could not be read: {}", sanitize_error(&err.to_string())),
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

pub fn load_profile_settings(workspace_id: &str, workspace_root: &Path) -> ProfileSettingsResponse {
    let mut diagnostics = Vec::new();
    let registry = match read_registry(workspace_root) {
        Ok(registry) => registry,
        Err(err) => {
            diagnostics.push(diagnostic(
                "profile_registry_schema_invalid",
                DiagnosticSeverity::Error,
                err,
            ));
            ProfileRegistryDocument::default()
        }
    };
    let registry_revision = file_revision(&registry_path(workspace_root));
    let mut profiles = builtin_profile_summaries(registry.default.as_deref());
    let mut sources = Vec::new();
    let mut seen_selectors = BTreeSet::new();
    for profile in &profiles {
        seen_selectors.insert(profile.selector.clone());
    }
    for entry in project_entries(&registry, &mut diagnostics) {
        let selector = project_selector(&entry.name);
        let source_id = project_source_id(&entry.name);
        let mut entry_diagnostics = Vec::new();
        if !seen_selectors.insert(selector.clone()) {
            entry_diagnostics.push(diagnostic(
                "profile_selector_duplicate",
                DiagnosticSeverity::Error,
                format!("Profile selector '{selector}' is duplicated."),
            ));
        }
        let source_summary = summarize_source(workspace_root, &source_id, &entry.relative_path);
        entry_diagnostics.extend(source_summary.diagnostics.clone());
        profiles.push(WorkspaceProfileSummary {
            profile_id: selector.clone(),
            selector,
            label: entry.name.clone(),
            source_kind: "project".to_string(),
            profile_source_id: Some(source_id.clone()),
            description: entry.description.clone(),
            editable: true,
            is_default: registry.default.as_deref() == Some(project_selector(&entry.name).as_str()),
            diagnostics: entry_diagnostics,
        });
        sources.push(source_summary);
    }
    diagnostics.extend(validate_project_profiles(workspace_root, &registry));
    ProfileSettingsResponse {
        workspace_id: workspace_id.to_string(),
        registry_revision,
        default_profile: registry.default,
        profiles,
        sources,
        diagnostics,
    }
}

pub fn read_profile_source(
    workspace_id: &str,
    workspace_root: &Path,
    source_id: &str,
) -> Result<WorkspaceProfileSourceDetailResponse> {
    let registry = read_registry(workspace_root).map_err(Error::Config)?;
    let (name, entry) = entry_for_source_id(&registry, source_id)?;
    let full = checked_source_path(workspace_root, &entry.relative_path)?;
    let metadata = source_metadata(&full)?;
    if metadata.len() > MAX_PROFILE_SOURCE_BYTES {
        return Err(Error::RuntimeOperationFailed {
            runtime_id: "workspace-backend".to_string(),
            code: "profile_source_too_large".to_string(),
            message: "Profile source is too large for browser editing".to_string(),
        });
    }
    let content = fs::read_to_string(&full)?;
    let source = summarize_source(workspace_root, source_id, &entry.relative_path);
    let selector = project_selector(&name);
    let profile = WorkspaceProfileSummary {
        profile_id: selector.clone(),
        selector: selector.clone(),
        label: name,
        source_kind: "project".to_string(),
        profile_source_id: Some(source_id.to_string()),
        description: entry.description,
        editable: true,
        is_default: registry.default.as_deref() == Some(selector.as_str()),
        diagnostics: source.diagnostics.clone(),
    };
    Ok(WorkspaceProfileSourceDetailResponse {
        workspace_id: workspace_id.to_string(),
        profile,
        source,
        content,
        diagnostics: Vec::new(),
    })
}

pub fn create_profile_source(
    workspace_id: &str,
    workspace_root: &Path,
    request: CreateWorkspaceProfileSourceRequest,
) -> Result<ProfileSettingsMutationResponse> {
    let registry_path = registry_path(workspace_root);
    ensure_revision(&registry_path, &request.registry_revision, "profile_registry_revision_conflict")?;
    let mut registry = read_registry(workspace_root).map_err(Error::Config)?;
    let name = validate_profile_name(&request.name)?;
    if registry.profile.contains_key(&name) || BUILTIN_PROFILE_SLUGS.contains(&name.as_str()) {
        return Err(Error::RuntimeOperationFailed {
            runtime_id: "workspace-backend".to_string(),
            code: "profile_selector_duplicate".to_string(),
            message: "Profile selector already exists".to_string(),
        });
    }
    let relative_path = PathBuf::from(".yoi").join("profiles").join(format!("{name}.dcdl"));
    validate_source_content(workspace_root, &name, &relative_path, &request.content)?;
    let full = checked_source_path(workspace_root, &relative_path)?;
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&full, request.content)?;
    registry.profile.insert(
        name.clone(),
        ProfileEntryFile::Table(ProfileEntryTable {
            path: format!("profiles/{name}.dcdl"),
            description: request.description.and_then(|value| optional_trim(value.as_str())),
        }),
    );
    write_registry(workspace_root, &registry)?;
    Ok(ProfileSettingsMutationResponse {
        workspace_id: workspace_id.to_string(),
        settings: load_profile_settings(workspace_id, workspace_root),
        diagnostics: vec![diagnostic(
            "profile_settings_updated",
            DiagnosticSeverity::Info,
            "Profile source was created and profile discovery was refreshed.",
        )],
    })
}

pub fn update_profile_registry(
    workspace_id: &str,
    workspace_root: &Path,
    request: UpdateWorkspaceProfileRegistryRequest,
) -> Result<ProfileSettingsMutationResponse> {
    let path = registry_path(workspace_root);
    ensure_revision(&path, &request.registry_revision, "profile_registry_revision_conflict")?;
    validate_default_profile(request.default_profile.as_deref())?;
    let mut profile = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for update in request.profiles {
        let name = validate_profile_name(&update.name)?;
        if !seen.insert(name.clone()) || BUILTIN_PROFILE_SLUGS.contains(&name.as_str()) {
            return Err(Error::RuntimeOperationFailed {
                runtime_id: "workspace-backend".to_string(),
                code: "profile_selector_duplicate".to_string(),
                message: "Profile selector duplicate in registry update".to_string(),
            });
        }
        let source_id = update
            .profile_source_id
            .as_deref()
            .unwrap_or(project_source_id(&name).as_str())
            .to_string();
        let (source_name, _) = parse_project_source_id(&source_id)?;
        if source_name != name {
            return Err(Error::RuntimeOperationFailed {
                runtime_id: "workspace-backend".to_string(),
                code: "profile_source_id_mismatch".to_string(),
                message: "Profile source id must match its registry selector".to_string(),
            });
        }
        profile.insert(
            name.clone(),
            ProfileEntryFile::Table(ProfileEntryTable {
                path: format!("profiles/{name}.dcdl"),
                description: update.description.and_then(|value| optional_trim(value.as_str())),
            }),
        );
    }
    let registry = ProfileRegistryDocument {
        default: request.default_profile,
        profile,
    };
    for entry in project_entries(&registry, &mut Vec::new()) {
        let full = checked_source_path(workspace_root, &entry.relative_path)?;
        if full.exists() {
            let content = fs::read_to_string(&full)?;
            validate_source_content(workspace_root, &entry.name, &entry.relative_path, &content)?;
        }
    }
    write_registry(workspace_root, &registry)?;
    Ok(ProfileSettingsMutationResponse {
        workspace_id: workspace_id.to_string(),
        settings: load_profile_settings(workspace_id, workspace_root),
        diagnostics: vec![diagnostic(
            "profile_registry_updated",
            DiagnosticSeverity::Info,
            "Profile registry was updated and profile discovery was refreshed.",
        )],
    })
}

pub fn update_profile_source(
    workspace_id: &str,
    workspace_root: &Path,
    source_id: &str,
    request: UpdateWorkspaceProfileSourceRequest,
) -> Result<ProfileSettingsMutationResponse> {
    let registry = read_registry(workspace_root).map_err(Error::Config)?;
    let (name, entry) = entry_for_source_id(&registry, source_id)?;
    let full = checked_source_path(workspace_root, &entry.relative_path)?;
    ensure_revision(&full, &request.revision, "profile_source_revision_conflict")?;
    validate_source_content(workspace_root, &name, &entry.relative_path, &request.content)?;
    fs::write(&full, request.content)?;
    Ok(ProfileSettingsMutationResponse {
        workspace_id: workspace_id.to_string(),
        settings: load_profile_settings(workspace_id, workspace_root),
        diagnostics: vec![diagnostic(
            "profile_source_updated",
            DiagnosticSeverity::Info,
            "Profile source was updated and profile discovery was refreshed.",
        )],
    })
}

pub fn delete_profile_source(
    workspace_id: &str,
    workspace_root: &Path,
    source_id: &str,
    request: DeleteWorkspaceProfileSourceRequest,
) -> Result<ProfileSettingsMutationResponse> {
    let registry_path = registry_path(workspace_root);
    ensure_revision(&registry_path, &request.registry_revision, "profile_registry_revision_conflict")?;
    let mut registry = read_registry(workspace_root).map_err(Error::Config)?;
    let (name, entry) = entry_for_source_id(&registry, source_id)?;
    let full = checked_source_path(workspace_root, &entry.relative_path)?;
    ensure_revision(&full, &request.source_revision, "profile_source_revision_conflict")?;
    registry.profile.remove(&name);
    if registry.default.as_deref() == Some(project_selector(&name).as_str()) {
        registry.default = None;
    }
    if full.exists() {
        fs::remove_file(&full)?;
    }
    write_registry(workspace_root, &registry)?;
    Ok(ProfileSettingsMutationResponse {
        workspace_id: workspace_id.to_string(),
        settings: load_profile_settings(workspace_id, workspace_root),
        diagnostics: vec![diagnostic(
            "profile_source_deleted",
            DiagnosticSeverity::Info,
            "Profile source and registry entry were deleted and profile discovery was refreshed.",
        )],
    })
}

pub fn build_workspace_profile_archive(
    workspace_root: &Path,
    selector: &str,
) -> Result<Option<ProfileSourceArchive>> {
    if !selector.starts_with("project:") {
        return Ok(None);
    }
    let registry = read_registry(workspace_root).map_err(Error::Config)?;
    let mut entrypoints = BTreeMap::new();
    let mut sources = BTreeMap::new();
    for entry in project_entries(&registry, &mut Vec::new()) {
        let path = archive_path_for_entry(&entry.name);
        let full = checked_source_path(workspace_root, &entry.relative_path)?;
        let content = fs::read_to_string(&full)?;
        sources.insert(path.clone(), content);
        entrypoints.insert(project_selector(&entry.name), path);
    }
    if !entrypoints.contains_key(selector) {
        return Err(Error::RuntimeOperationFailed {
            runtime_id: "workspace-backend".to_string(),
            code: "unknown_profile_selector".to_string(),
            message: "Selected project profile is not present in the workspace profile registry".to_string(),
        });
    }
    if let Some(default) = registry.default.as_deref().filter(|value| value.starts_with("project:")) {
        if let Some(path) = entrypoints.get(default).cloned() {
            entrypoints.insert("default".to_string(), path);
        }
    }
    let archive = ProfileSourceArchive::build(ProfileSourceArchiveInput {
        id: "workspace-project-decodal-profiles-v1".to_string(),
        entrypoints,
        imports: BTreeMap::new(),
        sources,
    })
    .map_err(|err| Error::RuntimeOperationFailed {
        runtime_id: "workspace-backend".to_string(),
        code: "profile_source_archive_invalid".to_string(),
        message: err.to_string(),
    })?;
    archive.verify().and_then(|verified| {
        verified
            .resolve_profile(selector, workspace_root, "workspace-settings-validation")
            .map(|_| ())
    }).map_err(|err| Error::RuntimeOperationFailed {
        runtime_id: "workspace-backend".to_string(),
        code: "profile_source_invalid".to_string(),
        message: err.to_string(),
    })?;
    Ok(Some(archive))
}

pub fn build_workspace_profile_config_bundle(
    workspace_root: &Path,
    workspace_id: &str,
    workspace_created_at: &str,
    selector: &str,
) -> Result<Option<ConfigBundle>> {
    let Some(archive) = build_workspace_profile_archive(workspace_root, selector)? else {
        return Ok(None);
    };
    let bundle = ConfigBundle {
        metadata: ConfigBundleMetadata {
            id: "workspace-project-profile-settings-v1".to_string(),
            digest: String::new(),
            revision: file_revision(&registry_path(workspace_root)),
            workspace_id: workspace_id.to_string(),
            created_at: workspace_created_at.to_string(),
            provenance: ConfigBundleProvenance {
                source: "workspace_profile_settings".to_string(),
                detail: Some("workspace Decodal profile registry".to_string()),
            },
        },
        profiles: vec![ConfigProfileDescriptor {
            selector: worker_runtime::catalog::ProfileSelector::Named(selector.to_string()),
            label: Some(selector.to_string()),
        }],
        declarations: Vec::new(),
        profile_source_archive: Some(archive),
        profile_source_archive_handle: None,
    }
    .with_computed_digest();
    Ok(Some(bundle))
}

pub fn project_profile_candidates(workspace_root: &Path) -> Vec<WorkspaceProfileSummary> {
    load_profile_settings("workspace", workspace_root)
        .profiles
        .into_iter()
        .filter(|profile| profile.source_kind == "project")
        .collect()
}

pub fn is_profile_candidate(workspace_root: &Path, profile_id: &str) -> bool {
    BUILTIN_PROFILE_IDS.contains(&profile_id)
        || profile_id == "runtime_default"
        || project_profile_candidates(workspace_root)
            .into_iter()
            .any(|profile| profile.profile_id == profile_id)
}

fn builtin_profile_summaries(default_profile: Option<&str>) -> Vec<WorkspaceProfileSummary> {
    let labels = [
        ("builtin:default", "Default", "Bundled default Yoi profile"),
        ("builtin:companion", "Companion", "Bundled Companion role profile"),
        ("builtin:intake", "Intake", "Bundled Intake role profile"),
        ("builtin:orchestrator", "Orchestrator", "Bundled Orchestrator role profile"),
        ("builtin:coder", "Coder", "Bundled Coder role profile"),
        ("builtin:reviewer", "Reviewer", "Bundled Reviewer role profile"),
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

fn validate_project_profiles(workspace_root: &Path, registry: &ProfileRegistryDocument) -> Vec<RuntimeDiagnostic> {
    let mut diagnostics = Vec::new();
    for entry in project_entries(registry, &mut diagnostics) {
        let full = match checked_source_path(workspace_root, &entry.relative_path) {
            Ok(path) => path,
            Err(err) => {
                diagnostics.push(diagnostic(
                    "profile_source_path_escape",
                    DiagnosticSeverity::Error,
                    err.to_string(),
                ));
                continue;
            }
        };
        match fs::read_to_string(&full) {
            Ok(content) => {
                if let Err(err) = validate_source_content(workspace_root, &entry.name, &entry.relative_path, &content) {
                    diagnostics.push(diagnostic(
                        "profile_source_invalid",
                        DiagnosticSeverity::Error,
                        sanitize_error(&err.to_string()),
                    ));
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => diagnostics.push(diagnostic(
                "profile_source_missing",
                DiagnosticSeverity::Error,
                format!("Profile source '{}' is missing.", entry.name),
            )),
            Err(err) => diagnostics.push(diagnostic(
                "profile_source_read_failed",
                DiagnosticSeverity::Error,
                sanitize_error(&err.to_string()),
            )),
        }
    }
    if let Some(default) = registry.default.as_deref() {
        if let Err(err) = validate_default_profile(Some(default)) {
            diagnostics.push(diagnostic(
                "profile_default_invalid",
                DiagnosticSeverity::Error,
                sanitize_error(&err.to_string()),
            ));
        }
    }
    diagnostics
}

fn validate_source_content(
    workspace_root: &Path,
    name: &str,
    relative_path: &Path,
    content: &str,
) -> Result<()> {
    if content.as_bytes().len() as u64 > MAX_PROFILE_SOURCE_BYTES {
        return Err(Error::RuntimeOperationFailed {
            runtime_id: "workspace-backend".to_string(),
            code: "profile_source_too_large".to_string(),
            message: "Profile source exceeds the browser editing size limit".to_string(),
        });
    }
    let archive_path = archive_path_for_entry(name);
    let selector = project_selector(name);
    let mut entrypoints = BTreeMap::new();
    entrypoints.insert(selector.clone(), archive_path.clone());
    entrypoints.insert("default".to_string(), archive_path.clone());
    let mut sources = BTreeMap::new();
    sources.insert(archive_path, content.to_string());
    let archive = ProfileSourceArchive::build(ProfileSourceArchiveInput {
        id: format!("workspace-profile-validation-{name}"),
        entrypoints,
        imports: BTreeMap::new(),
        sources,
    })
    .map_err(|err| Error::RuntimeOperationFailed {
        runtime_id: "workspace-backend".to_string(),
        code: "profile_source_archive_invalid".to_string(),
        message: err.to_string(),
    })?;
    let verified = archive.verify().map_err(|err| Error::RuntimeOperationFailed {
        runtime_id: "workspace-backend".to_string(),
        code: "profile_source_archive_invalid".to_string(),
        message: err.to_string(),
    })?;
    checked_source_path(workspace_root, relative_path)?;
    verified
        .resolve_profile(&selector, workspace_root, "workspace-settings-validation")
        .map_err(|err| Error::RuntimeOperationFailed {
            runtime_id: "workspace-backend".to_string(),
            code: "profile_source_syntax_invalid".to_string(),
            message: err.to_string(),
        })?;
    Ok(())
}

fn summarize_source(
    workspace_root: &Path,
    source_id: &str,
    relative_path: &Path,
) -> WorkspaceProfileSourceSummary {
    let mut diagnostics = Vec::new();
    let display_path = display_source_path(relative_path);
    let mut revision = "missing".to_string();
    let mut size_bytes = 0;
    match checked_source_path(workspace_root, relative_path) {
        Ok(full) => match source_metadata(&full) {
            Ok(metadata) => {
                size_bytes = metadata.len();
                revision = file_revision(&full);
                if size_bytes > MAX_PROFILE_SOURCE_BYTES {
                    diagnostics.push(diagnostic(
                        "profile_source_too_large",
                        DiagnosticSeverity::Error,
                        "Profile source is too large for browser editing.",
                    ));
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => diagnostics.push(diagnostic(
                "profile_source_missing",
                DiagnosticSeverity::Error,
                "Profile source file is missing.",
            )),
            Err(err) => diagnostics.push(diagnostic(
                "profile_source_metadata_failed",
                DiagnosticSeverity::Error,
                sanitize_error(&err.to_string()),
            )),
        },
        Err(err) => diagnostics.push(diagnostic(
            "profile_source_path_escape",
            DiagnosticSeverity::Error,
            sanitize_error(&err.to_string()),
        )),
    }
    WorkspaceProfileSourceSummary {
        profile_source_id: source_id.to_string(),
        display_path,
        kind: "decodal".to_string(),
        editable: diagnostics.iter().all(|d| d.severity != DiagnosticSeverity::Error),
        revision,
        size_bytes,
        diagnostics,
    }
}

fn read_registry(workspace_root: &Path) -> std::result::Result<ProfileRegistryDocument, String> {
    let path = registry_path(workspace_root);
    match fs::read_to_string(&path) {
        Ok(raw) => toml::from_str(&raw).map_err(|err| format!("invalid profile registry schema: {err}")),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(ProfileRegistryDocument::default()),
        Err(err) => Err(format!("failed to read profile registry: {}", sanitize_error(&err.to_string()))),
    }
}

fn write_registry(workspace_root: &Path, registry: &ProfileRegistryDocument) -> Result<()> {
    let path = registry_path(workspace_root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let raw = toml::to_string_pretty(registry)
        .map_err(|err| Error::Config(format!("failed to serialize profile registry: {err}")))?;
    fs::write(path, raw)?;
    Ok(())
}

fn project_entries(registry: &ProfileRegistryDocument, diagnostics: &mut Vec<RuntimeDiagnostic>) -> Vec<ProjectProfileEntry> {
    registry
        .profile
        .iter()
        .filter_map(|(name, entry)| match validate_profile_name(name) {
            Ok(name) => {
                if BUILTIN_PROFILE_SLUGS.contains(&name.as_str()) {
                    diagnostics.push(diagnostic(
                        "profile_selector_duplicate",
                        DiagnosticSeverity::Error,
                        format!("Project profile '{name}' conflicts with a builtin selector."),
                    ));
                }
                let (path, description) = match entry {
                    ProfileEntryFile::Path(path) => (path.clone(), None),
                    ProfileEntryFile::Table(table) => (table.path.clone(), table.description.clone()),
                };
                match registry_relative_source_path(&path) {
                    Ok(relative_path) => Some(ProjectProfileEntry {
                        name,
                        description,
                        relative_path,
                    }),
                    Err(err) => {
                        diagnostics.push(diagnostic(
                            "profile_source_path_escape",
                            DiagnosticSeverity::Error,
                            err,
                        ));
                        None
                    }
                }
            }
            Err(err) => {
                diagnostics.push(diagnostic(
                    "profile_selector_invalid",
                    DiagnosticSeverity::Error,
                    err.to_string(),
                ));
                None
            }
        })
        .collect()
}

fn entry_for_source_id(registry: &ProfileRegistryDocument, source_id: &str) -> Result<(String, ProjectProfileEntry)> {
    let (name, _) = parse_project_source_id(source_id)?;
    let mut diagnostics = Vec::new();
    let entry = project_entries(registry, &mut diagnostics)
        .into_iter()
        .find(|entry| entry.name == name)
        .ok_or_else(|| Error::RuntimeOperationFailed {
            runtime_id: "workspace-backend".to_string(),
            code: "unknown_profile_source".to_string(),
            message: "Unknown profile source id".to_string(),
        })?;
    Ok((name, entry))
}

fn registry_relative_source_path(raw: &str) -> std::result::Result<PathBuf, String> {
    let path = Path::new(raw);
    if path.is_absolute() {
        return Err("Profile source path must be workspace-relative and safe.".to_string());
    }
    let path = if path.starts_with(".yoi") {
        path.to_path_buf()
    } else {
        PathBuf::from(".yoi").join(path)
    };
    validate_relative_source_path(&path)?;
    Ok(path)
}

fn validate_relative_source_path(path: &Path) -> std::result::Result<(), String> {
    if !path.starts_with(PROFILE_SOURCE_ROOT_RELATIVE_PATH) {
        return Err("Profile source path must be under the workspace profile source root.".to_string());
    }
    for component in path.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err("Profile source path must not contain absolute, parent, or prefix components.".to_string());
        }
    }
    if path.extension().and_then(|value| value.to_str()) != Some("dcdl") {
        return Err("Profile source path must use the .dcdl extension.".to_string());
    }
    Ok(())
}

fn checked_source_path(workspace_root: &Path, relative_path: &Path) -> Result<PathBuf> {
    validate_relative_source_path(relative_path).map_err(|message| Error::RuntimeOperationFailed {
        runtime_id: "workspace-backend".to_string(),
        code: "profile_source_path_escape".to_string(),
        message,
    })?;
    let source_root = workspace_root.join(PROFILE_SOURCE_ROOT_RELATIVE_PATH);
    fs::create_dir_all(&source_root)?;
    let canonical_root = fs::canonicalize(&source_root)?;
    let full = workspace_root.join(relative_path);
    if let Ok(canonical_full) = fs::canonicalize(&full) {
        if !canonical_full.starts_with(&canonical_root) {
            return Err(Error::RuntimeOperationFailed {
                runtime_id: "workspace-backend".to_string(),
                code: "profile_source_symlink_escape".to_string(),
                message: "Profile source resolves outside the workspace profile source root".to_string(),
            });
        }
    } else if let Some(parent) = full.parent() {
        let canonical_parent = fs::canonicalize(parent)?;
        if !canonical_parent.starts_with(&canonical_root) {
            return Err(Error::RuntimeOperationFailed {
                runtime_id: "workspace-backend".to_string(),
                code: "profile_source_path_escape".to_string(),
                message: "Profile source parent resolves outside the workspace profile source root".to_string(),
            });
        }
    }
    Ok(full)
}

fn validate_default_profile(value: Option<&str>) -> Result<()> {
    if let Some(value) = value {
        if BUILTIN_PROFILE_IDS.contains(&value) || value.starts_with("project:") {
            return Ok(());
        }
        return Err(Error::RuntimeOperationFailed {
            runtime_id: "workspace-backend".to_string(),
            code: "profile_default_invalid".to_string(),
            message: "Default profile must be a Backend-published builtin or project selector".to_string(),
        });
    }
    Ok(())
}

fn validate_profile_name(value: &str) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.len() > 64
        || !trimmed
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
    {
        return Err(Error::RuntimeOperationFailed {
            runtime_id: "workspace-backend".to_string(),
            code: "profile_selector_invalid".to_string(),
            message: "Profile selector must contain only ASCII letters, digits, '-' or '_'".to_string(),
        });
    }
    Ok(trimmed.to_string())
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

fn parse_project_source_id(source_id: &str) -> Result<(String, String)> {
    let Some(name) = source_id.strip_prefix("project:") else {
        return Err(Error::RuntimeOperationFailed {
            runtime_id: "workspace-backend".to_string(),
            code: "unsupported_profile_source_id".to_string(),
            message: "Profile source id is not a project profile source".to_string(),
        });
    };
    let name = validate_profile_name(name)?;
    Ok((name.clone(), project_source_id(&name)))
}

pub fn project_selector(name: &str) -> String {
    format!("project:{name}")
}

pub fn project_source_id(name: &str) -> String {
    format!("project:{name}")
}

pub fn selector_for_builtin_candidate(id: &str) -> Option<worker_runtime::catalog::ProfileSelector> {
    match id {
        "runtime_default" => Some(worker_runtime::catalog::ProfileSelector::RuntimeDefault),
        "builtin:default" | "builtin:companion" | "builtin:intake" | "builtin:orchestrator"
        | "builtin:coder" | "builtin:reviewer" => {
            Some(worker_runtime::catalog::ProfileSelector::Builtin(id.to_string()))
        }
        value if value.starts_with("project:") => {
            Some(worker_runtime::catalog::ProfileSelector::Named(value.to_string()))
        }
        _ => None,
    }
}

fn archive_path_for_entry(name: &str) -> String {
    format!("profiles/{name}.dcdl")
}

fn display_source_path(relative_path: &Path) -> String {
    relative_path
        .strip_prefix(".yoi")
        .unwrap_or(relative_path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn registry_path(workspace_root: &Path) -> PathBuf {
    workspace_root.join(PROFILE_REGISTRY_RELATIVE_PATH)
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

fn source_metadata(path: &Path) -> std::io::Result<std::fs::Metadata> {
    fs::symlink_metadata(path)
}

fn ensure_revision(path: &Path, expected: &str, code: &'static str) -> Result<()> {
    let actual = file_revision(path);
    if expected != actual {
        return Err(Error::RuntimeOperationFailed {
            runtime_id: "workspace-backend".to_string(),
            code: code.to_string(),
            message: "Settings changed before this update was applied".to_string(),
        });
    }
    Ok(())
}

fn optional_trim(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
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
            if token.starts_with('/') || token.contains("/.yoi/") || token.contains(".yoi/sessions") {
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
        format!(
            r#"{{
                slug = "{slug}";
                description = "Test";
                scope = "workspace_read";
            }}"#
        )
    }

    #[test]
    fn profile_settings_create_update_and_discover_project_profile() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".yoi")).unwrap();
        fs::write(dir.path().join(".yoi/profiles.toml"), "").unwrap();
        let revision = file_revision(&dir.path().join(".yoi/profiles.toml"));
        let created = create_profile_source(
            "workspace-test",
            dir.path(),
            CreateWorkspaceProfileSourceRequest {
                name: "alpha".to_string(),
                description: Some("Alpha".to_string()),
                content: valid_decodal("alpha"),
                registry_revision: revision,
            },
        )
        .unwrap();
        assert!(created
            .settings
            .profiles
            .iter()
            .any(|profile| profile.profile_id == "project:alpha"));
        assert!(build_workspace_profile_archive(dir.path(), "project:alpha")
            .unwrap()
            .is_some());
    }

    #[test]
    fn profile_source_rejects_path_escape_and_revision_conflict() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".yoi")).unwrap();
        fs::write(
            dir.path().join(".yoi/profiles.toml"),
            "[profile.bad]\npath = \"../bad.dcdl\"\n",
        )
        .unwrap();
        let settings = load_profile_settings("workspace-test", dir.path());
        assert!(settings
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "profile_source_path_escape"));

        let err = update_profile_registry(
            "workspace-test",
            dir.path(),
            UpdateWorkspaceProfileRegistryRequest {
                registry_revision: "stale".to_string(),
                default_profile: None,
                profiles: Vec::new(),
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("profile_registry_revision_conflict"));
    }

    #[test]
    fn profile_source_rejects_invalid_decodal() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".yoi")).unwrap();
        fs::write(dir.path().join(".yoi/profiles.toml"), "").unwrap();
        let revision = file_revision(&dir.path().join(".yoi/profiles.toml"));
        let err = create_profile_source(
            "workspace-test",
            dir.path(),
            CreateWorkspaceProfileSourceRequest {
                name: "bad".to_string(),
                description: None,
                content: "not decodal".to_string(),
                registry_revision: revision,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("profile_source_syntax_invalid") || err.to_string().contains("profile_source_archive_invalid"));
    }
}
