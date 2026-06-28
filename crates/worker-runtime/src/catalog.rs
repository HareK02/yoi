use crate::execution::WorkerExecutionStatus;
use crate::identity::{RuntimeId, WorkerId, WorkerRef};
use crate::interaction::WorkerInput;
use serde::{Deserialize, Serialize};

/// Profile selector boundary. This is a selector, not a resolved config bundle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ProfileSelector {
    RuntimeDefault,
    Builtin(String),
    Named(String),
}

impl Default for ProfileSelector {
    fn default() -> Self {
        Self::RuntimeDefault
    }
}

/// Backend-synced config bundle reference used during Worker creation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigBundleRef {
    pub id: String,
    pub digest: String,
}

/// Canonical Runtime Worker creation request.
///
/// Browser/product launch semantics are resolved by a backend before this
/// request is built. The request contains only durable Runtime identity inputs:
/// a backend-decided profile selector, a previously synced ConfigBundle identity,
/// and optional initial user input that is committed in the same transaction as
/// Worker catalog/transcript persistence. It carries no cwd/workspace path, tool
/// scope, credential, socket/session path, raw config body, or execution binding
/// internals.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateWorkerRequest {
    pub profile: ProfileSelector,
    pub config_bundle: ConfigBundleRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_input: Option<WorkerInput>,
}

/// Worker lifecycle status for the in-memory embedded runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerStatus {
    Running,
    Stopped,
    Cancelled,
}

impl WorkerStatus {
    pub fn is_active(self) -> bool {
        matches!(self, Self::Running)
    }
}

/// Lightweight catalog row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerSummary {
    pub worker_ref: WorkerRef,
    pub runtime_id: RuntimeId,
    pub worker_id: WorkerId,
    pub status: WorkerStatus,
    pub execution: WorkerExecutionStatus,
    pub profile: ProfileSelector,
    pub config_bundle: ConfigBundleRef,
    pub transcript_len: usize,
    pub last_event_id: u64,
}

/// Full Worker catalog/lifecycle detail.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerDetail {
    pub worker_ref: WorkerRef,
    pub runtime_id: RuntimeId,
    pub worker_id: WorkerId,
    pub status: WorkerStatus,
    pub execution: WorkerExecutionStatus,
    pub profile: ProfileSelector,
    pub config_bundle: ConfigBundleRef,
    pub transcript_len: usize,
    pub last_event_id: u64,
}

/// Acknowledgement returned by stop/cancel lifecycle operations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerLifecycleAck {
    pub worker_ref: WorkerRef,
    pub status: WorkerStatus,
    pub event_id: u64,
}
