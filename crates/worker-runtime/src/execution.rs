use crate::catalog::CreateWorkerRequest;
use crate::error::RuntimeError;
use crate::identity::WorkerRef;
use crate::interaction::WorkerInput;
#[cfg(feature = "ws-server")]
use crate::observation::WorkerObservationEvent;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

/// Coarse execution attachment visible through Worker catalog/detail responses.
///
/// This deliberately does not expose backend handles, process paths, sockets,
/// credentials, session files, or manifest paths. It only says whether Runtime
/// has an execution backend attached for the Worker.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerExecutionBackendKind {
    #[default]
    Unconnected,
    /// A durable execution binding was restored, but no live handle was recovered.
    Stale,
    Connected,
}

/// Durable, non-authority execution binding projection.
///
/// This records only enough identity to diagnose stale mappings after restore.
/// It is not a live handle and must not contain sockets, paths, credentials, or
/// provider-private authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerExecutionBindingIdentity {
    pub backend_id: String,
}

impl WorkerExecutionBindingIdentity {
    pub fn from_handle(handle: &WorkerExecutionHandle) -> Self {
        Self {
            backend_id: handle.backend_id.clone(),
        }
    }
}

/// Current execution-side run state for a Worker.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerExecutionRunState {
    #[default]
    Unconnected,
    Idle,
    Busy,
    Rejected,
    Errored,
    Stopped,
}

/// Execution operation that produced a result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerExecutionOperation {
    Spawn,
    Input,
    Stop,
    Cancel,
}

/// Typed execution result class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerExecutionOutcome {
    Accepted,
    Busy,
    Rejected,
    Errored,
    Unsupported,
}

/// Backend result for a Worker execution operation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerExecutionResult {
    pub operation: WorkerExecutionOperation,
    pub outcome: WorkerExecutionOutcome,
    pub run_state: WorkerExecutionRunState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl WorkerExecutionResult {
    pub fn accepted(
        operation: WorkerExecutionOperation,
        run_state: WorkerExecutionRunState,
    ) -> Self {
        Self {
            operation,
            outcome: WorkerExecutionOutcome::Accepted,
            run_state,
            message: None,
        }
    }

    pub fn busy(operation: WorkerExecutionOperation, message: impl Into<String>) -> Self {
        Self {
            operation,
            outcome: WorkerExecutionOutcome::Busy,
            run_state: WorkerExecutionRunState::Busy,
            message: Some(message.into()),
        }
    }

    pub fn rejected(operation: WorkerExecutionOperation, message: impl Into<String>) -> Self {
        Self {
            operation,
            outcome: WorkerExecutionOutcome::Rejected,
            run_state: WorkerExecutionRunState::Rejected,
            message: Some(message.into()),
        }
    }

    pub fn errored(operation: WorkerExecutionOperation, message: impl Into<String>) -> Self {
        Self {
            operation,
            outcome: WorkerExecutionOutcome::Errored,
            run_state: WorkerExecutionRunState::Errored,
            message: Some(message.into()),
        }
    }

    pub fn unsupported(operation: WorkerExecutionOperation, message: impl Into<String>) -> Self {
        Self {
            operation,
            outcome: WorkerExecutionOutcome::Unsupported,
            run_state: WorkerExecutionRunState::Rejected,
            message: Some(message.into()),
        }
    }

    pub fn is_accepted(&self) -> bool {
        self.outcome == WorkerExecutionOutcome::Accepted
    }

    pub fn message_or_default(&self) -> String {
        self.message
            .clone()
            .unwrap_or_else(|| format!("{:?} {:?}", self.operation, self.outcome))
    }
}

/// Execution status surfaced in Worker summary/detail responses.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerExecutionStatus {
    pub backend: WorkerExecutionBackendKind,
    pub run_state: WorkerExecutionRunState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<WorkerExecutionBindingIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_result: Option<WorkerExecutionResult>,
}

impl WorkerExecutionStatus {
    pub fn unconnected() -> Self {
        Self::default()
    }

    pub fn connected(run_state: WorkerExecutionRunState) -> Self {
        Self {
            backend: WorkerExecutionBackendKind::Connected,
            run_state,
            binding: None,
            last_result: None,
        }
    }

    pub fn stale(mut previous: Self) -> Self {
        previous.backend = WorkerExecutionBackendKind::Stale;
        previous.run_state = WorkerExecutionRunState::Unconnected;
        previous
    }

    pub fn with_binding(mut self, binding: WorkerExecutionBindingIdentity) -> Self {
        self.binding = Some(binding);
        self
    }

    pub fn with_result(mut self, result: WorkerExecutionResult) -> Self {
        self.run_state = result.run_state;
        self.last_result = Some(result);
        self
    }
}

/// Opaque per-Worker execution handle returned by a backend.
///
/// The handle is a typed token for routing calls back into the same backend. It
/// intentionally contains no socket path, process id, credential, manifest path,
/// or session path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerExecutionHandle {
    worker_ref: WorkerRef,
    backend_id: String,
}

