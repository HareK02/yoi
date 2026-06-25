use crate::catalog::{
    CreateWorkerRequest, WorkerDetail, WorkerLifecycleAck, WorkerStatus, WorkerSummary,
};
use crate::diagnostics::{DiagnosticSeverity, RuntimeDiagnostic};
use crate::error::RuntimeError;
use crate::identity::{RuntimeId, WorkerId, WorkerRef};
use crate::interaction::{WorkerInput, WorkerInputKind, WorkerInteractionAck};
use crate::management::{
    RuntimeBackendKind, RuntimeLimits, RuntimeOptions, RuntimeStatus, RuntimeSummary,
};
use crate::observation::{
    EventCursor, EventSubscription, EventSubscriptionMode, RuntimeEvent, RuntimeEventBatch,
    RuntimeEventKind, TranscriptEntry, TranscriptProjection, TranscriptQuery, TranscriptRole,
};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

static NEXT_RUNTIME_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// Concrete embedded Runtime domain entity.
///
/// The current implementation is memory-backed and tools/provider-less by
/// design.  It provides a typed API boundary that can later be adapted by
/// backend registries or web servers without making sockets, sessions, or paths
/// public authority.
#[derive(Clone, Debug)]
pub struct Runtime {
    inner: Arc<Mutex<RuntimeState>>,
}

impl Runtime {
    /// Create a memory-backed Runtime with generated identity and default limits.
    pub fn new_memory() -> Self {
        Self::with_options(RuntimeOptions::default())
    }

    /// Create a memory-backed Runtime with explicit options.
    pub fn with_options(options: RuntimeOptions) -> Self {
        let runtime_id = options.runtime_id.unwrap_or_else(|| {
            RuntimeId::generated(NEXT_RUNTIME_SEQUENCE.fetch_add(1, Ordering::Relaxed))
        });
        let mut state = RuntimeState::new(runtime_id, options.display_name, options.limits);
        state.push_event(None, RuntimeEventKind::RuntimeStarted, "runtime started");
        Self {
            inner: Arc::new(Mutex::new(state)),
        }
    }

    /// Runtime id half of public Worker authority.
    pub fn runtime_id(&self) -> Result<RuntimeId, RuntimeError> {
        Ok(self.lock()?.runtime_id.clone())
    }

    /// Management-plane summary.
    pub fn summary(&self) -> Result<RuntimeSummary, RuntimeError> {
        let state = self.lock()?;
        let mut active_worker_count = 0;
        let mut stopped_worker_count = 0;
        let mut cancelled_worker_count = 0;
        for worker in state.workers.values() {
            match worker.status {
                WorkerStatus::Running => active_worker_count += 1,
                WorkerStatus::Stopped => stopped_worker_count += 1,
                WorkerStatus::Cancelled => cancelled_worker_count += 1,
            }
        }

        Ok(RuntimeSummary {
            runtime_id: state.runtime_id.clone(),
            display_name: state.display_name.clone(),
            backend: state.backend,
            status: state.status,
            worker_count: state.workers.len(),
            active_worker_count,
            stopped_worker_count,
            cancelled_worker_count,
            diagnostic_count: state.diagnostics.len(),
            limits: state.limits.clone(),
        })
    }

    /// Current Runtime lifecycle state.
    pub fn status(&self) -> Result<RuntimeStatus, RuntimeError> {
        Ok(self.lock()?.status)
    }

    /// Stop the Runtime.  v0 keeps data readable after stop, but rejects new
    /// create/send/worker lifecycle mutations.
    pub fn stop_runtime(&self) -> Result<u64, RuntimeError> {
        let mut state = self.lock()?;
        if state.status == RuntimeStatus::Stopped {
            return Ok(state.last_event_id());
        }
        state.status = RuntimeStatus::Stopped;
        let runtime_id = state.runtime_id.clone();
        for worker in state.workers.values_mut() {
            if worker.status.is_active() {
                worker.status = WorkerStatus::Stopped;
            }
        }
        Ok(state.push_event(
            None,
            RuntimeEventKind::RuntimeStopped,
            format!("runtime {runtime_id} stopped"),
        ))
    }

