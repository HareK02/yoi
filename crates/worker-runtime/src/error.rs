use crate::execution::WorkerExecutionResult;
use crate::identity::WorkerId;
use crate::working_directory::WorkingDirectoryDiagnostic;
use std::path::PathBuf;

/// Errors returned by the embedded Runtime API.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("runtime is stopped")]
    RuntimeStopped,

    #[error("initial worker input must be user input, got {kind}")]
    InvalidInitialInputKind { kind: String },

    #[error("worker {worker_id} Restore observation is stale; request was not accepted")]
    RestoreObservationStale { worker_id: WorkerId },

    #[error("worker {worker_id} Restore observation conflicts with the current operation")]
    RestoreObservationConflict { worker_id: WorkerId },

    #[error(
        "worker {worker_id} lifecycle lock is busy; release caller gates and retry the same Restore intent"
    )]
    RestoreAdmissionBusy { worker_id: WorkerId },

    #[error("worker {worker_id} Stop observation conflicts with the current lifecycle")]
    StopObservationConflict { worker_id: WorkerId },

    #[error(
        "worker {worker_id} Stop request ID is already bound to different input or another pending stop"
    )]
    StopRequestConflict { worker_id: WorkerId },

    #[error(
        "worker {worker_id} lifecycle lock is busy; release caller gates and retry the same Stop intent"
    )]
    StopAdmissionBusy { worker_id: WorkerId },

    #[error(
        "worker {worker_id} attachment operation request ID is bound to different input or pending authority"
    )]
    WorkdirAttachmentsRequestConflict { worker_id: WorkerId },

    #[error("worker {worker_id} attachment generation is stale; request was not accepted")]
    WorkdirAttachmentsGenerationStale { worker_id: WorkerId },

    #[error("worker {worker_id} was not found")]
    WorkerNotFound { worker_id: WorkerId },

    #[error("worker {worker_id} execution is unavailable: {message}")]
    WorkerExecutionUnavailable {
        worker_id: WorkerId,
        message: String,
    },

    #[error("worker {worker_id} metadata deletion failed: {message}")]
    WorkerDeletePersistenceFailed {
        worker_id: WorkerId,
        message: String,
    },

    #[error("worker creation has no execution backend: {message}")]
    ExecutionBackendUnavailable { message: String },

    #[error("worker {worker_id} execution {operation:?} returned {outcome:?}: {message}")]
    WorkerExecutionRejected {
        worker_id: WorkerId,
        operation: crate::execution::WorkerExecutionOperation,
        outcome: crate::execution::WorkerExecutionOutcome,
        message: String,
        result: WorkerExecutionResult,
    },

    /// Creation failed after connection; the original error remains internal source
    /// evidence. Display deliberately never renders it or the startup attachment.
    #[error("worker creation failed")]
    WorkerCreateFailed {
        failure: Box<runtime_api::startup::WorkerCreateFailure>,
        #[source]
        source: Box<RuntimeError>,
    },

    #[error("limit {requested} exceeds maximum {max}")]
    LimitTooLarge { requested: usize, max: usize },

    #[error("worker {worker_id} was already created with different request identity input")]
    WorkerCreateConflict { worker_id: WorkerId },

    #[error("invalid request: {0}")]
    InvalidRequest(String),

    #[error(
        "workspace `{workspace_id}` is owned by Runtime server `{owner_server_id}`, not `{requester_server_id}`"
    )]
    WorkspaceOwnerMismatch {
        workspace_id: String,
        owner_server_id: String,
        requester_server_id: String,
    },

    #[error("recording source unavailable: {message}")]
    RecordingSourceUnavailable { message: String },

    #[error("recording ownership unknown: {message}")]
    RecordingOwnershipUnknown { message: String },

    #[error("recording execution unconfirmed: {message}")]
    RecordingExecutionUnconfirmed { message: String },

    #[error(transparent)]
    WorkingDirectory(#[from] WorkingDirectoryDiagnostic),

    #[error("config bundle `{bundle_id}` was not found")]
    ConfigBundleMissing { bundle_id: String },

    #[error(
        "config bundle `{bundle_id}` digest mismatch: expected {expected_digest}, got {actual_digest}"
    )]
    ConfigBundleDigestMismatch {
        bundle_id: String,
        expected_digest: String,
        actual_digest: String,
    },

    #[error("invalid profile selector `{profile}` for config bundle {bundle_id:?}: {message}")]
    InvalidProfileSelector {
        profile: String,
        bundle_id: Option<String>,
        message: String,
    },

    #[error(
        "config bundle `{bundle_id}` contains unsupported declaration `{declaration_kind}` named `{name}`"
    )]
    UnsupportedConfigDeclaration {
        bundle_id: String,
        declaration_kind: String,
        name: String,
    },

    #[error("Runtime store is already owned by another process")]
    RuntimeStoreAlreadyOpen { path: PathBuf },

    #[error("runtime store {operation} failed at {}: {source}", path.display())]
    StoreIo {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error(
        "runtime store {operation} may have committed at {} before durability confirmation failed: {source}",
        path.display()
    )]
    StoreCommitOutcomeUnknown {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("runtime store {operation} missing data at {}", path.display())]
    StoreMissing {
        operation: &'static str,
        path: PathBuf,
    },

    #[error("runtime store {operation} found corrupt data at {}: {message}", path.display())]
    StoreCorrupt {
        operation: &'static str,
        path: PathBuf,
        message: String,
    },

    #[error("runtime state lock was poisoned")]
    StatePoisoned,
}

