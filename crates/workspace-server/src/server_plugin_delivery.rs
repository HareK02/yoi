//! Authenticated byte delivery and the in-process adapter share current Worker
//! authority and the persisted plan owner. Neither a cache hit nor an adapter
//! handle retains permission. No destination policy or credential reaches a guest.
use super::*;
use server_api::{PluginArtifactDeliveryMetadata, PluginArtifactDeliveryRequest};
use yoi_plugin_runtime::delivery::{ArtifactSource, ArtifactStream, DeliveryError};

pub(super) fn router(service: Arc<ServerApiContractService>) -> Router {
    Router::new()
        .merge(server_api::server_api_axum::plugin_artifact_authorize(
            service.clone(),
        ))
        .merge(server_api::server_api_axum::plugin_artifact_pull(service))
        .layer(middleware::from_fn(plugin::normalize_rejections))
}

fn failure(error: impl Into<ApiError>) -> server_api::RepositoryApiError {
    let error = error.into();
    let status = match &error.error {
        Error::PluginCatalog(error) => error.status(),
        _ => api_error_status(&error.error).as_u16(),
    };
    let (status, code) = match status {
        400 => (400, "plugin_delivery_invalid"),
        401 | 403 => (403, "plugin_delivery_denied"),
        404 => (404, "plugin_delivery_not_found"),
        409 => (409, "plugin_delivery_conflict"),
        413 => (413, "plugin_delivery_limit"),
        _ => (503, "plugin_delivery_unavailable"),
    };
    server_api::RepositoryApiError::new(status, code, "Plugin artifact delivery refused", vec![])
}

fn delivery_error(error: server_api::RepositoryApiError) -> DeliveryError {
    if api_macros::HttpError::status_code(&error) >= 500 {
        DeliveryError::Unavailable("owner unavailable".into())
    } else {
        DeliveryError::Denied
    }
}

async fn worker_from_context(
    api: &WorkspaceApi,
    workspace: &str,
    context: &server_api::ServerRequestContext,
) -> std::result::Result<RuntimeWorkerRef, server_api::RepositoryApiError> {
    validate_workspace_scope(api, workspace).map_err(failure)?;
    // Runtime source proof has already been checked by the contract router.
    // Byte preparation precedes controller publication, so a live Runtime
    // summary is not required; the reservation/lineage owner below is required.
    subject_authorization::source_from_context(context).map_err(failure)
}

impl WorkspaceApi {
    async fn plugin_artifact_execution_identity(
        &self,
        worker: &RuntimeWorkerRef,
    ) -> crate::Result<crate::plugin_plan::HostExecutionIdentity> {
        let api = self.clone();
        let worker_owned = worker.clone();
        let preparing = self
            .host_executors
            .host()?
            .run(move || {
                if api
                    .store
                    .has_active_worker_create_reservation(api.workspace_id(), &worker_owned)?
                {
                    api.plugin_reserved_identity(&worker_owned, None, None)
                        .map(Some)
                } else {
                    Ok(None)
                }
            })
            .await?;
        match preparing {
            Some(identity) => Ok(identity),
            None => self.plugin_execution_identity(worker).await,
        }
    }

    /// A Host-only adapter bound to this Server and Worker, not a loopback client.
    /// Construction resolves the active Server key; every operation checks it
    /// again, together with Worker membership, live grants, lineage and pin state.
    pub async fn plugin_artifact_source(
        &self,
        worker: RuntimeWorkerRef,
    ) -> crate::Result<InProcessPluginArtifactSource> {
        let authority = authority_identity(self)?;
        self.plugin_artifact_execution_identity(&worker).await?;
        Ok(InProcessPluginArtifactSource {
            api: self.clone(),
            worker,
            authority,
        })
    }
}

fn authority_identity(api: &WorkspaceApi) -> crate::Result<String> {
    let identity = api
        .store
        .get_workspace_signing_identity(api.workspace_id())?
        .ok_or_else(|| {
            Error::WorkspacePermissionDenied("Plugin Server identity unavailable".into())
        })?;
    let bundle = project_workspace_signing_identity(api, identity)?
        .public_bundle
        .ok_or_else(|| {
            Error::WorkspacePermissionDenied("Plugin Server identity inactive".into())
        })?;
    Ok(server_api::plugin_server_authority_identity(&bundle))
}

