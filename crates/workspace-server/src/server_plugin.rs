//! Human catalog management is separate from Runtime artifact delivery and execution grants.
//! The outer dispatcher authenticates each request. Mutations additionally require the
//! Workspace owner account; authenticated readers follow the existing Workspace read policy.
//! Neither model text nor a Runtime workspace.request proof can manufacture a human actor.
use super::*;
use server_api::*;

type CatalogResult<T> = std::result::Result<T, RepositoryApiError>;

pub(super) fn router(service: Arc<ServerApiContractService>) -> Router {
    Router::new()
        .merge(
            server_api_axum::plugin_upload(service.clone()).layer(DefaultBodyLimit::max(
                manifest::plugin::PluginPackageLimits::default().max_package_size_bytes as usize,
            )),
        )
        .merge(server_api_axum::plugin_install(service.clone()))
        .merge(server_api_axum::plugin_list(service.clone()))
        .merge(server_api_axum::plugin_show(service.clone()))
        .merge(server_api_axum::plugin_artifact_add(service.clone()))
        .merge(server_api_axum::plugin_revoke(service.clone()))
        .merge(server_api_axum::plugin_uninstall(service.clone()))
        .merge(server_api_axum::plugin_operation(service))
        .layer(middleware::from_fn(normalize_rejections))
}

pub(super) async fn normalize_rejections(request: Request, next: Next) -> Response {
    let response = next.run(request).await;
    // The management contract classifies malformed DTOs/queries as invalid=400,
    // including Axum's JSON data/media rejections. Preserve the common envelope
    // without reflecting attacker-supplied JSON values or transport diagnostics.
    if matches!(
        response.status(),
        StatusCode::BAD_REQUEST
            | StatusCode::UNPROCESSABLE_ENTITY
            | StatusCode::UNSUPPORTED_MEDIA_TYPE
    ) {
        (
            StatusCode::BAD_REQUEST,
            Json(RepositoryApiError::new(
                400,
                "plugin_invalid",
                "Invalid Plugin request",
                vec![],
            )),
        )
            .into_response()
    } else {
        response
    }
}

pub(super) fn is_catalog_path(path: &str) -> bool {
    path.strip_prefix("/api/w/")
        .and_then(|p| p.split_once('/'))
        .is_some_and(|(_, resource)| {
            matches!(resource, "plugins" | "plugin-uploads")
                || resource.starts_with("plugins/")
                || resource.starts_with("plugin-operations/")
        })
}

fn failure(error: Error) -> RepositoryApiError {
    let (status, code) = match error {
        Error::PluginCatalog(error) => (error.status(), error.code()),
        Error::WorkspacePermissionDenied(_) => (403, "access_denied"),
        Error::WorkspaceIdMismatch => (404, "not_found"),
        Error::DbCapacityExceeded => (503, "busy"),
        // A storage failure may be a lost acknowledgement. Do not assert rollback
        // or encourage a fresh operation key; the caller can reconcile its receipt.
        _ => (503, "catalog_outcome_unknown"),
    };
    RepositoryApiError::new(status, code, format!("Plugin catalog: {code}"), vec![])
}

async fn authorize(
    api: &WorkspaceApi,
    workspace: &str,
    actor: &RequestActor,
    manage: bool,
) -> CatalogResult<()> {
    validate_workspace_scope(api, workspace).map_err(|e| failure(e.error))?;
    if manage {
        require_workspace_owner(api, workspace, actor, "Plugin management")
            .await
            .map_err(|e| failure(e.error))?;
    }
    Ok(())
}

fn actor_identity(actor: &RequestActor) -> String {
    // Account alone is not a caller: two users of the same management account
    // must not consume one another's temporary uploads or operation keys.
    serde_json::to_string(&(&actor.account_id, &actor.user_id))
        .expect("string tuple is serializable")
}

