//! Declaration/resolution management transport only. These generated routes are
//! intentionally not mounted by this crate. The Workspace authority must bind
//! authenticated context, read current policy and authorize management access
//! before implementing/mounting them; no input can issue a namespace handle.

use crate::{RepositoryApiError, ServerRequestContext};
use feature_permissions::types::{
    PermissionCatalog, PermissionResolution, ValidatePermissionsRequest,
};

// Transparent wrappers keep the wire representation identical while the REST
// owner, not the resolver crate, attests to OpenAPI/Serde agreement.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(transparent)]
pub struct PermissionCatalogResponse(pub PermissionCatalog);
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(transparent)]
pub struct PermissionValidationRequest(pub ValidatePermissionsRequest);
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(transparent)]
pub struct PermissionResolutionResponse(pub PermissionResolution);
impl api_macros::openapi::OpenApiSchema for PermissionCatalogResponse {}
impl api_macros::openapi::OpenApiSchema for PermissionValidationRequest {}
impl api_macros::openapi::OpenApiSchema for PermissionResolutionResponse {}

#[api_macros::api(reqwest, axum, openapi)]
pub trait FeaturePermissionsApi {
    #[get("/api/w/{workspace_id}/permission-catalog", status = 200, error_status = 403, additional_error_statuses = [400, 404, 500], bearer_auth = true, browser_auth = true)]
    async fn permission_catalog(
        &self,
        #[extension] context: ServerRequestContext,
        #[path] workspace_id: String,
    ) -> Result<PermissionCatalogResponse, RepositoryApiError>;

    #[post("/api/w/{workspace_id}/permissions/validate", status = 200, error_status = 403, additional_error_statuses = [400, 404, 500], bearer_auth = true, browser_auth = true, normalize_body_errors = true)]
    async fn validate_permissions(
        &self,
        #[extension] context: ServerRequestContext,
        #[path] workspace_id: String,
        #[body] request: PermissionValidationRequest,
    ) -> Result<PermissionResolutionResponse, RepositoryApiError>;
}

/// Deterministic standalone contract. Its source identity hashes the normalized
/// schema, not a deployment URL or another API's unrelated route inventory.
pub fn permission_contract_openapi()
-> Result<api_macros::openapi::OpenApiDocument, api_macros::openapi::OpenApiError> {
    use sha2::{Digest, Sha256};
    let document = |source_digest: &str| {
        feature_permissions_api_openapi(api_macros::openapi::OpenApiInfo {
            title: "Feature permission declaration contract",
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

#[cfg(test)]
mod tests {
    use super::*;
    use feature_permissions::*;

    /// Wire contract + actual resolver, not a mounted HTTP authorization test.
    #[test]
    fn management_wire_preserves_tagged_assignments_and_explicit_children_into_resolution() {
        let catalog: PermissionCatalog = serde_json::from_str(include_str!(
            "../../feature-permissions/tests/fixtures/catalog.json"
        ))
        .unwrap();
        let catalog: PermissionCatalogResponse = serde_json::from_slice(
            &serde_json::to_vec(&PermissionCatalogResponse(catalog)).unwrap(),
        )
        .unwrap();
        let catalog = catalog.0;
        let mut host = PermissionHost::new(catalog.workspace.clone());
        for namespace in catalog.namespaces {
            let owner = host.register_feature(namespace.feature).unwrap();
            let handle = host.bind_namespace(&owner, namespace.namespace).unwrap();
            host.declare(&handle, namespace.declaration).unwrap();
        }
        let assignment: PermissionAssignment = serde_json::from_str(include_str!(
            "../../feature-permissions/tests/fixtures/assignment.json"
        ))
        .unwrap();
        let request = PermissionValidationRequest(ValidatePermissionsRequest { assignment });
        let request: PermissionValidationRequest =
            serde_json::from_slice(&serde_json::to_vec(&request).unwrap()).unwrap();
        let result = host.resolve(Some(&request.0.assignment));
        let response: PermissionResolutionResponse = serde_json::from_slice(
            &serde_json::to_vec(&PermissionResolutionResponse(result.projection().clone()))
                .unwrap(),
        )
        .unwrap();
        let response = response.0;
        assert!(
            response
                .allowed
                .contains_key(&PermissionKey::parse("ticket.hoge.fuga.finish").unwrap())
        );
        assert!(
            !response
                .allowed
                .contains_key(&PermissionKey::parse("ticket.hoge.unlinked").unwrap())
        );
        assert!(
            serde_json::from_value::<ValidatePermissionsRequest>(
                serde_json::json!({"assignment":request.0.assignment, "workspace":"other"})
            )
            .is_err()
        );
    }

    #[test]
    fn generated_management_contract_publishes_payload_free_schema_and_context_bound_routes() {
        let document = permission_contract_openapi().unwrap();
        let document: serde_json::Value =
            serde_json::from_str(&document.to_json().unwrap()).unwrap();
        let checked_in: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../openapi/feature-permissions-api.json"),
            )
            .expect("run generate_feature_permissions_openapi"),
        )
        .unwrap();
        assert_eq!(document, checked_in);
        let paths = document["paths"].as_object().unwrap();
        assert!(paths.contains_key("/api/w/{workspace_id}/permission-catalog"));
        assert!(paths.contains_key("/api/w/{workspace_id}/permissions/validate"));
        let schema =
            serde_json::to_value(schemars::schema_for!(ValidatePermissionsRequest)).unwrap();
        let serialized = serde_json::to_string(&schema).unwrap();
        assert!(serialized.contains("namespace"));
        assert!(serialized.contains("set"));
        assert!(!serialized.contains("payload"));
        assert_eq!(schema["additionalProperties"], false);
    }
}
