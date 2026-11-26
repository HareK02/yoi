//! Coordinates editing and publication of resident context generated from confirmed Memory.
//!
//! Budget checks cover the whole request, including the system prompt, JSON input representation,
//! and tool schemas, not just source material. Each LLM request repeats the check to account for
//! history growth in correction turns. This estimate excludes provider-specific HTTP representations;
//! it is neither a billing-token measurement nor a strict guarantee of model limits.
//!
//! Edit counts, execution time, and source refetches after publication conflicts are bounded.
//! Cancellation and background generations are rechecked before publication; the Host decides final
//! source freshness and persistence. Active Memory whose material exceeds the budget must not yield
//! successful empty generation. Generation failure does not undo confirmed Memory or applied candidates.

use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agen::EngineError;

use agen::llm_client::event::Event as LlmEvent;
use agen::llm_client::{ClientError, LlmClient, Request};
use arc_swap::ArcSwap;
use async_trait::async_trait;
use futures_util::{Stream, StreamExt};

use crate::PromptCatalog;
use crate::Scope;
use crate::feature::background::{
    BackgroundTaskCancellation, BackgroundTaskContext, BackgroundTaskSpec, BackgroundTaskTrigger,
    FeatureBackgroundTask,
};
use crate::feature::builtin::memory::is_builtin_subjektiv_consolidation_profile;
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
use crate::subjektiv::{SubjektivHostConnection, SubjektivHostError};
#[cfg(test)]
use crate::worker::WorkspaceClient;
use crate::worker::{WorkerError, WorkerFilesystemAuthority, WorkerWorkspaceContext};
use manifest::WorkerManifest;

const TASK_NAME: &str = "subjektiv-memory-surface";
const TASK_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_GENERATION_ATTEMPTS: usize = 2;
const EDITOR_MAX_TURNS: u32 = 3;
const EDITOR_QUESTION: &str = "この主体が次の作業を始める際、毎回思い出しておくべきことは何か。継続的な制約、判断の前提、未解決事項、再発を避けたい教訓を、指定予算内でまとめる。";

#[derive(Clone)]
pub struct SubjektivSurfaceLifecycleFeature {
    task: SubjektivSurfaceLifecycleTask,
}

#[derive(Clone)]
struct SubjektivSurfaceLifecycleTask {
    host: SubjektivHostConnection,
    manifest: WorkerManifest,
    client: Box<dyn LlmClient>,
    prompts: Arc<ArcSwap<PromptCatalog>>,
    workspace_context: WorkerWorkspaceContext,
}

const INPUT_BUDGET_ERROR_MARKER: &str = "surface_editor_input_budget_exceeded";

struct InputBudgetLlmClient {
    inner: Box<dyn LlmClient>,
    input_token_budget: usize,
    authorize_supply: Arc<dyn Fn() -> Result<(), SubjektivHostError> + Send + Sync>,
    // Safe summaries retain provenance before Worker can flatten a client error to Aborted.
    // In particular, provider text cannot impersonate a locally generated budget failure.
    failure_diagnostic: Arc<Mutex<Option<(&'static str, String)>>>,
}

impl Clone for InputBudgetLlmClient {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone_boxed(),
            input_token_budget: self.input_token_budget,
            authorize_supply: self.authorize_supply.clone(),
            failure_diagnostic: self.failure_diagnostic.clone(),
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
        *self.failure_diagnostic.lock().unwrap() = None;
        (self.authorize_supply)().map_err(|error| {
            *self.failure_diagnostic.lock().unwrap() = Some((
                "editor_failed",
                safe_host_diagnostic("memory.surface.supply", &error),
            ));
            ClientError::Config(error.to_string())
        })?;
        let estimated = estimated_editor_request_tokens(&request).map_err(|error| {
            *self.failure_diagnostic.lock().unwrap() = Some((
                "editor_failed",
                "stage=editor_input category=serialization".into(),
            ));
            ClientError::Config(error.to_string())
        })?;
        if estimated > self.input_token_budget {
            let message = format!(
                "{INPUT_BUDGET_ERROR_MARKER}: request estimate {estimated} exceeds {}",
                self.input_token_budget
            );
            *self.failure_diagnostic.lock().unwrap() = Some((
                "input_budget_exhausted",
                format!("stage=editor_input category=input_budget {message}"),
            ));
            return Err(ClientError::Config(message));
        }
        let stream = self.inner.stream(request).await.inspect_err(|error| {
            *self.failure_diagnostic.lock().unwrap() =
                Some(("editor_failed", safe_client_diagnostic(error)));
        })?;
        let diagnostic = self.failure_diagnostic.clone();
        Ok(Box::pin(stream.map(move |event| {
            event.inspect_err(|error| {
                *diagnostic.lock().unwrap() =
                    Some(("editor_failed", safe_client_diagnostic(error)));
            })
        })))
    }
}

impl SubjektivSurfaceLifecycleFeature {
    pub(crate) fn from_manifest(
        lifecycle_enabled: bool,
        host: Option<SubjektivHostConnection>,
        manifest: WorkerManifest,
        client: Box<dyn LlmClient>,
        prompts: Arc<ArcSwap<PromptCatalog>>,
        workspace_context: WorkerWorkspaceContext,
    ) -> std::io::Result<Option<Self>> {
        if !lifecycle_enabled
            || !is_builtin_subjektiv_consolidation_profile(&manifest)
            || host.is_none()
        {
            return Ok(None);
        }
        Self::for_host(
            host.expect("checked connection"),
            manifest,
            client,
            prompts,
            workspace_context,
        )
        .map(Some)
    }

    /// Install the reusable surface Feature for an explicitly granted Host.
    pub fn for_host(
        host: SubjektivHostConnection,
        manifest: WorkerManifest,
        client: Box<dyn LlmClient>,
        prompts: Arc<ArcSwap<PromptCatalog>>,
        workspace_context: WorkerWorkspaceContext,
    ) -> std::io::Result<Self> {
        manifest
            .feature
            .subjektiv
            .validate_execution()
            .map_err(std::io::Error::other)?;
        if !manifest.feature.subjektiv.profile.enabled || !host.host.consolidation_granted() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "surface generation requires explicit Host consolidation grant",
            ));
        }
        Ok(Self {
            task: SubjektivSurfaceLifecycleTask {
                host,
                manifest,
                client,
                prompts,
                workspace_context,
            },
        })
    }

    /// The Job tool awaits this clean-context lifecycle before submitting its result.
    pub async fn complete_for_job(
        &self,
        cancellation: BackgroundTaskCancellation,
        reusable_surface: Option<&serde_json::Value>,
    ) -> Result<serde_json::Value, HookError> {
        self.task
            .generate(None, cancellation, reusable_surface)
            .await
    }
}

impl FeatureModule for SubjektivSurfaceLifecycleFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builtin(
            "subjektiv-memory-surface-lifecycle",
            "subjektiv Memory Surface Lifecycle",
        )
        .with_description(
            "Rebuilds one bounded subject Memory surface after each committed consolidation run.",
        )
        .with_background_task(BackgroundTaskDeclaration::worker_managed(
            TASK_NAME,
            "Generate and publish a current subject Memory surface from bounded confirmed changes.",
        ))
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        let declaration = BackgroundTaskDeclaration::worker_managed(
            TASK_NAME,
            "Generate and publish a current subject Memory surface from bounded confirmed changes.",
        );
        let mut spec = BackgroundTaskSpec::single_flight(declaration, TASK_TIMEOUT);
        spec.trigger = BackgroundTaskTrigger::RunCommitted;
        context.background_tasks().register(spec, self.task.clone())
    }
}

struct SurfaceEditorFailure {
    error: HookError,
    reason_code: &'static str,
    diagnostic: String,
}

impl SurfaceEditorFailure {
    // Only errors generated by bounded_editor_input, never provider/Host prose.
    fn input_budget(error: HookError) -> Self {
        Self {
            diagnostic: format!("stage=editor_input category=input_budget {error}"),
            error,
            reason_code: "input_budget_exhausted",
        }
    }

