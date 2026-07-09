use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use worker_runtime::config_bundle::{
    ConfigBundle, ConfigBundleMetadata, ConfigBundleProvenance, ConfigProfileDescriptor,
};
use worker_runtime::profile_archive::{ProfileSourceArchive, ProfileSourceArchiveInput};

use crate::hosts::{DiagnosticSeverity, RuntimeDiagnostic};
use crate::{Error, Result};

const PROFILE_REGISTRY_RELATIVE_PATH: &str = ".yoi/profiles.toml";
const PROFILE_SOURCE_ROOT_RELATIVE_PATH: &str = ".yoi/profiles";
const PROFILE_SOURCE_TREE_ID: &str = "project";
const PROFILE_SOURCE_TREE_DISPLAY_ROOT: &str = "profiles";
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
    #[serde(default)]
    pub source_trees: Vec<WorkspaceProfileSourceTreeSummary>,
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
pub struct WorkspaceProfileSourceTreeSummary {
    pub source_tree_id: String,
    pub label: String,
    pub root_path: String,
    pub kind: String,
    pub content_type: String,
    pub content_digest: String,
    pub provenance: WorkspaceProfileSourceProvenance,
    pub editable: bool,
    pub revision: String,
    pub file_count: usize,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceProfileSourceTreeFileSummary {
    pub path: String,
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
pub struct WorkspaceProfileSourceTreeResponse {
    pub workspace_id: String,
    pub tree: WorkspaceProfileSourceTreeSummary,
    pub files: Vec<WorkspaceProfileSourceTreeFileSummary>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceProfileSourceTreeFileResponse {
    pub workspace_id: String,
    pub source_tree_id: String,
    pub file: WorkspaceProfileSourceTreeFileSummary,
    pub content: String,
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
pub struct WriteWorkspaceProfileTreeFileRequest {
    pub path: String,
    pub content: String,
    #[serde(default)]
    pub revision: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteWorkspaceProfileTreeFileRequest {
    pub path: String,
    pub revision: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadWorkspaceProfileTreeFileQuery {
    pub path: String,
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
        entry_diagnostics.extend(validate_project_profile_entry(workspace_root, &entry));
        if BUILTIN_PROFILE_SLUGS.contains(&entry.name.as_str()) {
            entry_diagnostics.push(diagnostic(
                "profile_selector_duplicate",
                DiagnosticSeverity::Error,
                format!(
                    "Project profile '{}' conflicts with a builtin selector.",
                    entry.name
                ),
            ));
        }
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
    let mut default_diagnostics = validate_registry_default(&registry)
        .err()
        .map(|err| vec![diagnostic_from_error(&err)])
        .unwrap_or_default();
    diagnostics.extend(
        profiles
            .iter()
            .filter(|profile| profile.source_kind == "project")
            .flat_map(|profile| profile.diagnostics.clone()),
    );
    diagnostics.append(&mut default_diagnostics);
    ProfileSettingsResponse {
        workspace_id: workspace_id.to_string(),
        registry_revision,
        default_profile: registry.default,
        profiles,
        sources,
        source_trees: vec![profile_source_tree_summary(workspace_root)],
        diagnostics,
    }
}

pub fn read_profile_source(
    workspace_id: &str,
    workspace_root: &Path,
    source_id: &str,
) -> Result<WorkspaceProfileSourceDetailResponse> {
    let registry = read_registry(workspace_root).map_err(profile_registry_error)?;
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
    ensure_revision(
        &registry_path,
        &request.registry_revision,
        "profile_registry_revision_conflict",
    )?;
    let mut registry = read_registry(workspace_root).map_err(profile_registry_error)?;
    let name = validate_profile_name(&request.name)?;
    if registry.profile.contains_key(&name) || BUILTIN_PROFILE_SLUGS.contains(&name.as_str()) {
        return Err(Error::RuntimeOperationFailed {
            runtime_id: "workspace-backend".to_string(),
            code: "profile_selector_duplicate".to_string(),
            message: "Profile selector already exists".to_string(),
        });
    }
    let relative_path = PathBuf::from(".yoi")
        .join("profiles")
        .join(format!("{name}.dcdl"));
    validate_source_content(workspace_root, &name, &relative_path, &request.content)?;
    let full = prepare_source_path_for_write(workspace_root, &relative_path)?;
    fs::write(&full, request.content)?;
    registry.profile.insert(
        name.clone(),
        ProfileEntryFile::Table(ProfileEntryTable {
            path: format!("profiles/{name}.dcdl"),
            description: request
                .description
                .and_then(|value| optional_trim(value.as_str())),
        }),
    );
    validate_registry_default(&registry)?;
    validate_all_project_profiles(workspace_root, &registry)?;
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
    ensure_revision(
        &path,
        &request.registry_revision,
        "profile_registry_revision_conflict",
    )?;
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
                description: update
                    .description
                    .and_then(|value| optional_trim(value.as_str())),
            }),
        );
    }
    let registry = ProfileRegistryDocument {
        default: request.default_profile,
        profile,
    };
    validate_registry_default(&registry)?;
    validate_all_project_profiles(workspace_root, &registry)?;
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
    let registry = read_registry(workspace_root).map_err(profile_registry_error)?;
    let (name, entry) = entry_for_source_id(&registry, source_id)?;
    let full = checked_source_path(workspace_root, &entry.relative_path)?;
    ensure_revision(&full, &request.revision, "profile_source_revision_conflict")?;
    validate_source_content(
        workspace_root,
        &name,
        &entry.relative_path,
        &request.content,
    )?;
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
    ensure_revision(
        &registry_path,
        &request.registry_revision,
        "profile_registry_revision_conflict",
    )?;
    let mut registry = read_registry(workspace_root).map_err(profile_registry_error)?;
    let (name, entry) = entry_for_source_id(&registry, source_id)?;
    let full = checked_source_path(workspace_root, &entry.relative_path)?;
    ensure_revision(
        &full,
        &request.source_revision,
        "profile_source_revision_conflict",
    )?;
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

pub fn read_profile_source_tree(
    workspace_id: &str,
    workspace_root: &Path,
    source_tree_id: &str,
) -> Result<WorkspaceProfileSourceTreeResponse> {
    ensure_profile_source_tree_id(source_tree_id)?;
    let files = list_profile_tree_files(workspace_root)?;
    Ok(WorkspaceProfileSourceTreeResponse {
        workspace_id: workspace_id.to_string(),
        tree: profile_source_tree_summary_with_count(workspace_root, files.len()),
        files,
        diagnostics: Vec::new(),
    })
}

pub fn read_profile_tree_file(
    workspace_id: &str,
    workspace_root: &Path,
    source_tree_id: &str,
    query: ReadWorkspaceProfileTreeFileQuery,
) -> Result<WorkspaceProfileSourceTreeFileResponse> {
    ensure_profile_source_tree_id(source_tree_id)?;
    let relative_path = relative_source_path_for_virtual_path(&query.path)?;
    let full = checked_source_path(workspace_root, &relative_path)?;
    let metadata = source_metadata(&full)?;
    if metadata.len() > MAX_PROFILE_SOURCE_BYTES {
        return Err(profile_validation_error(
            "profile_source_too_large",
            "Profile source is too large for browser editing",
        ));
    }
    let content = fs::read_to_string(&full)?;
    Ok(WorkspaceProfileSourceTreeFileResponse {
        workspace_id: workspace_id.to_string(),
        source_tree_id: source_tree_id.to_string(),
        file: summarize_tree_file(&full, display_source_path(&relative_path)),
        content,
        diagnostics: Vec::new(),
    })
}

pub fn write_profile_tree_file(
    workspace_id: &str,
    workspace_root: &Path,
    source_tree_id: &str,
    request: WriteWorkspaceProfileTreeFileRequest,
) -> Result<WorkspaceProfileSourceTreeFileResponse> {
    ensure_profile_source_tree_id(source_tree_id)?;
    if request.content.as_bytes().len() as u64 > MAX_PROFILE_SOURCE_BYTES {
        return Err(profile_validation_error(
            "profile_source_too_large",
            "Profile source exceeds the browser editing size limit",
        ));
    }
    let relative_path = relative_source_path_for_virtual_path(&request.path)?;
    let full = prepare_source_path_for_write(workspace_root, &relative_path)?;
    if let Some(expected) = request.revision.as_deref() {
        ensure_revision(&full, expected, "profile_source_revision_conflict")?;
    } else if full.exists() {
        return Err(profile_validation_error(
            "profile_source_revision_required",
            "Existing profile source edits require a revision token",
        ));
    }
    validate_tree_content(workspace_root, &relative_path, &request.content)?;
    fs::write(&full, &request.content)?;
    Ok(WorkspaceProfileSourceTreeFileResponse {
        workspace_id: workspace_id.to_string(),
        source_tree_id: source_tree_id.to_string(),
        file: summarize_tree_file(&full, display_source_path(&relative_path)),
        content: request.content,
        diagnostics: vec![diagnostic(
            "profile_source_file_written",
            DiagnosticSeverity::Info,
            "Profile source file was written through the source tree API.",
        )],
    })
}

pub fn delete_profile_tree_file(
    workspace_id: &str,
    workspace_root: &Path,
    source_tree_id: &str,
    request: DeleteWorkspaceProfileTreeFileRequest,
) -> Result<WorkspaceProfileSourceTreeResponse> {
    ensure_profile_source_tree_id(source_tree_id)?;
    let relative_path = relative_source_path_for_virtual_path(&request.path)?;
    let full = checked_source_path(workspace_root, &relative_path)?;
    ensure_revision(&full, &request.revision, "profile_source_revision_conflict")?;
    let registry = read_registry(workspace_root).map_err(profile_registry_error)?;
    if project_entries(&registry, &mut Vec::new())
        .iter()
        .any(|entry| entry.relative_path == relative_path)
    {
        return Err(profile_validation_error(
            "profile_source_registered",
            "Registered profile entry sources must be deleted through the profile registry API",
        ));
    }
    if full.exists() {
        fs::remove_file(full)?;
    }
    read_profile_source_tree(workspace_id, workspace_root, source_tree_id)
}

pub fn build_workspace_profile_archive(
    workspace_root: &Path,
    selector: &str,
) -> Result<Option<ProfileSourceArchive>> {
    if !selector.starts_with("project:") {
        return Ok(None);
    }
    let registry = read_registry(workspace_root).map_err(profile_registry_error)?;
    let sources = read_profile_source_tree_contents(workspace_root)?;
    let archive = build_profile_archive_for_selector(&registry, sources, selector)?;
    archive
        .verify()
        .and_then(|verified| {
            verified
                .resolve_profile(selector, workspace_root, "workspace-settings-validation")
                .map(|_| ())
        })
        .map_err(|err| Error::RuntimeOperationFailed {
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
        .filter(|profile| profile.source_kind == "project" && !has_error(&profile.diagnostics))
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

fn validate_project_profile_entry(
    workspace_root: &Path,
    entry: &ProjectProfileEntry,
) -> Vec<RuntimeDiagnostic> {
    let full = match checked_source_path(workspace_root, &entry.relative_path) {
        Ok(path) => path,
        Err(err) => return vec![diagnostic_from_error(&err)],
    };
    match fs::read_to_string(&full) {
        Ok(content) => {
            validate_source_content(workspace_root, &entry.name, &entry.relative_path, &content)
                .err()
                .map(|err| vec![diagnostic_from_error(&err)])
                .unwrap_or_default()
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => vec![diagnostic(
            "profile_source_missing",
            DiagnosticSeverity::Error,
            format!("Profile source '{}' is missing.", entry.name),
        )],
        Err(err) => vec![diagnostic(
            "profile_source_read_failed",
            DiagnosticSeverity::Error,
            sanitize_error(&err.to_string()),
        )],
    }
}

fn diagnostic_from_error(err: &Error) -> RuntimeDiagnostic {
    match err {
        Error::RuntimeOperationFailed { code, message, .. } => diagnostic(
            code.clone(),
            DiagnosticSeverity::Error,
            sanitize_error(message),
        ),
        other => diagnostic(
            "profile_settings_failed",
            DiagnosticSeverity::Error,
            sanitize_error(&other.to_string()),
        ),
    }
}

fn has_error(diagnostics: &[RuntimeDiagnostic]) -> bool {
    diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
}

fn validate_all_project_profiles(
    workspace_root: &Path,
    registry: &ProfileRegistryDocument,
) -> Result<()> {
    let mut diagnostics = Vec::new();
    let entries = project_entries(registry, &mut diagnostics);
    if let Some(diagnostic) = diagnostics
        .into_iter()
        .find(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
    {
        return Err(profile_validation_error(
            diagnostic.code,
            diagnostic.message,
        ));
    }
    for entry in entries {
        if BUILTIN_PROFILE_SLUGS.contains(&entry.name.as_str()) {
            return Err(profile_validation_error(
                "profile_selector_duplicate",
                "Project profile conflicts with a builtin selector",
            ));
        }
        if let Some(diagnostic) = validate_project_profile_entry(workspace_root, &entry)
            .into_iter()
            .find(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
        {
            return Err(profile_validation_error(
                diagnostic.code,
                diagnostic.message,
            ));
        }
    }
    Ok(())
}

fn profile_validation_error(code: impl Into<String>, message: impl Into<String>) -> Error {
    Error::RuntimeOperationFailed {
        runtime_id: "workspace-backend".to_string(),
        code: code.into(),
        message: message.into(),
    }
}

fn profile_registry_error(message: String) -> Error {
    profile_validation_error("profile_registry_schema_invalid", sanitize_error(&message))
}

fn validate_registry_default(registry: &ProfileRegistryDocument) -> Result<()> {
    let Some(value) = registry.default.as_deref() else {
        return Ok(());
    };
    if BUILTIN_PROFILE_IDS.contains(&value) {
        return Ok(());
    }
    if let Some(name) = value.strip_prefix("project:") {
        let name = validate_profile_name(name)?;
        if registry.profile.contains_key(&name) {
            return Ok(());
        }
        return Err(profile_validation_error(
            "profile_default_unknown",
            "Default project profile selector is not present in the workspace profile registry",
        ));
    }
    Err(profile_validation_error(
        "profile_default_invalid",
        "Default profile must be a Backend-published builtin or project selector",
    ))
}

fn profile_source_tree_summary(workspace_root: &Path) -> WorkspaceProfileSourceTreeSummary {
    let file_count = list_profile_tree_files(workspace_root)
        .map(|files| files.len())
        .unwrap_or(0);
    profile_source_tree_summary_with_count(workspace_root, file_count)
}

fn profile_source_tree_summary_with_count(
    workspace_root: &Path,
    file_count: usize,
) -> WorkspaceProfileSourceTreeSummary {
    WorkspaceProfileSourceTreeSummary {
        source_tree_id: PROFILE_SOURCE_TREE_ID.to_string(),
        label: "Project profile sources".to_string(),
        root_path: PROFILE_SOURCE_TREE_DISPLAY_ROOT.to_string(),
        kind: "decodal_source_tree".to_string(),
        content_type: "application/vnd.yoi.profile-source-tree+json".to_string(),
        content_digest: profile_source_tree_digest(workspace_root)
            .unwrap_or_else(|_| "sha256:unavailable".to_string()),
        provenance: WorkspaceProfileSourceProvenance::ProjectProfileSourceTree,
        editable: true,
        revision: file_revision(&workspace_root.join(PROFILE_SOURCE_ROOT_RELATIVE_PATH)),
        file_count,
        diagnostics: Vec::new(),
    }
}

fn ensure_profile_source_tree_id(source_tree_id: &str) -> Result<()> {
    if source_tree_id == PROFILE_SOURCE_TREE_ID {
        Ok(())
    } else {
        Err(Error::RuntimeOperationFailed {
            runtime_id: "workspace-backend".to_string(),
            code: "unknown_profile_source_tree".to_string(),
            message: "Unknown profile source tree".to_string(),
        })
    }
}

fn list_profile_tree_files(
    workspace_root: &Path,
) -> Result<Vec<WorkspaceProfileSourceTreeFileSummary>> {
    let Some(source_root) = existing_profile_source_root(workspace_root)? else {
        return Ok(Vec::new());
    };
    let mut files = Vec::new();
    collect_profile_tree_files(workspace_root, &source_root, &source_root.path, &mut files)?;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

fn collect_profile_tree_files(
    workspace_root: &Path,
    source_root: &ProfileSourceRoot,
    dir: &Path,
    files: &mut Vec<WorkspaceProfileSourceTreeFileSummary>,
) -> Result<()> {
    let canonical_dir = fs::canonicalize(dir)?;
    if !canonical_dir.starts_with(&source_root.canonical_path) {
        return Err(profile_source_symlink_escape(
            "Profile source directory resolves outside the workspace profile source root",
        ));
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            return Err(profile_source_symlink_escape(
                "Profile source tree entries must not be symlinks",
            ));
        }
        if file_type.is_dir() {
            collect_profile_tree_files(workspace_root, source_root, &path, files)?;
        } else if file_type.is_file()
            && path.extension().and_then(|value| value.to_str()) == Some("dcdl")
        {
            let canonical_file = fs::canonicalize(&path)?;
            if !canonical_file.starts_with(&source_root.canonical_path)
                || !canonical_file.starts_with(&source_root.canonical_workspace)
            {
                return Err(profile_source_symlink_escape(
                    "Profile source file resolves outside the workspace profile source root",
                ));
            }
            let relative = path
                .strip_prefix(workspace_root)
                .map_err(|_| {
                    profile_validation_error(
                        "profile_source_path_invalid",
                        "Profile source is outside workspace",
                    )
                })?
                .to_path_buf();
            files.push(summarize_tree_file(&path, display_source_path(&relative)));
        }
    }
    Ok(())
}

fn summarize_tree_file(path: &Path, virtual_path: String) -> WorkspaceProfileSourceTreeFileSummary {
    WorkspaceProfileSourceTreeFileSummary {
        path: virtual_path,
        kind: "decodal".to_string(),
        content_type: "text/x-decodal".to_string(),
        content_digest: file_content_digest(path),
        provenance: WorkspaceProfileSourceProvenance::ProjectProfileSourceTree,
        editable: true,
        revision: file_revision(path),
        size_bytes: source_metadata(path)
            .map(|metadata| metadata.len())
            .unwrap_or(0),
        diagnostics: Vec::new(),
    }
}

fn relative_source_path_for_virtual_path(virtual_path: &str) -> Result<PathBuf> {
    let normalized = normalize_virtual_profile_source_path(virtual_path)?;
    let Some(rest) = normalized.strip_prefix("profiles/") else {
        return Err(profile_validation_error(
            "profile_source_path_invalid",
            "Profile source paths must be under profiles/",
        ));
    };
    if rest.is_empty() || !rest.ends_with(".dcdl") {
        return Err(profile_validation_error(
            "profile_source_path_invalid",
            "Profile source files must use the .dcdl extension",
        ));
    }
    Ok(Path::new(PROFILE_SOURCE_ROOT_RELATIVE_PATH).join(rest))
}

fn display_source_path(relative_path: &Path) -> String {
    let profile_root = Path::new(PROFILE_SOURCE_ROOT_RELATIVE_PATH);
    let relative = relative_path
        .strip_prefix(profile_root)
        .unwrap_or(relative_path);
    let rest = relative.to_string_lossy().replace('\\', "/");
    format!("{PROFILE_SOURCE_TREE_DISPLAY_ROOT}/{rest}")
}

fn normalize_virtual_profile_source_path(path: &str) -> Result<String> {
    let path = path
        .strip_prefix("project:")
        .or_else(|| path.strip_prefix("workspace:"))
        .unwrap_or(path);
    if path.is_empty() || path.contains("://") || Path::new(path).is_absolute() {
        return Err(profile_validation_error(
            "profile_source_path_invalid",
            "Profile source path must be a virtual relative path",
        ));
    }
    let mut normalized = PathBuf::new();
    for component in Path::new(path).components() {
        match component {
            Component::CurDir => {}
            Component::Normal(value) => normalized.push(value),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(profile_validation_error(
                    "profile_source_path_invalid",
                    "Profile source path must not escape the virtual source tree",
                ));
            }
        }
    }
    let normalized = normalized.to_string_lossy().replace('\\', "/");
    if normalized.is_empty() {
        return Err(profile_validation_error(
            "profile_source_path_invalid",
            "Profile source path must not be empty",
        ));
    }
    Ok(normalized)
}

fn read_profile_source_tree_contents(workspace_root: &Path) -> Result<BTreeMap<String, String>> {
    let mut sources = BTreeMap::new();
    for file in list_profile_tree_files(workspace_root)? {
        let relative = relative_source_path_for_virtual_path(&file.path)?;
        let full = checked_source_path(workspace_root, &relative)?;
        sources.insert(file.path, fs::read_to_string(full)?);
    }
    Ok(sources)
}

fn build_profile_archive_from_tree_sources(
    registry: &ProfileRegistryDocument,
    sources: BTreeMap<String, String>,
) -> Result<(ProfileSourceArchive, BTreeMap<String, String>)> {
    let mut entrypoints = BTreeMap::new();
    for entry in project_entries(registry, &mut Vec::new()) {
        let path = display_source_path(&entry.relative_path);
        if sources.contains_key(&path) {
            entrypoints.insert(project_selector(&entry.name), path);
        }
    }
    if let Some(default) = registry
        .default
        .as_deref()
        .filter(|value| value.starts_with("project:"))
    {
        if let Some(path) = entrypoints.get(default).cloned() {
            entrypoints.insert("default".to_string(), path);
        }
    }
    build_profile_archive_from_source_set(entrypoints, sources)
}

fn build_profile_archive_for_selector(
    registry: &ProfileRegistryDocument,
    sources: BTreeMap<String, String>,
    selector: &str,
) -> Result<ProfileSourceArchive> {
    let Some(name) = selector.strip_prefix("project:") else {
        return Err(profile_validation_error(
            "profile_selector_invalid",
            "Project profile archive selector must use project:*",
        ));
    };
    let entry = project_entries(registry, &mut Vec::new())
        .into_iter()
        .find(|entry| entry.name == name)
        .ok_or_else(|| {
            profile_validation_error(
                "unknown_profile_selector",
                "Selected project profile is not present in the workspace profile registry",
            )
        })?;
    let root_path = display_source_path(&entry.relative_path);
    if !sources.contains_key(&root_path) {
        return Err(profile_validation_error(
            "profile_source_missing",
            "Selected project profile source is missing from the source tree",
        ));
    }
    let mut closure_sources = BTreeMap::new();
    let mut imports = BTreeMap::new();
    collect_profile_import_closure(&root_path, &sources, &mut closure_sources, &mut imports)?;
    let mut entrypoints = BTreeMap::new();
    entrypoints.insert(selector.to_string(), root_path);
    build_profile_archive_from_source_set_with_imports(entrypoints, closure_sources, imports)
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

fn build_profile_archive_from_source_set(
    entrypoints: BTreeMap<String, String>,
    sources: BTreeMap<String, String>,
) -> Result<(ProfileSourceArchive, BTreeMap<String, String>)> {
    let mut imports = BTreeMap::new();
    let source_snapshot = sources.clone();
    for (current_path, content) in &source_snapshot {
        for specifier in collect_decodal_import_specifiers(content) {
            let target = resolve_profile_source_import(current_path, &specifier)?;
            if source_snapshot.contains_key(&target) {
                imports.insert(format!("{current_path}\0{specifier}"), target);
            } else {
                return Err(profile_validation_error(
                    "profile_source_import_missing",
                    &format!(
                        "Profile source import {specifier:?} from {current_path} resolves to missing {target}"
                    ),
                ));
            }
        }
    }
    build_profile_archive_from_source_set_with_imports(entrypoints, sources, imports.clone())
        .map(|archive| (archive, imports))
}

fn build_profile_archive_from_source_set_with_imports(
    entrypoints: BTreeMap<String, String>,
    mut sources: BTreeMap<String, String>,
    imports: BTreeMap<String, String>,
) -> Result<ProfileSourceArchive> {
    if sources.is_empty() {
        sources.insert("profiles/.empty.dcdl".to_string(), "{}".to_string());
    }
    ProfileSourceArchive::build(ProfileSourceArchiveInput {
        id: "workspace-project-decodal-profiles-v1".to_string(),
        entrypoints,
        imports,
        sources,
    })
    .map_err(|err| Error::RuntimeOperationFailed {
        runtime_id: "workspace-backend".to_string(),
        code: "profile_source_archive_invalid".to_string(),
        message: err.to_string(),
    })
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
    if !normalized.starts_with("profiles/") {
        return Err(profile_validation_error(
            "profile_source_import_invalid",
            "Profile source import must resolve inside the profiles/ tree",
        ));
    }
    Ok(normalized)
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

fn validate_tree_content(workspace_root: &Path, relative_path: &Path, content: &str) -> Result<()> {
    let mut sources = read_profile_source_tree_contents(workspace_root)?;
    sources.insert(display_source_path(relative_path), content.to_string());
    let registry = read_registry(workspace_root).map_err(profile_registry_error)?;
    let (archive, _) = build_profile_archive_from_tree_sources(&registry, sources)?;
    archive
        .verify()
        .map_err(|err| Error::RuntimeOperationFailed {
            runtime_id: "workspace-backend".to_string(),
            code: "profile_source_invalid".to_string(),
            message: err.to_string(),
        })?;
    Ok(())
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
    validate_source_candidate_path(workspace_root, relative_path)?;
    let mut sources = read_profile_source_tree_contents(workspace_root)?;
    sources.insert(display_source_path(relative_path), content.to_string());
    let mut registry = read_registry(workspace_root).map_err(profile_registry_error)?;
    registry.profile.entry(name.to_string()).or_insert_with(|| {
        ProfileEntryFile::Table(ProfileEntryTable {
            path: display_source_path(relative_path),
            description: None,
        })
    });
    let selector = project_selector(name);
    let (archive, _) = build_profile_archive_from_tree_sources(&registry, sources)?;
    let verified = archive
        .verify()
        .map_err(|err| Error::RuntimeOperationFailed {
            runtime_id: "workspace-backend".to_string(),
            code: "profile_source_archive_invalid".to_string(),
            message: err.to_string(),
        })?;
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
        content_type: "text/x-decodal".to_string(),
        content_digest: checked_source_path(workspace_root, relative_path)
            .map(|path| file_content_digest(&path))
            .unwrap_or_else(|_| "sha256:unavailable".to_string()),
        provenance: WorkspaceProfileSourceProvenance::ProjectProfileSourceTree,
        editable: diagnostics
            .iter()
            .all(|d| d.severity != DiagnosticSeverity::Error),
        revision,
        size_bytes,
        diagnostics,
    }
}

fn read_registry(workspace_root: &Path) -> std::result::Result<ProfileRegistryDocument, String> {
    let path = registry_path(workspace_root);
    match fs::read_to_string(&path) {
        Ok(raw) => {
            toml::from_str(&raw).map_err(|err| format!("invalid profile registry schema: {err}"))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            Ok(ProfileRegistryDocument::default())
        }
        Err(err) => Err(format!(
            "failed to read profile registry: {}",
            sanitize_error(&err.to_string())
        )),
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

fn project_entries(
    registry: &ProfileRegistryDocument,
    diagnostics: &mut Vec<RuntimeDiagnostic>,
) -> Vec<ProjectProfileEntry> {
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
                    ProfileEntryFile::Table(table) => {
                        (table.path.clone(), table.description.clone())
                    }
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

fn entry_for_source_id(
    registry: &ProfileRegistryDocument,
    source_id: &str,
) -> Result<(String, ProjectProfileEntry)> {
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
        return Err(
            "Profile source path must be under the workspace profile source root.".to_string(),
        );
    }
    for component in path.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(
                "Profile source path must not contain absolute, parent, or prefix components."
                    .to_string(),
            );
        }
    }
    if path.extension().and_then(|value| value.to_str()) != Some("dcdl") {
        return Err("Profile source path must use the .dcdl extension.".to_string());
    }
    Ok(())
}

fn profile_source_tree_digest(workspace_root: &Path) -> Result<String> {
    let mut bytes = Vec::new();
    for file in list_profile_tree_files(workspace_root)? {
        bytes.extend_from_slice(file.path.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(file.content_digest.as_bytes());
        bytes.push(0);
    }
    Ok(sha256_hex(&bytes))
}

fn file_content_digest(path: &Path) -> String {
    fs::read(path)
        .map(|bytes| sha256_hex(&bytes))
        .unwrap_or_else(|_| "sha256:unavailable".to_string())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::from("sha256:");
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[derive(Debug, Clone)]
struct ProfileSourceRoot {
    path: PathBuf,
    canonical_path: PathBuf,
    canonical_workspace: PathBuf,
}

fn profile_source_symlink_escape(message: impl Into<String>) -> Error {
    Error::RuntimeOperationFailed {
        runtime_id: "workspace-backend".to_string(),
        code: "profile_source_symlink_escape".to_string(),
        message: message.into(),
    }
}

fn existing_profile_source_root(workspace_root: &Path) -> Result<Option<ProfileSourceRoot>> {
    let source_root = workspace_root.join(PROFILE_SOURCE_ROOT_RELATIVE_PATH);
    let metadata = match fs::symlink_metadata(&source_root) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    validate_profile_source_root_metadata(workspace_root, source_root, metadata).map(Some)
}

fn prepare_profile_source_root_for_write(workspace_root: &Path) -> Result<ProfileSourceRoot> {
    let canonical_workspace = fs::canonicalize(workspace_root)?;
    let yoi_dir = workspace_root.join(".yoi");
    match fs::symlink_metadata(&yoi_dir) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(profile_source_symlink_escape(
                    "Workspace .yoi directory must not be a symlink",
                ));
            }
            if !metadata.is_dir() {
                return Err(profile_validation_error(
                    "profile_source_path_invalid",
                    "Workspace .yoi path is not a directory",
                ));
            }
            let canonical_yoi = fs::canonicalize(&yoi_dir)?;
            if !canonical_yoi.starts_with(&canonical_workspace) {
                return Err(profile_source_symlink_escape(
                    "Workspace .yoi directory resolves outside the workspace root",
                ));
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => fs::create_dir(&yoi_dir)?,
        Err(err) => return Err(err.into()),
    }

    let source_root = workspace_root.join(PROFILE_SOURCE_ROOT_RELATIVE_PATH);
    match fs::symlink_metadata(&source_root) {
        Ok(metadata) => {
            validate_profile_source_root_metadata(workspace_root, source_root, metadata)
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&source_root)?;
            let metadata = fs::symlink_metadata(&source_root)?;
            validate_profile_source_root_metadata(workspace_root, source_root, metadata)
        }
        Err(err) => Err(err.into()),
    }
}

fn validate_profile_source_root_metadata(
    workspace_root: &Path,
    source_root: PathBuf,
    metadata: std::fs::Metadata,
) -> Result<ProfileSourceRoot> {
    if metadata.file_type().is_symlink() {
        return Err(profile_source_symlink_escape(
            "Workspace profile source root must not be a symlink",
        ));
    }
    if !metadata.is_dir() {
        return Err(profile_validation_error(
            "profile_source_path_invalid",
            "Workspace profile source root is not a directory",
        ));
    }
    let canonical_workspace = fs::canonicalize(workspace_root)?;
    let canonical_path = fs::canonicalize(&source_root)?;
    if !canonical_path.starts_with(&canonical_workspace) {
        return Err(profile_source_symlink_escape(
            "Workspace profile source root resolves outside the workspace root",
        ));
    }
    Ok(ProfileSourceRoot {
        path: source_root,
        canonical_path,
        canonical_workspace,
    })
}

fn validate_source_candidate_path(workspace_root: &Path, relative_path: &Path) -> Result<()> {
    validate_relative_source_path(relative_path).map_err(|message| {
        Error::RuntimeOperationFailed {
            runtime_id: "workspace-backend".to_string(),
            code: "profile_source_path_escape".to_string(),
            message,
        }
    })?;
    let Some(_) = existing_profile_source_root(workspace_root)? else {
        return Ok(());
    };
    let full = workspace_root.join(relative_path);
    if full.exists() || full.parent().is_some_and(Path::exists) {
        checked_source_path(workspace_root, relative_path)?;
    }
    Ok(())
}

fn prepare_source_path_for_write(workspace_root: &Path, relative_path: &Path) -> Result<PathBuf> {
    validate_relative_source_path(relative_path).map_err(|message| {
        Error::RuntimeOperationFailed {
            runtime_id: "workspace-backend".to_string(),
            code: "profile_source_path_escape".to_string(),
            message,
        }
    })?;
    let source_root = prepare_profile_source_root_for_write(workspace_root)?;
    let full = workspace_root.join(relative_path);
    let parent = full.parent().ok_or_else(|| {
        profile_validation_error(
            "profile_source_path_invalid",
            "Profile source path has no parent directory",
        )
    })?;
    let parent_relative = parent.strip_prefix(&source_root.path).map_err(|_| {
        profile_validation_error(
            "profile_source_path_escape",
            "Profile source parent must remain inside the source tree",
        )
    })?;
    let mut current = source_root.path.clone();
    for component in parent_relative.components() {
        let Component::Normal(name) = component else {
            return Err(profile_validation_error(
                "profile_source_path_escape",
                "Profile source parent must be a normalized relative path",
            ));
        };
        let next = current.join(name);
        match fs::symlink_metadata(&next) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(profile_source_symlink_escape(
                        "Profile source parent contains a symlink",
                    ));
                }
                if !metadata.is_dir() {
                    return Err(profile_validation_error(
                        "profile_source_path_invalid",
                        "Profile source parent component is not a directory",
                    ));
                }
                let canonical_next = fs::canonicalize(&next)?;
                if !canonical_next.starts_with(&source_root.canonical_path)
                    || !canonical_next.starts_with(&source_root.canonical_workspace)
                {
                    return Err(profile_source_symlink_escape(
                        "Profile source parent resolves outside the workspace profile source root",
                    ));
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => fs::create_dir(&next)?,
            Err(err) => return Err(err.into()),
        }
        current = next;
    }
    checked_source_path(workspace_root, relative_path)
}

fn checked_source_path(workspace_root: &Path, relative_path: &Path) -> Result<PathBuf> {
    validate_relative_source_path(relative_path).map_err(|message| {
        Error::RuntimeOperationFailed {
            runtime_id: "workspace-backend".to_string(),
            code: "profile_source_path_escape".to_string(),
            message,
        }
    })?;
    let source_root = existing_profile_source_root(workspace_root)?.ok_or_else(|| {
        profile_validation_error(
            "profile_source_missing",
            "Workspace profile source root does not exist",
        )
    })?;
    let full = workspace_root.join(relative_path);
    if let Ok(canonical_full) = fs::canonicalize(&full) {
        if !canonical_full.starts_with(&source_root.canonical_path)
            || !canonical_full.starts_with(&source_root.canonical_workspace)
        {
            return Err(profile_source_symlink_escape(
                "Profile source resolves outside the workspace profile source root",
            ));
        }
    } else if let Some(parent) = full.parent() {
        let canonical_parent = fs::canonicalize(parent)?;
        if !canonical_parent.starts_with(&source_root.canonical_path)
            || !canonical_parent.starts_with(&source_root.canonical_workspace)
        {
            return Err(Error::RuntimeOperationFailed {
                runtime_id: "workspace-backend".to_string(),
                code: "profile_source_path_escape".to_string(),
                message: "Profile source parent resolves outside the workspace profile source root"
                    .to_string(),
            });
        }
    }
    Ok(full)
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
            message: "Profile selector must contain only ASCII letters, digits, '-' or '_'"
                .to_string(),
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

pub fn selector_for_builtin_candidate(
    id: &str,
) -> Option<worker_runtime::catalog::ProfileSelector> {
    match id {
        "runtime_default" => Some(worker_runtime::catalog::ProfileSelector::RuntimeDefault),
        "builtin:default"
        | "builtin:companion"
        | "builtin:intake"
        | "builtin:orchestrator"
        | "builtin:coder"
        | "builtin:reviewer" => Some(worker_runtime::catalog::ProfileSelector::Builtin(
            id.to_string(),
        )),
        value if value.starts_with("project:") => Some(
            worker_runtime::catalog::ProfileSelector::Named(value.to_string()),
        ),
        _ => None,
    }
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
        assert!(
            created
                .settings
                .profiles
                .iter()
                .any(|profile| profile.profile_id == "project:alpha")
        );
        assert!(
            build_workspace_profile_archive(dir.path(), "project:alpha")
                .unwrap()
                .is_some()
        );
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
        assert!(
            settings
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "profile_source_path_escape")
        );

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
        assert!(
            err.to_string()
                .contains("profile_registry_revision_conflict")
        );
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
        assert!(
            err.to_string().contains("profile_source_syntax_invalid")
                || err.to_string().contains("profile_source_archive_invalid")
        );
    }

    #[test]
    fn invalid_project_profile_is_not_a_launch_candidate() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".yoi/profiles")).unwrap();
        fs::write(
            dir.path().join(".yoi/profiles.toml"),
            "[profile.bad]\npath = \"profiles/bad.dcdl\"\n",
        )
        .unwrap();
        fs::write(dir.path().join(".yoi/profiles/bad.dcdl"), "not decodal").unwrap();

        let settings = load_profile_settings("workspace-test", dir.path());
        let bad = settings
            .profiles
            .iter()
            .find(|profile| profile.profile_id == "project:bad")
            .expect("bad profile summary is still visible for repair");
        assert!(
            bad.diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code.starts_with("profile_source_"))
        );
        assert!(project_profile_candidates(dir.path()).is_empty());
        assert!(!is_profile_candidate(dir.path(), "project:bad"));
    }

    #[test]
    fn registry_update_rejects_missing_source_and_unknown_default() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".yoi")).unwrap();
        fs::write(dir.path().join(".yoi/profiles.toml"), "").unwrap();
        let revision = file_revision(&dir.path().join(".yoi/profiles.toml"));

        let err = update_profile_registry(
            "workspace-test",
            dir.path(),
            UpdateWorkspaceProfileRegistryRequest {
                registry_revision: revision.clone(),
                default_profile: Some("project:missing".to_string()),
                profiles: Vec::new(),
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("profile_default_unknown"));

        let err = update_profile_registry(
            "workspace-test",
            dir.path(),
            UpdateWorkspaceProfileRegistryRequest {
                registry_revision: revision,
                default_profile: None,
                profiles: vec![WorkspaceProfileRegistryEntryUpdate {
                    name: "missing".to_string(),
                    description: None,
                    profile_source_id: None,
                }],
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("profile_source_missing"));
        assert_eq!(
            fs::read_to_string(dir.path().join(".yoi/profiles.toml")).unwrap(),
            ""
        );
    }

    #[test]
    fn profile_source_rejects_too_large_content() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".yoi")).unwrap();
        fs::write(dir.path().join(".yoi/profiles.toml"), "").unwrap();
        let revision = file_revision(&dir.path().join(".yoi/profiles.toml"));
        let err = create_profile_source(
            "workspace-test",
            dir.path(),
            CreateWorkspaceProfileSourceRequest {
                name: "large".to_string(),
                description: None,
                content: "x".repeat((MAX_PROFILE_SOURCE_BYTES + 1) as usize),
                registry_revision: revision,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("profile_source_too_large"));
    }

