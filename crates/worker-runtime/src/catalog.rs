use crate::identity::{RuntimeId, WorkerId, WorkerRef};
use serde::{Deserialize, Serialize};

/// Intent supplied when a Worker is created.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkerIntent {
    Assistant {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        purpose: Option<String>,
    },
    Task {
        objective: String,
    },
    Role {
        role: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        purpose: Option<String>,
    },
}

impl Default for WorkerIntent {
    fn default() -> Self {
        Self::Assistant { purpose: None }
    }
}

/// Profile selector boundary.  This is a selector, not a resolved config bundle.
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

/// Placeholder for future config-bundle synchronization.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigBundleRef {
    pub id: String,
}

/// Requested capability name plus optional human-readable reason.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityRequest {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl CapabilityRequest {
    pub fn named(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            reason: None,
        }
    }
}

/// Opaque workspace reference supplied by a caller.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceRef {
    pub name: String,
    pub reference: String,
}

/// Opaque mount reference supplied by a caller.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MountRef {
    pub name: String,
    pub reference: String,
}

/// Worker creation request for the catalog/lifecycle API.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateWorkerRequest {
    pub intent: WorkerIntent,
    pub profile: ProfileSelector,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_bundle: Option<ConfigBundleRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requested_capabilities: Vec<CapabilityRequest>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspace_refs: Vec<WorkspaceRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mount_refs: Vec<MountRef>,
}

impl Default for CreateWorkerRequest {
    fn default() -> Self {
        Self {
            intent: WorkerIntent::default(),
            profile: ProfileSelector::default(),
            config_bundle: None,
            requested_capabilities: Vec::new(),
            workspace_refs: Vec::new(),
            mount_refs: Vec::new(),
        }
    }
}

impl CreateWorkerRequest {
    /// Create a tools-less Worker using runtime-local default resources.
    pub fn tools_less(intent: WorkerIntent, profile: ProfileSelector) -> Self {
        Self {
            intent,
            profile,
            config_bundle: None,
            requested_capabilities: Vec::new(),
            workspace_refs: Vec::new(),
            mount_refs: Vec::new(),
        }
    }
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
    pub intent: WorkerIntent,
    pub profile: ProfileSelector,
    pub requested_capability_count: usize,
    pub has_config_bundle: bool,
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
    pub intent: WorkerIntent,
    pub profile: ProfileSelector,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_bundle: Option<ConfigBundleRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requested_capabilities: Vec<CapabilityRequest>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspace_refs: Vec<WorkspaceRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mount_refs: Vec<MountRef>,
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
