use std::sync::Arc;
use std::time::Duration;

use agen::llm_client::LlmClient;
use arc_swap::ArcSwap;
use async_trait::async_trait;

use crate::PromptCatalog;
use crate::Scope;
use crate::feature::background::{
    BackgroundTaskCancellation, BackgroundTaskContext, BackgroundTaskSpec, BackgroundTaskTrigger,
    FeatureBackgroundTask,
};
use crate::feature::builtin::memory::WorkspaceMemoryBackendError;
use crate::feature::builtin::memory_surface_output::{
    MemorySurfaceOutputFeature, MemorySurfaceOutputState,
};
use crate::feature::{
    BackgroundTaskDeclaration, FeatureDescriptor, FeatureInstallContext, FeatureInstallError,
    FeatureModule, FeatureRegistryBuilder,
};
use crate::hook::{HookError, HookErrorCategory};
use crate::internal_worker::{
    InternalWorkerAuthority, InternalWorkerIdentity, InternalWorkerSpec,
    run_internal_worker_with_cancel_sender,
};
use crate::worker::{WorkerFilesystemAuthority, WorkerWorkspaceContext, WorkspaceClient};
use manifest::WorkerManifest;

const TASK_NAME: &str = "subjektiv-memory-surface";
const TASK_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_GENERATION_ATTEMPTS: usize = 2;
const EDITOR_MAX_TURNS: u32 = 3;

#[derive(Clone)]
pub(crate) struct SubjektivSurfaceLifecycleFeature {
    task: SubjektivSurfaceLifecycleTask,
}

#[derive(Clone)]
struct SubjektivSurfaceLifecycleTask {
    workspace_client: Arc<dyn WorkspaceClient>,
    manifest: WorkerManifest,
    client: Box<dyn LlmClient>,
    prompts: Arc<ArcSwap<PromptCatalog>>,
    workspace_context: WorkerWorkspaceContext,
}

impl SubjektivSurfaceLifecycleFeature {
    pub(crate) fn from_manifest(
        lifecycle_enabled: bool,
        workspace_client: Arc<dyn WorkspaceClient>,
        manifest: WorkerManifest,
        client: Box<dyn LlmClient>,
        prompts: Arc<ArcSwap<PromptCatalog>>,
        workspace_context: WorkerWorkspaceContext,
    ) -> std::io::Result<Option<Self>> {
        let dedicated = manifest.profile.as_ref().is_some_and(|snapshot| {
            matches!(
                &snapshot.source,
                manifest::ProfileSource::Registry {
                    source: manifest::ProfileRegistrySource::Builtin,
                    name,
                    ..
                } if name == "subjektiv-memory-consolidation"
            )
        });
        if !lifecycle_enabled || !dedicated {
            return Ok(None);
        }
        if !workspace_client.is_available() || workspace_client.workspace_id().is_none() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "subjektiv surface generation requires Backend Workspace API authority",
            ));
        }
        Ok(Some(Self {
            task: SubjektivSurfaceLifecycleTask {
                workspace_client,
                manifest,
                client,
                prompts,
                workspace_context,
            },
        }))
    }
}

impl FeatureModule for SubjektivSurfaceLifecycleFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builtin("subjektiv-memory-surface-lifecycle", "subjektiv Memory Surface Lifecycle")
            .with_description("Rebuilds one bounded subject Memory surface after each committed consolidation run.")
            .with_background_task(BackgroundTaskDeclaration::worker_managed(
                TASK_NAME,
                "Generate and publish a current subject Memory surface from bounded confirmed revisions.",
            ))
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        let declaration = BackgroundTaskDeclaration::worker_managed(
            TASK_NAME,
            "Generate and publish a current subject Memory surface from bounded confirmed revisions.",
        );
        let mut spec = BackgroundTaskSpec::single_flight(declaration, TASK_TIMEOUT);
        spec.trigger = BackgroundTaskTrigger::RunCommitted;
        context.background_tasks().register(spec, self.task.clone())
    }
}

