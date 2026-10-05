use std::sync::Arc;
use std::time::Duration;

use agen::llm_client::LlmClient;
use arc_swap::ArcSwap;
use async_trait::async_trait;
use memory::extract;
use memory::schema::SourceRef;
use tokio::sync::broadcast;

use crate::PromptCatalog;
use crate::Scope;
use crate::WorkerRunResult;
use crate::feature::background::{
    BackgroundTaskCancellation, BackgroundTaskContext, BackgroundTaskSpec, BackgroundTaskTrigger,
    FeatureBackgroundTask,
};
use crate::feature::builtin::memory_staging_output::{
    MemoryStagingOutputFeature, MemoryStagingOutputState, render_extract_input,
};
use crate::feature::builtin::session_explore::{SessionExploreFeature, SessionExploreState};
use crate::feature::session::{
    CommittedRunExit, CommittedSessionCapture, CommittedSessionCaptureHandle,
    SessionExtensionHandle,
};
use crate::feature::{
    BackgroundTaskDeclaration, FeatureDescriptor, FeatureInstallContext, FeatureInstallError,
    FeatureModule, FeatureRegistryBuilder,
};
use crate::hook::{HookError, HookErrorCategory};
use crate::internal_worker::{
    InternalWorkerAuthority, InternalWorkerError, InternalWorkerIdentity, InternalWorkerResult,
    InternalWorkerSpec, run_internal_worker_with_cancel_sender,
};
use crate::session_capture::SessionCapture;
use crate::worker::{WorkerFilesystemAuthority, WorkerWorkspaceContext, WorkspaceClient};
use agen::token_counter::total_tokens_at;
use manifest::WorkerManifest;
use protocol::Event;

const TASK_NAME: &str = "memory-lifecycle";
const SUBJEKTIV_TASK_NAME: &str = "subjektiv-memory-lifecycle";
const SUBJEKTIV_EXTRACT_DOMAIN: &str = "subjektiv.extract.v1";
const TASK_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExtractionTarget {
    WorkspaceMemory,
    Subjektiv,
}

#[derive(Clone)]
struct LifecycleConfig {
    extraction: manifest::MemoryExtractionProfileConfig,
    consolidation_request_enabled: bool,
    workspace_settings: Option<manifest::WorkspaceMemorySettingsSnapshot>,
}

impl LifecycleConfig {
    fn from_memory(config: manifest::ResolvedMemoryFeatureConfig) -> Self {
        Self {
            extraction: config.profile.extraction,
            consolidation_request_enabled: config.profile.consolidation.request_enabled,
            workspace_settings: config.workspace_settings,
        }
    }

    fn from_subjektiv(config: manifest::ResolvedSubjektivFeatureConfig) -> Self {
        Self {
            extraction: config.profile.extraction,
            consolidation_request_enabled: true,
            workspace_settings: config.workspace_settings,
        }
    }

    fn workspace_settings(&self) -> Option<manifest::WorkspaceMemorySettingsSnapshot> {
        self.workspace_settings.clone()
    }
}

/// Parent-Worker lifecycle Feature that observes committed runs and schedules
/// bounded extraction work. It owns the Memory pointer, audit, restricted
/// Internal Worker, and staging disposition; Worker core owns only generic
/// hook/task/session plumbing.
#[derive(Clone)]
pub(crate) struct MemoryLifecycleFeature {
    task: MemoryLifecycleTask,
}

/// Subject-scoped extraction is a distinct Feature module even though it reuses
/// the mature committed-capture/extract/pointer lifecycle implementation.
#[derive(Clone)]
pub(crate) struct SubjektivLifecycleFeature {
    task: MemoryLifecycleTask,
}

#[derive(Clone)]
struct MemoryLifecycleTask {
    config: LifecycleConfig,
    target: ExtractionTarget,
    capture: CommittedSessionCaptureHandle,
    extensions: SessionExtensionHandle,
    workspace_client: Arc<dyn WorkspaceClient>,
    manifest: WorkerManifest,
    client: Box<dyn LlmClient>,
    prompts: Arc<ArcSwap<PromptCatalog>>,
    workspace_context: WorkerWorkspaceContext,
    event_tx: Option<broadcast::Sender<Event>>,
}

fn memory_lifecycle_requested(
    lifecycle_enabled: bool,
    config: &manifest::ResolvedMemoryFeatureConfig,
) -> bool {
    lifecycle_enabled
        && config.profile.enabled
        && (config.profile.extraction.enabled || config.profile.consolidation.request_enabled)
}

impl MemoryLifecycleFeature {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_resolved_config(
        lifecycle_enabled: bool,
        config: manifest::ResolvedMemoryFeatureConfig,
        capture: CommittedSessionCaptureHandle,
        extensions: SessionExtensionHandle,
        workspace_client: Arc<dyn WorkspaceClient>,
        manifest: WorkerManifest,
        client: Box<dyn LlmClient>,
        prompts: Arc<ArcSwap<PromptCatalog>>,
        workspace_context: WorkerWorkspaceContext,
        event_tx: Option<broadcast::Sender<Event>>,
    ) -> std::io::Result<Option<Self>> {
        if !memory_lifecycle_requested(lifecycle_enabled, &config) {
            return Ok(None);
        }
        config
            .validate_execution()
            .map_err(|message| std::io::Error::new(std::io::ErrorKind::InvalidInput, message))?;
        if !workspace_client.is_available() || workspace_client.workspace_id().is_none() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Memory extraction requires Backend Workspace API authority",
            ));
        }
        Ok(Some(Self::new(
            config,
            capture,
            extensions,
            workspace_client,
            manifest,
            client,
            prompts,
            workspace_context,
            event_tx,
        )))
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        config: manifest::ResolvedMemoryFeatureConfig,
        capture: CommittedSessionCaptureHandle,
        extensions: SessionExtensionHandle,
        workspace_client: Arc<dyn WorkspaceClient>,
        manifest: WorkerManifest,
        client: Box<dyn LlmClient>,
        prompts: Arc<ArcSwap<PromptCatalog>>,
        workspace_context: WorkerWorkspaceContext,
        event_tx: Option<broadcast::Sender<Event>>,
    ) -> Self {
        Self {
            task: MemoryLifecycleTask {
                config: LifecycleConfig::from_memory(config),
                target: ExtractionTarget::WorkspaceMemory,
                capture,
                extensions,
                workspace_client,
                manifest,
                client,
                prompts,
                workspace_context,
                event_tx,
            },
        }
    }
}

impl SubjektivLifecycleFeature {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_resolved_config(
        lifecycle_enabled: bool,
        config: manifest::ResolvedSubjektivFeatureConfig,
        capture: CommittedSessionCaptureHandle,
        extensions: SessionExtensionHandle,
        workspace_client: Arc<dyn WorkspaceClient>,
        manifest: WorkerManifest,
        client: Box<dyn LlmClient>,
        prompts: Arc<ArcSwap<PromptCatalog>>,
        workspace_context: WorkerWorkspaceContext,
        event_tx: Option<broadcast::Sender<Event>>,
    ) -> std::io::Result<Option<Self>> {
        if !lifecycle_enabled || !config.execution_enabled() || !config.profile.extraction.enabled {
            return Ok(None);
        }
        config
            .validate_execution()
            .map_err(|message| std::io::Error::new(std::io::ErrorKind::InvalidInput, message))?;
        if !workspace_client.is_available() || workspace_client.workspace_id().is_none() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "subjektiv extraction requires Backend Workspace API authority",
            ));
        }
        Ok(Some(Self {
            task: MemoryLifecycleTask {
                config: LifecycleConfig::from_subjektiv(config),
                target: ExtractionTarget::Subjektiv,
                capture,
                extensions,
                workspace_client,
                manifest,
                client,
                prompts,
                workspace_context,
                event_tx,
            },
        }))
    }
}

impl FeatureModule for MemoryLifecycleFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builtin("memory-lifecycle", "Memory Lifecycle")
            .with_description(
                "Observes terminal committed runs and schedules bounded Memory extraction and Backend consolidation requests.",
            )
            .with_background_task(BackgroundTaskDeclaration::worker_managed(
                TASK_NAME,
                "Extract provenance-preserving Memory candidates and request Backend consolidation after committed runs.",
            ))
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        context
            .background_tasks()
            .register(memory_lifecycle_task_spec(), self.task.clone())
    }
}

