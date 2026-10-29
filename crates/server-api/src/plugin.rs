use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PluginInstallRequest {
    pub upload_id: String,
    pub alias: String,
    pub operation_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PluginAddArtifactRequest {
    pub upload_id: String,
    pub expected_last_operation: String,
    pub operation_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PluginRevokeRequest {
    pub artifact_digest: String,
    pub expected_last_operation: String,
    pub reason: String,
    pub operation_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PluginUninstallRequest {
    pub expected_last_operation: String,
    pub operation_key: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PluginListRequest {
    pub cursor: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PluginSchemaContribution {
    pub identity: String,
    pub version: String,
    pub digest: String,
    pub namespace: String,
    pub dcdl: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PluginToolMetadata {
    pub name: String,
    pub description: String,
    #[cfg_attr(feature = "typescript", ts(type = "unknown"))]
    pub input_schema: serde_json::Value,
    #[cfg_attr(feature = "typescript", ts(type = "unknown"))]
    pub output_schema: serde_json::Value,
    pub error_codes: Vec<String>,
    pub external_write: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PluginConnectionMetadata {
    pub name: String,
    pub methods: Vec<PluginHttpMethod>,
    pub path_prefixes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PluginGrantMetadata {
    Logging,
    Https { slot: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PluginArtifact {
    pub package_id: String,
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub package_digest: String,
    pub artifact_digest: String,
    pub artifact_size: u32,
    pub schema_version: u32,
    pub world: String,
    pub required_imports: Vec<String>,
    pub tools: Vec<PluginToolMetadata>,
    pub connections: Vec<PluginConnectionMetadata>,
    pub requested_grants: Vec<PluginGrantMetadata>,
    pub config_schema: PluginSchemaContribution,
    pub revoked: bool,
    pub revocation_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PluginUploadResponse {
    pub upload_id: String,
    pub expires_at: String,
    pub artifact: PluginArtifact,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PluginInstallation {
    pub installation_id: String,
    pub alias: String,
    pub package_id: String,
    pub last_operation: String,
    pub uninstalled: bool,
    pub artifacts: Vec<PluginArtifact>,
}

/// Catalog pages omit artifact/schema payloads so the item bound also bounds
/// response work. Exact immutable metadata is available from installation show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PluginInstallationSummary {
    pub installation_id: String,
    pub alias: String,
    pub package_id: String,
    pub last_operation: String,
    pub uninstalled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PluginListResponse {
    pub installations: Vec<PluginInstallationSummary>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum PluginOperationKind {
    Install,
    AddArtifact,
    Revoke,
    Uninstall,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PluginOperationResponse {
    pub operation_id: String,
    pub kind: PluginOperationKind,
    pub installation: PluginInstallation,
}

/// Namespace a byte cache by the authenticated Server origin and signing key.
/// This is not a package hash or an authorization decision. Hosts derive the
/// bundle from their current registered authority, never from a delivery reply.
pub fn plugin_server_authority_identity(bundle: &crate::WorkspacePublicIdentityBundle) -> String {
    serde_json::to_string(&(
        bundle.backend_url.trim_end_matches('/'),
        &bundle.public_key_fingerprint,
    ))
    .expect("string tuple is serializable")
}

/// A selection within a persisted plan, never digest-only retrieval authority.
/// Workspace and Worker identities come from authenticated Host context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PluginArtifactDeliveryRequest {
    pub plan_id: String,
    pub plan_fingerprint: String,
    pub installation_id: String,
    pub artifact_digest: String,
}

/// Fresh owner observation for one exact selection. Cache only bytes, not this
/// observation or the Workspace's configuration, grants or installation state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PluginArtifactDeliveryMetadata {
    pub workspace_id: String,
    pub request: PluginArtifactDeliveryRequest,
    pub artifact: PluginArtifact,
}
