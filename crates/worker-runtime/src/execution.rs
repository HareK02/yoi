use crate::catalog::{WorkingDirectoryRequest, WorkingDirectoryStatus};
use crate::config_bundle::ConfigBundle;
use crate::error::RuntimeError;
use crate::identity::WorkerRef;
use crate::interaction::WorkerInput;
#[cfg(feature = "ws-server")]
use crate::observation::WorkerObservationEvent;
use crate::working_directory::{WorkingDirectoryBinding, WorkingDirectoryDiagnostic};
use protocol::Method;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

/// Current execution-side run state for a Worker.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerExecutionRunState {
    #[default]
    Stopped,
    Idle,
    Busy,
    Rejected,
    Errored,
}

/// Execution operation that produced a result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerExecutionOperation {
    Spawn,
    Restore,
    Input,
    ProtocolMethod,
    Stop,
    Cancel,
}

/// Typed execution result class. Results are transient operation outcomes and
/// are not persisted as Worker lifecycle authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerExecutionResult {
    pub operation: WorkerExecutionOperation,
    pub outcome: WorkerExecutionOutcome,
    pub run_state: WorkerExecutionRunState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Backend result class for a Worker execution operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerExecutionOutcome {
    Accepted,
    Busy,
    Rejected,
    Errored,
    Unsupported,
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
            run_state: WorkerExecutionRunState::Stopped,
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
            run_state: WorkerExecutionRunState::Stopped,
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

    #[cfg(feature = "ws-server")]
    pub fn publish_observation(
        &self,
        payload: protocol::Event,
    ) -> Result<WorkerObservationEvent, RuntimeError> {
        (self.observation_publisher)(self.worker_ref.clone(), payload)
    }

    #[cfg(feature = "ws-server")]
    pub fn publish_protocol_event(
        &self,
        payload: protocol::Event,
    ) -> Result<WorkerObservationEvent, RuntimeError> {
        self.publish_observation(payload)
    }
}

impl fmt::Debug for WorkerExecutionContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkerExecutionContext")
            .field("worker_ref", &self.worker_ref)
            .finish_non_exhaustive()
    }
}

/// Request passed to a [`WorkerExecutionBackend`] when spawning a Worker.
#[derive(Clone, Debug)]
pub struct WorkerExecutionSpawnRequest {
    pub worker_ref: WorkerRef,
    pub request: crate::catalog::CreateWorkerRequest,
    pub context: WorkerExecutionContext,
    pub working_directory: Option<WorkingDirectoryBinding>,
    pub config_bundle: Option<ConfigBundle>,
}

/// Request passed to a [`WorkerExecutionBackend`] when restoring a persisted Worker.
#[derive(Clone, Debug)]
pub struct WorkerExecutionRestoreRequest {
    pub worker_ref: WorkerRef,
    pub request: crate::catalog::CreateWorkerRequest,
    pub context: WorkerExecutionContext,
    pub previous_working_directory: Option<WorkingDirectoryStatus>,
    pub working_directory: Option<WorkingDirectoryBinding>,
    pub config_bundle: Option<ConfigBundle>,
}

/// Backend outcome for Worker spawn/restore operations.
#[derive(Clone, Debug)]
pub enum WorkerExecutionSpawnResult {
    Connected {
        handle: WorkerExecutionHandle,
        run_state: WorkerExecutionRunState,
        working_directory: Option<WorkingDirectoryStatus>,
    },
    Rejected(WorkerExecutionResult),
    Errored(WorkerExecutionResult),
}

impl WorkerExecutionSpawnResult {
    pub fn connected(
        handle: WorkerExecutionHandle,
        run_state: WorkerExecutionRunState,
        working_directory: Option<WorkingDirectoryStatus>,
    ) -> Self {
        Self::Connected {
            handle,
            run_state,
            working_directory,
        }
    }
}

