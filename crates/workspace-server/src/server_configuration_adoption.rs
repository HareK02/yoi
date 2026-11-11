//! Owner-only publication of shared configuration. Subject policy is a separate domain.
//! Preview is read-only; commit and exact-request reconciliation use Server-owned receipts.

use super::*;
#[path = "server_configuration_publication.rs"]
mod publication;
use crate::configuration_failure::{
    ConfigurationFailure, ConfigurationFailureOperation as FailureOperation,
    ConfigurationFailureReason as FailureReason,
};
use publication::{dispatch_publication, publication_to_api};

use crate::config_adoption::{
    ConfigurationAdoptionRequest as AdoptionRequest, ConfigurationAdoptionResult as AdoptionResult,
    ConfigurationCurrent, request_fingerprint,
};
use crate::policy_config::AdoptedSubjectPolicy;
use feature_permissions::PermissionHost;
use server_api::{
    ConfigurationAdoptionApi, ConfigurationAdoptionReceipt, ConfigurationAdoptionRequest,
    ConfigurationApiError, ConfigurationPreviewResponse, ConfigurationReconciliation,
    ConfigurationSourceResponse, ConfigurationSubjectPolicy, RuntimePolicyApplication,
    ServerRequestContext,
};

/// Mount on the Server router, under its cookie-mutation-origin middleware.
/// These routes own admission because they bypass Workspace fallback dispatch.
pub(super) fn routes(service: WorkspaceServerApi) -> Router {
    let context_service = ServerApiContractService::Server(service.clone());
    let service = Arc::new(service);
    Router::new()
        .merge(server_api::configuration_adoption_api_axum::configuration_source(service.clone()))
        .merge(server_api::configuration_adoption_api_axum::configuration_preview(service.clone()))
        .merge(server_api::configuration_adoption_api_axum::configuration_adopt(service.clone()))
        .merge(
            server_api::configuration_adoption_api_axum::configuration_reconcile(service.clone()),
        )
        .merge(
            server_api::configuration_adoption_api_axum::configuration_publication(service.clone()),
        )
        .merge(
            server_api::feature_permissions::feature_permissions_api_axum::permission_catalog(
                service.clone(),
            ),
        )
        .merge(
            server_api::feature_permissions::feature_permissions_api_axum::validate_permissions(
                service,
            ),
        )
        .layer(middleware::from_fn_with_state(
            context_service,
            attach_server_request_context,
        ))
}

pub(super) async fn owner_operation<T: Send + 'static>(
    service: &WorkspaceServerApi,
    context: ServerRequestContext,
    workspace: String,
    failure_operation: FailureOperation,
    operation: impl FnOnce(WorkspaceApi, String) -> Result<T> + Send + 'static,
) -> std::result::Result<T, ConfigurationApiError> {
    with_owner_operation(
        service,
        context,
        workspace,
        Some(failure_operation),
        operation,
        |error, can_view_cause| {
            if can_view_cause {
                if let Error::Configuration(cause) = &error.error {
                    use error_contract::{Browser, FailureFacts, project_failure};
                    let visibility = crate::configuration_failure::ConfigurationFailureVisibility {
                        can_view_cause: true,
                    };
                    if let Ok(projected) = FailureFacts::new(*cause.details())
                        .and_then(|facts| project_failure::<Browser, _>(&facts, &visibility))
                    {
                        return ConfigurationApiError::projected(projected);
                    }
                }
            }
            error.into_repository_api_error().into()
        },
    )
    .await
}

pub(super) async fn owner_repository_operation<T: Send + 'static>(
    service: &WorkspaceServerApi,
    context: ServerRequestContext,
    workspace: String,
    operation: impl FnOnce(WorkspaceApi, String) -> Result<T> + Send + 'static,
) -> std::result::Result<T, server_api::RepositoryApiError> {
    with_owner_operation(service, context, workspace, None, operation, |error, _| {
        error.into_repository_api_error()
    })
    .await
}

