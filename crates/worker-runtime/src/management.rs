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

/// Options used to construct an embedded memory Runtime.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeOptions {
    pub display_name: Option<String>,
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
    #[serde(default = "unknown_platform_component")]
    pub os: String,
    #[serde(default = "unknown_platform_component")]
    pub arch: String,
    #[serde(default)]
    pub worker_creation_available: bool,
}
