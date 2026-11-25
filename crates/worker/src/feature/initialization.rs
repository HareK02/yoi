//! Feature-owned interpretation of durable launch data. The registry only routes
//! bounded envelopes and uses the normal descriptor planner; adapters own policy,
//! current Host checks and effects. Reconstructing a saved Manifest is read-only.

use agen::llm_client::client::LlmClient;
use protocol::feature_initialization::FeatureInitializations;
use session_store::{Store, WorkerMetadata, WorkerMetadataStore};

use super::{FeatureDescriptor, FeatureInstallPlan, build_feature_install_plan};
use crate::{BackendJobExecutionBinding, Worker, WorkerError};

pub struct InitializationContext<'a> {
    pub data: &'a FeatureInitializations,
    pub workspace_id: Option<&'a str>,
    pub memory_settings: Option<&'a manifest::SubjektivMemorySettingsSnapshot>,
    pub backend_job: Option<&'a BackendJobExecutionBinding>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitializationPhase {
    New,
    Restored,
    Recovery,
}

pub trait FeatureInitializationAdapter<C, St>: Send + Sync
where
    C: LlmClient + Clone + 'static,
    St: Store + WorkerMetadataStore + Clone + 'static,
{
    fn descriptor(&self) -> FeatureDescriptor;
    fn validate_transport(&self, context: &InitializationContext<'_>) -> Result<(), String>;
    fn bind_manifest(
        &self,
        manifest: &mut manifest::WorkerManifest,
        context: &InitializationContext<'_>,
    ) -> Result<(), String>;
    fn validate_manifest(
        &self,
        manifest: &manifest::WorkerManifest,
        context: &InitializationContext<'_>,
    ) -> Result<(), String>;
    /// No canonical writes or external creation. Current operation checks may read
    /// the Host; failures must not be interpreted as an unattached Worker.
    fn validate_saved_state(&self, _worker: &Worker<C, St>) -> Result<(), WorkerError> {
        Ok(())
    }
    fn prepare_host(
        &self,
        worker: &Worker<C, St>,
        data: &FeatureInitializations,
    ) -> Result<(), WorkerError>;
    fn prepare_for_effects(
        &self,
        worker: &Worker<C, St>,
        data: &FeatureInitializations,
        _phase: InitializationPhase,
    ) -> Result<(), WorkerError> {
        self.prepare_host(worker, data)
    }
    fn apply(
        &self,
        worker: &Worker<C, St>,
        data: &FeatureInitializations,
        phase: InitializationPhase,
    ) -> Result<(), WorkerError>;
    fn requires_source_repair(
        &self,
        metadata: &WorkerMetadata,
        data: &FeatureInitializations,
    ) -> Result<bool, String>;
}

pub struct FeatureInitializationRegistry<C, St>
where
    C: LlmClient + Clone + 'static,
    St: Store + WorkerMetadataStore + Clone + 'static,
{
    adapters: Vec<Box<dyn FeatureInitializationAdapter<C, St>>>,
}

impl<C, St> FeatureInitializationRegistry<C, St>
where
    C: LlmClient + Clone + 'static,
    St: Store + WorkerMetadataStore + Clone + 'static,
{
    pub fn new(adapters: Vec<Box<dyn FeatureInitializationAdapter<C, St>>>) -> Self {
        Self { adapters }
    }

    pub fn with_adapter(
        mut self,
        adapter: impl FeatureInitializationAdapter<C, St> + 'static,
    ) -> Self {
        self.adapters.push(Box::new(adapter));
        self
    }

    pub fn plan(&self, data: &FeatureInitializations) -> Result<FeatureInstallPlan, String> {
        let descriptors = self
            .adapters
            .iter()
            .map(|a| a.descriptor())
            .collect::<Vec<_>>();
        let plan = build_feature_install_plan(&descriptors).map_err(|e| e.to_string())?;
        for entry in data.iter() {
            if !descriptors
                .iter()
                .any(|d| d.id.as_str() == entry.feature_id())
            {
                return Err(format!(
                    "no selected initialization provider for {}",
                    entry.feature_id()
                ));
            }
        }
        Ok(plan)
    }

    fn ordered(
        &self,
        data: &FeatureInitializations,
    ) -> Result<Vec<&dyn FeatureInitializationAdapter<C, St>>, String> {
        let plan = self.plan(data)?;
        Ok(plan
            .ordered_features()
            .iter()
            .map(|id| {
                self.adapters
                    .iter()
                    .find(|a| a.descriptor().id == *id)
                    .expect("planned adapter")
                    .as_ref()
            })
            .collect())
    }

    pub fn validate_transport(&self, context: &InitializationContext<'_>) -> Result<(), String> {
        for adapter in self.ordered(context.data)? {
            adapter.validate_transport(context)?;
        }
        Ok(())
    }

    pub fn bind_manifest(
        &self,
        manifest: &mut manifest::WorkerManifest,
        context: &InitializationContext<'_>,
    ) -> Result<(), String> {
        self.validate_transport(context)?;
        for adapter in self.ordered(context.data)? {
            adapter.bind_manifest(manifest, context)?;
        }
        self.validate_manifest(manifest, context)
    }

    pub fn validate_manifest(
        &self,
        manifest: &manifest::WorkerManifest,
        context: &InitializationContext<'_>,
    ) -> Result<(), String> {
        self.validate_transport(context)?;
        for adapter in self.ordered(context.data)? {
            adapter.validate_manifest(manifest, context)?;
        }
        Ok(())
    }

    pub fn validate_saved_state(&self, worker: &Worker<C, St>) -> Result<(), WorkerError> {
        for adapter in &self.adapters {
            adapter.validate_saved_state(worker)?;
        }
        Ok(())
    }

    pub fn prepare_host(
        &self,
        worker: &Worker<C, St>,
        data: &FeatureInitializations,
    ) -> Result<(), WorkerError> {
        for adapter in self.ordered(data).map_err(WorkerError::FeatureLifecycle)? {
            adapter.prepare_host(worker, data)?;
        }
        Ok(())
    }

    pub fn apply(
        &self,
        worker: &Worker<C, St>,
        data: &FeatureInitializations,
        phase: InitializationPhase,
    ) -> Result<(), WorkerError> {
        for adapter in self.ordered(data).map_err(WorkerError::FeatureLifecycle)? {
            adapter.prepare_for_effects(worker, data, phase)?;
        }
        for adapter in self.ordered(data).map_err(WorkerError::FeatureLifecycle)? {
            adapter.apply(worker, data, phase)?;
        }
        Ok(())
    }

    pub fn requires_source_repair(
        &self,
        metadata: &WorkerMetadata,
        data: &FeatureInitializations,
    ) -> Result<bool, String> {
        let mut required = false;
        for adapter in self.ordered(data)? {
            required |= adapter.requires_source_repair(metadata, data)?;
        }
        Ok(required)
    }
}

/// Trusted composition, not a Feature-id dispatcher. Alternative Hosts/tests can
/// supply their own adapters to the same registry and saved transport.
pub fn builtin_registry<C, St>() -> FeatureInitializationRegistry<C, St>
where
    C: LlmClient + Clone + 'static,
    St: Store + WorkerMetadataStore + Clone + 'static,
{
    FeatureInitializationRegistry::new(vec![Box::new(
        super::builtin::subjektiv_initialization::SubjektivInitialization,
    )])
}