    /// Create a Worker in the embedded catalog.
    pub fn create_worker(
        &self,
        request: CreateWorkerRequest,
    ) -> Result<WorkerDetail, RuntimeError> {
        let mut state = self.lock()?;
        state.ensure_running()?;
        validate_create_worker_request(&request)?;

        let worker_id = WorkerId::generated(state.next_worker_sequence);
        state.next_worker_sequence += 1;
        let worker_ref = WorkerRef::new(state.runtime_id.clone(), worker_id.clone());
        let event_id = state.push_event(
            Some(worker_ref.clone()),
            RuntimeEventKind::WorkerCreated,
            format!("worker {worker_id} created"),
        );

        let record = WorkerRecord {
            worker_ref,
            worker_id: worker_id.clone(),
            status: WorkerStatus::Running,
            request,
            transcript: Vec::new(),
            next_transcript_sequence: 1,
            last_event_id: event_id,
        };
        let detail = record.detail(&state.runtime_id);
        state.emit_create_diagnostics(&detail);
        state.workers.insert(worker_id, record);
        Ok(detail)
    }

    /// List Workers known to this Runtime.
    pub fn list_workers(&self) -> Result<Vec<WorkerSummary>, RuntimeError> {
        let state = self.lock()?;
        Ok(state
            .workers
            .values()
            .map(|worker| worker.summary(&state.runtime_id))
            .collect())
    }

    /// Fetch Worker detail.  The supplied [`WorkerRef`] must match this Runtime.
    pub fn worker_detail(&self, worker_ref: &WorkerRef) -> Result<WorkerDetail, RuntimeError> {
        let state = self.lock()?;
        let worker = state.worker(worker_ref)?;
        Ok(worker.detail(&state.runtime_id))
    }

    /// Accept input into a Worker transcript.
    pub fn send_input(
        &self,
        worker_ref: &WorkerRef,
        input: WorkerInput,
    ) -> Result<WorkerInteractionAck, RuntimeError> {
        let mut state = self.lock()?;
        state.ensure_running()?;
        validate_worker_input(&input)?;
        state.ensure_worker_ref(worker_ref)?;
        {
            let worker = state.worker(worker_ref)?;
            if !worker.status.is_active() {
                return Err(RuntimeError::InvalidRequest(format!(
                    "worker {} is not running",
                    worker_ref.worker_id
                )));
            }
        }

        let event_id = state.push_event(
            Some(worker_ref.clone()),
            RuntimeEventKind::WorkerInputAccepted,
            "worker input accepted",
        );
        let worker = state.worker_mut(worker_ref)?;

        let role = match input.kind {
            WorkerInputKind::User => TranscriptRole::User,
            WorkerInputKind::System => TranscriptRole::System,
        };
        let transcript_sequence = worker.next_transcript_sequence;
        worker.next_transcript_sequence += 1;
        worker.last_event_id = event_id;
        worker.transcript.push(TranscriptEntry {
            sequence: transcript_sequence,
            worker_ref: worker_ref.clone(),
            role,
            content: input.content,
            event_id,
        });

        Ok(WorkerInteractionAck {
            worker_ref: worker_ref.clone(),
            status: worker.status,
            transcript_sequence,
            event_id,
        })
    }

    /// Stop a Worker.  Repeated stops are idempotent and return the last event id.
    pub fn stop_worker(
        &self,
        worker_ref: &WorkerRef,
        reason: Option<String>,
    ) -> Result<WorkerLifecycleAck, RuntimeError> {
        self.transition_worker(
            worker_ref,
            WorkerStatus::Stopped,
            RuntimeEventKind::WorkerStopped,
            reason.unwrap_or_else(|| "worker stopped".to_string()),
        )
    }

    /// Cancel a Worker.  Repeated cancels are idempotent and return the last event id.
    pub fn cancel_worker(
        &self,
        worker_ref: &WorkerRef,
        reason: Option<String>,
    ) -> Result<WorkerLifecycleAck, RuntimeError> {
        self.transition_worker(
            worker_ref,
            WorkerStatus::Cancelled,
            RuntimeEventKind::WorkerCancelled,
            reason.unwrap_or_else(|| "worker cancelled".to_string()),
        )
    }

