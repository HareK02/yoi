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
    CommittedSessionCapture, CommittedSessionCaptureHandle, SessionExtensionHandle,
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

const TASK_NAME: &str = "memory-extraction";
const TASK_TIMEOUT: Duration = Duration::from_secs(300);

/// Parent-Worker lifecycle Feature that observes committed runs and schedules
/// bounded extraction work. It owns the Memory pointer, audit, restricted
/// Internal Worker, and staging disposition; Worker core owns only generic
/// hook/task/session plumbing.
#[derive(Clone)]
pub(crate) struct MemoryExtractionLifecycleFeature {
    task: MemoryExtractionTask,
}

#[derive(Clone)]
struct MemoryExtractionTask {
    config: manifest::MemoryConfig,
    capture: CommittedSessionCaptureHandle,
    extensions: SessionExtensionHandle,
    workspace_client: Arc<dyn WorkspaceClient>,
    manifest: WorkerManifest,
    client: Box<dyn LlmClient>,
    prompts: Arc<ArcSwap<PromptCatalog>>,
    workspace_context: WorkerWorkspaceContext,
    event_tx: Option<broadcast::Sender<Event>>,
}

impl MemoryExtractionLifecycleFeature {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        config: manifest::MemoryConfig,
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
            task: MemoryExtractionTask {
                config,
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

impl FeatureModule for MemoryExtractionLifecycleFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builtin("memory-extraction-lifecycle", "Memory Extraction Lifecycle")
            .with_description(
                "Observes terminal committed runs and schedules bounded Memory extraction.",
            )
            .with_background_task(BackgroundTaskDeclaration::worker_managed(
                TASK_NAME,
                "Extract provenance-preserving Memory candidates after committed runs.",
            ))
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        context
            .background_tasks()
            .register(memory_extraction_task_spec(), self.task.clone())
    }
}

fn memory_extraction_task_spec() -> BackgroundTaskSpec {
    let declaration = BackgroundTaskDeclaration::worker_managed(
        TASK_NAME,
        "Extract provenance-preserving Memory candidates after committed runs.",
    );
    let mut spec = BackgroundTaskSpec::single_flight(declaration, TASK_TIMEOUT);
    spec.trigger = BackgroundTaskTrigger::RunCommitted;
    spec
}

