//! Memory extraction lifecycle using committed runs and a restricted Internal Worker.
//!
//! Worker core owns only generic hook/task/session plumbing; this Feature owns Subject extraction,
//! lifecycle events and pointers. Extraction uses the parent's committed capture and
//! resolved evidence, not uncommitted child Worker output.
//!
//! Pointers advance only on success with explicit FinishMemoryExtraction, not mere Engine completion.
//! Generation fences and cancellation are checked; extensions append only when the capture's Segment
//! matches the current one, preventing pre-rewrite extraction results from marking ranges processed
//! in a new Segment. Candidate staging is an independent durable write before Finish, so later
//! failure, cancellation, or fence loss does not roll it back. Retries without pointer advancement
//! can produce semantic duplicates; consolidation owns candidate comparison.

use std::sync::Arc;
use std::time::Duration;

use agen::llm_client::LlmClient;
use arc_swap::ArcSwap;
use async_trait::async_trait;
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
use crate::worker::{WorkerFilesystemAuthority, WorkerWorkspaceContext};
use agen::token_counter::total_tokens_at;
use manifest::WorkerManifest;
use protocol::Event;

const SUBJEKTIV_TASK_NAME: &str = "subjektiv-memory-lifecycle";
const SUBJEKTIV_EXTRACT_DOMAIN: &str = "subjektiv.extract.v1";
const TASK_TIMEOUT: Duration = Duration::from_secs(300);

/// Subject-scoped extraction of committed parent runs through an explicit Host.
#[derive(Clone)]
pub(crate) struct SubjektivLifecycleFeature {
    task: SubjektivLifecycleTask,
}

#[derive(Clone)]
struct SubjektivLifecycleTask {
    config: manifest::MemoryExtractionProfileConfig,
    capture: CommittedSessionCaptureHandle,
    extensions: SessionExtensionHandle,
    host: crate::subjektiv::SubjektivHostConnection,
    manifest: WorkerManifest,
    client: Box<dyn LlmClient>,
    prompts: Arc<ArcSwap<PromptCatalog>>,
    workspace_context: WorkerWorkspaceContext,
    event_tx: Option<broadcast::Sender<Event>>,
}

