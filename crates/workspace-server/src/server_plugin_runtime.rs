//! Embedded composition keeps only a weak Server view in the Runtime factory.
//! The outer WorkspaceApi owns the strong view; the inner view does not retain
//! itself. This avoids Runtime -> WorkspaceApi -> Runtime strong cycles
//! while retaining the same proof verifier, plan owner and artifact source.
use super::*;
use worker_runtime::plugin::{PluginRuntimeProviders, PluginRuntimeServices};

use crate::plugin_plan::{WORKER_LAUNCH_ACTOR as LAUNCH_ACTOR, worker_launch_key as launch_key};

impl WorkspaceApi {
    /// Seal the reserved launch and its Plugin pins in one write transaction.
    /// No external dispatch precedes this commit. Replays load immutable input;
    /// they do not resolve changed Workspace configuration or reacquire pins.
    pub(super) fn plugin_launch_binding(
        &self,
        runtime: &str,
        request: &WorkerSpawnRequest,
        reservation: &crate::store::WorkerCreateReservation,
    ) -> Result<WorkerCreateBinding> {
        use rusqlite::params;
        let prior = self.config_store.with_conn(|conn| {
            crate::plugin_plan::worker_launch_selection(
                conn,
                self.workspace_id(),
                runtime,
                &reservation.worker_id.to_string(),
            )
        })?;
        if let Some(prior) = prior {
            let snapshot = prior
                .map(|saved| {
                    if saved.pin_state == crate::plugin_plan::PlanPinState::Released {
                        return Err(Error::WorkspacePermissionDenied(
                            "Plugin launch released".into(),
                        ));
                    }
                    Ok(runtime_api::PluginExecutionSnapshot {
                        plan_id: saved.plan_id,
                        fingerprint: saved.fingerprint,
                        plan: saved.plan,
                    })
                })
                .transpose()?;
            let fingerprint = self.config_store.with_conn(|conn| Ok(conn.query_row("SELECT create_fingerprint FROM worker_create_reservations WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3",
                params![self.workspace_id(), runtime, reservation.worker_id.to_string()], |row| row.get::<_,String>(0))?))?;
            return Ok(WorkerCreateBinding {
                worker_id: reservation.worker_id,
                create_fingerprint: fingerprint,
                plugin_execution: snapshot,
            });
        }
        let worker = RuntimeWorkerRef::new(runtime, reservation.worker_id.to_string());
        if !self
            .store
            .has_active_worker_create_reservation(self.workspace_id(), &worker)?
        {
            return Ok(WorkerCreateBinding {
                worker_id: reservation.worker_id,
                create_fingerprint: reservation.create_fingerprint.clone(),
                plugin_execution: None,
            });
        }
        let selector_name = match &request.profile {
            worker_runtime::catalog::ProfileSelector::Builtin(s) if !s.starts_with("builtin:") => {
                format!("builtin:{s}")
            }
            worker_runtime::catalog::ProfileSelector::Builtin(s)
            | worker_runtime::catalog::ProfileSelector::Named(s) => s.clone(),
        };
        let selector = selector_name.as_str();
        let archive = match request.resolved_config_bundle.as_ref() {
            Some(bundle) if bundle.profile_source_archive.is_some() => bundle
                .profile_source_archive
                .clone()
                .expect("checked archive"),
            Some(bundle) if bundle.profile_source_archive_handle.is_some() => {
                let handle = bundle
                    .profile_source_archive_handle
                    .as_ref()
                    .expect("checked handle");
                self.resource_broker
                    .profile_source_archive(&handle.digest)
                    .ok_or_else(|| Error::Config("Plugin Profile archive unavailable".into()))?
            }
            _ => crate::profile_settings::builtin_profile_source_archive(&request.profile)
                .map_err(Error::Config)?,
        };
        let manifest = archive
            .verify()
            .map_err(|_| Error::Config("Plugin Profile archive invalid".into()))?
            .resolve_profile(selector, Path::new("/"), "plugin-launch")
            .map_err(|_| Error::Config("Plugin Profile unavailable".into()))?;
        if manifest.plugins.selections.is_empty() {
            return self.config_store.with_immediate_transaction(|tx| {
                tx.execute("INSERT INTO worker_plugin_launches(workspace_id,runtime_id,worker_id,plan_id) SELECT workspace_id,runtime_id,worker_id,NULL FROM worker_create_reservations WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3 AND create_fingerprint=?4 AND state='reserved' ON CONFLICT(workspace_id,worker_id) DO NOTHING",
                    params![self.workspace_id(), runtime, reservation.worker_id.to_string(), reservation.create_fingerprint])?;
                let selection = crate::plugin_plan::worker_launch_selection(tx, self.workspace_id(), runtime, &reservation.worker_id.to_string())?.ok_or_else(|| Error::WorkspaceConfigConflict("Plugin launch reservation changed".into()))?;
                let plugin_execution = selection.map(|saved| runtime_api::PluginExecutionSnapshot { plan_id: saved.plan_id, fingerprint: saved.fingerprint, plan: saved.plan });
                let fingerprint = tx.query_row("SELECT create_fingerprint FROM worker_create_reservations WHERE workspace_id=?1 AND worker_id=?2", params![self.workspace_id(), reservation.worker_id.to_string()], |row| row.get(0))?;
                Ok(WorkerCreateBinding { worker_id: reservation.worker_id, create_fingerprint: fingerprint, plugin_execution })
            });
        }
        let execution = self.plugin_reserved_execution_identity(
            runtime,
            reservation,
            request.resolved_execution_grant.as_ref(),
        )?;
        let key = launch_key(runtime, &reservation.worker_id.to_string());
        self.config_store.with_immediate_transaction(|tx| {
            if let Some(saved) = crate::plugin_plan::worker_launch_selection(tx, self.workspace_id(), runtime, &reservation.worker_id.to_string())? {
                let plugin_execution = saved.map(|saved| runtime_api::PluginExecutionSnapshot { plan_id: saved.plan_id, fingerprint: saved.fingerprint, plan: saved.plan });
                let fingerprint = tx.query_row("SELECT create_fingerprint FROM worker_create_reservations WHERE workspace_id=?1 AND worker_id=?2", params![self.workspace_id(), reservation.worker_id.to_string()], |row| row.get(0))?;
                return Ok(WorkerCreateBinding { worker_id: reservation.worker_id, create_fingerprint: fingerprint, plugin_execution });
            }
            let snapshot = {
                let state = crate::config_source::load_state(tx, self.workspace_id())?.ok_or_else(|| Error::Config("Plugin config unavailable".into()))?;
                let plan = crate::plugin_settings::resolve_worker_plugin_plan(tx, self.workspace_id(), &state.snapshot.digest, selector, execution)?;
                let requested: std::collections::BTreeMap<_, std::collections::BTreeSet<_>> = manifest.plugins.selections.iter().map(|p| (p.installation_id.as_str(), p.tools.iter().map(String::as_str).collect())).collect();
                let resolved: std::collections::BTreeMap<_, std::collections::BTreeSet<_>> = plan.plugins().iter().map(|p| (p.installation_id(), p.projection().tools.iter().map(String::as_str).collect())).collect();
                if requested != resolved {
                    return Err(Error::WorkspaceConfigConflict("Plugin Profile changed during launch".into()));
                }
                let saved = crate::plugin_plan::plugin_plan_create(tx, LAUNCH_ACTOR, &key, &plan)?;
                let fingerprint = config_source::digest_bytes(&serde_json::to_vec(&(&reservation.create_fingerprint, &saved.fingerprint)).map_err(|_| Error::Config("Plugin launch encoding failed".into()))?);
                let updated = tx.execute("UPDATE worker_create_reservations SET create_fingerprint=?1 WHERE workspace_id=?2 AND runtime_id=?3 AND worker_id=?4 AND create_fingerprint=?5 AND state='reserved'",
                    params![fingerprint, self.workspace_id(), runtime, reservation.worker_id.to_string(), reservation.create_fingerprint])?;
                if updated != 1 { return Err(Error::WorkspaceConfigConflict("Plugin launch reservation changed".into())); }
                tx.execute("INSERT INTO worker_plugin_launches(workspace_id,runtime_id,worker_id,plan_id) VALUES(?1,?2,?3,?4)", params![self.workspace_id(), runtime, reservation.worker_id.to_string(), saved.plan_id])?;
                runtime_api::PluginExecutionSnapshot { plan_id: saved.plan_id, fingerprint: saved.fingerprint, plan: saved.plan }
            };
            let fingerprint: String = tx.query_row("SELECT create_fingerprint FROM worker_create_reservations WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3 AND state='reserved'",
                params![self.workspace_id(), runtime, reservation.worker_id.to_string()], |row| row.get(0))?;
            Ok(WorkerCreateBinding { worker_id: reservation.worker_id, create_fingerprint: fingerprint, plugin_execution: Some(snapshot) })
        })
    }
}