#[async_trait]
impl FeatureBackgroundTask for MemoryExtractionTask {
    async fn run(
        &self,
        context: BackgroundTaskContext,
        cancellation: BackgroundTaskCancellation,
    ) -> Result<(), HookError> {
        context.generation_fence.ensure_current()?;
        let capture = self.capture.capture().map_err(hook_internal)?;
        let pointer = extract_pointer(&capture)?;
        if !extraction_threshold_reached(&capture, pointer.as_ref(), &self.config) {
            return Ok(());
        }

        let history_start = pointer
            .as_ref()
            .map(|pointer| pointer.processed_through_history_len)
            .unwrap_or(0)
            .min(capture.history.len());
        let history_end = capture.history.len();
        if history_start >= history_end || capture.entry_count == 0 {
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
        let audit = WorkerAuditBase::new(
            memory::audit::AuditWorker::MemoryExtract,
            memory::audit::AuditTrigger::TokenThreshold,
            self.config
                .extract_model
                .as_ref()
                .or(Some(&self.manifest.model))
                .map(model_audit_from_manifest),
        )
        .with_memory_settings(&self.config);
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
        let output_state = MemoryStagingOutputState::new(
            view.clone(),
            Arc::clone(&self.workspace_client),
            source,
            audit.run_id.to_string(),
        );
        let client = if let Some(model) = self.config.extract_model.as_ref() {
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
        let mut manifest = self.manifest.clone();
        if let Some(model) = self.config.extract_model.clone() {
            manifest.model = model;
        }

        let cancel_observer = move |sender: tokio::sync::mpsc::Sender<()>| {
            tokio::spawn(async move {
                cancellation.cancelled().await;
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
                    .extract_worker_max_turns
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

        context.generation_fence.ensure_current()?;
        let next_pointer = memory::ExtractPointerPayload {
            processed_through_entry: capture.entry_count - 1,
            processed_through_history_len: capture.history.len(),
            staging_id: pointer_staging_id,
        };
        let payload = serde_json::to_value(&next_pointer).map_err(hook_internal)?;
        if !self
            .extensions
            .append_if_current(&capture.location(), extract::EXTRACT_DOMAIN, payload)
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

impl MemoryExtractionTask {
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
            WorkerRunResult::RolledBack => {
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
    let pointer = memory::extract::fold_pointer(&capture.extensions);
    if pointer.is_none()
        && capture
            .extensions
            .iter()
            .any(|(domain, _)| domain == extract::EXTRACT_DOMAIN)
    {
        return Err(hook_internal(
            "latest committed Memory extraction pointer is malformed",
        ));
    }
    Ok(pointer)
}

fn extraction_threshold_reached(
    capture: &CommittedSessionCapture,
    pointer: Option<&memory::ExtractPointerPayload>,
    config: &manifest::MemoryConfig,
) -> bool {
    if capture.history.is_empty() {
        return false;
    }
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
    let Some(threshold) = config.extract_threshold.filter(|threshold| *threshold > 0) else {
        return false;
    };
    current.saturating_sub(baseline) >= threshold
}

#[derive(Clone)]
struct WorkerAuditBase {
    run_id: uuid::Uuid,
    worker: memory::audit::AuditWorker,
    trigger: memory::audit::AuditTrigger,
    memory_settings: Option<memory::audit::MemorySettingsAudit>,
    model: Option<memory::audit::ModelAudit>,
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
        }
    }

    fn with_memory_settings(mut self, config: &manifest::MemoryConfig) -> Self {
        self.memory_settings =
            config
                .workspace_settings()
                .map(|snapshot| memory::audit::MemorySettingsAudit {
                    workspace_id: snapshot.workspace_id,
                    settings_revision: snapshot.settings_revision,
                    language: snapshot.language,
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
        let _ = workspace_client
            .execute_memory_backend_operation(memory::backend::MemoryBackendOperation::AppendAudit(
                memory::backend::MemoryAppendAuditOperation {
                    event: memory::audit::AuditEvent::new(
                        memory::audit::AuditPayload::WorkerLifecycle(payload),
                    ),
                },
            ))
            .await;
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
    use agen::{HistoryEntry, Item, UsageRecord};

    use super::*;
    use crate::feature::background::{BackgroundTaskRewritePolicy, BackgroundTaskShutdownPolicy};
    use crate::session_history::SessionHistoryMetadata;

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
        let spec = memory_extraction_task_spec();
        assert_eq!(spec.trigger, BackgroundTaskTrigger::RunCommitted);
        assert_eq!(spec.max_concurrency, 1);
        assert_eq!(spec.rewrite, BackgroundTaskRewritePolicy::CancelAndWait);
        assert_eq!(spec.shutdown, BackgroundTaskShutdownPolicy::CancelAndWait);
    }

    #[test]
    fn threshold_uses_committed_usage_after_pointer() {
        let capture = capture(2, 250);
        let mut config = manifest::MemoryConfig::default();
        config.extract_threshold = Some(1);
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
        assert_eq!(extract_pointer(&capture).unwrap(), Some(latest));
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
        let controller_source = include_str!("../../controller.rs");
        assert!(controller_source.contains("if feature_config.memory.enabled"));
        assert!(controller_source.contains("MemoryExtractionLifecycleFeature::new"));
        let internal_worker_source = include_str!("../../internal_worker.rs");
        assert!(!internal_worker_source.contains("manifest.memory = None"));
    }

    #[test]
    fn empty_capture_never_schedules_extraction() {
        let capture = capture(0, 500);
        let mut config = manifest::MemoryConfig::default();
        config.extract_threshold = Some(1);
        assert!(!extraction_threshold_reached(&capture, None, &config));
    }
}
