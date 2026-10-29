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

pub struct PluginRuntimeServices {
    pub artifacts: std::sync::Arc<dyn yoi_plugin_runtime::delivery::ArtifactSource>,
    pub owner: std::sync::Arc<dyn yoi_plugin_runtime::owner::PluginHostOwner>,
}

/// Supplies authenticated process-owned adapters, not a Plugin resolver. Handles
/// are bound to the exact requested Worker; a connection failure has no fallback.
#[async_trait]
pub trait PluginRuntimeProviders: Send + Sync {
    async fn connect(
        &self,
        workspace: &str,
        worker: &WorkerRef,
    ) -> Result<PluginRuntimeServices, String>;
}

pub struct PinnedPluginToolRuntime {
    providers: std::sync::Arc<dyn PluginRuntimeProviders>,
    cache: yoi_plugin_runtime::delivery::ArtifactCache,
    limits: yoi_plugin_runtime::Limits,
}
impl PinnedPluginToolRuntime {
    pub fn new(
        providers: std::sync::Arc<dyn PluginRuntimeProviders>,
        private_cache_root: &std::path::Path,
    ) -> Result<Self, String> {
        Ok(Self {
            providers,
            cache: yoi_plugin_runtime::delivery::ArtifactCache::new(
                private_cache_root,
                yoi_plugin_runtime::delivery::CacheLimits::default(),
            )
            .map_err(|_| "Plugin cache initialization failed")?,
            limits: yoi_plugin_runtime::Limits::default(),
        })
    }
}
#[async_trait]
impl PluginToolRuntime for PinnedPluginToolRuntime {
    async fn prepare(
        &self,
        worker: &WorkerRef,
        snapshot: &runtime_api::PluginExecutionSnapshot,
        session_id: &str,
    ) -> Result<worker::plugin::PluginToolsFeature, String> {
        snapshot
            .plan
            .validate_canonical()
            .map_err(|_| "invalid Plugin plan")?;
        if snapshot
            .plan
            .fingerprint()
            .map_err(|_| "invalid Plugin plan")?
            != snapshot.fingerprint
            || snapshot.plan.execution().worker_id != worker.worker_id.to_string()
        {
            return Err("Plugin launch identity mismatch".into());
        }
        let services = self
            .providers
            .connect(snapshot.plan.workspace_id(), worker)
            .await?;
        for pin in snapshot.plan.plugins() {
            services
                .owner
                .prepare(
                    snapshot.plan_id.clone(),
                    pin.installation_id().into(),
                    pin.projection().tools.clone(),
                )
                .await
                .map_err(|_| "Plugin startup admission refused or unavailable")?;
        }
        let tools = materialize(
            worker,
            snapshot,
            session_id,
            services.artifacts.as_ref(),
            services.owner.clone(),
            &self.cache,
            self.limits.clone(),
        )
        .await?;
        // Materialization and compilation can wait on I/O. Recheck at publication
        // preparation, including pure Tools and cache hits, without making a call.
        for pin in snapshot.plan.plugins() {
            services
                .owner
                .prepare(
                    snapshot.plan_id.clone(),
                    pin.installation_id().into(),
                    pin.projection().tools.clone(),
                )
                .await
                .map_err(|_| "Plugin startup publication refused or unavailable")?;
        }
        Ok(tools)
    }
}

#[cfg(feature = "http-server")]
pub mod remote {
    use super::*;
    use crate::{
        workspace_issuer::WorkspaceIssuerTrustRecord,
        workspace_request::{
            RuntimeWorkspaceRequestClient,
            plugin_delivery::{CurrentArtifactServer, RemotePluginArtifactSource},
        },
    };
    use std::{collections::HashMap, sync::Arc};
    pub type CurrentPluginServer = dyn Fn(&str) -> Result<WorkspaceIssuerTrustRecord, yoi_plugin_runtime::delivery::DeliveryError>
        + Send
        + Sync;
    pub struct RemotePluginRuntimeProviders {
        clients: HashMap<String, RuntimeWorkspaceRequestClient>,
        current: Arc<CurrentPluginServer>,
    }
    impl RemotePluginRuntimeProviders {
        pub fn new(
            clients: HashMap<String, RuntimeWorkspaceRequestClient>,
            current: Arc<CurrentPluginServer>,
        ) -> Self {
            Self { clients, current }
        }
    }
    struct RegisteredOwner {
        source: Arc<RemotePluginArtifactSource>,
        owner: Arc<dyn yoi_plugin_runtime::owner::PluginHostOwner>,
    }
    #[async_trait]
    impl yoi_plugin_runtime::owner::PluginHostOwner for RegisteredOwner {
        async fn call(
            &self,
            request: server_api::PluginHostRequest,
        ) -> Result<server_api::PluginHostResponse, server_api::PluginHostError> {
            let denied = || server_api::PluginHostError {
                code: server_api::PluginHostErrorCode::Denied,
                effect: server_api::PluginEffectObservation::NotDispatched,
                diagnostic_id: String::new(),
            };
            self.source.registration().map_err(|_| denied())?;
            let dispatched = matches!(request, server_api::PluginHostRequest::Send { .. });
            let response = self.owner.call(request).await?;
            self.source
                .registration()
                .map_err(|_| server_api::PluginHostError {
                    code: server_api::PluginHostErrorCode::Denied,
                    effect: if dispatched {
                        server_api::PluginEffectObservation::Indeterminate
                    } else {
                        server_api::PluginEffectObservation::NotDispatched
                    },
                    diagnostic_id: String::new(),
                })?;
            Ok(response)
        }
    }
    #[async_trait]
    impl PluginRuntimeProviders for RemotePluginRuntimeProviders {
        async fn connect(
            &self,
            workspace: &str,
            worker: &WorkerRef,
        ) -> Result<PluginRuntimeServices, String> {
            let client = self
                .clients
                .get(workspace)
                .ok_or("Plugin Server is not registered")?
                .clone();
            let current = self.current.clone();
            let workspace = workspace.to_owned();
            let registered: Arc<CurrentArtifactServer> = Arc::new(move || current(&workspace));
            let source = Arc::new(
                RemotePluginArtifactSource::new(
                    client.clone(),
                    worker.worker_id.to_string(),
                    registered,
                )
                .map_err(|_| "Plugin Server trust unavailable")?,
            );
            let generated = client
                .plugin_host_client(worker.worker_id.to_string())
                .map_err(|_| "Plugin Host client unavailable")?;
            let owner = Arc::new(RegisteredOwner {
                source: source.clone(),
                owner: Arc::new(yoi_plugin_runtime::owner::RemotePluginHostOwner::new(
                    Arc::new(generated),
                    client.workspace_id().into(),
                )),
            });
            Ok(PluginRuntimeServices {
                artifacts: source,
                owner,
            })
        }
    }
}
