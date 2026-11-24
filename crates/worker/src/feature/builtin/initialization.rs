//! Memory owns snapshot binding; restore compares the saved snapshot instead of
//! rebinding current configuration. No storage handle belongs in the envelope.

use crate::feature::FeatureDescriptor;
use crate::feature::initialization::{
    FeatureInitializationAdapter, InitializationContext, InitializationPhase,
};
use crate::{Worker, WorkerError};
use agen::llm_client::client::LlmClient;
use protocol::feature_initialization::FeatureInitializations;
use session_store::{Store, WorkerMetadata, WorkerMetadataStore};

pub(crate) struct MemoryInitialization;

impl MemoryInitialization {
    fn validate_snapshot(
        context: &InitializationContext<'_>,
        restoring: bool,
    ) -> Result<(), String> {
        if context.data.get("builtin:memory").is_some() {
            return Err(
                "Memory has no envelope schema; settings must use the trusted snapshot".into(),
            );
        }
        let Some(settings) = context.memory_settings else {
            if context.workspace_id.is_some() {
                return Err(if restoring {
                    "Workspace Worker restore is missing its trusted settings snapshot"
                } else {
                    "Workspace Worker request is missing its bound Memory settings snapshot"
                }
                .into());
            }
            return Ok(());
        };
        if context
            .workspace_id
            .is_some_and(|id| id != settings.workspace_id)
        {
            return Err("trusted Memory settings do not match Workspace API scope".into());
        }
        Ok(())
    }
}

impl<C, St> FeatureInitializationAdapter<C, St> for MemoryInitialization
where
    C: LlmClient + Clone + 'static,
    St: Store + WorkerMetadataStore + Clone + 'static,
{
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builtin("memory", "Memory initialization")
    }
    fn validate_transport(&self, context: &InitializationContext<'_>) -> Result<(), String> {
        if let Some(binding) = context.backend_job {
            binding.validate().map_err(str::to_owned)?;
            if context.workspace_id.is_none() {
                return Err("Backend Job capability requires Workspace authority".into());
            }
        }
        Ok(())
    }
    fn bind_manifest(
        &self,
        manifest: &mut manifest::WorkerManifest,
        context: &InitializationContext<'_>,
    ) -> Result<(), String> {
        Self::validate_snapshot(context, false)?;
        if let Some(settings) = context.memory_settings
            && manifest.feature.memory.profile.enabled
        {
            manifest
                .feature
                .memory
                .bind_workspace_settings(settings.clone())
                .map_err(str::to_owned)?;
        }
        Ok(())
    }
    fn validate_manifest(
        &self,
        manifest: &manifest::WorkerManifest,
        context: &InitializationContext<'_>,
    ) -> Result<(), String> {
        Self::validate_snapshot(context, true)?;
        manifest
            .feature
            .memory
            .validate_execution()
            .map_err(str::to_owned)?;
        if let Some(expected) = context.memory_settings
            && manifest.feature.memory.profile.enabled
            && manifest.feature.memory.workspace_settings().as_ref() != Some(expected)
        {
            return Err("Workspace Worker Memory settings snapshot mismatch".into());
        }
        Ok(())
    }
    fn prepare_host(
        &self,
        _: &Worker<C, St>,
        _: &FeatureInitializations,
    ) -> Result<(), WorkerError> {
        Ok(())
    }
    fn apply(
        &self,
        _: &Worker<C, St>,
        _: &FeatureInitializations,
        _: InitializationPhase,
    ) -> Result<(), WorkerError> {
        Ok(())
    }
    fn requires_source_repair(
        &self,
        _: &WorkerMetadata,
        _: &FeatureInitializations,
    ) -> Result<bool, String> {
        Ok(false)
    }
}
