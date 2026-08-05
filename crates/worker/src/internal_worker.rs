//! Reusable execution substrate for Worker-backed internal jobs.
//!
//! Internal jobs are intentionally not Runtime-catalogued Workers. Each run still owns a
//! distinct Worker identity and executes through [`Worker`], including feature installation,
//! Workspace authority, session history, lifecycle records, usage accounting, cancellation,
//! and error handling. The session store is in-memory and dropped with the run; callers that
//! need durable domain audit must keep using their domain authority (for example Memory audit).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use llm_engine::timeline::event::UsageEvent;
use llm_engine::{Engine, llm_client::LlmClient};
use manifest::{Scope, WorkerManifest};
use session_store::{LogEntry, SegmentId, SessionId, Store, StoreError, TraceEntry};
use uuid::Uuid;

use crate::feature::FeatureRegistryBuilder;
use crate::worker::{
    Worker, WorkerError, WorkerFilesystemAuthority, WorkerRunResult, WorkerWorkspaceContext,
};

/// Per-run identity for an internal Worker that is not registered in the public Runtime catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InternalWorkerIdentity {
    pub kind: &'static str,
    pub run_id: Uuid,
}

/// Explicit authority granted to one internal Worker run.
///
/// Extraction currently receives Workspace authority but no filesystem authority. Workdir
/// capabilities for Flow verifiers are deliberately left to the downstream Flow ticket.
pub(crate) struct InternalWorkerAuthority {
    pub workspace: WorkerWorkspaceContext,
    pub filesystem: WorkerFilesystemAuthority,
    pub scope: Scope,
}

pub(crate) struct InternalWorkerSpec {
    pub identity: InternalWorkerIdentity,
    pub manifest: WorkerManifest,
    pub client: Box<dyn LlmClient>,
    pub system_prompt: String,
    pub input: String,
    pub cache_key: Option<String>,
    pub max_turns: Option<u32>,
    pub features: FeatureRegistryBuilder,
    pub required_tools: &'static [&'static str],
    pub authority: InternalWorkerAuthority,
}

pub(crate) struct InternalWorkerResult {
    pub usage: Option<UsageEvent>,
    pub identity: InternalWorkerIdentity,
    pub lifecycle: WorkerRunResult,
    pub history_entries: usize,
}

pub(crate) struct InternalWorkerError {
    pub source: WorkerError,
    pub usage: Option<UsageEvent>,
    pub identity: InternalWorkerIdentity,
    pub history_entries: usize,
}

