use crate::identity::{RuntimeId, WorkerId};

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

    #[error("limit {requested} exceeds maximum {max}")]
    LimitTooLarge { requested: usize, max: usize },

    #[error("invalid request: {0}")]
    InvalidRequest(String),

    #[error("runtime state lock was poisoned")]
    StatePoisoned,
}
