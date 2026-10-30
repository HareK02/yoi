//! Catalog-backed Plugin configuration. The virtual config tree remains the only
//! configuration authority; static metadata resolution never instantiates a guest.

use std::collections::{BTreeMap, BTreeSet};

use config_source::{
    ConfigSchemaContribution, ConfigTreeSnapshot, EvaluationResult, SnapshotEnvironment,
    ToolchainContract, WorkspaceConfigSchemaBundle,
};
use rusqlite::Transaction;
use serde::{Deserialize, Serialize};
use server_api::{PluginArtifact, PluginGrantMetadata};

use crate::config_source::WorkspaceConfigSchemaProvider;
use crate::plugin_binding::PluginConnectionBinding;
use crate::plugin_catalog::{
    PluginReference, PluginReferenceKind, plugin_artifact_admit, plugin_reference_put,
    plugin_reference_remove,
};
use crate::{Error, Result};

const PROVIDER: &str = "builtin:plugins";
const BOOTSTRAP: &str = include_str!("../../../resources/config-schema/plugins.dcdl");
const CONFIG_OWNER: &str = "workspace-config";

#[derive(Default)]
pub struct PluginConfigSchemaProvider;
impl WorkspaceConfigSchemaProvider for PluginConfigSchemaProvider {
    fn contribution(&self) -> Result<ConfigSchemaContribution> {
        ConfigSchemaContribution::new(
            PROVIDER,
            "plugins",
            config_source::digest_bytes(BOOTSTRAP.as_bytes()),
            BOOTSTRAP,
        )
        .map_err(config_error)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginEnablement {
    pub installation_id: String,
    pub expected_last_operation: String,
    pub package_digest: String,
    pub artifact_digest: String,
    pub tools: Vec<String>,
    #[serde(default)]
    pub services: Vec<String>,
    #[serde(default)]
    pub ticket_events: bool,
    pub logging: bool,
    #[serde(default)]
    pub ticket_read: bool,
    #[serde(default)]
    pub ticket_data_read: Vec<manifest::PluginTicketDataGrant>,
    #[serde(default)]
    pub ticket_data_write: Vec<manifest::PluginTicketDataGrant>,
    pub connections: BTreeMap<String, String>,
    pub config: serde_json::Value,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginSettings {
    pub enabled: BTreeMap<String, PluginEnablement>,
    pub bindings: BTreeMap<String, PluginConnectionBinding>,
}

#[derive(Debug, Clone)]
pub struct ResolvedPluginSettings {
    pub settings: PluginSettings,
    pub artifacts: BTreeMap<String, PluginArtifact>,
}

fn config_error(error: impl std::fmt::Display) -> Error {
    Error::InvalidInput(format!("Plugin config: {error}"))
}

fn contract(bundle: WorkspaceConfigSchemaBundle) -> ToolchainContract {
    ToolchainContract::with_schema_bundle(
        config_source::DEFAULT_SCHEMA_VERSION,
        vec![
            config_source::VirtualPath::parse(crate::config_source::MAIN_CONFIG_ENTRYPOINT)
                .expect("main path"),
        ],
        config_source::DEFAULT_IMPORT_POLICY_VERSION,
        bundle,
    )
}

fn evaluate(
    snapshot: &ConfigTreeSnapshot,
    contract: &ToolchainContract,
) -> Result<EvaluationResult> {
    SnapshotEnvironment::new(snapshot.clone())
        .evaluate_contract(contract)
        .map_err(|diagnostics| {
            config_error(
                serde_json::to_string(&diagnostics)
                    .unwrap_or_else(|_| "invalid static config".into()),
            )
        })
}

pub fn settings_from_evaluation(evaluation: &EvaluationResult) -> Result<PluginSettings> {
    let value = evaluation
        .projections
        .first()
        .ok_or_else(|| config_error("missing projection"))?;
    let root = &value.data_json;
    match root.get("plugins") {
        Some(value) => serde_json::from_value(value.clone()).map_err(config_error),
        None => Ok(PluginSettings::default()),
    }
}

/// Resolve core references before composing the closed artifact-specific schema.
/// Both reads use the owner's transaction; the resulting bundle fixes the exact
/// catalog operation as well as both byte identities, not a generic counter.
pub(crate) fn evaluate_catalog_config(
    tx: &Transaction<'_>,
    workspace: &str,
    snapshot: &ConfigTreeSnapshot,
    bundle: WorkspaceConfigSchemaBundle,
) -> Result<(ToolchainContract, EvaluationResult, ResolvedPluginSettings)> {
    let mut providers = bundle
        .contributions
        .into_iter()
        .filter(|p| p.provider_id != PROVIDER)
        .collect::<Vec<_>>();
    providers.push(PluginConfigSchemaProvider.contribution()?);
    let bootstrap =
        contract(WorkspaceConfigSchemaBundle::compose(providers.clone()).map_err(config_error)?);
    let core = evaluate(snapshot, &bootstrap)?;
    let settings = settings_from_evaluation(&core)?;
    crate::plugin_binding::validate_bindings(
        &settings.bindings,
        &crate::plugin_binding::PluginConnectionPolicyCeiling::default(),
    )
    .map_err(config_error)?;
    if settings.enabled.len() > 128 {
        return Err(config_error("too many enablements"));
    }
    let mut artifacts = BTreeMap::new();
    let mut schema_identities = BTreeMap::new();
    let mut installations = BTreeSet::new();
    for (alias, enabled) in &settings.enabled {
        if !installations.insert(&enabled.installation_id) {
            return Err(config_error("duplicate enabled installation"));
        }
        if !safe_name(alias) {
            return Err(config_error("invalid enablement alias"));
        }
        let artifact = plugin_artifact_admit(
            tx,
            workspace,
            &enabled.installation_id,
            &enabled.expected_last_operation,
            &enabled.package_digest,
            &enabled.artifact_digest,
        )?;
        validate_enablement(enabled, &artifact, &settings)?;
        let schema = &artifact.config_schema;
        let key = (schema.identity.clone(), schema.version.clone());
        let value = (schema.namespace.clone(), schema.digest.clone());
        if let Some(previous) = schema_identities.insert(key, value.clone()) {
            if previous != value {
                return Err(config_error("schema identity collision"));
            }
        }
        artifacts.insert(alias.clone(), artifact);
    }
    // Each alias has its own closed schema. Reusing an artifact with distinct
    // bindings is intentional and does not manufacture another schema authority.
    let enabled_fields = artifacts.iter().map(|(alias, artifact)| {
        format!("{} = {{ installation_id = String; expected_last_operation = String; package_digest = String; artifact_digest = String; tools = [...String]; services = [...String] default []; ticket_events = Bool default false; logging = Bool default false; ticket_read = Bool default false; ticket_data_read = [...{{ collection = String; schema_id = String; schema_version = String; }}] default []; ticket_data_write = [...{{ collection = String; schema_id = String; schema_version = String; }}] default []; connections = {{ ...String }} default {{}}; config = ({}); }};", alias, artifact.config_schema.dcdl)
    }).collect::<Vec<_>>().join("\n");
    let enabled_schema = if enabled_fields.is_empty() {
        "{} default {}".to_string()
    } else {
        format!("{{{enabled_fields}}}")
    };
    let (prefix, remaining) = BOOTSTRAP
        .split_once("enabled = ")
        .ok_or_else(|| config_error("missing bootstrap enablement"))?;
    let (_, suffix) = remaining
        .split_once(";\n        bindings = ")
        .ok_or_else(|| config_error("missing bootstrap bindings"))?;
    let source = format!("{prefix}enabled = {enabled_schema};\n        bindings = {suffix}");
    let identities = settings
        .enabled
        .iter()
        .map(|(alias, e)| {
            (
                alias,
                &e.installation_id,
                &e.expected_last_operation,
                &e.package_digest,
                &e.artifact_digest,
                &artifacts[alias].config_schema,
            )
        })
        .collect::<Vec<_>>();
    let version = format!(
        "{}:{}",
        config_source::digest_bytes(BOOTSTRAP.as_bytes()),
        config_source::digest_bytes(&serde_json::to_vec(&identities).map_err(config_error)?)
    );
    providers.retain(|p| p.provider_id != PROVIDER);
    providers.push(
        ConfigSchemaContribution::new(PROVIDER, "plugins", version, source)
            .map_err(config_error)?,
    );
    let full_contract =
        contract(WorkspaceConfigSchemaBundle::compose(providers).map_err(config_error)?);
    let full = evaluate(snapshot, &full_contract)?;
    let final_settings = settings_from_evaluation(&full)?;
    let resolved = ResolvedPluginSettings {
        settings: final_settings,
        artifacts,
    };
    validate_profile_selections(&full, &resolved)?;
    Ok((full_contract, full, resolved))
}

fn safe_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .as_bytes()
            .first()
            .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_')
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn validate_enablement(
    enabled: &PluginEnablement,
    artifact: &PluginArtifact,
    settings: &PluginSettings,
) -> Result<()> {
    if artifact.schema_version != 2
        || !matches!(
            artifact.world.as_str(),
            manifest::plugin::PLUGIN_COMPONENT_TOOL_WORLD
                | manifest::plugin::PLUGIN_COMPONENT_HTTPS_TOOL_WORLD
                | manifest::plugin::PLUGIN_COMPONENT_LOGGED_TOOL_WORLD
                | manifest::plugin::PLUGIN_COMPONENT_HTTPS_LOGGED_TOOL_WORLD
                | "yoi:plugin/ticket-tool-plugin@2.1.0"
                | "yoi:plugin/ticket-service-plugin@2.2.0"
        )
        || artifact.required_imports.iter().any(|i| {
            !matches!(
                i.as_str(),
                "yoi:host/errors@2.0.0"
                    | "yoi:host/connections@2.0.0"
                    | "yoi:host/http@2.0.0"
                    | "yoi:host/logging@2.0.0"
                    | "yoi:host/objects@2.1.0"
                    | "yoi:host/ticket-read@2.1.0"
                    | "yoi:host/extension-data@2.1.0"
            )
        })
    {
        return Err(config_error("unsupported Component API"));
    }
    let tools = enabled.tools.iter().collect::<BTreeSet<_>>();
    if tools.len() != enabled.tools.len()
        || tools
            .iter()
            .any(|name| !artifact.tools.iter().any(|tool| &tool.name == *name))
    {
        return Err(config_error("unknown or duplicate Tool selection"));
    }
    if enabled.services.iter().collect::<BTreeSet<_>>().len() != enabled.services.len()
        || enabled
            .services
            .iter()
            .any(|name| !artifact.services.iter().any(|s| &s.name == name))
        || (enabled.ticket_events
            && (!artifact
                .requested_grants
                .contains(&PluginGrantMetadata::TicketEvents)
                || artifact.world != "yoi:plugin/ticket-service-plugin@2.2.0"))
    {
        return Err(config_error(
            "Service selection/grant exceeds package request",
        ));
    }
    if enabled.logging
        && !artifact
            .requested_grants
            .contains(&PluginGrantMetadata::Logging)
    {
        return Err(config_error("logging exceeds package request"));
    }
    if enabled.ticket_read
        && !artifact
            .requested_grants
            .contains(&PluginGrantMetadata::TicketRead)
    {
        return Err(config_error("Ticket read exceeds package request"));
    }
    for (write, grants) in [
        (false, &enabled.ticket_data_read),
        (true, &enabled.ticket_data_write),
    ] {
        if grants.iter().collect::<BTreeSet<_>>().len() != grants.len() {
            return Err(config_error("duplicate Ticket data grant"));
        }
        for grant in grants {
            let metadata = if write {
                PluginGrantMetadata::TicketDataWrite {
                    collection: grant.collection.clone(),
                    schema_id: grant.schema_id.clone(),
                    schema_version: grant.schema_version.clone(),
                }
            } else {
                PluginGrantMetadata::TicketDataRead {
                    collection: grant.collection.clone(),
                    schema_id: grant.schema_id.clone(),
                    schema_version: grant.schema_version.clone(),
                }
            };
            if !artifact.requested_grants.contains(&metadata) {
                return Err(config_error("Ticket data grant exceeds package request"));
            }
        }
    }
    for (slot, binding) in &enabled.connections {
        let requested = artifact
            .connections
            .iter()
            .find(|s| &s.name == slot)
            .ok_or_else(|| config_error("undeclared connection slot"))?;
        let binding = settings
            .bindings
            .get(binding)
            .ok_or_else(|| config_error("missing connection binding"))?;
        // A binding can be narrower than the package request, never wider.
        if binding
            .methods
            .iter()
            .any(|m| !requested.methods.contains(m))
            || binding.path_prefixes.iter().any(|path| {
                !requested.path_prefixes.iter().any(|prefix| {
                    prefix == "/"
                        || path == prefix
                        || path
                            .strip_prefix(prefix)
                            .is_some_and(|rest| rest.starts_with('/'))
                })
            })
        {
            return Err(config_error("binding exceeds package request"));
        }
    }
    if serde_json::to_vec(&enabled.config)
        .map_err(config_error)?
        .len()
        > 64 * 1024
    {
        return Err(config_error("non-secret config exceeds 64 KiB"));
    }
    Ok(())
}

pub(crate) fn sync_config_references(
    tx: &Transaction<'_>,
    workspace: &str,
    resolved: &ResolvedPluginSettings,
) -> Result<()> {
    let existing = {
        let mut statement = tx.prepare("SELECT installation_id, artifact_digest FROM plugin_references WHERE workspace_id=?1 AND kind='config' AND owner=?2")?;
        statement
            .query_map(rusqlite::params![workspace, CONFIG_OWNER], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?
    };
    for (installation, digest) in existing {
        plugin_reference_remove(
            tx,
            workspace,
            PluginReferenceKind::Config,
            CONFIG_OWNER,
            &installation,
            &digest,
        )?;
    }
    for enabled in resolved.settings.enabled.values() {
        plugin_reference_put(
            tx,
            workspace,
            &PluginReference {
                kind: PluginReferenceKind::Config,
                owner: CONFIG_OWNER.into(),
                installation_id: enabled.installation_id.clone(),
                expected_last_operation: enabled.expected_last_operation.clone(),
                package_digest: enabled.package_digest.clone(),
                artifact_digest: enabled.artifact_digest.clone(),
            },
        )?;
    }
    Ok(())
}

fn validate_profile_selections(
    evaluation: &EvaluationResult,
    resolved: &ResolvedPluginSettings,
) -> Result<()> {
    let root = &evaluation.projections[0].data_json;
    if let Some(entries) = root
        .pointer("/profile/entries")
        .and_then(serde_json::Value::as_array)
    {
        for entry in entries {
            if let Some(plugins) = entry.pointer("/profile/plugins") {
                let plugins: manifest::PluginProfileConfig =
                    serde_json::from_value(plugins.clone()).map_err(config_error)?;
                project_profile_plugins(resolved, &plugins)?;
            }
        }
    }
    Ok(())
}

fn project_profile_plugins(
    resolved: &ResolvedPluginSettings,
    profile: &manifest::PluginProfileConfig,
) -> Result<Vec<crate::plugin_plan::WorkerPluginPin>> {
    use crate::plugin_plan::{
        BindingPolicyReference, PluginGrant, PluginPinProjection, PluginSurface, WorkerPluginPin,
    };
    let mut selected = BTreeSet::new();
    let mut pins = Vec::new();
    for selection in &profile.selections {
        if !selected.insert(&selection.installation_id) {
            return Err(config_error("duplicate Profile installation"));
        }
        let (alias, enabled) = resolved
            .settings
            .enabled
            .iter()
            .find(|(_, e)| e.installation_id == selection.installation_id)
            .ok_or_else(|| {
                config_error("Profile selects an installation not enabled by Workspace")
            })?;
        if selection
            .tools
            .iter()
            .any(|tool| !enabled.tools.contains(tool))
            || selection
                .services
                .iter()
                .any(|s| !enabled.services.contains(s))
            || (selection.ticket_events && !enabled.ticket_events)
            || (selection.logging && !enabled.logging)
            || (selection.ticket_read && !enabled.ticket_read)
            || selection
                .ticket_data_read
                .iter()
                .any(|grant| !enabled.ticket_data_read.contains(grant))
            || selection
                .ticket_data_write
                .iter()
                .any(|grant| !enabled.ticket_data_write.contains(grant))
            || selection
                .https_slots
                .iter()
                .any(|slot| !enabled.connections.contains_key(slot))
        {
            return Err(config_error(
                "Profile exceeds Workspace surface/grant ceiling",
            ));
        }
        let mut config = enabled.config.clone();
        let object = config
            .as_object_mut()
            .ok_or_else(|| config_error("non-secret config must be an object"))?;
        for (key, value) in &selection.config {
            object.insert(key.clone(), value.clone());
        }
        let config =
            evaluate_effective_config(&config, &resolved.artifacts[alias].config_schema.dcdl)?;
        let mut grants = Vec::new();
        if selection.ticket_events {
            grants.push(PluginGrant::TicketEvents);
        }
        if selection.logging {
            grants.push(PluginGrant::Logging);
        }
        if selection.ticket_read {
            grants.push(PluginGrant::TicketRead);
        }
        for grant in &selection.ticket_data_read {
            grants.push(PluginGrant::TicketDataRead {
                collection: grant.collection.clone(),
                schema_id: grant.schema_id.clone(),
                schema_version: grant.schema_version.clone(),
            });
        }
        for grant in &selection.ticket_data_write {
            grants.push(PluginGrant::TicketDataWrite {
                collection: grant.collection.clone(),
                schema_id: grant.schema_id.clone(),
                schema_version: grant.schema_version.clone(),
            });
        }
        let mut bindings = Vec::new();
        for slot in &selection.https_slots {
            let binding_id = &enabled.connections[slot];
            let binding = &resolved.settings.bindings[binding_id];
            grants.push(PluginGrant::Https { slot: slot.clone() });
            bindings.push(BindingPolicyReference {
                slot: slot.clone(),
                binding_id: binding_id.clone(),
                policy_digest: binding.digest(),
            });
        }
        pins.push(WorkerPluginPin::from_artifact(
            enabled.installation_id.clone(),
            enabled.expected_last_operation.clone(),
            &resolved.artifacts[alias],
            PluginPinProjection {
                config,
                surfaces: [
                    (!selection.tools.is_empty()).then_some(PluginSurface::Tool),
                    (!selection.services.is_empty()).then_some(PluginSurface::Service),
                ]
                .into_iter()
                .flatten()
                .collect(),
                tools: selection.tools.clone(),
                services: selection.services.clone(),
                grants,
                bindings,
            },
        )?);
    }
    Ok(pins)
}

fn evaluate_effective_config(value: &serde_json::Value, schema: &str) -> Result<serde_json::Value> {
    use config_source::{ConfigContentType, ConfigEntry, VirtualPath};
    if serde_json::to_vec(value).map_err(config_error)?.len() > 64 * 1024 {
        return Err(config_error("non-secret config exceeds 64 KiB"));
    }
    let path = VirtualPath::parse(crate::config_source::MAIN_CONFIG_ENTRYPOINT).expect("main path");
    let snapshot = ConfigTreeSnapshot::from_entries([ConfigEntry::new(
        path,
        ConfigContentType::Decodal,
        format!(
            "{{ config = {}; }} as WorkspaceConfigSchema",
            render_value(value)?
        ),
    )
    .map_err(config_error)?])
    .map_err(config_error)?;
    let bundle = WorkspaceConfigSchemaBundle::compose([ConfigSchemaContribution::new(
        "plugin-effective-config",
        "config",
        "1",
        format!("{{ config = ({schema}); }}"),
    )
    .map_err(config_error)?])
    .map_err(config_error)?;
    // Static singleton fields can concretize omitted nested override values.
    // Pin the complete projection, not just the candidate that passed validation.
    let evaluated = evaluate(&snapshot, &contract(bundle))?;
    let config = evaluated
        .projections
        .first()
        .and_then(|projection| projection.data_json.get("config"))
        .filter(|config| config.is_object())
        .ok_or_else(|| config_error("effective Plugin config projection must be an object"))?;
    if serde_json::to_vec(config).map_err(config_error)?.len() > 64 * 1024 {
        return Err(config_error("non-secret config exceeds 64 KiB"));
    }
    Ok(config.clone())
}

pub(crate) fn render_value(value: &serde_json::Value) -> Result<String> {
    Ok(match value {
        serde_json::Value::Object(fields) => format!(
            "{{ {} }}",
            fields
                .iter()
                .map(|(key, value)| Ok(format!(
                    "{} = {};",
                    if safe_name(key) {
                        key.clone()
                    } else {
                        return Err(config_error("config key is not a static identifier"));
                    },
                    render_value(value)?
                )))
                .collect::<Result<Vec<_>>>()?
                .join(" ")
        ),
        serde_json::Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(render_value)
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        ),
        serde_json::Value::Null => return Err(config_error("null is not a Plugin config value")),
        value => serde_json::to_string(value).map_err(config_error)?,
    })
}

/// Build a maximum immutable snapshot from the current Workspace config and an
/// actual registered Profile. The Host supplies execution/delegation identities;
/// a caller-provided projection, binding digest or package payload is never used.
/// The owning Worker-create transaction may further narrow this snapshot, then
/// persist it with `plugin_plan_create` and its own launch fingerprint atomically.
/// This does not admit execution, resolve secrets or start/restore a Worker.
pub fn resolve_worker_plugin_plan(
    tx: &Transaction<'_>,
    workspace: &str,
    expected_content_digest: &str,
    profile_selector: &str,
    execution: crate::plugin_plan::HostExecutionIdentity,
) -> Result<crate::plugin_plan::WorkerPluginExecutionPlan> {
    use crate::plugin_plan::{
        ConfigProvenance, ProfileProvenance, WorkerPluginExecutionPlan, WorkerPluginPlanInput,
    };
    let state = crate::config_source::load_state(tx, workspace)?
        .ok_or_else(|| config_error("Workspace config is not materialized"))?;
    if state.snapshot.digest != expected_content_digest {
        return Err(Error::WorkspaceConfigConflict(
            "plan source content changed".into(),
        ));
    }
    let (contract, evaluation, resolved) = evaluate_catalog_config(
        tx,
        workspace,
        &state.snapshot,
        state.contract.schema_bundle.clone(),
    )?;
    if state.contract != contract || state.projection_digest != evaluation.projection_digest {
        return Err(Error::WorkspaceConfigConflict(
            "plan source catalog/schema changed".into(),
        ));
    }
    let profile = resolve_profile(&evaluation, &state.snapshot.digest, profile_selector)?;
    let projection_digest = config_source::digest_bytes(
        &serde_json::to_vec(&profile.manifest_snapshot).map_err(config_error)?,
    );
    WorkerPluginExecutionPlan::canonicalize(WorkerPluginPlanInput {
        workspace_id: workspace.into(),
        config: ConfigProvenance {
            content_digest: state.snapshot.digest,
            toolchain_fingerprint: contract.fingerprint,
        },
        profile: ProfileProvenance {
            identity: profile_selector.into(),
            projection_digest,
        },
        execution,
        plugins: project_profile_plugins(&resolved, &profile.manifest.plugins)?,
    })
    .map_err(Into::into)
}

fn resolve_profile(
    evaluation: &EvaluationResult,
    content_digest: &str,
    selector: &str,
) -> Result<manifest::ResolvedProfile> {
    let root = &evaluation.projections[0].data_json;
    if selector.starts_with("project:") {
        let value = root
            .pointer("/profile/entries")
            .and_then(serde_json::Value::as_array)
            .and_then(|entries| {
                entries.iter().find(|entry| {
                    entry.get("selector").and_then(serde_json::Value::as_str) == Some(selector)
                })
            })
            .and_then(|entry| entry.get("profile"))
            .ok_or_else(|| config_error("unknown registered Profile"))?;
        manifest::resolve_profile_artifact_value(
            value.clone(),
            manifest::ProfileSource::Archive {
                archive_id: format!("workspace-config:{content_digest}"),
                source: selector.into(),
            },
            std::path::Path::new("/"),
            "plugin-plan-projection",
        )
        .map_err(config_error)
    } else if selector.starts_with("builtin:") {
        let registry = manifest::ProfileDiscovery::with_sources(None, None)
            .discover()
            .map_err(config_error)?;
        manifest::ProfileResolver::new()
            .with_workspace_base("/")
            .resolve_from_registry(
                &manifest::ProfileSelector::source_named(
                    manifest::ProfileRegistrySource::Builtin,
                    selector.trim_start_matches("builtin:"),
                ),
                &registry,
                manifest::ProfileResolveOptions::with_worker_name("plugin-plan-projection"),
            )
            .map_err(config_error)
    } else {
        Err(config_error("unknown registered Profile selector"))
    }
}

/// Check the owner-supplied launch snapshot against config authority on first
/// create. Exact replay deliberately uses the saved snapshot instead of rereading
/// changed config. Delegation can only remove pins/tools/grants from this maximum.
pub(crate) fn validate_plan_source(
    tx: &Transaction<'_>,
    plan: &crate::plugin_plan::WorkerPluginExecutionPlan,
) -> Result<()> {
    let maximum = resolve_worker_plugin_plan(
        tx,
        plan.workspace_id(),
        &plan.config().content_digest,
        &plan.profile().identity,
        plan.execution().clone(),
    )?;
    if maximum.config() != plan.config() || maximum.profile() != plan.profile() {
        return Err(config_error("plan source provenance mismatch"));
    }
    for pin in plan.plugins() {
        let upper = maximum
            .plugins()
            .iter()
            .find(|p| p.installation_id() == pin.installation_id())
            .ok_or_else(|| config_error("plan exceeds selected Profile"))?;
        let lower = pin.projection();
        let max = upper.projection();
        if pin.package_digest() != upper.package_digest()
            || pin.artifact_digest() != upper.artifact_digest()
            || pin.expected_last_operation() != upper.expected_last_operation()
            || pin.config_digest() != upper.config_digest()
            || pin.config_schema() != upper.config_schema()
            || lower.config != max.config
            || lower.tools.iter().any(|tool| !max.tools.contains(tool))
            || lower.grants.iter().any(|grant| !max.grants.contains(grant))
            || lower
                .bindings
                .iter()
                .any(|binding| !max.bindings.contains(binding))
            || lower
                .surfaces
                .iter()
                .any(|surface| !max.surfaces.contains(surface))
        {
            return Err(config_error("plan exceeds config/Profile upper bound"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;

pub(crate) fn base_schema_bundle(
    bundle: &WorkspaceConfigSchemaBundle,
) -> Result<WorkspaceConfigSchemaBundle> {
    let mut contributions = bundle
        .contributions
        .iter()
        .filter(|p| p.provider_id != PROVIDER)
        .cloned()
        .collect::<Vec<_>>();
    if bundle
        .contributions
        .iter()
        .any(|p| p.provider_id == PROVIDER)
    {
        contributions.push(PluginConfigSchemaProvider.contribution()?);
    }
    WorkspaceConfigSchemaBundle::compose(contributions).map_err(config_error)
}

// Saved config retains the schema that validated it, even when live catalog
// admission changes. Management must remain available to disable/rebind it after
// restart. New preview/create still rechecks the live catalog inside its tx.
pub(crate) fn materialization_schema(
    desired: WorkspaceConfigSchemaBundle,
    saved: &WorkspaceConfigSchemaBundle,
) -> Result<WorkspaceConfigSchemaBundle> {
    let prefix = format!("{}:", config_source::digest_bytes(BOOTSTRAP.as_bytes()));
    let active = saved
        .contributions
        .iter()
        .find(|p| p.provider_id == PROVIDER && p.version.starts_with(&prefix));
    let Some(active) = active else {
        return Ok(desired);
    };
    let contributions = desired.contributions.into_iter().map(|p| {
        if p.provider_id == PROVIDER {
            active.clone()
        } else {
            p
        }
    });
    WorkspaceConfigSchemaBundle::compose(contributions).map_err(config_error)
}
