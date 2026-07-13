use crate::identity::WorkerId;
use serde::{Deserialize, Serialize};

/// Runtime backend kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeBackendKind {
    Memory,
    #[cfg(feature = "fs-store")]
    FsStore,
}

/// Runtime lifecycle state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeStatus {
    Running,
    Stopped,
}

/// Guardrails for bounded Runtime APIs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeLimits {
    pub max_event_batch_items: usize,
}

impl Default for RuntimeLimits {
    fn default() -> Self {
        Self {
            max_event_batch_items: 256,
        }
    }
}

/// Options used to construct an embedded memory Runtime.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeOptions {
    pub display_name: Option<String>,
    pub limits: RuntimeLimits,
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        Self {
            display_name: None,
            limits: RuntimeLimits::default(),
        }
    }
}

fn unknown_platform_component() -> String {
    "unknown".to_string()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerDeleteResult {
    pub worker_id: WorkerId,
    pub deleted: bool,
}

/// Management-plane summary for a Runtime.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeSummary {
    pub display_name: Option<String>,
    pub backend: RuntimeBackendKind,
    pub status: RuntimeStatus,
    pub worker_count: usize,
    pub active_worker_count: usize,
    pub stopped_worker_count: usize,
    pub cancelled_worker_count: usize,
    pub diagnostic_count: usize,
    pub limits: RuntimeLimits,
    #[serde(default = "unknown_platform_component")]
    pub os: String,
    #[serde(default = "unknown_platform_component")]
    pub arch: String,
    #[serde(default)]
    pub worker_creation_available: bool,
}
