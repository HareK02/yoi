use crate::execution::WorkerExecutionResult;
use crate::identity::{RuntimeId, WorkerId};
use std::path::PathBuf;

/// Errors returned by the embedded Runtime API.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("runtime {runtime_id} is stopped")]
    RuntimeStopped { runtime_id: RuntimeId },

    #[error(
        "worker {worker_id} belongs to runtime {actual_runtime_id}, not runtime {expected_runtime_id}"
    )]
    WrongRuntime {
        expected_runtime_id: RuntimeId,
        actual_runtime_id: RuntimeId,
        worker_id: WorkerId,
    },

    #[error("cursor belongs to runtime {actual_runtime_id}, not runtime {expected_runtime_id}")]
    WrongRuntimeCursor {
        expected_runtime_id: RuntimeId,
        actual_runtime_id: RuntimeId,
    },

    #[error("worker {worker_id} was not found in runtime {runtime_id}")]
    WorkerNotFound {
        runtime_id: RuntimeId,
        worker_id: WorkerId,
    },

    #[error("worker {worker_id} has no execution backend: {message}")]
    WorkerExecutionUnavailable {
        worker_id: WorkerId,
        message: String,
    },

    #[error("worker {worker_id} execution {operation:?} returned {outcome:?}: {message}")]
    WorkerExecutionRejected {
        worker_id: WorkerId,
        operation: crate::execution::WorkerExecutionOperation,
        outcome: crate::execution::WorkerExecutionOutcome,
        message: String,
        result: WorkerExecutionResult,
    },

    #[error("limit {requested} exceeds maximum {max}")]
    LimitTooLarge { requested: usize, max: usize },

    #[error("invalid request: {0}")]
    InvalidRequest(String),

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

    #[error("runtime store {operation} failed at {}: {source}", path.display())]
    StoreIo {
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