impl SubjektivLifecycleFeature {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_resolved_config(
        lifecycle_enabled: bool,
        config: manifest::ResolvedSubjektivFeatureConfig,
        capture: CommittedSessionCaptureHandle,
        extensions: SessionExtensionHandle,
        host: Option<crate::subjektiv::SubjektivHostConnection>,
        manifest: WorkerManifest,
        client: Box<dyn LlmClient>,
        prompts: Arc<ArcSwap<PromptCatalog>>,
        workspace_context: WorkerWorkspaceContext,
        event_tx: Option<broadcast::Sender<Event>>,
    ) -> std::io::Result<Option<Self>> {
        if !lifecycle_enabled || !config.profile.enabled || !config.profile.extraction.enabled {
            return Ok(None);
        }
        let Some(host) = host else {
            return Ok(None);
        };
        config
            .validate_execution()
            .map_err(|message| std::io::Error::new(std::io::ErrorKind::InvalidInput, message))?;
        Ok(Some(Self {
            task: SubjektivLifecycleTask {
                config: config.profile.extraction,
                capture,
                extensions,
                host,
                manifest,
                client,
                prompts,
                workspace_context,
                event_tx,
            },
        }))
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

fn subjektiv_lifecycle_task_spec() -> BackgroundTaskSpec {
    let declaration = BackgroundTaskDeclaration::worker_managed(
        SUBJEKTIV_TASK_NAME,
        "Extract provenance-preserving candidates into subject-scoped staging after committed runs.",
    );
    let mut spec = BackgroundTaskSpec::single_flight(declaration, TASK_TIMEOUT);
    spec.trigger = BackgroundTaskTrigger::RunCommitted;
    spec
}

#[derive(Clone)]
struct SubjektivExtractionAuthority {
    host: crate::subjektiv::SubjektivHostConnection,
}

impl SubjektivExtractionAuthority {
    fn authorize(&self, operation: &str) -> Result<(), HookError> {
        let result = self
            .host
            .host
            .authorize_operation(&self.host.context, operation);
        result.map_err(|error| {
            HookError::new(
                HookErrorCategory::Dependency,
                format!("Memory background operation {operation} not started: {error}"),
            )
        })
    }
}

impl FeatureModule for SubjektivExtractionAuthority {
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builtin("memory-extraction-authority", "Memory extraction authority")
            .with_hook(crate::feature::HookDeclaration::new(
                "supply",
                crate::feature::FeatureHookPoint::PreLlmRequest,
            ))
            .with_hook(crate::feature::HookDeclaration::new(
                "access",
                crate::feature::FeatureHookPoint::PreToolCall,
            ))
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        context.hooks().add_pre_llm_request(
            "supply",
            crate::hook::HookExecutionPolicy::fail_closed(),
            self.clone(),
        )?;
        context.hooks().add_pre_tool_call("access", self.clone())
    }
}

#[async_trait]
impl crate::hook::Hook<crate::hook::PreLlmRequest> for SubjektivExtractionAuthority {
    async fn call(
        &self,
        _context: &crate::hook::PreRequestContext,
    ) -> Result<crate::hook::HookPreRequestAction, HookError> {
        self.authorize("memory.extract.supply")?;
        Ok(crate::hook::HookPreRequestAction::Continue)
    }
}

#[async_trait]
impl crate::hook::Hook<crate::hook::PreToolCall> for SubjektivExtractionAuthority {
    async fn call(
        &self,
        context: &crate::hook::ToolCallSummary,
    ) -> Result<crate::hook::HookPreToolAction, HookError> {
        match context.tool_name.as_str() {
            "ShowOverview" | "SearchEntries" | "ReadEntry" => {
                self.authorize("memory.extract.read")?
            }
            "StageMemoryCandidate" => self.authorize("memory.staging.write")?,
            _ => {}
        }
        Ok(crate::hook::HookPreToolAction::Continue)
    }
}

impl SubjektivLifecycleTask {
    fn authority(&self) -> SubjektivExtractionAuthority {
        SubjektivExtractionAuthority {
            host: self.host.clone(),
        }
    }