    fn editor(error: HookError, diagnostic: impl Into<String>) -> Self {
        Self {
            error,
            reason_code: "editor_failed",
            diagnostic: diagnostic.into(),
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
        self.generate(Some(&context), cancellation, None)
            .await
            .map(|_| ())
    }
}

impl SubjektivSurfaceLifecycleTask {
    async fn generate(
        &self,
        context: Option<&BackgroundTaskContext>,
        cancellation: BackgroundTaskCancellation,
        reusable_surface: Option<&serde_json::Value>,
    ) -> Result<serde_json::Value, HookError> {
        for attempt in 0..MAX_GENERATION_ATTEMPTS {
            if let Some(context) = context {
                context.generation_fence.ensure_current()?;
            }
            if cancellation.is_cancelled() {
                return Err(HookError::new(
                    HookErrorCategory::Cancelled,
                    "surface generation cancelled",
                ));
            }
            let generation = self
                .host
                .prepare_subjektiv_memory_surface()
                .await
                .map_err(surface_hook_error)?;
            if context.is_none() {
                if let Some(snapshot_id) = &generation.current_snapshot_id {
                    return Ok(serde_json::json!({
                        "availability": "ready", "generation_id": generation.generation_id,
                        "memory_fingerprint": generation.memory_fingerprint, "snapshot_id": snapshot_id,
                    }));
                }
            }
            // Preparation is Backend-fenced to the live attempt and immutable batch.
            // Only a ready snapshot is immutable within its Memory fingerprint. Never
            // reuse a failed marker, which a later generation can replace.
            if let Some(surface) = reusable_surface.filter(|surface| {
                surface["availability"] == "ready"
                    && surface["memory_fingerprint"].as_str()
                        == Some(generation.memory_fingerprint.as_str())
            }) {
                return Ok(surface.clone());
            }
            if generation.materials.is_empty() && generation.active_memory_count > 0 {
                return self
                    .record_failure(
                        &generation,
                        "input_budget_exhausted",
                        "active Memory has no materials within the surface input budget",
                        context.is_none(),
                    )
                    .await;
            }
            let points = if generation.materials.is_empty() {
                Vec::new()
            } else {
                match tokio::time::timeout(
                    TASK_TIMEOUT,
                    self.edit_surface(&generation, cancellation.clone()),
                )
                .await
                {
                    Ok(Ok(points)) => points,
                    Ok(Err(failure)) => {
                        if cancellation.is_cancelled() {
                            return Err(HookError::new(
                                HookErrorCategory::Cancelled,
                                "surface generation cancelled",
                            ));
                        }
                        tracing::warn!(error = %failure.error, reason_code = failure.reason_code, "subject Memory surface editor failed");
                        return self
                            .record_failure(
                                &generation,
                                failure.reason_code,
                                &failure.diagnostic,
                                context.is_none(),
                            )
                            .await;
                    }
                    Err(error) => {
                        return self
                            .record_failure(
                                &generation,
                                "editor_timeout",
                                &format!(
                                    "surface editor timed out after {} seconds: {error}",
                                    TASK_TIMEOUT.as_secs()
                                ),
                                context.is_none(),
                            )
                            .await;
                    }
                }
            };
            if let Some(context) = context {
                context.generation_fence.ensure_current()?;
            }
            if cancellation.is_cancelled() {
                return Err(HookError::new(
                    HookErrorCategory::Cancelled,
                    "surface generation cancelled",
                ));
            }
            match self
                .host
                .publish_subjektiv_memory_surface(server_api::SubjektivSurfacePublishRequest {
                    generation_id: generation.generation_id.clone(),
                    points,
                })
                .await
            {
                Ok(output) => {
                    return Ok(serde_json::json!({
                        "availability": "ready", "generation_id": generation.generation_id,
                        "memory_fingerprint": output.built_from_memory_fingerprint, "snapshot_id": output.snapshot_id,
                    }));
                }
                Err(SubjektivHostError::Conflict { .. })
                    if attempt + 1 < MAX_GENERATION_ATTEMPTS =>
                {
                    continue;
                }
                Err(error) => {
                    tracing::warn!(%error, "subject Memory surface publication failed");
                    return self
                        .record_failure(
                            &generation,
                            "publish_failed",
                            &safe_host_diagnostic("publish", &error),
                            context.is_none(),
                        )
                        .await;
                }
            }
        }
        unreachable!("bounded generation loop always returns on its last attempt")
    }