    /// Bounded transcript projection for a Worker.
    pub fn transcript_projection(
        &self,
        worker_ref: &WorkerRef,
        query: TranscriptQuery,
    ) -> Result<TranscriptProjection, RuntimeError> {
        let state = self.lock()?;
        if query.limit > state.limits.max_transcript_projection_items {
            return Err(RuntimeError::LimitTooLarge {
                requested: query.limit,
                max: state.limits.max_transcript_projection_items,
            });
        }
        let worker = state.worker(worker_ref)?;
        let total_items = worker.transcript.len();
        let end = query.start.saturating_add(query.limit).min(total_items);
        let items = if query.start >= total_items {
            Vec::new()
        } else {
            worker.transcript[query.start..end].to_vec()
        };
        let next_start = (end < total_items).then_some(end);
        Ok(TranscriptProjection {
            worker_ref: worker_ref.clone(),
            start: query.start,
            limit: query.limit,
            total_items,
            items,
            next_start,
        })
    }

    /// Cursor pointing to the beginning of Runtime events.
    pub fn event_cursor_from_start(&self) -> Result<EventCursor, RuntimeError> {
        let state = self.lock()?;
        Ok(EventCursor {
            runtime_id: state.runtime_id.clone(),
            next_event_id: 1,
        })
    }

    /// Cursor pointing after the current last event.
    pub fn event_cursor_now(&self) -> Result<EventCursor, RuntimeError> {
        let state = self.lock()?;
        Ok(EventCursor {
            runtime_id: state.runtime_id.clone(),
            next_event_id: state.last_event_id() + 1,
        })
    }

    /// Poll Runtime events from a cursor.
    pub fn read_events(
        &self,
        cursor: &EventCursor,
        limit: usize,
    ) -> Result<RuntimeEventBatch, RuntimeError> {
        let state = self.lock()?;
        if cursor.runtime_id != state.runtime_id {
            return Err(RuntimeError::WrongRuntimeCursor {
                expected_runtime_id: state.runtime_id.clone(),
                actual_runtime_id: cursor.runtime_id.clone(),
            });
        }
        if limit > state.limits.max_event_batch_items {
            return Err(RuntimeError::LimitTooLarge {
                requested: limit,
                max: state.limits.max_event_batch_items,
            });
        }

        let mut events = Vec::new();
        for event in state
            .events
            .iter()
            .filter(|event| event.id >= cursor.next_event_id)
            .take(limit)
        {
            events.push(event.clone());
        }
        let next_event_id = events
            .last()
            .map(|event| event.id + 1)
            .unwrap_or(cursor.next_event_id);
        let has_more = state.events.iter().any(|event| event.id >= next_event_id);
        Ok(RuntimeEventBatch {
            runtime_id: state.runtime_id.clone(),
            cursor: EventCursor {
                runtime_id: state.runtime_id.clone(),
                next_event_id,
            },
            events,
            has_more,
        })
    }

    /// Create a poll-only placeholder subscription boundary for future streaming.
    pub fn subscribe_events(&self, cursor: EventCursor) -> Result<EventSubscription, RuntimeError> {
        let state = self.lock()?;
        if cursor.runtime_id != state.runtime_id {
            return Err(RuntimeError::WrongRuntimeCursor {
                expected_runtime_id: state.runtime_id.clone(),
                actual_runtime_id: cursor.runtime_id,
            });
        }
        Ok(EventSubscription {
            runtime_id: state.runtime_id.clone(),
            cursor,
            mode: EventSubscriptionMode::PollOnly,
        })
    }

    /// Snapshot current diagnostics.
    pub fn diagnostics(&self) -> Result<Vec<RuntimeDiagnostic>, RuntimeError> {
        Ok(self.lock()?.diagnostics.clone())
    }

    fn transition_worker(
        &self,
        worker_ref: &WorkerRef,
        status: WorkerStatus,
        event_kind: RuntimeEventKind,
        reason: String,
    ) -> Result<WorkerLifecycleAck, RuntimeError> {
        let mut state = self.lock()?;
        state.ensure_running()?;
        state.ensure_worker_ref(worker_ref)?;

        let already_status = {
            let worker = state.worker(worker_ref)?;
            worker.status == status
        };
        if already_status {
            let worker = state.worker(worker_ref)?;
            return Ok(WorkerLifecycleAck {
                worker_ref: worker_ref.clone(),
                status: worker.status,
                event_id: worker.last_event_id,
            });
        }

        let event_id = state.push_event(Some(worker_ref.clone()), event_kind, reason);
        let worker = state.worker_mut(worker_ref)?;
        worker.status = status;
        worker.last_event_id = event_id;
        Ok(WorkerLifecycleAck {
            worker_ref: worker_ref.clone(),
            status: worker.status,
            event_id,
        })
    }

