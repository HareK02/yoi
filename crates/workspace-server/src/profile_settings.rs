use std::collections::BTreeMap;
use std::path::Path;

use config_source::ConfigSchemaContribution;
use manifest::{ProfileSource, builtin_profile_catalog_snapshot, resolve_profile_artifact_value};
use serde::Deserialize;
use server_api::{
    ProfileSettingsResponse, WorkspaceMetadataSettingsResponse, WorkspaceProfileSourceProvenance,
    WorkspaceProfileSourceSummary, WorkspaceProfileSummary,
};
use sha2::{Digest, Sha256};
use worker::EffectivePromptCatalog;
use worker_runtime::config_bundle::{
    ConfigBundle, ConfigBundleMetadata, ConfigBundleProvenance, ConfigProfileDescriptor,
};
use worker_runtime::profile_archive::{ProfileSourceArchive, ProfileSourceArchiveInput};

use crate::config_source::{
    WorkspaceConfigSchemaProvider, WorkspaceConfigState, evaluate_workspace_config_state,
};
use crate::store::WorkspaceRecord;
use crate::{Error, Result};

const PROFILE_SCHEMA_SOURCE: &str = include_str!("../../../resources/config-schema/profile.dcdl");

#[derive(Debug, Default)]
pub struct ProfileConfigSchemaProvider;

