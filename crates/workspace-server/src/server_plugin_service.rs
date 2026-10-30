//! Authenticated Remote and Embedded Service operations share this owner.
use super::*;
use crate::plugin_service as store;
use server_api::authorization::{OperationAuthorizationRequest, PermissionResource};

async fn check_service(
    api: Arc<WorkspaceApi>,
    context: ServerRequestContext,
    workspace: String,
    plan_id: String,
    installation: String,
    name: String,
    operation: PluginHostOperation,
    preparing: bool,
) -> std::result::Result<policy::AdmittedPluginOperation, PluginHostError> {
    validate_workspace_scope(&api, &workspace).map_err(|e| policy::map_error(e.error))?;
    let worker = subject_authorization::source_from_context(&context).map_err(|_| denied())?;
    let execution = if preparing
        && api
            .store
            .has_active_worker_create_reservation(api.workspace_id(), &worker)
            .map_err(policy::map_error)?
    {
        api.plugin_reserved_identity(&worker, None, None)
            .map_err(policy::map_error)?
    } else {
        api.plugin_execution_identity(&worker)
            .await
            .map_err(policy::map_error)?
    };
    let executor = api.config_store.db_executor();
    executor
        .run(move || {
            with_authority_database(&api, || {
                for op in ["plugin.service.consume", operation_name(&operation)] {
                    subject_authorization::authorize_scope(
                        &api,
                        &worker,
                        &OperationAuthorizationRequest {
                            operation: op.into(),
                            resource: PermissionResource::PluginInstallation {
                                workspace_id: workspace.clone(),
                                installation_id: installation.clone(),
                            },
                        },
                    )
                    .map_err(|e| e.error)?;
                }
                api.config_store.with_immediate_transaction(|tx| {
                    policy::authorize_service(
                        tx,
                        &workspace,
                        &execution,
                        &plan_id,
                        &installation,
                        &name,
                        &operation,
                    )
                })
            })
        })
        .await
        .map_err(policy::map_error)
}

pub(super) async fn check_consumer(
    api: Arc<WorkspaceApi>,
    context: ServerRequestContext,
    workspace: String,
    plan: String,
    installation: String,
    consumer_id: &str,
    event_id: &str,
    name: &str,
    operation: PluginHostOperation,
) -> std::result::Result<policy::AdmittedPluginOperation, PluginHostError> {
    let lease = service_lease().ok_or_else(denied)?;
    if lease.consumer_id != consumer_id {
        return Err(denied());
    }
    let admitted = check_service(
        api.clone(),
        context,
        workspace.clone(),
        plan.clone(),
        installation.clone(),
        name.into(),
        operation,
        false,
    )
    .await?;
    let config = api.config_store.clone();
    let name = name.to_owned();
    let event = event_id.to_owned();
    let execution = admitted.plan.plan.execution().clone();
    config
        .db_executor()
        .run(move || {
            config.with_conn(|conn| {
                let c = store::fence(conn, &workspace, &lease, now_ms())?;
                if c.plan_id != plan
                    || c.installation != installation
                    || c.service != name
                    || c.current_event_id.as_deref() != Some(&event)
                    || c.runtime_id != execution.runtime_id
                    || c.worker_id != execution.worker_id
                    || c.pause.is_some()
                {
                    return Err(policy::failure(PluginHostErrorCode::Denied));
                }
                Ok(())
            })
        })
        .await
        .map_err(policy::map_error)?;
    Ok(admitted)
}

