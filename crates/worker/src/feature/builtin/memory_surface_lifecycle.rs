use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use agen::llm_client::event::Event as LlmEvent;
use agen::llm_client::{ClientError, LlmClient, Request};
use arc_swap::ArcSwap;
use async_trait::async_trait;
use futures_util::Stream;

use crate::PromptCatalog;
use crate::Scope;
use crate::feature::background::{
    BackgroundTaskCancellation, BackgroundTaskContext, BackgroundTaskSpec, BackgroundTaskTrigger,
    FeatureBackgroundTask,
};
use crate::feature::builtin::memory::WorkspaceMemoryBackendError;
use crate::feature::builtin::memory_surface_output::{
    MemorySurfaceOutputFeature, MemorySurfaceOutputState, submit_llm_tool_definition,
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
const EDITOR_QUESTION: &str = "この主体が次の作業を始める際、毎回思い出しておくべきことは何か。継続的な制約、判断の前提、未解決事項、再発を避けたい教訓を、指定予算内でまとめる。";

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

const INPUT_BUDGET_ERROR_MARKER: &str = "surface_editor_input_budget_exceeded";

struct InputBudgetLlmClient {
    inner: Box<dyn LlmClient>,
    input_token_budget: usize,
}

impl Clone for InputBudgetLlmClient {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone_boxed(),
            input_token_budget: self.input_token_budget,
        }
    }
}