pub trait WorkerExecutionBackend: Send + Sync + 'static {
    fn backend_id(&self) -> &str;

    fn spawn_worker(&self, request: WorkerExecutionSpawnRequest) -> WorkerExecutionSpawnResult;

    fn restore_worker(
        &self,
        _request: WorkerExecutionRestoreRequest,
    ) -> WorkerExecutionSpawnResult {
        WorkerExecutionSpawnResult::Rejected(WorkerExecutionResult::unsupported(
            WorkerExecutionOperation::Restore,
            "execution backend does not support restoring workers",
        ))
    }

    fn create_working_directory(
        &self,
        _request: &WorkingDirectoryRequest,
    ) -> Result<WorkingDirectoryStatus, WorkingDirectoryDiagnostic> {
        Err(WorkingDirectoryDiagnostic::rejected(
            "working_directory_unsupported",
            "Worker execution backend does not support working directory materialization",
        ))
    }

    fn list_working_directories(&self) -> Vec<WorkingDirectoryStatus> {
        Vec::new()
    }

    fn working_directory(
        &self,
        working_directory_id: &str,
    ) -> Result<WorkingDirectoryStatus, WorkingDirectoryDiagnostic> {
        Err(WorkingDirectoryDiagnostic::rejected(
            "working_directory_unknown",
            format!("working directory `{working_directory_id}` is not known to this Runtime"),
        ))
    }

    fn cleanup_working_directory(
        &self,
        working_directory_id: &str,
    ) -> Result<WorkingDirectoryStatus, WorkingDirectoryDiagnostic> {
        Err(WorkingDirectoryDiagnostic::rejected(
            "working_directory_cleanup_unsupported",
            format!(
                "working directory `{working_directory_id}` cannot be cleaned up by this backend"
            ),
        ))
    }

    fn dispatch_input(
        &self,
        handle: &WorkerExecutionHandle,
        input: WorkerInput,
    ) -> WorkerExecutionResult;

    fn dispatch_method(
        &self,
        _handle: &WorkerExecutionHandle,
        _method: Method,
    ) -> WorkerExecutionResult {
        WorkerExecutionResult::unsupported(
            WorkerExecutionOperation::ProtocolMethod,
            "execution backend does not support direct Worker protocol methods",
        )
    }

    fn worker_completions(
        &self,
        _handle: &WorkerExecutionHandle,
        _kind: protocol::CompletionKind,
        _prefix: &str,
    ) -> Vec<protocol::CompletionEntry> {
        Vec::new()
    }

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

    #[cfg(feature = "ws-server")]
    fn worker_snapshot(&self, _handle: &WorkerExecutionHandle) -> Option<protocol::Event> {
        None
    }
}

#[derive(Clone)]
pub(crate) struct WorkerExecutionBackendRef {
    backend_id: String,
    backend: Arc<dyn WorkerExecutionBackend>,
}

impl WorkerExecutionBackendRef {
    pub(crate) fn new(backend: Arc<dyn WorkerExecutionBackend>) -> Result<Self, RuntimeError> {
        let backend_id = backend.backend_id().to_string();
        if backend_id.trim().is_empty() {
            return Err(RuntimeError::InvalidRequest(
                "execution backend id must not be empty".to_string(),
            ));
        }
        Ok(Self {
            backend_id,
            backend,
        })
    }

    pub(crate) fn spawn_worker(
        &self,
        request: WorkerExecutionSpawnRequest,
    ) -> WorkerExecutionSpawnResult {
        self.backend.spawn_worker(request)
    }

    pub(crate) fn restore_worker(
        &self,
        request: WorkerExecutionRestoreRequest,
    ) -> WorkerExecutionSpawnResult {
        self.backend.restore_worker(request)
    }

    pub(crate) fn create_working_directory(
        &self,
        request: &WorkingDirectoryRequest,
    ) -> Result<WorkingDirectoryStatus, WorkingDirectoryDiagnostic> {
        self.backend.create_working_directory(request)
    }

    pub(crate) fn list_working_directories(&self) -> Vec<WorkingDirectoryStatus> {
        self.backend.list_working_directories()
    }

    pub(crate) fn working_directory(
        &self,
        working_directory_id: &str,
    ) -> Result<WorkingDirectoryStatus, WorkingDirectoryDiagnostic> {
        self.backend.working_directory(working_directory_id)
    }

    pub(crate) fn cleanup_working_directory(
        &self,
        working_directory_id: &str,
    ) -> Result<WorkingDirectoryStatus, WorkingDirectoryDiagnostic> {
        self.backend.cleanup_working_directory(working_directory_id)
    }

    pub(crate) fn dispatch_input(
        &self,
        handle: &WorkerExecutionHandle,
        input: WorkerInput,
    ) -> WorkerExecutionResult {
        self.backend.dispatch_input(handle, input)
    }

    pub(crate) fn dispatch_method(
        &self,
        handle: &WorkerExecutionHandle,
        method: Method,
    ) -> WorkerExecutionResult {
        self.backend.dispatch_method(handle, method)
    }

    #[cfg(feature = "ws-server")]
    pub(crate) fn worker_snapshot(
        &self,
        handle: &WorkerExecutionHandle,
    ) -> Option<protocol::Event> {
        self.backend.worker_snapshot(handle)
    }

    pub(crate) fn worker_completions(
        &self,
        handle: &WorkerExecutionHandle,
        kind: protocol::CompletionKind,
        prefix: &str,
    ) -> Vec<protocol::CompletionEntry> {
        self.backend.worker_completions(handle, kind, prefix)
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
            .field("backend_id", &self.backend_id)
            .finish_non_exhaustive()
    }
}
