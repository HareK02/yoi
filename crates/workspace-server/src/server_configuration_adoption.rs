//! Owner-only HTTP boundary for source adoption and the one-way legacy cutover.
//!
//! Every preview and adoption acquires the native Feature authority guard before
//! entering the Server transaction. Subject references are checked against current
//! metadata/deletion fences under its IMMEDIATE transaction, not a preloaded set.
//! Preview and canonical saves never seal: their nonmutating Feature transaction
//! remains held through the Server operation and rolls back when the guard drops.
//!
//! Only migration adoption seals. Seal commits Feature evidence, not Server
//! adoption; the outer connection/lifecycle guard remains held until Server commit
//! or rollback, and no Feature access follows the seal. Legacy logical writers
//! must use that same domain guard and require_unsealed. IMMEDIATE serializes gate
//! users across connections/processes; after seal releases the SQLite write lock,
//! the durable seal rejects legacy writers. This is not a distributed commit:
//! Server failure can leave a pending transfer requiring the exact original retry.
//! Runtime operations must still check current policy and Subject lifecycle.
//! Callbacks must not reenter the Feature store or its canonical source provider.

use super::*;

use crate::config_adoption::{
    ConfigurationAdoptionRequest as AdoptionRequest, ConfigurationAdoptionResult as AdoptionResult,
    ConfigurationCurrent, ConfigurationMigrationBaseline, ConfigurationPolicyTransfer,
    request_fingerprint,
};
use crate::configuration_migration::{MigrationSeed, preview_migration};
use crate::policy_config::{AdoptedSubjectPolicy, MigrationDiagnostic};
use crate::subjektiv::{
    DatabaseIdentity, PermissionTransferReceipt, SubjektivError, SubjektivStore,
};
use feature_permissions::PermissionHost;
use server_api::{
    ConfigurationAdoptionApi, ConfigurationAdoptionReceipt, ConfigurationAdoptionRequest,
    ConfigurationMigrationDiagnostic, ConfigurationPreviewResponse, ConfigurationReconciliation,
    ConfigurationSourceResponse, ConfigurationSubjectPolicy, RepositoryApiError,
    RuntimePolicyApplication, ServerRequestContext,
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
        .merge(server_api::configuration_adoption_api_axum::configuration_reconcile(service))
        .layer(middleware::from_fn_with_state(
            context_service,
            attach_server_request_context,
        ))
}

async fn owner_operation<T: Send + 'static>(
    service: &WorkspaceServerApi,
    context: ServerRequestContext,
    workspace: String,
    operation: impl FnOnce(WorkspaceApi, String) -> Result<T> + Send + 'static,
) -> std::result::Result<T, RepositoryApiError> {
    // These owner-only routes are mounted above Workspace fallback dispatch.
    // A Runtime proof must not be ignored in favor of a shared human credential.
    if context.transport_headers.iter().any(|(name, _)| {
        name.eq_ignore_ascii_case(worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER)
    }) {
        return Err(ApiError::from(Error::WorkspacePermissionDenied(
            "Runtime source proofs cannot authorize owner configuration management".into(),
        ))
        .into_repository_api_error());
    }
    let actor = subject_authorization::administration_actor(&context)
        .map_err(ApiError::into_repository_api_error)?
        .clone();
    let result: ApiResult<T> = async {
        let gate = service.admission_for_workspace(&workspace).await;
        let lease = gate.admit(false).ok_or_else(|| {
            Error::WorkspaceConfigConflict("Workspace deletion is draining requests".into())
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
                return Err(Error::WorkspacePermissionDenied(
                    "configuration management requires the Workspace owner".into(),
                )
                .into());
            }
            if record.state != "active" {
                return Err(
                    Error::WorkspaceConfigConflict("Workspace is not active".into()).into(),
                );
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
                .run(move || operation(api, actor.account_id))
                .await
                .map_err(ApiError::from)
        })
        .await
    }
    .await;
    result.map_err(ApiError::into_repository_api_error)
}

fn conflict(message: &str) -> Error {
    Error::WorkspaceConfigConflict(message.into())
}

fn request_from_api(request: ConfigurationAdoptionRequest) -> Result<AdoptionRequest> {
    server_api::authorization::validate_authorization_identifier(&request.operation_id)
        .map_err(Error::InvalidInput)?;
    Ok(AdoptionRequest {
        operation_id: request.operation_id,
        expected_head: request.expected_head,
        expected_catalog_digest: request.expected_catalog_digest,
        expected_legacy_export: request.expected_legacy_export,
        source: config_commit_request_from_api(request.source)?,
    })
}