#[async_trait]
impl FeatureBackgroundTask for SubjektivSurfaceLifecycleTask {
    async fn run(
        &self,
        context: BackgroundTaskContext,
        cancellation: BackgroundTaskCancellation,
    ) -> Result<(), HookError> {
        for attempt in 0..MAX_GENERATION_ATTEMPTS {
            context.generation_fence.ensure_current()?;
            if cancellation.is_cancelled() {
                return Ok(());
            }
            let generation = self
                .workspace_client
                .prepare_subjektiv_memory_surface()
                .await
                .map_err(surface_hook_error)?;
            if generation.materials.is_empty() && generation.active_memory_count > 0 {
                self.record_failure(&generation.generation_id, "input_budget_exhausted")
                    .await;
                tracing::warn!(
                    active_memory_count = generation.active_memory_count,
                    "subject Memory surface materials could not fit the bounded input"
                );
                return Ok(());
            }
            let points = if generation.materials.is_empty() {
                Vec::new()
            } else {
                match self.edit_surface(&generation, cancellation.clone()).await {
                    Ok(points) => points,
                    Err(error) => {
                        self.record_failure(&generation.generation_id, "editor_failed")
                            .await;
                        tracing::warn!(%error, "subject Memory surface editor failed");
                        return Ok(());
                    }
                }
            };
            context.generation_fence.ensure_current()?;
            if cancellation.is_cancelled() {
                return Ok(());
            }
            let publish = self
                .workspace_client
                .publish_subjektiv_memory_surface(server_api::SubjektivSurfacePublishRequest {
                    generation_id: generation.generation_id.clone(),
                    points,
                })
                .await;
            match publish {
                Ok(output) => {
                    tracing::debug!(
                        snapshot_id = output.snapshot_id,
                        store_revision = output.built_from_store_revision,
                        empty = output.empty,
                        "published subject Memory surface"
                    );
                    return Ok(());
                }
                Err(WorkspaceMemoryBackendError::Http { status, .. })
                    if status == reqwest::StatusCode::CONFLICT
                        && attempt + 1 < MAX_GENERATION_ATTEMPTS =>
                {
                    // Confirmed Memory moved while editing. Discard the stale
                    // output and rebuild once from a fresh bounded generation.
                    continue;
                }
                Err(error) => {
                    self.record_failure(&generation.generation_id, "publish_failed")
                        .await;
                    tracing::warn!(%error, "subject Memory surface publication failed");
                    return Ok(());
                }
            }
        }
        Ok(())
    }
}

impl SubjektivSurfaceLifecycleTask {
    async fn edit_surface(
        &self,
        generation: &server_api::SubjektivSurfacePrepareResponse,
        cancellation: BackgroundTaskCancellation,
    ) -> Result<Vec<server_api::SubjektivSurfacePoint>, HookError> {
        let language = self
            .manifest
            .feature
            .memory
            .workspace_settings()
            .map(|settings| settings.language)
            .ok_or_else(|| {
                HookError::new(
                    HookErrorCategory::Internal,
                    "surface editor requires bound Workspace Memory settings",
                )
            })?;
        let system_prompt = self
            .prompts
            .load_full()
            .subjektiv_memory_surface_system(&language)
            .map_err(|error| HookError::new(HookErrorCategory::Internal, error.to_string()))?;
        let input = serde_json::to_string_pretty(&serde_json::json!({
            "question": "この主体が次の作業を始める際、毎回思い出しておくべきことは何か。継続的な制約、判断の前提、未解決事項、再発を避けたい教訓を、指定予算内でまとめる。",
            "body_token_budget": generation.body_token_budget,
            "input_token_budget": generation.input_token_budget,
            "materials": generation.materials,
        }))
        .map_err(|error| HookError::new(HookErrorCategory::Internal, error.to_string()))?;
        let output_state = MemorySurfaceOutputState::new(
            generation.materials.clone(),
            generation.body_token_budget,
        );
        let features = FeatureRegistryBuilder::new()
            .with_module(MemorySurfaceOutputFeature::new(output_state.clone()));
        let cancellation_observer = cancellation.clone();
        let cancel_observer = move |sender: tokio::sync::mpsc::Sender<()>| {
            tokio::spawn(async move {
                cancellation_observer.cancelled().await;
                let _ = sender.send(()).await;
            });
        };
        let result = run_internal_worker_with_cancel_sender(
            InternalWorkerSpec {
                identity: InternalWorkerIdentity {
                    kind: "subjektiv-memory-surface",
                    run_id: uuid::Uuid::now_v7(),
                },
                manifest: self.manifest.clone(),
                client: self.client.clone_boxed(),
                system_prompt,
                input,
                cache_key: None,
                max_turns: Some(EDITOR_MAX_TURNS),
                engine_configurator: None,
                features,
                required_tools: &["SubmitMemorySurface"],
                authority: InternalWorkerAuthority {
                    workspace: self.workspace_context.clone(),
                    filesystem: WorkerFilesystemAuthority::None,
                    scope: Scope::empty(),
                    workdir_session: None,
                },
            },
            cancel_observer,
        )
        .await;
        if let Err(error) = result {
            return Err(HookError::new(
                HookErrorCategory::Internal,
                format!("surface editor Internal Worker failed: {}", error.source),
            ));
        }
        output_state.submitted().ok_or_else(|| {
            HookError::new(
                HookErrorCategory::Internal,
                "surface editor finished without valid structured output",
            )
        })
    }

    async fn record_failure(&self, generation_id: &str, reason_code: &str) {
        if let Err(error) = self
            .workspace_client
            .fail_subjektiv_memory_surface(server_api::SubjektivSurfaceFailureRequest {
                generation_id: generation_id.to_string(),
                reason_code: reason_code.to_string(),
            })
            .await
        {
            tracing::debug!(%error, "could not record subject Memory surface failure");
        }
    }
}

fn surface_hook_error(error: WorkspaceMemoryBackendError) -> HookError {
    HookError::new(HookErrorCategory::Internal, error.to_string())
}