impl FeatureModule for SubjektivLifecycleFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builtin("subjektiv-memory-lifecycle", "subjektiv Memory Lifecycle")
            .with_description(
                "Observes terminal committed parent runs and stages bounded Memory candidates for the host-connected subject.",
            )
            .with_background_task(BackgroundTaskDeclaration::worker_managed(
                SUBJEKTIV_TASK_NAME,
                "Extract provenance-preserving candidates into subject-scoped staging after committed runs.",
            ))
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        context
            .background_tasks()
            .register(subjektiv_lifecycle_task_spec(), self.task.clone())
    }
}

fn memory_lifecycle_task_spec() -> BackgroundTaskSpec {
    lifecycle_task_spec(
        TASK_NAME,
        "Extract provenance-preserving Memory candidates and request Backend consolidation after committed runs.",
    )
}

fn subjektiv_lifecycle_task_spec() -> BackgroundTaskSpec {
    lifecycle_task_spec(
        SUBJEKTIV_TASK_NAME,
        "Extract provenance-preserving candidates into subject-scoped staging after committed runs.",
    )
}

fn lifecycle_task_spec(name: &'static str, description: &'static str) -> BackgroundTaskSpec {
    let declaration = BackgroundTaskDeclaration::worker_managed(name, description);
    let mut spec = BackgroundTaskSpec::single_flight(declaration, TASK_TIMEOUT);
    spec.trigger = BackgroundTaskTrigger::RunCommitted;
    spec
}

