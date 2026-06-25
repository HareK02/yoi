use crate::identity::RuntimeId;
use serde::{Deserialize, Serialize};

/// Runtime backend kind.  v0 intentionally ships only an embedded memory backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeBackendKind {
    Memory,
}

/// Runtime lifecycle state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeStatus {
    Running,
    Stopped,
}

/// Guardrails for bounded observation/projection APIs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeLimits {
    pub max_transcript_projection_items: usize,
    pub max_event_batch_items: usize,
}

impl Default for RuntimeLimits {
    fn default() -> Self {
        Self {
            max_transcript_projection_items: 256,
            max_event_batch_items: 256,
        }
    }
}

/// Options used to construct an embedded memory Runtime.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeOptions {
    pub runtime_id: Option<RuntimeId>,
    pub display_name: Option<String>,
    pub limits: RuntimeLimits,
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        Self {
            runtime_id: None,
            display_name: None,
            limits: RuntimeLimits::default(),
        }
    }
}

/// Management-plane summary for a Runtime.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeSummary {
    pub runtime_id: RuntimeId,
    pub display_name: Option<String>,
    pub backend: RuntimeBackendKind,
    pub status: RuntimeStatus,
    pub worker_count: usize,
    pub active_worker_count: usize,
    pub stopped_worker_count: usize,
    pub cancelled_worker_count: usize,
    pub diagnostic_count: usize,
    pub limits: RuntimeLimits,
}