async fn with_owner_operation<T: Send + 'static, E>(
    service: &WorkspaceServerApi,
    context: ServerRequestContext,
    workspace: String,
    failure_operation: Option<FailureOperation>,
    operation: impl FnOnce(WorkspaceApi, String) -> Result<T> + Send + 'static,
    project_error: impl Fn(ApiError, bool) -> E,
) -> std::result::Result<T, E> {
    // These owner-only routes are mounted above Workspace fallback dispatch.
    // A Runtime proof must not be ignored in favor of a shared human credential.
    if context.transport_headers.iter().any(|(name, _)| {
        name.eq_ignore_ascii_case(worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER)
    }) {
        return Err(project_error(
            ApiError::from(Error::WorkspacePermissionDenied(
                "Runtime source proofs cannot authorize owner configuration management".into(),
            )),
            false,
        ));
    }
    let actor = subject_authorization::administration_actor(&context)
        .map_err(|error| project_error(error, false))?
        .clone();
    let mut can_view_cause = false;
    let result: ApiResult<T> = async {
        let gate = service.admission_for_workspace(&workspace).await;
        let lease = gate.admit(false).ok_or_else(|| {
            failure_operation.map_or_else(
                || Error::WorkspaceConfigConflict("Workspace deletion is draining requests".into()),
                |operation| {
                    ConfigurationFailure::new(operation, FailureReason::WorkspaceDraining).into()
                },
            )
        })?;
        crate::db_executor::with_operation_lifetime(Arc::new(lease), async {
            // Authenticate against catalog ownership before resolving an API:
            // API construction itself can load configuration and Feature stores.
            let record = service
                .store
                .get_workspace(&workspace)
                .await?
                .ok_or(Error::WorkspaceIdMismatch)?;
            if record.owner_account_id != actor.account_id {
                return Err(failure_operation
                    .map_or_else(
                        || {
                            Error::WorkspacePermissionDenied(
                                "configuration management requires the Workspace owner".into(),
                            )
                        },
                        |operation| {
                            ConfigurationFailure::new(operation, FailureReason::OwnerRequired)
                                .into()
                        },
                    )
                    .into());
            }
            can_view_cause = true;
            if record.state != "active" {
                return Err(failure_operation
                    .map_or_else(
                        || Error::WorkspaceConfigConflict("Workspace is not active".into()),
                        |operation| {
                            ConfigurationFailure::new(operation, FailureReason::WorkspaceInactive)
                                .into()
                        },
                    )
                    .into());
            }
            let api = service
                .api_for_workspace(&workspace)
                .await?
                .ok_or(Error::WorkspaceIdMismatch)?;
            require_workspace_owner(&api, &workspace, &actor, "Configuration adoption").await?;
            // The admitted lifetime survives HTTP cancellation. The synchronous
            // DB job owns native Feature guards through Server commit/rollback.
            service
                .store
                .db_executor()
                .run(move || {
                    operation(api, actor.account_id).map_err(|error| {
                        match (error, failure_operation) {
                            (Error::Configuration(cause), Some(operation)) => {
                                cause.with_operation(operation).into()
                            }
                            (other, _) => other,
                        }
                    })
                })
                .await
                .map_err(ApiError::from)
        })
        .await
    }
    .await;
    result.map_err(|error| project_error(error, can_view_cause))
}

fn request_from_api(request: ConfigurationAdoptionRequest) -> Result<AdoptionRequest> {
    server_api::authorization::validate_authorization_identifier(&request.operation_id)
        .map_err(Error::InvalidInput)?;
    Ok(AdoptionRequest {
        operation_id: request.operation_id,
        expected_head: request.expected_head,
        expected_catalog_digest: request.expected_catalog_digest,
        source: config_commit_request_from_api(request.source)?,
    })
}

fn host(api: &WorkspaceApi) -> Result<PermissionHost> {
    api.refresh_plugin_permission_registry()?;
    api.permission_registry
        .read()
        .map(|registry| registry.host().clone())
        .map_err(|_| Error::Config("current Feature declarations unavailable".into()))
}

fn source_snapshot(api: &WorkspaceApi, account: &str) -> Result<ConfigurationCurrent> {
    api.config_store
        .configuration_current(&api.config.workspace_id, account)
}

fn saved_result(
    api: &WorkspaceApi,
    account: &str,
    request: &AdoptionRequest,
) -> Result<Option<AdoptionResult>> {
    // Core authenticates the current owner and compares the complete request.
    api.config_store
        .configuration_request_result(&api.config.workspace_id, account, request)
}

fn policy_to_api(
    policy: AdoptedSubjectPolicy,
    host: &PermissionHost,
) -> ConfigurationSubjectPolicy {
    let execution = policy.source.execution.assignment();
    let delegation = policy.source.delegation.assignment();
    let execution_resolution = host.resolve(Some(&execution)).projection().clone();
    let delegation_resolution = host.resolve(Some(&delegation)).projection().clone();
    ConfigurationSubjectPolicy {
        subject_id: policy.source.subject_id,
        change_id: policy.change_id,
        execution,
        delegation,
        execution_resolution,
        delegation_resolution,
        legacy_constraints: policy.legacy_constraints,
    }
}

fn receipt_to_api(result: AdoptionResult) -> ConfigurationAdoptionReceipt {
    ConfigurationAdoptionReceipt {
        workspace_id: result.workspace_id,
        operation_id: result.operation_id,
        history_head: result.history_head,
        state: workspace_config_state_to_api(result.state),
        source_stored: result.source_stored,
        policy_committed: result.policy_committed,
        runtime_application: match result.runtime_application {
            crate::config_adoption::RuntimePolicyApplication::NextCurrentPolicyCheck => {
                RuntimePolicyApplication::NextCurrentPolicyCheck
            }
        },
    }
}