#[derive(Default)]
pub(super) struct EmbeddedPluginRuntimeProviders {
    api: std::sync::Mutex<Option<std::sync::Weak<WorkspaceApi>>>,
}
impl EmbeddedPluginRuntimeProviders {
    pub(super) fn bind(&self, api: &Arc<WorkspaceApi>) {
        *self.api.lock().expect("Plugin Host binding lock") = Some(Arc::downgrade(api));
    }
}
#[async_trait::async_trait]
impl PluginRuntimeProviders for EmbeddedPluginRuntimeProviders {
    async fn connect(
        &self,
        workspace: &str,
        worker: &worker_runtime::identity::WorkerRef,
    ) -> std::result::Result<PluginRuntimeServices, String> {
        let api = self
            .api
            .lock()
            .map_err(|_| "Plugin Host binding unavailable")?
            .as_ref()
            .and_then(std::sync::Weak::upgrade)
            .ok_or("Plugin Host binding unavailable")?;
        if workspace != api.workspace_id() {
            return Err("Plugin Workspace mismatch".into());
        }
        let worker = RuntimeWorkerRef::new(EMBEDDED_RUNTIME_ID, worker.worker_id.to_string());
        let artifacts = api
            .plugin_artifact_source(worker.clone())
            .await
            .map_err(|_| "Plugin artifact source unavailable")?;
        let audience = embedded_runtime_request_audience(&api.config)
            .map_err(|_| "Plugin Host audience unavailable")?;
        let client = worker_runtime::workspace_request::RuntimeWorkspaceRequestClient::new(
            workspace,
            audience.clone(),
            EMBEDDED_RUNTIME_ID,
        )
        .with_runtime_request_source(&EMBEDDED_RUNTIME_REQUEST_IDENTITY, audience);
        #[cfg(test)]
        let transport = api.plugin_https_transport.clone();
        let owner = plugin_host::EmbeddedPluginHostOwner::authenticated(
            api,
            workspace.into(),
            client.plugin_host_authorizer(worker.worker_id),
        );
        #[cfg(test)]
        let owner = match transport {
            Some(transport) => owner.with_transport(transport),
            None => owner,
        };
        Ok(PluginRuntimeServices {
            artifacts: Arc::new(artifacts),
            owner: Arc::new(owner),
        })
    }
}