impl WorkspaceConfigSchemaProvider for ProfileConfigSchemaProvider {
    fn contribution(&self) -> Result<ConfigSchemaContribution> {
        ConfigSchemaContribution::new("builtin:profile", "profile", "2", PROFILE_SCHEMA_SOURCE)
            .map(|schema| {
                schema.with_authoring_source(include_str!(
                    "../../../resources/config-schema/profile-authoring.dcdl"
                ))
            })
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
    profile: serde_json::Value,
    label: String,
    description: String,
}

#[derive(Debug, Clone)]
pub struct ProfileConfigProjection {
    pub settings: ProfileSettingsResponse,
    entries: BTreeMap<String, VirtualProfileEntry>,
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
    project_profiles_from_evaluation(workspace_id, state, &evaluation)
}

pub(crate) fn project_profiles_from_evaluation(
    workspace_id: &str,
    state: &WorkspaceConfigState,
    evaluation: &config_source::EvaluationResult,
) -> Result<ProfileConfigProjection> {
    if evaluation.projection_digest != state.projection_digest {
        return Err(Error::Config(
            "Profile evaluation does not match Workspace config state".into(),
        ));
    }
    let projected = evaluation.projections.first().ok_or_else(|| {
        Error::RegistryInconsistency("Workspace config has no active projection".to_string())
    })?;
    if projected
        .data_json
        .pointer("/profile/entries")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|entries| entries.iter().any(|entry| entry.get("source").is_some()))
    {
        return Err(profile_validation_error(
            "profile_source_registration_removed",
            "profile.entries.source is no longer supported: replace source = \"profiles/name.dcdl\" with profile = import \"./profiles/name.dcdl\" (relative to the containing config file). Saved config is not rewritten.",
        ));
    }
    let config: VirtualProfileConfig = serde_json::from_value(projected.data_json.clone())
        .map_err(|error| Error::RegistryInconsistency(error.to_string()))?;
    let mut profiles = builtin_profile_summaries(Some(&config.profile.default_profile));
    let mut entries = BTreeMap::new();
    let mut source_summaries = Vec::new();
    for entry in config.profile.entries {
        if !entry.selector.starts_with("project:")
            || entry.selector.trim_start_matches("project:").is_empty()
        {
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
        let label = if entry.label.is_empty() {
            entry.selector.trim_start_matches("project:").to_string()
        } else {
            entry.label.clone()
        };
        resolve_profile_artifact_value(
            entry.profile.clone(),
            ProfileSource::Archive {
                archive_id: format!("workspace-config-r{}", state.snapshot.revision),
                source: entry.selector.clone(),
            },
            Path::new("/"),
            "workspace-config-validation",
        )
        .map_err(|error| profile_validation_error("profile_value_invalid", &error.to_string()))?;
        profiles.push(WorkspaceProfileSummary {
            profile_id: entry.selector.clone(),
            selector: entry.selector.clone(),
            label,
            source_kind: "project".to_string(),
            profile_source_id: Some(entry.selector.clone()),
            description: (!entry.description.is_empty()).then(|| entry.description.clone()),
            editable: false,
            is_default: config.profile.default_profile == entry.selector,
            diagnostics: Vec::new(),
        });
        let value_bytes = serde_json::to_vec(&entry.profile).expect("Profile value serializes");
        source_summaries.push(WorkspaceProfileSourceSummary {
            profile_source_id: entry.selector.clone(),
            display_path: format!("profile.entries[{}].profile", source_summaries.len()),
            kind: "evaluated_config".to_string(),
            content_type: "application/json".to_string(),
            content_digest: config_source::digest_bytes(&value_bytes),
            provenance: WorkspaceProfileSourceProvenance::ProjectProfileSourceTree,
            editable: false,
            revision: state.snapshot.revision.to_string(),
            size_bytes: value_bytes.len() as u64,
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
            projection_digest: Some(evaluation.projection_digest.clone()),
            default_profile: Some(config.profile.default_profile),
            profiles,
            sources: source_summaries,
            diagnostics: Vec::new(),
        },
        entries,
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

/// Resolve a Host-owned launch selector against the complete existing registry.
/// Browser candidates remain a narrower UI policy; this does not widen that surface.
pub(crate) fn selector_for_registered_profile(
    projection: &ProfileConfigProjection,
    profile: &str,
) -> Option<worker_runtime::catalog::ProfileSelector> {
    let catalog = builtin_profile_catalog_snapshot();
    let builtin = if profile.starts_with("builtin:") {
        profile.to_string()
    } else {
        format!("builtin:{profile}")
    };
    if catalog.entrypoints.contains_key(&builtin) {
        return Some(worker_runtime::catalog::ProfileSelector::Builtin(builtin));
    }
    projection
        .entries
        .contains_key(profile)
        .then(|| worker_runtime::catalog::ProfileSelector::Named(profile.into()))
}

fn validate_prompt_projection_matches_state(
    workspace_id: &str,
    state: &WorkspaceConfigState,
    projection: &worker::WorkspacePromptProjection,
) -> Result<()> {
    projection
        .validate()
        .map_err(|error| Error::Config(error.to_string()))?;
    let prompt_catalog = projection.catalog();
    let mismatches = [
        (
            projection.workspace_id != workspace_id,
            "workspace identity",
        ),
        (
            projection.source_digest != state.snapshot.digest,
            "source digest",
        ),
        (
            projection.projection_digest != prompt_catalog.catalog_digest,
            "projection digest",
        ),
        (
            prompt_catalog.config_revision != state.snapshot.revision,
            "config revision",
        ),
        (
            prompt_catalog.schema_fingerprint != state.contract.schema_bundle.fingerprint,
            "schema fingerprint",
        ),
        (
            prompt_catalog.toolchain_fingerprint != state.contract.fingerprint,
            "toolchain fingerprint",
        ),
    ]
    .into_iter()
    .filter_map(|(mismatch, label)| mismatch.then_some(label))
    .collect::<Vec<_>>();
    if mismatches.is_empty() {
        Ok(())
    } else {
        Err(Error::Config(format!(
            "Prompt projection does not match Workspace config state: {}",
            mismatches.join(", ")
        )))
    }
}

fn virtual_profile_bundle_id(
    state: &WorkspaceConfigState,
    workspace_id: &str,
    profile_selector: &worker_runtime::catalog::ProfileSelector,
    prompt_catalog: &EffectivePromptCatalog,
    archive: Option<&ProfileSourceArchive>,
) -> Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(b"workspace-profile-launch-v1\0");
    hasher.update(workspace_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(state.snapshot.revision.to_le_bytes());
    hasher.update(b"\0");
    hasher.update(state.snapshot.digest.as_bytes());
    hasher.update(b"\0");
    hasher.update(state.projection_digest.as_bytes());
    hasher.update(b"\0");
    hasher.update(
        serde_json::to_vec(profile_selector).map_err(|error| Error::Config(error.to_string()))?,
    );
    hasher.update(b"\0");
    hasher.update(prompt_catalog.catalog_digest.as_bytes());
    hasher.update(b"\0");
    hasher.update(prompt_catalog.schema_fingerprint.as_bytes());
    hasher.update(b"\0");
    hasher.update(prompt_catalog.toolchain_fingerprint.as_bytes());
    if let Some(archive) = archive {
        hasher.update(b"\0");
        hasher.update(archive.reference.digest.as_bytes());
    }
    let digest = hasher.finalize();
    let identity = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!(
        "workspace-config-profile-r{}-{identity}",
        state.snapshot.revision
    ))
}

pub fn build_virtual_profile_config_bundle(
    projection: &ProfileConfigProjection,
    state: &WorkspaceConfigState,
    workspace_id: &str,
    workspace_created_at: &str,
    selector: &str,
) -> Result<Option<ConfigBundle>> {
    let prompt_projection =
        crate::prompt_settings::project_workspace_prompt_projection(workspace_id, state)?;
    build_virtual_profile_config_bundle_with_prompt_projection(
        projection,
        state,
        workspace_id,
        workspace_created_at,
        selector,
        &prompt_projection,
    )
}

pub(crate) fn builtin_profile_source_archive(
    profile: &worker_runtime::catalog::ProfileSelector,
) -> std::result::Result<ProfileSourceArchive, String> {
    let selected_profile = match profile {
        worker_runtime::catalog::ProfileSelector::Builtin(value) => {
            if value.starts_with("builtin:") {
                value.clone()
            } else {
                format!("builtin:{value}")
            }
        }
        worker_runtime::catalog::ProfileSelector::Named(value) => {
            return Err(format!(
                "builtin profile source catalog has no named entrypoint for '{value}'"
            ));
        }
    };
    let catalog = builtin_profile_catalog_snapshot();
    if !catalog.entrypoints.contains_key(&selected_profile) {
        return Err(format!(
            "builtin profile source catalog has no entrypoint for '{selected_profile}'"
        ));
    }
    ProfileSourceArchive::build(ProfileSourceArchiveInput {
        id: catalog.id.to_owned(),
        entrypoints: catalog.entrypoints,
        imports: catalog.imports,
        sources: catalog.sources,
    })
    .map_err(|error| format!("failed to build builtin profile source archive: {error}"))
}

pub fn build_virtual_profile_config_bundle_with_prompt_projection(
    projection: &ProfileConfigProjection,
    state: &WorkspaceConfigState,
    workspace_id: &str,
    workspace_created_at: &str,
    selector: &str,
    prompt_projection: &worker::WorkspacePromptProjection,
) -> Result<Option<ConfigBundle>> {
    if projection.settings.workspace_id != workspace_id
        || projection.settings.config_revision != Some(state.snapshot.revision)
        || projection.settings.tree_digest.as_deref() != Some(state.snapshot.digest.as_str())
        || projection.settings.projection_digest.as_deref()
            != Some(state.projection_digest.as_str())
    {
        return Err(Error::Config(
            "Profile projection does not match Workspace config state".into(),
        ));
    }
    validate_prompt_projection_matches_state(workspace_id, state, prompt_projection)?;
    let prompt_catalog = prompt_projection.catalog().clone();
    let profile_selector =
        selector_for_registered_profile(projection, selector).ok_or_else(|| {
            Error::InvalidInput(format!(
                "Profile `{selector}` is not a registered launch selector"
            ))
        })?;
    let archive = match projection.entries.get(selector) {
        Some(entry) => Some(build_virtual_profile_archive(selector, entry, state)?),
        None => Some(builtin_profile_source_archive(&profile_selector).map_err(Error::Store)?),
    };
    let bundle_id = virtual_profile_bundle_id(
        state,
        workspace_id,
        &profile_selector,
        &prompt_catalog,
        archive.as_ref(),
    )?;
    let bundle = ConfigBundle {
        metadata: ConfigBundleMetadata {
            id: bundle_id,
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
        prompt_catalog: Some(prompt_catalog),
        profile_source_archive: archive,
        profile_source_archive_handle: None,
    }
    .with_computed_digest();
    Ok(Some(bundle))
}

pub(crate) fn resolve_profile_manifest_from_config_bundle(
    bundle: &ConfigBundle,
    selector: &str,
) -> Result<manifest::WorkerManifest> {
    let archive = bundle.profile_source_archive.as_ref().ok_or_else(|| {
        Error::Config(format!(
            "resolved Profile {selector:?} is missing its source archive"
        ))
    })?;
    let verified = archive.verify().map_err(|error| {
        Error::Config(format!("failed to verify Profile {selector:?}: {error}"))
    })?;
    verified
        .resolve_profile(selector, Path::new("/"), "worker-launch-validation")
        .map_err(|error| Error::Config(format!("failed to resolve Profile {selector:?}: {error}")))
}

fn build_virtual_profile_archive(
    selector: &str,
    entry: &VirtualProfileEntry,
    state: &WorkspaceConfigState,
) -> Result<ProfileSourceArchive> {
    let builtin_sources = config_source::SnapshotEnvironment::new(state.snapshot.clone())
        .builtin_import_sources(&state.contract)
        .map_err(|diagnostics| {
            profile_validation_error(
                "profile_builtin_snapshot_invalid",
                &serde_json::to_string(&diagnostics)
                    .unwrap_or_else(|_| "builtin import snapshot failed".to_string()),
            )
        })?;
    ProfileSourceArchive::build_evaluated_profile_with_builtin_sources(
        format!("workspace-config-profile-r{}", state.snapshot.revision),
        selector.to_string(),
        entry.profile.clone(),
        builtin_sources,
    )
    .map_err(|error| profile_validation_error("profile_value_archive_invalid", &error.to_string()))
}

pub fn workspace_metadata_settings(
    workspace: &WorkspaceRecord,
) -> WorkspaceMetadataSettingsResponse {
    WorkspaceMetadataSettingsResponse {
        workspace_id: workspace.workspace_id.clone(),
        display_name: workspace.display_name.clone(),
        created_at: workspace.created_at.clone(),
        revision: workspace.updated_at.clone(),
        source: "server_db".to_string(),
        diagnostics: Vec::new(),
    }
}

pub fn sanitize_workspace_display_name(value: &str) -> Result<String> {
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
#[cfg(test)]
mod tests {
    use super::*;
    use config_source::{ConfigContentType, VirtualPath};

    #[test]
    fn workspace_metadata_projects_server_database_record_without_filesystem_diagnostics() {
        let workspace = WorkspaceRecord {
            workspace_id: "workspace-a".to_string(),
            owner_account_id: "owner-account".to_string(),
            display_name: "Workspace A".to_string(),
            state: "active".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-02T00:00:00Z".to_string(),
        };

        let settings = workspace_metadata_settings(&workspace);

        assert_eq!(settings.workspace_id, workspace.workspace_id);
        assert_eq!(settings.display_name, workspace.display_name);
        assert_eq!(settings.created_at, workspace.created_at);
        assert_eq!(settings.revision, workspace.updated_at);
        assert_eq!(settings.source, "server_db");
        assert!(settings.diagnostics.is_empty());
    }

    #[test]
    fn workspace_display_name_validation_is_bounded() {
        assert_eq!(
            sanitize_workspace_display_name("  Workspace A  ").unwrap(),
            "Workspace A"
        );
        assert!(sanitize_workspace_display_name("\n").is_err());
        assert!(sanitize_workspace_display_name(&"a".repeat(121)).is_err());
    }

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
                r#"{ profile = { default_profile = "project:alpha"; entries = [{ selector = "project:alpha"; profile = import "./profiles/alpha.dcdl"; label = "Alpha"; }]; }; }"#,
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
            Some("profiles/evaluated.json")
        );
        assert_eq!(archive.reference.source_graph.source_count, 1);
    }

    #[test]
    fn virtual_config_projection_materializes_import_closure() {
        let state = virtual_state(vec![
            config_source::ConfigEntry::new(
                VirtualPath::parse("main.dcdl").unwrap(),
                ConfigContentType::Decodal,
                r#"{ profile = { entries = [{ selector = "project:alpha"; profile = import "./profiles/alpha.dcdl"; }]; }; }"#,
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
        assert_eq!(archive.reference.source_graph.source_count, 1);
        assert_eq!(archive.reference.source_graph.import_count, 0);
    }

    #[test]
    fn virtual_config_launch_bundle_identity_is_profile_specific_and_stable() {
        let state = virtual_state(vec![
            config_source::ConfigEntry::new(
                VirtualPath::parse("main.dcdl").unwrap(),
                ConfigContentType::Decodal,
                "{}",
            )
            .unwrap(),
        ]);
        let projection = project_profiles_from_workspace_config("workspace-test", &state).unwrap();

        let companion = build_virtual_profile_config_bundle(
            &projection,
            &state,
            "workspace-test",
            "2026-01-01T00:00:00Z",
            "builtin:companion",
        )
        .unwrap()
        .unwrap();
        let companion_retry = build_virtual_profile_config_bundle(
            &projection,
            &state,
            "workspace-test",
            "2026-01-01T00:00:00Z",
            "builtin:companion",
        )
        .unwrap()
        .unwrap();
        let coder = build_virtual_profile_config_bundle(
            &projection,
            &state,
            "workspace-test",
            "2026-01-01T00:00:00Z",
            "builtin:coder",
        )
        .unwrap()
        .unwrap();

        assert_eq!(companion.metadata.id, companion_retry.metadata.id);
        assert_eq!(companion.metadata.digest, companion_retry.metadata.digest);
        assert_ne!(companion.metadata.id, coder.metadata.id);
        assert_ne!(companion.metadata.digest, coder.metadata.digest);
        assert!(
            companion
                .metadata
                .id
                .starts_with("workspace-config-profile-r7-")
        );
        assert!(
            coder
                .metadata
                .id
                .starts_with("workspace-config-profile-r7-")
        );
    }

    #[test]
    fn virtual_config_launch_bundle_rejects_prompt_projection_from_other_revision() {
        let state = virtual_state(vec![
            config_source::ConfigEntry::new(
                VirtualPath::parse("main.dcdl").unwrap(),
                ConfigContentType::Decodal,
                "{}",
            )
            .unwrap(),
        ]);
        let projection = project_profiles_from_workspace_config("workspace-test", &state).unwrap();
        let prompt_projection =
            crate::prompt_settings::project_workspace_prompt_projection("workspace-test", &state)
                .unwrap();
        let mut mismatched_state = state.clone();
        mismatched_state.snapshot.revision += 1;

        let error = build_virtual_profile_config_bundle_with_prompt_projection(
            &projection,
            &mismatched_state,
            "workspace-test",
            "2026-01-01T00:00:00Z",
            "builtin:coder",
            &prompt_projection,
        )
        .unwrap_err();
        assert!(error.to_string().contains("does not match"));
    }

    #[test]
    fn virtual_config_launch_bundle_identity_separates_project_profile_archives() {
        let state = virtual_state(vec![
            config_source::ConfigEntry::new(
                VirtualPath::parse("main.dcdl").unwrap(),
                ConfigContentType::Decodal,
                r#"{ profile = { entries = [
                    { selector = "project:alpha"; profile = import "./profiles/alpha.dcdl"; },
                    { selector = "project:beta"; profile = import "./profiles/beta.dcdl"; },
                ]; }; }"#,
            )
            .unwrap(),
            config_source::ConfigEntry::new(
                VirtualPath::parse("profiles/alpha.dcdl").unwrap(),
                ConfigContentType::Decodal,
                valid_decodal("alpha"),
            )
            .unwrap(),
            config_source::ConfigEntry::new(
                VirtualPath::parse("profiles/beta.dcdl").unwrap(),
                ConfigContentType::Decodal,
                valid_decodal("beta"),
            )
            .unwrap(),
        ]);
        let projection = project_profiles_from_workspace_config("workspace-test", &state).unwrap();
        let alpha = build_virtual_profile_config_bundle(
            &projection,
            &state,
            "workspace-test",
            "2026-01-01T00:00:00Z",
            "project:alpha",
        )
        .unwrap()
        .unwrap();
        let beta = build_virtual_profile_config_bundle(
            &projection,
            &state,
            "workspace-test",
            "2026-01-01T00:00:00Z",
            "project:beta",
        )
        .unwrap()
        .unwrap();

        assert_ne!(alpha.metadata.id, beta.metadata.id);
        assert_ne!(alpha.metadata.digest, beta.metadata.digest);
        assert_ne!(
            alpha.profile_source_archive.unwrap().reference.digest,
            beta.profile_source_archive.unwrap().reference.digest
        );
    }

    #[test]
    fn value_profile_authoring_uses_current_provider_schema_without_inserting_defaults() {
        let bundle =
            config_source::WorkspaceConfigSchemaBundle::compose([ProfileConfigSchemaProvider
                .contribution()
                .unwrap()])
            .unwrap();
        let snapshot = config_source::ConfigTreeSnapshot::from_entries(1, [config_source::ConfigEntry::new(
            VirtualPath::parse("main.dcdl").unwrap(), ConfigContentType::Decodal,
            r#"{ profile = { entries = [{ selector = "project:alpha"; profile = {}; }]; }; } as WorkspaceConfigSchema"#,
        ).unwrap()]).unwrap();
        let environment = config_source::SnapshotEnvironment::new(snapshot.clone())
            .with_schema_bundle(bundle.clone());
        for (body, token, label) in [
            (
                "{ profile = { entries = [{ sel }] } } as WorkspaceConfigSchema",
                "sel",
                "selector",
            ),
            (
                "{ profile = { entries = [{ pro }] } } as WorkspaceConfigSchema",
                "pro",
                "profile",
            ),
            (
                "{ profile = { entries = [{ profile = { wor } }] } } as WorkspaceConfigSchema",
                "wor",
                "worker",
            ),
            (
                "{ profile = { entries = [{ profile = { worker = { mo } } }] } } as WorkspaceConfigSchema",
                "mo",
                "mode",
            ),
            (
                "{ profile = { entries = [{ profile = { feature = { ti } } }] } } as WorkspaceConfigSchema",
                "ti",
                "ticket",
            ),
        ] {
            let offset = body.rfind(token).unwrap() + token.len();
            let completion = environment
                .complete_config(
                    &VirtualPath::parse("main.dcdl").unwrap(),
                    body,
                    offset,
                    true,
                )
                .unwrap()
                .unwrap();
            assert!(
                completion.items.iter().any(|item| item.label == label),
                "{label}: {completion:?}"
            );
        }
        for invalid in [
            r#"{ profile = { entries = [{ selector = 42; profile = {}; }]; }; } as WorkspaceConfigSchema"#,
            r#"{ profile = { entries = [{ selector = "project:alpha"; profile = 42; }]; }; } as WorkspaceConfigSchema"#,
        ] {
            assert!(
                !environment
                    .analyze(&VirtualPath::parse("main.dcdl").unwrap(), Some(invalid))
                    .is_empty()
            );
        }
        let evaluation = environment
            .evaluate_contract(&config_source::ToolchainContract::with_schema_bundle(
                config_source::DEFAULT_SCHEMA_VERSION,
                vec![VirtualPath::parse("main.dcdl").unwrap()],
                config_source::DEFAULT_IMPORT_POLICY_VERSION,
                bundle,
            ))
            .unwrap();
        assert_eq!(
            evaluation.projections[0]
                .data_json
                .pointer("/profile/entries/0/profile"),
            Some(&serde_json::json!({}))
        );
    }
}