async fn checked_consumer(
    api: Arc<WorkspaceApi>,
    context: ServerRequestContext,
    workspace: String,
    id: String,
    lease: Option<PluginServiceLease>,
) -> std::result::Result<(store::Consumer, policy::AdmittedPluginOperation), PluginHostError> {
    let config = api.config_store.clone();
    let w = workspace.clone();
    let proof = lease.clone();
    let c = config
        .db_executor()
        .run(move || {
            config.with_conn(|conn| match proof {
                Some(proof) => store::fence(conn, &w, &proof, now_ms()),
                None => store::load(conn, &w, &id),
            })
        })
        .await
        .map_err(policy::map_error)?;
    let worker = subject_authorization::source_from_context(&context).map_err(|_| denied())?;
    let checked = check_service(
        api.clone(),
        context,
        workspace,
        c.plan_id.clone(),
        c.installation.clone(),
        c.service.clone(),
        PluginHostOperation::Admit,
        false,
    )
    .await;
    let admitted = match checked {
        Ok(admitted) => admitted,
        Err(error) => {
            if c.runtime_id == worker.runtime_id && c.worker_id == worker.worker_id {
                if let Some(proof) = lease {
                    let config = api.config_store.clone();
                    // Do not let an old driver's delayed authorization failure
                    // pause a newly leased consumer (or mutate diagnostics reads).
                    let _ = config
                        .db_executor()
                        .run(move || {
                            config.with_immediate_transaction(|tx| {
                                let current = store::fence(tx, &c.workspace, &proof, now_ms())?;
                                store::pause(tx, &current, PluginServicePauseReason::Authorization)
                            })
                        })
                        .await;
                }
            }
            return Err(error);
        }
    };
    let execution = admitted.plan.plan.execution();
    if c.runtime_id != execution.runtime_id || c.worker_id != execution.worker_id {
        return Err(denied());
    }
    Ok((c, admitted))
}