/// Execute an internal job through the normal Worker substrate.
///
/// The caller supplies the effective model client, an explicitly restricted feature set, and
/// explicit authority. No tools are registered directly on `Engine`, and no ambient filesystem
/// authority is inferred.
pub(crate) async fn run_internal_worker(
    spec: InternalWorkerSpec,
) -> Result<InternalWorkerResult, InternalWorkerError> {
    let InternalWorkerSpec {
        identity,
        mut manifest,
        client,
        system_prompt,
        input,
        cache_key,
        max_turns,
        features,
        required_tools,
        authority,
    } = spec;

    // Internal identities are run-scoped and never enter the public Runtime Worker catalog.
    manifest.worker.name = format!("internal-{}-{}", identity.kind, identity.run_id);
    // Internal jobs only receive features supplied below. A parent manifest must not accidentally
    // grant its normal public tool surface or recursively schedule memory work.
    manifest.feature = Default::default();
    manifest.plugins = Default::default();
    manifest.mcp = Default::default();
    manifest.skills = None;
    manifest.compaction = None;
    manifest.memory = None;

    let last_usage = Arc::new(Mutex::new(None::<UsageEvent>));
    let usage_slot = last_usage.clone();
    let mut engine = Engine::new(client).system_prompt(system_prompt);
    engine.on_usage(move |usage| {
        if let Ok(mut slot) = usage_slot.lock() {
            *slot = Some(usage.clone());
        }
    });
    engine.set_cache_key(cache_key);
    engine.set_max_turns(max_turns);
    let store = EphemeralSessionStore::default();
    let mut worker = Worker::new(
        manifest,
        engine,
        store.clone(),
        authority.workspace,
        authority.filesystem,
        authority.scope,
    )
    .await
    .map_err(|source| InternalWorkerError {
        source,
        usage: last_usage.lock().ok().and_then(|slot| slot.clone()),
        identity: identity.clone(),
        history_entries: 0,
    })?;

    let install_report = worker.install_features(features);
    let installed_tools = install_report.installed_tool_names();
    let required_tools_missing = required_tools.iter().any(|required| {
        !installed_tools
            .iter()
            .any(|installed| installed == required)
    });
    let install_failed = install_report
        .reports
        .iter()
        .any(|report| !report.installed);
    if install_failed || required_tools_missing {
        let diagnostics = install_report
            .reports
            .iter()
            .flat_map(|report| report.diagnostics.iter())
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        let missing = required_tools
            .iter()
            .filter(|required| {
                !installed_tools
                    .iter()
                    .any(|installed| installed == **required)
            })
            .copied()
            .collect::<Vec<_>>()
            .join(", ");
        return Err(InternalWorkerError {
            source: WorkerError::FeatureInstall(format!(
                "internal Worker feature installation failed: {diagnostics}; missing tools: {missing}"
            )),
            usage: last_usage.lock().ok().and_then(|slot| slot.clone()),
            identity,
            history_entries: 0,
        });
    }
    let session_id = worker.session_id();
    let segment_id = worker.segment_id();

    match worker.run_text(&input).await {
        Ok(lifecycle) => Ok(InternalWorkerResult {
            usage: last_usage.lock().ok().and_then(|slot| slot.clone()),
            identity,
            lifecycle,
            history_entries: store.entries_count(session_id, segment_id),
        }),
        Err(source) => Err(InternalWorkerError {
            source,
            usage: last_usage.lock().ok().and_then(|slot| slot.clone()),
            identity,
            history_entries: store.entries_count(session_id, segment_id),
        }),
    }
}

/// Session history for an ephemeral internal Worker.
///
/// Keeping the normal Store contract makes history/lifecycle/error records identical to a normal
/// Worker while avoiding a second public persistence/catalog policy for helper executions.
#[derive(Clone, Default)]
struct EphemeralSessionStore {
    entries: Arc<Mutex<HashMap<(SessionId, SegmentId), Vec<LogEntry>>>>,
    traces: Arc<Mutex<HashMap<(SessionId, SegmentId), Vec<TraceEntry>>>>,
}

impl EphemeralSessionStore {
    fn entries_count(&self, session_id: SessionId, segment_id: SegmentId) -> usize {
        self.entries
            .lock()
            .ok()
            .and_then(|entries| entries.get(&(session_id, segment_id)).map(Vec::len))
            .unwrap_or_default()
    }
}

impl Store for EphemeralSessionStore {
    fn append(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
        entry: &LogEntry,
    ) -> Result<(), StoreError> {
        self.entries
            .lock()
            .expect("ephemeral session store mutex poisoned")
            .entry((session_id, segment_id))
            .or_default()
            .push(entry.clone());
        Ok(())
    }

    fn read_all(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
    ) -> Result<Vec<LogEntry>, StoreError> {
        Ok(self
            .entries
            .lock()
            .expect("ephemeral session store mutex poisoned")
            .get(&(session_id, segment_id))
            .cloned()
            .unwrap_or_default())
    }

    fn list_sessions(&self) -> Result<Vec<SessionId>, StoreError> {
        let mut sessions = self
            .entries
            .lock()
            .expect("ephemeral session store mutex poisoned")
            .keys()
            .map(|(session_id, _)| *session_id)
            .collect::<Vec<_>>();
        sessions.sort_unstable();
        sessions.dedup();
        sessions.reverse();
        Ok(sessions)
    }

    fn list_segments(&self, session_id: SessionId) -> Result<Vec<SegmentId>, StoreError> {
        let mut segments = self
            .entries
            .lock()
            .expect("ephemeral session store mutex poisoned")
            .keys()
            .filter_map(|(entry_session_id, segment_id)| {
                (*entry_session_id == session_id).then_some(*segment_id)
            })
            .collect::<Vec<_>>();
        segments.sort_unstable();
        segments.reverse();
        Ok(segments)
    }

