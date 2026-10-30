//! Owner-only observations reuse live execution policy without issuing authority.
//! No artifact bytes, guest, invocation or receipt is involved. The DB snapshot
//! and live Worker checks are point-in-time observations, not an execution lease.
use super::*;
use crate::plugin_host as policy;
use server_api::authorization::{OperationAuthorizationRequest, PermissionResource};
use server_api::*;

fn reason(error: Error) -> PluginHostErrorCode {
    policy::map_error(error).code
}

fn wire_limit(value: impl TryInto<u32>) -> Result<u32> {
    value
        .try_into()
        .map_err(|_| Error::Config("Plugin Runtime budget exceeds diagnostic wire range".into()))
}

fn runtime_limits(limits: &yoi_plugin_runtime::Limits) -> Result<PluginRuntimeLimits> {
    Ok(PluginRuntimeLimits {
        payload_bytes: wire_limit(limits.payload_bytes)?,
        config_bytes: wire_limit(limits.config_bytes)?,
        memory_bytes: wire_limit(limits.memory_bytes)?,
        table_elements: wire_limit(limits.table_elements)?,
        instances: wire_limit(limits.instances)?,
        memories: wire_limit(limits.memories)?,
        tables: wire_limit(limits.tables)?,
        resources: wire_limit(limits.resources)?,
        fuel: wire_limit(limits.fuel)?,
        wall_time_ms: wire_limit(limits.wall_time.as_millis())?,
        host_operations: wire_limit(limits.host_operations)?,
        http_body_bytes: wire_limit(limits.http_body_bytes)?,
        http_header_bytes: wire_limit(limits.http_header_bytes)?,
        http_header_pairs: wire_limit(limits.http_header_pairs)?,
        log_bytes: wire_limit(limits.log_bytes)?,
    })
}

fn effective_limits(
    runtime: &yoi_plugin_runtime::Limits,
    binding: &crate::plugin_binding::PluginConnectionLimits,
) -> PluginDiagnosticConnectionLimits {
    PluginDiagnosticConnectionLimits::Available {
        timeout_ms: u64::from(binding.timeout_ms).min(runtime.wall_time.as_millis() as u64) as u32,
        max_request_bytes: u64::from(binding.max_request_bytes).min(runtime.http_body_bytes as u64)
            as u32,
        max_response_bytes: u64::from(binding.max_response_bytes)
            .min(runtime.http_body_bytes as u64) as u32,
        max_header_bytes: u64::from(binding.max_header_bytes).min(runtime.http_header_bytes as u64)
            as u32,
    }
}

