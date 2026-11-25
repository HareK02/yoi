//! Subjektiv's trusted composition root supplies its Host to owned consumers.
//! Controller sees only staged modules and neutral prompt contributions.

use crate::{Worker, feature::FeatureRegistryBuilder};
use agen::llm_client::client::LlmClient;
use session_store::{Store, WorkerMetadataStore};
use std::sync::Arc;

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
    let subjektiv_host = worker.subjektiv_host_connection()?;
    let mut feature_prompt_contribution = None;
    let subjektiv_consolidation_plan =
        crate::feature::builtin::memory::SubjektivConsolidationFeatureInstallPlan::prepare(
            worker.manifest(),
            subjektiv_host.clone(),
            worker.prompts().load_full(),
        )?;
    if let Some(plan) = subjektiv_consolidation_plan {
        if backend_job.is_none() {
            feature_prompt_contribution = Some((None, Some(plan.system_prompt_override)));
        }
        feature_registry.add_module(plan.module);
    }
    let ordinary_subjektiv_features_enabled = subjektiv_host.is_some()
        && crate::feature::builtin::memory::ordinary_subjektiv_features_enabled(worker.manifest());
    if let Some(resident_summary_source) =
        crate::feature::builtin::memory::ordinary_subjektiv_resident_summary_source(
            worker.manifest(),
            subjektiv_host.clone(),
            worker.prompts(),
        )?
    {
        let workspace_id = subjektiv_host
            .as_ref()
            .and_then(|host| host.host.workspace_id());
        if let Some(refresh_feature) =
            crate::feature::builtin::memory::SubjektivResidentRestoreRefreshFeature::for_connection(
                lifecycle_enabled,
                Arc::clone(&resident_summary_source),
                worker.prompts(),
                workspace_id,
            )
        {
            feature_registry.add_module(refresh_feature);
        }
        feature_prompt_contribution = Some((Some(resident_summary_source), None));
    }
    if backend_job.is_none()
        && let Some(chat) =
            crate::feature::builtin::subjektiv_chat::SubjektivChatFeature::from_resolved_config(
                &worker.manifest().feature.subjektiv,
                subjektiv_host.clone(),
            )?
    {
        feature_registry.add_module(chat);
    }
    if ordinary_subjektiv_features_enabled
        && let Some(subjektiv_memory) =
            crate::feature::builtin::subjektiv_memory::SubjektivMemoryFeature::from_resolved_config(
                &worker.manifest().feature.subjektiv,
                worker.committed_session_capture_handle(),
                subjektiv_host.clone(),
            )?
    {
        feature_registry.add_module(subjektiv_memory);
    }
    if ordinary_subjektiv_features_enabled
        && let Some(subjektiv_sessions) =
            crate::feature::builtin::subjektiv_session::SubjektivSessionFeature::from_resolved_config(
                &worker.manifest().feature.subjektiv,
                subjektiv_host.clone(),
            )?
    {
        feature_registry.add_module(subjektiv_sessions);
    }
    if ordinary_subjektiv_features_enabled
        && lifecycle_enabled
        && let Some(host) = subjektiv_host.as_ref()
    {
        worker.install_session_context_source(Arc::new(
            crate::feature::builtin::journal::JournalContextSource::new(
                host.clone(),
                worker.prompts(),
            ),
        ));
    }
    if ordinary_subjektiv_features_enabled
        && lifecycle_enabled
        && let Some(host) = subjektiv_host.as_ref()
        && host.host.journal_recording_granted()
    {
        feature_registry.add_module(crate::subjektiv::JournalRecordingFeature::new(
            host.clone(),
        )?);
    }
    if backend_job.is_none() && let Some(surface_lifecycle) = crate::feature::builtin::memory_surface_lifecycle::SubjektivSurfaceLifecycleFeature::from_manifest(
        lifecycle_enabled,
        subjektiv_host.clone(),
        spawner_manifest.clone(),
        worker.llm_client_handle(),
        prompts.clone(),
        spawner_workspace_context.clone(),
    )? {
        feature_registry.add_module(surface_lifecycle);
    }
    if ordinary_subjektiv_features_enabled
        && let Some(subjektiv_lifecycle) =
            crate::feature::builtin::memory_lifecycle::SubjektivLifecycleFeature::from_resolved_config(
                lifecycle_enabled,
                worker.manifest().feature.subjektiv.clone(),
                worker.committed_session_capture_handle(),
                worker.session_extension_handle(),
                subjektiv_host.clone(),
                spawner_manifest.clone(),
                worker.llm_client_handle(),
                prompts.clone(),
                spawner_workspace_context.clone(),
                worker.working_event_sender(),
            )?
    {
        feature_registry.add_module(subjektiv_lifecycle);
    }
    Ok(feature_prompt_contribution)
}

pub(crate) fn compose_jobs<C, St>(worker: &Worker<C, St>) -> std::io::Result<FeatureRegistryBuilder>
where
    C: LlmClient + Clone + 'static,
    St: Store + WorkerMetadataStore + Clone + 'static,
{
    let backend_job = worker.backend_job().cloned();
    let spawner_manifest = worker.manifest().clone();
    let spawner_workspace_context = worker.workspace_context_handle();
    let prompts = worker.prompts().clone();
    let subjektiv_host = worker.subjektiv_host_connection()?;
    let journal_grant = backend_job
        .as_ref()
        .filter(|binding| binding.subjektiv_journal)
        .map(|binding| {
            crate::subjektiv::BackendJournalJobHost::new(worker.workspace_client_handle(), binding)
                .map(|host| host.into_grant())
        })
        .transpose()?;
    let (mut feature_registry, journal_finalizer) = if let Some(grant) = journal_grant {
        (grant.features, grant.finalizer)
    } else {
        (FeatureRegistryBuilder::new(), None)
    };
    if let Some(binding) = backend_job.as_ref() {
        let workspace_client = worker.workspace_client_handle();
        if !workspace_client.is_available() || workspace_client.workspace_id().is_none() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Backend Job result capability requires Backend Workspace API authority",
            ));
        }
        if binding.subjektiv_consolidation || binding.subjektiv_journal {
            feature_registry.add_module(
                crate::feature::builtin::backend_job_input::SubjectJobInputFeature::new(
                    workspace_client.clone(),
                    if binding.subjektiv_consolidation {
                        "memory.consolidation.supply"
                    } else {
                        "journal.job.input.supply"
                    },
                ),
            );
        }
        let mut result_feature =
            crate::feature::builtin::backend_job_result::BackendJobResultFeature::new(
                workspace_client.clone(),
            );
        if binding.subjektiv_consolidation {
            let surface = crate::feature::builtin::memory_surface_lifecycle::SubjektivSurfaceLifecycleFeature::for_host(
                subjektiv_host.clone().ok_or_else(|| std::io::Error::other("Backend Job requires subjektiv Host capability"))?, spawner_manifest.clone(), worker.llm_client_handle(),
                prompts.clone(), spawner_workspace_context.clone(),
            )?;
            result_feature = result_feature.with_surface(surface);
        }
        if let Some(finalizer) = journal_finalizer {
            result_feature = result_feature.with_finalizer(
                finalizer,
                spawner_manifest.clone(),
                worker.llm_client_handle(),
            )?;
        }
        feature_registry.add_module(result_feature);
    }
    Ok(feature_registry)
}