pub(super) async fn upload(
    api: &WorkspaceApi,
    actor: RequestActor,
    workspace: String,
    body: BinaryBody,
) -> CatalogResult<PluginUploadResponse> {
    authorize(api, &workspace, &actor, true).await?;
    let root =
        ServerConfig::server_data_root_for_data_dir(&api.config.data_root).join("plugin-artifacts");
    let store = api.config_store.clone();
    let actor = actor_identity(&actor);
    let executor = store.db_executor();
    executor
        .run(move || store.plugin_upload(&root, &workspace, &actor, body.as_ref()))
        .await
        .map_err(failure)
}

pub(super) async fn install(
    api: &WorkspaceApi,
    actor: RequestActor,
    workspace: String,
    request: PluginInstallRequest,
) -> CatalogResult<PluginOperationResponse> {
    authorize(api, &workspace, &actor, true).await?;
    let store = api.config_store.clone();
    let actor = actor_identity(&actor);
    let executor = store.db_executor();
    executor
        .run(move || store.plugin_install(&workspace, &actor, &request))
        .await
        .map_err(failure)
}

pub(super) async fn list(
    api: &WorkspaceApi,
    actor: RequestActor,
    workspace: String,
    query: PluginListRequest,
) -> CatalogResult<PluginListResponse> {
    authorize(api, &workspace, &actor, false).await?;
    let store = api.config_store.clone();
    let executor = store.db_executor();
    executor
        .run(move || store.plugin_list(&workspace, &query))
        .await
        .map_err(failure)
}

pub(super) async fn show(
    api: &WorkspaceApi,
    actor: RequestActor,
    workspace: String,
    installation_id: String,
) -> CatalogResult<PluginInstallation> {
    authorize(api, &workspace, &actor, false).await?;
    let store = api.config_store.clone();
    let executor = store.db_executor();
    executor
        .run(move || store.plugin_show(&workspace, &installation_id))
        .await
        .map_err(failure)
}

pub(super) async fn artifact_add(
    api: &WorkspaceApi,
    actor: RequestActor,
    workspace: String,
    installation_id: String,
    request: PluginAddArtifactRequest,
) -> CatalogResult<PluginOperationResponse> {
    authorize(api, &workspace, &actor, true).await?;
    let store = api.config_store.clone();
    let actor = actor_identity(&actor);
    let executor = store.db_executor();
    executor
        .run(move || store.plugin_add_artifact(&workspace, &actor, &installation_id, &request))
        .await
        .map_err(failure)
}

pub(super) async fn revoke(
    api: &WorkspaceApi,
    actor: RequestActor,
    workspace: String,
    installation_id: String,
    request: PluginRevokeRequest,
) -> CatalogResult<PluginOperationResponse> {
    authorize(api, &workspace, &actor, true).await?;
    let store = api.config_store.clone();
    let actor = actor_identity(&actor);
    let executor = store.db_executor();
    executor
        .run(move || store.plugin_revoke(&workspace, &actor, &installation_id, &request))
        .await
        .map_err(failure)
}

pub(super) async fn uninstall(
    api: &WorkspaceApi,
    actor: RequestActor,
    workspace: String,
    installation_id: String,
    request: PluginUninstallRequest,
) -> CatalogResult<PluginOperationResponse> {
    authorize(api, &workspace, &actor, true).await?;
    let store = api.config_store.clone();
    let actor = actor_identity(&actor);
    let executor = store.db_executor();
    executor
        .run(move || store.plugin_uninstall(&workspace, &actor, &installation_id, &request))
        .await
        .map_err(failure)
}

pub(super) async fn operation(
    api: &WorkspaceApi,
    actor: RequestActor,
    workspace: String,
    operation_id: String,
) -> CatalogResult<PluginOperationResponse> {
    authorize(api, &workspace, &actor, false).await?;
    let store = api.config_store.clone();
    let executor = store.db_executor();
    executor
        .run(move || store.plugin_operation(&workspace, &operation_id))
        .await
        .map_err(failure)
}
