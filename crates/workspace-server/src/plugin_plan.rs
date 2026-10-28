//! Immutable, Server-owned Plugin execution snapshots and same-database pins.
//!
//! The config owner supplies an already validated, non-secret projection; this
//! module never reads config, selects a Profile, fetches packages or resolves
//! credentials. Host-bound execution identities are trusted inputs, not caller
//! authentication. Service entrypoints must authenticate/authorize every call,
//! including replay and reads. A saved plan is an upper bound, not live authority.
//!
//! Owner transaction functions compose with Worker creation in the catalog DB.
//! Their local savepoints also roll back partial pin acquisition if an owner
//! catches an error and commits other work. There is no cross-store compensation.
use crate::{Error, Result, plugin_catalog::PluginCatalogError};
use manifest::plugin;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use server_api::{PluginArtifact, PluginGrantMetadata};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

mod store;
pub use store::*;

/// Included by the config owner's ordered database migration, never at runtime.
pub const SCHEMA_SQL: &str = include_str!("plugin_plan/schema.sql");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigProvenance {
    pub content_digest: String,
    pub toolchain_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileProvenance {
    pub identity: String,
    pub projection_digest: String,
}

/// Supplied by the authenticated Host launch/delegation boundary, never by guest
/// or model input. These are identities only, not credentials or provider handles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostExecutionIdentity {
    pub runtime_id: String,
    pub worker_id: String,
    pub principal_id: String,
    pub delegation_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingPolicyReference {
    pub slot: String,
    pub binding_id: String,
    pub policy_digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginSurface {
    Tool,
}

/// Only capabilities implemented by the initial Tool Host are representable.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PluginGrant {
    Logging,
    Https { slot: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginSchemaIdentity {
    pub identity: String,
    pub version: String,
    pub digest: String,
    pub namespace: String,
}

/// The upstream config/schema authority must validate config values as non-secret
/// and intersect grants/surfaces with Workspace, Profile and delegation bounds.
/// Binding policies remain with their owner: only identities and digests travel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginPinProjection {
    pub config: Value,
    pub surfaces: Vec<PluginSurface>,
    pub tools: Vec<String>,
    pub grants: Vec<PluginGrant>,
    pub bindings: Vec<BindingPolicyReference>,
}

/// All fields are private: construct from admitted artifact metadata, not a
/// hand-built wire snapshot. Deserialization alone does not establish admission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerPluginPin {
    installation_id: String,
    expected_last_operation: String,
    package_id: String,
    package_version: String,
    package_digest: String,
    artifact_digest: String,
    artifact_size: u32,
    package_schema_version: u32,
    component_world: String,
    required_imports: Vec<String>,
    config_schema: PluginSchemaIdentity,
    config_digest: String,
    projection: PluginPinProjection,
}

impl WorkerPluginPin {
    pub fn from_artifact(
        installation_id: String,
        expected_last_operation: String,
        artifact: &PluginArtifact,
        mut projection: PluginPinProjection,
    ) -> Result<Self> {
        if artifact.revoked {
            return conflict();
        }
        if !valid_identity(&installation_id)
            || !valid_identity(&expected_last_operation)
            || !valid_digest(&artifact.package_digest)
            || !valid_digest(&artifact.artifact_digest)
            || artifact.artifact_size == 0
            || artifact.schema_version != plugin::SUPPORTED_PLUGIN_API_VERSION
            || !matches!(
                artifact.world.as_str(),
                plugin::PLUGIN_COMPONENT_TOOL_WORLD
                    | plugin::PLUGIN_COMPONENT_HTTPS_TOOL_WORLD
                    | plugin::PLUGIN_COMPONENT_LOGGED_TOOL_WORLD
                    | plugin::PLUGIN_COMPONENT_HTTPS_LOGGED_TOOL_WORLD
            )
            || !valid_digest(&artifact.config_schema.digest)
            || !valid_identity(&artifact.package_id)
            || !valid_identity(&artifact.version)
            || !valid_identity(&artifact.config_schema.identity)
            || !valid_identity(&artifact.config_schema.version)
            || !valid_identity(&artifact.config_schema.namespace)
            || !projection.config.is_object()
        {
            return invalid();
        }
        projection.surfaces.sort();
        projection.surfaces.dedup();
        projection.tools.sort();
        projection.tools.dedup();
        projection.grants.sort();
        projection.grants.dedup();
        projection.bindings.sort();
        projection.config = canonical_value(&projection.config);
        if (!projection.tools.is_empty() && !projection.surfaces.contains(&PluginSurface::Tool))
            || projection.tools.iter().any(|name| {
                !valid_identity(name) || !artifact.tools.iter().any(|tool| tool.name == *name)
            })
            || projection
                .bindings
                .windows(2)
                .any(|b| b[0].slot == b[1].slot)
            || projection.bindings.iter().any(|binding| {
                !valid_identity(&binding.slot)
                    || !valid_identity(&binding.binding_id)
                    || !valid_digest(&binding.policy_digest)
                    || !artifact
                        .connections
                        .iter()
                        .any(|slot| slot.name == binding.slot)
            })
            || projection.grants.iter().any(|grant| match grant {
                PluginGrant::Logging => !artifact
                    .requested_grants
                    .contains(&PluginGrantMetadata::Logging),
                PluginGrant::Https { slot } => {
                    !valid_identity(slot)
                        || !artifact
                            .requested_grants
                            .contains(&PluginGrantMetadata::Https { slot: slot.clone() })
                        || !projection
                            .bindings
                            .iter()
                            .any(|binding| binding.slot == *slot)
                }
            })
            || projection.bindings.iter().any(|binding| {
                !projection.grants.contains(&PluginGrant::Https {
                    slot: binding.slot.clone(),
                })
            })
        {
            return invalid();
        }
        let mut required_imports = artifact.required_imports.clone();
        required_imports.sort();
        required_imports.dedup();
        Ok(Self {
            installation_id,
            expected_last_operation,
            package_id: artifact.package_id.clone(),
            package_version: artifact.version.clone(),
            package_digest: artifact.package_digest.clone(),
            artifact_digest: artifact.artifact_digest.clone(),
            artifact_size: artifact.artifact_size,
            package_schema_version: artifact.schema_version,
            component_world: artifact.world.clone(),
            required_imports,
            config_schema: PluginSchemaIdentity {
                identity: artifact.config_schema.identity.clone(),
                version: artifact.config_schema.version.clone(),
                digest: artifact.config_schema.digest.clone(),
                namespace: artifact.config_schema.namespace.clone(),
            },
            config_digest: digest_json(&projection.config)?,
            projection,
        })
    }

    pub fn installation_id(&self) -> &str {
        &self.installation_id
    }
    pub fn expected_last_operation(&self) -> &str {
        &self.expected_last_operation
    }
    pub fn package_id(&self) -> &str {
        &self.package_id
    }
    pub fn package_version(&self) -> &str {
        &self.package_version
    }
    pub fn package_schema_version(&self) -> u32 {
        self.package_schema_version
    }
    pub fn component_world(&self) -> &str {
        &self.component_world
    }
    pub fn required_imports(&self) -> &[String] {
        &self.required_imports
    }
    pub fn package_digest(&self) -> &str {
        &self.package_digest
    }
    pub fn artifact_digest(&self) -> &str {
        &self.artifact_digest
    }
    pub fn artifact_size(&self) -> u32 {
        self.artifact_size
    }
    pub fn config_schema(&self) -> &PluginSchemaIdentity {
        &self.config_schema
    }
    pub fn config_digest(&self) -> &str {
        &self.config_digest
    }
    pub fn projection(&self) -> &PluginPinProjection {
        &self.projection
    }

    pub(super) fn matches_artifact(&self, artifact: &PluginArtifact) -> Result<bool> {
        Ok(Self::from_artifact(
            self.installation_id.clone(),
            self.expected_last_operation.clone(),
            artifact,
            self.projection.clone(),
        )? == *self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerPluginPlanInput {
    pub workspace_id: String,
    pub config: ConfigProvenance,
    pub profile: ProfileProvenance,
    pub execution: HostExecutionIdentity,
    pub plugins: Vec<WorkerPluginPin>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerPluginExecutionPlan {
    format_version: u32,
    input: WorkerPluginPlanInput,
}

impl WorkerPluginExecutionPlan {
    pub fn canonicalize(mut input: WorkerPluginPlanInput) -> Result<Self> {
        if !valid_identity(&input.workspace_id)
            || !valid_digest(&input.config.content_digest)
            || !valid_identity(&input.config.toolchain_fingerprint)
            || !valid_identity(&input.profile.identity)
            || !valid_digest(&input.profile.projection_digest)
            || !valid_identity(&input.execution.runtime_id)
            || !valid_identity(&input.execution.worker_id)
            || !valid_identity(&input.execution.principal_id)
            || input
                .execution
                .delegation_id
                .as_ref()
                .is_some_and(|id| !valid_identity(id))
            || input.plugins.len() > 128
        {
            return invalid();
        }
        input
            .plugins
            .sort_by(|a, b| a.installation_id.cmp(&b.installation_id));
        // One selection per installation; two aliases of one package instead use
        // distinct installation identities, as required by the catalog contract.
        if input
            .plugins
            .windows(2)
            .any(|pins| pins[0].installation_id == pins[1].installation_id)
        {
            return invalid();
        }
        // Public Tool names use immutable installation identities, never aliases.
        let mut names = BTreeSet::new();
        for pin in &input.plugins {
            for tool in &pin.projection.tools {
                let name = format!("{}__{}", pin.installation_id, tool);
                if name.len() > 128 || !names.insert(name) {
                    return invalid();
                }
            }
        }
        Ok(Self {
            format_version: 1,
            input,
        })
    }

    pub fn workspace_id(&self) -> &str {
        &self.input.workspace_id
    }
    pub fn config(&self) -> &ConfigProvenance {
        &self.input.config
    }
    pub fn profile(&self) -> &ProfileProvenance {
        &self.input.profile
    }
    pub fn execution(&self) -> &HostExecutionIdentity {
        &self.input.execution
    }
    pub fn plugins(&self) -> &[WorkerPluginPin] {
        &self.input.plugins
    }
    pub fn fingerprint(&self) -> Result<String> {
        digest_json(self)
    }
    pub fn canonical_json(&self) -> Result<String> {
        encode_canonical(self)
    }

    pub(super) fn validate_canonical(&self) -> Result<()> {
        if self.format_version != 1 || Self::canonicalize(self.input.clone())? != *self {
            return invalid();
        }
        Ok(())
    }
}

fn invalid<T>() -> Result<T> {
    Err(Error::PluginCatalog(PluginCatalogError::Invalid))
}
fn conflict<T>() -> Result<T> {
    Err(Error::PluginCatalog(PluginCatalogError::Conflict))
}
fn not_found<T>() -> Result<T> {
    Err(Error::PluginCatalog(PluginCatalogError::NotFound))
}
fn valid_identity(value: &str) -> bool {
    !value.is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
}
fn valid_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
fn canonical_value(value: &Value) -> Value {
    match value {
        Value::Object(fields) => {
            let mut keys: Vec<_> = fields.keys().collect();
            keys.sort();
            Value::Object(
                keys.into_iter()
                    .map(|key| (key.clone(), canonical_value(&fields[key])))
                    .collect(),
            )
        }
        Value::Array(values) => Value::Array(values.iter().map(canonical_value).collect()),
        _ => value.clone(),
    }
}
fn encode_canonical(value: &impl Serialize) -> Result<String> {
    let value = serde_json::to_value(value)
        .map_err(|_| Error::PluginCatalog(PluginCatalogError::Invalid))?;
    serde_json::to_string(&canonical_value(&value))
        .map_err(|_| Error::PluginCatalog(PluginCatalogError::Invalid))
}
fn digest_json(value: &impl Serialize) -> Result<String> {
    let bytes = encode_canonical(value)?;
    Ok(hash_bytes(bytes.as_bytes()))
}
fn hash_bytes(bytes: &[u8]) -> String {
    let hex: String = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("sha256:{hex}")
}

#[cfg(test)]
mod tests;