pub(super) async fn diagnostics(
    api: &WorkspaceApi,
    actor: RequestActor,
    workspace: String,
    runtime: String,
    worker: String,
) -> std::result::Result<PluginWorkerDiagnostics, RepositoryApiError> {
    // Management authority permits reading this projection only. Never turn the
    // human actor into a Worker source or borrow its authority for live policy.
    plugin::authorize(api, &workspace, &actor, true).await?;
    let store = api.config_store.clone();
    let w = workspace.clone();
    let r = runtime.clone();
    let k = worker.clone();
    let selection = store
        .db_executor()
        .run(move || {
            store.with_conn(|conn| crate::plugin_plan::worker_launch_selection(conn, &w, &r, &k))
        })
        .await
        .map_err(plugin::failure)?;
    let limits = yoi_plugin_runtime::Limits::default();
    let selection = match selection {
        None => PluginWorkerDiagnosticSelection::Unrecorded,
        Some(None) => PluginWorkerDiagnosticSelection::Empty,
        Some(Some(saved)) => {
            let worker_ref = RuntimeWorkerRef::new(&runtime, &worker);
            let execution = api
                .plugin_execution_identity(&worker_ref)
                .await
                .map_err(reason);
            let api = api.clone();
            let store = api.config_store.clone();
            let w = workspace.clone();
            store
                .db_executor()
                .run(move || {
                    // Keep the existing Subject/delegation owner outside the
                    // config-store lock, as in the live Host boundary.
                    let scopes: Vec<_> = saved
                        .plan
                        .plugins()
                        .iter()
                        .map(|pin| {
                            let scope = |operation: &str| {
                                subject_authorization::authorize_scope(
                                    &api,
                                    &worker_ref,
                                    &OperationAuthorizationRequest {
                                        operation: operation.into(),
                                        resource: PermissionResource::PluginInstallation {
                                            workspace_id: w.clone(),
                                            installation_id: pin.installation_id().into(),
                                        },
                                    },
                                )
                                .map(|_| ())
                                .map_err(|error| reason(error.error))
                            };
                            scope("plugin.admit")?;
                            if !pin.projection().services.is_empty() {
                                scope("plugin.service.consume")?;
                            }
                            Ok(())
                        })
                        .collect();
                    store.with_conn_mut(|conn| {
                        // Deferred read transaction: policy::authorize and
                        // live_binding only SELECT; dropping it writes nothing.
                        let tx = conn
                            .transaction_with_behavior(rusqlite::TransactionBehavior::Deferred)?;
                        let saved = crate::plugin_plan::plugin_plan_load(&tx, &w, &saved.plan_id)?;
                        let runtime = yoi_plugin_runtime::Limits::default();
                        let mut plugins = Vec::with_capacity(saved.plan.plugins().len());
                        for (pin, scope) in saved.plan.plugins().iter().zip(scopes) {
                            let observed =
                                execution
                                    .as_ref()
                                    .map_err(|code| *code)
                                    .and_then(|execution| {
                                        scope?;
                                        if !pin.projection().tools.is_empty() || pin.projection().services.is_empty() {
                                            policy::authorize(
                                                &tx,
                                                &w,
                                                execution,
                                                &saved.plan_id,
                                                pin.installation_id(),
                                                &pin.projection().tools,
                                                &PluginHostOperation::Admit,
                                            ).map_err(reason)?;
                                        }
                                        for service in &pin.projection().services {
                                            policy::authorize_service(
                                                &tx, &w, execution, &saved.plan_id,
                                                pin.installation_id(), service,
                                                &PluginHostOperation::Admit,
                                            ).map_err(reason)?;
                                        }
                                        Ok(())
                                    });
                            let admission = match observed {
                                Ok(()) => PluginDiagnosticAdmission::Admissible,
                                Err(reason_code) => {
                                    PluginDiagnosticAdmission::Refused { reason_code }
                                }
                            };
                            let connections = pin
                                .projection()
                                .bindings
                                .iter()
                                .map(|reference| {
                                    let effective_limits =
                                        match policy::live_binding(&tx, &w, reference) {
                                            Ok(binding) => {
                                                effective_limits(&runtime, &binding.limits)
                                            }
                                            Err(error) => {
                                                PluginDiagnosticConnectionLimits::Unavailable {
                                                    reason_code: reason(error),
                                                }
                                            }
                                        };
                                    PluginDiagnosticConnection {
                                        slot: reference.slot.clone(),
                                        effective_limits,
                                    }
                                })
                                .collect();
                            let consumers = tx.prepare(
                                "SELECT consumer_id FROM plugin_service_consumers WHERE workspace_id=?1 AND plan_id=?2 AND installation_id=?3 ORDER BY service_name LIMIT 32"
                            )?.query_map(rusqlite::params![w, saved.plan_id, pin.installation_id()], |row| row.get::<_, String>(0))?
                                .collect::<rusqlite::Result<Vec<_>>>()?;
                            let service_consumers = consumers.into_iter().map(|id| {
                                let consumer = crate::plugin_service::load(&tx, &w, &id)?;
                                Ok(PluginServiceConsumerDiagnostics {
                                    service_name: consumer.service.clone(),
                                    diagnostics: crate::plugin_service::diagnostics(&tx, &consumer, chrono::Utc::now().timestamp_millis())?,
                                })
                            }).collect::<Result<Vec<_>>>()?;
                            plugins.push(PluginWorkerDiagnostic {
                                installation_id: pin.installation_id().into(),
                                package_id: pin.package_id().into(),
                                package_version: pin.package_version().into(),
                                package_digest: pin.package_digest().into(),
                                artifact_digest: pin.artifact_digest().into(),
                                artifact_size: pin.artifact_size(),
                                surfaces: pin
                                    .projection()
                                    .surfaces
                                    .iter()
                                    .map(|surface| match surface {
                                        crate::plugin_plan::PluginSurface::Tool => {
                                            PluginDiagnosticSurface::Tool
                                        }
                                        crate::plugin_plan::PluginSurface::Service => {
                                            PluginDiagnosticSurface::Service
                                        }
                                    })
                                    .collect(),
                                selected_tools: pin.projection().tools.clone(),
                                selected_services: pin.projection().services.clone(),
                                service_consumers,
                                admission,
                                connections,
                            });
                        }
                        let pin_state = match saved.pin_state {
                            crate::plugin_plan::PlanPinState::Pending => {
                                PluginDiagnosticPinState::Pending
                            }
                            crate::plugin_plan::PlanPinState::Active => {
                                PluginDiagnosticPinState::Active
                            }
                            crate::plugin_plan::PlanPinState::Restorable => {
                                PluginDiagnosticPinState::Restorable
                            }
                            crate::plugin_plan::PlanPinState::Released => {
                                PluginDiagnosticPinState::Released
                            }
                        };
                        Ok(PluginWorkerDiagnosticSelection::Plan {
                            plan_id: saved.plan_id,
                            fingerprint: saved.fingerprint,
                            pin_state,
                            plugins,
                        })
                    })
                })
                .await
                .map_err(plugin::failure)?
        }
    };
    Ok(PluginWorkerDiagnostics {
        workspace_id: workspace,
        runtime_id: runtime,
        worker_id: worker,
        runtime_limits: runtime_limits(&limits).map_err(plugin::failure)?,
        selection,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_limits_intersect_each_runtime_and_binding_budget() {
        let runtime = yoi_plugin_runtime::Limits {
            wall_time: std::time::Duration::from_millis(200),
            http_body_bytes: 300,
            http_header_bytes: 100,
            ..Default::default()
        };
        for (binding, expected) in [
            ((150, 200, 250, 50), (150, 200, 250, 50)),
            ((250, 400, 500, 150), (200, 300, 300, 100)),
            ((150, 400, 250, 150), (150, 300, 250, 100)),
        ] {
            let binding = crate::plugin_binding::PluginConnectionLimits {
                timeout_ms: binding.0,
                max_request_bytes: binding.1,
                max_response_bytes: binding.2,
                max_header_bytes: binding.3,
            };
            assert_eq!(
                effective_limits(&runtime, &binding),
                PluginDiagnosticConnectionLimits::Available {
                    timeout_ms: expected.0,
                    max_request_bytes: expected.1,
                    max_response_bytes: expected.2,
                    max_header_bytes: expected.3,
                }
            );
        }
    }
}