#[async_trait]
impl LlmClient for InputBudgetLlmClient {
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }

    async fn stream(
        &self,
        request: Request,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent, ClientError>> + Send>>, ClientError>
    {
        let estimated = estimated_editor_request_tokens(&request)
            .map_err(|error| ClientError::Config(error.to_string()))?;
        if estimated > self.input_token_budget {
            return Err(ClientError::Config(format!(
                "{INPUT_BUDGET_ERROR_MARKER}: request estimate {estimated} exceeds {}",
                self.input_token_budget
            )));
        }
        self.inner.stream(request).await
    }
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

struct SurfaceEditorFailure {
    error: HookError,
    reason_code: &'static str,
}

impl SurfaceEditorFailure {
    fn input_budget(error: HookError) -> Self {
        Self {
            error,
            reason_code: "input_budget_exhausted",
        }
    }

    fn editor(error: HookError) -> Self {
        Self {
            error,
            reason_code: "editor_failed",
        }
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
                    Err(failure) => {
                        self.record_failure(&generation.generation_id, failure.reason_code)
                            .await;
                        tracing::warn!(error = %failure.error, reason_code = failure.reason_code, "subject Memory surface editor failed");
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
    ) -> Result<Vec<server_api::SubjektivSurfacePoint>, SurfaceEditorFailure> {
        let language = self
            .manifest
            .feature
            .memory
            .workspace_settings()
            .map(|settings| settings.language)
            .ok_or_else(|| {
                SurfaceEditorFailure::editor(HookError::new(
                    HookErrorCategory::Internal,
                    "surface editor requires bound Workspace Memory settings",
                ))
            })?;
        let system_prompt = self
            .prompts
            .load_full()
            .subjektiv_memory_surface_system(&language)
            .map_err(|error| {
                SurfaceEditorFailure::editor(HookError::new(
                    HookErrorCategory::Internal,
                    error.to_string(),
                ))
            })?;
        let (input, editor_materials) = bounded_editor_input(system_prompt.as_str(), generation)
            .map_err(SurfaceEditorFailure::input_budget)?;
        let output_state =
            MemorySurfaceOutputState::new(editor_materials, generation.body_token_budget);
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
                client: Box::new(InputBudgetLlmClient {
                    inner: self.client.clone_boxed(),
                    input_token_budget: generation.input_token_budget,
                }),
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
            let message = format!("surface editor Internal Worker failed: {}", error.source);
            let error = HookError::new(HookErrorCategory::Internal, message.clone());
            return Err(if message.contains(INPUT_BUDGET_ERROR_MARKER) {
                SurfaceEditorFailure::input_budget(error)
            } else {
                SurfaceEditorFailure::editor(error)
            });
        }
        output_state
            .submitted()
            .ok_or_else(|| {
                HookError::new(
                    HookErrorCategory::Internal,
                    "surface editor finished without valid structured output",
                )
            })
            .map_err(SurfaceEditorFailure::editor)
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

fn bounded_editor_input(
    system_prompt: &str,
    generation: &server_api::SubjektivSurfacePrepareResponse,
) -> Result<(String, Vec<server_api::SubjektivSurfaceMaterial>), HookError> {
    let mut materials = generation.materials.clone();
    loop {
        let input = serde_json::to_string(&serde_json::json!({
            "question": EDITOR_QUESTION,
            "body_token_budget": generation.body_token_budget,
            "input_token_budget": generation.input_token_budget,
            "materials": &materials,
        }))
        .map_err(|error| HookError::new(HookErrorCategory::Internal, error.to_string()))?;
        let request = Request::new()
            .system(system_prompt)
            .user(input.clone())
            .tool(submit_llm_tool_definition());
        let estimated_tokens = estimated_editor_request_tokens(&request)?;
        if estimated_tokens <= generation.input_token_budget {
            if materials.is_empty() && !generation.materials.is_empty() {
                return Err(HookError::new(
                    HookErrorCategory::Internal,
                    format!(
                        "surface editor fixed prompt and material overhead left no grounded material within the {} token input budget",
                        generation.input_token_budget
                    ),
                ));
            }
            return Ok((input, materials));
        }
        if materials.pop().is_none() {
            return Err(HookError::new(
                HookErrorCategory::Internal,
                format!(
                    "surface editor fixed prompt, JSON framing, and tool schema exceed the {} token input budget (estimated {estimated_tokens})",
                    generation.input_token_budget
                ),
            ));
        }
    }
}

fn estimated_editor_request_tokens(request: &Request) -> Result<usize, HookError> {
    // Serialize the actual prompt-bearing Agen request fields. This is the
    // stable provider-independent boundary; provider-specific HTTP envelopes
    // are intentionally outside the configured editor-input budget.
    let request = serde_json::to_vec(&serde_json::json!({
        "system_prompt": &request.system_prompt,
        "items": &request.items,
        "tools": &request.tools,
    }))
    .map_err(|error| HookError::new(HookErrorCategory::Internal, error.to_string()))?;
    Ok(request.len().saturating_add(3) / 4)
}

fn surface_hook_error(error: WorkspaceMemoryBackendError) -> HookError {
    HookError::new(HookErrorCategory::Internal, error.to_string())
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use agen::llm_client::event::{Event as LlmEvent, ResponseStatus, StatusEvent};
    use agen::llm_client::{ClientError, Request};
    use futures::Stream;
    use memory::extract::CandidateKind;

    use super::*;
    use crate::feature::FeatureId;
    use crate::feature::background::FeatureBackgroundTaskRegistryBuilder;
    use crate::hook::HookInvocationContext;
    use crate::worker::{WorkspaceClientError, WorkspaceRequest, WorkspaceResponse};

    fn material(id: &str, body_bytes: usize) -> server_api::SubjektivSurfaceMaterial {
        server_api::SubjektivSurfaceMaterial {
            memory_id: id.into(),
            revision: 1,
            kind: CandidateKind::Constraint,
            body_md: "x".repeat(body_bytes),
            why_useful: "budget fixture".into(),
            staleness: None,
        }
    }

    fn generation(
        input_token_budget: usize,
        materials: Vec<server_api::SubjektivSurfaceMaterial>,
    ) -> server_api::SubjektivSurfacePrepareResponse {
        server_api::SubjektivSurfacePrepareResponse {
            generation_id: "generation-1".into(),
            store_revision: 1,
            active_memory_count: materials.len(),
            materials,
            body_token_budget: 1_024,
            input_token_budget,
            per_kind_limit: 8,
            total_material_limit: 24,
        }
    }

    fn editor_request(system_prompt: &str, input: &str) -> Request {
        Request::new()
            .system(system_prompt)
            .user(input)
            .tool(submit_llm_tool_definition())
    }

    #[derive(Debug)]
    struct SurfaceWorkspaceClient {
        conflict_count: usize,
        input_token_budget: usize,
        prepare_calls: AtomicUsize,
        publish_calls: AtomicUsize,
        failure_reasons: Mutex<Vec<String>>,
    }

    impl SurfaceWorkspaceClient {
        fn new(conflict_count: usize) -> Self {
            Self {
                conflict_count,
                input_token_budget: 12_000,
                prepare_calls: AtomicUsize::new(0),
                publish_calls: AtomicUsize::new(0),
                failure_reasons: Mutex::new(Vec::new()),
            }
        }

        fn with_input_token_budget(input_token_budget: usize) -> Self {
            Self {
                input_token_budget,
                ..Self::new(0)
            }
        }

        fn response(value: server_api::SubjektivMemoryBackendResponse) -> WorkspaceResponse {
            WorkspaceResponse {
                status: 200,
                body: serde_json::to_string(&value).unwrap(),
            }
        }
    }

    impl WorkspaceClient for SurfaceWorkspaceClient {
        fn workspace_id(&self) -> Option<&str> {
            Some("workspace-1")
        }

        fn kind(&self) -> &str {
            "surface-lifecycle-test"
        }

        fn is_available(&self) -> bool {
            true
        }

        fn execute(
            &self,
            request: WorkspaceRequest,
        ) -> Result<WorkspaceResponse, WorkspaceClientError> {
            let request: server_api::SubjektivMemoryBackendRequest =
                serde_json::from_str(request.body.as_deref().unwrap_or_default())
                    .map_err(|error| WorkspaceClientError::Request(error.to_string()))?;
            match request.operation {
                server_api::SubjektivMemoryBackendOperation::PrepareSurface(_) => {
                    let call = self.prepare_calls.fetch_add(1, Ordering::SeqCst) + 1;
                    let mut prepared =
                        generation(self.input_token_budget, vec![material("memory-1", 32)]);
                    prepared.generation_id = format!("generation-{call}");
                    prepared.store_revision = call as u64;
                    Ok(Self::response(
                        server_api::SubjektivMemoryBackendResponse::SurfacePrepared(prepared),
                    ))
                }
                server_api::SubjektivMemoryBackendOperation::PublishSurface(input) => {
                    assert_eq!(input.points.len(), 1);
                    let call = self.publish_calls.fetch_add(1, Ordering::SeqCst) + 1;
                    if call <= self.conflict_count {
                        return Ok(WorkspaceResponse {
                            status: 409,
                            body: "store generation moved".into(),
                        });
                    }
                    Ok(Self::response(
                        server_api::SubjektivMemoryBackendResponse::SurfacePublished(
                            server_api::SubjektivSurfacePublishResponse {
                                snapshot_id: format!("surface-{call}"),
                                built_from_store_revision: call as u64,
                                empty: false,
                            },
                        ),
                    ))
                }
                server_api::SubjektivMemoryBackendOperation::FailSurface(input) => {
                    self.failure_reasons.lock().unwrap().push(input.reason_code);
                    Ok(Self::response(
                        server_api::SubjektivMemoryBackendResponse::SurfaceFailed(
                            server_api::SubjektivSurfaceFailureResponse {
                                store_revision: self.prepare_calls.load(Ordering::SeqCst) as u64,
                                status: "failed".into(),
                            },
                        ),
                    ))
                }
                _ => Err(WorkspaceClientError::Unavailable(
                    "surface lifecycle test only accepts surface operations".into(),
                )),
            }
        }
    }

    #[derive(Clone)]
    struct SurfaceEditorClient {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl LlmClient for SurfaceEditorClient {
        fn clone_boxed(&self) -> Box<dyn LlmClient> {
            Box::new(self.clone())
        }

        async fn stream(
            &self,
            _request: Request,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent, ClientError>> + Send>>, ClientError>
        {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            let events = if call % 2 == 0 {
                vec![
                    LlmEvent::tool_use_start(0, format!("submit-{call}"), "SubmitMemorySurface"),
                    LlmEvent::tool_input_delta(
                        0,
                        serde_json::json!({
                            "points": [{
                                "body_md": "- Keep the confirmed constraint.",
                                "memory_refs": [{"memory_id": "memory-1", "revision": 1}]
                            }]
                        })
                        .to_string(),
                    ),
                    LlmEvent::tool_use_stop(0),
                    LlmEvent::Status(StatusEvent {
                        status: ResponseStatus::Completed,
                    }),
                ]
            } else {
                vec![
                    LlmEvent::text_block_start(0),
                    LlmEvent::text_delta(0, "submitted"),
                    LlmEvent::text_block_stop(0, None),
                    LlmEvent::Status(StatusEvent {
                        status: ResponseStatus::Completed,
                    }),
                ]
            };
            Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
        }
    }

    fn test_manifest() -> WorkerManifest {
        let mut manifest = WorkerManifest::from_toml(
            r#"
[worker]
name = "surface-lifecycle-test"
scope = "main"

[model]
scheme = "anthropic"
model_id = "test-model"

[engine]

[[scope.allow]]
target = "/surface-lifecycle-test"
permission = "write"
"#,
        )
        .unwrap();
        manifest.feature.memory.profile.enabled = true;
        manifest
            .feature
            .memory
            .bind_workspace_settings(manifest::WorkspaceMemorySettingsSnapshot {
                workspace_id: "workspace-1".into(),
                settings_revision: 1,
                language: "English".into(),
            })
            .unwrap();
        manifest
    }

    fn test_task(
        workspace_client: Arc<dyn WorkspaceClient>,
        calls: Arc<AtomicUsize>,
    ) -> SubjektivSurfaceLifecycleTask {
        SubjektivSurfaceLifecycleTask {
            workspace_client,
            manifest: test_manifest(),
            client: Box::new(SurfaceEditorClient { calls }),
            prompts: Arc::new(ArcSwap::from(PromptCatalog::builtins_only().unwrap())),
            workspace_context: WorkerWorkspaceContext::no_workspace(),
        }
    }

    async fn run_task(task: SubjektivSurfaceLifecycleTask) {
        let declaration = BackgroundTaskDeclaration::worker_managed(
            TASK_NAME,
            "surface lifecycle integration test",
        );
        let mut spec = BackgroundTaskSpec::single_flight(declaration, Duration::from_secs(5));
        spec.trigger = BackgroundTaskTrigger::RunCommitted;
        let mut builder = FeatureBackgroundTaskRegistryBuilder::default();
        builder
            .register(FeatureId::builtin("surface-lifecycle-test"), spec, task)
            .unwrap();
        let registry = builder.build();
        registry
            .start_run_committed(HookInvocationContext {
                worker_id: "worker-1".into(),
                session_id: "session-1".into(),
                session_revision: 1,
                run_id: Some("run-1".into()),
                ..Default::default()
            })
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if !registry.diagnostics().is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("surface lifecycle task should finish");
        registry.shutdown().await.unwrap();
    }

    #[test]
    fn editor_input_budget_counts_prompt_json_question_and_tool_schema() {
        let generation = generation(
            4_000,
            vec![material("memory-1", 9_000), material("memory-2", 9_000)],
        );
        let (input, selected) = bounded_editor_input("system policy", &generation).unwrap();

        assert_eq!(selected.len(), 1, "deterministically trims the tail");
        assert_eq!(selected[0].memory_id, "memory-1");
        assert!(input.contains(EDITOR_QUESTION));
        assert!(
            estimated_editor_request_tokens(&editor_request("system policy", &input)).unwrap()
                <= generation.input_token_budget
        );
        let materials_only_tokens = (serde_json::to_vec(&selected)
            .unwrap()
            .len()
            .saturating_add(3))
            / 4;
        assert!(
            estimated_editor_request_tokens(&editor_request("system policy", &input)).unwrap()
                > materials_only_tokens,
            "full accounting must include framing and the SubmitMemorySurface schema"
        );
    }

    #[tokio::test]
    async fn actual_editor_request_is_rejected_before_provider_when_over_budget() {
        let calls = Arc::new(AtomicUsize::new(0));
        let client = InputBudgetLlmClient {
            inner: Box::new(SurfaceEditorClient {
                calls: calls.clone(),
            }),
            input_token_budget: 32,
        };
        let request = editor_request("system policy", &"x".repeat(1_000));

        let error = match client.stream(request).await {
            Err(error) => error,
            Ok(_) => panic!("over-budget request reached the provider"),
        };
        assert!(error.to_string().contains(INPUT_BUDGET_ERROR_MARKER));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn correction_turns_remain_subject_to_editor_input_budget() {
        let calls = Arc::new(AtomicUsize::new(0));
        let client = InputBudgetLlmClient {
            inner: Box::new(SurfaceEditorClient {
                calls: calls.clone(),
            }),
            input_token_budget: 2_000,
        };
        let _stream = client
            .stream(editor_request("system policy", "small initial request"))
            .await
            .unwrap();
        let request =
            editor_request("system policy", "small initial request").assistant("x".repeat(12_000));
        let error = match client.stream(request).await {
            Err(error) => error,
            Ok(_) => panic!("over-budget request reached the provider"),
        };

        assert!(error.to_string().contains(INPUT_BUDGET_ERROR_MARKER));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn lifecycle_rebuilds_once_after_a_store_generation_conflict() {
        let workspace = Arc::new(SurfaceWorkspaceClient::new(1));
        let editor_calls = Arc::new(AtomicUsize::new(0));
        run_task(test_task(workspace.clone(), editor_calls.clone())).await;

        assert_eq!(workspace.prepare_calls.load(Ordering::SeqCst), 2);
        assert_eq!(workspace.publish_calls.load(Ordering::SeqCst), 2);
        assert!(workspace.failure_reasons.lock().unwrap().is_empty());
        assert_eq!(editor_calls.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn lifecycle_bounds_conflict_retries_and_records_terminal_failure() {
        let workspace = Arc::new(SurfaceWorkspaceClient::new(MAX_GENERATION_ATTEMPTS));
        let editor_calls = Arc::new(AtomicUsize::new(0));
        run_task(test_task(workspace.clone(), editor_calls.clone())).await;

        assert_eq!(workspace.prepare_calls.load(Ordering::SeqCst), 2);
        assert_eq!(workspace.publish_calls.load(Ordering::SeqCst), 2);
        assert_eq!(editor_calls.load(Ordering::SeqCst), 4);
        assert_eq!(
            workspace.failure_reasons.lock().unwrap().as_slice(),
            ["publish_failed"]
        );
    }

    #[tokio::test]
    async fn lifecycle_records_input_budget_exhaustion_without_provider_or_publish() {
        let workspace = Arc::new(SurfaceWorkspaceClient::with_input_token_budget(1));
        let editor_calls = Arc::new(AtomicUsize::new(0));
        run_task(test_task(workspace.clone(), editor_calls.clone())).await;

        assert_eq!(workspace.prepare_calls.load(Ordering::SeqCst), 1);
        assert_eq!(workspace.publish_calls.load(Ordering::SeqCst), 0);
        assert_eq!(editor_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            workspace.failure_reasons.lock().unwrap().as_slice(),
            ["input_budget_exhausted"]
        );
    }

    #[test]
    fn editor_input_budget_never_relabels_trimmed_nonempty_materials_as_empty() {
        let generation = generation(1, vec![material("memory-1", 32)]);
        let error = bounded_editor_input("system policy", &generation).unwrap_err();
        assert!(error.to_string().contains("input budget"));
    }
}