    fn extraction_manifest(&self) -> WorkerManifest {
        let mut manifest = self.manifest.clone();
        if let Some(model) = self.config.model.clone() {
            manifest.model = model;
        }
        if let Some(reasoning) = self.config.reasoning.clone() {
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
        let events = MemoryWorkerEvents::extraction();
        self.authority().authorize("memory.extract.read")?;
        let capture = match self.capture.capture() {
            Ok(capture) => capture,
            Err(error) => {
                events.emit(
                    self.event_tx.as_ref(),
                    MemoryWorkerStatus::Failed,
                    format!("committed_session_capture_failed: {error}"),
                );
                return Ok(());
            }
        };
        if !extraction_run_eligible(capture.run_exit) {
            events.emit(
                self.event_tx.as_ref(),
                MemoryWorkerStatus::Skipped,
                format!("parent_run_not_finished: {:?}", capture.run_exit),
            );
            return Ok(());
        }
        let pointer = match extract_pointer(&capture) {
            Ok(pointer) => pointer,
            Err(error) => {
                events.emit(
                    self.event_tx.as_ref(),
                    MemoryWorkerStatus::Failed,
                    format!("extract_pointer_invalid: {error}"),
                );
                return Ok(());
            }
        };
        let Some(threshold) = self.config.threshold.filter(|threshold| *threshold > 0) else {
            events.emit(
                self.event_tx.as_ref(),
                MemoryWorkerStatus::Skipped,
                "token_threshold_disabled",
            );
            return Ok(());
        };
        let tokens_since = tokens_since_pointer(&capture, pointer.as_ref());
        if tokens_since < threshold {
            events.emit(
                self.event_tx.as_ref(),
                MemoryWorkerStatus::Skipped,
                format!(
                    "token_threshold_not_reached tokens_since={tokens_since} threshold={threshold}"
                ),
            );
            return Ok(());
        }

        let history_start = pointer
            .as_ref()
            .map(|pointer| pointer.processed_through_history_len)
            .unwrap_or(0)
            .min(capture.history.len());
        let history_end = capture.history.len();
        if history_start >= history_end || capture.entry_count == 0 {
            events.emit(
                self.event_tx.as_ref(),
                MemoryWorkerStatus::Skipped,
                "no_new_committed_session_entries",
            );
            return Ok(());
        }
        let view = SessionCapture::from_complete_history(
            capture.session_id.clone(),
            capture.segment_id.clone(),
            capture.history.clone(),
        )
        .select_order_range(history_start as u64..history_end as u64)
        .map_err(hook_internal)?;
        let start_entry = pointer
            .as_ref()
            .map(|pointer| pointer.processed_through_entry + 1)
            .unwrap_or(0);
        let source = SourceRef {
            segment_id: capture.segment_id.clone(),
            range: [start_entry as u64, (capture.entry_count - 1) as u64],
        };
        events.emit(
            self.event_tx.as_ref(),
            MemoryWorkerStatus::Started,
            "token_threshold_reached",
        );
        let output_state = MemoryStagingOutputState::new_subjektiv(
            view.clone(),
            self.host.clone(),
            source,
            events.run_id.to_string(),
        );
        let client = if let Some(model) = self.config.model.as_ref() {
            match crate::model_client::build_client(model) {
                Ok(client) => client,
                Err(error) => {
                    self.record_preparation_failure(&events, error.to_string());
                    return Ok(());
                }
            }
        } else {
            self.client.clone_boxed()
        };
        let memory_language = self.host.host.settings().language;
        let system_prompt = match self
            .prompts
            .load_full()
            .memory_extract_system(&memory_language)
        {
            Ok(prompt) => prompt,
            Err(error) => {
                self.record_preparation_failure(&events, error.to_string());
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
        self.authority().authorize("memory.extract.supply")?;
        let features = FeatureRegistryBuilder::new()
            .with_module(self.authority())
            .with_module(SessionExploreFeature::new(SessionExploreState::new(
                view.clone(),
            )))
            .with_module(MemoryStagingOutputFeature::new(output_state.clone()));
        let result = run_internal_worker_with_cancel_sender(
            InternalWorkerSpec {
                identity: InternalWorkerIdentity {
                    kind: "memory-extract",
                    run_id: events.run_id,
                },
                manifest,
                client,
                system_prompt,
                input: render_extract_input(&view),
                cache_key: Some(capture.segment_id.clone()),
                max_turns: self
                    .config
                    .worker_max_turns
                    .or(manifest::defaults::MEMORY_EXTRACT_WORKER_MAX_TURNS),
                compaction_budget: None,
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

        if let Ok(run) = &result {
            tracing::debug!(
                worker_kind = run.identity.kind,
                run_id = %run.identity.run_id,
                history_entries = run.history_entries,
                "Subjektiv extraction Internal Worker completed"
            );
        }
        let pointer_staging_id = output_state.staged().first().cloned().unwrap_or_default();

        match extraction_disposition(&result, output_state.is_finished()) {
            ExtractionDisposition::Cancelled(reason) => {
                events.emit(
                    self.event_tx.as_ref(),
                    MemoryWorkerStatus::Cancelled,
                    reason,
                );
                return Ok(());
            }
            ExtractionDisposition::Failed(reason) => {
                events.emit(self.event_tx.as_ref(), MemoryWorkerStatus::Failed, reason);
                return Ok(());
            }
            ExtractionDisposition::Completed => {}
        }

        if cancellation.is_cancelled() {
            events.emit(
                self.event_tx.as_ref(),
                MemoryWorkerStatus::Cancelled,
                "memory-extract cancelled before completion side effects",
            );
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
            events.emit(
                self.event_tx.as_ref(),
                MemoryWorkerStatus::Cancelled,
                "memory-extract cancelled before pointer commit",
            );
            return Ok(());
        }
        self.authority().authorize("memory.extract.commit")?;
        context.generation_fence.ensure_current()?;
        if cancellation.is_cancelled() {
            return Err(HookError::new(
                HookErrorCategory::Cancelled,
                "Memory extraction cancelled before pointer commit",
            ));
        }
        if !self
            .extensions
            .append_if_current(&capture.location(), SUBJEKTIV_EXTRACT_DOMAIN, payload)
            .map_err(hook_internal)?
        {
            events.emit(
                self.event_tx.as_ref(),
                MemoryWorkerStatus::Cancelled,
                "session changed before memory-extract pointer commit",
            );
            return Ok(());
        }
        events.emit(
            self.event_tx.as_ref(),
            MemoryWorkerStatus::Completed,
            "memory-extract completed",
        );
        Ok(())
    }
}

#[async_trait]
impl FeatureBackgroundTask for SubjektivLifecycleTask {
    async fn run(
        &self,
        context: BackgroundTaskContext,
        cancellation: BackgroundTaskCancellation,
    ) -> Result<(), HookError> {
        self.run_extraction(context.clone(), cancellation.clone())
            .await?;
        if !cancellation.is_cancelled() {
            context.generation_fence.ensure_current()?;
            self.request_consolidation()?;
        }
        Ok(())
    }
}

impl SubjektivLifecycleTask {
    fn request_consolidation(&self) -> Result<(), HookError> {
        self.authority().authorize("memory.consolidation.request")?;
        let events = MemoryWorkerEvents::consolidation();
        let operation = memory::subject::MemoryConsolidateStagingOperation { force: false };
        let request = self
            .host
            .host
            .request_consolidation(&self.host.context, operation);
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
                events.emit(
                    self.event_tx.as_ref(),
                    MemoryWorkerStatus::Skipped,
                    "consolidation_backend_operation_failed",
                );
                return Err(HookError::new(
                    HookErrorCategory::Dependency,
                    format!("Memory consolidation request not completed: {error}"),
                ));
            }
        }
        Ok(())
    }

    fn record_preparation_failure(&self, events: &MemoryWorkerEvents, reason: impl Into<String>) {
        events.emit(self.event_tx.as_ref(), MemoryWorkerStatus::Failed, reason);
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
            // evidence for diagnostics even though the public event reason is
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
) -> Result<Option<memory::ExtractPointerPayload>, HookError> {
    let pointer = capture
        .extensions
        .iter()
        .rev()
        .find(|(candidate, _)| candidate == SUBJEKTIV_EXTRACT_DOMAIN)
        .and_then(|(_, payload)| serde_json::from_value(payload.clone()).ok());
    if pointer.is_none()
        && capture
            .extensions
            .iter()
            .any(|(candidate, _)| candidate == SUBJEKTIV_EXTRACT_DOMAIN)
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
    config: &manifest::ResolvedSubjektivFeatureConfig,
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

/// Ephemeral UI events; candidate persistence belongs solely to the Subjektiv Host.
#[derive(Clone)]
struct MemoryWorkerEvents {
    run_id: uuid::Uuid,
    worker: &'static str,
    trigger: &'static str,
}

enum MemoryWorkerStatus {
    Started,
    Completed,
    Skipped,
    Failed,
    Cancelled,
}

impl MemoryWorkerStatus {
    fn label(&self) -> &'static str {
        match self {
            Self::Started => "running",
            Self::Completed => "done",
            Self::Skipped => "skipped",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

impl MemoryWorkerEvents {
    fn extraction() -> Self {
        Self {
            run_id: uuid::Uuid::now_v7(),
            worker: "extract",
            trigger: "token_threshold",
        }
    }

    fn consolidation() -> Self {
        Self {
            run_id: uuid::Uuid::now_v7(),
            worker: "consolidation",
            trigger: "staging_backlog",
        }
    }

    fn emit(
        &self,
        event_tx: Option<&broadcast::Sender<Event>>,
        status: MemoryWorkerStatus,
        reason: impl Into<String>,
    ) {
        let reason = reason.into();
        if let Some(tx) = event_tx {
            let _ = tx.send(Event::MemoryWorker(protocol::MemoryWorkerEvent {
                worker: self.worker.to_string(),
                status: status.label().to_string(),
                run_id: self.run_id.to_string(),
                trigger: self.trigger.to_string(),
                reason: reason.clone(),
                message: format!("memory {} {}: {reason}", self.worker, status.label()),
                timestamp_ms: now_millis() as i64,
            }));
        }
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
    use crate::worker::WorkspaceClient;

    #[derive(Debug, Default)]
    struct RecordingSubjektivClient {
        denied: std::sync::atomic::AtomicBool,
        checks: Mutex<Vec<String>>,
        requests: Mutex<Vec<crate::worker::WorkspaceRequest>>,
    }

    impl WorkspaceClient for RecordingSubjektivClient {
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
            if request.path.contains("/authorization/") {
                let check: serde_json::Value =
                    serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
                self.checks
                    .lock()
                    .unwrap()
                    .push(check["operation"].as_str().unwrap().to_owned());
                return Ok(crate::subjektiv::test_subject_authorization_response(
                    &request,
                    "workspace-1",
                    if self.denied.load(Ordering::SeqCst) {
                        &[]
                    } else {
                        &[
                            "memory.extract.read",
                            "memory.extract.supply",
                            "memory.extract.commit",
                            "memory.staging.write",
                            "memory.consolidation.request",
                        ]
                    },
                )
                .expect("Subject authorization request"));
            }
            let is_subjektiv_stage = request.path.ends_with("/subjektiv/staging");
            let is_subjektiv_consolidation = request.path.ends_with("/subjektiv/consolidation");
            self.requests.lock().unwrap().push(request);
            if is_subjektiv_consolidation {
                return Ok(crate::worker::WorkspaceResponse {
                    content_type: None,
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
                    content_type: None,
                    status: 200,
                    body: serde_json::to_string(&server_api::SubjektivStageCandidateResponse {
                        staging_id: "subject-candidate-1".to_string(),
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
        revoke_after_request: Option<Arc<RecordingSubjektivClient>>,
        responses: Arc<Vec<Vec<LlmEvent>>>,
        calls: Arc<AtomicUsize>,
    }

    impl ScriptClient {
        fn new(responses: Vec<Vec<LlmEvent>>) -> Self {
            Self {
                revoke_after_request: None,
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
            if let Some(host) = &self.revoke_after_request {
                host.denied.store(true, Ordering::SeqCst);
            }
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

    fn test_config() -> manifest::ResolvedSubjektivFeatureConfig {
        let mut config = manifest::ResolvedSubjektivFeatureConfig::default();
        config.profile.enabled = true;
        config.profile.extraction.enabled = true;
        config.profile.extraction.threshold = Some(1);
        config
            .bind_workspace_settings(manifest::SubjektivMemorySettingsSnapshot {
                workspace_id: "workspace-1".to_string(),

                language: "English".to_string(),
            })
            .unwrap();
        config
    }

    fn test_task(
        capture: CommittedSessionCapture,
        client: Box<dyn LlmClient>,
        extension_writes: Arc<Mutex<Vec<(CommittedSessionLocation, String, serde_json::Value)>>>,
        event_tx: broadcast::Sender<Event>,
        workspace_client: Arc<dyn WorkspaceClient>,
    ) -> SubjektivLifecycleTask {
        let capture_handle = CommittedSessionCaptureHandle::new(move || Ok(capture.clone()));
        let extensions = SessionExtensionHandle::new(move |location, domain, payload| {
            extension_writes
                .lock()
                .unwrap()
                .push((location.clone(), domain.to_string(), payload));
            Ok(true)
        });
        SubjektivLifecycleFeature::from_resolved_config(
            true,
            test_config(),
            capture_handle,
            extensions,
            Some(crate::subjektiv::test_connection(workspace_client)),
            test_manifest(),
            client,
            Arc::new(ArcSwap::from(PromptCatalog::builtins_only().unwrap())),
            WorkerWorkspaceContext::no_workspace(),
            Some(event_tx),
        )
        .unwrap()
        .unwrap()
        .task
    }

    fn start_background_task(
        task: SubjektivLifecycleTask,
    ) -> crate::feature::background::FeatureBackgroundTaskRegistry {
        let mut builder = FeatureBackgroundTaskRegistryBuilder::default();
        builder
            .register(
                crate::feature::FeatureId::builtin("subjektiv-memory-lifecycle"),
                subjektiv_lifecycle_task_spec(),
                task,
            )
            .unwrap();
        let registry = builder.build();
        registry
            .start_run_committed(HookInvocationContext {
                worker_id: "worker-1".to_string(),
                session_id: "session-1".to_string(),

                run_id: Some("run-1".to_string()),
                ..Default::default()
            })
            .unwrap();
        registry
    }

    async fn run_background_task(task: SubjektivLifecycleTask) {
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
            None,
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
            None,
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
            .bind_workspace_settings(manifest::SubjektivMemorySettingsSnapshot {
                workspace_id: "workspace-1".to_string(),

                language: "English".to_string(),
            })
            .unwrap();

        let feature = SubjektivLifecycleFeature::from_resolved_config(
            false,
            config,
            capture,
            extensions,
            None,
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
            Arc::new(RecordingSubjektivClient::default());
        let mut task = test_task(
            capture(2, 250),
            Box::new(client),
            extension_writes,
            event_tx,
            workspace_client,
        );
        task.config.model = Some(manifest::ModelManifest {
            ref_: Some("codex-oauth/gpt-5.6-luna".to_string()),
            ..Default::default()
        });
        task.config.reasoning = Some(ReasoningControl::Effort(ReasoningEffort::Medium));

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
    async fn denied_background_extraction_never_reads_capture_or_calls_model_or_writes() {
        let client = ScriptClient::new(Vec::new());
        let calls = Arc::clone(&client.calls);
        let extension_writes = Arc::new(Mutex::new(Vec::new()));
        let (event_tx, _) = broadcast::channel(16);
        let workspace_client = Arc::new(RecordingSubjektivClient::default());
        workspace_client.denied.store(true, Ordering::SeqCst);
        let mut task = test_task(
            capture(2, 250),
            Box::new(client),
            extension_writes.clone(),
            event_tx,
            workspace_client.clone(),
        );
        task.capture =
            CommittedSessionCaptureHandle::new(|| panic!("denial must precede protected capture"));
        run_background_task(task).await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(extension_writes.lock().unwrap().is_empty());
        assert!(workspace_client.requests.lock().unwrap().is_empty());
        assert_eq!(
            *workspace_client.checks.lock().unwrap(),
            vec!["memory.extract.read"]
        );
    }

    #[tokio::test]
    async fn background_revocation_blocks_next_model_supply_and_pointer_commit() {
        let workspace_client = Arc::new(RecordingSubjektivClient::default());
        let mut client =
            ScriptClient::new(vec![finish_empty_events("finish-1"), completed_events()]);
        client.revoke_after_request = Some(workspace_client.clone());
        let calls = Arc::clone(&client.calls);
        let extension_writes = Arc::new(Mutex::new(Vec::new()));
        let (event_tx, _) = broadcast::channel(16);
        run_background_task(test_task(
            capture(2, 250),
            Box::new(client),
            extension_writes.clone(),
            event_tx,
            workspace_client.clone(),
        ))
        .await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(extension_writes.lock().unwrap().is_empty());
        assert!(
            !workspace_client
                .requests
                .lock()
                .unwrap()
                .iter()
                .any(|request| request.path.ends_with("/consolidation"))
        );
        assert!(
            workspace_client
                .checks
                .lock()
                .unwrap()
                .iter()
                .filter(|operation| operation.as_str() == "memory.extract.supply")
                .count()
                >= 3
        );
    }

    #[tokio::test]
    async fn run_committed_background_task_finishes_empty_extraction_and_commits_pointer() {
        let client = ScriptClient::new(vec![finish_empty_events("finish-1"), completed_events()]);
        let calls = Arc::clone(&client.calls);
        let extension_writes = Arc::new(Mutex::new(Vec::new()));
        let (event_tx, _) = broadcast::channel(16);
        let workspace_client: Arc<dyn WorkspaceClient> =
            Arc::new(RecordingSubjektivClient::default());
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
        assert_eq!(writes[0].1, SUBJEKTIV_EXTRACT_DOMAIN);
        let pointer: memory::ExtractPointerPayload =
            serde_json::from_value(writes[0].2.clone()).unwrap();
        assert_eq!(pointer.processed_through_history_len, 2);
        assert_eq!(pointer.processed_through_entry, 1);
    }

    #[tokio::test]
    async fn run_committed_background_task_stages_non_empty_extraction_and_commits_pointer() {
        let source = capture(2, 250);
        let entry_ref = SessionCapture::from_complete_history(
            source.session_id.clone(),
            source.segment_id.clone(),
            source.history.clone(),
        )
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
        let workspace_client = Arc::new(RecordingSubjektivClient::default());
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
        assert_eq!(pointer.staging_id, "subject-candidate-1");
        assert!(
            workspace_client
                .requests
                .lock()
                .unwrap()
                .iter()
                .any(|request| request.path.ends_with("/subjektiv/staging"))
        );
    }

    // Real background extraction -> common fixed selection -> StageMemoryCandidate
    // -> Subject Host transport and pointer write. Only model/backend transport
    // are replaced; the second extraction uses the first run's saved pointer.
    #[tokio::test]
    async fn second_extraction_preserves_original_evidence_positions_after_nonzero_pointer() {
        let first = capture(2, 250);
        let mut second = capture(4, 750);
        second.history[..2].clone_from_slice(&first.history);
        second
            .usage_history
            .insert(0, first.usage_history[0].clone());
        let selected_ref =
            crate::SessionEntryRef::from_history_entry_id(&second.history[2].annotation.entry_id)
                .to_string();
        let extension_writes = Arc::new(Mutex::new(Vec::new()));
        let workspace_client = Arc::new(RecordingSubjektivClient::default());
        let (event_tx, _) = broadcast::channel(64);
        run_background_task(test_task(
            first,
            Box::new(ScriptClient::new(vec![
                finish_empty_events("first-empty"),
                completed_events(),
            ])),
            extension_writes.clone(),
            event_tx.clone(),
            workspace_client.clone(),
        ))
        .await;
        {
            let writes = extension_writes.lock().unwrap();
            assert_eq!(writes.len(), 1);
            let pointer: memory::ExtractPointerPayload =
                serde_json::from_value(writes[0].2.clone()).unwrap();
            assert_eq!(pointer.processed_through_history_len, 2);
            second
                .extensions
                .push((writes[0].1.clone(), writes[0].2.clone()));
        }
        run_background_task(test_task(
            second,
            Box::new(ScriptClient::new(vec![
                stage_candidate_events("second-stage", &selected_ref),
                finish_events("second-finish", 1),
                completed_events(),
            ])),
            extension_writes.clone(),
            event_tx,
            workspace_client.clone(),
        ))
        .await;
        let operations = workspace_client
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.path.ends_with("/subjektiv/staging"))
            .map(|request| {
                serde_json::from_str::<server_api::SubjektivStageCandidateRequest>(
                    request.body.as_deref().unwrap(),
                )
                .unwrap()
                .operation
            })
            .collect::<Vec<_>>();
        assert_eq!(operations.len(), 1);
        let operation = &operations[0];
        assert_eq!(operation.source.range, [2, 3]);
        assert_eq!(operation.evidence[0].id, selected_ref);
        assert_eq!(operation.evidence[0].entry_range, Some([2, 2]));
        assert_eq!(
            operation.source_refs[0].segment_id.as_deref(),
            Some("segment-1")
        );
        assert_eq!(operation.source_refs[0].entry_range, Some([2, 2]));
        let writes = extension_writes.lock().unwrap();
        assert_eq!(writes.len(), 2);
        let pointer: memory::ExtractPointerPayload =
            serde_json::from_value(writes[1].2.clone()).unwrap();
        assert_eq!(pointer.processed_through_history_len, 4);
        assert_eq!(pointer.processed_through_entry, 3);
    }

    #[tokio::test]
    async fn subjektiv_extraction_stages_once_and_commits_its_independent_pointer() {
        let source = capture(2, 250);
        let entry_ref = SessionCapture::from_complete_history(
            source.session_id.clone(),
            source.segment_id.clone(),
            source.history.clone(),
        )
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
        let workspace_client = Arc::new(RecordingSubjektivClient::default());
        let task = test_task(
            source,
            Box::new(client),
            Arc::clone(&extension_writes),
            event_tx,
            workspace_client.clone(),
        );
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
        let workspace_client = Arc::new(RecordingSubjektivClient::default());
        let task = test_task(
            capture(2, 250),
            Box::new(client),
            Arc::clone(&extension_writes),
            event_tx,
            workspace_client.clone(),
        );
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
        let entry_ref = SessionCapture::from_complete_history(
            source.session_id.clone(),
            source.segment_id.clone(),
            source.history.clone(),
        )
        .overview()[0]
            .id
            .to_string();
        let client = ScriptClient::new(vec![stage_candidate_events("stage-1", &entry_ref)]);
        let extension_writes = Arc::new(Mutex::new(Vec::new()));
        let (event_tx, _) = broadcast::channel(32);
        let workspace_client = Arc::new(RecordingSubjektivClient::default());
        let task = test_task(
            source,
            Box::new(client),
            Arc::clone(&extension_writes),
            event_tx,
            workspace_client.clone(),
        );
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
    async fn failed_extraction_emits_failure_event_without_pointer() {
        let client = ScriptClient::new(Vec::new());
        let calls = Arc::clone(&client.calls);
        let extension_writes = Arc::new(Mutex::new(Vec::new()));
        let (event_tx, mut event_rx) = broadcast::channel(16);
        let workspace_client = Arc::new(RecordingSubjektivClient::default());
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
            requests
                .iter()
                .all(|request| request.path.ends_with("/subjektiv/consolidation")),
            "failed extraction must not stage candidates: {requests:?}"
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
        let workspace_client = Arc::new(RecordingSubjektivClient::default());
        let task = test_task(
            capture(2, 250),
            Box::new(client),
            Arc::clone(&extension_writes),
            event_tx,
            workspace_client.clone(),
        );
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
            Arc::new(RecordingSubjektivClient::default());
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
        let workspace_client = Arc::new(RecordingSubjektivClient::default());
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
                request.path.ends_with("/subjektiv/consolidation")
                    && request
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
        let spec = subjektiv_lifecycle_task_spec();
        assert_eq!(spec.trigger, BackgroundTaskTrigger::RunCommitted);
        assert_eq!(spec.max_concurrency, 1);
        assert_eq!(spec.rewrite, BackgroundTaskRewritePolicy::CancelAndWait);
        assert_eq!(spec.shutdown, BackgroundTaskShutdownPolicy::CancelAndWait);
    }

    #[test]
    fn threshold_uses_committed_usage_after_pointer() {
        let capture = capture(2, 250);
        let mut config = manifest::ResolvedSubjektivFeatureConfig::default();
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
                SUBJEKTIV_EXTRACT_DOMAIN.to_string(),
                serde_json::to_value(&first).unwrap(),
            ),
            ("other.feature".to_string(), serde_json::json!({})),
            (
                SUBJEKTIV_EXTRACT_DOMAIN.to_string(),
                serde_json::to_value(&latest).unwrap(),
            ),
        ];
        assert_eq!(extract_pointer(&capture).unwrap(), Some(latest));
    }

    #[test]
    fn malformed_latest_pointer_fails_closed_instead_of_using_older_pointer() {
        let mut capture = capture(2, 250);
        capture.extensions = vec![
            (
                SUBJEKTIV_EXTRACT_DOMAIN.to_string(),
                serde_json::to_value(memory::ExtractPointerPayload {
                    processed_through_entry: 1,
                    processed_through_history_len: 1,
                    staging_id: "staging-1".to_string(),
                })
                .unwrap(),
            ),
            (
                SUBJEKTIV_EXTRACT_DOMAIN.to_string(),
                serde_json::json!({"invalid": true}),
            ),
        ];
        assert!(extract_pointer(&capture).is_err());
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
    }

    #[test]
    fn empty_capture_never_schedules_extraction() {
        let capture = capture(0, 500);
        let mut config = manifest::ResolvedSubjektivFeatureConfig::default();
        config.profile.extraction.threshold = Some(1);
        assert!(!extraction_threshold_reached(&capture, None, &config));
    }
}