fn host(workspace: &str) -> Result<PermissionHost> {
    let workspace =
        feature_permissions::WorkspaceId::parse(workspace).map_err(Error::InvalidInput)?;
    worker::feature::permissions::builtin_permission_host(workspace)
        .map_err(|error| Error::Config(error.to_string()))
}

fn raw_legacy(api: &WorkspaceApi) -> Result<SubjektivStore> {
    // Intentionally do not call open_subjektiv_store: the normal helper connects
    // the canonical provider. Transfer preparation must retain the raw guard.
    open_legacy_subjektiv_store(api).map_err(|error| error.error)
}

fn transfer_error(error: SubjektivError) -> Error {
    match error {
        SubjektivError::PermissionTransferConflict | SubjektivError::PermissionAuthoritySealed => {
            conflict("legacy permission transfer changed or is sealed for another request")
        }
        error => subjektiv_store_error(error),
    }
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

fn pending_transfer(
    legacy: &SubjektivStore,
    workspace: &str,
    account: &str,
    request: &AdoptionRequest,
) -> Result<Option<PermissionTransferReceipt>> {
    let identity = legacy.database_identity().map_err(transfer_error)?;
    if identity.scope_id != workspace || identity.feature_id != subjektiv::SUBJEKTIV_FEATURE_ID {
        return Err(Error::WorkspaceIdMismatch);
    }
    // Read-only guarded lookup verifies native identity; lookup must never seal.
    let receipt = legacy
        .permission_transfer_receipt(&identity)
        .map_err(transfer_error)?;
    if let Some(receipt) = &receipt {
        if receipt.snapshot.identity != identity
            || receipt.transfer_id != request_fingerprint(workspace, account, request)?
            || request.expected_legacy_export.as_deref()
                != Some(receipt.snapshot.export_digest.as_str())
        {
            return Err(conflict(
                "sealed transfer belongs to another adoption request",
            ));
        }
    }
    Ok(receipt)
}

fn baseline(seed: &MigrationSeed) -> ConfigurationMigrationBaseline<'_> {
    ConfigurationMigrationBaseline {
        original: &seed.original,
        state: &seed.state,
        policies: &seed.policies,
        diagnostics: &seed.diagnostics,
        export_digest: &seed.snapshot.export_digest,
    }
}

struct Preparation {
    host: PermissionHost,
    schema: ::config_source::WorkspaceConfigSchemaBundle,
    legacy: SubjektivStore,
    identity: DatabaseIdentity,
    seed: Option<MigrationSeed>,
}

fn prepare(api: &WorkspaceApi, account: &str, request: &AdoptionRequest) -> Result<Preparation> {
    let current = source_snapshot(api, account)?;
    if current.canonical && request.expected_legacy_export.is_some() {
        return Err(conflict(
            "legacy transfer is already adopted; reload current source",
        ));
    }
    let host = host(&api.config.workspace_id)?;
    let schema = api.config_schema_registry.compose()?;
    let legacy = raw_legacy(api)?;
    let sealed = if current.canonical {
        None
    } else {
        pending_transfer(&legacy, &api.config.workspace_id, account, request)?
    };
    let seed = if current.canonical {
        None
    } else {
        let seed = preview_migration(
            &api.config_store,
            &legacy,
            &api.config.workspace_id,
            schema.clone(),
            &host,
        )?;
        if sealed
            .as_ref()
            .is_some_and(|receipt| receipt.snapshot != seed.snapshot)
        {
            return Err(conflict(
                "sealed transfer differs from the evaluated migration snapshot",
            ));
        }
        Some(seed)
    };
    // Observe identity outside the guarded Server operation. Inside the native
    // gate, use only authority methods; reopening this store would reenter it.
    let identity = match &seed {
        Some(seed) => seed.snapshot.identity.clone(),
        None => legacy.database_identity().map_err(transfer_error)?,
    };
    Ok(Preparation {
        host,
        schema,
        legacy,
        identity,
        seed,
    })
}

fn policy_to_api(policy: AdoptedSubjectPolicy) -> ConfigurationSubjectPolicy {
    ConfigurationSubjectPolicy {
        subject_id: policy.source.subject_id,
        change_id: policy.change_id,
        execution: policy.source.execution.assignment(),
        delegation: policy.source.delegation.assignment(),
        legacy_constraints: policy.legacy_constraints,
    }
}