async fn current_execution(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
    request: &PluginArtifactDeliveryRequest,
) -> std::result::Result<crate::plugin_plan::HostExecutionIdentity, server_api::RepositoryApiError>
{
    let execution = api
        .plugin_artifact_execution_identity(worker)
        .await
        .map_err(failure)?;
    let stage_api = api.clone();
    let stage_worker = worker.clone();
    let installation_id = request.installation_id.clone();
    api.host_executors
        .host()
        .map_err(failure)?
        .run(move || {
            subject_authorization::authorize(
                &stage_api,
                &stage_worker,
                &server_api::authorization::OperationAuthorizationRequest {
                    operation: "plugin.artifact.read".into(),
                    resource: server_api::authorization::PermissionResource::PluginInstallation {
                        workspace_id: stage_api.workspace_id().into(),
                        installation_id,
                    },
                },
            )
            .map_err(|error| error.error)
        })
        .await
        .map_err(failure)?;
    Ok(execution)
}

async fn metadata_for_worker(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
    request: PluginArtifactDeliveryRequest,
) -> std::result::Result<PluginArtifactDeliveryMetadata, server_api::RepositoryApiError> {
    let execution = current_execution(api, worker, &request).await?;
    let store = api.config_store.clone();
    let workspace = api.workspace_id().to_owned();
    store
        .db_executor()
        .run(move || store.plugin_delivery_authorize(&workspace, &execution, &request))
        .await
        .map_err(failure)
}

async fn bytes_for_worker(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
    request: PluginArtifactDeliveryRequest,
) -> std::result::Result<Vec<u8>, server_api::RepositoryApiError> {
    let execution = current_execution(api, worker, &request).await?;
    let store = api.config_store.clone();
    let workspace = api.workspace_id().to_owned();
    let root =
        ServerConfig::server_data_root_for_data_dir(&api.config.data_root).join("plugin-artifacts");
    let before_read = request.clone();
    let bytes = store
        .db_executor()
        .run(move || store.plugin_delivery_bytes(&root, &workspace, &execution, &request))
        .await
        .map_err(failure)?;
    // The storage read is not publication authority. Recheck security narrowing
    // and the exact persisted pin after I/O; Runtime also checks before returning.
    metadata_for_worker(api, worker, before_read).await?;
    Ok(bytes)
}

pub(super) async fn authorize(
    api: &WorkspaceApi,
    context: server_api::ServerRequestContext,
    workspace: String,
    request: PluginArtifactDeliveryRequest,
) -> std::result::Result<PluginArtifactDeliveryMetadata, server_api::RepositoryApiError> {
    let worker = worker_from_context(api, &workspace, &context).await?;
    metadata_for_worker(api, &worker, request).await
}

pub(super) async fn pull(
    api: &WorkspaceApi,
    context: server_api::ServerRequestContext,
    workspace: String,
    request: PluginArtifactDeliveryRequest,
) -> std::result::Result<server_api::BinaryBody, server_api::RepositoryApiError> {
    let worker = worker_from_context(api, &workspace, &context).await?;
    bytes_for_worker(api, &worker, request)
        .await
        .map(server_api::BinaryBody::from)
}

pub struct InProcessPluginArtifactSource {
    api: WorkspaceApi,
    worker: RuntimeWorkerRef,
    authority: String,
}

#[async_trait::async_trait]
impl ArtifactSource for InProcessPluginArtifactSource {
    fn server_authority_identity(&self) -> &str {
        &self.authority
    }

    async fn authorize(
        &self,
        request: &PluginArtifactDeliveryRequest,
    ) -> std::result::Result<PluginArtifactDeliveryMetadata, DeliveryError> {
        if authority_identity(&self.api).map_err(|_| DeliveryError::Denied)? != self.authority {
            return Err(DeliveryError::Denied);
        }
        metadata_for_worker(&self.api, &self.worker, request.clone())
            .await
            .map_err(delivery_error)
    }

    async fn fetch(
        &self,
        request: &PluginArtifactDeliveryRequest,
    ) -> std::result::Result<ArtifactStream, DeliveryError> {
        self.authorize(request).await?;
        let bytes = bytes_for_worker(&self.api, &self.worker, request.clone())
            .await
            .map_err(delivery_error)?;
        Ok(Box::pin(std::io::Cursor::new(bytes)))
    }
}