impl MemoryLifecycleTask {
    fn pointer_domain(&self) -> &'static str {
        match self.target {
            ExtractionTarget::WorkspaceMemory => extract::EXTRACT_DOMAIN,
            ExtractionTarget::Subjektiv => SUBJEKTIV_EXTRACT_DOMAIN,
        }
    }

    fn extraction_manifest(&self) -> WorkerManifest {
        let mut manifest = self.manifest.clone();
        if let Some(model) = self.config.extraction.model.clone() {
            manifest.model = model;
        }
        if let Some(reasoning) = self.config.extraction.reasoning.clone() {
            manifest.engine.reasoning = Some(reasoning);
        }
        manifest
    }

    async fn run_extraction(
        &self,
        context: BackgroundTaskContext,
        cancellation: BackgroundTaskCancellation,
    ) -> Result<(), HookError> {
        context.generation_fence.ensure_current()?;
        let audit = WorkerAuditBase::new(
            memory::audit::AuditWorker::MemoryExtract,
            memory::audit::AuditTrigger::TokenThreshold,
            self.config
                .extraction
                .model
                .as_ref()
                .or(Some(&self.manifest.model))
                .map(model_audit_from_manifest),
        )
        .with_workspace_settings(self.config.workspace_settings.as_ref());
        let audit = if self.target == ExtractionTarget::Subjektiv {
            audit.without_legacy_backend()
        } else {
            audit
        };
        let capture = match self.capture.capture() {
            Ok(capture) => capture,
            Err(error) => {
                audit
                    .emit(
                        self.workspace_client.as_ref(),
                        self.event_tx.as_ref(),
                        memory::audit::WorkerLifecycleStatus::Failed,
                        format!("committed_session_capture_failed: {error}"),
                        None,
                        None,
                        None,
                    )
                    .await;
                return Ok(());
            }
        };
        if !extraction_run_eligible(capture.run_exit) {
            audit
                .emit(
                    self.workspace_client.as_ref(),
                    self.event_tx.as_ref(),
                    memory::audit::WorkerLifecycleStatus::Skipped,
                    format!("parent_run_not_finished: {:?}", capture.run_exit),
                    None,
                    None,
                    None,
                )
                .await;
            return Ok(());
        }
        let pointer = match extract_pointer(&capture, self.pointer_domain()) {
            Ok(pointer) => pointer,
            Err(error) => {
                audit
                    .emit(
                        self.workspace_client.as_ref(),
                        self.event_tx.as_ref(),
                        memory::audit::WorkerLifecycleStatus::Failed,
                        format!("extract_pointer_invalid: {error}"),
                        None,
                        None,
                        None,
                    )
                    .await;
                return Ok(());
            }
        };
        let Some(threshold) = self
            .config
            .extraction
            .threshold
            .filter(|threshold| *threshold > 0)
        else {
            audit
                .emit(
                    self.workspace_client.as_ref(),
                    self.event_tx.as_ref(),
                    memory::audit::WorkerLifecycleStatus::Skipped,
                    "token_threshold_disabled",
                    None,
                    None,
                    None,
                )
                .await;
            return Ok(());
        };
        let tokens_since = tokens_since_pointer(&capture, pointer.as_ref());
        if tokens_since < threshold {
            audit
                .emit(
                    self.workspace_client.as_ref(),
                    self.event_tx.as_ref(),
                    memory::audit::WorkerLifecycleStatus::Skipped,
                    format!(
                        "token_threshold_not_reached tokens_since={tokens_since} threshold={threshold}"
                    ),
                    None,
                    None,
                    None,
                )
                .await;
            return Ok(());
        }

        let history_start = pointer
            .as_ref()
            .map(|pointer| pointer.processed_through_history_len)
            .unwrap_or(0)
            .min(capture.history.len());
        let history_end = capture.history.len();
        if history_start >= history_end || capture.entry_count == 0 {
            audit
                .emit(
                    self.workspace_client.as_ref(),
                    self.event_tx.as_ref(),
                    memory::audit::WorkerLifecycleStatus::Skipped,
                    "no_new_committed_session_entries",
                    None,
                    Some(memory::audit::ExtractAudit {
                        session_id: Some(capture.session_id.clone()),
                        segment_id: Some(capture.segment_id.clone()),
                        history_range: Some([history_start as u64, history_end as u64]),
                        ..Default::default()
                    }),
                    None,
                )
                .await;
            return Ok(());
        }
        let view = SessionCapture::from_history_entries(
            capture.segment_id.clone(),
            capture.history[history_start..history_end].to_vec(),
        );
        let start_entry = pointer
            .as_ref()
            .map(|pointer| pointer.processed_through_entry + 1)
            .unwrap_or(0);
        let source = SourceRef {
            segment_id: capture.segment_id.clone(),
            range: [start_entry as u64, (capture.entry_count - 1) as u64],
        };
        let extract_audit_base = memory::audit::ExtractAudit {
            session_id: Some(capture.session_id.clone()),
            segment_id: Some(capture.segment_id.clone()),
            entry_range: Some([start_entry as u64, (capture.entry_count - 1) as u64]),
            history_range: Some([history_start as u64, history_end as u64]),
            ..Default::default()
        };
        audit
            .emit(
                self.workspace_client.as_ref(),
                self.event_tx.as_ref(),
                memory::audit::WorkerLifecycleStatus::Started,
                "token_threshold_reached",
                None,
                Some(extract_audit_base.clone()),
                None,
            )
            .await;
        let output_state = match self.target {
            ExtractionTarget::WorkspaceMemory => MemoryStagingOutputState::new(
                view.clone(),
                Arc::clone(&self.workspace_client),
                source,
                audit.run_id.to_string(),
            ),
            ExtractionTarget::Subjektiv => MemoryStagingOutputState::new_subjektiv(
                view.clone(),
                Arc::clone(&self.workspace_client),
                source,
                audit.run_id.to_string(),
                capture.session_id.clone(),
            ),
        };
        let client = if let Some(model) = self.config.extraction.model.as_ref() {
            match crate::model_client::build_client(model) {
                Ok(client) => client,
                Err(error) => {
                    self.record_preparation_failure(&audit, &extract_audit_base, error.to_string())
                        .await;
                    return Ok(());
                }
            }
        } else {
            self.client.clone_boxed()
        };
        let Some(memory_language) = self
            .config
            .workspace_settings()
            .map(|snapshot| snapshot.language)
        else {
            self.record_preparation_failure(
                &audit,
                &extract_audit_base,
                "Memory extraction requires a bound Workspace Memory settings snapshot",
            )
            .await;
            return Ok(());
        };
        let system_prompt = match self
            .prompts
            .load_full()
            .memory_extract_system(&memory_language)
        {
            Ok(prompt) => prompt,
            Err(error) => {
                self.record_preparation_failure(&audit, &extract_audit_base, error.to_string())
                    .await;
                return Ok(());
            }
        };
        let manifest = self.extraction_manifest();

        let cancellation_observer = cancellation.clone();
        let cancel_observer = move |sender: tokio::sync::mpsc::Sender<()>| {
            tokio::spawn(async move {
                cancellation_observer.cancelled().await;
                let _ = sender.send(()).await;
            });
        };
        let features = FeatureRegistryBuilder::new()
            .with_module(SessionExploreFeature::new(SessionExploreState::new(
                view.clone(),
            )))
            .with_module(MemoryStagingOutputFeature::new(output_state.clone()));
        let result = run_internal_worker_with_cancel_sender(
            InternalWorkerSpec {
                identity: InternalWorkerIdentity {
                    kind: "memory-extract",
                    run_id: audit.run_id,
                },
                manifest,
                client,
                system_prompt,
                input: render_extract_input(&view),
                cache_key: Some(capture.segment_id.clone()),
                max_turns: self
                    .config
                    .extraction
                    .worker_max_turns
                    .or(manifest::defaults::MEMORY_EXTRACT_WORKER_MAX_TURNS),
                engine_configurator: None,
                features,
                required_tools: &[
                    "ShowOverview",
                    "SearchEntries",
                    "ReadEntry",
                    "StageMemoryCandidate",
                    "FinishMemoryExtraction",
                ],
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

        let usage_event = match &result {
            Ok(run) => {
                tracing::debug!(
                    worker_kind = run.identity.kind,
                    run_id = %run.identity.run_id,
                    history_entries = run.history_entries,
                    "memory extraction Internal Worker completed"
                );
                run.usage.as_ref()
            }
            Err(error) => error.usage.as_ref(),
        };
        let usage_audit = usage_event.map(|event| memory::audit::UsageAudit {
            input_tokens: event.input_tokens,
            output_tokens: event.output_tokens,
            total_tokens: event.total_tokens,
            cache_read_input_tokens: event.cache_read_input_tokens,
            cache_creation_input_tokens: event.cache_creation_input_tokens,
        });
        let staging_ids = output_state.staged();
        let pointer_staging_id = staging_ids.first().cloned().unwrap_or_default();
        let extract_audit = Some(memory::audit::ExtractAudit {
            staging_count: staging_ids.len(),
            staging_paths: staging_ids,
            ..extract_audit_base
        });

        match extraction_disposition(&result, output_state.is_finished()) {
            ExtractionDisposition::Cancelled(reason) => {
                audit
                    .emit(
                        self.workspace_client.as_ref(),
                        self.event_tx.as_ref(),
                        memory::audit::WorkerLifecycleStatus::Cancelled,
                        reason,
                        usage_audit,
                        extract_audit,
                        None,
                    )
                    .await;
                return Ok(());
            }
            ExtractionDisposition::Failed(reason) => {
                audit
                    .emit(
                        self.workspace_client.as_ref(),
                        self.event_tx.as_ref(),
                        memory::audit::WorkerLifecycleStatus::Failed,
                        reason,
                        usage_audit,
                        extract_audit,
                        None,
                    )
                    .await;
                return Ok(());
            }
            ExtractionDisposition::Completed => {}
        }

        if cancellation.is_cancelled() {
            audit
                .emit(
                    self.workspace_client.as_ref(),
                    self.event_tx.as_ref(),
                    memory::audit::WorkerLifecycleStatus::Cancelled,
                    "memory-extract cancelled before completion side effects",
                    usage_audit,
                    extract_audit,
                    None,
                )
                .await;
            return Ok(());
        }

        context.generation_fence.ensure_current()?;
        let next_pointer = memory::ExtractPointerPayload {
            processed_through_entry: capture.entry_count - 1,
            processed_through_history_len: capture.history.len(),
            staging_id: pointer_staging_id,
        };
        let payload = serde_json::to_value(&next_pointer).map_err(hook_internal)?;
        if cancellation.is_cancelled() {
            audit
                .emit(
                    self.workspace_client.as_ref(),
                    self.event_tx.as_ref(),
                    memory::audit::WorkerLifecycleStatus::Cancelled,
                    "memory-extract cancelled before pointer commit",
                    usage_audit,
                    extract_audit,
                    None,
                )
                .await;
            return Ok(());
        }
        if !self
            .extensions
            .append_if_current(&capture.location(), self.pointer_domain(), payload)
            .map_err(hook_internal)?
        {
            audit
                .emit(
                    self.workspace_client.as_ref(),
                    self.event_tx.as_ref(),
                    memory::audit::WorkerLifecycleStatus::Cancelled,
                    "session changed before memory-extract pointer commit",
                    usage_audit,
                    extract_audit,
                    None,
                )
                .await;
            return Ok(());
        }
        audit
            .emit(
                self.workspace_client.as_ref(),
                self.event_tx.as_ref(),
                memory::audit::WorkerLifecycleStatus::Completed,
                "memory-extract completed",
                usage_audit,
                extract_audit,
                None,
            )
            .await;
        Ok(())
    }
}

#[async_trait]
impl FeatureBackgroundTask for MemoryLifecycleTask {
    async fn run(
        &self,
        context: BackgroundTaskContext,
        cancellation: BackgroundTaskCancellation,
    ) -> Result<(), HookError> {
        let extraction = if self.config.extraction.enabled {
            self.run_extraction(context.clone(), cancellation.clone())
                .await
        } else {
            Ok(())
        };
        if !cancellation.is_cancelled() {
            context.generation_fence.ensure_current()?;
            match self.target {
                ExtractionTarget::WorkspaceMemory if self.config.consolidation_request_enabled => {
                    self.request_consolidation(false).await;
                }
                ExtractionTarget::Subjektiv => {
                    self.request_consolidation(true).await;
                }
                ExtractionTarget::WorkspaceMemory => {}
            }
        }
        extraction
    }
}

impl MemoryLifecycleTask {
    async fn request_consolidation(&self, subjektiv: bool) {
        let audit = WorkerAuditBase::new(
            memory::audit::AuditWorker::MemoryConsolidation,
            memory::audit::AuditTrigger::StagingBacklog,
            Some(model_audit_from_manifest(&self.manifest.model)),
        )
        .with_workspace_settings(self.config.workspace_settings.as_ref());
        let operation = memory::backend::MemoryConsolidateStagingOperation { force: false };
        let request = if subjektiv {
            self.workspace_client
                .request_subjektiv_memory_staging_consolidation(operation)
                .await
        } else {
            self.workspace_client
                .request_memory_staging_consolidation(operation)
                .await
        };
        match request {
            Ok(output) => {
                tracing::debug!(
                    status = output.status.as_str(),
                    summary = output.summary.as_str(),
                    "requested Backend Memory staging consolidation"
                );
            }
            Err(error) => {
                tracing::warn!(%error, "request Backend Memory staging consolidation failed");
                audit
                    .emit(
                        self.workspace_client.as_ref(),
                        self.event_tx.as_ref(),
                        memory::audit::WorkerLifecycleStatus::Skipped,
                        "consolidation_backend_operation_failed",
                        None,
                        None,
                        None,
                    )
                    .await;
            }
        }
    }

    async fn record_preparation_failure(
        &self,
        audit: &WorkerAuditBase,
        extract: &memory::audit::ExtractAudit,
        reason: impl Into<String>,
    ) {
        audit
            .emit(
                self.workspace_client.as_ref(),
                self.event_tx.as_ref(),
                memory::audit::WorkerLifecycleStatus::Failed,
                reason,
                None,
                Some(extract.clone()),
                None,
            )
            .await;
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ExtractionDisposition {
    Completed,
    Failed(String),
    Cancelled(String),
}

fn extraction_disposition(
    result: &Result<InternalWorkerResult, InternalWorkerError>,
    finish_called: bool,
) -> ExtractionDisposition {
    match result {
        Err(error) => {
            // Preserve the Internal Worker result's immutable identity/history
            // evidence for diagnostics even though the public audit reason is
            // intentionally bounded to the typed source error.
            tracing::debug!(
                worker_kind = error.identity.kind,
                run_id = %error.identity.run_id,
                history_entries = error.history_entries,
                "memory extraction Internal Worker failed"
            );
            ExtractionDisposition::Failed(error.source.to_string())
        }
        Ok(run) => match &run.lifecycle {
            WorkerRunResult::Cancelled | WorkerRunResult::RolledBack => {
                ExtractionDisposition::Cancelled("memory-extract cancelled".to_string())
            }
            WorkerRunResult::Interrupted { message, .. } => {
                ExtractionDisposition::Failed(message.clone())
            }
            WorkerRunResult::Finished | WorkerRunResult::Paused | WorkerRunResult::LimitReached
                if finish_called =>
            {
                ExtractionDisposition::Completed
            }
            WorkerRunResult::Finished | WorkerRunResult::Paused | WorkerRunResult::LimitReached => {
                ExtractionDisposition::Failed(
                    "memory-extract did not call FinishMemoryExtraction".to_string(),
                )
            }
        },
    }
}

fn now_millis() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn hook_internal(error: impl std::fmt::Display) -> HookError {
    HookError::new(HookErrorCategory::Internal, error.to_string())
}

fn extract_pointer(
    capture: &CommittedSessionCapture,
    domain: &str,
) -> Result<Option<memory::ExtractPointerPayload>, HookError> {
    let pointer = capture
        .extensions
        .iter()
        .rev()
        .find(|(candidate, _)| candidate == domain)
        .and_then(|(_, payload)| serde_json::from_value(payload.clone()).ok());
    if pointer.is_none()
        && capture
            .extensions
            .iter()
            .any(|(candidate, _)| candidate == domain)
    {
        return Err(hook_internal(
            "latest committed Memory extraction pointer is malformed",
        ));
    }
    Ok(pointer)
}

fn extraction_run_eligible(exit: CommittedRunExit) -> bool {
    exit == CommittedRunExit::Finished
}

fn tokens_since_pointer(
    capture: &CommittedSessionCapture,
    pointer: Option<&memory::ExtractPointerPayload>,
) -> u64 {
    let history_pointer = pointer
        .map(|pointer| pointer.processed_through_history_len)
        .unwrap_or(0)
        .min(capture.history.len());
    let items = capture
        .history
        .iter()
        .map(|entry| entry.item.clone())
        .collect::<Vec<_>>();
    let current = total_tokens_at(&items, &capture.usage_history, capture.history.len()).tokens;
    let baseline = total_tokens_at(&items, &capture.usage_history, history_pointer).tokens;
    current.saturating_sub(baseline)
}

#[cfg(test)]
fn extraction_threshold_reached(
    capture: &CommittedSessionCapture,
    pointer: Option<&memory::ExtractPointerPayload>,
    config: &manifest::ResolvedMemoryFeatureConfig,
) -> bool {
    if capture.history.is_empty() {
        return false;
    }
    let Some(threshold) = config
        .profile
        .extraction
        .threshold
        .filter(|threshold| *threshold > 0)
    else {
        return false;
    };
    tokens_since_pointer(capture, pointer) >= threshold
}

#[derive(Clone)]
struct WorkerAuditBase {
    run_id: uuid::Uuid,
    worker: memory::audit::AuditWorker,
    trigger: memory::audit::AuditTrigger,
    memory_settings: Option<memory::audit::MemorySettingsAudit>,
    model: Option<memory::audit::ModelAudit>,
    persist_legacy_backend: bool,
}

impl WorkerAuditBase {
    fn new(
        worker: memory::audit::AuditWorker,
        trigger: memory::audit::AuditTrigger,
        model: Option<memory::audit::ModelAudit>,
    ) -> Self {
        Self {
            run_id: uuid::Uuid::now_v7(),
            worker,
            trigger,
            memory_settings: None,
            model,
            persist_legacy_backend: true,
        }
    }

    fn without_legacy_backend(mut self) -> Self {
        self.persist_legacy_backend = false;
        self
    }

    fn with_workspace_settings(
        mut self,
        settings: Option<&manifest::WorkspaceMemorySettingsSnapshot>,
    ) -> Self {
        self.memory_settings = settings.map(|snapshot| memory::audit::MemorySettingsAudit {
            workspace_id: snapshot.workspace_id.clone(),
            settings_revision: snapshot.settings_revision,
            language: snapshot.language.clone(),
        });
        self
    }

    async fn emit(
        &self,
        workspace_client: &dyn WorkspaceClient,
        event_tx: Option<&broadcast::Sender<Event>>,
        status: memory::audit::WorkerLifecycleStatus,
        reason: impl Into<String>,
        usage: Option<memory::audit::UsageAudit>,
        extract: Option<memory::audit::ExtractAudit>,
        consolidation: Option<memory::audit::ConsolidationAudit>,
    ) {
        let reason = reason.into();
        let payload = memory::audit::WorkerLifecycleAudit {
            run_id: self.run_id,
            worker: self.worker.clone(),
            status,
            trigger: self.trigger,
            reason: reason.clone(),
            memory_settings: self.memory_settings.clone(),
            model: self.model.clone(),
            usage,
            extract,
            consolidation,
        };
        if self.persist_legacy_backend {
            let _ = workspace_client
                .execute_memory_backend_operation(
                    memory::backend::MemoryBackendOperation::AppendAudit(
                        memory::backend::MemoryAppendAuditOperation {
                            event: memory::audit::AuditEvent::new(
                                memory::audit::AuditPayload::WorkerLifecycle(payload),
                            ),
                        },
                    ),
                )
                .await;
        }
        if let Some(tx) = event_tx {
            let _ = tx.send(Event::MemoryWorker(protocol::MemoryWorkerEvent {
                worker: self.worker.label().to_string(),
                status: status.label().to_string(),
                run_id: self.run_id.to_string(),
                trigger: self.trigger.label().to_string(),
                reason: reason.clone(),
                message: format!(
                    "memory {} {}: {reason}",
                    self.worker.label(),
                    status.label()
                ),
                timestamp_ms: now_millis() as i64,
            }));
        }
    }
}

fn model_audit_from_manifest(model: &manifest::ModelManifest) -> memory::audit::ModelAudit {
    memory::audit::ModelAudit {
        ref_: model.ref_.clone(),
        scheme: model.scheme.map(|scheme| format!("{scheme:?}")),
        model_id: model.model_id.clone(),
    }
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use agen::llm_client::event::{Event as LlmEvent, ResponseStatus, StatusEvent};
    use agen::llm_client::{ClientError, Request};
    use agen::{HistoryEntry, Item, UsageRecord};
    use futures::Stream;

    use super::*;
    use crate::feature::background::FeatureBackgroundTaskRegistryBuilder;
    use crate::feature::background::{BackgroundTaskRewritePolicy, BackgroundTaskShutdownPolicy};
    use crate::feature::session::CommittedSessionLocation;
    use crate::hook::HookInvocationContext;
    use crate::session_history::SessionHistoryMetadata;

    #[derive(Debug, Default)]
    struct RecordingWorkspaceClient {
        requests: Mutex<Vec<crate::worker::WorkspaceRequest>>,
    }

    impl WorkspaceClient for RecordingWorkspaceClient {
        fn workspace_id(&self) -> Option<&str> {
            Some("workspace-1")
        }

        fn kind(&self) -> &str {
            "memory-lifecycle-test"
        }

        fn is_available(&self) -> bool {
            true
        }

        fn execute(
            &self,
            request: crate::worker::WorkspaceRequest,
        ) -> Result<crate::worker::WorkspaceResponse, crate::worker::WorkspaceClientError> {
            let is_subjektiv_stage = request.path.ends_with("/subjektiv/staging");
            let is_subjektiv_consolidation = request.path.ends_with("/subjektiv/consolidation");
            let is_stage_candidate = request
                .body
                .as_deref()
                .is_some_and(|body| body.contains("stage_candidate"));
            let is_append_audit = request
                .body
                .as_deref()
                .is_some_and(|body| body.contains("append_audit"));
            self.requests.lock().unwrap().push(request);
            if is_subjektiv_consolidation {
                return Ok(crate::worker::WorkspaceResponse {
                    status: 200,
                    body: serde_json::to_string(&server_api::MemoryConsolidationResponse {
                        status: "started".into(),
                        summary: "subject consolidation requested".into(),
                        candidate_count: 1,
                        total_bytes: 1,
                    })
                    .unwrap(),
                });
            }
            if is_subjektiv_stage {
                return Ok(crate::worker::WorkspaceResponse {
                    status: 200,
                    body: serde_json::to_string(&server_api::SubjektivStageCandidateResponse {
                        staging_id: "subject-candidate-1".to_string(),
                    })
                    .unwrap(),
                });
            }
            if is_stage_candidate {
                return Ok(crate::worker::WorkspaceResponse {
                    status: 200,
                    body: serde_json::to_string(&memory::backend::MemoryBackendHttpResponse::Ok {
                        result: memory::backend::MemoryBackendOperationResult::StagingWritten(
                            memory::backend::MemoryStagingWriteOutput {
                                staging_count: 1,
                                staging_ids: vec!["candidate-1".to_string()],
                            },
                        ),
                    })
                    .unwrap(),
                });
            }
            if is_append_audit {
                return Ok(crate::worker::WorkspaceResponse {
                    status: 200,
                    body: serde_json::to_string(&memory::backend::MemoryBackendHttpResponse::Ok {
                        result: memory::backend::MemoryBackendOperationResult::Acknowledged(
                            memory::backend::MemoryBackendAckOutput {
                                summary: "audit recorded".to_string(),
                            },
                        ),
                    })
                    .unwrap(),
                });
            }
            Err(crate::worker::WorkspaceClientError::Unavailable(
                "recording client".to_string(),
            ))
        }
    }

    #[derive(Clone)]
    struct PendingClient {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl LlmClient for PendingClient {
        fn clone_boxed(&self) -> Box<dyn LlmClient> {
            Box::new(self.clone())
        }

        async fn stream(
            &self,
            _request: Request,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent, ClientError>> + Send>>, ClientError>
        {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(Box::pin(futures::stream::pending()))
        }
    }

    #[derive(Clone)]
    struct ScriptClient {
        responses: Arc<Vec<Vec<LlmEvent>>>,
        calls: Arc<AtomicUsize>,
    }

    impl ScriptClient {
        fn new(responses: Vec<Vec<LlmEvent>>) -> Self {
            Self {
                responses: Arc::new(responses),
                calls: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    #[async_trait]
    impl LlmClient for ScriptClient {
        fn clone_boxed(&self) -> Box<dyn LlmClient> {
            Box::new(self.clone())
        }

        async fn stream(
            &self,
            _request: Request,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent, ClientError>> + Send>>, ClientError>
        {
            let index = self.calls.fetch_add(1, Ordering::SeqCst);
            let events = self.responses.get(index).cloned().ok_or_else(|| {
                ClientError::Config("memory lifecycle test client exhausted".to_string())
            })?;
            Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
        }
    }

    fn stage_candidate_events(call_id: &str, entry_ref: &str) -> Vec<LlmEvent> {
        vec![
            LlmEvent::tool_use_start(0, call_id, "StageMemoryCandidate"),
            LlmEvent::tool_input_delta(
                0,
                serde_json::json!({
                    "kind": "decision",
                    "claim": "Keep lifecycle work feature-owned.",
                    "why_useful": "Prevents Worker core coupling.",
                    "entry_refs": [entry_ref]
                })
                .to_string(),
            ),
            LlmEvent::tool_use_stop(0),
            LlmEvent::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }),
        ]
    }

    fn finish_events(call_id: &str, staged_count: usize) -> Vec<LlmEvent> {
        vec![
            LlmEvent::tool_use_start(0, call_id, "FinishMemoryExtraction"),
            LlmEvent::tool_input_delta(
                0,
                serde_json::json!({"staged_count": staged_count}).to_string(),
            ),
            LlmEvent::tool_use_stop(0),
            LlmEvent::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }),
        ]
    }

    fn finish_empty_events(call_id: &str) -> Vec<LlmEvent> {
        vec![
            LlmEvent::tool_use_start(0, call_id, "FinishMemoryExtraction"),
            LlmEvent::tool_input_delta(
                0,
                serde_json::json!({
                    "staged_count": 0,
                    "no_candidates_reason": "no durable candidates"
                })
                .to_string(),
            ),
            LlmEvent::tool_use_stop(0),
            LlmEvent::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }),
        ]
    }

    fn completed_events() -> Vec<LlmEvent> {
        vec![
            LlmEvent::text_block_start(0),
            LlmEvent::text_delta(0, "done"),
            LlmEvent::text_block_stop(0, None),
            LlmEvent::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }),
        ]
    }

    fn test_manifest() -> WorkerManifest {
        WorkerManifest::from_toml(
            r#"
[worker]
name = "memory-lifecycle-test"
scope = "main"

[model]
scheme = "anthropic"
model_id = "test-model"

[engine]

[[scope.allow]]
target = "/memory-lifecycle-test"
permission = "write"
"#,
        )
        .unwrap()
    }

    fn test_config() -> manifest::ResolvedMemoryFeatureConfig {
        let mut config = manifest::ResolvedMemoryFeatureConfig::default();
        config.profile.enabled = true;
        config.profile.extraction.enabled = true;
        config.profile.extraction.threshold = Some(1);
        config
            .bind_workspace_settings(manifest::WorkspaceMemorySettingsSnapshot {
                workspace_id: "workspace-1".to_string(),
                settings_revision: 1,
                language: "English".to_string(),
            })
            .unwrap();
        config
    }

    #[test]
    fn resolved_subjektiv_consolidation_profile_does_not_install_global_memory_lifecycle() {
        let workspace = tempfile::tempdir().unwrap();
        let resolved = manifest::ProfileResolver::new()
            .with_workspace_base(workspace.path())
            .resolve(
                &manifest::ProfileSelector::source_named(
                    manifest::ProfileRegistrySource::Builtin,
                    "subjektiv-memory-consolidation",
                ),
                manifest::ProfileResolveOptions::with_worker_name("subject-consolidator"),
            )
            .unwrap();

        assert!(!memory_lifecycle_requested(
            true,
            &resolved.manifest.feature.memory
        ));
    }

    fn test_task(
        capture: CommittedSessionCapture,
        client: Box<dyn LlmClient>,
        extension_writes: Arc<Mutex<Vec<(CommittedSessionLocation, String, serde_json::Value)>>>,
        event_tx: broadcast::Sender<Event>,
        workspace_client: Arc<dyn WorkspaceClient>,
    ) -> MemoryLifecycleTask {
        let capture_handle = CommittedSessionCaptureHandle::new(move || Ok(capture.clone()));
        let extensions = SessionExtensionHandle::new(move |location, domain, payload| {
            extension_writes
                .lock()
                .unwrap()
                .push((location.clone(), domain.to_string(), payload));
            Ok(true)
        });
        MemoryLifecycleTask {
            config: LifecycleConfig::from_memory(test_config()),
            target: ExtractionTarget::WorkspaceMemory,
            capture: capture_handle,
            extensions,
            workspace_client,
            manifest: test_manifest(),
            client,
            prompts: Arc::new(ArcSwap::from(PromptCatalog::builtins_only().unwrap())),
            workspace_context: WorkerWorkspaceContext::no_workspace(),
            event_tx: Some(event_tx),
        }
    }

    fn start_background_task(
        task: MemoryLifecycleTask,
    ) -> crate::feature::background::FeatureBackgroundTaskRegistry {
        let mut builder = FeatureBackgroundTaskRegistryBuilder::default();
        builder
            .register(
                crate::feature::FeatureId::builtin("memory-lifecycle"),
                memory_lifecycle_task_spec(),
                task,
            )
            .unwrap();
        let registry = builder.build();
        registry
            .start_run_committed(HookInvocationContext {
                worker_id: "worker-1".to_string(),
                session_id: "session-1".to_string(),
                session_revision: 1,
                run_id: Some("run-1".to_string()),
                ..Default::default()
            })
            .unwrap();
        registry
    }

    async fn run_background_task(task: MemoryLifecycleTask) {
        let registry = start_background_task(task);
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if !registry.diagnostics().is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("memory lifecycle background task should finish");
        registry.shutdown().await.unwrap();
    }

    #[test]
    fn disabled_subjektiv_feature_installs_nothing_without_workspace_side_effects() {
        let capture = CommittedSessionCaptureHandle::new(|| {
            panic!("disabled subjektiv must not capture a Session")
        });
        let extensions = SessionExtensionHandle::new(|_, _, _| {
            panic!("disabled subjektiv must not write a pointer")
        });
        let feature = SubjektivLifecycleFeature::from_resolved_config(
            true,
            manifest::ResolvedSubjektivFeatureConfig::default(),
            capture,
            extensions,
            crate::worker::marker_workspace_client(None, "disabled"),
            test_manifest(),
            Box::new(ScriptClient::new(Vec::new())),
            Arc::new(ArcSwap::from(PromptCatalog::builtins_only().unwrap())),
            WorkerWorkspaceContext::no_workspace(),
            None,
        )
        .unwrap();
        assert!(feature.is_none());
    }

    #[test]
    fn subjektiv_without_extraction_installs_no_session_lifecycle() {
        let capture = CommittedSessionCaptureHandle::new(|| {
            panic!("subjektiv without extraction must not capture a Session")
        });
        let extensions = SessionExtensionHandle::new(|_, _, _| {
            panic!("subjektiv without extraction must not write a pointer")
        });
        let mut config = manifest::ResolvedSubjektivFeatureConfig::default();
        config.profile.enabled = true;
        config.profile.extraction.enabled = false;

        let feature = SubjektivLifecycleFeature::from_resolved_config(
            true,
            config,
            capture,
            extensions,
            crate::worker::marker_workspace_client(None, "extraction-disabled"),
            test_manifest(),
            Box::new(ScriptClient::new(Vec::new())),
            Arc::new(ArcSwap::from(PromptCatalog::builtins_only().unwrap())),
            WorkerWorkspaceContext::no_workspace(),
            None,
        )
        .unwrap();

        assert!(feature.is_none());
    }

    #[test]
    fn child_host_suppresses_enabled_subjektiv_lifecycle_without_capture_side_effects() {
        let capture = CommittedSessionCaptureHandle::new(|| {
            panic!("a child Worker must not capture for an inherited subjektiv profile")
        });
        let extensions = SessionExtensionHandle::new(|_, _, _| {
            panic!("a child Worker must not write a subjektiv extraction pointer")
        });
        let mut config = manifest::ResolvedSubjektivFeatureConfig::default();
        config.profile.enabled = true;
        config.profile.extraction.enabled = true;
        config.profile.extraction.threshold = Some(1);
        config
            .bind_workspace_settings(manifest::WorkspaceMemorySettingsSnapshot {
                workspace_id: "workspace-1".to_string(),
                settings_revision: 1,
                language: "English".to_string(),
            })
            .unwrap();

        let feature = SubjektivLifecycleFeature::from_resolved_config(
            false,
            config,
            capture,
            extensions,
            crate::worker::marker_workspace_client(None, "child-disabled"),
            test_manifest(),
            Box::new(ScriptClient::new(Vec::new())),
            Arc::new(ArcSwap::from(PromptCatalog::builtins_only().unwrap())),
            WorkerWorkspaceContext::no_workspace(),
            None,
        )
        .unwrap();

        assert!(feature.is_none());
    }

    #[test]
    fn extraction_manifest_applies_memory_model_and_reasoning_overrides() {
        use agen::llm_client::capability::{ReasoningControl, ReasoningEffort};

        let client = ScriptClient::new(Vec::new());
        let extension_writes = Arc::new(Mutex::new(Vec::new()));
        let (event_tx, _) = broadcast::channel(16);
        let workspace_client: Arc<dyn WorkspaceClient> =
            Arc::new(RecordingWorkspaceClient::default());
        let mut task = test_task(
            capture(2, 250),
            Box::new(client),
            extension_writes,
            event_tx,
            workspace_client,
        );
        task.config.extraction.model = Some(manifest::ModelManifest {
            ref_: Some("codex-oauth/gpt-5.6-luna".to_string()),
            ..Default::default()
        });
        task.config.extraction.reasoning = Some(ReasoningControl::Effort(ReasoningEffort::Medium));

        let manifest = task.extraction_manifest();
        assert_eq!(
            manifest.model.ref_.as_deref(),
            Some("codex-oauth/gpt-5.6-luna")
        );
        assert_eq!(
            manifest.engine.reasoning,
            Some(ReasoningControl::Effort(ReasoningEffort::Medium))
        );
    }

    #[tokio::test]
    async fn run_committed_background_task_finishes_empty_extraction_and_commits_pointer() {
        let client = ScriptClient::new(vec![finish_empty_events("finish-1"), completed_events()]);
        let calls = Arc::clone(&client.calls);
        let extension_writes = Arc::new(Mutex::new(Vec::new()));
        let (event_tx, _) = broadcast::channel(16);
        let workspace_client: Arc<dyn WorkspaceClient> =
            Arc::new(RecordingWorkspaceClient::default());
        run_background_task(test_task(
            capture(2, 250),
            Box::new(client),
            Arc::clone(&extension_writes),
            event_tx,
            workspace_client,
        ))
        .await;

        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let writes = extension_writes.lock().unwrap();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].1, extract::EXTRACT_DOMAIN);
        let pointer: memory::ExtractPointerPayload =
            serde_json::from_value(writes[0].2.clone()).unwrap();
        assert_eq!(pointer.processed_through_history_len, 2);
        assert_eq!(pointer.processed_through_entry, 1);
    }

    #[tokio::test]
    async fn run_committed_background_task_stages_non_empty_extraction_and_commits_pointer() {
        let source = capture(2, 250);
        let entry_ref =
            SessionCapture::from_history_entries(source.segment_id.clone(), source.history.clone())
                .overview()[0]
                .id
                .to_string();
        let client = ScriptClient::new(vec![
            stage_candidate_events("stage-1", &entry_ref),
            finish_events("finish-1", 1),
            completed_events(),
        ]);
        let calls = Arc::clone(&client.calls);
        let extension_writes = Arc::new(Mutex::new(Vec::new()));
        let (event_tx, mut event_rx) = broadcast::channel(64);
        let workspace_client = Arc::new(RecordingWorkspaceClient::default());
        run_background_task(test_task(
            source,
            Box::new(client),
            Arc::clone(&extension_writes),
            event_tx,
            workspace_client.clone(),
        ))
        .await;

        assert_eq!(calls.load(Ordering::SeqCst), 3);
        let writes = extension_writes.lock().unwrap();
        assert_eq!(
            writes.len(),
            1,
            "recorded requests: {:?}; events: {:?}",
            workspace_client.requests.lock().unwrap(),
            std::iter::from_fn(|| event_rx.try_recv().ok()).collect::<Vec<_>>()
        );
        let pointer: memory::ExtractPointerPayload =
            serde_json::from_value(writes[0].2.clone()).unwrap();
        assert_eq!(pointer.staging_id, "candidate-1");
        assert!(
            workspace_client
                .requests
                .lock()
                .unwrap()
                .iter()
                .any(|request| {
                    request
                        .body
                        .as_deref()
                        .is_some_and(|body| body.contains("stage_candidate"))
                })
        );
    }

    #[tokio::test]
    async fn subjektiv_extraction_stages_once_and_commits_its_independent_pointer() {
        let source = capture(2, 250);
        let entry_ref =
            SessionCapture::from_history_entries(source.segment_id.clone(), source.history.clone())
                .overview()[0]
                .id
                .to_string();
        let client = ScriptClient::new(vec![
            stage_candidate_events("stage-1", &entry_ref),
            finish_events("finish-1", 1),
            completed_events(),
        ]);
        let extension_writes = Arc::new(Mutex::new(Vec::new()));
        let (event_tx, _) = broadcast::channel(64);
        let workspace_client = Arc::new(RecordingWorkspaceClient::default());
        let mut task = test_task(
            source,
            Box::new(client),
            Arc::clone(&extension_writes),
            event_tx,
            workspace_client.clone(),
        );
        task.target = ExtractionTarget::Subjektiv;
        task.config.consolidation_request_enabled = false;
        run_background_task(task).await;

        let writes = extension_writes.lock().unwrap();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].1, SUBJEKTIV_EXTRACT_DOMAIN);
        let pointer: memory::ExtractPointerPayload =
            serde_json::from_value(writes[0].2.clone()).unwrap();
        assert_eq!(pointer.staging_id, "subject-candidate-1");
        let requests = workspace_client.requests.lock().unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.path.ends_with("/subjektiv/staging"))
                .count(),
            1
        );
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.path.ends_with("/subjektiv/sessions"))
                .count(),
            0,
            "Memory extraction must not own Session attribution",
        );
        assert!(requests.iter().all(|request| {
            request
                .body
                .as_deref()
                .is_none_or(|body| !body.contains("append_audit"))
        }));
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.path.ends_with("/subjektiv/consolidation"))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn empty_subjektiv_extraction_commits_pointer_without_attribution_or_candidate() {
        let client = ScriptClient::new(vec![finish_empty_events("finish-1"), completed_events()]);
        let extension_writes = Arc::new(Mutex::new(Vec::new()));
        let (event_tx, _) = broadcast::channel(32);
        let workspace_client = Arc::new(RecordingWorkspaceClient::default());
        let mut task = test_task(
            capture(2, 250),
            Box::new(client),
            Arc::clone(&extension_writes),
            event_tx,
            workspace_client.clone(),
        );
        task.target = ExtractionTarget::Subjektiv;
        task.config.consolidation_request_enabled = false;
        run_background_task(task).await;

        let writes = extension_writes.lock().unwrap();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].1, SUBJEKTIV_EXTRACT_DOMAIN);
        let pointer: memory::ExtractPointerPayload =
            serde_json::from_value(writes[0].2.clone()).unwrap();
        assert!(pointer.staging_id.is_empty());
        let requests = workspace_client.requests.lock().unwrap();
        assert!(
            !requests
                .iter()
                .any(|request| request.path.ends_with("/subjektiv/staging"))
        );
        assert!(
            !requests
                .iter()
                .any(|request| request.path.ends_with("/subjektiv/sessions"))
        );
    }

    #[tokio::test]
    async fn subjektiv_candidate_remains_staged_when_later_extraction_fails_without_pointer() {
        let source = capture(2, 250);
        let entry_ref =
            SessionCapture::from_history_entries(source.segment_id.clone(), source.history.clone())
                .overview()[0]
                .id
                .to_string();
        let client = ScriptClient::new(vec![stage_candidate_events("stage-1", &entry_ref)]);
        let extension_writes = Arc::new(Mutex::new(Vec::new()));
        let (event_tx, _) = broadcast::channel(32);
        let workspace_client = Arc::new(RecordingWorkspaceClient::default());
        let mut task = test_task(
            source,
            Box::new(client),
            Arc::clone(&extension_writes),
            event_tx,
            workspace_client.clone(),
        );
        task.target = ExtractionTarget::Subjektiv;
        task.config.consolidation_request_enabled = false;
        run_background_task(task).await;

        assert!(extension_writes.lock().unwrap().is_empty());
        let requests = workspace_client.requests.lock().unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.path.ends_with("/subjektiv/staging"))
                .count(),
            1,
            "the immutable partial candidate remains staged for retry/deduplication"
        );
        assert!(
            !requests
                .iter()
                .any(|request| request.path.ends_with("/subjektiv/sessions"))
        );
    }

    #[tokio::test]
    async fn failed_extraction_emits_failure_event_and_durable_audit_without_pointer() {
        let client = ScriptClient::new(Vec::new());
        let calls = Arc::clone(&client.calls);
        let extension_writes = Arc::new(Mutex::new(Vec::new()));
        let (event_tx, mut event_rx) = broadcast::channel(16);
        let workspace_client = Arc::new(RecordingWorkspaceClient::default());
        run_background_task(test_task(
            capture(2, 250),
            Box::new(client),
            Arc::clone(&extension_writes),
            event_tx,
            workspace_client.clone(),
        ))
        .await;

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(extension_writes.lock().unwrap().is_empty());
        let events = std::iter::from_fn(|| event_rx.try_recv().ok()).collect::<Vec<_>>();
        assert!(events.iter().any(|event| {
            matches!(event, Event::MemoryWorker(event) if event.status == "failed")
        }));
        let requests = workspace_client.requests.lock().unwrap();
        assert!(
            requests.iter().any(|request| {
                request.body.as_deref().is_some_and(|body| {
                    body.contains("append_audit")
                        && body.contains("worker_lifecycle")
                        && body.contains("failed")
                })
            }),
            "recorded requests: {requests:?}"
        );
    }

    #[tokio::test]
    async fn rewrite_barrier_cancels_active_extraction_and_emits_cancelled_without_pointer() {
        let calls = Arc::new(AtomicUsize::new(0));
        let client = PendingClient {
            calls: Arc::clone(&calls),
        };
        let extension_writes = Arc::new(Mutex::new(Vec::new()));
        let (event_tx, mut event_rx) = broadcast::channel(16);
        let workspace_client = Arc::new(RecordingWorkspaceClient::default());
        let registry = start_background_task(test_task(
            capture(2, 250),
            Box::new(client),
            Arc::clone(&extension_writes),
            event_tx,
            workspace_client.clone(),
        ));
        tokio::time::timeout(Duration::from_secs(5), async {
            while calls.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("extraction child should reach its first provider request");
        let rewrite_guard = registry.begin_session_rewrite().await.unwrap();
        drop(rewrite_guard);
        registry.shutdown().await.unwrap();

        assert!(extension_writes.lock().unwrap().is_empty());
        let events = std::iter::from_fn(|| event_rx.try_recv().ok()).collect::<Vec<_>>();
        assert!(events.iter().any(|event| {
            matches!(event, Event::MemoryWorker(event) if event.status == "cancelled")
        }));
        let requests = workspace_client.requests.lock().unwrap();
        assert!(
            requests.iter().any(|request| {
                request.body.as_deref().is_some_and(|body| {
                    body.contains("append_audit")
                        && body.contains("worker_lifecycle")
                        && body.contains("cancelled")
                })
            }),
            "recorded requests: {requests:?}"
        );
    }

    #[tokio::test]
    async fn subjektiv_rewrite_barrier_cancels_without_pointer_or_legacy_side_effects() {
        let calls = Arc::new(AtomicUsize::new(0));
        let client = PendingClient {
            calls: Arc::clone(&calls),
        };
        let extension_writes = Arc::new(Mutex::new(Vec::new()));
        let (event_tx, mut event_rx) = broadcast::channel(16);
        let workspace_client = Arc::new(RecordingWorkspaceClient::default());
        let mut task = test_task(
            capture(2, 250),
            Box::new(client),
            Arc::clone(&extension_writes),
            event_tx,
            workspace_client.clone(),
        );
        task.target = ExtractionTarget::Subjektiv;
        task.config.consolidation_request_enabled = false;
        let registry = start_background_task(task);
        tokio::time::timeout(Duration::from_secs(5), async {
            while calls.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("subjektiv extraction child should reach its first provider request");
        let rewrite_guard = registry.begin_session_rewrite().await.unwrap();
        drop(rewrite_guard);
        registry.shutdown().await.unwrap();

        assert!(extension_writes.lock().unwrap().is_empty());
        let events = std::iter::from_fn(|| event_rx.try_recv().ok()).collect::<Vec<_>>();
        assert!(events.iter().any(|event| {
            matches!(event, Event::MemoryWorker(event) if event.status == "cancelled")
        }));
        let requests = workspace_client.requests.lock().unwrap();
        assert!(
            requests.is_empty(),
            "cancelled subjektiv requests: {requests:?}"
        );
    }

    #[tokio::test]
    async fn interrupted_committed_run_skips_internal_worker_and_pointer_commit() {
        let client = ScriptClient::new(Vec::new());
        let calls = Arc::clone(&client.calls);
        let extension_writes = Arc::new(Mutex::new(Vec::new()));
        let (event_tx, mut event_rx) = broadcast::channel(16);
        let workspace_client: Arc<dyn WorkspaceClient> =
            Arc::new(RecordingWorkspaceClient::default());
        let mut interrupted = capture(2, 250);
        interrupted.run_exit = CommittedRunExit::Interrupted;
        run_background_task(test_task(
            interrupted,
            Box::new(client),
            Arc::clone(&extension_writes),
            event_tx,
            workspace_client,
        ))
        .await;

        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(extension_writes.lock().unwrap().is_empty());
        let events = std::iter::from_fn(|| event_rx.try_recv().ok()).collect::<Vec<_>>();
        assert!(events.iter().any(|event| {
            matches!(event, Event::MemoryWorker(event) if event.reason.contains("parent_run_not_finished"))
        }));
    }

    #[tokio::test]
    async fn lifecycle_task_requests_backend_owned_consolidation_eligibility() {
        let client = ScriptClient::new(Vec::new());
        let extension_writes = Arc::new(Mutex::new(Vec::new()));
        let (event_tx, _) = broadcast::channel(16);
        let workspace_client = Arc::new(RecordingWorkspaceClient::default());
        let mut interrupted = capture(2, 250);
        interrupted.run_exit = CommittedRunExit::Interrupted;
        let task = test_task(
            interrupted,
            Box::new(client),
            extension_writes,
            event_tx,
            workspace_client.clone(),
        );
        run_background_task(task).await;

        let requests = workspace_client.requests.lock().unwrap();
        assert!(
            requests.iter().any(|request| {
                request.path.contains("memory")
                    && request
                        .body
                        .as_deref()
                        .is_some_and(|body| body == "{\"force\":false}")
            }),
            "recorded requests: {requests:?}"
        );
    }

    #[tokio::test]
    async fn lifecycle_task_does_not_request_consolidation_when_profile_disables_it() {
        let client = ScriptClient::new(Vec::new());
        let extension_writes = Arc::new(Mutex::new(Vec::new()));
        let (event_tx, _) = broadcast::channel(16);
        let workspace_client = Arc::new(RecordingWorkspaceClient::default());
        let mut interrupted = capture(2, 250);
        interrupted.run_exit = CommittedRunExit::Interrupted;
        let mut task = test_task(
            interrupted,
            Box::new(client),
            extension_writes,
            event_tx,
            workspace_client.clone(),
        );
        task.config.consolidation_request_enabled = false;
        run_background_task(task).await;

        let requests = workspace_client.requests.lock().unwrap();
        assert!(
            !requests.iter().any(|request| {
                request
                    .body
                    .as_deref()
                    .is_some_and(|body| body == "{\"force\":false}")
            }),
            "recorded requests: {requests:?}"
        );
    }

    #[tokio::test]
    async fn lifecycle_task_requests_consolidation_when_extraction_is_disabled() {
        let client = ScriptClient::new(Vec::new());
        let extension_writes = Arc::new(Mutex::new(Vec::new()));
        let (event_tx, _) = broadcast::channel(16);
        let workspace_client = Arc::new(RecordingWorkspaceClient::default());
        let mut interrupted = capture(2, 250);
        interrupted.run_exit = CommittedRunExit::Interrupted;
        let mut task = test_task(
            interrupted,
            Box::new(client),
            extension_writes.clone(),
            event_tx,
            workspace_client.clone(),
        );
        task.config.extraction.enabled = false;
        task.config.consolidation_request_enabled = true;
        run_background_task(task).await;

        assert!(extension_writes.lock().unwrap().is_empty());
        let requests = workspace_client.requests.lock().unwrap();
        assert!(
            requests.iter().any(|request| {
                request
                    .body
                    .as_deref()
                    .is_some_and(|body| body == "{\"force\":false}")
            }),
            "recorded requests: {requests:?}"
        );
    }

    fn internal_result(
        lifecycle: WorkerRunResult,
    ) -> Result<InternalWorkerResult, InternalWorkerError> {
        Ok(InternalWorkerResult {
            usage: None,
            identity: InternalWorkerIdentity {
                kind: "memory-extract",
                run_id: uuid::Uuid::now_v7(),
            },
            lifecycle,
            history_entries: 1,
        })
    }

    fn capture(history_len: usize, input_total_tokens: u64) -> CommittedSessionCapture {
        CommittedSessionCapture {
            session_id: "session-1".to_string(),
            segment_id: "segment-1".to_string(),
            session_revision: history_len.try_into().unwrap(),
            entry_count: history_len,
            run_exit: CommittedRunExit::Finished,
            history: (0..history_len)
                .map(|index| HistoryEntry {
                    item: Item::user_message(format!("message-{index}")),
                    annotation: SessionHistoryMetadata::legacy_unknown(),
                })
                .collect(),
            usage_history: vec![UsageRecord {
                history_len,
                input_total_tokens,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                output_tokens: 0,
            }],
            extensions: Vec::new(),
        }
    }

    #[test]
    fn interrupted_parent_run_is_not_extraction_eligible() {
        assert!(extraction_run_eligible(CommittedRunExit::Finished));
        assert!(!extraction_run_eligible(CommittedRunExit::NonFinal));
        assert!(!extraction_run_eligible(CommittedRunExit::Interrupted));
    }

    #[test]
    fn normal_and_empty_extraction_require_explicit_finish() {
        let result = internal_result(WorkerRunResult::Finished);
        assert_eq!(
            extraction_disposition(&result, true),
            ExtractionDisposition::Completed
        );
        // `finish_called = true` with no staged ids is the explicit empty
        // extraction outcome. Missing Finish is a failed extraction.
        assert!(matches!(
            extraction_disposition(&result, false),
            ExtractionDisposition::Failed(reason)
                if reason.contains("FinishMemoryExtraction")
        ));
    }

    #[test]
    fn failed_and_pre_ai_cancelled_extraction_never_reach_pointer_commit() {
        let failed = internal_result(WorkerRunResult::Interrupted {
            code: crate::ErrorCode::Internal,
            message: "provider failed".to_string(),
        });
        assert!(matches!(
            extraction_disposition(&failed, true),
            ExtractionDisposition::Failed(_)
        ));
        let cancelled = internal_result(WorkerRunResult::RolledBack);
        assert_eq!(
            extraction_disposition(&cancelled, true),
            ExtractionDisposition::Cancelled("memory-extract cancelled".to_string())
        );
    }

    #[test]
    fn task_scope_cancels_and_joins_before_rewrite_and_shutdown() {
        let spec = memory_lifecycle_task_spec();
        assert_eq!(spec.trigger, BackgroundTaskTrigger::RunCommitted);
        assert_eq!(spec.max_concurrency, 1);
        assert_eq!(spec.rewrite, BackgroundTaskRewritePolicy::CancelAndWait);
        assert_eq!(spec.shutdown, BackgroundTaskShutdownPolicy::CancelAndWait);
    }

    #[test]
    fn threshold_uses_committed_usage_after_pointer() {
        let capture = capture(2, 250);
        let mut config = manifest::ResolvedMemoryFeatureConfig::default();
        config.profile.extraction.threshold = Some(1);
        assert!(extraction_threshold_reached(
            &capture,
            Some(&memory::ExtractPointerPayload {
                processed_through_entry: 0,
                processed_through_history_len: 1,
                staging_id: "staging-1".to_string(),
            }),
            &config
        ));
    }

    #[test]
    fn pointer_folds_latest_committed_extraction_extension() {
        let mut capture = capture(2, 250);
        let first = memory::ExtractPointerPayload {
            processed_through_entry: 1,
            processed_through_history_len: 1,
            staging_id: "staging-1".to_string(),
        };
        let latest = memory::ExtractPointerPayload {
            processed_through_entry: 3,
            processed_through_history_len: 2,
            staging_id: "staging-2".to_string(),
        };
        capture.extensions = vec![
            (
                extract::EXTRACT_DOMAIN.to_string(),
                serde_json::to_value(&first).unwrap(),
            ),
            ("other.feature".to_string(), serde_json::json!({})),
            (
                extract::EXTRACT_DOMAIN.to_string(),
                serde_json::to_value(&latest).unwrap(),
            ),
        ];
        assert_eq!(
            extract_pointer(&capture, extract::EXTRACT_DOMAIN).unwrap(),
            Some(latest)
        );
    }

    #[test]
    fn malformed_latest_pointer_fails_closed_instead_of_using_older_pointer() {
        let mut capture = capture(2, 250);
        capture.extensions = vec![
            (
                extract::EXTRACT_DOMAIN.to_string(),
                serde_json::to_value(memory::ExtractPointerPayload {
                    processed_through_entry: 1,
                    processed_through_history_len: 1,
                    staging_id: "staging-1".to_string(),
                })
                .unwrap(),
            ),
            (
                extract::EXTRACT_DOMAIN.to_string(),
                serde_json::json!({"invalid": true}),
            ),
        ];
        assert!(extract_pointer(&capture, extract::EXTRACT_DOMAIN).is_err());
    }

    #[test]
    fn worker_core_no_longer_owns_memory_extraction_scheduler() {
        let worker_source = include_str!("../../worker.rs");
        for removed in [
            "spawn_post_run_memory_jobs",
            "run_extract_once_with_cancel_observer",
            "consolidation_in_flight",
            "extract_in_flight",
            "memory_task:",
        ] {
            assert!(
                !worker_source.contains(removed),
                "Worker core still contains removed extraction scheduler symbol {removed}"
            );
        }
        let controller_source = include_str!("../../controller.rs");
        assert!(controller_source.contains("MemoryFeatureInstallPlan::prepare"));
        assert!(controller_source.contains("MemoryLifecycleFeature::from_resolved_config"));
        let worker_production = worker_source.split("#[cfg(test)]").next().unwrap();
        assert!(!worker_production.contains(".feature.memory"));
        let lifecycle_source = include_str!("memory_lifecycle.rs");
        assert!(lifecycle_source.contains("request_memory_staging_consolidation"));
        let internal_worker_source = include_str!("../../internal_worker.rs");
        assert!(!internal_worker_source.contains("manifest.memory = None"));
    }

    #[test]
    fn empty_capture_never_schedules_extraction() {
        let capture = capture(0, 500);
        let mut config = manifest::ResolvedMemoryFeatureConfig::default();
        config.profile.extraction.threshold = Some(1);
        assert!(!extraction_threshold_reached(&capture, None, &config));
    }
}