fn diagnostic_to_api(diagnostic: MigrationDiagnostic) -> ConfigurationMigrationDiagnostic {
    match diagnostic {
        MigrationDiagnostic::Converted {
            subject_id,
            operation,
        } => ConfigurationMigrationDiagnostic::Converted {
            subject_id,
            operation,
        },
        MigrationDiagnostic::DomainConstraint {
            subject_id,
            permission,
        } => ConfigurationMigrationDiagnostic::DomainConstraint {
            subject_id,
            permission,
        },
        MigrationDiagnostic::ExplicitConfigurationRequired {
            subject_id,
            permission,
        } => ConfigurationMigrationDiagnostic::ExplicitConfigurationRequired {
            subject_id,
            permission,
        },
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
    async fn configuration_source(
        &self,
        context: ServerRequestContext,
        workspace_id: String,
    ) -> std::result::Result<ConfigurationSourceResponse, RepositoryApiError> {
        owner_operation(self, context, workspace_id, |api, account| {
            let current = source_snapshot(&api, &account)?;
            let host = host(&api.config.workspace_id)?;
            let catalog_digest = crate::config_adoption::catalog_digest(&host)?;
            let (state, expected_legacy_export, policies, diagnostics) = if current.canonical {
                (current.state, None, current.policies, vec![])
            } else {
                let legacy = raw_legacy(&api)?;
                let seed = preview_migration(
                    &api.config_store,
                    &legacy,
                    &api.config.workspace_id,
                    api.config_schema_registry.compose()?,
                    &host,
                )?;
                let observed = source_snapshot(&api, &account)?;
                if observed.canonical
                    || observed.history_head != current.history_head
                    || observed.state != seed.original
                {
                    return Err(conflict(
                        "configuration changed during migration preview; reload source",
                    ));
                }
                (
                    seed.state,
                    Some(seed.snapshot.export_digest),
                    seed.policies,
                    seed.diagnostics,
                )
            };
            Ok(ConfigurationSourceResponse {
                state: workspace_config_state_to_api(state),
                history_head: current.history_head,
                source_stored: current.canonical,
                catalog: host.catalog(),
                catalog_digest,
                expected_legacy_export,
                policies: policies.into_values().map(policy_to_api).collect(),
                migration_diagnostics: diagnostics.into_iter().map(diagnostic_to_api).collect(),
            })
        })
        .await
    }

    async fn configuration_preview(
        &self,
        context: ServerRequestContext,
        workspace_id: String,
        request: ConfigurationAdoptionRequest,
    ) -> std::result::Result<ConfigurationPreviewResponse, RepositoryApiError> {
        owner_operation(self, context, workspace_id, move |api, account| {
            let request = request_from_api(request)?;
            // Detect operation-ID reuse even though preview itself never persists.
            saved_result(&api, &account, &request)?;
            let Preparation {
                host,
                schema,
                legacy,
                identity,
                seed,
            } = prepare(&api, &account, &request)?;
            let baseline = seed.as_ref().map(baseline);
            // Preview never seals. The native read-only IMMEDIATE transaction
            // fences metadata updates through core validation, then rolls back.
            let preview = legacy
                .with_permission_authority_guard(&identity, |authority| {
                    Ok(api.config_store.preview_configuration_adoption(
                        &api.config.workspace_id,
                        &account,
                        &request,
                        schema,
                        &host,
                        baseline.as_ref(),
                        |subject| authority.require_subject(subject).map_err(transfer_error),
                    ))
                })
                .map_err(transfer_error)??;
            Ok(ConfigurationPreviewResponse {
                expected_head: preview.expected_head,
                state: workspace_config_state_to_api(preview.state),
                changed_paths: preview.changed_paths,
                changed_subjects: preview.changed_subjects,
            })
        })
        .await
    }

    async fn configuration_adopt(
        &self,
        context: ServerRequestContext,
        workspace_id: String,
        request: ConfigurationAdoptionRequest,
    ) -> std::result::Result<ConfigurationAdoptionReceipt, RepositoryApiError> {
        owner_operation(self, context, workspace_id, move |api, account| {
            let request = request_from_api(request)?;
            if let Some(result) = saved_result(&api, &account, &request)? {
                return Ok(receipt_to_api(result));
            }
            let Preparation {
                host,
                schema,
                legacy,
                identity,
                seed,
            } = prepare(&api, &account, &request)?;
            let baseline = seed.as_ref().map(baseline);
            // Feature connection + IMMEDIATE transaction always precede Server.
            // Never call store-level Feature APIs from inside this closure.
            let result = legacy
                .with_permission_authority_guard(&identity, |authority| {
                    // Keep Server failures as the inner Result. Only native gate
                    // acquisition/finalization failures belong to the outer lane.
                    Ok(api.config_store.adopt_configuration(
                        &api.config.workspace_id,
                        &account,
                        &request,
                        schema,
                        &host,
                        baseline.as_ref(),
                        |subject| authority.require_subject(subject).map_err(transfer_error),
                        |fingerprint| {
                            // Canonical saves must not seal. Their Feature transaction
                            // stays nonmutating until guard-drop rollback after core.
                            let seed = seed.as_ref().ok_or_else(|| {
                                conflict("canonical adoption must not prepare a legacy transfer")
                            })?;
                            // Core completes every reference check before this final
                            // Feature access. One method verifies a retained seal or
                            // creates the first one; the rest is in-memory work only.
                            let receipt = authority
                                .seal_permission_sources(fingerprint, &seed.snapshot.export_digest)
                                .map_err(transfer_error)?;
                            if receipt.transfer_id != fingerprint
                                || receipt.snapshot != seed.snapshot
                            {
                                return Err(conflict(
                                    "sealed transfer differs from the evaluated migration snapshot",
                                ));
                            }
                            Ok(ConfigurationPolicyTransfer {
                                transfer_identity: receipt.transfer_id,
                                physical_identity: receipt.snapshot.identity.physical_id,
                                export_digest: receipt.snapshot.export_digest,
                                policies: seed.policies.clone(),
                            })
                        },
                    ))
                })
                .map_err(transfer_error)??;
            // Core and Feature guards are released before best-effort Runtime
            // publication. Dispatch transfers the admitted lifetime without
            // keeping the SQL lane occupied by an external Runtime response.
            // Exact historical receipt replay above must not publish old state.
            let publication_api = api.clone();
            let state = result.state.clone();
            let _ = api.host_executors.host().and_then(|executor| {
                executor.dispatch(move || {
                    if let Ok(projection) = publication_api
                        .prompt_projection_cache
                        .resolve(&publication_api.config.workspace_id, &state)
                    {
                        let _ = publication_api
                            .runtime
                            .observe_workspace_prompt_projection((*projection).clone());
                    }
                })
            });
            Ok(receipt_to_api(result))
        })
        .await
    }

    async fn configuration_reconcile(
        &self,
        context: ServerRequestContext,
        workspace_id: String,
        request: ConfigurationAdoptionRequest,
    ) -> std::result::Result<ConfigurationReconciliation, RepositoryApiError> {
        owner_operation(self, context, workspace_id, move |api, account| {
            let request = request_from_api(request)?;
            if let Some(result) = saved_result(&api, &account, &request)? {
                return Ok(ConfigurationReconciliation::Adopted {
                    receipt: receipt_to_api(result),
                });
            }
            // Canonical operations do not use the old transfer. A missing normal
            // save receipt is not evidence about the unrelated initial cutover.
            if source_snapshot(&api, &account)?.canonical {
                return Ok(match saved_result(&api, &account, &request)? {
                    Some(result) => ConfigurationReconciliation::Adopted {
                        receipt: receipt_to_api(result),
                    },
                    None => ConfigurationReconciliation::NotObserved,
                });
            }
            // No draft generation, validation against today's head/catalog, seal,
            // or adoption here: this is observation of the exact original input.
            let legacy = raw_legacy(&api)?;
            let pending = pending_transfer(&legacy, &api.config.workspace_id, &account, &request)?;
            // Adoption may have committed between the first lookup and Feature read.
            if let Some(result) = saved_result(&api, &account, &request)? {
                return Ok(ConfigurationReconciliation::Adopted {
                    receipt: receipt_to_api(result),
                });
            }
            Ok(match pending {
                Some(receipt) => ConfigurationReconciliation::PendingTransfer {
                    transfer_identity: receipt.transfer_id,
                    export_digest: receipt.snapshot.export_digest,
                },
                None => ConfigurationReconciliation::NotObserved,
            })
        })
        .await
    }
}