    fn lock(&self) -> Result<MutexGuard<'_, RuntimeState>, RuntimeError> {
        self.inner.lock().map_err(|_| RuntimeError::StatePoisoned)
    }
}

#[derive(Debug)]
struct RuntimeState {
    runtime_id: RuntimeId,
    display_name: Option<String>,
    backend: RuntimeBackendKind,
    status: RuntimeStatus,
    limits: RuntimeLimits,
    next_worker_sequence: u64,
    next_event_id: u64,
    next_diagnostic_id: u64,
    workers: BTreeMap<WorkerId, WorkerRecord>,
    events: Vec<RuntimeEvent>,
    diagnostics: Vec<RuntimeDiagnostic>,
}

impl RuntimeState {
    fn new(runtime_id: RuntimeId, display_name: Option<String>, limits: RuntimeLimits) -> Self {
        Self {
            runtime_id,
            display_name,
            backend: RuntimeBackendKind::Memory,
            status: RuntimeStatus::Running,
            limits,
            next_worker_sequence: 1,
            next_event_id: 1,
            next_diagnostic_id: 1,
            workers: BTreeMap::new(),
            events: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn ensure_running(&self) -> Result<(), RuntimeError> {
        if self.status == RuntimeStatus::Stopped {
            Err(RuntimeError::RuntimeStopped {
                runtime_id: self.runtime_id.clone(),
            })
        } else {
            Ok(())
        }
    }

    fn ensure_worker_ref(&self, worker_ref: &WorkerRef) -> Result<(), RuntimeError> {
        if worker_ref.runtime_id != self.runtime_id {
            return Err(RuntimeError::WrongRuntime {
                expected_runtime_id: self.runtime_id.clone(),
                actual_runtime_id: worker_ref.runtime_id.clone(),
                worker_id: worker_ref.worker_id.clone(),
            });
        }
        if !self.workers.contains_key(&worker_ref.worker_id) {
            return Err(RuntimeError::WorkerNotFound {
                runtime_id: self.runtime_id.clone(),
                worker_id: worker_ref.worker_id.clone(),
            });
        }
        Ok(())
    }

    fn worker(&self, worker_ref: &WorkerRef) -> Result<&WorkerRecord, RuntimeError> {
        self.ensure_worker_ref(worker_ref)?;
        self.workers
            .get(&worker_ref.worker_id)
            .ok_or_else(|| RuntimeError::WorkerNotFound {
                runtime_id: self.runtime_id.clone(),
                worker_id: worker_ref.worker_id.clone(),
            })
    }

    fn worker_mut(&mut self, worker_ref: &WorkerRef) -> Result<&mut WorkerRecord, RuntimeError> {
        self.ensure_worker_ref(worker_ref)?;
        self.workers
            .get_mut(&worker_ref.worker_id)
            .ok_or_else(|| RuntimeError::WorkerNotFound {
                runtime_id: self.runtime_id.clone(),
                worker_id: worker_ref.worker_id.clone(),
            })
    }

    fn push_event(
        &mut self,
        worker_ref: Option<WorkerRef>,
        kind: RuntimeEventKind,
        message: impl Into<String>,
    ) -> u64 {
        let id = self.next_event_id;
        self.next_event_id += 1;
        self.events.push(RuntimeEvent {
            id,
            worker_ref,
            kind,
            message: message.into(),
        });
        id
    }

    fn last_event_id(&self) -> u64 {
        self.next_event_id.saturating_sub(1)
    }

    fn push_diagnostic(
        &mut self,
        severity: DiagnosticSeverity,
        code: impl Into<String>,
        message: impl Into<String>,
        worker_ref: Option<WorkerRef>,
    ) {
        let id = self.next_diagnostic_id;
        self.next_diagnostic_id += 1;
        self.diagnostics.push(RuntimeDiagnostic {
            id,
            severity,
            code: code.into(),
            message: message.into(),
            worker_ref,
        });
    }

    fn emit_create_diagnostics(&mut self, detail: &WorkerDetail) {
        if detail.config_bundle.is_none() {
            self.push_diagnostic(
                DiagnosticSeverity::Info,
                "runtime.local_default_resources",
                "worker created without ConfigBundleRef; runtime-local defaults are assumed",
                Some(detail.worker_ref.clone()),
            );
        }
        if detail.requested_capabilities.is_empty() {
            self.push_diagnostic(
                DiagnosticSeverity::Info,
                "worker.tools_less",
                "worker created without requested tool capabilities",
                Some(detail.worker_ref.clone()),
            );
        }
    }
}

#[derive(Debug)]
struct WorkerRecord {
    worker_ref: WorkerRef,
    worker_id: WorkerId,
    status: WorkerStatus,
    request: CreateWorkerRequest,
    transcript: Vec<TranscriptEntry>,
    next_transcript_sequence: u64,
    last_event_id: u64,
}

impl WorkerRecord {
    fn summary(&self, runtime_id: &RuntimeId) -> WorkerSummary {
        WorkerSummary {
            worker_ref: self.worker_ref.clone(),
            runtime_id: runtime_id.clone(),
            worker_id: self.worker_id.clone(),
            status: self.status,
            intent: self.request.intent.clone(),
            profile: self.request.profile.clone(),
            requested_capability_count: self.request.requested_capabilities.len(),
            has_config_bundle: self.request.config_bundle.is_some(),
            transcript_len: self.transcript.len(),
            last_event_id: self.last_event_id,
        }
    }

    fn detail(&self, runtime_id: &RuntimeId) -> WorkerDetail {
        WorkerDetail {
            worker_ref: self.worker_ref.clone(),
            runtime_id: runtime_id.clone(),
            worker_id: self.worker_id.clone(),
            status: self.status,
            intent: self.request.intent.clone(),
            profile: self.request.profile.clone(),
            config_bundle: self.request.config_bundle.clone(),
            requested_capabilities: self.request.requested_capabilities.clone(),
            workspace_refs: self.request.workspace_refs.clone(),
            mount_refs: self.request.mount_refs.clone(),
            transcript_len: self.transcript.len(),
            last_event_id: self.last_event_id,
        }
    }
}

fn validate_create_worker_request(request: &CreateWorkerRequest) -> Result<(), RuntimeError> {
    if let crate::catalog::WorkerIntent::Task { objective } = &request.intent {
        if objective.trim().is_empty() {
            return Err(RuntimeError::InvalidRequest(
                "task objective must not be empty".to_string(),
            ));
        }
    }
    for capability in &request.requested_capabilities {
        if capability.name.trim().is_empty() {
            return Err(RuntimeError::InvalidRequest(
                "capability name must not be empty".to_string(),
            ));
        }
    }
    Ok(())
}

fn validate_worker_input(input: &WorkerInput) -> Result<(), RuntimeError> {
    if input.content.trim().is_empty() {
        return Err(RuntimeError::InvalidRequest(
            "worker input content must not be empty".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{CapabilityRequest, ConfigBundleRef, ProfileSelector, WorkerIntent};
    use crate::management::RuntimeLimits;

    fn task_request(objective: &str) -> CreateWorkerRequest {
        CreateWorkerRequest {
            intent: WorkerIntent::Task {
                objective: objective.to_string(),
            },
            profile: ProfileSelector::Builtin("builtin:coder".to_string()),
            config_bundle: Some(ConfigBundleRef {
                id: "bundle-1".to_string(),
            }),
            requested_capabilities: vec![CapabilityRequest::named("read")],
            workspace_refs: Vec::new(),
            mount_refs: Vec::new(),
        }
    }

    #[test]
    fn create_list_and_detail_preserve_runtime_worker_authority() {
        let runtime = Runtime::new_memory();
        let detail = runtime.create_worker(task_request("implement v0")).unwrap();

        assert_eq!(detail.worker_ref.runtime_id, runtime.runtime_id().unwrap());
        assert_eq!(detail.status, WorkerStatus::Running);
        assert!(detail.config_bundle.is_some());

        let list = runtime.list_workers().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].worker_ref, detail.worker_ref);
        assert_eq!(list[0].requested_capability_count, 1);

        let fetched = runtime.worker_detail(&detail.worker_ref).unwrap();
        assert_eq!(fetched.worker_id, detail.worker_id);
        assert_eq!(fetched.intent, detail.intent);
    }

    #[test]
    fn rejects_worker_refs_from_another_runtime() {
        let runtime_a = Runtime::new_memory();
        let runtime_b = Runtime::new_memory();
        let detail = runtime_a.create_worker(task_request("runtime a")).unwrap();

        let err = runtime_b.worker_detail(&detail.worker_ref).unwrap_err();
        assert!(matches!(err, RuntimeError::WrongRuntime { .. }));
    }

    #[test]
    fn tools_less_worker_without_config_bundle_uses_local_defaults_and_diagnostics() {
        let runtime = Runtime::new_memory();
        let detail = runtime
            .create_worker(CreateWorkerRequest::tools_less(
                WorkerIntent::default(),
                ProfileSelector::RuntimeDefault,
            ))
            .unwrap();

        assert!(detail.config_bundle.is_none());
        assert!(detail.requested_capabilities.is_empty());
        let diagnostics = runtime.diagnostics().unwrap();
        assert_eq!(diagnostics.len(), 2);
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "runtime.local_default_resources")
        );
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "worker.tools_less")
        );
    }

    #[test]
    fn send_input_and_project_bounded_transcript() {
        let runtime = Runtime::with_options(RuntimeOptions {
            limits: RuntimeLimits {
                max_transcript_projection_items: 2,
                max_event_batch_items: 16,
            },
            ..RuntimeOptions::default()
        });
        let detail = runtime.create_worker(task_request("chat")).unwrap();

        let first = runtime
            .send_input(&detail.worker_ref, WorkerInput::user("hello"))
            .unwrap();
        assert_eq!(first.transcript_sequence, 1);
        runtime
            .send_input(&detail.worker_ref, WorkerInput::system("note"))
            .unwrap();
        runtime
            .send_input(&detail.worker_ref, WorkerInput::user("again"))
            .unwrap();

        let projection = runtime
            .transcript_projection(&detail.worker_ref, TranscriptQuery::new(0, 2))
            .unwrap();
        assert_eq!(projection.total_items, 3);
        assert_eq!(projection.items.len(), 2);
        assert_eq!(projection.items[0].content, "hello");
        assert_eq!(projection.items[1].role, TranscriptRole::System);
        assert_eq!(projection.next_start, Some(2));

        let err = runtime
            .transcript_projection(&detail.worker_ref, TranscriptQuery::new(0, 3))
            .unwrap_err();
        assert!(matches!(err, RuntimeError::LimitTooLarge { .. }));
    }

    #[test]
    fn stop_and_cancel_workers_update_summary() {
        let runtime = Runtime::new_memory();
        let stopped = runtime.create_worker(task_request("stop me")).unwrap();
        let cancelled = runtime.create_worker(task_request("cancel me")).unwrap();

        let stop_ack = runtime
            .stop_worker(&stopped.worker_ref, Some("done".to_string()))
            .unwrap();
        assert_eq!(stop_ack.status, WorkerStatus::Stopped);

        let cancel_ack = runtime
            .cancel_worker(&cancelled.worker_ref, Some("abort".to_string()))
            .unwrap();
        assert_eq!(cancel_ack.status, WorkerStatus::Cancelled);

        let summary = runtime.summary().unwrap();
        assert_eq!(summary.worker_count, 2);
        assert_eq!(summary.active_worker_count, 0);
        assert_eq!(summary.stopped_worker_count, 1);
        assert_eq!(summary.cancelled_worker_count, 1);
    }

    #[test]
    fn event_cursor_and_poll_only_subscription_are_bounded_placeholders() {
        let runtime = Runtime::new_memory();
        let cursor = runtime.event_cursor_from_start().unwrap();
        let subscription = runtime.subscribe_events(cursor.clone()).unwrap();
        assert_eq!(subscription.mode, EventSubscriptionMode::PollOnly);

        let worker = runtime.create_worker(task_request("events")).unwrap();
        runtime
            .send_input(&worker.worker_ref, WorkerInput::user("eventful"))
            .unwrap();

        let batch = runtime.read_events(&cursor, 2).unwrap();
        assert_eq!(batch.events.len(), 2);
        assert!(batch.has_more);
        assert_eq!(batch.events[0].kind, RuntimeEventKind::RuntimeStarted);
        assert_eq!(batch.events[1].kind, RuntimeEventKind::WorkerCreated);

        let next = runtime.read_events(&batch.cursor, 2).unwrap();
        assert_eq!(next.events.len(), 1);
        assert_eq!(next.events[0].kind, RuntimeEventKind::WorkerInputAccepted);
        assert!(!next.has_more);
    }
}