    fn lookup_session_of(&self, segment_id: SegmentId) -> Result<Option<SessionId>, StoreError> {
        Ok(self
            .entries
            .lock()
            .expect("ephemeral session store mutex poisoned")
            .keys()
            .find_map(|(session_id, entry_segment_id)| {
                (*entry_segment_id == segment_id).then_some(*session_id)
            }))
    }

    fn create_segment(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
        entries: &[LogEntry],
    ) -> Result<(), StoreError> {
        self.entries
            .lock()
            .expect("ephemeral session store mutex poisoned")
            .insert((session_id, segment_id), entries.to_vec());
        Ok(())
    }

    fn exists(&self, session_id: SessionId, segment_id: SegmentId) -> Result<bool, StoreError> {
        Ok(self
            .entries
            .lock()
            .expect("ephemeral session store mutex poisoned")
            .contains_key(&(session_id, segment_id)))
    }

    fn read_entry_count(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
    ) -> Result<usize, StoreError> {
        Ok(self.entries_count(session_id, segment_id))
    }

    fn append_trace(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
        entry: &TraceEntry,
    ) -> Result<(), StoreError> {
        self.traces
            .lock()
            .expect("ephemeral session trace store mutex poisoned")
            .entry((session_id, segment_id))
            .or_default()
            .push(entry.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use futures::Stream;
    use llm_engine::llm_client::event::{Event as LlmEvent, ResponseStatus, StatusEvent};
    use llm_engine::llm_client::{ClientError, Request};

    use super::*;

    #[derive(Clone)]
    struct OneTurnClient {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl LlmClient for OneTurnClient {
        fn clone_boxed(&self) -> Box<dyn LlmClient> {
            Box::new(self.clone())
        }

        async fn stream(
            &self,
            _request: Request,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent, ClientError>> + Send>>, ClientError>
        {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(Box::pin(futures::stream::iter(vec![
                Ok(LlmEvent::text_block_start(0)),
                Ok(LlmEvent::text_delta(0, "done")),
                Ok(LlmEvent::text_block_stop(0, None)),
                Ok(LlmEvent::Status(StatusEvent {
                    status: ResponseStatus::Completed,
                })),
            ])))
        }
    }

    fn manifest() -> WorkerManifest {
        WorkerManifest::from_toml(
            r#"
[worker]
name = "parent"

[model]
scheme = "anthropic"
model_id = "test-model"

[engine]

[[scope.allow]]
target = "/abs/scope"
permission = "write"
"#,
        )
        .unwrap()
    }

    fn spec(
        calls: Arc<AtomicUsize>,
        required_tools: &'static [&'static str],
    ) -> InternalWorkerSpec {
        InternalWorkerSpec {
            identity: InternalWorkerIdentity {
                kind: "test",
                run_id: Uuid::from_u128(1),
            },
            manifest: manifest(),
            client: Box::new(OneTurnClient { calls }),
            system_prompt: "system".to_string(),
            input: "input".to_string(),
            cache_key: Some("internal-test".to_string()),
            max_turns: Some(1),
            features: FeatureRegistryBuilder::new(),
            required_tools,
            authority: InternalWorkerAuthority {
                workspace: WorkerWorkspaceContext::no_workspace(),
                filesystem: WorkerFilesystemAuthority::None,
                scope: Scope::empty(),
            },
        }
    }

    #[tokio::test]
    async fn executes_through_worker_and_records_ephemeral_history() {
        let calls = Arc::new(AtomicUsize::new(0));
        let result = match run_internal_worker(spec(calls.clone(), &[])).await {
            Ok(result) => result,
            Err(error) => panic!("internal Worker should complete: {}", error.source),
        };

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(matches!(result.lifecycle, WorkerRunResult::Finished));
        assert!(result.history_entries >= 4);
        assert_eq!(result.identity.kind, "test");
    }

    #[tokio::test]
    async fn rejects_missing_explicit_tools_before_model_execution() {
        let calls = Arc::new(AtomicUsize::new(0));
        let error = match run_internal_worker(spec(calls.clone(), &["missing_tool"])).await {
            Err(error) => error,
            Ok(_) => panic!("required tool must be installed through the Worker registry"),
        };

        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(matches!(error.source, WorkerError::FeatureInstall(_)));
    }
}