impl RuntimeError {
    pub fn command_result(&self) -> Option<&protocol::WorkerCommandResult> {
        self.execution_result()
            .and_then(|result| result.command.as_ref())
    }

    /// Only an owning create boundary or a Spawn result can identify a creation
    /// failure. Independent Input errors retain their ordinary API contract.
    pub fn create_failure(&self) -> Option<runtime_api::startup::WorkerCreateFailure> {
        match self {
            Self::WorkerCreateFailed { failure, .. } => Some((**failure).clone()),
            Self::WorkerExecutionRejected { result, .. } => {
                crate::execution::startup_create_failure(result)
                    .ok()
                    .flatten()
            }
            _ => None,
        }
    }

    /// The original execution result, including primary facts and independent secondary
    /// failures. Display is presentation only and must not be parsed to recover causes.
    pub fn execution_result(&self) -> Option<&WorkerExecutionResult> {
        match self {
            Self::WorkerExecutionRejected { result, .. } => Some(result),
            Self::WorkerCreateFailed { source, .. } => source.execution_result(),
            _ => None,
        }
    }
}

impl RuntimeError {
    pub fn control_cause(&self) -> protocol::control_failure::RuntimeControlCause {
        use RuntimeError as E;
        use protocol::control_failure::RuntimeControlCause;
        let error = self;
        use RuntimeControlCause as C;
        match error {
            E::WorkerCreateFailed { source, .. } => source.control_cause(),
            E::WorkerNotFound { .. } => C::WorkerMissing,
            E::WorkerCreateConflict { .. } => C::CreateRequestConflict,
            E::RuntimeStopped => C::RuntimeStopped,
            E::InvalidInitialInputKind { .. } | E::LimitTooLarge { .. } | E::InvalidRequest(_) => {
                C::InvalidRequest
            }
            E::RestoreObservationStale { .. } => C::RestoreObservationStale,
            E::RestoreObservationConflict { .. } => C::RestoreObservationConflict,
            E::RestoreAdmissionBusy { .. } => C::RestoreAdmissionBusy,
            E::StopObservationConflict { .. } => C::StopObservationConflict,
            E::StopRequestConflict { .. } => C::StopRequestConflict,
            E::StopAdmissionBusy { .. } => C::StopAdmissionBusy,
            E::WorkerExecutionUnavailable { .. } | E::ExecutionBackendUnavailable { .. } => {
                C::ExecutionUnavailable
            }
            E::WorkerExecutionRejected { result, .. } => match result.command.as_ref() {
                Some(protocol::WorkerCommandResult::Unconfirmed { .. }) => C::ResultUnavailable,
                Some(protocol::WorkerCommandResult::NotDelivered { .. }) => C::ExecutionUnavailable,
                _ => C::ExecutionRejected,
            },
            E::WorkdirAttachmentsRequestConflict { .. }
            | E::WorkdirAttachmentsGenerationStale { .. } => C::ExecutionRejected,
            E::WorkerDeletePersistenceFailed { .. } | E::StoreIo { .. } => C::PersistenceFailed,
            E::StoreCommitOutcomeUnknown { .. } => C::PersistenceOutcomeUnknown,
            E::WorkspaceOwnerMismatch { .. } => C::WorkspaceBindingMismatch,
            E::WorkingDirectory(_) => C::WorkingDirectoryFailed,
            E::ConfigBundleMissing { .. }
            | E::ConfigBundleDigestMismatch { .. }
            | E::InvalidProfileSelector { .. }
            | E::UnsupportedConfigDeclaration { .. } => C::ConfigurationUnavailable,
            E::RuntimeStoreAlreadyOpen { .. } | E::StoreMissing { .. } | E::StoreCorrupt { .. } => {
                C::StoreUnavailable
            }
            E::RecordingSourceUnavailable { .. } => C::RecordingSourceUnavailable,
            E::RecordingOwnershipUnknown { .. } => C::RecordingOwnershipUnknown,
            E::RecordingExecutionUnconfirmed { .. } => C::RecordingExecutionUnconfirmed,
            E::StatePoisoned => C::StateUnavailable,
        }
    }
}