    #[cfg(unix)]
    #[test]
    fn registry_update_rejects_symlink_escape() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".yoi/profiles")).unwrap();
        fs::write(dir.path().join(".yoi/profiles.toml"), "").unwrap();
        let outside = dir.path().join("outside.dcdl");
        fs::write(&outside, valid_decodal("escape")).unwrap();
        std::os::unix::fs::symlink(&outside, dir.path().join(".yoi/profiles/escape.dcdl")).unwrap();
        let revision = file_revision(&dir.path().join(".yoi/profiles.toml"));
        let err = update_profile_registry(
            "workspace-test",
            dir.path(),
            UpdateWorkspaceProfileRegistryRequest {
                registry_revision: revision,
                default_profile: None,
                profiles: vec![WorkspaceProfileRegistryEntryUpdate {
                    name: "escape".to_string(),
                    description: None,
                    profile_source_id: None,
                }],
            },
        )
        .unwrap_err();
        let rendered = err.to_string();
        assert!(rendered.contains("profile_source_symlink_escape"));
        assert!(!rendered.contains(dir.path().to_string_lossy().as_ref()));
    }

    #[test]
    fn selected_profile_archive_contains_only_import_closure() {
        let mut registry = ProfileRegistryDocument::default();
        registry.profile.insert(
            "alpha".to_string(),
            ProfileEntryFile::Table(ProfileEntryTable {
                path: "profiles/alpha.dcdl".to_string(),
                description: None,
            }),
        );
        registry.profile.insert(
            "beta".to_string(),
            ProfileEntryFile::Table(ProfileEntryTable {
                path: "profiles/beta.dcdl".to_string(),
                description: None,
            }),
        );
        let mut sources = BTreeMap::new();
        sources.insert(
            "profiles/alpha.dcdl".to_string(),
            r#"{ extra = import "./shared.dcdl"; }"#.to_string(),
        );
        sources.insert("profiles/shared.dcdl".to_string(), "{}".to_string());
        sources.insert("profiles/beta.dcdl".to_string(), "{}".to_string());
        sources.insert("profiles/unregistered.dcdl".to_string(), "{}".to_string());

        let archive =
            build_profile_archive_for_selector(&registry, sources, "project:alpha").unwrap();
        let verified = archive.verify().unwrap();
        let manifest = verified.manifest();
        let source_paths: Vec<_> = manifest
            .sources
            .iter()
            .map(|source| source.path.as_str())
            .collect();
        assert_eq!(manifest.entrypoints.len(), 1);
        assert_eq!(
            manifest
                .entrypoints
                .get("project:alpha")
                .map(String::as_str),
            Some("profiles/alpha.dcdl")
        );
        assert!(source_paths.contains(&"profiles/alpha.dcdl"));
        assert!(source_paths.contains(&"profiles/shared.dcdl"));
        assert!(!source_paths.contains(&"profiles/beta.dcdl"));
        assert!(!source_paths.contains(&"profiles/unregistered.dcdl"));
    }

    #[cfg(unix)]
    #[test]
    fn tree_write_rejects_symlink_parent_before_outside_side_effect() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".yoi/profiles")).unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join(".yoi/profiles/link")).unwrap();

        let err = write_profile_tree_file(
            "workspace-test",
            dir.path(),
            PROFILE_SOURCE_TREE_ID,
            WriteWorkspaceProfileTreeFileRequest {
                path: "profiles/link/nested/new.dcdl".to_string(),
                content: valid_decodal("new"),
                revision: None,
            },
        )
        .unwrap_err();

        assert!(err.to_string().contains("profile_source_symlink_escape"));
        assert!(!outside.path().join("nested").exists());
    }

    #[cfg(unix)]
    #[test]
    fn source_root_symlink_is_rejected_before_tree_side_effects() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".yoi")).unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join(".yoi/profiles")).unwrap();

        let err = write_profile_tree_file(
            "workspace-test",
            dir.path(),
            PROFILE_SOURCE_TREE_ID,
            WriteWorkspaceProfileTreeFileRequest {
                path: "profiles/nested/new.dcdl".to_string(),
                content: valid_decodal("new"),
                revision: None,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("profile_source_symlink_escape"));
        assert!(!outside.path().join("nested").exists());
        assert!(!outside.path().join("new.dcdl").exists());

        let err = read_profile_source_tree("workspace-test", dir.path(), PROFILE_SOURCE_TREE_ID)
            .unwrap_err();
        assert!(err.to_string().contains("profile_source_symlink_escape"));

        let err = build_workspace_profile_archive(dir.path(), "project:any").unwrap_err();
        assert!(err.to_string().contains("profile_source_symlink_escape"));
    }
}