impl WorkerExecutionHandle {
    pub fn new(worker_ref: WorkerRef, backend_id: impl Into<String>) -> Self {
        Self {
            worker_ref,
            backend_id: backend_id.into(),
        }
    }

    pub fn worker_ref(&self) -> &WorkerRef {
        &self.worker_ref
    }

    pub fn backend_id(&self) -> &str {
        &self.backend_id
    }
}

/// Runtime hooks available to an execution backend for one Worker.
#[derive(Clone)]
pub struct WorkerExecutionContext {
    worker_ref: WorkerRef,
    #[cfg(feature = "ws-server")]
    observation_publisher: Arc<
        dyn Fn(WorkerRef, protocol::Event) -> Result<WorkerObservationEvent, RuntimeError>
            + Send
            + Sync,
    >,
}

impl WorkerExecutionContext {
    #[cfg(feature = "ws-server")]
    pub(crate) fn new(
        worker_ref: WorkerRef,
        observation_publisher: Arc<
            dyn Fn(WorkerRef, protocol::Event) -> Result<WorkerObservationEvent, RuntimeError>
                + Send
                + Sync,
        >,
    ) -> Self {
        Self {
            worker_ref,
            observation_publisher,
        }
    }

    #[cfg(not(feature = "ws-server"))]
    pub(crate) fn new(worker_ref: WorkerRef) -> Self {
        Self { worker_ref }
    }

    pub fn worker_ref(&self) -> &WorkerRef {
        &self.worker_ref
    }

    /// Publish a protocol event into the Runtime observation bus.
    #[cfg(feature = "ws-server")]
    pub fn publish_protocol_event(
        &self,
        payload: protocol::Event,
    ) -> Result<WorkerObservationEvent, RuntimeError> {
        (self.observation_publisher)(self.worker_ref.clone(), payload)
    }
}

impl fmt::Debug for WorkerExecutionContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkerExecutionContext")
            .field("worker_ref", &self.worker_ref)
            .finish_non_exhaustive()
    }
}

/// Spawn/initialization request passed to an execution backend.
#[derive(Clone, Debug)]
pub struct WorkerExecutionSpawnRequest {
    pub worker_ref: WorkerRef,
    pub request: CreateWorkerRequest,
    pub context: WorkerExecutionContext,
}

/// Result of backend Worker spawn/initialization.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkerExecutionSpawnResult {
    Connected {
        handle: WorkerExecutionHandle,
        run_state: WorkerExecutionRunState,
    },
    Rejected(WorkerExecutionResult),
    Errored(WorkerExecutionResult),
}

/// Backend boundary for Worker execution.
///
/// Runtime owns Worker catalog, transcript, observation, and lifecycle state. A
/// backend owns concrete execution. The default Runtime has no backend, so input
/// to those Workers is rejected instead of producing providerless responses.
pub trait WorkerExecutionBackend: Send + Sync + 'static {
    fn backend_id(&self) -> &str;

    fn spawn_worker(&self, request: WorkerExecutionSpawnRequest) -> WorkerExecutionSpawnResult;

    fn dispatch_input(
        &self,
        handle: &WorkerExecutionHandle,
        input: WorkerInput,
    ) -> WorkerExecutionResult;

    fn stop_worker(&self, _handle: &WorkerExecutionHandle) -> WorkerExecutionResult {
        WorkerExecutionResult::unsupported(
            WorkerExecutionOperation::Stop,
            "execution backend does not support stopping workers",
        )
    }

    fn cancel_worker(&self, _handle: &WorkerExecutionHandle) -> WorkerExecutionResult {
        WorkerExecutionResult::unsupported(
            WorkerExecutionOperation::Cancel,
            "execution backend does not support cancelling workers",
        )
    }
}

#[derive(Clone)]
pub(crate) struct WorkerExecutionBackendRef {
    id: String,
    backend: Arc<dyn WorkerExecutionBackend>,
}

impl WorkerExecutionBackendRef {
    pub(crate) fn new(backend: Arc<dyn WorkerExecutionBackend>) -> Result<Self, RuntimeError> {
        let id = backend.backend_id().trim().to_string();
        if id.is_empty() {
            return Err(RuntimeError::InvalidRequest(
                "execution backend id must not be empty".to_string(),
            ));
        }
        Ok(Self { id, backend })
    }

    pub(crate) fn spawn_worker(
        &self,
        request: WorkerExecutionSpawnRequest,
    ) -> WorkerExecutionSpawnResult {
        self.backend.spawn_worker(request)
    }

    pub(crate) fn dispatch_input(
        &self,
        handle: &WorkerExecutionHandle,
        input: WorkerInput,
    ) -> WorkerExecutionResult {
        self.backend.dispatch_input(handle, input)
    }

    pub(crate) fn stop_worker(&self, handle: &WorkerExecutionHandle) -> WorkerExecutionResult {
        self.backend.stop_worker(handle)
    }

    pub(crate) fn cancel_worker(&self, handle: &WorkerExecutionHandle) -> WorkerExecutionResult {
        self.backend.cancel_worker(handle)
    }
}

impl fmt::Debug for WorkerExecutionBackendRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkerExecutionBackendRef")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}
