//! Workspace-owner source adoption transport. The Server authenticates the owner
//! before reading source, previewing, sealing migration data, adopting or reconciling.
//! A result lookup is not a new adoption and absence is not proof of failure.
use crate::{
    ConfigCommitRequest, RepositoryApiError, ServerRequestContext, WorkspaceConfigTreeResponse,
};
use feature_permissions::{PermissionAssignment, PermissionCatalog};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ConfigurationAdoptionRequest {
    pub operation_id: String,
    pub expected_head: Option<String>,
    pub expected_catalog_digest: String,
    pub expected_legacy_export: Option<String>,
    pub source: ConfigCommitRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum RuntimePolicyApplication {
    NextCurrentPolicyCheck,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ConfigurationAdoptionReceipt {
    pub workspace_id: String,
    pub operation_id: String,
    pub history_head: String,
    pub state: WorkspaceConfigTreeResponse,
    pub source_stored: bool,
    pub policy_committed: bool,
    pub runtime_application: RuntimePolicyApplication,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ConfigurationSubjectPolicy {
    pub subject_id: String,
    pub change_id: String,
    pub execution: PermissionAssignment,
    pub delegation: PermissionAssignment,
    /// Exact retained domain restrictions, not editable payloads on permission keys.
    pub legacy_constraints: Option<crate::authorization::SubjectPermissions>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConfigurationMigrationDiagnostic {
    Converted {
        subject_id: String,
        operation: String,
    },
    DomainConstraint {
        subject_id: String,
        permission: crate::authorization::ScopedPermission,
    },
    ExplicitConfigurationRequired {
        subject_id: String,
        permission: crate::authorization::ScopedPermission,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ConfigurationSourceResponse {
    /// Before the first adoption this is an editable, evaluated migration draft.
    /// source_stored distinguishes it from the unchanged stored source.
    pub state: WorkspaceConfigTreeResponse,
    pub history_head: Option<String>,
    pub source_stored: bool,
    pub catalog: PermissionCatalog,
    pub catalog_digest: String,
    pub expected_legacy_export: Option<String>,
    pub policies: Vec<ConfigurationSubjectPolicy>,
    pub migration_diagnostics: Vec<ConfigurationMigrationDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ConfigurationPreviewResponse {
    pub expected_head: Option<String>,
    pub state: WorkspaceConfigTreeResponse,
    pub changed_paths: Vec<String>,
    pub changed_subjects: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConfigurationReconciliation {
    Adopted {
        receipt: ConfigurationAdoptionReceipt,
    },
    PendingTransfer {
        transfer_identity: String,
        export_digest: String,
    },
    /// No matching evidence was observed. This is not permission to change the ID.
    NotObserved,
}

#[api_macros::api(reqwest, axum, openapi)]
pub trait ConfigurationAdoptionApi {
    #[get("/api/w/{workspace_id}/config/adoption",status=200,error_status=400,additional_error_statuses=[401,403,404,409,500],bearer_auth=true,browser_auth=true)]
    async fn configuration_source(
        &self,
        #[extension] context: ServerRequestContext,
        #[path] workspace_id: String,
    ) -> Result<ConfigurationSourceResponse, RepositoryApiError>;
    #[post("/api/w/{workspace_id}/config/adoption/preview",status=200,error_status=400,additional_error_statuses=[401,403,404,409,500],bearer_auth=true,browser_auth=true,normalize_body_errors=true)]
    async fn configuration_preview(
        &self,
        #[extension] context: ServerRequestContext,
        #[path] workspace_id: String,
        #[body] request: ConfigurationAdoptionRequest,
    ) -> Result<ConfigurationPreviewResponse, RepositoryApiError>;
    #[post("/api/w/{workspace_id}/config/adoption/commit",status=200,error_status=400,additional_error_statuses=[401,403,404,409,500],bearer_auth=true,browser_auth=true,normalize_body_errors=true)]
    async fn configuration_adopt(
        &self,
        #[extension] context: ServerRequestContext,
        #[path] workspace_id: String,
        #[body] request: ConfigurationAdoptionRequest,
    ) -> Result<ConfigurationAdoptionReceipt, RepositoryApiError>;
    #[post("/api/w/{workspace_id}/config/adoption/reconcile",status=200,error_status=400,additional_error_statuses=[401,403,404,409,500],bearer_auth=true,browser_auth=true,normalize_body_errors=true)]
    async fn configuration_reconcile(
        &self,
        #[extension] context: ServerRequestContext,
        #[path] workspace_id: String,
        #[body] request: ConfigurationAdoptionRequest,
    ) -> Result<ConfigurationReconciliation, RepositoryApiError>;
}

impl api_macros::openapi::OpenApiSchema for ConfigurationSourceResponse {}
impl api_macros::openapi::OpenApiSchema for ConfigurationAdoptionRequest {}
impl api_macros::openapi::OpenApiSchema for ConfigurationPreviewResponse {}
impl api_macros::openapi::OpenApiSchema for ConfigurationAdoptionReceipt {}
impl api_macros::openapi::OpenApiSchema for ConfigurationReconciliation {}

pub fn configuration_contract_openapi()
-> Result<api_macros::openapi::OpenApiDocument, api_macros::openapi::OpenApiError> {
    use sha2::{Digest, Sha256};
    let document = |source_digest: &str| {
        configuration_adoption_api_openapi(api_macros::openapi::OpenApiInfo {
            title: "Workspace configuration adoption",
            version: env!("CARGO_PKG_VERSION"),
            source_digest,
        })
    };
    let normalized =
        document("sha256:0000000000000000000000000000000000000000000000000000000000000000")?
            .to_json()?;
    let digest: String = Sha256::digest(normalized.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    document(&format!("sha256:{digest}"))
}

#[cfg(feature = "typescript")]
pub fn configuration_typescript() -> String {
    use ts_rs::TS;
    let config = ts_rs::Config::default();
    let declarations = [
        ConfigurationAdoptionRequest::decl(&config),
        RuntimePolicyApplication::decl(&config),
        ConfigurationAdoptionReceipt::decl(&config),
        ConfigurationSubjectPolicy::decl(&config),
        ConfigurationMigrationDiagnostic::decl(&config),
        ConfigurationSourceResponse::decl(&config),
        ConfigurationPreviewResponse::decl(&config),
        ConfigurationReconciliation::decl(&config),
    ];
    format!(
        "// Generated by `cargo run -p server-api --features typescript --example generate_configuration_adoption_types`.\n// Do not edit manually.\n\nimport type {{ ConfigCommitRequest, WorkspaceConfigTreeResponse, ScopedPermission, SubjectPermissions }} from \"./legacy-server-api.ts\";\nimport type {{ PermissionAssignment, PermissionCatalog }} from \"./feature-permissions.generated.ts\";\n\n{}\n",
        declarations
            .into_iter()
            .map(|declaration| format!("export {declaration}"))
            .collect::<Vec<_>>()
            .join("\n\n")
    )
}