    async fn edit_surface(
        &self,
        generation: &server_api::SubjektivSurfacePrepareResponse,
        cancellation: BackgroundTaskCancellation,
    ) -> Result<Vec<server_api::SubjektivSurfacePoint>, SurfaceEditorFailure> {
        let language = self.host.host.settings().language;
        let system_prompt = self
            .prompts
            .load_full()
            .subjektiv_memory_surface_system(&language)
            .map_err(|error| {
                SurfaceEditorFailure::editor(
                    HookError::new(HookErrorCategory::Internal, error.to_string()),
                    "stage=editor_prompt category=prompt_catalog",
                )
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
        let failure_diagnostic = Arc::new(Mutex::new(None));
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
                    authorize_supply: {
                        let host = self.host.clone();
                        Arc::new(move || host.authorize_operation("memory.surface.supply"))
                    },
                    failure_diagnostic: failure_diagnostic.clone(),
                }),
                system_prompt,
                input,
                cache_key: None,
                max_turns: Some(EDITOR_MAX_TURNS),
                compaction_budget: None,
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
            let (reason_code, diagnostic) = failure_diagnostic
                .lock()
                .unwrap()
                .take()
                .unwrap_or_else(|| ("editor_failed", safe_worker_diagnostic(&error.source)));
            let message = format!("surface editor Internal Worker failed: {}", error.source);
            return Err(SurfaceEditorFailure {
                error: HookError::new(HookErrorCategory::Internal, message),
                reason_code,
                diagnostic,
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
            .map_err(|error| SurfaceEditorFailure::editor(
                error,
                "stage=editor_output category=missing_output surface editor finished without valid structured output",
            ))
    }

    async fn record_failure(
        &self,
        generation: &server_api::SubjektivSurfacePrepareResponse,
        reason_code: &str,
        detail: &str,
        job_confirmation_required: bool,
    ) -> Result<serde_json::Value, HookError> {
        let failure = self
            .host
            .fail_subjektiv_memory_surface(server_api::SubjektivSurfaceFailureRequest {
                generation_id: generation.generation_id.clone(),
                reason_code: reason_code.to_string(),
                detail: Some(bounded_failure_detail(detail)),
            })
            .await
            .map_err(surface_hook_error)?;
        // Success must attest the actual persisted marker, not merely receipt of
        // a failure notification. A protected ready surface or moved fingerprint
        // must yield a Backend rejection/non-failed status, never a failed claim.
        let confirmed_status = if job_confirmation_required {
            "failed_confirmed"
        } else {
            "failed"
        };
        if failure.status != confirmed_status
            || failure.memory_fingerprint != generation.memory_fingerprint
        {
            return Err(HookError::new(
                HookErrorCategory::Internal,
                "Backend did not confirm a failed surface for this generation's fingerprint; no Job result was submitted",
            ));
        }
        Ok(serde_json::json!({
            "availability": "failed", "generation_id": generation.generation_id,
            "memory_fingerprint": failure.memory_fingerprint, "reason_code": reason_code,
        }))
    }
}

fn bounded_editor_input(
    system_prompt: &str,
    generation: &server_api::SubjektivSurfacePrepareResponse,
) -> Result<(String, Vec<server_api::SubjektivSurfaceMaterial>), HookError> {
    let empty_tokens = editor_request_tokens_for_materials(system_prompt, generation, &[])?;
    if empty_tokens > generation.input_token_budget {
        return Err(HookError::new(
            HookErrorCategory::Internal,
            format!(
                "surface editor fixed prompt, JSON framing, and tool schema exceed the {} token input budget (estimated {empty_tokens})",
                generation.input_token_budget
            ),
        ));
    }

    let mut populated_kinds = Vec::new();
    for material in &generation.materials {
        if !populated_kinds.contains(&material.kind) {
            populated_kinds.push(material.kind.clone());
        }
    }
    let material_token_budget = generation.input_token_budget - empty_tokens;
    let per_kind_token_budget = material_token_budget / populated_kinds.len().max(1);
    let mut accepted = vec![false; generation.materials.len()];
    let mut deferred = Vec::new();

    for (index, material) in generation.materials.iter().enumerate() {
        let kind_materials = generation
            .materials
            .iter()
            .zip(&accepted)
            .filter(|(selected, accepted)| **accepted && selected.kind == material.kind)
            .map(|(selected, _)| selected)
            .chain(std::iter::once(material))
            .collect::<Vec<_>>();
        let all_materials = generation
            .materials
            .iter()
            .zip(&accepted)
            .filter(|(_, accepted)| **accepted)
            .map(|(selected, _)| selected)
            .chain(std::iter::once(material))
            .collect::<Vec<_>>();
        let kind_cost =
            editor_request_tokens_for_materials(system_prompt, generation, &kind_materials)?
                .saturating_sub(empty_tokens);
        let all_tokens =
            editor_request_tokens_for_materials(system_prompt, generation, &all_materials)?;
        if kind_cost <= per_kind_token_budget && all_tokens <= generation.input_token_budget {
            accepted[index] = true;
        } else {
            deferred.push(index);
        }
    }

    // Equal per-kind shares reserve room in the final normalized request. Any
    // unused share is reclaimed in the original canonical round-robin order.
    for index in deferred {
        let material = &generation.materials[index];
        let all_materials = generation
            .materials
            .iter()
            .zip(&accepted)
            .filter(|(_, accepted)| **accepted)
            .map(|(selected, _)| selected)
            .chain(std::iter::once(material))
            .collect::<Vec<_>>();
        if editor_request_tokens_for_materials(system_prompt, generation, &all_materials)?
            <= generation.input_token_budget
        {
            accepted[index] = true;
        }
    }

    let materials = generation
        .materials
        .iter()
        .zip(accepted)
        .filter_map(|(material, accepted)| accepted.then_some(material.clone()))
        .collect::<Vec<_>>();
    if materials.is_empty() && !generation.materials.is_empty() {
        return Err(HookError::new(
            HookErrorCategory::Internal,
            format!(
                "surface editor fixed prompt and material overhead left no grounded material within the {} token input budget",
                generation.input_token_budget
            ),
        ));
    }
    let material_refs = materials.iter().collect::<Vec<_>>();
    let input = serialize_editor_input(generation, &material_refs)?;
    Ok((input, materials))
}

fn editor_request_tokens_for_materials(
    system_prompt: &str,
    generation: &server_api::SubjektivSurfacePrepareResponse,
    materials: &[&server_api::SubjektivSurfaceMaterial],
) -> Result<usize, HookError> {
    let input = serialize_editor_input(generation, materials)?;
    let request = Request::new()
        .system(system_prompt)
        .user(input)
        .tool(submit_llm_tool_definition());
    estimated_editor_request_tokens(&request)
}

fn serialize_editor_input(
    generation: &server_api::SubjektivSurfacePrepareResponse,
    materials: &[&server_api::SubjektivSurfaceMaterial],
) -> Result<String, HookError> {
    serde_json::to_string(&serde_json::json!({
        "question": EDITOR_QUESTION,
        "body_token_budget": generation.body_token_budget,
        "input_token_budget": generation.input_token_budget,
        "materials": materials,
    }))
    .map_err(|error| HookError::new(HookErrorCategory::Internal, error.to_string()))
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

// Persist only locally selected labels and typed numeric fields. Display/Debug text (including
// provider codes, JSON/SSE errors, URLs and Workspace messages) is deliberately not a fallback.
fn safe_client_diagnostic(error: &ClientError) -> String {
    let category = match error {
        ClientError::Http(error) => {
            return format!(
                "stage=editor_client category=http status={} timeout={} connect={} builder={} decode={}",
                error
                    .status()
                    .map(|status| status.as_u16().to_string())
                    .unwrap_or_else(|| "unknown".into()),
                error.is_timeout(),
                error.is_connect(),
                error.is_builder(),
                error.is_decode(),
            );
        }
        ClientError::Api { status, .. } => {
            return format!(
                "stage=editor_client category=api status={}",
                status
                    .map(|status| status.to_string())
                    .unwrap_or_else(|| "unknown".into()),
            );
        }
        ClientError::Timeout { phase, timeout } => {
            let phase = match *phase {
                "stream_open" => "stream_open",
                "stream_first_event" => "stream_first_event",
                _ => "unknown",
            };
            return format!(
                "stage=editor_client category=timeout phase={phase} timeout_ms={}",
                timeout.as_millis()
            );
        }
        ClientError::Json(_) => "json",
        ClientError::Sse(_) => "sse",
        ClientError::Config(_) => "config",
        ClientError::ContextWindowExceeded => "context_window_exceeded",
    };
    format!("stage=editor_client category={category}")
}

fn safe_worker_diagnostic(error: &WorkerError) -> String {
    let category = match error {
        WorkerError::StartupFailure { source, .. }
        | WorkerError::ConstructionCleanup { source, .. }
        | WorkerError::RestoreActivationEffects { source }
        | WorkerError::RunFailureAlreadyDelivered { source } => {
            return safe_worker_diagnostic(source);
        }
        WorkerError::RunTerminalPersistence { run_failure, .. } => {
            return format!(
                "{} terminal_persistence=failed",
                safe_worker_diagnostic(run_failure)
            );
        }
        WorkerError::SessionRegistration { source } => {
            return safe_host_diagnostic("editor_session_registration", source);
        }
        WorkerError::Engine(error) => {
            let category = match error {
                EngineError::Client(error) => return safe_client_diagnostic(error),
                // Exact local Internal Worker message, never an arbitrary aborted payload.
                EngineError::Aborted(message)
                    if message == "internal Worker reached its turn limit" =>
                {
                    return format!(
                        "stage=editor_engine category=max_turns internal Worker reached its turn limit (max_turns={EDITOR_MAX_TURNS})"
                    );
                }
                EngineError::Aborted(_) => "aborted",
                EngineError::Tool(_) => "tool",
                EngineError::Cancelled => "cancelled",
                EngineError::PauseRequested => "paused",
                EngineError::ConfigWarnings(_) => "config_warnings",
                EngineError::HistoryAppend(_) => "history_append",
                EngineError::Interceptor(_) => "interceptor",
                EngineError::ToolAttemptFence(_) => "tool_attempt_fence",
            };
            return format!("stage=editor_engine category={category}");
        }
        WorkerError::FeatureInstall(_) => "feature_install",
        WorkerError::FeatureLifecycle(_) => "feature_lifecycle",
        WorkerError::InvalidState(_) => "invalid_state",
        WorkerError::Provider(_) => "provider",
        WorkerError::PromptCatalog(_)
        | WorkerError::WorkspacePromptProjection { .. }
        | WorkerError::InvalidSystemPromptTemplate { .. }
        | WorkerError::SystemPromptRender { .. } => "prompt",
        WorkerError::Store(_) | WorkerError::WorkerStore(_) => "storage",
        _ => "worker",
    };
    format!("stage=editor_worker category={category}")
}

fn safe_host_diagnostic(stage: &'static str, error: &SubjektivHostError) -> String {
    let category = match error {
        SubjektivHostError::SessionRegistration { .. } => "session_registration",
        SubjektivHostError::Conflict { .. } => "conflict",
        SubjektivHostError::InvalidArgument(_) => "invalid_argument",
        SubjektivHostError::PermissionDenied(_) => "permission_denied",
        SubjektivHostError::Unavailable(_) => "unavailable",
    };
    format!("stage={stage} category={category}")
}

// Bounding is not redaction: callers must supply an already-safe diagnostic.
fn bounded_failure_detail(detail: &str) -> String {
    let mut detail = detail
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect::<String>();
    let mut end = detail
        .len()
        .min(server_api::SUBJEKTIV_SURFACE_FAILURE_DETAIL_MAX_BYTES);
    while !detail.is_char_boundary(end) {
        end -= 1;
    }
    detail.truncate(end);
    detail
}

fn surface_hook_error(error: SubjektivHostError) -> HookError {
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
        material_with_body(id, CandidateKind::Constraint, "x".repeat(body_bytes))
    }

    fn material_with_body(
        id: &str,
        kind: CandidateKind,
        body_md: String,
    ) -> server_api::SubjektivSurfaceMaterial {
        server_api::SubjektivSurfaceMaterial {
            memory_id: id.into(),
            change_id: "change-1".into(),
            kind,
            body_md,
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
            current_snapshot_id: None,
            memory_fingerprint: "memory-set-1".into(),
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
        allowed_operations: Mutex<Vec<&'static str>>,
        input_token_budget: usize,
        prepare_calls: AtomicUsize,
        publish_calls: AtomicUsize,
        failure_reasons: Mutex<Vec<String>>,
        failure_details: Mutex<Vec<Option<String>>>,
        job_results: Mutex<Vec<serde_json::Value>>,
        result_responses:
            Mutex<std::collections::VecDeque<Result<WorkspaceResponse, WorkspaceClientError>>>,
        current_fingerprint: Mutex<Option<String>>,
        failure_status: Mutex<String>,
        failure_fingerprint: Mutex<Option<String>>,
        ready_snapshot: Mutex<Option<String>>,
        memory_body: Option<String>,
        publish_response: Mutex<Option<Result<WorkspaceResponse, WorkspaceClientError>>>,
    }

    impl SurfaceWorkspaceClient {
        fn new(conflict_count: usize) -> Self {
            Self {
                conflict_count,
                allowed_operations: Mutex::new(vec![
                    "memory.surface.prepare",
                    "memory.surface.supply",
                    "memory.surface.publish",
                    "memory.surface.fail",
                ]),
                input_token_budget: 12_000,
                prepare_calls: AtomicUsize::new(0),
                publish_calls: AtomicUsize::new(0),
                failure_reasons: Mutex::new(Vec::new()),
                failure_details: Mutex::new(Vec::new()),
                job_results: Mutex::new(Vec::new()),
                result_responses: Mutex::new(Default::default()),
                current_fingerprint: Mutex::new(None),
                failure_status: Mutex::new("failed".into()),
                failure_fingerprint: Mutex::new(None),
                ready_snapshot: Mutex::new(None),
                memory_body: None,
                publish_response: Mutex::new(None),
            }
        }

        fn with_input_token_budget(input_token_budget: usize) -> Self {
            Self {
                input_token_budget,
                ..Self::new(0)
            }
        }

        fn fingerprint(&self, fallback: usize) -> String {
            self.current_fingerprint
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| format!("memory-set-{fallback}"))
        }

        fn response(value: server_api::SubjektivMemoryBackendResponse) -> WorkspaceResponse {
            WorkspaceResponse {
                content_type: None,
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
            if let Some(response) = crate::subjektiv::test_subject_authorization_response(
                &request,
                "workspace-1",
                &self.allowed_operations.lock().unwrap(),
            ) {
                return Ok(response);
            }
            if request.path.ends_with("/workers/self/backend-job-result") {
                assert!(
                    self.publish_calls.load(Ordering::SeqCst) > 0
                        || !self.failure_reasons.lock().unwrap().is_empty(),
                    "result must follow publication or recorded failure"
                );
                self.job_results
                    .lock()
                    .unwrap()
                    .push(serde_json::from_str(request.body.as_deref().unwrap()).unwrap());
                return self
                    .result_responses
                    .lock()
                    .unwrap()
                    .pop_front()
                    .unwrap_or_else(|| {
                        Ok(WorkspaceResponse {
                            content_type: None,
                            status: 200,
                            body: "{\"replayed\":false}".into(),
                        })
                    });
            }
            let request: server_api::SubjektivMemoryBackendRequest =
                serde_json::from_str(request.body.as_deref().unwrap_or_default())
                    .map_err(|error| WorkspaceClientError::Request(error.to_string()))?;
            match request.operation {
                server_api::SubjektivMemoryBackendOperation::PrepareSurface(_) => {
                    let call = self.prepare_calls.fetch_add(1, Ordering::SeqCst) + 1;
                    let mut prepared =
                        generation(self.input_token_budget, vec![material("memory-1", 32)]);
                    if let Some(body) = &self.memory_body {
                        prepared.materials[0].body_md = body.clone();
                    }
                    prepared.generation_id = format!("generation-{call}");
                    prepared.memory_fingerprint = self.fingerprint(call);
                    prepared.current_snapshot_id = self.ready_snapshot.lock().unwrap().clone();
                    Ok(Self::response(
                        server_api::SubjektivMemoryBackendResponse::SurfacePrepared(prepared),
                    ))
                }
                server_api::SubjektivMemoryBackendOperation::PublishSurface(input) => {
                    assert_eq!(input.points.len(), 1);
                    let call = self.publish_calls.fetch_add(1, Ordering::SeqCst) + 1;
                    if let Some(response) = self.publish_response.lock().unwrap().take() {
                        return response;
                    }
                    if call <= self.conflict_count {
                        return Ok(WorkspaceResponse {
                            content_type: None,
                            status: 409,
                            body: "store generation moved".into(),
                        });
                    }
                    Ok(Self::response(
                        server_api::SubjektivMemoryBackendResponse::SurfacePublished(
                            server_api::SubjektivSurfacePublishResponse {
                                snapshot_id: format!("surface-{call}"),
                                built_from_memory_fingerprint: self.fingerprint(call),
                                empty: false,
                            },
                        ),
                    ))
                }
                server_api::SubjektivMemoryBackendOperation::FailSurface(input) => {
                    let detail = input.detail.as_deref().unwrap();
                    assert!(!detail.chars().any(char::is_control));
                    assert!(detail.len() <= server_api::SUBJEKTIV_SURFACE_FAILURE_DETAIL_MAX_BYTES);
                    self.failure_reasons.lock().unwrap().push(input.reason_code);
                    self.failure_details.lock().unwrap().push(input.detail);
                    Ok(Self::response(
                        server_api::SubjektivMemoryBackendResponse::SurfaceFailed(
                            server_api::SubjektivSurfaceFailureResponse {
                                memory_fingerprint: self
                                    .failure_fingerprint
                                    .lock()
                                    .unwrap()
                                    .clone()
                                    .unwrap_or_else(|| {
                                        self.fingerprint(self.prepare_calls.load(Ordering::SeqCst))
                                    }),
                                status: self.failure_status.lock().unwrap().clone(),
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
                                "memory_refs": [{"memory_id": "memory-1", "change_id": "change-1"}]
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
        manifest.feature.subjektiv.profile.enabled = true;
        manifest
            .feature
            .subjektiv
            .bind_workspace_settings(manifest::SubjektivMemorySettingsSnapshot {
                workspace_id: "workspace-1".into(),

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
            host: crate::subjektiv::test_connection(workspace_client),
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

    #[tokio::test]
    async fn failure_request_detail_is_utf8_bounded_and_not_returned_as_surface_text() {
        let workspace = Arc::new(SurfaceWorkspaceClient::new(0));
        *workspace.current_fingerprint.lock().unwrap() = Some("memory-set-1".into());
        let task = test_task(workspace.clone(), Arc::new(AtomicUsize::new(0)));
        let limit = server_api::SUBJEKTIV_SURFACE_FAILURE_DETAIL_MAX_BYTES;
        for detail in [
            "short diagnostic".to_string(),
            "x".repeat(limit),
            "x".repeat(limit + 1),
            format!("{}界trailing", "x".repeat(limit - 1)),
            "界".repeat(limit),
        ] {
            let outcome = task
                .record_failure(&generation(1, vec![]), "publish_failed", &detail, false)
                .await
                .unwrap();
            let details = workspace.failure_details.lock().unwrap();
            let sent = details.last().unwrap().as_deref().unwrap();
            assert!(sent.len() <= limit);
            assert!(detail.starts_with(sent));
            if detail.len() <= limit {
                assert_eq!(sent, detail);
            } else {
                let next_char_len = detail[sent.len()..].chars().next().unwrap().len_utf8();
                assert!(sent.len() + next_char_len > limit);
            }
            assert!(outcome.get("detail").is_none());
        }
    }

    const SECRET_URL: &str =
        "https://sentinel-user:sentinel-password@example.invalid/api?auth=sentinel-key";
    const SECRET_INPUT: &str = "sentinel-private-memory-input";

    #[derive(Clone)]
    struct FailingSurfaceEditorClient {
        kind: &'static str,
        stream_error: bool,
    }

    #[async_trait]
    impl LlmClient for FailingSurfaceEditorClient {
        fn clone_boxed(&self) -> Box<dyn LlmClient> {
            Box::new(self.clone())
        }

        async fn stream(
            &self,
            request: Request,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent, ClientError>> + Send>>, ClientError>
        {
            let echoed = serde_json::to_string(&request.items).unwrap();
            assert!(echoed.contains(SECRET_INPUT));
            let raw = format!("{SECRET_URL} echoed input={echoed}\0");
            let error = match self.kind {
                "api" => ClientError::Api {
                    status: Some(401),
                    code: Some(raw.clone()),
                    message: raw,
                    retry_after: None,
                },
                "http" => ClientError::Http(
                    reqwest::Client::new()
                        .get(SECRET_URL)
                        .header("x-invalid", "\0")
                        .build()
                        .unwrap_err()
                        .with_url(SECRET_URL.parse().unwrap()),
                ),
                "sse" => ClientError::Sse(raw),
                // A provider cannot impersonate the local budget failure marker.
                "config" => ClientError::Config(format!("{INPUT_BUDGET_ERROR_MARKER}: {raw}")),
                "json" => ClientError::Json(<serde_json::Error as serde::de::Error>::custom(raw)),
                "timeout" => ClientError::Timeout {
                    phase: "stream_open",
                    timeout: Duration::from_secs(7),
                },
                _ => panic!("unknown test error kind"),
            };
            if self.stream_error {
                Ok(Box::pin(futures::stream::iter([Err(error)])))
            } else {
                Err(error)
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn actual_editor_errors_persist_typed_diagnostics_without_provider_secrets() {
        for kind in ["api", "http", "sse", "config", "json", "timeout"] {
            for stream_error in [false, true] {
                let workspace = Arc::new(SurfaceWorkspaceClient {
                    memory_body: Some(SECRET_INPUT.into()),
                    ..SurfaceWorkspaceClient::new(0)
                });
                *workspace.failure_status.lock().unwrap() = "failed_confirmed".into();
                let mut task = test_task(workspace.clone(), Arc::new(AtomicUsize::new(0)));
                task.client = Box::new(FailingSurfaceEditorClient { kind, stream_error });
                let outcome = task
                    .generate(None, BackgroundTaskCancellation::default(), None)
                    .await
                    .unwrap();
                assert_eq!(outcome["reason_code"], "editor_failed", "{kind}");
                let details = workspace.failure_details.lock().unwrap();
                let detail = details[0].as_deref().unwrap();
                assert!(detail.contains("stage=editor_client"), "{kind}: {detail}");
                assert!(detail.contains(&format!("category={kind}")), "{detail}");
                if kind == "api" {
                    assert!(detail.contains("status=401"));
                }
                if kind == "http" {
                    assert!(detail.contains("timeout=false"));
                    assert!(detail.contains("builder=true"));
                }
                if kind == "timeout" {
                    assert!(detail.contains("phase=stream_open"));
                    assert!(detail.contains("timeout_ms=7000"));
                }
                assert!(!detail.contains("sentinel"), "{detail}");
                assert!(!detail.contains("https://"));
                assert!(!detail.contains(EDITOR_QUESTION));
                assert!(!detail.contains(INPUT_BUDGET_ERROR_MARKER));
                assert!(outcome.get("detail").is_none());
                assert_eq!(workspace.publish_calls.load(Ordering::SeqCst), 0);
            }
        }
    }

    #[derive(Clone)]
    struct IncompleteSurfaceEditorClient {
        reach_turn_limit: bool,
    }

    #[async_trait]
    impl LlmClient for IncompleteSurfaceEditorClient {
        fn clone_boxed(&self) -> Box<dyn LlmClient> {
            Box::new(self.clone())
        }

        async fn stream(
            &self,
            request: Request,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent, ClientError>> + Send>>, ClientError>
        {
            // Repeated tool turns reach max_turns; plain text finishes without structured output.
            SurfaceEditorClient {
                calls: Arc::new(AtomicUsize::new(if self.reach_turn_limit { 0 } else { 1 })),
            }
            .stream(request)
            .await
        }
    }

    #[tokio::test]
    async fn actual_local_editor_failures_keep_useful_messages() {
        for (reach_turn_limit, category, message) in [
            (true, "max_turns", "internal Worker reached its turn limit"),
            (
                false,
                "missing_output",
                "surface editor finished without valid structured output",
            ),
        ] {
            let workspace = Arc::new(SurfaceWorkspaceClient::new(0));
            *workspace.failure_status.lock().unwrap() = "failed_confirmed".into();
            let mut task = test_task(workspace.clone(), Arc::new(AtomicUsize::new(0)));
            task.client = Box::new(IncompleteSurfaceEditorClient { reach_turn_limit });
            let outcome = task
                .generate(None, BackgroundTaskCancellation::default(), None)
                .await
                .unwrap();
            assert_eq!(outcome["reason_code"], "editor_failed");
            let details = workspace.failure_details.lock().unwrap();
            let detail = details[0].as_deref().unwrap();
            assert!(detail.contains(&format!("category={category}")), "{detail}");
            assert!(detail.contains(message));
        }
    }

    #[tokio::test]
    async fn actual_publication_errors_do_not_persist_workspace_messages() {
        for response in [
            Ok(WorkspaceResponse {
                content_type: None,
                status: 403,
                body: format!("{SECRET_URL} {SECRET_INPUT}\0"),
            }),
            Err(WorkspaceClientError::Request(format!(
                "{SECRET_URL} {SECRET_INPUT}\0"
            ))),
        ] {
            let workspace = Arc::new(SurfaceWorkspaceClient::new(0));
            *workspace.publish_response.lock().unwrap() = Some(response);
            *workspace.failure_status.lock().unwrap() = "failed_confirmed".into();
            let outcome = test_task(workspace.clone(), Arc::new(AtomicUsize::new(0)))
                .generate(None, BackgroundTaskCancellation::default(), None)
                .await
                .unwrap();
            assert_eq!(outcome["reason_code"], "publish_failed");
            let details = workspace.failure_details.lock().unwrap();
            let detail = details[0].as_deref().unwrap();
            assert!(detail.starts_with("stage=publish category="));
            assert!(!detail.contains("sentinel"));
            assert!(!detail.contains("https://"));
        }
    }

    #[tokio::test]
    async fn failure_request_normalizes_controls_before_utf8_truncation() {
        let workspace = Arc::new(SurfaceWorkspaceClient::new(0));
        *workspace.current_fingerprint.lock().unwrap() = Some("memory-set-1".into());
        let task = test_task(workspace.clone(), Arc::new(AtomicUsize::new(0)));
        let limit = server_api::SUBJEKTIV_SURFACE_FAILURE_DETAIL_MAX_BYTES;
        for (raw, expected) in [
            (
                "stage=publish\0category=conflict\n\r\t\u{7f}\u{85}".into(),
                "stage=publish category=conflict     ".into(),
            ),
            (
                format!("{}\u{85}界trailing", "x".repeat(limit - 4)),
                format!("{} 界", "x".repeat(limit - 4)),
            ),
            (
                format!("{}\0界", "x".repeat(limit - 2)),
                format!("{} ", "x".repeat(limit - 2)),
            ),
        ] {
            let outcome = task
                .record_failure(&generation(1, vec![]), "publish_failed", &raw, false)
                .await
                .unwrap();
            assert_eq!(outcome["availability"], "failed");
            assert_eq!(
                workspace
                    .failure_details
                    .lock()
                    .unwrap()
                    .last()
                    .unwrap()
                    .as_deref(),
                Some(expected.as_str())
            );
        }
    }

    #[test]
    fn typed_diagnostic_fallbacks_never_copy_arbitrary_error_text() {
        let secret = format!("{SECRET_URL} {SECRET_INPUT}");
        for error in [
            WorkerError::FeatureInstall(secret.clone()),
            WorkerError::Engine(EngineError::Aborted(secret.clone())),
            WorkerError::Engine(EngineError::HistoryAppend(secret.clone())),
            WorkerError::SessionRegistration {
                source: SubjektivHostError::Unavailable(secret.clone()),
            },
            WorkerError::Engine(EngineError::Client(ClientError::Timeout {
                phase: SECRET_URL,
                timeout: Duration::from_secs(1),
            })),
        ] {
            let detail = safe_worker_diagnostic(&error);
            assert!(detail.contains("stage="));
            assert!(detail.contains("category="));
            assert!(!detail.contains("sentinel"));
        }
        let limit = safe_worker_diagnostic(&WorkerError::Engine(EngineError::Aborted(
            "internal Worker reached its turn limit".into(),
        )));
        assert!(limit.contains("category=max_turns"));
        assert!(limit.contains("max_turns=3"));
    }

    #[test]
    fn editor_input_budget_counts_prompt_json_question_and_tool_schema() {
        let generation = generation(
            4_000,
            vec![material("memory-1", 9_000), material("memory-2", 9_000)],
        );
        let (input, selected) = bounded_editor_input("system policy", &generation).unwrap();

        assert_eq!(selected.len(), 1, "deterministically bounds one category");
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

    #[test]
    fn final_editor_budget_preserves_feasible_nonempty_categories() {
        let materials = vec![
            material_with_body(
                "preference-large",
                CandidateKind::Preference,
                "\\\"".repeat(8_000),
            ),
            material_with_body(
                "constraint-small",
                CandidateKind::Constraint,
                "constraint".into(),
            ),
            material_with_body("decision-small", CandidateKind::Decision, "decision".into()),
            material_with_body(
                "assumption-small",
                CandidateKind::WorkingAssumption,
                "assumption".into(),
            ),
            material_with_body(
                "question-small",
                CandidateKind::OpenQuestion,
                "question".into(),
            ),
            material_with_body("lesson-small", CandidateKind::Lesson, "lesson".into()),
        ];
        let mut generation = generation(9_999, materials);
        for _ in 0..3 {
            let large = editor_request_tokens_for_materials(
                "system policy",
                &generation,
                &[&generation.materials[0]],
            )
            .unwrap();
            let small = editor_request_tokens_for_materials(
                "system policy",
                &generation,
                &generation.materials[1..].iter().collect::<Vec<_>>(),
            )
            .unwrap();
            generation.input_token_budget = large.max(small);
        }
        let all = editor_request_tokens_for_materials(
            "system policy",
            &generation,
            &generation.materials.iter().collect::<Vec<_>>(),
        )
        .unwrap();
        assert!(all > generation.input_token_budget);

        let (input, selected) = bounded_editor_input("system policy", &generation).unwrap();
        assert!(
            !selected
                .iter()
                .any(|item| item.kind == CandidateKind::Preference)
        );
        for kind in [
            CandidateKind::Constraint,
            CandidateKind::Decision,
            CandidateKind::WorkingAssumption,
            CandidateKind::OpenQuestion,
            CandidateKind::Lesson,
        ] {
            assert!(selected.iter().any(|item| item.kind == kind));
        }
        assert!(
            estimated_editor_request_tokens(&editor_request("system policy", &input)).unwrap()
                <= generation.input_token_budget
        );
    }

    #[tokio::test]
    async fn revoked_surface_supply_stops_the_next_request_before_provider() {
        let calls = Arc::new(AtomicUsize::new(0));
        let allowed = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let check = allowed.clone();
        let client = InputBudgetLlmClient {
            inner: Box::new(SurfaceEditorClient {
                calls: calls.clone(),
            }),
            input_token_budget: 2_000,
            authorize_supply: Arc::new(move || {
                if check.load(Ordering::SeqCst) {
                    Ok(())
                } else {
                    Err(SubjektivHostError::PermissionDenied(
                        "memory.surface.supply denied".into(),
                    ))
                }
            }),
            failure_diagnostic: Arc::new(Mutex::new(None)),
        };
        let _stream = client
            .stream(editor_request("system policy", "protected surface"))
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        allowed.store(false, Ordering::SeqCst);
        let result = client
            .stream(editor_request("system policy", "same protected surface"))
            .await;
        assert!(
            matches!(result, Err(ClientError::Config(reason)) if reason.contains("memory.surface.supply denied"))
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn actual_editor_request_is_rejected_before_provider_when_over_budget() {
        let calls = Arc::new(AtomicUsize::new(0));
        let client = InputBudgetLlmClient {
            inner: Box::new(SurfaceEditorClient {
                calls: calls.clone(),
            }),
            input_token_budget: 32,
            authorize_supply: Arc::new(|| Ok(())),
            failure_diagnostic: Arc::new(Mutex::new(None)),
        };
        let request = editor_request("system policy", &"x".repeat(1_000));

        let error = match client.stream(request).await {
            Err(error) => error,
            Ok(_) => panic!("over-budget request reached the provider"),
        };
        assert!(error.to_string().contains(INPUT_BUDGET_ERROR_MARKER));
        let diagnostic = client.failure_diagnostic.lock().unwrap();
        let (reason, detail) = diagnostic.as_ref().unwrap();
        assert_eq!(*reason, "input_budget_exhausted");
        assert!(detail.contains("stage=editor_input category=input_budget"));
        assert!(detail.contains("request estimate"));
        assert!(detail.contains("exceeds 32"));
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
            authorize_supply: Arc::new(|| Ok(())),
            failure_diagnostic: Arc::new(Mutex::new(None)),
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
    async fn denied_surface_prepare_never_reads_materials_or_calls_provider() {
        let workspace = Arc::new(SurfaceWorkspaceClient::new(0));
        workspace
            .allowed_operations
            .lock()
            .unwrap()
            .retain(|operation| *operation != "memory.surface.prepare");
        let calls = Arc::new(AtomicUsize::new(0));
        let error = test_task(workspace.clone(), calls.clone())
            .generate(None, BackgroundTaskCancellation::default(), None)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("denied"));
        assert_eq!(workspace.prepare_calls.load(Ordering::SeqCst), 0);
        assert_eq!(workspace.publish_calls.load(Ordering::SeqCst), 0);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(workspace.failure_reasons.lock().unwrap().is_empty());
        assert!(workspace.job_results.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn denied_surface_supply_stops_actual_editor_before_provider_or_publish() {
        let workspace = Arc::new(SurfaceWorkspaceClient::new(0));
        workspace
            .allowed_operations
            .lock()
            .unwrap()
            .retain(|operation| *operation != "memory.surface.supply");
        *workspace.failure_status.lock().unwrap() = "failed_confirmed".into();
        let calls = Arc::new(AtomicUsize::new(0));
        let outcome = test_task(workspace.clone(), calls.clone())
            .generate(None, BackgroundTaskCancellation::default(), None)
            .await
            .unwrap();
        assert_eq!(outcome["availability"], "failed");
        assert_eq!(outcome["reason_code"], "editor_failed");
        assert_eq!(workspace.prepare_calls.load(Ordering::SeqCst), 1);
        assert_eq!(workspace.publish_calls.load(Ordering::SeqCst), 0);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            workspace.failure_reasons.lock().unwrap().as_slice(),
            ["editor_failed"]
        );
        let details = workspace.failure_details.lock().unwrap();
        let detail = details[0].as_deref().unwrap();
        assert!(detail.contains("stage=memory.surface.supply"));
        assert!(detail.contains("memory.surface.supply"));
        assert!(detail.contains("denied"));
        assert!(
            outcome.get("detail").is_none(),
            "diagnostic is not surface text"
        );
        assert!(workspace.job_results.lock().unwrap().is_empty());
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
        assert!(
            workspace.failure_details.lock().unwrap()[0]
                .as_deref()
                .unwrap()
                .contains("stage=publish category=conflict")
        );
    }

    #[tokio::test]
    async fn lifecycle_records_input_budget_exhaustion_without_provider_or_publish() {
        let workspace = Arc::new(SurfaceWorkspaceClient::with_input_token_budget(1));
        let editor_calls = Arc::new(AtomicUsize::new(0));
        let task = test_task(workspace.clone(), editor_calls.clone());
        let expected = match task
            .edit_surface(
                &generation(1, vec![material("memory-1", 32)]),
                BackgroundTaskCancellation::default(),
            )
            .await
        {
            Err(failure) => failure.diagnostic,
            Ok(_) => panic!("fixture must exhaust input budget"),
        };
        run_task(task).await;
        assert_eq!(
            workspace.failure_details.lock().unwrap().as_slice(),
            &[Some(expected)]
        );

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
    #[tokio::test]
    async fn job_result_waits_for_surface_and_retries_the_exact_outcome() {
        for failed in [false, true] {
            let workspace = Arc::new(if failed {
                SurfaceWorkspaceClient::with_input_token_budget(1)
            } else {
                SurfaceWorkspaceClient::new(0)
            });
            *workspace.failure_status.lock().unwrap() = "failed_confirmed".into();
            let lifecycle = SubjektivSurfaceLifecycleFeature {
                task: test_task(workspace.clone(), Arc::new(AtomicUsize::new(0))),
            };
            let mut pending = Vec::new();
            let mut hooks = crate::hook::HookRegistryBuilder::default();
            FeatureRegistryBuilder::new()
                .with_module(
                    crate::feature::builtin::backend_job_result::BackendJobResultFeature::new(
                        workspace.clone(),
                    )
                    .with_surface(lifecycle),
                )
                .install_into_pending(&mut pending, &mut hooks);
            let (_, tool) = pending.remove(0)();
            let input = r#"{"result":{"subject_id":"subject-1","candidate_ids":["candidate-1"]}}"#;
            for _ in 0..2 {
                tool.execute(input, agen::tool::ToolExecutionContext::direct())
                    .await
                    .unwrap();
            }
            let results = workspace.job_results.lock().unwrap();
            assert_eq!(results.len(), 2);
            assert_eq!(results[0], results[1]);
            assert_eq!(
                results[0]["result"]["surface"]["availability"],
                if failed { "failed" } else { "ready" }
            );
            assert_eq!(
                results[0]["result"]["surface"]["generation_id"],
                "generation-1"
            );
            assert_eq!(
                results[0]["result"]["surface"]["memory_fingerprint"],
                "memory-set-1"
            );
            assert_eq!(workspace.prepare_calls.load(Ordering::SeqCst), 1);
            assert!(
                tool.execute(
                    r#"{"result":{"subject_id":"subject-1","candidate_ids":[]}}"#,
                    agen::tool::ToolExecutionContext::direct()
                )
                .await
                .is_err()
            );
        }
    }

    #[tokio::test]
    async fn job_uses_backend_confirmed_current_surface_without_regeneration() {
        let workspace = Arc::new(SurfaceWorkspaceClient::new(0));
        *workspace.ready_snapshot.lock().unwrap() = Some("already-current".into());
        let calls = Arc::new(AtomicUsize::new(0));
        let task = test_task(workspace.clone(), calls.clone());
        let outcome = task
            .generate(None, BackgroundTaskCancellation::default(), None)
            .await
            .unwrap();
        assert_eq!(outcome["availability"], "ready");
        assert_eq!(outcome["generation_id"], "generation-1");
        assert_eq!(outcome["snapshot_id"], "already-current");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(workspace.publish_calls.load(Ordering::SeqCst), 0);
        assert!(workspace.failure_reasons.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn job_result_rejects_model_surface_claims_before_generation() {
        let workspace = Arc::new(SurfaceWorkspaceClient::new(0));
        let lifecycle = SubjektivSurfaceLifecycleFeature {
            task: test_task(workspace.clone(), Arc::new(AtomicUsize::new(0))),
        };
        let mut pending = Vec::new();
        let mut hooks = crate::hook::HookRegistryBuilder::default();
        FeatureRegistryBuilder::new()
            .with_module(
                crate::feature::builtin::backend_job_result::BackendJobResultFeature::new(
                    workspace.clone(),
                )
                .with_surface(lifecycle),
            )
            .install_into_pending(&mut pending, &mut hooks);
        let (_, tool) = pending.remove(0)();
        assert!(tool.execute(r#"{"result":{"subject_id":"subject-1","candidate_ids":[],"surface":{"availability":"ready"}}}"#, agen::tool::ToolExecutionContext::direct()).await.is_err());
        assert_eq!(workspace.prepare_calls.load(Ordering::SeqCst), 0);
        assert!(workspace.job_results.lock().unwrap().is_empty());
    }

    #[derive(Clone)]
    struct PendingSurfaceEditorClient(Arc<tokio::sync::Notify>);
    #[async_trait]
    impl LlmClient for PendingSurfaceEditorClient {
        fn clone_boxed(&self) -> Box<dyn LlmClient> {
            Box::new(self.clone())
        }
        async fn stream(
            &self,
            _request: Request,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent, ClientError>> + Send>>, ClientError>
        {
            self.0.notify_one();
            Ok(Box::pin(futures::stream::pending()))
        }
    }

    #[tokio::test(start_paused = true)]
    async fn editor_timeout_sends_diagnostic_only_in_failure_request() {
        let workspace = Arc::new(SurfaceWorkspaceClient::new(0));
        *workspace.failure_status.lock().unwrap() = "failed_confirmed".into();
        let entered = Arc::new(tokio::sync::Notify::new());
        let mut task = test_task(workspace.clone(), Arc::new(AtomicUsize::new(0)));
        task.client = Box::new(PendingSurfaceEditorClient(entered.clone()));
        let running = tokio::spawn(async move {
            task.generate(None, BackgroundTaskCancellation::default(), None)
                .await
        });
        entered.notified().await;
        tokio::time::advance(TASK_TIMEOUT).await;
        let outcome = running.await.unwrap().unwrap();
        assert_eq!(outcome["reason_code"], "editor_timeout");
        assert!(outcome.get("detail").is_none());
        assert_eq!(workspace.publish_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            workspace.failure_details.lock().unwrap().as_slice(),
            &[Some(
                "surface editor timed out after 300 seconds: deadline has elapsed".into()
            )]
        );
    }

    #[tokio::test]
    async fn cancelling_job_surface_waits_for_editor_and_never_submits_result() {
        let workspace = Arc::new(SurfaceWorkspaceClient::new(0));
        let entered = Arc::new(tokio::sync::Notify::new());
        let mut task = test_task(workspace.clone(), Arc::new(AtomicUsize::new(0)));
        task.client = Box::new(PendingSurfaceEditorClient(entered.clone()));
        let mut pending = Vec::new();
        let mut hooks = crate::hook::HookRegistryBuilder::default();
        FeatureRegistryBuilder::new()
            .with_module(
                crate::feature::builtin::backend_job_result::BackendJobResultFeature::new(
                    workspace.clone(),
                )
                .with_surface(SubjektivSurfaceLifecycleFeature { task }),
            )
            .install_into_pending(&mut pending, &mut hooks);
        let (_, tool) = pending.remove(0)();
        let running_tool = tool.clone();
        let context = agen::tool::ToolExecutionContext::new("result-call", "batch", 0);
        let running_context = context.clone();
        let running = tokio::spawn(async move {
            running_tool
                .execute(
                    r#"{"result":{"subject_id":"subject-1","candidate_ids":[]}}"#,
                    running_context,
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), entered.notified())
            .await
            .unwrap();
        tool.cancel_execution(&context).await.unwrap();
        let outcome = tokio::time::timeout(Duration::from_secs(5), running)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(outcome, Err(agen::tool::ToolError::Cancelled(_))));
        assert_eq!(workspace.publish_calls.load(Ordering::SeqCst), 0);
        assert!(workspace.job_results.lock().unwrap().is_empty());
        assert!(workspace.failure_reasons.lock().unwrap().is_empty());
    }
    fn job_result_tool(workspace: Arc<SurfaceWorkspaceClient>) -> Arc<dyn agen::tool::Tool> {
        *workspace.failure_status.lock().unwrap() = "failed_confirmed".into();
        let mut pending = Vec::new();
        let mut hooks = crate::hook::HookRegistryBuilder::default();
        FeatureRegistryBuilder::new()
            .with_module(
                crate::feature::builtin::backend_job_result::BackendJobResultFeature::new(
                    workspace.clone(),
                )
                .with_surface(SubjektivSurfaceLifecycleFeature {
                    task: test_task(workspace, Arc::new(AtomicUsize::new(0))),
                }),
            )
            .install_into_pending(&mut pending, &mut hooks);
        pending.remove(0)().1
    }

    const CORRECT_RESULT: &str =
        r#"{"result":{"subject_id":"subject-1","candidate_ids":["candidate-1"]}}"#;
    const WRONG_RESULT: &str = r#"{"result":{"subject_id":"wrong-subject","candidate_ids":[]}}"#;

    fn rejection(status: u16) -> Result<WorkspaceResponse, WorkspaceClientError> {
        Ok(WorkspaceResponse {
            content_type: None,
            status,
            body: "explicit rejection before acceptance".into(),
        })
    }

    #[tokio::test]
    async fn job_result_definitive_rejection_allows_correction_and_reuses_current_ready_surface() {
        for status in [400, 403, 422] {
            let workspace = Arc::new(SurfaceWorkspaceClient::new(0));
            *workspace.current_fingerprint.lock().unwrap() = Some("memory-set-1".into());
            workspace
                .result_responses
                .lock()
                .unwrap()
                .push_back(rejection(status));
            let tool = job_result_tool(workspace.clone());
            assert!(
                tool.execute(WRONG_RESULT, agen::tool::ToolExecutionContext::direct())
                    .await
                    .is_err()
            );
            tool.execute(CORRECT_RESULT, agen::tool::ToolExecutionContext::direct())
                .await
                .unwrap();
            let results = workspace.job_results.lock().unwrap();
            assert_eq!(results.len(), 2);
            assert_ne!(
                results[0]["result"]["subject_id"],
                results[1]["result"]["subject_id"]
            );
            assert_eq!(
                results[0]["result"]["surface"],
                results[1]["result"]["surface"]
            );
            assert_eq!(
                workspace.prepare_calls.load(Ordering::SeqCst),
                2,
                "reuse must verify current Memory fingerprint"
            );
            assert_eq!(
                workspace.publish_calls.load(Ordering::SeqCst),
                1,
                "correction must not duplicate publication"
            );
            drop(results);
            assert!(
                tool.execute(WRONG_RESULT, agen::tool::ToolExecutionContext::direct())
                    .await
                    .is_err(),
                "accepted results stay sealed"
            );
        }
    }

    #[tokio::test]
    async fn job_result_rejection_rebuilds_conflicting_or_stale_surfaces() {
        for status in [400, 409] {
            let workspace = Arc::new(SurfaceWorkspaceClient::new(0));
            *workspace.current_fingerprint.lock().unwrap() = Some("memory-set-1".into());
            workspace
                .result_responses
                .lock()
                .unwrap()
                .push_back(rejection(status));
            let tool = job_result_tool(workspace.clone());
            assert!(
                tool.execute(CORRECT_RESULT, agen::tool::ToolExecutionContext::direct())
                    .await
                    .is_err()
            );
            *workspace.current_fingerprint.lock().unwrap() = Some("memory-set-2".into());
            tool.execute(CORRECT_RESULT, agen::tool::ToolExecutionContext::direct())
                .await
                .unwrap();
            let results = workspace.job_results.lock().unwrap();
            assert_eq!(results.len(), 2);
            assert_eq!(
                results[0]["result"]["surface"]["memory_fingerprint"],
                "memory-set-1"
            );
            assert_eq!(
                results[1]["result"]["surface"]["memory_fingerprint"],
                "memory-set-2"
            );
            assert_ne!(
                results[0]["result"]["surface"]["generation_id"],
                results[1]["result"]["surface"]["generation_id"]
            );
            assert_eq!(workspace.prepare_calls.load(Ordering::SeqCst), 2);
            assert_eq!(workspace.publish_calls.load(Ordering::SeqCst), 2);
        }
    }

    #[tokio::test]
    async fn job_result_ambiguous_acceptance_never_reopens_even_after_later_rejection() {
        for first in [
            Err(WorkspaceClientError::Request(
                "accepted but response lost".into(),
            )),
            rejection(500),
        ] {
            let workspace = Arc::new(SurfaceWorkspaceClient::new(0));
            workspace
                .result_responses
                .lock()
                .unwrap()
                .extend([first, rejection(403)]);
            let tool = job_result_tool(workspace.clone());
            assert!(
                tool.execute(CORRECT_RESULT, agen::tool::ToolExecutionContext::direct())
                    .await
                    .is_err()
            );
            assert!(
                tool.execute(WRONG_RESULT, agen::tool::ToolExecutionContext::direct())
                    .await
                    .is_err()
            );
            assert!(
                tool.execute(CORRECT_RESULT, agen::tool::ToolExecutionContext::direct())
                    .await
                    .is_err()
            );
            assert!(
                tool.execute(WRONG_RESULT, agen::tool::ToolExecutionContext::direct())
                    .await
                    .is_err()
            );
            tool.execute(CORRECT_RESULT, agen::tool::ToolExecutionContext::direct())
                .await
                .unwrap();
            let results = workspace.job_results.lock().unwrap();
            assert_eq!(results.len(), 3);
            assert_eq!(results[0], results[1]);
            assert_eq!(results[1], results[2]);
            assert_eq!(workspace.prepare_calls.load(Ordering::SeqCst), 1);
            assert_eq!(workspace.publish_calls.load(Ordering::SeqCst), 1);
        }
    }

    #[tokio::test]
    async fn job_result_never_submits_unconfirmed_failure_or_wrong_fingerprint() {
        for (status, fingerprint) in [
            ("failed", "memory-set-1"),
            ("ready", "memory-set-1"),
            ("stale", "memory-set-1"),
            ("failed_confirmed", "memory-set-2"),
        ] {
            let workspace = Arc::new(SurfaceWorkspaceClient::with_input_token_budget(1));
            *workspace.current_fingerprint.lock().unwrap() = Some("memory-set-1".into());
            let tool = job_result_tool(workspace.clone());
            *workspace.failure_status.lock().unwrap() = status.into();
            *workspace.failure_fingerprint.lock().unwrap() = Some(fingerprint.into());
            assert!(
                tool.execute(CORRECT_RESULT, agen::tool::ToolExecutionContext::direct())
                    .await
                    .is_err()
            );
            assert!(workspace.job_results.lock().unwrap().is_empty());
            // Nothing was dispatched/sealed: a real confirmed outcome remains retryable.
            *workspace.failure_status.lock().unwrap() = "failed_confirmed".into();
            *workspace.failure_fingerprint.lock().unwrap() = None;
            tool.execute(CORRECT_RESULT, agen::tool::ToolExecutionContext::direct())
                .await
                .unwrap();
            assert_eq!(workspace.job_results.lock().unwrap().len(), 1);
        }
    }
}