pub(super) async fn execute(
    api: Arc<WorkspaceApi>,
    context: ServerRequestContext,
    workspace: String,
    request: PluginServiceRequest,
    transport_override: Option<Arc<https::HttpsTransport>>,
) -> std::result::Result<PluginServiceResponse, PluginHostError> {
    match request {
        PluginServiceRequest::Prepare {
            plan_id,
            installation_id,
            service_name,
            operation,
        } => {
            let operation = match operation {
                PluginServiceLifecycleOperation::Admit => PluginHostOperation::Admit,
                PluginServiceLifecycleOperation::Publish => PluginHostOperation::Publish,
            };
            check_service(
                api,
                context,
                workspace,
                plan_id,
                installation_id,
                service_name,
                operation,
                true,
            )
            .await?;
            Ok(PluginServiceResponse::Prepared)
        }
        PluginServiceRequest::LifecycleLog {
            plan_id,
            installation_id,
            service_name,
            entry,
        } => {
            check_service(
                api,
                context,
                workspace,
                plan_id.clone(),
                installation_id,
                service_name,
                PluginHostOperation::Log,
                true,
            )
            .await?;
            validate_log_entry(&entry)?;
            // Do not retain untrusted guest text/fields, including echoed secrets.
            tracing::debug!(plan = %plan_id, "plugin service lifecycle log admitted");
            Ok(PluginServiceResponse::Logged)
        }
        PluginServiceRequest::Acquire {
            plan_id,
            installation_id,
            service_name,
        } => {
            let admitted = check_service(
                api.clone(),
                context,
                workspace,
                plan_id,
                installation_id.clone(),
                service_name.clone(),
                PluginHostOperation::Admit,
                false,
            )
            .await?;
            let declarations = admitted
                .services
                .iter()
                .find(|s| s.name == service_name)
                .ok_or_else(denied)?;
            let subscriptions = declarations
                .subscriptions
                .iter()
                .map(|s| match s.as_str() {
                    "ticket_changed" => Ok(PluginTicketEventType::Changed),
                    "ticket_deleted" => Ok(PluginTicketEventType::Deleted),
                    _ => Err(denied()),
                })
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let config = api.config_store.clone();
            let lease = config
                .db_executor()
                .run(move || {
                    config.with_immediate_transaction(|tx| {
                        store::acquire(
                            tx,
                            &admitted.plan,
                            &installation_id,
                            &service_name,
                            &subscriptions,
                            now_ms(),
                        )
                    })
                })
                .await
                .map_err(policy::map_error)?;
            Ok(PluginServiceResponse::Acquired { lease })
        }
        PluginServiceRequest::Poll { lease } => {
            let (_, admitted) = checked_consumer(
                api.clone(),
                context.clone(),
                workspace.clone(),
                lease.consumer_id.clone(),
                Some(lease.clone()),
            )
            .await?;
            let config = api.config_store.clone();
            let w = workspace.clone();
            let proof = lease.clone();
            let response = config
                .db_executor()
                .run(move || {
                    config.with_immediate_transaction(|tx| {
                        store::poll(tx, &w, &proof, &admitted.plan, now_ms())
                    })
                })
                .await
                .map_err(policy::map_error)?;
            let PluginServiceResponse::Delivery { envelope, .. } = &response else {
                return Ok(response);
            };
            // Subject authorization re-enters the config store for live Worker
            // lineage. Never call it while holding the consumer write transaction.
            // The envelope remains Host-local until authorization and a fresh
            // lease/event fence both succeed; no guest sees an unchecked delivery.
            let config = api.config_store.clone();
            let ticket = envelope.ticket_id.clone();
            let event = envelope.event_id.clone();
            let kind = envelope.event_type;
            config
                .db_executor()
                .run(move || {
                    let worker = subject_authorization::source_from_context(&context)
                        .map_err(|e| e.error)?;
                    let allowed = subject_authorization::authorize(
                        &api,
                        &worker,
                        &OperationAuthorizationRequest {
                            operation: "ticket.read".into(),
                            resource: PermissionResource::Ticket {
                                workspace_id: workspace.clone(),
                                ticket_id: ticket.clone(),
                            },
                        },
                    );
                    config.with_immediate_transaction(|tx| {
                        let c = store::fence(tx, &workspace, &lease, now_ms())?;
                        if c.current_event_id.as_deref() != Some(event.as_str()) {
                            return Err(policy::failure(PluginHostErrorCode::Denied));
                        }
                        // Deleted references are metadata only: this does not
                        // resolve or recreate a Ticket capability.
                        let exists: bool = tx.query_row(
                            "SELECT EXISTS(SELECT 1 FROM typed_tickets WHERE workspace_id=?1 AND ticket_id=?2)",
                            rusqlite::params![workspace, ticket],
                            |r| r.get(0),
                        )?;
                        if allowed.is_err() || (kind == PluginTicketEventType::Changed && !exists) {
                            store::pause(tx, &c, PluginServicePauseReason::Authorization)?;
                            return Ok(PluginServiceResponse::Diagnostics {
                                diagnostics: store::diagnostics(
                                    tx,
                                    &store::load(tx, &workspace, &lease.consumer_id)?,
                                    now_ms(),
                                )?,
                            });
                        }
                        Ok(response)
                    })
                })
                .await
                .map_err(policy::map_error)
        }
        PluginServiceRequest::Complete {
            lease,
            event_id,
            result,
        } => {
            checked_consumer(
                api.clone(),
                context,
                workspace.clone(),
                lease.consumer_id.clone(),
                Some(lease.clone()),
            )
            .await?;
            let config = api.config_store.clone();
            config
                .db_executor()
                .run(move || {
                    config.with_immediate_transaction(|tx| {
                        store::complete(tx, &workspace, &lease, &event_id, &result, now_ms())
                    })
                })
                .await
                .map_err(policy::map_error)?;
            Ok(PluginServiceResponse::Completed)
        }
        PluginServiceRequest::Release { lease } => {
            // Authenticate the same Worker without an active lease fence or
            // revoked plugin grants: cleanup and lost-response reconciliation
            // must remain possible after expiry, release, or lease rotation.
            validate_workspace_scope(&api, &workspace).map_err(|e| policy::map_error(e.error))?;
            let worker =
                subject_authorization::source_from_context(&context).map_err(|_| denied())?;
            let config = api.config_store.clone();
            config
                .db_executor()
                .run(move || {
                    config.with_immediate_transaction(|tx| {
                        let c = store::load(tx, &workspace, &lease.consumer_id)?;
                        if c.runtime_id != worker.runtime_id || c.worker_id != worker.worker_id {
                            return Err(policy::failure(PluginHostErrorCode::Denied));
                        }
                        store::release(tx, &workspace, &lease, now_ms())
                    })
                })
                .await
                .map_err(policy::map_error)?;
            Ok(PluginServiceResponse::Released)
        }
        PluginServiceRequest::Diagnostics { consumer_id } => {
            let (c, _) =
                checked_consumer(api.clone(), context, workspace, consumer_id, None).await?;
            let config = api.config_store.clone();
            let diagnostics = config
                .db_executor()
                .run(move || config.with_conn(|conn| store::diagnostics(conn, &c, now_ms())))
                .await
                .map_err(policy::map_error)?;
            Ok(PluginServiceResponse::Diagnostics { diagnostics })
        }
        PluginServiceRequest::Host { lease, request } => {
            // The finite invocation DTO cannot represent Service/control nesting.
            checked_consumer(
                api.clone(),
                context.clone(),
                workspace.clone(),
                lease.consumer_id.clone(),
                Some(lease.clone()),
            )
            .await?;
            let response = SERVICE_LEASE
                .scope(
                    lease,
                    Box::pin(execute_with_transport(
                        api,
                        context,
                        workspace,
                        (*request).into(),
                        transport_override,
                    )),
                )
                .await?;
            Ok(PluginServiceResponse::Host {
                response: Box::new(response.try_into().map_err(|_| denied())?),
            })
        }
    }
}