impl ConfigurationAdoptionApi for WorkspaceServerApi {
    async fn configuration_publication(
        &self,
        context: ServerRequestContext,
        workspace_id: String,
        operation_id: String,
    ) -> std::result::Result<server_api::ConfigurationPublicationLookup, ConfigurationApiError>
    {
        owner_operation(
            self,
            context,
            workspace_id,
            FailureOperation::Publication,
            move |api, account| {
                server_api::authorization::validate_authorization_identifier(&operation_id)
                    .map_err(Error::InvalidInput)?;
                Ok(server_api::ConfigurationPublicationLookup {
                    publication: api
                        .config_store
                        .configuration_publication(
                            &api.config.workspace_id,
                            &account,
                            &operation_id,
                        )?
                        .map(publication_to_api)
                        .transpose()?,
                })
            },
        )
        .await
    }

    async fn configuration_source(
        &self,
        context: ServerRequestContext,
        workspace_id: String,
    ) -> std::result::Result<ConfigurationSourceResponse, ConfigurationApiError> {
        owner_operation(
            self,
            context,
            workspace_id,
            FailureOperation::Source,
            |api, account| {
                let host = host(&api)?;
                let current = source_snapshot(&api, &account)?;
                let catalog_digest = crate::config_adoption::catalog_digest(&host)?;
                Ok(ConfigurationSourceResponse {
                    state: workspace_config_state_to_api(current.state),
                    history_head: current.history_head,
                    source_stored: true,
                    catalog: host.catalog(),
                    catalog_digest,
                    policies: current
                        .policies
                        .into_values()
                        .map(|policy| policy_to_api(policy, &host))
                        .collect(),
                })
            },
        )
        .await
    }

    async fn configuration_preview(
        &self,
        context: ServerRequestContext,
        workspace_id: String,
        request: ConfigurationAdoptionRequest,
    ) -> std::result::Result<ConfigurationPreviewResponse, ConfigurationApiError> {
        owner_operation(
            self,
            context,
            workspace_id,
            FailureOperation::Preview,
            move |api, account| {
                let request = request_from_api(request)?;
                // Detect operation-ID reuse even though preview itself never persists.
                saved_result(&api, &account, &request)?;
                api.refresh_plugin_permission_registry()?;
                let registry = api.permission_registry.read().map_err(|_| {
                    Error::Config("current Feature declarations unavailable".into())
                })?;
                let preview = api.config_store.preview_configuration_adoption(
                    &api.config.workspace_id,
                    &account,
                    &request,
                    api.config_schema_registry.compose()?,
                    registry.host(),
                )?;
                Ok(ConfigurationPreviewResponse {
                    expected_head: preview.expected_head,
                    state: workspace_config_state_to_api(preview.state),
                    changed_paths: preview.changed_paths,
                    changed_subjects: preview.changed_subjects,
                })
            },
        )
        .await
    }

    async fn configuration_adopt(
        &self,
        context: ServerRequestContext,
        workspace_id: String,
        request: ConfigurationAdoptionRequest,
    ) -> std::result::Result<ConfigurationAdoptionReceipt, ConfigurationApiError> {
        owner_operation(
            self,
            context,
            workspace_id,
            FailureOperation::Adopt,
            move |api, account| {
                let request = request_from_api(request)?;
                if let Some(result) = saved_result(&api, &account, &request)? {
                    return Ok(receipt_to_api(result));
                }
                api.refresh_plugin_permission_registry()?;
                let result = {
                    let registry = api.permission_registry.read().map_err(|_| {
                        Error::Config("current Feature declarations unavailable".into())
                    })?;
                    api.config_store.adopt_configuration(
                        &api.config.workspace_id,
                        &account,
                        &request,
                        api.config_schema_registry.compose()?,
                        registry.host(),
                    )?
                };
                // Core and Feature guards are released before best-effort Runtime
                // publication. Dispatch transfers the admitted lifetime without
                // keeping the SQL lane occupied by an external Runtime response.
                // Exact historical receipt replay above must not publish old state.
                let publication_identity = crate::config_publication::PublicationIdentity {
                    request_fingerprint: request_fingerprint(
                        &api.config.workspace_id,
                        &account,
                        &request,
                    )?,
                    receipt: result.clone(),
                };
                dispatch_publication(&api, account, publication_identity);
                Ok(receipt_to_api(result))
            },
        )
        .await
    }

    async fn configuration_reconcile(
        &self,
        context: ServerRequestContext,
        workspace_id: String,
        request: ConfigurationAdoptionRequest,
    ) -> std::result::Result<ConfigurationReconciliation, ConfigurationApiError> {
        owner_operation(
            self,
            context,
            workspace_id,
            FailureOperation::Reconcile,
            move |api, account| {
                let request = request_from_api(request)?;
                if let Some(result) = saved_result(&api, &account, &request)? {
                    return Ok(ConfigurationReconciliation::Adopted {
                        receipt: receipt_to_api(result),
                    });
                }
                Ok(ConfigurationReconciliation::NotObserved)
            },
        )
        .await
    }
}
