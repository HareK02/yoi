//! Process-owned bridge for exact Plugin launch inputs. Implementations compose
//! the existing delivery/cache/executor and live owner protocols; they may not
//! reselect a Profile, search an artifact inventory, or mint execution authority.
use crate::identity::WorkerRef;
use async_trait::async_trait;

#[async_trait]
pub trait PluginToolRuntime: Send + Sync {
    /// Prepare unpublished contributions for this exact persisted plan. Both
    /// create and restore call this boundary before Feature/controller publication.
    /// Success requires current owner startup admission, not only verified bytes.
    async fn prepare(
        &self,
        worker: &WorkerRef,
        snapshot: &runtime_api::PluginExecutionSnapshot,
        session_id: &str,
    ) -> Result<worker::plugin::PluginToolsFeature, String>;
}

/// Materialize only the exact Server-authored pins. This prepares contributions,
/// never publishes them or claims startup admission: the Host bridge must perform
/// its live owner preflight before returning from `PluginToolRuntime::prepare`.
/// Metadata from delivery becomes an expectation only after full immutable-pin
/// comparison; byte equality never issues an execution grant.
pub async fn materialize(
    worker: &WorkerRef,
    snapshot: &runtime_api::PluginExecutionSnapshot,
    session_id: &str,
    source: &dyn yoi_plugin_runtime::delivery::ArtifactSource,
    owner: std::sync::Arc<dyn yoi_plugin_runtime::owner::PluginHostOwner>,
    cache: &yoi_plugin_runtime::delivery::ArtifactCache,
    limits: yoi_plugin_runtime::Limits,
) -> Result<worker::plugin::PluginToolsFeature, String> {
    use yoi_plugin_runtime::{delivery::ArtifactExpectation, host::Grants};
    let plan = &snapshot.plan;
    if plan.validate_canonical().is_err()
        || plan.fingerprint().ok().as_ref() != Some(&snapshot.fingerprint)
        || plan.execution().worker_id != worker.worker_id.to_string()
        || snapshot.plan_id.is_empty()
    {
        return Err("Plugin launch snapshot mismatch".into());
    }
    let mut pins = Vec::new();
    for pin in plan.plugins() {
        let request = server_api::PluginArtifactDeliveryRequest {
            plan_id: snapshot.plan_id.clone(),
            plan_fingerprint: snapshot.fingerprint.clone(),
            installation_id: pin.installation_id().into(),
            artifact_digest: pin.artifact_digest().into(),
        };
        let metadata = source
            .authorize(&request)
            .await
            .map_err(|_| "Plugin artifact admission unavailable")?;
        if metadata.workspace_id != plan.workspace_id()
            || metadata.request != request
            || !pin
                .matches_artifact(&metadata.artifact)
                .map_err(|_| "Plugin artifact metadata mismatch")?
        {
            return Err("Plugin artifact metadata mismatch".into());
        }
        let mut grants = Grants::default();
        for grant in &pin.projection().grants {
            match grant {
                plugin_plan::PluginGrant::Logging => grants.logging = true,
                plugin_plan::PluginGrant::Https { slot } => {
                    grants.connection_slots.insert(slot.clone());
                }
            }
        }
        pins.push(worker::plugin::PluginToolPin {
            workspace_id: plan.workspace_id().into(),
            installation_id: pin.installation_id().into(),
            plan_id: snapshot.plan_id.clone(),
            subject: plan.execution().principal_id.clone(),
            request,
            expectation: ArtifactExpectation::new(metadata.artifact),
            tools: pin.projection().tools.clone(),
            config: pin.projection().config.clone(),
            grants,
        });
    }
    worker::plugin::PluginToolsFeature::prepare(pins, source, cache, owner, session_id, limits)
        .await
        .map_err(|error| error.to_string())
}