fn management_failure(error: Error) -> RepositoryApiError {
    let error = policy::map_error(error);
    let (status, code) = match error.code {
        PluginHostErrorCode::InvalidInput | PluginHostErrorCode::Unsupported => {
            (400, "plugin_service_invalid")
        }
        PluginHostErrorCode::Denied => (403, "plugin_service_denied"),
        PluginHostErrorCode::NotFound => (404, "plugin_service_not_found"),
        PluginHostErrorCode::Conflict | PluginHostErrorCode::ReceiptExpired => {
            (409, "plugin_service_conflict")
        }
        // A DB/transport failure is not proof that a recovery mutation failed.
        _ => (503, "plugin_service_outcome_unknown"),
    };
    RepositoryApiError::new(status, code, error.message(), vec![])
}

impl WorkspaceApi {
    /// Management observations never run a guest or require a live Runtime.
    pub async fn plugin_service_diagnostics(
        &self,
        workspace: String,
        actor: RequestActor,
        consumer_id: String,
    ) -> std::result::Result<PluginServiceDiagnostics, RepositoryApiError> {
        super::super::plugin::authorize(self, &workspace, &actor, false).await?;
        let config = self.config_store.clone();
        config
            .db_executor()
            .run(move || {
                config.with_conn(|conn| {
                    let consumer = store::load(conn, &workspace, &consumer_id)?;
                    store::diagnostics(conn, &consumer, now_ms())
                })
            })
            .await
            .map_err(management_failure)
    }

    /// Management adapter for the parent contract dispatcher. This is not a guest
    /// Host import; the ordinary owner-account verifier precedes the DB owner.
    pub async fn recover_plugin_service(
        &self,
        workspace: String,
        actor: RequestActor,
        request: PluginServiceRecoveryRequest,
    ) -> std::result::Result<PluginServiceDiagnostics, RepositoryApiError> {
        super::super::plugin::authorize(self, &workspace, &actor, true).await?;
        let actor = serde_json::to_string(&(&actor.account_id, &actor.user_id))
            .map_err(|_| management_failure(policy::failure(PluginHostErrorCode::InvalidInput)))?;
        let config = self.config_store.clone();
        config
            .db_executor()
            .run(move || {
                config.with_immediate_transaction(|tx| {
                    store::recover(
                        tx,
                        &workspace,
                        &request.consumer_id,
                        request.expected_pause,
                        &request.action,
                        &request.operation_key,
                        &actor,
                        &request.reason,
                        now_ms(),
                    )?;
                    store::diagnostics(
                        tx,
                        &store::load(tx, &workspace, &request.consumer_id)?,
                        now_ms(),
                    )
                })
            })
            .await
            .map_err(management_failure)
    }
}
