//! Owner-only diagnostics resolve against the same live registry as operation use.
//! These endpoints return explanations, never handles or execution authority.
use super::*;
use server_api::feature_permissions::{
    FeaturePermissionsApi, PermissionCatalogResponse, PermissionResolutionResponse,
    PermissionValidationRequest,
};
use server_api::{RepositoryApiError, ServerRequestContext};

impl FeaturePermissionsApi for WorkspaceServerApi {
    async fn permission_catalog(
        &self,
        context: ServerRequestContext,
        workspace_id: String,
    ) -> std::result::Result<PermissionCatalogResponse, RepositoryApiError> {
        configuration_adoption::owner_repository_operation(
            self,
            context,
            workspace_id,
            move |api, _account| {
                api.refresh_plugin_permission_registry()?;
                let registry = api.permission_registry.read().map_err(|_| {
                    Error::Config("current Feature declarations unavailable".into())
                })?;
                Ok(PermissionCatalogResponse(registry.host().catalog()))
            },
        )
        .await
    }

    async fn validate_permissions(
        &self,
        context: ServerRequestContext,
        workspace_id: String,
        request: PermissionValidationRequest,
    ) -> std::result::Result<PermissionResolutionResponse, RepositoryApiError> {
        configuration_adoption::owner_repository_operation(
            self,
            context,
            workspace_id,
            move |api, _account| {
                api.refresh_plugin_permission_registry()?;
                let registry = api.permission_registry.read().map_err(|_| {
                    Error::Config("current Feature declarations unavailable".into())
                })?;
                Ok(PermissionResolutionResponse(
                    registry
                        .host()
                        .resolve(Some(&request.0.assignment))
                        .projection()
                        .clone(),
                ))
            },
        )
        .await
    }
}
