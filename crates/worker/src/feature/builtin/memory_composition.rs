//! Workspace Memory composes its own hooks and prompt source without requiring
//! Subjektiv knowledge in Controller. Protected reads remain in live providers.

use crate::{Worker, feature::FeatureRegistryBuilder};
use agen::llm_client::client::LlmClient;
use session_store::{Store, WorkerMetadataStore};

pub(crate) fn compose<C, St>(
    worker: &mut Worker<C, St>,
    feature_registry: &mut FeatureRegistryBuilder,
) -> std::io::Result<Option<crate::feature::PromptContribution>>
where
    C: LlmClient + Clone + 'static,
    St: Store + WorkerMetadataStore + Clone + 'static,
{
    let backend_job = worker.backend_job().cloned();
    let lifecycle_enabled = worker.manifest_lifecycle_features_enabled() && backend_job.is_none();
    let spawner_manifest = worker.manifest().clone();
    let spawner_workspace_context = worker.workspace_context_handle();
    let prompts = worker.prompts().clone();
    let memory_install_plan = crate::feature::builtin::memory::MemoryFeatureInstallPlan::prepare(
        worker.manifest(),
        worker.workspace_client_handle(),
        worker.prompts().load_full(),
    )?;
    let feature_prompt_contribution = memory_install_plan.as_ref().map(|plan| {
        (
            plan.resident_summary_source.clone(),
            if backend_job.is_none() {
                plan.system_prompt_override.clone()
            } else {
                None
            },
        )
    });
    let memory_lifecycle_config = worker.manifest().feature.memory.clone();
    if let Some(plan) = memory_install_plan {
        feature_registry.add_module(plan.module);
    }
    if let Some(memory_lifecycle) =
        crate::feature::builtin::memory_lifecycle::MemoryLifecycleFeature::from_resolved_config(
            lifecycle_enabled,
            memory_lifecycle_config,
            worker.committed_session_capture_handle(),
            worker.session_extension_handle(),
            worker.workspace_client_handle(),
            spawner_manifest.clone(),
            worker.llm_client_handle(),
            prompts.clone(),
            spawner_workspace_context.clone(),
            worker.working_event_sender(),
        )?
    {
        feature_registry.add_module(memory_lifecycle);
    }
    Ok(feature_prompt_contribution)
}
